//! Agent registry API: the agents of a workspace, their templates, versions, message routes and
//! outgoing mail. Reading needs membership; changing a workspace agent needs an admin. A personal
//! agent belongs to its owner alone: everyone else gets 404, admins included.

use std::collections::HashMap;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::AppState;
use crate::{
    agent_profiles,
    agent_registry::{
        routing, seed,
        spec::{self, AgentSpec, CATEGORIES, Issue, MAX_AUTONOMY, Trigger},
        store::{self, AgentRow, NewAgent},
        templates::{self, AgentTemplate},
        versions::{self, NewVersion},
    },
    agents::autonomy::{self, LevelInputs},
    audit::{self, Actor, Entry},
    auth::{AuthUser, require_role},
    db::{iso, now_ms},
    domain::Role,
    error::{AppError, AppResult},
    packs,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/workspaces/{ws}/agents", get(list).post(create))
        .route("/workspaces/{ws}/agent-templates", get(template_catalogue))
        .route("/workspaces/{ws}/agents/pause-all", post(pause_all))
        .route("/workspaces/{ws}/agents/{id}", get(detail).patch(update).delete(remove))
        .route("/workspaces/{ws}/agents/{id}/clone", post(clone_agent))
        .route("/workspaces/{ws}/agents/{id}/spec", axum::routing::put(put_spec))
        .route("/workspaces/{ws}/agents/{id}/versions", get(list_versions))
        .route("/workspaces/{ws}/agents/{id}/versions/{version}", get(get_version))
        .route("/workspaces/{ws}/agents/{id}/versions/{version}/activate", post(activate_version))
        .route("/workspaces/{ws}/agents/{id}/outgoing", get(outgoing))
        .route("/workspaces/{ws}/agents/{id}/audit", get(audit_trail))
        .route("/workspaces/{ws}/messages/{id}/route", get(get_route).post(set_route))
}

// ---------------------------------------------------------------- shared pieces

/// An agent the member may see; another member's personal agent is not found.
pub(super) async fn visible_agent(state: &AppState, ws: &str, user: &AuthUser, id: &str) -> AppResult<AgentRow> {
    let agent = store::get_in(&state.db, ws, id).await?.ok_or(AppError::NotFound)?;
    if !store::visible_to(&agent, &user.id) {
        return Err(AppError::NotFound);
    }
    Ok(agent)
}

/// May this member change the agent? A personal agent's owner may; a workspace agent needs an admin.
pub(super) async fn require_edit(state: &AppState, ws: &str, user: &AuthUser, agent: &AgentRow) -> AppResult<()> {
    let role = require_role(&state.db, ws, user, Role::Operator).await?;
    if agent.is_personal() || role == Role::Admin {
        return Ok(());
    }
    Err(AppError::Forbidden)
}

fn category_for_pack(pack: &str) -> &str {
    if pack == packs::STAFFING { "hrms" } else { pack }
}

/// Everything about a spec that depends on the workspace, on top of the basic checks.
async fn validate_in_workspace(state: &AppState, ws: &str, spec: &AgentSpec, ceiling: u8, previous: Option<&AgentSpec>) -> AppResult<Vec<Issue>> {
    let mut issues = spec::validate_basic(spec);
    let enabled = packs::enabled_keys(&state.db, ws).await?;
    let allowed_category = spec.category == "custom" || enabled.iter().any(|p| category_for_pack(p) == spec.category) || spec.category == "general";
    if !allowed_category && CATEGORIES.contains(&spec.category.as_str()) && previous.is_none_or(|p| p.category != spec.category) {
        issues.push(Issue { path: "category".into(), code: "category_not_enabled", message: "that category's pack is not turned on for this workspace".into() });
    }
    let labels: Vec<String> = sqlx::query_scalar("SELECT key FROM labels WHERE workspace_id = ?1 AND archived = 0").bind(ws).fetch_all(&state.db).await?;
    for (i, t) in spec.triggers.iter().enumerate() {
        if let Trigger::EmailReceived { matcher } = t
            && let Some(bad) = matcher.labels.iter().find(|l| !labels.contains(l) && !previous.is_some_and(|p| p.triggers.contains(t)))
        {
            issues.push(Issue { path: format!("triggers[{i}].match.labels"), code: "unknown_label", message: format!("this workspace has no label \"{bad}\"") });
        }
    }
    if !packs::has_feature(&state.db, ws, packs::FEATURE_MATCHING).await?
        && let Some(i) = spec
            .tools
            .iter()
            .position(|t| crate::agent_registry::runtime::MATCHING_TOOLS.contains(&t.as_str()) && !previous.is_some_and(|p| p.has_tool(t)))
    {
        issues.push(Issue { path: format!("tools[{i}]"), code: "tool_needs_staffing", message: "candidate matching needs the HR & recruiting pack".into() });
    }
    if spec.permissions.max_autonomy > ceiling {
        issues.push(Issue { path: "permissions.max_autonomy".into(), code: "above_ceiling", message: format!("this agent can be at most level {ceiling}") });
    }
    Ok(issues)
}

