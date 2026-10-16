//! Replying from the connected mailbox: save the draft into Gmail's Drafts, or send it.
//! Threaded into the original conversation and audited. People trigger these from the UI;
//! automations and agents call the same `save_draft` / `send_draft` core, so the guards are shared.
//!
//! Sending is two-step so it can be undone: `send_draft` runs the safety check and schedules the
//! send `SEND_UNDO_SECONDS` ahead; the `SEND_DRAFT` job re-checks and calls Gmail. Each Gmail call
//! carries a durable attempt id (our own Message-ID). If Gmail's answer is ambiguous (timeout, 5xx)
//! the attempt stays open and is reconciled against Gmail before any retry, so a reply is never sent twice.

use std::sync::LazyLock;

use axum::{
    Json,
    extract::{Path, State},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use lettre::message::{Mailbox, Message, header::ContentType};
use regex::Regex;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    ai::policy,
    platform::audit::{self, Actor, Entry},
    platform::auth::{AuthUser, require_role},
    platform::db::{iso, new_id, now_ms},
    platform::domain::{Capabilities, Role},
    platform::error::{AppError, AppResult},
    http::AppState,
    jobs::{self, handlers::JobError, kind},
    replies::send_safety::CheckInput,
    mail::sources::nango::NangoError,
};

mod message;
mod drafts;
mod reconcile;
mod send;
mod job;
pub mod live_draft;
pub mod compose;

pub use message::MAX_SUBJECT_CHARS;
pub use message::header_safe;
pub use message::reply_subject;
pub use message::safe_message_id;
pub use message::build_raw;
pub use message::build_raw_with_id;
pub use message::mailbox_address;
pub use drafts::save_draft;
pub use drafts::save_to_provider;
pub use reconcile::attempt_message_id;
pub use reconcile::RECONCILE_GRACE_MS;
pub use reconcile::Reconciled;
pub use reconcile::sent_in_thread;
pub use reconcile::reconcile;
pub use send::SendFailure;
pub use send::send_now;
pub use send::send_draft;
pub use send::send_draft_held;
pub use send::send;
pub use send::undo_send;
pub use job::send_scheduled;
pub use job::give_up_scheduled;
use message::raw_for;
use reconcile::clear_attempt;
use reconcile::finalize_sent;

#[derive(sqlx::FromRow)]
pub struct ReplyContext {
    pub draft_id: String,
    pub message_id: String,
    pub body_text: String,
    pub status: String,
    pub provider_draft_id: Option<String>,
    pub send_at: Option<i64>,
    pub send_requested_by: Option<String>,
    pub send_attempt_id: Option<String>,
    pub send_started_at: Option<i64>,
    pub subject: String,
    pub sender_address: String,
    pub rfc_message_id: Option<String>,
    pub thread_id: String,
    pub direction: String,
    pub original_body: String,
    pub source_id: String,
    pub provider_thread_id: Option<String>,
    pub source_kind: String,
    pub capabilities_json: String,
    pub connection_id: Option<String>,
    pub provider_config_key: Option<String>,
}

pub async fn load(state: &AppState, ws: &str, draft_id: &str) -> AppResult<ReplyContext> {
    sqlx::query_as(
        "SELECT d.id AS draft_id, d.message_id, d.body_text, d.status, d.provider_draft_id,
                d.send_at, d.send_requested_by, d.send_attempt_id, d.send_started_at,
                m.subject, m.sender_address, m.rfc_message_id, m.thread_id, m.direction, m.body_text AS original_body,
                m.source_id, t.provider_thread_id,
                s.kind AS source_kind, s.capabilities_json, s.external_connection_id AS connection_id, s.provider_config_key
         FROM drafts d
         JOIN email_messages m ON m.id = d.message_id
         JOIN email_threads t ON t.id = m.thread_id
         JOIN email_sources s ON s.id = m.source_id
         WHERE d.id = ?1 AND d.workspace_id = ?2",
    )
    .bind(draft_id)
    .bind(ws)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)
}

pub fn capabilities(ctx: &ReplyContext) -> Capabilities {
    serde_json::from_str(&ctx.capabilities_json).unwrap_or_else(|_| crate::mail::sources::capabilities(&ctx.source_kind))
}

pub fn connection(ctx: &ReplyContext) -> AppResult<(&str, &str)> {
    match (ctx.connection_id.as_deref(), ctx.provider_config_key.as_deref()) {
        (Some(c), Some(p)) => Ok((c, p)),
        _ => Err(AppError::Conflict("this email didn't come from a connected mailbox".into())),
    }
}

#[derive(Deserialize)]
pub struct SendBody {
    /// The UI sends `true` only after the person confirms recipient + subject.
    confirm: bool,
}

/// True while the draft still has a gap the AI could not fill ("[NEEDS: ...]").
pub fn has_open_gaps(body: &str) -> bool {
    body.contains("[NEEDS:")
}

/// Result of `send_draft`: the reply goes out at `send_at` unless someone undoes it.
#[derive(Debug, Clone, Serialize)]
pub struct ScheduledSend {
    pub status: &'static str,
    pub draft_id: String,
    pub send_at: String,
    #[serde(skip)]
    pub send_at_ms: i64,
}

pub mod send_safety;
pub mod signoff;
pub mod reply_settings;
pub mod repair;
pub mod gap_resolver;
pub mod owner_said;
pub mod skills;
