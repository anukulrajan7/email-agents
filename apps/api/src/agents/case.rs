//! Case and step persistence, status transitions and step scheduling.

use serde_json::{Value, json};
use sqlx::{SqliteConnection, SqlitePool};

use crate::{
    audit::{self, Actor, Entry},
    config::Config,
    db::{new_id, now_ms},
    jobs::{self, kind},
};

pub const KINDS: &[&str] = &["requirement", "inbox"];

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Case {
    pub id: String,
    pub workspace_id: String,
    pub kind: String,
    pub title: String,
    pub goal: String,
    pub status: String,
    pub outcome: Option<String>,
    pub status_note: Option<String>,
    pub failure_reason: Option<String>,
    pub state_json: String,
    pub priority: String,
    pub step_count: i64,
    pub max_steps: i64,
    pub spent_usd: f64,
    pub budget_usd: f64,
    pub error_streak: i64,
    pub message_id: Option<String>,
    pub requirement_id: Option<String>,
    pub thread_id: Option<String>,
    pub next_wake_at: Option<i64>,
    /// Whose mailbox the case's email arrived in (the person the agent works for).
    pub agent_user_id: Option<String>,
    /// The agent working the case (None for cases from before the registry: the lead).
    pub agent_id: Option<String>,
    pub agent_version: Option<i64>,
    /// What started it: `email_received`, `schedule`, `manual` or `requirement`.
    pub trigger: Option<String>,
    pub outcome_detail: Option<String>,
    /// An observing agent's case: runs beside the real one and changes nothing.
    pub shadow: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

pub const CASE_COLUMNS: &str = "id, workspace_id, kind, title, goal, status, outcome, status_note, failure_reason, state_json, priority,
    step_count, max_steps, spent_usd, budget_usd, error_streak, message_id, requirement_id, thread_id, next_wake_at, agent_user_id,
    agent_id, agent_version, \"trigger\", outcome_detail, shadow, created_at, updated_at";

impl Case {
    pub fn state(&self) -> Value {
        serde_json::from_str(&self.state_json).unwrap_or_else(|_| json!({}))
    }

    pub fn is_terminal(&self) -> bool {
        is_terminal(&self.status)
    }

    /// High-priority requirement cases get the premium planner.
    pub fn premium(&self) -> bool {
        self.kind == "requirement" && self.priority == "high"
    }
}

pub fn is_terminal(status: &str) -> bool {
    matches!(status, "done" | "failed" | "cancelled")
}

/// Allowed status changes. Terminal cases never move again.
pub fn can_transition(from: &str, to: &str) -> bool {
    match from {
        "running" => matches!(to, "running" | "waiting_approval" | "waiting_event" | "done" | "failed" | "cancelled"),
        "waiting_approval" | "waiting_event" => matches!(to, "running" | "failed" | "cancelled"),
        _ => false,
    }
}

/// The one case lookup: always scoped by workspace, so another tenant's case id is simply not found.
pub async fn get_in(db: &SqlitePool, ws: &str, case_id: &str) -> sqlx::Result<Option<Case>> {
    sqlx::query_as(&format!("SELECT {CASE_COLUMNS} FROM cases WHERE id = ?1 AND workspace_id = ?2"))
        .bind(case_id)
        .bind(ws)
        .fetch_optional(db)
        .await
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StepRow {
    pub seq: i64,
    pub turn: i64,
    pub kind: String,
    pub tool: Option<String>,
    pub summary: String,
    pub input_json: String,
    pub output_json: String,
    pub status: String,
    pub cost_usd: f64,
    pub created_at: i64,
}

pub async fn steps(db: &SqlitePool, ws: &str, case_id: &str) -> sqlx::Result<Vec<StepRow>> {
    sqlx::query_as(
        "SELECT seq, turn, kind, tool, summary, input_json, output_json, status, cost_usd, created_at
         FROM case_steps WHERE case_id = ?1 AND workspace_id = ?2 ORDER BY seq",
    )
    .bind(case_id)
    .bind(ws)
    .fetch_all(db)
    .await
}

pub struct NewStep<'a> {
    pub turn: i64,
    pub kind: &'a str,
    pub tool: Option<&'a str>,
    pub summary: String,
    pub input: Value,
    pub output: Value,
    pub status: &'a str,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub cost_usd: f64,
    pub model_id: Option<String>,
}

impl<'a> NewStep<'a> {
    pub fn note(turn: i64, summary: impl Into<String>, status: &'a str) -> Self {
        Self {
            turn,
            kind: "note",
            tool: None,
            summary: summary.into(),
            input: Value::Null,
            output: Value::Null,
            status,
            tokens_in: 0,
            tokens_out: 0,
            cost_usd: 0.0,
            model_id: None,
        }
    }
}

/// Appends a step; returns its `seq`.
pub async fn insert_step(conn: &mut SqliteConnection, case: &Case, step: NewStep<'_>) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "INSERT INTO case_steps (id, case_id, workspace_id, seq, turn, kind, tool, summary, input_json, output_json, status,
                                 tokens_in, tokens_out, cost_usd, model_id, created_at)
         VALUES (?1, ?2, ?3, (SELECT COALESCE(MAX(seq), 0) + 1 FROM case_steps WHERE case_id = ?2), ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                 ?11, ?12, ?13, ?14, ?15)
         RETURNING seq",
    )
    .bind(new_id("stp"))
    .bind(&case.id)
    .bind(&case.workspace_id)
    .bind(step.turn)
    .bind(step.kind)
    .bind(step.tool)
    .bind(super::sentence(&step.summary, 240))
    .bind(step.input.to_string())
    .bind(step.output.to_string())
    .bind(step.status)
    .bind(step.tokens_in)
    .bind(step.tokens_out)
    .bind(step.cost_usd)
    .bind(step.model_id)
    .bind(now_ms())
    .fetch_one(conn)
    .await
}

