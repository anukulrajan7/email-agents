//! Workspace flags (off / compare / on) for admins: list them and set one.

use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, put},
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::AppState;
use crate::platform::{
    auth::{AuthUser, require_role},
    domain::Role,
    error::{AppError, AppResult},
    flags::{self, Flag, Key, Mode},
};

pub fn routes() -> Router<AppState> {
    Router::new().route("/workspaces/{ws}/flags", get(list)).route("/workspaces/{ws}/flags/{key}", put(set))
}

pub async fn list(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    Ok(Json(json!({ "items": flags::list(&state.db, &state.config, &ws).await? })))
}

#[derive(Deserialize)]
pub struct SetFlag {
    mode: Mode,
}

pub async fn set(State(state): State<AppState>, user: AuthUser, Path((ws, key)): Path<(String, String)>, Json(body): Json<SetFlag>) -> AppResult<Json<Flag>> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    let key = Key::parse(&key).ok_or_else(|| AppError::BadRequest(format!("unknown flag '{key}'")))?;
    Ok(Json(flags::set(&state.db, &state.config, &ws, &user.id, key, body.mode).await?))
}
