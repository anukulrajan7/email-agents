//! Portal reconciliation: CSV exports from the client's portal (hours per person per month, submissions)
//! joined exactly against what the mail says (timesheet mails, detected submissions). Differences become
//! `portal_mismatch` alerts that escalate owner, manager, final contacts.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::{
    name_key,
    policy::{self, started_by_month},
};
use crate::{
    db::{new_id, now_ms},
    http::AppState,
    team::{
        clock::{self, DAY_MS},
        escalation::{self, PersonAlert},
        people::{self, ImportReport, RowError},
        settings,
    },
};

pub const PORTAL_MISMATCH: &str = "portal_mismatch";
const MAX_ROWS: usize = 5_000;

/// (candidate key, display name) rows from the portal and from mail, for one person.
type Sides<'a> = (Vec<(&'a str, &'a str)>, Vec<(&'a str, &'a str)>);
type Cells = (Vec<String>, Vec<csv::StringRecord>);

fn read(csv_text: &str) -> Result<Cells, String> {
    let mut reader = csv::ReaderBuilder::new().trim(csv::Trim::All).flexible(true).from_reader(csv_text.trim_start_matches('\u{feff}').as_bytes());
    let headers: Vec<String> = reader.headers().map_err(|e| format!("could not read the header row: {e}"))?.iter().map(|h| h.trim().to_lowercase().replace(' ', "_")).collect();
    let mut rows = vec![];
    for r in reader.records() {
        rows.push(r.map_err(|e| format!("could not read the file: {e}"))?);
        if rows.len() > MAX_ROWS {
            return Err(format!("at most {MAX_ROWS} rows per import"));
        }
    }
    Ok((headers, rows))
}

fn col(headers: &[String], names: &[&str]) -> Option<usize> {
    headers.iter().position(|h| names.contains(&h.as_str()))
}

async fn people_by_email(db: &SqlitePool, workspace_id: &str) -> sqlx::Result<HashMap<String, String>> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT e.email, e.person_id FROM person_emails e JOIN people p ON p.id = e.person_id WHERE e.workspace_id = ?1 AND p.active = 1",
    )
    .bind(workspace_id)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().collect())
}

fn fail(report: &mut ImportReport, row: usize, field: &str, message: impl Into<String>) {
    report.errors.push(RowError { row, field: field.into(), message: message.into() });
    report.skipped += 1;
}

/// Columns: email, month (YYYY-MM), hours. Re-importing a month replaces its hours.
pub async fn import_hours(db: &SqlitePool, workspace_id: &str, csv_text: &str, dry_run: bool) -> Result<ImportReport, String> {
    let (headers, rows) = read(csv_text)?;
    let (Some(e), Some(m), Some(h)) =
        (col(&headers, &["email", "employee_email"]), col(&headers, &["month", "period"]), col(&headers, &["hours", "total_hours"]))
    else {
        return Err("the first row must have the columns email, month and hours".into());
    };
    let people = people_by_email(db, workspace_id).await.map_err(|x| x.to_string())?;
    let mut report = ImportReport { dry_run, ..Default::default() };
    let mut tx = db.begin().await.map_err(|x| x.to_string())?;
    for (i, r) in rows.iter().enumerate() {
        let row = i + 2;
        report.total_rows += 1;
        let email = r.get(e).unwrap_or("").trim().to_lowercase();
        let month: String = r.get(m).unwrap_or("").trim().chars().take(7).collect();
        let Some(person) = people.get(&email) else {
            fail(&mut report, row, "email", format!("no one in the directory has the email '{email}'"));
            continue;
        };
        if !clock::valid_month(&month) {
            fail(&mut report, row, "month", format!("'{month}' is not a month like 2026-09"));
            continue;
        }
        let hours = match r.get(h).unwrap_or("").trim().parse::<f64>() {
            Ok(v) if v.is_finite() && (0.0..=744.0).contains(&v) => v,
            _ => {
                fail(&mut report, row, "hours", format!("'{}' is not hours between 0 and 744", r.get(h).unwrap_or("")));
                continue;
            }
        };
        let existed: Option<i64> = sqlx::query_scalar("SELECT 1 FROM portal_hours WHERE workspace_id = ?1 AND person_id = ?2 AND month = ?3")
            .bind(workspace_id)
            .bind(person)
            .bind(&month)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|x| x.to_string())?;
        sqlx::query(
            "INSERT INTO portal_hours (workspace_id, person_id, month, hours, imported_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(workspace_id, person_id, month) DO UPDATE SET hours = excluded.hours, imported_at = excluded.imported_at",
        )
        .bind(workspace_id)
        .bind(person)
        .bind(&month)
        .bind(hours)
        .bind(now_ms())
        .execute(&mut *tx)
        .await
        .map_err(|x| x.to_string())?;
        if existed.is_some() { report.updated += 1 } else { report.created += 1 }
    }
    if dry_run { tx.rollback().await } else { tx.commit().await }.map_err(|x| x.to_string())?;
    Ok(report)
}

