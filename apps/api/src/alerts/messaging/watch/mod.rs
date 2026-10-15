//! Ask and report back. A watch is something Meera asked on a person's behalf and is waiting on: a reply (an email
//! in the thread, an in-app reply, or a finished desk case) wakes it and the answer is posted into that person's
//! Meera conversation, with the email it came from. If nobody answers within the nudge time she nudges once and
//! tells the person; a later answer still gets through.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};
use sqlx::SqliteConnection;

use super::recipients::Kind;
use crate::{
    agents::{injection, sentence},
    assistant::{command::Citation, plain_answer},
    platform::audit::{self, Actor, Entry},
    platform::db::{iso, new_id, now_ms},
    platform::error::{AppError, AppResult},
    http::AppState,
    jobs::{self, handlers::JobError, kind},
    replies::send_safety::is_automated_address,
    learning::knowledge,
};

mod create;
mod wake;
mod reply;
mod nudge;
mod close;
mod view;

pub use create::create;
pub use create::email_sent;
pub use wake::Automatic;
pub use wake::automatic;
pub use wake::on_inbound_email;
pub use wake::wake_for_reply;
pub use wake::on_case_ended;
pub use reply::reply_job;
pub use nudge::nudge_job;
pub use close::close_for_outbound;
pub use close::close_if_nothing_delivered;
pub use view::list;
pub use view::close;

pub const DEFAULT_NUDGE_HOURS: i64 = 24;
pub const MAX_NUDGE_HOURS: i64 = 24 * 14;
const ANSWER_WORDS: usize = 70;

pub struct NewWatch<'a> {
    pub ws: &'a str,
    pub user_id: &'a str,
    pub agent_id: Option<&'a str>,
    /// `ask`, `ticket` or `desk`.
    pub kind: &'a str,
    pub title: &'a str,
    /// `email`, `in_app` or `desk`.
    pub channel: &'a str,
    pub counterpart: &'a str,
    pub outbound_email_id: Option<&'a str>,
    pub thread_key: Option<&'a str>,
    pub source_id: Option<&'a str>,
    pub case_id: Option<&'a str>,
    pub question: &'a str,
    pub nudge_hours: i64,
}

#[derive(sqlx::FromRow)]
pub struct Watch {
    pub id: String,
    pub workspace_id: String,
    pub user_id: String,
    pub agent_id: Option<String>,
    pub kind: String,
    pub title: String,
    pub channel: String,
    pub counterpart: String,
    pub status: String,
    pub outbound_email_id: Option<String>,
    pub thread_key: Option<String>,
    pub case_id: Option<String>,
    pub question: String,
    pub nudge_hours: i64,
    pub nudged_at: Option<i64>,
    pub answer_text: Option<String>,
    pub last_update_at: i64,
}

const WATCH_COLUMNS: &str = "id, workspace_id, user_id, agent_id, kind, title, channel, counterpart, status, outbound_email_id, thread_key, case_id,
    question, nudge_hours, nudged_at, answer_text, last_update_at";

pub async fn get(state: &AppState, ws: &str, id: &str) -> sqlx::Result<Option<Watch>> {
    sqlx::query_as(&format!("SELECT {WATCH_COLUMNS} FROM watches WHERE id = ?1 AND workspace_id = ?2")).bind(id).bind(ws).fetch_optional(&state.db).await
}

fn job_fields(payload: &Value) -> Result<(String, String), JobError> {
    let field = |k: &str| payload.get(k).and_then(Value::as_str).map(str::to_string);
    match (field("workspace_id"), field("watch_id")) {
        (Some(ws), Some(id)) => Ok((ws, id)),
        _ => Err(JobError::Permanent("payload missing workspace_id/watch_id".into())),
    }
}

