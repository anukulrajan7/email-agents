//! The short summary the manager gets when the desk has recorded a sheet: in the app and by email. It is claimed
//! atomically on the record (`summarized_at`), so reading the same sheet again tells nobody twice, and it is only
//! for recent mail: the backfill of old mail stays silent. Nothing here writes to the sender.

use serde_json::json;
use sqlx::SqlitePool;

use super::{
    RecordRow,
    rules::{Decision, MessageKind},
};
use crate::{
    alerts::{self, messaging::inapp},
    hr::staffing::policy::address_of,
    hr::team::escalation,
    http::AppState,
    platform::{
        db::now_ms,
        mailer::{self, OutgoingEmail},
    },
};

/// Who hears about these records: each person's manager (else the final contacts), and the final contacts. Deduped,
/// and never the person who sent the sheet.
pub async fn recipients(db: &SqlitePool, ws: &str, rows: &[RecordRow]) -> sqlx::Result<Vec<String>> {
    let rows: Vec<&RecordRow> = rows.iter().collect();
    recipients_of(db, ws, &rows).await
}

async fn recipients_of(db: &SqlitePool, ws: &str, rows: &[&RecordRow]) -> sqlx::Result<Vec<String>> {
    let fallback = alerts::load_settings(db, ws).await?.recipients;
    let senders: Vec<String> = rows.iter().filter_map(|r| r.sender_address.as_deref()).map(address_of).collect();
    let mut out: Vec<String> = vec![];
    for person in rows.iter().filter_map(|r| r.person_id.as_deref()) {
        for level in [2, 3] {
            for email in escalation::recipients(db, ws, person, level, &fallback).await? {
                let email = email.trim().to_lowercase();
                if !out.contains(&email) && !senders.contains(&email) {
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

/// Tells the manager once. Returns how many notices and emails actually went out (0 when there was nothing to say, the
/// mail is old, nobody can be told, or an earlier run already did). The summary is only claimed when someone can be
/// told, and the claim is given back if every delivery failed.
pub async fn summarize(state: &AppState, ws: &str, agent_id: Option<&str>, rows: &[RecordRow], decision: &Decision) -> sqlx::Result<usize> {
    let Some(summary) = &decision.manager_summary else { return Ok(0) };
    let rows: Vec<&RecordRow> = rows.iter().filter(|r| r.person_id.is_some() && r.status != "replaced").collect();
    // A "recorded" summary is only true when every sheet is recorded; a question to the manager is about a sheet that needs a look.
    let ack = decision.message.as_ref().is_some_and(|m| m.kind == MessageKind::Ack);
    if rows.is_empty() || !is_recent(state, &rows) || (ack && rows.iter().any(|r| r.status != "recorded")) {
        return Ok(0);
    }
    let recipients = recipients_of(&state.db, ws, &rows).await?;
    let mut members: Vec<String> = vec![];
    for email in &recipients {
        let user: Option<String> = sqlx::query_scalar(
            "SELECT u.id FROM users u JOIN memberships m ON m.user_id = u.id WHERE m.workspace_id = ?1 AND lower(u.email) = ?2 LIMIT 1",
        )
        .bind(ws)
        .bind(email)
        .fetch_optional(&state.db)
        .await?;
        members.extend(user);
    }
    if recipients.is_empty() || (members.is_empty() && state.alert_mailer.is_none()) {
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
    let message_id = rows.iter().find_map(|r| r.message_id.clone());
    let link = json!({ "kind": "timesheet", "record_id": claimed[0], "record_ids": claimed, "message_id": message_id });
    for user in &members {
        inapp::notify_in(&mut tx, ws, user, agent_id, &crate::agents::sentence(&summary.headline, 190), &format!("{body}\nOpen Timesheets to review or approve."), link.clone(), false).await?;
    }
    tx.commit().await?;
    let mut told = members.len();
    if let Some(mailer) = &state.alert_mailer {
        let url = format!("{}/w/{ws}/timesheets", state.config.app_base_url.trim_end_matches('/'));
        let subject = format!("[Post Room] {}", crate::agents::sentence(&summary.headline, 160));
        let text = format!("{body}\n\nReview or approve it: {url}\n");
        let html = format!("<p>{}</p><p><a href=\"{url}\">Review or approve it</a></p>", alerts::escape_html(&body).replace('\n', "<br>"));
        for to in &recipients {
            let email = OutgoingEmail { kind: mailer::kind::TIMESHEET_SUMMARY, to, subject: &subject, text: &text, html: &html };
            match mailer::deliver(&state.db, mailer.as_ref(), email).await {
                Ok(()) => told += 1,
                Err(e) => tracing::warn!(error = %e, "timesheet summary email not sent"),
            }
        }
    }
    if told == 0 {
        sqlx::query("UPDATE timesheet_records SET summarized_at = NULL WHERE workspace_id = ?1 AND id IN (SELECT value FROM json_each(?2))")
            .bind(ws)
            .bind(json!(claimed).to_string())
            .execute(&state.db)
            .await?;
    }
    Ok(told)
}
