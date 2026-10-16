use super::super::{
    case::{self, Case},
    sentence,
};
use crate::{
    agents::registry::runtime::CaseAgent,
    http::AppState,
    jobs::handlers::JobError,
    platform::audit::{self, Actor, Entry},
};

/// How long a case waits while its agent is paused before looking again.
const PAUSED_RECHECK_MS: i64 = 15 * 60 * 1000;

/// An observing case ends quietly at its step limit: it was only ever a trial run.
pub(super) async fn end_observation(conn: &mut sqlx::SqliteConnection, case: &Case, reason: &str) -> sqlx::Result<()> {
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
pub(super) async fn park_while_inactive(state: &AppState, case: &Case, agent: &CaseAgent, turn: i64) -> Result<(), JobError> {
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
    let resume_at = crate::platform::db::now_ms() + PAUSED_RECHECK_MS;
    sqlx::query("UPDATE cases SET status = 'waiting_event', next_wake_at = ?1, updated_at = ?2 WHERE id = ?3 AND workspace_id = ?4 AND status = 'running'")
        .bind(resume_at)
        .bind(crate::platform::db::now_ms())
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

const CHECK_WAIT_NOTE: &str = "Waiting to run the safety check again";
pub(super) const CHECK_WAIT_NOTE_LIKE: &str = "Waiting to run the safety check again%";

/// The safety check could not run: the case waits and takes the same turn again in `minutes`. Nothing is
/// recorded as a step, so nothing here counts against the agent, and the turn's planner cost is not added to
/// the case's spend: an outage can never use up a case's budget.
pub(super) async fn defer_check(state: &AppState, case: &Case, turn: i64, minutes: i64, count: u32) -> Result<(), JobError> {
    let now = crate::platform::db::now_ms();
    let resume_at = now + minutes * 60_000;
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let waiting = sqlx::query(
        "UPDATE cases SET status = 'waiting_event', next_wake_at = ?1, updated_at = ?2, status_note = ?3
         WHERE id = ?4 AND workspace_id = ?5 AND step_count = ?6 - 1 AND status IN ('running', 'waiting_event')",
    )
    .bind(resume_at)
    .bind(now)
    .bind(format!("{CHECK_WAIT_NOTE}: {minutes} minutes"))
    .bind(&case.id)
    .bind(&case.workspace_id)
    .bind(turn)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if !waiting {
        tx.rollback().await?;
        return Ok(());
    }
    case::merge_state(&mut tx, &case.workspace_id, &case.id, &serde_json::json!({"check_deferrals": {"turn": turn, "n": count}})).await?;
    crate::jobs::enqueue_at(
        &mut tx,
        crate::jobs::kind::AGENT_STEP,
        &format!("{}:check:{count}", case::step_key(&case.id, turn)),
        Some(&case.workspace_id),
        &serde_json::json!({ "case_id": case.id, "turn": turn, "workspace_id": case.workspace_id }),
        crate::jobs::DEFAULT_MAX_ATTEMPTS,
        resume_at,
    )
    .await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &case.workspace_id,
            actor: Actor::System,
            action: "agent.check_deferred",
            entity_type: "case",
            entity_id: &case.id,
            message_id: case.message_id.as_deref(),
            metadata: serde_json::json!({"turn": turn, "deferrals": count, "minutes": minutes}),
        },
    )
    .await?;
    tx.commit().await?;
    tracing::info!(case_id = %case.id, turn, deferrals = count, minutes, "safety check could not run; the step waits");
    Ok(())
}
