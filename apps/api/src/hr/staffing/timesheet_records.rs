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

// ---------------------------------------------------------------- people

async fn load_people(db: &SqlitePool, ws: &str) -> sqlx::Result<(Vec<Candidate>, Aliases)> {
    let rows: Vec<(String, String, Option<String>)> =
        sqlx::query_as("SELECT id, name, employee_code FROM people WHERE workspace_id = ?1 AND active = 1").bind(ws).fetch_all(db).await?;
    let emails: Vec<(String, String)> = sqlx::query_as("SELECT person_id, email FROM person_emails WHERE workspace_id = ?1").bind(ws).fetch_all(db).await?;
    let mut by_person: HashMap<String, Vec<String>> = HashMap::new();
    for (p, e) in emails {
        by_person.entry(p).or_default().push(e);
    }
    let people = rows.into_iter().map(|(id, name, code)| Candidate { emails: by_person.remove(&id).unwrap_or_default(), id, name, code }).collect();
    let mut aliases = Aliases::default();
    let rows: Vec<(String, String, String)> = sqlx::query_as("SELECT kind, value, person_id FROM timesheet_aliases WHERE workspace_id = ?1").bind(ws).fetch_all(db).await?;
    for (kind, value, person_id) in rows {
        if kind == "code" { aliases.codes.insert(value, person_id) } else { aliases.names.insert(value, person_id) };
    }
    Ok((people, aliases))
}

// ---------------------------------------------------------------- reading

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

fn mismatch(sheet: &Sheet, tolerance: f64) -> bool {
    match (sheet.total, sheet.entries.is_empty()) {
        (Some(total), false) => (sheet.entries.iter().map(|e| e.hours).sum::<f64>() - total).abs() > tolerance,
        _ => false,
    }
}

/// A month or period the text really names: both end dates are written, or it is the month named in the text or subject.
fn period_supported(period: &(String, String), text: &str, subject: &str, hint_year: i64) -> bool {
    let written: HashSet<String> = text.lines().flat_map(|l| parse::dates_in(l, hint_year)).collect();
    (written.contains(&period.0) && written.contains(&period.1)) || parse::month_named(text, subject, hint_year).as_ref() == Some(period)
}

/// Keeps only what the text itself states: a model may copy but not invent. A day stays only when its date is written
/// in the text and its hours stand on the same line; the period must be written too; a total must be a stated number.
fn sanitize_model(mut s: Sheet, text: &str, subject: &str, hint_year: i64) -> (Sheet, Vec<Issue>) {
    let mut issues = vec![];
    let numbers = parse::numbers_in(text);
    let stated = |n: f64| numbers.iter().any(|x| parse::same_hours(*x, n));
    let lines: Vec<(Vec<String>, Vec<f64>)> =
        text.lines().filter(|l| l.chars().any(|c| c.is_ascii_digit())).map(|l| (parse::dates_in(l, hint_year), parse::numbers_in(l))).collect();
    let before = s.entries.len();
    s.entries.retain(|e| match &e.date {
        Some(d) => lines.iter().any(|(dates, nums)| dates.contains(d) && nums.iter().any(|n| parse::same_hours(*n, e.hours))),
        None => stated(e.hours),
    });
    let dropped = before - s.entries.len();
    if dropped > 0 {
        issues.push(check("model_days_dropped", format!("{dropped} day(s) the model returned are not in the sheet's text and were left out.")));
    }
    if s.period.as_ref().is_some_and(|p| !period_supported(p, text, subject, hint_year)) {
        s.period = None;
    }
    if s.total.is_some_and(|t| !stated(t)) {
        s.total = None;
    }
    let lower = text.to_lowercase();
    let written = |v: &Option<String>| v.as_ref().is_some_and(|v| lower.contains(&v.to_lowercase()));
    for field in [&mut s.identity.code, &mut s.identity.name, &mut s.identity.email, &mut s.identity.manager, &mut s.identity.client, &mut s.identity.location] {
        if !written(field) {
            *field = None;
        }
    }
    (s, issues)
}

/// The rules' reading wins wherever it found something: its total is never replaced by the model's.
fn merge(mut base: Sheet, model: Sheet) -> Sheet {
    let id = &mut base.identity;
    id.code = id.code.take().or(model.identity.code);
    id.name = id.name.take().or(model.identity.name);
    id.email = id.email.take().or(model.identity.email);
    id.client = id.client.take().or(model.identity.client);
    id.manager = id.manager.take().or(model.identity.manager);
    id.location = id.location.take().or(model.identity.location);
    base.period = model.period.or(base.period);
    base.total = base.total.or(model.total);
    if !model.entries.is_empty() {
        base.entries = model.entries;
    }
    base
}

struct Read {
    sheet: Sheet,
    cost: f64,
    issues: Vec<Issue>,
    /// The model could not be asked now (retryable error, over budget): record nothing yet.
    deferred: bool,
}

/// Plain rules first. The model reads only what they could not: nothing found, or entries that do not add up to
/// the stated total. Cheap model, then the strong one when the cheap answer still fails or the sheet is large.
async fn read_text(state: &AppState, ws: &str, subject: &str, text: &str, hint_year: i64, tolerance: f64, allow_model: bool) -> Read {
    let mut out = Read { sheet: parse::parse_sheet(text, subject, hint_year), cost: 0.0, issues: vec![], deferred: false };
    let bad = |s: &Sheet| !s.has_data() || mismatch(s, tolerance);
    if !allow_model || !bad(&out.sheet) || !text.chars().any(|c| c.is_ascii_digit()) {
        return out;
    }
    let (chars, rows) = (text.chars().count(), text.lines().count());
    let wanted: &[SheetTier] = if sheet_tier(chars, rows, false) == SheetTier::Strong { &[SheetTier::Strong] } else { &[SheetTier::Cheap, SheetTier::Strong] };
    let tiers: Vec<SheetTier> = wanted.iter().copied().filter(|t| state.timesheet_reader.has(*t)).collect();
    if tiers.is_empty() {
        return out;
    }
    if !crate::ai::ledger::within_budget(&state.db, state.config.ai_daily_budget_usd, ws).await {
        out.deferred = !out.sheet.has_data();
        return out;
    }
    let rules = out.sheet.clone();
    let mut model_issues: Vec<Issue> = vec![];
    let mut from_model = false;
    for tier in tiers {
        match state.timesheet_reader.read(tier, subject, text, hint_year).await {
            Ok(m) => {
                out.cost += m.cost_usd;
                let (clean, dropped) = sanitize_model(m.sheet, text, subject, hint_year);
                let uses_model_days = !clean.entries.is_empty();
                let candidate = merge(rules.clone(), clean);
                let better = !bad(&candidate) || (!out.sheet.has_data() && candidate.has_data());
                if better {
                    out.sheet = candidate;
                    model_issues = dropped;
                    from_model = uses_model_days;
                }
                if !bad(&out.sheet) {
                    break;
                }
            }
            Err(AiError::Retryable(e)) => {
                tracing::warn!(error = %e, %ws, "timesheet model is not available just now; the sheet is read again later");
                out.deferred = true;
                return out;
            }
            Err(e) => tracing::warn!(error = %e, %ws, "timesheet model could not read the sheet"),
        }
    }
    out.issues = model_issues;
    let sum: f64 = out.sheet.entries.iter().map(|e| e.hours).sum();
    if from_model && !out.sheet.total.is_some_and(|t| parse::same_hours(sum, t)) {
        out.issues.push(check("model_read", "The days were read by an AI model and do not add up to a total the sheet states; check them against the sheet."));
    }
    out
}

