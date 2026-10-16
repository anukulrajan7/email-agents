//! The log of every identity change: link, unlink, create person, mark external, not a person, reopen. A row keeps what it
//! touched and what was there before, so it can be undone; the row that undoes it is linked back (`undone_by`).

use serde_json::{Value, json};
use sqlx::{SqliteConnection, SqlitePool};

use crate::platform::{
    audit::Actor,
    db::{iso, new_id, now_ms},
};

pub struct NewChange<'a> {
    pub ws: &'a str,
    pub kind: &'a str,
    pub person_id: Option<&'a str>,
    pub identity_id: Option<&'a str>,
    pub record_ids: &'a [String],
    pub before: Value,
    pub after: Value,
    pub actor: Actor<'a>,
    pub evidence: Value,
}

/// Writes one change and returns its id.
pub async fn record(conn: &mut SqliteConnection, c: NewChange<'_>) -> sqlx::Result<String> {
    let id = new_id("idc");
    let (class, actor_id) = super::actor_parts(c.actor);
    sqlx::query(
        "INSERT INTO identity_changes (id, workspace_id, kind, person_id, identity_id, record_ids_json, before_json, after_json, actor_class, actor_id, evidence_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
    )
    .bind(&id)
    .bind(c.ws)
    .bind(c.kind)
    .bind(c.person_id)
    .bind(c.identity_id)
    .bind(json!(c.record_ids).to_string())
    .bind(c.before.to_string())
    .bind(c.after.to_string())
    .bind(class)
    .bind(actor_id)
    .bind(c.evidence.to_string())
    .bind(now_ms())
    .execute(conn)
    .await?;
    Ok(id)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Change {
    pub id: String,
    pub kind: String,
    pub person_id: Option<String>,
    pub identity_id: Option<String>,
    pub record_ids_json: String,
    pub before_json: String,
    pub after_json: String,
    pub actor_class: String,
    pub actor_id: Option<String>,
    pub undone_by: Option<String>,
    pub created_at: i64,
}

impl Change {
    pub fn record_ids(&self) -> Vec<String> {
        serde_json::from_str(&self.record_ids_json).unwrap_or_default()
    }

    pub fn before(&self) -> Value {
        serde_json::from_str(&self.before_json).unwrap_or(Value::Null)
    }

    pub fn after(&self) -> Value {
        serde_json::from_str(&self.after_json).unwrap_or(Value::Null)
    }

    /// Links, "external" and "not a person" can be undone, once. Creating a person is undone by archiving them on the People page.
    pub fn undoable(&self) -> bool {
        self.undone_by.is_none() && matches!(self.kind.as_str(), "link" | "mark_external" | "dismiss")
    }
}

const SELECT: &str = "SELECT id, kind, person_id, identity_id, record_ids_json, before_json, after_json, actor_class, actor_id, undone_by, created_at FROM identity_changes";

pub async fn get(conn: &mut SqliteConnection, ws: &str, id: &str) -> sqlx::Result<Option<Change>> {
    sqlx::query_as(&format!("{SELECT} WHERE workspace_id = ?1 AND id = ?2")).bind(ws).bind(id).fetch_optional(conn).await
}

pub async fn mark_undone(conn: &mut SqliteConnection, ws: &str, id: &str, by: &str) -> sqlx::Result<bool> {
    Ok(sqlx::query("UPDATE identity_changes SET undone_by = ?1 WHERE workspace_id = ?2 AND id = ?3 AND undone_by IS NULL").bind(by).bind(ws).bind(id).execute(conn).await?.rows_affected() == 1)
}

/// What the admin sees as "recent": the latest changes with the names, and whether each can still be undone.
pub async fn recent(db: &SqlitePool, ws: &str, limit: i64) -> sqlx::Result<Vec<Value>> {
    let rows: Vec<Change> = sqlx::query_as(&format!("{SELECT} WHERE workspace_id = ?1 AND kind != 'alias' ORDER BY created_at DESC, rowid DESC LIMIT ?2")).bind(ws).bind(limit).fetch_all(db).await?;
    let mut out = vec![];
    for c in rows {
        let person: Option<String> = match &c.person_id {
            Some(p) => sqlx::query_scalar("SELECT name FROM people WHERE id = ?1 AND workspace_id = ?2").bind(p).bind(ws).fetch_optional(db).await?,
            None => None,
        };
        let name: Option<String> = match &c.identity_id {
            Some(i) => sqlx::query_scalar("SELECT name_as_written FROM unresolved_identities WHERE id = ?1 AND workspace_id = ?2").bind(i).bind(ws).fetch_optional(db).await?.flatten(),
            None => None,
        };
        // A link that came with "add as a person": undoing it moves the sheets back, the person stays on People.
        let added_person: bool = match (&c.person_id, &c.identity_id) {
            (Some(p), Some(i)) if c.kind == "link" => {
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM identity_changes WHERE workspace_id = ?1 AND kind = 'create_person' AND person_id = ?2 AND identity_id = ?3)").bind(ws).bind(p).bind(i).fetch_one(db).await?
            }
            _ => false,
        };
        out.push(json!({
            "id": c.id, "kind": c.kind, "person_id": c.person_id, "person_name": person, "name": name, "added_person": added_person,
            "record_count": c.record_ids().len(), "by": c.actor_class, "undone": c.undone_by.is_some(), "can_undo": c.undoable(), "at": iso(c.created_at),
        }));
    }
    Ok(out)
}
