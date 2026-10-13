use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::FromRow;

use super::{
    AppState,
    dto::{Classification, ClassificationRow, Draft, DraftRow, draft_with_memories},
};
use crate::{
    ai::{ClassifyOutput, policy},
    audit::{self, Actor, Entry},
    auth::{AuthUser, require_role},
    db::{iso, new_id, now_ms},
    domain::{Capabilities, Category, ClassificationSource, Priority, Role, TAXONOMY_VERSION},
    error::{AppError, AppResult},
    jobs::{self, handlers, kind},
    memory,
};

// ---------------------------------------------------------------- list

#[derive(Deserialize)]
pub struct ListQuery {
    cursor: Option<String>,
    category: Option<String>,
    q: Option<String>,
    limit: Option<i64>,
}

#[derive(Serialize)]
pub struct MessageSummary {
    id: String,
    subject: String,
    from: String,
    original_from: Option<String>,
    received_at: String,
    processing_status: String,
    category: Option<String>,
    priority: Option<String>,
    needs_review: bool,
    attachment_count: i64,
}

#[derive(FromRow)]
struct SummaryRow {
    id: String,
    subject: String,
    sender_address: String,
    original_sender_address: Option<String>,
    received_at: i64,
    processing_status: String,
    category: Option<String>,
    priority: Option<String>,
    needs_review: Option<bool>,
    attachment_count: i64,
}

fn encode_cursor(received_at: i64, id: &str) -> String {
    URL_SAFE_NO_PAD.encode(format!("{received_at}:{id}"))
}

fn decode_cursor(cursor: &str) -> AppResult<(i64, String)> {
    let bad = || AppError::BadRequest("invalid cursor".into());
    let raw = String::from_utf8(URL_SAFE_NO_PAD.decode(cursor).map_err(|_| bad())?).map_err(|_| bad())?;
    let (ts, id) = raw.split_once(':').ok_or_else(bad)?;
    Ok((ts.parse().map_err(|_| bad())?, id.to_string()))
}

fn like_pattern(q: &str) -> String {
    let escaped = q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    format!("%{escaped}%")
}

pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    Path(ws): Path<String>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let limit = query.limit.unwrap_or(50).clamp(1, 100);
    let category = match query.category.as_deref().filter(|c| !c.is_empty()) {
        Some(c) => Some(Category::parse(c).ok_or_else(|| AppError::BadRequest("unknown category".into()))?.as_str()),
        None => None,
    };
    let search = query.q.as_deref().map(str::trim).filter(|q| !q.is_empty()).map(like_pattern);
    let (cursor_ts, cursor_id) = match query.cursor.as_deref().filter(|c| !c.is_empty()) {
        Some(c) => {
            let (ts, id) = decode_cursor(c)?;
            (Some(ts), Some(id))
        }
        None => (None, None),
    };

    // Keyset pagination on (received_at DESC, id DESC), backed by idx_inbox.
    let rows: Vec<SummaryRow> = sqlx::query_as(
        "SELECT m.id, m.subject, m.sender_address, m.original_sender_address, m.received_at, m.processing_status,
                c.category, c.priority, c.needs_review,
                (SELECT COUNT(*) FROM attachments a WHERE a.message_id = m.id) AS attachment_count
         FROM email_messages m
         LEFT JOIN classifications c ON c.message_id = m.id
         WHERE m.workspace_id = ?1
           AND (?2 IS NULL OR c.category = ?2)
           AND (?3 IS NULL OR m.subject LIKE ?3 ESCAPE '\\' OR m.sender_address LIKE ?3 ESCAPE '\\'
                OR m.original_sender_address LIKE ?3 ESCAPE '\\')
           AND (?4 IS NULL OR m.received_at < ?4 OR (m.received_at = ?4 AND m.id < ?5))
         ORDER BY m.received_at DESC, m.id DESC
         LIMIT ?6",
    )
    .bind(&ws)
    .bind(category)
    .bind(search)
    .bind(cursor_ts)
    .bind(cursor_id)
    .bind(limit + 1)
    .fetch_all(&state.db)
    .await?;

    let has_more = rows.len() as i64 > limit;
    let rows: Vec<SummaryRow> = rows.into_iter().take(limit as usize).collect();
    let next_cursor = if has_more { rows.last().map(|r| encode_cursor(r.received_at, &r.id)) } else { None };
    let items: Vec<MessageSummary> = rows
        .into_iter()
        .map(|r| MessageSummary {
            id: r.id,
            subject: r.subject,
            from: r.sender_address,
            original_from: r.original_sender_address,
            received_at: iso(r.received_at),
            processing_status: r.processing_status,
            category: r.category,
            priority: r.priority,
            needs_review: r.needs_review.unwrap_or(true),
            attachment_count: r.attachment_count,
        })
        .collect();
    Ok(Json(json!({ "items": items, "next_cursor": next_cursor })))
}

