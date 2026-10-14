//! Dry run: what a candidate agent would do with recent mail, without doing any of it. The same
//! routing function the live system uses is replayed over the sample, a few hits are simulated step
//! by step (real reading and drafting preview, simulated writes), and the cost is estimated.
//! Nothing here writes a draft, case, approval, task, label, route or message; only the session row.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::{session::{self, Session}, validate};
use crate::{
    agent_registry::{
        routing::{self, Hints, MessageFacts, RoutingAgent, Source},
        spec::{AgentSpec, Mode, SendMode, Step},
        store::AgentRow,
    },
    agents::{
        case::Case,
        planner::{Decision, PlanRequest},
        tools::{self, Risk, ToolCtx},
        workflow::{self, WfStep},
    },
    ai::DraftQuality,
    audit::Actor,
    db::now_ms,
    http::AppState,
    jobs::handlers::{self, JobError},
};

pub const SAMPLE_SIZE: i64 = 50;
pub const SIMULATED_HITS: usize = 5;
const AGENT_SIM_STEPS: usize = 6;
const DAY_MS: i64 = 86_400_000;
const DRAFT_PREVIEW_CHARS: usize = 900;
const CANDIDATE_ID: &str = "dry_run_candidate";

#[derive(sqlx::FromRow)]
struct Sampled {
    id: String,
    subject: String,
    received_at: i64,
}

async fn sample(state: &AppState, ws: &str, picked: &[String]) -> sqlx::Result<Vec<Sampled>> {
    sqlx::query_as(
        "SELECT id, subject, received_at FROM email_messages
         WHERE workspace_id = ?1 AND direction = 'inbound' AND (json_array_length(?2) = 0 OR id IN (SELECT value FROM json_each(?2)))
         ORDER BY received_at DESC, id DESC LIMIT ?3",
    )
    .bind(ws)
    .bind(json!(picked).to_string())
    .bind(SAMPLE_SIZE)
    .fetch_all(&state.db)
    .await
}

fn candidate_agents(baseline: &[RoutingAgent], spec: &AgentSpec, existing: Option<&AgentRow>, ws: &str) -> (Vec<RoutingAgent>, String) {
    match existing {
        Some(row) => {
            let list = baseline
                .iter()
                .map(|a| if a.row.id == row.id { RoutingAgent { spec: spec.clone(), ..a.clone() } } else { a.clone() })
                .collect();
            (list, row.id.clone())
        }
        None => {
            let now = now_ms();
            let row = AgentRow {
                id: CANDIDATE_ID.into(),
                workspace_id: ws.into(),
                key: spec.key.clone(),
                kind: "custom".into(),
                scope: "workspace".into(),
                owner_user_id: None,
                category: spec.category.clone(),
                template_key: None,
                template_version: None,
                cloned_from: None,
                name: spec.name.clone(),
                avatar: "🤖".into(),
                color: "slate".into(),
                description: spec.description.clone(),
                status: "active".into(),
                paused_reason: None,
                active_version: Some(1),
                tuned: false,
                autonomy_level: 1,
                autonomy_cap: 1,
                next_run_at: None,
                created_by: None,
                created_at: now,
                updated_at: now,
            };
            let mut list = baseline.to_vec();
            list.push(RoutingAgent { row, spec: spec.clone(), version: 1, level: 1 });
            (list, CANDIDATE_ID.into())
        }
    }
}

fn reason_in_words(source: Source) -> &'static str {
    match source {
        Source::Rule => "it matches the sender, subject or attachment rules",
        Source::Label => "it carries one of the labels this agent takes",
        Source::Generic => "it is an email that needs a reply",
        Source::Personal => "it arrived in a personal mailbox",
        Source::Fallback => "no other agent takes it",
        Source::Thread => "the conversation was already with this agent",
        Source::Manual => "a person chose this agent",
    }
}

// ---------------------------------------------------------------- safety and cost