fn reject(issues: &[Issue]) -> AppError {
    AppError::BadRequest(spec::issues_message(issues))
}

#[derive(Default)]
struct Today {
    routed: i64,
    cases: i64,
    approvals_pending: i64,
    cost_usd: f64,
}

async fn today_counts(state: &AppState, ws: &str) -> sqlx::Result<HashMap<String, Today>> {
    const DAY_MS: i64 = 86_400_000;
    let since = now_ms() / DAY_MS * DAY_MS;
    let mut out: HashMap<String, Today> = HashMap::new();
    let routed: Vec<(String, i64)> = sqlx::query_as(
        "SELECT agent_id, COUNT(*) FROM message_routes WHERE workspace_id = ?1 AND created_at >= ?2 GROUP BY agent_id",
    )
    .bind(ws)
    .bind(since)
    .fetch_all(&state.db)
    .await?;
    for (id, n) in routed {
        out.entry(id).or_default().routed = n;
    }
    let cases: Vec<(String, i64, f64)> = sqlx::query_as(
        "SELECT agent_id, COUNT(*), COALESCE(SUM(spent_usd), 0) FROM cases
         WHERE workspace_id = ?1 AND created_at >= ?2 AND agent_id IS NOT NULL GROUP BY agent_id",
    )
    .bind(ws)
    .bind(since)
    .fetch_all(&state.db)
    .await?;
    for (id, n, cost) in cases {
        let t = out.entry(id).or_default();
        t.cases = n;
        t.cost_usd = (cost * 10_000.0).round() / 10_000.0;
    }
    let pending: Vec<(String, i64)> = sqlx::query_as(
        "SELECT agent_id, COUNT(*) FROM agent_approvals WHERE workspace_id = ?1 AND status = 'pending' AND agent_id IS NOT NULL GROUP BY agent_id",
    )
    .bind(ws)
    .fetch_all(&state.db)
    .await?;
    for (id, n) in pending {
        out.entry(id).or_default().approvals_pending = n;
    }
    Ok(out)
}

fn summary(state: &AppState, a: &AgentRow, spec: &AgentSpec, today: Option<&Today>, owner_profile_asks_first: bool) -> Value {
    let level = autonomy::effective_level(LevelInputs {
        earned: a.autonomy_level,
        cap: a.autonomy_cap,
        spec_max: i64::from(spec.permissions.max_autonomy),
        env_max: state.config.agent_max_autonomy,
        profile_asks_first: owner_profile_asks_first,
        active: a.status == "active",
    });
    let update_available = a.tuned
        && a.template_key.as_deref().and_then(templates::template).is_some_and(|t| a.template_version.unwrap_or(0) < i64::from(t.version));
    let zero = Today::default();
    let today = today.unwrap_or(&zero);
    json!({
        "id": a.id, "key": a.key, "kind": a.kind, "scope": a.scope, "owner_user_id": a.owner_user_id, "category": a.category,
        "name": a.name, "avatar": a.avatar, "color": a.color, "description": a.description,
        "status": a.status, "paused_reason": a.paused_reason, "template_key": a.template_key, "tuned": a.tuned,
        "update_available": update_available, "active_version": a.active_version,
        "autonomy": { "earned": a.autonomy_level, "cap": a.autonomy_cap, "effective": level.level, "limited_by": level.limited_by },
        "today": { "routed": today.routed, "cases": today.cases, "approvals_pending": today.approvals_pending, "cost_usd": today.cost_usd },
    })
}

