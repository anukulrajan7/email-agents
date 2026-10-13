//! Agent versions: every change to an agent's spec is a new immutable version. Rolling back copies
//! an earlier version forward (so history only grows) and restores the automation versions it pinned.

use serde::Serialize;
use serde_json::{Map, Value, json};
use sqlx::{SqliteConnection, SqliteExecutor};

use super::{
    spec::AgentSpec,
    store::{self, AgentRow},
    templates,
};
use crate::db::{iso, now_ms};

pub const SOURCES: &[&str] = &["template", "tuned", "designer", "manual", "rollback", "backfill"];

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct VersionRow {
    pub agent_id: String,
    pub version: i64,
    pub spec_json: String,
    pub spec_hash: String,
    pub source: String,
    pub designer_session_id: Option<String>,
    pub note: Option<String>,
    pub gate_status: String,
    pub automation_pins_json: String,
    pub created_by: Option<String>,
    pub created_at: i64,
}

const VERSION_COLUMNS: &str =
    "agent_id, version, spec_json, spec_hash, source, designer_session_id, note, gate_status, automation_pins_json, created_by, created_at";

impl VersionRow {
    pub fn spec(&self) -> Option<AgentSpec> {
        serde_json::from_str(&self.spec_json).ok()
    }

    pub fn info(&self, active_version: Option<i64>) -> Value {
        json!({
            "version": self.version, "source": self.source, "gate_status": self.gate_status, "note": self.note,
            "spec_hash": self.spec_hash, "active": active_version == Some(self.version), "created_at": iso(self.created_at),
        })
    }
}

/// Called whenever a version is saved or activated. Replaced by the evals gate (safety and replay
/// suites) when that package merges; until then a version stays `pending`.
pub async fn on_new_version(_conn: &mut SqliteConnection, _ws: &str, _agent_id: &str, _version: i64) -> sqlx::Result<()> {
    Ok(())
}

pub async fn list<'e>(db: impl SqliteExecutor<'e>, agent_id: &str) -> sqlx::Result<Vec<VersionRow>> {
    sqlx::query_as(&format!("SELECT {VERSION_COLUMNS} FROM agent_versions WHERE agent_id = ?1 ORDER BY version DESC"))
        .bind(agent_id)
        .fetch_all(db)
        .await
}

pub async fn get<'e>(db: impl SqliteExecutor<'e>, agent_id: &str, version: i64) -> sqlx::Result<Option<VersionRow>> {
    sqlx::query_as(&format!("SELECT {VERSION_COLUMNS} FROM agent_versions WHERE agent_id = ?1 AND version = ?2"))
        .bind(agent_id)
        .bind(version)
        .fetch_optional(db)
        .await
}

/// The automations this agent owns and the version each is at, so a rollback can restore them.
async fn automation_pins(conn: &mut SqliteConnection, ws: &str, agent_id: &str) -> sqlx::Result<Value> {
    let rows: Vec<(String, i64)> = sqlx::query_as("SELECT id, version FROM automations WHERE workspace_id = ?1 AND agent_id = ?2")
        .bind(ws)
        .bind(agent_id)
        .fetch_all(conn)
        .await?;
    Ok(Value::Object(rows.into_iter().map(|(id, v)| (id, json!(v))).collect::<Map<_, _>>()))
}

pub struct NewVersion<'a> {
    pub source: &'a str,
    pub note: Option<&'a str>,
    pub created_by: Option<&'a str>,
    pub designer_session_id: Option<&'a str>,
    /// Make it the agent's active version.
    pub activate: bool,
}

