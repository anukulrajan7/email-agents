//! Nango-connected mailboxes (Gmail first). Rust talks to Nango over HTTP; the browser
//! only uses Nango Connect for the consent UX.
//!
//! Flow: admin asks for a short-lived connect session (tagged with workspace + user) →
//! Nango Connect runs OAuth → the browser reports the new `connection_id` → we fetch the
//! connection from Nango and check its tags ourselves before storing it → a
//! `sync_nango_source` job pulls records from Nango's `GET /records` (bounded backfill,
//! then cursor-based increments). Nango sync webhooks just enqueue that job.

use std::{collections::HashMap, time::Duration};

use axum::http::HeaderMap;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use crate::{
    audit::{self, Actor, Entry},
    db::{iso, new_id, now_ms, parse_iso_ms},
    domain::{AttachmentMeta, NormalizedEmail},
    error::AppError,
    http::AppState,
    ingest,
    jobs::{self, handlers::JobError, kind},
    sources::{self, normalize},
};

pub const GMAIL_KIND: &str = "nango_gmail";
const PAGE_LIMIT: usize = 100;
const MAX_PAGES_PER_RUN: usize = 50;
const PERSIST_CHUNK: usize = 50;

#[derive(Debug, thiserror::Error)]
pub enum NangoError {
    #[error("Nango is not configured")]
    NotConfigured,
    #[error("Nango rejected the request: {0}")]
    Rejected(String),
    #[error("connection unusable: {0}")]
    Connection(String),
    #[error("Nango unavailable: {0}")]
    Unavailable(String),
}

impl From<NangoError> for JobError {
    fn from(e: NangoError) -> Self {
        match e {
            NangoError::Unavailable(m) => JobError::Retry(m),
            other => JobError::Permanent(other.to_string()),
        }
    }
}

