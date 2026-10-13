//! Memory learning jobs: `learn_from_message` (after classification) and `learn_from_reply`
//! (when a draft is approved or sent).

use serde_json::{Value, json};

use super::{handlers::JobError, kind};
use crate::{
    ai::memory_extractor::{LearnDirection, MemoryExtractInput},
    http::AppState,
    memory_learning::{self, AUTO_MEMORY_CAP},
};

const MIN_TEXT_CHARS: usize = 20;

fn str_field(payload: &Value, key: &str) -> Result<String, JobError> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| JobError::Permanent(format!("payload missing {key}")))
}

#[derive(sqlx::FromRow)]
struct MessageRow {
    workspace_id: String,
    thread_id: String,
    subject: String,
    sender_address: String,
    body_text: String,
    recipients_json: String,
    source_message_id: String,
    received_at: i64,
    category: Option<String>,
    needs_reply: Option<bool>,
    display_name: Option<String>,
}

pub async fn learn_from_message(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let message_id = str_field(payload, "message_id")?;
    let Some(m): Option<MessageRow> = sqlx::query_as(
        "SELECT m.workspace_id, m.thread_id, m.subject, m.sender_address, m.body_text, m.recipients_json,
                m.source_message_id, m.received_at, c.category,
                COALESCE(i.needs_reply, c.requires_reply) AS needs_reply, ct.display_name
         FROM email_messages m
         LEFT JOIN classifications c ON c.message_id = m.id
         LEFT JOIN message_insights i ON i.message_id = m.id
         LEFT JOIN contacts ct ON ct.workspace_id = m.workspace_id AND ct.email = m.sender_address
         WHERE m.id = ?1",
    )
    .bind(&message_id)
    .fetch_optional(&state.db)
    .await?
    else {
        return Ok(());
    };
    if m.body_text.trim().chars().count() < MIN_TEXT_CHARS {
        return Ok(());
    }
    let team = memory_learning::team_addresses(&state.db, &m.workspace_id).await?;
    let outbound = team.contains(&m.sender_address.to_lowercase());

    let input = if outbound {
        // Mail the team sent from the mailbox. Skip it if it is a reply we already learned from.
        let learned_as_draft: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM drafts WHERE workspace_id = ?1 AND status = 'sent' AND provider_message_id = ?2)",
        )
        .bind(&m.workspace_id)
        .bind(&m.source_message_id)
        .fetch_one(&state.db)
        .await?;
        if learned_as_draft {
            return Ok(());
        }
        let recipients: Vec<String> = serde_json::from_str(&m.recipients_json).unwrap_or_default();
        let counterpart = recipients.into_iter().find(|r| !team.contains(&r.to_lowercase()));
        let context: Option<String> = sqlx::query_scalar(
            "SELECT body_text FROM email_messages WHERE thread_id = ?1 AND received_at < ?2 AND id != ?3
             ORDER BY received_at DESC LIMIT 1",
        )
        .bind(&m.thread_id)
        .bind(m.received_at)
        .bind(&message_id)
        .fetch_optional(&state.db)
        .await?;
        MemoryExtractInput {
            direction: LearnDirection::Reply,
            subject: m.subject,
            counterpart,
            counterpart_name: None,
            label: m.category,
            text: m.body_text,
            context,
        }
    } else {
        // Newsletters/notifications: labelled `other` with nothing to answer, or an automated sender.
        let is_noise = m.category.as_deref() == Some(crate::domain::OTHER_LABEL) && m.needs_reply != Some(true);
        if is_noise || memory_learning::is_automated_sender(&m.sender_address) {
            return Ok(());
        }
        MemoryExtractInput {
            direction: LearnDirection::Incoming,
            subject: m.subject,
            counterpart: Some(m.sender_address.clone()),
            counterpart_name: m.display_name,
            label: m.category,
            text: m.body_text,
            context: None,
        }
    };
    let facts = state.memory_extractor.extract(&input).await?;
    let learned = memory_learning::sanitize(input.direction, input.counterpart.as_deref(), facts);
    let report =
        memory_learning::store(&state.db, &m.workspace_id, &message_id, &message_id, input.direction, learned, AUTO_MEMORY_CAP)
            .await?;
    tracing::info!(%message_id, created = report.created, merged = report.merged, "learned from message");
    Ok(())
}

#[derive(sqlx::FromRow)]
struct ReplyRow {
    workspace_id: String,
    message_id: String,
    reply: String,
    status: String,
    subject: String,
    sender: String,
    original: String,
    category: Option<String>,
}

/// Enqueues `learn_from_reply` for an approved or sent draft (deduped per draft).
pub async fn enqueue_reply_learning(pool: &sqlx::SqlitePool, draft_id: &str) -> sqlx::Result<()> {
    let ws: Option<String> =
        sqlx::query_scalar("SELECT workspace_id FROM drafts WHERE id = ?1").bind(draft_id).fetch_optional(pool).await?;
    let Some(ws) = ws else { return Ok(()) };
    let mut conn = pool.acquire().await?;
    super::enqueue(&mut conn, kind::LEARN_FROM_REPLY, &format!("learn-reply:{draft_id}"), Some(&ws), &json!({ "draft_id": draft_id }))
        .await?;
    Ok(())
}

pub async fn learn_from_reply(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let draft_id = str_field(payload, "draft_id")?;
    let row: Option<ReplyRow> = sqlx::query_as(
        "SELECT d.workspace_id, d.message_id, d.body_text AS reply, d.status, m.subject, m.sender_address AS sender,
                m.body_text AS original, c.category
         FROM drafts d
         JOIN email_messages m ON m.id = d.message_id
         LEFT JOIN classifications c ON c.message_id = m.id
         WHERE d.id = ?1",
    )
    .bind(&draft_id)
    .fetch_optional(&state.db)
    .await?;
    let Some(ReplyRow { workspace_id, message_id, reply, status, subject, sender, original, category }) = row else {
        return Ok(());
    };
    if !matches!(status.as_str(), "approved" | "sent") || reply.trim().chars().count() < MIN_TEXT_CHARS {
        return Ok(());
    }
    let input = MemoryExtractInput {
        direction: LearnDirection::Reply,
        subject,
        counterpart: Some(sender),
        counterpart_name: None,
        label: category,
        text: reply,
        context: Some(original),
    };
    let facts = state.memory_extractor.extract(&input).await?;
    let learned = memory_learning::sanitize(LearnDirection::Reply, input.counterpart.as_deref(), facts);
    let report =
        memory_learning::store(&state.db, &workspace_id, &message_id, &draft_id, LearnDirection::Reply, learned, AUTO_MEMORY_CAP)
            .await?;
    tracing::info!(%draft_id, created = report.created, merged = report.merged, "learned from reply");
    Ok(())
}
