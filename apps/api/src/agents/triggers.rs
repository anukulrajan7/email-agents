//! When cases start and wake: a new requirement record, a new email that needs a reply,
//! a person's request, or a reply arriving in a conversation a case is waiting on.

use serde_json::{Value, json};
use sqlx::SqliteConnection;

use super::case::{self, NewCase};
use crate::{
    agent_profiles::{self, TASK_REPLY, TASK_REQUIREMENTS},
    agent_registry::{
        SHADOW_MAX_PER_DAY,
        routing::{self, RoutingAgent},
        spec::Trigger,
        store, templates,
    },
    ai::Insight,
    db::now_ms,
    domain::OTHER_LABEL,
    http::AppState,
    jobs::{self, Job, handlers::JobError, kind, matching_jobs::REQUIREMENT_LABEL},
};

/// Mail older than `AI_AUTO_HOURS` never starts a case on its own (same rule as other AI stages).
fn is_recent(state: &AppState, received_at: i64) -> bool {
    received_at >= now_ms() - state.config.ai_auto_hours * 3_600_000
}

#[derive(sqlx::FromRow)]
struct RequirementFacts {
    message_id: String,
    client: Option<String>,
    role_title: String,
    location: Option<String>,
    positions: Option<i64>,
    must_have_skills_json: String,
    received_at: i64,
    thread_id: String,
    sender_address: String,
    priority: Option<String>,
}

/// What starting a case did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Started {
    /// The case id, and whether it was created now (false: it already existed).
    Case(String, bool),
    /// The requirement or email isn't in this workspace.
    NotFound,
    /// The workspace agent doesn't do this job (a plain sentence why).
    NotAllowed(String),
}

/// The person whose mailbox the email arrived in, if the agent does `task`; else why not.
async fn agent_for(state: &AppState, ws: &str, message_id: &str, task: &str) -> sqlx::Result<Result<Option<String>, String>> {
    let ctx = agent_profiles::context_for_message(&state.db, &state.config, ws, message_id).await?;
    let name = &ctx.agent.name;
    if !ctx.agent.enabled {
        return Ok(Err(format!("{name} is switched off for this workspace")));
    }
    if !ctx.agent.does(task) {
        let job = if task == TASK_REPLY { "reply to emails" } else { "work on client requirements" };
        return Ok(Err(format!("{name} is not set up to {job}")));
    }
    Ok(Ok(ctx.owner.map(|o| o.user_id)))
}

/// Starts (or finds) the requirement case.
pub async fn start_requirement_case(state: &AppState, ws: &str, requirement_id: &str, created_by: Option<&str>) -> sqlx::Result<Started> {
    let Some(r) = requirement_facts(state, ws, requirement_id).await? else { return Ok(Started::NotFound) };
    if !crate::packs::has_feature(&state.db, ws, crate::packs::FEATURE_REQUIREMENT_AGENT).await? {
        return Ok(Started::NotAllowed("The HR & recruiting pack is not turned on for this workspace".into()));
    }
    let owner = match agent_for(state, ws, &r.message_id, TASK_REQUIREMENTS).await? {
        Ok(owner) => owner,
        Err(why) => return Ok(Started::NotAllowed(why)),
    };
    let desk = requirement_desk(state, ws).await?;
    let role_at = match r.client.as_deref().filter(|c| !c.trim().is_empty()) {
        Some(client) => format!("{} at {client}", r.role_title),
        None => r.role_title.clone(),
    };
    let skills: Value = serde_json::from_str(&r.must_have_skills_json).unwrap_or_else(|_| json!([]));
    let mut conn = state.db.acquire().await?;
    let created = case::create(
        &mut conn,
        &state.config,
        NewCase {
            workspace_id: ws,
            kind: "requirement",
            title: role_at.clone(),
            goal: format!("Shortlist up to 3 candidates for {role_at}, get approval, submit, follow up until an outcome"),
            priority: r.priority.clone().unwrap_or_else(|| "normal".into()),
            message_id: Some(&r.message_id),
            requirement_id: Some(requirement_id),
            thread_id: Some(&r.thread_id),
            state: json!({
                "client": r.client, "role": r.role_title, "location": r.location, "positions": r.positions,
                "must_have_skills": skills, "client_email": r.sender_address,
            }),
            created_by,
            agent_user_id: owner.as_deref(),
            agent_id: desk.as_ref().map(|d| d.0.as_str()),
            agent_version: desk.as_ref().map(|d| d.1),
            trigger: Some("requirement"),
            shadow: false,
        },
    )
    .await?;
    state.jobs_notify.notify_one();
    Ok(Started::Case(created.0, created.1))
}

