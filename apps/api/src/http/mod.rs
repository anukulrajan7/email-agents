mod dto;
mod jobs_api;
mod memories;
mod messages;
mod reviews;
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
    pub jobs_notify: Arc<Notify>,
}

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/me", get(workspaces::me))
        .route("/workspaces", get(workspaces::list).post(workspaces::create))
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
        .route("/workspaces/{ws}/messages/{id}/draft", post(messages::request_draft))
        .route("/workspaces/{ws}/messages/{id}/reprocess", post(messages::reprocess))
        .route("/workspaces/{ws}/drafts/{id}", patch(messages::update_draft))
        .route("/workspaces/{ws}/reviews", get(reviews::list))
        .route("/workspaces/{ws}/reviews/{id}", patch(reviews::update))
        .route("/workspaces/{ws}/workflows", get(workflows::list))
        .route("/workspaces/{ws}/workflows/{id}", patch(workflows::update))
        .route("/workspaces/{ws}/workflow-runs", get(workflows::runs))
        .route("/workspaces/{ws}/memories", get(memories::list).post(memories::create))
        .route("/workspaces/{ws}/memories/{id}", patch(memories::update))
        .route("/workspaces/{ws}/contacts", get(memories::contacts))
        .route("/workspaces/{ws}/contacts/{email}", get(memories::contact))
        .route("/workspaces/{ws}/jobs", get(jobs_api::list))
        .route("/workspaces/{ws}/jobs/{id}/retry", post(jobs_api::retry))
        .fallback(|| async { AppError::NotFound })
        .layer(DefaultBodyLimit::max(256 * 1024));

    let cors = CorsLayer::new()
        .allow_origin(
            HeaderValue::from_str(&state.config.public_web_url).unwrap_or(HeaderValue::from_static("http://localhost:5173")),
        )
        .allow_methods([Method::GET, Method::POST, Method::PATCH])
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
        .with_state(state)
}

async fn ready(State(state): State<AppState>) -> AppResult<Json<Value>> {
    sqlx::query("SELECT 1").execute(&state.db).await.map_err(|_| AppError::Unavailable)?;
    Ok(Json(json!({ "status": "ok" })))
}
