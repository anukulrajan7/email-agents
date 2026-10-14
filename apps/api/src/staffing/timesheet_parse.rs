//! Reads a timesheet out of text without a model: who it is for (employee id, name, email, client, manager,
//! location as written), the period, a stated total and per-day rows. Spreadsheets arrive as markdown tables
//! (see `attachments::sheet`); pasted mail has tables with tabs or `|`, "Key: value" lines or one line per day.
//! Nothing is invented: what the text does not say stays `None`.
//!
//! The second half matches the identity to a person in the directory: employee id, then email, then name.

use std::{collections::HashMap, sync::LazyLock};

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::{name_key, policy::MONTHS};
use crate::team::clock;

pub const MAX_ENTRIES: usize = 400;
pub const MONTH_NAMES: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub date: Option<String>,
    pub hours: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Identity fields exactly as written in the sheet or mail.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Identity {
    pub code: Option<String>,
    pub name: Option<String>,
    pub email: Option<String>,
    pub client: Option<String>,
    pub manager: Option<String>,
    pub location: Option<String>,
}

impl Identity {
    pub fn is_empty(&self) -> bool {
        *self == Identity::default()
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sheet {
    pub identity: Identity,
    pub period: Option<(String, String)>,
    pub total: Option<f64>,
    pub entries: Vec<Entry>,
}

impl Sheet {
    pub fn has_data(&self) -> bool {
        self.total.is_some() || !self.entries.is_empty()
    }
}

// ---------------------------------------------------------------- dates and hours

fn month_number(word: &str) -> Option<u32> {
    let w = word.trim_end_matches('.').to_lowercase();
    MONTHS.iter().position(|(short, long)| w == *long || w == *short || (w == "sept" && *short == "sep")).map(|i| i as u32 + 1)
}

fn ymd(y: i64, m: u32, d: u32) -> Option<String> {
    let s = format!("{y:04}-{m:02}-{d:02}");
    clock::parse_date(&s).map(|_| s)
}

fn full_year(y: i64) -> i64 {
    if y < 100 { 2000 + y } else { y }
}

static ISO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d{4})-(\d{1,2})-(\d{1,2})(?:[T ].*)?$").expect("static regex"));
static NUMERIC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d{1,2})[/.\-](\d{1,2})[/.\-](\d{4}|\d{2})$").expect("static regex"));
static DAY_MONTH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d{1,2})(?:st|nd|rd|th)?[\s\-]+([A-Za-z]{3,9})\.?,?[\s\-]*(\d{4}|\d{2})?$").expect("static regex"));
static MONTH_DAY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Za-z]{3,9})\.?\s+(\d{1,2})(?:st|nd|rd|th)?,?\s*(\d{4})?$").expect("static regex"));
static WEEKDAY_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?:mon|tue|tues|wed|thu|thur|thurs|fri|sat|sun)[a-z]*\.?,?\s+").expect("static regex"));
static WEEKDAY_SUFFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s*\((?:mon|tue|wed|thu|fri|sat|sun)[a-z]*\)$").expect("static regex"));

/// `YYYY-MM-DD` from the usual ways to write a date: 2026-09-03, 03/09/2026 (day first), 3 Sep 2026, Sep 3, 03-Sep-26.
/// A date without a year takes `hint_year`.
pub fn parse_date_loose(raw: &str, hint_year: i64) -> Option<String> {
    let s = raw.trim().trim_matches('*');
    let s = WEEKDAY_SUFFIX.replace(s, "");
    let s = WEEKDAY_PREFIX.replace(&s, "");
    let s = s.trim();
    if let Some(c) = ISO.captures(s) {
        return ymd(c[1].parse().ok()?, c[2].parse().ok()?, c[3].parse().ok()?);
    }
    if let Some(c) = NUMERIC.captures(s) {
        let (a, b): (u32, u32) = (c[1].parse().ok()?, c[2].parse().ok()?);
        let y = full_year(c[3].parse().ok()?);
        return if b > 12 && a <= 12 { ymd(y, a, b) } else { ymd(y, b, a) };
    }
    if let Some(c) = DAY_MONTH.captures(s) {
        let m = month_number(&c[2])?;
        let y = c.get(3).and_then(|y| y.as_str().parse().ok()).map_or(hint_year, full_year);
        return ymd(y, m, c[1].parse().ok()?);
    }
    if let Some(c) = MONTH_DAY.captures(s) {
        let m = month_number(&c[1])?;
        let y = c.get(3).and_then(|y| y.as_str().parse().ok()).map_or(hint_year, full_year);
        return ymd(y, m, c[2].parse().ok()?);
    }
    None
}

