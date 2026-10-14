//! The hook points: one call from each place a person's action teaches an agent. Every function writes on the
//! caller's connection, so the signal commits (or rolls back) with the action itself.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::SqliteConnection;

use super::{
    edit_distance::{self, TINY_EDIT},
    signals::{self, Kind, Signal},
};
use crate::jobs::{self, kind};

#[derive(sqlx::FromRow)]
struct DraftRow {
    message_id: String,
    agent_id: Option<String>,
    original_body_text: Option<String>,
    body_text: String,
}

fn words(text: &str) -> usize {
    text.split_whitespace().count()
}

/// Where a finalised draft came from.
pub enum Via<'a> {
    /// A person approved it in the inbox.
    Inbox,
    /// A person approved an agent's request (with the case and approval ids).
    Approval { approval_id: &'a str, case_id: &'a str, agent_id: Option<&'a str> },
}

/// A person approved the final text of a draft: records how far they moved it, stores the distance on the
/// draft and queues learning from it. Returns the edit distance.
pub async fn draft_finalised(conn: &mut SqliteConnection, ws: &str, user_id: &str, draft_id: &str, via: Via<'_>) -> sqlx::Result<Option<f64>> {
    let Some(d): Option<DraftRow> =
        sqlx::query_as("SELECT message_id, agent_id, original_body_text, body_text FROM drafts WHERE id = ?1 AND workspace_id = ?2")
            .bind(draft_id)
            .bind(ws)
            .fetch_optional(&mut *conn)
            .await?
    else {
        return Ok(None);
    };
    let original = d.original_body_text.as_deref().unwrap_or(&d.body_text);
    let distance = edit_distance::normalised(original, &d.body_text);
    sqlx::query("UPDATE drafts SET edit_distance = ?1 WHERE id = ?2").bind(distance).bind(draft_id).execute(&mut *conn).await?;
    let edited = distance >= TINY_EDIT;
    let words_json = json!({"words": words(original)});
    let after = json!({"words": words(&d.body_text)});
    let signal = match via {
        Via::Inbox => Signal::new(ws, if edited { Kind::DraftEdited } else { Kind::DraftApproved }).agent(d.agent_id.as_deref()),
        Via::Approval { approval_id, case_id, agent_id } => {
            Signal::new(ws, if edited { Kind::ApprovalEdited } else { Kind::ApprovalApproved })
                .agent(agent_id.or(d.agent_id.as_deref()))
                .case(case_id)
                .approval(approval_id)
        }
    };
    signals::record(
        conn,
        signal.actor(user_id).message(Some(&d.message_id)).draft(Some(draft_id)).distance(Some(distance)).change(words_json, after),
    )
    .await?;
    enqueue_learning(conn, ws, draft_id, &d.body_text).await?;
    Ok(Some(distance))
}

/// Queues `learn_from_signal` for a finalised draft: once per distinct final text.
pub async fn enqueue_learning(conn: &mut SqliteConnection, ws: &str, draft_id: &str, final_text: &str) -> sqlx::Result<()> {
    let hash: String = Sha256::digest(final_text.trim().as_bytes()).iter().take(4).map(|b| format!("{b:02x}")).collect();
    jobs::enqueue(conn, kind::LEARN_FROM_SIGNAL, &format!("learn-draft:{draft_id}:{hash}"), Some(ws), &json!({"draft_id": draft_id, "kind": "draft"}))
        .await?;
    Ok(())
}

pub struct Declined<'a> {
    pub ws: &'a str,
    pub user_id: &'a str,
    pub approval_id: &'a str,
    pub case_id: &'a str,
    pub agent_id: Option<&'a str>,
    pub message_id: Option<&'a str>,
    pub note: Option<&'a str>,
}

/// A person declined an agent's request. A note, if any, is queued to become a `process` suggestion.
pub async fn approval_declined(conn: &mut SqliteConnection, d: Declined<'_>) -> sqlx::Result<()> {
    let id = signals::record(
        conn,
        Signal::new(d.ws, Kind::ApprovalDeclined)
            .agent(d.agent_id)
            .actor(d.user_id)
            .message(d.message_id)
            .case(d.case_id)
            .approval(d.approval_id)
            .note(d.note),
    )
    .await?;
    if d.note.is_some_and(|n| !n.trim().is_empty()) {
        let key = format!("learn-note:{}", d.approval_id);
        jobs::enqueue(conn, kind::LEARN_FROM_SIGNAL, &key, Some(d.ws), &json!({"signal_id": id, "kind": "decline_note"})).await?;
    }
    Ok(())
}

/// The label a person set (or confirmed) on an email.
pub async fn label_decision(conn: &mut SqliteConnection, ws: &str, user_id: &str, message_id: &str, before: Option<&str>, after: &str) -> sqlx::Result<()> {
    let kind = if before == Some(after) { Kind::LabelConfirmed } else { Kind::LabelCorrected };
    signals::record(
        conn,
        Signal::new(ws, kind).actor(user_id).message(Some(message_id)).change(json!({"label": before}), json!({"label": after})),
    )
    .await?;
    Ok(())
}

/// A person moved an email to another agent. Works from the audit event WP-B writes for it.
pub async fn route_corrected(conn: &mut SqliteConnection, ws: &str, user_id: &str, message_id: &str, from_agent: Option<&str>, to_agent: &str) -> sqlx::Result<()> {
    signals::record(
        conn,
        Signal::new(ws, Kind::RouteCorrected)
            .agent(from_agent)
            .actor(user_id)
            .message(Some(message_id))
            .change(json!({"agent_id": from_agent}), json!({"agent_id": to_agent})),
    )
    .await?;
    Ok(())
}

/// A person cancelled a scheduled automatic send.
pub async fn auto_send_stopped(conn: &mut SqliteConnection, ws: &str, user_id: &str, draft_id: &str, message_id: &str, agent_id: Option<&str>) -> sqlx::Result<()> {
    signals::record(conn, Signal::new(ws, Kind::AutoSendStopped).agent(agent_id).actor(user_id).message(Some(message_id)).draft(Some(draft_id))).await?;
    Ok(())
}

pub async fn case_cancelled(conn: &mut SqliteConnection, ws: &str, user_id: &str, case_id: &str, message_id: Option<&str>, agent_id: Option<&str>) -> sqlx::Result<()> {
    signals::record(conn, Signal::new(ws, Kind::CaseCancelled).agent(agent_id).actor(user_id).message(message_id).case(case_id)).await?;
    Ok(())
}

/// A person accepted or rejected a learned preference, or confirmed or archived a learned memory.
pub async fn judged(conn: &mut SqliteConnection, ws: &str, user_id: &str, kind: Kind, agent_id: Option<&str>, subject: Value) -> sqlx::Result<()> {
    signals::record(conn, Signal::new(ws, kind).agent(agent_id).actor(user_id).change(json!({}), subject)).await?;
    Ok(())
}
