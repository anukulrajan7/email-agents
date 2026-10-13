//! Staffing operations for the HR & recruiting pack: submissions and outcomes read from sent mail, the
//! billable-staff mail policy and timesheet-missing alerts, portal reconciliation and the HRMS metrics
//! (which plug into `crate::team::metrics`). Everything here is gated by the `staffing` pack.
//!
//! The people directory, escalation, scoping and the metrics framework are generic and live in `crate::team`.

pub mod metrics;
pub mod policy;
pub mod portal;
pub mod submissions;

use serde_json::{Value, json};

use crate::{
    db::now_ms,
    error::{AppError, AppResult},
    http::AppState,
    jobs::{self, handlers::JobError, kind},
    packs,
};

pub const SUBMISSION_UPDATE_LABEL: &str = "submission_update";
pub const SCAN_EVERY_MS: i64 = 15 * 60_000;

pub async fn enabled(db: &sqlx::SqlitePool, workspace_id: &str) -> sqlx::Result<bool> {
    packs::enabled(db, workspace_id, packs::STAFFING).await
}

/// 409 unless the workspace has the HR & recruiting pack.
pub async fn require(db: &sqlx::SqlitePool, workspace_id: &str) -> AppResult<()> {
    if enabled(db, workspace_id).await? {
        Ok(())
    } else {
        Err(AppError::Conflict("this is part of the HR & recruiting pack; turn that pack on first".into()))
    }
}

/// Words that are not part of a person's name in a CV file name or a portal row.
const NOISE: &[&str] = &["cv", "resume", "profile", "updated", "latest", "final", "naukri", "new", "copy", "docx", "pdf", "doc", "rtf", "resumes", "of"];

/// Identity of a candidate across file names and portal rows: lower-case name tokens, noise removed, sorted.
/// "Rahul_Verma_CV.pdf", "Verma, Rahul" and "rahul verma" all give "rahul verma".
pub fn name_key(raw: &str) -> String {
    let stem = match raw.rsplit_once('.') {
        Some((stem, ext)) if (2..=4).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric()) => stem,
        _ => raw,
    };
    let mut tokens: Vec<String> = stem
        .split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|t| t.chars().count() >= 2 && !t.chars().any(|c| c.is_ascii_digit()) && !NOISE.contains(&t.as_str()))
        .collect();
    tokens.sort();
    tokens.dedup();
    tokens.join(" ")
}

/// Enqueues one scan per staffing workspace per 15 minutes (called from the worker's maintenance tick).
pub async fn schedule(state: &AppState) -> sqlx::Result<()> {
    let bucket = now_ms() / SCAN_EVERY_MS;
    let workspaces: Vec<String> =
        sqlx::query_scalar("SELECT workspace_id FROM workspace_packs WHERE pack_key = ?1").bind(packs::STAFFING).fetch_all(&state.db).await?;
    let mut conn = state.db.acquire().await?;
    for ws in workspaces {
        jobs::enqueue_with_attempts(&mut conn, kind::STAFFING_SCAN, &format!("staffing_scan:{ws}:{bucket}"), Some(&ws), &json!({ "workspace_id": ws }), 3)
            .await?;
    }
    Ok(())
}

pub async fn scan_job(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let ws = payload
        .get("workspace_id")
        .and_then(Value::as_str)
        .ok_or_else(|| JobError::Permanent("payload missing workspace_id".into()))?;
    if !enabled(&state.db, ws).await? {
        return Ok(());
    }
    let now = now_ms();
    let report = submissions::scan_workspace(state, ws, now).await?;
    let updated = submissions::update_outcomes(&state.db, ws, now).await?;
    if report.created > 0 || updated > 0 {
        tracing::info!(workspace_id = %ws, created = report.created, outcomes = updated, "submissions scanned");
    }
    Ok(())
}

/// Alert rules that belong to the HR & recruiting pack; the alerts engine calls this on each pass.
pub async fn evaluate_alerts(state: &AppState, workspace_id: &str, now: i64) -> sqlx::Result<usize> {
    if !enabled(&state.db, workspace_id).await? {
        return Ok(0);
    }
    Ok(policy::evaluate_timesheets(state, workspace_id, now).await? + portal::reconcile_and_alert(state, workspace_id, now).await?)
}

#[cfg(test)]
mod tests {
    use super::name_key;

    #[test]
    fn name_keys_ignore_order_case_and_noise() {
        assert_eq!(name_key("Rahul_Verma_CV.pdf"), "rahul verma");
        assert_eq!(name_key("Verma, Rahul"), "rahul verma");
        assert_eq!(name_key("rahul-verma-resume-v2-5yrs.docx"), "rahul verma");
        assert_eq!(name_key("CV.pdf"), "");
    }
}
