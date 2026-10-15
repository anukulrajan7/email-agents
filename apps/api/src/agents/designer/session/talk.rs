use super::*;

/// Labels, packs, agents and limits of this workspace. `editing` is the agent a session changes.
pub async fn load_ctx(state: &AppState, ws: &str, editing: Option<&AgentRow>) -> sqlx::Result<WsCtx> {
    let packs = packs::enabled_keys(&state.db, ws).await?.into_iter().map(str::to_string).collect();
    let labels: Vec<(String, String)> =
        sqlx::query_as("SELECT key, name FROM labels WHERE workspace_id = ?1 AND archived = 0 ORDER BY name").bind(ws).fetch_all(&state.db).await?;
    let matching = packs::has_feature(&state.db, ws, packs::FEATURE_MATCHING).await?;
    let mut agents = Vec::new();
    let mut conn = state.db.acquire().await?;
    for row in store::list(&mut *conn, ws).await?.into_iter().filter(|a| !a.is_personal() && a.status != "archived") {
        let (spec, _) = versions::active_spec(&mut conn, &row).await?;
        let labels = spec.triggers.iter().filter_map(|t| if let Trigger::EmailReceived { matcher } = t { Some(matcher.labels.clone()) } else { None }).flatten().collect();
        agents.push(AgentBrief { key: row.key, name: row.name, category: row.category, triggers: validate::trigger_summary(&spec), labels });
    }
    let current = match editing {
        Some(row) => Some(versions::active_spec(&mut conn, row).await?.0),
        None => None,
    };
    Ok(WsCtx {
        packs,
        labels: labels.into_iter().map(|(key, name)| LabelBrief { key, name }).collect(),
        agents,
        matching,
        ceiling: CUSTOM_CEILING,
        max_case_budget_usd: validate::case_budget_ceiling(state),
        editing: current,
        request: String::new(),
    })
}


/// Opens a session. `agent` is the agent it edits, if any. Nothing is asked of the model yet.
pub async fn create(state: &AppState, ws: &str, user_id: &str, goal: &str, agent: Option<&AgentRow>) -> AppResult<Session> {
    let goal = goal.trim();
    if goal.is_empty() || goal.chars().count() > MAX_GOAL_CHARS {
        return Err(AppError::BadRequest(format!("describe the job in 1 to {MAX_GOAL_CHARS} characters")));
    }
    let id = new_id("dsn");
    let now = now_ms();
    sqlx::query(
        "INSERT INTO designer_sessions (id, workspace_id, user_id, agent_id, base_version, goal, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
    )
    .bind(&id)
    .bind(ws)
    .bind(user_id)
    .bind(agent.map(|a| a.id.as_str()))
    .bind(agent.and_then(|a| a.active_version))
    .bind(goal)
    .bind(now)
    .execute(&state.db)
    .await?;
    get(&state.db, ws, &id).await?.ok_or(AppError::NotFound)
}

pub const COULD_NOT_BUILD: &str = "I couldn't build that agent. Try describing it in one or two sentences.";

/// Marks a session whose last request produced nothing usable, so it never sits in `drafting` with no way forward.
async fn mark_failed(db: &SqlitePool, ws: &str, id: &str, message: &str) {
    let issues = json!([{"path": "", "code": "could_not_build", "message": message}]).to_string();
    let _ = sqlx::query("UPDATE designer_sessions SET status = 'failed', issues_json = ?1, updated_at = ?2, rev = rev + 1 WHERE id = ?3 AND workspace_id = ?4 AND status IN ('drafting', 'ready', 'failed')")
        .bind(issues)
        .bind(now_ms())
        .bind(id)
        .bind(ws)
        .execute(db)
        .await;
}

fn designer_line(spec: Option<&AgentSpec>, issues: &[Issue], repairs: usize) -> String {
    match (spec, issues.is_empty()) {
        (Some(s), true) => {
            let fixed = if repairs > 0 { " I fixed a few problems along the way." } else { "" };
            format!("Here is \"{}\".{fixed} Read it below, run the dry run on recent mail, then save it.", s.name)
        }
        (Some(_), false) => format!(
            "I could not fix everything: {} problem{} left. Edit the fields marked below, or tell me what to change.",
            issues.len(),
            if issues.len() == 1 { "" } else { "s" }
        ),
        (None, _) => COULD_NOT_BUILD.to_string(),
    }
}

