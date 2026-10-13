//! Knowledge store indexing: emails (+ extracted attachment text) after classification,
//! replies once approved or sent, and a bounded backfill for mail that predates the store.

use serde_json::{Value, json};
use sqlx::{SqliteConnection, SqlitePool};

use super::{Job, handlers::JobError, kind};
use crate::{
    http::AppState,
    jobs,
    knowledge::{self, IndexError},
};

/// Items enqueued per backfill run; the job re-enqueues itself with a cursor until done.
pub const BACKFILL_BATCH: i64 = 50;

impl From<IndexError> for JobError {
    fn from(e: IndexError) -> Self {
        match e {
            IndexError::Db(e) => e.into(),
            IndexError::Ai(e) => e.into(),
        }
    }
}

pub async fn handle(state: &AppState, job: &Job, payload: &Value) -> Result<(), JobError> {
    match job.kind.as_str() {
        kind::INDEX_MESSAGE => index_message(state, payload).await,
        kind::INDEX_REPLY => index_reply(state, payload).await,
        kind::BACKFILL_KNOWLEDGE => backfill(state, payload).await,
        other => Err(JobError::Permanent(format!("not a knowledge job: {other}"))),
    }
}

pub async fn enqueue_index_message(conn: &mut SqliteConnection, workspace_id: &str, message_id: &str) -> sqlx::Result<bool> {
    jobs::enqueue(conn, kind::INDEX_MESSAGE, &format!("index:{message_id}"), Some(workspace_id), &json!({ "message_id": message_id }))
        .await
}

/// Re-index after the message's content changed (e.g. attachment text became available).
pub async fn enqueue_reindex_message(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    message_id: &str,
    reason: &str,
) -> sqlx::Result<bool> {
    jobs::enqueue(
        conn,
        kind::INDEX_MESSAGE,
        &format!("index:{message_id}:{reason}"),
        Some(workspace_id),
        &json!({ "message_id": message_id }),
    )
    .await
}

/// `version` makes the key unique per approval/send so later edits are re-indexed.
pub async fn enqueue_index_reply(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    draft_id: &str,
    version: &str,
) -> sqlx::Result<bool> {
    jobs::enqueue(
        conn,
        kind::INDEX_REPLY,
        &format!("index:reply:{draft_id}:{version}"),
        Some(workspace_id),
        &json!({ "draft_id": draft_id }),
    )
    .await
}

/// Starts the one-time backfill (idempotent: a finished backfill is not repeated).
pub async fn enqueue_backfill(db: &SqlitePool) -> sqlx::Result<bool> {
    let mut conn = db.acquire().await?;
    jobs::enqueue(&mut conn, kind::BACKFILL_KNOWLEDGE, "backfill:knowledge:v1", None, &json!({ "phase": "messages", "after": "" }))
        .await
}

fn str_field(payload: &Value, key: &str) -> Result<String, JobError> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| JobError::Permanent(format!("payload missing {key}")))
}

async fn index_message(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let message_id = str_field(payload, "message_id")?;
    let row: Option<(String, String, String)> =
        sqlx::query_as("SELECT workspace_id, subject, body_text FROM email_messages WHERE id = ?1")
            .bind(&message_id)
            .fetch_optional(&state.db)
            .await?;
    let Some((workspace_id, subject, body)) = row else {
        return Ok(());
    };
    let attachments: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT id, filename, text_content FROM attachments
         WHERE message_id = ?1 AND text_status = 'extracted' AND text_content IS NOT NULL
         ORDER BY filename LIMIT ?2",
    )
    .bind(&message_id)
    .bind(knowledge::MAX_ATTACHMENTS)
    .fetch_all(&state.db)
    .await?;

    let mut docs = vec![knowledge::email_document(&message_id, &subject, &body)];
    docs.extend(attachments.iter().map(|(id, name, text)| knowledge::attachment_document(id, name, text)));
    let mut tx = state.db.begin().await?;
    let mut changed = 0;
    for doc in &docs {
        changed += knowledge::upsert_document(&mut tx, &workspace_id, &message_id, doc).await? as usize;
    }
    tx.commit().await?;
    let embedded = knowledge::embed_pending(&state.db, state.embedder.as_ref(), &message_id).await?;
    tracing::info!(%message_id, changed, embedded, "indexed message");
    Ok(())
}

