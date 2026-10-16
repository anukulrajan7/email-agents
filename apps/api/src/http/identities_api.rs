//! "People we don't know yet": the names timesheets carried that the roster does not have, grouped, with what an admin may
//! do about each (link to a person, add as a person, mark as outside staff, say it is not a person) and undo. Admin only,
//! and only while the `handoff` flag acts. Nothing here creates a user: an unknown name is never someone who can sign in.

use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{AppState, staffing_api::gate, team_api};
use crate::{
    hr::team::identity::{
        changes,
        resolve::{self, NewPerson, Resolved},
        unresolved,
    },
    platform::{
        audit::Actor,
        auth::AuthUser,
        error::{AppError, AppResult},
        flags::{self, Key},
    },
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/workspaces/{ws}/staffing/identities", get(list))
        .route("/workspaces/{ws}/staffing/identities/{id}/link", post(link))
        .route("/workspaces/{ws}/staffing/identities/{id}/add-person", post(add_person))
        .route("/workspaces/{ws}/staffing/identities/{id}/external", post(external))
        .route("/workspaces/{ws}/staffing/identities/{id}/dismiss", post(dismiss))
        .route("/workspaces/{ws}/staffing/identity-changes/{id}/undo", post(undo))
}

async fn enabled(state: &AppState, ws: &str) -> AppResult<bool> {
    Ok(flags::mode(&state.db, &state.config, ws, Key::Handoff).await?.acts())
}

/// The admin, with the feature on. Anyone else is refused; a workspace with the feature off answers 409.
async fn admin(state: &AppState, ws: &str, user: &AuthUser) -> AppResult<()> {
    gate(state, ws, user).await?.require_owner()?;
    if !enabled(state, ws).await? {
        return Err(AppError::Conflict("People we don't know yet is turned off for this workspace".into()));
    }
    Ok(())
}

fn resolved(r: Resolved) -> Json<Value> {
    Json(json!({ "person_id": r.person_id, "record_ids": r.record_ids, "change_id": r.change_id }))
}

pub async fn list(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    gate(&state, &ws, &user).await?.require_owner()?;
    if !enabled(&state, &ws).await? {
        return Ok(Json(json!({ "enabled": false, "items": [], "recent": [] })));
    }
    Ok(Json(json!({ "enabled": true, "items": unresolved::open(&state.db, &ws).await?, "recent": changes::recent(&state.db, &ws, 10).await? })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkBody {
    person_id: String,
    /// Also remember the written name, so the same name matches next time. Off unless the admin says so.
    #[serde(default)]
    remember_name: bool,
}

/// "This is X": every sheet waiting under the card joins that person.
pub async fn link(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>, Json(body): Json<LinkBody>) -> AppResult<Json<Value>> {
    admin(&state, &ws, &user).await?;
    Ok(resolved(resolve::link_identity(&state, &ws, Actor::User(&user.id), &id, &body.person_id, body.remember_name).await?))
}

/// "Add them as a person": the card becomes a person on the roster (not a user) and every waiting sheet joins them.
pub async fn add_person(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>, Json(body): Json<NewPerson>) -> AppResult<Json<Value>> {
    admin(&state, &ws, &user).await?;
    let ctx = team_api::context(&state, &ws).await?;
    Ok(resolved(resolve::add_person(&state, &ws, Actor::User(&user.id), &id, body, &ctx).await?))
}

pub async fn external(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>) -> AppResult<Json<Value>> {
    admin(&state, &ws, &user).await?;
    let change = resolve::mark_external(&state, &ws, Actor::User(&user.id), &id).await?;
    Ok(Json(json!({ "change_id": change })))
}

pub async fn dismiss(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>) -> AppResult<Json<Value>> {
    admin(&state, &ws, &user).await?;
    let change = resolve::dismiss(&state, &ws, Actor::User(&user.id), &id).await?;
    Ok(Json(json!({ "change_id": change })))
}

pub async fn undo(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>) -> AppResult<Json<Value>> {
    admin(&state, &ws, &user).await?;
    let change = resolve::undo(&state, &ws, Actor::User(&user.id), &id).await?;
    Ok(Json(json!({ "change_id": change })))
}
