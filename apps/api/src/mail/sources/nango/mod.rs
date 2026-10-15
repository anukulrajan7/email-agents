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
    platform::audit::{self, Actor, Entry},
    platform::db::{iso, new_id, now_ms, parse_iso_ms},
    platform::domain::{AttachmentMeta, NormalizedEmail},
    platform::error::AppError,
    http::AppState,
    mail::ingest,
    jobs::{self, handlers::JobError, kind},
    mail::sources::{self, normalize},
};

mod client;
mod webhook;
mod mapping;
mod connect;
mod sync;

pub use client::ConnectSession;
pub use client::Connection;
pub use client::RecordsPage;
pub use client::NangoClient;
pub use webhook::verify_webhook;
pub use webhook::sign_webhook;
pub use mapping::MappedRecord;
pub use mapping::map_gmail_record;
pub use mapping::apply_gmail_payload;
pub use connect::register_connection;
pub use connect::enqueue_sync;
pub use connect::register_from_webhook;
pub use sync::sync_source;
use mapping::has_sent_label;

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

