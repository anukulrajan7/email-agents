use std::net::IpAddr;

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::AppState;
use crate::{
    auth::{accept_invitations, request_token},
    config::AuthMode,
    db::{iso, new_id, now_ms},
    error::{AppError, AppResult},
    security::{self, ClientIp},
    sessions::{self, MAX_PASSWORD, MIN_PASSWORD},
    sources::normalize,
};

const VERIFY_EMAIL: &str = "verify_email";
const PASSWORD_RESET: &str = "password_reset";
/// Reset codes per email per hour (on top of one per minute).
const RESETS_PER_HOUR: i64 = 5;

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

/// Open to: allow-listed emails or domains, anyone an admin invited, and, when no allow-list
/// is configured, the very first account (bootstrap admin).
async fn signup_allowed(state: &AppState, email: &str) -> AppResult<bool> {
    let cfg = &state.config;
    let domain = email.rsplit('@').next().unwrap_or_default();
    if cfg.signup_allowed_emails.iter().any(|e| e == email) || cfg.signup_allowed_domains.iter().any(|d| d == domain) {
        return Ok(true);
    }
    let invited: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM invitations WHERE email = ?1)")
        .bind(email)
        .fetch_one(&state.db)
        .await?;
    if invited {
        return Ok(true);
    }
    let lists_empty = cfg.signup_allowed_emails.is_empty() && cfg.signup_allowed_domains.is_empty();
    let no_users: bool = sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM users)").fetch_one(&state.db).await?;
    Ok(lists_empty && no_users)
}

fn validate_password(password: &str) -> AppResult<()> {
    let len = password.chars().count();
    if !(MIN_PASSWORD..=MAX_PASSWORD).contains(&len) {
        return Err(AppError::BadRequest(format!("password must be {MIN_PASSWORD}-{MAX_PASSWORD} characters")));
    }
    Ok(())
}