fn weekday_name(d: i64) -> &'static str {
    ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"][d.rem_euclid(7) as usize]
}

fn nice_date(d: &str) -> String {
    let (m, day) = (&d[5..7], &d[8..10]);
    let month = parse::MONTH_NAMES.get(m.parse::<usize>().unwrap_or(1).saturating_sub(1)).copied().unwrap_or("");
    format!("{} {month}", day.trim_start_matches('0'))
}

/// Total to keep and the issues found. Missing things stay missing; nothing is filled in.
fn assess(sheet: &Sheet, period: &(String, String), period_assumed: bool, tolerance: f64, matched: bool, who_text: &str) -> (Option<f64>, Vec<Issue>) {
    let mut issues = vec![];
    let sum: f64 = sheet.entries.iter().map(|e| e.hours).sum();
    let total = match (sheet.total, sheet.entries.is_empty()) {
        (Some(t), _) => Some(t),
        (None, false) => {
            issues.push(note("total_added_up", "The sheet states no total; this is the sum of its days."));
            Some((sum * 100.0).round() / 100.0)
        }
        (None, true) => None,
    };
    if !sheet.has_data() {
        issues.push(check("no_hours", "No hours could be read from this timesheet."));
    }
    if mismatch(sheet, tolerance) {
        issues.push(check("total_mismatch", format!("The days add up to {sum} hours but the sheet says {}.", total.unwrap_or_default())));
    }
    if period_assumed && sheet.has_data() {
        issues.push(note("period_assumed", "The sheet names no period; the month of the email was used."));
    }
    if !matched {
        issues.push(check("person_unmatched", format!("Who is this? ({who_text})")));
    }
    let dated: Vec<(&Entry, &str)> = sheet.entries.iter().filter_map(|e| e.date.as_deref().map(|d| (e, d))).collect();
    let outside: Vec<&str> = dated.iter().filter(|(_, d)| *d < period.0.as_str() || *d > period.1.as_str()).map(|(_, d)| *d).collect();
    if !outside.is_empty() {
        issues.push(note("outside_period", format!("{} day(s) fall outside the period, e.g. {}.", outside.len(), nice_date(outside[0]))));
    }
    let impossible: Vec<String> = sheet.entries.iter().filter(|e| e.hours > 24.0).map(|e| e.date.as_deref().map(nice_date).unwrap_or_else(|| "a row".into())).collect();
    if !impossible.is_empty() {
        issues.push(check("impossible_hours", format!("More than 24 hours on {}.", impossible.join(", "))));
    }
    let mut per_day: HashMap<&str, f64> = HashMap::new();
    for (e, d) in &dated {
        *per_day.entry(d).or_default() += e.hours;
    }
    let mut long: Vec<&str> = per_day.iter().filter(|(_, h)| **h > 12.0 && **h <= 24.0).map(|(d, _)| *d).collect();
    long.sort();
    if !long.is_empty() {
        issues.push(note("long_days", format!("More than 12 hours on {}.", long.iter().take(4).map(|d| nice_date(d)).collect::<Vec<_>>().join(", "))));
    }
    let mut weekend: Vec<String> = per_day
        .iter()
        .filter(|(d, h)| **h > 0.0 && parse::weekday(d).is_some_and(|w| w >= 5))
        .map(|(d, _)| format!("{} ({})", nice_date(d), weekday_name(parse::weekday(d).unwrap_or(0))))
        .collect();
    weekend.sort();
    if !weekend.is_empty() {
        issues.push(note("weekend_hours", format!("Hours on a weekend: {}.", weekend.iter().take(4).cloned().collect::<Vec<_>>().join(", "))));
    }
    if dated.len() >= 5
        && let (Some(a), Some(b)) = (clock::parse_date(&period.0), clock::parse_date(&period.1))
        && b - a <= 31
    {
        let worked: HashSet<&str> = per_day.keys().copied().collect();
        let missing: Vec<String> = (a..=b)
            .map(clock::date_string)
            .filter(|d| parse::weekday(d).is_some_and(|w| w < 5) && !worked.contains(d.as_str()))
            .collect();
        if !missing.is_empty() {
            let shown: Vec<String> = missing.iter().take(5).map(|d| nice_date(d)).collect();
            issues.push(note("missing_days", format!("No hours for {} weekday(s): {}{}.", missing.len(), shown.join(", "), if missing.len() > 5 { ", ..." } else { "" })));
        }
    }
    (total, issues)
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

/// `Some(true)` for a forwarded-message header, `Some(false)` for the header of a quoted reply.
fn quote_marker(lines: &[&str], i: usize) -> Option<bool> {
    let lower = lines[i].trim().to_lowercase();
    if lower.starts_with("begin forwarded message") || (lower.starts_with("--") && lower.contains("forwarded message")) {
        return Some(true);
    }
    if lower.starts_with("--") && lower.contains("original message") {
        return Some(false);
    }
    let wraps = lines.get(i + 1).is_some_and(|n| n.trim().to_lowercase().starts_with("wrote:"));
    (lower.starts_with("on ") && (lower.ends_with("wrote:") || wraps)).then_some(false)
}

/// What the sender wrote in this email: quoted lines (`>`) and everything under an "On ... wrote:" or "Original
/// message" header are an earlier email. A forward whose own words are only a short note is the exception: the
/// forwarded sheet is what the email is about.
fn own_text(subject: &str, body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let fresh = |part: &[&str]| -> Vec<String> { part.iter().filter(|l| !l.trim_start().starts_with('>')).map(|l| (*l).to_string()).collect() };
    let Some((cut, marker_forward)) = (0..lines.len()).find_map(|i| quote_marker(&lines, i).map(|forward| (i, forward))) else {
        return fresh(&lines).join("\n");
    };
    let mut kept = fresh(&lines[..cut]);
    let note_chars: usize = kept.iter().map(|l| l.trim().chars().count()).sum();
    let note_lines = kept.iter().filter(|l| !l.trim().is_empty()).count();
    let subject = subject.trim().to_lowercase();
    let forward = marker_forward || subject.starts_with("fwd:") || subject.starts_with("fw:");
    if forward && note_chars <= 300 && note_lines <= 6 {
        kept.extend(lines[cut + 1..].iter().map(|l| l.trim_start_matches(['>', ' ']).to_string()));
    }
    kept.join("\n")
}

async fn load_units(db: &SqlitePool, message_id: &str, msg: &MessageRow) -> sqlx::Result<(Vec<Unit>, bool)> {
    let rows: Vec<AttachmentRow> = sqlx::query_as(
        "SELECT id, filename, text_content, extract_status, extract_kind FROM attachments WHERE message_id = ?1 ORDER BY filename",
    )
    .bind(message_id)
    .fetch_all(db)
    .await?;
    let mut units = vec![Unit {
        source_key: format!("msg:{message_id}:body"),
        source: "pasted",
        attachment_id: None,
        filename: None,
        kind: None,
        text: own_text(&msg.subject, &msg.body_text),
        unreadable: false,
    }];
    let mut waiting = false;
    let sole = rows.len() == 1;
    for a in rows {
        let text = a.text_content.as_deref().unwrap_or_default().trim().to_string();
        if !text.is_empty() {
            units.push(Unit { source_key: format!("msg:{message_id}:att:{}", a.id), source: "attachment", attachment_id: Some(a.id), filename: Some(a.filename), kind: a.extract_kind, text, unreadable: false });
        } else if a.extract_status.is_none() {
            waiting = true;
        } else if NAME_CUE.is_match(&a.filename) || sole {
            units.push(Unit { source_key: format!("msg:{message_id}:att:{}", a.id), source: "attachment", attachment_id: Some(a.id), filename: Some(a.filename), kind: a.extract_kind, text: String::new(), unreadable: true });
        }
    }
    Ok((units, waiting))
}

fn kind_label(kind: Option<&str>, filename: Option<&str>) -> Option<String> {
    let ext = filename.and_then(|f| f.rsplit_once('.')).map(|(_, e)| e.to_lowercase());
    let label = match (kind, ext.as_deref()) {
        (_, Some("xlsx" | "xls" | "xlsm" | "xlsb" | "ods")) | (Some("sheet" | "spreadsheet"), _) => "Excel",
        (_, Some("csv" | "tsv")) => "CSV",
        (_, Some("pdf")) | (Some("pdf"), _) => "PDF",
        (_, Some("docx" | "doc" | "rtf" | "odt")) | (Some("docx" | "word"), _) => "Word",
        (Some("image"), _) | (_, Some("png" | "jpg" | "jpeg" | "webp")) => "Image",
        _ => return filename.map(|_| "File".to_string()),
    };
    Some(label.to_string())
}

/// Reads one email into records. Safe to run again: a source that already has a record is left as it is.
pub async fn record_message(state: &AppState, ws: &str, message_id: &str) -> sqlx::Result<Run> {
    let db = &state.db;
    let msg: Option<MessageRow> = sqlx::query_as(
        "SELECT subject, body_text, sender_address, original_sender_address, received_at, direction, suppressed
         FROM email_messages WHERE id = ?1 AND workspace_id = ?2",
    )
    .bind(message_id)
    .bind(ws)
    .fetch_optional(db)
    .await?;
    let Some(msg) = msg else { return Ok(Run::default()) };
    if msg.direction != "inbound" || msg.suppressed {
        return Ok(Run::default());
    }
    let received_at = msg.received_at;
    let cfg = settings::load(db, ws).await?;
    let (people, aliases) = load_people(db, ws).await?;
    let hint_year: i64 = clock::month_of(received_at, cfg.tz_offset_minutes)[..4].parse().unwrap_or(2026);
    let (units, waiting) = load_units(db, message_id, &msg).await?;
    // Files still being read: the run happens again when they are, and no model is paid for in the meantime.
    if waiting {
        return Ok(Run { waiting_for_files: true, ..Default::default() });
    }
    let has_files = units.iter().any(|u| u.source == "attachment");
    // Words alone, with no figure and no file, hold no timesheet.
    if !has_files && !units.iter().any(|u| u.text.chars().any(|c| c.is_ascii_digit())) {
        return Ok(Run::default());
    }
    let mut run = Run::default();
    let mut file_record_made = false;

    // Second pass: if the first one left nothing (a body that only says "see attached" next to files with no hours),
    // the body is recorded as empty so that someone looks, and the email is not read again and again.
    for force in [false, true] {
        if force && (!run.records.is_empty() || run.deferred) {
            break;
        }
        for unit in units.iter().take(if force { 1 } else { usize::MAX }) {
            if let Some(existing) = existing_by_source(db, ws, &unit.source_key).await? {
                file_record_made |= unit.source == "attachment";
                run.records.push(existing);
                continue;
            }
            let read = if unit.unreadable {
                Read { sheet: Sheet::default(), cost: 0.0, issues: vec![], deferred: false }
            } else {
                let allow_model = !(unit.source == "pasted" && has_files);
                read_text(state, ws, &msg.subject, &unit.text, hint_year, cfg.hours_tolerance, allow_model).await
            };
            if read.deferred {
                run.deferred = true;
                continue;
            }
            if read.cost > 0.0 {
                tracing::info!(%message_id, cost_usd = read.cost, "timesheet read with a model");
            }
            let Read { sheet, issues: read_issues, .. } = read;
            let empty = !sheet.has_data();
            if !force && unit.source == "pasted" && empty && has_files {
                continue;
            }
            if !force && unit.source == "attachment" && empty && !unit.unreadable && !NAME_CUE.is_match(unit.filename.as_deref().unwrap_or_default()) {
                continue;
            }
            let m = parse::match_person(&people, &aliases, &sheet.identity, &address_of(&msg.sender_address), msg.original_sender_address.as_deref().map(address_of).as_deref());
            let (period, assumed) = match sheet.period.clone() {
                Some(p) => (p, false),
                None => {
                    let month = period_of(&msg.subject, received_at, cfg.tz_offset_minutes, cfg.timesheet_grace_days);
                    (parse::month_dates(&month).unwrap_or_else(|| (clock::date_string(received_at.div_euclid(clock::DAY_MS)), clock::date_string(received_at.div_euclid(clock::DAY_MS)))), true)
                }
            };
            let who_text = match (&sheet.identity.name, &sheet.identity.code) {
                (Some(n), Some(c)) => format!("{n}, ID {c}"),
                (Some(n), None) => n.clone(),
                (None, Some(c)) => format!("ID {c}"),
                (None, None) => format!("no name or ID in the sheet; sent by {}", address_of(&msg.sender_address)),
            };
            let (total, mut issues) = assess(&sheet, &period, assumed, cfg.hours_tolerance, m.person_id.is_some(), &who_text);
            issues.extend(read_issues);
            if m.ambiguous {
                issues.push(note("ambiguous_person", "More than one person has this name."));
            }
            if unit.unreadable {
                issues.retain(|i| i.code != "no_hours");
                issues.push(check("unreadable", format!("The file {} could not be read.", unit.filename.as_deref().unwrap_or("attached"))));
            }
            let status = if issues.iter().any(|i| i.severity == CHECK) { "needs_check" } else { "recorded" };
            let draft = Draft {
                ws,
                message_id,
                unit,
                msg: &msg,
                sheet: &sheet,
                person_id: m.person_id.clone(),
                method: m.method,
                confidence: m.person_id.as_ref().map(|_| m.confidence),
                period,
                assumed,
                total,
                issues,
                status,
                client: sheet.identity.client.clone(),
            };
            let saved = insert_record(db, draft, people.iter().find(|p| Some(&p.id) == m.person_id.as_ref()).map(|p| p.name.clone())).await?;
            file_record_made |= unit.source == "attachment" && !unit.unreadable;
            run.records.push(saved);
        }
    }
    if file_record_made {
        retire_empty_pasted(db, ws, message_id).await?;
    }
    Ok(run)
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

async fn existing_by_source(db: &SqlitePool, ws: &str, source_key: &str) -> sqlx::Result<Option<Saved>> {
    let row: Option<RecordRow> = sqlx::query_as(&format!("{SELECT} WHERE r.workspace_id = ?1 AND r.source_key = ?2")).bind(ws).bind(source_key).fetch_optional(db).await?;
    Ok(row.map(|r| r.saved(false)))
}

/// A record already kept for the same person whose period overlaps.
#[derive(sqlx::FromRow)]
struct Kept {
    id: String,
    message_id: Option<String>,
    client: Option<String>,
    period_start: String,
    period_end: String,
    email_received_at: Option<i64>,
    issues_json: String,
}

impl Kept {
    fn assumed(&self) -> bool {
        serde_json::from_str::<Vec<Value>>(&self.issues_json).unwrap_or_default().iter().any(|i| i["code"] == "period_assumed")
    }

    fn received(&self) -> i64 {
        self.email_received_at.unwrap_or(0)
    }
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

fn client_key(c: Option<&str>) -> Option<String> {
    c.map(|c| c.trim().to_lowercase()).filter(|c| !c.is_empty())
}

fn same_mail(a: Option<&str>, b: Option<&str>) -> bool {
    a.is_some() && a == b
}

impl Placed<'_> {
    fn same_client(&self, old: &Kept) -> bool {
        client_key(self.client) == client_key(old.client.as_deref())
    }

    fn clients_differ(&self, old: &Kept) -> bool {
        matches!((client_key(self.client), client_key(old.client.as_deref())), (Some(a), Some(b)) if a != b)
    }

    /// The corrected version of `old`: same client, a stated period that covers it, from a later email.
    fn replaces(&self, old: &Kept) -> bool {
        !self.assumed
            && self.same_client(old)
            && self.start <= old.period_start.as_str()
            && self.end >= old.period_end.as_str()
            && !same_mail(self.message_id, old.message_id.as_deref())
            && self.received >= old.received()
    }

    /// `old` is a later email that would have replaced this one (mail read out of order).
    fn replaced_by(&self, old: &Kept) -> bool {
        !old.assumed()
            && self.same_client(old)
            && old.period_start.as_str() <= self.start
            && old.period_end.as_str() >= self.end
            && !same_mail(self.message_id, old.message_id.as_deref())
            && old.received() > self.received
    }
}

#[derive(Default)]
struct Settled {
    /// A later email already replaces this record.
    superseded: bool,
    replaced: Vec<String>,
    /// This record is the newer of two overlapping sheets that were both kept.
    duplicate_of: Option<(String, String)>,
}

fn duplicate_issue(start: &str, end: &str) -> Issue {
    note("possible_duplicate", format!("Another timesheet for this person covers {start} to {end}; check this is not a duplicate."))
}

/// Decides what an overlapping record means for the one being placed: replace it, be replaced, or sit beside it.
async fn settle(conn: &mut SqliteConnection, ws: &str, person_id: &str, me: &Placed<'_>, now: i64) -> sqlx::Result<Settled> {
    let kept: Vec<Kept> = sqlx::query_as(
        "SELECT id, message_id, client, period_start, period_end, email_received_at, issues_json FROM timesheet_records
         WHERE workspace_id = ?1 AND person_id = ?2 AND status != 'replaced' AND id != ?5 AND period_start <= ?4 AND period_end >= ?3
         ORDER BY COALESCE(email_received_at, 0) DESC, created_at DESC, id",
    )
    .bind(ws)
    .bind(person_id)
    .bind(me.start)
    .bind(me.end)
    .bind(me.id)
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Settled { superseded: kept.iter().any(|k| me.replaced_by(k)), ..Default::default() };
    for old in &kept {
        if !out.superseded && me.replaces(old) {
            sqlx::query("UPDATE timesheet_records SET status = 'replaced', updated_at = ?1 WHERE id = ?2 AND workspace_id = ?3").bind(now).bind(&old.id).bind(ws).execute(&mut *conn).await?;
            out.replaced.push(old.id.clone());
        } else if !out.superseded && !me.clients_differ(old) {
            if old.received() > me.received {
                add_issue(&mut *conn, ws, &old.id, duplicate_issue(me.start, me.end), now).await?;
            } else if out.duplicate_of.is_none() {
                out.duplicate_of = Some((old.period_start.clone(), old.period_end.clone()));
            }
        }
    }
    Ok(out)
}

async fn add_issue(conn: &mut SqliteConnection, ws: &str, id: &str, issue: Issue, now: i64) -> sqlx::Result<()> {
    let json: Option<String> = sqlx::query_scalar("SELECT issues_json FROM timesheet_records WHERE id = ?1 AND workspace_id = ?2").bind(id).bind(ws).fetch_optional(&mut *conn).await?;
    let Some(json) = json else { return Ok(()) };
    let mut issues: Vec<Value> = serde_json::from_str(&json).unwrap_or_default();
    if issues.iter().any(|i| i["code"] == issue.code) {
        return Ok(());
    }
    issues.push(json!(issue));
    sqlx::query("UPDATE timesheet_records SET issues_json = ?1, updated_at = ?2 WHERE id = ?3 AND workspace_id = ?4")
        .bind(serde_json::to_string(&issues).unwrap_or_else(|_| "[]".into()))
        .bind(now)
        .bind(id)
        .bind(ws)
        .execute(conn)
        .await?;
    Ok(())
}

async fn insert_record(db: &SqlitePool, d: Draft<'_>, person_name: Option<String>) -> sqlx::Result<Saved> {
    let id = new_id("tsr");
    let now = now_ms();
    let identity_json = serde_json::to_string(&d.sheet.identity).unwrap_or_else(|_| "{}".into());
    let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
    if let Some(existing) = existing_by_source_tx(&mut tx, d.ws, &d.unit.source_key).await? {
        tx.commit().await?;
        return Ok(existing);
    }
    let mut issues = d.issues.clone();
    let mut status = d.status;
    let mut replaces: Option<String> = None;
    if let Some(person) = &d.person_id {
        let me = Placed { id: &id, message_id: Some(d.message_id), client: d.client.as_deref(), start: &d.period.0, end: &d.period.1, received: d.msg.received_at, assumed: d.assumed };
        let settled = settle(&mut tx, d.ws, person, &me, now).await?;
        if settled.superseded {
            status = "replaced";
        }
        if let Some((a, b)) = &settled.duplicate_of {
            issues.push(duplicate_issue(a, b));
        }
        replaces = settled.replaced.into_iter().next();
    }
    sqlx::query(
        "INSERT INTO timesheet_records
           (id, workspace_id, person_id, employee_code, person_name, sender_address, identity_json, match_method, match_confidence,
            message_id, attachment_id, source_key, source, email_subject, email_received_at, attachment_filename, attachment_kind,
            period_start, period_end, total_hours, entries_json, client, status, replaces_id, issues_json, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?26)",
    )
    .bind(&id)
    .bind(d.ws)
    .bind(&d.person_id)
    .bind(&d.sheet.identity.code)
    .bind(&d.sheet.identity.name)
    .bind(address_of(&d.msg.sender_address))
    .bind(identity_json)
    .bind(d.method)
    .bind(d.confidence)
    .bind(d.message_id)
    .bind(&d.unit.attachment_id)
    .bind(&d.unit.source_key)
    .bind(d.unit.source)
    .bind(&d.msg.subject)
    .bind(d.msg.received_at)
    .bind(&d.unit.filename)
    .bind(kind_label(d.unit.kind.as_deref(), d.unit.filename.as_deref()))
    .bind(&d.period.0)
    .bind(&d.period.1)
    .bind(d.total)
    .bind(serde_json::to_string(&d.sheet.entries).unwrap_or_else(|_| "[]".into()))
    .bind(&d.client)
    .bind(status)
    .bind(&replaces)
    .bind(serde_json::to_string(&issues).unwrap_or_else(|_| "[]".into()))
    .bind(now)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Saved {
        id,
        status: status.into(),
        person_id: d.person_id,
        person_name,
        period_start: d.period.0,
        period_end: d.period.1,
        total_hours: d.total,
        issues: issues.iter().map(|i| json!(i)).collect(),
        created: true,
    })
}

async fn existing_by_source_tx(conn: &mut SqliteConnection, ws: &str, source_key: &str) -> sqlx::Result<Option<Saved>> {
    let row: Option<RecordRow> = sqlx::query_as(&format!("{SELECT} WHERE r.workspace_id = ?1 AND r.source_key = ?2")).bind(ws).bind(source_key).fetch_optional(&mut *conn).await?;
    Ok(row.map(|r| r.saved(false)))
}

/// A body that only said "see attached" and was recorded empty before the file was read: the file's record replaces it.
async fn retire_empty_pasted(db: &SqlitePool, ws: &str, message_id: &str) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE timesheet_records SET status = 'replaced', updated_at = ?3
         WHERE workspace_id = ?1 AND message_id = ?2 AND source = 'pasted' AND total_hours IS NULL AND entries_json = '[]' AND status != 'replaced'",
    )
    .bind(ws)
    .bind(message_id)
    .bind(now_ms())
    .execute(db)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------- reading records back