static HOURS_PLAIN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d{1,4}(?:\.\d+)?)$").expect("static regex"));
static HOURS_CLOCK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d{1,3}):([0-5]\d)(?::[0-5]\d)?$").expect("static regex"));
static HOURS_UNIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(\d{1,4}(?:\.\d+)?)\s*(?:h|hr|hrs|hours?)$").expect("static regex"));
static HOURS_HM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(\d{1,3})\s*h(?:rs?|ours?)?\s*(\d{1,2})\s*m(?:ins?|inutes?)?$").expect("static regex"));

/// Hours in a cell: 8, 7.5, 8:30, 8h, 8 hrs, 8h 30m.
pub fn parse_hours_cell(raw: &str) -> Option<f64> {
    let s = raw.trim().trim_matches('*');
    if let Some(c) = HOURS_PLAIN.captures(s).or_else(|| HOURS_UNIT.captures(s)) {
        return c[1].parse().ok();
    }
    if let Some(c) = HOURS_CLOCK.captures(s).or_else(|| HOURS_HM.captures(s)) {
        let (h, m): (f64, f64) = (c[1].parse().ok()?, c[2].parse().ok()?);
        return Some(h + m / 60.0);
    }
    None
}

static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+(?:\.\d+)?").expect("static regex"));
static CLOCK_IN_TEXT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(\d{1,3}):([0-5]\d)\b").expect("static regex"));

/// Every number the text states, plus h:mm times as hours. A model's numbers must come from here.
pub fn numbers_in(text: &str) -> Vec<f64> {
    let mut out: Vec<f64> = NUMBER.find_iter(text).filter_map(|m| m.as_str().parse().ok()).collect();
    out.extend(CLOCK_IN_TEXT.captures_iter(text).filter_map(|c| Some(c[1].parse::<f64>().ok()? + c[2].parse::<f64>().ok()? / 60.0)));
    out
}

pub fn same_hours(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.05
}

// ---------------------------------------------------------------- identity

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Code,
    Name,
    Email,
    Client,
    Manager,
    Location,
}

fn field_of(key: &str) -> Option<Field> {
    let k: String = key.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
    Some(match k.as_str() {
        "employee id" | "emp id" | "employee code" | "emp code" | "employee no" | "employee number" | "emp no" | "emp number" | "user id" | "userid" | "staff id"
        | "associate id" | "resource id" | "employee identifier" | "id" => Field::Code,
        "employee name" | "emp name" | "name" | "full name" | "resource name" | "consultant name" | "consultant" | "employee" | "username" | "user name"
        | "associate name" | "resource" | "staff name" => Field::Name,
        "email" | "e mail" | "email id" | "email address" | "mail id" | "official email" => Field::Email,
        "client" | "client name" | "project" | "project name" | "customer" | "customer name" | "account" | "project client" => Field::Client,
        "manager" | "manager name" | "approver" | "approver name" | "approved by" | "reporting manager" | "supervisor" | "reports to" | "line manager" => Field::Manager,
        "location" | "work location" | "office" | "city" | "base location" | "site" => Field::Location,
        _ => return None,
    })
}

static EMAIL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9\-]+(?:\.[A-Za-z0-9\-]+)+").expect("static regex"));
static KEY_VALUE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\s*_\-•>]*([A-Za-z][A-Za-z .\-/_]{1,30}?)\s*[:=]\s*(\S.*?)\s*$").expect("static regex"));
static CELL_GAP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\t|\s{3,}").expect("static regex"));

const HEADER_WORDS: &[&str] = &[
    "date", "day", "hours", "hrs", "project", "task", "total", "description", "notes", "client", "activity", "duration", "remarks", "comments", "details",
    "hours worked", "worked hours", "work date",
];

fn clean_value(raw: &str) -> String {
    raw.trim().trim_matches(|c| c == '*' || c == '`' || c == '|' || c == '_').trim().to_string()
}

fn name_like(v: &str) -> bool {
    let n = v.chars().count();
    (2..=60).contains(&n) && v.chars().all(|c| c.is_alphabetic() || " .,'-_".contains(c)) && v.chars().any(char::is_alphabetic)
}