async fn index_reply(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let draft_id = str_field(payload, "draft_id")?;
    let row: Option<(String, String, String, String, String, String)> = sqlx::query_as(
        "SELECT d.workspace_id, d.message_id, d.status, d.body_text, m.subject, m.body_text
         FROM drafts d JOIN email_messages m ON m.id = d.message_id WHERE d.id = ?1",
    )
    .bind(&draft_id)
    .fetch_optional(&state.db)
    .await?;
    let Some((workspace_id, message_id, status, reply, subject, email_body)) = row else {
        return Ok(());
    };
    let mut tx = state.db.begin().await?;
    if matches!(status.as_str(), "approved" | "sent") {
        let doc = knowledge::reply_document(&draft_id, &subject, &email_body, &reply);
        knowledge::upsert_document(&mut tx, &workspace_id, &message_id, &doc).await?;
    } else {
        // Re-opened for editing: only approved or sent replies are examples.
        knowledge::delete_document(&mut tx, &workspace_id, knowledge::kind::REPLY, &draft_id).await?;
    }
    tx.commit().await?;
    knowledge::embed_pending(&state.db, state.embedder.as_ref(), &message_id).await?;
    Ok(())
}

/// Enqueues indexing for up to `BACKFILL_BATCH` unindexed items, then itself with a cursor.
/// Phase `messages` (emails + attachments), then phase `replies`.
async fn backfill(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let phase = payload.get("phase").and_then(Value::as_str).unwrap_or("messages");
    let after = payload.get("after").and_then(Value::as_str).unwrap_or_default();
    let rows: Vec<(String, String)> = match phase {
        "messages" => {
            sqlx::query_as(
                "SELECT m.id, m.workspace_id FROM email_messages m
                 WHERE m.id > ?1 AND m.direction = 'inbound'
                   -- Only the latest 100 emails per workspace are worth indexing retroactively.
                   AND m.id IN (SELECT x.id FROM email_messages x WHERE x.workspace_id = m.workspace_id
                                AND x.direction = 'inbound' ORDER BY x.received_at DESC LIMIT 100)
                   AND NOT EXISTS (
                   SELECT 1 FROM knowledge_chunks c WHERE c.message_id = m.id AND c.kind = 'email' AND c.embedded = 1)
                 ORDER BY m.id LIMIT ?2",
            )
            .bind(after)
            .bind(BACKFILL_BATCH)
            .fetch_all(&state.db)
            .await?
        }
        "replies" => {
            sqlx::query_as(
                "SELECT d.id, d.workspace_id FROM drafts d
                 WHERE d.id > ?1 AND d.status IN ('approved', 'sent') AND NOT EXISTS (
                   SELECT 1 FROM knowledge_chunks c WHERE c.kind = 'reply' AND c.ref_id = d.id AND c.embedded = 1)
                 ORDER BY d.id LIMIT ?2",
            )
            .bind(after)
            .bind(BACKFILL_BATCH)
            .fetch_all(&state.db)
            .await?
        }
        other => return Err(JobError::Permanent(format!("unknown backfill phase {other}"))),
    };
    let mut tx = state.db.begin().await?;
    for (id, workspace_id) in &rows {
        if phase == "messages" {
            enqueue_index_message(&mut tx, workspace_id, id).await?;
        } else {
            enqueue_index_reply(&mut tx, workspace_id, id, "backfill").await?;
        }
    }
    let next = match (rows.last(), phase) {
        (Some((last, _)), _) if rows.len() as i64 == BACKFILL_BATCH => Some((phase, last.as_str())),
        (_, "messages") => Some(("replies", "")),
        _ => None,
    };
    if let Some((next_phase, cursor)) = next {
        jobs::enqueue(
            &mut tx,
            kind::BACKFILL_KNOWLEDGE,
            &format!("backfill:knowledge:v1:{next_phase}:{cursor}"),
            None,
            &json!({ "phase": next_phase, "after": cursor }),
        )
        .await?;
    }
    tx.commit().await?;
    tracing::info!(phase, enqueued = rows.len(), "knowledge backfill step");
    state.jobs_notify.notify_one();
    Ok(())
}