const SELECT: &str = "SELECT r.id, r.person_id, p.name AS linked_name, r.employee_code, r.person_name, r.sender_address, r.identity_json, r.match_method,
        r.match_confidence, r.message_id, r.source, r.email_subject, r.email_received_at, r.attachment_filename, r.attachment_kind,
        r.period_start, r.period_end, r.total_hours, r.entries_json, r.client, r.status, r.replaces_id, r.issues_json, r.created_at, r.updated_at
    FROM timesheet_records r LEFT JOIN people p ON p.id = r.person_id";

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

impl RecordRow {
    fn saved(self, created: bool) -> Saved {
        Saved {
            id: self.id,
            status: self.status,
            person_id: self.person_id,
            person_name: self.linked_name,
            period_start: self.period_start,
            period_end: self.period_end,
            total_hours: self.total_hours,
            issues: serde_json::from_str(&self.issues_json).unwrap_or_default(),
            created,
        }
    }

    fn entries(&self) -> Vec<Entry> {
        serde_json::from_str(&self.entries_json).unwrap_or_default()
    }

    pub fn json(&self, detail: bool) -> Value {
        let issues: Value = serde_json::from_str(&self.issues_json).unwrap_or_else(|_| json!([]));
        let source_label = if self.source == "pasted" { "Pasted".to_string() } else { self.attachment_kind.clone().unwrap_or_else(|| "File".into()) };
        let mut v = json!({
            "id": self.id,
            "person_id": self.person_id,
            "person_name": self.linked_name.clone().or_else(|| self.person_name.clone()),
            "name_as_written": self.person_name,
            "employee_code": self.employee_code,
            "client": self.client,
            "period_start": self.period_start,
            "period_end": self.period_end,
            "total_hours": self.total_hours,
            "status": self.status,
            "source": self.source,
            "source_label": source_label,
            "attachment_filename": self.attachment_filename,
            "match_method": self.match_method,
            "match_confidence": self.match_confidence,
            "needs_person": self.person_id.is_none(),
            "issues": issues,
            "replaces_id": self.replaces_id,
            "email": {
                "message_id": self.message_id,
                "available": self.message_id.is_some(),
                "subject": self.email_subject,
                "from": self.sender_address,
                "received_at": self.email_received_at.map(iso),
            },
            "created_at": iso(self.created_at),
            "updated_at": iso(self.updated_at),
        });
        if detail {
            v["entries"] = json!(self.entries());
            v["identity"] = serde_json::from_str(&self.identity_json).unwrap_or_else(|_| json!({}));
        }
        v
    }
}

