//! Automations API: list/create/update/delete, presets, run log and a dry-run test.
//! Admins configure, operators can read. Validation failures return `invalid_automation`.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, patch, post},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sqlx::FromRow;

use super::{AppState, dto::Items};
use crate::{
    audit::{self, Actor, Entry},
    auth::{AuthUser, require_role},
    automations::{self, Action, Automation, AutomationInput, AutomationPatch, Condition, MessageContext, PresetInfo},
    db::iso,
    domain::Role,
    error::{AppError, current_request_id},
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/workspaces/{ws}/automations", get(list).post(create))
        .route("/workspaces/{ws}/automations/presets", get(presets))
        .route("/workspaces/{ws}/automations/presets/{key}", post(create_from_preset))
        .route("/workspaces/{ws}/automations/{id}", patch(update).delete(remove))
        .route("/workspaces/{ws}/automations/{id}/runs", get(runs))
        .route("/workspaces/{ws}/automations/{id}/test", post(test))
}

#[derive(Debug)]
pub enum ApiError {
    App(AppError),
    Invalid(String),
}

impl From<AppError> for ApiError {
    fn from(e: AppError) -> Self {
        ApiError::App(e)
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        ApiError::App(e.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::App(e) => e.into_response(),
            ApiError::Invalid(message) => {
                let body = json!({ "error": { "code": "invalid_automation", "message": message, "request_id": current_request_id() } });
                (StatusCode::BAD_REQUEST, Json(body)).into_response()
            }
        }
    }
}

type ApiResult<T> = Result<T, ApiError>;

fn parse<T: DeserializeOwned>(body: Value) -> ApiResult<T> {
    serde_json::from_value(body).map_err(|e| ApiError::Invalid(e.to_string()))
}

fn check(name: &str, conditions: &[Condition], actions: &[Action]) -> ApiResult<()> {
    automations::validate(name, conditions, actions).map_err(ApiError::Invalid)
}

pub async fn list(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> ApiResult<Json<Items<Automation>>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    automations::seed_if_needed(&state.db, &ws).await?;
    Ok(Json(Items { items: automations::list(&state.db, &ws).await? }))
}

async fn create_validated(
    state: &AppState,
    ws: &str,
    user: &AuthUser,
    input: &AutomationInput,
    preset_key: Option<&str>,
) -> ApiResult<Automation> {
    check(&input.name, &input.conditions, &input.actions)?;
    let mut tx = state.db.begin().await?;
    let id = automations::insert(&mut tx, ws, input, preset_key).await?;
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM automations WHERE workspace_id = ?1")
        .bind(ws)
        .fetch_one(&mut *tx)
        .await?;
    if total > automations::MAX_AUTOMATIONS {
        tx.rollback().await?;
        return Err(ApiError::Invalid(format!("at most {} automations per workspace", automations::MAX_AUTOMATIONS)));
    }
    audit::record(
        &mut tx,
        Entry {
            workspace_id: ws,
            actor: Actor::User(&user.id),
            action: "automation.created",
            entity_type: "automation",
            entity_id: &id,
            message_id: None,
            metadata: json!({ "preset_key": preset_key, "enabled": input.enabled }),
        },
    )
    .await?;
    tx.commit().await?;
    automations::get(&state.db, ws, &id).await?.ok_or(ApiError::App(AppError::NotFound))
}

pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    Path(ws): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<(StatusCode, Json<Automation>)> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    let input: AutomationInput = parse(body)?;
    let created = create_validated(&state, &ws, &user, &input, None).await?;
    Ok((StatusCode::CREATED, Json(created)))
}

pub async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Automation>> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    let patch: AutomationPatch = parse(body)?;
    let current = automations::get(&state.db, &ws, &id).await?.ok_or(AppError::NotFound)?;
    let name = patch.name.unwrap_or(current.name);
    let enabled = patch.enabled.unwrap_or(current.enabled);
    let conditions = patch.conditions.unwrap_or(current.conditions);
    let actions = patch.actions.unwrap_or(current.actions);
    check(&name, &conditions, &actions)?;

    let mut tx = state.db.begin().await?;
    sqlx::query(
        "UPDATE automations SET name = ?1, enabled = ?2, conditions_json = ?3, actions_json = ?4, updated_at = ?5
         WHERE id = ?6 AND workspace_id = ?7",
    )
    .bind(name.trim())
    .bind(enabled)
    .bind(serde_json::to_string(&conditions).unwrap_or_else(|_| "[]".into()))
    .bind(serde_json::to_string(&actions).unwrap_or_else(|_| "[]".into()))
    .bind(crate::db::now_ms())
    .bind(&id)
    .bind(&ws)
    .execute(&mut *tx)
    .await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &ws,
            actor: Actor::User(&user.id),
            action: "automation.updated",
            entity_type: "automation",
            entity_id: &id,
            message_id: None,
            metadata: json!({ "enabled": enabled }),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Json(automations::get(&state.db, &ws, &id).await?.ok_or(AppError::NotFound)?))
}

