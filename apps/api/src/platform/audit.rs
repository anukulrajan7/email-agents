use serde_json::Value;
use sqlx::SqliteConnection;

use crate::db::{new_id, now_ms};

#[derive(Debug, Clone, Copy)]
pub enum Actor<'a> {
    User(&'a str),
    System,
}

pub struct Entry<'a> {
    pub workspace_id: &'a str,
    pub actor: Actor<'a>,
    pub action: &'a str,
    pub entity_type: &'a str,
    pub entity_id: &'a str,
    pub message_id: Option<&'a str>,
    /// IDs and enum values only. Never email bodies, tokens or addresses.
    pub metadata: Value,
}

pub async fn record(conn: &mut SqliteConnection, e: Entry<'_>) -> sqlx::Result<()> {
    let (actor_type, actor_id) = match e.actor {
        Actor::User(id) => ("user", Some(id)),
        Actor::System => ("system", None),
    };
    sqlx::query(
        "INSERT INTO audit_events (id, workspace_id, actor_type, actor_id, action, entity_type, entity_id, message_id, metadata_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )
    .bind(new_id("aud"))
    .bind(e.workspace_id)
    .bind(actor_type)
    .bind(actor_id)
    .bind(e.action)
    .bind(e.entity_type)
    .bind(e.entity_id)
    .bind(e.message_id)
    .bind(e.metadata.to_string())
    .bind(now_ms())
    .execute(conn)
    .await?;
    Ok(())
}
