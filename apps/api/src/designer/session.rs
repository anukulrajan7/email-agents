//! Designer sessions: storage, the per-workspace and per-session limits, the design turn, manual
//! spec edits, the dry-run request and saving as an agent (a new one, or a new version of one).

use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::{
    DAILY_LIMIT, MAX_GOAL_CHARS, MAX_TURNS, SESSION_BUDGET_USD, Turn, prompt, repair, schema,
    validate::{self, AgentBrief, CUSTOM_CEILING, LabelBrief, MAX_CASE_BUDGET_USD, WsCtx},
};
use crate::{
    agent_registry::{
        spec::{AgentSpec, Issue},
        store::{self, AgentRow, NewAgent},
        versions::{self, NewVersion},
    },
    audit::{self, Actor, Entry},
    db::{iso, new_id, now_ms},
    error::{AppError, AppResult},
    http::AppState,
    jobs, packs,
};

const DAY_MS: i64 = 86_400_000;

#[derive(Debug, sqlx::FromRow)]
struct Row {
    id: String,
    workspace_id: String,
    user_id: String,
    agent_id: Option<String>,
    base_version: Option<i64>,
    goal: String,
    turns_json: String,
    spec_json: Option<String>,
    issues_json: String,
    dry_run_json: Option<String>,
    dry_run_status: String,
    status: String,
    cost_usd: f64,
    created_at: i64,
    updated_at: i64,
}

const COLUMNS: &str = "id, workspace_id, user_id, agent_id, base_version, goal, turns_json, spec_json, issues_json, dry_run_json,
    dry_run_status, status, cost_usd, created_at, updated_at";

