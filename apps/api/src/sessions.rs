//! Email + password accounts: Argon2id hashing (off the async runtime), opaque session
//! tokens stored only as SHA-256 hashes, and a small in-memory login rate limiter.

use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngCore;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::{
    db::{new_id, now_ms},
    error::AppError,
};

pub const SESSION_TTL_MS: i64 = 30 * 86_400_000;
/// Avoid a write on every request: only refresh `last_used_at` when it is this stale.
const TOUCH_AFTER_MS: i64 = 10 * 60_000;
pub const MIN_PASSWORD: usize = 10;
pub const MAX_PASSWORD: usize = 128;

pub fn hash_token(token: &str) -> String {
    Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn new_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("pr_{}", URL_SAFE_NO_PAD.encode(bytes))
}

pub async fn hash_password(password: String) -> Result<String, AppError> {
    tokio::task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default().hash_password(password.as_bytes(), &salt).map(|h| h.to_string())
    })
    .await
    .map_err(|e| AppError::Internal(e.into()))?
    .map_err(|e| AppError::Internal(anyhow::anyhow!("hashing failed: {e}")))
}

/// Verifies against the stored hash, or against a dummy hash when the account does not
/// exist, so response time doesn't reveal which emails have accounts.
pub async fn verify_password(password: String, stored: Option<String>) -> bool {
    static DUMMY: OnceLock<String> = OnceLock::new();
    let stored = match stored {
        Some(s) => s,
        None => DUMMY
            .get_or_init(|| {
                let salt = SaltString::generate(&mut OsRng);
                Argon2::default().hash_password(b"not-a-real-password", &salt).map(|h| h.to_string()).unwrap_or_default()
            })
            .clone(),
    };
    tokio::task::spawn_blocking(move || {
        PasswordHash::new(&stored).is_ok_and(|parsed| Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok())
    })
    .await
    .unwrap_or(false)
}

pub struct NewSession {
    pub token: String,
    pub expires_at: i64,
}

pub async fn create_session(db: &SqlitePool, user_id: &str) -> sqlx::Result<NewSession> {
    let token = new_token();
    let now = now_ms();
    let expires_at = now + SESSION_TTL_MS;
    sqlx::query(
        "INSERT INTO sessions (id, user_id, token_hash, created_at, expires_at, last_used_at) VALUES (?1, ?2, ?3, ?4, ?5, ?4)",
    )
    .bind(new_id("ses"))
    .bind(user_id)
    .bind(hash_token(&token))
    .bind(now)
    .bind(expires_at)
    .execute(db)
    .await?;
    Ok(NewSession { token, expires_at })
}

/// Returns (user_id, email) for a live session.
pub async fn lookup(db: &SqlitePool, token: &str) -> sqlx::Result<Option<(String, String)>> {
    let now = now_ms();
    let row: Option<(String, String, String, i64)> = sqlx::query_as(
        "SELECT u.id, u.email, s.id, s.last_used_at FROM sessions s JOIN users u ON u.id = s.user_id
         WHERE s.token_hash = ?1 AND s.expires_at > ?2",
    )
    .bind(hash_token(token))
    .bind(now)
    .fetch_optional(db)
    .await?;
    let Some((user_id, email, session_id, last_used)) = row else {
        return Ok(None);
    };
    if now - last_used > TOUCH_AFTER_MS {
        sqlx::query("UPDATE sessions SET last_used_at = ?1 WHERE id = ?2").bind(now).bind(&session_id).execute(db).await?;
    }
    Ok(Some((user_id, email)))
}

pub async fn revoke(db: &SqlitePool, token: &str) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM sessions WHERE token_hash = ?1").bind(hash_token(token)).execute(db).await?;
    Ok(())
}

/// Failed-login limiter keyed by email: at most `MAX_FAILURES` per window.
pub struct LoginLimiter {
    failures: Mutex<HashMap<String, (u32, Instant)>>,
}

const MAX_FAILURES: u32 = 10;
const WINDOW: Duration = Duration::from_secs(15 * 60);

impl Default for LoginLimiter {
    fn default() -> Self {
        Self { failures: Mutex::new(HashMap::new()) }
    }
}

impl LoginLimiter {
    pub fn check(&self, key: &str) -> Result<(), AppError> {
        let map = self.failures.lock().expect("limiter lock");
        match map.get(key) {
            Some((n, since)) if *n >= MAX_FAILURES && since.elapsed() < WINDOW => Err(AppError::RateLimited),
            _ => Ok(()),
        }
    }

    pub fn fail(&self, key: &str) {
        let mut map = self.failures.lock().expect("limiter lock");
        if map.len() > 10_000 {
            map.retain(|_, (_, since)| since.elapsed() < WINDOW);
        }
        let entry = map.entry(key.to_string()).or_insert((0, Instant::now()));
        if entry.1.elapsed() >= WINDOW {
            *entry = (0, Instant::now());
        }
        entry.0 += 1;
    }

    pub fn reset(&self, key: &str) {
        self.failures.lock().expect("limiter lock").remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hashes_and_verifies() {
        let h = hash_password("correct horse battery".into()).await.unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(verify_password("correct horse battery".into(), Some(h.clone())).await);
        assert!(!verify_password("wrong password!".into(), Some(h)).await);
        assert!(!verify_password("anything at all".into(), None).await);
    }

    #[test]
    fn limiter_blocks_after_max_failures() {
        let l = LoginLimiter::default();
        for _ in 0..MAX_FAILURES {
            assert!(l.check("a@b.co").is_ok());
            l.fail("a@b.co");
        }
        assert!(matches!(l.check("a@b.co"), Err(AppError::RateLimited)));
        assert!(l.check("other@b.co").is_ok());
        l.reset("a@b.co");
        assert!(l.check("a@b.co").is_ok());
    }

    #[test]
    fn tokens_are_unique_and_hash_stably() {
        let (a, b) = (new_token(), new_token());
        assert_ne!(a, b);
        assert!(a.starts_with("pr_") && a.len() > 40);
        assert_eq!(hash_token(&a), hash_token(&a));
    }
}
