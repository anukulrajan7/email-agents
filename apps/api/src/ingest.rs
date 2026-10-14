//! Source-agnostic persistence of a normalized email. Every adapter (forwarding,
//! Nango Gmail, ...) ends here, so dedupe and pipeline kickoff behave identically.

use serde_json::json;
use sqlx::SqliteConnection;

use crate::{
    audit::{self, Actor, Entry},
    db::{new_id, now_ms},
    domain::{NormalizedEmail, TAXONOMY_VERSION},
    jobs::{self, kind},
    memory, text_clean,
};

/// True for mail the connected mailbox itself sent: Gmail's SENT label, or a sender that is the
/// source's own mailbox address.
async fn is_outbound(conn: &mut SqliteConnection, source_id: &str, email: &NormalizedEmail) -> sqlx::Result<bool> {
    if email.outbound {
        return Ok(true);
    }
    let mailbox: Option<String> =
        sqlx::query_scalar("SELECT mailbox_address FROM email_sources WHERE id = ?1").bind(source_id).fetch_optional(&mut *conn).await?.flatten();
    Ok(mailbox.is_some_and(|m| !m.is_empty() && m.eq_ignore_ascii_case(&email.sender_address)))
}

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

    let suppressed = crate::staffing::policy::should_suppress(conn, source_id, workspace_id, email).await?;

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

    let outbound = is_outbound(&mut *conn, source_id, email).await?;
    let body_text = text_clean::clean_body(&email.body_text, email.body_html.as_deref());
    let message_id = new_id("msg");
    sqlx::query(
        "INSERT INTO email_messages
           (id, workspace_id, source_id, thread_id, source_message_id, rfc_message_id, sender_address,
            original_sender_address, recipients_json, subject, body_text, body_html, received_at, created_at, processing_status,
            content_complete, sender_name, cc_json, direction, bulk_signals)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?18, ?15, ?16, ?17, ?19, ?20)",
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
    .bind(&body_text)
    .bind(&email.body_html)
    .bind(email.received_at)
    .bind(now)
    .bind(!email.preview_only)
    .bind(&email.sender_name)
    .bind(json!(email.cc).to_string())
    .bind(if outbound { "processed" } else { "normalized" })
    .bind(if outbound { "outbound" } else { "inbound" })
    .bind(json!(email.bulk_signals).to_string())
    .execute(&mut *conn)
    .await?;

    if suppressed {
        sqlx::query("UPDATE email_messages SET suppressed = 1 WHERE id = ?1").bind(&message_id).execute(&mut *conn).await?;
        // Metadata rows only, so a later release can read the files; nothing is extracted while suppressed.
        for a in &email.attachments {
            sqlx::query("INSERT INTO attachments (id, message_id, filename, media_type, size_bytes, provider_attachment_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")
                .bind(new_id("att"))
                .bind(&message_id)
                .bind(&a.filename)
                .bind(&a.media_type)
                .bind(a.size_bytes)
                .bind(&a.provider_ref)
                .execute(&mut *conn)
                .await?;
        }
        tracing::warn!(%message_id, %source_id, "billable policy: mail from a billable person outside the HR mailbox stored as suppressed");
        audit::record(
            &mut *conn,
            Entry {
                workspace_id,
                actor: Actor::System,
                action: "message.suppressed",
                entity_type: "message",
                entity_id: &message_id,
                message_id: Some(&message_id),
                metadata: json!({ "source_id": source_id, "reason": "billable_policy" }),
            },
        )
        .await?;
        return Ok(Some(message_id));
    }

    if !outbound {
        memory::record_contact(&mut *conn, workspace_id, &email.sender_address, email.sender_name.as_deref(), email.received_at)
            .await?;
        crate::agents::triggers::wake_for_thread(&mut *conn, workspace_id, &thread_id, &message_id).await?;
    }

    for a in &email.attachments {
        sqlx::query(
            "INSERT INTO attachments (id, message_id, filename, media_type, size_bytes, provider_attachment_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
            .bind(new_id("att"))
            .bind(&message_id)
            .bind(&a.filename)
            .bind(&a.media_type)
            .bind(a.size_bytes)
            .bind(&a.provider_ref)
            .execute(&mut *conn)
            .await?;
    }
    if !outbound && !email.attachments.is_empty() {
        jobs::attachment_jobs::enqueue_extract(&mut *conn, workspace_id, &message_id, false).await?;
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
    // Our own sent mail is kept as evidence of replies; it is never triaged, analysed or drafted against.
    if !outbound {
        jobs::enqueue(
            &mut *conn,
            kind::CLASSIFY_EMAIL,
            &format!("classify:{message_id}:{TAXONOMY_VERSION}"),
            Some(workspace_id),
            &json!({ "message_id": message_id }),
        )
        .await?;
    }
    Ok(Some(message_id))
}

/// Upgrades an already-stored message whose body was only a preview (e.g. Gmail snippet)
/// and re-queues labelling, since the first label was judged on less text. Returns the
/// message ID when something changed.
pub async fn refresh_content(
    conn: &mut SqliteConnection,
    source_id: &str,
    email: &NormalizedEmail,
) -> sqlx::Result<Option<String>> {
    if email.preview_only {
        return Ok(None);
    }
    let outbound = is_outbound(&mut *conn, source_id, email).await?;
    let body_text = text_clean::clean_body(&email.body_text, email.body_html.as_deref());
    let updated: Option<(String, String, String)> = sqlx::query_as(
        "UPDATE email_messages SET body_text = ?1, body_html = COALESCE(?2, body_html), content_complete = 1, text_clean_v = 1,
                sender_name = COALESCE(?5, sender_name), cc_json = ?6,
                bulk_signals = CASE WHEN ?8 != '[]' THEN ?8 ELSE bulk_signals END,
                direction = CASE WHEN ?7 THEN 'outbound' ELSE direction END,
                processing_status = CASE WHEN ?7 THEN 'processed' ELSE processing_status END
         WHERE source_id = ?3 AND source_message_id = ?4 AND content_complete = 0 AND suppressed = 0
         RETURNING id, workspace_id, direction",
    )
    .bind(&body_text)
    .bind(&email.body_html)
    .bind(source_id)
    .bind(&email.source_message_id)
    .bind(&email.sender_name)
    .bind(json!(email.cc).to_string())
    .bind(outbound)
    .bind(json!(email.bulk_signals).to_string())
    .fetch_optional(&mut *conn)
    .await?;
    let Some((message_id, workspace_id, direction)) = updated else { return Ok(None) };
    let has_attachments: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM attachments WHERE message_id = ?1")
        .bind(&message_id)
        .fetch_one(&mut *conn)
        .await?;
    if has_attachments == 0 {
        for a in &email.attachments {
            sqlx::query(
                "INSERT INTO attachments (id, message_id, filename, media_type, size_bytes, provider_attachment_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
                .bind(new_id("att"))
                .bind(&message_id)
                .bind(&a.filename)
                .bind(&a.media_type)
                .bind(a.size_bytes)
                .bind(&a.provider_ref)
                .execute(&mut *conn)
                .await?;
        }
        if direction == "inbound" && !email.attachments.is_empty() {
            jobs::attachment_jobs::enqueue_extract(&mut *conn, &workspace_id, &message_id, false).await?;
        }
    }
    if direction == "inbound" {
        jobs::enqueue(
            &mut *conn,
            kind::CLASSIFY_EMAIL,
            &format!("classify:{message_id}:{TAXONOMY_VERSION}:content-refresh"),
            Some(&workspace_id),
            &json!({ "message_id": message_id }),
        )
        .await?;
    }
    Ok(Some(message_id))
}
