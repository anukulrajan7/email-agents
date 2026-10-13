//! Persistence of workspace agents. Every lookup is scoped by workspace.

use sqlx::{SqliteConnection, SqliteExecutor};

use crate::db::now_ms;

pub const KINDS: &[&str] = &["lead", "system", "custom"];
pub const STATUSES: &[&str] = &["draft", "active", "paused", "archived"];
pub const PAUSE_REASONS: &[&str] = &["pack_off", "user", "demoted_incident", "pause_all"];
pub const SCOPE_WORKSPACE: &str = "workspace";
pub const SCOPE_PERSONAL: &str = "personal";

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct AgentRow {
    pub id: String,
    pub workspace_id: String,
    pub key: String,
    pub kind: String,
    pub scope: String,
    pub owner_user_id: Option<String>,
    pub category: String,
    pub template_key: Option<String>,
    pub template_version: Option<i64>,
    pub cloned_from: Option<String>,
    pub name: String,
    pub avatar: String,
    pub color: String,
    pub description: String,
    pub status: String,
    pub paused_reason: Option<String>,
    pub active_version: Option<i64>,
    pub tuned: bool,
    pub autonomy_level: i64,
    pub autonomy_cap: i64,
    pub next_run_at: Option<i64>,
    pub created_by: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

pub const AGENT_COLUMNS: &str = "id, workspace_id, key, kind, scope, owner_user_id, category, template_key, template_version, cloned_from,
    name, avatar, color, description, status, paused_reason, active_version, tuned, autonomy_level, autonomy_cap, next_run_at,
    created_by, created_at, updated_at";

impl AgentRow {
    pub fn is_personal(&self) -> bool {
        self.scope == SCOPE_PERSONAL
    }
}

/// One agent of this workspace. Another workspace's agent id is simply not found.
pub async fn get_in<'e>(db: impl SqliteExecutor<'e>, ws: &str, id: &str) -> sqlx::Result<Option<AgentRow>> {
    sqlx::query_as(&format!("SELECT {AGENT_COLUMNS} FROM agents WHERE id = ?1 AND workspace_id = ?2"))
        .bind(id)
        .bind(ws)
        .fetch_optional(db)
        .await
}

/// Sets the earned autonomy level (0 to 3). A promotion above the admin's cap is refused; a
/// demotion always works. Returns true when the level was written. This is the only writer of
/// `agents.autonomy_level`, so the evals gate and the incident demotion go through it.
pub async fn set_autonomy_level(conn: &mut SqliteConnection, ws: &str, agent_id: &str, level: i64) -> sqlx::Result<bool> {
    if !(0..=i64::from(super::spec::MAX_AUTONOMY)).contains(&level) {
        return Ok(false);
    }
    Ok(sqlx::query(
        "UPDATE agents SET autonomy_level = ?1, updated_at = ?2
         WHERE id = ?3 AND workspace_id = ?4 AND (?1 <= autonomy_level OR ?1 <= autonomy_cap)",
    )
    .bind(level)
    .bind(now_ms())
    .bind(agent_id)
    .bind(ws)
    .execute(conn)
    .await?
    .rows_affected()
        == 1)
}
