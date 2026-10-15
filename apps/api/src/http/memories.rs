use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::FromRow;

use super::AppState;
use crate::{
    platform::audit::{self, Actor, Entry},
    platform::auth::{AuthUser, require_role},
    platform::db::{iso, new_id, now_ms},
    platform::domain::Role,
    platform::error::{AppError, AppResult},
    learning::memory::{self, ContactRow, MAX_MEMORY_CHARS},
    mail::sources::normalize,
};

pub const MEMORY_COLUMNS: &str =
    "id, scope, contact_email, kind, content, status, source, created_by, evidence_message_id, created_at, updated_at,
     origin, confidence, pinned, use_count, last_used_at, agent_id";

#[derive(FromRow)]
pub struct MemoryRow {
    id: String,
    scope: String,
    contact_email: Option<String>,
    kind: String,
    content: String,
    status: String,
    source: String,
    created_by: Option<String>,
    evidence_message_id: Option<String>,
    created_at: i64,
    updated_at: i64,
    origin: String,
    confidence: f64,
    pinned: bool,
    use_count: i64,
    last_used_at: Option<i64>,
    agent_id: Option<String>,
}

#[derive(Serialize)]
pub struct Memory {
    id: String,
    scope: String,
    contact_email: Option<String>,
    kind: String,
    content: String,
    status: String,
    source: String,
    created_by: Option<String>,
    evidence_message_id: Option<String>,
    created_at: String,
    updated_at: String,
    origin: String,
    confidence: f64,
    /// Latest email this memory was learned from (same as `evidence_message_id`).
    source_message_id: Option<String>,
    pinned: bool,
    use_count: i64,
    last_used_at: Option<String>,
    /// The agent this memory belongs to; `None` = shared team memory. A personal agent's memories are visible to its owner only.
    agent_id: Option<String>,
}

impl From<MemoryRow> for Memory {
    fn from(r: MemoryRow) -> Self {
        Self {
            id: r.id,
            scope: r.scope,
            contact_email: r.contact_email,
            kind: r.kind,
            content: r.content,
            status: r.status,
            source: r.source,
            created_by: r.created_by,
            source_message_id: r.evidence_message_id.clone(),
            evidence_message_id: r.evidence_message_id,
            created_at: iso(r.created_at),
            updated_at: iso(r.updated_at),
            origin: r.origin,
            confidence: r.confidence,
            pinned: r.pinned,
            use_count: r.use_count,
            last_used_at: r.last_used_at.map(iso),
            agent_id: r.agent_id,
        }
    }
}