// ---------------------------------------------------------------- detail

#[derive(FromRow)]
struct DetailRow {
    id: String,
    source_id: String,
    source_kind: String,
    capabilities_json: String,
    sender_address: String,
    original_sender_address: Option<String>,
    recipients_json: String,
    subject: String,
    body_text: String,
    received_at: i64,
    processing_status: String,
}

#[derive(FromRow, Serialize)]
struct AttachmentDto {
    id: String,
    name: String,
    media_type: Option<String>,
    size_bytes: Option<i64>,
    scan_status: String,
}

#[derive(FromRow)]
struct AuditRow {
    id: String,
    actor_type: String,
    actor_id: Option<String>,
    action: String,
    entity_type: String,
    entity_id: String,
    created_at: i64,
}

pub async fn detail(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let m: DetailRow = sqlx::query_as(
        "SELECT m.id, m.source_id, s.kind AS source_kind, s.capabilities_json, m.sender_address, m.original_sender_address,
                m.recipients_json, m.subject, m.body_text, m.received_at, m.processing_status
         FROM email_messages m JOIN email_sources s ON s.id = m.source_id
         WHERE m.id = ?1 AND m.workspace_id = ?2",
    )
    .bind(&id)
    .bind(&ws)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)?;

    let attachments: Vec<AttachmentDto> = sqlx::query_as(
        "SELECT id, filename AS name, media_type, size_bytes, scan_status FROM attachments WHERE message_id = ?1 ORDER BY filename",
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await?;
    let classification: Option<ClassificationRow> = sqlx::query_as(
        "SELECT category, priority, requires_reply, confidence, needs_review, source, model_id, updated_at
         FROM classifications WHERE message_id = ?1",
    )
    .bind(&id)
    .fetch_optional(&state.db)
    .await?;
    let draft: Option<DraftRow> = sqlx::query_as(
        "SELECT id, message_id, status, body_text, model_id, memory_ids_json, created_at, updated_at
         FROM drafts WHERE message_id = ?1 AND workspace_id = ?2 ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .bind(&id)
    .bind(&ws)
    .fetch_optional(&state.db)
    .await?;
    let review: Option<(String, String, String)> = sqlx::query_as(
        "SELECT id, status, kind FROM review_items WHERE message_id = ?1 AND workspace_id = ?2 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(&id)
    .bind(&ws)
    .fetch_optional(&state.db)
    .await?;
    let run: Option<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT id, status, error FROM workflow_runs WHERE message_id = ?1 AND workspace_id = ?2 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(&id)
    .bind(&ws)
    .fetch_optional(&state.db)
    .await?;
    let audit_rows: Vec<AuditRow> = sqlx::query_as(
        "SELECT id, actor_type, actor_id, action, entity_type, entity_id, created_at
         FROM audit_events WHERE workspace_id = ?1 AND message_id = ?2 ORDER BY created_at, id",
    )
    .bind(&ws)
    .bind(&id)
    .fetch_all(&state.db)
    .await?;

    let draft = match draft {
        Some(row) => Some(draft_with_memories(&state.db, row).await?),
        None => None,
    };
    let contact = match memory::get_contact(&state.db, &ws, &m.sender_address).await? {
        Some(c) => Some(memory::contact_json(&state.db, &ws, c).await?),
        None => None,
    };
    let contact_memories = super::memories::contact_memories(&state, &ws, &m.sender_address).await?;

    let capabilities: Capabilities =
        serde_json::from_str(&m.capabilities_json).unwrap_or_else(|_| crate::sources::capabilities(&m.source_kind));
    let to: Vec<String> = serde_json::from_str(&m.recipients_json).unwrap_or_default();

    Ok(Json(json!({
        "id": m.id,
        "source": {
            "id": m.source_id,
            "kind": m.source_kind,
            "can_create_provider_draft": policy::can_create_provider_draft(&capabilities),
        },
        "from": m.sender_address,
        "original_from": m.original_sender_address,
        "to": to,
        "subject": m.subject,
        "body_text": m.body_text,
        "received_at": iso(m.received_at),
        "processing_status": m.processing_status,
        "attachments": attachments,
        "classification": classification.map(Classification::from),
        "draft": draft,
        "contact": contact,
        "contact_memories": contact_memories,
        "review": review.map(|(id, status, kind)| json!({ "id": id, "status": status, "kind": kind })),
        "workflow_run": run.map(|(id, status, error)| json!({ "id": id, "status": status, "error": error })),
        "audit": audit_rows.into_iter().map(|a| json!({
            "id": a.id, "actor_type": a.actor_type, "actor_id": a.actor_id, "action": a.action,
            "entity_type": a.entity_type, "entity_id": a.entity_id, "created_at": iso(a.created_at),
        })).collect::<Vec<_>>(),
    })))
}

async fn ensure_message(state: &AppState, ws: &str, id: &str) -> AppResult<()> {
    let found: Option<String> = sqlx::query_scalar("SELECT id FROM email_messages WHERE id = ?1 AND workspace_id = ?2")
        .bind(id)
        .bind(ws)
        .fetch_optional(&state.db)
        .await?;
    found.map(|_| ()).ok_or(AppError::NotFound)
}

// ---------------------------------------------------------------- classification override

#[derive(Deserialize)]
pub struct OverrideBody {
    category: String,
    priority: Option<String>,
    requires_reply: Option<bool>,
}

pub async fn override_classification(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
    Json(body): Json<OverrideBody>,
) -> AppResult<Json<Classification>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    ensure_message(&state, &ws, &id).await?;
    let category = Category::parse(&body.category).ok_or_else(|| AppError::BadRequest("unknown category".into()))?;
    let existing: Option<(String, bool)> =
        sqlx::query_as("SELECT priority, requires_reply FROM classifications WHERE message_id = ?1")
            .bind(&id)
            .fetch_optional(&state.db)
            .await?;
    let priority = match body.priority.as_deref() {
        Some(p) => Priority::parse(p).ok_or_else(|| AppError::BadRequest("unknown priority".into()))?,
        None => existing.as_ref().and_then(|(p, _)| Priority::parse(p)).unwrap_or(Priority::Normal),
    };
    let requires_reply = body.requires_reply.or(existing.map(|(_, r)| r)).unwrap_or(true);
    let out = ClassifyOutput {
        category,
        priority,
        requires_reply,
        confidence: None,
        source: ClassificationSource::Manual,
        model_id: None,
        model_version: None,
    };
    handlers::persist_classification(&state, &id, &out, Actor::User(&user.id), None, handlers::WriteMode::Upsert)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e.to_string())))?;
    state.jobs_notify.notify_one();
    let row: ClassificationRow = sqlx::query_as(
        "SELECT category, priority, requires_reply, confidence, needs_review, source, model_id, updated_at
         FROM classifications WHERE message_id = ?1",
    )
    .bind(&id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(row.into()))
}

