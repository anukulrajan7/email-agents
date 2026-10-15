//! Email + password accounts: Argon2id hashing (off the async runtime, at most 4 at once),
//! opaque session tokens stored only as SHA-256 hashes, and small in-memory rate limiters.

use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{LazyLock, Mutex, OnceLock},
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
use tokio::sync::Semaphore;

use crate::{
    platform::db::{new_id, now_ms},
    platform::error::AppError,
};

/// Sliding: every use (at most every `TOUCH_AFTER_MS`) pushes the expiry this far out again.
pub const SESSION_TTL_MS: i64 = 7 * 86_400_000;
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

/// Argon2 is deliberately expensive: bound how many run at once so a login flood can't
/// starve the blocking pool or the CPU.
static ARGON2_PERMITS: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(4));

pub async fn hash_password(password: String) -> Result<String, AppError> {
    let _permit = ARGON2_PERMITS.acquire().await.map_err(|e| AppError::Internal(e.into()))?;
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
    let Ok(_permit) = ARGON2_PERMITS.acquire().await else { return false };
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
        sqlx::query("UPDATE sessions SET last_used_at = ?1, expires_at = ?2 WHERE id = ?3")
            .bind(now)
            .bind(now + SESSION_TTL_MS)
            .bind(&session_id)
            .execute(db)
            .await?;
    }
    Ok(Some((user_id, email)))
}

pub async fn revoke(db: &SqlitePool, token: &str) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM sessions WHERE token_hash = ?1").bind(hash_token(token)).execute(db).await?;
    Ok(())
}

/// Every session of this user (password change, reset, first verification).
pub async fn revoke_all(db: &SqlitePool, user_id: &str) -> sqlx::Result<u64> {
    Ok(sqlx::query("DELETE FROM sessions WHERE user_id = ?1").bind(user_id).execute(db).await?.rows_affected())
}

/// In-memory limits for the auth endpoints:
/// - failures per key (an email + IP pair): at most `MAX_FAILURES` per window, then refused;
/// - failures per email across all IPs: past `EMAIL_SOFT_LIMIT`, each attempt is slowed down
///   (never refused, so nobody can lock a victim out of their own account);
/// - requests per client IP: a token bucket (`ip_burst`, refilled at `ip_per_minute`).
pub struct LoginLimiter {
    failures: Mutex<HashMap<String, (u32, Instant)>>,
    email_failures: Mutex<HashMap<String, (u32, Instant)>>,
    ip_buckets: Mutex<HashMap<IpAddr, (f64, Instant)>>,
    ip_burst: f64,
    ip_per_minute: f64,
}

const MAX_FAILURES: u32 = 10;
const WINDOW: Duration = Duration::from_secs(15 * 60);
pub const EMAIL_SOFT_LIMIT: u32 = 20;
const MAX_SLOWDOWN: Duration = Duration::from_secs(5);
const MAX_TRACKED: usize = 10_000;

impl Default for LoginLimiter {
    fn default() -> Self {
        Self::new(20, 10)
    }
}

fn pair_key(email: &str, ip: IpAddr) -> String {
    format!("{email}|{ip}")
}

impl LoginLimiter {
    pub fn new(ip_burst: u32, ip_per_minute: u32) -> Self {
        Self {
            failures: Mutex::new(HashMap::new()),
            email_failures: Mutex::new(HashMap::new()),
            ip_buckets: Mutex::new(HashMap::new()),
            ip_burst: f64::from(ip_burst.max(1)),
            ip_per_minute: f64::from(ip_per_minute.max(1)),
        }
    }

