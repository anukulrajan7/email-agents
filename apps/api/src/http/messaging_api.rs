//! Messaging API: the caller's in-app messages and notices, what Meera sent for them (with a Stop while it is held),
//! the conversations Meera is handling, support tickets, and teams. Everything here is scoped to the caller:
//! another member's messages, emails and tickets are never listed or reachable.

use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, post, put},
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::AppState;
use crate::{
    auth::{AuthUser, require_role},
    domain::Role,
    error::AppResult,
    messaging::{email, inapp, support, teams, watch},
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/workspaces/{ws}/in-app", get(inbox))
        .route("/workspaces/{ws}/in-app/unread", get(unread))
        .route("/workspaces/{ws}/in-app/sent", get(sent))
        .route("/workspaces/{ws}/in-app/sent/{id}/stop", post(stop))
        .route("/workspaces/{ws}/in-app/sent/{id}/approve", post(approve))
        .route("/workspaces/{ws}/in-app/{id}/read", post(read))
        .route("/workspaces/{ws}/in-app/{id}/reply", post(reply))
        .route("/workspaces/{ws}/conversations", get(conversations))
        .route("/workspaces/{ws}/conversations/{id}/close", post(close_conversation))
        .route("/workspaces/{ws}/support-tickets", get(tickets))
        .route("/workspaces/{ws}/support-tickets/{id}/status", post(ticket_status))
        .route("/workspaces/{ws}/teams", get(list_teams).post(create_team))
        .route("/workspaces/{ws}/teams/{id}", put(update_team).delete(delete_team))
}

async fn inbox(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let items = inapp::inbox(&state.db, &ws, &user.id).await?;
    let unread = inapp::unread_count(&state.db, &ws, &user.id).await?;
    Ok(Json(json!({ "items": items, "unread": unread })))
}

async fn unread(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    Ok(Json(json!({ "count": inapp::unread_count(&state.db, &ws, &user.id).await? })))
}

async fn read(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    inapp::mark_read(&state.db, &ws, &user.id, &id).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplyBody {
    body: String,
}

async fn reply(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
    Json(body): Json<ReplyBody>,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let reply_id = inapp::reply(&state, &ws, &user.id, &id, &body.body).await?;
    Ok(Json(json!({ "ok": true, "id": reply_id })))
}

/// What Meera sent for the caller: emails and in-app messages, newest first.
async fn sent(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let mut items: Vec<Value> = email::list_for(&state, &ws, &user.id).await?.iter().map(email::Outbound::to_json).collect();
    items.extend(inapp::sent(&state.db, &ws, &user.id).await?);
    items.sort_by(|a, b| b["at"].as_str().cmp(&a["at"].as_str()));
    Ok(Json(json!({ "items": items })))
}

async fn stop(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    if id.starts_with("imsg_") {
        inapp::stop(&state.db, &ws, &user.id, &id).await?;
    } else {
        email::stop(&state, &ws, &id, &user.id).await?;
    }
    Ok(Json(json!({ "ok": true, "status": "stopped" })))
}

async fn approve(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let scheduled = email::approve(&state, &ws, &id, &user.id).await?;
    Ok(Json(json!({ "ok": true, "send_at": crate::db::iso(scheduled.send_at_ms) })))
}

async fn conversations(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    Ok(Json(json!({ "items": watch::list(&state, &ws, &user.id).await? })))
}

async fn close_conversation(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    watch::close(&state, &ws, &user.id, &id).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn tickets(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    Ok(Json(json!({ "items": support::list(&state, &ws, &user.id).await? })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TicketStatus {
    status: String,
}

async fn ticket_status(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
    Json(body): Json<TicketStatus>,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    support::set_status(&state, &ws, &user.id, &id, &body.status).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn list_teams(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    Ok(Json(json!({ "items": teams::list(&state.db, &ws).await? })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamBody {
    name: String,
    #[serde(default)]
    member_ids: Vec<String>,
    desk_agent_id: Option<String>,
}

async fn create_team(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>, Json(body): Json<TeamBody>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    let id = teams::save(&state, &ws, &user.id, None, &body.name, &body.member_ids, body.desk_agent_id.as_deref()).await?;
    Ok(Json(json!({ "id": id })))
}

async fn update_team(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
    Json(body): Json<TeamBody>,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    teams::save(&state, &ws, &user.id, Some(&id), &body.name, &body.member_ids, body.desk_agent_id.as_deref()).await?;
    Ok(Json(json!({ "id": id })))
}

async fn delete_team(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    teams::delete(&state, &ws, &user.id, &id).await?;
    Ok(Json(json!({ "ok": true })))
}
