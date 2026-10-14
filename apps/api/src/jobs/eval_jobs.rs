//! `run_eval` executes one queued eval run; `rollup_agent_metrics` (hourly) refreshes the metrics
//! rollup and then demotes agents that had an incident or whose approval rate fell.

use serde_json::{Value, json};

use super::{handlers::JobError, kind};
use crate::{db::now_ms, evals, http::AppState, jobs, metrics::rollup};

pub async fn run_eval(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let run_id = payload.get("run_id").and_then(Value::as_str).ok_or_else(|| JobError::Permanent("payload missing run_id".into()))?;
    evals::run::execute(state, run_id).await?;
    Ok(())
}

pub async fn rollup_agent_metrics(state: &AppState) -> Result<(), JobError> {
    let rows = rollup::run_recent(&state.db, None).await?;
    let demoted = evals::gate::sweep(&state.db).await?;
    tracing::info!(rows, demoted, "agent metrics rolled up");
    Ok(())
}

/// A failed run must not stay `running` forever.
pub async fn on_dead(state: &AppState, payload: &Value, error: &str) -> Result<(), JobError> {
    let Some(run_id) = payload.get("run_id").and_then(Value::as_str) else { return Ok(()) };
    sqlx::query("UPDATE eval_runs SET status = 'error', summary_json = ?1, finished_at = ?2 WHERE id = ?3 AND status IN ('queued','running')")
        .bind(json!({"note": "The run could not finish.", "error": crate::sources::normalize::truncate_chars(error, 200)}).to_string())
        .bind(now_ms())
        .bind(run_id)
        .execute(&state.db)
        .await?;
    Ok(())
}

/// Once an hour.
pub async fn schedule(state: &AppState) -> sqlx::Result<()> {
    let hour = now_ms() / 3_600_000;
    let mut conn = state.db.acquire().await?;
    jobs::enqueue(&mut conn, kind::ROLLUP_AGENT_METRICS, &format!("rollup_agent_metrics:{hour}"), None, &json!({})).await?;
    Ok(())
}
