//! In-product evals: the safety suite (the golden harness as a library), replay of past approved
//! drafts against a candidate version, and the gates that decide when an agent has earned more
//! autonomy. Runs are stored in `eval_runs` / `eval_results` and shown on the Evals page.

pub mod gate;
pub mod judge;
pub mod learning_lift;
pub mod replay;
pub mod run;
pub mod safety;

pub const SUITES: &[&str] = &["safety", "replay", "golden_live", "learning_lift"];
pub const TRIGGERS: &[&str] = &["version", "schedule", "manual", "promotion"];