async fn profile_asks_first(state: &AppState, ws: &str, a: &AgentRow) -> sqlx::Result<bool> {
    let Some(owner) = a.owner_user_id.as_deref() else { return Ok(false) };
    Ok(!agent_profiles::user_profile(&state.db, ws, owner).await?.is_some_and(|p| p.routine_ok()))
}

pub(super) async fn detail_json(state: &AppState, ws: &str, a: &AgentRow) -> AppResult<Value> {
    let (spec, _) = versions::active_spec_in(&state.db, a).await?;
    let today = today_counts(state, ws).await?;
    let mut v = summary(state, a, &spec, today.get(&a.id), profile_asks_first(state, ws, a).await?);
    v["spec"] = serde_json::to_value(&spec).map_err(|e| AppError::Internal(e.into()))?;
    v["versions"] = json!(versions::list(&state.db, &a.id).await?.iter().map(|r| r.info(a.active_version)).collect::<Vec<_>>());
    Ok(v)
}

pub(super) async fn audit_change(conn: &mut sqlx::SqliteConnection, ws: &str, user: &AuthUser, action: &str, agent_id: &str, metadata: Value) -> sqlx::Result<()> {
    audit::record(conn, Entry { workspace_id: ws, actor: Actor::User(&user.id), action, entity_type: "agent", entity_id: agent_id, message_id: None, metadata }).await
}

// ---------------------------------------------------------------- list, templates, detail

async fn list(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    seed::ensure_workspace(&state.db, &ws).await?;
    let today = today_counts(&state, &ws).await?;
    let mut by_category: Vec<(&str, Vec<Value>)> = CATEGORIES.iter().map(|c| (*c, vec![])).collect();
    let mut me = None;
    let mut paused_all = false;
    let mut rows: Vec<AgentRow> = store::list(&state.db, &ws).await?.into_iter().filter(|a| store::visible_to(a, &user.id)).collect();
    let position = |a: &AgentRow| templates::TEMPLATES.iter().position(|t| Some(t.key) == a.template_key.as_deref()).unwrap_or(usize::MAX);
    rows.sort_by_key(|a| (a.kind != "lead", position(a), a.created_at));
    for a in rows {
        let (spec, _) = versions::active_spec_in(&state.db, &a).await?;
        paused_all |= a.paused_reason.as_deref() == Some("pause_all");
        let item = summary(&state, &a, &spec, today.get(&a.id), profile_asks_first(&state, &ws, &a).await?);
        if a.is_personal() {
            me = Some(item.clone());
        }
        let at = by_category.iter().position(|(c, _)| *c == a.category).unwrap_or(by_category.len() - 1);
        by_category[at].1.push(item);
    }
    let categories: Vec<Value> = by_category
        .into_iter()
        .filter(|(_, agents)| !agents.is_empty())
        .map(|(key, agents)| json!({"key": key, "name": templates::category_name(key), "agents": agents}))
        .collect();
    Ok(Json(json!({ "paused_all": paused_all, "me": me, "categories": categories })))
}

fn template_json(t: &AgentTemplate, installed: bool, pack_enabled: bool) -> Value {
    let spec = t.spec();
    json!({
        "key": t.key, "category": t.category, "pack": t.pack, "version": t.version, "scope": t.scope,
        "name": t.name, "avatar": t.avatar, "description": spec.description, "mode": spec.mode,
        "max_autonomy": spec.permissions.max_autonomy, "installed": installed, "pack_enabled": pack_enabled, "spec": spec,
    })
}

async fn template_catalogue(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    seed::ensure_workspace(&state.db, &ws).await?;
    let enabled = packs::enabled_keys(&state.db, &ws).await?;
    let installed: Vec<String> = store::list(&state.db, &ws)
        .await?
        .into_iter()
        .filter(|a| store::visible_to(a, &user.id))
        .filter_map(|a| a.template_key)
        .collect();
    let items: Vec<Value> = templates::TEMPLATES
        .iter()
        .map(|t| template_json(t, installed.iter().any(|k| k == t.key), t.pack == packs::GENERAL || enabled.contains(&t.pack)))
        .collect();
    Ok(Json(json!({ "items": items })))
}

