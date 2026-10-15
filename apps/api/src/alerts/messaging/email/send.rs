use super::*;

/// RFC 5322 message for Gmail's `raw` field. A new conversation: no "Re:" is added.
pub fn build_raw_new(from: &str, to: &str, subject: &str, in_reply_to: Option<&str>, message_id: &str, body: &str) -> AppResult<String> {
    let bad = |e: String| AppError::BadRequest(e);
    let from: LettreMailbox = header_safe(from).parse().map_err(|_| bad("connected mailbox address is invalid".into()))?;
    let to: LettreMailbox = header_safe(to).parse().map_err(|_| bad("the recipient's address is invalid".into()))?;
    let mut builder = Message::builder().from(from).to(to).subject(truncate_chars(&header_safe(subject), MAX_SUBJECT_CHARS));
    if let Some(id) = in_reply_to.and_then(crate::replies::safe_message_id) {
        builder = builder.in_reply_to(id.clone()).references(id);
    }
    if let Some(id) = crate::replies::safe_message_id(message_id) {
        builder = builder.message_id(Some(id));
    }
    let msg = builder
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_string())
        .map_err(|e| AppError::Internal(anyhow::anyhow!("building email: {e}")))?;
    Ok(URL_SAFE_NO_PAD.encode(msg.formatted()))
}

enum Outcome {
    Sent { message_id: String, thread_id: Option<String> },
    NotSent,
    Unsure(String),
}

async fn reconcile(state: &AppState, row: &Outbound, mailbox: &Mailbox) -> Outcome {
    let Some(attempt) = row.send_attempt_id.as_deref() else { return Outcome::NotSent };
    let mid = attempt_message_id(attempt);
    let query = format!("rfc822msgid:{} in:anywhere", mid.trim_matches(['<', '>']));
    match state.nango.gmail_list_ids(&mailbox.connection_id, &mailbox.config_key, &query, None).await {
        Ok((ids, _)) => {
            if let Some(id) = ids.into_iter().next() {
                return Outcome::Sent { message_id: id, thread_id: None };
            }
        }
        Err(e) => return Outcome::Unsure(format!("Gmail search failed: {e}")),
    }
    if now_ms() - row.send_started_at.unwrap_or(0) < RECONCILE_GRACE_MS {
        return Outcome::Unsure("the last attempt is too recent to be sure".into());
    }
    Outcome::NotSent
}

fn job_err(e: AppError) -> JobError {
    match e {
        AppError::Unavailable | AppError::AiUnavailable | AppError::Internal(_) => JobError::Retry(e.to_string()),
        other => JobError::Permanent(other.to_string()),
    }
}

/// An automatic send is checked again when it comes due. Why it must wait for a person instead, if it must.
async fn auto_stopped(state: &AppState, row: &Outbound) -> sqlx::Result<Option<&'static str>> {
    let Some(owner) = row.owner_user_id.as_deref() else { return Ok(Some("there is no mailbox owner to answer for it")) };
    if crate::alerts::messaging::rules::personal_level(state, &row.workspace_id, owner).await? < 2 {
        return Ok(Some("the assistant no longer has permission to send on its own"));
    }
    let person = recipients::classify(&state.db, &row.workspace_id, &row.to_address).await?;
    if !person.kind.is_internal() {
        return Ok(Some("the recipient is not inside your workspace"));
    }
    Ok(None)
}

async fn is_personal_agent(state: &AppState, agent_id: Option<&str>) -> sqlx::Result<bool> {
    let Some(id) = agent_id else { return Ok(false) };
    Ok(sqlx::query_scalar::<_, String>("SELECT scope FROM agents WHERE id = ?1").bind(id).fetch_optional(&state.db).await?.as_deref() == Some("personal"))
}

