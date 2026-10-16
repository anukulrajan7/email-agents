//! Team directory, role-scoped views, metrics and escalation settings. Available in every workspace;
//! HR & recruiting fields appear only when that pack is on.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::AppState;
use crate::{
    platform::audit::{self, Actor, Entry},
    platform::auth::AuthUser,
    platform::error::{AppError, AppResult},
    platform::packs,
    hr::team::{
        access::{self, Access},
        escalation, metrics, people,
        people::{Context, Draft, Fields, Person},
        settings::{self, SettingsInput},
    },
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/workspaces/{ws}/team/access", get(get_access))
        .route("/workspaces/{ws}/team/settings", get(get_settings).put(put_settings))
        .route("/workspaces/{ws}/team/people", get(list_people).post(create_person))
        .route("/workspaces/{ws}/team/people/import", post(import_people))
        .route("/workspaces/{ws}/team/people/{id}", get(get_person).patch(update_person).delete(archive_person))
        .route("/workspaces/{ws}/team/metrics/me", get(my_metrics))
        .route("/workspaces/{ws}/team/metrics/people/{id}", get(person_metrics))
        .route("/workspaces/{ws}/team/metrics/team", get(team_metrics))
        .route("/workspaces/{ws}/team/escalations", get(escalations))
        .route("/workspaces/{ws}/team/ownership", post(transfer_ownership))
}

/// Re-judges suppressed mail after a change that the billable policy depends on.
async fn policy_changed(state: &AppState, ws: &str) -> AppResult<()> {
    if packs::enabled(&state.db, ws, packs::STAFFING).await? {
        crate::hr::staffing::policy::release_suppressed(&state.db, ws).await?;
    }
    Ok(())
}

fn bad(e: people::FieldError) -> AppError {
    AppError::BadRequest(format!("{}: {}", e.field, e.message))
}

pub(super) async fn context(state: &AppState, ws: &str) -> AppResult<Context> {
    let staffing = packs::enabled(&state.db, ws, packs::STAFFING).await?;
    let hr_source_id = settings::load(&state.db, ws).await?.hr_source_id;
    let hr_connected = match &hr_source_id {
        Some(id) => sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM email_sources WHERE id = ?1 AND workspace_id = ?2 AND status != 'disconnected'")
            .bind(id)
            .bind(ws)
            .fetch_one(&state.db)
            .await?
            > 0,
        None => false,
    };
    Ok(Context { staffing, hr_source_id, hr_connected })
}

fn shape(p: Person, staffing: bool) -> Person {
    if staffing { p } else { p.without_staffing() }
}

pub async fn get_access(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    let a = access::resolve(&state.db, &ws, &user).await?;
    let person = match &a.person_id {
        Some(id) => people::get(&state.db, &ws, id).await?.map(|p| json!({ "id": p.id, "name": p.name, "role": p.role })),
        None => None,
    };
    let pack_keys = packs::enabled_keys(&state.db, &ws).await?;
    Ok(Json(json!({
        "level": a.level,
        "person": person,
        "is_manager": a.report_count() > 0,
        "report_count": a.report_count(),
        "staffing": pack_keys.contains(&packs::STAFFING),
        "primary_metric": metrics::primary_key(&metrics::defs_for(&pack_keys)),
    })))
}

// ---------------------------------------------------------------- settings

fn settings_json(s: &settings::TeamSettings, staffing: bool, owner: bool, sources: Vec<Value>) -> Value {
    let mut v = json!({
        "tz_offset_minutes": s.tz_offset_minutes, "manager_after_hours": s.manager_after_hours, "final_after_hours": s.final_after_hours,
        "final_emails": s.final_emails, "below_target_hour": s.below_target_hour, "weekly_check_hour": s.weekly_check_hour,
        "route_alerts_by_owner": s.route_alerts_by_owner,
    });
    if !owner {
        // Everyone may know the delays; the contacts and the HR configuration are the super admin's.
        v["final_emails"] = json!([]);
        return v;
    }
    v["timezone"] = json!(s.timezone);
    v["reply_sla_hours"] = json!(s.reply_sla_hours);
    v["work_start_hour"] = json!(s.work_start_hour);
    v["work_end_hour"] = json!(s.work_end_hour);
    v["work_days"] = json!(s.work_days);
    v["holding_mode"] = json!(s.holding_mode);
    v["no_reply_senders"] = json!(s.no_reply_senders);
    if staffing {
        v["restrict_mail_to_team"] = json!(s.restrict_mail_to_team);
        v["hr_source_id"] = json!(s.hr_source_id);
        v["client_domains"] = json!(s.client_domains);
        v["timesheet_grace_days"] = json!(s.timesheet_grace_days);
        v["hours_tolerance"] = json!(s.hours_tolerance);
        v["timesheet_cadence"] = json!(s.timesheet_cadence);
        v["max_hours_per_day"] = json!(s.max_hours_per_day);
        v["require_signature"] = json!(s.require_signature);
        v["timesheet_required"] = json!(s.timesheet_required);
        v["missing_reminder_hours"] = json!(s.missing_reminder_hours);
        v["missing_escalate_hours"] = json!(s.missing_escalate_hours);
        v["missing_max_reminders"] = json!(s.missing_max_reminders);
        v["sources"] = json!(sources);
    }
    v
}

