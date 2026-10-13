//! The workspace agent (admins configure it, everyone sees it) and each person's profile (people
//! edit their own; admins can read all). "Test on an email" writes a draft without saving it.

use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::AppState;
use crate::{
    agent_profiles::{self, AgentInput, Owner, UserProfile, UserProfileInput},
    ai::DraftQuality,
    audit::{self, Actor, Entry},
    auth::{AuthUser, require_role},
    domain::Role,
    error::{AppError, AppResult},
    jobs::handlers,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/workspaces/{ws}/agent", get(get_agent).put(put_agent))
        .route("/workspaces/{ws}/agent/profiles", get(list_profiles))
        .route("/workspaces/{ws}/agent/preview", post(preview))
        .route("/workspaces/{ws}/me/agent-profile", get(get_my_profile).put(put_my_profile))
        .route("/workspaces/{ws}/me/agent-profile/skip", post(skip_my_profile))
}

async fn record(state: &AppState, ws: &str, user: &AuthUser, action: &str, entity_type: &str, entity_id: &str, metadata: Value) -> AppResult<()> {
    let mut conn = state.db.acquire().await?;
    audit::record(
        &mut conn,
        Entry { workspace_id: ws, actor: Actor::User(&user.id), action, entity_type, entity_id, message_id: None, metadata },
    )
    .await?;
    Ok(())
}

async fn get_agent(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    let role = require_role(&state.db, &ws, &user, Role::Operator).await?;
    let agent = agent_profiles::agent(&state.db, &state.config, &ws).await?;
    Ok(Json(json!({
        "agent": agent.to_json(),
        "standard_guidance": agent_profiles::standard_guidance(),
        "can_edit": role == Role::Admin,
    })))
}

async fn put_agent(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>, Json(body): Json<Value>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    let input: AgentInput = serde_json::from_value(body).map_err(|e| AppError::BadRequest(format!("invalid agent settings: {e}")))?;
    let current = agent_profiles::agent(&state.db, &state.config, &ws).await?;
    let next = current.clone().apply(&input).map_err(AppError::BadRequest)?;
    agent_profiles::save_agent(&state.db, &ws, &next, &user.id).await?;
    record(
        &state,
        &ws,
        &user,
        "agent.updated",
        "workspace_agent",
        &ws,
        json!({
            "quality": next.quality, "send": next.permissions.send, "draft": next.permissions.draft, "tasks": next.tasks,
            "enabled": next.enabled, "persona_changed": next.persona != current.persona, "name_changed": next.name != current.name,
        }),
    )
    .await?;
    let saved = agent_profiles::agent(&state.db, &state.config, &ws).await?;
    Ok(Json(saved.to_json()))
}

async fn my_mailboxes(state: &AppState, ws: &str, user_id: &str) -> AppResult<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE(mailbox_address, inbound_alias, 'Connected mailbox') FROM email_sources
         WHERE workspace_id = ?1 AND connected_by_user_id = ?2 AND status != 'disconnected' ORDER BY created_at",
    )
    .bind(ws)
    .bind(user_id)
    .fetch_all(&state.db)
    .await?)
}

async fn get_my_profile(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let agent = agent_profiles::agent(&state.db, &state.config, &ws).await?;
    let profile = agent_profiles::user_profile(&state.db, &ws, &user.id).await?.unwrap_or_else(UserProfile::blank);
    let mailboxes = my_mailboxes(&state, &ws, &user.id).await?;
    let needs_onboarding = !mailboxes.is_empty() && !profile.saved && !profile.skipped;
    Ok(Json(json!({
        "profile": profile.to_json(),
        "agent": agent.badge(),
        "mailboxes": mailboxes,
        "needs_onboarding": needs_onboarding,
    })))
}

