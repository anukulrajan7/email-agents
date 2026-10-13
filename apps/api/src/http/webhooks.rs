use axum::{
    Json,
    body::Bytes,
    extract::State,
    http::HeaderMap,
};
use serde_json::{Value, json};
use sha2::Digest;

use super::AppState;
use crate::{
    db::{new_id, now_ms},
    error::{AppError, AppResult},
    jobs::{self, kind},
    sources::{
        nango,
        resend::{WebhookEnvelope, verify_svix},
    },
};

/// Verify → persist event + job in one transaction → 2xx. No provider calls, no AI here.
pub async fn resend(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> AppResult<Json<Value>> {
    let delivery_id = verify_svix(&state.config.resend_webhook_secret, &headers, &body, now_ms() / 1000).map_err(|e| {
        tracing::warn!(reason = %e, "resend webhook signature rejected");
        AppError::Unauthorized
    })?;
    let envelope: WebhookEnvelope =
        serde_json::from_slice(&body).map_err(|_| AppError::BadRequest("malformed webhook payload".into()))?;
    if envelope.event_type != "email.received" {
        return Ok(Json(json!({ "status": "ignored" })));
    }
    let email_id = envelope
        .data
        .get("email_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::BadRequest("email.received without data.email_id".into()))?
        .to_string();

    let event_id = new_id("evt");
    let raw = String::from_utf8(body.to_vec()).map_err(|_| AppError::BadRequest("payload is not UTF-8".into()))?;
    let mut tx = state.db.begin().await?;
    let inserted = sqlx::query(
        "INSERT OR IGNORE INTO inbound_events (id, provider, provider_event_id, payload_json, created_at)
         VALUES (?1, 'resend', ?2, ?3, ?4)",
    )
    .bind(&event_id)
    .bind(&delivery_id)
    .bind(raw)
    .bind(now_ms())
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    // On a redelivery, re-enqueueing is a no-op unless the earlier fetch job died, in which
    // case it is revived. Provider retries therefore recover a dead-lettered fetch.
    let event_id: String = if inserted {
        event_id
    } else {
        sqlx::query_scalar("SELECT id FROM inbound_events WHERE provider = 'resend' AND provider_event_id = ?1")
            .bind(&delivery_id)
            .fetch_one(&mut *tx)
            .await?
    };
    let revived = jobs::enqueue_with_attempts(
        &mut tx,
        kind::FETCH_FORWARDED_EMAIL,
        &format!("fetch:resend:{email_id}"),
        None,
        &json!({ "event_id": event_id, "provider": "resend", "email_id": email_id }),
        jobs::PROVIDER_MAX_ATTEMPTS,
    )
    .await?;
    if revived && !inserted {
        sqlx::query("UPDATE inbound_events SET status = 'received' WHERE id = ?1").bind(&event_id).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    state.jobs_notify.notify_one();
    Ok(Json(json!({ "status": if inserted { "accepted" } else { "duplicate" } })))
}

/// Nango events: `auth` (new connection / refresh failure) and `sync` (new records).
/// Verified, recorded, turned into jobs; no Nango API calls inline.
pub async fn nango(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> AppResult<Json<Value>> {
    if !nango::verify_webhook(&state.config.nango_webhook_secret, &headers, &body) {
        tracing::warn!("nango webhook signature rejected");
        return Err(AppError::Unauthorized);
    }
    let event: Value = serde_json::from_slice(&body).map_err(|_| AppError::BadRequest("malformed webhook payload".into()))?;
    let field = |k: &str| event.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let (event_type, connection_id, pck) = (field("type"), field("connectionId"), field("providerConfigKey"));
    let success = event.get("success").and_then(Value::as_bool).unwrap_or(false);

    // Nango sends no delivery ID header; the body digest identifies a delivery.
    let digest: String = sha2::Sha256::digest(&body).iter().map(|b| format!("{b:02x}")).collect();
    let raw = String::from_utf8(body.to_vec()).map_err(|_| AppError::BadRequest("payload is not UTF-8".into()))?;
    let mut tx = state.db.begin().await?;
    let inserted = sqlx::query(
        "INSERT OR IGNORE INTO inbound_events (id, provider, provider_event_id, payload_json, created_at)
         VALUES (?1, 'nango', ?2, ?3, ?4)",
    )
    .bind(new_id("evt"))
    .bind(&digest)
    .bind(raw)
    .bind(now_ms())
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if !inserted {
        tx.commit().await?;
        return Ok(Json(json!({ "status": "duplicate" })));
    }

    let source: Option<(String, String)> = if connection_id.is_empty() {
        None
    } else {
        sqlx::query_as("SELECT id, workspace_id FROM email_sources WHERE kind = ?1 AND external_connection_id = ?2")
            .bind(nango::GMAIL_KIND)
            .bind(&connection_id)
            .fetch_optional(&mut *tx)
            .await?
    };
    let status = match (event_type.as_str(), field("operation").as_str(), success, source) {
        ("auth", "creation", true, _) => {
            jobs::enqueue_with_attempts(
                &mut tx,
                kind::REGISTER_NANGO_CONNECTION,
                &format!("nango-register:{connection_id}"),
                None,
                &json!({ "connection_id": connection_id, "provider_config_key": pck }),
                jobs::PROVIDER_MAX_ATTEMPTS,
            )
            .await?;
            "accepted"
        }
        ("auth", _, false, Some((source_id, _))) => {
            sqlx::query("UPDATE email_sources SET status = 'degraded', last_error = 'authorization failed; reconnect' WHERE id = ?1 AND status != 'disconnected'")
                .bind(&source_id)
                .execute(&mut *tx)
                .await?;
            "accepted"
        }
        ("sync", _, true, Some((source_id, workspace_id))) => {
            nango::enqueue_sync(&mut tx, &workspace_id, &source_id, "webhook").await?;
            "accepted"
        }
        _ => "ignored",
    };
    tx.commit().await?;
    state.jobs_notify.notify_one();
    Ok(Json(json!({ "status": status })))
}
