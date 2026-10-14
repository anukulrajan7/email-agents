//! Billable policy and timesheets.
//!
//! Billable staff's mail arrives only through the company HR / timesheet mailbox (`hr_source_id`). Mail
//! to or from a billable person's address that shows up in any other mailbox is never stored. Non-billable
//! staff keep their full mailbox. A billable person's monthly timesheet is expected by month end plus a
//! grace period (3 days by default); after that a `timesheet_missing` alert goes up the escalation chain.

use std::sync::OnceLock;

use regex::Regex;
use serde::Serialize;
use serde_json::json;
use sqlx::{SqliteConnection, SqlitePool};

use crate::{
    domain::NormalizedEmail,
    http::AppState,
    packs,
    team::{
        clock::{self, DAY_MS},
        escalation::{self, PersonAlert},
        people::{self, Person},
        settings,
    },
};

pub const TIMESHEET_MISSING: &str = "timesheet_missing";

/// "Name <a@b.co>" and bare addresses, lower-cased.
pub fn address_of(raw: &str) -> String {
    let raw = raw.trim();
    match (raw.rfind('<'), raw.rfind('>')) {
        (Some(a), Some(b)) if a < b => raw[a + 1..b].trim().to_lowercase(),
        _ => raw.to_lowercase(),
    }
}

/// True when the billable policy keeps this mail out of sight: a billable person SENT it (the sender or, for
/// forwarded mail, the original sender), or it arrived in a mailbox linked to a billable person, and it did not
/// come through the HR mailbox. Recipients and Cc never count, so a client copying a consultant, a team list, or a
/// recruiter's sent CV mail copying a consultant are all kept. The mail is stored but marked `suppressed`.
pub async fn should_suppress(conn: &mut SqliteConnection, source_id: &str, workspace_id: &str, email: &NormalizedEmail) -> sqlx::Result<bool> {
    let senders: Vec<String> = std::iter::once(&email.sender_address)
        .chain(email.original_sender_address.as_ref())
        .map(|a| address_of(a))
        .filter(|a| !a.is_empty())
        .collect();
    qualifies(conn, source_id, workspace_id, &senders).await
}

async fn qualifies(conn: &mut SqliteConnection, source_id: &str, workspace_id: &str, senders: &[String]) -> sqlx::Result<bool> {
    let hr: Option<String> = sqlx::query_scalar("SELECT hr_source_id FROM team_settings WHERE workspace_id = ?1")
        .bind(workspace_id)
        .fetch_optional(&mut *conn)
        .await?
        .flatten();
    let Some(hr) = hr else { return Ok(false) };
    if hr == source_id || !packs::enabled(&mut *conn, workspace_id, packs::STAFFING).await? {
        return Ok(false);
    }
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM people p
                       WHERE p.workspace_id = ?1 AND p.active = 1 AND p.employee_type = 'billable'
                         AND (p.source_id = ?3
                              OR EXISTS (SELECT 1 FROM person_emails e WHERE e.person_id = p.id AND e.email IN (SELECT value FROM json_each(?2)))))",
    )
    .bind(workspace_id)
    .bind(json!(senders).to_string())
    .bind(source_id)
    .fetch_one(conn)
    .await
}

#[derive(sqlx::FromRow)]
struct Held {
    id: String,
    source_id: String,
    thread_id: String,
    direction: String,
    sender_address: String,
    original_sender_address: Option<String>,
    sender_name: Option<String>,
    received_at: i64,
}

/// Run after anything the policy depends on changes (HR mailbox, a person's billable flag, emails or mailbox).
/// Each suppressed message is judged again against the policy as it is now; only those that no longer qualify are
/// released, and they get the steps they skipped: contact, agent wake-up, attachment reading and labelling.
pub async fn release_suppressed(db: &SqlitePool, workspace_id: &str) -> sqlx::Result<usize> {
    let held: Vec<Held> = sqlx::query_as(
        "SELECT id, source_id, thread_id, direction, sender_address, original_sender_address, sender_name, received_at
         FROM email_messages WHERE workspace_id = ?1 AND suppressed = 1",
    )
    .bind(workspace_id)
    .fetch_all(db)
    .await?;
    let mut conn = db.acquire().await?;
    let mut released = 0;
    for m in held {
        let senders: Vec<String> = std::iter::once(&m.sender_address)
            .chain(m.original_sender_address.as_ref())
            .map(|a| address_of(a))
            .filter(|a| !a.is_empty())
            .collect();
        if qualifies(&mut conn, &m.source_id, workspace_id, &senders).await? {
            continue;
        }
        sqlx::query("UPDATE email_messages SET suppressed = 0 WHERE id = ?1").bind(&m.id).execute(&mut *conn).await?;
        released += 1;
        if m.direction != "inbound" {
            continue;
        }
        crate::memory::record_contact(&mut conn, workspace_id, &m.sender_address, m.sender_name.as_deref(), m.received_at).await?;
        crate::agents::triggers::wake_for_thread(&mut conn, workspace_id, &m.thread_id, &m.id).await?;
        let files: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM attachments WHERE message_id = ?1").bind(&m.id).fetch_one(&mut *conn).await?;
        if files > 0 {
            crate::jobs::attachment_jobs::enqueue_extract(&mut conn, workspace_id, &m.id, false).await?;
        }
        crate::jobs::enqueue(
            &mut conn,
            crate::jobs::kind::CLASSIFY_EMAIL,
            &format!("classify:{}:{}:released", m.id, crate::domain::TAXONOMY_VERSION),
            Some(workspace_id),
            &json!({ "message_id": m.id }),
        )
        .await?;
    }
    Ok(released)
}