/// The agent for requirement cases: the Candidates & Requirements desk when it is on, else the lead.
async fn requirement_desk(state: &AppState, ws: &str) -> sqlx::Result<Option<(String, i64)>> {
    let desk = store::by_template(&state.db, ws, templates::REQUIREMENTS_KEY).await?.filter(|a| a.status == "active");
    let agent = match desk {
        Some(d) => Some(d),
        None => store::lead(&state.db, ws).await?.filter(|a| a.status == "active"),
    };
    Ok(agent.map(|a| (a.id, a.active_version.unwrap_or(0))))
}

async fn requirement_facts(state: &AppState, ws: &str, requirement_id: &str) -> sqlx::Result<Option<RequirementFacts>> {
    sqlx::query_as(
        "SELECT r.message_id, r.client, r.role_title, r.location, r.positions, r.must_have_skills_json,
                m.received_at, m.thread_id, m.sender_address, c.priority
         FROM requirements r JOIN email_messages m ON m.id = r.message_id
         LEFT JOIN classifications c ON c.message_id = m.id
         WHERE r.id = ?1 AND r.workspace_id = ?2",
    )
    .bind(requirement_id)
    .bind(ws)
    .fetch_optional(&state.db)
    .await
}

/// Hook after matching ran for a requirement record: start its case once, for new mail only.
pub async fn after_requirement_matched(state: &AppState, ws: &str, requirement_id: &str) -> Result<(), JobError> {
    if !state.planner.enabled() || !crate::packs::has_feature(&state.db, ws, crate::packs::FEATURE_REQUIREMENT_AGENT).await? {
        return Ok(());
    }
    let Some(r) = requirement_facts(state, ws, requirement_id).await? else { return Ok(()) };
    if !is_recent(state, r.received_at) {
        return Ok(());
    }
    // A client's reply in the conversation can be read as a "new" requirement: one case per conversation.
    let thread_taken: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM cases WHERE thread_id = ?1 AND kind = 'requirement' AND shadow = 0 AND requirement_id IS NOT ?2)",
    )
    .bind(&r.thread_id)
    .bind(requirement_id)
    .fetch_one(&state.db)
    .await?;
    if thread_taken {
        return Ok(());
    }
    start_requirement_case(state, ws, requirement_id, None).await?;
    Ok(())
}

#[derive(sqlx::FromRow)]
struct MessageFacts {
    subject: String,
    sender_address: String,
    received_at: i64,
    direction: String,
    thread_id: String,
    category: Option<String>,
    priority: Option<String>,
}

async fn message_facts(state: &AppState, ws: &str, message_id: &str) -> sqlx::Result<Option<MessageFacts>> {
    sqlx::query_as(
        "SELECT m.subject, m.sender_address, m.received_at, m.direction, m.thread_id, c.category, c.priority
         FROM email_messages m LEFT JOIN classifications c ON c.message_id = m.id
         WHERE m.id = ?1 AND m.workspace_id = ?2",
    )
    .bind(message_id)
    .bind(ws)
    .fetch_optional(&state.db)
    .await
}

/// Starts (or finds) the inbox case for one received email of this workspace. The email is routed
/// first; the routed agent works it. An observing (level 0) agent gets a shadow case beside the
/// real one, unless a person started this case by hand.
pub async fn start_inbox_case(state: &AppState, ws: &str, message_id: &str, created_by: Option<&str>) -> sqlx::Result<Started> {
    let Some(m) = message_facts(state, ws, message_id).await?.filter(|m| m.direction == "inbound") else { return Ok(Started::NotFound) };
    let owner = match agent_for(state, ws, message_id, TASK_REPLY).await? {
        Ok(owner) => owner,
        Err(why) => return Ok(Started::NotAllowed(why)),
    };
    let routed = routing::route_message(&state.db, &state.config, ws, message_id).await?;
    let handler = routed.as_ref().map(|r| (r.handler.row.id.clone(), r.handler.version));
    let title = if m.subject.trim().is_empty() { "(no subject)".to_string() } else { m.subject.clone() };
    let mut conn = state.db.acquire().await?;
    let created = case::create(
        &mut conn,
        &state.config,
        NewCase {
            workspace_id: ws,
            kind: "inbox",
            title: title.clone(),
            goal: format!("Resolve this email from {}: reply, or hand it to a person", m.sender_address),
            priority: m.priority.clone().unwrap_or_else(|| "normal".into()),
            message_id: Some(message_id),
            requirement_id: None,
            thread_id: Some(&m.thread_id),
            state: json!({"from": m.sender_address, "subject": m.subject, "label": m.category}),
            created_by,
            agent_user_id: owner.as_deref(),
            agent_id: handler.as_ref().map(|h| h.0.as_str()),
            agent_version: handler.as_ref().map(|h| h.1),
            trigger: Some(if created_by.is_some() { "manual" } else { "email_received" }),
            shadow: false,
        },
    )
    .await?;
    if let Some(r) = routed.as_ref().filter(|r| r.route.shadow && created_by.is_none()) {
        start_shadow_case(state, &mut conn, ws, &r.primary, &m, message_id, owner.as_deref()).await?;
    }
    drop(conn);
    state.jobs_notify.notify_one();
    Ok(Started::Case(created.0, created.1))
}