/// `SEND_OUTBOUND`: at `send_at`, send the email unless it was stopped.
pub async fn run_scheduled(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let field = |k: &str| payload.get(k).and_then(Value::as_str).map(str::to_string);
    let (Some(ws), Some(id), Some(send_at)) = (field("workspace_id"), field("outbound_id"), payload.get("send_at").and_then(Value::as_i64)) else {
        return Err(JobError::Permanent("payload missing workspace_id/outbound_id/send_at".into()));
    };
    let Some(mut row) = get(state, &ws, &id).await? else { return Ok(()) };
    if row.status != "scheduled" || row.send_at != Some(send_at) {
        return Ok(());
    }
    if row.owner_user_id.is_none() && is_personal_agent(state, row.agent_id.as_deref()).await? {
        return give_up(state, &row, "Not sent: the person this assistant works for is gone, and a personal assistant never uses the shared mailbox.").await;
    }
    let mailbox = mailbox_by_source(state, &ws, &row.source_id).await.map_err(job_err)?;
    if let Some(attempt) = row.send_attempt_id.clone() {
        match reconcile(state, &row, &mailbox).await {
            Outcome::Sent { message_id, thread_id } => return finalize_sent(state, &row, &message_id, thread_id.as_deref()).await.map_err(job_err),
            Outcome::NotSent => {
                sqlx::query("UPDATE outbound_emails SET send_attempt_id = NULL, send_started_at = NULL WHERE id = ?1 AND send_attempt_id = ?2")
                    .bind(&id)
                    .bind(&attempt)
                    .execute(&state.db)
                    .await?;
                row = get(state, &ws, &id).await?.ok_or_else(|| JobError::Permanent("email vanished".into()))?;
            }
            Outcome::Unsure(why) => return Err(JobError::Retry(format!("confirming the previous send attempt: {why}"))),
        }
    }
    if row.send_requested_by.is_none()
        && let Some(reason) = auto_stopped(state, &row).await?
    {
        sqlx::query(
            "UPDATE outbound_emails SET status = 'awaiting_approval', send_at = NULL, auto_sent = 0, approval_reason = ?1, updated_at = ?2
             WHERE id = ?3 AND status = 'scheduled' AND send_attempt_id IS NULL",
        )
        .bind(format!("Held for you: {reason}."))
        .bind(now_ms())
        .bind(&id)
        .execute(&state.db)
        .await?;
        tracing::info!(outbound_id = %id, reason, "held automatic email needs a person");
        return Ok(());
    }
    let own = own_addresses(state, &row).await?;
    let report = check(&row.to_address, &row.subject, &row.body_text, &own);
    if report.is_blocked() {
        let reason = format!("Not sent: {}", report.blocking.iter().map(|i| i.message.as_str()).collect::<Vec<_>>().join(" "));
        return give_up(state, &row, &reason).await;
    }
    let attempt = new_id("snd");
    let claimed = sqlx::query(
        "UPDATE outbound_emails SET send_attempt_id = ?1, send_started_at = ?2
         WHERE id = ?3 AND workspace_id = ?4 AND status = 'scheduled' AND send_attempt_id IS NULL AND send_at IS ?5",
    )
    .bind(&attempt)
    .bind(now_ms())
    .bind(&id)
    .bind(&ws)
    .bind(send_at)
    .execute(&state.db)
    .await?
    .rows_affected()
        == 1;
    if !claimed {
        return Ok(());
    }
    let release = |state: AppState, id: String, attempt: String| async move {
        let _ = sqlx::query("UPDATE outbound_emails SET send_attempt_id = NULL, send_started_at = NULL WHERE id = ?1 AND send_attempt_id = ?2")
            .bind(id)
            .bind(attempt)
            .execute(&state.db)
            .await;
    };
    let from = match &mailbox.address {
        Some(a) => a.clone(),
        None => match crate::replies::mailbox_address(state, &mailbox.connection_id, &mailbox.config_key).await {
            Ok(a) => a,
            Err(e) => {
                release(state.clone(), id.clone(), attempt).await;
                return Err(job_err(e));
            }
        },
    };
    let mid = attempt_message_id(&attempt);
    let raw = match build_raw_new(&from, &row.to_address, &row.subject, row.in_reply_to.as_deref(), &mid, &row.body_text) {
        Ok(r) => r,
        Err(e) => {
            release(state.clone(), id.clone(), attempt).await;
            return Err(job_err(e));
        }
    };
    let mut message = json!({ "raw": raw });
    if let Some(t) = row.provider_thread_id.as_deref().filter(|t| !t.is_empty()) {
        message["threadId"] = json!(t);
    }
    sqlx::query("UPDATE outbound_emails SET rfc_message_id = ?1 WHERE id = ?2").bind(&mid).bind(&id).execute(&state.db).await?;
    match state.nango.gmail_call(Method::POST, &mailbox.connection_id, &mailbox.config_key, "messages/send", Some(&message)).await {
        Ok(sent) => {
            let message_id = sent.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
            let thread_id = sent.get("threadId").and_then(Value::as_str).map(str::to_string);
            let row = get(state, &ws, &id).await?.ok_or_else(|| JobError::Permanent("email vanished".into()))?;
            finalize_sent(state, &row, &message_id, thread_id.as_deref()).await.map_err(|e| {
                tracing::error!(error = %e, "email sent, but recording it failed");
                JobError::Retry(format!("sent, but recording failed: {e}"))
            })
        }
        Err(NangoError::Unavailable(m)) => Err(JobError::Retry(format!("Gmail's answer was unclear; will confirm before retrying: {m}"))),
        Err(e) => {
            release(state.clone(), id.clone(), attempt).await;
            give_up(state, &row, &format!("Not sent: {e}")).await
        }
    }
}

