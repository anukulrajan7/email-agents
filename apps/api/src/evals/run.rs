//! Eval runs: queued in `eval_runs`, executed by the `run_eval` job, results kept per case.

use serde::Serialize;
use serde_json::{Value, json};
use sqlx::{SqliteConnection, SqliteExecutor, SqlitePool};

use super::{gate, learning_lift, replay, safety};
use crate::{
    agents::registry::{store, versions},
    ai::ledger::{self, AiContext},
    platform::db::{iso, new_id, now_ms},
    http::AppState,
    jobs::{self, kind},
};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EvalRun {
    pub id: String,
    pub workspace_id: Option<String>,
    pub agent_id: Option<String>,
    pub agent_version: Option<i64>,
    pub suite: String,
    pub trigger: String,
    pub status: String,
    pub score: Option<f64>,
    pub cases_total: i64,
    pub cases_passed: i64,
    pub baseline_run_id: Option<String>,
    pub cost_usd: f64,
    pub summary_json: String,
    pub requested_by: Option<String>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub created_at: i64,
}

const COLUMNS: &str = "id, workspace_id, agent_id, agent_version, suite, trigger, status, score, cases_total, cases_passed, baseline_run_id,
    cost_usd, summary_json, requested_by, started_at, finished_at, created_at";

impl EvalRun {
    pub fn summary(&self) -> Value {
        serde_json::from_str(&self.summary_json).unwrap_or(Value::Null)
    }

    /// The API shape (`EvalRun` in the web contract).
    pub fn json(&self, baseline_score: Option<f64>) -> Value {
        json!({
            "id": self.id, "suite": self.suite, "trigger": self.trigger, "status": self.status, "score": self.score,
            "cases_total": self.cases_total, "cases_passed": self.cases_passed, "baseline_score": baseline_score,
            "agent_version": self.agent_version, "cost_usd": self.cost_usd, "note": self.summary()["note"],
            "created_at": iso(self.created_at), "finished_at": self.finished_at.map(iso),
        })
    }
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct EvalResult {
    pub case_key: String,
    pub passed: bool,
    pub score: Option<f64>,
    #[serde(skip)]
    pub detail_json: String,
}

pub async fn get<'e>(db: impl SqliteExecutor<'e>, ws: &str, id: &str) -> sqlx::Result<Option<EvalRun>> {
    sqlx::query_as(&format!("SELECT {COLUMNS} FROM eval_runs WHERE id = ?1 AND workspace_id = ?2")).bind(id).bind(ws).fetch_optional(db).await
}

pub async fn list<'e>(db: impl SqliteExecutor<'e>, ws: &str, agent_id: &str, limit: i64) -> sqlx::Result<Vec<EvalRun>> {
    sqlx::query_as(&format!("SELECT {COLUMNS} FROM eval_runs WHERE workspace_id = ?1 AND agent_id = ?2 ORDER BY created_at DESC, id DESC LIMIT ?3"))
        .bind(ws)
        .bind(agent_id)
        .bind(limit)
        .fetch_all(db)
        .await
}

pub async fn results<'e>(db: impl SqliteExecutor<'e>, run_id: &str) -> sqlx::Result<Vec<EvalResult>> {
    sqlx::query_as("SELECT case_key, passed, score, detail_json FROM eval_results WHERE run_id = ?1 ORDER BY passed, case_key")
        .bind(run_id)
        .fetch_all(db)
        .await
}

/// The newest run of a suite, whatever its state (a version when given).
pub async fn latest<'e>(db: impl SqliteExecutor<'e>, ws: &str, agent_id: &str, version: Option<i64>, suite: &str) -> sqlx::Result<Option<EvalRun>> {
    sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM eval_runs WHERE workspace_id = ?1 AND agent_id = ?2 AND suite = ?3 AND (?4 IS NULL OR agent_version = ?4)
         ORDER BY created_at DESC, id DESC LIMIT 1"
    ))
    .bind(ws)
    .bind(agent_id)
    .bind(suite)
    .bind(version)
    .fetch_optional(db)
    .await
}

/// The newest run that finished with a verdict.
pub async fn latest_finished<'e>(db: impl SqliteExecutor<'e>, ws: &str, agent_id: &str, version: Option<i64>, suite: &str) -> sqlx::Result<Option<EvalRun>> {
    sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM eval_runs WHERE workspace_id = ?1 AND agent_id = ?2 AND suite = ?3 AND (?4 IS NULL OR agent_version = ?4)
           AND status IN ('passed','failed') ORDER BY created_at DESC, id DESC LIMIT 1"
    ))
    .bind(ws)
    .bind(agent_id)
    .bind(suite)
    .bind(version)
    .fetch_optional(db)
    .await
}