/// Shallow-merges `patch` into the case's state (JSON merge patch).
pub async fn merge_state(conn: &mut SqliteConnection, ws: &str, case_id: &str, patch: &Value) -> sqlx::Result<()> {
    sqlx::query("UPDATE cases SET state_json = json_patch(state_json, ?1) WHERE id = ?2 AND workspace_id = ?3")
        .bind(patch.to_string())
        .bind(case_id)
        .bind(ws)
        .execute(conn)
        .await?;
    Ok(())
}

pub fn step_key(case_id: &str, turn: i64) -> String {
    format!("agent_step:{case_id}:{turn}")
}

/// Schedules planner turn `turn` (idempotent per turn).
pub async fn enqueue_step(conn: &mut SqliteConnection, ws: &str, case_id: &str, turn: i64, run_at: i64) -> sqlx::Result<bool> {
    jobs::enqueue_at(
        conn,
        kind::AGENT_STEP,
        &step_key(case_id, turn),
        Some(ws),
        &json!({ "case_id": case_id, "turn": turn, "workspace_id": ws }),
        jobs::DEFAULT_MAX_ATTEMPTS,
        run_at,
    )
    .await
}

pub struct NewCase<'a> {
    pub workspace_id: &'a str,
    pub kind: &'a str,
    pub title: String,
    pub goal: String,
    pub priority: String,
    pub message_id: Option<&'a str>,
    pub requirement_id: Option<&'a str>,
    pub thread_id: Option<&'a str>,
    pub state: Value,
    pub created_by: Option<&'a str>,
    pub agent_user_id: Option<&'a str>,
    pub agent_id: Option<&'a str>,
    pub agent_version: Option<i64>,
    pub trigger: Option<&'a str>,
    /// An observing agent's case: few steps, no side effects.
    pub shadow: bool,
}

