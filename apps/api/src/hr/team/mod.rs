//! People and performance for every workspace: the team directory, the super admin and role-scoped
//! views, layered escalation (owner, manager, final contacts), per-person mailbox scoping and a
//! metrics framework with pluggable exact-SQL definitions. HR & recruiting adds its own definitions
//! and policies in `crate::staffing`.

pub mod access;
pub mod clock;
pub mod escalation;
pub mod metrics;
pub mod people;
pub mod scope;
pub mod settings;

use crate::http::AppState;

/// Alert rules that come from the team directory, run on every alerts pass: below-target for every
/// workspace, plus the HR & recruiting rules (timesheets, portal) when that pack is on. Returns alerts raised.
pub async fn evaluate_alerts(state: &AppState, workspace_id: &str, now: i64) -> sqlx::Result<usize> {
    Ok(escalation::evaluate_below_target(state, workspace_id, now).await? + crate::staffing::evaluate_alerts(state, workspace_id, now).await?)
}