/// Queues a run and the job that executes it. A run for the same agent, version and suite that is
/// still waiting is returned instead of a second one.
pub async fn enqueue(
    conn: &mut SqliteConnection,
    ws: &str,
    agent_id: &str,
    version: Option<i64>,
    suite: &str,
    trigger: &str,
    requested_by: Option<&str>,
) -> sqlx::Result<String> {
    debug_assert!(super::SUITES.contains(&suite) && super::TRIGGERS.contains(&trigger));
    let waiting: Option<String> = sqlx::query_scalar(
        "SELECT id FROM eval_runs WHERE workspace_id = ?1 AND agent_id = ?2 AND suite = ?3 AND agent_version IS ?4 AND status IN ('queued','running')",
    )
    .bind(ws)
    .bind(agent_id)
    .bind(suite)
    .bind(version)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(id) = waiting {
        return Ok(id);
    }
    let id = new_id("evr");
    sqlx::query(
        "INSERT INTO eval_runs (id, workspace_id, agent_id, agent_version, suite, trigger, status, requested_by, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'queued', ?7, ?8)",
    )
    .bind(&id)
    .bind(ws)
    .bind(agent_id)
    .bind(version)
    .bind(suite)
    .bind(trigger)
    .bind(requested_by)
    .bind(now_ms())
    .execute(&mut *conn)
    .await?;
    jobs::enqueue(conn, kind::RUN_EVAL, &format!("run_eval:{id}"), Some(ws), &json!({"run_id": id})).await?;
    Ok(id)
}

struct Finished {
    status: &'static str,
    score: Option<f64>,
    total: i64,
    passed: i64,
    cost_usd: f64,
    baseline: Option<String>,
    summary: Value,
    results: Vec<(String, bool, Option<f64>, Value)>,
}

impl Finished {
    fn error(note: &str) -> Self {
        Finished { status: "error", score: None, total: 0, passed: 0, cost_usd: 0.0, baseline: None, summary: json!({"note": note}), results: vec![] }
    }
}