pub async fn contact_memories(state: &AppState, ws: &str, viewer: &str, email: &str) -> sqlx::Result<Vec<Memory>> {
    let rows: Vec<MemoryRow> = sqlx::query_as(&format!(
        "SELECT {MEMORY_COLUMNS} FROM memories
         WHERE workspace_id = ?1 AND scope = 'contact' AND contact_email = ?2 AND status = 'active' AND {}
         ORDER BY updated_at DESC",
        crate::agents::registry::store::hide_foreign_personal("memories", "?3")
    ))
    .bind(ws)
    .bind(email)
    .bind(viewer)
    .fetch_all(&state.db)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

fn validate_content(content: &str) -> AppResult<String> {
    let c = content.trim();
    if c.is_empty() || c.chars().count() > MAX_MEMORY_CHARS {
        return Err(AppError::BadRequest(format!("content must be 1-{MAX_MEMORY_CHARS} characters")));
    }
    Ok(c.to_string())
}

fn validate_kind(kind: &str) -> AppResult<()> {
    matches!(kind, "fact" | "preference" | "style")
        .then_some(())
        .ok_or_else(|| AppError::BadRequest("kind must be fact, preference or style".into()))
}

// ---------------------------------------------------------------- list / search

#[derive(Deserialize)]
pub struct ListQuery {
    status: Option<String>,
    scope: Option<String>,
    contact: Option<String>,
    q: Option<String>,
    origin: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    Path(ws): Path<String>,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let status = q.status.unwrap_or_else(|| "active".into());
    if !matches!(status.as_str(), "active" | "suggested" | "archived") {
        return Err(AppError::BadRequest("unknown status".into()));
    }
    let origin = q.origin.filter(|o| !o.is_empty());
    if origin.as_deref().is_some_and(|o| !matches!(o, "auto" | "manual")) {
        return Err(AppError::BadRequest("origin must be auto or manual".into()));
    }
    let scope = q.scope.filter(|s| !s.is_empty());
    let contact = q.contact.filter(|c| !c.is_empty()).map(|c| normalize::address(&c));
    let search = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let ids: Option<Vec<String>> = match search {
        Some(text) => Some(memory::search_ids(&state.db, &ws, text, 50).await?),
        None => None,
    };
    if ids.as_ref().is_some_and(Vec::is_empty) {
        return Ok(Json(json!({ "items": [] })));
    }
    let rows: Vec<MemoryRow> = sqlx::query_as(&format!(
        "SELECT {MEMORY_COLUMNS} FROM memories
         WHERE workspace_id = ?1 AND status = ?2
           AND (?3 IS NULL OR scope = ?3)
           AND (?4 IS NULL OR contact_email = ?4)
           AND (?5 IS NULL OR id IN (SELECT value FROM json_each(?5)))
           AND (?6 IS NULL OR origin = ?6)
           AND {}
         ORDER BY updated_at DESC LIMIT 200",
        crate::agents::registry::store::hide_foreign_personal("memories", "?7")
    ))
    .bind(&ws)
    .bind(&status)
    .bind(scope)
    .bind(contact)
    .bind(ids.map(|i| json!(i).to_string()))
    .bind(origin)
    .bind(&user.id)
    .fetch_all(&state.db)
    .await?;
    let items: Vec<Memory> = rows.into_iter().map(Into::into).collect();
    Ok(Json(json!({ "items": items })))
}

// ---------------------------------------------------------------- create / update

#[derive(Deserialize)]
pub struct CreateBody {
    scope: String,
    contact_email: Option<String>,
    kind: String,
    content: String,
}

pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    Path(ws): Path<String>,
    Json(body): Json<CreateBody>,
) -> AppResult<(StatusCode, Json<Memory>)> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    validate_kind(&body.kind)?;
    let content = validate_content(&body.content)?;
    let contact = match (body.scope.as_str(), body.contact_email.as_deref().map(str::trim).filter(|c| !c.is_empty())) {
        ("workspace", None) => None,
        ("contact", Some(email)) if email.contains('@') => Some(normalize::address(email)),
        _ => return Err(AppError::BadRequest("contact_email is required for scope=contact and only allowed there".into())),
    };
    let id = new_id("mem");
    let now = now_ms();
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let row: MemoryRow = sqlx::query_as(&format!(
        "INSERT INTO memories (id, workspace_id, scope, contact_email, kind, content, status, source, created_by, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', 'user', ?7, ?8, ?8)
         RETURNING {MEMORY_COLUMNS}"
    ))
    .bind(&id)
    .bind(&ws)
    .bind(&body.scope)
    .bind(&contact)
    .bind(&body.kind)
    .bind(&content)
    .bind(&user.id)
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &ws,
            actor: Actor::User(&user.id),
            action: "memory.created",
            entity_type: "memory",
            entity_id: &id,
            message_id: None,
            metadata: json!({ "scope": body.scope, "kind": body.kind }),
        },
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(row.into())))
}

#[derive(Deserialize)]
pub struct Patch {
    content: Option<String>,
    kind: Option<String>,
    status: Option<String>,
    pinned: Option<bool>,
    /// `true` = a person vouches for it: confidence 1 (and a suggestion becomes active).
    confirmed: Option<bool>,
}