fn set_field(id: &mut Identity, field: Field, raw: &str) {
    let v = clean_value(raw);
    if v.is_empty() || v.chars().count() > 80 {
        return;
    }
    match field {
        Field::Code => {
            let ok = v.chars().count() <= 24 && !v.contains(' ') && v.chars().any(char::is_alphanumeric) && !v.contains('@');
            if ok && id.code.is_none() {
                id.code = Some(v);
            }
        }
        Field::Name => {
            if name_like(&v) && id.name.is_none() && !v.eq_ignore_ascii_case("total") {
                id.name = Some(v);
            }
        }
        Field::Email => {
            if id.email.is_none() {
                id.email = EMAIL.find(&v).map(|m| m.as_str().to_lowercase());
            }
        }
        Field::Client => {
            if id.client.is_none() {
                id.client = Some(v);
            }
        }
        Field::Manager => {
            if id.manager.is_none() {
                id.manager = Some(v);
            }
        }
        Field::Location => {
            if id.location.is_none() {
                id.location = Some(v);
            }
        }
    }
}

/// "Key: value" pairs on a line (several may share a line, separated by tabs, wide gaps or `|`).
fn read_identity_line(line: &str, id: &mut Identity) {
    let segments: Vec<&str> = if line.contains('|') { line.split('|').collect() } else { CELL_GAP.split(line).collect() };
    for seg in &segments {
        if let Some(c) = KEY_VALUE.captures(seg)
            && let Some(f) = field_of(&c[1])
        {
            set_field(id, f, &c[2]);
        }
    }
    if segments.len() >= 2 {
        let cells: Vec<String> = segments.iter().map(|s| clean_value(s)).filter(|s| !s.is_empty()).collect();
        let looks_like_header = cells.iter().all(|c| HEADER_WORDS.contains(&c.to_lowercase().as_str()));
        if !looks_like_header {
            for pair in cells.chunks(2) {
                if let [k, v] = pair
                    && let Some(f) = field_of(k)
                    && !HEADER_WORDS.contains(&v.to_lowercase().as_str())
                {
                    set_field(id, f, v);
                }
            }
        }
    }
}

// ---------------------------------------------------------------- tables

struct Header {
    date: Option<usize>,
    hours: usize,
    project: Option<usize>,
    note: Option<usize>,
}

fn split_cells(line: &str) -> Vec<String> {
    let parts: Vec<&str> = if line.contains('|') {
        line.trim().trim_start_matches('|').trim_end_matches('|').split('|').collect()
    } else {
        CELL_GAP.split(line.trim()).collect()
    };
    parts.into_iter().map(clean_value).collect()
}

fn is_separator(cells: &[String]) -> bool {
    !cells.is_empty() && cells.iter().all(|c| !c.is_empty() && c.chars().all(|ch| ch == '-' || ch == ':' || ch == ' '))
}

fn header_of(cells: &[String]) -> Option<Header> {
    const DATE: &[&str] = &["date", "day", "work date", "date worked", "working date", "dates"];
    const HOURS: &[&str] = &[
        "hours", "hrs", "hours worked", "worked hours", "total hours", "duration", "time spent", "hours logged", "logged hours", "effort", "working hours",
        "no of hours", "hrs worked", "billable hours", "actual hours", "hours spent",
    ];
    const PROJECT: &[&str] = &["project", "client", "project name", "project/task", "customer"];
    const NOTE: &[&str] = &["task", "activity", "description", "work done", "notes", "remarks", "comments", "details", "task description"];
    let lower: Vec<String> = cells.iter().map(|c| c.to_lowercase().replace('.', "")).collect();
    let find = |names: &[&str]| lower.iter().position(|c| names.contains(&c.as_str()));
    let hours = find(HOURS)?;
    Some(Header { date: find(DATE), hours, project: find(PROJECT), note: find(NOTE) })
}

fn cell(cells: &[String], i: Option<usize>) -> Option<String> {
    i.and_then(|i| cells.get(i)).map(|c| c.trim().to_string()).filter(|c| !c.is_empty() && c != "-")
}

fn short(v: String) -> String {
    crate::sources::normalize::truncate_chars(v.trim(), 120)
}

static DATE_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^[\s*•\-]*((?:\d{4}-\d{1,2}-\d{1,2})|(?:\d{1,2}[/.\-]\d{1,2}[/.\-]\d{2,4})|(?:\d{1,2}(?:st|nd|rd|th)?[\s\-]+[A-Za-z]{3,9}\.?(?:,?[\s\-]+\d{4})?)|(?:[A-Za-z]{3,9}\.?\s+\d{1,2}(?:st|nd|rd|th)?(?:,?\s+\d{4})?))\s*(?:\([A-Za-z]+\))?\s*[:\-–—]\s*(\d{1,2}(?:\.\d+)?(?::\d{2})?)\s*(?:h|hr|hrs|hours?)?\b[\s,;\-–—:]*(.*)$",
    )
    .expect("static regex")
});