async fn finish(db: &SqlitePool, run: &EvalRun, f: Finished) -> sqlx::Result<()> {
    let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query("DELETE FROM eval_results WHERE run_id = ?1").bind(&run.id).execute(&mut *tx).await?;
    for (key, passed, score, detail) in f.results {
        sqlx::query("INSERT INTO eval_results (run_id, case_key, passed, score, detail_json) VALUES (?1, ?2, ?3, ?4, ?5)")
            .bind(&run.id)
            .bind(key)
            .bind(passed)
            .bind(score)
            .bind(detail.to_string())
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query(
        "UPDATE eval_runs SET status = ?1, score = ?2, cases_total = ?3, cases_passed = ?4, baseline_run_id = ?5, cost_usd = ?6,
                              summary_json = ?7, finished_at = ?8 WHERE id = ?9",
    )
    .bind(f.status)
    .bind(f.score)
    .bind(f.total)
    .bind(f.passed)
    .bind(f.baseline)
    .bind(f.cost_usd)
    .bind(f.summary.to_string())
    .bind(now_ms())
    .bind(&run.id)
    .execute(&mut *tx)
    .await?;
    if let (Some(ws), Some(agent), Some(version)) = (&run.workspace_id, &run.agent_id, run.agent_version)
        && run.trigger == "version"
    {
        gate::resolve_version(&mut tx, ws, agent, version).await?;
    }
    tx.commit().await
}

fn from_suite(r: &safety::SuiteResult, note: Option<&str>) -> Finished {
    let results: Vec<_> = r
        .outcomes
        .iter()
        .map(|o| {
            (
                o.id.clone(),
                o.safe(),
                None,
                json!({"category": o.category, "outcome": o.class, "steps": o.steps, "problems": o.failures}),
            )
        })
        .collect();
    let (total, passed) = (r.total() as i64, r.passed() as i64);
    Finished {
        status: if r.all_safe() { "passed" } else { "failed" },
        score: (total > 0).then(|| passed as f64 / total as f64),
        total,
        passed,
        cost_usd: r.cost_usd,
        baseline: None,
        summary: json!({"budget_hit": r.budget_hit, "note": note}),
        results,
    }
}

/// Executes a queued run. Idempotent: a run that already has a verdict is left alone.
pub async fn execute(state: &AppState, run_id: &str) -> sqlx::Result<()> {
    let Some(run): Option<EvalRun> = sqlx::query_as(&format!("SELECT {COLUMNS} FROM eval_runs WHERE id = ?1")).bind(run_id).fetch_optional(&state.db).await? else {
        return Ok(());
    };
    if !matches!(run.status.as_str(), "queued" | "running") {
        return Ok(());
    }
    sqlx::query("UPDATE eval_runs SET status = 'running', started_at = ?1 WHERE id = ?2").bind(now_ms()).bind(run_id).execute(&state.db).await?;
    let finished = compute(state, &run).await?;
    finish(&state.db, &run, finished).await
}

async fn compute(state: &AppState, run: &EvalRun) -> sqlx::Result<Finished> {
    if run.trigger == "version" && !state.config.evals_on_version {
        return Ok(Finished::error("Automatic checks of new versions are switched off."));
    }
    let (Some(ws), Some(agent_id)) = (run.workspace_id.as_deref(), run.agent_id.as_deref()) else {
        return Ok(Finished::error("This run has no agent."));
    };
    let Some(agent) = store::get_in(&state.db, ws, agent_id).await? else { return Ok(Finished::error("The agent no longer exists.")) };
    let spec = match run.agent_version {
        Some(v) => match versions::get(&state.db, agent_id, v).await?.and_then(|r| r.spec()) {
            Some(spec) => spec,
            None => return Ok(Finished::error("That version can no longer be read.")),
        },
        None => versions::active_spec_in(&state.db, &agent).await?.0,
    };
    match run.suite.as_str() {
        "safety" => Ok(from_suite(&safety::run_for_spec(&spec).await, None)),
        "golden_live" => {
            let budget = state.config.evals_live_max_usd;
            if state.config.openrouter_api_key.trim().is_empty() || budget <= 0.0 {
                return Ok(Finished::error("The live suite needs the AI service and a budget."));
            }
            let result = safety::run_live(&state.config, budget, "").await;
            let note = result.budget_hit.then_some("Stopped early: the budget for this run was reached.");
            Ok(from_suite(&result, note))
        }
        "replay" => {
            let ctx = AiContext { workspace_id: Some(ws.to_string()), message_id: Some(format!("eval:{}", run.id)), case_id: None, attempt: 1 };
            let result = ledger::with_context(ctx, replay::run(state, ws, agent_id, &spec)).await?;
            let cost: f64 = sqlx::query_scalar("SELECT COALESCE(SUM(cost_usd), 0.0) FROM ai_calls WHERE message_id = ?1")
                .bind(format!("eval:{}", run.id))
                .fetch_one(&state.db)
                .await?;
            let score = result.score();
            let baseline = match run.agent_version {
                Some(v) => {
                    sqlx::query_as::<_, EvalRun>(&format!(
                        "SELECT {COLUMNS} FROM eval_runs WHERE workspace_id = ?1 AND agent_id = ?2 AND suite = 'replay' AND agent_version < ?3
                           AND status = 'passed' AND score IS NOT NULL ORDER BY agent_version DESC, created_at DESC LIMIT 1"
                    ))
                    .bind(ws)
                    .bind(agent_id)
                    .bind(v)
                    .fetch_optional(&state.db)
                    .await?
                }
                None => None,
            };
            let violations = result.cases.iter().filter(|c| !c.passed && c.score.is_some()).count();
            let regressed = matches!((score, baseline.as_ref().and_then(|b| b.score)), (Some(s), Some(b)) if s < b - replay::TOLERANCE);
            let note = if result.cases.is_empty() {
                Some("There are no approved or sent replies from this agent to replay yet.")
            } else if result.budget_hit {
                Some("Stopped early: the daily AI budget was reached.")
            } else if regressed {
                Some("Replies are further from what people sent than the previous version's.")
            } else {
                None
            };
            Ok(Finished {
                status: if regressed || violations > 0 { "failed" } else { "passed" },
                score,
                total: result.cases.len() as i64,
                passed: result.cases.iter().filter(|c| c.passed).count() as i64,
                cost_usd: cost,
                baseline: baseline.map(|b| b.id),
                summary: json!({"note": note, "judged": result.judged, "budget_hit": result.budget_hit}),
                results: result.cases.into_iter().map(|c| (c.key, c.passed, c.score, c.detail)).collect(),
            })
        }
        "learning_lift" => Ok(Finished::error(learning_lift::UNAVAILABLE)),
        other => Ok(Finished::error(&format!("Unknown suite {other}."))),
    }
}
