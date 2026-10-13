//! Resend Inbound: Svix-signed webhooks + "retrieve received email" API.

use std::time::Duration;

use axum::http::HeaderMap;
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use crate::{
    db::{now_ms, parse_iso_ms},
    domain::{AttachmentMeta, NormalizedEmail},
    sources::normalize,
};

const TOLERANCE_SECS: i64 = 5 * 60;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SignatureError {
    #[error("missing signature headers")]
    MissingHeaders,
    #[error("timestamp outside tolerance")]
    Stale,
    #[error("invalid signature")]
    Invalid,
    #[error("webhook secret not configured")]
    NotConfigured,
}

/// Verifies a Svix signature over the *raw* body. Returns the delivery ID (`svix-id`),
/// which is stable across retries of the same delivery and is our idempotency key.
pub fn verify_svix(secret: &str, headers: &HeaderMap, body: &[u8], now_secs: i64) -> Result<String, SignatureError> {
    let key = secret.strip_prefix("whsec_").unwrap_or(secret);
    if key.is_empty() {
        return Err(SignatureError::NotConfigured);
    }
    let key = B64.decode(key).map_err(|_| SignatureError::NotConfigured)?;
    let get = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
    let (Some(id), Some(ts), Some(sigs)) = (get("svix-id"), get("svix-timestamp"), get("svix-signature")) else {
        return Err(SignatureError::MissingHeaders);
    };
    let ts_num: i64 = ts.parse().map_err(|_| SignatureError::Invalid)?;
    if (now_secs - ts_num).abs() > TOLERANCE_SECS {
        return Err(SignatureError::Stale);
    }
    let expected = sign(&key, &id, &ts, body);
    let ok = sigs
        .split_whitespace()
        .filter_map(|s| s.strip_prefix("v1,"))
        .filter_map(|s| B64.decode(s).ok())
        .any(|candidate| candidate.ct_eq(&expected).into());
    if ok { Ok(id) } else { Err(SignatureError::Invalid) }
}

pub fn sign(key: &[u8], id: &str, ts: &str, body: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(id.as_bytes());
    mac.update(b".");
    mac.update(ts.as_bytes());
    mac.update(b".");
    mac.update(body);
    mac.finalize().into_bytes().to_vec()
}

/// `email.received` webhook envelope. Metadata only; the body is fetched separately.
#[derive(Debug, Deserialize)]
pub struct WebhookEnvelope {
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(default)]
    pub data: serde_json::Value,
}

/// Shape of `GET /emails/receiving/{id}`. Also used for locally injected test emails.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReceivedEmail {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub to: Vec<String>,
    #[serde(default)]
    pub cc: Option<Vec<String>>,
    #[serde(default)]
    pub received_for: Option<serde_json::Value>,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub html: Option<String>,
    #[serde(default)]
    pub message_id: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub attachments: Vec<ReceivedAttachment>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReceivedAttachment {
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub size: Option<i64>,
}

impl ReceivedEmail {
    /// Every address this copy was delivered to; used to resolve the workspace alias.
    pub fn delivery_addresses(&self) -> Vec<String> {
        let mut out: Vec<String> = self.to.iter().chain(self.cc.iter().flatten()).map(|a| normalize::address(a)).collect();
        match &self.received_for {
            Some(serde_json::Value::String(s)) => out.push(normalize::address(s)),
            Some(serde_json::Value::Array(items)) => {
                out.extend(items.iter().filter_map(|v| v.as_str()).map(normalize::address))
            }
            _ => {}
        }
        out.sort();
        out.dedup();
        out
    }