#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub workspace_id: String,
    pub user_id: String,
    pub agent_id: Option<String>,
    pub base_version: Option<i64>,
    pub goal: String,
    pub turns: Vec<Turn>,
    pub spec: Option<AgentSpec>,
    pub issues: Vec<Value>,
    pub dry_run: Option<Value>,
    pub dry_run_status: String,
    pub status: String,
    pub cost_usd: f64,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<Row> for Session {
    fn from(r: Row) -> Self {
        Self {
            turns: serde_json::from_str(&r.turns_json).unwrap_or_default(),
            spec: r.spec_json.as_deref().and_then(|s| serde_json::from_str(s).ok()),
            issues: serde_json::from_str(&r.issues_json).unwrap_or_default(),
            dry_run: r.dry_run_json.as_deref().and_then(|s| serde_json::from_str(s).ok()),
            id: r.id,
            workspace_id: r.workspace_id,
            user_id: r.user_id,
            agent_id: r.agent_id,
            base_version: r.base_version,
            goal: r.goal,
            dry_run_status: r.dry_run_status,
            status: r.status,
            cost_usd: r.cost_usd,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

impl Session {
    pub fn is_open(&self) -> bool {
        matches!(self.status.as_str(), "drafting" | "ready")
    }
}

pub fn issues_json(issues: &[Issue]) -> Vec<Value> {
    issues.iter().map(|i| json!({"path": i.path, "code": i.code, "message": i.message})).collect()
}

pub async fn get(db: &SqlitePool, ws: &str, id: &str) -> sqlx::Result<Option<Session>> {
    let row: Option<Row> = sqlx::query_as(&format!("SELECT {COLUMNS} FROM designer_sessions WHERE id = ?1 AND workspace_id = ?2"))
        .bind(id)
        .bind(ws)
        .fetch_optional(db)
        .await?;
    Ok(row.map(Into::into))
}

/// Designer requests this workspace has made since 00:00 UTC.
pub async fn calls_today(db: &SqlitePool, ws: &str) -> sqlx::Result<i64> {
    let since = now_ms() - now_ms().rem_euclid(DAY_MS);
    sqlx::query_scalar("SELECT COUNT(*) FROM designer_calls WHERE workspace_id = ?1 AND created_at >= ?2").bind(ws).bind(since).fetch_one(db).await
}

// ---------------------------------------------------------------- the workspace as the designer sees it

/// Labels, packs, agents and limits of this workspace. `editing` is the agent a session changes.
pub async fn load_ctx(state: &AppState, ws: &str, editing: Option<&AgentRow>) -> sqlx::Result<WsCtx> {
    let packs = packs::enabled_keys(&state.db, ws).await?.into_iter().map(str::to_string).collect();
    let labels: Vec<(String, String)> =
        sqlx::query_as("SELECT key, name FROM labels WHERE workspace_id = ?1 AND archived = 0 ORDER BY name").bind(ws).fetch_all(&state.db).await?;
    let mut agents = Vec::new();
    let mut conn = state.db.acquire().await?;
    for row in store::list(&mut *conn, ws).await?.into_iter().filter(|a| !a.is_personal() && a.status != "archived") {
        let (spec, _) = versions::active_spec(&mut conn, &row).await?;
        agents.push(AgentBrief { key: row.key, name: row.name, category: row.category, triggers: validate::trigger_summary(&spec) });
    }
    let current = match editing {
        Some(row) => Some(versions::active_spec(&mut conn, row).await?.0),
        None => None,
    };
    Ok(WsCtx {
        packs,
        labels: labels.into_iter().map(|(key, name)| LabelBrief { key, name }).collect(),
        agents,
        matching: packs::has_feature(&state.db, ws, packs::FEATURE_MATCHING).await?,
        ceiling: CUSTOM_CEILING,
        max_case_budget_usd: MAX_CASE_BUDGET_USD.max(state.config.agent_budget_usd),
        editing: current,
    })
}

// ---------------------------------------------------------------- create and talk

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
        (None, _) => "I could not turn that into an agent. Try describing the job again with a bit more detail.".to_string(),
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
    if session.cost_usd >= SESSION_BUDGET_USD {
        return Err(AppError::LimitReached(format!("this session has reached its ${SESSION_BUDGET_USD:.2} limit; save what you have or start a new one")));
    }
    if calls_today(&state.db, ws).await? >= DAILY_LIMIT {
        return Err(AppError::LimitReached(format!("this workspace has used its {DAILY_LIMIT} designer requests for today; try again tomorrow")));
    }
    if !crate::ai_ledger::within_budget(&state.db, state.config.ai_daily_budget_usd, ws).await {
        return Err(AppError::LimitReached("this workspace has used its AI budget for today; try again tomorrow".into()));
    }
    let editing = match &session.agent_id {
        Some(agent_id) => Some(store::get_in(&state.db, ws, agent_id).await?.ok_or(AppError::NotFound)?),
        None => None,
    };
    let ctx = load_ctx(state, ws, editing.as_ref()).await?;
    let request = super::DesignRequest {
        system: prompt::system_prompt(&ctx),
        user: prompt::design_user(&ctx, &session.turns, session.spec.as_ref(), text),
        schema: schema::spec_schema(),
    };
    let outcome = repair::run(state.designer.as_ref(), &ctx, request, SESSION_BUDGET_USD - session.cost_usd).await.map_err(|e| {
        tracing::warn!(error = %e, session_id = %session.id, "designer unavailable");
        AppError::AiUnavailable
    })?;

    let mut turns = session.turns.clone();
    turns.push(Turn { role: "user".into(), text: crate::agents::sentence(text, MAX_GOAL_CHARS) });
    turns.push(Turn { role: "designer".into(), text: designer_line(outcome.spec.as_ref(), &outcome.issues, outcome.repairs) });
    let spec = outcome.spec.clone().or(session.spec.clone());
    let issues = issues_json(&outcome.issues);
    let ready = outcome.spec.is_some() && outcome.issues.is_empty();
    let changed = spec.as_ref().map(AgentSpec::hash) != session.spec.as_ref().map(AgentSpec::hash);
    let mut tx = state.db.begin().await?;
    sqlx::query("INSERT INTO designer_calls (id, workspace_id, session_id, cost_usd, created_at) VALUES (?1, ?2, ?3, ?4, ?5)")
        .bind(new_id("dcl"))
        .bind(ws)
        .bind(&session.id)
        .bind(outcome.cost_usd)
        .bind(now_ms())
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE designer_sessions SET turns_json = ?1, spec_json = ?2, issues_json = ?3, status = ?4, cost_usd = cost_usd + ?5, updated_at = ?6,
           dry_run_json = CASE WHEN ?7 THEN NULL ELSE dry_run_json END, dry_run_status = CASE WHEN ?7 THEN 'idle' ELSE dry_run_status END
         WHERE id = ?8 AND workspace_id = ?9",
    )
    .bind(serde_json::to_string(&turns).unwrap_or_else(|_| "[]".into()))
    .bind(spec.as_ref().map(AgentSpec::to_json))
    .bind(json!(issues).to_string())
    .bind(if ready { "ready" } else { "drafting" })
    .bind(outcome.cost_usd)
    .bind(now_ms())
    .bind(changed)
    .bind(&session.id)
    .bind(ws)
    .execute(&mut *tx)
    .await?;
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
    sqlx::query(
        "UPDATE designer_sessions SET spec_json = ?1, issues_json = ?2, status = ?3, updated_at = ?4,
           dry_run_json = CASE WHEN ?5 THEN NULL ELSE dry_run_json END, dry_run_status = CASE WHEN ?5 THEN 'idle' ELSE dry_run_status END
         WHERE id = ?6 AND workspace_id = ?7",
    )
    .bind(spec.to_json())
    .bind(json!(issues_json(&issues)).to_string())
    .bind(if issues.is_empty() { "ready" } else { "drafting" })
    .bind(now_ms())
    .bind(changed)
    .bind(id)
    .bind(ws)
    .execute(&state.db)
    .await?;
    get(&state.db, ws, id).await?.ok_or(AppError::NotFound)
}

// ---------------------------------------------------------------- dry run

pub const DRY_RUN_MAX_MESSAGES: usize = 50;
/// A dry run that has not finished after this long may be asked for again.
const STALE_DRY_RUN_MS: i64 = 10 * 60 * 1000;

/// Queues the dry run. The spec must be valid; picked messages must be inbound mail of this workspace.
pub async fn request_dry_run(state: &AppState, ws: &str, id: &str, message_ids: &[String]) -> AppResult<Session> {
    let session = get(&state.db, ws, id).await?.ok_or(AppError::NotFound)?;
    if !session.is_open() {
        return Err(AppError::Conflict("this session is closed; start a new one".into()));
    }
    if session.spec.is_none() || !session.issues.is_empty() {
        return Err(AppError::Conflict("fix the problems with the spec first; a dry run needs a valid spec".into()));
    }
    if message_ids.len() > DRY_RUN_MAX_MESSAGES {
        return Err(AppError::BadRequest(format!("pick at most {DRY_RUN_MAX_MESSAGES} emails")));
    }
    if !message_ids.is_empty() {
        let found: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM email_messages WHERE workspace_id = ?1 AND direction = 'inbound' AND id IN (SELECT value FROM json_each(?2))",
        )
        .bind(ws)
        .bind(json!(message_ids).to_string())
        .fetch_one(&state.db)
        .await?;
        if found != message_ids.len() as i64 {
            return Err(AppError::BadRequest("one of those emails was not found".into()));
        }
    }
    if session.dry_run_status == "running" && now_ms() - session.updated_at < STALE_DRY_RUN_MS {
        return Ok(session);
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE designer_sessions SET dry_run_status = 'running', dry_run_json = NULL, updated_at = ?1 WHERE id = ?2 AND workspace_id = ?3")
        .bind(now_ms())
        .bind(id)
        .bind(ws)
        .execute(&mut *tx)
        .await?;
    jobs::enqueue(
        &mut tx,
        jobs::kind::DESIGNER_DRY_RUN,
        &format!("{}:{id}:{}", jobs::kind::DESIGNER_DRY_RUN, now_ms()),
        Some(ws),
        &json!({"session_id": id, "message_ids": message_ids}),
    )
    .await?;
    tx.commit().await?;
    state.jobs_notify.notify_one();
    get(&state.db, ws, id).await?.ok_or(AppError::NotFound)
}

// ---------------------------------------------------------------- save

pub struct SaveOptions<'a> {
    /// 0 observes only (shadow); 1 drafts for a person to approve.
    pub start_level: i64,
    pub avatar: &'a str,
    pub color: &'a str,
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
        return Err(AppError::BadRequest(crate::agent_registry::spec::issues_message(&issues)));
    }
    let dry = session.dry_run.as_ref().filter(|_| session.dry_run_status == "done");
    if dry.and_then(|d| d["spec_hash"].as_str()) != Some(spec.hash().as_str()) {
        return Err(AppError::Conflict("run the dry run on this version of the spec before saving".into()));
    }
    let note = format!("Designed: {}", crate::agents::sentence(&session.goal, 160));
    let mut tx = state.db.begin().await?;
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
    sqlx::query("UPDATE designer_sessions SET status = 'saved', agent_id = ?1, updated_at = ?2 WHERE id = ?3 AND workspace_id = ?4")
        .bind(&agent_id)
        .bind(now_ms())
        .bind(id)
        .bind(ws)
        .execute(&mut *tx)
        .await?;
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
    sqlx::query("UPDATE designer_sessions SET status = 'abandoned', updated_at = ?1 WHERE id = ?2 AND workspace_id = ?3 AND status IN ('drafting', 'ready')")
        .bind(now_ms())
        .bind(id)
        .bind(ws)
        .execute(db)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------- the API view