/// Columns: email, date (YYYY-MM-DD), candidate, client (optional).
pub async fn import_submissions(db: &SqlitePool, workspace_id: &str, csv_text: &str, dry_run: bool) -> Result<ImportReport, String> {
    let (headers, rows) = read(csv_text)?;
    let (Some(e), Some(d), Some(c)) = (
        col(&headers, &["email", "employee_email", "recruiter_email"]),
        col(&headers, &["date", "day", "submitted_on"]),
        col(&headers, &["candidate", "candidate_name"]),
    ) else {
        return Err("the first row must have the columns email, date and candidate".into());
    };
    let client_c = col(&headers, &["client"]);
    let people = people_by_email(db, workspace_id).await.map_err(|x| x.to_string())?;
    let mut report = ImportReport { dry_run, ..Default::default() };
    let mut tx = db.begin().await.map_err(|x| x.to_string())?;
    for (i, r) in rows.iter().enumerate() {
        let row = i + 2;
        report.total_rows += 1;
        let email = r.get(e).unwrap_or("").trim().to_lowercase();
        let day = r.get(d).unwrap_or("").trim().to_string();
        let candidate = r.get(c).unwrap_or("").trim().to_string();
        let Some(person) = people.get(&email) else {
            fail(&mut report, row, "email", format!("no one in the directory has the email '{email}'"));
            continue;
        };
        if clock::parse_date(&day).is_none() {
            fail(&mut report, row, "date", format!("'{day}' is not a date like 2026-09-14"));
            continue;
        }
        let key = name_key(&candidate);
        if key.is_empty() {
            fail(&mut report, row, "candidate", "candidate name is empty");
            continue;
        }
        let client = client_c.and_then(|c| r.get(c)).map(str::trim).filter(|s| !s.is_empty());
        let inserted = sqlx::query(
            "INSERT OR IGNORE INTO portal_submissions (id, workspace_id, person_id, day, month, candidate, candidate_key, client, imported_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )
        .bind(new_id("psb"))
        .bind(workspace_id)
        .bind(person)
        .bind(&day)
        .bind(&day[..7])
        .bind(&candidate)
        .bind(&key)
        .bind(client)
        .bind(now_ms())
        .execute(&mut *tx)
        .await
        .map_err(|x| x.to_string())?
        .rows_affected();
        if inserted > 0 { report.created += 1 } else { report.skipped += 1 }
    }
    if dry_run { tx.rollback().await } else { tx.commit().await }.map_err(|x| x.to_string())?;
    Ok(report)
}

// ---------------------------------------------------------------- reconciliation

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Mismatch {
    pub person_id: String,
    pub person_name: String,
    pub month: String,
    pub kind: &'static str,
    pub detail: Value,
}

impl Mismatch {
    fn title(&self) -> String {
        let what = match self.kind {
            "hours_differ" => "hours differ between the portal and the timesheet mail",
            "missing_in_email" => "portal hours but no timesheet mail",
            "missing_in_portal" => "timesheet mail but no portal hours",
            "submissions_missing_in_portal" => "submissions sent but not in the portal",
            _ => "portal entries with no sent submission",
        };
        format!("Portal mismatch: {} ({}): {what}", self.person_name, self.month)
    }

    fn body(&self) -> String {
        format!("{} for {}. Details: {}", self.person_name, self.month, self.detail)
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Reconciliation {
    pub months: Vec<String>,
    pub matched: usize,
    /// Timesheet mails whose hours could not be read, so they could not be compared.
    pub unverified: usize,
    pub mismatches: Vec<Mismatch>,
}

/// Exact joins of portal rows against mail-derived data. Reads only; see `reconcile_and_alert` to act on it.
pub async fn reconcile(db: &SqlitePool, workspace_id: &str, now: i64) -> sqlx::Result<Reconciliation> {
    let cfg = settings::load(db, workspace_id).await?;
    let all = people::list(db, workspace_id, false).await?;
    let name_of: HashMap<&str, &str> = all.iter().map(|p| (p.id.as_str(), p.name.as_str())).collect();
    let mut out = Reconciliation::default();

    let hour_months: Vec<String> = sqlx::query_scalar("SELECT DISTINCT month FROM portal_hours WHERE workspace_id = ?1 ORDER BY month").bind(workspace_id).fetch_all(db).await?;
    for month in &hour_months {
        let Some((start, end)) = clock::month_bounds(month, cfg.tz_offset_minutes) else { continue };
        let due = end + cfg.timesheet_grace_days * DAY_MS;
        let portal: HashMap<String, f64> = sqlx::query_as::<_, (String, f64)>("SELECT person_id, hours FROM portal_hours WHERE workspace_id = ?1 AND month = ?2")
            .bind(workspace_id)
            .bind(month)
            .fetch_all(db)
            .await?
            .into_iter()
            .collect();
        out.months.push(month.clone());
        for p in all.iter().filter(|p| (p.is_billable() && started_by_month(p, month)) || portal.contains_key(&p.id)) {
            let email = policy::timesheet_mail(db, workspace_id, &p.id, start, now.max(due) + 1).await?;
            let mut push = |kind: &'static str, detail: Value| {
                out.mismatches.push(Mismatch { person_id: p.id.clone(), person_name: p.name.clone(), month: month.clone(), kind, detail })
            };
            match (portal.get(&p.id), email) {
                (Some(ph), Some(t)) => match t.hours {
                    Some(eh) if (eh - ph).abs() > cfg.hours_tolerance => {
                        push("hours_differ", json!({ "portal_hours": ph, "email_hours": eh, "message_id": t.message_id }))
                    }
                    Some(_) => out.matched += 1,
                    None => out.unverified += 1,
                },
                (Some(ph), None) if now >= due => push("missing_in_email", json!({ "portal_hours": ph })),
                (None, Some(t)) => push("missing_in_portal", json!({ "email_hours": t.hours, "message_id": t.message_id })),
                _ => {}
            }
        }
    }

    let sub_months: Vec<String> = sqlx::query_scalar("SELECT DISTINCT month FROM portal_submissions WHERE workspace_id = ?1 ORDER BY month").bind(workspace_id).fetch_all(db).await?;
    for month in &sub_months {
        let Some((start, end)) = clock::month_bounds(month, cfg.tz_offset_minutes) else { continue };
        if !out.months.contains(month) {
            out.months.push(month.clone());
        }
        let portal_rows: Vec<(String, String, String)> =
            sqlx::query_as("SELECT person_id, candidate_key, candidate FROM portal_submissions WHERE workspace_id = ?1 AND month = ?2")
                .bind(workspace_id)
                .bind(month)
                .fetch_all(db)
                .await?;
        let email_rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
            "SELECT person_id, candidate_key, COALESCE(candidate_name, attachment_name) FROM submissions
             WHERE workspace_id = ?1 AND status = 'active' AND sent_at >= ?2 AND sent_at < ?3",
        )
        .bind(workspace_id)
        .bind(start)
        .bind(end)
        .fetch_all(db)
        .await?;
        let mut by_person: BTreeMap<&str, Sides> = BTreeMap::new();
        for (p, k, n) in &portal_rows {
            by_person.entry(p).or_default().0.push((k, n));
        }
        for (p, k, n) in &email_rows {
            by_person.entry(p).or_default().1.push((k, n.as_deref().unwrap_or(k)));
        }
        for (person, (portal, email)) in by_person {
            let Some(name) = name_of.get(person) else { continue };
            let missing = |a: &[(&str, &str)], b: &[(&str, &str)]| -> Vec<String> {
                let mut left: Vec<&str> = b.iter().map(|(k, _)| *k).collect();
                a.iter()
                    .filter(|(k, _)| match left.iter().position(|l| l == k) {
                        Some(i) => {
                            left.swap_remove(i);
                            false
                        }
                        None => true,
                    })
                    .map(|(_, n)| n.to_string())
                    .collect()
            };
            let (not_in_portal, not_in_email) = (missing(&email, &portal), missing(&portal, &email));
            if not_in_portal.is_empty() && not_in_email.is_empty() {
                out.matched += 1;
            }
            for (kind, list) in [("submissions_missing_in_portal", not_in_portal), ("submissions_missing_in_email", not_in_email)] {
                if !list.is_empty() {
                    out.mismatches.push(Mismatch {
                        person_id: person.to_string(),
                        person_name: name.to_string(),
                        month: month.clone(),
                        kind,
                        detail: json!({ "candidates": list }),
                    });
                }
            }
        }
    }
    Ok(out)
}

/// Saves the current mismatches, raises an alert for each new one and resolves the ones that cleared.
/// Returns alerts raised. Does nothing until a portal file has been imported.
pub async fn reconcile_and_alert(state: &AppState, workspace_id: &str, now: i64) -> sqlx::Result<usize> {
    let db = &state.db;
    let any: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM portal_hours WHERE workspace_id = ?1) + (SELECT COUNT(*) FROM portal_submissions WHERE workspace_id = ?1)
         + (SELECT COUNT(*) FROM portal_mismatches WHERE workspace_id = ?1 AND status = 'open')",
    )
    .bind(workspace_id)
    .fetch_one(db)
    .await?;
    if any == 0 {
        return Ok(0);
    }
    let result = reconcile(db, workspace_id, now).await?;
    let mut raised = 0;
    let mut current: Vec<(String, String, &str)> = vec![];
    for m in &result.mismatches {
        sqlx::query(
            "INSERT INTO portal_mismatches (id, workspace_id, person_id, month, kind, detail_json, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'open', ?7, ?7)
             ON CONFLICT(workspace_id, person_id, month, kind) DO UPDATE SET detail_json = excluded.detail_json, status = 'open', updated_at = excluded.updated_at",
        )
        .bind(new_id("pmm"))
        .bind(workspace_id)
        .bind(&m.person_id)
        .bind(&m.month)
        .bind(m.kind)
        .bind(m.detail.to_string())
        .bind(now)
        .execute(db)
        .await?;
        let (_, created) = escalation::raise_for_person(
            db,
            PersonAlert {
                workspace_id,
                person_id: &m.person_id,
                kind: PORTAL_MISMATCH,
                severity: "normal",
                title: m.title(),
                body: m.body(),
                dedupe_key: dedupe(&m.person_id, &m.month, m.kind),
                message_id: None,
                now,
            },
        )
        .await?;
        raised += created as usize;
        current.push((m.person_id.clone(), m.month.clone(), m.kind));
    }
    let open: Vec<(String, String, String)> =
        sqlx::query_as("SELECT person_id, month, kind FROM portal_mismatches WHERE workspace_id = ?1 AND status = 'open'").bind(workspace_id).fetch_all(db).await?;
    for (person, month, kind) in open {
        if current.iter().any(|(p, m, k)| *p == person && *m == month && *k == kind) {
            continue;
        }
        sqlx::query("UPDATE portal_mismatches SET status = 'resolved', updated_at = ?1 WHERE workspace_id = ?2 AND person_id = ?3 AND month = ?4 AND kind = ?5")
            .bind(now)
            .bind(workspace_id)
            .bind(&person)
            .bind(&month)
            .bind(&kind)
            .execute(db)
            .await?;
        sqlx::query(
            "UPDATE alerts SET status = 'resolved', resolved_at = ?1, resolved_by = 'system', snoozed_until = NULL, updated_at = ?1
             WHERE workspace_id = ?2 AND dedupe_key = ?3 AND status != 'resolved'",
        )
        .bind(now)
        .bind(workspace_id)
        .bind(dedupe(&person, &month, &kind))
        .execute(db)
        .await?;
    }
    Ok(raised)
}

fn dedupe(person_id: &str, month: &str, kind: &str) -> String {
    format!("{PORTAL_MISMATCH}:{person_id}:{month}:{kind}")
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct MismatchRow {
    pub id: String,
    pub person_id: String,
    pub person_name: String,
    pub month: String,
    pub kind: String,
    pub detail_json: String,
    pub status: String,
    pub updated_at: i64,
}

pub async fn list_mismatches(db: &SqlitePool, workspace_id: &str) -> sqlx::Result<Vec<MismatchRow>> {
    sqlx::query_as(
        "SELECT x.id, x.person_id, p.name AS person_name, x.month, x.kind, x.detail_json, x.status, x.updated_at
         FROM portal_mismatches x JOIN people p ON p.id = x.person_id
         WHERE x.workspace_id = ?1 ORDER BY x.status = 'open' DESC, x.month DESC, p.name LIMIT 500",
    )
    .bind(workspace_id)
    .fetch_all(db)
    .await
}