/// Appends a version (numbered after the highest) and optionally activates it.
pub async fn create(conn: &mut SqliteConnection, ws: &str, agent_id: &str, spec: &AgentSpec, new: NewVersion<'_>) -> sqlx::Result<i64> {
    debug_assert!(SOURCES.contains(&new.source));
    let pins = automation_pins(conn, ws, agent_id).await?;
    let version: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) + 1 FROM agent_versions WHERE agent_id = ?1")
        .bind(agent_id)
        .fetch_one(&mut *conn)
        .await?;
    sqlx::query(
        "INSERT INTO agent_versions (agent_id, version, spec_json, spec_hash, source, designer_session_id, note, automation_pins_json,
                                     created_by, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )
    .bind(agent_id)
    .bind(version)
    .bind(spec.to_json())
    .bind(spec.hash())
    .bind(new.source)
    .bind(new.designer_session_id)
    .bind(new.note)
    .bind(pins.to_string())
    .bind(new.created_by)
    .bind(now_ms())
    .execute(&mut *conn)
    .await?;
    if new.activate {
        sqlx::query("UPDATE agents SET active_version = ?1, updated_at = ?2 WHERE id = ?3 AND workspace_id = ?4")
            .bind(version)
            .bind(now_ms())
            .bind(agent_id)
            .bind(ws)
            .execute(&mut *conn)
            .await?;
    }
    on_new_version(conn, ws, agent_id, version).await?;
    Ok(version)
}

/// The spec an agent runs now and its version. An agent with no stored version (never happens after
/// seeding) runs its template, else the lead's.
pub async fn active_spec(conn: &mut SqliteConnection, agent: &AgentRow) -> sqlx::Result<(AgentSpec, i64)> {
    if let Some(v) = agent.active_version
        && let Some(spec) = get(&mut *conn, &agent.id, v).await?.and_then(|r| r.spec())
    {
        return Ok((spec, v));
    }
    let template = agent.template_key.as_deref().and_then(templates::template).unwrap_or_else(templates::lead_template);
    Ok((template.spec(), 0))
}

pub async fn active_spec_in(db: &sqlx::SqlitePool, agent: &AgentRow) -> sqlx::Result<(AgentSpec, i64)> {
    let mut conn = db.acquire().await?;
    active_spec(&mut conn, agent).await
}

/// A change that widens what an agent may do or how it behaves: tools, permissions, steps or
/// drafting. Such a version is capped at level 1 until it passes the regression gate.
pub fn behaviour_changed(old: &AgentSpec, new: &AgentSpec) -> bool {
    let mut old_tools = old.tools.clone();
    let mut new_tools = new.tools.clone();
    old_tools.sort();
    new_tools.sort();
    old_tools != new_tools || old.permissions != new.permissions || old.steps != new.steps || old.drafting != new.drafting
}