/// Creates the case and schedules its first turn, unless an equivalent case exists
/// (same requirement or message). Returns `(case_id, created)`.
pub async fn create(conn: &mut SqliteConnection, config: &Config, new: NewCase<'_>) -> sqlx::Result<(String, bool)> {
    let id = new_id("case");
    let now = now_ms();
    let created = sqlx::query(
        "INSERT OR IGNORE INTO cases (id, workspace_id, kind, title, goal, status, state_json, priority, max_steps, budget_usd,
                                      message_id, requirement_id, thread_id, created_by, created_at, updated_at, agent_user_id,
                                      agent_id, agent_version, \"trigger\", shadow)
         VALUES (?1, ?2, ?3, ?4, ?5, 'running', ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?14, ?15, ?16, ?17, ?18, ?19)",
    )
    .bind(&id)
    .bind(new.workspace_id)
    .bind(new.kind)
    .bind(super::sentence(&new.title, 160))
    .bind(super::sentence(&new.goal, 400))
    .bind(new.state.to_string())
    .bind(&new.priority)
    .bind(if new.shadow { config.agent_max_steps.clamp(1, crate::agent_registry::SHADOW_MAX_STEPS) } else { config.agent_max_steps.max(1) })
    .bind(config.agent_budget_usd.max(0.0))
    .bind(new.message_id)
    .bind(new.requirement_id)
    .bind(new.thread_id)
    .bind(new.created_by)
    .bind(now)
    .bind(new.agent_user_id)
    .bind(new.agent_id)
    .bind(new.agent_version)
    .bind(new.trigger)
    .bind(new.shadow)
    .execute(&mut *conn)
    .await?
    .rows_affected()
        == 1;
    if !created {
        let existing: String = sqlx::query_scalar(
            "SELECT id FROM cases WHERE workspace_id = ?1 AND kind = ?2 AND shadow = ?5
               AND ((?3 IS NOT NULL AND requirement_id = ?3) OR (?4 IS NOT NULL AND message_id = ?4))
             ORDER BY created_at LIMIT 1",
        )
        .bind(new.workspace_id)
        .bind(new.kind)
        .bind(new.requirement_id)
        .bind(new.message_id)
        .bind(new.shadow)
        .fetch_one(&mut *conn)
        .await?;
        return Ok((existing, false));
    }
    audit::record(
        &mut *conn,
        Entry {
            workspace_id: new.workspace_id,
            actor: new.created_by.map(Actor::User).unwrap_or(Actor::System),
            action: "case.started",
            entity_type: "case",
            entity_id: &id,
            message_id: new.message_id,
            metadata: json!({ "kind": new.kind, "requirement_id": new.requirement_id }),
        },
    )
    .await?;
    enqueue_step(&mut *conn, new.workspace_id, &id, 1, now).await?;
    Ok((id, true))
}

/// Moves a case to `to` if the transition is allowed from its current status. Returns true if changed.
pub async fn set_status(conn: &mut SqliteConnection, ws: &str, case_id: &str, to: &str) -> sqlx::Result<bool> {
    let from: Option<String> =
        sqlx::query_scalar("SELECT status FROM cases WHERE id = ?1 AND workspace_id = ?2").bind(case_id).bind(ws).fetch_optional(&mut *conn).await?;
    let Some(from) = from else { return Ok(false) };
    if !can_transition(&from, to) {
        return Ok(false);
    }
    let changed = sqlx::query("UPDATE cases SET status = ?1, updated_at = ?2 WHERE id = ?3 AND workspace_id = ?5 AND status = ?4")
        .bind(to)
        .bind(now_ms())
        .bind(case_id)
        .bind(&from)
        .bind(ws)
        .execute(conn)
        .await?
        .rows_affected()
        == 1;
    Ok(changed)
}

/// Ends a case as failed with a plain reason (no-op if it already ended).
pub async fn fail(conn: &mut SqliteConnection, case: &Case, reason: &str) -> sqlx::Result<bool> {
    if !set_status(&mut *conn, &case.workspace_id, &case.id, "failed").await? {
        return Ok(false);
    }
    sqlx::query("UPDATE cases SET failure_reason = ?1, next_wake_at = NULL WHERE id = ?2 AND workspace_id = ?3")
        .bind(super::sentence(reason, 240))
        .bind(&case.id)
        .bind(&case.workspace_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("UPDATE agent_approvals SET status = 'declined', note = 'Case ended', decided_at = ?1 WHERE case_id = ?2 AND workspace_id = ?3 AND status = 'pending'")
        .bind(now_ms())
        .bind(&case.id)
        .bind(&case.workspace_id)
        .execute(&mut *conn)
        .await?;
    insert_step(&mut *conn, case, NewStep::note(case.step_count, format!("Stopped: {reason}"), "error")).await?;
    audit::record(
        &mut *conn,
        Entry {
            workspace_id: &case.workspace_id,
            actor: Actor::System,
            action: "case.failed",
            entity_type: "case",
            entity_id: &case.id,
            message_id: case.message_id.as_deref(),
            metadata: json!({ "steps": case.step_count, "spent_usd": case.spent_usd }),
        },
    )
    .await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transitions() {
        assert!(can_transition("running", "waiting_approval"));
        assert!(can_transition("waiting_event", "running"));
        assert!(can_transition("waiting_approval", "cancelled"));
        assert!(!can_transition("waiting_approval", "done"));
        for terminal in ["done", "failed", "cancelled"] {
            assert!(is_terminal(terminal));
            assert!(!can_transition(terminal, "running"));
        }
    }

    #[test]
    fn step_keys_follow_the_contract() {
        assert_eq!(step_key("case_1", 4), "agent_step:case_1:4");
    }
}
