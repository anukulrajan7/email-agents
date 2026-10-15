use super::*;

#[derive(Debug)]
pub enum SendFailure {
    /// Nothing went out; the attempt was released. `retry` = worth trying again later.
    NotSent { error: String, retry: bool },
    /// Gmail's answer was ambiguous; the attempt stays open until reconciled.
    Unknown(String),
    /// Someone else holds the send (another attempt, undone, already sent).
    Skipped,
}

/// Sends now, in the original thread, under a fresh attempt id. Callers have run the safety check.
pub async fn send_now(state: &AppState, ws: &str, ctx: &ReplyContext, actor: Actor<'_>) -> Result<String, SendFailure> {
    let not_sent = |e: AppError, retry| SendFailure::NotSent { error: e.to_string(), retry };
    let (conn, pck) = connection(ctx).map_err(|e| not_sent(e, false))?;
    let attempt = new_id("snd");
    let claimed = sqlx::query(
        "UPDATE drafts SET send_attempt_id = ?1, send_started_at = ?2
         WHERE id = ?3 AND workspace_id = ?4 AND status != 'sent' AND send_attempt_id IS NULL AND send_at IS ?5",
    )
    .bind(&attempt)
    .bind(now_ms())
    .bind(&ctx.draft_id)
    .bind(ws)
    .bind(ctx.send_at)
    .execute(&state.db)
    .await
    .map_err(|e| not_sent(e.into(), true))?
    .rows_affected()
        == 1;
    if !claimed {
        return Err(SendFailure::Skipped);
    }
    let attempt_ref = attempt.as_str();
    let release = |error: AppError, retry: bool| async move {
        if let Err(e) = clear_attempt(state, &ctx.draft_id, attempt_ref).await {
            tracing::error!(error = %e, "could not release a send attempt");
        }
        SendFailure::NotSent { error: error.to_string(), retry }
    };
    let mid = attempt_message_id(&attempt);
    // Everything before the final send call can't have sent anything.
    let message = match raw_for(state, ctx, conn, pck, Some(&mid)).await {
        Ok(m) => m,
        Err(e) => {
            let retry = matches!(e, AppError::Unavailable);
            return Err(release(e, retry).await);
        }
    };
    let (path, body) = match &ctx.provider_draft_id {
        Some(gid) => {
            if let Err(e) =
                state.nango.gmail_call(Method::PUT, conn, pck, &format!("drafts/{gid}"), Some(&json!({ "message": message }))).await
            {
                let retry = matches!(e, NangoError::Unavailable(_));
                return Err(release(e.into(), retry).await);
            }
            ("drafts/send", json!({ "id": gid }))
        }
        None => ("messages/send", message),
    };
    match state.nango.gmail_call(Method::POST, conn, pck, path, Some(&body)).await {
        Ok(sent) => {
            let provider_message_id = sent.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
            if let Err(e) = finalize_sent(state, ws, ctx, &provider_message_id, actor).await {
                // Sent, but not recorded: leave the attempt open so the next run reconciles it as sent.
                tracing::error!(error = %e, "reply sent but recording it failed");
                return Err(SendFailure::Unknown(format!("sent, but recording failed: {e}")));
            }
            Ok(provider_message_id)
        }
        // Timeout / 5xx / 429 / bad response body: Gmail may have sent it.
        Err(NangoError::Unavailable(m)) => Err(SendFailure::Unknown(m)),
        // A clear rejection (4xx) before anything went out.
        Err(e) => Err(release(e.into(), false).await),
    }
}

// ---------------------------------------------------------------- schedule / undo

fn requested_by<'a>(actor: Actor<'a>) -> Option<&'a str> {
    match actor {
        Actor::User(u) => Some(u),
        Actor::System => None,
    }
}

/// Checks the reply and schedules it `SEND_UNDO_SECONDS` ahead. Every send path (person,
/// automation, agent approval) goes through here, so these guards cannot be bypassed.
pub async fn send_draft(state: &AppState, ws: &str, draft_id: &str, actor: Actor<'_>) -> AppResult<ScheduledSend> {
    send_draft_held(state, ws, draft_id, actor, 0).await
}

