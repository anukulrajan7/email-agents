//! Source-agnostic persistence of a normalized email. Every adapter (forwarding,
//! Nango Gmail, ...) ends here, so dedupe and pipeline kickoff behave identically.

use serde_json::json;
use sqlx::SqliteConnection;

use crate::{
    audit::{self, Actor, Entry},
    db::{new_id, now_ms},
    domain::{NormalizedEmail, TAXONOMY_VERSION},
    jobs::{self, kind},
    memory,
};

/// Inserts the message (+ thread, attachments, audit, classify job) unless
/// `(source_id, source_message_id)` already exists. Returns the new message ID.
///
/// Caller must hold an open transaction whose first statement was a write, so the
/// existence check below runs under SQLite's write lock.
pub async fn persist_message(
    conn: &mut SqliteConnection,
    source_id: &str,
    workspace_id: &str,
    email: &NormalizedEmail,
    provider_thread_id: Option<&str>,
    origin: serde_json::Value,
) -> sqlx::Result<Option<String>> {
    let exists: Option<String> =
        sqlx::query_scalar("SELECT id FROM email_messages WHERE source_id = ?1 AND source_message_id = ?2")
            .bind(source_id)
            .bind(&email.source_message_id)
            .fetch_optional(&mut *conn)
            .await?;
    if exists.is_some() {
        return Ok(None);
    }

    let now = now_ms();
    let thread_id = match provider_thread_id {
        Some(ptid) => {
            sqlx::query(
                "INSERT OR IGNORE INTO email_threads (id, workspace_id, source_id, provider_thread_id, subject, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .bind(new_id("thr"))
            .bind(workspace_id)
            .bind(source_id)
            .bind(ptid)
            .bind(&email.subject)
            .bind(now)
            .execute(&mut *conn)
            .await?;
            let id: String = sqlx::query_scalar(
                "UPDATE email_threads SET updated_at = ?3 WHERE source_id = ?1 AND provider_thread_id = ?2 RETURNING id",
            )
            .bind(source_id)
            .bind(ptid)
            .bind(now)
            .fetch_one(&mut *conn)
            .await?;
            id
        }
        // No reliable thread ID (forwarding): one message per thread avoids bad merges.
        None => {
            let id = new_id("thr");
            sqlx::query(
                "INSERT INTO email_threads (id, workspace_id, source_id, provider_thread_id, subject, updated_at)
                 VALUES (?1, ?2, ?3, NULL, ?4, ?5)",
            )
            .bind(&id)
            .bind(workspace_id)
            .bind(source_id)
            .bind(&email.subject)
            .bind(now)
            .execute(&mut *conn)
            .await?;
            id
        }
    };

    let message_id = new_id("msg");
    sqlx::query(
        "INSERT INTO email_messages
           (id, workspace_id, source_id, thread_id, source_message_id, rfc_message_id, sender_address,
            original_sender_address, recipients_json, subject, body_text, body_html, received_at, created_at, processing_status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 'normalized')",
    )
    .bind(&message_id)
    .bind(workspace_id)
    .bind(source_id)
    .bind(&thread_id)
    .bind(&email.source_message_id)
    .bind(&email.rfc_message_id)
    .bind(&email.sender_address)
    .bind(&email.original_sender_address)
    .bind(json!(email.recipients).to_string())
    .bind(&email.subject)
    .bind(&email.body_text)
    .bind(&email.body_html)
    .bind(email.received_at)
    .bind(now)
    .execute(&mut *conn)
    .await?;

    memory::record_contact(&mut *conn, workspace_id, &email.sender_address, email.sender_name.as_deref(), email.received_at)
        .await?;

    for a in &email.attachments {
        sqlx::query("INSERT INTO attachments (id, message_id, filename, media_type, size_bytes) VALUES (?1, ?2, ?3, ?4, ?5)")
            .bind(new_id("att"))
            .bind(&message_id)
            .bind(&a.filename)
            .bind(&a.media_type)
            .bind(a.size_bytes)
            .execute(&mut *conn)
            .await?;
    }

    let mut metadata = origin;
    metadata["source_id"] = json!(source_id);
    metadata["attachments"] = json!(email.attachments.len());
    audit::record(
        &mut *conn,
        Entry {
            workspace_id,
            actor: Actor::System,
            action: "message.received",
            entity_type: "message",
            entity_id: &message_id,
            message_id: Some(&message_id),
            metadata,
        },
    )
    .await?;
    jobs::enqueue(
        &mut *conn,
        kind::CLASSIFY_EMAIL,
        &format!("classify:{message_id}:{TAXONOMY_VERSION}"),
        Some(workspace_id),
        &json!({ "message_id": message_id }),
    )
    .await?;
    Ok(Some(message_id))
}
