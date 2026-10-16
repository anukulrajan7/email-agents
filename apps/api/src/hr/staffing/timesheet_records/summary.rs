//! The short summary the manager gets when the desk has recorded a sheet: in the app and by email. It is claimed
//! atomically on the record (`summarized_at`), so reading the same sheet again tells nobody twice, and it is only
//! for recent mail: the backfill of old mail stays silent. Nothing here writes to the sender.

use serde_json::json;
use sqlx::SqlitePool;

use super::{RecordRow, rules::Decision};
use crate::{
    alerts::{self, messaging::inapp},
    hr::team::escalation,
    http::AppState,
    platform::{
        db::now_ms,
        mailer::{self, OutgoingEmail},
    },
};

/// Who hears about these records: each person's manager (else the final contacts), and the final contacts. Deduped.
pub async fn recipients(db: &SqlitePool, ws: &str, rows: &[RecordRow]) -> sqlx::Result<Vec<String>> {
    let persons: Vec<&str> = rows.iter().filter_map(|r| r.person_id.as_deref()).collect();
    recipients_of(db, ws, &persons).await
}

async fn recipients_of(db: &SqlitePool, ws: &str, persons: &[&str]) -> sqlx::Result<Vec<String>> {
    let fallback = alerts::load_settings(db, ws).await?.recipients;
    let mut out: Vec<String> = vec![];
    for person in persons.iter().copied() {
        for level in [2, 3] {
            for email in escalation::recipients(db, ws, person, level, &fallback).await? {
                let email = email.trim().to_lowercase();
                if !out.contains(&email) {
                    out.push(email);
                }
            }
        }
    }
    Ok(out)
}

fn is_recent(state: &AppState, rows: &[&RecordRow]) -> bool {
    let cutoff = now_ms() - state.config.ai_auto_hours * 3_600_000;
    rows.iter().all(|r| r.email_received_at.is_some_and(|at| at >= cutoff))
}

/// Tells the manager once. Returns how many people were told (0 when there was nothing to say, the mail is old, nobody
/// is to be told, or an earlier run already did).
pub async fn summarize(state: &AppState, ws: &str, agent_id: Option<&str>, rows: &[RecordRow], decision: &Decision) -> sqlx::Result<usize> {
    let Some(summary) = &decision.manager_summary else { return Ok(0) };
    let rows: Vec<&RecordRow> = rows.iter().filter(|r| r.person_id.is_some() && r.status != "replaced").collect();
    if rows.is_empty() || !is_recent(state, &rows) {
        return Ok(0);
    }
    let persons: Vec<&str> = rows.iter().filter_map(|r| r.person_id.as_deref()).collect();
    let recipients = recipients_of(&state.db, ws, &persons).await?;
    if recipients.is_empty() {
        return Ok(0);
    }
    let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let claimed: Vec<String> = sqlx::query_scalar(
        "UPDATE timesheet_records SET summarized_at = ?1 WHERE workspace_id = ?2 AND id IN (SELECT value FROM json_each(?3)) AND summarized_at IS NULL RETURNING id",
    )
    .bind(now_ms())
    .bind(ws)
    .bind(json!(ids).to_string())
    .fetch_all(&mut *tx)
    .await?;
    if claimed.is_empty() {
        tx.rollback().await?;
        return Ok(0);
    }
    let mut body = summary.headline.clone();
    for note in &summary.notes {
        body.push_str(&format!("\n- {note}"));
    }
    let link = json!({ "kind": "timesheet", "record_id": claimed[0] });
    let mut members: Vec<(String, String)> = vec![];
    for email in &recipients {
        let user: Option<String> = sqlx::query_scalar(
            "SELECT u.id FROM users u JOIN memberships m ON m.user_id = u.id WHERE m.workspace_id = ?1 AND lower(u.email) = ?2 LIMIT 1",
        )
        .bind(ws)
        .bind(email)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(user) = user {
            members.push((email.clone(), user));
        }
    }
    for (_, user) in &members {
        inapp::notify_in(&mut tx, ws, user, agent_id, &crate::agents::sentence(&summary.headline, 190), &format!("{body}\nOpen Timesheets to review or approve."), link.clone(), false).await?;
    }
    tx.commit().await?;
    if let Some(mailer) = &state.alert_mailer {
        let url = format!("{}/w/{ws}/timesheets", state.config.app_base_url.trim_end_matches('/'));
        let subject = format!("[Post Room] {}", crate::agents::sentence(&summary.headline, 160));
        let text = format!("{body}\n\nReview or approve it: {url}\n");
        let html = format!("<p>{}</p><p><a href=\"{url}\">Review or approve it</a></p>", alerts::escape_html(&body).replace('\n', "<br>"));
        for to in &recipients {
            let email = OutgoingEmail { kind: mailer::kind::TIMESHEET_SUMMARY, to, subject: &subject, text: &text, html: &html };
            if let Err(e) = mailer::deliver(&state.db, mailer.as_ref(), email).await {
                tracing::warn!(error = %e, "timesheet summary email not sent");
            }
        }
    }
    Ok(recipients.len())
}
