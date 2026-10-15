//! Learned style preferences: how a reply is written, scoped to (agent, person). Kept apart from facts.
//! Suggested until a person accepts them (or enough independent evidence builds up); archived ones are
//! tombstones that are never relearned.

use std::collections::HashSet;

use serde::Serialize;
use serde_json::{Value, json};
use sqlx::SqliteConnection;

use super::guard::{self, PROCESS, Rejection};
use crate::{
    platform::db::{iso, new_id, now_ms},
    learning::memory::dedupe_key,
};

pub const STATUSES: &[&str] = &["suggested", "active", "archived"];
pub const SOURCES: &[&str] = &["edit", "decline_note", "manual"];
/// Evidence needed to activate a style preference without a person: this many pieces from at least two drafts.
pub const AUTO_ACTIVATE_EVIDENCE: i64 = 3;
pub const AUTO_ACTIVATE_DRAFTS: usize = 2;

pub const COLUMNS: &str =
    "id, workspace_id, agent_id, user_id, category, content, status, source, evidence_count, evidence_json, use_count, embedding, created_at, updated_at";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Preference {
    pub id: String,
    pub workspace_id: String,
    pub agent_id: Option<String>,
    pub user_id: Option<String>,
    pub category: String,
    pub content: String,
    pub status: String,
    pub source: String,
    pub evidence_count: i64,
    pub evidence_json: String,
    pub use_count: i64,
    pub embedding: Option<Vec<u8>>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PreferenceView {
    pub id: String,
    pub agent_id: Option<String>,
    pub user_id: Option<String>,
    pub category: String,
    pub content: String,
    pub status: String,
    pub source: String,
    pub evidence_count: i64,
    pub use_count: i64,
    pub updated_at: String,
}

impl From<Preference> for PreferenceView {
    fn from(p: Preference) -> Self {
        Self {
            id: p.id,
            agent_id: p.agent_id,
            user_id: p.user_id,
            category: p.category,
            content: p.content,
            status: p.status,
            source: p.source,
            evidence_count: p.evidence_count,
            use_count: p.use_count,
            updated_at: iso(p.updated_at),
        }
    }
}

/// Who a preference is for: the workspace, optionally one agent, optionally one person.
#[derive(Debug, Clone, Copy)]
pub struct Target<'a> {
    pub ws: &'a str,
    pub agent_id: Option<&'a str>,
    pub user_id: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Created(String),
    Merged(String),
    /// A person archived this before: it is never relearned.
    Tombstoned,
    Rejected(Rejection),
}

fn evidence_of(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

fn auto_activates(category: &str, source: &str, evidence: &[String]) -> bool {
    if category == PROCESS || source != "edit" {
        return false;
    }
    let drafts: HashSet<&str> = evidence.iter().map(|e| e.split('#').next().unwrap_or(e)).collect();
    i64::try_from(evidence.len()).unwrap_or(0) >= AUTO_ACTIVATE_EVIDENCE && drafts.len() >= AUTO_ACTIVATE_DRAFTS
}

/// Stores a lesson as a suggestion (or adds evidence to the matching one). `evidence_ref` identifies what
/// taught it (a draft id, or `draft#n` for another piece from the same draft); the same ref counts once.
pub async fn suggest(
    conn: &mut SqliteConnection,
    t: &Target<'_>,
    category: &str,
    content: &str,
    source: &str,
    evidence_ref: &str,
) -> sqlx::Result<Outcome> {
    let content = match guard::check_preference(category, content) {
        Ok(c) => c,
        Err(r) => return Ok(Outcome::Rejected(r)),
    };
    let key = dedupe_key(&content);
    let existing: Option<Preference> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM agent_preferences
         WHERE workspace_id = ?1 AND COALESCE(agent_id, '') = COALESCE(?2, '') AND COALESCE(user_id, '') = COALESCE(?3, '') AND dedupe_key = ?4"
    ))
    .bind(t.ws)
    .bind(t.agent_id)
    .bind(t.user_id)
    .bind(&key)
    .fetch_optional(&mut *conn)
    .await?;
    let now = now_ms();
    match existing {
        None => {
            let id = new_id("pref");
            sqlx::query(
                "INSERT INTO agent_preferences (id, workspace_id, agent_id, user_id, category, content, status, source, dedupe_key,
                                                evidence_count, evidence_json, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'suggested', ?7, ?8, 1, ?9, ?10, ?10)",
            )
            .bind(&id)
            .bind(t.ws)
            .bind(t.agent_id)
            .bind(t.user_id)
            .bind(category)
            .bind(&content)
            .bind(source)
            .bind(&key)
            .bind(json!([evidence_ref]).to_string())
            .bind(now)
            .execute(&mut *conn)
            .await?;
            Ok(Outcome::Created(id))
        }
        Some(p) if p.status == "archived" => Ok(Outcome::Tombstoned),
        Some(p) => {
            let mut evidence = evidence_of(&p.evidence_json);
            if !evidence.iter().any(|e| e == evidence_ref) {
                evidence.push(evidence_ref.to_string());
            }
            let status = if p.status == "suggested" && auto_activates(&p.category, &p.source, &evidence) { "active" } else { p.status.as_str() };
            sqlx::query("UPDATE agent_preferences SET evidence_count = ?1, evidence_json = ?2, status = ?3, updated_at = ?4 WHERE id = ?5")
                .bind(i64::try_from(evidence.len()).unwrap_or(i64::MAX))
                .bind(Value::from(evidence).to_string())
                .bind(status)
                .bind(now)
                .bind(&p.id)
                .execute(&mut *conn)
                .await?;
            Ok(Outcome::Merged(p.id))
        }
    }
}

