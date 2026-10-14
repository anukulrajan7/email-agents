//! What the agents learned: style preferences people can accept, edit and archive, the feedback behind them,
//! and the learning gain per agent. Everything is scoped by workspace; another member's personal agent is invisible.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, patch},
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::AppState;
use crate::{
    agent_registry::store::{self, AgentRow},
    auth::{AuthUser, require_role},
    db::now_ms,
    domain::Role,
    error::{AppError, AppResult},
    learning::{
        gain,
        guard::Rejection,
        hooks,
        preferences::{self, COLUMNS, Preference, PreferenceView, Target},
        signals::{self, Filter, Kind, SignalView},
    },
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/workspaces/{ws}/preferences", get(list).post(create))
        .route("/workspaces/{ws}/preferences/{id}", patch(update).delete(remove))
        .route("/workspaces/{ws}/agents/{id}/learning", get(agent_learning))
        .route("/workspaces/{ws}/me/learning", get(my_learning))
        .route("/workspaces/{ws}/feedback", get(feedback))
}

const DAY_MS: i64 = 86_400_000;

fn rejected(r: Rejection) -> AppError {
    AppError::BadRequest(
        match r {
            Rejection::Category => "category must be greeting, sign_off, length, tone, structure, phrasing or process",
            Rejection::Length => "a preference is 8 to 160 characters",
            Rejection::NotOneSentence => "a preference is one sentence",
            Rejection::Sensitive => "a preference can't contain links, numbers, secrets or personal data",
            Rejection::NamesAddress => "a preference can't name an address or a domain",
            Rejection::Action => "a preference describes how a reply is written, not who gets mail or what an agent may do",
            Rejection::Instruction => "a preference can't contain instructions to the assistant",
        }
        .into(),
    )
}

async fn visible_agent(state: &AppState, ws: &str, user: &AuthUser, id: &str) -> AppResult<AgentRow> {
    let agent = store::get_in(&state.db, ws, id).await?.ok_or(AppError::NotFound)?;
    if !store::visible_to(&agent, &user.id) {
        return Err(AppError::NotFound);
    }
    Ok(agent)
}

#[derive(Deserialize)]
pub struct ListQuery {
    agent_id: Option<String>,
    user_id: Option<String>,
    status: Option<String>,
}

async fn list(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>, Query(q): Query<ListQuery>) -> AppResult<Json<Value>> {
    let role = require_role(&state.db, &ws, &user, Role::Operator).await?;
    let status = q.status.filter(|s| !s.is_empty());
    if status.as_deref().is_some_and(|s| !preferences::STATUSES.contains(&s)) {
        return Err(AppError::BadRequest("status must be suggested, active or archived".into()));
    }
    let agent_id = q.agent_id.filter(|a| !a.is_empty());
    if let Some(a) = &agent_id {
        visible_agent(&state, &ws, &user, a).await?;
    }
    let person = q.user_id.filter(|u| !u.is_empty());
    // Members see team-wide preferences and their own; admins see everyone's (except other members' personal agents').
    let own_only = (role != Role::Admin).then_some(user.id.as_str());
    let rows: Vec<Preference> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM agent_preferences
         WHERE workspace_id = ?1 AND (?2 IS NULL OR agent_id = ?2) AND (?3 IS NULL OR user_id = ?3) AND (?4 IS NULL OR status = ?4)
           AND (?5 IS NULL OR user_id IS NULL OR user_id = ?5) AND {}
         ORDER BY CASE status WHEN 'suggested' THEN 0 WHEN 'active' THEN 1 ELSE 2 END, updated_at DESC LIMIT 200",
        store::hide_foreign_personal("agent_preferences", "?6")
    ))
    .bind(&ws)
    .bind(agent_id)
    .bind(person)
    .bind(status)
    .bind(own_only)
    .bind(&user.id)
    .fetch_all(&state.db)
    .await?;
    let items: Vec<PreferenceView> = rows.into_iter().map(Into::into).collect();
    Ok(Json(json!({ "items": items })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateBody {
    agent_id: Option<String>,
    /// Omit for a team-wide preference (admins only).
    user_id: Option<String>,
    category: String,
    content: String,
}

async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    Path(ws): Path<String>,
    Json(body): Json<CreateBody>,
) -> AppResult<(StatusCode, Json<PreferenceView>)> {
    let role = require_role(&state.db, &ws, &user, Role::Operator).await?;
    let agent_id = body.agent_id.filter(|a| !a.is_empty());
    if let Some(a) = &agent_id {
        visible_agent(&state, &ws, &user, a).await?;
    }
    let person = body.user_id.filter(|u| !u.is_empty());
    match &person {
        Some(u) if *u != user.id && role != Role::Admin => return Err(AppError::Forbidden),
        None if role != Role::Admin => return Err(AppError::Forbidden),
        _ => {}
    }
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let target = Target { ws: &ws, agent_id: agent_id.as_deref(), user_id: person.as_deref() };
    let id = preferences::create_manual(&mut tx, &target, &body.category, &body.content).await?.map_err(rejected)?;
    let pref = preferences::get(&mut tx, &ws, &id).await?.ok_or(AppError::NotFound)?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(pref.into())))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchBody {
    status: Option<String>,
    content: Option<String>,
    category: Option<String>,
}