async fn put_my_profile(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>, Json(body): Json<Value>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let input: UserProfileInput = serde_json::from_value(body).map_err(|e| AppError::BadRequest(format!("invalid profile: {e}")))?;
    let current = agent_profiles::user_profile(&state.db, &ws, &user.id).await?.unwrap_or_else(UserProfile::blank);
    let next = current.apply(&input).map_err(AppError::BadRequest)?;
    agent_profiles::save_user_profile(&state.db, &ws, &user.id, &next).await?;
    record(&state, &ws, &user, "user_agent_profile.updated", "user", &user.id, json!({ "send_approval": next.approvals.send_email })).await?;
    let saved = agent_profiles::user_profile(&state.db, &ws, &user.id).await?.unwrap_or_else(UserProfile::blank);
    Ok(Json(saved.to_json()))
}

async fn skip_my_profile(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    agent_profiles::skip_user_profile(&state.db, &ws, &user.id).await?;
    record(&state, &ws, &user, "user_agent_profile.skipped", "user", &user.id, json!({})).await?;
    Ok(Json(json!({ "skipped": true })))
}

#[derive(sqlx::FromRow)]
struct MemberRow {
    user_id: String,
    email: String,
    role: String,
}

async fn list_profiles(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    let members: Vec<MemberRow> = sqlx::query_as(
        "SELECT u.id AS user_id, u.email, ms.role FROM memberships ms JOIN users u ON u.id = ms.user_id
         WHERE ms.workspace_id = ?1 ORDER BY u.email",
    )
    .bind(&ws)
    .fetch_all(&state.db)
    .await?;
    let mut items = Vec::with_capacity(members.len());
    for m in members {
        let profile = agent_profiles::user_profile(&state.db, &ws, &m.user_id).await?.filter(|p| p.saved);
        items.push(json!({
            "user_id": m.user_id, "email": m.email, "role": m.role,
            "mailboxes": my_mailboxes(&state, &ws, &m.user_id).await?,
            "profile": profile.map(|p| p.to_json()),
        }));
    }
    Ok(Json(json!({ "items": items })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewBody {
    message_id: String,
    /// Unsaved agent settings to try (admins).
    agent: Option<AgentInput>,
    /// Unsaved profile to try, as the current person.
    profile: Option<UserProfileInput>,
}

async fn preview(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>, Json(body): Json<PreviewBody>) -> AppResult<Json<Value>> {
    let role = require_role(&state.db, &ws, &user, Role::Operator).await?;
    let direction: Option<String> = sqlx::query_scalar("SELECT direction FROM email_messages WHERE id = ?1 AND workspace_id = ?2")
        .bind(&body.message_id)
        .bind(&ws)
        .fetch_optional(&state.db)
        .await?;
    match direction.as_deref() {
        None => return Err(AppError::NotFound),
        Some("inbound") => {}
        Some(_) => return Err(AppError::BadRequest("pick an email you received".into())),
    }
    let mut ctx = agent_profiles::context_for_message(&state.db, &state.config, &ws, &body.message_id).await?;
    if let Some(input) = &body.agent {
        if role != Role::Admin {
            return Err(AppError::Forbidden);
        }
        ctx.agent = ctx.agent.clone().apply(input).map_err(AppError::BadRequest)?;
    }
    if let Some(input) = &body.profile {
        let mine = agent_profiles::user_profile(&state.db, &ws, &user.id).await?.unwrap_or_else(UserProfile::blank);
        let profile = UserProfile { saved: true, ..mine.apply(input).map_err(AppError::BadRequest)? };
        ctx.owner = Some(Owner { user_id: user.id.clone(), email: user.email.clone(), profile: Some(profile) });
    }
    let quality = (ctx.agent.quality == "best").then_some(DraftQuality::Premium);
    let style = ctx.draft_style(&state.config);
    let text = agent_profiles::with_draft_style(style, handlers::preview_draft(&state, &body.message_id, quality)).await.map_err(|e| {
        tracing::warn!(message_id = %body.message_id, error = %e, "agent preview failed");
        AppError::AiUnavailable
    })?;
    Ok(Json(json!({ "body": text, "needs_reply": !text.trim().is_empty(), "agent": ctx.agent.badge() })))
}
