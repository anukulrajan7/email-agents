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
    audit::{self, Actor, Entry},
    auth::AuthUser,
    error::{AppError, AppResult},
    packs,
    team::{
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
}

fn bad(e: people::FieldError) -> AppError {
    AppError::BadRequest(format!("{}: {}", e.field, e.message))
}

async fn context(state: &AppState, ws: &str) -> AppResult<Context> {
    let staffing = packs::enabled(&state.db, ws, packs::STAFFING).await?;
    Ok(Context { staffing, hr_source_id: settings::load(&state.db, ws).await?.hr_source_id })
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

fn settings_json(s: &settings::TeamSettings, staffing: bool, sources: Vec<Value>) -> Value {
    let mut v = json!({
        "tz_offset_minutes": s.tz_offset_minutes, "manager_after_hours": s.manager_after_hours, "final_after_hours": s.final_after_hours,
        "final_emails": s.final_emails, "below_target_hour": s.below_target_hour, "weekly_check_hour": s.weekly_check_hour,
    });
    if staffing {
        v["hr_source_id"] = json!(s.hr_source_id);
        v["client_domains"] = json!(s.client_domains);
        v["timesheet_grace_days"] = json!(s.timesheet_grace_days);
        v["hours_tolerance"] = json!(s.hours_tolerance);
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
    access::resolve(&state.db, &ws, &user).await?;
    let staffing = packs::enabled(&state.db, &ws, packs::STAFFING).await?;
    let s = settings::load(&state.db, &ws).await?;
    let sources = if staffing { source_options(&state, &ws).await? } else { vec![] };
    Ok(Json(settings_json(&s, staffing, sources)))
}

pub async fn put_settings(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>, Json(body): Json<SettingsInput>) -> AppResult<Json<Value>> {
    access::resolve(&state.db, &ws, &user).await?.require_owner()?;
    let staffing = packs::enabled(&state.db, &ws, packs::STAFFING).await?;
    let current = settings::load(&state.db, &ws).await?;
    let next = body.validated(&current, staffing).map_err(AppError::BadRequest)?;
    if let Some(src) = &next.hr_source_id {
        let ok: Option<i64> = sqlx::query_scalar("SELECT 1 FROM email_sources WHERE id = ?1 AND workspace_id = ?2").bind(src).bind(&ws).fetch_optional(&state.db).await?;
        if ok.is_none() {
            return Err(AppError::BadRequest("hr_source_id: unknown mailbox".into()));
        }
    }
    settings::save(&state.db, &ws, &next).await?;
    let mut conn = state.db.acquire().await?;
    audit::record(&mut conn, Entry { workspace_id: &ws, actor: Actor::User(&user.id), action: "team_settings.updated", entity_type: "workspace", entity_id: &ws, message_id: None, metadata: json!({}) }).await?;
    let sources = if staffing { source_options(&state, &ws).await? } else { vec![] };
    Ok(Json(settings_json(&next, staffing, sources)))
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
    let mut tx = state.db.begin().await?;
    people::validate(&mut tx, &ws, None, &draft, &ctx).await.map_err(bad)?;
    let id = people::insert(&mut tx, &ws, &draft).await?;
    audit::record(&mut tx, Entry { workspace_id: &ws, actor: Actor::User(&user.id), action: "person.created", entity_type: "person", entity_id: &id, message_id: None, metadata: json!({}) }).await?;
    tx.commit().await?;
    let p = people::get(&state.db, &ws, &id).await?.ok_or(AppError::NotFound)?;
    Ok((StatusCode::CREATED, Json(shape(p, ctx.staffing))))
}

pub async fn update_person(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>, Json(body): Json<Fields>) -> AppResult<Json<Person>> {
    access::resolve(&state.db, &ws, &user).await?.require_owner()?;
    let ctx = context(&state, &ws).await?;
    let current = people::get(&state.db, &ws, &id).await?.ok_or(AppError::NotFound)?;
    let draft: Draft = people::merge(Draft::from_person(&current), body, &ctx).map_err(bad)?;
    let mut tx = state.db.begin().await?;
    people::validate(&mut tx, &ws, Some(&id), &draft, &ctx).await.map_err(bad)?;
    people::update(&mut tx, &ws, &id, &draft).await?;
    audit::record(&mut tx, Entry { workspace_id: &ws, actor: Actor::User(&user.id), action: "person.updated", entity_type: "person", entity_id: &id, message_id: None, metadata: json!({}) }).await?;
    tx.commit().await?;
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
    let mut tx = state.db.begin().await?;
    people::update(&mut tx, &ws, &id, &draft).await?;
    sqlx::query("UPDATE people SET manager_id = NULL WHERE manager_id = ?1").bind(&id).execute(&mut *tx).await?;
    audit::record(&mut tx, Entry { workspace_id: &ws, actor: Actor::User(&user.id), action: "person.archived", entity_type: "person", entity_id: &id, message_id: None, metadata: json!({}) }).await?;
    tx.commit().await?;
    let _ = ctx;
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
        let mut conn = state.db.acquire().await?;
        audit::record(&mut conn, Entry { workspace_id: &ws, actor: Actor::User(&user.id), action: "people.imported", entity_type: "workspace", entity_id: &ws, message_id: None, metadata: json!({ "created": report.created, "updated": report.updated, "skipped": report.skipped }) }).await?;
    }
    Ok(Json(report))
}

// ---------------------------------------------------------------- metrics

pub async fn my_metrics(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    let a = access::resolve(&state.db, &ws, &user).await?;
    let Some(person_id) = &a.person_id else { return Ok(Json(json!({ "person": null, "metrics": null }))) };
    let m = crate::staffing::metrics::person_metrics(&state.db, &ws, person_id, crate::db::now_ms()).await?;
    Ok(Json(json!({ "person": m.as_ref().map(|m| json!({ "id": m.person_id, "name": m.name })), "metrics": m })))
}

pub async fn person_metrics(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>) -> AppResult<Json<Value>> {
    let a = access::resolve(&state.db, &ws, &user).await?;
    a.require_visible(&id)?;
    let m = crate::staffing::metrics::person_metrics(&state.db, &ws, &id, crate::db::now_ms()).await?.ok_or(AppError::NotFound)?;
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
    let t = crate::staffing::metrics::team_metrics(&state.db, &ws, scope.as_deref(), crate::db::now_ms()).await?;
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
        let steps = escalation::chain(&state.db, &ws, &p.id, &alert_settings.recipients).await?;
        rows.push(json!({ "person": { "id": p.id, "name": p.name, "role": p.role }, "steps": steps }));
    }
    let default_hours = alert_settings.escalate_after_hours;
    let _ = staffing;
    Ok(Json(json!({
        "manager_after_hours": s.manager_after_hours.unwrap_or(default_hours),
        "final_after_hours": s.final_after_hours.unwrap_or(default_hours),
        "final_emails": if s.final_emails.is_empty() { alert_settings.recipients.clone() } else { s.final_emails.clone() },
        "chains": rows,
        "can_edit": a.is_owner(),
    })))
}