/// One design turn: checks the limits, asks the designer (with repairs), stores the result.
pub async fn design_turn(state: &AppState, ws: &str, id: &str, text: &str) -> AppResult<Session> {
    let text = text.trim();
    if text.is_empty() || text.chars().count() > MAX_GOAL_CHARS {
        return Err(AppError::BadRequest(format!("write 1 to {MAX_GOAL_CHARS} characters")));
    }
    let session = get(&state.db, ws, id).await?.ok_or(AppError::NotFound)?;
    if !session.is_open() {
        return Err(AppError::Conflict("this session is closed; start a new one".into()));
    }
    if session.turns.len() + 2 > MAX_TURNS {
        return Err(AppError::LimitReached("this conversation is as long as it can get; save what you have or start a new one".into()));
    }
    if !state.designer.enabled() {
        return Err(AppError::AiUnavailable);
    }
    if !crate::ai::ledger::within_budget(&state.db, state.config.ai_daily_budget_usd, ws).await {
        return Err(AppError::LimitReached("this workspace has used its AI budget for today; try again tomorrow".into()));
    }
    let editing = match &session.agent_id {
        Some(agent_id) => Some(store::get_in(&state.db, ws, agent_id).await?.ok_or(AppError::NotFound)?),
        None => None,
    };
    let kind = if editing.as_ref().is_some_and(AgentRow::is_personal) { "personal" } else { "design" };
    let call_id = reserve_call(&mut *state.db.acquire().await?, ws, &session, kind).await?;
    let ctx = match load_ctx(state, ws, editing.as_ref()).await {
        Ok(ctx) => WsCtx { request: format!("{} {}", session.goal, text), ..ctx },
        Err(e) => {
            settle_call(&state.db, &call_id, "failed", 0.0).await?;
            return Err(e.into());
        }
    };
    let request = crate::agents::designer::DesignRequest {
        system: prompt::system_prompt(&ctx),
        user: prompt::design_user(&ctx, &session.turns, session.spec.as_ref(), text),
        schema: schema::spec_schema(),
    };
    let outcome = match repair::run(state.designer.as_ref(), &ctx, request, SESSION_BUDGET_USD - session.cost_usd).await {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!(error = %e, session_id = %session.id, "designer unavailable");
            settle_call(&state.db, &call_id, "failed", 0.0).await?;
            if session.spec.is_none() {
                mark_failed(&state.db, ws, &session.id, COULD_NOT_BUILD).await;
            }
            return Err(AppError::AiUnavailable);
        }
    };

    let mut turns = session.turns.clone();
    turns.push(Turn { role: "user".into(), text: crate::agents::sentence(text, MAX_GOAL_CHARS) });
    turns.push(Turn { role: "designer".into(), text: designer_line(outcome.spec.as_ref(), &outcome.issues, outcome.repairs) });
    let spec = outcome.spec.clone().or(session.spec.clone());
    let unreadable = outcome.spec.is_none();
    if unreadable {
        tracing::warn!(session_id = %session.id, issues = ?outcome.issues, fallback = outcome.fallback_used, "designer could not produce a readable spec");
    }
    let failed = unreadable && session.spec.is_none();
    let issues = match (unreadable, failed) {
        (false, _) => issues_json(&outcome.issues),
        (true, true) => vec![json!({"path": "", "code": "could_not_build", "message": COULD_NOT_BUILD})],
        (true, false) => session.issues.clone(),
    };
    let ready = if unreadable { session.spec.is_some() && session.issues.is_empty() } else { outcome.issues.is_empty() };
    let changed = spec.as_ref().map(AgentSpec::hash) != session.spec.as_ref().map(AgentSpec::hash);
    let (warnings, routing) = match (&spec, unreadable) {
        (Some(spec), false) => (validate::warnings(spec, &ctx), Some(validate::routing_preview(spec))),
        _ => (session.warnings.clone(), session.routing_preview.clone()),
    };
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query("UPDATE designer_calls SET status = 'done', cost_usd = ?1 WHERE id = ?2").bind(outcome.cost_usd).bind(&call_id).execute(&mut *tx).await?;
    let swapped = sqlx::query(
        "UPDATE designer_sessions SET turns_json = ?1, spec_json = ?2, issues_json = ?3, status = ?4, cost_usd = cost_usd + ?5, updated_at = ?6, rev = rev + 1,
           dry_run_json = CASE WHEN ?7 THEN NULL ELSE dry_run_json END, dry_run_status = CASE WHEN ?7 THEN 'idle' ELSE dry_run_status END,
           warnings_json = ?11, routing_preview = ?12
         WHERE id = ?8 AND workspace_id = ?9 AND rev = ?10 AND status IN ('drafting', 'ready', 'failed')",
    )
    .bind(serde_json::to_string(&turns).unwrap_or_else(|_| "[]".into()))
    .bind(spec.as_ref().map(AgentSpec::to_json))
    .bind(json!(issues).to_string())
    .bind(if failed { "failed" } else if ready { "ready" } else { "drafting" })
    .bind(outcome.cost_usd)
    .bind(now_ms())
    .bind(changed)
    .bind(&session.id)
    .bind(ws)
    .bind(session.rev)
    .bind(json!(warnings).to_string())
    .bind(routing)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if swapped == 0 {
        sqlx::query("UPDATE designer_sessions SET cost_usd = cost_usd + ?1 WHERE id = ?2 AND workspace_id = ?3").bind(outcome.cost_usd).bind(&session.id).bind(ws).execute(&mut *tx).await?;
        tx.commit().await?;
        return Err(AppError::Conflict("this session changed while the designer was working; reload it and try again".into()));
    }
    tx.commit().await?;
    get(&state.db, ws, id).await?.ok_or(AppError::NotFound)
}