/// The checks every designed spec must pass before it is saved. Stands in for the evals safety suite.
pub fn safety(spec: &AgentSpec) -> Value {
    let checks: Vec<(&str, bool)> = vec![
        ("autonomy is capped at level 2", spec.permissions.max_autonomy <= validate::CUSTOM_CEILING),
        ("sending needs the send tool and a level of 2 or more", spec.permissions.send != SendMode::Earned || (spec.has_tool("send_email") && spec.permissions.max_autonomy >= 2)),
        ("an agent that never sends has no send tool", spec.permissions.send != SendMode::Never || !spec.has_tool("send_email")),
        ("no rule forwards, copies, links, attaches or weakens approval", spec.drafting.rules.iter().all(|r| validate::forbidden_reason(r).is_none())),
        ("no step is a send step", !spec.steps.iter().any(|s| s.describe().to_lowercase().contains("send the reply"))),
        ("every tool is one agents can use", spec.tools.iter().all(|t| tools::registry().get(t).is_some())),
        ("the daily send cap is at most 50", spec.permissions.daily_send_cap <= 50),
        ("the cost of one case is capped", spec.budget.usd_per_case <= validate::MAX_CASE_BUDGET_USD && spec.budget.max_steps <= 30),
    ];
    let failed: Vec<&str> = checks.iter().filter(|(_, ok)| !ok).map(|(name, _)| *name).collect();
    json!({"passed": failed.is_empty(), "cases": checks.len(), "failed": failed})
}

fn model_for(state: &AppState, tier: crate::agent_registry::spec::QualityTier) -> String {
    let c = &state.config;
    let configured = match tier {
        crate::agent_registry::spec::QualityTier::Fast => &c.agent_fast_model_id,
        crate::agent_registry::spec::QualityTier::Balanced => &c.agent_balanced_model_id,
        crate::agent_registry::spec::QualityTier::Best => &c.agent_best_model_id,
    };
    if configured.is_empty() { "deepseek/deepseek-v4-flash".into() } else { configured.clone() }
}

/// What one email costs this agent, from typical token counts and the configured models' prices.
pub fn estimate_cost_per_email(state: &AppState, spec: &AgentSpec) -> f64 {
    let price = |tier, tokens_in, tokens_out| crate::ai_ledger::cost_usd(&model_for(state, tier), tokens_in, tokens_out, None);
    let cost = match spec.mode {
        Mode::Agent => {
            let turns = i64::from(spec.budget.max_steps.min(6));
            price(crate::agent_registry::spec::QualityTier::Best, 3_000 * turns, 200 * turns)
        }
        Mode::Workflow => spec
            .steps
            .iter()
            .map(|s| match s {
                Step::Extract { .. } => price(crate::agent_registry::spec::QualityTier::Fast, 1_500, 150),
                Step::DraftReply { quality } => price(*quality, 2_500, 350),
                _ => 0.0,
            })
            .sum(),
    };
    (cost * 1e6).round() / 1e6
}

// ---------------------------------------------------------------- simulation

#[derive(Default)]
struct Example {
    steps: Vec<Value>,
    draft: Option<String>,
}

impl Example {
    fn step(&mut self, summary: impl Into<String>, status: &str) {
        self.steps.push(json!({"summary": summary.into(), "status": status}));
    }
}

async fn has_insight(state: &AppState, message_id: &str) -> sqlx::Result<bool> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM message_insights WHERE message_id = ?1)").bind(message_id).fetch_one(&state.db).await
}

async fn preview(state: &AppState, message_id: &str, quality: Option<DraftQuality>) -> Option<String> {
    if !has_insight(state, message_id).await.unwrap_or(false) {
        return None;
    }
    handlers::preview_draft(state, message_id, quality).await.ok().filter(|b| !b.trim().is_empty())
}

fn short(text: &str) -> String {
    crate::agents::sentence(text, DRAFT_PREVIEW_CHARS)
}

