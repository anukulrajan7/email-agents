//! Names the roster does not know, grouped so an admin sees one card per person. The key is the employee ID, else the
//! normalised name together with where the sheet came from (`resolve::group_key`). A group never decides anything: it
//! lists the sheets that wait, the name as written and who it might be.

use serde_json::{Value, json};
use sqlx::{SqliteConnection, SqlitePool};

use super::resolve::{self, Suggestion};
use crate::{
    hr::staffing::timesheet_parse::{Candidate, Identity},
    platform::db::{iso, new_id, now_ms},
};

/// The people sheets can be compared with: active, by name. Suggestions use names only.
pub async fn roster(db: &SqlitePool, ws: &str) -> sqlx::Result<Vec<Candidate>> {
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT id, name FROM people WHERE workspace_id = ?1 AND active = 1 ORDER BY lower(name), id").bind(ws).fetch_all(db).await?;
    Ok(rows.into_iter().map(|(id, name)| Candidate { id, name, code: None, emails: vec![] }).collect())
}

/// The domains that are a company's own: the roster's email domains and the client domains the owner named, never a free-mail
/// one (a person on gmail is trusted by their exact address, not their domain).
pub async fn company_domains(conn: &mut SqliteConnection, ws: &str) -> sqlx::Result<Vec<String>> {
    let mut out: Vec<String> = sqlx::query_scalar("SELECT DISTINCT lower(substr(email, instr(email, '@') + 1)) FROM person_emails WHERE workspace_id = ?1 AND instr(email, '@') > 0")
        .bind(ws)
        .fetch_all(&mut *conn)
        .await?;
    let clients: Option<String> = sqlx::query_scalar("SELECT client_domains_json FROM team_settings WHERE workspace_id = ?1").bind(ws).fetch_optional(conn).await?;
    out.extend(serde_json::from_str::<Vec<String>>(&clients.unwrap_or_default()).unwrap_or_default().into_iter().map(|d| d.to_lowercase()));
    out.retain(|d| !resolve::is_free_mail(d));
    out.sort();
    out.dedup();
    Ok(out)
}

/// The group a sheet's identity belongs to, created on first sight. `None` when the sheet names nobody. A group that was
/// linked to a person reopens when a new unmatched sheet arrives under it: that sheet needs a decision of its own.
pub async fn attach(conn: &mut SqliteConnection, ws: &str, identity: &Identity, sender: &str, roster: &[Candidate], company: &[String]) -> sqlx::Result<Option<String>> {
    let Some(key) = resolve::group_key(identity.code.as_deref(), identity.name.as_deref(), sender, company) else { return Ok(None) };
    let (domain, _) = resolve::sender_hint(sender, company);
    let suggestions = serde_json::to_string(&resolve::suggest(identity.name.as_deref(), roster)).unwrap_or_else(|_| "[]".into());
    let now = now_ms();
    let id: String = sqlx::query_scalar(
        "INSERT INTO unresolved_identities (id, workspace_id, key, name_as_written, employee_code, sender_domain, sender_address, suggestions_json, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
         ON CONFLICT(workspace_id, key) DO UPDATE SET
           name_as_written = COALESCE(unresolved_identities.name_as_written, excluded.name_as_written),
           employee_code = COALESCE(unresolved_identities.employee_code, excluded.employee_code),
           suggestions_json = excluded.suggestions_json,
           status = CASE WHEN unresolved_identities.status = 'linked' THEN 'open' ELSE unresolved_identities.status END,
           person_id = CASE WHEN unresolved_identities.status = 'linked' THEN NULL ELSE unresolved_identities.person_id END,
           updated_at = excluded.updated_at
         RETURNING id",
    )
    .bind(new_id("idn"))
    .bind(ws)
    .bind(key)
    .bind(identity.name.as_deref().map(str::trim).filter(|n| !n.is_empty()))
    .bind(identity.code.as_deref().map(str::trim).filter(|c| !c.is_empty()))
    .bind(domain)
    .bind(sender.trim().trim_matches(['<', '>']).to_lowercase())
    .bind(suggestions)
    .bind(now)
    .fetch_one(conn)
    .await?;
    Ok(Some(id))
}

/// How many sheets still wait under a group.
pub async fn recount(conn: &mut SqliteConnection, ws: &str, identity_id: &str) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE unresolved_identities SET record_count =
           (SELECT COUNT(*) FROM timesheet_records WHERE workspace_id = ?1 AND identity_id = ?2 AND person_id IS NULL AND status != 'replaced'),
           updated_at = ?3
         WHERE workspace_id = ?1 AND id = ?2",
    )
    .bind(ws)
    .bind(identity_id)
    .bind(now_ms())
    .execute(conn)
    .await?;
    Ok(())
}