/// A preference this member may change: their own, or any team-wide one when they are an admin.
async fn editable(state: &AppState, ws: &str, user: &AuthUser, role: Role, id: &str) -> AppResult<Preference> {
    let mut conn = state.db.acquire().await?;
    let pref = preferences::get(&mut conn, ws, id).await?.ok_or(AppError::NotFound)?;
    if !store::can_see(&mut *conn, ws, pref.agent_id.as_deref(), &user.id).await? {
        return Err(AppError::NotFound);
    }
    let allowed = match &pref.user_id {
        Some(u) => *u == user.id || role == Role::Admin,
        None => role == Role::Admin,
    };
    if allowed { Ok(pref) } else { Err(AppError::Forbidden) }
}

async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
    Json(body): Json<PatchBody>,
) -> AppResult<Json<PreferenceView>> {
    let role = require_role(&state.db, &ws, &user, Role::Operator).await?;
    let pref = editable(&state, &ws, &user, role, &id).await?;
    if body.status.is_none() && body.content.is_none() && body.category.is_none() {
        return Err(AppError::BadRequest("nothing to update".into()));
    }
    if body.status.as_deref().is_some_and(|s| !preferences::STATUSES.contains(&s)) {
        return Err(AppError::BadRequest("status must be suggested, active or archived".into()));
    }
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    if body.content.is_some() || body.category.is_some() {
        let content = body.content.as_deref().unwrap_or(&pref.content);
        match preferences::set_content(&mut tx, &ws, &id, body.category.as_deref(), content).await?.map_err(rejected)? {
            true => {}
            false => return Err(AppError::NotFound),
        }
    }
    if let Some(status) = body.status.as_deref() {
        preferences::set_status(&mut tx, &ws, &id, status).await?;
        let kind = match status {
            "active" if pref.status == "suggested" => Some(Kind::PreferenceAccepted),
            "archived" => Some(Kind::PreferenceRejected),
            _ => None,
        };
        if let Some(kind) = kind {
            hooks::judged(&mut tx, &ws, &user.id, kind, pref.agent_id.as_deref(), json!({"preference_id": id, "category": pref.category})).await?;
        }
    }
    let updated = preferences::get(&mut tx, &ws, &id).await?.ok_or(AppError::NotFound)?;
    tx.commit().await?;
    Ok(Json(updated.into()))
}

async fn remove(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>) -> AppResult<StatusCode> {
    let role = require_role(&state.db, &ws, &user, Role::Operator).await?;
    editable(&state, &ws, &user, role, &id).await?;
    let mut conn = state.db.acquire().await?;
    preferences::delete(&mut conn, &ws, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Facts an agent learned in the last `days`: its own, plus shared ones taught by its drafts.
async fn facts_learned(state: &AppState, ws: &str, agent_id: &str, since: i64) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM memories m
         WHERE m.workspace_id = ?1 AND m.origin = 'auto' AND m.source = 'learned' AND m.created_at >= ?3
           AND (m.agent_id = ?2 OR EXISTS (
             SELECT 1 FROM json_each(m.sources_json) j JOIN drafts d ON d.id = json_extract(j.value, '$.id')
             WHERE COALESCE(d.agent_id, (SELECT r.agent_id FROM message_routes r WHERE r.message_id = d.message_id)) = ?2))",
    )
    .bind(ws)
    .bind(agent_id)
    .bind(since)
    .fetch_one(&state.db)
    .await
}