async fn detail(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let agent = visible_agent(&state, &ws, &user, &id).await?;
    Ok(Json(detail_json(&state, &ws, &agent).await?))
}

// ---------------------------------------------------------------- change an agent

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentPatch {
    status: Option<String>,
    name: Option<String>,
    avatar: Option<String>,
    color: Option<String>,
    autonomy_cap: Option<i64>,
}

async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
    Json(body): Json<AgentPatch>,
) -> AppResult<Json<Value>> {
    let agent = visible_agent(&state, &ws, &user, &id).await?;
    require_edit(&state, &ws, &user, &agent).await?;
    let is_lead = agent.kind == "lead";
    let mut tx = state.db.begin().await?;
    if let Some(status) = body.status.as_deref() {
        match status {
            "active" => {
                if agent.paused_reason.as_deref() == Some("pack_off") {
                    return Err(AppError::Conflict("turn this agent's pack back on first".into()));
                }
                store::set_status(&mut tx, &ws, &id, "active", None).await?;
            }
            "paused" if is_lead => return Err(AppError::Conflict("the lead cannot be paused here; use pause all".into())),
            "paused" => {
                store::set_status(&mut tx, &ws, &id, "paused", Some("user")).await?;
            }
            "archived" if agent.kind != "custom" => return Err(AppError::Conflict("only custom agents can be archived; pause this one instead".into())),
            "archived" => {
                store::set_status(&mut tx, &ws, &id, "archived", None).await?;
            }
            _ => return Err(AppError::BadRequest("status must be active, paused or archived".into())),
        }
    }
    if body.name.is_some() || body.avatar.is_some() || body.color.is_some() {
        if is_lead {
            return Err(AppError::Conflict("edit the lead on the Agent page".into()));
        }
        let name = match &body.name {
            Some(n) => n.split_whitespace().collect::<Vec<_>>().join(" "),
            None => agent.name.clone(),
        };
        if name.is_empty() || name.chars().count() > spec::MAX_NAME_CHARS {
            return Err(AppError::BadRequest(format!("the name must be 1 to {} characters", spec::MAX_NAME_CHARS)));
        }
        let avatar = match &body.avatar {
            Some(a) => agent_profiles::valid_avatar(a).ok_or_else(|| AppError::BadRequest("the avatar must be one emoji or 1 to 2 letters".into()))?,
            None => agent.avatar.clone(),
        };
        let color = match &body.color {
            Some(c) if agent_profiles::COLORS.contains(&c.as_str()) => c.clone(),
            Some(_) => return Err(AppError::BadRequest(format!("color must be one of {}", agent_profiles::COLORS.join(", ")))),
            None => agent.color.clone(),
        };
        store::set_identity(&mut tx, &ws, &id, &name, &avatar, &color).await?;
    }
    if let Some(cap) = body.autonomy_cap {
        let template_max = agent.template_key.as_deref().and_then(templates::template).map_or(MAX_AUTONOMY, AgentTemplate::max_autonomy);
        let ceiling = if agent.is_personal() { i64::from(template_max) } else { i64::from(MAX_AUTONOMY) };
        if !(0..=ceiling).contains(&cap) {
            return Err(AppError::BadRequest(format!("autonomy_cap must be 0 to {ceiling}")));
        }
        store::set_autonomy_cap(&mut tx, &ws, &id, cap).await?;
    }
    if body.status.as_deref().is_some_and(|s| s != "active") || body.autonomy_cap.is_some_and(|c| c < 2) {
        store::release_held_sends(&mut tx, &ws, Some(&id), None).await?;
    }
    audit_change(
        &mut tx,
        &ws,
        &user,
        "agent.updated",
        &id,
        json!({"status": body.status, "renamed": body.name.is_some(), "autonomy_cap": body.autonomy_cap}),
    )
    .await?;
    tx.commit().await?;
    let agent = visible_agent(&state, &ws, &user, &id).await?;
    Ok(Json(detail_json(&state, &ws, &agent).await?))
}