/// Sheets kept before groups existed (or while the feature was off) get their group now. Safe to repeat and safe next to a
/// link: nothing is done when nothing waits, the sheets are read again inside the transaction, and a sheet is only given a
/// group while it is still unmatched and has none.
pub async fn ensure_groups(db: &SqlitePool, ws: &str) -> sqlx::Result<usize> {
    let waiting: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM timesheet_records WHERE workspace_id = ?1 AND person_id IS NULL AND identity_id IS NULL AND status != 'replaced'")
        .bind(ws)
        .fetch_one(db)
        .await?;
    if waiting == 0 {
        return Ok(0);
    }
    let roster = roster(db, ws).await?;
    let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
    let rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT id, identity_json, sender_address FROM timesheet_records WHERE workspace_id = ?1 AND person_id IS NULL AND identity_id IS NULL AND status != 'replaced'",
    )
    .bind(ws)
    .fetch_all(&mut *tx)
    .await?;
    let company = company_domains(&mut tx, ws).await?;
    let mut made = 0;
    let mut touched: Vec<String> = vec![];
    for (id, identity_json, sender) in rows {
        let identity: Identity = serde_json::from_str(&identity_json).unwrap_or_default();
        let Some(group) = attach(&mut tx, ws, &identity, sender.as_deref().unwrap_or_default(), &roster, &company).await? else { continue };
        let joined = sqlx::query("UPDATE timesheet_records SET identity_id = ?1 WHERE id = ?2 AND workspace_id = ?3 AND identity_id IS NULL AND person_id IS NULL")
            .bind(&group)
            .bind(&id)
            .bind(ws)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        made += joined as usize;
        if !touched.contains(&group) {
            touched.push(group);
        }
    }
    for group in &touched {
        recount(&mut tx, ws, group).await?;
    }
    tx.commit().await?;
    Ok(made)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Group {
    pub id: String,
    pub key: String,
    pub name_as_written: Option<String>,
    pub employee_code: Option<String>,
    pub sender_domain: Option<String>,
    pub sender_address: Option<String>,
    pub record_count: i64,
    pub status: String,
    pub suggestions_json: String,
    pub person_id: Option<String>,
    pub person_name: Option<String>,
    pub updated_at: i64,
}

const SELECT: &str = "SELECT g.id, g.key, g.name_as_written, g.employee_code, g.sender_domain, g.sender_address, g.record_count, g.status, g.suggestions_json,
        g.person_id, p.name AS person_name, g.updated_at
    FROM unresolved_identities g LEFT JOIN people p ON p.id = g.person_id";

pub async fn get(conn: &mut SqliteConnection, ws: &str, id: &str) -> sqlx::Result<Option<Group>> {
    sqlx::query_as(&format!("{SELECT} WHERE g.workspace_id = ?1 AND g.id = ?2")).bind(ws).bind(id).fetch_optional(conn).await
}

impl Group {
    pub fn suggestions(&self) -> Vec<Suggestion> {
        serde_json::from_str(&self.suggestions_json).unwrap_or_default()
    }
}

#[derive(sqlx::FromRow)]
struct Waiting {
    id: String,
    period_start: String,
    period_end: String,
    total_hours: Option<f64>,
    attachment_filename: Option<String>,
    email_subject: Option<String>,
    email_received_at: Option<i64>,
    message_id: Option<String>,
    issues_json: String,
}

async fn waiting(db: &SqlitePool, ws: &str, identity_id: &str) -> sqlx::Result<Vec<Value>> {
    let rows: Vec<Waiting> = sqlx::query_as(
        "SELECT id, period_start, period_end, total_hours, attachment_filename, email_subject, email_received_at, message_id, issues_json
         FROM timesheet_records WHERE workspace_id = ?1 AND identity_id = ?2 AND person_id IS NULL AND status != 'replaced' ORDER BY period_start, created_at",
    )
    .bind(ws)
    .bind(identity_id)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let issues: Vec<Value> = serde_json::from_str(&r.issues_json).unwrap_or_default();
            json!({
                "id": r.id, "period_start": r.period_start, "period_end": r.period_end, "total_hours": r.total_hours,
                "file": r.attachment_filename, "subject": r.email_subject, "received_at": r.email_received_at.map(iso),
                "message_id": r.message_id, "possible_duplicate": issues.iter().any(|i| i["code"] == "possible_duplicate"),
            })
        })
        .collect())
}

/// The cards for the admin: groups still waiting (`open`), with their sheets, and who each might be. Read-only: the
/// suggestions are worked out in memory against today's roster (so a person added a minute ago shows up), and sheets that
/// still have no group are grouped by `ensure_groups` (a job, or the refresh button), never by a read.
pub async fn open(db: &SqlitePool, ws: &str) -> sqlx::Result<Vec<Value>> {
    let groups: Vec<Group> = sqlx::query_as(&format!("{SELECT} WHERE g.workspace_id = ?1 AND g.status = 'open' AND g.record_count > 0 ORDER BY g.updated_at DESC, g.id")).bind(ws).fetch_all(db).await?;
    let roster = roster(db, ws).await?;
    let mut out = vec![];
    for g in groups {
        out.push(json!({
            "id": g.id,
            "name": g.name_as_written,
            "employee_code": g.employee_code,
            "sender": g.sender_address,
            "sender_domain": g.sender_domain,
            "record_count": g.record_count,
            "records": waiting(db, ws, &g.id).await?,
            "suggestions": resolve::suggest(g.name_as_written.as_deref(), &roster),
            "updated_at": iso(g.updated_at),
        }));
    }
    Ok(out)
}

/// The `refresh_identities` job: groups the sheets kept before groups existed, once the feature is on.
pub async fn refresh_job(state: &crate::http::AppState, payload: &Value) -> Result<(), crate::jobs::handlers::JobError> {
    let ws = payload.get("workspace_id").and_then(Value::as_str).ok_or_else(|| crate::jobs::handlers::JobError::Permanent("payload missing workspace_id".into()))?;
    if crate::platform::flags::mode(&state.db, &state.config, ws, crate::platform::flags::Key::Handoff).await?.acts() {
        ensure_groups(&state.db, ws).await?;
    }
    Ok(())
}
