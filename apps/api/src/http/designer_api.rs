//! System agent designer API. A session belongs to the person who opened it; creating an agent
//! needs an admin, changing an existing one needs whoever may edit it.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post, put},
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    AppState,
    agent_registry_api::{detail_json, look, require_edit, visible_agent},
};
use crate::{
    agents::registry::{spec::AgentSpec, store},
    platform::auth::{AuthUser, require_role},
    agents::designer::session::{self, SaveOptions, Session},
    platform::domain::Role,
    platform::error::{AppError, AppResult},
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/workspaces/{ws}/designer/sessions", post(create))
        .route("/workspaces/{ws}/designer/sessions/{id}", get(read).delete(abandon))
        .route("/workspaces/{ws}/designer/sessions/{id}/messages", post(message))
        .route("/workspaces/{ws}/designer/sessions/{id}/spec", put(put_spec))
        .route("/workspaces/{ws}/designer/sessions/{id}/dry-run", post(dry_run))
        .route("/workspaces/{ws}/designer/sessions/{id}/save", post(save))
}

/// A session of this member, in this workspace. Anyone else's session is not found.
async fn own_session(state: &AppState, ws: &str, user: &AuthUser, id: &str) -> AppResult<Session> {
    require_role(&state.db, ws, user, Role::Operator).await?;
    let s = session::get(&state.db, ws, id).await?.ok_or(AppError::NotFound)?;
    if s.user_id != user.id {
        return Err(AppError::NotFound);
    }
    Ok(s)
}

/// What the member may do with this session's target: change that agent, or create a new one.
async fn require_target(state: &AppState, ws: &str, user: &AuthUser, s: &Session) -> AppResult<()> {
    match &s.agent_id {
        Some(agent_id) => {
            let agent = visible_agent(state, ws, user, agent_id).await?;
            require_edit(state, ws, user, &agent).await
        }
        None => require_role(&state.db, ws, user, Role::Admin).await.map(|_| ()),
    }
}

async fn respond(state: &AppState, ws: &str, id: &str) -> AppResult<Json<Value>> {
    let s = session::get(&state.db, ws, id).await?.ok_or(AppError::NotFound)?;
    Ok(Json(session::view(state, ws, &s).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewSession {
    goal: String,
    agent_id: Option<String>,
}

async fn create(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>, Json(body): Json<NewSession>) -> AppResult<(StatusCode, Json<Value>)> {
    let agent = match &body.agent_id {
        Some(agent_id) => {
            require_role(&state.db, &ws, &user, Role::Operator).await?;
            let agent = visible_agent(&state, &ws, &user, agent_id).await?;
            require_edit(&state, &ws, &user, &agent).await?;
            Some(agent)
        }
        None => {
            require_role(&state.db, &ws, &user, Role::Admin).await?;
            None
        }
    };
    let s = session::create(&state, &ws, &user.id, &body.goal, agent.as_ref()).await?;
    let first_goal = s.goal.clone();
    // The first design turn runs with the request; if the designer cannot answer, the session still exists and can be retried.
    match session::design_turn(&state, &ws, &s.id, &first_goal).await {
        Ok(_) => {}
        Err(e) => {
            session::abandon(&state.db, &ws, &s.id).await?;
            return Err(e);
        }
    }
    let out = respond(&state, &ws, &s.id).await?;
    Ok((StatusCode::CREATED, out))
}

async fn read(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>) -> AppResult<Json<Value>> {
    own_session(&state, &ws, &user, &id).await?;
    respond(&state, &ws, &id).await
}

async fn abandon(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>) -> AppResult<Json<Value>> {
    own_session(&state, &ws, &user, &id).await?;
    session::abandon(&state.db, &ws, &id).await?;
    respond(&state, &ws, &id).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageBody {
    text: String,
}

async fn message(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
    Json(body): Json<MessageBody>,
) -> AppResult<Json<Value>> {
    let s = own_session(&state, &ws, &user, &id).await?;
    require_target(&state, &ws, &user, &s).await?;
    session::design_turn(&state, &ws, &id, &body.text).await?;
    respond(&state, &ws, &id).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpecBody {
    spec: AgentSpec,
}

async fn put_spec(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
    Json(body): Json<SpecBody>,
) -> AppResult<Json<Value>> {
    let s = own_session(&state, &ws, &user, &id).await?;
    require_target(&state, &ws, &user, &s).await?;
    session::set_spec(&state, &ws, &id, body.spec).await?;
    respond(&state, &ws, &id).await
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct DryRunBody {
    #[serde(default)]
    message_ids: Vec<String>,
}

async fn dry_run(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
    body: Option<Json<DryRunBody>>,
) -> AppResult<(StatusCode, Json<Value>)> {
    let s = own_session(&state, &ws, &user, &id).await?;
    require_target(&state, &ws, &user, &s).await?;
    let ids = body.map(|Json(b)| b.message_ids).unwrap_or_default();
    session::request_dry_run(&state, &ws, &id, &ids).await?;
    Ok((StatusCode::ACCEPTED, respond(&state, &ws, &id).await?))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SaveBody {
    start_level: Option<i64>,
    avatar: Option<String>,
    color: Option<String>,
    #[serde(default)]
    confirm_steals: bool,
}

async fn save(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
    body: Option<Json<SaveBody>>,
) -> AppResult<Json<Value>> {
    let s = own_session(&state, &ws, &user, &id).await?;
    require_target(&state, &ws, &user, &s).await?;
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let (avatar, color) = look(body.avatar.as_deref(), body.color.as_deref())?;
    let saved = session::save(&state, &ws, &user.id, &id, SaveOptions { start_level: body.start_level.unwrap_or(0), avatar: &avatar, color: &color, confirm_steals: body.confirm_steals }).await?;
    let agent = store::get_in(&state.db, &ws, &saved.agent_id).await?.ok_or(AppError::NotFound)?;
    let session = respond(&state, &ws, &id).await?.0;
    Ok(Json(json!({"agent": detail_json(&state, &ws, &agent).await?, "version": saved.version, "session": session})))
}