static HOURS_TOTAL: OnceLock<Regex> = OnceLock::new();
static HOURS_ANY: OnceLock<Regex> = OnceLock::new();

/// Hours written in a timesheet mail: "Total hours: 168", else the last "N hours" / "N hrs" figure.
pub fn parse_hours(text: &str) -> Option<f64> {
    let total = HOURS_TOTAL.get_or_init(|| Regex::new(r"(?i)total(?:\s+(?:working|billable))?(?:\s+(?:hours|hrs))?\s*(?:worked)?\s*[:=\-]?\s*(\d{1,4}(?:\.\d+)?)").expect("static regex"));
    if let Some(c) = total.captures(text) {
        return c[1].parse().ok();
    }
    let any = HOURS_ANY.get_or_init(|| Regex::new(r"(?i)(\d{1,4}(?:\.\d+)?)\s*(?:hours|hrs|hr)\b").expect("static regex"));
    any.captures_iter(text).last().and_then(|c| c[1].parse().ok())
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Timesheet {
    pub message_id: String,
    pub subject: String,
    pub received_at: i64,
    pub hours: Option<f64>,
}

const MONTHS: [(&str, &str); 12] = [
    ("jan", "january"), ("feb", "february"), ("mar", "march"), ("apr", "april"), ("may", "may"), ("jun", "june"),
    ("jul", "july"), ("aug", "august"), ("sep", "september"), ("oct", "october"), ("nov", "november"), ("dec", "december"),
];

/// The month a timesheet mail is for (`YYYY-MM`). A month named in the subject wins ("September timesheet",
/// "Timesheet Sept (late)", "2026-09"); the year is the explicit one, else the latest such month not after the
/// mail. Otherwise it is the month of `received_at` minus the grace period, so a timesheet sent on 2 Sep
/// belongs to August.
pub fn period_of(subject: &str, received_at: i64, tz_min: i64, grace_days: i64) -> String {
    let received = clock::month_of(received_at, tz_min);
    let lower = subject.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric() && c != '-').filter(|w| !w.is_empty()).collect();
    if let Some(w) = words.iter().find(|w| w.len() == 7 && w.as_bytes()[4] == b'-' && clock::valid_month(w)) {
        return (*w).to_string();
    }
    let named = words.iter().find_map(|w| MONTHS.iter().position(|(short, long)| *w == *long || *w == *short || (*w == "sept" && *short == "sep")));
    if let Some(i) = named {
        let m = i as i64 + 1;
        let year_word = words.iter().find(|w| w.len() == 4 && w.chars().all(|c| c.is_ascii_digit()) && w.starts_with("20"));
        let (ry, rm): (i64, i64) = (received[..4].parse().unwrap_or(1970), received[5..].parse().unwrap_or(1));
        let year = match year_word {
            Some(y) => y.parse().unwrap_or(ry),
            None if m <= rm => ry,
            None => ry - 1,
        };
        return format!("{year:04}-{m:02}");
    }
    clock::month_of(received_at - grace_days * DAY_MS, tz_min)
}

/// The newest `timesheet` mail from this person that is for `month` (see `period_of`), with hours read from the
/// mail and its attachments. A late timesheet still counts for its own month; another month's never does.
pub async fn timesheet_mail(db: &SqlitePool, workspace_id: &str, person_id: &str, month: &str) -> sqlx::Result<Option<Timesheet>> {
    let s = settings::load(db, workspace_id).await?;
    let Some((start, end)) = clock::month_bounds(month, s.tz_offset_minutes) else { return Ok(None) };
    let rows: Vec<(String, String, i64, String)> = sqlx::query_as(
        "SELECT m.id, m.subject, m.received_at, m.body_text
         FROM email_messages m JOIN classifications c ON c.message_id = m.id
         WHERE m.workspace_id = ?1 AND m.direction = 'inbound' AND m.suppressed = 0 AND c.category = 'timesheet'
           AND m.received_at >= ?3 AND m.received_at < ?4
           AND (lower(m.sender_address) IN (SELECT email FROM person_emails WHERE person_id = ?2)
                OR lower(COALESCE(m.original_sender_address, '')) IN (SELECT email FROM person_emails WHERE person_id = ?2))
         ORDER BY m.received_at DESC LIMIT 30",
    )
    .bind(workspace_id)
    .bind(person_id)
    .bind(start)
    .bind(end + 62 * DAY_MS)
    .fetch_all(db)
    .await?;
    let Some((message_id, subject, received_at, body)) =
        rows.into_iter().find(|(_, subject, at, _)| period_of(subject, *at, s.tz_offset_minutes, s.timesheet_grace_days) == month)
    else {
        return Ok(None);
    };
    let attachments: Vec<String> =
        sqlx::query_scalar("SELECT text_content FROM attachments WHERE message_id = ?1 AND text_content IS NOT NULL").bind(&message_id).fetch_all(db).await?;
    let hours = parse_hours(&format!("{subject}\n{body}")).or_else(|| attachments.iter().find_map(|t| parse_hours(t)));
    Ok(Some(Timesheet { message_id, subject, received_at, hours }))
}