#[derive(Debug, Default)]
pub struct Filter {
    pub person_id: Option<String>,
    pub month: Option<String>,
    pub status: Option<String>,
    pub q: Option<String>,
}

fn visible(a: &Access, r: &RecordRow) -> bool {
    match &r.person_id {
        Some(p) => a.can_see(p),
        None => a.visible().is_none(),
    }
}

pub async fn list(db: &SqlitePool, ws: &str, f: &Filter, a: &Access) -> sqlx::Result<Vec<Value>> {
    let bounds = f.month.as_deref().and_then(parse::month_dates);
    let like = f.q.as_deref().map(str::trim).filter(|q| !q.is_empty()).map(|q| format!("%{}%", q.replace(['%', '_'], "")));
    let rows: Vec<RecordRow> = sqlx::query_as(&format!(
        "{SELECT} WHERE r.workspace_id = ?1 AND (?2 IS NULL OR r.person_id = ?2) AND (?3 IS NULL OR r.status = ?3)
           AND (?4 IS NULL OR (r.period_start <= ?5 AND r.period_end >= ?4))
           AND (?6 IS NULL OR r.person_name LIKE ?6 OR p.name LIKE ?6 OR r.employee_code LIKE ?6 OR r.client LIKE ?6
                OR r.email_subject LIKE ?6 OR r.attachment_filename LIKE ?6 OR r.sender_address LIKE ?6)
           AND (?7 IS NULL OR r.person_id IN (SELECT value FROM json_each(?7)))
         ORDER BY r.period_start DESC, r.created_at DESC LIMIT 500"
    ))
    .bind(ws)
    .bind(&f.person_id)
    .bind(&f.status)
    .bind(bounds.as_ref().map(|b| b.0.clone()))
    .bind(bounds.as_ref().map(|b| b.1.clone()))
    .bind(like)
    .bind(a.visible().map(|v| json!(v).to_string()))
    .fetch_all(db)
    .await?;
    Ok(rows.iter().filter(|r| visible(a, r)).map(|r| r.json(false)).collect())
}

