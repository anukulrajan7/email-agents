//! Maintenance jobs over stored messages: re-clean old plain text, re-fetch the HTML part of Gmail
//! mail that was stored without one, and learn which messages the mailbox itself sent.
//! All are idempotent and bounded per run.

use std::sync::Arc;

use reqwest::Method;
use serde_json::{Value, json};
use tokio::{sync::Semaphore, task::JoinSet};

use super::{handlers::JobError, kind};
use crate::{
    platform::db::now_ms,
    http::AppState,
    jobs,
    mail::sources::nango,
    mail::text_clean,
};

const BATCH: i64 = 200;
const MAX_REFETCH_TRIES: i64 = 3;
const REFETCH_DONE: i64 = 99;
const FETCH_CONCURRENCY: usize = 8;

/// Re-cleans stored text bodies that predate text cleaning (`text_clean_v = 0`).
pub async fn clean_stored_text(state: &AppState) -> Result<(), JobError> {
    let rows: Vec<(String, String, String, Option<String>)> = sqlx::query_as(
        "SELECT id, workspace_id, body_text, body_html FROM email_messages WHERE text_clean_v = 0 ORDER BY id LIMIT ?1",
    )
    .bind(BATCH)
    .fetch_all(&state.db)
    .await?;
    let full_batch = rows.len() as i64 == BATCH;
    let last_id = rows.last().map(|r| r.0.clone());
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    for (id, workspace_id, text, html) in rows {
        let cleaned = text_clean::clean_body(&text, html.as_deref());
        sqlx::query("UPDATE email_messages SET body_text = ?1, text_clean_v = 1 WHERE id = ?2")
            .bind(&cleaned)
            .bind(&id)
            .execute(&mut *tx)
            .await?;
        // Search must see the cleaned text, not the junk.
        if cleaned != text {
            super::knowledge_jobs::enqueue_reindex_message(&mut tx, &workspace_id, &id, "clean_v1").await?;
        }
    }
    if let (true, Some(last)) = (full_batch, last_id) {
        jobs::enqueue(&mut tx, kind::CLEAN_STORED_TEXT, &format!("clean_stored_text:v1:after:{last}"), None, &json!({})).await?;
    }
    tx.commit().await?;
    state.jobs_notify.notify_one();
    Ok(())
}

/// Worker maintenance hook: queues the one-time re-clean while stored rows still need it. The fixed
/// dedupe key makes it a no-op once that job has run; each run chains the next batch itself.
pub async fn schedule_text_clean(state: &AppState) -> sqlx::Result<()> {
    let pending: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM email_messages WHERE text_clean_v = 0)")
        .fetch_one(&state.db)
        .await?;
    if pending {
        let mut conn = state.db.acquire().await?;
        jobs::enqueue(&mut conn, kind::CLEAN_STORED_TEXT, "clean_stored_text:v1", None, &json!({})).await?;
    }
    Ok(())
}

/// Queues an HTML re-fetch for the source when legacy Gmail rows without HTML remain.
pub async fn enqueue_html_refetch(state: &AppState, workspace_id: &str, source_id: &str) -> sqlx::Result<()> {
    let pending: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM email_messages WHERE source_id = ?1 AND body_html IS NULL AND html_refetch_tries < ?2)",
    )
    .bind(source_id)
    .bind(MAX_REFETCH_TRIES)
    .fetch_one(&state.db)
    .await?;
    if pending {
        let mut conn = state.db.acquire().await?;
        let bucket = now_ms() / 300_000;
        jobs::enqueue_with_attempts(
            &mut conn,
            kind::REFETCH_GMAIL_HTML,
            &format!("refetch_html:{source_id}:{bucket}"),
            Some(workspace_id),
            &json!({ "source_id": source_id }),
            jobs::PROVIDER_MAX_ATTEMPTS,
        )
        .await?;
    }
    Ok(())
}

