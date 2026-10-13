use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::AppState;
use crate::{
    auth::request_token,
    config::AuthMode,
    db::{iso, new_id, now_ms},
    error::{AppError, AppResult},
    sessions::{self, MAX_PASSWORD, MIN_PASSWORD},
    sources::normalize,
};

fn require_password_mode(state: &AppState) -> AppResult<()> {
    (state.config.auth_mode == AuthMode::Password)
        .then_some(())
        .ok_or_else(|| AppError::NotImplemented("password sign-in is not enabled on this server".into()))
}

fn normalize_email(raw: &str) -> AppResult<String> {
    let email = normalize::address(raw);
    let valid = email.len() <= 254
        && email.split_once('@').is_some_and(|(l, d)| !l.is_empty() && d.contains('.') && !d.starts_with('.'))
        && !email.chars().any(char::is_whitespace);
    valid.then_some(email).ok_or_else(|| AppError::BadRequest("enter a valid email address".into()))
}

fn session_json(token: String, expires_at: i64, id: &str, email: &str, name: Option<&str>) -> Value {
    json!({ "token": token, "expires_at": iso(expires_at), "user": { "id": id, "email": email, "name": name } })
}

pub async fn config(State(state): State<AppState>) -> Json<Value> {
    let mode = match state.config.auth_mode {
        AuthMode::Dev => "dev",
        AuthMode::Password => "password",
        AuthMode::Oidc => "oidc",
    };
    let signup = state.config.auth_mode == AuthMode::Password && state.config.signup_enabled;
    Json(json!({ "mode": mode, "signup_enabled": signup, "email_verification": state.mailer.is_some() }))
}

#[derive(Deserialize)]
pub struct SignupBody {
    email: String,
    password: String,
    name: Option<String>,
}

