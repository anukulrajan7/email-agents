//! A new email from a person's own mailbox. It is checked, held for a few seconds (ten minutes when nobody asked
//! for it), sent through Gmail under a durable attempt id, and recorded. A reply to an email is still a draft;
//! this is for mail that starts a conversation (or follows one up).

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use lettre::message::{Mailbox as LettreMailbox, Message, header::ContentType};
use reqwest::Method;
use serde_json::{Value, json};

use super::recipients::{self, Kind};
use crate::{
    platform::audit::{self, Actor, Entry},
    platform::db::{iso, new_id, now_ms},
    platform::domain::Capabilities,
    platform::error::{AppError, AppResult},
    http::AppState,
    jobs::{self, handlers::JobError, kind},
    replies::{attempt_message_id, header_safe, MAX_SUBJECT_CHARS},
    replies::send_safety::{self, Issue, SafetyReport, bare_address, valid_address},
    mail::sources::{nango::NangoError, normalize::truncate_chars},
};

mod mailbox;
mod create;
mod schedule;
mod send;

pub use mailbox::Mailbox;
pub use mailbox::mailbox_for;
pub use mailbox::mailbox_by_source;
pub use create::check;
pub use create::NewEmail;
pub use create::validate;
pub use create::insert;
pub use create::create;
pub use schedule::Scheduled;
pub use schedule::schedule;
pub use schedule::stop;
pub use schedule::approve;
pub use send::build_raw_new;
pub use send::run_scheduled;
pub use send::give_up_scheduled;
use create::issue;
use schedule::own_addresses;

pub const SENT_ACTION: &str = "message.email_sent";
pub const AUTO_SENT_ACTION: &str = "message.auto_sent";
/// An open attempt younger than this may not be visible in Gmail yet: never call it "not sent".
const RECONCILE_GRACE_MS: i64 = 120_000;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Outbound {
    pub id: String,
    pub workspace_id: String,
    pub source_id: String,
    pub owner_user_id: Option<String>,
    pub agent_id: Option<String>,
    pub to_address: String,
    pub to_label: String,
    pub subject: String,
    pub body_text: String,
    pub recipient_kind: String,
    pub authored_by: String,
    pub approval_reason: Option<String>,
    pub status: String,
    pub auto_sent: bool,
    pub send_at: Option<i64>,
    pub send_requested_by: Option<String>,
    pub send_error: Option<String>,
    pub send_attempt_id: Option<String>,
    pub send_started_at: Option<i64>,
    pub provider_message_id: Option<String>,
    pub provider_thread_id: Option<String>,
    pub rfc_message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub watch_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

const COLUMNS: &str = "id, workspace_id, source_id, owner_user_id, agent_id, to_address, to_label, subject, body_text, recipient_kind,
    authored_by, approval_reason, status, auto_sent, send_at, send_requested_by, send_error, send_attempt_id, send_started_at,
    provider_message_id, provider_thread_id, rfc_message_id, in_reply_to, watch_id, created_at, updated_at";

pub async fn get(state: &AppState, ws: &str, id: &str) -> sqlx::Result<Option<Outbound>> {
    sqlx::query_as(&format!("SELECT {COLUMNS} FROM outbound_emails WHERE id = ?1 AND workspace_id = ?2"))
        .bind(id)
        .bind(ws)
        .fetch_optional(&state.db)
        .await
}

impl Outbound {
    pub fn to_label_or_address(&self) -> String {
        if self.to_label.trim().is_empty() || self.to_label == self.to_address {
            self.to_address.clone()
        } else {
            format!("{} ({})", self.to_label, self.to_address)
        }
    }

    pub fn to_json(&self) -> Value {
        let status = match self.status.as_str() {
            "awaiting_approval" => "needs_you",
            other => other,
        };
        json!({
            "id": self.id, "channel": "email", "to": self.to_label_or_address(), "subject": self.subject, "body": self.body_text,
            "status": status, "reason": self.approval_reason, "error": self.send_error, "auto": self.auto_sent,
            "at": self.send_at.map(iso).unwrap_or_else(|| iso(self.updated_at)),
            "can_stop": self.status == "scheduled" && self.send_attempt_id.is_none(),
            "can_approve": self.status == "awaiting_approval",
        })
    }
}

/// The caller's emails (as owner), newest first.
pub async fn list_for(state: &AppState, ws: &str, user_id: &str) -> sqlx::Result<Vec<Outbound>> {
    sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM outbound_emails WHERE workspace_id = ?1 AND owner_user_id = ?2 ORDER BY created_at DESC, id DESC LIMIT 100"
    ))
    .bind(ws)
    .bind(user_id)
    .fetch_all(&state.db)
    .await
}

/// Emails sent on their own today for one person, against the daily cap.
pub async fn auto_sent_today(state: &AppState, ws: &str, owner: &str) -> sqlx::Result<i64> {
    let mut conn = state.db.acquire().await?;
    auto_sent_today_in(&mut conn, ws, owner).await
}

/// The same count inside a transaction, so the cap and the send it allows are one step.
pub async fn auto_sent_today_in(conn: &mut sqlx::SqliteConnection, ws: &str, owner: &str) -> sqlx::Result<i64> {
    const DAY_MS: i64 = 86_400_000;
    let since = now_ms() / DAY_MS * DAY_MS;
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE workspace_id = ?1 AND action = ?2 AND created_at >= ?3 AND json_extract(metadata_json, '$.owner') = ?4",
    )
    .bind(ws)
    .bind(AUTO_SENT_ACTION)
    .bind(since)
    .bind(owner)
    .fetch_one(conn)
    .await
}