pub async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
    Json(body): Json<Patch>,
) -> AppResult<Json<Memory>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let content = body.content.as_deref().map(validate_content).transpose()?;
    if let Some(k) = &body.kind {
        validate_kind(k)?;
    }
    if let Some(s) = &body.status
        && !matches!(s.as_str(), "active" | "archived")
    {
        return Err(AppError::BadRequest("status must be active or archived".into()));
    }
    if body.confirmed == Some(false) {
        return Err(AppError::BadRequest("confirmed can only be true".into()));
    }
    let confirmed = body.confirmed == Some(true);
    if content.is_none() && body.kind.is_none() && body.status.is_none() && body.pinned.is_none() && !confirmed {
        return Err(AppError::BadRequest("nothing to update".into()));
    }
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    // A person accepting, confirming or rewriting a memory vouches for it (confidence 1).
    let row: MemoryRow = sqlx::query_as(&format!(
        "UPDATE memories SET content = COALESCE(?1, content), kind = COALESCE(?2, kind),
                confidence = CASE WHEN ?7 OR ?1 IS NOT NULL OR (?3 = 'active' AND status = 'suggested') THEN 1.0 ELSE confidence END,
                status = COALESCE(?3, CASE WHEN ?7 AND status = 'suggested' THEN 'active' ELSE status END),
                pinned = COALESCE(?8, pinned), updated_at = ?4
         WHERE id = ?5 AND workspace_id = ?6 AND {}
         RETURNING {MEMORY_COLUMNS}",
        crate::agents::registry::store::hide_foreign_personal("memories", "?9")
    ))
    .bind(&content)
    .bind(&body.kind)
    .bind(&body.status)
    .bind(now_ms())
    .bind(&id)
    .bind(&ws)
    .bind(confirmed)
    .bind(body.pinned)
    .bind(&user.id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    let action = match (body.status.as_deref(), body.pinned) {
        (Some("active"), _) if row.source == "learned" => "memory.accepted",
        (Some("archived"), _) => "memory.archived",
        _ if confirmed => "memory.confirmed",
        (None, Some(true)) => "memory.pinned",
        (None, Some(false)) => "memory.unpinned",
        _ => "memory.updated",
    };
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &ws,
            actor: Actor::User(&user.id),
            action,
            entity_type: "memory",
            entity_id: &id,
            message_id: row.evidence_message_id.as_deref(),
            metadata: json!({}),
        },
    )
    .await?;
    let judged = match (body.status.as_deref(), confirmed) {
        (Some("archived"), _) if row.origin == "auto" => Some(crate::learning::signals::Kind::MemoryArchived),
        (_, true) | (Some("active"), _) if row.origin == "auto" => Some(crate::learning::signals::Kind::MemoryConfirmed),
        _ => None,
    };
    if let Some(kind) = judged {
        crate::learning::hooks::judged(&mut tx, &ws, &user.id, kind, row.agent_id.as_deref(), json!({"memory_id": id})).await?;
    }
    tx.commit().await?;
    Ok(Json(row.into()))
}

// ---------------------------------------------------------------- contacts

#[derive(Deserialize)]
pub struct ContactQuery {
    q: Option<String>,
}

pub async fn contacts(
    State(state): State<AppState>,
    user: AuthUser,
    Path(ws): Path<String>,
    Query(q): Query<ContactQuery>,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let pattern = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| {
        format!("%{}%", s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"))
    });
    let rows: Vec<ContactRow> = sqlx::query_as(
        "SELECT email, display_name, message_count, first_seen_at, last_seen_at FROM contacts
         WHERE workspace_id = ?1 AND (?2 IS NULL OR email LIKE ?2 ESCAPE '\\' OR display_name LIKE ?2 ESCAPE '\\')
         ORDER BY last_seen_at DESC LIMIT 100",
    )
    .bind(&ws)
    .bind(pattern)
    .fetch_all(&state.db)
    .await?;
    let mut items = Vec::with_capacity(rows.len());
    for r in rows {
        items.push(memory::contact_json(&state.db, &ws, r).await?);
    }
    Ok(Json(json!({ "items": items })))
}

pub async fn contact(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, email)): Path<(String, String)>,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let email = normalize::address(&email);
    let row = memory::get_contact(&state.db, &ws, &email).await?.ok_or(AppError::NotFound)?;
    let contact = memory::contact_json(&state.db, &ws, row).await?;
    let memories = contact_memories(&state, &ws, &user.id, &email).await?;
    let wall = crate::hr::team::scope::wall_for(&state.db, &ws, &user).await?;
    let recent: Vec<(String, String, i64, String, Option<String>)> = sqlx::query_as(
        "SELECT m.id, m.subject, m.received_at, m.processing_status, c.category
         FROM email_messages m LEFT JOIN classifications c ON c.message_id = m.id
         WHERE m.workspace_id = ?1 AND m.sender_address = ?2 AND m.suppressed = 0
           AND (?3 IS NULL OR m.source_id IN (SELECT value FROM json_each(?3)))
         ORDER BY m.received_at DESC LIMIT 5",
    )
    .bind(&ws)
    .bind(&email)
    .bind(&wall)
    .fetch_all(&state.db)
    .await?;
    let recent_messages: Vec<Value> = recent
        .into_iter()
        .map(|(id, subject, at, status, category)| {
            json!({
                "id": id, "subject": subject, "from": email, "original_from": null,
                "received_at": iso(at), "processing_status": status, "category": category,
                "priority": null, "needs_review": false, "attachment_count": 0,
            })
        })
        .collect();
    Ok(Json(json!({ "contact": contact, "memories": memories, "recent_messages": recent_messages })))
}