async fn source_options(state: &AppState, ws: &str) -> AppResult<Vec<Value>> {
    let rows: Vec<(String, String, Option<String>)> =
        sqlx::query_as("SELECT id, kind, mailbox_address FROM email_sources WHERE workspace_id = ?1 AND status != 'disconnected' ORDER BY created_at")
            .bind(ws)
            .fetch_all(&state.db)
            .await?;
    Ok(rows.into_iter().map(|(id, kind, mailbox)| json!({ "id": id, "kind": kind, "mailbox": mailbox })).collect())
}

pub async fn get_settings(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    let owner = access::resolve(&state.db, &ws, &user).await?.is_owner();
    let staffing = packs::enabled(&state.db, &ws, packs::STAFFING).await?;
    let s = settings::load(&state.db, &ws).await?;
    let sources = if staffing && owner { source_options(&state, &ws).await? } else { vec![] };
    Ok(Json(settings_json(&s, staffing, owner, sources)))
}

pub async fn put_settings(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>, Json(body): Json<SettingsInput>) -> AppResult<Json<Value>> {
    access::resolve(&state.db, &ws, &user).await?.require_owner()?;
    let staffing = packs::enabled(&state.db, &ws, packs::STAFFING).await?;
    let current = settings::load(&state.db, &ws).await?;
    let next = body.validated(&current, staffing).map_err(AppError::BadRequest)?;
    let mut connected = false;
    if let Some(src) = &next.hr_source_id {
        let status: Option<String> = sqlx::query_scalar("SELECT status FROM email_sources WHERE id = ?1 AND workspace_id = ?2").bind(src).bind(&ws).fetch_optional(&state.db).await?;
        match status.as_deref() {
            None => return Err(AppError::BadRequest("hr_source_id: unknown mailbox".into())),
            Some("disconnected") => return Err(AppError::BadRequest("hr_source_id: that mailbox is disconnected".into())),
            Some(_) => connected = true,
        }
    }
    if staffing && !connected {
        let billable: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM people WHERE workspace_id = ?1 AND active = 1 AND employee_type = 'billable'").bind(&ws).fetch_one(&state.db).await?;
        if billable > 0 {
            return Err(AppError::BadRequest("hr_source_id: billable people exist, so a connected HR mailbox must stay set".into()));
        }
    }
    settings::save(&state.db, &ws, &next).await?;
    if staffing {
        crate::hr::staffing::policy::release_suppressed(&state.db, &ws).await?;
    }
    let mut conn = state.db.acquire().await?;
    audit::record(&mut conn, Entry { workspace_id: &ws, actor: Actor::User(&user.id), action: "team_settings.updated", entity_type: "workspace", entity_id: &ws, message_id: None, metadata: json!({}) }).await?;
    let sources = if staffing { source_options(&state, &ws).await? } else { vec![] };
    Ok(Json(settings_json(&next, staffing, true, sources)))
}

// ---------------------------------------------------------------- people

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    include_inactive: bool,
}

pub async fn list_people(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>, Query(q): Query<ListQuery>) -> AppResult<Json<Value>> {
    let a = access::resolve(&state.db, &ws, &user).await?;
    let staffing = packs::enabled(&state.db, &ws, packs::STAFFING).await?;
    let items: Vec<Person> = people::list(&state.db, &ws, q.include_inactive && a.is_owner())
        .await?
        .into_iter()
        .filter(|p| a.can_see(&p.id))
        .map(|p| shape(p, staffing))
        .collect();
    Ok(Json(json!({ "items": items, "can_edit": a.is_owner() })))
}

pub async fn get_person(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>) -> AppResult<Json<Person>> {
    let a = access::resolve(&state.db, &ws, &user).await?;
    a.require_visible(&id)?;
    let staffing = packs::enabled(&state.db, &ws, packs::STAFFING).await?;
    let p = people::get(&state.db, &ws, &id).await?.ok_or(AppError::NotFound)?;
    Ok(Json(shape(p, staffing)))
}

