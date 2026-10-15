//! Agent metrics: one scorecard per agent and one for the workspace. Reading needs membership; a
//! member's personal agent is visible to that member alone.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::get,
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::AppState;
use crate::{
    agents::registry::{
        seed,
        store::{self, AgentRow},
    },
    platform::auth::{AuthUser, require_role},
    platform::domain::Role,
    platform::error::{AppError, AppResult},
    evals::{gate, run},
    metrics::{Counts, Totals, queries, rollup},
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/workspaces/{ws}/agents/{id}/metrics", get(agent_metrics))
        .route("/workspaces/{ws}/metrics/agents", get(all_agents))
        .route("/workspaces/{ws}/scorecard", get(scorecard))
}

#[derive(Deserialize)]
pub struct DaysQuery {
    days: Option<i64>,
}

pub(super) fn identity(a: &AgentRow) -> Value {
    json!({
        "id": a.id, "key": a.key, "kind": a.kind, "scope": a.scope, "category": a.category, "name": a.name, "avatar": a.avatar,
        "color": a.color, "status": a.status, "active_version": a.active_version,
    })
}

/// Agents this member may see, after making sure the workspace has its agents.
async fn visible_agents(state: &AppState, ws: &str, user: &AuthUser) -> AppResult<Vec<AgentRow>> {
    require_role(&state.db, ws, user, Role::Operator).await?;
    seed::ensure_workspace(&state.db, ws).await?;
    Ok(store::list(&state.db, ws).await?.into_iter().filter(|a| store::visible_to(a, &user.id) && a.status != "archived").collect())
}

async fn agent_metrics(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
    Query(q): Query<DaysQuery>,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let agent = store::get_in(&state.db, &ws, &id).await?.filter(|a| store::visible_to(a, &user.id)).ok_or(AppError::NotFound)?;
    let days = queries::clamp_days(q.days);
    rollup::run_recent(&state.db, Some(&ws)).await?;
    let totals = queries::totals(&state.db, &ws, Some(&id), days).await?.remove(&id).unwrap_or_default();
    let series = queries::series(&state.db, &ws, &id, days).await?;
    Ok(Json(json!({ "days": days, "agent": identity(&agent), "totals": totals, "series": series })))
}

async fn all_agents(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>, Query(q): Query<DaysQuery>) -> AppResult<Json<Value>> {
    let agents = visible_agents(&state, &ws, &user).await?;
    let days = queries::clamp_days(q.days);
    rollup::run_recent(&state.db, Some(&ws)).await?;
    let mut totals = queries::totals(&state.db, &ws, None, days).await?;
    let items: Vec<Value> = agents.iter().map(|a| json!({ "agent": identity(a), "totals": totals.remove(&a.id).unwrap_or_default() })).collect();
    Ok(Json(json!({ "days": days, "items": items })))
}

/// The workspace at a glance: everything the visible agents did, and for each agent where it
/// stands on autonomy.
async fn scorecard(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    const DAYS: i64 = 30;
    let agents = visible_agents(&state, &ws, &user).await?;
    rollup::run_recent(&state.db, Some(&ws)).await?;
    let counts = queries::counts(&state.db, &ws, None, DAYS).await?;
    let mut totals = queries::totals(&state.db, &ws, None, DAYS).await?;
    let mut all = Counts::default();
    let mut items = Vec::new();
    for a in &agents {
        if let Some(c) = counts.get(&a.id) {
            all = add(all, c);
        }
        let stats = gate::stats(&state.db, &state.config, &ws, a).await?;
        let next = a.autonomy_level + 1;
        let ready = next <= 3 && gate::all_met(&gate::requirements(next, &stats));
        let safety = run::latest_finished(&state.db, &ws, &a.id, None, "safety").await?;
        items.push(json!({
            "agent": identity(a),
            "totals": totals.remove(&a.id).unwrap_or_default(),
            "autonomy": { "earned": a.autonomy_level, "cap": a.autonomy_cap, "ready_for": ready.then_some(next) },
            "safety": safety.map(|r| json!({ "status": r.status, "score": r.score, "created_at": crate::platform::db::iso(r.created_at) })),
        }));
    }
    Ok(Json(json!({ "days": DAYS, "totals": Totals::from_counts(&all, None, &[]), "agents": items })))
}

fn add(mut a: Counts, b: &Counts) -> Counts {
    a.routed += b.routed;
    a.cases += b.cases;
    a.done += b.done;
    a.failed += b.failed;
    a.cancelled += b.cancelled;
    a.approvals_requested += b.approvals_requested;
    a.approved += b.approved;
    a.edited += b.edited;
    a.declined += b.declined;
    a.drafts_created += b.drafts_created;
    a.drafts_sent += b.drafts_sent;
    a.edit_sum += b.edit_sum;
    a.edit_n += b.edit_n;
    a.auto_sends += b.auto_sends;
    a.stopped += b.stopped;
    a.incidents += b.incidents;
    a.label_corrections += b.label_corrections;
    a.labels_total += b.labels_total;
    a.shadow_sum += b.shadow_sum;
    a.shadow_n += b.shadow_n;
    a.cost_usd += b.cost_usd;
    a
}
