use super::*;

/// What disconnecting a mailbox will permanently delete from Post Room.
pub async fn source_counts(db: &SqlitePool, ws: &str, source_id: &str) -> sqlx::Result<Removed> {
    let mut conn = db.acquire().await?;
    let (emails, attachments, drafts, summaries, chunks): (i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM email_messages WHERE workspace_id = ?1 AND source_id = ?2),
                (SELECT COUNT(*) FROM attachments a JOIN email_messages m ON m.id = a.message_id WHERE m.workspace_id = ?1 AND m.source_id = ?2),
                (SELECT COUNT(*) FROM drafts d JOIN email_messages m ON m.id = d.message_id WHERE m.workspace_id = ?1 AND m.source_id = ?2),
                (SELECT COUNT(*) FROM message_insights i JOIN email_messages m ON m.id = i.message_id WHERE m.workspace_id = ?1 AND m.source_id = ?2),
                (SELECT COUNT(*) FROM knowledge_chunks c JOIN email_messages m ON m.id = c.message_id WHERE m.workspace_id = ?1 AND m.source_id = ?2)",
    )
    .bind(ws)
    .bind(source_id)
    .fetch_one(&mut *conn)
    .await?;
    Ok(Removed { emails, attachments, drafts, summaries, chunks, cases_cancelled: 0 })
}

/// `(scheduled replies a person set up, support tickets still open or waiting)` that disconnecting a mailbox ends.
pub async fn source_loose_ends(db: &SqlitePool, ws: &str, source_id: &str) -> sqlx::Result<(i64, i64)> {
    sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM drafts d JOIN email_messages m ON m.id = d.message_id
                 WHERE m.workspace_id = ?1 AND m.source_id = ?2 AND d.status != 'sent' AND d.send_at IS NOT NULL AND d.send_requested_by IS NOT NULL),
                (SELECT COUNT(*) FROM support_tickets t JOIN watches w ON w.id = t.watch_id
                 WHERE w.workspace_id = ?1 AND w.source_id = ?2 AND t.status IN ('open', 'waiting'))",
    )
    .bind(ws)
    .bind(source_id)
    .fetch_one(db)
    .await
}

/// Marks the source as being purged and queues the background job. Runs inside the caller's transaction
/// (the same one that disconnects the mailbox). Safe to call again: a purge already running keeps its
/// original totals.
pub async fn start_purge(conn: &mut SqliteConnection, ws: &str, source_id: &str, requested_by: &str) -> sqlx::Result<()> {
    let started = sqlx::query(
        "UPDATE email_sources SET purge_status = 'running', purge_done = 0,
                purge_total = (SELECT COUNT(*) FROM email_messages WHERE source_id = ?1),
                purge_counts_json = ?3, purge_started_at = ?4, purge_error = NULL
         WHERE id = ?1 AND workspace_id = ?2 AND (purge_status IS NULL OR purge_status = 'failed')",
    )
    .bind(source_id)
    .bind(ws)
    .bind(counts_in_tx(conn, ws, source_id).await?)
    .bind(now_ms())
    .execute(&mut *conn)
    .await?
    .rows_affected()
        == 1;
    if !started {
        let running: Option<String> =
            sqlx::query_scalar("SELECT purge_status FROM email_sources WHERE id = ?1 AND workspace_id = ?2").bind(source_id).bind(ws).fetch_optional(&mut *conn).await?.flatten();
        if running.as_deref() != Some("running") {
            return Ok(());
        }
    }
    jobs::enqueue_with_attempts(
        conn,
        kind::PURGE_SOURCE,
        &if started { format!("purge_source:{source_id}:start:{}", now_ms()) } else { format!("purge_source:{source_id}:{}", now_ms() / 60_000) },
        Some(ws),
        &json!({ "source_id": source_id, "requested_by": requested_by }),
        jobs::PROVIDER_MAX_ATTEMPTS,
    )
    .await?;
    Ok(())
}

async fn counts_in_tx(conn: &mut SqliteConnection, ws: &str, source_id: &str) -> sqlx::Result<String> {
    let (emails, attachments, drafts, summaries): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM email_messages WHERE workspace_id = ?1 AND source_id = ?2),
                (SELECT COUNT(*) FROM attachments a JOIN email_messages m ON m.id = a.message_id WHERE m.workspace_id = ?1 AND m.source_id = ?2),
                (SELECT COUNT(*) FROM drafts d JOIN email_messages m ON m.id = d.message_id WHERE m.workspace_id = ?1 AND m.source_id = ?2),
                (SELECT COUNT(*) FROM message_insights i JOIN email_messages m ON m.id = i.message_id WHERE m.workspace_id = ?1 AND m.source_id = ?2)",
    )
    .bind(ws)
    .bind(source_id)
    .fetch_one(conn)
    .await?;
    Ok(json!({ "emails": emails, "attachments": attachments, "drafts": drafts, "summaries": summaries }).to_string())
}

