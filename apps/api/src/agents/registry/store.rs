//! Persistence of workspace agents. Every lookup is scoped by workspace.

use sqlx::{SqliteConnection, SqliteExecutor};

use crate::platform::db::now_ms;

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

pub async fn list<'e>(db: impl SqliteExecutor<'e>, ws: &str) -> sqlx::Result<Vec<AgentRow>> {
    sqlx::query_as(&format!(
        "SELECT {AGENT_COLUMNS} FROM agents WHERE workspace_id = ?1 ORDER BY kind = 'lead' DESC, created_at, name, id"
    ))
    .bind(ws)
    .fetch_all(db)
    .await
}

pub async fn lead<'e>(db: impl SqliteExecutor<'e>, ws: &str) -> sqlx::Result<Option<AgentRow>> {
    sqlx::query_as(&format!("SELECT {AGENT_COLUMNS} FROM agents WHERE workspace_id = ?1 AND kind = 'lead'"))
        .bind(ws)
        .fetch_optional(db)
        .await
}

pub async fn by_template<'e>(db: impl SqliteExecutor<'e>, ws: &str, template_key: &str) -> sqlx::Result<Option<AgentRow>> {
    sqlx::query_as(&format!("SELECT {AGENT_COLUMNS} FROM agents WHERE workspace_id = ?1 AND template_key = ?2 ORDER BY created_at LIMIT 1"))
        .bind(ws)
        .bind(template_key)
        .fetch_optional(db)
        .await
}

pub async fn personal_of<'e>(db: impl SqliteExecutor<'e>, ws: &str, user_id: &str) -> sqlx::Result<Option<AgentRow>> {
    sqlx::query_as(&format!("SELECT {AGENT_COLUMNS} FROM agents WHERE workspace_id = ?1 AND scope = 'personal' AND owner_user_id = ?2"))
        .bind(ws)
        .bind(user_id)
        .fetch_optional(db)
        .await
}

/// A personal agent belongs to its owner alone; every other agent is visible to all members.
pub fn visible_to(agent: &AgentRow, user_id: &str) -> bool {
    !agent.is_personal() || agent.owner_user_id.as_deref() == Some(user_id)
}

/// SQL predicate (on a table with an `agent_id` column) that hides other members' personal agents' work.
/// Binds nothing: the viewer id is passed as the numbered placeholder `viewer`.
pub fn hide_foreign_personal(table: &str, viewer: &str) -> String {
    format!(
        "({table}.agent_id IS NULL OR NOT EXISTS (SELECT 1 FROM agents pa WHERE pa.id = {table}.agent_id AND pa.scope = 'personal'
                                                   AND pa.owner_user_id IS NOT {viewer}))"
    )
}

/// The agent behind an audited case, approval or draft, if it has one.
pub async fn entity_agent<'e>(db: impl SqliteExecutor<'e>, ws: &str, entity_type: &str, entity_id: &str) -> sqlx::Result<Option<String>> {
    let table = match entity_type {
        "case" => "cases",
        "agent_approval" => "agent_approvals",
        "draft" => "drafts",
        _ => return Ok(None),
    };
    Ok(sqlx::query_scalar::<_, Option<String>>(&format!("SELECT agent_id FROM {table} WHERE id = ?1 AND workspace_id = ?2"))
        .bind(entity_id)
        .bind(ws)
        .fetch_optional(db)
        .await?
        .flatten())
}

/// May this member see work done by this agent (a case, an approval, a draft)?
pub async fn can_see<'e>(db: impl SqliteExecutor<'e>, ws: &str, agent_id: Option<&str>, user_id: &str) -> sqlx::Result<bool> {
    let Some(agent_id) = agent_id else { return Ok(true) };
    let personal_of: Option<Option<String>> =
        sqlx::query_scalar("SELECT owner_user_id FROM agents WHERE id = ?1 AND workspace_id = ?2 AND scope = 'personal'")
            .bind(agent_id)
            .bind(ws)
            .fetch_optional(db)
            .await?;
    Ok(personal_of.is_none_or(|owner| owner.as_deref() == Some(user_id)))
}

pub struct NewAgent<'a> {
    pub workspace_id: &'a str,
    pub key: &'a str,
    pub kind: &'a str,
    pub scope: &'a str,
    pub owner_user_id: Option<&'a str>,
    pub category: &'a str,
    pub template_key: Option<&'a str>,
    pub template_version: Option<i64>,
    pub cloned_from: Option<&'a str>,
    pub name: &'a str,
    pub avatar: &'a str,
    pub color: &'a str,
    pub description: &'a str,
    pub status: &'a str,
    pub autonomy_level: i64,
    pub autonomy_cap: i64,
    pub created_by: Option<&'a str>,
}

