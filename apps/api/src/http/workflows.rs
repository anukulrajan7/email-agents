use std::collections::HashSet;

use axum::{
    Json,
    extract::{Path, State},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::FromRow;

use super::{AppState, dto::Items};
use crate::{
    audit::{self, Actor, Entry},
    auth::{AuthUser, require_role},
    db::iso,
    domain::{Role, template},
    error::{AppError, AppResult},
    labels,
};

#[derive(Serialize)]
pub struct Workflow {
    id: String,
    template_key: String,
    label_key: String,
    name: String,
    description: String,
    enabled: bool,
    available: bool,
    last_run: Option<Value>,
}

#[derive(FromRow)]
struct WorkflowRow {
    id: String,
    template_key: String,
    enabled: bool,
    last_status: Option<String>,
    last_created_at: Option<i64>,
}

const WORKFLOW_SELECT: &str = "SELECT w.id, w.template_key, w.enabled,
        (SELECT r.status FROM workflow_runs r WHERE r.workflow_id = w.id ORDER BY r.created_at DESC LIMIT 1) AS last_status,
        (SELECT r.created_at FROM workflow_runs r WHERE r.workflow_id = w.id ORDER BY r.created_at DESC LIMIT 1) AS last_created_at
 FROM workflows w";

fn to_dto(r: WorkflowRow, active_labels: &HashSet<String>) -> Workflow {
    let t = template(&r.template_key);
    let label_key = t.map(|t| t.label_key).unwrap_or_default().to_string();
    Workflow {
        name: t.map(|t| t.name).unwrap_or(&r.template_key).to_string(),
        description: t.map(|t| t.description).unwrap_or_default().to_string(),
        available: t.is_some_and(|t| t.available) && active_labels.contains(&label_key),
        label_key,
        enabled: r.enabled,
        last_run: match (r.last_status, r.last_created_at) {
            (Some(status), Some(at)) => Some(json!({ "status": status, "created_at": iso(at) })),
            _ => None,
        },
        id: r.id,
        template_key: r.template_key,
    }
}

async fn active_label_keys(state: &AppState, ws: &str) -> sqlx::Result<HashSet<String>> {
    Ok(labels::active(&state.db, ws).await?.into_iter().map(|l| l.key).collect())
}

pub async fn list(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Items<Workflow>>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let rows: Vec<WorkflowRow> = sqlx::query_as(&format!("{WORKFLOW_SELECT} WHERE w.workspace_id = ?1 ORDER BY w.created_at, w.template_key"))
        .bind(&ws)
        .fetch_all(&state.db)
        .await?;
    let active = active_label_keys(&state, &ws).await?;
    Ok(Json(Items { items: rows.into_iter().map(|r| to_dto(r, &active)).collect() }))
}

#[derive(Deserialize)]
pub struct Patch {
    enabled: bool,
}

pub async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
    Json(body): Json<Patch>,
) -> AppResult<Json<Workflow>> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    let key: String = sqlx::query_scalar("SELECT template_key FROM workflows WHERE id = ?1 AND workspace_id = ?2")
        .bind(&id)
        .bind(&ws)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound)?;
    let active = active_label_keys(&state, &ws).await?;
    if body.enabled && !template(&key).is_some_and(|t| t.available && active.contains(t.label_key)) {
        return Err(AppError::Conflict("add and activate this workflow's label before enabling it".into()));
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE workflows SET enabled = ?1 WHERE id = ?2 AND workspace_id = ?3")
        .bind(body.enabled)
        .bind(&id)
        .bind(&ws)
        .execute(&mut *tx)
        .await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &ws,
            actor: Actor::User(&user.id),
            action: if body.enabled { "workflow.enabled" } else { "workflow.disabled" },
            entity_type: "workflow",
            entity_id: &id,
            message_id: None,
            metadata: json!({ "template_key": key }),
        },
    )
    .await?;
    let row: WorkflowRow = sqlx::query_as(&format!("{WORKFLOW_SELECT} WHERE w.id = ?1"))
        .bind(&id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(to_dto(row, &active)))
}

#[derive(FromRow, Serialize)]
pub struct RunRow {
    id: String,
    workflow_id: String,
    template_key: String,
    message_id: String,
    message_subject: String,
    status: String,
    error: Option<String>,
    #[serde(skip)]
    created_at: i64,
}

pub async fn runs(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let rows: Vec<RunRow> = sqlx::query_as(
        "SELECT r.id, r.workflow_id, w.template_key, r.message_id, m.subject AS message_subject, r.status, r.error, r.created_at
         FROM workflow_runs r
         JOIN workflows w ON w.id = r.workflow_id
         JOIN email_messages m ON m.id = r.message_id
         WHERE r.workspace_id = ?1
         ORDER BY r.created_at DESC LIMIT 100",
    )
    .bind(&ws)
    .fetch_all(&state.db)
    .await?;
    let items: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            let created_at = iso(r.created_at);
            let mut v = serde_json::to_value(&r).unwrap_or_default();
            v["created_at"] = json!(created_at);
            v
        })
        .collect();
    Ok(Json(json!({ "items": items })))
}

/// One click from "Needs label" to running: create (or restore) the workflow's label
/// from the built-in templates, then enable the workflow.
pub async fn setup(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
) -> AppResult<Json<Workflow>> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    let key: String = sqlx::query_scalar("SELECT template_key FROM workflows WHERE id = ?1 AND workspace_id = ?2")
        .bind(&id)
        .bind(&ws)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound)?;
    let t = template(&key).filter(|t| t.available).ok_or_else(|| AppError::Conflict("this workflow is not available".into()))?;
    let seed = crate::domain::LABEL_TEMPLATES
        .iter()
        .flat_map(|tpl| tpl.labels.iter())
        .find(|l| l.key == t.label_key)
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("no seed for label {}", t.label_key)))?;
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "INSERT INTO labels (id, workspace_id, key, name, description, color, position, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, (SELECT COALESCE(MAX(position), 0) + 1 FROM labels WHERE workspace_id = ?2), ?7)
         ON CONFLICT(workspace_id, key) DO UPDATE SET archived = 0, archived_by_pack = 0",
    )
    .bind(crate::db::new_id("lbl"))
    .bind(&ws)
    .bind(seed.key)
    .bind(seed.name)
    .bind(seed.description)
    .bind(seed.color)
    .bind(crate::db::now_ms())
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE workflows SET enabled = 1 WHERE id = ?1 AND workspace_id = ?2").bind(&id).bind(&ws).execute(&mut *tx).await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &ws,
            actor: Actor::User(&user.id),
            action: "workflow.set_up",
            entity_type: "workflow",
            entity_id: &id,
            message_id: None,
            metadata: json!({ "template_key": key, "label_key": t.label_key }),
        },
    )
    .await?;
    let row: WorkflowRow = sqlx::query_as(&format!("{WORKFLOW_SELECT} WHERE w.id = ?1")).bind(&id).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    let active = active_label_keys(&state, &ws).await?;
    Ok(Json(to_dto(row, &active)))
}