#[derive(Debug, PartialEq, Eq)]
pub enum PurgeOutcome {
    /// Everything is deleted and the source is cleaned up.
    Done,
    /// The time budget ran out with work left.
    MoreToDo,
    /// Only emails or sends that are mid-send are left; try again shortly.
    Waiting,
}

#[derive(Debug)]
pub struct PurgeRun {
    pub outcome: PurgeOutcome,
    pub batches: usize,
    /// The longest a batch held the write lock.
    pub max_lock: Duration,
    /// How long each batch held it.
    pub locks: Vec<Duration>,
}

/// Deletes the mailbox's emails batch by batch (reads first, then one short write transaction per batch),
/// then its source-scoped rows. Emails with a reply being sent right now are left for a later run.
pub async fn purge_run(state: &AppState, ws: &str, source_id: &str, requested_by: Option<&str>, budget: Duration, pause: Duration) -> Result<PurgeRun, JobError> {
    let started = Instant::now();
    let opts = Opts { actor: Actor::System, tombstone: false, audit: false };
    let mut run = PurgeRun { outcome: PurgeOutcome::Done, batches: 0, max_lock: Duration::ZERO, locks: vec![] };
    loop {
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT m.id FROM email_messages m WHERE m.source_id = ?1
               AND NOT EXISTS (SELECT 1 FROM drafts d WHERE d.message_id = m.id AND d.status != 'sent' AND d.send_attempt_id IS NOT NULL)
             ORDER BY m.id LIMIT ?2",
        )
        .bind(source_id)
        .bind(BATCH as i64)
        .fetch_all(&state.db)
        .await?;
        if ids.is_empty() {
            break;
        }
        let prep = prepare(&state.db, ws, &ids).await?;
        let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
        let locked = Instant::now();
        match delete_batch(&mut tx, ws, &ids, opts, &prep).await {
            Ok(_) => {}
            Err(DeleteError::Busy) => continue,
            Err(DeleteError::Db(e)) => return Err(e.into()),
        }
        sqlx::query(
            "UPDATE email_sources SET purge_done = MAX(0, COALESCE(purge_total, 0) - (SELECT COUNT(*) FROM email_messages WHERE source_id = ?1))
             WHERE id = ?1",
        )
        .bind(source_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        run.max_lock = run.max_lock.max(locked.elapsed());
        run.locks.push(locked.elapsed());
        run.batches += 1;
        if started.elapsed() >= budget {
            run.outcome = PurgeOutcome::MoreToDo;
            return Ok(run);
        }
        tokio::time::sleep(pause).await;
    }
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM email_messages WHERE source_id = ?1").bind(source_id).fetch_one(&state.db).await?;
    if left > 0 || !finish_purge(state, ws, source_id, requested_by).await? {
        run.outcome = PurgeOutcome::Waiting;
    }
    Ok(run)
}

/// The `purge_source` job. Resumable: a run that is cut off, runs out of budget or finds only busy sends is
/// continued by a new run. It never stays `running` for good: after `GIVE_UP_MS` it marks the purge failed.
pub async fn purge_source_job(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let source_id = payload
        .get("source_id")
        .and_then(Value::as_str)
        .ok_or_else(|| JobError::Permanent("payload missing source_id".into()))?
        .to_string();
    let requested_by = payload.get("requested_by").and_then(Value::as_str).map(str::to_string);
    let row: Option<(String, Option<String>, Option<i64>)> =
        sqlx::query_as("SELECT workspace_id, purge_status, purge_started_at FROM email_sources WHERE id = ?1").bind(&source_id).fetch_optional(&state.db).await?;
    let Some((ws, Some(status), started_at)) = row else { return Ok(()) };
    if status != "running" {
        return Ok(());
    }
    let run = purge_run(state, &ws, &source_id, requested_by.as_deref(), RUN_BUDGET, BATCH_PAUSE).await?;
    let next_at = match run.outcome {
        PurgeOutcome::Done => return Ok(()),
        PurgeOutcome::MoreToDo => now_ms(),
        PurgeOutcome::Waiting => {
            if now_ms() - started_at.unwrap_or_else(now_ms) > GIVE_UP_MS {
                purge_failed(&state.db, &source_id, "Some replies were still being sent, so the deletion could not finish. Disconnect again to retry.").await?;
                return Ok(());
            }
            now_ms() + BUSY_RETRY_MS
        }
    };
    let mut conn = state.db.acquire().await?;
    jobs::enqueue_at(
        &mut conn,
        kind::PURGE_SOURCE,
        &format!("purge_source:{source_id}:next:{next_at}"),
        Some(&ws),
        payload,
        jobs::PROVIDER_MAX_ATTEMPTS,
        next_at,
    )
    .await?;
    state.jobs_notify.notify_one();
    Ok(())
}

/// Stops a purge for good: the source is no longer blocked for mail, and the person can start it again.
pub async fn purge_failed(db: &SqlitePool, source_id: &str, message: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE email_sources SET purge_status = 'failed', purge_error = ?2 WHERE id = ?1 AND purge_status = 'running'")
        .bind(source_id)
        .bind(message)
        .execute(db)
        .await?;
    Ok(())
}