impl From<NangoError> for AppError {
    fn from(e: NangoError) -> Self {
        match e {
            NangoError::NotConfigured => AppError::NotImplemented("Nango is not configured (set NANGO_API_KEY)".into()),
            NangoError::Unavailable(_) => AppError::Unavailable,
            NangoError::Rejected(m) | NangoError::Connection(m) => AppError::BadRequest(m),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ConnectSession {
    pub token: String,
    pub expires_at: String,
}

#[derive(Debug, Deserialize)]
pub struct Connection {
    pub connection_id: String,
    pub provider_config_key: String,
    #[serde(default)]
    pub tags: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct RecordsPage {
    #[serde(default)]
    pub records: Vec<Value>,
    #[serde(default)]
    pub next_cursor: Option<String>,
}

pub struct NangoClient {
    http: reqwest::Client,
    api_url: String,
    secret_key: String,
}

impl NangoClient {
    pub fn new(http: reqwest::Client, api_url: &str, secret_key: &str) -> Self {
        Self { http, api_url: api_url.trim_end_matches('/').to_string(), secret_key: secret_key.to_string() }
    }

    pub fn configured(&self) -> bool {
        !self.secret_key.is_empty()
    }

    async fn send<T: serde::de::DeserializeOwned>(&self, req: reqwest::RequestBuilder) -> Result<T, NangoError> {
        if !self.configured() {
            return Err(NangoError::NotConfigured);
        }
        let res = req
            .bearer_auth(&self.secret_key)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| NangoError::Unavailable(e.to_string()))?;
        let status = res.status();
        if status.is_success() {
            return res.json().await.map_err(|e| NangoError::Unavailable(format!("decode: {e}")));
        }
        let msg = format!("Nango returned {status}");
        Err(match status.as_u16() {
            429 | 500..=599 => NangoError::Unavailable(msg),
            // 424 = connection refresh exhausted: the user must reconnect.
            401 | 403 | 404 | 424 => NangoError::Connection(msg),
            _ => NangoError::Rejected(msg),
        })
    }

    /// Short-lived (≈30 min) session restricted to one integration and tagged with
    /// the workspace and user, so the resulting connection can be verified later.
    pub async fn create_connect_session(
        &self,
        workspace_id: &str,
        user_id: &str,
        user_email: &str,
        integration: &str,
    ) -> Result<ConnectSession, NangoError> {
        #[derive(Deserialize)]
        struct Envelope {
            data: ConnectSession,
        }
        let mut tags = json!({ "end_user_id": user_id, "organization_id": workspace_id });
        if !user_email.is_empty() {
            tags["end_user_email"] = json!(user_email);
        }
        let body = json!({ "tags": tags, "allowed_integrations": [integration] });
        let env: Envelope = self.send(self.http.post(format!("{}/connect/sessions", self.api_url)).json(&body)).await?;
        Ok(env.data)
    }

    pub async fn get_connection(&self, connection_id: &str, provider_config_key: &str) -> Result<Connection, NangoError> {
        self.send(
            self.http
                .get(format!("{}/connections/{}", self.api_url, urlencode(connection_id)))
                .query(&[("provider_config_key", provider_config_key)]),
        )
        .await
    }

    pub async fn delete_connection(&self, connection_id: &str, provider_config_key: &str) -> Result<(), NangoError> {
        let _: Value = self
            .send(
                self.http
                    .delete(format!("{}/connections/{}", self.api_url, urlencode(connection_id)))
                    .query(&[("provider_config_key", provider_config_key)]),
            )
            .await
            .or_else(|e| match e {
                // Some responses have an empty body; treat decode failure after 2xx as success.
                NangoError::Unavailable(m) if m.starts_with("decode") => Ok(Value::Null),
                other => Err(other),
            })?;
        Ok(())
    }

    pub async fn list_records(
        &self,
        connection_id: &str,
        provider_config_key: &str,
        model: &str,
        cursor: Option<&str>,
        modified_after: Option<&str>,
    ) -> Result<RecordsPage, NangoError> {
        let limit = PAGE_LIMIT.to_string();
        let mut query: Vec<(&str, &str)> = vec![("model", model), ("limit", &limit)];
        if let Some(c) = cursor {
            query.push(("cursor", c));
        } else if let Some(m) = modified_after {
            query.push(("modified_after", m));
        }
        self.send(
            self.http
                .get(format!("{}/records", self.api_url))
                .header("Connection-Id", connection_id)
                .header("Provider-Config-Key", provider_config_key)
                .query(&query),
        )
        .await
    }
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// `X-Nango-Hmac-Sha256` = hex(HMAC-SHA256(signing key, raw body)). `secrets` may hold
/// several comma-separated keys to cover Nango's key-rotation window.
pub fn verify_webhook(secrets: &str, headers: &HeaderMap, body: &[u8]) -> bool {
    let Some(given) = headers.get("x-nango-hmac-sha256").and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let Some(given) = decode_hex(given.trim()) else {
        return false;
    };
    secrets.split(',').map(str::trim).filter(|s| !s.is_empty()).any(|secret| {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
        mac.update(body);
        mac.finalize().into_bytes().ct_eq(&given).into()
    })
}

pub fn sign_webhook(secret: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(body);
    mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

// ---------------------------------------------------------------- record mapping

pub struct MappedRecord {
    pub email: NormalizedEmail,
    pub thread_id: Option<String>,
    pub deleted: bool,
    pub cursor: Option<String>,
}

fn str_of(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| match v.get(*k) {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    })
}

fn addresses(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::String(s)) => s.split(',').map(normalize::address).filter(|a| !a.is_empty()).collect(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|i| i.as_str().map(str::to_string).or_else(|| str_of(i, &["email", "address"])))
            .map(|a| normalize::address(&a))
            .collect(),
        _ => vec![],
    }
}

/// Maps a Nango Gmail record. Field names differ between template versions, so this
/// accepts the common spellings; anything without an ID is skipped.
pub fn map_gmail_record(rec: &Value, max_body_chars: usize) -> Option<MappedRecord> {
    let id = str_of(rec, &["id"])?;
    let meta = rec.get("_nango_metadata");
    let deleted = meta.is_some_and(|m| {
        m.get("deleted_at").is_some_and(|d| !d.is_null()) || m.get("last_action").and_then(Value::as_str) == Some("DELETED")
    });
    let cursor = meta.and_then(|m| m.get("cursor")).and_then(Value::as_str).map(str::to_string);

    let raw_body = str_of(rec, &["body", "text", "bodyText", "snippet"]).unwrap_or_default();
    let looks_html = raw_body.contains("</") || raw_body.contains("<br");
    let body_text = if looks_html { normalize::html_to_text(&raw_body) } else { raw_body.clone() };
    let attachments = rec
        .get("attachments")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|a| AttachmentMeta {
                    filename: normalize::truncate_chars(&str_of(a, &["filename", "name"]).unwrap_or_else(|| "attachment".into()), 255),
                    media_type: str_of(a, &["mimeType", "mime_type", "contentType", "content_type"]),
                    size_bytes: a.get("size").and_then(Value::as_i64),
                })
                .collect()
        })
        .unwrap_or_default();

    let email = NormalizedEmail {
        source_message_id: id,
        rfc_message_id: str_of(rec, &["messageId", "message_id", "rfcMessageId"]),
        sender_address: normalize::address(&str_of(rec, &["sender", "from"]).unwrap_or_default()),
        // A connected mailbox sees the real sender; there is no forwarded wrapper to parse.
        original_sender_address: None,
        recipients: addresses(rec.get("recipients").or_else(|| rec.get("to"))),
        subject: normalize::truncate_chars(
            str_of(rec, &["subject"]).as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("(no subject)"),
            998,
        ),
        body_text: normalize::truncate_chars(&body_text, max_body_chars),
        body_html: looks_html.then(|| normalize::truncate_chars(&raw_body, max_body_chars)),
        received_at: str_of(rec, &["date", "receivedAt", "received_at", "internalDate"])
            .and_then(|d| parse_iso_ms(&d).or_else(|| d.parse::<i64>().ok()))
            .unwrap_or_else(now_ms),
        attachments,
    };
    Some(MappedRecord { email, thread_id: str_of(rec, &["threadId", "thread_id"]), deleted, cursor })
}

