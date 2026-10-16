//! One planner turn: load the case → plan → check → run a tool or ask for approval → record the
//! step (summary, tokens, cost) → schedule the next turn, sleep, wait or finish.

mod evaluate;
mod park;
mod prompt;

pub use prompt::{render_prompt, rule_context};

use serde_json::{Value, json};

use self::{evaluate::evaluate, park::{end_observation, park_while_inactive}};
use super::{
    MAX_ERROR_STREAK,
    autonomy,
    case::{self, Case, NewStep},
    planner::{self, Decision, PlanOutput, PlanRequest, Usage},
    workflow,
    sentence,
    tools::{self, Preview, ToolOutput},
};
use crate::{
    agents::profiles,
    agents::registry::spec::scope_of_tool,
    agents::registry::runtime,
    ai::AiError,
    platform::audit::{self, Actor, Entry},
    platform::db::{new_id, now_ms},
    http::AppState,
    jobs::handlers::JobError,
};

/// Why a case must stop before planning another turn, if it must.
pub fn limit_reached(step_count: i64, max_steps: i64, spent_usd: f64, budget_usd: f64) -> Option<String> {
    if step_count >= max_steps {
        return Some(format!("reached the limit of {max_steps} steps without finishing"));
    }
    if spent_usd >= budget_usd {
        return Some(format!("used this case's AI budget of ${budget_usd:.2} without finishing"));
    }
    None
}

enum TurnResult {
    Ran { tool: String, input: Value, out: ToolOutput, auto_sent: bool },
    Error { tool: Option<String>, input: Value, summary: String, error: String },
    Blocked { tool: String, input: Value, reason: String },
    Approval { tool: String, input: Value, preview: Preview },
    Finish { outcome: Option<String>, summary: String },
    /// The safety check could not run: the same turn is taken again in `minutes` (`count` = deferrals used).
    Defer { minutes: i64, count: u32 },
}

/// Audit action written for every tool call an agent makes (reads included).
pub const TOOL_CALL_ACTION: &str = "agent.tool_call";

/// What the audit trail records about one turn's tool call: the tool, how it ended and its input.
fn audited_call(result: &TurnResult) -> Option<(&str, &'static str, &Value)> {
    match result {
        TurnResult::Ran { tool, input, out, .. } => {
            let simulated = out.value.get("simulated").and_then(Value::as_bool) == Some(true);
            Some((tool, if simulated { "simulated" } else { "ok" }, input))
        }
        TurnResult::Error { tool: Some(tool), input, .. } => Some((tool, "error", input)),
        TurnResult::Blocked { tool, input, .. } => Some((tool, "blocked", input)),
        TurnResult::Approval { tool, input, .. } => Some((tool, "waiting_approval", input)),
        TurnResult::Error { tool: None, .. } | TurnResult::Finish { .. } | TurnResult::Defer { .. } => None,
    }
}

/// Ids a call touched (never bodies), so the trail shows what was read and acted on.
fn call_refs(input: &Value) -> Value {
    let pick = |k: &str| input.get(k).filter(|v| v.is_string() || v.is_array()).cloned();
    json!({
        "message_id": pick("message_id"), "draft_id": pick("draft_id"), "requirement_id": pick("requirement_id"),
        "candidate_ids": pick("candidate_ids"),
    })
}


