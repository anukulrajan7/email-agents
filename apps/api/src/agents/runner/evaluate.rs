use serde_json::{Value, json};

use super::{
    super::{
        autonomy,
        case::Case,
        critic::{self, CriticInput},
        planner::Usage,
        sentence,
        tools::{self, Preview, Risk, ToolCtx, ToolError, ToolOutput},
    },
    TurnResult, rule_context,
};
use crate::{
    agents::profiles::AgentContext,
    agents::registry::runtime::CaseAgent,
    http::AppState,
    jobs::handlers::JobError,
    platform::audit::Actor,
};

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

#[allow(clippy::too_many_arguments)]
pub(super) async fn evaluate(
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
    let mut critic_down = false;
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
        if !workflow {
            match critic_objection(state, case, tool, &args, preview.as_ref(), usage).await {
                Critique::Fine => {}
                Critique::Objection(reason) => return Ok(TurnResult::Blocked { tool: tool_name, input: args, reason }),
                Critique::Unavailable => {
                    tracing::warn!(case_id = %case.id, tool = %tool_name, "critic unavailable; low-risk step proceeds, sends go to a person");
                    critic_down = true;
                }
            }
        }
    }
    let mut auto_sent = false;
    if tool.risk() == Risk::High {
        let preview = preview.unwrap_or_default();
        let on_its_own = match tool.name() {
            "send_email" => autonomy::may_send_on_its_own(state, case, agent, case_agent, &preview).await?,
            "send_message" => crate::alerts::messaging::rules::may_send_on_its_own(state, case, agent, case_agent, &args).await?,
            _ => false,
        };
        if critic_down {
            let mut preview = preview;
            preview.summary = format!("{} (the safety check couldn't run; please read it before sending)", preview.summary);
            return Ok(TurnResult::Approval { tool: tool_name, input: args, preview });
        }
        if !on_its_own {
            return Ok(TurnResult::Approval { tool: tool_name, input: args, preview });
        }
        // A fixed workflow only asks the critic when a send would go out without a person.
        if workflow && !matches!(critic_objection(state, case, tool, &args, Some(&preview), usage).await, Critique::Fine) {
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
                    out.value["held_until"] = json!(crate::platform::db::iso(at));
                    out.summary = format!("{} Held until {} so you can stop it.", out.summary, crate::platform::db::iso(at));
                }
            }
            if critic_down {
                out.summary = format!("{} (the safety check was unavailable; this step only writes a draft, so it went ahead)", out.summary);
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

/// What the critic made of a proposal.
///
/// When the critic is down, a step that only writes a draft (low risk) still runs: nothing leaves the
/// mailbox, and the timeline says so. A send always goes to a person instead, once, never as a retry loop.
enum Critique {
    Fine,
    Objection(String),
    /// The check could not run at all (provider down).
    Unavailable,
}

async fn critic_objection(state: &AppState, case: &Case, tool: &dyn tools::Tool, args: &Value, preview: Option<&Preview>, usage: &mut Usage) -> Critique {
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
            if v.ok {
                Critique::Fine
            } else {
                Critique::Objection(if v.reason.is_empty() { "The check did not approve this action.".into() } else { v.reason })
            }
        }
        Err(e) => {
            tracing::warn!(case_id = %case.id, error = %e, "critic unavailable");
            Critique::Unavailable
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