// ---------------------------------------------------------------- connection registration

/// Verifies a connection against Nango (never trusting browser-supplied IDs), then
/// stores it as a source and schedules the initial backfill. `expected_*` are set when a
/// signed-in admin reports the connection; the webhook path relies on tags alone.
pub async fn register_connection(
    state: &AppState,
    expected_workspace: Option<&str>,
    expected_user: Option<&str>,
    connection_id: &str,
    provider_config_key: &str,
) -> Result<String, AppError> {
    if provider_config_key != state.config.nango_gmail_integration {
        return Err(AppError::BadRequest("unsupported integration".into()));
    }
    let conn = state.nango.get_connection(connection_id, provider_config_key).await?;
    let workspace_id = conn.tags.get("organization_id").cloned().ok_or(AppError::Forbidden)?;
    let owner = conn.tags.get("end_user_id").cloned().ok_or(AppError::Forbidden)?;
    if expected_workspace.is_some_and(|w| w != workspace_id) || expected_user.is_some_and(|u| u != owner) {
        return Err(AppError::Forbidden);
    }
    if conn.connection_id != connection_id || conn.provider_config_key != provider_config_key {
        return Err(AppError::Forbidden);
    }
    // The person who consented must (still) be an admin of the tagged workspace.
    let role: Option<String> = sqlx::query_scalar("SELECT role FROM memberships WHERE workspace_id = ?1 AND user_id = ?2")
        .bind(&workspace_id)
        .bind(&owner)
        .fetch_optional(&state.db)
        .await?;
    if role.as_deref() != Some("admin") {
        return Err(AppError::Forbidden);
    }

    let mut tx = state.db.begin().await?;
    sqlx::query(
        "INSERT INTO email_sources
           (id, workspace_id, kind, status, external_connection_id, provider_config_key, capabilities_json,
            connected_by_user_id, created_at)
         VALUES (?1, ?2, ?3, 'connected', ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(kind, external_connection_id) DO UPDATE SET status = 'connected', last_error = NULL
         WHERE email_sources.workspace_id = excluded.workspace_id",
    )
    .bind(new_id("src"))
    .bind(&workspace_id)
    .bind(GMAIL_KIND)
    .bind(connection_id)
    .bind(provider_config_key)
    .bind(serde_json::to_string(&sources::capabilities(GMAIL_KIND)).expect("serializable"))
    .bind(&owner)
    .bind(now_ms())
    .execute(&mut *tx)
    .await?;
    let (source_id, owner_ws): (String, String) =
        sqlx::query_as("SELECT id, workspace_id FROM email_sources WHERE kind = ?1 AND external_connection_id = ?2")
            .bind(GMAIL_KIND)
            .bind(connection_id)
            .fetch_one(&mut *tx)
            .await?;
    if owner_ws != workspace_id {
        return Err(AppError::Forbidden);
    }
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &workspace_id,
            actor: Actor::User(&owner),
            action: "source.connected",
            entity_type: "email_source",
            entity_id: &source_id,
            message_id: None,
            metadata: json!({ "kind": GMAIL_KIND }),
        },
    )
    .await?;
    enqueue_sync(&mut tx, &workspace_id, &source_id, "connected").await?;
    tx.commit().await?;
    state.jobs_notify.notify_one();
    Ok(source_id)
}

