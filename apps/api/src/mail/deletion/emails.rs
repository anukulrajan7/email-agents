use super::*;

/// 404 unless every email is visible to the user and deletable by them: inside their wall, and (for operators)
/// either in a shared mailbox or in their own. Admins may delete from any mailbox of the workspace.
/// Nothing is revealed about ids the user may not touch.
pub async fn authorize(db: &SqlitePool, ws: &str, user: &AuthUser, ids: &[String]) -> AppResult<()> {
    let access = resolve(db, ws, user).await?;
    let wall = crate::hr::team::scope::wall(db, ws, &access).await?.map(|w| json!(w).to_string());
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT m.id, s.connected_by_user_id FROM email_messages m JOIN email_sources s ON s.id = m.source_id
         WHERE m.workspace_id = ?1 AND m.suppressed = 0 AND m.id IN (SELECT value FROM json_each(?2))
           AND (?3 IS NULL OR m.source_id IN (SELECT value FROM json_each(?3)))",
    )
    .bind(ws)
    .bind(ids_json(ids))
    .bind(wall)
    .fetch_all(db)
    .await?;
    let found: HashSet<&str> = rows.iter().map(|(id, _)| id.as_str()).collect();
    if ids.iter().any(|id| !found.contains(id.as_str())) {
        return Err(AppError::NotFound);
    }
    let someone_elses = |owner: &Option<String>| owner.as_deref().is_some_and(|o| o != user.id);
    if access.level == Level::Operator && rows.iter().any(|(_, owner)| someone_elses(owner)) {
        return Err(AppError::NotFound);
    }
    Ok(())
}

/// The whole conversation an email belongs to, suppressed mail included (same mailbox, same permission).
pub async fn thread_of(db: &SqlitePool, ws: &str, message_id: &str) -> AppResult<Vec<String>> {
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM email_messages WHERE workspace_id = ?1
           AND thread_id = (SELECT thread_id FROM email_messages WHERE id = ?2 AND workspace_id = ?1)
         ORDER BY received_at, id",
    )
    .bind(ws)
    .bind(message_id)
    .fetch_all(db)
    .await?;
    if ids.is_empty() { Err(AppError::NotFound) } else { Ok(ids) }
}

/// Emails of this list with a reply being sent at this moment.
pub async fn busy_ids(db: &SqlitePool, ws: &str, ids: &[String]) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar(
        "SELECT DISTINCT d.message_id FROM drafts d WHERE d.workspace_id = ?1 AND d.message_id IN (SELECT value FROM json_each(?2))
           AND d.status != 'sent' AND d.send_attempt_id IS NOT NULL",
    )
    .bind(ws)
    .bind(ids_json(ids))
    .fetch_all(db)
    .await
}

/// Replies a person scheduled for these emails; deleting the emails cancels them.
pub async fn scheduled_replies(db: &SqlitePool, ws: &str, ids: &[String]) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM drafts WHERE workspace_id = ?1 AND message_id IN (SELECT value FROM json_each(?2))
           AND status != 'sent' AND send_at IS NOT NULL AND send_requested_by IS NOT NULL",
    )
    .bind(ws)
    .bind(ids_json(ids))
    .fetch_one(db)
    .await
}

/// Deletes in short transactions of `BATCH` emails each. All-or-nothing about busy replies: if any email has a
/// reply being sent right now, nothing is deleted. Idempotent: a retry skips what is already gone.
pub async fn delete_emails(db: &SqlitePool, ws: &str, ids: &[String], opts: Opts<'_>) -> Result<Removed, DeleteError> {
    if !busy_ids(db, ws, ids).await?.is_empty() {
        return Err(DeleteError::Busy);
    }
    let mut total = Removed::default();
    for chunk in ids.chunks(BATCH) {
        let prep = prepare(db, ws, chunk).await?;
        let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
        total.add(delete_batch(&mut tx, ws, chunk, opts, &prep).await?);
        tx.commit().await?;
    }
    Ok(total)
}

/// Removes tombstones old enough that no sync window can bring the email back.
pub async fn prune_tombstones(db: &SqlitePool) -> sqlx::Result<u64> {
    Ok(sqlx::query("DELETE FROM deleted_messages WHERE deleted_at < ?1").bind(now_ms() - TOMBSTONE_TTL_MS).execute(db).await?.rows_affected())
}