// ---------------------------------------------------------------- drafts

async fn enqueue_for_message(
    state: &AppState,
    ws: &str,
    message_id: &str,
    user: &AuthUser,
    job_kind: &str,
    dedupe_key: String,
    action: &str,
) -> AppResult<(StatusCode, Json<Value>)> {
    let mut tx = state.db.begin().await?;
    jobs::enqueue(&mut tx, job_kind, &dedupe_key, Some(ws), &json!({ "message_id": message_id })).await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: ws,
            actor: Actor::User(&user.id),
            action,
            entity_type: "message",
            entity_id: message_id,
            message_id: Some(message_id),
            metadata: json!({}),
        },
    )
    .await?;
    tx.commit().await?;
    state.jobs_notify.notify_one();
    Ok((StatusCode::ACCEPTED, Json(json!({ "status": "queued" }))))
}

pub async fn request_draft(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
) -> AppResult<(StatusCode, Json<Value>)> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    ensure_message(&state, &ws, &id).await?;
    let key = format!("draft:{id}:manual:{}", new_id("req"));
    enqueue_for_message(&state, &ws, &id, &user, kind::GENERATE_DRAFT, key, "draft.requested").await
}

pub async fn reprocess(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
) -> AppResult<(StatusCode, Json<Value>)> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    ensure_message(&state, &ws, &id).await?;
    let key = format!("classify:{id}:{TAXONOMY_VERSION}:reprocess:{}", new_id("req"));
    enqueue_for_message(&state, &ws, &id, &user, kind::CLASSIFY_EMAIL, key, "message.reprocess_requested").await
}

