//! Delete emails from Post Room (the provider keeps its copy): one, a selection, or a whole thread.

use axum::{
    Json,
    extract::{Path, State},
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::AppState;
use crate::{
    audit::Actor,
    auth::AuthUser,
    deletion::{self, MAX_SELECTION, Opts, Removed},
    error::{AppError, AppResult},
};

#[derive(Deserialize)]
pub struct Selection {
    ids: Vec<String>,
}

async fn run(state: &AppState, ws: &str, user: &AuthUser, ids: Vec<String>) -> AppResult<Json<Value>> {
    let mut unique: Vec<String> = Vec::with_capacity(ids.len());
    for id in ids {
        if !unique.contains(&id) {
            unique.push(id);
        }
    }
    if unique.is_empty() {
        return Err(AppError::BadRequest("choose at least one email".into()));
    }
    if unique.len() > MAX_SELECTION {
        return Err(AppError::BadRequest(format!("delete at most {MAX_SELECTION} emails at a time")));
    }
    deletion::authorize(&state.db, ws, user, &unique).await?;
    let removed: Removed =
        deletion::delete_emails(&state.db, ws, &unique, Opts { actor: Actor::User(&user.id), tombstone: true, audit: true }).await?;
    Ok(Json(json!({ "deleted": removed.emails, "removed": removed })))
}

pub async fn delete_one(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
) -> AppResult<Json<Value>> {
    run(&state, &ws, &user, vec![id]).await
}

pub async fn delete_many(
    State(state): State<AppState>,
    user: AuthUser,
    Path(ws): Path<String>,
    Json(body): Json<Selection>,
) -> AppResult<Json<Value>> {
    run(&state, &ws, &user, body.ids).await
}

/// Every email of the conversation this email belongs to.
pub async fn delete_thread(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
) -> AppResult<Json<Value>> {
    deletion::authorize(&state.db, &ws, &user, std::slice::from_ref(&id)).await?;
    let ids = deletion::thread_of(&state.db, &ws, &id).await?;
    let removed = deletion::delete_emails(&state.db, &ws, &ids, Opts { actor: Actor::User(&user.id), tombstone: true, audit: true }).await?;
    Ok(Json(json!({ "deleted": removed.emails, "removed": removed })))
}