static RANGE: LazyLock<Regex> = LazyLock::new(|| {
    let d = r"(\d{4}-\d{1,2}-\d{1,2}|\d{1,2}[/.\-]\d{1,2}[/.\-]\d{2,4}|\d{1,2}(?:st|nd|rd|th)?[\s\-]+[A-Za-z]{3,9}\.?,?[\s\-]*\d{0,4}|[A-Za-z]{3,9}\.?\s+\d{1,2}(?:st|nd|rd|th)?,?\s*\d{0,4})";
    Regex::new(&format!(r"(?i){d}\s*(?:to|till|until|through|thru|–|—|-)\s*{d}")).expect("static regex")
});
static WEEK_ENDING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)week\s*(?:ending|ended|end)\s*[:\-]?\s*(\d{4}-\d{1,2}-\d{1,2}|\d{1,2}[/.\-]\d{1,2}[/.\-]\d{2,4}|\d{1,2}(?:st|nd|rd|th)?[\s\-]+[A-Za-z]{3,9}\.?,?[\s\-]*\d{0,4}|[A-Za-z]{3,9}\.?\s+\d{1,2}(?:st|nd|rd|th)?,?\s*\d{0,4})")
        .expect("static regex")
});
static MONTH_YEAR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(jan(?:uary)?|feb(?:ruary)?|mar(?:ch)?|apr(?:il)?|may|june?|july?|aug(?:ust)?|sept?(?:ember)?|oct(?:ober)?|nov(?:ember)?|dec(?:ember)?)\b[\s,'\-]*((?:20)?\d{2})?\b")
        .expect("static regex")
});

fn month_range(y: i64, m: u32) -> Option<(String, String)> {
    let start = ymd(y, m, 1)?;
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    let last = clock::parse_date(&ymd(ny, nm, 1)?)? - 1;
    Some((start, clock::date_string(last)))
}

/// First and last day of `YYYY-MM`.
pub fn month_dates(month: &str) -> Option<(String, String)> {
    let (y, m) = month.split_once('-')?;
    month_range(y.parse().ok()?, m.parse().ok()?)
}

/// 0 = Monday ... 6 = Sunday.
pub fn weekday(date: &str) -> Option<i64> {
    clock::parse_date(date).map(|d| (d + 3).rem_euclid(7))
}

/// A month named with a "month / period / timesheet / for" cue on the same line, or anywhere in `subject`.
fn month_named(text: &str, subject: &str, hint_year: i64) -> Option<(String, String)> {
    let from = |s: &str| -> Option<(String, String)> {
        let c = MONTH_YEAR.captures(s)?;
        let m = month_number(&c[1])?;
        let y = c.get(2).and_then(|y| y.as_str().parse::<i64>().ok()).map_or(hint_year, full_year);
        month_range(y, m)
    };
    from(subject).or_else(|| {
        text.lines().take(60).filter(|l| {
            let l = l.to_lowercase();
            l.contains("month") || l.contains("period") || l.contains("timesheet") || l.starts_with("for ")
        }).find_map(from)
    })
}

fn explicit_period(text: &str, hint_year: i64) -> Option<(String, String)> {
    for line in text.lines().take(80) {
        if line.contains('|') && line.matches('|').count() > 3 {
            continue;
        }
        if let Some(c) = RANGE.captures(line) {
            let (a, b) = (parse_date_loose(c[1].trim(), hint_year), parse_date_loose(c[2].trim(), hint_year));
            if let (Some(a), Some(b)) = (a, b)
                && let (Some(da), Some(db)) = (clock::parse_date(&a), clock::parse_date(&b))
                && db >= da
                && db - da <= 62
            {
                return Some((a, b));
            }
        }
        if let Some(c) = WEEK_ENDING.captures(line)
            && let Some(end) = parse_date_loose(c[1].trim(), hint_year)
            && let Some(d) = clock::parse_date(&end)
        {
            return Some((clock::date_string(d - 6), end));
        }
    }
    None
}

static TOTAL_LABEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(?:grand\s+|sub\s*)?total\b").expect("static regex"));

