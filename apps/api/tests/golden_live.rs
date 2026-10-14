//! Golden-set quality eval against the real configured models. Never runs in CI:
//!
//! ```text
//! OPENROUTER_API_KEY=... cargo test --test golden_live -- --ignored --nocapture
//! GOLDEN_FILTER=inj_ cargo test --test golden_live -- --ignored --nocapture   # a subset by id prefix
//! ```
//!
//! It runs the same scenarios as the offline eval (minus the ones that need a scripted planner),
//! scores them against the same expectations plus the safety invariants, and prints a scorecard.
//! Sends go to a local mock Gmail, so nothing real leaves the machine, but model calls cost money.

#[path = "golden/harness.rs"]
mod harness;

use harness::{live_config, load_scenarios, scorecard};

#[tokio::test]
#[ignore = "calls real models and costs money; run with --ignored"]
async fn golden_scenarios_against_the_real_planner() {
    if live_config("sqlite://unused").is_none() {
        println!("OPENROUTER_API_KEY is not set: nothing to evaluate.");
        return;
    }
    let filter = std::env::var("GOLDEN_FILTER").unwrap_or_default();
    let mut outcomes = Vec::new();
    for s in load_scenarios().into_iter().filter(|s| !s.setup.offline_only && s.id.starts_with(&filter)) {
        let o = harness::run_scenario(&s, true).await;
        println!("{} {} -> {} ({} steps, ${:.5})", if o.passed() { "PASS" } else { "FAIL" }, o.id, o.class, o.steps, o.case_cost_usd);
        outcomes.push(o);
    }
    let card = scorecard("Live golden eval (real planner, drafter, critic)", &outcomes);
    println!("{card}");
    let safety: Vec<&str> = outcomes.iter().filter(|o| o.failures.iter().any(|f| f.starts_with("SAFETY"))).map(|o| o.id.as_str()).collect();
    assert!(safety.is_empty(), "safety invariants broken in: {safety:?}");
}
