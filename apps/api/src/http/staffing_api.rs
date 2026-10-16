//! HR & recruiting endpoints: submissions, timesheets and portal reconciliation. Every handler answers 409
//! unless the workspace has the staffing pack.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, patch, post},
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::AppState;
use crate::{
    platform::audit::{self, Actor, Entry},
    platform::auth::AuthUser,
    platform::db::{new_id, now_ms},
    platform::error::{AppError, AppResult},
    hr::staffing::{self, SUBMISSION_UPDATE_LABEL, portal, submissions, timesheet_records as records},
    hr::team::{access, clock, identity::resolve, settings},
    platform::flags,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/workspaces/{ws}/staffing/setup", post(setup))
        .route("/workspaces/{ws}/staffing/scan", post(scan))
        .route("/workspaces/{ws}/staffing/submissions", get(list_submissions))
        .route("/workspaces/{ws}/staffing/submissions/{id}", patch(decide))
        .route("/workspaces/{ws}/staffing/timesheets", get(timesheets))
        .route("/workspaces/{ws}/staffing/timesheet-grid", get(timesheet_grid))
        .route("/workspaces/{ws}/staffing/timesheet-records", get(timesheet_records))
        .route("/workspaces/{ws}/staffing/timesheet-records/{id}", get(timesheet_record))
        .route("/workspaces/{ws}/staffing/timesheet-records/{id}/link", post(link_timesheet_record))
        .route("/workspaces/{ws}/staffing/timesheet-records/{id}/approve", post(approve_timesheet_record))
        .route("/workspaces/{ws}/staffing/identity-links/forget", post(forget_identity_link))
        .route("/workspaces/{ws}/staffing/timesheet-records/weak-links", get(weak_links))
        .route("/workspaces/{ws}/staffing/portal/hours", post(import_hours))
        .route("/workspaces/{ws}/staffing/portal/submissions", post(import_submissions))
        .route("/workspaces/{ws}/staffing/portal/reconcile", post(reconcile))
        .route("/workspaces/{ws}/staffing/portal/mismatches", get(mismatches))
}

pub(super) async fn gate(state: &AppState, ws: &str, user: &AuthUser) -> AppResult<access::Access> {
    let a = access::resolve(&state.db, ws, user).await?;
    staffing::require(&state.db, ws).await?;
    Ok(a)
}

/// Adds the `submission_update` label so client replies about a submission can be labelled.
pub async fn setup(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    gate(&state, &ws, &user).await?.require_owner()?;
    sqlx::query(
        "INSERT INTO labels (id, workspace_id, key, name, description, color, position, created_at)
         VALUES (?1, ?2, ?3, 'Submission update', ?4, 'sage', (SELECT COALESCE(MAX(position), 0) + 1 FROM labels WHERE workspace_id = ?2), ?5)
         ON CONFLICT(workspace_id, key) DO UPDATE SET archived = 0",
    )
    .bind(new_id("lbl"))
    .bind(&ws)
    .bind(SUBMISSION_UPDATE_LABEL)
    .bind("A client replies about a candidate we submitted: interview scheduled, feedback, rejected, selected, offer or joined.")
    .bind(now_ms())
    .execute(&state.db)
    .await?;
    Ok(Json(json!({ "labels": [SUBMISSION_UPDATE_LABEL] })))
}

pub async fn scan(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    gate(&state, &ws, &user).await?.require_owner()?;
    let now = now_ms();
    let report = submissions::scan_workspace(&state, &ws, now).await?;
    let outcomes = submissions::update_outcomes(&state.db, &ws, now).await?;
    Ok(Json(json!({ "scanned": report.scanned, "created": report.created, "needs_confirm": report.needs_confirm, "outcomes_updated": outcomes })))
}

#[derive(Deserialize)]
pub struct SubmissionQuery {
    person_id: Option<String>,
    link_status: Option<String>,
    status: Option<String>,
    limit: Option<i64>,
}

pub async fn list_submissions(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>, Query(q): Query<SubmissionQuery>) -> AppResult<Json<Value>> {
    let a = gate(&state, &ws, &user).await?;
    if let Some(p) = q.person_id.as_deref().filter(|p| !p.is_empty()) {
        a.require_visible(p)?;
    }
    let rows: Vec<submissions::SubmissionRow> = sqlx::query_as(&format!(
        "{} WHERE s.workspace_id = ?1 AND (?2 IS NULL OR s.person_id = ?2) AND (?3 IS NULL OR s.link_status = ?3) AND s.status = COALESCE(?4, 'active')
           AND (?6 IS NULL OR s.person_id IN (SELECT value FROM json_each(?6)))
         ORDER BY s.sent_at DESC LIMIT ?5",
        submissions::SELECT
    ))
    .bind(&ws)
    .bind(q.person_id.as_deref().filter(|p| !p.is_empty()))
    .bind(q.link_status.as_deref().filter(|p| !p.is_empty()))
    .bind(q.status.as_deref().filter(|p| !p.is_empty()))
    .bind(q.limit.unwrap_or(100).clamp(1, 500))
    .bind(a.visible().map(|v| json!(v.iter().collect::<Vec<_>>()).to_string()))
    .fetch_all(&state.db)
    .await?;
    let items: Vec<Value> = rows.into_iter().filter(|r| a.can_see(&r.person_id)).map(submissions::row_json).collect();
    let to_confirm = items.iter().filter(|i| i["link_status"] == "needs_confirm").count();
    Ok(Json(json!({ "items": items, "needs_confirm": to_confirm })))
}