pub async fn signup(State(state): State<AppState>, Json(body): Json<SignupBody>) -> AppResult<(StatusCode, Json<Value>)> {
    require_password_mode(&state)?;
    if !state.config.signup_enabled {
        return Err(AppError::Forbidden);
    }
    let email = normalize_email(&body.email)?;
    let domain = email.rsplit('@').next().unwrap_or_default();
    let allowed = &state.config.signup_allowed_domains;
    if !allowed.is_empty() && !allowed.iter().any(|d| d == domain) {
        return Err(AppError::Forbidden);
    }
    let len = body.password.chars().count();
    if !(MIN_PASSWORD..=MAX_PASSWORD).contains(&len) {
        return Err(AppError::BadRequest(format!("password must be {MIN_PASSWORD}-{MAX_PASSWORD} characters")));
    }
    let name = body.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(|n| normalize::truncate_chars(n, 100));

    let hash = sessions::hash_password(body.password).await?;
    let id = new_id("usr");
    let now = now_ms();
    let verify = state.mailer.is_some();
    let inserted = sqlx::query(
        "INSERT OR IGNORE INTO users (id, external_auth_subject, email, name, password_hash, email_verified_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(&id)
    .bind(format!("pw|{email}"))
    .bind(&email)
    .bind(&name)
    .bind(&hash)
    .bind((!verify).then_some(now))
    .bind(now)
    .execute(&state.db)
    .await?
    .rows_affected()
        == 1;
    if !inserted {
        return Err(AppError::Conflict("an account with this email already exists".into()));
    }
    if verify {
        issue_code(&state, &id, &email).await?;
        return Ok((StatusCode::ACCEPTED, Json(json!({ "verification_required": true, "email": email }))));
    }
    let s = sessions::create_session(&state.db, &id).await?;
    Ok((StatusCode::CREATED, Json(session_json(s.token, s.expires_at, &id, &email, name.as_deref()))))
}

/// (id, name, password_hash, email_verified_at)
type LoginRow = (String, Option<String>, Option<String>, Option<i64>);

#[derive(Deserialize)]
pub struct LoginBody {
    email: String,
    password: String,
}

pub async fn login(State(state): State<AppState>, Json(body): Json<LoginBody>) -> AppResult<Json<Value>> {
    require_password_mode(&state)?;
    let email = normalize::address(&body.email);
    state.login_limiter.check(&email)?;
    let user: Option<LoginRow> =
        sqlx::query_as("SELECT id, name, password_hash, email_verified_at FROM users WHERE external_auth_subject = ?1")
            .bind(format!("pw|{email}"))
            .fetch_optional(&state.db)
            .await?;
    let stored = user.as_ref().and_then(|(_, _, h, _)| h.clone());
    let ok = body.password.chars().count() <= MAX_PASSWORD && sessions::verify_password(body.password, stored).await;
    let Some((id, name, _, verified_at)) = user.filter(|_| ok) else {
        state.login_limiter.fail(&email);
        tracing::info!("failed login");
        return Err(AppError::Unauthorized);
    };
    state.login_limiter.reset(&email);
    if verified_at.is_none() && state.mailer.is_some() {
        issue_code(&state, &id, &email).await?;
        return Err(AppError::EmailNotVerified);
    }
    let s = sessions::create_session(&state.db, &id).await?;
    Ok(Json(session_json(s.token, s.expires_at, &id, &email, name.as_deref())))
}

pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> AppResult<StatusCode> {
    if let Some(token) = request_token(&headers) {
        sessions::revoke(&state.db, token).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------- email verification

const CODE_TTL_MS: i64 = 10 * 60_000;
const CODE_MAX_ATTEMPTS: i64 = 5;
const RESEND_AFTER_MS: i64 = 60_000;

fn code_hash(user_id: &str, code: &str) -> String {
    sessions::hash_token(&format!("{user_id}:{code}"))
}

/// Creates and emails a fresh 6-digit code (at most one per minute), with retries. A failed send
/// is returned (`EmailNotSent`) so the sign-in screen can say so instead of waiting for a code
/// that will never arrive.
async fn issue_code(state: &AppState, user_id: &str, email: &str) -> AppResult<()> {
    let Some(mailer) = state.mailer.clone() else { return Ok(()) };
    let now = now_ms();
    let recent: Option<i64> = sqlx::query_scalar(
        "SELECT created_at FROM email_codes WHERE user_id = ?1 AND purpose = 'verify_email' ORDER BY created_at DESC LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();
    if recent.is_some_and(|t| now - t < RESEND_AFTER_MS) {
        return Ok(());
    }
    let code = format!("{:06}", rand::random::<u32>() % 1_000_000);
    let code_id = new_id("otp");
    let stored = sqlx::query(
        "INSERT INTO email_codes (id, user_id, purpose, code_hash, created_at, expires_at) VALUES (?1, ?2, 'verify_email', ?3, ?4, ?5)",
    )
    .bind(&code_id)
    .bind(user_id)
    .bind(code_hash(user_id, &code))
    .bind(now)
    .bind(now + CODE_TTL_MS)
    .execute(&state.db)
    .await;
    if let Err(e) = stored {
        tracing::error!(error = %e, "failed to store verification code");
        return Err(AppError::EmailNotSent(email.to_string()));
    }
    let (subject, text, html) = crate::mailer::verification_email(&code);
    let out = crate::mailer::OutgoingEmail { kind: crate::mailer::kind::OTP, to: email, subject: &subject, text: &text, html: &html };
    if let Err(e) = crate::mailer::deliver(&state.db, mailer.as_ref(), out).await {
        tracing::error!(error = %e, "failed to send verification email");
        // Not delivered: drop the code so "resend" isn't throttled by a code nobody received.
        if let Err(e) = sqlx::query("DELETE FROM email_codes WHERE id = ?1").bind(&code_id).execute(&state.db).await {
            tracing::warn!(error = %e, "could not drop an undelivered verification code");
        }
        return Err(AppError::EmailNotSent(email.to_string()));
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct VerifyBody {
    email: String,
    code: String,
}

pub async fn verify(State(state): State<AppState>, Json(body): Json<VerifyBody>) -> AppResult<Json<Value>> {
    require_password_mode(&state)?;
    let email = normalize::address(&body.email);
    let limit_key = format!("verify:{email}");
    state.login_limiter.check(&limit_key)?;
    let invalid = || AppError::BadRequest("invalid or expired code".into());
    let code = body.code.trim();
    let user: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT id, name FROM users WHERE external_auth_subject = ?1")
            .bind(format!("pw|{email}"))
            .fetch_optional(&state.db)
            .await?;
    let Some((user_id, name)) = user else {
        state.login_limiter.fail(&limit_key);
        return Err(invalid());
    };
    let now = now_ms();
    let latest: Option<(String, String, i64)> = sqlx::query_as(
        "SELECT id, code_hash, attempts FROM email_codes
         WHERE user_id = ?1 AND purpose = 'verify_email' AND consumed_at IS NULL AND expires_at > ?2
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(&user_id)
    .bind(now)
    .fetch_optional(&state.db)
    .await?;
    let Some((code_id, expected, _)) = latest.filter(|(_, _, a)| *a < CODE_MAX_ATTEMPTS) else {
        state.login_limiter.fail(&limit_key);
        return Err(invalid());
    };
    let matches = code.len() == 6 && code.chars().all(|c| c.is_ascii_digit()) && {
        use subtle::ConstantTimeEq;
        bool::from(code_hash(&user_id, code).as_bytes().ct_eq(expected.as_bytes()))
    };
    if !matches {
        sqlx::query("UPDATE email_codes SET attempts = attempts + 1 WHERE id = ?1").bind(&code_id).execute(&state.db).await?;
        state.login_limiter.fail(&limit_key);
        return Err(invalid());
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE email_codes SET consumed_at = ?1 WHERE id = ?2").bind(now).bind(&code_id).execute(&mut *tx).await?;
    sqlx::query("UPDATE users SET email_verified_at = COALESCE(email_verified_at, ?1) WHERE id = ?2")
        .bind(now)
        .bind(&user_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    state.login_limiter.reset(&limit_key);
    let s = sessions::create_session(&state.db, &user_id).await?;
    Ok(Json(session_json(s.token, s.expires_at, &user_id, &email, name.as_deref())))
}

#[derive(Deserialize)]
pub struct ResendBody {
    email: String,
}

/// Always 202: never reveals whether an account exists or is already verified.
pub async fn resend_code(State(state): State<AppState>, Json(body): Json<ResendBody>) -> AppResult<(StatusCode, Json<Value>)> {
    require_password_mode(&state)?;
    let email = normalize::address(&body.email);
    let user: Option<(String, Option<i64>)> =
        sqlx::query_as("SELECT id, email_verified_at FROM users WHERE external_auth_subject = ?1")
            .bind(format!("pw|{email}"))
            .fetch_optional(&state.db)
            .await?;
    if let Some((id, None)) = user {
        issue_code(&state, &id, &email).await?;
    }
    Ok((StatusCode::ACCEPTED, Json(json!({ "status": "sent" }))))
}