pub async fn get(db: &SqlitePool, ws: &str, id: &str) -> sqlx::Result<Option<RecordRow>> {
    sqlx::query_as(&format!("{SELECT} WHERE r.workspace_id = ?1 AND r.id = ?2")).bind(ws).bind(id).fetch_optional(db).await
}

pub fn can_see_record(a: &Access, r: &RecordRow) -> bool {
    visible(a, r)
}

/// The records read from one email, for the reader's chip and the message detail.
pub async fn for_message(db: &SqlitePool, ws: &str, message_id: &str) -> sqlx::Result<Vec<Value>> {
    let rows: Vec<RecordRow> =
        sqlx::query_as(&format!("{SELECT} WHERE r.workspace_id = ?1 AND r.message_id = ?2 AND r.status != 'replaced' ORDER BY r.created_at")).bind(ws).bind(message_id).fetch_all(db).await?;
    Ok(rows.iter().map(|r| r.json(false)).collect())
}

/// Is a person needed? `None`: nothing recorded for this email.
pub async fn attention(db: &SqlitePool, ws: &str, message_id: &str) -> sqlx::Result<Option<bool>> {
    let rows: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT status, person_id FROM timesheet_records WHERE workspace_id = ?1 AND message_id = ?2 AND status != 'replaced'").bind(ws).bind(message_id).fetch_all(db).await?;
    if rows.is_empty() {
        return Ok(None);
    }
    Ok(Some(rows.iter().any(|(status, person)| status == "needs_check" || person.is_none())))
}