/// A person wrote this preference: active at once (still guard-checked). Re-adds an archived one.
pub async fn create_manual(
    conn: &mut SqliteConnection,
    t: &Target<'_>,
    category: &str,
    content: &str,
) -> sqlx::Result<Result<String, Rejection>> {
    let content = match guard::check_preference(category, content) {
        Ok(c) => c,
        Err(r) => return Ok(Err(r)),
    };
    let key = dedupe_key(&content);
    let now = now_ms();
    let id = new_id("pref");
    let id: String = sqlx::query_scalar(
        "INSERT INTO agent_preferences (id, workspace_id, agent_id, user_id, category, content, status, source, dedupe_key,
                                        evidence_count, evidence_json, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', 'manual', ?7, 1, '[]', ?8, ?8)
         ON CONFLICT(workspace_id, COALESCE(agent_id, ''), COALESCE(user_id, ''), dedupe_key)
         DO UPDATE SET status = 'active', source = 'manual', updated_at = excluded.updated_at
         RETURNING id",
    )
    .bind(&id)
    .bind(t.ws)
    .bind(t.agent_id)
    .bind(t.user_id)
    .bind(category)
    .bind(&content)
    .bind(&key)
    .bind(now)
    .fetch_one(&mut *conn)
    .await?;
    Ok(Ok(id))
}

pub async fn get(conn: &mut SqliteConnection, ws: &str, id: &str) -> sqlx::Result<Option<Preference>> {
    sqlx::query_as(&format!("SELECT {COLUMNS} FROM agent_preferences WHERE id = ?1 AND workspace_id = ?2"))
        .bind(id)
        .bind(ws)
        .fetch_optional(conn)
        .await
}

pub async fn set_status(conn: &mut SqliteConnection, ws: &str, id: &str, status: &str) -> sqlx::Result<bool> {
    Ok(sqlx::query("UPDATE agent_preferences SET status = ?1, updated_at = ?2 WHERE id = ?3 AND workspace_id = ?4")
        .bind(status)
        .bind(now_ms())
        .bind(id)
        .bind(ws)
        .execute(conn)
        .await?
        .rows_affected()
        == 1)
}

/// Rewrites a preference's text (guard-checked); a person's rewrite makes it active.
pub async fn set_content(conn: &mut SqliteConnection, ws: &str, id: &str, category: Option<&str>, content: &str) -> sqlx::Result<Result<bool, Rejection>> {
    let Some(current) = get(conn, ws, id).await? else { return Ok(Ok(false)) };
    let category = category.unwrap_or(&current.category);
    let content = match guard::check_preference(category, content) {
        Ok(c) => c,
        Err(r) => return Ok(Err(r)),
    };
    let updated = sqlx::query(
        "UPDATE agent_preferences SET content = ?1, category = ?2, dedupe_key = ?3, status = CASE WHEN status = 'archived' THEN status ELSE 'active' END,
                embedding = NULL, updated_at = ?4
         WHERE id = ?5 AND workspace_id = ?6",
    )
    .bind(&content)
    .bind(category)
    .bind(dedupe_key(&content))
    .bind(now_ms())
    .bind(id)
    .bind(ws)
    .execute(conn)
    .await;
    match updated {
        Ok(r) => Ok(Ok(r.rows_affected() == 1)),
        // Same text already exists for this scope.
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Ok(Err(Rejection::Instruction)),
        Err(e) => Err(e),
    }
}

pub async fn delete(conn: &mut SqliteConnection, ws: &str, id: &str) -> sqlx::Result<bool> {
    Ok(sqlx::query("DELETE FROM agent_preferences WHERE id = ?1 AND workspace_id = ?2").bind(id).bind(ws).execute(conn).await?.rows_affected() == 1)
}

pub async fn set_embedding(conn: &mut SqliteConnection, id: &str, blob: &[u8]) -> sqlx::Result<()> {
    sqlx::query("UPDATE agent_preferences SET embedding = ?1 WHERE id = ?2").bind(blob).bind(id).execute(conn).await?;
    Ok(())
}

pub async fn record_use(conn: &mut SqliteConnection, ids: &[String]) -> sqlx::Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    sqlx::query("UPDATE agent_preferences SET use_count = use_count + 1 WHERE id IN (SELECT value FROM json_each(?1))")
        .bind(json!(ids).to_string())
        .execute(conn)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_activates_at_three_pieces_from_two_drafts() {
        let ev = |v: &[&str]| v.iter().map(ToString::to_string).collect::<Vec<_>>();
        assert!(!auto_activates("tone", "edit", &ev(&["d1", "d2"])));
        assert!(!auto_activates("tone", "edit", &ev(&["d1", "d1#2", "d1#3"])), "one draft is not independent evidence");
        assert!(auto_activates("tone", "edit", &ev(&["d1", "d1#2", "d2"])));
        assert!(auto_activates("tone", "edit", &ev(&["d1", "d2", "d3"])));
        assert!(!auto_activates("process", "edit", &ev(&["d1", "d2", "d3"])), "process always needs a person");
        assert!(!auto_activates("tone", "decline_note", &ev(&["d1", "d2", "d3"])));
    }
}