/// Runs planner turn `turn` of a case. Idempotent: a turn already taken, or a case that is
/// waiting for a person or has ended, is left alone. The case must belong to `ws`.
pub async fn run_step(state: &AppState, ws: &str, case_id: &str, turn: i64) -> Result<(), JobError> {
    let Some(case) = case::get_in(&state.db, ws, case_id).await? else { return Ok(()) };
    if case.is_terminal() || case.status == "waiting_approval" || turn != case.step_count + 1 {
        return Ok(());
    }
    // Over the workspace's daily AI budget: sleep until the next UTC midnight, then take this turn.
    if !crate::ai::ledger::within_budget(&state.db, state.config.ai_daily_budget_usd, &case.workspace_id).await {
        const DAY_MS: i64 = 86_400_000;
        let resume_at = (crate::platform::db::now_ms() / DAY_MS + 1) * DAY_MS;
        let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query("UPDATE cases SET status = 'waiting_event', next_wake_at = ?1, updated_at = ?2 WHERE id = ?3 AND status = 'running'")
            .bind(resume_at)
            .bind(crate::platform::db::now_ms())
            .bind(&case.id)
            .execute(&mut *tx)
            .await?;
        crate::jobs::enqueue_at(
            &mut tx,
            crate::jobs::kind::AGENT_STEP,
            &format!("{}:budget:{}", case::step_key(&case.id, turn), resume_at / DAY_MS),
            Some(&case.workspace_id),
            &serde_json::json!({ "case_id": case.id, "turn": turn }),
            crate::jobs::DEFAULT_MAX_ATTEMPTS,
            resume_at,
        )
        .await?;
        tx.commit().await?;
        tracing::info!(case_id = %case.id, "daily AI budget reached; case resumes tomorrow");
        return Ok(());
    }
    if let Some(reason) = limit_reached(case.step_count, case.max_steps, case.spent_usd, case.budget_usd) {
        let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
        if case.shadow {
            end_observation(&mut tx, &case, &reason).await?;
        } else {
            case::fail(&mut tx, &case, &reason).await?;
        }
        tx.commit().await?;
        return Ok(());
    }

    let (agent, case_agent) = runtime::context_for_case(&state.db, &state.config, &case).await?;
    if !case_agent.is_active() {
        return park_while_inactive(state, &case, &case_agent, turn).await;
    }
    let steps = case::steps(&state.db, &case.workspace_id, &case.id).await?;
    let registry = tools::registry();
    let tier = profiles::effective_tier(&agent.agent.quality, case.premium());
    let allowed = case_agent.allowed_tools(runtime::matching_enabled(&state.db, &case.workspace_id).await?);
    let req = PlanRequest {
        case_id: case.id.clone(),
        kind: if case.shadow { "shadow".into() } else { case.kind.clone() },
        premium: tier == "best",
        requirement_id: case.requirement_id.clone(),
        message_id: case.message_id.clone(),
        facts: case.state(),
        system: planner::system_prompt(&agent.planner_block()),
        model: Some(profiles::tier_model(&state.config, tier).to_string()).filter(|m| !m.is_empty()),
        prompt: render_prompt(&case, &steps, turn),
        tools: registry.specs_for(&allowed),
    };
    let mut handover: Option<Value> = None;
    let mut fixed: Option<Decision> = None;
    if workflow::applies(&case_agent.spec, &case) && workflow::mode_of(&case).as_deref() != Some("planner") {
        let first_turn = workflow::mode_of(&case).is_none();
        let why = if first_turn {
            workflow::escalation_reason(&state.db, &case).await?
        } else {
            case.state().get("declined_draft_id").is_some().then_some("a person declined the draft")
        };
        fixed = if why.is_none() { workflow::decide(&case_agent.spec, &case) } else { None };
        match why.or(fixed.is_none().then_some("the next step needs a person's judgement")) {
            Some(why) => handover = Some(json!({workflow::MODE_KEY: "planner", workflow::WHY_KEY: why})),
            None if first_turn => handover = Some(json!({workflow::MODE_KEY: "fixed"})),
            None => {}
        }
    }
    let (decision, mut usage) = match fixed.clone() {
        Some(decision) => (decision, Usage::default()),
        None => match state.planner.plan(&req).await {
            Ok(PlanOutput { decision, usage }) => (decision, usage),
            Err(AiError::Retryable(m)) => return Err(JobError::Retry(m)),
            Err(AiError::Permanent(e)) => {
                // The same request fails the same way: stop now instead of retrying it.
                tracing::warn!(case_id = %case.id, error = %e, "planner refused the request");
                crate::ai::incidents::record(
                    &state.db,
                    crate::ai::incidents::NewIncident::new(&case.workspace_id, crate::ai::incidents::Kind::Refusal, json!({"source": "planner"}))
                        .agent(case.agent_id.as_deref())
                        .case(Some(&case.id))
                        .message(case.message_id.as_deref())
                        .dedupe(format!("refusal:{}", case.id)),
                )
                .await;
                let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
                case::fail(&mut tx, &case, &format!("The AI model couldn't take this request ({e})")).await?;
                tx.commit().await?;
                return Ok(());
            }
        },
    };

    let result = match decision {
        Decision::Finish { outcome, summary } => TurnResult::Finish { outcome, summary },
        Decision::Invalid { error } => TurnResult::Error {
            tool: None,
            input: Value::Null,
            summary: "The assistant made an unusable choice and will try again".into(),
            error,
        },
        Decision::Call { tool, args } => evaluate(state, &case, &agent, &case_agent, &allowed, turn, &tool, args, fixed.is_some(), &mut usage).await?,
    };
    if fixed.is_some() {
        let progress = match &result {
            TurnResult::Ran { .. } | TurnResult::Approval { .. } => Some(json!({workflow::POS_KEY: workflow::position(&case) + 1})),
            TurnResult::Error { .. } | TurnResult::Blocked { .. } => {
                Some(json!({workflow::MODE_KEY: "planner", workflow::WHY_KEY: "a step failed or was stopped"}))
            }
            TurnResult::Finish { .. } | TurnResult::Defer { .. } => None,
        };
        handover = match (handover, progress) {
            (Some(mut a), Some(b)) => {
                a.as_object_mut().expect("object").extend(b.as_object().cloned().unwrap_or_default());
                Some(a)
            }
            (a, b) => a.or(b),
        };
    }
    commit(state, &case, turn, &usage, result, handover).await
}