    pub fn normalize(&self, max_body_chars: usize) -> NormalizedEmail {
        let body_text = match (&self.text, &self.html) {
            (Some(t), _) if !t.trim().is_empty() => t.clone(),
            (_, Some(h)) => normalize::html_to_text(h),
            _ => String::new(),
        };
        let body_text = normalize::truncate_chars(&body_text, max_body_chars);
        NormalizedEmail {
            source_message_id: self.id.clone(),
            rfc_message_id: self.message_id.clone(),
            sender_address: normalize::address(&self.from),
            sender_name: normalize::display_name(&self.from),
            original_sender_address: normalize::forwarded_original_sender(&body_text),
            recipients: self.to.iter().map(|a| normalize::address(a)).collect(),
            subject: normalize::truncate_chars(
                self.subject.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("(no subject)"),
                998,
            ),
            body_text,
            body_html: self.html.as_deref().map(|h| normalize::truncate_chars(h, max_body_chars)),
            received_at: self.created_at.as_deref().and_then(parse_iso_ms).unwrap_or_else(now_ms),
            attachments: self
                .attachments
                .iter()
                .map(|a| AttachmentMeta {
                    filename: normalize::truncate_chars(a.filename.as_deref().unwrap_or("attachment"), 255),
                    media_type: a.content_type.clone(),
                    size_bytes: a.size,
                    provider_ref: None,
                })
                .collect(),
        }
    }
}

pub struct ResendClient {
    http: reqwest::Client,
    api_url: String,
    api_key: String,
}

impl ResendClient {
    pub fn new(http: reqwest::Client, api_url: &str, api_key: &str) -> Self {
        Self { http, api_url: api_url.trim_end_matches('/').to_string(), api_key: api_key.to_string() }
    }

    pub async fn get_received_email(&self, email_id: &str) -> Result<ReceivedEmail, FetchError> {
        if self.api_key.is_empty() {
            return Err(FetchError::Permanent("RESEND_API_KEY is not configured".into()));
        }
        if !email_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') || email_id.is_empty() {
            return Err(FetchError::Permanent("invalid email id".into()));
        }
        let res = self
            .http
            .get(format!("{}/emails/receiving/{email_id}", self.api_url))
            .bearer_auth(&self.api_key)
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .map_err(|e| FetchError::Retryable(e.to_string()))?;
        let status = res.status();
        if status.is_success() {
            return res.json().await.map_err(|e| FetchError::Retryable(format!("decode: {e}")));
        }
        let msg = format!("resend returned {status}");
        if status.as_u16() == 429 || status.is_server_error() || status.as_u16() == 404 {
            // 404 can be a short replication delay right after the webhook.
            Err(FetchError::Retryable(msg))
        } else {
            Err(FetchError::Permanent(msg))
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("{0}")]
    Retryable(String),
    #[error("{0}")]
    Permanent(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn secret() -> String {
        crate::config::test_webhook_secret()
    }

    fn headers(id: &str, ts: i64, sig: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("svix-id", HeaderValue::from_str(id).unwrap());
        h.insert("svix-timestamp", HeaderValue::from_str(&ts.to_string()).unwrap());
        h.insert("svix-signature", HeaderValue::from_str(sig).unwrap());
        h
    }

    fn valid_sig(id: &str, ts: i64, body: &[u8]) -> String {
        let key = B64.decode(secret().trim_start_matches("whsec_")).unwrap();
        format!("v1,{}", B64.encode(sign(&key, id, &ts.to_string(), body)))
    }

    #[test]
    fn accepts_valid_signature_among_several() {
        let body = br#"{"type":"email.received"}"#;
        let sig = format!("v1,bm9wZQ== {}", valid_sig("msg_1", 1000, body));
        assert_eq!(verify_svix(&secret(), &headers("msg_1", 1000, &sig), body, 1010), Ok("msg_1".into()));
    }

    #[test]
    fn rejects_tampered_body_stale_and_missing() {
        let body = br#"{"type":"email.received"}"#;
        let sig = valid_sig("msg_1", 1000, body);
        assert_eq!(verify_svix(&secret(), &headers("msg_1", 1000, &sig), b"{}", 1000), Err(SignatureError::Invalid));
        assert_eq!(verify_svix(&secret(), &headers("msg_1", 1000, &sig), body, 2000), Err(SignatureError::Stale));
        assert_eq!(verify_svix(&secret(), &HeaderMap::new(), body, 1000), Err(SignatureError::MissingHeaders));
        assert_eq!(verify_svix("", &headers("msg_1", 1000, &sig), body, 1000), Err(SignatureError::NotConfigured));
    }
}