async fn remove(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>) -> AppResult<Json<Value>> {
    let agent = visible_agent(&state, &ws, &user, &id).await?;
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    if agent.kind != "custom" {
        return Err(AppError::Conflict("lead and system agents cannot be deleted; pause or tune them instead".into()));
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM agents WHERE id = ?1 AND workspace_id = ?2").bind(&id).bind(&ws).execute(&mut *tx).await?;
    audit_change(&mut tx, &ws, &user, "agent.deleted", &id, json!({"key": agent.key})).await?;
    tx.commit().await?;
    Ok(Json(json!({ "deleted": true })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewAgentBody {
    spec: AgentSpec,
    avatar: Option<String>,
    color: Option<String>,
}

async fn insert_custom(
    state: &AppState,
    ws: &str,
    user: &AuthUser,
    spec: &AgentSpec,
    avatar: &str,
    color: &str,
    cloned_from: Option<&str>,
) -> AppResult<AgentRow> {
    let mut tx = state.db.begin().await?;
    let id = store::insert(
        &mut tx,
        NewAgent {
            workspace_id: ws,
            key: &spec.key,
            kind: "custom",
            scope: store::SCOPE_WORKSPACE,
            owner_user_id: None,
            category: &spec.category,
            template_key: None,
            template_version: None,
            cloned_from,
            name: &spec.name,
            avatar,
            color,
            description: &spec.description,
            status: "draft",
            autonomy_level: 0,
            autonomy_cap: 1,
            created_by: Some(&user.id),
        },
    )
    .await?
    .ok_or_else(|| AppError::Conflict(format!("an agent with the key \"{}\" already exists", spec.key)))?;
    versions::create(&mut tx, ws, &id, spec, NewVersion { source: "manual", note: None, created_by: Some(&user.id), designer_session_id: None, activate: true }).await?;
    audit_change(&mut tx, ws, user, "agent.created", &id, json!({"key": spec.key, "cloned_from": cloned_from})).await?;
    tx.commit().await?;
    store::get_in(&state.db, ws, &id).await?.ok_or(AppError::NotFound)
}

pub(super) fn look(avatar: Option<&str>, color: Option<&str>) -> AppResult<(String, String)> {
    let avatar = match avatar {
        Some(a) => agent_profiles::valid_avatar(a).ok_or_else(|| AppError::BadRequest("the avatar must be one emoji or 1 to 2 letters".into()))?,
        None => "🤖".to_string(),
    };
    let color = match color {
        Some(c) if agent_profiles::COLORS.contains(&c) => c.to_string(),
        Some(_) => return Err(AppError::BadRequest(format!("color must be one of {}", agent_profiles::COLORS.join(", ")))),
        None => "slate".to_string(),
    };
    Ok((avatar, color))
}

async fn create(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>, Json(body): Json<NewAgentBody>) -> AppResult<(axum::http::StatusCode, Json<Value>)> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    let issues = validate_in_workspace(&state, &ws, &body.spec, 2, None).await?;
    if !issues.is_empty() {
        return Err(reject(&issues));
    }
    let (avatar, color) = look(body.avatar.as_deref(), body.color.as_deref())?;
    let agent = insert_custom(&state, &ws, &user, &body.spec, &avatar, &color, None).await?;
    Ok((axum::http::StatusCode::CREATED, Json(detail_json(&state, &ws, &agent).await?)))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct CloneBody {
    name: Option<String>,
}

async fn clone_agent(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
    body: Option<Json<CloneBody>>,
) -> AppResult<(axum::http::StatusCode, Json<Value>)> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    let source = visible_agent(&state, &ws, &user, &id).await?;
    if source.is_personal() {
        return Err(AppError::Conflict("personal agents are not cloned".into()));
    }
    let (mut spec, _) = versions::active_spec_in(&state.db, &source).await?;
    let taken: Vec<String> = store::list(&state.db, &ws).await?.into_iter().map(|a| a.key).collect();
    let base = spec.key.clone();
    spec.key = (1..)
        .map(|n| if n == 1 { format!("{base}_copy") } else { format!("{base}_copy{n}") })
        .find(|k| !taken.contains(k))
        .expect("an unused key exists");
    spec.name = body.and_then(|Json(b)| b.name).map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| format!("{} (copy)", spec.name));
    spec.permissions.max_autonomy = spec.permissions.max_autonomy.min(2);
    if !matches!(spec.category.as_str(), "general" | "custom") && !packs::enabled_keys(&state.db, &ws).await?.iter().any(|p| category_for_pack(p) == spec.category) {
        spec.category = "custom".into();
    }
    let issues = validate_in_workspace(&state, &ws, &spec, 2, None).await?;
    if !issues.is_empty() {
        return Err(reject(&issues));
    }
    let agent = insert_custom(&state, &ws, &user, &spec, &source.avatar, &source.color, Some(&source.id)).await?;
    Ok((axum::http::StatusCode::CREATED, Json(detail_json(&state, &ws, &agent).await?)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpecBody {
    spec: AgentSpec,
    note: Option<String>,
}

async fn put_spec(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
    Json(body): Json<SpecBody>,
) -> AppResult<Json<Value>> {
    let agent = visible_agent(&state, &ws, &user, &id).await?;
    require_edit(&state, &ws, &user, &agent).await?;
    let template = agent.template_key.as_deref().and_then(templates::template);
    let ceiling = match (&agent.kind[..], template) {
        ("custom", _) => 2,
        (_, Some(t)) => t.max_autonomy(),
        _ => 2,
    };
    let (previous, _) = versions::active_spec_in(&state.db, &agent).await?;
    let mut issues = validate_in_workspace(&state, &ws, &body.spec, ceiling, Some(&previous)).await?;
    if body.spec.key != agent.key {
        issues.push(Issue { path: "key".into(), code: "key_changed", message: "the key of an existing agent cannot change".into() });
    }
    if !issues.is_empty() {
        return Err(reject(&issues));
    }
    let source = if agent.kind == "custom" { "manual" } else { "tuned" };
    let note = body.note.as_deref().map(agent_note).filter(|n| !n.is_empty());
    let mut tx = state.db.begin().await?;
    let version = versions::create(
        &mut tx,
        &ws,
        &id,
        &body.spec,
        NewVersion { source, note: note.as_deref(), created_by: Some(&user.id), designer_session_id: None, activate: true },
    )
    .await?;
    let capped = versions::cap_after_behaviour_change(&mut tx, &ws, &agent, &previous, &body.spec).await?;
    sqlx::query("UPDATE agents SET tuned = tuned OR ?1, description = ?2, name = CASE WHEN kind = 'custom' THEN ?3 ELSE name END WHERE id = ?4 AND workspace_id = ?5")
        .bind(agent.kind != "custom")
        .bind(&body.spec.description)
        .bind(&body.spec.name)
        .bind(&id)
        .bind(&ws)
        .execute(&mut *tx)
        .await?;
    audit_change(&mut tx, &ws, &user, "agent.spec_saved", &id, json!({"version": version, "source": source, "level_capped": capped})).await?;
    tx.commit().await?;
    let agent = visible_agent(&state, &ws, &user, &id).await?;
    Ok(Json(detail_json(&state, &ws, &agent).await?))
}

fn agent_note(note: &str) -> String {
    crate::agents::sentence(note, 240)
}

// ---------------------------------------------------------------- versions

async fn list_versions(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let agent = visible_agent(&state, &ws, &user, &id).await?;
    let items: Vec<Value> = versions::list(&state.db, &id).await?.iter().map(|r| r.info(agent.active_version)).collect();
    Ok(Json(json!({ "items": items })))
}

#[derive(Deserialize)]
pub struct DiffQuery {
    diff: Option<String>,
}

async fn get_version(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id, version)): Path<(String, String, i64)>,
    Query(q): Query<DiffQuery>,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let agent = visible_agent(&state, &ws, &user, &id).await?;
    let row = versions::get(&state.db, &id, version).await?.ok_or(AppError::NotFound)?;
    let spec = row.spec().ok_or_else(|| AppError::Internal(anyhow::anyhow!("stored spec unreadable")))?;
    let mut v = row.info(agent.active_version);
    v["spec"] = serde_json::to_value(&spec).map_err(|e| AppError::Internal(e.into()))?;
    match q.diff.as_deref() {
        None => {}
        Some("prev") => {
            let before = match versions::get(&state.db, &id, version - 1).await? {
                Some(prev) => prev.spec(),
                None => None,
            };
            v["diff"] = match before {
                Some(before) => json!(versions::diff(&before, &spec)),
                None => Value::Null,
            };
        }
        Some(_) => return Err(AppError::BadRequest("diff must be prev".into())),
    }
    Ok(Json(v))
}

async fn activate_version(State(state): State<AppState>, user: AuthUser, Path((ws, id, version)): Path<(String, String, i64)>) -> AppResult<Json<Value>> {
    let agent = visible_agent(&state, &ws, &user, &id).await?;
    require_edit(&state, &ws, &user, &agent).await?;
    let mut tx = state.db.begin().await?;
    let new_version = match versions::activate(&mut tx, &ws, &agent, version, Some(&user.id)).await? {
        Ok(v) => v,
        Err(versions::ActivateError::NoSuchVersion) => return Err(AppError::NotFound),
        Err(versions::ActivateError::Unreadable) => return Err(AppError::Conflict("that version can no longer be read".into())),
    };
    audit_change(&mut tx, &ws, &user, "agent.version_activated", &id, json!({"from": version, "version": new_version})).await?;
    tx.commit().await?;
    let agent = visible_agent(&state, &ws, &user, &id).await?;
    Ok(Json(detail_json(&state, &ws, &agent).await?))
}

// ---------------------------------------------------------------- pause all

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PauseBody {
    paused: bool,
}

async fn pause_all(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>, Json(body): Json<PauseBody>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    seed::ensure_workspace(&state.db, &ws).await?;
    let mut tx = state.db.begin().await?;
    let affected = if body.paused {
        sqlx::query("UPDATE agents SET status = 'paused', paused_reason = 'pause_all', updated_at = ?2 WHERE workspace_id = ?1 AND status = 'active'")
    } else {
        sqlx::query("UPDATE agents SET status = 'active', paused_reason = NULL, updated_at = ?2 WHERE workspace_id = ?1 AND paused_reason = 'pause_all'")
    }
    .bind(&ws)
    .bind(now_ms())
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if body.paused {
        store::release_held_sends(&mut tx, &ws, None, None).await?;
    }
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &ws,
            actor: Actor::User(&user.id),
            action: "agents.pause_all",
            entity_type: "workspace",
            entity_id: &ws,
            message_id: None,
            metadata: json!({"paused": body.paused, "agents": affected}),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "paused": body.paused, "affected": affected })))
}

