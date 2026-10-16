//! Waiting and waking for expert decisions. An expert (the timesheet desk) can decide "tell the sender now, and the
//! rest when X happens". `poke` is X happening: it finds the messages whose parked requests were waiting for it and
//! lets the desk look at them again, which releases what is now ready. Everything it does is repeatable: looking again at
//! a message whose reply already went out files nothing new.

pub mod wait;

pub use wait::{Wait, Wake};

use serde_json::{Value, json};

use crate::{
    hr::{staffing::timesheet_records::desk, team::settings},
    http::AppState,
    jobs::handlers::JobError,
    platform::{
        audit::{self, Actor, Entry},
        db::now_ms,
        flags::{self, Key},
        mailer::{self, OutgoingEmail},
    },
};

/// Something a wait was about has happened. Returns how many messages were looked at again.
pub async fn poke(state: &AppState, ws: &str, wake: &Wake) -> sqlx::Result<usize> {
    if !flags::mode(&state.db, &state.config, ws, Key::Handoff).await?.acts() {
        return Ok(0);
    }
    let Wake::PersonLinked { record_ids } = wake;
    let messages: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT message_id FROM timesheet_records WHERE workspace_id = ?1 AND id IN (SELECT value FROM json_each(?2)) AND message_id IS NOT NULL ORDER BY message_id",
    )
    .bind(ws)
    .bind(serde_json::json!(record_ids).to_string())
    .fetch_all(&state.db)
    .await?;
    for message_id in &messages {
        desk::resume(state, ws, message_id).await?;
    }
    Ok(messages.len())
}

/// The `person_linked` job, written in the transaction of the link: it wakes the desk for the sheets that were linked. The
/// worker retries it; the link itself is never undone by a failure here.
pub async fn person_linked_job(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let ws = payload.get("workspace_id").and_then(Value::as_str).ok_or_else(|| JobError::Permanent("payload missing workspace_id".into()))?;
    let record_ids: Vec<String> = payload.get("record_ids").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    poke(state, ws, &Wake::PersonLinked { record_ids }).await?;
    Ok(())
}

/// D38: a request parked for a person to be linked is a sender waiting for a confirmation. Once one has waited longer than the
/// workspace's `manager_after_hours`, the final contacts get one note (counts and a link, never a name). Each request is
/// reminded once. Returns how many requests the note was about.
pub async fn remind_waiting(state: &AppState, ws: &str) -> sqlx::Result<usize> {
    if !flags::mode(&state.db, &state.config, ws, Key::Handoff).await?.acts() {
        return Ok(0);
    }
    let cfg = settings::load(&state.db, ws).await?;
    let (Some(hours), Some(mailer)) = (cfg.manager_after_hours, state.alert_mailer.as_ref()) else { return Ok(0) };
    let recipients: Vec<String> = if cfg.final_emails.is_empty() { crate::alerts::load_settings(&state.db, ws).await?.recipients } else { cfg.final_emails.clone() };
    if recipients.is_empty() {
        return Ok(0);
    }
    let now = now_ms();
    let due: Vec<String> = sqlx::query_scalar(
        "UPDATE message_requests SET reminded_at = ?3 WHERE workspace_id = ?1 AND status = 'waiting' AND reminded_at IS NULL AND created_at < ?2 RETURNING id",
    )
    .bind(ws)
    .bind(now - hours * 3_600_000)
    .bind(now)
    .fetch_all(&state.db)
    .await?;
    if due.is_empty() {
        return Ok(0);
    }
    let n = due.len();
    let url = format!("{}/w/{ws}/timesheets", state.config.app_base_url.trim_end_matches('/'));
    let subject = format!("[Post Room] {n} timesheet {} waiting for someone to say who it is from", if n == 1 { "is" } else { "sheets are" });
    let text = format!("{n} timesheet {} still waiting: the person is not on your people list, so the sender has had a receipt but not yet a confirmation.\n\nOpen People we don't know yet: {url}\n", if n == 1 { "is" } else { "sheets are" });
    let html = format!("<p>{n} timesheet {} still waiting for someone to say who it is from.</p><p><a href=\"{url}\">People we don't know yet</a></p>", if n == 1 { "is" } else { "sheets are" });
    let mut sent = 0;
    for to in &recipients {
        let email = OutgoingEmail { kind: mailer::kind::ALERT, to, subject: &subject, text: &text, html: &html };
        match mailer::deliver(&state.db, mailer.as_ref(), email).await {
            Ok(()) => sent += 1,
            Err(e) => tracing::warn!(error = %e, "waiting-timesheet reminder not sent"),
        }
    }
    if sent == 0 {
        sqlx::query("UPDATE message_requests SET reminded_at = NULL WHERE workspace_id = ?1 AND id IN (SELECT value FROM json_each(?2))").bind(ws).bind(json!(due).to_string()).execute(&state.db).await?;
        return Ok(0);
    }
    let mut conn = state.db.acquire().await?;
    audit::record(&mut conn, Entry { workspace_id: ws, actor: Actor::System, action: "handoff.reminded", entity_type: "workspace", entity_id: ws, message_id: None, metadata: json!({"requests": n, "recipients": sent}) }).await?;
    Ok(n)
}
