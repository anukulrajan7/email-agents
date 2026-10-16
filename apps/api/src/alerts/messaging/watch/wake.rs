use super::*;

const LISTENING: &str = "(w.status IN ('waiting','nudged') OR (w.kind = 'ticket' AND w.status = 'replied'))";

async fn wake(conn: &mut SqliteConnection, ws: &str, watch_ids: Vec<String>, what: &str, reference: &str) -> sqlx::Result<()> {
    for watch_id in watch_ids {
        jobs::enqueue(
            conn,
            kind::WATCH_REPLY,
            &format!("watch_reply:{watch_id}:{reference}"),
            Some(ws),
            &json!({ "workspace_id": ws, "watch_id": watch_id, "what": what, "ref": reference }),
        )
        .await?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Automatic {
    /// A delivery failure notice: the message did not arrive.
    Bounce,
    /// An out-of-office or other automatic answer: nobody has read it yet.
    AutoReply,
}

/// Is this mail a machine talking: a bounce, or an automatic answer? Neither is the answer someone is waiting for.
pub fn automatic(sender: &str, subject: &str, signals_json: &str) -> Option<Automatic> {
    let signals: Vec<String> = serde_json::from_str(signals_json).unwrap_or_default();
    let has = |s: &str| signals.iter().any(|x| x == s);
    let local = sender.split('@').next().unwrap_or_default().to_lowercase();
    if matches!(local.as_str(), "mailer-daemon" | "postmaster") || has(crate::mail::bulk::DELIVERY_STATUS) || crate::mail::bots::is_bounce_subject(subject) {
        return Some(Automatic::Bounce);
    }
    if is_automated_address(sender) || has(crate::mail::bulk::AUTO_SUBMITTED) || has(crate::mail::bulk::AUTO_REPLY) || crate::mail::bots::is_auto_reply_subject(subject) {
        return Some(Automatic::AutoReply);
    }
    None
}

/// An email arrived in a stored thread: a watch on that Gmail thread wakes. A bounce tells the owner instead,
/// and an automatic answer is ignored; both leave the watch waiting.
pub async fn on_inbound_email(conn: &mut SqliteConnection, ws: &str, source_id: &str, thread_id: &str, message_id: &str) -> sqlx::Result<()> {
    let head: Option<(String, String, String)> =
        sqlx::query_as("SELECT sender_address, subject, bulk_signals FROM email_messages WHERE id = ?1").bind(message_id).fetch_optional(&mut *conn).await?;
    let auto = head.and_then(|(sender, subject, signals)| automatic(&sender, &subject, &signals));
    if auto == Some(Automatic::AutoReply) {
        return Ok(());
    }
    let ids: Vec<String> = sqlx::query_scalar(&format!(
        "SELECT w.id FROM watches w JOIN email_threads t ON t.provider_thread_id = w.provider_thread_id AND t.source_id = w.source_id
         WHERE w.workspace_id = ?1 AND w.source_id = ?2 AND t.id = ?3 AND {LISTENING}"
    ))
    .bind(ws)
    .bind(source_id)
    .bind(thread_id)
    .fetch_all(&mut *conn)
    .await?;
    wake(conn, ws, ids, if auto == Some(Automatic::Bounce) { "bounce" } else { "email" }, message_id).await
}

/// A recipient answered an in-app message in this thread.
pub async fn wake_for_reply(conn: &mut SqliteConnection, ws: &str, thread_key: &str, reply_id: &str) -> sqlx::Result<()> {
    let ids: Vec<String> = sqlx::query_scalar(&format!(
        "SELECT w.id FROM watches w WHERE w.workspace_id = ?1 AND w.thread_key = ?2 AND w.channel = 'in_app' AND {LISTENING}"
    ))
    .bind(ws)
    .bind(thread_key)
    .fetch_all(&mut *conn)
    .await?;
    wake(conn, ws, ids, "in_app", reply_id).await
}

/// A case ended: whoever handed it to a desk hears about it.
pub async fn on_case_ended(conn: &mut SqliteConnection, ws: &str, case_id: &str) -> sqlx::Result<()> {
    let ids: Vec<String> =
        sqlx::query_scalar(&format!("SELECT w.id FROM watches w WHERE w.workspace_id = ?1 AND w.case_id = ?2 AND {LISTENING}"))
            .bind(ws)
            .bind(case_id)
            .fetch_all(&mut *conn)
            .await?;
    wake(conn, ws, ids, "case", case_id).await
}
