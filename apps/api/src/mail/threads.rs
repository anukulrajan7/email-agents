//! Thread-level questions other features ask, e.g. "was this conversation already answered?".

use sqlx::SqlitePool;

/// True when someone already replied in the thread after `after_ms`: an outbound message in the
/// same thread, or a draft (for a message in that thread) that was sent.
///
/// `thread_key` is either our thread id (`email_threads.id`) or the provider's thread id
/// (`email_threads.provider_thread_id`, e.g. the Gmail threadId).
pub async fn thread_has_reply_after(
    db: &SqlitePool,
    workspace_id: &str,
    thread_key: &str,
    after_ms: i64,
) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (
           SELECT 1 FROM email_messages m
           JOIN email_threads t ON t.id = m.thread_id
           WHERE m.workspace_id = ?1 AND (t.id = ?2 OR t.provider_thread_id = ?2)
             AND m.direction = 'outbound' AND m.suppressed = 0 AND m.received_at > ?3
         ) OR EXISTS (
           SELECT 1 FROM drafts d
           JOIN email_messages m ON m.id = d.message_id
           JOIN email_threads t ON t.id = m.thread_id
           WHERE d.workspace_id = ?1 AND (t.id = ?2 OR t.provider_thread_id = ?2)
             AND d.status = 'sent' AND d.updated_at > ?3
         )",
    )
    .bind(workspace_id)
    .bind(thread_key)
    .bind(after_ms)
    .fetch_one(db)
    .await
}
