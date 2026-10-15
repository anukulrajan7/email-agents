use super::*;

pub const DRY_RUN_MAX_MESSAGES: usize = 50;
/// A dry run that has not finished after this long may be asked for again.
const STALE_DRY_RUN_MS: i64 = 10 * 60 * 1000;

/// Queues the dry run. The spec must be valid; picked messages must be inbound mail the session may see
/// (a personal agent's session only sees its owner's mailbox). Claiming the run, the hourly limit and the
/// session budget are decided in one transaction, so a second request cannot start a second run.
pub async fn request_dry_run(state: &AppState, ws: &str, id: &str, message_ids: &[String]) -> AppResult<Session> {
    let session = get(&state.db, ws, id).await?.ok_or(AppError::NotFound)?;
    if !session.is_open() {
        return Err(AppError::Conflict("this session is closed; start a new one".into()));
    }
    let Some(spec) = session.spec.clone().filter(|_| session.issues.is_empty()) else {
        return Err(AppError::Conflict("fix the problems with the spec first; a dry run needs a valid spec".into()));
    };
    if message_ids.len() > DRY_RUN_MAX_MESSAGES {
        return Err(AppError::BadRequest(format!("pick at most {DRY_RUN_MAX_MESSAGES} emails")));
    }
    let owner = owner_of(state, ws, &session).await?;
    if !message_ids.is_empty() {
        let found: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM email_messages m WHERE m.workspace_id = ?1 AND m.direction = 'inbound' AND m.id IN (SELECT value FROM json_each(?2)) AND {}",
            crate::agents::designer::dry_run::owned_clause(3)
        ))
        .bind(ws)
        .bind(json!(message_ids).to_string())
        .bind(owner.as_deref())
        .fetch_one(&state.db)
        .await?;
        if found != message_ids.len() as i64 {
            return Err(AppError::BadRequest("one of those emails was not found".into()));
        }
    }
    if !crate::ai::ledger::within_budget(&state.db, state.config.ai_daily_budget_usd, ws).await {
        return Err(AppError::LimitReached("this workspace has used its AI budget for today; try again tomorrow".into()));
    }
    let hash = spec.hash();
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let claimed = sqlx::query(
        "UPDATE designer_sessions SET dry_run_status = 'running', dry_run_json = NULL, dry_run_spec_hash = ?1, updated_at = ?2, rev = rev + 1
         WHERE id = ?3 AND workspace_id = ?4 AND rev = ?5 AND status IN ('drafting', 'ready', 'failed') AND (dry_run_status != 'running' OR updated_at < ?6)",
    )
    .bind(&hash)
    .bind(now_ms())
    .bind(id)
    .bind(ws)
    .bind(session.rev)
    .bind(now_ms() - STALE_DRY_RUN_MS)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if claimed == 0 {
        drop(tx);
        let current = get(&state.db, ws, id).await?.ok_or(AppError::NotFound)?;
        return if current.dry_run_status == "running" { Ok(current) } else { Err(AppError::Conflict("this session changed; reload it and try again".into())) };
    }
    let call_id = reserve_call(&mut tx, ws, &session, "dry_run").await?;
    jobs::enqueue(
        &mut tx,
        jobs::kind::DESIGNER_DRY_RUN,
        &format!("{}:{id}:{}", jobs::kind::DESIGNER_DRY_RUN, now_ms()),
        Some(ws),
        &json!({"session_id": id, "message_ids": message_ids, "spec_hash": hash, "call_id": call_id}),
    )
    .await?;
    tx.commit().await?;
    state.jobs_notify.notify_one();
    get(&state.db, ws, id).await?.ok_or(AppError::NotFound)
}

/// The member whose mailbox this session may read: the owner of the personal agent it edits, else nobody in particular.
pub async fn owner_of(state: &AppState, ws: &str, session: &Session) -> AppResult<Option<String>> {
    let Some(agent_id) = &session.agent_id else { return Ok(None) };
    let agent = store::get_in(&state.db, ws, agent_id).await?.ok_or(AppError::NotFound)?;
    if agent.is_personal() { agent.owner_user_id.map(Some).ok_or(AppError::Forbidden) } else { Ok(None) }
}
