//! Agent memory: learned contact profiles and user/learned notes, retrieved with
//! SQLite FTS5. Memories are *trusted* context written or accepted by staff; they are
//! kept separate from the untrusted email text in every prompt.

use std::collections::HashSet;

use serde_json::json;
use sqlx::{SqliteConnection, SqlitePool};

use crate::{
    agent_registry::spec::MemoryScope,
    audit::{self, Actor, Entry},
    db::{iso, new_id, now_ms},
};

const MAX_WORKSPACE_MEMORIES: i64 = 20;
const MAX_CONTACT_MEMORIES: i64 = 10;
const MAX_SEARCH_HITS: i64 = 5;
const MAX_CONTEXT_CHARS: usize = 2_500;
pub const MAX_MEMORY_CHARS: usize = 500;
/// Auto-learned memories below this confidence are kept but never given to the drafter until confirmed.
pub const USE_THRESHOLD: f64 = 0.6;
/// SQL predicate: a memory the agent may use.
const USABLE: &str = "status = 'active' AND (origin = 'manual' OR confidence >= 0.6)";

/// Upserts the sender's profile. Called once per newly stored message.
pub async fn record_contact(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    email: &str,
    display_name: Option<&str>,
    seen_at: i64,
) -> sqlx::Result<()> {
    if email.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO contacts (workspace_id, email, display_name, message_count, first_seen_at, last_seen_at)
         VALUES (?1, ?2, ?3, 1, ?4, ?4)
         ON CONFLICT(workspace_id, email) DO UPDATE SET
           message_count = message_count + 1,
           display_name = COALESCE(excluded.display_name, contacts.display_name),
           first_seen_at = MIN(contacts.first_seen_at, excluded.first_seen_at),
           last_seen_at = MAX(contacts.last_seen_at, excluded.last_seen_at)",
    )
    .bind(workspace_id)
    .bind(email)
    .bind(display_name)
    .bind(seen_at)
    .execute(conn)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ContactRow {
    pub email: String,
    pub display_name: Option<String>,
    pub message_count: i64,
    pub first_seen_at: i64,
    pub last_seen_at: i64,
}

pub async fn top_categories(pool: &SqlitePool, workspace_id: &str, email: &str) -> sqlx::Result<Vec<(String, i64)>> {
    sqlx::query_as(
        "SELECT c.category, COUNT(*) AS n FROM email_messages m JOIN classifications c ON c.message_id = m.id
         WHERE m.workspace_id = ?1 AND m.sender_address = ?2
         GROUP BY c.category ORDER BY n DESC, c.category LIMIT 3",
    )
    .bind(workspace_id)
    .bind(email)
    .fetch_all(pool)
    .await
}

pub async fn contact_json(pool: &SqlitePool, workspace_id: &str, c: ContactRow) -> sqlx::Result<serde_json::Value> {
    let top = top_categories(pool, workspace_id, &c.email).await?;
    Ok(json!({
        "email": c.email,
        "display_name": c.display_name,
        "message_count": c.message_count,
        "first_seen_at": iso(c.first_seen_at),
        "last_seen_at": iso(c.last_seen_at),
        "top_categories": top.into_iter().map(|(category, count)| json!({"category": category, "count": count})).collect::<Vec<_>>(),
    }))
}

pub async fn get_contact(pool: &SqlitePool, workspace_id: &str, email: &str) -> sqlx::Result<Option<ContactRow>> {
    sqlx::query_as(
        "SELECT email, display_name, message_count, first_seen_at, last_seen_at FROM contacts
         WHERE workspace_id = ?1 AND email = ?2",
    )
    .bind(workspace_id)
    .bind(email)
    .fetch_optional(pool)
    .await
}

/// Which memories an agent may read: its own (`agent_id = this`), the shared team's (`agent_id IS NULL`), or both.
/// Contact facts are always readable when shared; another agent's own memories are never readable.
#[derive(Debug, Clone)]
pub struct AgentMemory {
    pub agent_id: String,
    pub scope: MemoryScope,
}

impl AgentMemory {
    /// SQL for workspace-wide notes; `p` is the placeholder the agent id is bound to (always referenced, so it can always be bound).
    fn workspace_filter(view: Option<&AgentMemory>, p: &str) -> String {
        match view.map(|v| v.scope) {
            Some(MemoryScope::Own) => format!("agent_id = {p}"),
            Some(MemoryScope::OwnShared) => format!("(agent_id IS NULL OR agent_id = {p})"),
            Some(MemoryScope::Shared) | None => format!("(agent_id IS NULL AND {p} IS {p})"),
        }
    }

    /// SQL for contact notes: shared ones always, plus this agent's own.
    fn contact_filter(view: Option<&AgentMemory>, p: &str) -> String {
        match view.map(|v| v.scope) {
            Some(MemoryScope::Own | MemoryScope::OwnShared) => format!("(agent_id IS NULL OR agent_id = {p})"),
            Some(MemoryScope::Shared) | None => format!("(agent_id IS NULL AND {p} IS {p})"),
        }
    }
}