// ---------------------------------------------------------------- routes, outgoing, audit

async fn route_json(state: &AppState, ws: &str, message_id: &str) -> AppResult<Value> {
    let row: Option<(String, String, i64, String, bool, i64)> = sqlx::query_as(
        "SELECT r.agent_id, COALESCE(a.name, ''), r.agent_version, r.source, r.shadow, r.created_at
         FROM message_routes r LEFT JOIN agents a ON a.id = r.agent_id
         WHERE r.message_id = ?1 AND r.workspace_id = ?2",
    )
    .bind(message_id)
    .bind(ws)
    .fetch_optional(&state.db)
    .await?;
    let route = row.map(|(agent_id, agent_name, agent_version, source, shadow, at)| {
        json!({"agent_id": agent_id, "agent_name": agent_name, "agent_version": agent_version, "source": source, "shadow": shadow, "created_at": iso(at)})
    });
    Ok(json!({ "message_id": message_id, "route": route }))
}

async fn message_in_workspace(state: &AppState, ws: &str, id: &str) -> AppResult<()> {
    let found: Option<String> = sqlx::query_scalar("SELECT id FROM email_messages WHERE id = ?1 AND workspace_id = ?2").bind(id).bind(ws).fetch_optional(&state.db).await?;
    found.map(|_| ()).ok_or(AppError::NotFound)
}