async fn simulate_workflow(state: &AppState, ws: &str, spec: &AgentSpec, message_id: &str) -> sqlx::Result<Example> {
    let mut ex = Example::default();
    let row: Option<(String, String)> = sqlx::query_as("SELECT subject, body_text FROM email_messages WHERE id = ?1 AND workspace_id = ?2")
        .bind(message_id)
        .bind(ws)
        .fetch_optional(&state.db)
        .await?;
    let Some((subject, body)) = row else { return Ok(ex) };
    for step in workflow::steps_for(spec) {
        match step {
            WfStep::Extract(fields) => match state.field_extractor.extract(&subject, &crate::knowledge::clean_body(&body), &fields).await {
                Ok(found) => {
                    let missing: Vec<&str> = fields.iter().map(|f| f.name.as_str()).filter(|n| !found.contains_key(*n)).collect();
                    let list = found.iter().map(|(k, v)| format!("{k} = {v}")).collect::<Vec<_>>().join(", ");
                    ex.step(
                        format!(
                            "Would read the email: found {}; missing {}",
                            if list.is_empty() { "nothing".to_string() } else { list },
                            if missing.is_empty() { "nothing".to_string() } else { missing.join(", ") }
                        ),
                        "read",
                    );
                }
                Err(_) => ex.step("Would read details from the email (the reader was not available for this preview)", "skipped"),
            },
            WfStep::Draft => match preview(state, message_id, None).await {
                Some(body) => {
                    ex.step("Would write a reply (shown below; nothing was saved)", "simulated");
                    ex.draft = Some(short(&body));
                }
                None => ex.step("Would write a reply (no preview: this email has not been read yet)", "skipped"),
            },
            WfStep::Send => ex.step(
                if spec.permissions.send == SendMode::Earned { "Would ask a person to approve the reply before it goes out (it earns sending on its own later)" } else { "Would ask a person to approve the reply before it goes out" },
                "simulated",
            ),
            WfStep::Task(title) => ex.step(format!("Would ask a person: {title}"), "simulated"),
            WfStep::Sleep(hours) => ex.step(format!("Would wait {hours} hours for a reply"), "simulated"),
            WfStep::Nudge => ex.step("Would ask a person to follow up if nobody replied", "simulated"),
            WfStep::Label(value) => ex.step(format!("Would label the email {value}"), "simulated"),
            WfStep::Notify(to) => ex.step(format!("Would let {} know", if to == "owner" { "the owner" } else { "the team" }), "simulated"),
            WfStep::StartCase => ex.step("Would start a case for this email", "simulated"),
            WfStep::SaveDraft => ex.step("Would save the draft to Gmail Drafts", "simulated"),
        }
    }
    Ok(ex)
}

fn stand_in_case(ws: &str, message_id: &str, thread_id: &str, title: &str, spec: &AgentSpec) -> Case {
    Case {
        id: "dry_run".into(),
        workspace_id: ws.into(),
        kind: "inbox".into(),
        title: title.into(),
        goal: spec.goal.clone(),
        status: "running".into(),
        outcome: None,
        status_note: None,
        failure_reason: None,
        state_json: "{}".into(),
        priority: "normal".into(),
        step_count: 0,
        max_steps: AGENT_SIM_STEPS as i64,
        spent_usd: 0.0,
        budget_usd: spec.budget.usd_per_case,
        error_streak: 0,
        message_id: Some(message_id.into()),
        requirement_id: None,
        thread_id: Some(thread_id.into()),
        next_wake_at: None,
        agent_user_id: None,
        agent_id: None,
        agent_version: None,
        trigger: None,
        outcome_detail: None,
        shadow: true,
        created_at: now_ms(),
        updated_at: now_ms(),
    }
}

