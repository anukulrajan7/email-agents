use super::*;

/// Starts watching and schedules the one nudge.
pub async fn create(conn: &mut SqliteConnection, w: NewWatch<'_>) -> sqlx::Result<String> {
    let id = new_id("wch");
    let now = now_ms();
    let hours = w.nudge_hours.clamp(1, MAX_NUDGE_HOURS);
    let nudge_at = now + hours * 3_600_000;
    sqlx::query(
        "INSERT INTO watches (id, workspace_id, user_id, agent_id, kind, title, channel, counterpart, status, outbound_email_id, thread_key,
                              source_id, case_id, question, nudge_hours, nudge_after_at, last_update_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'waiting', ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?16)",
    )
    .bind(&id)
    .bind(w.ws)
    .bind(w.user_id)
    .bind(w.agent_id)
    .bind(w.kind)
    .bind(sentence(w.title, 160))
    .bind(w.channel)
    .bind(sentence(w.counterpart, 120))
    .bind(w.outbound_email_id)
    .bind(w.thread_key)
    .bind(w.source_id)
    .bind(w.case_id)
    .bind(sentence(w.question, 500))
    .bind(hours)
    .bind(nudge_at)
    .bind(now)
    .execute(&mut *conn)
    .await?;
    if let Some(out) = w.outbound_email_id {
        sqlx::query("UPDATE outbound_emails SET watch_id = ?1 WHERE id = ?2").bind(&id).bind(out).execute(&mut *conn).await?;
    }
    enqueue_nudge(conn, w.ws, &id, nudge_at).await?;
    Ok(id)
}

async fn enqueue_nudge(conn: &mut SqliteConnection, ws: &str, watch_id: &str, at: i64) -> sqlx::Result<()> {
    jobs::enqueue_at(
        conn,
        kind::WATCH_NUDGE,
        &format!("watch_nudge:{watch_id}"),
        Some(ws),
        &json!({ "workspace_id": ws, "watch_id": watch_id }),
        jobs::DEFAULT_MAX_ATTEMPTS,
        at,
    )
    .await?;
    Ok(())
}

/// The email went out: its Gmail thread is what a reply will arrive in. A ticket starts waiting.
pub async fn email_sent(conn: &mut SqliteConnection, ws: &str, outbound_id: &str, source_id: &str, thread: Option<&str>) -> sqlx::Result<()> {
    let now = now_ms();
    let watch: Option<String> = sqlx::query_scalar(
        "UPDATE watches SET source_id = ?1, provider_thread_id = COALESCE(?2, provider_thread_id), last_update_at = ?3
         WHERE workspace_id = ?4 AND outbound_email_id = ?5 RETURNING id",
    )
    .bind(source_id)
    .bind(thread)
    .bind(now)
    .bind(ws)
    .bind(outbound_id)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(watch_id) = watch {
        sqlx::query("UPDATE support_tickets SET status = 'waiting', last_update_at = ?1 WHERE workspace_id = ?2 AND watch_id = ?3 AND status = 'open'")
            .bind(now)
            .bind(ws)
            .bind(&watch_id)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}
