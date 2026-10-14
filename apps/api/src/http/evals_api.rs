//! Evals and autonomy: run the safety, replay and live suites against an agent, read their
//! history, see why an agent is at its level and what it still needs, and promote it when the
//! checklist is met. Reading needs membership; changing needs an admin (or, for a personal agent,
//! its owner). Nothing here shows model names.

use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::AppState;
use crate::{
    agent_profiles,
    agent_registry::{
        seed,
        store::{self, AgentRow},
        versions,
    },
    agents::autonomy::{self, LevelInputs},
    auth::{AuthUser, require_role},
    db::iso,
    domain::Role,
    error::{AppError, AppResult},
    evals::{self, gate, run},
    metrics::rollup,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/workspaces/{ws}/agents/{id}/evals", get(list_runs).post(start_run))
        .route("/workspaces/{ws}/eval-runs/{id}", get(run_detail))
        .route("/workspaces/{ws}/agents/{id}/autonomy", get(autonomy_state).post(change_level))
        .route("/workspaces/{ws}/agents/{id}/versions/{version}/gate-override", post(override_gate))
}

async fn visible_agent(state: &AppState, ws: &str, user: &AuthUser, id: &str) -> AppResult<AgentRow> {
    require_role(&state.db, ws, user, Role::Operator).await?;
    seed::ensure_workspace(&state.db, ws).await?;
    store::get_in(&state.db, ws, id).await?.filter(|a| store::visible_to(a, &user.id)).ok_or(AppError::NotFound)
}

async fn editable_agent(state: &AppState, ws: &str, user: &AuthUser, id: &str) -> AppResult<AgentRow> {
    let agent = visible_agent(state, ws, user, id).await?;
    let role = require_role(&state.db, ws, user, Role::Operator).await?;
    if agent.is_personal() || role == Role::Admin {
        return Ok(agent);
    }
    Err(AppError::Forbidden)
}

async fn baseline_score(state: &AppState, ws: &str, r: &run::EvalRun) -> sqlx::Result<Option<f64>> {
    let Some(id) = r.baseline_run_id.as_deref() else { return Ok(None) };
    sqlx::query_scalar("SELECT score FROM eval_runs WHERE id = ?1 AND workspace_id = ?2").bind(id).bind(ws).fetch_optional(&state.db).await.map(Option::flatten)
}

// ---------------------------------------------------------------- runs

async fn list_runs(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>) -> AppResult<Json<Value>> {
    let agent = visible_agent(&state, &ws, &user, &id).await?;
    let runs = run::list(&state.db, &ws, &id, 50).await?;
    let mut items = Vec::new();
    for r in &runs {
        items.push(r.json(baseline_score(&state, &ws, r).await?));
    }
    let versions: Vec<Value> = versions::list(&state.db, &id).await?.iter().map(|v| v.info(agent.active_version)).collect();
    Ok(Json(json!({ "items": items, "versions": versions, "live_available": live_available(&state) })))
}