/// Mail the timesheet desk has taken: it is labelled `timesheet` now and a record with real hours was read from it.
/// No automatic reply is drafted for it; a person can still ask for a draft by hand. Relabelled mail, and a record
/// with nothing in it (no hours found) get the normal replies again. A timesheet file that could not be read is
/// still the desk's: a person is asked to look at it.
pub async fn suppresses_auto_reply(db: &SqlitePool, ws: &str, message_id: &str) -> sqlx::Result<bool> {
    let labelled: Option<String> = sqlx::query_scalar("SELECT category FROM classifications WHERE message_id = ?1").bind(message_id).fetch_optional(db).await?;
    if labelled.as_deref() != Some(LABEL) {
        return Ok(false);
    }
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM timesheet_records WHERE workspace_id = ?1 AND message_id = ?2 AND (total_hours IS NOT NULL OR entries_json != '[]' OR issues_json LIKE '%\"unreadable\"%'))",
    )
    .bind(ws)
    .bind(message_id)
    .fetch_one(db)
    .await
}

// ---------------------------------------------------------------- link to a person

pub async fn link_person(db: &SqlitePool, ws: &str, record_id: &str, person_id: &str) -> sqlx::Result<bool> {
    let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
    let person: Option<String> = sqlx::query_scalar("SELECT name FROM people WHERE id = ?1 AND workspace_id = ?2").bind(person_id).bind(ws).fetch_optional(&mut *tx).await?;
    let rec: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT employee_code, person_name FROM timesheet_records WHERE id = ?1 AND workspace_id = ?2").bind(record_id).bind(ws).fetch_optional(&mut *tx).await?;
    let (Some(_), Some((code, name))) = (person, rec) else { return Ok(false) };
    let now = now_ms();
    if let Some(code) = code.as_deref().map(str::to_lowercase) {
        remember(&mut tx, ws, "code", &code, person_id, now).await?;
    }
    let key = name.as_deref().map(name_key).filter(|k| !k.is_empty());
    if let Some(key) = &key {
        remember(&mut tx, ws, "name", key, person_id, now).await?;
    }
    let others: Vec<(String, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT id, employee_code, person_name FROM timesheet_records WHERE workspace_id = ?1 AND person_id IS NULL ORDER BY created_at, id").bind(ws).fetch_all(&mut *tx).await?;
    for (id, other_code, other_name) in others {
        let same = id == record_id
            || (code.is_some() && other_code.as_deref().map(str::to_lowercase) == code.as_deref().map(str::to_lowercase))
            || (key.is_some() && other_name.as_deref().map(name_key) == key);
        if same {
            attach_person(&mut tx, ws, &id, person_id, now).await?;
        }
    }
    tx.commit().await?;
    Ok(true)
}

async fn remember(conn: &mut SqliteConnection, ws: &str, kind: &str, value: &str, person_id: &str, now: i64) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO timesheet_aliases (workspace_id, kind, value, person_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(workspace_id, kind, value) DO UPDATE SET person_id = excluded.person_id, created_at = excluded.created_at",
    )
    .bind(ws)
    .bind(kind)
    .bind(value)
    .bind(person_id)
    .bind(now)
    .execute(conn)
    .await?;
    Ok(())
}

/// Gives an unmatched record its person: clears "who is this", settles the status and applies the replace rule.
async fn attach_person(conn: &mut SqliteConnection, ws: &str, id: &str, person_id: &str, now: i64) -> sqlx::Result<()> {
    type Own = (String, String, String, Option<i64>, Option<String>, Option<String>);
    let (issues_json, start, end, received, message_id, client): Own = sqlx::query_as(
        "SELECT issues_json, period_start, period_end, email_received_at, message_id, client FROM timesheet_records WHERE id = ?1 AND workspace_id = ?2",
    )
    .bind(id)
    .bind(ws)
    .fetch_one(&mut *conn)
    .await?;
    let mut issues: Vec<Value> = serde_json::from_str(&issues_json).unwrap_or_default();
    issues.retain(|i| i["code"] != "person_unmatched" && i["code"] != "ambiguous_person" && i["code"] != "possible_duplicate");
    let assumed = issues.iter().any(|i| i["code"] == "period_assumed");
    let me = Placed { id, message_id: message_id.as_deref(), client: client.as_deref(), start: &start, end: &end, received: received.unwrap_or(0), assumed };
    let settled = settle(&mut *conn, ws, person_id, &me, now).await?;
    if let Some((a, b)) = &settled.duplicate_of {
        issues.push(json!(duplicate_issue(a, b)));
    }
    let status = if settled.superseded {
        "replaced"
    } else if issues.iter().any(|i| i["severity"] == CHECK) {
        "needs_check"
    } else {
        "recorded"
    };
    sqlx::query(
        "UPDATE timesheet_records SET person_id = ?1, match_method = 'manual', match_confidence = 1, issues_json = ?2, status = ?3,
                replaces_id = COALESCE(?4, replaces_id), updated_at = ?5 WHERE id = ?6 AND workspace_id = ?7",
    )
    .bind(person_id)
    .bind(serde_json::to_string(&issues).unwrap_or_else(|_| "[]".into()))
    .bind(status)
    .bind(settled.replaced.first())
    .bind(now)
    .bind(id)
    .bind(ws)
    .execute(conn)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------- the team grid

#[derive(sqlx::FromRow)]
struct GridPerson {
    id: String,
    name: String,
    employee_code: Option<String>,
    client: Option<String>,
    employee_type: String,
    joining_date: Option<String>,
}

fn expected_people_sql() -> &'static str {
    "SELECT id, name, employee_code, client, employee_type, joining_date FROM people WHERE workspace_id = ?1 AND active = 1 ORDER BY lower(name), id"
}