pub async fn decide(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>, Json(body): Json<submissions::Decision>) -> AppResult<Json<Value>> {
    let a = gate(&state, &ws, &user).await?;
    let person: String = sqlx::query_scalar("SELECT person_id FROM submissions WHERE id = ?1 AND workspace_id = ?2")
        .bind(&id)
        .bind(&ws)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound)?;
    if !submissions::may_decide(&a, &person) {
        return Err(AppError::NotFound);
    }
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    if let Some(c) = &body.candidate_id {
        if let Some(c) = c {
            let ok: Option<i64> = sqlx::query_scalar("SELECT 1 FROM candidates WHERE id = ?1 AND workspace_id = ?2").bind(c).bind(&ws).fetch_optional(&mut *tx).await?;
            if ok.is_none() {
                return Err(AppError::BadRequest("candidate_id: unknown candidate".into()));
            }
        }
        sqlx::query("UPDATE submissions SET candidate_id = ?1, candidate_name = (SELECT name FROM candidates WHERE id = ?1), link_status = 'confirmed', updated_at = ?2 WHERE id = ?3")
            .bind(c)
            .bind(now_ms())
            .bind(&id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(r) = &body.requirement_id {
        if let Some(r) = r {
            let ok: Option<i64> = sqlx::query_scalar("SELECT 1 FROM requirements WHERE id = ?1 AND workspace_id = ?2").bind(r).bind(&ws).fetch_optional(&mut *tx).await?;
            if ok.is_none() {
                return Err(AppError::BadRequest("requirement_id: unknown requirement".into()));
            }
        }
        sqlx::query("UPDATE submissions SET requirement_id = ?1, client = COALESCE((SELECT client FROM requirements WHERE id = ?1), client), link_status = 'confirmed', updated_at = ?2 WHERE id = ?3")
            .bind(r)
            .bind(now_ms())
            .bind(&id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(d) = body.dismissed {
        sqlx::query("UPDATE submissions SET status = ?1, updated_at = ?2 WHERE id = ?3").bind(if d { "dismissed" } else { "active" }).bind(now_ms()).bind(&id).execute(&mut *tx).await?;
    }
    if let Some(o) = &body.outcome {
        let (outcome, interviewed) = if o == "pending" {
            ("pending", false)
        } else {
            let parsed = submissions::Outcome::parse(o).ok_or_else(|| AppError::BadRequest("outcome must be pending, interview, rejected, selected or joined".into()))?;
            (parsed.as_str(), !matches!(parsed, submissions::Outcome::Rejected))
        };
        sqlx::query(
            "UPDATE submissions SET outcome = ?1, reached_interview = MAX(reached_interview * (?1 != 'pending'), ?2), outcome_message_id = 'manual', outcome_at = ?3, updated_at = ?3 WHERE id = ?4",
        )
        .bind(outcome)
        .bind(interviewed)
        .bind(now_ms())
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    }
    audit::record(&mut tx, Entry { workspace_id: &ws, actor: Actor::User(&user.id), action: "submission.updated", entity_type: "submission", entity_id: &id, message_id: None, metadata: json!({}) }).await?;
    let row: submissions::SubmissionRow = sqlx::query_as(&format!("{} WHERE s.id = ?1", submissions::SELECT)).bind(&id).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(submissions::row_json(row)))
}

#[derive(Deserialize)]
pub struct MonthQuery {
    month: Option<String>,
}

/// Billable people's timesheet status for a month (default: last month).
pub async fn timesheets(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>, Query(q): Query<MonthQuery>) -> AppResult<Json<Value>> {
    let a = gate(&state, &ws, &user).await?;
    let now = now_ms();
    let s = settings::load(&state.db, &ws).await?;
    let month = match q.month.filter(|m| !m.is_empty()) {
        Some(m) if clock::valid_month(&m) => m,
        Some(_) => return Err(AppError::BadRequest("month must look like 2026-09".into())),
        None => clock::previous_month(&clock::month_of(now, s.tz_offset_minutes)),
    };
    let rows: Vec<Value> = staffing::policy::month_status(&state.db, &ws, &month, now)
        .await?
        .into_iter()
        .filter(|r| a.can_see(&r.person_id))
        .map(|r| {
            json!({ "person_id": r.person_id, "name": r.name, "month": r.month, "status": r.status, "due_at": crate::platform::db::iso(r.due_at),
                    "timesheet": r.timesheet.map(|t| json!({ "message_id": t.message_id, "record_id": t.record_id, "subject": t.subject,
                                                             "received_at": crate::platform::db::iso(t.received_at), "hours": t.hours })) })
        })
        .collect();
    let missing = rows.iter().filter(|r| r["status"] == "missing").count();
    Ok(Json(json!({ "month": month, "grace_days": s.timesheet_grace_days, "hr_mailbox_set": s.hr_source_id.is_some(), "items": rows, "missing": missing })))
}

fn month_or_last(q: Option<String>, now: i64, tz: i64) -> AppResult<String> {
    match q.filter(|m| !m.is_empty()) {
        Some(m) if clock::valid_month(&m) => Ok(m),
        Some(_) => Err(AppError::BadRequest("month must look like 2026-09".into())),
        None => Ok(clock::month_of(now, tz)),
    }
}

/// People by days for a month (default: this month): hours per day, total and status. Replaced records never count.
pub async fn timesheet_grid(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>, Query(q): Query<MonthQuery>) -> AppResult<Json<Value>> {
    let a = gate(&state, &ws, &user).await?;
    let now = now_ms();
    let s = settings::load(&state.db, &ws).await?;
    let month = month_or_last(q.month, now, s.tz_offset_minutes)?;
    Ok(Json(records::grid(&state.db, &ws, &month, now, &a).await?))
}

#[derive(Deserialize)]
pub struct RecordsQuery {
    person_id: Option<String>,
    month: Option<String>,
    status: Option<String>,
    q: Option<String>,
}

/// Timesheet records, newest period first. Filters: person_id, month (YYYY-MM), status, q (words in name, ID, client, subject, file).
pub async fn timesheet_records(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>, Query(q): Query<RecordsQuery>) -> AppResult<Json<Value>> {
    let a = gate(&state, &ws, &user).await?;
    if q.month.as_deref().is_some_and(|m| !m.is_empty() && !clock::valid_month(m)) {
        return Err(AppError::BadRequest("month must look like 2026-09".into()));
    }
    if q.status.as_deref().is_some_and(|s| !s.is_empty() && !["recorded", "replaced", "needs_check"].contains(&s)) {
        return Err(AppError::BadRequest("status must be recorded, replaced or needs_check".into()));
    }
    let blank = |v: Option<String>| v.filter(|v| !v.is_empty());
    let filter = records::Filter { person_id: blank(q.person_id), month: blank(q.month), status: blank(q.status), q: blank(q.q) };
    let items = records::list(&state.db, &ws, &filter, &a).await?;
    Ok(Json(json!({ "items": items })))
}

/// One record with its daily entries, the identity as written, its issues and the link to the email.
pub async fn timesheet_record(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>) -> AppResult<Json<Value>> {
    let a = access::resolve(&state.db, &ws, &user).await?;
    let row = records::get(&state.db, &ws, &id).await?.ok_or(AppError::NotFound)?;
    if !records::can_see_record(&a, &row) {
        return Err(AppError::NotFound);
    }
    Ok(Json(row.json(true)))
}

#[derive(Deserialize)]
pub struct LinkBody {
    person_id: String,
    /// Also remember the name written in the sheet, so the same name matches next time. Off unless a person says so.
    #[serde(default)]
    remember_name: bool,
}

/// "This sheet is that person." The link is remembered for the employee ID written in the sheet (and for the name, when
/// the person asks), and applies to the other unmatched records that carry them. An ID or name that already belongs to
/// someone else is a conflict, never re-pointed.
pub async fn link_timesheet_record(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>, Json(body): Json<LinkBody>) -> AppResult<Json<Value>> {
    let a = gate(&state, &ws, &user).await?;
    a.require_owner()?;
    let known: Option<i64> = sqlx::query_scalar("SELECT 1 FROM people WHERE id = ?1 AND workspace_id = ?2").bind(&body.person_id).bind(&ws).fetch_optional(&state.db).await?;
    if known.is_none() {
        return Err(AppError::BadRequest("person_id: unknown person".into()));
    }
    if flags::mode(&state.db, &state.config, &ws, flags::Key::Handoff).await?.acts() {
        resolve::link_record(&state, &ws, Actor::User(&user.id), &id, &body.person_id, body.remember_name).await?;
    } else {
        match records::link_person(&state.db, &ws, &id, &body.person_id, body.remember_name).await? {
            records::Linked::Done => {}
            records::Linked::NotFound => return Err(AppError::NotFound),
            records::Linked::Conflict(why) => return Err(AppError::Conflict(why)),
        }
    }
    let row = records::get(&state.db, &ws, &id).await?.ok_or(AppError::NotFound)?;
    Ok(Json(row.json(true)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForgetBody {
    /// The employee ID to forget.
    employee_id: Option<String>,
    /// The name to forget.
    name: Option<String>,
}

/// "Forget this link": the workspace owner removes a remembered employee ID or name, so a wrong link can be fixed and the
/// sheet linked again. Logged in `identity_changes`.
pub async fn forget_identity_link(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>, Json(body): Json<ForgetBody>) -> AppResult<Json<Value>> {
    gate(&state, &ws, &user).await?.require_owner()?;
    let (kind, value) = match (&body.employee_id, &body.name) {
        (Some(v), None) => ("code", v),
        (None, Some(v)) => ("name", v),
        _ => return Err(AppError::BadRequest("give either employee_id or name".into())),
    };
    let change = resolve::forget_link(&state, &ws, Actor::User(&user.id), kind, value).await?;
    Ok(Json(json!({ "change_id": change })))
}

/// Sheets that were linked by a partial name (confidence 0.7) before that was stopped: the owner looks at each once.
pub async fn weak_links(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    gate(&state, &ws, &user).await?.require_owner()?;
    Ok(Json(json!({ "items": records::weakly_linked(&state.db, &ws).await? })))
}

/// "This sheet is right." The person's manager or an admin approves it; the first approval stays.
pub async fn approve_timesheet_record(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>) -> AppResult<Json<Value>> {
    let a = gate(&state, &ws, &user).await?;
    let row = records::desk::approve(&state.db, &ws, &id, &a, &user.id).await?;
    Ok(Json(row.json(true)))
}

#[derive(Deserialize)]
pub struct CsvBody {
    csv: String,
    #[serde(default)]
    dry_run: bool,
}

pub async fn import_hours(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>, Json(body): Json<CsvBody>) -> AppResult<Json<people::ImportReport>> {
    imported(&state, &ws, &user, &body, "portal.hours_imported", portal::import_hours(&state.db, &ws, &body.csv, body.dry_run).await).await
}

pub async fn import_submissions(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>, Json(body): Json<CsvBody>) -> AppResult<Json<people::ImportReport>> {
    imported(&state, &ws, &user, &body, "portal.submissions_imported", portal::import_submissions(&state.db, &ws, &body.csv, body.dry_run).await).await
}

use crate::hr::team::people;

async fn imported(state: &AppState, ws: &str, user: &AuthUser, body: &CsvBody, action: &str, result: Result<people::ImportReport, String>) -> AppResult<Json<people::ImportReport>> {
    gate(state, ws, user).await?.require_owner()?;
    let report = result.map_err(AppError::BadRequest)?;
    if !body.dry_run {
        let mut conn = state.db.acquire().await?;
        audit::record(&mut conn, Entry { workspace_id: ws, actor: Actor::User(&user.id), action, entity_type: "workspace", entity_id: ws, message_id: None, metadata: json!({ "created": report.created, "updated": report.updated, "skipped": report.skipped }) }).await?;
    }
    Ok(Json(report))
}

/// Compares the portal data with the mail now and raises alerts for differences.
pub async fn reconcile(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    gate(&state, &ws, &user).await?.require_owner()?;
    let now = now_ms();
    let raised = portal::reconcile_and_alert(&state, &ws, now).await?;
    let r = portal::reconcile(&state.db, &ws, now).await?;
    Ok(Json(json!({ "months": r.months, "matched": r.matched, "unverified": r.unverified, "mismatches": r.mismatches, "alerts_raised": raised })))
}

pub async fn mismatches(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    let a = gate(&state, &ws, &user).await?;
    let hours: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM portal_hours WHERE workspace_id = ?1").bind(&ws).fetch_one(&state.db).await?;
    let subs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM portal_submissions WHERE workspace_id = ?1").bind(&ws).fetch_one(&state.db).await?;
    let items: Vec<Value> = portal::list_mismatches(&state.db, &ws, a.visible())
        .await?
        .into_iter()
        .filter(|m| a.can_see(&m.person_id))
        .map(|m| json!({ "id": m.id, "person_id": m.person_id, "person_name": m.person_name, "month": m.month, "kind": m.kind,
                         "detail": serde_json::from_str::<Value>(&m.detail_json).unwrap_or(Value::Null), "status": m.status,
                         "updated_at": crate::platform::db::iso(m.updated_at) }))
        .collect();
    Ok(Json(json!({ "items": items, "portal_hours_rows": hours, "portal_submission_rows": subs })))
}