/// Re-fetches up to 200 Gmail messages stored without HTML (full format, via the Nango proxy) and
/// fills `body_html`, re-cleans the text, and refreshes the header fields. Each row is tried at most
/// 3 times; a message that really has no HTML part is marked done.
pub async fn refetch_gmail_html(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let source_id = payload
        .get("source_id")
        .and_then(Value::as_str)
        .ok_or_else(|| JobError::Permanent("payload missing source_id".into()))?
        .to_string();
    let src: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT workspace_id, external_connection_id, provider_config_key FROM email_sources
         WHERE id = ?1 AND kind = 'nango_gmail' AND status != 'disconnected'",
    )
    .bind(&source_id)
    .fetch_optional(&state.db)
    .await?;
    let Some((workspace_id, Some(connection_id), Some(pck))) = src else { return Ok(()) };

    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, source_message_id FROM email_messages
         WHERE source_id = ?1 AND body_html IS NULL AND html_refetch_tries < ?2
           AND id IN (SELECT id FROM email_messages WHERE source_id = ?1 ORDER BY received_at DESC LIMIT 100)
         ORDER BY received_at DESC LIMIT ?3",
    )
    .bind(&source_id)
    .bind(MAX_REFETCH_TRIES)
    .bind(BATCH)
    .fetch_all(&state.db)
    .await?;
    if rows.is_empty() {
        return Ok(());
    }
    let full_batch = rows.len() as i64 == BATCH;

    let permits = Arc::new(Semaphore::new(FETCH_CONCURRENCY));
    let mut tasks = JoinSet::new();
    for (message_id, gmail_id) in rows {
        let (nango, permits) = (state.nango.clone(), permits.clone());
        let (conn, pck) = (connection_id.clone(), pck.clone());
        tasks.spawn(async move {
            let _permit = permits.acquire_owned().await.ok();
            let full = nango.gmail_message_full(&conn, &pck, &gmail_id).await;
            (message_id, full)
        });
    }
    let (mut filled, mut failed) = (0usize, 0usize);
    let mut last_id = String::new();
    while let Some(done) = tasks.join_next().await {
        let Ok((message_id, full)) = done else { continue };
        last_id = last_id.max(message_id.clone());
        let mapped = full.ok().and_then(|f| nango::map_gmail_record(&f, state.config.max_body_chars));
        let Some(mapped) = mapped else {
            sqlx::query("UPDATE email_messages SET html_refetch_tries = html_refetch_tries + 1 WHERE id = ?1")
                .bind(&message_id)
                .execute(&state.db)
                .await?;
            failed += 1;
            continue;
        };
        let email = mapped.email;
        let text = text_clean::clean_body(&email.body_text, email.body_html.as_deref());
        sqlx::query(
            "UPDATE email_messages SET
               body_html = COALESCE(?2, body_html),
               body_text = CASE WHEN ?3 != '' AND NOT ?7 THEN ?3 ELSE body_text END,
               text_clean_v = 1,
               cc_json = CASE WHEN ?4 != '[]' THEN ?4 ELSE cc_json END,
               sender_name = COALESCE(?5, sender_name),
               direction = CASE WHEN ?6 THEN 'outbound' ELSE direction END,
               processing_status = CASE WHEN ?6 AND processing_status IN ('pending','normalized','classified')
                                        THEN 'processed' ELSE processing_status END,
               html_refetch_tries = ?8
             WHERE id = ?1",
        )
        .bind(&message_id)
        .bind(&email.body_html)
        .bind(&text)
        .bind(json!(email.cc).to_string())
        .bind(&email.sender_name)
        .bind(email.outbound)
        .bind(email.preview_only)
        .bind(REFETCH_DONE)
        .execute(&state.db)
        .await?;
        if email.body_html.is_some() && !email.outbound {
            refresh_understanding(state, &workspace_id, &message_id).await?;
        }
        filled += usize::from(email.body_html.is_some());
    }
    tracing::info!(%source_id, filled, failed, "gmail html refetch finished");
    if full_batch {
        let mut conn = state.db.acquire().await?;
        jobs::enqueue_with_attempts(
            &mut conn,
            kind::REFETCH_GMAIL_HTML,
            &format!("refetch_html:{source_id}:after:{last_id}"),
            Some(&workspace_id),
            &json!({ "source_id": source_id }),
            jobs::PROVIDER_MAX_ATTEMPTS,
        )
        .await?;
        state.jobs_notify.notify_one();
    }
    Ok(())
}