async fn finalize_sent(state: &AppState, row: &Outbound, provider_message_id: &str, thread_id: Option<&str>) -> AppResult<()> {
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let changed = sqlx::query(
        "UPDATE outbound_emails SET status = 'sent', provider_message_id = ?1, provider_thread_id = COALESCE(?2, provider_thread_id),
                send_at = NULL, send_error = NULL, updated_at = ?3
         WHERE id = ?4 AND status != 'sent'",
    )
    .bind(provider_message_id)
    .bind(thread_id)
    .bind(now_ms())
    .bind(&row.id)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if !changed {
        return Ok(());
    }
    let actor = match row.send_requested_by.as_deref() {
        Some(u) => Actor::User(u),
        None => Actor::System,
    };
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &row.workspace_id,
            actor,
            action: SENT_ACTION,
            entity_type: "outbound_email",
            entity_id: &row.id,
            message_id: None,
            metadata: json!({ "recipient_kind": row.recipient_kind, "auto": row.auto_sent, "authored_by": row.authored_by, "provider_message_id": provider_message_id }),
        },
    )
    .await?;
    let thread = thread_id.or(row.provider_thread_id.as_deref());
    crate::alerts::messaging::watch::email_sent(&mut tx, &row.workspace_id, &row.id, &row.source_id, thread).await?;
    tx.commit().await?;
    if let Some(owner) = row.owner_user_id.as_deref() {
        let held = if row.auto_sent { " I held it for a few minutes first so you could stop it." } else { "" };
        let text = format!("I sent “{}” to {}.{held}", crate::agents::sentence(&row.subject, 80), row.to_label_or_address());
        let links = json!([{ "kind": "sent", "id": row.id, "label": "See what I sent" }]);
        if let Err(e) = crate::alerts::messaging::report(state, &row.workspace_id, owner, &text, &[], &links).await {
            tracing::warn!(error = %e, "could not tell the person about the sent email");
        }
    }
    Ok(())
}


/// The email will not go out: say why, tell the owner and keep the email for a person to look at.
async fn give_up(state: &AppState, row: &Outbound, reason: &str) -> Result<(), JobError> {
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query(
        "UPDATE outbound_emails SET status = 'blocked', send_at = NULL, send_error = ?1, updated_at = ?2
         WHERE id = ?3 AND status = 'scheduled'",
    )
    .bind(truncate_chars(reason, 500))
    .bind(now_ms())
    .bind(&row.id)
    .execute(&mut *tx)
    .await?;
    crate::alerts::messaging::watch::close_for_outbound(&mut tx, &row.workspace_id, &row.id, "The email could not be sent.").await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &row.workspace_id,
            actor: Actor::System,
            action: "message.email_blocked",
            entity_type: "outbound_email",
            entity_id: &row.id,
            message_id: None,
            metadata: json!({ "unconfirmed": row.send_attempt_id.is_some() }),
        },
    )
    .await?;
    tx.commit().await?;
    if let Some(owner) = row.owner_user_id.as_deref() {
        let subject = format!("Email not sent: {}", crate::agents::sentence(&row.subject, 80));
        let link = json!({ "kind": "sent", "id": row.id });
        if let Err(e) = crate::alerts::messaging::inapp::notify_owner(state, &row.workspace_id, owner, row.agent_id.as_deref(), &subject, reason, link, false).await {
            tracing::warn!(error = %e, "could not tell the owner an email was not sent");
        }
    }
    Ok(())
}

/// Dead-lettered `SEND_OUTBOUND`.
pub async fn give_up_scheduled(state: &AppState, payload: &Value, error: &str) -> Result<(), JobError> {
    let field = |k: &str| payload.get(k).and_then(Value::as_str).map(str::to_string);
    let (Some(ws), Some(id)) = (field("workspace_id"), field("outbound_id")) else { return Ok(()) };
    let Some(row) = get(state, &ws, &id).await? else { return Ok(()) };
    if row.status != "scheduled" {
        return Ok(());
    }
    let reason = if row.send_attempt_id.is_some() {
        "We couldn't confirm whether this email went out. Check Gmail's Sent folder before sending it again.".to_string()
    } else {
        format!("Not sent: {error}")
    };
    give_up(state, &row, &reason).await
}