async fn get_route(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    message_in_workspace(&state, &ws, &id).await?;
    let mut v = route_json(&state, &ws, &id).await?;
    let hidden = match v["route"]["agent_id"].as_str() {
        Some(agent_id) => !store::can_see(&state.db, &ws, Some(agent_id), &user.id).await?,
        None => false,
    };
    if hidden {
        v["route"] = Value::Null;
    }
    Ok(Json(v))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteBody {
    agent_id: String,
}

async fn set_route(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
    Json(body): Json<RouteBody>,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    message_in_workspace(&state, &ws, &id).await?;
    let agent = visible_agent(&state, &ws, &user, &body.agent_id).await?;
    if agent.status != "active" {
        return Err(AppError::Conflict("that agent is not active".into()));
    }
    let candidates = routing::active_agents(&state.db, &state.config, &ws).await?;
    let chosen = candidates.iter().find(|a| a.row.id == agent.id).ok_or_else(|| AppError::Conflict("that agent is not active".into()))?;
    let before: Option<String> = sqlx::query_scalar("SELECT agent_id FROM message_routes WHERE message_id = ?1 AND workspace_id = ?2")
        .bind(&id)
        .bind(&ws)
        .fetch_optional(&state.db)
        .await?;
    let route = routing::Route { agent_id: agent.id.clone(), source: routing::Source::Manual, shadow: chosen.level == 0, handler_id: agent.id.clone() };
    let mut tx = state.db.begin().await?;
    routing::record(&mut tx, &ws, &id, &route, chosen.version).await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &ws,
            actor: Actor::User(&user.id),
            action: "message.route_corrected",
            entity_type: "message_route",
            entity_id: &id,
            message_id: Some(&id),
            metadata: json!({"from_agent_id": before, "to_agent_id": agent.id}),
        },
    )
    .await?;
    crate::learning::hooks::route_corrected(&mut tx, &ws, &user.id, &id, before.as_deref(), &agent.id).await?;
    tx.commit().await?;
    Ok(Json(route_json(&state, &ws, &id).await?))
}