/// Runs the planner for a few turns: read tools run for real, every other tool is only described.
async fn simulate_agent(state: &AppState, ws: &str, spec: &AgentSpec, message_id: &str, subject: &str, thread_id: &str) -> sqlx::Result<Example> {
    let mut ex = Example::default();
    if !state.planner.enabled() {
        ex.step("The planner is not available, so this open-ended agent could not be previewed", "skipped");
        return Ok(ex);
    }
    let case = stand_in_case(ws, message_id, thread_id, subject, spec);
    let allowed = spec.usable_tools();
    let system = crate::agents::planner::system_prompt(&format!("Your role: {}. {}\nYour goal: {}\nThis is a dry run: nothing you choose is carried out.", spec.name, spec.description, spec.goal));
    let mut history: Vec<String> = Vec::new();
    for turn in 1..=AGENT_SIM_STEPS {
        let req = PlanRequest {
            case_id: case.id.clone(),
            kind: "dry_run".into(),
            premium: false,
            requirement_id: None,
            message_id: Some(message_id.into()),
            facts: json!({}),
            system: system.clone(),
            model: None,
            prompt: format!("Goal: {}\nEmail id: {message_id}\nSteps so far:\n{}\nChoose the next tool.", spec.goal, history.join("\n")),
            tools: tools::registry().specs_for(&allowed),
        };
        let decision = match state.planner.plan(&req).await {
            Ok(out) => out.decision,
            Err(_) => {
                ex.step("The planner stopped answering, so the preview ends here", "skipped");
                break;
            }
        };
        match decision {
            Decision::Finish { summary, .. } => {
                ex.step(format!("Would finish: {}", crate::agents::sentence(&summary, 160)), "simulated");
                break;
            }
            Decision::Invalid { .. } => {
                ex.step("The planner gave an unusable answer, so the preview ends here", "skipped");
                break;
            }
            Decision::Call { tool, args } => {
                let Some(t) = tools::registry().get(&tool).filter(|t| allowed.iter().any(|a| a == t.name())) else {
                    ex.step(format!("Would try {tool}, which this agent may not use"), "blocked");
                    history.push(format!("{tool}: not allowed for this agent"));
                    continue;
                };
                if t.risk() == Risk::Read && tools::validate_args(&t.schema(), &args).is_ok() {
                    let ctx = ToolCtx { state, case: &case, turn: turn as i64, actor: Actor::System };
                    match t.run(&ctx, &args).await {
                        Ok(out) => {
                            ex.step(format!("Would {}: {}", t.name().replace('_', " "), out.summary), "read");
                            history.push(format!("{tool}: {}", crate::agents::sentence(&out.summary, 160)));
                        }
                        Err(_) => {
                            ex.step(format!("Would use {tool}, but it failed in the preview"), "skipped");
                            history.push(format!("{tool}: failed"));
                        }
                    }
                } else {
                    if tool == "draft_email" && ex.draft.is_none() {
                        ex.draft = preview(state, message_id, None).await.map(|b| short(&b));
                    }
                    ex.step(format!("Would use {}: nothing was done in this preview", t.name().replace('_', " ")), "simulated");
                    history.push(format!("{tool}: simulated, nothing was done"));
                }
            }
        }
    }
    Ok(ex)
}

// ---------------------------------------------------------------- the run

