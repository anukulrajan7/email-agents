use super::*;

fn job_err(e: AppError) -> JobError {
    match e {
        AppError::Unavailable | AppError::AiUnavailable | AppError::Internal(_) => JobError::Retry(e.to_string()),
        other => JobError::Permanent(other.to_string()),
    }
}

/// The scheduled send didn't go out: clear the schedule, say why on the draft, audit, and alert the team.
async fn hold(state: &AppState, ws: &str, ctx: &ReplyContext, action: &str, reason: &str, metadata: Value) -> Result<(), JobError> {
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query(
        "UPDATE drafts SET send_at = NULL, send_requested_by = NULL, send_error = ?1, updated_at = ?2
         WHERE id = ?3 AND status != 'sent'",
    )
    .bind(crate::mail::sources::normalize::truncate_chars(reason, 500))
    .bind(now_ms())
    .bind(&ctx.draft_id)
    .execute(&mut *tx)
    .await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: ws,
            actor: Actor::System,
            action,
            entity_type: "draft",
            entity_id: &ctx.draft_id,
            message_id: Some(&ctx.message_id),
            metadata,
        },
    )
    .await?;
    tx.commit().await?;
    let alert = crate::alerts::NewAlert {
        workspace_id: ws,
        kind: crate::alerts::AUTOMATION,
        severity: "high",
        title: format!("Reply not sent: {}", ctx.subject),
        body: reason.to_string(),
        message_id: Some(&ctx.message_id),
        level: 1,
        dedupe_key: Some(format!("send_held:{}:{}", ctx.draft_id, ctx.send_at.unwrap_or_default())),
    };
    if let Err(e) = crate::alerts::raise(&state.db, alert).await {
        tracing::warn!(error = %e, "could not raise the held-send alert");
    }
    Ok(())
}

/// `SEND_DRAFT`: at `send_at`, reconcile any open attempt, re-run the safety check, then send.
pub async fn send_scheduled(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let field = |k: &str| payload.get(k).and_then(Value::as_str).map(str::to_string);
    let (Some(ws), Some(draft_id), Some(send_at)) =
        (field("workspace_id"), field("draft_id"), payload.get("send_at").and_then(Value::as_i64))
    else {
        return Err(JobError::Permanent("payload missing workspace_id/draft_id/send_at".into()));
    };
    let mut ctx = match load(state, &ws, &draft_id).await {
        Ok(c) => c,
        Err(AppError::NotFound) => return Ok(()),
        Err(e) => return Err(job_err(e)),
    };
    // Undone, rescheduled, or already sent: this job is stale.
    if ctx.status == "sent" || ctx.send_at != Some(send_at) {
        return Ok(());
    }
    let requester = ctx.send_requested_by.clone();
    let actor = match requester.as_deref() {
        Some(u) => Actor::User(u),
        None => Actor::System,
    };
    if let Some(attempt) = ctx.send_attempt_id.clone() {
        match reconcile(state, &ctx).await {
            Reconciled::Sent(id) => return finalize_sent(state, &ws, &ctx, &id, actor).await.map_err(job_err),
            Reconciled::NotSent => {
                clear_attempt(state, &draft_id, &attempt).await?;
                ctx = load(state, &ws, &draft_id).await.map_err(job_err)?;
            }
            Reconciled::Unsure(why) => return Err(JobError::Retry(format!("confirming the previous send attempt: {why}"))),
        }
    }
    if requester.is_none()
        && let Some(reason) = crate::hr::candidate_watch::stop_reason(state, &ws, &ctx.draft_id).await.map_err(|e| job_err(e.into()))?
    {
        tracing::info!(draft_id = %ctx.draft_id, reason, "a watch email is cancelled");
        crate::hr::candidate_watch::discard(state, &ws, &ctx.draft_id).await.map_err(|e| job_err(e.into()))?;
        return Ok(());
    }
    if requester.is_none()
        && let Some(reason) = crate::agents::autonomy::automated_send_stopped(state, &ws, &ctx.draft_id).await.map_err(|e| job_err(e.into()))?
    {
        sqlx::query("UPDATE drafts SET send_at = NULL, updated_at = ?1 WHERE id = ?2 AND workspace_id = ?3 AND send_attempt_id IS NULL")
            .bind(now_ms())
            .bind(&ctx.draft_id)
            .bind(&ws)
            .execute(&state.db)
            .await
            .map_err(|e| job_err(e.into()))?;
        tracing::info!(draft_id = %ctx.draft_id, reason, "held automatic send cancelled; it needs a person");
        return Ok(());
    }
    let input = CheckInput { actor_user: requester.as_deref(), workspace_id: &ws, reply: &ctx, body: &ctx.body_text, automated: requester.is_none() };
    let report = send_safety::check(state, &input).await.map_err(job_err)?;
    if report.is_blocked() {
        let reasons: Vec<&str> = report.blocking.iter().map(|i| i.message.as_str()).collect();
        let codes: Vec<&str> = report.blocking.iter().map(|i| i.code).collect();
        let reason = format!("Not sent: {}", reasons.join(" "));
        return hold(state, &ws, &ctx, "draft.send_blocked", &reason, json!({ "codes": codes })).await;
    }
    match send_now(state, &ws, &ctx, actor).await {
        Ok(_) | Err(SendFailure::Skipped) => Ok(()),
        Err(SendFailure::NotSent { error, retry: true }) => Err(JobError::Retry(error)),
        Err(SendFailure::NotSent { error, retry: false }) => Err(JobError::Permanent(error)),
        Err(SendFailure::Unknown(e)) => Err(JobError::Retry(format!("Gmail's answer was unclear; will confirm before retrying: {e}"))),
    }
}

/// Dead-lettered `SEND_DRAFT`: stop retrying, keep any open attempt (the next send reconciles it).
pub async fn give_up_scheduled(state: &AppState, payload: &Value, error: &str) -> Result<(), JobError> {
    let field = |k: &str| payload.get(k).and_then(Value::as_str).map(str::to_string);
    let (Some(ws), Some(draft_id)) = (field("workspace_id"), field("draft_id")) else { return Ok(()) };
    let ctx = match load(state, &ws, &draft_id).await {
        Ok(c) => c,
        Err(AppError::NotFound) => return Ok(()),
        Err(e) => return Err(job_err(e)),
    };
    if ctx.status == "sent" || ctx.send_at != payload.get("send_at").and_then(Value::as_i64) {
        return Ok(());
    }
    let reason = if ctx.send_attempt_id.is_some() {
        "We couldn't confirm whether this reply went out. Check Gmail's Sent folder before sending again.".to_string()
    } else {
        format!("Not sent: {error}")
    };
    hold(state, &ws, &ctx, "draft.send_failed", &reason, json!({ "unconfirmed": ctx.send_attempt_id.is_some() })).await
}