/// People by days for a month: one row per expected person (and per unmatched sheet), hours per day, a total and
/// a status. Replaced records never count.
pub async fn grid(db: &SqlitePool, ws: &str, month: &str, now: i64, a: &Access) -> sqlx::Result<Value> {
    let cfg = settings::load(db, ws).await?;
    let Some((first, last)) = parse::month_dates(month) else { return Ok(json!({ "month": month, "days": [], "rows": [] })) };
    let (d0, d1) = (clock::parse_date(&first).unwrap_or(0), clock::parse_date(&last).unwrap_or(0));
    let days: Vec<Value> = (d0..=d1)
        .map(|d| {
            let w = (d + 3).rem_euclid(7);
            json!({ "date": clock::date_string(d), "weekday": weekday_name(w), "weekend": w >= 5 })
        })
        .collect();
    let people: Vec<GridPerson> = sqlx::query_as(expected_people_sql()).bind(ws).fetch_all(db).await?;
    let any_billable = people.iter().any(|p| p.employee_type == "billable");
    let records: Vec<RecordRow> = sqlx::query_as(&format!(
        "{SELECT} WHERE r.workspace_id = ?1 AND r.status != 'replaced' AND r.period_start <= ?3 AND r.period_end >= ?2 ORDER BY r.period_start, r.created_at"
    ))
    .bind(ws)
    .bind(&first)
    .bind(&last)
    .fetch_all(db)
    .await?;
    let due_at = clock::month_bounds(month, cfg.tz_offset_minutes).map_or(i64::MAX, |(_, end)| end + cfg.timesheet_grace_days * clock::DAY_MS);

    #[derive(Default)]
    struct Row {
        person_id: Option<String>,
        name: String,
        code: Option<String>,
        client: Option<String>,
        cells: HashMap<String, f64>,
        undated: f64,
        records: Vec<String>,
        check: bool,
    }
    let mut rows: Vec<Row> = vec![];
    let mut index: HashMap<String, usize> = HashMap::new();
    for p in &people {
        let started = p.joining_date.as_deref().is_none_or(|j| j.get(..7).unwrap_or(j) <= month);
        if !a.can_see(&p.id) {
            continue;
        }
        let has_record = records.iter().any(|r| r.person_id.as_deref() == Some(p.id.as_str()));
        if (any_billable && p.employee_type != "billable" && !has_record) || (!started && !has_record) {
            continue;
        }
        index.insert(p.id.clone(), rows.len());
        rows.push(Row { person_id: Some(p.id.clone()), name: p.name.clone(), code: p.employee_code.clone(), client: p.client.clone(), ..Default::default() });
    }
    for r in &records {
        if !visible(a, r) {
            continue;
        }
        let i = match r.person_id.as_ref().and_then(|p| index.get(p)) {
            Some(i) => *i,
            None => {
                rows.push(Row { person_id: r.person_id.clone(), name: r.linked_name.clone().or_else(|| r.person_name.clone()).unwrap_or_else(|| "Unknown".into()), code: r.employee_code.clone(), client: r.client.clone(), ..Default::default() });
                if let Some(p) = &r.person_id {
                    index.insert(p.clone(), rows.len() - 1);
                }
                rows.len() - 1
            }
        };
        let row = &mut rows[i];
        row.client = row.client.take().or_else(|| r.client.clone()).or_else(|| r.entries().iter().find_map(|e| e.project.clone()));
        row.records.push(r.id.clone());
        row.check |= r.status == "needs_check" || r.person_id.is_none();
        let entries = r.entries();
        // A sheet that spans months counts its undated hours once, in the month it starts.
        let starts_here = r.period_start.as_str() >= first.as_str() && r.period_start.as_str() <= last.as_str();
        if entries.is_empty() && starts_here {
            row.undated += r.total_hours.unwrap_or(0.0);
        }
        for e in entries {
            match e.date {
                Some(d) if d.as_str() >= first.as_str() && d.as_str() <= last.as_str() => *row.cells.entry(d).or_default() += e.hours,
                Some(_) => {}
                None if starts_here => row.undated += e.hours,
                None => {}
            }
        }
    }
    let (mut recorded, mut needs_check, mut missing) = (0, 0, 0);
    let out: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            let status = if r.records.is_empty() {
                if now >= due_at { "missing" } else { "pending" }
            } else if r.check {
                "needs_check"
            } else {
                "recorded"
            };
            match status {
                "recorded" => recorded += 1,
                "needs_check" => needs_check += 1,
                "missing" => missing += 1,
                _ => {}
            }
            let day_sum: f64 = r.cells.values().sum();
            let total = (day_sum + r.undated) * 100.0;
            json!({
                "person_id": r.person_id, "name": r.name, "employee_code": r.code, "client": r.client,
                "cells": r.cells, "total": (total.round() / 100.0), "status": status, "record_ids": r.records,
            })
        })
        .collect();
    Ok(json!({
        "month": month, "days": days, "rows": out, "due_at": iso(due_at.min(i64::MAX / 2)),
        "summary": { "recorded": recorded, "needs_check": needs_check, "missing": missing },
    }))
}

// ---------------------------------------------------------------- backfill

/// A mail the scan may try this many times: after that it is left for a person (or a new label) to bring back.
const MAX_ATTEMPTS: i64 = 3;

/// Records timesheets from mail labelled `timesheet` that has none yet (oldest first, a bounded batch per call).
/// Safe to repeat: mail that already has a record is skipped, and mail that gave nothing is tried at most
/// `MAX_ATTEMPTS` times so a model is not paid for the same mail every scan.
pub async fn backfill(state: &AppState, ws: &str, limit: i64) -> sqlx::Result<usize> {
    if !packs::enabled(&state.db, ws, packs::STAFFING).await? {
        return Ok(0);
    }
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT m.id FROM email_messages m JOIN classifications c ON c.message_id = m.id
         WHERE m.workspace_id = ?1 AND m.direction = 'inbound' AND m.suppressed = 0 AND c.category = ?2
           AND NOT EXISTS (SELECT 1 FROM timesheet_records r WHERE r.message_id = m.id)
           AND COALESCE((SELECT a.attempts FROM timesheet_attempts a WHERE a.message_id = m.id), 0) < ?4
         ORDER BY m.received_at, m.id LIMIT ?3",
    )
    .bind(ws)
    .bind(LABEL)
    .bind(limit)
    .bind(MAX_ATTEMPTS)
    .fetch_all(&state.db)
    .await?;
    let mut recorded = 0;
    for id in ids {
        let run = record_message(state, ws, &id).await?;
        let made = run.records.iter().filter(|r| r.created).count();
        recorded += made;
        if made == 0 && !run.waiting_for_files {
            sqlx::query(
                "INSERT INTO timesheet_attempts (message_id, workspace_id, attempts, last_at) VALUES (?1, ?2, 1, ?3)
                 ON CONFLICT(message_id) DO UPDATE SET attempts = attempts + 1, last_at = excluded.last_at",
            )
            .bind(&id)
            .bind(ws)
            .bind(now_ms())
            .execute(&state.db)
            .await?;
        }
    }
    Ok(recorded)
}