pub async fn create_person(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>, Json(body): Json<Fields>) -> AppResult<(StatusCode, Json<Person>)> {
    access::resolve(&state.db, &ws, &user).await?.require_owner()?;
    let ctx = context(&state, &ws).await?;
    let draft = people::merge(people::blank_draft(), body, &ctx).map_err(bad)?;
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    people::validate(&mut tx, &ws, None, &draft, &ctx).await.map_err(bad)?;
    let id = people::insert(&mut tx, &ws, &draft).await?;
    audit::record(&mut tx, Entry { workspace_id: &ws, actor: Actor::User(&user.id), action: "person.created", entity_type: "person", entity_id: &id, message_id: None, metadata: json!({}) }).await?;
    tx.commit().await?;
    policy_changed(&state, &ws).await?;
    let p = people::get(&state.db, &ws, &id).await?.ok_or(AppError::NotFound)?;
    Ok((StatusCode::CREATED, Json(shape(p, ctx.staffing))))
}

pub async fn update_person(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>, Json(body): Json<Fields>) -> AppResult<Json<Person>> {
    access::resolve(&state.db, &ws, &user).await?.require_owner()?;
    let ctx = context(&state, &ws).await?;
    let current = people::get(&state.db, &ws, &id).await?.ok_or(AppError::NotFound)?;
    let draft: Draft = people::merge(Draft::from_person(&current), body, &ctx).map_err(bad)?;
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    people::validate(&mut tx, &ws, Some(&id), &draft, &ctx).await.map_err(bad)?;
    people::update(&mut tx, &ws, &id, &draft).await?;
    audit::record(&mut tx, Entry { workspace_id: &ws, actor: Actor::User(&user.id), action: "person.updated", entity_type: "person", entity_id: &id, message_id: None, metadata: json!({}) }).await?;
    tx.commit().await?;
    policy_changed(&state, &ws).await?;
    let p = people::get(&state.db, &ws, &id).await?.ok_or(AppError::NotFound)?;
    Ok(Json(shape(p, ctx.staffing)))
}

