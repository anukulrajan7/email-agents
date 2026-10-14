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

    /// Full Gmail message via Nango's proxy (the catalog sync only stores metadata).
    /// Retries once: right after a connection is created the proxy can briefly 403.
    pub async fn gmail_message_full(&self, connection_id: &str, provider_config_key: &str, id: &str) -> Result<Value, NangoError> {
        if !id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(NangoError::Rejected("invalid message id".into()));
        }
        let call = || {
            self.send::<Value>(
                self.http
                    .get(format!("{}/proxy/gmail/v1/users/me/messages/{id}", self.api_url))
                    .header("Connection-Id", connection_id)
                    .header("Provider-Config-Key", provider_config_key)
                    .query(&[("format", "full")]),
            )
        };
        match call().await {
            Err(NangoError::Connection(_) | NangoError::Unavailable(_)) => {
                tokio::time::sleep(Duration::from_millis(800)).await;
                call().await
            }
            other => other,
        }
    }

    /// Generic Gmail call through Nango's proxy (`path` is relative to `users/me/`).
    pub async fn gmail_call(
        &self,
        method: reqwest::Method,
        connection_id: &str,
        provider_config_key: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, NangoError> {
        let mut req = self
            .http
            .request(method, format!("{}/proxy/gmail/v1/users/me/{path}", self.api_url))
            .header("Connection-Id", connection_id)
            .header("Provider-Config-Key", provider_config_key);
        if let Some(b) = body {
            req = req.json(b);
        }
        self.send(req).await
    }

    /// One page of Gmail message IDs matching `q` (e.g. `newer_than:14d`).
    pub async fn gmail_list_ids(
        &self,
        connection_id: &str,
        provider_config_key: &str,
        q: &str,
        page_token: Option<&str>,
    ) -> Result<(Vec<String>, Option<String>), NangoError> {
        let mut query: Vec<(&str, &str)> = vec![("q", q), ("maxResults", "100")];
        if let Some(t) = page_token {
            query.push(("pageToken", t));
        }
        let page: Value = self
            .send(
                self.http
                    .get(format!("{}/proxy/gmail/v1/users/me/messages", self.api_url))
                    .header("Connection-Id", connection_id)
                    .header("Provider-Config-Key", provider_config_key)
                    .query(&query),
            )
            .await?;
        let ids = page
            .get("messages")
            .and_then(Value::as_array)
            .map(|m| m.iter().filter_map(|x| x.get("id").and_then(Value::as_str).map(str::to_string)).collect())
            .unwrap_or_default();
        Ok((ids, page.get("nextPageToken").and_then(Value::as_str).map(str::to_string)))
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

    // Nango's catalog `messages` sync stores Gmail-native records: headers live in
    // payload.headers and there is no body (only `snippet`) unless enriched later.
    let header = |name: &str| gmail_header(rec.get("payload"), name);
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
                    provider_ref: None,
                })
                .collect()
        })
        .unwrap_or_default();

    let email = NormalizedEmail {
        source_message_id: id,
        rfc_message_id: str_of(rec, &["messageId", "message_id", "rfcMessageId"]).or_else(|| header("Message-ID")),
        sender_address: normalize::address(&str_of(rec, &["sender", "from"]).or_else(|| header("From")).unwrap_or_default()),
        sender_name: str_of(rec, &["sender", "from"]).or_else(|| header("From")).as_deref().and_then(normalize::display_name),
        // A connected mailbox sees the real sender; there is no forwarded wrapper to parse.
        original_sender_address: None,
        recipients: match rec.get("recipients").or_else(|| rec.get("to")) {
            Some(v) => addresses(Some(v)),
            None => [header("To"), header("Cc")]
                .into_iter()
                .flatten()
                .flat_map(|h| addresses(Some(&Value::String(h))))
                .collect(),
        },
        cc: match rec.get("cc") {
            Some(v) => addresses(Some(v)),
            None => header("Cc").map(|h| addresses(Some(&Value::String(h)))).unwrap_or_default(),
        },
        outbound: has_sent_label(rec),
        subject: normalize::truncate_chars(
            str_of(rec, &["subject"])
                .or_else(|| header("Subject"))
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("(no subject)"),
            998,
        ),
        body_text: normalize::truncate_chars(&body_text, max_body_chars),
        body_html: looks_html.then(|| normalize::truncate_chars(&raw_body, max_body_chars)),
        received_at: str_of(rec, &["date", "receivedAt", "received_at", "internalDate"])
            .and_then(|d| parse_iso_ms(&d).or_else(|| d.parse::<i64>().ok()))
            .unwrap_or_else(now_ms),
        attachments,
        // Only a snippet unless an explicit body field was present.
        preview_only: str_of(rec, &["body", "text", "bodyText"]).is_none(),
        bulk_signals: crate::bulk::header_signals(&header, &label_ids(rec)),
    };
    let mut mapped = MappedRecord { email, thread_id: str_of(rec, &["threadId", "thread_id"]), deleted, cursor };
    // If the record already carries MIME parts (full format), use them directly.
    if let Some(payload) = rec.get("payload") {
        apply_gmail_payload(&mut mapped.email, payload, max_body_chars);
    }
    Some(mapped)
}