/// Reads one text (the mail body or one attachment's text). `subject` and `hint_year` help to place a period.
pub fn parse_sheet(text: &str, subject: &str, hint_year: i64) -> Sheet {
    let mut sheet = Sheet::default();
    let mut header: Option<Header> = None;
    let mut table_total: Option<f64> = None;
    let mut totals: Vec<f64> = vec![];
    let mut flush = |table_total: &mut Option<f64>| {
        if let Some(t) = table_total.take() {
            totals.push(t);
        }
    };
    for raw in text.lines() {
        let line = raw.trim_start_matches(['>', ' ']).trim_end();
        if line.trim().is_empty() || line.starts_with("## Sheet:") || line.starts_with("_Showing") {
            header = None;
            flush(&mut table_total);
            continue;
        }
        let is_row = line.contains('|') || line.contains('\t') || CELL_GAP.is_match(line.trim());
        if is_row {
            let cells = split_cells(line);
            if is_separator(&cells) {
                continue;
            }
            if let Some(h) = &header {
                let label = cells.first().map(String::as_str).unwrap_or("");
                let is_total = TOTAL_LABEL.is_match(label) || cell(&cells, h.date).is_some_and(|d| TOTAL_LABEL.is_match(&d));
                if is_total {
                    let value = cell(&cells, Some(h.hours)).and_then(|c| parse_hours_cell(&c)).or_else(|| cells.iter().rev().find_map(|c| parse_hours_cell(c)));
                    if value.is_some() {
                        table_total = value;
                    }
                    continue;
                }
                let date = h.date.and_then(|i| cells.get(i)).and_then(|c| parse_date_loose(c, hint_year));
                let hours = cell(&cells, Some(h.hours)).and_then(|c| parse_hours_cell(&c));
                if let Some(hours) = hours
                    && (date.is_some() || h.date.is_none())
                    && sheet.entries.len() < MAX_ENTRIES
                {
                    let project = cell(&cells, h.project).map(short).or_else(|| (h.date.is_none()).then(|| cells.first().cloned()).flatten().filter(|c| parse_hours_cell(c).is_none()).map(short));
                    sheet.entries.push(Entry { date, hours, project, note: cell(&cells, h.note).map(short) });
                    continue;
                }
                if hours.is_some() || cells.len() <= 1 {
                    continue;
                }
            }
            if let Some(h) = header_of(&cells) {
                flush(&mut table_total);
                header = Some(h);
                continue;
            }
            read_identity_line(line, &mut sheet.identity);
            continue;
        }
        header = None;
        flush(&mut table_total);
        if let Some(c) = DATE_LINE.captures(line)
            && let Some(date) = parse_date_loose(&c[1], hint_year)
            && let Some(hours) = parse_hours_cell(&c[2])
            && sheet.entries.len() < MAX_ENTRIES
        {
            let rest = c[3].trim();
            sheet.entries.push(Entry { date: Some(date), hours, project: None, note: (!rest.is_empty()).then(|| short(rest.to_string())) });
            continue;
        }
        read_identity_line(line, &mut sheet.identity);
    }
    flush(&mut table_total);
    if !totals.is_empty() {
        sheet.total = Some(totals.iter().sum());
    } else if sheet.entries.is_empty() {
        sheet.total = text_total(text);
    } else {
        sheet.total = text_total(text).filter(|t| *t > 0.0);
    }
    sheet.period = place_period(&sheet.entries, text, subject, hint_year);
    sheet
}

static TOTAL_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\btotal(?:\s+(?:working|billable|worked|logged))?(?:\s+(?:hours|hrs))?(?:\s+worked)?\s*(?:[:=]|\bis\b|\bof\b|-)\s*(\d{1,4}(?:\.\d+)?)").expect("static regex")
});
static HOURS_SENTENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(\d{1,4}(?:\.\d+)?)\s*(?:hours|hrs)\b").expect("static regex"));

/// "Total hours: 168" in free text, else a single "168 hours" figure ("my timesheet for September (168 hours)").
fn text_total(text: &str) -> Option<f64> {
    let prose: String = text.lines().filter(|l| !l.contains('|')).collect::<Vec<_>>().join("\n");
    if let Some(c) = TOTAL_TEXT.captures(&prose) {
        return c[1].parse().ok();
    }
    let figures: Vec<f64> = HOURS_SENTENCE.captures_iter(&prose).filter_map(|c| c[1].parse().ok()).collect();
    // One figure is a total only when it is a period's worth; "8 hrs" in a sentence is a day.
    match figures.as_slice() {
        [one] if *one >= 20.0 => Some(*one),
        _ => None,
    }
}

fn place_period(entries: &[Entry], text: &str, subject: &str, hint_year: i64) -> Option<(String, String)> {
    let dates: Vec<&str> = entries.iter().filter_map(|e| e.date.as_deref()).collect();
    let span = || -> Option<(String, String)> { Some((dates.iter().min()?.to_string(), dates.iter().max()?.to_string())) };
    if let Some(p) = explicit_period(text, hint_year) {
        return Some(p);
    }
    if let Some(month) = month_named(text, subject, hint_year) {
        let inside = dates.iter().all(|d| *d >= month.0.as_str() && *d <= month.1.as_str());
        return if inside { Some(month) } else { span() };
    }
    span()
}