async fn shadow_cases_today(state: &AppState, ws: &str) -> sqlx::Result<i64> {
    const DAY_MS: i64 = 86_400_000;
    sqlx::query_scalar("SELECT COUNT(*) FROM cases WHERE workspace_id = ?1 AND shadow = 1 AND created_at >= ?2")
        .bind(ws)
        .bind(now_ms() / DAY_MS * DAY_MS)
        .fetch_one(&state.db)
        .await
}

async fn start_shadow_case(
    state: &AppState,
    conn: &mut sqlx::SqliteConnection,
    ws: &str,
    agent: &RoutingAgent,
    m: &MessageFacts,
    message_id: &str,
    owner: Option<&str>,
) -> sqlx::Result<()> {
    if shadow_cases_today(state, ws).await? >= SHADOW_MAX_PER_DAY {
        return Ok(());
    }
    case::create(
        conn,
        &state.config,
        NewCase {
            workspace_id: ws,
            kind: "inbox",
            title: if m.subject.trim().is_empty() { "(no subject)".into() } else { m.subject.clone() },
            goal: format!("Observe: how {} would handle this email from {}", agent.row.name, m.sender_address),
            priority: m.priority.clone().unwrap_or_else(|| "normal".into()),
            message_id: Some(message_id),
            requirement_id: None,
            thread_id: Some(&m.thread_id),
            state: json!({"from": m.sender_address, "subject": m.subject, "label": m.category}),
            created_by: None,
            agent_user_id: owner,
            agent_id: Some(&agent.row.id),
            agent_version: Some(agent.version),
            trigger: Some("email_received"),
            shadow: true,
        },
    )
    .await?;
    Ok(())
}

async fn thread_has_open_case(state: &AppState, thread_id: &str) -> sqlx::Result<bool> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM cases WHERE thread_id = ?1 AND shadow = 0 AND status NOT IN ('done', 'failed', 'cancelled'))")
        .bind(thread_id)
        .fetch_one(&state.db)
        .await
}

/// Hook after a new email was understood: route it (recorded for every inbound email), then start
/// the routed agent when it needs a reply, isn't `other` (requirements have their own agent), is
/// new, and the workspace drafts replies automatically.
pub async fn after_analysis(state: &AppState, job: &Job, message_id: &str, insight: &Insight) -> Result<(), JobError> {
    if job.dedupe_key != format!("analyze:{message_id}") {
        return Ok(());
    }
    let Some(ws) = job.workspace_id.as_deref() else { return Ok(()) };
    routing::route_message(&state.db, &state.config, ws, message_id).await?;
    if !insight.needs_reply || !state.planner.enabled() {
        return Ok(());
    }
    let Some(m) = message_facts(state, ws, message_id).await? else { return Ok(()) };
    let label_ok = m.category.as_deref().is_some_and(|c| c != OTHER_LABEL && c != REQUIREMENT_LABEL);
    if m.direction != "inbound" || !label_ok || !is_recent(state, m.received_at) || thread_has_open_case(state, &m.thread_id).await? {
        return Ok(());
    }
    if !crate::reply_settings::load(&state.db, &state.config, ws).await?.auto_reply {
        return Ok(());
    }
    start_inbox_case(state, ws, message_id, None).await?;
    Ok(())
}

