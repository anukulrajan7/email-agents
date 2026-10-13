//! One planner turn: load the case → plan → check → run a tool or ask for approval → record the
//! step (summary, tokens, cost) → schedule the next turn, sleep, wait or finish.

use serde_json::{Value, json};

use super::{
    MAX_ERROR_STREAK,
    case::{self, Case, NewStep, StepRow},
    critic::{self, CriticInput, RuleContext},
    planner::{Decision, PlanOutput, PlanRequest, Usage},
    sentence,
    tools::{self, Preview, Risk, ToolCtx, ToolError, ToolOutput},
};
use crate::{
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
    Ran { tool: String, input: Value, out: ToolOutput },
    Error { tool: Option<String>, input: Value, summary: String, error: String },
    Blocked { tool: String, input: Value, reason: String },
    Approval { tool: String, input: Value, preview: Preview },
    Finish { outcome: Option<String>, summary: String },
}

fn clip(v: &str, max: usize) -> String {
    crate::sources::normalize::truncate_chars(v, max)
}

/// The planner's view of the case: goal, facts and what happened so far.
pub fn render_prompt(case: &Case, steps: &[StepRow], turn: i64) -> String {
    let mut out = format!(
        "Case: {}\nKind: {}\nGoal: {}\nRequirement id: {}\nCase email id: {}\nCase facts: {}\n\nSteps so far (oldest first):\n",
        case.title,
        case.kind,
        case.goal,
        case.requirement_id.as_deref().unwrap_or("none"),
        case.message_id.as_deref().unwrap_or("none"),
        clip(&case.state_json, 2_000),
    );
    if steps.is_empty() {
        out.push_str("(none yet)\n");
    }
    let detailed_from = steps.len().saturating_sub(DETAILED_STEPS);
    for (i, s) in steps.iter().enumerate() {
        let tool = s.tool.as_deref().map(|t| format!(" {t}")).unwrap_or_default();
        out.push_str(&format!("#{} [{}{tool}] {}: {}\n", s.seq, s.kind, s.status, s.summary));
        if i >= detailed_from {
            if s.input_json != "null" {
                out.push_str(&format!("   input: {}\n", clip(&s.input_json, 400)));
            }
            if s.output_json != "null" {
                out.push_str(&format!("   result: {}\n", clip(&s.output_json, OUTPUT_CHARS)));
            }
        }
    }
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

/// Runs planner turn `turn` of a case. Idempotent: a turn already taken, or a case that is
/// waiting for a person or has ended, is left alone.
pub async fn run_step(state: &AppState, case_id: &str, turn: i64) -> Result<(), JobError> {
    let Some(case) = case::get(&state.db, case_id).await? else { return Ok(()) };
    if case.is_terminal() || case.status == "waiting_approval" || turn != case.step_count + 1 {
        return Ok(());
    }
    // Over the workspace's daily AI budget: sleep until the next UTC midnight, then take this turn.
    if !crate::ai_ledger::within_budget(&state.db, state.config.ai_daily_budget_usd, &case.workspace_id).await {
        const DAY_MS: i64 = 86_400_000;
        let resume_at = (crate::db::now_ms() / DAY_MS + 1) * DAY_MS;
        let mut tx = state.db.begin().await?;
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
        let mut tx = state.db.begin().await?;
        case::fail(&mut tx, &case, &reason).await?;
        tx.commit().await?;
        return Ok(());
    }

    let steps = case::steps(&state.db, &case.id).await?;
    let registry = tools::registry();
    let req = PlanRequest {
        case_id: case.id.clone(),
        kind: case.kind.clone(),
        premium: case.premium(),
        requirement_id: case.requirement_id.clone(),
        message_id: case.message_id.clone(),
        facts: case.state(),
        prompt: render_prompt(&case, &steps, turn),
        tools: registry.specs(),
    };
    let PlanOutput { decision, mut usage } = match state.planner.plan(&req).await {
        Ok(p) => p,
        Err(AiError::Retryable(m)) => return Err(JobError::Retry(m)),
        Err(AiError::Permanent(e)) => {
            tracing::warn!(case_id = %case.id, error = %e, "planner refused the request");
            PlanOutput { decision: Decision::Invalid { error: "the AI service refused this request".into() }, usage: Usage::default() }
        }
    };

    let result = match decision {
        Decision::Finish { outcome, summary } => TurnResult::Finish { outcome, summary },
        Decision::Invalid { error } => TurnResult::Error {
            tool: None,
            input: Value::Null,
            summary: "The assistant made an unusable choice and will try again".into(),
            error,
        },
        Decision::Call { tool, args } => evaluate(state, &case, turn, &tool, args, &mut usage).await?,
    };
    commit(state, &case, turn, &usage, result).await
}

async fn evaluate(state: &AppState, case: &Case, turn: i64, name: &str, args: Value, usage: &mut Usage) -> Result<TurnResult, JobError> {
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
    let ctx = ToolCtx { state, case, turn, actor: Actor::System };
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
        let input = CriticInput {
            goal: case.goal.clone(),
            facts: case.state(),
            tool: tool_name.clone(),
            risk: tool.risk().as_str(),
            args: args.clone(),
            preview: preview.clone(),
        };
        match state.critic.review(&input).await {
            Ok(v) => {
                usage.add(&v.usage);
                if !v.ok {
                    let reason = if v.reason.is_empty() { "The check did not approve this action.".into() } else { v.reason };
                    return Ok(TurnResult::Blocked { tool: tool_name, input: args, reason });
                }
            }
            Err(e) => {
                tracing::warn!(case_id = %case.id, error = %e, "critic unavailable");
                return Ok(TurnResult::Blocked { tool: tool_name, input: args, reason: "The safety check is unavailable right now.".into() });
            }
        }
    }
    if tool.risk() == Risk::High {
        let preview = preview.unwrap_or_default();
        return Ok(TurnResult::Approval { tool: tool_name, input: args, preview });
    }
    Ok(match tool.run(&ctx, &args).await {
        Ok(out) => TurnResult::Ran { tool: tool_name, input: args, out },
        Err(ToolError::Failed(m)) => TurnResult::Error {
            tool: Some(tool_name.clone()),
            input: args,
            summary: format!("{} failed: {m}", tool_name),
            error: m,
        },
        Err(ToolError::Db(e)) => return Err(e.into()),
    })
}