fn live_available(state: &AppState) -> bool {
    !state.config.openrouter_api_key.trim().is_empty() && state.config.evals_live_max_usd > 0.0
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartRun {
    suite: String,
}

async fn start_run(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>, Json(body): Json<StartRun>) -> AppResult<Json<Value>> {
    let agent = editable_agent(&state, &ws, &user, &id).await?;
    if !evals::SUITES.contains(&body.suite.as_str()) {
        return Err(AppError::BadRequest(format!("suite must be one of {}", evals::SUITES.join(", "))));
    }
    if body.suite == "golden_live" && !live_available(&state) {
        return Err(AppError::Conflict("the live suite needs the AI service and a budget for evals".into()));
    }
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let version = agent.active_version.unwrap_or(0);
    let run_id = run::enqueue(&mut tx, &ws, &id, Some(version), &body.suite, "manual", Some(&user.id)).await?;
    tx.commit().await?;
    state.jobs_notify.notify_one();
    let r = run::get(&state.db, &ws, &run_id).await?.ok_or(AppError::NotFound)?;
    Ok(Json(r.json(None)))
}

async fn run_detail(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let r = run::get(&state.db, &ws, &id).await?.ok_or(AppError::NotFound)?;
    if let Some(agent_id) = r.agent_id.as_deref() {
        visible_agent(&state, &ws, &user, agent_id).await?;
    }
    let results: Vec<Value> = run::results(&state.db, &id)
        .await?
        .into_iter()
        .map(|c| {
            let detail: Value = serde_json::from_str(&c.detail_json).unwrap_or(Value::Null);
            json!({ "case_key": c.case_key, "passed": c.passed, "score": c.score, "detail": detail })
        })
        .collect();
    let mut out = r.json(baseline_score(&state, &ws, &r).await?);
    out["results"] = json!(results);
    Ok(Json(out))
}

// ---------------------------------------------------------------- autonomy

async fn profile_asks_first(state: &AppState, ws: &str, a: &AgentRow) -> sqlx::Result<bool> {
    let Some(owner) = a.owner_user_id.as_deref() else { return Ok(false) };
    Ok(!agent_profiles::user_profile(&state.db, ws, owner).await?.is_some_and(|p| p.routine_ok()))
}

const LEVEL_NAMES: [&str; 4] = ["Observe only", "Draft for approval", "Send routine replies", "Send most replies"];

fn limit_text(limited_by: &str) -> &'static str {
    match limited_by {
        "earned" => "what it has earned so far",
        "admin_cap" => "the limit an admin set",
        "spec" => "what its design allows",
        "workspace" => "the limit set for this installation",
        "profile" => "the mailbox owner's choice to approve replies first",
        "paused" => "being paused",
        _ => "a limit",
    }
}

async fn state_json(state: &AppState, ws: &str, agent: &AgentRow) -> AppResult<Value> {
    rollup::run_recent(&state.db, Some(ws)).await?;
    let (spec, version) = versions::active_spec_in(&state.db, agent).await?;
    let effective = autonomy::effective_level(LevelInputs {
        earned: agent.autonomy_level,
        cap: agent.autonomy_cap,
        spec_max: i64::from(spec.permissions.max_autonomy),
        env_max: state.config.agent_max_autonomy,
        profile_asks_first: profile_asks_first(state, ws, agent).await?,
        active: agent.status == "active",
    });
    let stats = gate::stats(&state.db, &state.config, ws, agent).await?;
    let next_level = agent.autonomy_level + 1;
    let next = (next_level <= 3).then(|| json!({ "level": next_level, "name": LEVEL_NAMES[next_level as usize], "requirements": gate::requirements(next_level, &stats) }));
    let history: Vec<(i64, i64, String, String, i64)> = sqlx::query_as(
        "SELECT from_level, to_level, reason, actor, created_at FROM autonomy_changes WHERE workspace_id = ?1 AND agent_id = ?2 ORDER BY created_at DESC, id DESC LIMIT 30",
    )
    .bind(ws)
    .bind(&agent.id)
    .fetch_all(&state.db)
    .await?;
    let version_row = versions::get(&state.db, &agent.id, version).await?;
    let mut why = vec![format!("Level {} ({}) is the highest this agent has earned.", agent.autonomy_level, LEVEL_NAMES[agent.autonomy_level.clamp(0, 3) as usize])];
    if effective.level < agent.autonomy_level || effective.limited_by != "earned" {
        why.push(format!("It works at level {} because of {}.", effective.level, limit_text(effective.limited_by)));
    }
    if let Some(row) = &version_row
        && matches!(row.gate_status.as_str(), "pending" | "failed")
    {
        why.push(if row.gate_status == "pending" {
            "The current version is still being checked, so it stays at level 1 or below.".to_string()
        } else {
            "The current version failed its checks and is held back until it passes.".to_string()
        });
    }
    if let Some(n) = &next {
        let unmet: Vec<String> = n["requirements"].as_array().into_iter().flatten().filter(|r| r["met"] == false).filter_map(|r| r["name"].as_str().map(str::to_string)).collect();
        why.push(if unmet.is_empty() { format!("It is ready for level {next_level}.") } else { format!("Not ready for level {next_level} yet: {}.", unmet.join("; ")) });
    }
    Ok(json!({
        "earned": agent.autonomy_level, "cap": agent.autonomy_cap, "effective": effective.level, "limited_by": effective.limited_by,
        "level_name": LEVEL_NAMES[agent.autonomy_level.clamp(0, 3) as usize], "why": why,
        "version": { "number": version, "gate_status": version_row.map(|v| v.gate_status) },
        "next": next,
        "history": history.into_iter().map(|(from, to, reason, actor, at)| json!({
            "from": from, "to": to, "reason": reason, "actor": if actor == "system" { "system" } else { "person" }, "created_at": iso(at),
        })).collect::<Vec<_>>(),
    }))
}