/// Like `send_draft`, with the send held at least `hold_secs` from now. The hold is part of the same
/// claim, so the reply is never scheduled earlier than the hold.
async fn record_send_blocked(state: &AppState, ws: &str, ctx: &ReplyContext, report: &send_safety::SafetyReport, automated: bool) {
    let agent_id: Option<String> = if automated {
        sqlx::query_scalar("SELECT agent_id FROM drafts WHERE id = ?1 AND workspace_id = ?2")
            .bind(&ctx.draft_id)
            .bind(ws)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten()
            .flatten()
    } else {
        None
    };
    let codes: Vec<&str> = report.blocking.iter().map(|i| i.code).collect();
    let severity = crate::ai::incidents::send_block_severity(&codes);
    crate::ai::incidents::record(
        &state.db,
        crate::ai::incidents::NewIncident::new(ws, crate::ai::incidents::Kind::SendBlocked, serde_json::json!({"codes": codes, "automated": automated}))
            .severity(severity)
            .agent(agent_id.as_deref())
            .message(Some(&ctx.message_id))
            .dedupe(format!("blocked:{}:{}", ctx.draft_id, codes.join(","))),
    )
    .await;
}

pub async fn send_draft_held(state: &AppState, ws: &str, draft_id: &str, actor: Actor<'_>, hold_secs: i64) -> AppResult<ScheduledSend> {
    let mut ctx = load(state, ws, draft_id).await?;
    if ctx.status == "sent" {
        return Err(AppError::Conflict("this reply was already sent".into()));
    }
    if ctx.send_at.is_some() {
        return Err(AppError::Conflict("this reply is already scheduled to send".into()));
    }
    // Text that can never go out is refused first, whatever the source.
    if let Some(gaps) = send_safety::check_gaps(&ctx.body_text) {
        return Err(send_safety::SafetyReport { blocking: vec![gaps], warnings: vec![] }.into_error());
    }
    if !policy::can_send(&capabilities(&ctx)) {
        return Err(AppError::Conflict("this source can't send; copy the draft into your mail client".into()));
    }
    connection(&ctx)?;
    if let Some(attempt) = ctx.send_attempt_id.clone() {
        match reconcile(state, &ctx).await {
            Reconciled::Sent(id) => {
                finalize_sent(state, ws, &ctx, &id, actor).await?;
                return Err(AppError::Conflict("this reply was already sent".into()));
            }
            Reconciled::NotSent => {
                clear_attempt(state, &ctx.draft_id, &attempt).await?;
                ctx = load(state, ws, draft_id).await?;
            }
            Reconciled::Unsure(why) => {
                tracing::warn!(draft_id, %why, "previous send attempt not reconciled yet");
                return Err(AppError::Conflict(
                    "we're still confirming whether the previous send went out; check Gmail's Sent folder or try again in a few minutes".into(),
                ));
            }
        }
    }
    let automated = matches!(actor, Actor::System);
    let mut report = send_safety::check(state, &CheckInput { actor_user: requested_by(actor), workspace_id: ws, reply: &ctx, body: &ctx.body_text, automated }).await?;
    // An automatic send gets one repair of an unsupported claim; a person's send never needs it (warning only).
    if report.is_blocked() && automated && crate::replies::repair::apply(state, ws, &ctx, &report).await? {
        ctx = load(state, ws, draft_id).await?;
        report = send_safety::check(state, &CheckInput { actor_user: requested_by(actor), workspace_id: ws, reply: &ctx, body: &ctx.body_text, automated }).await?;
    }
    if report.is_blocked() {
        if automated {
            crate::replies::repair::hand_off(state, ws, &ctx, &report).await?;
        }
        record_send_blocked(state, ws, &ctx, &report, automated).await;
        return Err(report.into_error());
    }
    let now = now_ms();
    let send_at = now + state.config.send_undo_seconds.max(hold_secs).max(0) * 1_000;
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let claimed = sqlx::query(
        "UPDATE drafts SET send_at = ?1, send_requested_by = ?2, send_error = NULL, updated_at = ?3
         WHERE id = ?4 AND workspace_id = ?5 AND status != 'sent' AND send_at IS NULL AND send_attempt_id IS NULL",
    )
    .bind(send_at)
    .bind(requested_by(actor))
    .bind(now)
    .bind(&ctx.draft_id)
    .bind(ws)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if !claimed {
        return Err(AppError::Conflict("this reply is already being sent".into()));
    }
    jobs::enqueue_at(
        &mut tx,
        kind::SEND_DRAFT,
        &format!("send_draft:{}:{send_at}", ctx.draft_id),
        Some(ws),
        &json!({ "workspace_id": ws, "draft_id": ctx.draft_id, "send_at": send_at }),
        jobs::PROVIDER_MAX_ATTEMPTS,
        send_at,
    )
    .await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: ws,
            actor,
            action: "draft.send_scheduled",
            entity_type: "draft",
            entity_id: &ctx.draft_id,
            message_id: Some(&ctx.message_id),
            metadata: json!({ "send_at": send_at, "warnings": report.warnings.iter().map(|w| w.code).collect::<Vec<_>>() }),
        },
    )
    .await?;
    tx.commit().await?;
    state.jobs_notify.notify_one();
    Ok(ScheduledSend { status: "scheduled", draft_id: ctx.draft_id, send_at: iso(send_at), send_at_ms: send_at })
}

