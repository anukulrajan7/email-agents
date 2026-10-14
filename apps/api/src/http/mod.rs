mod agent_profiles_api;
mod agent_registry_api;
mod ai_usage_api;
mod agents_api;
mod alerts_api;
mod assistant_api;
mod auth_api;
mod automations_api;
mod dto;
mod jobs_api;
mod label_health;
mod labels_api;
mod matching_api;
mod memories;
mod messages;
mod reply_settings_api;
mod reviews;
mod search_api;
mod sources;
mod webhooks;
mod workflows;
mod workspaces;

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderName, HeaderValue, Method, header},
    middleware,
    routing::{get, patch, post},
};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tokio::sync::Notify;
use tower_http::{
    cors::CorsLayer,
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};

use crate::{
    ai::{Classifier, Drafter, Learner},
    auth::Authenticator,
    config::Config,
    error::{AppError, AppResult, request_id_middleware},
    sources::{nango::NangoClient, resend::ResendClient},
};

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub config: Arc<Config>,
    pub auth: Arc<Authenticator>,
    pub resend: Arc<ResendClient>,
    pub nango: Arc<NangoClient>,
    pub classifier: Arc<dyn Classifier>,
    pub drafter: Arc<dyn Drafter>,
    /// Proposes memory suggestions from operator draft edits; `None` disables learning.
    pub learner: Option<Arc<dyn Learner>>,
    pub extractor: Arc<dyn crate::ai::Extractor>,
    pub analyst: Arc<dyn crate::ai::Analyst>,
    pub embedder: Arc<dyn crate::ai::embeddings::Embedder>,
    pub label_suggester: Arc<dyn crate::ai::label_suggest::LabelSuggester>,
    pub login_limiter: Arc<crate::sessions::LoginLimiter>,
    /// Sends verification codes; `None` = verification disabled.
    pub mailer: Option<Arc<dyn crate::mailer::Mailer>>,
    pub jobs_notify: Arc<Notify>,
    /// Learns durable facts from emails and replies (no-op without a model).
    pub memory_extractor: Arc<dyn crate::ai::memory_extractor::MemoryExtractor>,
    /// Sends alert emails; `None` = alerts stay in-app only.
    pub alert_mailer: Option<Arc<dyn crate::mailer::Mailer>>,
    /// Picks each agent step (tool calling). Disabled without a model: no automatic cases.
    pub planner: Arc<dyn crate::agents::planner::Planner>,
    /// Checks agent proposals before they run or go to a person.
    pub critic: Arc<dyn crate::agents::critic::Critic>,
    /// Cheap typed decisions (yes/no, choice, score); see `ai::judge`.
    pub judge: Arc<dyn crate::ai::judge::Judge>,
    /// Answers voice/chat commands with a bounded tool loop; actions wait for confirmation.
    pub assistant_planner: Arc<dyn crate::assistant::CommandPlanner>,
    /// Speech to text for the assistant's push-to-talk.
    pub transcriber: Arc<dyn crate::assistant::transcribe::Transcriber>,
    /// Reads image attachments; `None` (no OpenRouter key or vision model) skips them.
    pub vision: Option<Arc<dyn crate::attachments::vision::VisionReader>>,
    /// Transcribes audio attachments; `None` skips them.
    pub audio: Option<Arc<dyn crate::attachments::audio::AudioReader>>,
}