/// Everything the agent may use for one message.
#[derive(Debug, Clone, Default)]
pub struct MemoryContext {
    /// Learned style preferences that went into the prompt (their use is counted when the draft is stored).
    pub preference_ids: Vec<String>,
    /// (memory_id, rendered line) in priority order: contact → search hits → workspace.
    pub notes: Vec<(String, String)>,
    /// One line of learned history about the sender, e.g. "12 emails since Mar 2026; usually timesheet (8)".
    pub sender_history: Option<String>,
}

impl MemoryContext {
    pub fn ids(&self) -> Vec<String> {
        self.notes.iter().map(|(id, _)| id.clone()).collect()
    }
    pub fn lines(&self) -> Vec<String> {
        self.notes.iter().map(|(_, l)| l.clone()).collect()
    }
}

#[derive(sqlx::FromRow)]
struct NoteRow {
    id: String,
    kind: String,
    scope: String,
    content: String,
    origin: String,
}

/// FTS5 query from free text: up to 6 distinct word tokens, each quoted (so user/email
/// text can never inject FTS operators), OR-ed together.
pub fn fts_query(text: &str) -> Option<String> {
    let mut seen = HashSet::new();
    let tokens: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.chars().count() >= 3)
        .map(str::to_lowercase)
        .filter(|t| seen.insert(t.clone()))
        .take(6)
        .map(|t| format!("\"{t}\""))
        .collect();
    (!tokens.is_empty()).then(|| tokens.join(" OR "))
}

pub async fn search_ids(pool: &SqlitePool, workspace_id: &str, text: &str, limit: i64) -> sqlx::Result<Vec<String>> {
    let Some(q) = fts_query(text) else {
        return Ok(vec![]);
    };
    sqlx::query_scalar(
        "SELECT m.id FROM memories_fts f JOIN memories m ON m.id = f.memory_id
         WHERE memories_fts MATCH ?1 AND m.workspace_id = ?2 AND m.status = 'active'
         ORDER BY f.rank LIMIT ?3",
    )
    .bind(q)
    .bind(workspace_id)
    .bind(limit)
    .fetch_all(pool)
    .await
}

pub async fn context_for(pool: &SqlitePool, workspace_id: &str, sender: &str, topic: &str) -> sqlx::Result<MemoryContext> {
    context_with(pool, workspace_id, sender, topic, true, None).await
}

/// Like `context_for`, for one agent: its memory scope decides whether its own and/or the shared memories are used.
pub async fn context_for_agent(
    pool: &SqlitePool,
    workspace_id: &str,
    sender: &str,
    topic: &str,
    automated: bool,
    view: Option<&AgentMemory>,
) -> sqlx::Result<MemoryContext> {
    context_with(pool, workspace_id, sender, topic, !automated, view).await
}

/// For drafts nobody asked for (workflows, automations, auto-send): workspace-wide notes only if
/// a person wrote them, so an auto-learned fact can't steer replies to everyone.
pub async fn context_for_automation(pool: &SqlitePool, workspace_id: &str, sender: &str, topic: &str) -> sqlx::Result<MemoryContext> {
    context_with(pool, workspace_id, sender, topic, false, None).await
}