/// Builds the report for a session's spec. Pure of writes: callers store it on the session.
pub async fn report(state: &AppState, s: &Session, picked: &[String]) -> Result<Value, JobError> {
    let ws = &s.workspace_id;
    let Some(spec) = s.spec.clone() else { return Err(JobError::Permanent("the session has no spec".into())) };
    let existing = match &s.agent_id {
        Some(id) => crate::agent_registry::store::get_in(&state.db, ws, id).await?,
        None => None,
    };
    let messages = sample(state, ws, picked).await?;
    let baseline = routing::active_agents(&state.db, &state.config, ws).await?;
    let (candidates, candidate_id) = candidate_agents(&baseline, &spec, existing.as_ref(), ws);
    let name_of = |id: &str| baseline.iter().find(|a| a.row.id == id).map(|a| a.row.name.clone()).unwrap_or_else(|| "another agent".into());

    let mut would_handle = 0;
    let mut steals: BTreeMap<String, i64> = BTreeMap::new();
    let mut hits: Vec<(String, String, String, String, String)> = Vec::new();
    for m in &messages {
        let Some((facts, thread_id)) = routing::message_facts(&state.db, ws, &m.id).await? else { continue };
        let facts: MessageFacts = facts;
        let hints = Hints::default();
        let now_route = routing::route(&facts, &hints, &candidates);
        let Some(now_route) = now_route.filter(|r| r.agent_id == candidate_id) else { continue };
        let before = routing::route(&facts, &hints, &baseline);
        would_handle += 1;
        let currently = before.as_ref().filter(|r| r.agent_id != candidate_id).map(|r| r.agent_id.clone());
        if let Some(id) = &currently {
            *steals.entry(id.clone()).or_default() += 1;
        }
        hits.push((m.id.clone(), m.subject.clone(), thread_id, reason_in_words(now_route.source).to_string(), currently.map(|id| name_of(&id)).unwrap_or_default()));
    }

    let mut examples = Vec::new();
    for (message_id, subject, thread_id, reason, currently) in hits.iter().take(SIMULATED_HITS) {
        let ex = match spec.mode {
            Mode::Workflow => simulate_workflow(state, ws, &spec, message_id).await?,
            Mode::Agent => simulate_agent(state, ws, &spec, message_id, subject, thread_id).await?,
        };
        let needs_person = spec.steps.iter().any(|st| matches!(st, Step::CreateReview { .. } | Step::Notify { .. }))
            || (spec.has_tool("send_email") && spec.permissions.send != SendMode::Never)
            || spec.mode == Mode::Agent;
        examples.push(json!({
            "message_id": message_id, "subject": crate::agents::sentence(subject, 120),
            "route_reason": format!("Taken because {reason}"),
            "currently_handled_by": (!currently.is_empty()).then_some(currently),
            "steps": ex.steps, "draft": ex.draft, "needs_person": needs_person,
        }));
    }

    let steals: Vec<Value> = steals.into_iter().map(|(id, count)| json!({"agent_id": id, "name": name_of(&id), "count": count})).collect();
    let sampled = messages.len() as i64;
    let window_days = match (messages.first(), messages.last()) {
        (Some(newest), Some(oldest)) => ((newest.received_at - oldest.received_at) as f64 / DAY_MS as f64).max(1.0),
        _ => 1.0,
    };
    let per_email = estimate_cost_per_email(state, &spec);
    let monthly = would_handle as f64 / window_days * 30.0 * per_email;
    let mut warnings: Vec<String> = steals
        .iter()
        .map(|s| format!("It would take {} email{} from {}.", s["count"], if s["count"] == 1 { "" } else { "s" }, s["name"].as_str().unwrap_or("another agent")))
        .collect();
    if sampled > 0 && would_handle == 0 {
        warnings.push(format!("None of the last {sampled} emails would reach this agent. Check its triggers."));
    }
    if sampled >= 10 && would_handle == sampled {
        warnings.push("It would take every one of the sampled emails. Narrow its triggers if that is too many.".into());
    }
    Ok(json!({
        "would_handle": would_handle, "sampled": sampled, "window_days": (window_days * 10.0).round() / 10.0,
        "steals": steals, "examples": examples, "safety": safety(&spec),
        "est_cost_per_email_usd": per_email, "est_monthly_usd": (monthly * 100.0).round() / 100.0,
        "warnings": warnings, "spec_hash": spec.hash(), "ran_at": crate::db::iso(now_ms()),
    }))
}

/// The `designer_dry_run` job: builds the report and stores it (or the failure) on the session.
pub async fn run(state: &AppState, ws: &str, session_id: &str, picked: &[String]) -> Result<(), JobError> {
    let Some(s) = session::get(&state.db, ws, session_id).await? else { return Ok(()) };
    if s.dry_run_status != "running" {
        return Ok(());
    }
    let (status, body) = match report(state, &s, picked).await {
        Ok(report) => ("done", report),
        Err(JobError::Retry(m)) => return Err(JobError::Retry(m)),
        Err(JobError::Permanent(m)) => ("failed", json!({"error": crate::agents::sentence(&m, 200)})),
    };
    sqlx::query("UPDATE designer_sessions SET dry_run_status = ?1, dry_run_json = ?2, updated_at = ?3 WHERE id = ?4 AND workspace_id = ?5 AND dry_run_status = 'running'")
        .bind(status)
        .bind(body.to_string())
        .bind(now_ms())
        .bind(session_id)
        .bind(ws)
        .execute(&state.db)
        .await?;
    Ok(())
}