pub fn router(state: AppState) -> Router {
    // Endpoints that hash passwords or send email: limited per client IP.
    let auth = Router::new()
        .route("/auth/signup", post(auth_api::signup))
        .route("/auth/login", post(auth_api::login))
        .route("/auth/verify", post(auth_api::verify))
        .route("/auth/resend-code", post(auth_api::resend_code))
        .route("/auth/password-reset/request", post(auth_api::password_reset_request))
        .route("/auth/password-reset/confirm", post(auth_api::password_reset_confirm))
        .route_layer(middleware::from_fn_with_state(state.clone(), crate::security::auth_rate_limit));
    let api = Router::new()
        .route("/me", get(workspaces::me))
        .route("/auth/config", get(auth_api::config))
        .route("/auth/logout", post(auth_api::logout))
        .merge(auth)
        .route("/workspaces", get(workspaces::list).post(workspaces::create))
        .route("/label-templates", get(labels_api::templates))
        .route("/packs", get(workspaces::pack_catalogue))
        .route("/workspaces/{ws}/packs", get(workspaces::get_packs).put(workspaces::put_packs))
        .route("/workspaces/{ws}/labels", get(labels_api::list).post(labels_api::create))
        .route("/workspaces/{ws}/labels/suggest", post(labels_api::suggest))
        .route("/workspaces/{ws}/labels/health", get(label_health::health))
        .route("/workspaces/{ws}/labels/{id}", patch(labels_api::update))
        .route("/workspaces/{ws}/members", post(workspaces::add_member))
        .route("/workspaces/{ws}/sources", get(sources::list))
        .route("/workspaces/{ws}/sources/forwarding", post(sources::create_forwarding))
        .route("/workspaces/{ws}/sources/nango-session", post(sources::nango_session))
        .route("/workspaces/{ws}/sources/nango-connections", post(sources::nango_connection))
        .route("/workspaces/{ws}/sources/{id}/disconnect", post(sources::disconnect))
        .route("/workspaces/{ws}/sources/{id}/sync", post(sources::sync))
        .route("/workspaces/{ws}/sources/{id}/test-email", post(sources::test_email))
        .route("/workspaces/{ws}/messages", get(messages::list))
        .route("/workspaces/{ws}/messages/{id}", get(messages::detail))
        .route("/workspaces/{ws}/messages/{id}/classification", patch(messages::override_classification))
        .route("/workspaces/{ws}/messages/{id}/classification/approve", post(messages::approve_classification))
        .route("/workspaces/{ws}/messages/{id}/draft", post(messages::request_draft))
        .route("/workspaces/{ws}/messages/{id}/reprocess", post(messages::reprocess))
        .route("/workspaces/{ws}/messages/{id}/insight", post(messages::understand_now))
        .route("/workspaces/{ws}/drafts/{id}", patch(messages::update_draft))
        .route("/workspaces/{ws}/drafts/{id}/provider-draft", post(crate::replies::save_to_provider))
        .route("/workspaces/{ws}/drafts/{id}/send", post(crate::replies::send))
        .route("/workspaces/{ws}/drafts/{id}/check", post(crate::send_safety::check_draft))
        .route("/workspaces/{ws}/drafts/{id}/undo-send", post(crate::replies::undo_send))
        .route("/workspaces/{ws}/reviews", get(reviews::list))
        .route("/workspaces/{ws}/reviews/{id}", patch(reviews::update))
        .route("/workspaces/{ws}/workflows", get(workflows::list))
        .route("/workspaces/{ws}/workflows/{id}", patch(workflows::update))
        .route("/workspaces/{ws}/workflows/{id}/setup", post(workflows::setup))
        .route("/workspaces/{ws}/workflow-runs", get(workflows::runs))
        .route("/workspaces/{ws}/memories", get(memories::list).post(memories::create))
        .route("/workspaces/{ws}/memories/{id}", patch(memories::update))
        .route("/workspaces/{ws}/contacts", get(memories::contacts))
        .route("/workspaces/{ws}/contacts/{email}", get(memories::contact))
        .route("/workspaces/{ws}/matching/setup", post(matching_api::setup))
        .route("/workspaces/{ws}/matching/requirements", get(matching_api::requirements))
        .route("/workspaces/{ws}/matching/requirements/{id}", patch(matching_api::update_requirement))
        .route("/workspaces/{ws}/matching/candidates", get(matching_api::candidates))
        .route("/workspaces/{ws}/matching/matches", get(matching_api::matches))
        .route("/workspaces/{ws}/matching/matches/{id}", patch(matching_api::decide))
        .route("/workspaces/{ws}/search", get(search_api::search))
        .route("/workspaces/{ws}/alerts", get(alerts_api::list))
        .route("/workspaces/{ws}/alerts/{id}", patch(alerts_api::update))
        .route("/workspaces/{ws}/alert-settings", get(alerts_api::get_settings).put(alerts_api::put_settings))
        .route("/workspaces/{ws}/alert-settings/test", post(alerts_api::test))
        .route("/workspaces/{ws}/reply-settings", get(reply_settings_api::get_settings).put(reply_settings_api::put_settings))
        .route("/workspaces/{ws}/ai-usage", get(ai_usage_api::get))
        .route("/workspaces/{ws}/jobs", get(jobs_api::list))
        .route("/workspaces/{ws}/jobs/{id}/retry", post(jobs_api::retry))
        .merge(automations_api::routes())
        .merge(agents_api::routes())
        .merge(agent_profiles_api::routes())
        .merge(agent_registry_api::routes())
        .merge(assistant_api::routes())
        .fallback(|| async { AppError::NotFound })
        .layer(middleware::from_fn(crate::ai_ledger::attach_context))
        .layer(DefaultBodyLimit::max(256 * 1024));

    let cors = CorsLayer::new()
        .allow_origin(
            HeaderValue::from_str(&state.config.public_web_url).unwrap_or(HeaderValue::from_static("http://localhost:5173")),
        )
        .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::PUT, Method::DELETE])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE, HeaderName::from_static("x-auth-token")])
        .expose_headers([HeaderName::from_static("x-request-id")]);

    let web_dir = state.config.web_dist_dir.clone();
    let app = Router::new()
        .route("/health/live", get(|| async { Json(json!({ "status": "ok" })) }))
        .route("/health/ready", get(ready))
        .route("/webhooks/resend", post(webhooks::resend).layer(DefaultBodyLimit::max(1024 * 1024)))
        .route("/webhooks/nango", post(webhooks::nango))
        .nest("/api/v1", api);
    // Production: serve the built SPA; unknown paths fall back to index.html for client routing.
    let app = if web_dir.is_empty() {
        app.fallback(|| async { AppError::NotFound })
    } else {
        let index = std::path::Path::new(&web_dir).join("index.html");
        // `fallback` (not `not_found_service`) so deep links return 200, not 404.
        app.fallback_service(ServeDir::new(&web_dir).fallback(ServeFile::new(index)))
    };
    app.layer(TraceLayer::new_for_http())
        .layer(middleware::from_fn(request_id_middleware))
        .layer(cors)
        .layer(middleware::from_fn(crate::security::security_headers))
        .with_state(state)
}

async fn ready(State(state): State<AppState>) -> AppResult<Json<Value>> {
    sqlx::query("SELECT 1").execute(&state.db).await.map_err(|_| AppError::Unavailable)?;
    Ok(Json(json!({ "status": "ok" })))
}
