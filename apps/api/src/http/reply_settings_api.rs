use axum::{
    Json,
    extract::{Path, State},
};
use serde::Serialize;
use serde_json::json;

use super::AppState;
use crate::{
    platform::audit::{self, Actor, Entry},
    platform::auth::{AuthUser, require_role},
    platform::domain::Role,
    platform::error::{AppError, AppResult},
    replies::reply_settings::{self, ReplySettings},
};

#[derive(Serialize)]
pub struct ReplySettingsView {
    #[serde(flatten)]
    settings: ReplySettings,
    /// Used for drafts while `reply_instructions` is empty; shown as the textarea placeholder.
    default_reply_style: &'static str,
}

pub async fn get_settings(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<ReplySettingsView>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let settings = reply_settings::load(&state.db, &state.config, &ws).await?;
    let default_reply_style = crate::platform::packs::reply_style(&state.db, &ws).await?;
    Ok(Json(ReplySettingsView { settings, default_reply_style }))
}

pub async fn put_settings(
    State(state): State<AppState>,
    user: AuthUser,
    Path(ws): Path<String>,
    Json(body): Json<ReplySettings>,
) -> AppResult<Json<ReplySettings>> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    let settings = body.validated().map_err(AppError::BadRequest)?;
    reply_settings::save(&state.db, &ws, &settings).await?;
    let mut conn = state.db.acquire().await?;
    audit::record(
        &mut conn,
        Entry {
            workspace_id: &ws,
            actor: Actor::User(&user.id),
            action: "reply_settings.updated",
            entity_type: "workspace",
            entity_id: &ws,
            message_id: None,
            metadata: json!({ "auto_reply": settings.auto_reply, "has_instructions": !settings.reply_instructions.is_empty(),
                              "has_signature": settings.signature.is_some() }),
        },
    )
    .await?;
    Ok(Json(settings))
}
