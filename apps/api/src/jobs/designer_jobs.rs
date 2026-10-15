//! `designer_dry_run:<session>` jobs: replay recent mail against a designer session's spec and store
//! the report on the session. The job writes nothing but that report.

use serde_json::{Value, json};

use super::{Job, handlers::JobError};
use crate::{platform::db::now_ms, agents::designer::dry_run, http::AppState};

fn session_id(payload: &Value) -> Result<&str, JobError> {
    payload.get("session_id").and_then(Value::as_str).ok_or_else(|| JobError::Permanent("payload missing session_id".into()))
}

fn workspace_of(job: &Job) -> Result<&str, JobError> {
    job.workspace_id.as_deref().ok_or_else(|| JobError::Permanent("designer dry run without a workspace".into()))
}

pub async fn designer_dry_run(state: &AppState, job: &Job, payload: &Value) -> Result<(), JobError> {
    let picked: Vec<String> = payload
        .get("message_ids")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    let hash = payload.get("spec_hash").and_then(Value::as_str).ok_or_else(|| JobError::Permanent("payload missing spec_hash".into()))?;
    let call_id = payload.get("call_id").and_then(Value::as_str).unwrap_or_default();
    dry_run::run(state, workspace_of(job)?, session_id(payload)?, &picked, hash, call_id).await
}

/// A dry run that kept failing is shown as failed instead of running forever.
pub async fn on_dead(state: &AppState, job: &Job, payload: &Value) -> Result<(), JobError> {
    sqlx::query(
        "UPDATE designer_sessions SET dry_run_status = 'failed', dry_run_json = ?1, updated_at = ?2
         WHERE id = ?3 AND workspace_id = ?4 AND dry_run_status = 'running'",
    )
    .bind(json!({"error": "The dry run could not finish. Try again."}).to_string())
    .bind(now_ms())
    .bind(session_id(payload)?)
    .bind(workspace_of(job)?)
    .execute(&state.db)
    .await?;
    if let Some(call_id) = payload.get("call_id").and_then(Value::as_str) {
        dry_run::settle(state, call_id, "failed", 0.0).await?;
    }
    Ok(())
}
