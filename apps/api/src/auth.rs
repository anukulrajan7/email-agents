use std::time::{Duration, Instant};

use axum::{extract::FromRequestParts, http::request::Parts};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::Deserialize;
use sqlx::SqlitePool;
use tokio::sync::RwLock;

use crate::{
    config::{AuthMode, Config},
    db::{new_id, now_ms},
    domain::Role,
    error::{AppError, AppResult},
    http::AppState,
};

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: String,
    pub email: String,
}

#[derive(Deserialize)]
struct Claims {
    sub: String,
    email: Option<String>,
    #[serde(default)]
    email_verified: Option<bool>,
}

const JWKS_TTL: Duration = Duration::from_secs(600);
const JWKS_MIN_REFRESH: Duration = Duration::from_secs(60);

pub struct Authenticator {
    mode: AuthMode,
    issuer: String,
    audience: String,
    http: reqwest::Client,
    jwks: RwLock<Option<(JwkSet, Instant)>>,
}

impl Authenticator {
    pub fn new(config: &Config, http: reqwest::Client) -> Self {
        Self {
            mode: config.auth_mode,
            issuer: config.oidc_issuer_url.trim_end_matches('/').to_string(),
            audience: config.oidc_audience.clone(),
            http,
            jwks: RwLock::new(None),
        }
    }

    pub fn mode(&self) -> AuthMode {
        self.mode
    }

    pub async fn authenticate(&self, db: &SqlitePool, token: &str) -> AppResult<AuthUser> {
        let (subject, email) = match self.mode {
            AuthMode::Password => {
                let (id, email) = crate::sessions::lookup(db, token).await?.ok_or(AppError::Unauthorized)?;
                return Ok(AuthUser { id, email });
            }
            AuthMode::Dev => dev_identity(token)?,
            AuthMode::Oidc => self.verify_oidc(token).await?,
        };
        // Read first: most requests are polling GETs and must not take SQLite's write lock.
        let existing: Option<(String, String)> =
            sqlx::query_as("SELECT id, email FROM users WHERE external_auth_subject = ?1")
                .bind(&subject)
                .fetch_optional(db)
                .await?;
        if let Some((id, stored)) = existing {
            if stored != email {
                sqlx::query("UPDATE users SET email = ?1 WHERE id = ?2").bind(&email).bind(&id).execute(db).await?;
            }
            return Ok(AuthUser { id, email });
        }
        let id: String = sqlx::query_scalar(
            "INSERT INTO users (id, external_auth_subject, email, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(external_auth_subject) DO UPDATE SET email = excluded.email
             RETURNING id",
        )
        .bind(new_id("usr"))
        .bind(&subject)
        .bind(&email)
        .bind(now_ms())
        .fetch_one(db)
        .await?;
        Ok(AuthUser { id, email })
    }

    async fn verify_oidc(&self, token: &str) -> AppResult<(String, String)> {
        let header = decode_header(token).map_err(|_| AppError::Unauthorized)?;
        // Only asymmetric algorithms: never let the token pick an HMAC "secret".
        if !matches!(
            header.alg,
            Algorithm::RS256 | Algorithm::RS384 | Algorithm::RS512 | Algorithm::ES256 | Algorithm::ES384 | Algorithm::PS256
        ) {
            return Err(AppError::Unauthorized);
        }
        let kid = header.kid.ok_or(AppError::Unauthorized)?;
        let jwk = match self.find_key(&kid, false).await? {
            Some(k) => k,
            None => self.find_key(&kid, true).await?.ok_or(AppError::Unauthorized)?,
        };
        let key = DecodingKey::from_jwk(&jwk).map_err(|_| AppError::Unauthorized)?;
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[&self.issuer, &format!("{}/", self.issuer)]);
        validation.set_audience(&[&self.audience]);
        let data = decode::<Claims>(token, &key, &validation).map_err(|_| AppError::Unauthorized)?;
        // Only a verified email may be used for invitations; otherwise store none.
        let email = match (data.claims.email, data.claims.email_verified) {
            (Some(e), Some(true)) => e.to_lowercase(),
            _ => String::new(),
        };
        Ok((format!("oidc|{}", data.claims.sub), email))
    }

    async fn find_key(&self, kid: &str, force_refresh: bool) -> AppResult<Option<jsonwebtoken::jwk::Jwk>> {
        {
            let cache = self.jwks.read().await;
            if let Some((set, fetched)) = cache.as_ref() {
                let fresh = fetched.elapsed() < JWKS_TTL;
                let throttled = fetched.elapsed() < JWKS_MIN_REFRESH;
                if (fresh && !force_refresh) || throttled {
                    return Ok(set.find(kid).cloned());
                }
            }
        }
        let set = self.fetch_jwks().await.map_err(|e| {
            tracing::warn!(error = %e, "failed to fetch JWKS");
            AppError::Unavailable
        })?;
        let key = set.find(kid).cloned();
        *self.jwks.write().await = Some((set, Instant::now()));
        Ok(key)
    }

    async fn fetch_jwks(&self) -> anyhow::Result<JwkSet> {
        #[derive(Deserialize)]
        struct Discovery {
            jwks_uri: String,
        }
        let discovery: Discovery = self
            .http
            .get(format!("{}/.well-known/openid-configuration", self.issuer))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(self.http.get(discovery.jwks_uri).send().await?.error_for_status()?.json().await?)
    }
}

fn dev_identity(token: &str) -> AppResult<(String, String)> {
    let email = token.strip_prefix("dev:").ok_or(AppError::Unauthorized)?.trim().to_lowercase();
    let valid = email.len() <= 254
        && email.split_once('@').is_some_and(|(l, d)| !l.is_empty() && d.contains('.'))
        && !email.chars().any(char::is_whitespace);
    if !valid {
        return Err(AppError::Unauthorized);
    }
    Ok((format!("dev|{email}"), email))
}

/// The client's session token: `X-Auth-Token` first (so a proxy Basic-auth gate can
/// keep `Authorization`), then `Authorization: Bearer`.
pub fn request_token(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get("x-auth-token")
        .and_then(|v| v.to_str().ok())
        .or_else(|| {
            headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
        })
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let token = request_token(&parts.headers).ok_or(AppError::Unauthorized)?;
        state.auth.authenticate(&state.db, token).await
    }
}

/// Server-side tenant authorization. The workspace ID in a URL is only a request;
/// membership is the proof. Unknown workspaces and non-members both get 403.
pub async fn require_role(db: &SqlitePool, workspace_id: &str, user: &AuthUser, min: Role) -> AppResult<Role> {
    let role: Option<String> =
        sqlx::query_scalar("SELECT role FROM memberships WHERE workspace_id = ?1 AND user_id = ?2")
            .bind(workspace_id)
            .bind(&user.id)
            .fetch_optional(db)
            .await?;
    let role = role.and_then(|r| Role::parse(&r)).ok_or(AppError::Forbidden)?;
    if min == Role::Admin && role != Role::Admin {
        return Err(AppError::Forbidden);
    }
    Ok(role)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_tokens() {
        assert!(dev_identity("dev:a@b.co").is_ok());
        assert_eq!(dev_identity("dev:A@B.CO").unwrap().1, "a@b.co");
        assert!(dev_identity("a@b.co").is_err());
        assert!(dev_identity("dev:nope").is_err());
        assert!(dev_identity("dev:a b@c.co").is_err());
    }
}