pub async fn signup(State(state): State<AppState>, Json(body): Json<SignupBody>) -> AppResult<(StatusCode, Json<Value>)> {
    require_password_mode(&state)?;
    if !state.config.signup_enabled {
        return Err(AppError::Denied("Sign-up is by invitation".into()));
    }
    let email = normalize_email(&body.email)?;
    if !signup_allowed(&state, &email).await? {
        return Err(AppError::Denied("Sign-up is by invitation".into()));
    }
    validate_password(&body.password)?;
    let name = body.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(|n| normalize::truncate_chars(n, 100));

    let hash = sessions::hash_password(body.password).await?;
    let now = now_ms();
    let verify = state.mailer.is_some();
    let existing: Option<(String, Option<i64>)> =
        sqlx::query_as("SELECT id, email_verified_at FROM users WHERE external_auth_subject = ?1")
            .bind(format!("pw|{email}"))
            .fetch_optional(&state.db)
            .await?;
    let mut tx = state.db.begin().await?;
    let id = match existing {
        Some((_, Some(_))) => return Err(AppError::Conflict("an account with this email already exists".into())),
        // Never verified: nobody has proven they own this inbox, so an earlier sign-up can't
        // pre-claim it. The new password replaces the old one; old codes and sessions die.
        Some((id, None)) => {
            let replaced = sqlx::query(
                "UPDATE users SET password_hash = ?1, name = ?2, email_verified_at = ?3 WHERE id = ?4 AND email_verified_at IS NULL",
            )
            .bind(&hash)
            .bind(&name)
            .bind((!verify).then_some(now))
            .bind(&id)
            .execute(&mut *tx)
            .await?
            .rows_affected()
                == 1;
            if !replaced {
                return Err(AppError::Conflict("an account with this email already exists".into()));
            }
            sqlx::query("DELETE FROM email_codes WHERE user_id = ?1").bind(&id).execute(&mut *tx).await?;
            sqlx::query("DELETE FROM sessions WHERE user_id = ?1").bind(&id).execute(&mut *tx).await?;
            id
        }
        None => {
            let id = new_id("usr");
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
            .execute(&mut *tx)
            .await?
            .rows_affected()
                == 1;
            if !inserted {
                return Err(AppError::Conflict("an account with this email already exists".into()));
            }
            id
        }
    };
    if !verify {
        accept_invitations(&mut tx, &id, &email).await?;
    }
    tx.commit().await?;
    if verify {
        issue_code(&state, &id, &email, VERIFY_EMAIL).await?;
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

pub async fn login(State(state): State<AppState>, ClientIp(ip): ClientIp, Json(body): Json<LoginBody>) -> AppResult<Json<Value>> {
    require_password_mode(&state)?;
    let email = normalize::address(&body.email);
    slow_down(state.login_limiter.check_login(&email, ip)?).await;
    let user: Option<LoginRow> =
        sqlx::query_as("SELECT id, name, password_hash, email_verified_at FROM users WHERE external_auth_subject = ?1")
            .bind(format!("pw|{email}"))
            .fetch_optional(&state.db)
            .await?;
    let stored = user.as_ref().and_then(|(_, _, h, _)| h.clone());
    let ok = body.password.chars().count() <= MAX_PASSWORD && sessions::verify_password(body.password, stored).await;
    let Some((id, name, _, verified_at)) = user.filter(|_| ok) else {
        state.login_limiter.fail_login(&email, ip);
        tracing::info!("failed login");
        return Err(AppError::Unauthorized);
    };
    state.login_limiter.reset_login(&email, ip);
    if verified_at.is_none() && state.mailer.is_some() {
        issue_code(&state, &id, &email, VERIFY_EMAIL).await?;
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

async fn slow_down(delay: std::time::Duration) {
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }
}

// ---------------------------------------------------------------- email codes

const CODE_TTL_MS: i64 = 10 * 60_000;
const CODE_MAX_ATTEMPTS: i64 = 5;
const RESEND_AFTER_MS: i64 = 60_000;

fn code_hash(user_id: &str, code: &str) -> String {
    sessions::hash_token(&format!("{user_id}:{code}"))
}

/// Creates and emails a fresh 6-digit code for `purpose` (at most one per minute), with retries and
/// an outbox record. A failed send is returned (`EmailNotSent`); sign-up/verify callers surface it,
/// the password-reset caller ignores it so responses never reveal whether an account exists.
async fn issue_code(state: &AppState, user_id: &str, email: &str, purpose: &'static str) -> AppResult<()> {
    let Some(mailer) = state.mailer.clone() else { return Ok(()) };
    let now = now_ms();
    let recent: Option<i64> = sqlx::query_scalar(
        "SELECT created_at FROM email_codes WHERE user_id = ?1 AND purpose = ?2 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(user_id)
    .bind(purpose)
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
        "INSERT INTO email_codes (id, user_id, purpose, code_hash, created_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )
    .bind(&code_id)
    .bind(user_id)
    .bind(purpose)
    .bind(code_hash(user_id, &code))
    .bind(now)
    .bind(now + CODE_TTL_MS)
    .execute(&state.db)
    .await;
    if let Err(e) = stored {
        tracing::error!(error = %e, "failed to store email code");
        return Err(AppError::EmailNotSent(email.to_string()));
    }
    let (subject, text, html) =
        if purpose == PASSWORD_RESET { password_reset_email(&code) } else { crate::mailer::verification_email(&code) };
    let out = crate::mailer::OutgoingEmail { kind: crate::mailer::kind::OTP, to: email, subject: &subject, text: &text, html: &html };
    if let Err(e) = crate::mailer::deliver(&state.db, mailer.as_ref(), out).await {
        tracing::error!(error = %e, %purpose, "failed to send email code");
        // Not delivered: drop the code so "resend" isn't throttled by a code nobody received.
        if let Err(e) = sqlx::query("DELETE FROM email_codes WHERE id = ?1").bind(&code_id).execute(&state.db).await {
            tracing::warn!(error = %e, "could not drop an undelivered email code");
        }
        return Err(AppError::EmailNotSent(email.to_string()));
    }
    Ok(())
}

fn password_reset_email(code: &str) -> (String, String, String) {
    let subject = format!("{code} is your Post Room password reset code");
    let text = format!(
        "Your Post Room password reset code is {code}.\n\nIt expires in 10 minutes. If you didn't ask to reset your password, ignore this email: your password stays the same."
    );
    let html = format!(
        r#"<div style="font-family:Georgia,serif;background:#F3EFE6;padding:32px;color:#1C1B18">
<h1 style="font-weight:600;font-size:24px;margin:0 0 8px">Reset your password</h1>
<p style="font-family:Arial,sans-serif;color:#5E5A50;margin:0 0 16px">Enter this code in Post Room. It expires in 10 minutes.</p>
<p style="font-family:'Courier New',monospace;font-size:32px;letter-spacing:8px;margin:0 0 24px"><strong>{code}</strong></p>
<p style="font-family:Arial,sans-serif;font-size:12px;color:#5E5A50">If you didn't ask to reset your password, ignore this email.</p>
</div>"#
    );
    (subject, text, html)
}

/// Checks `code` against the user's latest live code for `purpose`; returns the code's id.
/// A wrong code counts against both the code (5 tries) and the limiter key.
async fn check_code(state: &AppState, user_id: &str, purpose: &str, code: &str, limit_key: &str, ip: IpAddr) -> AppResult<String> {
    let invalid = || AppError::BadRequest("invalid or expired code".into());
    let latest: Option<(String, String, i64)> = sqlx::query_as(
        "SELECT id, code_hash, attempts FROM email_codes
         WHERE user_id = ?1 AND purpose = ?2 AND consumed_at IS NULL AND expires_at > ?3
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(user_id)
    .bind(purpose)
    .bind(now_ms())
    .fetch_optional(&state.db)
    .await?;
    let Some((code_id, expected, _)) = latest.filter(|(_, _, a)| *a < CODE_MAX_ATTEMPTS) else {
        state.login_limiter.fail_login(limit_key, ip);
        return Err(invalid());
    };
    let matches = code.len() == 6 && code.chars().all(|c| c.is_ascii_digit()) && {
        use subtle::ConstantTimeEq;
        bool::from(code_hash(user_id, code).as_bytes().ct_eq(expected.as_bytes()))
    };
    if !matches {
        sqlx::query("UPDATE email_codes SET attempts = attempts + 1 WHERE id = ?1").bind(&code_id).execute(&state.db).await?;
        state.login_limiter.fail_login(limit_key, ip);
        return Err(invalid());
    }
    Ok(code_id)
}

#[derive(Deserialize)]
pub struct VerifyBody {
    email: String,
    code: String,
}

pub async fn verify(State(state): State<AppState>, ClientIp(ip): ClientIp, Json(body): Json<VerifyBody>) -> AppResult<Json<Value>> {
    require_password_mode(&state)?;
    let email = normalize::address(&body.email);
    let limit_key = format!("verify:{email}");
    slow_down(state.login_limiter.check_login(&limit_key, ip)?).await;
    let user: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT id, name FROM users WHERE external_auth_subject = ?1")
            .bind(format!("pw|{email}"))
            .fetch_optional(&state.db)
            .await?;
    let Some((user_id, name)) = user else {
        state.login_limiter.fail_login(&limit_key, ip);
        return Err(AppError::BadRequest("invalid or expired code".into()));
    };
    let code_id = check_code(&state, &user_id, VERIFY_EMAIL, body.code.trim(), &limit_key, ip).await?;
    let now = now_ms();
    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE email_codes SET consumed_at = ?1 WHERE id = ?2").bind(now).bind(&code_id).execute(&mut *tx).await?;
    let first_verification = sqlx::query("UPDATE users SET email_verified_at = ?1 WHERE id = ?2 AND email_verified_at IS NULL")
        .bind(now)
        .bind(&user_id)
        .execute(&mut *tx)
        .await?
        .rows_affected()
        == 1;
    if first_verification {
        // Sessions from before the inbox was proven (e.g. a pre-claim) don't survive it.
        sqlx::query("DELETE FROM sessions WHERE user_id = ?1").bind(&user_id).execute(&mut *tx).await?;
        accept_invitations(&mut tx, &user_id, &email).await?;
    }
    tx.commit().await?;
    state.login_limiter.reset_login(&limit_key, ip);
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
        issue_code(&state, &id, &email, VERIFY_EMAIL).await?;
    }
    Ok((StatusCode::ACCEPTED, Json(json!({ "status": "sent" }))))
}

// ---------------------------------------------------------------- password reset

/// Always 200 `{status: "sent"}`: neither the answer nor its timing (the code is sent in the
/// background) reveals whether the account exists.
pub async fn password_reset_request(State(state): State<AppState>, Json(body): Json<ResendBody>) -> AppResult<Json<Value>> {
    require_password_mode(&state)?;
    let email = normalize::address(&body.email);
    let user: Option<String> = sqlx::query_scalar("SELECT id FROM users WHERE external_auth_subject = ?1")
        .bind(format!("pw|{email}"))
        .fetch_optional(&state.db)
        .await?;
    if let Some(id) = user {
        let task = async move {
            match security::take(&state.db, PASSWORD_RESET, &email, RESETS_PER_HOUR, 3_600_000).await {
                Ok(true) => {
                    // Never surfaced: the reset response must not reveal account state.
                    let _ = issue_code(&state, &id, &email, PASSWORD_RESET).await;
                }
                Ok(false) => tracing::info!("password reset codes rate limited"),
                Err(e) => tracing::warn!(error = %e, "password reset limit check failed"),
            }
        };
        tokio::spawn(task);
    }
    Ok(Json(json!({ "status": "sent" })))
}

#[derive(Deserialize)]
pub struct ResetConfirmBody {
    email: String,
    code: String,
    new_password: String,
}

/// Sets the new password and signs out every session; the person then signs in again.
/// Receiving the code also proves the inbox, so an unverified account becomes verified.
pub async fn password_reset_confirm(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Json(body): Json<ResetConfirmBody>,
) -> AppResult<Json<Value>> {
    require_password_mode(&state)?;
    validate_password(&body.new_password)?;
    let email = normalize::address(&body.email);
    let limit_key = format!("reset:{email}");
    slow_down(state.login_limiter.check_login(&limit_key, ip)?).await;
    let user: Option<String> = sqlx::query_scalar("SELECT id FROM users WHERE external_auth_subject = ?1")
        .bind(format!("pw|{email}"))
        .fetch_optional(&state.db)
        .await?;
    let Some(user_id) = user else {
        state.login_limiter.fail_login(&limit_key, ip);
        return Err(AppError::BadRequest("invalid or expired code".into()));
    };
    let code_id = check_code(&state, &user_id, PASSWORD_RESET, body.code.trim(), &limit_key, ip).await?;
    let hash = sessions::hash_password(body.new_password).await?;
    let now = now_ms();
    let mut tx = state.db.begin().await?;
    let consumed = sqlx::query("UPDATE email_codes SET consumed_at = ?1 WHERE id = ?2 AND consumed_at IS NULL")
        .bind(now)
        .bind(&code_id)
        .execute(&mut *tx)
        .await?
        .rows_affected()
        == 1;
    if !consumed {
        return Err(AppError::BadRequest("invalid or expired code".into()));
    }
    let first_verification: bool = sqlx::query_scalar("SELECT email_verified_at IS NULL FROM users WHERE id = ?1")
        .bind(&user_id)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("UPDATE users SET password_hash = ?1, email_verified_at = COALESCE(email_verified_at, ?2) WHERE id = ?3")
        .bind(&hash)
        .bind(now)
        .bind(&user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE email_codes SET consumed_at = ?1 WHERE user_id = ?2 AND consumed_at IS NULL")
        .bind(now)
        .bind(&user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM sessions WHERE user_id = ?1").bind(&user_id).execute(&mut *tx).await?;
    if first_verification {
        accept_invitations(&mut tx, &user_id, &email).await?;
    }
    tx.commit().await?;
    state.login_limiter.reset_login(&limit_key, ip);
    state.login_limiter.reset_login(&email, ip);
    tracing::info!(user_id = %user_id, "password reset");
    Ok(Json(json!({ "status": "password_reset" })))
}