/// Forgetting a sender erases what was kept of their timesheets: the records and the code / name links a person
/// made for them. (Deleting an email or disconnecting a mailbox does not call this: the records are kept on purpose.)
pub async fn erase_sender(conn: &mut SqliteConnection, ws: &str, sender: &str) -> sqlx::Result<u64> {
    let address = address_of(sender);
    let who: Vec<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT employee_code, person_name FROM timesheet_records WHERE workspace_id = ?1 AND lower(sender_address) = lower(?2)").bind(ws).bind(&address).fetch_all(&mut *conn).await?;
    for (code, name) in who {
        if let Some(code) = code.map(|c| c.to_lowercase()) {
            sqlx::query("DELETE FROM timesheet_aliases WHERE workspace_id = ?1 AND kind = 'code' AND value = ?2").bind(ws).bind(code).execute(&mut *conn).await?;
        }
        if let Some(key) = name.as_deref().map(name_key).filter(|k| !k.is_empty()) {
            sqlx::query("DELETE FROM timesheet_aliases WHERE workspace_id = ?1 AND kind = 'name' AND value = ?2").bind(ws).bind(key).execute(&mut *conn).await?;
        }
    }
    Ok(sqlx::query("DELETE FROM timesheet_records WHERE workspace_id = ?1 AND lower(sender_address) = lower(?2)").bind(ws).bind(&address).execute(conn).await?.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hr::staffing::timesheet_parse::Identity;

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

    fn sheet(entries: &[(&str, f64)], total: Option<f64>) -> Sheet {
        Sheet {
            identity: Identity::default(),
            period: None,
            total,
            entries: entries.iter().map(|(d, h)| Entry { date: Some((*d).into()), hours: *h, project: None, note: None }).collect(),
        }
    }

    fn period() -> (String, String) {
        ("2026-09-01".into(), "2026-09-30".into())
    }

    #[test]
    fn days_that_do_not_add_up_need_a_check() {
        let s = sheet(&[("2026-09-01", 8.0), ("2026-09-02", 8.0)], Some(20.0));
        let (total, issues) = assess(&s, &period(), false, 0.5, true, "x");
        assert_eq!(total, Some(20.0));
        assert!(issues.iter().any(|i| i.code == "total_mismatch" && i.severity == CHECK));
        let ok = sheet(&[("2026-09-01", 8.0), ("2026-09-02", 8.0)], Some(16.0));
        assert!(!assess(&ok, &period(), false, 0.5, true, "x").1.iter().any(|i| i.severity == CHECK));
    }

    #[test]
    fn a_missing_total_is_added_up_with_a_note_and_nothing_is_invented() {
        let s = sheet(&[("2026-09-01", 8.0), ("2026-09-02", 7.5)], None);
        let (total, issues) = assess(&s, &period(), false, 0.5, true, "x");
        assert_eq!(total, Some(15.5));
        assert!(issues.iter().any(|i| i.code == "total_added_up" && i.severity == NOTE));
        let (total, issues) = assess(&Sheet::default(), &period(), true, 0.5, true, "x");
        assert_eq!(total, None);
        assert!(issues.iter().any(|i| i.code == "no_hours" && i.severity == CHECK));
    }

    #[test]
    fn weekends_long_days_and_missing_weekdays_are_noted_not_blocking() {
        let s = sheet(&[("2026-09-01", 8.0), ("2026-09-02", 13.0), ("2026-09-03", 8.0), ("2026-09-05", 4.0), ("2026-09-04", 8.0)], Some(41.0));
        let (_, issues) = assess(&s, &period(), false, 0.5, true, "x");
        let codes: Vec<&str> = issues.iter().map(|i| i.code.as_str()).collect();
        assert!(codes.contains(&"weekend_hours") && codes.contains(&"long_days") && codes.contains(&"missing_days"), "{codes:?}");
        assert!(issues.iter().all(|i| i.severity == NOTE));
    }

    #[test]
    fn an_unmatched_person_is_a_question_for_a_person() {
        let s = sheet(&[("2026-09-01", 8.0)], Some(8.0));
        let (_, issues) = assess(&s, &period(), false, 0.5, false, "Ravi Kumar, ID E1");
        let q = issues.iter().find(|i| i.code == "person_unmatched").unwrap();
        assert_eq!((q.severity, q.message.as_str()), (CHECK, "Who is this? (Ravi Kumar, ID E1)"));
    }

    fn entry(date: &str, hours: f64) -> Entry {
        Entry { date: Some(date.into()), hours, project: None, note: None }
    }

    #[test]
    fn a_model_cannot_add_numbers_the_sheet_does_not_state() {
        let text = "Ravi Kumar\n01 Sep 8\n02 Sep 8\nTotal 16";
        let model = Sheet {
            identity: Identity { name: Some("Ravi Kumar".into()), code: Some("E999".into()), ..Default::default() },
            period: None,
            total: Some(18.0),
            entries: vec![entry("2026-09-01", 8.0), entry("2026-09-02", 9.5)],
        };
        let (s, issues) = sanitize_model(model, text, "Timesheet", 2026);
        assert_eq!(s.entries.len(), 1);
        assert_eq!(s.total, None);
        assert_eq!(s.identity.code, None);
        assert_eq!(s.identity.name.as_deref(), Some("Ravi Kumar"));
        assert!(issues.iter().any(|i| i.code == "model_days_dropped" && i.severity == CHECK));
    }

    #[test]
    fn a_model_day_needs_its_date_and_its_hours_on_one_line_in_any_date_format() {
        let text = "Name: Ravi\n3 Sep 2026   8\n2026-09-04 | 6\n05/09/2026 | 7.5\nSep 8, 2026: 4\n1 Sep 16\nHours: 8 on one day";
        let model = Sheet {
            entries: vec![
                entry("2026-09-03", 8.0),
                entry("2026-09-04", 6.0),
                entry("2026-09-05", 7.5),
                entry("2026-09-08", 4.0),
                entry("2026-09-01", 16.0),
                entry("2026-09-04", 8.0),
                entry("2026-09-20", 6.0),
            ],
            ..Default::default()
        };
        let kept: Vec<(String, f64)> = sanitize_model(model, text, "Timesheet", 2026).0.entries.into_iter().map(|e| (e.date.unwrap(), e.hours)).collect();
        let want: Vec<(String, f64)> =
            [("2026-09-03", 8.0), ("2026-09-04", 6.0), ("2026-09-05", 7.5), ("2026-09-08", 4.0), ("2026-09-01", 16.0)].iter().map(|(d, h)| ((*d).to_string(), *h)).collect();
        assert_eq!(kept, want, "8h on 4 Sep is on another line, 20 Sep is nowhere");
    }

    #[test]
    fn a_model_period_must_be_written_in_the_text() {
        let model = |p: (&str, &str)| Sheet { period: Some((p.0.into(), p.1.into())), ..Default::default() };
        let text = "Timesheet 1 Sep 2026 to 30 Sep 2026\n1 Sep 8";
        assert!(sanitize_model(model(("2026-09-01", "2026-09-30")), text, "x", 2026).0.period.is_some());
        assert!(sanitize_model(model(("2026-08-01", "2026-08-31")), text, "x", 2026).0.period.is_none());
        assert!(sanitize_model(model(("2026-09-01", "2026-09-30")), "Hours for the month\n1 Sep 8", "September timesheet", 2026).0.period.is_some());
        assert!(sanitize_model(model(("2026-09-01", "2026-09-30")), "1 Sep 8", "Hello", 2026).0.period.is_none(), "a month nobody named");
    }

    #[test]
    fn a_rule_total_is_never_replaced_by_the_model_total() {
        let base = Sheet { total: Some(24.0), ..Default::default() };
        let model = Sheet { total: Some(30.0), entries: vec![entry("2026-09-01", 8.0)], ..Default::default() };
        assert_eq!(merge(base, model).total, Some(24.0));
    }

    #[test]
    fn replies_and_forwards_keep_only_what_the_sender_wrote() {
        let table = "| Date | Hours |\n| 2026-09-01 | 8 |";
        assert_eq!(own_text("Re: timesheet", &format!("Thanks!\n\nOn Mon, 5 Oct 2026 at 10:00, Ravi <r@x.test> wrote:\n> {table}")).trim(), "Thanks!");
        assert_eq!(own_text("Re: x", "Looks right.\n> | 2026-09-01 | 8 |").trim(), "Looks right.");
        let fwd = own_text("Fwd: Sept timesheet", &format!("FYI\n\n---------- Forwarded message ---------\nFrom: Ravi\n\n{table}"));
        assert!(fwd.contains("FYI") && fwd.contains("2026-09-01 | 8"));
        let long_note = format!("{}\n\n---------- Forwarded message ---------\n{table}", "Please read this carefully. ".repeat(20));
        assert!(!own_text("Fwd: x", &long_note).contains("2026-09-01"));
        let wrapped = own_text("Re: x", "ok\nOn Mon, 5 Oct 2026 at 10:00, Ravi Kumar <ravi@x.test>\nwrote:\n2026-09-01 | 8");
        assert_eq!(wrapped.trim(), "ok");
    }
}
