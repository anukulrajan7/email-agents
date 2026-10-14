//! Agents API: cases with their step timeline, and the approvals inbox.
//! Operators read, start cases and decide approvals; admins cancel cases.

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
    agents::{
        approvals,
        case::{self, Case, KINDS, NewStep},
        triggers,
    },
    audit::{self, Actor, Entry},
    auth::{AuthUser, require_role},
    db::{iso, now_ms},
    domain::Role,
    error::{AppError, AppResult},
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/workspaces/{ws}/cases", get(list_cases).post(create_case))
        .route("/workspaces/{ws}/cases/{id}", get(case_detail))
        .route("/workspaces/{ws}/cases/{id}/cancel", post(cancel_case))
        .route("/workspaces/{ws}/approvals", get(list_approvals))
        .route("/workspaces/{ws}/approvals/{id}", post(decide))
}

const STATUSES: &[&str] = &["running", "waiting_approval", "waiting_event", "done", "failed", "cancelled"];

/// The workspace agent's badge and the people it works for, for decorating cases and approvals.
struct Who {
    agent: Value,
    /// Badge of every agent of the workspace, by id.
    agents: HashMap<String, Value>,
    people: HashMap<String, Value>,
}

impl Who {
    async fn load(state: &AppState, ws: &str) -> AppResult<Self> {
        let agent = agent_profiles::agent(&state.db, &state.config, ws).await?.badge();
        let agents = crate::agent_registry::store::list(&state.db, ws)
            .await?
            .into_iter()
            .map(|a| {
                let badge = if a.kind == "lead" || a.is_personal() { agent.clone() } else { json!({"name": a.name, "avatar": a.avatar, "color": a.color}) };
                (a.id, badge)
            })
            .collect();
        let rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
            "SELECT u.id, u.email, p.display_name FROM memberships ms JOIN users u ON u.id = ms.user_id
             LEFT JOIN user_agent_profiles p ON p.workspace_id = ms.workspace_id AND p.user_id = u.id
             WHERE ms.workspace_id = ?1",
        )
        .bind(ws)
        .fetch_all(&state.db)
        .await?;
        let people = rows
            .into_iter()
            .map(|(id, email, name)| {
                let name = name.filter(|n| !n.trim().is_empty());
                (id.clone(), json!({ "user_id": id, "email": email, "name": name }))
            })
            .collect();
        Ok(Self { agent, agents, people })
    }

    fn working_for(&self, user_id: Option<&str>) -> Value {
        user_id.and_then(|u| self.people.get(u)).cloned().unwrap_or(Value::Null)
    }

    fn badge(&self, agent_id: Option<&str>) -> Value {
        agent_id.and_then(|id| self.agents.get(id)).unwrap_or(&self.agent).clone()
    }

    fn case(&self, c: &Case) -> Value {
        let mut v = summary_json(c);
        v["agent"] = self.badge(c.agent_id.as_deref());
        v["working_for"] = self.working_for(c.agent_user_id.as_deref());
        v
    }

    fn approval(&self, a: &approvals::ApprovalRow) -> Value {
        let mut v = a.to_json();
        v["agent"] = self.badge(a.agent_id.as_deref());
        v["working_for"] = self.working_for(a.agent_user_id.as_deref());
        v
    }
}

/// A case of this workspace that the member may see: another member's personal agent's cases are not found.
async fn visible_case(state: &AppState, ws: &str, user_id: &str, id: &str) -> AppResult<Case> {
    let c = case::get_in(&state.db, ws, id).await?.ok_or(AppError::NotFound)?;
    if !crate::agent_registry::store::can_see(&state.db, ws, c.agent_id.as_deref(), user_id).await? {
        return Err(AppError::NotFound);
    }
    Ok(c)
}

fn summary_json(c: &Case) -> Value {
    json!({
        "id": c.id, "kind": c.kind, "title": c.title, "goal": c.goal, "status": c.status, "outcome": c.outcome,
        "steps": c.step_count, "max_steps": c.max_steps,
        "spent_usd": (c.spent_usd * 10_000.0).round() / 10_000.0, "budget_usd": c.budget_usd,
        "message_id": c.message_id, "requirement_id": c.requirement_id,
        "agent_id": c.agent_id, "shadow": c.shadow, "trigger": c.trigger,
        "next_wake_at": c.next_wake_at.map(iso), "created_at": iso(c.created_at), "updated_at": iso(c.updated_at),
    })
}

