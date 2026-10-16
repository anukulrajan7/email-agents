//! Timesheets kept as records. Mail labelled `timesheet` is read (the body, and each attachment's text) into
//! `timesheet_records`: who it is for, the period, the hours per day and a total, plus the issues found. A new
//! record replaces an old one (`replaced`, linked) only when it is plainly the corrected version: the same person,
//! the same client, a stated period that covers the old one, from another email. Anything else that overlaps is kept
//! and flagged `possible_duplicate`. Reading the same email again changes nothing.
//!
//! Records outlive their email: `message_id` and `attachment_id` become NULL when the email goes (deleted, or its
//! mailbox disconnected), and the record keeps the sender, subject, date and file name it came from.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::{SqliteConnection, SqlitePool};

use super::{
    name_key,
    policy::{address_of, period_of},
    timesheet_parse::{self as parse, Aliases, Candidate, Entry, Sheet},
};
use crate::{
    ai::{
        AiError,
        routing::{SheetTier, sheet_tier},
    },
    platform::db::{iso, new_id, now_ms},
    http::AppState,
    platform::packs,
    hr::team::{access::Access, clock, settings},
};

mod source;
mod read;
mod record;
mod settle;
mod place;
mod query;
mod link;
mod grid;
mod backfill;
pub mod rules;

use source::load_people;
use source::load_units;
use source::kind_label;
use read::read_text;
use read::assess;
pub use record::record_message;
use settle::settle;
use settle::duplicate_issue;
use place::existing_by_source;
use place::insert_record;
use place::retire_empty_pasted;
pub use query::Filter;
pub use query::list;
pub use query::get;
pub use query::can_see_record;
pub use query::for_message;
pub use query::attention;
pub use query::suppresses_auto_reply;
pub use link::link_person;
pub use grid::grid;
pub use backfill::backfill;
pub use backfill::erase_sender;
use query::SELECT;
use query::visible;
use read::weekday_name;

pub const LABEL: &str = "timesheet";
/// Recorded as the model of a label given by the plain rule.
pub const MODEL_ID: &str = "timesheet-rules-v1";
const CHECK: &str = "check";
const NOTE: &str = "note";

static NAME_CUE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)time[\s_\-]?sheets?|time[\s_\-]?cards?|hours?[\s_\-]?(?:report|log)").expect("static regex"));

const QUESTION_WORDS: [&str; 9] = ["question", "policy", "reminder", "approv", "submit your", "please submit", "overdue", "deadline", "?"];

/// Mail that talks about timesheets rather than carrying one: a reply, a question, a policy or a reminder.
fn talks_about_timesheets(subject: &str) -> bool {
    let lower = subject.trim().to_lowercase();
    lower.starts_with("re:") || lower.starts_with("re :") || QUESTION_WORDS.iter().any(|w| lower.contains(w))
}

