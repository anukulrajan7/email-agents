//! Designer sessions: storage, the per-workspace and per-session limits, the design turn, manual
//! spec edits, the dry-run request and saving as an agent (a new one, or a new version of one).

use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::{
    DAILY_LIMIT, MAX_GOAL_CHARS, MAX_TURNS, SESSION_BUDGET_USD, Turn, prompt, repair, schema,
    validate::{self, AgentBrief, CUSTOM_CEILING, LabelBrief, WsCtx},
};
use crate::{
    agents::registry::{
        spec::{AgentSpec, Issue, Trigger},
        store::{self, AgentRow, NewAgent},
        versions::{self, NewVersion},
    },
    platform::audit::{self, Actor, Entry},
    platform::db::{iso, new_id, now_ms},
    platform::error::{AppError, AppResult},
    http::AppState,
    jobs, platform::packs,
};

mod quota;
mod talk;
mod dry_request;
mod save;
mod view;

pub use talk::load_ctx;
pub use talk::create;
pub use talk::COULD_NOT_BUILD;
pub use talk::design_turn;
pub use talk::set_spec;
pub use dry_request::DRY_RUN_MAX_MESSAGES;
pub use dry_request::request_dry_run;
pub use dry_request::owner_of;
pub use save::SaveOptions;
pub use save::Saved;
pub use save::save;
pub use save::abandon;
pub use view::view;
pub use view::open_from_proposal;
use quota::reserve_call;
use quota::settle_call;

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
    warnings_json: String,
    routing_preview: Option<String>,
    dry_run_json: Option<String>,
    dry_run_status: String,
    status: String,
    cost_usd: f64,
    rev: i64,
    created_at: i64,
    updated_at: i64,
}

const COLUMNS: &str = "id, workspace_id, user_id, agent_id, base_version, goal, turns_json, spec_json, issues_json, warnings_json, routing_preview, dry_run_json,
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
    pub warnings: Vec<String>,
    pub routing_preview: Option<String>,
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
            warnings: serde_json::from_str(&r.warnings_json).unwrap_or_default(),
            routing_preview: r.routing_preview,
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

