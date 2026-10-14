//! `agent_step:<case>:<turn>` jobs: one planner turn each, so cases survive restarts and can sleep for days.

use serde_json::Value;

use super::{Job, handlers::JobError};
use crate::{agents, http::AppState};

fn case_and_turn(payload: &Value) -> Result<(String, i64), JobError> {
    let case_id = payload.get("case_id").and_then(Value::as_str).ok_or_else(|| JobError::Permanent("payload missing case_id".into()))?;
    let turn = payload.get("turn").and_then(Value::as_i64).ok_or_else(|| JobError::Permanent("payload missing turn".into()))?;
    Ok((case_id.to_string(), turn))
}

/// The workspace comes from the job record, which the queue set when the step was scheduled.
fn workspace_of(job: &Job) -> Result<&str, JobError> {
    job.workspace_id.as_deref().ok_or_else(|| JobError::Permanent("agent step without a workspace".into()))
}

pub async fn agent_step(state: &AppState, job: &Job, payload: &Value) -> Result<(), JobError> {
    let (case_id, turn) = case_and_turn(payload)?;
    agents::runner::run_step(state, workspace_of(job)?, &case_id, turn).await
}

/// A turn that kept failing (e.g. the AI service was down for its whole retry window) fails the case.
pub async fn on_dead(state: &AppState, job: &Job, payload: &Value, _error: &str) -> Result<(), JobError> {
    let (case_id, _) = case_and_turn(payload)?;
    let Some(case) = agents::case::get_in(&state.db, workspace_of(job)?, &case_id).await? else { return Ok(()) };
    let mut tx = state.db.begin().await?;
    agents::case::fail(&mut tx, &case, "the AI service could not be reached for this step").await?;
    tx.commit().await?;
    Ok(())
}
