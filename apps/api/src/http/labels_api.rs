use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::FromRow;

use std::collections::HashSet;

use super::AppState;
use crate::{
    ai::label_suggest::{MAX_SAMPLES, SampleMail, finalize},
    audit::{self, Actor, Entry},
    auth::{AuthUser, require_role},
    db::{new_id, now_ms},
    domain::{LABEL_TEMPLATES, OTHER_LABEL, Role, WORKFLOW_TEMPLATES},
    error::{AppError, AppResult},
    labels::{COLORS, MAX_ACTIVE_LABELS, slugify},
    sources::normalize::truncate_chars,
};

const LABEL_SELECT: &str = "SELECT l.id, l.key, l.name, l.description, l.color, l.position, l.system, l.archived,
        (SELECT COUNT(*) FROM classifications c JOIN email_messages m ON m.id = c.message_id
         WHERE m.workspace_id = l.workspace_id AND c.category = l.key) AS message_count
 FROM labels l";

#[derive(FromRow, Serialize)]
pub struct Label {
    id: String,
    key: String,
    name: String,
    description: String,
    color: String,
    position: i64,
    system: bool,
    archived: bool,
    message_count: i64,
}

pub async fn templates(_user: AuthUser) -> Json<Value> {
    let items: Vec<Value> = LABEL_TEMPLATES
        .iter()
        .map(|t| {
            json!({
                "key": t.key, "name": t.name, "description": t.description,
                "labels": t.labels.iter().map(|l| json!({"key": l.key, "name": l.name, "description": l.description})).collect::<Vec<_>>(),
            })
        })
        .collect();
    Json(json!({ "items": items }))
}

#[derive(Deserialize)]
pub struct ListQuery {
    include_archived: Option<bool>,
}

pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    Path(ws): Path<String>,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let rows: Vec<Label> = sqlx::query_as(&format!(
        "{LABEL_SELECT} WHERE l.workspace_id = ?1 AND (?2 OR l.archived = 0) ORDER BY l.archived, l.position, l.created_at"
    ))
    .bind(&ws)
    .bind(q.include_archived.unwrap_or(false))
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": rows })))
}

fn validate(name: Option<&str>, description: Option<&str>, color: Option<&str>) -> AppResult<()> {
    if let Some(n) = name
        && !(1..=40).contains(&n.trim().chars().count())
    {
        return Err(AppError::BadRequest("name must be 1-40 characters".into()));
    }
    if let Some(d) = description
        && !(10..=300).contains(&d.trim().chars().count())
    {
        return Err(AppError::BadRequest("description must be 10-300 characters".into()));
    }
    if let Some(c) = color
        && !COLORS.contains(&c)
    {
        return Err(AppError::BadRequest("unknown colour".into()));
    }
    Ok(())
}

async fn active_count(state: &AppState, ws: &str) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT COUNT(*) FROM labels WHERE workspace_id = ?1 AND archived = 0").bind(ws).fetch_one(&state.db).await
}

#[derive(Deserialize)]
pub struct CreateBody {
    name: String,
    description: String,
    color: Option<String>,
}

pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    Path(ws): Path<String>,
    Json(body): Json<CreateBody>,
) -> AppResult<(StatusCode, Json<Label>)> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    validate(Some(&body.name), Some(&body.description), body.color.as_deref())?;
    let key = slugify(&body.name);
    if key.is_empty() {
        return Err(AppError::BadRequest("name needs at least one letter or digit".into()));
    }
    if active_count(&state, &ws).await? >= MAX_ACTIVE_LABELS {
        return Err(AppError::Conflict(format!("a workspace can have at most {MAX_ACTIVE_LABELS} active labels")));
    }
    let id = new_id("lbl");
    let mut tx = state.db.begin().await?;
    let inserted = sqlx::query(
        "INSERT OR IGNORE INTO labels (id, workspace_id, key, name, description, color, position, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, (SELECT COALESCE(MAX(position), 0) + 1 FROM labels WHERE workspace_id = ?2), ?7)",
    )
    .bind(&id)
    .bind(&ws)
    .bind(&key)
    .bind(body.name.trim())
    .bind(body.description.trim())
    .bind(body.color.as_deref().unwrap_or("ink"))
    .bind(now_ms())
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if !inserted {
        return Err(AppError::Conflict(format!("a label with key \"{key}\" already exists (it may be archived)")));
    }
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &ws,
            actor: Actor::User(&user.id),
            action: "label.created",
            entity_type: "label",
            entity_id: &id,
            message_id: None,
            metadata: json!({ "key": key }),
        },
    )
    .await?;
    let row: Label = sqlx::query_as(&format!("{LABEL_SELECT} WHERE l.id = ?1")).bind(&id).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(row)))
}

#[derive(Deserialize)]
pub struct Patch {
    name: Option<String>,
    description: Option<String>,
    color: Option<String>,
    archived: Option<bool>,
    position: Option<i64>,
}