#[derive(Debug, Clone, Serialize)]
pub struct TimesheetRow {
    pub person_id: String,
    pub name: String,
    pub month: String,
    /// received | pending (not due yet) | missing (due date passed)
    pub status: &'static str,
    pub due_at: i64,
    pub timesheet: Option<Timesheet>,
}

pub fn started_by_month(p: &Person, month: &str) -> bool {
    p.joining_date.as_deref().is_none_or(|j| j.get(..7).unwrap_or(j) <= month)
}

/// Every active billable person's timesheet status for `month` (`YYYY-MM`), at time `now`.
pub async fn month_status(db: &SqlitePool, workspace_id: &str, month: &str, now: i64) -> sqlx::Result<Vec<TimesheetRow>> {
    let s = settings::load(db, workspace_id).await?;
    let Some((_, end)) = clock::month_bounds(month, s.tz_offset_minutes) else { return Ok(vec![]) };
    let due_at = end + s.timesheet_grace_days * DAY_MS;
    let mut rows = vec![];
    for p in people::list(db, workspace_id, false).await?.into_iter().filter(|p| p.is_billable() && started_by_month(p, month)) {
        let timesheet = timesheet_mail(db, workspace_id, &p.id, month).await?;
        let status = match (&timesheet, now >= due_at) {
            (Some(_), _) => "received",
            (None, true) => "missing",
            (None, false) => "pending",
        };
        rows.push(TimesheetRow { person_id: p.id, name: p.name, month: month.to_string(), status, due_at, timesheet });
    }
    Ok(rows)
}

/// Raises `timesheet_missing` for last month's missing timesheets once the grace period is over, and
/// resolves earlier ones whose timesheet has since arrived. Returns alerts raised.
pub async fn evaluate_timesheets(state: &AppState, workspace_id: &str, now: i64) -> sqlx::Result<usize> {
    let db = &state.db;
    let s = settings::load(db, workspace_id).await?;
    let month = clock::previous_month(&clock::month_of(now, s.tz_offset_minutes));
    let mut raised = 0;
    for row in month_status(db, workspace_id, &month, now).await?.into_iter().filter(|r| r.status == "missing") {
        let (_, created) = escalation::raise_for_person(
            db,
            PersonAlert {
                workspace_id,
                person_id: &row.person_id,
                kind: TIMESHEET_MISSING,
                severity: "high",
                title: format!("Timesheet missing: {} ({month})", row.name),
                body: format!("No timesheet from {} for {month} has arrived, and it was due {} days after month end.", row.name, s.timesheet_grace_days),
                dedupe_key: format!("{TIMESHEET_MISSING}:{}:{month}", row.person_id),
                message_id: None,
                now,
                period_key: None,
            },
        )
        .await?;
        raised += created as usize;
    }
    let open: Vec<(String, String)> =
        sqlx::query_as("SELECT id, dedupe_key FROM alerts WHERE workspace_id = ?1 AND kind = ?2 AND status != 'resolved'")
            .bind(workspace_id)
            .bind(TIMESHEET_MISSING)
            .fetch_all(db)
            .await?;
    for (id, key) in open {
        let mut parts = key.splitn(3, ':').skip(1);
        let (Some(person_id), Some(month)) = (parts.next(), parts.next()) else { continue };
        if timesheet_mail(db, workspace_id, person_id, month).await?.is_some() {
            sqlx::query("UPDATE alerts SET status = 'resolved', resolved_at = ?1, resolved_by = 'system', snoozed_until = NULL, updated_at = ?1 WHERE id = ?2")
                .bind(now)
                .bind(&id)
                .execute(db)
                .await?;
        }
    }
    Ok(raised)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_and_hours() {
        assert_eq!(address_of("Priya <Priya@Acme.com>"), "priya@acme.com");
        assert_eq!(address_of(" a@b.co "), "a@b.co");
        assert_eq!(parse_hours("Hi, please find September timesheet. Total hours: 168"), Some(168.0));
        assert_eq!(parse_hours("Total: 160.5"), Some(160.5));
        assert_eq!(parse_hours("worked 20 hrs in week 1 and 150 hours overall"), Some(150.0));
        assert_eq!(parse_hours("see attached"), None);
    }
}