fn label_ids(rec: &Value) -> Vec<String> {
    rec.get("labelIds").and_then(Value::as_array).map(|l| l.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

/// Gmail marks mail the mailbox itself sent with the SENT label.
fn has_sent_label(rec: &Value) -> bool {
    rec.get("labelIds").and_then(Value::as_array).is_some_and(|l| l.iter().any(|x| x.as_str() == Some("SENT")))
}

fn gmail_header(payload: Option<&Value>, name: &str) -> Option<String> {
    payload?
        .get("headers")?
        .as_array()?
        .iter()
        .find(|h| h.get("name").and_then(Value::as_str).is_some_and(|n| n.eq_ignore_ascii_case(name)))
        .and_then(|h| h.get("value").and_then(Value::as_str))
        .map(str::to_string)
        .filter(|v| !v.trim().is_empty())
}

fn decode_b64url(data: &str) -> Option<String> {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    let bytes = URL_SAFE_NO_PAD.decode(data.trim().trim_end_matches('=')).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

#[derive(Default)]
struct GmailContent {
    text: Option<String>,
    html: Option<String>,
    attachments: Vec<AttachmentMeta>,
}

/// Walks a Gmail `format=full` MIME tree: first text/plain and text/html bodies, plus
/// attachment metadata (parts with a filename). Attachment bytes are never fetched.
fn walk_gmail_part(part: &Value, out: &mut GmailContent) {
    let mime = part.get("mimeType").and_then(Value::as_str).unwrap_or_default();
    let filename = part.get("filename").and_then(Value::as_str).unwrap_or_default();
    let body = part.get("body");
    if !filename.is_empty() {
        out.attachments.push(AttachmentMeta {
            filename: normalize::truncate_chars(filename, 255),
            media_type: (!mime.is_empty()).then(|| mime.to_string()),
            size_bytes: body.and_then(|b| b.get("size")).and_then(Value::as_i64),
            provider_ref: body.and_then(|b| b.get("attachmentId")).and_then(Value::as_str).map(str::to_string),
        });
    } else if let Some(data) = body.and_then(|b| b.get("data")).and_then(Value::as_str) {
        if mime == "text/plain" && out.text.is_none() {
            out.text = decode_b64url(data);
        } else if mime == "text/html" && out.html.is_none() {
            out.html = decode_b64url(data);
        }
    }
    for child in part.get("parts").and_then(Value::as_array).into_iter().flatten() {
        walk_gmail_part(child, out);
    }
}

/// Fills body/attachments (and any missing headers) from a Gmail payload.
pub fn apply_gmail_payload(email: &mut NormalizedEmail, payload: &Value, max_body_chars: usize) {
    let mut content = GmailContent::default();
    walk_gmail_part(payload, &mut content);
    match (content.text, content.html) {
        (Some(t), html) if !t.trim().is_empty() => {
            email.body_text = normalize::truncate_chars(&t, max_body_chars);
            email.body_html = html.map(|h| normalize::truncate_chars(&h, max_body_chars));
            email.preview_only = false;
        }
        (_, Some(h)) => {
            email.body_text = normalize::truncate_chars(&normalize::html_to_text(&h), max_body_chars);
            email.body_html = Some(normalize::truncate_chars(&h, max_body_chars));
            email.preview_only = false;
        }
        _ => {}
    }
    if !content.attachments.is_empty() {
        email.attachments = content.attachments;
    }
    crate::bulk::merge(&mut email.bulk_signals, crate::bulk::header_signals(&|n| gmail_header(Some(payload), n), &[]));
    if email.sender_address.is_empty()
        && let Some(from) = gmail_header(Some(payload), "From")
    {
        email.sender_address = normalize::address(&from);
        email.sender_name = normalize::display_name(&from);
    }
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
    backfilled_at: Option<i64>,
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
        "SELECT workspace_id, status, external_connection_id, provider_config_key, sync_cursor, backfilled_at
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

    // Learn the mailbox's own address first, so mail it sent is stored as outbound.
    jobs::message_jobs::learn_mailbox_address(state, &source_id, &connection_id, &pck).await;

    // One-time direct backfill from Gmail (the catalog sync only keeps ~100 recent).
    if src.backfilled_at.is_none() {
        match backfill_from_gmail(state, &source_id, &src.workspace_id, &connection_id, &pck).await {
            Ok(n) => tracing::info!(%source_id, imported_or_refreshed = n, "gmail backfill finished"),
            Err(e) => tracing::warn!(%source_id, error = %e, "gmail backfill failed; will retry next sync"),
        }
    } else {
        match incremental_from_gmail(state, &source_id, &src.workspace_id, &connection_id, &pck).await {
            Ok(n) => tracing::info!(%source_id, new = n, "gmail incremental pull finished"),
            Err(JobError::Permanent(e)) => {
                mark_source(state, &source_id, "degraded", Some(&e)).await?;
                return Err(JobError::Permanent(e));
            }
            Err(e) => tracing::warn!(%source_id, error = %e, "gmail incremental pull failed; Nango records still checked"),
        }
    }

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

    // Skip messages we already have, then fetch full content for the new ones only.
    let ids: Vec<&str> = candidates.iter().map(|r| r.email.source_message_id.as_str()).collect();
    let existing: Vec<String> = sqlx::query_scalar(
        "SELECT source_message_id FROM email_messages WHERE source_id = ?1 AND source_message_id IN (SELECT value FROM json_each(?2))",
    )
    .bind(&source_id)
    .bind(json!(ids).to_string())
    .fetch_all(&state.db)
    .await?;
    let existing: std::collections::HashSet<String> = existing.into_iter().collect();
    candidates.retain(|r| !existing.contains(&r.email.source_message_id));
    enrich_with_full_messages(state, &connection_id, &pck, &mut candidates).await;

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
    jobs::message_jobs::enqueue_html_refetch(state, &src.workspace_id, &source_id).await?;
    if more_pages {
        let mut conn = state.db.acquire().await?;
        enqueue_sync(&mut conn, &src.workspace_id, &source_id, "continue").await?;
    }
    tracing::info!(%source_id, imported, initial, "nango sync finished");
    state.jobs_notify.notify_one();
    Ok(())
}

const ENRICH_CONCURRENCY: usize = 8;

/// One-time: the last N days of mail, straight from Gmail. Marks the source backfilled.
async fn backfill_from_gmail(
    state: &AppState,
    source_id: &str,
    workspace_id: &str,
    connection_id: &str,
    pck: &str,
) -> Result<usize, JobError> {
    let q = format!("newer_than:{}d -in:chats", state.config.nango_backfill_days);
    let changed =
        pull_from_gmail(state, source_id, workspace_id, connection_id, pck, &q, state.config.nango_backfill_max, "backfill").await?;
    sqlx::query("UPDATE email_sources SET backfilled_at = ?1 WHERE id = ?2").bind(now_ms()).bind(source_id).execute(&state.db).await?;
    Ok(changed)
}

/// Every sync: mail newer than the newest stored message (minus 1 h overlap; dedupe
/// absorbs repeats). Independent of Nango's hourly template sync.
async fn incremental_from_gmail(
    state: &AppState,
    source_id: &str,
    workspace_id: &str,
    connection_id: &str,
    pck: &str,
) -> Result<usize, JobError> {
    let newest: Option<i64> = sqlx::query_scalar("SELECT MAX(received_at) FROM email_messages WHERE source_id = ?1")
        .bind(source_id)
        .fetch_one(&state.db)
        .await?;
    let since_secs = newest.map(|ms| ms / 1000 - 3600).unwrap_or_else(|| now_ms() / 1000 - 86_400);
    let q = format!("after:{since_secs} -in:chats");
    pull_from_gmail(state, source_id, workspace_id, connection_id, pck, &q, 200, "incremental").await
}

/// Lists Gmail message ids matching `q`, fetches the new ones in full and stores them;
/// already-stored preview-only messages are upgraded. Returns how many changed.
#[allow(clippy::too_many_arguments)]
async fn pull_from_gmail(
    state: &AppState,
    source_id: &str,
    workspace_id: &str,
    connection_id: &str,
    pck: &str,
    q: &str,
    cap: usize,
    via: &str,
) -> Result<usize, JobError> {
    let mut ids: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let (page, next) = state.nango.gmail_list_ids(connection_id, pck, q, token.as_deref()).await?;
        ids.extend(page);
        if ids.len() >= cap || next.is_none() {
            break;
        }
        token = next;
    }
    ids.truncate(cap);
    // Skip what we already have with a full body (incremental runs re-list an hour of overlap).
    let known: Vec<String> = sqlx::query_scalar(
        "SELECT source_message_id FROM email_messages WHERE source_id = ?1 AND content_complete = 1
           AND source_message_id IN (SELECT value FROM json_each(?2))",
    )
    .bind(source_id)
    .bind(json!(ids).to_string())
    .fetch_all(&state.db)
    .await?;
    let known: std::collections::HashSet<String> = known.into_iter().collect();
    ids.retain(|id| !known.contains(id));

    // Reuse the enrichment path: start from bare records, fill from format=full.
    let mut records: Vec<MappedRecord> = ids
        .into_iter()
        .filter_map(|id| map_gmail_record(&json!({ "id": id }), state.config.max_body_chars))
        .collect();
    enrich_with_full_messages(state, connection_id, pck, &mut records).await;
    records.retain(|r| !r.email.sender_address.is_empty()); // fetch failed → skip, retry next time

    let mut changed = 0usize;
    for chunk in records.chunks(PERSIST_CHUNK) {
        let mut tx = state.db.begin().await?;
        sqlx::query("UPDATE email_sources SET last_synced_at = ?1 WHERE id = ?2")
            .bind(now_ms())
            .bind(source_id)
            .execute(&mut *tx)
            .await?;
        for r in chunk {
            let created = ingest::persist_message(
                &mut tx,
                source_id,
                workspace_id,
                &r.email,
                r.thread_id.as_deref(),
                json!({ "provider": GMAIL_KIND, "via": via }),
            )
            .await?;
            if created.is_some() || ingest::refresh_content(&mut tx, source_id, &r.email).await?.is_some() {
                changed += 1;
            }
        }
        tx.commit().await?;
    }
    state.jobs_notify.notify_one();
    Ok(changed)
}

/// Fetches bodies + attachment metadata through Nango's proxy, 8 at a time, outside any
/// transaction. A failed fetch keeps the snippet rather than failing the whole sync.
async fn enrich_with_full_messages(state: &AppState, connection_id: &str, pck: &str, records: &mut [MappedRecord]) {
    let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(ENRICH_CONCURRENCY));
    let mut tasks = tokio::task::JoinSet::new();
    for (i, r) in records.iter().enumerate() {
        let (nango, permits) = (state.nango.clone(), permits.clone());
        let (conn, pck, id) = (connection_id.to_string(), pck.to_string(), r.email.source_message_id.clone());
        tasks.spawn(async move {
            let _permit = permits.acquire_owned().await.ok()?;
            match nango.gmail_message_full(&conn, &pck, &id).await {
                Ok(full) => Some((i, full)),
                Err(e) => {
                    tracing::warn!(error = %e, "could not fetch full Gmail message; keeping snippet");
                    None
                }
            }
        });
    }
    while let Some(done) = tasks.join_next().await {
        if let Ok(Some((i, full))) = done {
            // A bare record (backfill) gets everything from the full message: headers,
            // thread, date; a catalog record keeps its headers and gains body + attachments.
            if records[i].email.sender_address.is_empty()
                && let Some(mapped) = map_gmail_record(&full, state.config.max_body_chars)
            {
                let cursor = records[i].cursor.take();
                records[i] = MappedRecord { cursor, ..mapped };
            } else if let Some(payload) = full.get("payload") {
                apply_gmail_payload(&mut records[i].email, payload, state.config.max_body_chars);
                records[i].email.outbound |= has_sent_label(&full);
            }
        }
    }
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
    fn maps_nango_catalog_gmail_messages() {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        let rec = json!({
            "id": "18c2ab", "threadId": "t9", "snippet": "Please find my timesheet", "historyId": "1",
            "internalDate": "1791900000000",
            "payload": {"headers": [
                {"name": "From", "value": "Priya Sharma <Priya@Acme.com>"},
                {"name": "To", "value": "hr@acme.com"}, {"name": "Cc", "value": "lead@acme.com"},
                {"name": "Subject", "value": "September timesheet"}, {"name": "Message-ID", "value": "<abc@mail>"}
            ]},
            "_nango_metadata": {"deleted_at": null, "last_action": "ADDED", "cursor": "c9"}
        });
        let m = map_gmail_record(&rec, 10_000).unwrap();
        assert_eq!(m.email.sender_address, "priya@acme.com");
        assert_eq!(m.email.sender_name.as_deref(), Some("Priya Sharma"));
        assert_eq!(m.email.recipients, vec!["hr@acme.com", "lead@acme.com"]);
        assert_eq!(m.email.subject, "September timesheet");
        assert_eq!(m.email.received_at, 1_791_900_000_000);
        assert_eq!(m.email.body_text, "Please find my timesheet", "snippet until enriched");

        // Enrichment from format=full: nested multipart with a PDF attachment.
        let full = json!({"mimeType": "multipart/mixed", "headers": [], "parts": [
            {"mimeType": "multipart/alternative", "parts": [
                {"mimeType": "text/plain", "body": {"data": URL_SAFE_NO_PAD.encode("Hi HR,\nAttached: 168 hours.")}},
                {"mimeType": "text/html", "body": {"data": URL_SAFE_NO_PAD.encode("<p>Hi HR</p>")}}
            ]},
            {"mimeType": "application/pdf", "filename": "timesheet.pdf", "body": {"attachmentId": "a1", "size": 4821}}
        ]});
        let mut email = m.email.clone();
        apply_gmail_payload(&mut email, &full, 10_000);
        assert_eq!(email.body_text, "Hi HR,\nAttached: 168 hours.");
        assert_eq!(email.body_html.as_deref(), Some("<p>Hi HR</p>"));
        assert_eq!(email.attachments.len(), 1);
        assert_eq!((email.attachments[0].filename.as_str(), email.attachments[0].size_bytes), ("timesheet.pdf", Some(4821)));
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
