//! Assistant API: speech to text, a command that answers or prepares plan cards, and
//! single-use confirmation of those cards. Operators only.

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Multipart, Path, State},
    routing::post,
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::AppState;
use crate::{
    ai_ledger,
    assistant::{
        MAX_COMMAND_CHARS, command, confirm,
        transcribe::{AudioClip, MAX_AUDIO_BYTES, MAX_AUDIO_SECONDS},
    },
    auth::{AuthUser, require_role},
    domain::Role,
    error::{AppError, AppResult},
    sources::normalize::truncate_chars,
};

/// Room for the multipart envelope around a maximum-size clip.
const UPLOAD_LIMIT: usize = MAX_AUDIO_BYTES + 64 * 1024;
const MAX_TRANSCRIPT_CHARS: usize = 1_000;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/workspaces/{ws}/assistant/transcribe", post(transcribe).layer(DefaultBodyLimit::max(UPLOAD_LIMIT)))
        .route("/workspaces/{ws}/assistant/command", post(run_command))
        .route("/workspaces/{ws}/assistant/confirm", post(confirm_plan))
}

fn bad_upload(_: axum::extract::multipart::MultipartError) -> AppError {
    AppError::BadRequest("the recording could not be read; keep it under 2 MB".into())
}

async fn transcribe(
    State(state): State<AppState>,
    user: AuthUser,
    Path(ws): Path<String>,
    mut form: Multipart,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    if !state.transcriber.enabled() {
        return Err(AppError::Conflict("voice input is not set up yet; type your request instead".into()));
    }
    let mut audio: Option<(Vec<u8>, String)> = None;
    let mut duration_ms: Option<f64> = None;
    while let Some(field) = form.next_field().await.map_err(bad_upload)? {
        match field.name().unwrap_or_default() {
            "audio" | "file" => {
                let content_type = field.content_type().unwrap_or_default().to_string();
                let bytes = field.bytes().await.map_err(bad_upload)?;
                audio = Some((bytes.to_vec(), content_type));
            }
            "duration_ms" => duration_ms = field.text().await.map_err(bad_upload)?.trim().parse().ok(),
            _ => {}
        }
    }
    let (bytes, content_type) = audio.ok_or_else(|| AppError::BadRequest("send the recording in an audio field".into()))?;
    if bytes.is_empty() {
        return Err(AppError::BadRequest("the recording is empty".into()));
    }
    if bytes.len() > MAX_AUDIO_BYTES {
        return Err(AppError::BadRequest("the recording is too large; keep it under 2 MB".into()));
    }
    let lower = content_type.to_ascii_lowercase();
    if !(lower.starts_with("audio/") || lower.starts_with("video/webm")) {
        return Err(AppError::BadRequest("the recording must be audio".into()));
    }
    if duration_ms.is_some_and(|d| !d.is_finite() || d < 0.0 || d > MAX_AUDIO_SECONDS as f64 * 1000.0) {
        return Err(AppError::BadRequest(format!("the recording can be at most {MAX_AUDIO_SECONDS} seconds")));
    }
    let clip = AudioClip { bytes, content_type, seconds: duration_ms.map(|d| d / 1000.0) };
    let text = state.transcriber.transcribe(&clip).await.map_err(|e| {
        tracing::warn!(error = %e, "transcription failed");
        AppError::AiUnavailable
    })?;
    Ok(Json(json!({ "text": truncate_chars(&text, MAX_TRANSCRIPT_CHARS) })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandBody {
    text: String,
}

async fn run_command(
    State(state): State<AppState>,
    user: AuthUser,
    Path(ws): Path<String>,
    Json(body): Json<CommandBody>,
) -> AppResult<Json<command::Outcome>> {
    let role = require_role(&state.db, &ws, &user, Role::Operator).await?;
    let text = body.text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return Err(AppError::BadRequest("say or type what you need".into()));
    }
    if text.chars().count() > MAX_COMMAND_CHARS {
        return Err(AppError::BadRequest(format!("keep the request under {MAX_COMMAND_CHARS} characters")));
    }
    if !state.assistant_planner.enabled() {
        return Err(AppError::Conflict("the assistant is not set up yet; ask an admin to configure the AI service".into()));
    }
    // A person asked, so this always runs; the cost is recorded either way and the UI can say so.
    let over_budget = !ai_ledger::within_budget(&state.db, state.config.ai_daily_budget_usd, &ws).await;
    let mut outcome = command::run(&state, &ws, &user, role, &text).await?;
    outcome.over_budget = over_budget;
    Ok(Json(outcome))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmBody {
    plan_item_id: String,
}

async fn confirm_plan(
    State(state): State<AppState>,
    user: AuthUser,
    Path(ws): Path<String>,
    Json(body): Json<ConfirmBody>,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    Ok(Json(confirm::confirm(&state, &ws, &user, body.plan_item_id.trim()).await?))
}