async fn outgoing(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    visible_agent(&state, &ws, &user, &id).await?;
    let rows: Vec<(String, String, String, String, i64, String)> = sqlx::query_as(
        "SELECT d.id, d.message_id, m.subject, m.sender_address, d.send_at, d.body_text
         FROM drafts d JOIN email_messages m ON m.id = d.message_id
         WHERE d.workspace_id = ?1 AND d.agent_id = ?2 AND d.send_at IS NOT NULL AND d.status != 'sent'
         ORDER BY d.send_at LIMIT 100",
    )
    .bind(&ws)
    .bind(&id)
    .fetch_all(&state.db)
    .await?;
    let items: Vec<Value> = rows
        .into_iter()
        .map(|(draft_id, message_id, subject, to, send_at, body)| {
            json!({"draft_id": draft_id, "message_id": message_id, "subject": subject, "to": to, "send_at": iso(send_at),
                   "body_preview": crate::agents::sentence(&body, 200)})
        })
        .collect();
    Ok(Json(json!({ "items": items })))
}

#[derive(Deserialize)]
pub struct AuditQuery {
    limit: Option<i64>,
}

/// What an agent read and did: one entry per tool call (ids and outcomes, never mail text).
async fn audit_trail(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
    Query(q): Query<AuditQuery>,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    visible_agent(&state, &ws, &user, &id).await?;
    let rows: Vec<(String, Option<String>, String, i64)> = sqlx::query_as(
        "SELECT entity_id, message_id, metadata_json, created_at FROM audit_events
         WHERE workspace_id = ?1 AND action = ?2 AND json_extract(metadata_json, '$.agent_id') = ?3
         ORDER BY created_at DESC, id DESC LIMIT ?4",
    )
    .bind(&ws)
    .bind(crate::agents::runner::TOOL_CALL_ACTION)
    .bind(&id)
    .bind(q.limit.unwrap_or(100).clamp(1, 500))
    .fetch_all(&state.db)
    .await?;
    let items: Vec<Value> = rows
        .into_iter()
        .map(|(case_id, message_id, meta, at)| {
            let m: Value = serde_json::from_str(&meta).unwrap_or(Value::Null);
            json!({
                "at": iso(at), "case_id": case_id, "message_id": message_id, "tool": m["tool"], "scope": m["scope"], "status": m["status"],
                "turn": m["turn"], "shadow": m["shadow"], "refs": m["refs"],
            })
        })
        .collect();
    Ok(Json(json!({ "items": items })))
}