pub async fn enqueue_sync(
    conn: &mut sqlx::SqliteConnection,
    workspace_id: &str,
    source_id: &str,
    reason: &str,
) -> sqlx::Result<bool> {
    // One pending sync per source per reason-bucket; a running sync picks up everything anyway.
    let bucket = now_ms() / 60_000;
    jobs::enqueue_with_attempts(
        conn,
        kind::SYNC_NANGO_SOURCE,
        &format!("sync:{source_id}:{reason}:{bucket}"),
        Some(workspace_id),
        &json!({ "source_id": source_id }),
        jobs::PROVIDER_MAX_ATTEMPTS,
    )
    .await
}

// ---------------------------------------------------------------- sync job

#[derive(sqlx::FromRow)]
struct SourceForSync {
    workspace_id: String,
    status: String,
    external_connection_id: Option<String>,
    provider_config_key: Option<String>,
    sync_cursor: Option<String>,
}

async fn mark_source(state: &AppState, source_id: &str, status: &str, error: Option<&str>) -> sqlx::Result<()> {
    sqlx::query("UPDATE email_sources SET status = ?1, last_error = ?2 WHERE id = ?3 AND status != 'disconnected'")
        .bind(status)
        .bind(error)
        .bind(source_id)
        .execute(&state.db)
        .await?;
    Ok(())
}

pub async fn sync_source(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let source_id = payload
        .get("source_id")
        .and_then(Value::as_str)
        .ok_or_else(|| JobError::Permanent("payload missing source_id".into()))?
        .to_string();
    let src: SourceForSync = sqlx::query_as(
        "SELECT workspace_id, status, external_connection_id, provider_config_key, sync_cursor
         FROM email_sources WHERE id = ?1 AND kind = ?2",
    )
    .bind(&source_id)
    .bind(GMAIL_KIND)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| JobError::Permanent("source not found".into()))?;
    if src.status == "disconnected" {
        return Ok(());
    }
    let (Some(connection_id), Some(pck)) = (src.external_connection_id, src.provider_config_key) else {
        return Err(JobError::Permanent("source has no Nango connection".into()));
    };

    // Network first, outside any transaction: page through everything new.
    let initial = src.sync_cursor.is_none();
    let cutoff = now_ms() - state.config.nango_backfill_days * 86_400_000;
    let modified_after = iso(cutoff);
    let mut records: Vec<MappedRecord> = Vec::new();
    let mut page_cursor = src.sync_cursor.clone();
    let mut more_pages = false;
    for page_no in 0.. {
        if page_no == MAX_PAGES_PER_RUN {
            more_pages = true;
            break;
        }
        let page = match state
            .nango
            .list_records(&connection_id, &pck, &state.config.nango_gmail_model, page_cursor.as_deref(), Some(&modified_after))
            .await
        {
            Ok(p) => p,
            Err(e @ NangoError::Connection(_)) => {
                mark_source(state, &source_id, "degraded", Some(&e.to_string())).await?;
                return Err(e.into());
            }
            Err(e) => return Err(e.into()),
        };
        records.extend(page.records.iter().filter_map(|r| map_gmail_record(r, state.config.max_body_chars)));
        match page.next_cursor {
            Some(next) if !page.records.is_empty() => page_cursor = Some(next),
            _ => break,
        }
    }
    // Resume point = cursor of the last record seen (records arrive oldest-modified first).
    let new_cursor = records.iter().rev().find_map(|r| r.cursor.clone()).or(src.sync_cursor);

    // Deleted upstream: keep local history (reviews/audit reference it), just don't import.
    let mut candidates: Vec<MappedRecord> = records.into_iter().filter(|r| !r.deleted).collect();
    if initial {
        // Bounded backfill: newest N messages from the last D days.
        candidates.retain(|r| r.email.received_at >= cutoff);
        candidates.sort_by_key(|r| std::cmp::Reverse(r.email.received_at));
        candidates.truncate(state.config.nango_backfill_max);
    }

    let mut imported = 0usize;
    let chunks: Vec<&[MappedRecord]> = candidates.chunks(PERSIST_CHUNK).collect();
    let total_chunks = chunks.len().max(1);
    for i in 0..total_chunks {
        let chunk = chunks.get(i).copied().unwrap_or(&[]);
        let last = i + 1 == total_chunks;
        let mut tx = state.db.begin().await?;
        // Write first (takes the lock); the cursor only advances with the final chunk, so a
        // crash mid-way re-imports from the old cursor and dedupe absorbs the repeats.
        sqlx::query(
            "UPDATE email_sources SET last_synced_at = ?1, sync_cursor = CASE WHEN ?2 THEN ?3 ELSE sync_cursor END
             WHERE id = ?4",
        )
        .bind(now_ms())
        .bind(last)
        .bind(&new_cursor)
        .bind(&source_id)
        .execute(&mut *tx)
        .await?;
        for r in chunk {
            let created = ingest::persist_message(
                &mut tx,
                &source_id,
                &src.workspace_id,
                &r.email,
                r.thread_id.as_deref(),
                json!({ "provider": GMAIL_KIND }),
            )
            .await?;
            imported += usize::from(created.is_some());
        }
        tx.commit().await?;
    }
    mark_source(state, &source_id, "connected", None).await?;
    if more_pages {
        let mut conn = state.db.acquire().await?;
        enqueue_sync(&mut conn, &src.workspace_id, &source_id, "continue").await?;
    }
    tracing::info!(%source_id, imported, initial, "nango sync finished");
    state.jobs_notify.notify_one();
    Ok(())
}

