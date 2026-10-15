use super::*;

/// An email that never went out (stopped, blocked) is not waiting on anyone: close what watched it.
pub async fn close_for_outbound(conn: &mut SqliteConnection, ws: &str, outbound_id: &str, note: &str) -> sqlx::Result<()> {
    let now = now_ms();
    let ids: Vec<String> = sqlx::query_scalar(
        "UPDATE watches SET status = 'closed', last_update_at = ?1 WHERE workspace_id = ?2 AND outbound_email_id = ?3 AND status IN ('waiting','nudged') RETURNING id",
    )
    .bind(now)
    .bind(ws)
    .bind(outbound_id)
    .fetch_all(&mut *conn)
    .await?;
    for id in ids {
        sqlx::query("UPDATE support_tickets SET status = 'resolved', last_summary = ?1, last_update_at = ?2 WHERE workspace_id = ?3 AND watch_id = ?4")
            .bind(note)
            .bind(now)
            .bind(ws)
            .bind(id)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// After an in-app message was stopped or held back: when nothing of that conversation reached anyone, stop watching it.
pub async fn close_if_nothing_delivered(conn: &mut SqliteConnection, ws: &str, message_id: &str) -> sqlx::Result<()> {
    let row: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT thread_key, sender_user_id FROM in_app_messages WHERE id = ?1 AND workspace_id = ?2").bind(message_id).bind(ws).fetch_optional(&mut *conn).await?;
    let Some((key, sender)) = row else { return Ok(()) };
    let left: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM in_app_messages WHERE workspace_id = ?1 AND thread_key = ?2 AND sender_user_id IS ?3 AND cancelled_at IS NULL",
    )
    .bind(ws)
    .bind(&key)
    .bind(&sender)
    .fetch_one(&mut *conn)
    .await?;
    if left == 0 {
        sqlx::query("UPDATE watches SET status = 'closed', last_update_at = ?1 WHERE workspace_id = ?2 AND thread_key = ?3 AND channel = 'in_app' AND status IN ('waiting','nudged')")
            .bind(now_ms())
            .bind(ws)
            .bind(key)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}
