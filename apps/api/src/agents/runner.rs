//! One planner turn: load the case → plan → check → run a tool or ask for approval → record the
//! step (summary, tokens, cost) → schedule the next turn, sleep, wait or finish.

use serde_json::{Value, json};

use super::{
    MAX_ERROR_STREAK,
    autonomy,
    case::{self, Case, NewStep, StepRow},
    critic::{self, CriticInput, RuleContext},
    planner::{self, Decision, PlanOutput, PlanRequest, Usage},
    workflow,
    sentence,
    tools::{self, Preview, Risk, ToolCtx, ToolError, ToolOutput},
};
use crate::{
    agent_profiles::{self, AgentContext},
    agent_registry::spec::scope_of_tool,
    agent_registry::runtime::{self, CaseAgent},
    ai::AiError,
    audit::{self, Actor, Entry},
    db::{new_id, now_ms},
    http::AppState,
    jobs::handlers::JobError,
};

/// Steps whose full input/output is shown to the planner; older ones appear as their summary only.
const DETAILED_STEPS: usize = 8;
const OUTPUT_CHARS: usize = 900;

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
}

fn clip(v: &str, max: usize) -> String {
    crate::sources::normalize::truncate_chars(v, max)
}

/// The planner's view of the case, ordered for prompt caching: what never changes first (the
/// case header), then the append-only step list, then what changes every turn (the latest
/// detailed results, the case facts) and the turn and budget line last.
pub fn render_prompt(case: &Case, steps: &[StepRow], turn: i64) -> String {
    let mut out = format!(
        "Case: {}\nKind: {}\nGoal: {}\nRequirement id: {}\nCase email id: {}\n\nSteps so far (oldest first):\n",
        case.title,
        case.kind,
        case.goal,
        case.requirement_id.as_deref().unwrap_or("none"),
        case.message_id.as_deref().unwrap_or("none"),
    );
    if steps.is_empty() {
        out.push_str("(none yet)\n");
    }
    for s in steps {
        let tool = s.tool.as_deref().map(|t| format!(" {t}")).unwrap_or_default();
        out.push_str(&format!("#{} [{}{tool}] {}: {}\n", s.seq, s.kind, s.status, s.summary));
    }
    let detailed_from = steps.len().saturating_sub(DETAILED_STEPS);
    let mut latest = String::new();
    for s in &steps[detailed_from..] {
        if s.input_json != "null" {
            latest.push_str(&format!("#{} input: {}\n", s.seq, clip(&s.input_json, 400)));
        }
        if s.output_json != "null" {
            latest.push_str(&format!("#{} result: {}\n", s.seq, clip(&s.output_json, OUTPUT_CHARS)));
        }
    }
    if !latest.is_empty() {
        out.push_str("\nLatest details:\n");
        out.push_str(&latest);
    }
    out.push_str(&format!("\nCase facts: {}\n", clip(&case.state_json, 2_000)));
    out.push_str(&format!(
        "\nThis is turn {turn} of at most {}. Budget left: ${:.3}.\nCall the single best next tool, or finish.",
        case.max_steps,
        (case.budget_usd - case.spent_usd).max(0.0)
    ));
    out
}

pub async fn rule_context(state: &AppState, case: &Case) -> sqlx::Result<RuleContext> {
    let allowed_recipients: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT lower(sender_address) FROM email_messages
         WHERE workspace_id = ?1 AND direction = 'inbound'
           AND (thread_id = ?2 OR (?2 IS NULL AND id = ?3))",
    )
    .bind(&case.workspace_id)
    .bind(&case.thread_id)
    .bind(&case.message_id)
    .fetch_all(&state.db)
    .await?;
    let declined_draft_ids: Vec<String> =
        sqlx::query_scalar("SELECT draft_id FROM agent_approvals WHERE case_id = ?1 AND status = 'declined' AND draft_id IS NOT NULL")
            .bind(&case.id)
            .fetch_all(&state.db)
            .await?;
    Ok(RuleContext { case_thread_id: case.thread_id.clone(), allowed_recipients, declined_draft_ids })
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
        TurnResult::Error { tool: None, .. } | TurnResult::Finish { .. } => None,
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