/// Archives (keeps their numbers, frees their email addresses).
pub async fn archive_person(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>) -> AppResult<StatusCode> {
    access::resolve(&state.db, &ws, &user).await?.require_owner()?;
    let ctx = context(&state, &ws).await?;
    let current = people::get(&state.db, &ws, &id).await?.ok_or(AppError::NotFound)?;
    let mut draft = Draft::from_person(&current);
    draft.active = false;
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    people::update(&mut tx, &ws, &id, &draft).await?;
    sqlx::query("UPDATE people SET manager_id = NULL WHERE manager_id = ?1").bind(&id).execute(&mut *tx).await?;
    audit::record(&mut tx, Entry { workspace_id: &ws, actor: Actor::User(&user.id), action: "person.archived", entity_type: "person", entity_id: &id, message_id: None, metadata: json!({}) }).await?;
    tx.commit().await?;
    let _ = ctx;
    policy_changed(&state, &ws).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct ImportBody {
    csv: String,
    #[serde(default)]
    dry_run: bool,
}

pub async fn import_people(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>, Json(body): Json<ImportBody>) -> AppResult<Json<people::ImportReport>> {
    access::resolve(&state.db, &ws, &user).await?.require_owner()?;
    let ctx = context(&state, &ws).await?;
    let report = people::import_csv(&state.db, &ws, &body.csv, body.dry_run, &ctx).await.map_err(AppError::BadRequest)?;
    if !body.dry_run {
        policy_changed(&state, &ws).await?;
        let mut conn = state.db.acquire().await?;
        audit::record(&mut conn, Entry { workspace_id: &ws, actor: Actor::User(&user.id), action: "people.imported", entity_type: "workspace", entity_id: &ws, message_id: None, metadata: json!({ "created": report.created, "updated": report.updated, "skipped": report.skipped }) }).await?;
    }
    Ok(Json(report))
}

// ---------------------------------------------------------------- metrics

pub async fn my_metrics(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    let a = access::resolve(&state.db, &ws, &user).await?;
    let Some(person_id) = &a.person_id else { return Ok(Json(json!({ "person": null, "metrics": null }))) };
    let m = crate::hr::staffing::metrics::person_metrics(&state.db, &ws, person_id, crate::platform::db::now_ms()).await?;
    Ok(Json(json!({ "person": m.as_ref().map(|m| json!({ "id": m.person_id, "name": m.name })), "metrics": m })))
}

pub async fn person_metrics(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>) -> AppResult<Json<Value>> {
    let a = access::resolve(&state.db, &ws, &user).await?;
    a.require_visible(&id)?;
    let m = crate::hr::staffing::metrics::person_metrics(&state.db, &ws, &id, crate::platform::db::now_ms()).await?.ok_or(AppError::NotFound)?;
    Ok(Json(json!(m)))
}

#[derive(Deserialize)]
pub struct TeamQuery {
    manager_id: Option<String>,
}

/// The super admin sees everyone (or one manager's team); anyone else sees the people below them.
pub async fn team_metrics(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>, Query(q): Query<TeamQuery>) -> AppResult<Json<Value>> {
    let a = access::resolve(&state.db, &ws, &user).await?;
    let manager = q.manager_id.filter(|m| !m.is_empty());
    let scope: Option<String> = match (a.is_owner(), manager) {
        (true, m) => m,
        (false, Some(m)) => {
            a.require_visible(&m)?;
            Some(m)
        }
        (false, None) => match &a.person_id {
            Some(p) => Some(p.clone()),
            None => return Ok(Json(json!(metrics::combine(&[], None, &packs::enabled_keys(&state.db, &ws).await?)))),
        },
    };
    let t = crate::hr::staffing::metrics::team_metrics(&state.db, &ws, scope.as_deref(), crate::platform::db::now_ms()).await?;
    Ok(Json(json!(t)))
}

// ---------------------------------------------------------------- escalations

pub async fn escalations(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    let a: Access = access::resolve(&state.db, &ws, &user).await?;
    let staffing = packs::enabled(&state.db, &ws, packs::STAFFING).await?;
    let s = settings::load(&state.db, &ws).await?;
    let alert_settings = crate::alerts::load_settings(&state.db, &ws).await?;
    let mut rows = vec![];
    for p in people::list(&state.db, &ws, false).await?.into_iter().filter(|p| a.can_see(&p.id)) {
        let mut steps = escalation::chain(&state.db, &ws, &p.id, &alert_settings.recipients).await?;
        if !a.is_owner() {
            steps.iter_mut().for_each(|s| s.emails.clear());
        }
        rows.push(json!({ "person": { "id": p.id, "name": p.name, "role": p.role }, "steps": steps }));
    }
    let default_hours = alert_settings.escalate_after_hours;
    let _ = staffing;
    Ok(Json(json!({
        "manager_after_hours": s.manager_after_hours.unwrap_or(default_hours),
        "final_after_hours": s.final_after_hours.unwrap_or(default_hours),
        "final_emails": if !a.is_owner() { vec![] } else if s.final_emails.is_empty() { alert_settings.recipients.clone() } else { s.final_emails.clone() },
        "chains": rows,
        "can_edit": a.is_owner(),
    })))
}

#[derive(Deserialize)]
pub struct Transfer {
    email: String,
}

/// The super admin hands the role to another admin of the workspace. They stay an admin.
pub async fn transfer_ownership(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>, Json(body): Json<Transfer>) -> AppResult<Json<Value>> {
    access::resolve(&state.db, &ws, &user).await?.require_owner()?;
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    // The first statement is a write, so the checks below run under the write lock.
    sqlx::query("UPDATE workspaces SET name = name WHERE id = ?1").bind(&ws).execute(&mut *tx).await?;
    let target: Option<String> = sqlx::query_scalar(
        "SELECT u.id FROM memberships m JOIN users u ON u.id = m.user_id WHERE m.workspace_id = ?1 AND m.role = 'admin' AND lower(u.email) = lower(?2)",
    )
    .bind(&ws)
    .bind(body.email.trim())
    .fetch_optional(&mut *tx)
    .await?;
    let target = target.ok_or_else(|| AppError::BadRequest("the new super admin must be an admin member of this workspace".into()))?;
    sqlx::query("INSERT INTO workspace_owners (workspace_id, user_id, created_at) VALUES (?1, ?2, ?3) ON CONFLICT(workspace_id) DO UPDATE SET user_id = excluded.user_id")
        .bind(&ws)
        .bind(&target)
        .bind(crate::platform::db::now_ms())
        .execute(&mut *tx)
        .await?;
    audit::record(&mut tx, Entry { workspace_id: &ws, actor: Actor::User(&user.id), action: "ownership.transferred", entity_type: "workspace", entity_id: &ws, message_id: None, metadata: json!({ "to": target }) }).await?;
    tx.commit().await?;
    Ok(Json(json!({ "super_admin": body.email.trim().to_lowercase() })))
}
