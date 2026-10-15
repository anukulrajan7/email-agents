use super::*;

#[derive(Debug, Clone, Default)]
pub struct SuiteResult {
    pub outcomes: Vec<Outcome>,
    /// What the live models cost, from the isolated workspaces' AI ledgers.
    pub cost_usd: f64,
    /// A live run stopped early because the budget cap was reached.
    pub budget_hit: bool,
}

impl SuiteResult {
    pub fn total(&self) -> usize {
        self.outcomes.len()
    }
    pub fn passed(&self) -> usize {
        self.outcomes.iter().filter(|o| o.safe()).count()
    }
    /// Safety is all-or-nothing.
    pub fn all_safe(&self) -> bool {
        !self.outcomes.is_empty() && self.outcomes.iter().all(Outcome::safe)
    }
}

/// One scenario on its own thread and runtime, so a harness panic is a failed case and not a dead worker.
async fn run_isolated(s: &Scenario, options: &RunOptions) -> Outcome {
    let (scenario, opts) = (s.clone(), options.clone());
    let task = tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a tokio runtime");
        rt.block_on(crate::ai::ledger::with_context(crate::ai::ledger::AiContext::default(), run_scenario_with(&scenario, &opts)))
    });
    match task.await {
        Ok(outcome) => outcome,
        Err(_) => Outcome {
            id: s.id.clone(),
            category: s.category.clone(),
            class: "failed".into(),
            status: "error".into(),
            steps: 0,
            case_cost_usd: 0.0,
            total_cost_usd: 0.0,
            emails: s.emails.len(),
            failures: vec!["SAFETY the eval harness crashed on this scenario".into()],
            agent: s.agent().to_string(),
            pending: s.pending.clone(),
        },
    }
}

/// The offline safety suite for one spec: every golden scenario with a scripted planner (including
/// one fooled by injected mail), the spec installed as the working agent at its highest level.
/// Costs nothing.
pub async fn run_for_spec(spec: &AgentSpec) -> SuiteResult {
    let options = RunOptions { live: None, spec: Some(spec.clone()) };
    let limit = Arc::new(tokio::sync::Semaphore::new(8));
    let mut tasks = tokio::task::JoinSet::new();
    for (i, s) in load_scenarios().into_iter().enumerate() {
        let (options, limit) = (options.clone(), limit.clone());
        tasks.spawn(async move {
            let _slot = limit.acquire_owned().await.expect("the semaphore stays open");
            (i, run_isolated(&s, &options).await)
        });
    }
    let mut done = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        done.push(joined.expect("run_isolated catches harness panics"));
    }
    done.sort_by_key(|(i, _)| *i);
    SuiteResult { outcomes: done.into_iter().map(|(_, o)| o).collect(), cost_usd: 0.0, budget_hit: false }
}

/// The golden scenarios against the server's real models, stopping once `budget_usd` is spent.
/// Scenarios that need a scripted planner are skipped.
pub async fn run_live(server: &Config, budget_usd: f64, id_prefix: &str) -> SuiteResult {
    let options = RunOptions { live: Some(LiveModels::Server(Box::new(server.clone()))), spec: None };
    let mut result = SuiteResult::default();
    for s in load_scenarios().into_iter().filter(|s| !s.setup.offline_only && s.id.starts_with(id_prefix)) {
        if result.cost_usd >= budget_usd {
            result.budget_hit = true;
            break;
        }
        let o = run_isolated(&s, &options).await;
        result.cost_usd += o.total_cost_usd.max(o.case_cost_usd);
        result.outcomes.push(o);
    }
    result
}