pub async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
    Json(body): Json<Patch>,
) -> AppResult<Json<Label>> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    validate(body.name.as_deref(), body.description.as_deref(), body.color.as_deref())?;
    let (key, archived): (String, bool) =
        sqlx::query_as("SELECT key, archived FROM labels WHERE id = ?1 AND workspace_id = ?2")
            .bind(&id)
            .bind(&ws)
            .fetch_optional(&state.db)
            .await?
            .ok_or(AppError::NotFound)?;
    match body.archived {
        Some(true) if key == OTHER_LABEL => return Err(AppError::Conflict("the Other label is the safety net and cannot be archived".into())),
        Some(true) => {
            let keys: Vec<&str> = WORKFLOW_TEMPLATES.iter().filter(|t| t.label_key == key).map(|t| t.key).collect();
            for template_key in keys {
                let enabled: Option<bool> =
                    sqlx::query_scalar("SELECT enabled FROM workflows WHERE workspace_id = ?1 AND template_key = ?2")
                        .bind(&ws)
                        .bind(template_key)
                        .fetch_optional(&state.db)
                        .await?;
                if enabled == Some(true) {
                    return Err(AppError::Conflict("disable the workflow that uses this label first".into()));
                }
            }
        }
        Some(false) if archived && active_count(&state, &ws).await? >= MAX_ACTIVE_LABELS => {
            return Err(AppError::Conflict(format!("a workspace can have at most {MAX_ACTIVE_LABELS} active labels")));
        }
        _ => {}
    }
    let mut tx = state.db.begin().await?;
    // Reordering: whoever holds the target position takes this label's old position (swap).
    if let Some(target) = body.position {
        sqlx::query(
            "UPDATE labels SET position = (SELECT position FROM labels WHERE id = ?1)
             WHERE workspace_id = ?2 AND position = ?3 AND id != ?1",
        )
        .bind(&id)
        .bind(&ws)
        .bind(target)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "UPDATE labels SET name = COALESCE(?1, name), description = COALESCE(?2, description), color = COALESCE(?3, color),
                archived = COALESCE(?4, archived), position = COALESCE(?5, position)
         WHERE id = ?6 AND workspace_id = ?7",
    )
    .bind(body.name.as_deref().map(str::trim))
    .bind(body.description.as_deref().map(str::trim))
    .bind(&body.color)
    .bind(body.archived)
    .bind(body.position)
    .bind(&id)
    .bind(&ws)
    .execute(&mut *tx)
    .await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &ws,
            actor: Actor::User(&user.id),
            action: match body.archived {
                Some(true) => "label.archived",
                Some(false) => "label.restored",
                None => "label.updated",
            },
            entity_type: "label",
            entity_id: &id,
            message_id: None,
            metadata: json!({ "key": key }),
        },
    )
    .await?;
    let row: Label = sqlx::query_as(&format!("{LABEL_SELECT} WHERE l.id = ?1")).bind(&id).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(row))
}

// ---------------------------------------------------------------- AI suggestions

#[derive(FromRow)]
struct SampleRow {
    subject: String,
    body_text: String,
}

/// Admin: the AI reads up to 60 recent emails (unlabelled `other` first) and proposes up to 5 new labels.
pub async fn suggest(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    let rows: Vec<SampleRow> = sqlx::query_as(
        "SELECT m.subject, m.body_text FROM email_messages m
         LEFT JOIN classifications c ON c.message_id = m.id
         WHERE m.workspace_id = ?1 AND m.direction = 'inbound'
         ORDER BY CASE WHEN c.category IS NULL OR c.category = ?2 THEN 0 ELSE 1 END, m.received_at DESC
         LIMIT ?3",
    )
    .bind(&ws)
    .bind(OTHER_LABEL)
    .bind(MAX_SAMPLES as i64)
    .fetch_all(&state.db)
    .await?;
    if rows.is_empty() {
        return Ok(Json(json!({ "items": [] })));
    }
    let samples: Vec<SampleMail> = rows
        .iter()
        .map(|r| SampleMail {
            subject: r.subject.clone(),
            snippet: truncate_chars(&r.body_text.split_whitespace().collect::<Vec<_>>().join(" "), 200),
        })
        .collect();
    let existing = crate::labels::active(&state.db, &ws).await?;
    // Archived labels keep their key too, so a suggestion can never collide with one.
    let taken: HashSet<String> = sqlx::query_scalar::<_, String>("SELECT key FROM labels WHERE workspace_id = ?1")
        .bind(&ws)
        .fetch_all(&state.db)
        .await?
        .into_iter()
        .chain(existing.iter().map(|l| l.key.clone()))
        .collect();
    let proposals = state.label_suggester.suggest(&samples, &existing).await.map_err(|e| {
        tracing::warn!(error = %e, "label suggestion failed");
        AppError::AiUnavailable
    })?;
    Ok(Json(json!({ "items": finalize(proposals, &taken, samples.len()) })))
}