/// Learns the mailbox's own address once, then marks mail it sent as outbound. Failures are
/// logged only: the Gmail SENT label still catches new outgoing mail.
pub async fn learn_mailbox_address(state: &AppState, source_id: &str, connection_id: &str, pck: &str) {
    let result: Result<(), String> = async {
        let known: Option<String> = sqlx::query_scalar("SELECT mailbox_address FROM email_sources WHERE id = ?1")
            .bind(source_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| e.to_string())?
            .flatten();
        let address = match known.filter(|a| !a.is_empty()) {
            Some(a) => a,
            None => {
                let profile = state
                    .nango
                    .gmail_call(Method::GET, connection_id, pck, "profile", None)
                    .await
                    .map_err(|e| e.to_string())?;
                let a = profile
                    .get("emailAddress")
                    .and_then(Value::as_str)
                    .map(|a| a.trim().to_lowercase())
                    .filter(|a| a.contains('@'))
                    .ok_or("profile had no emailAddress")?;
                sqlx::query("UPDATE email_sources SET mailbox_address = ?1 WHERE id = ?2")
                    .bind(&a)
                    .bind(source_id)
                    .execute(&state.db)
                    .await
                    .map_err(|e| e.to_string())?;
                a
            }
        };
        mark_sent_by_mailbox(state, source_id, &address).await.map_err(|e| e.to_string())
    }
    .await;
    if let Err(e) = result {
        tracing::warn!(%source_id, error = %e, "could not learn the mailbox address");
    }
}

/// Existing rows sent from the mailbox itself become outbound (idempotent).
async fn mark_sent_by_mailbox(state: &AppState, source_id: &str, address: &str) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE email_messages SET direction = 'outbound',
           processing_status = CASE WHEN processing_status IN ('pending','normalized','classified') THEN 'processed'
                                    ELSE processing_status END
         WHERE source_id = ?1 AND direction = 'inbound' AND sender_address = ?2",
    )
    .bind(source_id)
    .bind(address)
    .execute(&state.db)
    .await?;
    Ok(())
}

/// True for messages the mailbox sent: they are never classified, analysed or drafted against.
pub async fn is_outbound(state: &AppState, message_id: &str) -> sqlx::Result<bool> {
    let direction: Option<String> =
        sqlx::query_scalar("SELECT direction FROM email_messages WHERE id = ?1").bind(message_id).fetch_optional(&state.db).await?;
    Ok(direction.as_deref() == Some("outbound"))
}

/// The full body replaced a stored snippet: re-index it and re-read what the sender asks
/// (the old insight may have been built from the snippet alone).
async fn refresh_understanding(state: &AppState, workspace_id: &str, message_id: &str) -> Result<(), JobError> {
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    super::knowledge_jobs::enqueue_reindex_message(&mut tx, workspace_id, message_id, "html_refetch").await?;
    let had_insight = sqlx::query("DELETE FROM message_insights WHERE message_id = ?1").bind(message_id).execute(&mut *tx).await?.rows_affected() > 0;
    if had_insight {
        jobs::enqueue(&mut tx, kind::ANALYZE_MESSAGE, &format!("analyze:{message_id}:refetch"), Some(workspace_id), &json!({ "message_id": message_id }))
            .await?;
    }
    tx.commit().await?;
    Ok(())
}