#[derive(Deserialize)]
pub struct DraftPatch {
    body_text: Option<String>,
    status: Option<String>,
}

pub async fn update_draft(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
    Json(body): Json<DraftPatch>,
) -> AppResult<Json<Draft>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    // Clients can only edit or approve. "sent" is reserved for a future, audited send action.
    let status = match body.status.as_deref() {
        Some(s @ ("edited" | "approved")) => Some(s),
        Some(_) => return Err(AppError::BadRequest("status must be edited or approved".into())),
        None if body.body_text.is_some() => Some("edited"),
        None => return Err(AppError::BadRequest("nothing to update".into())),
    };
    if let Some(text) = &body.body_text
        && (text.trim().is_empty() || text.chars().count() > 10_000)
    {
        return Err(AppError::BadRequest("body_text must be 1-10000 characters".into()));
    }
    let mut tx = state.db.begin().await?;
    let row: DraftRow = sqlx::query_as(
        "UPDATE drafts SET body_text = COALESCE(?1, body_text), status = COALESCE(?2, status), updated_at = ?3
         WHERE id = ?4 AND workspace_id = ?5 AND status != 'sent'
         RETURNING id, message_id, status, body_text, model_id, memory_ids_json, created_at, updated_at",
    )
    .bind(&body.body_text)
    .bind(status)
    .bind(now_ms())
    .bind(&id)
    .bind(&ws)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &ws,
            actor: Actor::User(&user.id),
            action: if row.status == "approved" { "draft.approved" } else { "draft.edited" },
            entity_type: "draft",
            entity_id: &row.id,
            message_id: Some(&row.message_id),
            metadata: json!({ "status": row.status }),
        },
    )
    .await?;
    // Learn from the operator's final version (only when they approve, and only if it changed).
    if row.status == "approved" {
        jobs::enqueue(
            &mut tx,
            kind::LEARN_FROM_DRAFT,
            &format!("learn:{}:{}", row.id, row.updated_at),
            Some(&ws),
            &json!({ "draft_id": row.id }),
        )
        .await?;
    }
    tx.commit().await?;
    state.jobs_notify.notify_one();
    Ok(Json(draft_with_memories(&state.db, row).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_roundtrip_and_like_escape() {
        let c = encode_cursor(1_700_000_000_000, "msg_abc");
        assert_eq!(decode_cursor(&c).unwrap(), (1_700_000_000_000, "msg_abc".to_string()));
        assert!(decode_cursor("!!").is_err());
        assert_eq!(like_pattern("50%_off"), "%50\\%\\_off%");
    }
}