#[derive(Deserialize)]
pub struct CaseQuery {
    status: Option<String>,
    kind: Option<String>,
    agent_id: Option<String>,
    /// Observing agents' trial cases are left out unless asked for.
    include_shadow: Option<bool>,
}

async fn list_cases(
    State(state): State<AppState>,
    user: AuthUser,
    Path(ws): Path<String>,
    Query(q): Query<CaseQuery>,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let status = q.status.filter(|s| !s.is_empty());
    let kind = q.kind.filter(|k| !k.is_empty());
    if status.as_deref().is_some_and(|s| !STATUSES.contains(&s)) {
        return Err(AppError::BadRequest("unknown case status".into()));
    }
    if kind.as_deref().is_some_and(|k| !KINDS.contains(&k)) {
        return Err(AppError::BadRequest("kind must be requirement or inbox".into()));
    }
    let rows: Vec<Case> = sqlx::query_as(&format!(
        "SELECT {} FROM cases WHERE workspace_id = ?1 AND (?2 IS NULL OR status = ?2) AND (?3 IS NULL OR kind = ?3)
           AND (?4 IS NULL OR agent_id = ?4) AND (?5 = 1 OR shadow = 0) AND {}
         ORDER BY updated_at DESC, id DESC LIMIT 200",
        case::CASE_COLUMNS,
        crate::agent_registry::store::hide_foreign_personal("cases", "?6")
    ))
    .bind(&ws)
    .bind(status)
    .bind(kind)
    .bind(q.agent_id.filter(|a| !a.is_empty()))
    .bind(q.include_shadow.unwrap_or(false))
    .bind(&user.id)
    .fetch_all(&state.db)
    .await?;
    let who = Who::load(&state, &ws).await?;
    Ok(Json(json!({ "items": rows.iter().map(|c| who.case(c)).collect::<Vec<_>>() })))
}

async fn case_detail(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let c = visible_case(&state, &ws, &user.id, &id).await?;
    let who = Who::load(&state, &ws).await?;
    let steps: Vec<Value> = case::steps(&state.db, &ws, &c.id)
        .await?
        .into_iter()
        .map(|s| {
            json!({
                "seq": s.seq, "kind": s.kind, "tool": s.tool, "summary": s.summary,
                "input": serde_json::from_str::<Value>(&s.input_json).unwrap_or(Value::Null),
                "output": serde_json::from_str::<Value>(&s.output_json).unwrap_or(Value::Null),
                "status": s.status, "cost_usd": s.cost_usd, "created_at": iso(s.created_at),
            })
        })
        .collect();
    let approvals: Vec<Value> =
        approvals::list(&state.db, &ws, &user.id, None, Some(&c.id)).await?.iter().rev().map(|a| who.approval(a)).collect();
    let mut detail = who.case(&c);
    // In the detail, `steps` is the timeline itself (the count is `steps.length`).
    detail["steps"] = json!(steps);
    detail["state"] = c.state();
    detail["approvals"] = json!(approvals);
    detail["status_note"] = json!(c.status_note);
    detail["failure_reason"] = json!(c.failure_reason);
    Ok(Json(detail))
}

#[derive(Deserialize)]
pub struct NewCaseBody {
    kind: String,
    requirement_id: Option<String>,
    message_id: Option<String>,
}

async fn create_case(
    State(state): State<AppState>,
    user: AuthUser,
    Path(ws): Path<String>,
    Json(body): Json<NewCaseBody>,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let started = match body.kind.as_str() {
        "requirement" => {
            let id = body.requirement_id.as_deref().filter(|s| !s.is_empty()).ok_or_else(|| AppError::BadRequest("requirement_id is required".into()))?;
            ensure_enabled(&state)?;
            triggers::start_requirement_case(&state, &ws, id, Some(&user.id)).await?
        }
        "inbox" => {
            let id = body.message_id.as_deref().filter(|s| !s.is_empty()).ok_or_else(|| AppError::BadRequest("message_id is required".into()))?;
            ensure_enabled(&state)?;
            triggers::start_inbox_case(&state, &ws, id, Some(&user.id)).await?
        }
        _ => return Err(AppError::BadRequest("kind must be requirement or inbox".into())),
    };
    let case_id = match started {
        triggers::Started::Case(id, _) => id,
        triggers::Started::NotFound => return Err(AppError::NotFound),
        triggers::Started::NotAllowed(why) => return Err(AppError::Conflict(why)),
    };
    let c = visible_case(&state, &ws, &user.id, &case_id).await?;
    Ok(Json(Who::load(&state, &ws).await?.case(&c)))
}