/// The plain-rule label: an attachment named timesheet / time sheet / timecard / hours report, or a subject that says
/// so and is not a reply or a question about timesheets (those are left to the model classifier).
pub fn looks_like_timesheet(subject: &str, filenames: &[String]) -> bool {
    filenames.iter().any(|f| NAME_CUE.is_match(f)) || (NAME_CUE.is_match(subject) && !talks_about_timesheets(subject))
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Issue {
    pub code: String,
    pub message: String,
    /// `check` makes the record need a person's look; `note` is information.
    pub severity: &'static str,
}

fn check(code: &str, message: impl Into<String>) -> Issue {
    Issue { code: code.into(), message: message.into(), severity: CHECK }
}

fn note(code: &str, message: impl Into<String>) -> Issue {
    Issue { code: code.into(), message: message.into(), severity: NOTE }
}

/// What reading one email left behind, for the pipeline and the agent step.
#[derive(Debug, Clone, Serialize)]
pub struct Saved {
    pub id: String,
    pub status: String,
    pub person_id: Option<String>,
    pub person_name: Option<String>,
    pub period_start: String,
    pub period_end: String,
    pub total_hours: Option<f64>,
    pub issues: Vec<Value>,
    pub created: bool,
}

impl Saved {
    /// A person must look: the numbers need a check or nobody could be matched.
    pub fn needs_attention(&self) -> bool {
        self.status == "needs_check" || self.person_id.is_none()
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Run {
    pub records: Vec<Saved>,
    /// Attachments are still being read; the run happens again when they are.
    pub waiting_for_files: bool,
    /// The model could not read a sheet just now (a retryable error or the budget); nothing was recorded for it and
    /// it is tried again later.
    pub deferred: bool,
}

impl Run {
    pub fn needs_attention(&self) -> bool {
        self.records.iter().any(Saved::needs_attention)
    }
}

struct Unit {
    source_key: String,
    source: &'static str,
    attachment_id: Option<String>,
    filename: Option<String>,
    kind: Option<String>,
    text: String,
    unreadable: bool,
}

#[derive(sqlx::FromRow)]
struct AttachmentRow {
    id: String,
    filename: String,
    text_content: Option<String>,
    extract_status: Option<String>,
    extract_kind: Option<String>,
}

struct Read {
    sheet: Sheet,
    cost: f64,
    issues: Vec<Issue>,
    /// The model could not be asked now (retryable error, over budget): record nothing yet.
    deferred: bool,
}

#[derive(sqlx::FromRow)]
struct MessageRow {
    subject: String,
    body_text: String,
    sender_address: String,
    original_sender_address: Option<String>,
    received_at: i64,
    direction: String,
    suppressed: bool,
}

struct Draft<'a> {
    ws: &'a str,
    message_id: &'a str,
    unit: &'a Unit,
    msg: &'a MessageRow,
    sheet: &'a Sheet,
    person_id: Option<String>,
    method: &'static str,
    confidence: Option<f64>,
    period: (String, String),
    /// The sheet named no period; the month of the email was used.
    assumed: bool,
    total: Option<f64>,
    issues: Vec<Issue>,
    status: &'static str,
    client: Option<String>,
}

/// The record being placed.
struct Placed<'a> {
    id: &'a str,
    message_id: Option<&'a str>,
    client: Option<&'a str>,
    start: &'a str,
    end: &'a str,
    received: i64,
    assumed: bool,
}

#[derive(Default)]
struct Settled {
    /// A later email already replaces this record.
    superseded: bool,
    replaced: Vec<String>,
    /// This record is the newer of two overlapping sheets that were both kept.
    duplicate_of: Option<(String, String)>,
}

#[derive(sqlx::FromRow)]
pub struct RecordRow {
    id: String,
    person_id: Option<String>,
    linked_name: Option<String>,
    employee_code: Option<String>,
    person_name: Option<String>,
    sender_address: Option<String>,
    identity_json: String,
    match_method: String,
    match_confidence: Option<f64>,
    message_id: Option<String>,
    source: String,
    email_subject: Option<String>,
    email_received_at: Option<i64>,
    attachment_filename: Option<String>,
    attachment_kind: Option<String>,
    period_start: String,
    period_end: String,
    total_hours: Option<f64>,
    entries_json: String,
    client: Option<String>,
    status: String,
    replaces_id: Option<String>,
    issues_json: String,
    created_at: i64,
    updated_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_label_rule_reads_subject_and_file_names() {
        assert!(looks_like_timesheet("September timesheet", &[]));
        assert!(looks_like_timesheet("September timesheet - Priya", &[]));
        assert!(looks_like_timesheet("Question", &["timesheet_sep.xlsx".into()]));
        for talk in ["Question about my timesheet", "Timesheet policy for Q4", "Re: timesheet not approved", "Reminder: submit your timesheet", "Timesheet approval", "Where is my timesheet?"] {
            assert!(!looks_like_timesheet(talk, &[]), "{talk}");
        }
        assert!(looks_like_timesheet("Hello", &["Ravi_Time Sheet_Sep.xlsx".into()]));
        assert!(looks_like_timesheet("fyi", &["timecard-09.pdf".into()]));
        assert!(looks_like_timesheet("Monthly hours report", &[]));
        assert!(!looks_like_timesheet("Invoice for September", &["invoice.pdf".into()]));
        assert!(!looks_like_timesheet("Lunch?", &[]));
    }
}