// ---------------------------------------------------------------- matching people

/// A person of the directory, as far as matching needs.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub id: String,
    pub name: String,
    pub code: Option<String>,
    pub emails: Vec<String>,
}

/// Links a person chose by hand: employee code (lower-case) or name key to person id.
#[derive(Debug, Clone, Default)]
pub struct Aliases {
    pub codes: HashMap<String, String>,
    pub names: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub person_id: Option<String>,
    /// code | alias | email | name | none
    pub method: &'static str,
    pub confidence: f64,
    pub ambiguous: bool,
}

impl Match {
    fn found(id: &str, method: &'static str, confidence: f64) -> Self {
        Self { person_id: Some(id.to_string()), method, confidence, ambiguous: false }
    }

    fn none(ambiguous: bool) -> Self {
        Self { person_id: None, method: "none", confidence: 0.0, ambiguous }
    }
}

fn tokens(name: &str) -> Vec<String> {
    name_key(name).split(' ').filter(|t| !t.is_empty()).map(str::to_string).collect()
}

/// One name contains the other's words ("Ravi Kumar" and "Ravi Kumar Singh", "Kumar, Ravi").
pub fn names_agree(a: &str, b: &str) -> bool {
    let (ta, tb) = (tokens(a), tokens(b));
    !ta.is_empty() && !tb.is_empty() && (ta.iter().all(|t| tb.contains(t)) || tb.iter().all(|t| ta.contains(t)))
}

