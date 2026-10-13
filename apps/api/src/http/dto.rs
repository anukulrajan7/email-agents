//! Response DTOs. Mirrors docs/api.md and apps/web/src/api/types.ts.

use serde::Serialize;
use sqlx::FromRow;

use crate::{db::iso, domain::Capabilities, jobs::Job};

#[derive(Serialize)]
pub struct Items<T> {
    pub items: Vec<T>,
}

#[derive(Serialize)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub role: String,
    pub created_at: String,
}

#[derive(FromRow)]
pub struct WorkspaceRow {
    pub id: String,
    pub name: String,
    pub role: String,
    pub created_at: i64,
}

impl From<WorkspaceRow> for Workspace {
    fn from(r: WorkspaceRow) -> Self {
        Self { id: r.id, name: r.name, role: r.role, created_at: iso(r.created_at) }
    }
}

#[derive(Serialize)]
pub struct Source {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub inbound_address: Option<String>,
    pub capabilities: Capabilities,
    pub last_synced_at: Option<String>,
    pub last_error: Option<String>,
    pub created_at: String,
}

#[derive(FromRow)]
pub struct SourceRow {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub inbound_alias: Option<String>,
    pub capabilities_json: String,
    pub last_synced_at: Option<i64>,
    pub last_error: Option<String>,
    pub created_at: i64,
}

impl From<SourceRow> for Source {
    fn from(r: SourceRow) -> Self {
        let capabilities =
            serde_json::from_str(&r.capabilities_json).unwrap_or_else(|_| crate::sources::capabilities(&r.kind));
        Self {
            id: r.id,
            kind: r.kind,
            status: r.status,
            inbound_address: r.inbound_alias,
            capabilities,
            last_synced_at: r.last_synced_at.map(iso),
            last_error: r.last_error,
            created_at: iso(r.created_at),
        }
    }
}

#[derive(Serialize)]
pub struct Classification {
    pub category: String,
    pub priority: String,
    pub requires_reply: bool,
    pub confidence: Option<f64>,
    pub needs_review: bool,
    pub source: String,
    pub model_id: Option<String>,
    pub updated_at: String,
}

#[derive(FromRow)]
pub struct ClassificationRow {
    pub category: String,
    pub priority: String,
    pub requires_reply: bool,
    pub confidence: Option<f64>,
    pub needs_review: bool,
    pub source: String,
    pub model_id: Option<String>,
    pub updated_at: i64,
}

impl From<ClassificationRow> for Classification {
    fn from(r: ClassificationRow) -> Self {
        Self {
            category: r.category,
            priority: r.priority,
            requires_reply: r.requires_reply,
            confidence: r.confidence,
            needs_review: r.needs_review,
            source: r.source,
            model_id: r.model_id,
            updated_at: iso(r.updated_at),
        }
    }
}

#[derive(Serialize)]
pub struct Draft {
    pub id: String,
    pub message_id: String,
    pub status: String,
    pub body_text: String,
    pub model_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// What the drafter was given: `[{id, content}]`.
    pub memories_used: Vec<serde_json::Value>,
    /// Asks the draft could not answer; each appears in the body as `[NEEDS: …]`.
    pub unanswered: Vec<String>,
}

#[derive(FromRow)]
pub struct DraftRow {
    pub id: String,
    pub message_id: String,
    pub status: String,
    pub body_text: String,
    pub model_id: Option<String>,
    pub memory_ids_json: String,
    pub unanswered_json: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<DraftRow> for Draft {
    fn from(r: DraftRow) -> Self {
        Self {
            id: r.id,
            message_id: r.message_id,
            status: r.status,
            body_text: r.body_text,
            model_id: r.model_id,
            created_at: iso(r.created_at),
            updated_at: iso(r.updated_at),
            memories_used: vec![],
            unanswered: serde_json::from_str(&r.unanswered_json).unwrap_or_default(),
        }
    }
}

/// Resolves the memory IDs a draft was given (archived ones still show, for honesty).
pub async fn draft_with_memories(db: &sqlx::SqlitePool, row: DraftRow) -> sqlx::Result<Draft> {
    let used: Vec<(String, String)> =
        sqlx::query_as(
            // Join on json_each and sort by its index: keeps the order the drafter saw.
            "SELECT m.id, m.content FROM json_each(?1) j JOIN memories m ON m.id = j.value ORDER BY j.key",
        )
            .bind(&row.memory_ids_json)
            .fetch_all(db)
            .await?;
    let mut draft = Draft::from(row);
    draft.memories_used = used.into_iter().map(|(id, content)| serde_json::json!({ "id": id, "content": content })).collect();
    Ok(draft)
}

/// The AI's reading of an email (`message_insights`), as returned in message detail.
#[derive(Serialize)]
pub struct Insight {
    pub summary: String,
    pub asks: Vec<String>,
    pub deadline: Option<String>,
    pub needs_reply: bool,
    pub missing_info: Vec<String>,
}

#[derive(FromRow)]
pub struct InsightRow {
    pub summary: String,
    pub asks_json: String,
    pub deadline: Option<String>,
    pub needs_reply: bool,
    pub missing_json: String,
}

impl From<InsightRow> for Insight {
    fn from(r: InsightRow) -> Self {
        Self {
            summary: r.summary,
            asks: serde_json::from_str(&r.asks_json).unwrap_or_default(),
            deadline: r.deadline,
            needs_reply: r.needs_reply,
            missing_info: serde_json::from_str(&r.missing_json).unwrap_or_default(),
        }
    }
}

#[derive(Serialize)]
pub struct ReviewItem {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub status: String,
    pub message_id: String,
    pub message_subject: String,
    pub assigned_user_id: Option<String>,
    pub created_at: String,
}

#[derive(FromRow)]
pub struct ReviewRow {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub status: String,
    pub message_id: String,
    pub message_subject: String,
    pub assigned_user_id: Option<String>,
    pub created_at: i64,
}

impl From<ReviewRow> for ReviewItem {
    fn from(r: ReviewRow) -> Self {
        Self {
            id: r.id,
            kind: r.kind,
            title: r.title,
            status: r.status,
            message_id: r.message_id,
            message_subject: r.message_subject,
            assigned_user_id: r.assigned_user_id,
            created_at: iso(r.created_at),
        }
    }
}

#[derive(Serialize)]
pub struct JobDto {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub attempts: i64,
    pub max_attempts: i64,
    pub last_error: Option<String>,
    pub run_at: String,
    pub updated_at: String,
}

impl From<Job> for JobDto {
    fn from(j: Job) -> Self {
        Self {
            id: j.id,
            kind: j.kind,
            status: j.status,
            attempts: j.attempts,
            max_attempts: j.max_attempts,
            last_error: j.last_error,
            run_at: iso(j.run_at),
            updated_at: iso(j.updated_at),
        }
    }
}