/// Replaces the spec with the person's own edit. Validated, not normalised: the person sees exactly what is wrong.
pub async fn set_spec(state: &AppState, ws: &str, id: &str, spec: AgentSpec) -> AppResult<Session> {
    let session = get(&state.db, ws, id).await?.ok_or(AppError::NotFound)?;
    if !session.is_open() {
        return Err(AppError::Conflict("this session is closed; start a new one".into()));
    }
    let editing = match &session.agent_id {
        Some(agent_id) => Some(store::get_in(&state.db, ws, agent_id).await?.ok_or(AppError::NotFound)?),
        None => None,
    };
    let ctx = load_ctx(state, ws, editing.as_ref()).await?;
    let issues = validate::validate(&spec, &ctx);
    let changed = session.spec.as_ref().map(AgentSpec::hash) != Some(spec.hash());
    let swapped = sqlx::query(
        "UPDATE designer_sessions SET spec_json = ?1, issues_json = ?2, status = ?3, updated_at = ?4, rev = rev + 1,
           dry_run_json = CASE WHEN ?5 THEN NULL ELSE dry_run_json END, dry_run_status = CASE WHEN ?5 THEN 'idle' ELSE dry_run_status END,
           warnings_json = ?9, routing_preview = ?10
         WHERE id = ?6 AND workspace_id = ?7 AND rev = ?8",
    )
    .bind(spec.to_json())
    .bind(json!(issues_json(&issues)).to_string())
    .bind(if issues.is_empty() { "ready" } else { "drafting" })
    .bind(now_ms())
    .bind(changed)
    .bind(id)
    .bind(ws)
    .bind(session.rev)
    .bind(json!(validate::warnings(&spec, &ctx)).to_string())
    .bind(validate::routing_preview(&spec))
    .execute(&state.db)
    .await?
    .rows_affected();
    if swapped == 0 {
        return Err(AppError::Conflict("this session changed while you were editing; reload it and try again".into()));
    }
    get(&state.db, ws, id).await?.ok_or(AppError::NotFound)
}