/// Caps the agent at level 1 when its behaviour changed. Returns true when the level was lowered.
pub async fn cap_after_behaviour_change(conn: &mut SqliteConnection, ws: &str, agent: &AgentRow, old: &AgentSpec, new: &AgentSpec) -> sqlx::Result<bool> {
    if agent.autonomy_level > 1 && behaviour_changed(old, new) {
        return store::set_autonomy_level(conn, ws, &agent.id, 1).await;
    }
    Ok(false)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DiffEntry {
    pub path: String,
    pub before: Value,
    pub after: Value,
}

fn walk(path: &str, before: &Value, after: &Value, out: &mut Vec<DiffEntry>) {
    match (before, after) {
        (Value::Object(a), Value::Object(b)) => {
            let mut keys: Vec<&String> = a.keys().chain(b.keys()).collect();
            keys.sort();
            keys.dedup();
            for k in keys {
                let child = if path.is_empty() { k.to_string() } else { format!("{path}.{k}") };
                walk(&child, a.get(k).unwrap_or(&Value::Null), b.get(k).unwrap_or(&Value::Null), out);
            }
        }
        (a, b) if a != b => out.push(DiffEntry { path: path.to_string(), before: a.clone(), after: b.clone() }),
        _ => {}
    }
}

/// Field-by-field changes from `before` to `after` (lists compare as a whole).
pub fn diff(before: &AgentSpec, after: &AgentSpec) -> Vec<DiffEntry> {
    let mut out = Vec::new();
    walk("", &serde_json::to_value(before).unwrap_or(Value::Null), &serde_json::to_value(after).unwrap_or(Value::Null), &mut out);
    out
}

/// Puts the automations this version pinned back to the pinned definitions.
async fn restore_pins(conn: &mut SqliteConnection, ws: &str, pins_json: &str) -> sqlx::Result<()> {
    let pins: Map<String, Value> = serde_json::from_str(pins_json).unwrap_or_default();
    for (automation_id, version) in pins {
        let Some(version) = version.as_i64() else { continue };
        let def: Option<String> = sqlx::query_scalar(
            "SELECT v.definition_json FROM automation_versions v JOIN automations a ON a.id = v.automation_id
             WHERE v.automation_id = ?1 AND v.version = ?2 AND a.workspace_id = ?3",
        )
        .bind(&automation_id)
        .bind(version)
        .bind(ws)
        .fetch_optional(&mut *conn)
        .await?;
        let Some(def) = def.and_then(|d| serde_json::from_str::<Value>(&d).ok()) else { continue };
        sqlx::query(
            "UPDATE automations SET name = COALESCE(?1, name), enabled = COALESCE(?2, enabled), conditions_json = COALESCE(?3, conditions_json),
                                    actions_json = COALESCE(?4, actions_json), version = ?5, updated_at = ?6
             WHERE id = ?7 AND workspace_id = ?8",
        )
        .bind(def.get("name").and_then(Value::as_str))
        .bind(def.get("enabled").and_then(Value::as_bool))
        .bind(def.get("conditions").map(Value::to_string))
        .bind(def.get("actions").map(Value::to_string))
        .bind(version)
        .bind(now_ms())
        .bind(&automation_id)
        .bind(ws)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub enum ActivateError {
    NoSuchVersion,
    Unreadable,
}

/// Activates an earlier (or any) version by copying it forward as a new `rollback` version, and
/// restores its automation pins. Returns the new version number.
pub async fn activate(conn: &mut SqliteConnection, ws: &str, agent: &AgentRow, version: i64, actor: Option<&str>) -> Result<Result<i64, ActivateError>, sqlx::Error> {
    let Some(source) = get(&mut *conn, &agent.id, version).await? else { return Ok(Err(ActivateError::NoSuchVersion)) };
    let Some(spec) = source.spec() else { return Ok(Err(ActivateError::Unreadable)) };
    let previous = active_spec(&mut *conn, agent).await?.0;
    let note = format!("Rolled back to version {version}");
    let new_version = create(&mut *conn, ws, &agent.id, &spec, NewVersion { source: "rollback", note: Some(&note), created_by: actor, designer_session_id: None, activate: true }).await?;
    sqlx::query("UPDATE agent_versions SET automation_pins_json = ?1 WHERE agent_id = ?2 AND version = ?3")
        .bind(&source.automation_pins_json)
        .bind(&agent.id)
        .bind(new_version)
        .execute(&mut *conn)
        .await?;
    restore_pins(conn, ws, &source.automation_pins_json).await?;
    cap_after_behaviour_change(conn, ws, agent, &previous, &spec).await?;
    Ok(Ok(new_version))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_registry::templates::{INBOX_KEY, template};

    #[test]
    fn diff_lists_changed_fields_only() {
        let a = template(INBOX_KEY).unwrap().spec();
        let mut b = a.clone();
        b.drafting.rules.push("Be brief".into());
        b.budget.max_steps = 9;
        let d = diff(&a, &b);
        assert_eq!(d.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(), ["budget.max_steps", "drafting.rules"]);
        assert_eq!(d[0].before, json!(15));
        assert_eq!(d[0].after, json!(9));
        assert!(diff(&a, &a).is_empty());
    }

    #[test]
    fn only_widening_changes_cap_the_level() {
        let a = template(INBOX_KEY).unwrap().spec();
        let mut reordered = a.clone();
        reordered.tools.reverse();
        reordered.description = "Different words".into();
        reordered.budget.max_steps = 9;
        assert!(!behaviour_changed(&a, &reordered), "tools in another order, text and budget do not matter");
        let mut tools = a.clone();
        tools.tools.push("create_task".into());
        assert!(behaviour_changed(&a, &tools) || a.has_tool("create_task"));
        let mut drafting = a.clone();
        drafting.drafting.never_do.push("Offer a discount".into());
        assert!(behaviour_changed(&a, &drafting));
        let mut perms = a.clone();
        perms.permissions.daily_send_cap = 99;
        assert!(behaviour_changed(&a, &perms));
        let mut steps = a.clone();
        steps.steps.clear();
        assert!(behaviour_changed(&a, &steps));
    }
}