async fn autonomy_state(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>) -> AppResult<Json<Value>> {
    let agent = visible_agent(&state, &ws, &user, &id).await?;
    Ok(Json(state_json(&state, &ws, &agent).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangeLevel {
    level: i64,
}

/// Promotes by one level when the checklist is met; demotes to any lower level at any time.
async fn change_level(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>, Json(body): Json<ChangeLevel>) -> AppResult<Json<Value>> {
    let agent = editable_agent(&state, &ws, &user, &id).await?;
    if !(0..=3).contains(&body.level) {
        return Err(AppError::BadRequest("level must be 0 to 3".into()));
    }
    if body.level == agent.autonomy_level {
        return Ok(Json(state_json(&state, &ws, &agent).await?));
    }
    if body.level < agent.autonomy_level {
        let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
        gate::demote(&mut tx, &ws, &id, body.level, "Lowered by a person", json!({}), &format!("user:{}", user.id)).await?;
        tx.commit().await?;
    } else {
        rollup::run_recent(&state.db, Some(&ws)).await?;
        match gate::promote(&state.db, &state.config, &ws, &agent, body.level, &user.id).await? {
            Ok(()) => {}
            Err(gate::PromoteError::NotNextLevel) => return Err(AppError::BadRequest("levels are earned one at a time".into())),
            Err(gate::PromoteError::Unmet(reqs)) => {
                if reqs.iter().any(|r| r.name.starts_with("Safety checks") && r.actual == "not run yet") {
                    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
                    run::enqueue(&mut tx, &ws, &id, Some(agent.active_version.unwrap_or(0)), "safety", "promotion", Some(&user.id)).await?;
                    tx.commit().await?;
                    state.jobs_notify.notify_one();
                }
                let first = reqs.iter().find(|r| !r.met).map(|r| format!("{}: needs {}, now {}", r.name, r.target, r.actual)).unwrap_or_default();
                return Err(AppError::Conflict(format!("Not ready for level {} yet. {first}", body.level)));
            }
        }
    }
    let agent = store::get_in(&state.db, &ws, &id).await?.ok_or(AppError::NotFound)?;
    Ok(Json(state_json(&state, &ws, &agent).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Override {
    #[serde(default)]
    note: String,
}

/// An admin accepts a version whose replay fell short. It stays capped at level 1. A failed safety
/// run cannot be overridden.
async fn override_gate(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id, version)): Path<(String, String, i64)>,
    Json(body): Json<Override>,
) -> AppResult<Json<Value>> {
    let agent = visible_agent(&state, &ws, &user, &id).await?;
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    let note = crate::agents::sentence(&body.note, 240);
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    match gate::override_replay(&mut tx, &ws, &agent.id, version, &user.id, &note).await? {
        Ok(()) => {}
        Err(gate::OverrideError::NoSuchVersion) => return Err(AppError::NotFound),
        Err(gate::OverrideError::NotFailed) => return Err(AppError::Conflict("only a version that failed its checks can be accepted".into())),
        Err(gate::OverrideError::SafetyFailed) => return Err(AppError::Conflict("a version that did not pass the safety checks cannot be accepted".into())),
    }
    tx.commit().await?;
    let agent = store::get_in(&state.db, &ws, &id).await?.ok_or(AppError::NotFound)?;
    Ok(Json(state_json(&state, &ws, &agent).await?))
}