async fn commit(state: &AppState, case: &Case, turn: i64, usage: &Usage, result: TurnResult, state_patch: Option<Value>) -> Result<(), JobError> {
    if let TurnResult::Defer { minutes, count } = result {
        return park::defer_check(state, case, turn, minutes, count, usage).await;
    }
    let now = now_ms();
    let tool_cost = match &result {
        TurnResult::Ran { out, .. } => out.cost_usd,
        _ => 0.0,
    };
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    // Fence: only one run of this turn may record it, and never after a cancel.
    let taken = sqlx::query(
        "UPDATE cases SET step_count = ?1, spent_usd = spent_usd + ?2, updated_at = ?3, next_wake_at = NULL,
           status = CASE WHEN status = 'waiting_event' THEN 'running' ELSE status END,
           status_note = CASE WHEN status_note LIKE ?5 THEN NULL ELSE status_note END
         WHERE id = ?4 AND step_count = ?1 - 1 AND status IN ('running', 'waiting_event')",
    )
    .bind(turn)
    .bind(usage.cost_usd + tool_cost)
    .bind(now)
    .bind(&case.id)
    .bind(park::CHECK_WAIT_NOTE_LIKE)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if !taken {
        tx.rollback().await?;
        return Ok(());
    }
    let step = |kind, tool: Option<&'static str>, summary: String, input: Value, output: Value, status| NewStep {
        turn,
        kind,
        tool,
        summary,
        input,
        output,
        status,
        tokens_in: usage.tokens_in,
        tokens_out: usage.tokens_out,
        cost_usd: usage.cost_usd + tool_cost,
        model_id: usage.model_id.clone(),
    };
    let static_name = |name: &str| tools::registry().get(name).map(|t| t.name());
    if let Some(patch) = &state_patch {
        case::merge_state(&mut tx, &case.workspace_id, &case.id, patch).await?;
    }
    if let Some((tool, status, input)) = audited_call(&result) {
        audit::record(
            &mut tx,
            Entry {
                workspace_id: &case.workspace_id,
                actor: Actor::System,
                action: TOOL_CALL_ACTION,
                entity_type: "case",
                entity_id: &case.id,
                message_id: case.message_id.as_deref(),
                metadata: json!({
                    "tool": tool, "scope": scope_of_tool(tool).as_str(), "status": status, "turn": turn,
                    "agent_id": case.agent_id, "agent_version": case.agent_version, "shadow": case.shadow, "refs": call_refs(input),
                }),
            },
        )
        .await?;
    }
    let mut next_at = Some(now);
    let mut error_streak = 0;
    match result {
        TurnResult::Ran { tool, input, out, auto_sent } => {
            if auto_sent {
                audit::record(
                    &mut tx,
                    Entry {
                        workspace_id: &case.workspace_id,
                        actor: Actor::System,
                        action: autonomy::AUTO_SENT_ACTION,
                        entity_type: "case",
                        entity_id: &case.id,
                        message_id: case.message_id.as_deref(),
                        metadata: json!({"tool": tool, "draft_id": out.value.get("draft_id")}),
                    },
                )
                .await?;
            }
            case::insert_step(&mut tx, case, step("tool", static_name(&tool), out.summary, input, out.value, "ok")).await?;
            if let Some(patch) = &out.state {
                case::merge_state(&mut tx, &case.workspace_id, &case.id, patch).await?;
            }
            if let Some(wake) = out.sleep_until {
                sqlx::query("UPDATE cases SET status = 'waiting_event', next_wake_at = ?1 WHERE id = ?2 AND workspace_id = ?3")
                    .bind(wake)
                    .bind(&case.id)
                    .bind(&case.workspace_id)
                    .execute(&mut *tx)
                    .await?;
                next_at = Some(wake);
            }
        }
        TurnResult::Error { tool, input, summary, error } => {
            let tool = tool.as_deref().and_then(static_name);
            let kind = if tool.is_some() { "tool" } else { "note" };
            case::insert_step(&mut tx, case, step(kind, tool, summary, input, json!({"error": error}), "error")).await?;
            error_streak = case.error_streak + 1;
        }
        TurnResult::Blocked { tool, input, reason } => {
            let summary = format!("Check stopped {tool}: {reason}");
            case::insert_step(&mut tx, case, step("critic", static_name(&tool), summary, input, json!({"reason": reason}), "blocked")).await?;
            error_streak = case.error_streak;
        }
        TurnResult::Approval { tool, input, preview } => {
            let seq = case::insert_step(
                &mut tx,
                case,
                step("approval", static_name(&tool), format!("Waiting for approval: {}", preview.summary), input.clone(), json!(preview), "waiting"),
            )
            .await?;
            let approval_id = new_id("apr");
            sqlx::query(
                "INSERT INTO agent_approvals (id, workspace_id, case_id, step_seq, action, summary, to_address, subject, body, draft_id,
                                              requirement_id, candidates_json, input_json, status, created_at, agent_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 'pending', ?14, ?15)",
            )
            .bind(&approval_id)
            .bind(&case.workspace_id)
            .bind(&case.id)
            .bind(seq)
            .bind(tools::approval_action(&tool))
            .bind(sentence(&preview.summary, 200))
            .bind(&preview.to)
            .bind(&preview.subject)
            .bind(&preview.body)
            .bind(&preview.draft_id)
            .bind(&preview.requirement_id)
            .bind(json!(preview.candidates).to_string())
            .bind(input.to_string())
            .bind(now)
            .bind(&case.agent_id)
            .execute(&mut *tx)
            .await?;
            sqlx::query("UPDATE cases SET status = 'waiting_approval' WHERE id = ?1 AND workspace_id = ?2").bind(&case.id).bind(&case.workspace_id).execute(&mut *tx).await?;
            audit::record(
                &mut tx,
                Entry {
                    workspace_id: &case.workspace_id,
                    actor: Actor::System,
                    action: "agent_approval.requested",
                    entity_type: "agent_approval",
                    entity_id: &approval_id,
                    message_id: case.message_id.as_deref(),
                    metadata: json!({"case_id": case.id, "action": tool, "draft_id": preview.draft_id}),
                },
            )
            .await?;
            next_at = None;
        }
        TurnResult::Defer { .. } => {}
        TurnResult::Finish { outcome, summary } => {
            let summary = sentence(&summary, 240);
            case::insert_step(&mut tx, case, step("finish", None, summary.clone(), json!({"outcome": outcome}), Value::Null, "ok")).await?;
            case::escalate_unresolved_reply(&mut tx, case, turn).await?;
            sqlx::query("UPDATE cases SET status = 'done', outcome = COALESCE(?1, outcome), status_note = ?2 WHERE id = ?3 AND workspace_id = ?4")
                .bind(&outcome)
                .bind(&summary)
                .bind(&case.id)
                .bind(&case.workspace_id)
                .execute(&mut *tx)
                .await?;
            crate::alerts::messaging::watch::on_case_ended(&mut tx, &case.workspace_id, &case.id).await?;
            audit::record(
                &mut tx,
                Entry {
                    workspace_id: &case.workspace_id,
                    actor: Actor::System,
                    action: "case.finished",
                    entity_type: "case",
                    entity_id: &case.id,
                    message_id: case.message_id.as_deref(),
                    metadata: json!({"outcome": outcome, "steps": turn}),
                },
            )
            .await?;
            next_at = None;
        }
    }
    sqlx::query("UPDATE cases SET error_streak = ?1 WHERE id = ?2 AND workspace_id = ?3")
        .bind(error_streak)
        .bind(&case.id)
        .bind(&case.workspace_id)
        .execute(&mut *tx)
        .await?;
    if error_streak >= MAX_ERROR_STREAK {
        let current = Case { step_count: turn, spent_usd: case.spent_usd + usage.cost_usd, ..case.clone() };
        case::fail(&mut tx, &current, &format!("{MAX_ERROR_STREAK} steps in a row failed")).await?;
    } else if let Some(at) = next_at {
        case::enqueue_step(&mut tx, &case.workspace_id, &case.id, turn + 1, at).await?;
    }
    tx.commit().await?;
    state.jobs_notify.notify_one();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_stop_a_case() {
        assert_eq!(limit_reached(3, 15, 0.02, 0.10), None);
        assert!(limit_reached(15, 15, 0.02, 0.10).unwrap().contains("15 steps"));
        assert!(limit_reached(3, 15, 0.10, 0.10).unwrap().contains("budget of $0.10"));
        assert!(limit_reached(3, 15, 0.25, 0.10).is_some());
    }
}