/// Checks and schedules the reply; it goes out after the undo window.
pub async fn send(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
    Json(body): Json<SendBody>,
) -> AppResult<Json<ScheduledSend>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    crate::hr::team::scope::require_draft(&state.db, &ws, &user, &id).await?;
    if !body.confirm {
        return Err(AppError::BadRequest("sending needs explicit confirmation".into()));
    }
    Ok(Json(send_draft(&state, &ws, &id, Actor::User(&user.id)).await?))
}

/// Cancels a scheduled send. 409 once Gmail has been called (sending or sent).
pub async fn undo_send(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    crate::hr::team::scope::require_draft(&state.db, &ws, &user, &id).await?;
    // Read before the update clears it: an agent's own send (nobody asked for it) that a person stops is a learning signal.
    let sender: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT message_id, agent_id, send_requested_by FROM drafts WHERE id = ?1 AND workspace_id = ?2 AND send_at IS NOT NULL",
    )
    .bind(&id)
    .bind(&ws)
    .fetch_optional(&state.db)
    .await?;
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let undone = sqlx::query(
        "UPDATE drafts SET send_at = NULL, send_requested_by = NULL, updated_at = ?1
         WHERE id = ?2 AND workspace_id = ?3 AND status != 'sent' AND send_at IS NOT NULL AND send_attempt_id IS NULL",
    )
    .bind(now_ms())
    .bind(&id)
    .bind(&ws)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if !undone {
        let row: Option<(String, Option<i64>, Option<String>)> =
            sqlx::query_as("SELECT status, send_at, send_attempt_id FROM drafts WHERE id = ?1 AND workspace_id = ?2")
                .bind(&id)
                .bind(&ws)
                .fetch_optional(&mut *tx)
                .await?;
        return Err(match row {
            None => AppError::NotFound,
            Some((s, _, _)) if s == "sent" => AppError::Conflict("too late: this reply was already sent".into()),
            Some((_, _, Some(_))) => AppError::Conflict("too late: this reply is being sent".into()),
            Some(_) => AppError::Conflict("this reply isn't scheduled to send".into()),
        });
    }
    let message_id: String = sqlx::query_scalar("SELECT message_id FROM drafts WHERE id = ?1").bind(&id).fetch_one(&mut *tx).await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &ws,
            actor: Actor::User(&user.id),
            action: "draft.send_undone",
            entity_type: "draft",
            entity_id: &id,
            message_id: Some(&message_id),
            metadata: json!({}),
        },
    )
    .await?;
    if let Some((_, Some(agent_id), None)) = &sender {
        crate::learning::hooks::auto_send_stopped(&mut tx, &ws, &user.id, &id, &message_id, Some(agent_id)).await?;
    }
    tx.commit().await?;
    Ok(Json(json!({ "status": "cancelled", "draft_id": id })))
}
