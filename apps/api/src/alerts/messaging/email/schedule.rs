use super::*;

#[derive(Debug, Clone)]
pub struct Scheduled {
    pub id: String,
    pub send_at_ms: i64,
}

/// Holds the email for at least the undo window (`hold_secs` when longer) and queues the send. `auto` = nobody
/// asked for this send: a person's own words are refused, and the send is checked again when it comes due.
pub async fn schedule(state: &AppState, ws: &str, id: &str, actor: Actor<'_>, hold_secs: i64, auto: bool, daily_cap: Option<i64>) -> AppResult<Scheduled> {
    let row = get(state, ws, id).await?.ok_or(AppError::NotFound)?;
    if row.status != "awaiting_approval" {
        return Err(AppError::Conflict(match row.status.as_str() {
            "sent" => "this email was already sent".into(),
            "scheduled" => "this email is already on its way".into(),
            _ => "this email can no longer be sent".into(),
        }));
    }
    if auto && row.authored_by == "person" {
        return Err(AppError::Conflict("an email a person wrote is never sent on its own".into()));
    }
    let own = own_addresses(state, &row).await?;
    let report = check(&row.to_address, &row.subject, &row.body_text, &own);
    if report.is_blocked() {
        return Err(report.into_error());
    }
    let duplicate: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM outbound_emails WHERE workspace_id = ?1 AND to_address = ?2 AND subject = ?3 AND id != ?4
                          AND status IN ('scheduled','sent') AND created_at > ?5)",
    )
    .bind(ws)
    .bind(&row.to_address)
    .bind(&row.subject)
    .bind(id)
    .bind(now_ms() - send_safety::DUPLICATE_WINDOW_MS)
    .fetch_one(&state.db)
    .await?;
    if duplicate {
        return Err(SafetyReport { blocking: vec![issue("duplicate", "The same email just went to this address.", "Wait a few minutes.")], warnings: vec![] }.into_error());
    }
    let now = now_ms();
    let send_at = now + state.config.send_undo_seconds.max(hold_secs).max(0) * 1_000;
    let requested_by = match actor {
        Actor::User(u) => Some(u),
        Actor::System => None,
    };
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    if let (true, Some(cap), Some(owner)) = (auto, daily_cap, row.owner_user_id.as_deref())
        && auto_sent_today_in(&mut tx, ws, owner).await? >= cap
    {
        return Err(AppError::Conflict("today's limit for messages sent on their own is reached".into()));
    }
    let claimed = sqlx::query(
        "UPDATE outbound_emails SET status = 'scheduled', send_at = ?1, send_requested_by = ?2, auto_sent = ?3, send_error = NULL, updated_at = ?4
         WHERE id = ?5 AND workspace_id = ?6 AND status = 'awaiting_approval'",
    )
    .bind(send_at)
    .bind(requested_by)
    .bind(auto)
    .bind(now)
    .bind(id)
    .bind(ws)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if !claimed {
        return Err(AppError::Conflict("this email is already on its way".into()));
    }
    jobs::enqueue_at(
        &mut tx,
        kind::SEND_OUTBOUND,
        &format!("send_outbound:{id}:{send_at}"),
        Some(ws),
        &json!({ "workspace_id": ws, "outbound_id": id, "send_at": send_at }),
        jobs::PROVIDER_MAX_ATTEMPTS,
        send_at,
    )
    .await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: ws,
            actor,
            action: "message.email_scheduled",
            entity_type: "outbound_email",
            entity_id: id,
            message_id: None,
            metadata: json!({ "recipient_kind": row.recipient_kind, "auto": auto, "send_at": send_at, "authored_by": row.authored_by }),
        },
    )
    .await?;
    if auto {
        audit::record(
            &mut tx,
            Entry {
                workspace_id: ws,
                actor: Actor::System,
                action: AUTO_SENT_ACTION,
                entity_type: "outbound_email",
                entity_id: id,
                message_id: None,
                metadata: json!({ "channel": "email", "owner": row.owner_user_id, "hold_secs": (send_at - now) / 1000 }),
            },
        )
        .await?;
    }
    tx.commit().await?;
    state.jobs_notify.notify_one();
    Ok(Scheduled { id: id.to_string(), send_at_ms: send_at })
}

pub(super) async fn own_addresses(state: &AppState, row: &Outbound) -> sqlx::Result<Vec<String>> {
    let mailbox: Option<String> = sqlx::query_scalar("SELECT mailbox_address FROM email_sources WHERE id = ?1").bind(&row.source_id).fetch_optional(&state.db).await?.flatten();
    let owner: Option<String> = match &row.owner_user_id {
        Some(u) => sqlx::query_scalar("SELECT email FROM users WHERE id = ?1").bind(u).fetch_optional(&state.db).await?,
        None => None,
    };
    Ok(mailbox.into_iter().chain(owner).collect())
}

/// A person stops an email that has not gone out yet (the one who asked for it, or the mailbox owner).
pub async fn stop(state: &AppState, ws: &str, id: &str, user_id: &str) -> AppResult<()> {
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let stopped = sqlx::query(
        "UPDATE outbound_emails SET status = 'cancelled', send_at = NULL, updated_at = ?1
         WHERE id = ?2 AND workspace_id = ?3 AND status IN ('scheduled','awaiting_approval') AND send_attempt_id IS NULL
           AND (owner_user_id = ?4 OR send_requested_by = ?4)",
    )
    .bind(now_ms())
    .bind(id)
    .bind(ws)
    .bind(user_id)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if !stopped {
        let status: Option<(String, Option<String>)> =
            sqlx::query_as("SELECT status, owner_user_id FROM outbound_emails WHERE id = ?1 AND workspace_id = ?2").bind(id).bind(ws).fetch_optional(&mut *tx).await?;
        return Err(match status {
            Some((_, owner)) if owner.as_deref() != Some(user_id) => AppError::NotFound,
            Some((s, _)) if s == "sent" => AppError::Conflict("too late: this email was already sent".into()),
            Some(_) => AppError::Conflict("this email can no longer be stopped".into()),
            None => AppError::NotFound,
        });
    }
    crate::alerts::messaging::watch::close_for_outbound(&mut tx, ws, id, "You stopped the email before it went out.").await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: ws,
            actor: Actor::User(user_id),
            action: "message.email_stopped",
            entity_type: "outbound_email",
            entity_id: id,
            message_id: None,
            metadata: json!({}),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// A person approves a waiting email (a follow-up, say): it is held for the undo window, then sent.
pub async fn approve(state: &AppState, ws: &str, id: &str, user_id: &str) -> AppResult<Scheduled> {
    let row = get(state, ws, id).await?.ok_or(AppError::NotFound)?;
    if row.owner_user_id.as_deref() != Some(user_id) {
        return Err(AppError::NotFound);
    }
    schedule(state, ws, id, Actor::User(user_id), 0, false, None).await
}