async fn commit(state: &AppState, case: &Case, turn: i64, usage: &Usage, result: TurnResult) -> Result<(), JobError> {
    let now = now_ms();
    let mut tx = state.db.begin().await?;
    // Fence: only one run of this turn may record it, and never after a cancel.
    let taken = sqlx::query(
        "UPDATE cases SET step_count = ?1, spent_usd = spent_usd + ?2, updated_at = ?3, next_wake_at = NULL,
           status = CASE WHEN status = 'waiting_event' THEN 'running' ELSE status END
         WHERE id = ?4 AND step_count = ?1 - 1 AND status IN ('running', 'waiting_event')",
    )
    .bind(turn)
    .bind(usage.cost_usd)
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
        cost_usd: usage.cost_usd,
        model_id: usage.model_id.clone(),
    };
    let static_name = |name: &str| tools::registry().get(name).map(|t| t.name());
    let mut next_at = Some(now);
    let mut error_streak = 0;
    match result {
        TurnResult::Ran { tool, input, out } => {
            case::insert_step(&mut tx, case, step("tool", static_name(&tool), out.summary, input, out.value, "ok")).await?;
            if let Some(patch) = &out.state {
                case::merge_state(&mut tx, &case.id, patch).await?;
            }
            if let Some(wake) = out.sleep_until {
                sqlx::query("UPDATE cases SET status = 'waiting_event', next_wake_at = ?1 WHERE id = ?2")
                    .bind(wake)
                    .bind(&case.id)
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
                                              requirement_id, candidates_json, input_json, status, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 'pending', ?14)",
            )
            .bind(&approval_id)
            .bind(&case.workspace_id)
            .bind(&case.id)
            .bind(seq)
            .bind(&tool)
            .bind(sentence(&preview.summary, 200))
            .bind(&preview.to)
            .bind(&preview.subject)
            .bind(&preview.body)
            .bind(&preview.draft_id)
            .bind(&preview.requirement_id)
            .bind(json!(preview.candidates).to_string())
            .bind(input.to_string())
            .bind(now)
            .execute(&mut *tx)
            .await?;
            sqlx::query("UPDATE cases SET status = 'waiting_approval' WHERE id = ?1").bind(&case.id).execute(&mut *tx).await?;
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
            sqlx::query("UPDATE cases SET status = 'done', outcome = COALESCE(?1, outcome), status_note = ?2 WHERE id = ?3")
                .bind(&outcome)
                .bind(&summary)
                .bind(&case.id)
                .execute(&mut *tx)
                .await?;
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
    sqlx::query("UPDATE cases SET error_streak = ?1 WHERE id = ?2").bind(error_streak).bind(&case.id).execute(&mut *tx).await?;
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

    #[test]
    fn prompt_shows_goal_facts_and_recent_detail() {
        let steps: Vec<StepRow> = (1..=10)
            .map(|i| StepRow {
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
            })
            .collect();
        let p = render_prompt(&case(), &steps, 11);
        assert!(p.contains("Goal: Shortlist up to 3 candidates") && p.contains("Requirement id: req_1") && p.contains("Capgemini"));
        assert!(p.contains("#1 [tool search_mail] ok: step 1"));
        assert!(!p.contains("out1\"") && !p.contains("out2\""), "old steps are summarised only");
        assert!(p.contains("out3") && p.contains("out10"));
        assert!(p.contains("turn 11 of at most 15") && p.contains("Budget left: $0.090"));
    }
}
