use super::super::case::{self, Case};
use crate::{agents::registry::runtime::CaseAgent, http::AppState, jobs::handlers::JobError};
use super::super::sentence;

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