async fn context_with(
    pool: &SqlitePool,
    workspace_id: &str,
    sender: &str,
    topic: &str,
    auto_workspace_notes: bool,
    view: Option<&AgentMemory>,
) -> sqlx::Result<MemoryContext> {
    let workspace_origin = if auto_workspace_notes { "" } else { " AND origin = 'manual'" };
    let agent_id = view.map(|v| v.agent_id.as_str());
    let contact_notes: Vec<NoteRow> = sqlx::query_as(&format!(
        "SELECT id, kind, scope, content, origin FROM memories
         WHERE workspace_id = ?1 AND {USABLE} AND scope = 'contact' AND contact_email = ?2 AND {}
         ORDER BY pinned DESC, (origin = 'manual') DESC, confidence DESC, updated_at DESC LIMIT ?3",
        AgentMemory::contact_filter(view, "?4")
    ))
    .bind(workspace_id)
    .bind(sender)
    .bind(MAX_CONTACT_MEMORIES)
    .bind(agent_id)
    .fetch_all(pool)
    .await?;
    let hit_ids = search_ids(pool, workspace_id, topic, MAX_SEARCH_HITS).await?;
    let hits: Vec<NoteRow> = if hit_ids.is_empty() {
        vec![]
    } else {
        sqlx::query_as(&format!(
            "SELECT id, kind, scope, content, origin FROM memories
             WHERE id IN (SELECT value FROM json_each(?1)) AND scope = 'workspace' AND {USABLE}{workspace_origin} AND {}",
            AgentMemory::workspace_filter(view, "?2")
        ))
        .bind(json!(hit_ids).to_string())
        .bind(agent_id)
        .fetch_all(pool)
        .await?
    };
    let workspace_notes: Vec<NoteRow> = sqlx::query_as(&format!(
        "SELECT id, kind, scope, content, origin FROM memories
         WHERE workspace_id = ?1 AND {USABLE} AND scope = 'workspace'{workspace_origin} AND {}
         ORDER BY pinned DESC, (origin = 'manual') DESC, confidence DESC, updated_at DESC LIMIT ?2",
        AgentMemory::workspace_filter(view, "?3")
    ))
    .bind(workspace_id)
    .bind(MAX_WORKSPACE_MEMORIES)
    .bind(agent_id)
    .fetch_all(pool)
    .await?;

    let mut ctx = MemoryContext::default();
    let mut seen = HashSet::new();
    let mut budget = MAX_CONTEXT_CHARS;
    for n in contact_notes.into_iter().chain(hits).chain(workspace_notes) {
        if !seen.insert(n.id.clone()) {
            continue;
        }
        let about = if n.scope == "contact" { "about this sender" } else { "team" };
        let provenance = if n.origin == "auto" { " · learned" } else { "" };
        let line = format!("[{} · {about}{provenance}] {}", n.kind, n.content);
        if line.len() > budget {
            break;
        }
        budget -= line.len();
        ctx.notes.push((n.id, line));
    }

    if let Some(c) = get_contact(pool, workspace_id, sender).await? {
        let top = top_categories(pool, workspace_id, sender).await?;
        let usual = if top.is_empty() {
            String::new()
        } else {
            let parts: Vec<String> = top.iter().map(|(cat, n)| format!("{cat} ({n})")).collect();
            format!("; usually {}", parts.join(", "))
        };
        let name = c.display_name.as_deref().map(|n| format!("{n} <{}>", c.email)).unwrap_or(c.email.clone());
        let since = iso(c.first_seen_at).get(..10).unwrap_or_default().to_string();
        ctx.sender_history = Some(format!("{name}: {} emails since {since}{usual}", c.message_count));
    }
    Ok(ctx)
}

/// Normalised form used to avoid storing the same lesson twice.
pub fn dedupe_key(content: &str) -> String {
    content
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub struct Suggestion {
    pub kind: String,
    pub scope: String,
    pub content: String,
}

/// Stores learned lessons as `suggested` (unused until a person accepts them).
pub async fn store_suggestions(
    pool: &SqlitePool,
    workspace_id: &str,
    contact_email: &str,
    evidence_message_id: &str,
    suggestions: Vec<Suggestion>,
) -> sqlx::Result<usize> {
    let existing: Vec<String> = sqlx::query_scalar(
        "SELECT content FROM memories WHERE workspace_id = ?1 AND status != 'archived'
         AND (scope = 'workspace' OR contact_email = ?2)",
    )
    .bind(workspace_id)
    .bind(contact_email)
    .fetch_all(pool)
    .await?;
    let mut known: HashSet<String> = existing.iter().map(|c| dedupe_key(c)).collect();
    let now = now_ms();
    let mut tx = pool.begin().await?;
    let mut stored = 0;
    for s in suggestions {
        if !known.insert(dedupe_key(&s.content)) {
            continue;
        }
        let id = new_id("mem");
        let contact = (s.scope == "contact").then_some(contact_email);
        sqlx::query(
            "INSERT INTO memories (id, workspace_id, scope, contact_email, kind, content, status, source,
                                   evidence_message_id, created_at, updated_at, origin, confidence, sources_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'suggested', 'learned', ?7, ?8, ?8, 'auto', 0.5,
                     json_array(json_object('id', ?7, 'via', 'reply')))",
        )
        .bind(&id)
        .bind(workspace_id)
        .bind(&s.scope)
        .bind(contact)
        .bind(&s.kind)
        .bind(&s.content)
        .bind(evidence_message_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        audit::record(
            &mut tx,
            Entry {
                workspace_id,
                actor: Actor::System,
                action: "memory.suggested",
                entity_type: "memory",
                entity_id: &id,
                message_id: Some(evidence_message_id),
                metadata: json!({ "kind": s.kind, "scope": s.scope }),
            },
        )
        .await?;
        stored += 1;
    }
    tx.commit().await?;
    Ok(stored)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fts_query_quotes_tokens_and_drops_operators() {
        assert_eq!(fts_query("Re: September timesheet").as_deref(), Some(r#""september" OR "timesheet""#));
        assert_eq!(fts_query(r#"NEAR(" OR *) AND payroll"#).as_deref(), Some(r#""near" OR "and" OR "payroll""#));
        assert_eq!(fts_query("a b"), None);
    }

    #[test]
    fn usable_predicate_matches_threshold() {
        assert!(USABLE.contains(&format!("confidence >= {USE_THRESHOLD}")));
    }

    #[test]
    fn dedupe_key_ignores_case_and_punctuation() {
        assert_eq!(dedupe_key("Sign off as  'HR Team'."), dedupe_key("sign off as hr team"));
    }
}
