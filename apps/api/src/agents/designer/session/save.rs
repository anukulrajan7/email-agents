use super::*;

pub struct SaveOptions<'a> {
    /// 0 observes only (shadow); 1 drafts for a person to approve.
    pub start_level: i64,
    pub avatar: &'a str,
    pub color: &'a str,
    /// The person agreed to the agent taking emails other agents handle today.
    pub confirm_steals: bool,
}

pub struct Saved {
    pub agent_id: String,
    pub version: i64,
}

/// Saves the session's spec: a new custom agent, or a new `designer` version of the agent it edits.
/// The spec must be valid and its dry run done on exactly this spec.
pub async fn save(state: &AppState, ws: &str, user_id: &str, id: &str, opts: SaveOptions<'_>) -> AppResult<Saved> {
    if !(0..=1).contains(&opts.start_level) {
        return Err(AppError::BadRequest("start_level must be 0 (observe) or 1 (draft for review)".into()));
    }
    let session = get(&state.db, ws, id).await?.ok_or(AppError::NotFound)?;
    if !session.is_open() {
        return Err(AppError::Conflict("this session was already saved or closed".into()));
    }
    let spec = session.spec.clone().ok_or_else(|| AppError::Conflict("there is no spec to save yet".into()))?;
    let editing = match &session.agent_id {
        Some(agent_id) => Some(store::get_in(&state.db, ws, agent_id).await?.ok_or(AppError::NotFound)?),
        None => None,
    };
    let ctx = load_ctx(state, ws, editing.as_ref()).await?;
    let issues = validate::validate(&spec, &ctx);
    if !issues.is_empty() {
        return Err(AppError::BadRequest(crate::agents::registry::spec::issues_message(&issues)));
    }
    let dry = session.dry_run.as_ref().filter(|_| session.dry_run_status == "done");
    if dry.and_then(|d| d["spec_hash"].as_str()) != Some(spec.hash().as_str()) {
        return Err(AppError::Conflict("run the dry run on this version of the spec before saving".into()));
    }
    let safety = crate::agents::designer::dry_run::safety(&spec, validate::case_budget_ceiling(state));
    if safety["passed"] != true {
        return Err(AppError::BadRequest(format!("this agent does not pass the safety checks: {}", safety["failed"].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join("; "))));
    }
    let steals: Vec<String> = dry
        .and_then(|d| d["steals"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|s| Some(format!("{} ({})", s["name"].as_str()?, s["count"].as_i64()?)))
        .collect();
    if !steals.is_empty() && !opts.confirm_steals {
        return Err(AppError::Conflict(format!("this agent would take emails from {}; confirm that to save it", steals.join(", "))));
    }
    if let Some(agent) = &editing
        && agent.active_version != session.base_version
    {
        return Err(AppError::Conflict("this agent changed since you started; start a new session from its current version".into()));
    }
    let note = format!("Designed: {}", crate::agents::sentence(&session.goal, 160));
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let (agent_id, version, action) = match editing {
        None => {
            let agent_id = store::insert(
                &mut tx,
                NewAgent {
                    workspace_id: ws,
                    key: &spec.key,
                    kind: "custom",
                    scope: store::SCOPE_WORKSPACE,
                    owner_user_id: None,
                    category: &spec.category,
                    template_key: None,
                    template_version: None,
                    cloned_from: None,
                    name: &spec.name,
                    avatar: opts.avatar,
                    color: opts.color,
                    description: &spec.description,
                    status: "active",
                    autonomy_level: opts.start_level,
                    autonomy_cap: 1,
                    created_by: Some(user_id),
                },
            )
            .await?
            .ok_or_else(|| AppError::Conflict(format!("an agent with the key \"{}\" already exists", spec.key)))?;
            let version = versions::create(
                &mut tx,
                ws,
                &agent_id,
                &spec,
                NewVersion { source: "designer", note: Some(&note), created_by: Some(user_id), designer_session_id: Some(id), activate: true },
            )
            .await?;
            (agent_id, version, "agent.created")
        }
        Some(agent) => {
            let (previous, _) = versions::active_spec(&mut tx, &agent).await?;
            let version = versions::create(
                &mut tx,
                ws,
                &agent.id,
                &spec,
                NewVersion { source: "designer", note: Some(&note), created_by: Some(user_id), designer_session_id: Some(id), activate: true },
            )
            .await?;
            versions::cap_after_behaviour_change(&mut tx, ws, &agent, &previous, &spec).await?;
            sqlx::query(
                "UPDATE agents SET tuned = tuned OR ?1, description = ?2, name = CASE WHEN kind = 'custom' THEN ?3 ELSE name END
                 WHERE id = ?4 AND workspace_id = ?5",
            )
            .bind(agent.kind != "custom")
            .bind(&spec.description)
            .bind(&spec.name)
            .bind(&agent.id)
            .bind(ws)
            .execute(&mut *tx)
            .await?;
            (agent.id, version, "agent.spec_saved")
        }
    };
    let closed = sqlx::query(
        "UPDATE designer_sessions SET status = 'saved', agent_id = ?1, updated_at = ?2, rev = rev + 1 WHERE id = ?3 AND workspace_id = ?4 AND rev = ?5 AND status IN ('drafting', 'ready', 'failed')",
    )
    .bind(&agent_id)
    .bind(now_ms())
    .bind(id)
    .bind(ws)
    .bind(session.rev)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if closed == 0 {
        return Err(AppError::Conflict("this session changed while saving; reload it and try again".into()));
    }
    audit::record(
        &mut tx,
        Entry {
            workspace_id: ws,
            actor: Actor::User(user_id),
            action,
            entity_type: "agent",
            entity_id: &agent_id,
            message_id: None,
            metadata: json!({"key": spec.key, "version": version, "source": "designer", "designer_session_id": id, "start_level": opts.start_level}),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Saved { agent_id, version })
}

/// Marks a session abandoned (the person left without saving).
pub async fn abandon(db: &SqlitePool, ws: &str, id: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE designer_sessions SET status = 'abandoned', updated_at = ?1 WHERE id = ?2 AND workspace_id = ?3 AND status IN ('drafting', 'ready', 'failed')")
        .bind(now_ms())
        .bind(id)
        .bind(ws)
        .execute(db)
        .await?;
    Ok(())
}