    /// Takes one token from this IP's bucket; `false` = over the limit.
    pub fn allow_ip(&self, ip: IpAddr) -> bool {
        let mut map = self.ip_buckets.lock().expect("limiter lock");
        if map.len() > MAX_TRACKED {
            let full_after = Duration::from_secs_f64(self.ip_burst / self.ip_per_minute * 60.0);
            map.retain(|_, (_, at)| at.elapsed() < full_after);
        }
        let (tokens, at) = map.entry(ip).or_insert((self.ip_burst, Instant::now()));
        let refill = at.elapsed().as_secs_f64() * self.ip_per_minute / 60.0;
        *tokens = (*tokens + refill).min(self.ip_burst);
        *at = Instant::now();
        if *tokens >= 1.0 {
            *tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Before checking a password: refused when this email + IP failed too often; otherwise
    /// the delay to wait first (zero unless the email is under a distributed attack).
    pub fn check_login(&self, email: &str, ip: IpAddr) -> Result<Duration, AppError> {
        self.check(&pair_key(email, ip))?;
        let map = self.email_failures.lock().expect("limiter lock");
        Ok(match map.get(email) {
            Some((n, since)) if *n >= EMAIL_SOFT_LIMIT && since.elapsed() < WINDOW => {
                let steps = (*n - EMAIL_SOFT_LIMIT).min(5);
                (Duration::from_millis(250) * 2u32.pow(steps)).min(MAX_SLOWDOWN)
            }
            _ => Duration::ZERO,
        })
    }

    pub fn fail_login(&self, email: &str, ip: IpAddr) {
        self.fail(&pair_key(email, ip));
        let mut map = self.email_failures.lock().expect("limiter lock");
        if map.len() > MAX_TRACKED {
            map.retain(|_, (_, since)| since.elapsed() < WINDOW);
        }
        let entry = map.entry(email.to_string()).or_insert((0, Instant::now()));
        if entry.1.elapsed() >= WINDOW {
            *entry = (0, Instant::now());
        }
        entry.0 += 1;
    }

    pub fn reset_login(&self, email: &str, ip: IpAddr) {
        self.reset(&pair_key(email, ip));
        self.email_failures.lock().expect("limiter lock").remove(email);
    }

    pub fn check(&self, key: &str) -> Result<(), AppError> {
        let map = self.failures.lock().expect("limiter lock");
        match map.get(key) {
            Some((n, since)) if *n >= MAX_FAILURES && since.elapsed() < WINDOW => Err(AppError::RateLimited),
            _ => Ok(()),
        }
    }

    pub fn fail(&self, key: &str) {
        let mut map = self.failures.lock().expect("limiter lock");
        if map.len() > MAX_TRACKED {
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
    fn lockout_is_per_email_and_ip_with_a_soft_global_slowdown() {
        let l = LoginLimiter::default();
        let (attacker, victim): (IpAddr, IpAddr) = ("203.0.113.9".parse().unwrap(), "198.51.100.7".parse().unwrap());
        for _ in 0..MAX_FAILURES {
            l.fail_login("v@b.co", attacker);
        }
        assert!(matches!(l.check_login("v@b.co", attacker), Err(AppError::RateLimited)));
        assert_eq!(l.check_login("v@b.co", victim).unwrap(), Duration::ZERO, "the victim is not locked out");
        // A distributed attack (many IPs) only slows attempts on that email down, within a bound.
        for i in 0..40u8 {
            l.fail_login("v@b.co", IpAddr::from([10, 0, 0, i]));
        }
        let delay = l.check_login("v@b.co", victim).unwrap();
        assert!(delay > Duration::ZERO && delay <= MAX_SLOWDOWN, "{delay:?}");
        l.reset_login("v@b.co", victim);
        assert_eq!(l.check_login("v@b.co", victim).unwrap(), Duration::ZERO);
    }

    #[test]
    fn ip_bucket_allows_a_burst_then_refuses() {
        let l = LoginLimiter::new(3, 10);
        let ip: IpAddr = "203.0.113.1".parse().unwrap();
        assert!((0..3).all(|_| l.allow_ip(ip)));
        assert!(!l.allow_ip(ip));
        assert!(l.allow_ip("203.0.113.2".parse().unwrap()), "buckets are per IP");
    }

    #[test]
    fn tokens_are_unique_and_hash_stably() {
        let (a, b) = (new_token(), new_token());
        assert_ne!(a, b);
        assert!(a.starts_with("pr_") && a.len() > 40);
        assert_eq!(hash_token(&a), hash_token(&a));
    }
}