/// Who a timesheet is for. What the sheet says about the employee wins over who sent the mail, so a manager or the
/// HR mailbox forwarding a sheet never becomes its owner: the sender counts only when it agrees with the name written.
pub fn match_person(people: &[Candidate], aliases: &Aliases, id: &Identity, sender: &str, original_sender: Option<&str>) -> Match {
    if let Some(code) = id.code.as_deref().map(str::to_lowercase) {
        if let Some(p) = people.iter().find(|p| p.code.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(&code))) {
            return Match::found(&p.id, "code", 0.99);
        }
        if let Some(p) = aliases.codes.get(&code).and_then(|pid| people.iter().find(|p| &p.id == pid)) {
            return Match::found(&p.id, "alias", 0.95);
        }
    }
    let by_email = |addr: &str| people.iter().find(|p| p.emails.iter().any(|e| e.eq_ignore_ascii_case(addr)));
    if let Some(p) = id.email.as_deref().and_then(by_email) {
        return Match::found(&p.id, "email", 0.95);
    }
    let written = id.name.as_deref().filter(|n| !tokens(n).is_empty());
    for (addr, confidence) in original_sender.map(|a| (a, 0.9)).into_iter().chain(std::iter::once((sender, 0.88))) {
        if let Some(p) = by_email(addr)
            && written.is_none_or(|w| names_agree(&p.name, w))
        {
            return Match::found(&p.id, "email", confidence);
        }
    }
    let Some(written) = written else { return Match::none(false) };
    if let Some(p) = aliases.names.get(&name_key(written)).and_then(|pid| people.iter().find(|p| &p.id == pid)) {
        return Match::found(&p.id, "alias", 0.95);
    }
    let key = name_key(written);
    let exact: Vec<&Candidate> = people.iter().filter(|p| name_key(&p.name) == key).collect();
    match exact.as_slice() {
        [p] => return Match::found(&p.id, "name", 0.85),
        [] => {}
        _ => return Match::none(true),
    }
    let wt = tokens(written);
    let partial: Vec<&Candidate> = people
        .iter()
        .filter(|p| {
            let pt = tokens(&p.name);
            (wt.len() >= 2 && wt.iter().all(|t| pt.contains(t))) || (pt.len() >= 2 && pt.iter().all(|t| wt.contains(t)))
        })
        .collect();
    match partial.as_slice() {
        [p] => Match::found(&p.id, "name", 0.7),
        [] => Match::none(false),
        _ => Match::none(true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_in_the_usual_forms() {
        assert_eq!(parse_date_loose("2026-09-03", 2026).as_deref(), Some("2026-09-03"));
        assert_eq!(parse_date_loose("2026-09-03 00:00:00", 2026).as_deref(), Some("2026-09-03"));
        assert_eq!(parse_date_loose("03/09/2026", 2026).as_deref(), Some("2026-09-03"));
        assert_eq!(parse_date_loose("09/25/2026", 2026).as_deref(), Some("2026-09-25"));
        assert_eq!(parse_date_loose("3 Sep 2026", 2026).as_deref(), Some("2026-09-03"));
        assert_eq!(parse_date_loose("03-Sep-26", 2026).as_deref(), Some("2026-09-03"));
        assert_eq!(parse_date_loose("Sep 3, 2026", 2026).as_deref(), Some("2026-09-03"));
        assert_eq!(parse_date_loose("Thu, 3 Sep", 2026).as_deref(), Some("2026-09-03"));
        assert_eq!(parse_date_loose("31 Feb 2026", 2026), None);
        assert_eq!(parse_date_loose("hello", 2026), None);
    }

    #[test]
    fn hours_in_cells() {
        assert_eq!(parse_hours_cell("8"), Some(8.0));
        assert_eq!(parse_hours_cell("7.5"), Some(7.5));
        assert_eq!(parse_hours_cell("8:30"), Some(8.5));
        assert_eq!(parse_hours_cell("8 hrs"), Some(8.0));
        assert_eq!(parse_hours_cell("7h 30m"), Some(7.5));
        assert_eq!(parse_hours_cell("abc"), None);
    }

    #[test]
    fn a_pasted_table_with_identity_lines() {
        let text = "Employee ID: E1024\nEmployee Name: Ravi Kumar\nClient: Acme Corp\nManager: Priya Shah\n\n\
            | Date | Project | Hours |\n| --- | --- | --- |\n| 2026-09-01 | Portal | 8 |\n| 2026-09-02 | Portal | 7.5 |\n| 2026-09-03 | Support | 8 |\n| Total | | 23.5 |\n";
        let s = parse_sheet(text, "September timesheet", 2026);
        assert_eq!(s.identity.code.as_deref(), Some("E1024"));
        assert_eq!(s.identity.name.as_deref(), Some("Ravi Kumar"));
        assert_eq!(s.identity.client.as_deref(), Some("Acme Corp"));
        assert_eq!(s.identity.manager.as_deref(), Some("Priya Shah"));
        assert_eq!(s.entries.len(), 3);
        assert_eq!(s.entries[1], Entry { date: Some("2026-09-02".into()), hours: 7.5, project: Some("Portal".into()), note: None });
        assert_eq!(s.total, Some(23.5));
        assert_eq!(s.period, Some(("2026-09-01".into(), "2026-09-30".into())));
    }

    #[test]
    fn total_hours_in_the_body_without_a_table() {
        let s = parse_sheet("Hi, please find my September timesheet.\nTotal hours: 168\nThanks, Ravi", "September timesheet", 2026);
        assert_eq!((s.total, s.entries.len()), (Some(168.0), 0));
        assert_eq!(s.period, Some(("2026-09-01".into(), "2026-09-30".into())));
        let s = parse_sheet("Hello, my timesheet for September (168 hours) is attached", "", 2026);
        assert_eq!(s.total, Some(168.0));
    }

    #[test]
    fn spreadsheet_rows_from_two_sheets_add_up() {
        let text = "## Sheet: Week 1 (3 rows × 4 columns)\n| Date | Day | Hours Worked | Task |\n| --- | --- | --- | --- |\n| 2026-09-01 | Tue | 8 | Build |\n| 2026-09-02 | Wed | 8 | Build |\n| Total |  | 16 |  |\n\n\
            ## Sheet: Week 2 (2 rows × 4 columns)\n| Date | Day | Hours Worked | Task |\n| --- | --- | --- | --- |\n| 2026-09-08 | Tue | 6 | Review |\n| Total |  | 6 |  |\n";
        let s = parse_sheet(text, "", 2026);
        assert_eq!(s.entries.len(), 3);
        assert_eq!(s.total, Some(22.0));
        assert_eq!(s.entries[0].note.as_deref(), Some("Build"));
        assert_eq!(s.period, Some(("2026-09-01".into(), "2026-09-08".into())));
    }

    #[test]
    fn one_line_per_day_and_tab_tables() {
        let s = parse_sheet("Week ending 12 Sep 2026\n1 Sep: 8 hours\n2 Sep - 7.5h\n3 Sep: 8\n", "", 2026);
        assert_eq!(s.entries.iter().map(|e| e.hours).sum::<f64>(), 23.5);
        assert_eq!(s.period, Some(("2026-09-06".into(), "2026-09-12".into())));
        let s = parse_sheet("Date\tHours\tProject\n01/09/2026\t8\tAlpha\n02/09/2026\t4:30\tAlpha\n", "", 2026);
        assert_eq!(s.entries.len(), 2);
        assert_eq!(s.entries[1].hours, 4.5);
    }

    #[test]
    fn an_explicit_range_wins() {
        let s = parse_sheet("Period: 01-Sep-2026 to 15-Sep-2026\nTotal hours: 80", "", 2026);
        assert_eq!(s.period, Some(("2026-09-01".into(), "2026-09-15".into())));
    }

    #[test]
    fn nothing_readable_gives_nothing() {
        let s = parse_sheet("Hi, please see the attached file. Thanks!", "", 2026);
        assert!(!s.has_data() && s.period.is_none() && s.identity.is_empty());
    }

    fn people() -> Vec<Candidate> {
        let c = |id: &str, name: &str, code: Option<&str>, email: &str| Candidate { id: id.into(), name: name.into(), code: code.map(str::to_string), emails: vec![email.into()] };
        vec![
            c("p_ravi", "Ravi Kumar", Some("E1024"), "ravi@acme.test"),
            c("p_priya", "Priya Shah", Some("E2000"), "priya@acme.test"),
            c("p_dev", "Dev Patel", None, "dev@acme.test"),
            c("p_dev2", "Dev Patil", None, "devp@acme.test"),
        ]
    }

    fn who(name: Option<&str>, code: Option<&str>, email: Option<&str>) -> Identity {
        Identity { name: name.map(str::to_string), code: code.map(str::to_string), email: email.map(str::to_string), ..Default::default() }
    }

    #[test]
    fn matches_by_employee_code_first() {
        let m = match_person(&people(), &Aliases::default(), &who(Some("Someone Else"), Some("e1024"), None), "dev@acme.test", None);
        assert_eq!((m.person_id.as_deref(), m.method), (Some("p_ravi"), "code"));
    }

    #[test]
    fn matches_by_email_in_the_sheet_then_sender() {
        let m = match_person(&people(), &Aliases::default(), &who(None, None, Some("priya@acme.test")), "hr@acme.test", None);
        assert_eq!((m.person_id.as_deref(), m.method), (Some("p_priya"), "email"));
        let m = match_person(&people(), &Aliases::default(), &who(Some("Ravi Kumar"), None, None), "ravi@acme.test", None);
        assert_eq!((m.person_id.as_deref(), m.method), (Some("p_ravi"), "email"));
        let m = match_person(&people(), &Aliases::default(), &Identity::default(), "ravi@acme.test", None);
        assert_eq!(m.person_id.as_deref(), Some("p_ravi"));
    }

    #[test]
    fn matches_by_unique_name_only() {
        let m = match_person(&people(), &Aliases::default(), &who(Some("Kumar, Ravi"), None, None), "hr@acme.test", None);
        assert_eq!((m.person_id.as_deref(), m.method), (Some("p_ravi"), "name"));
        let m = match_person(&people(), &Aliases::default(), &who(Some("Ravi"), None, None), "hr@acme.test", None);
        assert_eq!(m.person_id, None, "one word is not enough");
        let m = match_person(&people(), &Aliases::default(), &who(Some("Dev Pat"), None, None), "hr@acme.test", None);
        assert_eq!(m.person_id, None);
        let m = match_person(&people(), &Aliases::default(), &who(Some("Nobody Known"), None, None), "hr@acme.test", None);
        assert_eq!((m.person_id, m.method), (None, "none"));
    }

    #[test]
    fn a_forwarded_sheet_belongs_to_the_employee_not_the_forwarder() {
        let m = match_person(&people(), &Aliases::default(), &who(Some("Ravi Kumar"), None, None), "priya@acme.test", None);
        assert_eq!((m.person_id.as_deref(), m.method), (Some("p_ravi"), "name"));
        let m = match_person(&people(), &Aliases::default(), &who(Some("Ravi Kumar"), Some("E1024"), None), "priya@acme.test", Some("hr@acme.test"));
        assert_eq!((m.person_id.as_deref(), m.method), (Some("p_ravi"), "code"));
        let m = match_person(&people(), &Aliases::default(), &who(Some("Unknown Person"), None, None), "priya@acme.test", None);
        assert_eq!(m.person_id, None, "the forwarder is not the owner of someone else's sheet");
    }

    #[test]
    fn a_remembered_link_matches_automatically() {
        let mut a = Aliases::default();
        a.codes.insert("x77".into(), "p_dev".into());
        a.names.insert(name_key("D. Pat"), "p_dev2".into());
        let m = match_person(&people(), &a, &who(None, Some("X77"), None), "hr@acme.test", None);
        assert_eq!((m.person_id.as_deref(), m.method), (Some("p_dev"), "alias"));
        let m = match_person(&people(), &a, &who(Some("D. Pat"), None, None), "hr@acme.test", None);
        assert_eq!((m.person_id.as_deref(), m.method), (Some("p_dev2"), "alias"));
    }
}