/// Background registration from an `auth` webhook (no signed-in user available).
pub async fn register_from_webhook(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let connection_id = payload.get("connection_id").and_then(Value::as_str).unwrap_or_default();
    let pck = payload.get("provider_config_key").and_then(Value::as_str).unwrap_or_default();
    match register_connection(state, None, None, connection_id, pck).await {
        Ok(_) => Ok(()),
        Err(AppError::Unavailable | AppError::Internal(_)) => Err(JobError::Retry("registration failed".into())),
        Err(e) => {
            tracing::warn!(error = %e, "nango connection from webhook rejected");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn webhook_signature_with_rotation() {
        let body = br#"{"type":"sync"}"#;
        let mut h = HeaderMap::new();
        h.insert("x-nango-hmac-sha256", HeaderValue::from_str(&sign_webhook("new", body)).unwrap());
        assert!(verify_webhook("old, new", &h, body));
        assert!(!verify_webhook("old", &h, body));
        assert!(!verify_webhook("new", &h, b"{}"));
        assert!(!verify_webhook("new", &HeaderMap::new(), body));
    }

    #[test]
    fn maps_gmail_records_tolerantly() {
        let rec = json!({
            "id": "18c2", "threadId": "t1", "sender": "Priya <Priya@Acme.com>",
            "recipients": "hr@acme.com, boss@acme.com", "date": "2026-10-10T09:00:00Z",
            "subject": "Leave next week", "body": "<p>Hi</p><p>Can I take Friday off?</p>",
            "attachments": [{"filename":"note.pdf","mimeType":"application/pdf","size":12}],
            "_nango_metadata": {"deleted_at": null, "last_action": "ADDED", "cursor": "c1"}
        });
        let m = map_gmail_record(&rec, 10_000).unwrap();
        assert_eq!(m.email.sender_address, "priya@acme.com");
        assert_eq!(m.email.recipients, vec!["hr@acme.com", "boss@acme.com"]);
        assert_eq!(m.email.body_text, "Hi\n\nCan I take Friday off?");
        assert_eq!(m.thread_id.as_deref(), Some("t1"));
        assert_eq!((m.deleted, m.cursor.as_deref()), (false, Some("c1")));
        assert_eq!(m.email.attachments[0].media_type.as_deref(), Some("application/pdf"));

        let gone = json!({"id": 7, "_nango_metadata": {"deleted_at": "2026-10-11T00:00:00Z"}});
        let g = map_gmail_record(&gone, 100).unwrap();
        assert!(g.deleted);
        assert_eq!(g.email.source_message_id, "7");
        assert!(map_gmail_record(&json!({"subject":"no id"}), 100).is_none());
    }
}