/// The session as the web app reads it.
pub async fn view(state: &AppState, ws: &str, s: &Session) -> AppResult<Value> {
    let diff = match (&s.agent_id, &s.spec) {
        (Some(agent_id), Some(spec)) => match store::get_in(&state.db, ws, agent_id).await? {
            Some(agent) => {
                let (current, _) = versions::active_spec_in(&state.db, &agent).await?;
                Some(versions::diff(&current, spec))
            }
            None => None,
        },
        _ => None,
    };
    let dry_run = match s.dry_run_status.as_str() {
        "idle" => Value::Null,
        "running" => json!({"status": "running"}),
        status => {
            let mut v = s.dry_run.clone().unwrap_or_else(|| json!({}));
            v["status"] = json!(status);
            v
        }
    };
    Ok(json!({
        "id": s.id, "goal": s.goal, "agent_id": s.agent_id, "base_version": s.base_version, "status": s.status,
        "turns": s.turns, "spec": s.spec, "issues": s.issues,
        "questions": s.spec.as_ref().map(|x| x.questions_for_user.clone()).unwrap_or_default(),
        "assumptions": s.spec.as_ref().map(|x| x.assumptions.clone()).unwrap_or_default(),
        "diff": diff, "dry_run": dry_run, "cost_usd": (s.cost_usd * 1e6).round() / 1e6,
        "limits": {"calls_today": calls_today(&state.db, ws).await?, "daily_limit": DAILY_LIMIT, "session_budget_usd": SESSION_BUDGET_USD},
        "created_at": iso(s.created_at), "updated_at": iso(s.updated_at),
    }))
}

/// Opens a designer session from an agent's suggested automation, so an admin can review it,
/// dry-run it and save it as a real agent. Nothing runs until they save. If the designer cannot
/// answer right now (limit, no model) the session still opens with the proposal as its description.
pub async fn open_from_proposal(state: &AppState, ws: &str, user_id: &str, name: &str, description: &str) -> AppResult<Session> {
    let session = create(state, ws, user_id, description, None).await?;
    match design_turn(state, ws, &session.id, description).await {
        Ok(s) => Ok(s),
        Err(AppError::LimitReached(_) | AppError::AiUnavailable) => {
            tracing::info!(session_id = %session.id, %name, "designer session opened without a first draft");
            Ok(session)
        }
        Err(e) => Err(e),
    }
}