/// Inserts an agent unless one with this key (or, for a personal agent, this owner) exists.
/// Returns the new id, or `None` when it already existed.
pub async fn insert(conn: &mut SqliteConnection, a: NewAgent<'_>) -> sqlx::Result<Option<String>> {
    let id = crate::platform::db::new_id("agt");
    let now = now_ms();
    let created = sqlx::query(
        "INSERT OR IGNORE INTO agents (id, workspace_id, key, kind, scope, owner_user_id, category, template_key, template_version, cloned_from,
                                       name, avatar, color, description, status, autonomy_level, autonomy_cap, created_by, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?19)",
    )
    .bind(&id)
    .bind(a.workspace_id)
    .bind(a.key)
    .bind(a.kind)
    .bind(a.scope)
    .bind(a.owner_user_id)
    .bind(a.category)
    .bind(a.template_key)
    .bind(a.template_version)
    .bind(a.cloned_from)
    .bind(a.name)
    .bind(a.avatar)
    .bind(a.color)
    .bind(a.description)
    .bind(a.status)
    .bind(a.autonomy_level)
    .bind(a.autonomy_cap)
    .bind(a.created_by)
    .bind(now)
    .execute(conn)
    .await?
    .rows_affected()
        == 1;
    Ok(created.then_some(id))
}

pub async fn set_status(conn: &mut SqliteConnection, ws: &str, agent_id: &str, status: &str, reason: Option<&str>) -> sqlx::Result<bool> {
    Ok(sqlx::query("UPDATE agents SET status = ?1, paused_reason = ?2, updated_at = ?3 WHERE id = ?4 AND workspace_id = ?5")
        .bind(status)
        .bind(reason)
        .bind(now_ms())
        .bind(agent_id)
        .bind(ws)
        .execute(conn)
        .await?
        .rows_affected()
        == 1)
}

/// The cap an admin (or a personal agent's owner) grants. Lowering it also lowers the earned level.
pub async fn set_autonomy_cap(conn: &mut SqliteConnection, ws: &str, agent_id: &str, cap: i64) -> sqlx::Result<bool> {
    if !(0..=i64::from(super::spec::MAX_AUTONOMY)).contains(&cap) {
        return Ok(false);
    }
    Ok(sqlx::query(
        "UPDATE agents SET autonomy_cap = ?1, autonomy_level = MIN(autonomy_level, ?1), updated_at = ?2 WHERE id = ?3 AND workspace_id = ?4",
    )
    .bind(cap)
    .bind(now_ms())
    .bind(agent_id)
    .bind(ws)
    .execute(conn)
    .await?
    .rows_affected()
        == 1)
}

pub async fn set_identity(conn: &mut SqliteConnection, ws: &str, agent_id: &str, name: &str, avatar: &str, color: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE agents SET name = ?1, avatar = ?2, color = ?3, updated_at = ?4 WHERE id = ?5 AND workspace_id = ?6")
        .bind(name)
        .bind(avatar)
        .bind(color)
        .bind(now_ms())
        .bind(agent_id)
        .bind(ws)
        .execute(conn)
        .await?;
    Ok(())
}

/// Stops automatic sends that are still held (not yet being sent, not requested by a person):
/// those of one agent, or of every agent when `agent_id` is `None`, optionally only for one email.
/// The held reply stays a draft and waits for a person.
pub async fn release_held_sends(conn: &mut SqliteConnection, ws: &str, agent_id: Option<&str>, message_id: Option<&str>) -> sqlx::Result<u64> {
    Ok(sqlx::query(
        "UPDATE drafts SET send_at = NULL, updated_at = ?4
         WHERE workspace_id = ?1 AND agent_id IS NOT NULL AND (?2 IS NULL OR agent_id = ?2) AND (?3 IS NULL OR message_id = ?3)
           AND send_at IS NOT NULL AND send_attempt_id IS NULL AND send_requested_by IS NULL AND status != 'sent'",
    )
    .bind(ws)
    .bind(agent_id)
    .bind(message_id)
    .bind(now_ms())
    .execute(conn)
    .await?
    .rows_affected())
}

/// The workspace's "let routine replies go" switch (Meera's permissions) moves the default agents
/// (the lead and the personal agents) when it CHANGES: anything but `auto` caps them at level 1.
/// Turning it on raises nothing: levels are earned and caps are granted by an admin.
pub async fn sync_default_agents_from_settings(conn: &mut SqliteConnection, ws: &str, previous_send: &str, send: &str) -> sqlx::Result<()> {
    if previous_send == send || send == "auto" {
        return Ok(());
    }
    sqlx::query(
        "UPDATE agents SET autonomy_level = MIN(autonomy_level, 1), updated_at = ?2
         WHERE workspace_id = ?1 AND (kind = 'lead' OR scope = 'personal') AND autonomy_level > 1",
    )
    .bind(ws)
    .bind(now_ms())
    .execute(&mut *conn)
    .await?;
    release_held_sends(conn, ws, None, None).await?;
    Ok(())
}