pub async fn remove(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    let mut tx = state.db.begin().await?;
    let deleted = sqlx::query("DELETE FROM automations WHERE id = ?1 AND workspace_id = ?2")
        .bind(&id)
        .bind(&ws)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if deleted == 0 {
        tx.rollback().await?;
        return Err(AppError::NotFound.into());
    }
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &ws,
            actor: Actor::User(&user.id),
            action: "automation.deleted",
            entity_type: "automation",
            entity_id: &id,
            message_id: None,
            metadata: json!({}),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn presets(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> ApiResult<Json<Items<PresetInfo>>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let offered = crate::packs::presets_for(&crate::packs::enabled_keys(&state.db, &ws).await?);
    Ok(Json(Items { items: offered.iter().filter_map(|k| automations::preset(k)).collect() }))
}

pub async fn create_from_preset(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, key)): Path<(String, String)>,
) -> ApiResult<(StatusCode, Json<Automation>)> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    let offered = crate::packs::presets_for(&crate::packs::enabled_keys(&state.db, &ws).await?);
    let preset = automations::preset(&key).filter(|_| offered.contains(&key.as_str())).ok_or(AppError::NotFound)?;
    let created = create_validated(&state, &ws, &user, &preset.automation.into(), Some(&key)).await?;
    Ok((StatusCode::CREATED, Json(created)))
}

#[derive(Deserialize)]
pub struct RunsQuery {
    limit: Option<i64>,
}

#[derive(FromRow)]
struct RunRow {
    id: String,
    automation_id: String,
    message_id: String,
    message_subject: String,
    status: String,
    actions_json: String,
    created_at: i64,
}

#[derive(Serialize)]
pub struct RunDto {
    id: String,
    automation_id: String,
    message_id: String,
    message_subject: String,
    status: String,
    actions: Value,
    created_at: String,
}

pub async fn runs(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
    Query(q): Query<RunsQuery>,
) -> ApiResult<Json<Items<RunDto>>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    automations::get(&state.db, &ws, &id).await?.ok_or(AppError::NotFound)?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let rows: Vec<RunRow> = sqlx::query_as(
        "SELECT r.id, r.automation_id, r.message_id, m.subject AS message_subject, r.status, r.actions_json, r.created_at
         FROM automation_runs r JOIN email_messages m ON m.id = r.message_id
         WHERE r.automation_id = ?1 AND r.workspace_id = ?2
         ORDER BY r.created_at DESC, r.id DESC LIMIT ?3",
    )
    .bind(&id)
    .bind(&ws)
    .bind(limit)
    .fetch_all(&state.db)
    .await?;
    let items = rows
        .into_iter()
        .map(|r| RunDto {
            actions: serde_json::from_str(&r.actions_json).unwrap_or_else(|_| json!([])),
            id: r.id,
            automation_id: r.automation_id,
            message_id: r.message_id,
            message_subject: r.message_subject,
            status: r.status,
            created_at: iso(r.created_at),
        })
        .collect();
    Ok(Json(Items { items }))
}

/// Dry run over the last 50 messages. Nothing is written or sent.
pub async fn test(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let automation = automations::get(&state.db, &ws, &id).await?.ok_or(AppError::NotFound)?;
    let messages: Vec<MessageContext> = sqlx::query_as(&format!(
        "{} WHERE m.workspace_id = ?1 ORDER BY m.received_at DESC, m.id DESC LIMIT 50",
        automations::CONTEXT_SELECT
    ))
    .bind(&ws)
    .fetch_all(&state.db)
    .await?;
    let matched: Vec<Value> = messages
        .iter()
        .filter(|m| automations::matches_all(&automation.conditions, m))
        .map(|m| json!({ "message_id": m.id, "subject": m.subject }))
        .collect();
    Ok(Json(json!({ "matched": matched })))
}
