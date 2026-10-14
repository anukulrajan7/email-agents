//! Cross-cutting protections: security headers on every response, the client IP behind the
//! local reverse proxy, a per-IP limit on the auth endpoints, and persistent sliding-window
//! limits for actions that send email.

use std::{
    convert::Infallible,
    net::{IpAddr, Ipv4Addr, SocketAddr},
};

use axum::{
    extract::{ConnectInfo, FromRequestParts, Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};
use sqlx::SqlitePool;

use crate::{db::now_ms, error::AppError, http::AppState};

/// The email body renders in a `srcdoc` iframe, which inherits this policy (plus its own
/// stricter one): remote images over https, inline styles. The Gmail connect flow is Nango's
/// hosted iframe.
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; img-src 'self' https: data:; style-src 'self' 'unsafe-inline'; \
font-src 'self' data: https:; frame-src 'self' blob: about: https://connect.nango.dev; frame-ancestors 'none'; \
base-uri 'self'; form-action 'self'; object-src 'none'";

const HEADERS: &[(&str, &str)] = &[
    ("content-security-policy", CONTENT_SECURITY_POLICY),
    ("strict-transport-security", "max-age=31536000; includeSubDomains"),
    ("x-content-type-options", "nosniff"),
    ("referrer-policy", "strict-origin-when-cross-origin"),
    ("x-frame-options", "DENY"),
    // Microphone only for our own origin: Ask Meera voice input. Everything else stays off.
    ("permissions-policy", "camera=(), microphone=(self), geolocation=(), payment=(), usb=(), interest-cohort=()"),
];

pub async fn security_headers(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let headers = res.headers_mut();
    for (name, value) in HEADERS {
        headers.insert(HeaderName::from_static(name), HeaderValue::from_static(value));
    }
    res
}

/// The caller's IP. `X-Forwarded-For` is honoured only when the TCP peer is loopback (Caddy
/// on the same host); its right-most entry is the address Caddy itself saw. Without connect
/// info (in-process calls, tests) the peer counts as loopback.
pub fn client_ip(headers: &HeaderMap, peer: Option<IpAddr>) -> IpAddr {
    let peer = peer.unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
    if !peer.is_loopback() {
        return peer;
    }
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit(',').next())
        .and_then(|ip| ip.trim().parse().ok())
        .unwrap_or(peer)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientIp(pub IpAddr);

impl<S: Send + Sync> FromRequestParts<S> for ClientIp {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let peer = parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|c| c.0.ip());
        Ok(ClientIp(client_ip(&parts.headers, peer)))
    }
}

/// Per-IP token bucket in front of the auth endpoints (sign-up, login, codes, resets).
pub async fn auth_rate_limit(State(state): State<AppState>, ClientIp(ip): ClientIp, req: Request, next: Next) -> Response {
    if !state.login_limiter.allow_ip(ip) {
        tracing::info!(%ip, "auth rate limit hit");
        return AppError::RateLimited.into_response();
    }
    next.run(req).await
}

/// Records one `scope`/`key` event if fewer than `limit` happened in the last `window_ms`.
/// `false` = over the limit (nothing recorded). Survives restarts.
pub async fn take(db: &SqlitePool, scope: &str, key: &str, limit: i64, window_ms: i64) -> sqlx::Result<bool> {
    let now = now_ms();
    let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
    // A write first, so SQLite takes the write lock before the count below.
    sqlx::query("DELETE FROM rate_events WHERE scope = ?1 AND key = ?2 AND created_at <= ?3")
        .bind(scope)
        .bind(key)
        .bind(now - window_ms)
        .execute(&mut *tx)
        .await?;
    let used: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rate_events WHERE scope = ?1 AND key = ?2")
        .bind(scope)
        .bind(key)
        .fetch_one(&mut *tx)
        .await?;
    if used >= limit {
        tx.commit().await?;
        return Ok(false);
    }
    sqlx::query("INSERT INTO rate_events (scope, key, created_at) VALUES (?1, ?2, ?3)")
        .bind(scope)
        .bind(key)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

/// Drops events older than any window in use (two days).
pub async fn purge_rate_events(db: &SqlitePool) -> sqlx::Result<u64> {
    Ok(sqlx::query("DELETE FROM rate_events WHERE created_at < ?1").bind(now_ms() - 2 * 86_400_000).execute(db).await?.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xff(v: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", HeaderValue::from_str(v).unwrap());
        h
    }

    #[test]
    fn forwarded_for_is_trusted_only_from_loopback() {
        let loopback: IpAddr = "127.0.0.1".parse().unwrap();
        let remote: IpAddr = "198.51.100.4".parse().unwrap();
        assert_eq!(client_ip(&xff("1.2.3.4, 203.0.113.5"), Some(loopback)), "203.0.113.5".parse::<IpAddr>().unwrap());
        assert_eq!(client_ip(&xff("203.0.113.5"), Some(remote)), remote, "spoofed header from a remote peer is ignored");
        assert_eq!(client_ip(&xff("not-an-ip"), Some(loopback)), loopback);
        assert_eq!(client_ip(&HeaderMap::new(), None), loopback);
    }
}