fn ensure_enabled(state: &AppState) -> AppResult<()> {
    if state.planner.enabled() {
        Ok(())
    } else {
        Err(AppError::Conflict("the assistant is not set up yet; ask an admin to configure the AI service".into()))
    }
}

async fn cancel_case(State(state): State<AppState>, user: AuthUser, Path((ws, id)): Path<(String, String)>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    let c = visible_case(&state, &ws, &user.id, &id).await?;
    let mut tx = state.db.begin().await?;
    if !case::set_status(&mut tx, &ws, &c.id, "cancelled").await? {
        return Err(AppError::Conflict("this case already ended".into()));
    }
    let now = now_ms();
    sqlx::query("UPDATE cases SET next_wake_at = NULL WHERE id = ?1").bind(&c.id).execute(&mut *tx).await?;
    sqlx::query(
        "UPDATE agent_approvals SET status = 'declined', decided_by = ?1, decided_at = ?2, note = 'Case cancelled'
         WHERE case_id = ?3 AND status = 'pending'",
    )
    .bind(&user.id)
    .bind(now)
    .bind(&c.id)
    .execute(&mut *tx)
    .await?;
    if let (Some(agent_id), Some(message_id)) = (c.agent_id.as_deref(), c.message_id.as_deref()) {
        crate::agent_registry::store::release_held_sends(&mut tx, &ws, Some(agent_id), Some(message_id)).await?;
    }
    case::insert_step(&mut tx, &c, NewStep::note(c.step_count, "Cancelled by a person", "ok")).await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &ws,
            actor: Actor::User(&user.id),
            action: "case.cancelled",
            entity_type: "case",
            entity_id: &c.id,
            message_id: c.message_id.as_deref(),
            metadata: json!({ "kind": c.kind, "steps": c.step_count }),
        },
    )
    .await?;
    tx.commit().await?;
    let c = case::get_in(&state.db, &ws, &id).await?.ok_or(AppError::NotFound)?;
    Ok(Json(Who::load(&state, &ws).await?.case(&c)))
}

#[derive(Deserialize)]
pub struct ApprovalQuery {
    status: Option<String>,
}

async fn list_approvals(
    State(state): State<AppState>,
    user: AuthUser,
    Path(ws): Path<String>,
    Query(q): Query<ApprovalQuery>,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let status = q.status.filter(|s| !s.is_empty());
    if status.as_deref().is_some_and(|s| !matches!(s, "pending" | "approved" | "declined")) {
        return Err(AppError::BadRequest("status must be pending, approved or declined".into()));
    }
    let who = Who::load(&state, &ws).await?;
    let items: Vec<Value> = approvals::list(&state.db, &ws, &user.id, status.as_deref(), None).await?.iter().map(|a| who.approval(a)).collect();
    let pending_count = approvals::pending_count(&state.db, &ws, &user.id).await?;
    Ok(Json(json!({ "items": items, "pending_count": pending_count })))
}

#[derive(Deserialize)]
pub struct DecisionBody {
    decision: String,
    edited_body: Option<String>,
    note: Option<String>,
}

async fn decide(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
    Json(body): Json<DecisionBody>,
) -> AppResult<Json<Value>> {
    let role = require_role(&state.db, &ws, &user, Role::Operator).await?;
    let pending = approvals::get(&state.db, &ws, &user.id, &id).await?.ok_or(AppError::NotFound)?;
    // Creating an automation is an admin's call, like in the Automations page.
    if pending.action == "create_automation" && role != Role::Admin && body.decision == "approve" {
        return Err(AppError::Forbidden);
    }
    let approve = match body.decision.as_str() {
        "approve" => true,
        "decline" => false,
        _ => return Err(AppError::BadRequest("decision must be approve or decline".into())),
    };
    let row = approvals::decide(
        &state,
        &ws,
        &id,
        &user.id,
        approvals::Decision { approve, edited_body: body.edited_body.as_deref(), note: body.note.as_deref() },
    )
    .await?;
    Ok(Json(Who::load(&state, &ws).await?.approval(&row)))
}