/// Called when an inbound email is stored: open cases on that conversation learn about it, and
/// sleeping ones wake now instead of at their scheduled check.
pub async fn wake_for_thread(conn: &mut SqliteConnection, ws: &str, thread_id: &str, message_id: &str) -> sqlx::Result<()> {
    let now = now_ms();
    sqlx::query(
        "UPDATE cases SET state_json = json_set(state_json, '$.new_message_id', ?1), updated_at = ?2
         WHERE workspace_id = ?3 AND thread_id = ?4 AND status NOT IN ('done', 'failed', 'cancelled')",
    )
    .bind(message_id)
    .bind(now)
    .bind(ws)
    .bind(thread_id)
    .execute(&mut *conn)
    .await?;
    let sleeping: Vec<(String, i64)> = sqlx::query_as(
        "UPDATE cases SET next_wake_at = ?1 WHERE workspace_id = ?2 AND thread_id = ?3 AND status = 'waiting_event'
         RETURNING id, step_count",
    )
    .bind(now)
    .bind(ws)
    .bind(thread_id)
    .fetch_all(&mut *conn)
    .await?;
    for (case_id, step_count) in sleeping {
        sqlx::query("UPDATE jobs SET run_at = ?1, updated_at = ?1 WHERE dedupe_key = ?2 AND status = 'pending' AND run_at > ?1")
            .bind(now)
            .bind(case::step_key(&case_id, step_count + 1))
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

// ---------------------------------------------------------------- schedule triggers

const TICK_EVERY_MS: i64 = 15 * 60 * 1000;

/// Enqueues one `agent_schedule_tick` per workspace per 15-minute bucket (called from the worker's maintenance pass).
pub async fn schedule(state: &AppState) -> sqlx::Result<()> {
    let bucket = now_ms() / TICK_EVERY_MS;
    let workspaces: Vec<String> = sqlx::query_scalar("SELECT id FROM workspaces").fetch_all(&state.db).await?;
    let mut conn = state.db.acquire().await?;
    for ws in workspaces {
        jobs::enqueue_with_attempts(&mut conn, kind::AGENT_SCHEDULE_TICK, &format!("agent_tick:{ws}:{bucket}"), Some(&ws), &json!({ "workspace_id": ws }), 3)
            .await?;
    }
    Ok(())
}

pub async fn schedule_tick_job(state: &AppState, job: &Job) -> Result<(), JobError> {
    let ws = job.workspace_id.as_deref().ok_or_else(|| JobError::Permanent("agent tick without a workspace".into()))?;
    run_schedule_tick(state, ws, now_ms()).await?;
    Ok(())
}

/// Starts a case for every active agent whose schedule trigger is due. The first pass only sets the
/// clock (an agent never fires the moment it appears). Returns how many cases were started.
pub async fn run_schedule_tick(state: &AppState, ws: &str, now: i64) -> sqlx::Result<usize> {
    if !state.planner.enabled() {
        return Ok(0);
    }
    let mut started = 0;
    for agent in routing::active_agents(&state.db, &state.config, ws).await? {
        let Some(hours) = agent
            .spec
            .triggers
            .iter()
            .filter_map(|t| match t {
                Trigger::Schedule { every_hours } => Some(i64::from(*every_hours)),
                _ => None,
            })
            .min()
        else {
            continue;
        };
        let next = now + hours * 3_600_000;
        let due = match agent.row.next_run_at {
            None => false,
            Some(at) => at <= now,
        };
        let claimed = sqlx::query("UPDATE agents SET next_run_at = ?1 WHERE id = ?2 AND workspace_id = ?3 AND next_run_at IS ?4")
            .bind(next)
            .bind(&agent.row.id)
            .bind(ws)
            .bind(agent.row.next_run_at)
            .execute(&state.db)
            .await?
            .rows_affected()
            == 1;
        if !claimed || !due {
            continue;
        }
        let open: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM cases WHERE workspace_id = ?1 AND agent_id = ?2 AND \"trigger\" = 'schedule' AND status NOT IN ('done', 'failed', 'cancelled'))",
        )
        .bind(ws)
        .bind(&agent.row.id)
        .fetch_one(&state.db)
        .await?;
        let shadow = agent.level == 0;
        if open || (shadow && shadow_cases_today(state, ws).await? >= SHADOW_MAX_PER_DAY) {
            continue;
        }
        let mut conn = state.db.acquire().await?;
        case::create(
            &mut conn,
            &state.config,
            NewCase {
                workspace_id: ws,
                kind: "inbox",
                title: format!("Scheduled: {}", agent.row.name),
                goal: agent.spec.goal.clone(),
                priority: "normal".into(),
                message_id: None,
                requirement_id: None,
                thread_id: None,
                state: json!({"scheduled_by": agent.row.key}),
                created_by: None,
                agent_user_id: agent.row.owner_user_id.as_deref(),
                agent_id: Some(&agent.row.id),
                agent_version: Some(agent.version),
                trigger: Some("schedule"),
                shadow,
            },
        )
        .await?;
        started += 1;
    }
    if started > 0 {
        state.jobs_notify.notify_one();
    }
    Ok(started)
}
