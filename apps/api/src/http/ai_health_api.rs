//! AI health for admins: the report, the incident list and resolving an incident.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::AppState;
use crate::{
    ai::health::{self, HealthReport},
    platform::audit::{self, Actor, Entry},
    platform::auth::{AuthUser, require_role},
    platform::domain::Role,
    platform::error::{AppError, AppResult},
    ai::incidents,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/workspaces/{ws}/ai-health", get(health))
        .route("/workspaces/{ws}/ai-incidents", get(list))
        .route("/workspaces/{ws}/ai-incidents/{id}/resolve", post(resolve))
}

#[derive(Deserialize)]
pub struct HealthQuery {
    days: Option<i64>,
}

pub async fn health(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>, Query(q): Query<HealthQuery>) -> AppResult<Json<HealthReport>> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    Ok(Json(health::report(&state.db, &ws, q.days.unwrap_or(14), state.config.ai_daily_budget_usd).await?))
}

#[derive(Deserialize)]
pub struct ListQuery {
    status: Option<String>,
}

pub async fn list(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>, Query(q): Query<ListQuery>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    let open_only = match q.status.as_deref().unwrap_or("open") {
        "open" => true,
        "all" => false,
        _ => return Err(AppError::BadRequest("status must be open or all".into())),
    };
    let rows = incidents::list(&state.db, &ws, open_only, 200).await?;
    Ok(Json(json!({ "items": rows })))
}

pub async fn resolve(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    if !incidents::resolve(&mut tx, &ws, &id, &user.id).await? {
        return Err(AppError::NotFound);
    }
    audit::record(
        &mut tx,
        Entry { workspace_id: &ws, actor: Actor::User(&user.id), action: "ai_incident.resolved", entity_type: "ai_incident", entity_id: &id, message_id: None, metadata: json!({}) },
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "id": id, "resolved": true })))
}
