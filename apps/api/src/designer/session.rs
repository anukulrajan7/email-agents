//! Designer sessions: storage, the per-workspace and per-session limits, the design turn, manual
//! spec edits, the dry-run request and saving as an agent (a new one, or a new version of one).

use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::{
    DAILY_LIMIT, MAX_GOAL_CHARS, MAX_TURNS, SESSION_BUDGET_USD, Turn, prompt, repair, schema,
    validate::{self, AgentBrief, CUSTOM_CEILING, LabelBrief, WsCtx},
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
    rev: i64,
    created_at: i64,
    updated_at: i64,
}

const COLUMNS: &str = "id, workspace_id, user_id, agent_id, base_version, goal, turns_json, spec_json, issues_json, dry_run_json,
    dry_run_status, status, cost_usd, rev, created_at, updated_at";

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
    pub rev: i64,
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
            rev: r.rev,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

impl Session {
    pub fn is_open(&self) -> bool {
        matches!(self.status.as_str(), "drafting" | "ready" | "failed")
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
    sqlx::query_scalar("SELECT COUNT(*) FROM designer_calls WHERE workspace_id = ?1 AND kind = 'design' AND created_at >= ?2").bind(ws).bind(since).fetch_one(db).await
}

/// Designer requests on members' own agents, which have a small limit of their own per member.
pub const PERSONAL_DAILY_LIMIT: i64 = 5;
/// Dry runs one session may start per hour.
pub const DRY_RUNS_PER_HOUR: i64 = 5;
/// A reserved call that never finished stops blocking its session after this long.
const PENDING_STALE_MS: i64 = 5 * 60 * 1000;
const HOUR_MS: i64 = 3_600_000;

/// Takes one designer call slot before the model is asked. The checks and the insert are one statement, so
/// requests that race cannot go past the daily limit, the per-member limit, the session's budget, or run two
/// at once in one session. Returns the id of the reserved row.
async fn reserve_call(conn: &mut sqlx::SqliteConnection, ws: &str, session: &Session, kind: &str) -> AppResult<String> {
    let id = new_id("dcl");
    let now = now_ms();
    let day_start = now - now.rem_euclid(DAY_MS);
    let inserted = sqlx::query(
        "INSERT INTO designer_calls (id, workspace_id, session_id, user_id, kind, status, cost_usd, created_at)
         SELECT ?1, ?2, ?3, ?4, ?5, 'pending', 0, ?6
         WHERE (?5 != 'design' OR (SELECT COUNT(*) FROM designer_calls WHERE workspace_id = ?2 AND kind = 'design' AND created_at >= ?7) < ?8)
           AND (?5 != 'personal' OR (SELECT COUNT(*) FROM designer_calls WHERE workspace_id = ?2 AND kind = 'personal' AND user_id = ?4 AND created_at >= ?7) < ?9)
           AND (SELECT cost_usd FROM designer_sessions WHERE id = ?3) < ?10
           AND NOT EXISTS (SELECT 1 FROM designer_calls WHERE session_id = ?3 AND status = 'pending' AND created_at > ?11 AND (kind = 'dry_run') = (?5 = 'dry_run'))
           AND (?5 != 'dry_run' OR (SELECT COUNT(*) FROM designer_calls WHERE session_id = ?3 AND kind = 'dry_run' AND created_at > ?12) < ?13)",
    )
    .bind(&id)
    .bind(ws)
    .bind(&session.id)
    .bind(&session.user_id)
    .bind(kind)
    .bind(now)
    .bind(day_start)
    .bind(DAILY_LIMIT)
    .bind(PERSONAL_DAILY_LIMIT)
    .bind(SESSION_BUDGET_USD)
    .bind(now - PENDING_STALE_MS)
    .bind(now - HOUR_MS)
    .bind(DRY_RUNS_PER_HOUR)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if inserted == 1 {
        return Ok(id);
    }
    let spent: f64 = sqlx::query_scalar("SELECT cost_usd FROM designer_sessions WHERE id = ?1").bind(&session.id).fetch_one(&mut *conn).await?;
    if spent >= SESSION_BUDGET_USD {
        return Err(AppError::LimitReached(format!("this session has reached its ${SESSION_BUDGET_USD:.2} limit; save what you have or start a new one")));
    }
    if kind == "dry_run" {
        let recent: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM designer_calls WHERE session_id = ?1 AND kind = 'dry_run' AND created_at > ?2")
            .bind(&session.id)
            .bind(now - HOUR_MS)
            .fetch_one(&mut *conn)
            .await?;
        if recent >= DRY_RUNS_PER_HOUR {
            return Err(AppError::LimitReached(format!("this session has run {DRY_RUNS_PER_HOUR} dry runs in the last hour; try again later")));
        }
    }
    if kind == "personal" {
        return Err(AppError::LimitReached(format!("you have used your {PERSONAL_DAILY_LIMIT} designer requests for your own agent today; try again tomorrow")));
    }
    if kind == "design" {
        let today: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM designer_calls WHERE workspace_id = ?1 AND kind = 'design' AND created_at >= ?2")
            .bind(ws)
            .bind(day_start)
            .fetch_one(&mut *conn)
            .await?;
        if today >= DAILY_LIMIT {
            return Err(AppError::LimitReached(format!("this workspace has used its {DAILY_LIMIT} designer requests for today; try again tomorrow")));
        }
    }
    Err(AppError::Conflict("the designer is still working on this session; wait for it to finish".into()))
}

/// Settles a reserved call with what it cost (failed calls are kept too).
async fn settle_call(db: &SqlitePool, id: &str, status: &str, cost_usd: f64) -> sqlx::Result<()> {
    sqlx::query("UPDATE designer_calls SET status = ?1, cost_usd = ?2 WHERE id = ?3").bind(status).bind(cost_usd).bind(id).execute(db).await?;
    Ok(())
}

// ---------------------------------------------------------------- the workspace as the designer sees it

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
        matching,
        ceiling: CUSTOM_CEILING,
        max_case_budget_usd: validate::case_budget_ceiling(state),
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
    if !crate::ai_ledger::within_budget(&state.db, state.config.ai_daily_budget_usd, ws).await {
        return Err(AppError::LimitReached("this workspace has used its AI budget for today; try again tomorrow".into()));
    }
    let editing = match &session.agent_id {
        Some(agent_id) => Some(store::get_in(&state.db, ws, agent_id).await?.ok_or(AppError::NotFound)?),
        None => None,
    };
    let kind = if editing.as_ref().is_some_and(AgentRow::is_personal) { "personal" } else { "design" };
    let call_id = reserve_call(&mut *state.db.acquire().await?, ws, &session, kind).await?;
    let ctx = match load_ctx(state, ws, editing.as_ref()).await {
        Ok(ctx) => ctx,
        Err(e) => {
            settle_call(&state.db, &call_id, "failed", 0.0).await?;
            return Err(e.into());
        }
    };
    let request = super::DesignRequest {
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
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query("UPDATE designer_calls SET status = 'done', cost_usd = ?1 WHERE id = ?2").bind(outcome.cost_usd).bind(&call_id).execute(&mut *tx).await?;
    let swapped = sqlx::query(
        "UPDATE designer_sessions SET turns_json = ?1, spec_json = ?2, issues_json = ?3, status = ?4, cost_usd = cost_usd + ?5, updated_at = ?6, rev = rev + 1,
           dry_run_json = CASE WHEN ?7 THEN NULL ELSE dry_run_json END, dry_run_status = CASE WHEN ?7 THEN 'idle' ELSE dry_run_status END
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
           dry_run_json = CASE WHEN ?5 THEN NULL ELSE dry_run_json END, dry_run_status = CASE WHEN ?5 THEN 'idle' ELSE dry_run_status END
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
    .execute(&state.db)
    .await?
    .rows_affected();
    if swapped == 0 {
        return Err(AppError::Conflict("this session changed while you were editing; reload it and try again".into()));
    }
    get(&state.db, ws, id).await?.ok_or(AppError::NotFound)
}

// ---------------------------------------------------------------- dry run

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
            super::dry_run::owned_clause(3)
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
    if !crate::ai_ledger::within_budget(&state.db, state.config.ai_daily_budget_usd, ws).await {
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

// ---------------------------------------------------------------- save

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
        return Err(AppError::BadRequest(crate::agent_registry::spec::issues_message(&issues)));
    }
    let dry = session.dry_run.as_ref().filter(|_| session.dry_run_status == "done");
    if dry.and_then(|d| d["spec_hash"].as_str()) != Some(spec.hash().as_str()) {
        return Err(AppError::Conflict("run the dry run on this version of the spec before saving".into()));
    }
    let safety = super::dry_run::safety(&spec, validate::case_budget_ceiling(state));
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
    let role: Option<String> = sqlx::query_scalar("SELECT role FROM memberships WHERE workspace_id = ?1 AND user_id = ?2").bind(ws).bind(user_id).fetch_optional(&state.db).await?;
    if role.as_deref() != Some("admin") {
        return Err(AppError::Forbidden);
    }
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