/// How long a case waits while its agent is paused before looking again.
const PAUSED_RECHECK_MS: i64 = 15 * 60 * 1000;

/// Runs planner turn `turn` of a case. Idempotent: a turn already taken, or a case that is
/// waiting for a person or has ended, is left alone. The case must belong to `ws`.
pub async fn run_step(state: &AppState, ws: &str, case_id: &str, turn: i64) -> Result<(), JobError> {
    let Some(case) = case::get_in(&state.db, ws, case_id).await? else { return Ok(()) };
    if case.is_terminal() || case.status == "waiting_approval" || turn != case.step_count + 1 {
        return Ok(());
    }
    // Over the workspace's daily AI budget: sleep until the next UTC midnight, then take this turn.
    if !crate::ai_ledger::within_budget(&state.db, state.config.ai_daily_budget_usd, &case.workspace_id).await {
        const DAY_MS: i64 = 86_400_000;
        let resume_at = (crate::db::now_ms() / DAY_MS + 1) * DAY_MS;
        let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query("UPDATE cases SET status = 'waiting_event', next_wake_at = ?1, updated_at = ?2 WHERE id = ?3 AND status = 'running'")
            .bind(resume_at)
            .bind(crate::db::now_ms())
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
    let tier = agent_profiles::effective_tier(&agent.agent.quality, case.premium());
    let allowed = case_agent.allowed_tools(runtime::matching_enabled(&state.db, &case.workspace_id).await?);
    let req = PlanRequest {
        case_id: case.id.clone(),
        kind: if case.shadow { "shadow".into() } else { case.kind.clone() },
        premium: tier == "best",
        requirement_id: case.requirement_id.clone(),
        message_id: case.message_id.clone(),
        facts: case.state(),
        system: planner::system_prompt(&agent.planner_block()),
        model: Some(agent_profiles::tier_model(&state.config, tier).to_string()).filter(|m| !m.is_empty()),
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
            TurnResult::Finish { .. } => None,
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

/// Why the workspace's permissions stop this tool, if they do.
fn permission_block(agent: &AgentContext, tool: &str) -> Option<String> {
    let name = &agent.agent.name;
    match tool {
        "draft_email" | "save_to_gmail_drafts" if !agent.agent.permissions.draft => {
            Some(format!("Writing drafts is switched off for {name}; create a task for a person instead."))
        }
        "send_email" | "submit_candidate" | "send_message" if agent.agent.permissions.send == "never" => {
            Some(format!("Sending is switched off for {name}; create a task for a person instead."))
        }
        _ => None,
    }
}

/// An observing case ends quietly at its step limit: it was only ever a trial run.
async fn end_observation(conn: &mut sqlx::SqliteConnection, case: &Case, reason: &str) -> sqlx::Result<()> {
    if case::set_status(&mut *conn, &case.workspace_id, &case.id, "done").await? {
        sqlx::query("UPDATE cases SET status_note = ?1, next_wake_at = NULL WHERE id = ?2 AND workspace_id = ?3")
            .bind(sentence(&format!("Observation ended: {reason}"), 240))
            .bind(&case.id)
            .bind(&case.workspace_id)
            .execute(conn)
            .await?;
    }
    Ok(())
}

/// A paused agent does no work: its cases wait and pick up again when it is switched back on.
/// An agent that was archived or never activated ends its case instead.
async fn park_while_inactive(state: &AppState, case: &Case, agent: &CaseAgent, turn: i64) -> Result<(), JobError> {
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    if agent.row.status != "paused" {
        let reason = format!("{} is not active", agent.row.name);
        if case.shadow {
            end_observation(&mut tx, case, &reason).await?;
        } else {
            case::fail(&mut tx, case, &reason).await?;
        }
        tx.commit().await?;
        return Ok(());
    }
    let resume_at = crate::db::now_ms() + PAUSED_RECHECK_MS;
    sqlx::query("UPDATE cases SET status = 'waiting_event', next_wake_at = ?1, updated_at = ?2 WHERE id = ?3 AND workspace_id = ?4 AND status = 'running'")
        .bind(resume_at)
        .bind(crate::db::now_ms())
        .bind(&case.id)
        .bind(&case.workspace_id)
        .execute(&mut *tx)
        .await?;
    crate::jobs::enqueue_at(
        &mut tx,
        crate::jobs::kind::AGENT_STEP,
        &format!("{}:paused:{}", case::step_key(&case.id, turn), resume_at / PAUSED_RECHECK_MS),
        Some(&case.workspace_id),
        &serde_json::json!({ "case_id": case.id, "turn": turn, "workspace_id": case.workspace_id }),
        crate::jobs::DEFAULT_MAX_ATTEMPTS,
        resume_at,
    )
    .await?;
    tx.commit().await?;
    tracing::info!(case_id = %case.id, "agent paused; case waits");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn evaluate(
    state: &AppState,
    case: &Case,
    agent: &AgentContext,
    case_agent: &CaseAgent,
    allowed: &[String],
    turn: i64,
    name: &str,
    args: Value,
    workflow: bool,
    usage: &mut Usage,
) -> Result<TurnResult, JobError> {
    let Some(tool) = tools::registry().get(name) else {
        return Ok(TurnResult::Error {
            tool: None,
            input: json!({"tool": sentence(name, 60), "args": args}),
            summary: "The assistant asked for an action that does not exist and will try again".into(),
            error: format!("unknown tool {}", sentence(name, 60)),
        });
    };
    let tool_name = tool.name().to_string();
    if let Err(e) = tools::validate_args(&tool.schema(), &args) {
        return Ok(TurnResult::Error {
            tool: Some(tool_name),
            input: args,
            summary: format!("The assistant gave incomplete details for {} and will try again", tool.name()),
            error: e,
        });
    }
    if !allowed.iter().any(|a| a == &tool_name) {
        let reason = format!("{} does not use {tool_name}; create a task for a person instead.", agent.agent.name);
        return Ok(TurnResult::Blocked { tool: tool_name, input: args, reason });
    }
    if let Some(reason) = permission_block(agent, &tool_name) {
        return Ok(TurnResult::Blocked { tool: tool_name, input: args, reason });
    }
    if case.shadow && (tool.risk() == Risk::High || tool_name == "create_task") {
        let summary = format!("Observe mode: would have used {tool_name}; nothing was done");
        let out = ToolOutput { summary, value: json!({"simulated": true, "tool": tool_name}), ..Default::default() };
        return Ok(TurnResult::Ran { tool: tool_name, input: args, out, auto_sent: false });
    }
    let mut ctx = ToolCtx { state, case, turn, actor: Actor::System, hold_secs: 0 };
    let mut preview = None;
    if tool.risk() != Risk::Read {
        if tool.risk() == Risk::High {
            preview = match tool.preview(&ctx, &args).await {
                Ok(p) => p,
                Err(ToolError::Failed(m)) => {
                    return Ok(TurnResult::Error { tool: Some(tool_name), input: args, summary: format!("Could not prepare {}: {m}", tool.name()), error: m });
                }
                Err(ToolError::Db(e)) => return Err(e.into()),
            };
            if let Some(reason) = critic::hard_rules(tool.name(), preview.as_ref(), &rule_context(state, case).await?) {
                return Ok(TurnResult::Blocked { tool: tool_name, input: args, reason });
            }
        }
        if !workflow && let Some(reason) = critic_objection(state, case, tool, &args, preview.as_ref(), usage).await {
            return Ok(TurnResult::Blocked { tool: tool_name, input: args, reason });
        }
    }
    let mut auto_sent = false;
    if tool.risk() == Risk::High {
        let preview = preview.unwrap_or_default();
        let on_its_own = match tool.name() {
            "send_email" => autonomy::may_send_on_its_own(state, case, agent, case_agent, &preview).await?,
            "send_message" => crate::messaging::rules::may_send_on_its_own(state, case, agent, case_agent, &args).await?,
            _ => false,
        };
        if !on_its_own {
            return Ok(TurnResult::Approval { tool: tool_name, input: args, preview });
        }
        // A fixed workflow only asks the critic when a send would go out without a person.
        if workflow && critic_objection(state, case, tool, &args, Some(&preview), usage).await.is_some() {
            return Ok(TurnResult::Approval { tool: tool_name, input: args, preview });
        }
        auto_sent = true;
        ctx.hold_secs = state.config.agent_auto_send_delay_secs;
    }
    Ok(match tool.run(&ctx, &args).await {
        Ok(mut out) => {
            if auto_sent {
                out.summary = format!("Sent automatically (your profile allows routine replies). {}", out.summary);
                out.value["sent_without_approval"] = json!(true);
                if ctx.hold_secs > 0
                    && let (Some(draft_id), Some(at)) = (out.value.get("draft_id").and_then(Value::as_str), out.value.get("send_at_ms").and_then(Value::as_i64))
                {
                    if let Err(e) = autonomy::record_hold(state, &case.workspace_id, draft_id, at, ctx.hold_secs).await {
                        tracing::warn!(case_id = %case.id, draft_id, error = %e, "could not note the send hold in the audit trail; the send stays scheduled");
                    }
                    out.value["held_until"] = json!(crate::db::iso(at));
                    out.summary = format!("{} Held until {} so you can stop it.", out.summary, crate::db::iso(at));
                }
            }
            TurnResult::Ran { tool: tool_name, input: args, out, auto_sent }
        }
        // A send check stopped the automatic send: a person decides instead.
        Err(ToolError::Failed(m)) if auto_sent => {
            tracing::info!(case_id = %case.id, reason = %m, "automatic send stopped; asking a person");
            let mut preview = preview_again(&ctx, tool, &args).await?;
            preview.summary = format!("{} (sending on its own was stopped: {})", preview.summary, sentence(&m, 120));
            TurnResult::Approval { tool: tool_name, input: args, preview }
        }
        Err(ToolError::Failed(m)) => TurnResult::Error {
            tool: Some(tool_name.clone()),
            input: args,
            summary: format!("{} failed: {m}", tool_name),
            error: m,
        },
        Err(ToolError::Db(e)) => return Err(e.into()),
    })
}

/// Why the critic does not approve this action, if it does not (also when it cannot be reached).
async fn critic_objection(state: &AppState, case: &Case, tool: &dyn tools::Tool, args: &Value, preview: Option<&Preview>, usage: &mut Usage) -> Option<String> {
    let input = CriticInput {
        goal: case.goal.clone(),
        facts: case.state(),
        tool: tool.name().to_string(),
        risk: tool.risk().as_str(),
        args: args.clone(),
        preview: preview.cloned(),
    };
    match state.critic.review(&input).await {
        Ok(v) => {
            usage.add(&v.usage);
            (!v.ok).then(|| if v.reason.is_empty() { "The check did not approve this action.".into() } else { v.reason })
        }
        Err(e) => {
            tracing::warn!(case_id = %case.id, error = %e, "critic unavailable");
            Some("The safety check is unavailable right now.".into())
        }
    }
}

async fn preview_again(ctx: &ToolCtx<'_>, tool: &dyn tools::Tool, args: &Value) -> Result<Preview, JobError> {
    match tool.preview(ctx, args).await {
        Ok(p) => Ok(p.unwrap_or_default()),
        Err(ToolError::Failed(m)) => Ok(Preview { summary: m, ..Default::default() }),
        Err(ToolError::Db(e)) => Err(e.into()),
    }
}

async fn commit(state: &AppState, case: &Case, turn: i64, usage: &Usage, result: TurnResult, state_patch: Option<Value>) -> Result<(), JobError> {
    let now = now_ms();
    let tool_cost = match &result {
        TurnResult::Ran { out, .. } => out.cost_usd,
        _ => 0.0,
    };
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    // Fence: only one run of this turn may record it, and never after a cancel.
    let taken = sqlx::query(
        "UPDATE cases SET step_count = ?1, spent_usd = spent_usd + ?2, updated_at = ?3, next_wake_at = NULL,
           status = CASE WHEN status = 'waiting_event' THEN 'running' ELSE status END
         WHERE id = ?4 AND step_count = ?1 - 1 AND status IN ('running', 'waiting_event')",
    )
    .bind(turn)
    .bind(usage.cost_usd + tool_cost)
    .bind(now)
    .bind(&case.id)
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
            crate::messaging::watch::on_case_ended(&mut tx, &case.workspace_id, &case.id).await?;
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

    fn case() -> Case {
        Case {
            id: "case_1".into(),
            workspace_id: "ws".into(),
            kind: "requirement".into(),
            title: "Java Developer at Capgemini".into(),
            goal: "Shortlist up to 3 candidates".into(),
            status: "running".into(),
            outcome: None,
            status_note: None,
            failure_reason: None,
            state_json: r#"{"client":"Capgemini"}"#.into(),
            priority: "normal".into(),
            step_count: 2,
            max_steps: 15,
            spent_usd: 0.01,
            budget_usd: 0.10,
            error_streak: 0,
            message_id: Some("msg_1".into()),
            requirement_id: Some("req_1".into()),
            thread_id: Some("thr_1".into()),
            next_wake_at: None,
            agent_user_id: None,
            agent_id: None,
            agent_version: None,
            trigger: None,
            outcome_detail: None,
            shadow: false,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn limits_stop_a_case() {
        assert_eq!(limit_reached(3, 15, 0.02, 0.10), None);
        assert!(limit_reached(15, 15, 0.02, 0.10).unwrap().contains("15 steps"));
        assert!(limit_reached(3, 15, 0.10, 0.10).unwrap().contains("budget of $0.10"));
        assert!(limit_reached(3, 15, 0.25, 0.10).is_some());
    }

    fn steps_row(i: i64) -> StepRow {
        StepRow {
            seq: i,
            turn: i,
            kind: "tool".into(),
            tool: Some("search_mail".into()),
            summary: format!("step {i}"),
            input_json: r#"{"query":"java"}"#.into(),
            output_json: format!(r#"{{"marker":"out{i}"}}"#),
            status: "ok".into(),
            cost_usd: 0.0,
            created_at: 0,
        }
    }

    #[test]
    fn prompt_shows_goal_facts_and_recent_detail() {
        let steps: Vec<StepRow> = (1..=10).map(steps_row).collect();
        let p = render_prompt(&case(), &steps, 11);
        assert!(p.contains("Goal: Shortlist up to 3 candidates") && p.contains("Requirement id: req_1") && p.contains("Capgemini"));
        assert!(p.contains("#1 [tool search_mail] ok: step 1"));
        assert!(!p.contains("out1\"") && !p.contains("out2\""), "old steps are summarised only");
        assert!(p.contains("out3") && p.contains("out10"));
        assert!(p.contains("turn 11 of at most 15") && p.contains("Budget left: $0.090"));
        let (header, steps_at, facts_at, turn_at) = (p.find("Case: Java").unwrap(), p.find("Steps so far").unwrap(), p.find("Case facts").unwrap(), p.find("This is turn").unwrap());
        assert!(header < steps_at && steps_at < facts_at && facts_at < turn_at, "stable parts first, volatile last");
        let later = render_prompt(&case(), &(1..=11).map(|i| StepRow { seq: i, ..steps_row(i) }).collect::<Vec<_>>(), 12);
        let prefix = &p[..p.find("\nLatest details").unwrap()];
        assert!(later.starts_with(prefix), "the header and earlier steps do not change when a step is added");
    }
}