/// Removes what only existed because of the mailbox and records the result. One short transaction.
/// `false` = not yet: a send from this mailbox is in flight.
async fn finish_purge(state: &AppState, ws: &str, source_id: &str, requested_by: Option<&str>) -> sqlx::Result<bool> {
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let counts: Option<String> = sqlx::query_scalar(
        "SELECT purge_counts_json FROM email_sources WHERE id = ?1 AND workspace_id = ?2 AND purge_status = 'running'",
    )
    .bind(source_id)
    .bind(ws)
    .fetch_optional(&mut *tx)
    .await?
    .flatten();
    let Some(counts) = counts else { return Ok(true) };
    let remaining: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM email_messages WHERE source_id = ?1").bind(source_id).fetch_one(&mut *tx).await?;
    if remaining > 0 {
        return Ok(false);
    }
    let sending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM outbound_emails WHERE workspace_id = ?1 AND source_id = ?2 AND status != 'sent' AND send_attempt_id IS NOT NULL",
    )
    .bind(ws)
    .bind(source_id)
    .fetch_one(&mut *tx)
    .await?;
    if sending > 0 {
        return Ok(false);
    }
    let outbound: Vec<String> = sqlx::query_scalar("SELECT id FROM outbound_emails WHERE workspace_id = ?1 AND source_id = ?2")
        .bind(ws)
        .bind(source_id)
        .fetch_all(&mut *tx)
        .await?;
    for id in &outbound {
        sqlx::query(
            "UPDATE agent_approvals SET status = CASE WHEN status = 'pending' THEN 'declined' ELSE status END,
                    decided_at = CASE WHEN status = 'pending' THEN ?3 ELSE decided_at END,
                    note = CASE WHEN status = 'pending' THEN ?4 ELSE note END, body = '', input_json = '{}'
             WHERE workspace_id = ?1 AND instr(input_json, ?2) > 0",
        )
        .bind(ws)
        .bind(id)
        .bind(now_ms())
        .bind(CASE_NOTE)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM jobs WHERE status != 'running' AND instr(payload_json, ?1) > 0")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("DELETE FROM email_threads WHERE source_id = ?1").bind(source_id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM support_tickets WHERE watch_id IN (SELECT id FROM watches WHERE workspace_id = ?1 AND source_id = ?2)")
        .bind(ws)
        .bind(source_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM watches WHERE workspace_id = ?1 AND source_id = ?2").bind(ws).bind(source_id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM outbound_emails WHERE workspace_id = ?1 AND source_id = ?2").bind(ws).bind(source_id).execute(&mut *tx).await?;
    sqlx::query("UPDATE people SET source_id = NULL WHERE workspace_id = ?1 AND source_id = ?2").bind(ws).bind(source_id).execute(&mut *tx).await?;
    sqlx::query("UPDATE team_settings SET hr_source_id = NULL WHERE workspace_id = ?1 AND hr_source_id = ?2")
        .bind(ws)
        .bind(source_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM deleted_messages WHERE source_id = ?1").bind(source_id).execute(&mut *tx).await?;
    sqlx::query("UPDATE jobs SET status = 'dead', last_error = 'source purged' WHERE status = 'pending' AND json_extract(payload_json, '$.source_id') = ?1 AND kind != ?2")
        .bind(source_id)
        .bind(kind::PURGE_SOURCE)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE email_sources SET status = 'disconnected', purge_status = 'done', purge_done = purge_total, purged_at = ?3,
                sync_cursor = NULL, external_connection_id = NULL, provider_config_key = NULL, mailbox_address = NULL,
                inbound_alias = NULL, last_error = NULL, last_synced_at = NULL
         WHERE id = ?1 AND workspace_id = ?2",
    )
    .bind(source_id)
    .bind(ws)
    .bind(now_ms())
    .execute(&mut *tx)
    .await?;
    let counts: Value = serde_json::from_str(&counts).unwrap_or_else(|_| json!({}));
    audit::record(
        &mut tx,
        Entry {
            workspace_id: ws,
            actor: requested_by.map_or(Actor::System, Actor::User),
            action: "source.purged",
            entity_type: "email_source",
            entity_id: source_id,
            message_id: None,
            metadata: counts,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(true)
}

/// May this user disconnect (and purge) this mailbox? Admins any; a member their own personal mailbox.
/// Someone else's personal mailbox is a 404.
pub async fn authorize_source(db: &SqlitePool, ws: &str, user: &AuthUser, source_id: &str) -> AppResult<()> {
    let role = require_role(db, ws, user, Role::Operator).await?;
    let owner: Option<Option<String>> =
        sqlx::query_scalar("SELECT connected_by_user_id FROM email_sources WHERE id = ?1 AND workspace_id = ?2")
            .bind(source_id)
            .bind(ws)
            .fetch_optional(db)
            .await?;
    let owner = owner.ok_or(AppError::NotFound)?;
    if role == Role::Admin || owner.as_deref() == Some(user.id.as_str()) {
        return Ok(());
    }
    Err(if owner.is_some() { AppError::NotFound } else { AppError::Forbidden })
}
