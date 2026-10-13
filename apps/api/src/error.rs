use axum::{
    Json,
    extract::Request,
    http::{HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde_json::json;
use tracing::Instrument;
use uuid::Uuid;

tokio::task_local! {
    static REQUEST_ID: String;
}

pub fn current_request_id() -> String {
    REQUEST_ID.try_with(|id| id.clone()).unwrap_or_default()
}

/// Assigns a request ID, exposes it to error responses and log spans, and echoes it back.
pub async fn request_id_middleware(req: Request, next: Next) -> Response {
    let id = req
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty() && v.len() <= 64 && v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::now_v7().to_string());
    let span = tracing::info_span!("request", request_id = %id, method = %req.method(), path = %req.uri().path());
    let mut res = REQUEST_ID.scope(id.clone(), next.run(req).instrument(span)).await;
    if let Ok(v) = HeaderValue::from_str(&id) {
        res.headers_mut().insert("x-request-id", v);
    }
    res
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("authentication required")]
    Unauthorized,
    #[error("you do not have access to this resource")]
    Forbidden,
    /// 403 with a specific, user-facing reason (e.g. "Sign-up is by invitation").
    #[error("{0}")]
    Denied(String),
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    DraftIncomplete(String),
    #[error("{0}")]
    NotImplemented(String),
    #[error("dependency unavailable")]
    Unavailable,
    #[error("the AI service is unavailable right now; try again shortly")]
    AiUnavailable,
    #[error("too many attempts; try again later")]
    RateLimited,
    #[error("verify your email first; we sent you a new code")]
    EmailNotVerified,
    /// The send safety check found blocking issues. `details` = `{ blocking: Issue[], warnings: Issue[] }`.
    #[error("{0}")]
    SendBlocked(String, serde_json::Value),
    #[error("we couldn't send the email to {0}; check the address and try again in a minute")]
    EmailNotSent(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        match e {
            sqlx::Error::RowNotFound => AppError::NotFound,
            other => AppError::Internal(other.into()),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            AppError::Forbidden | AppError::Denied(_) => (StatusCode::FORBIDDEN, "forbidden"),
            AppError::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            AppError::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            AppError::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
            AppError::DraftIncomplete(_) => (StatusCode::CONFLICT, "draft_incomplete"),
            AppError::NotImplemented(_) => (StatusCode::NOT_IMPLEMENTED, "not_implemented"),
            AppError::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
            AppError::AiUnavailable => (StatusCode::SERVICE_UNAVAILABLE, "ai_unavailable"),
            AppError::RateLimited => (StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
            AppError::EmailNotVerified => (StatusCode::FORBIDDEN, "email_not_verified"),
            AppError::SendBlocked(..) => (StatusCode::CONFLICT, "send_blocked"),
            AppError::EmailNotSent(_) => (StatusCode::SERVICE_UNAVAILABLE, "email_not_sent"),
            AppError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        };
        let message = match &self {
            AppError::Internal(e) => {
                tracing::error!(error = ?e, "internal error");
                "internal server error".to_string()
            }
            other => other.to_string(),
        };
        let mut body = json!({ "error": { "code": code, "message": message, "request_id": current_request_id() } });
        if let AppError::SendBlocked(_, details) = &self
            && let (Some(err), Some(extra)) = (body["error"].as_object_mut(), details.as_object())
        {
            err.extend(extra.clone());
        }
        (status, Json(body)).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