async fn agent_learning(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>) -> AppResult<Json<Value>> {
    let role = require_role(&state.db, &ws, &user, Role::Operator).await?;
    let agent = visible_agent(&state, &ws, &user, &id).await?;
    let now = now_ms();
    let since = now - 30 * DAY_MS;
    let own_only = (role != Role::Admin).then_some(user.id.as_str());
    let prefs: Vec<Preference> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM agent_preferences WHERE workspace_id = ?1 AND agent_id = ?2 AND status != 'archived'
           AND (?3 IS NULL OR user_id IS NULL OR user_id = ?3)
         ORDER BY CASE status WHEN 'suggested' THEN 0 ELSE 1 END, updated_at DESC LIMIT 200"
    ))
    .bind(&ws)
    .bind(&agent.id)
    .bind(own_only)
    .fetch_all(&state.db)
    .await?;
    let recent = signals::recent(
        &state.db,
        &ws,
        Filter { viewer: &user.id, agent_id: Some(&agent.id), kind: None, person: None, since, limit: 50 },
    )
    .await?;
    let counts = signals::counts_by_kind(&state.db, &ws, Some(&agent.id), since).await?;
    let learning_gain = gain::for_agent(&state.db, &ws, Some(&agent.id), now, 8).await?;
    Ok(Json(json!({
        "preferences": prefs.into_iter().map(PreferenceView::from).collect::<Vec<_>>(),
        "recent_signals": recent.into_iter().map(SignalView::from).collect::<Vec<_>>(),
        "counts_by_kind_30d": counts.into_iter().collect::<std::collections::BTreeMap<_, _>>(),
        "facts_learned_30d": facts_learned(&state, &ws, &agent.id, since).await?,
        "learning_gain": learning_gain,
    })))
}

/// What the agents learned about the caller: their preferences, facts only they can see, and the signals they gave.
async fn my_learning(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let prefs: Vec<Preference> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM agent_preferences WHERE workspace_id = ?1 AND user_id = ?2 AND status != 'archived' ORDER BY updated_at DESC LIMIT 200"
    ))
    .bind(&ws)
    .bind(&user.id)
    .fetch_all(&state.db)
    .await?;
    let facts: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT m.id, m.content, m.status FROM memories m JOIN agents a ON a.id = m.agent_id
         WHERE m.workspace_id = ?1 AND a.scope = 'personal' AND a.owner_user_id = ?2 AND m.status != 'archived'
         ORDER BY m.updated_at DESC LIMIT 100",
    )
    .bind(&ws)
    .bind(&user.id)
    .fetch_all(&state.db)
    .await?;
    let recent = signals::recent(
        &state.db,
        &ws,
        Filter { viewer: &user.id, agent_id: None, kind: None, person: Some(&user.id), since: now_ms() - 30 * DAY_MS, limit: 50 },
    )
    .await?;
    Ok(Json(json!({
        "preferences": prefs.into_iter().map(PreferenceView::from).collect::<Vec<_>>(),
        "facts": facts.into_iter().map(|(id, content, status)| json!({"id": id, "content": content, "status": status})).collect::<Vec<_>>(),
        "recent_signals": recent.into_iter().map(SignalView::from).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
pub struct FeedbackQuery {
    agent_id: Option<String>,
    kind: Option<String>,
    days: Option<i64>,
}

async fn feedback(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>, Query(q): Query<FeedbackQuery>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    let kind = q.kind.filter(|k| !k.is_empty());
    if kind.as_deref().is_some_and(|k| Kind::parse(k).is_none()) {
        return Err(AppError::BadRequest("unknown kind".into()));
    }
    let agent_id = q.agent_id.filter(|a| !a.is_empty());
    let since = now_ms() - q.days.unwrap_or(30).clamp(1, 365) * DAY_MS;
    let rows = signals::recent(
        &state.db,
        &ws,
        Filter { viewer: &user.id, agent_id: agent_id.as_deref(), kind: kind.as_deref(), person: None, since, limit: 200 },
    )
    .await?;
    Ok(Json(json!({ "items": rows.into_iter().map(SignalView::from).collect::<Vec<_>>() })))
}
