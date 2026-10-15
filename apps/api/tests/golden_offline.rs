//! Golden-set safety eval, offline and deterministic (runs in CI). Every scenario goes through the
//! real runner, tools, critic rules and send gates with a scripted planner that does what the
//! scenario says, including what a planner fooled by an injected email would do. What is checked
//! does not depend on a model: nothing leaves the mailbox without a person (except the explicit
//! auto-send control), `[NEEDS: ...]` drafts never go out, step and budget caps hold, and
//! switched-off permissions block their tools. Quality against a real model: `golden_live.rs`.

use crate::golden_harness as harness;

use std::collections::HashSet;

use harness::{load_scenarios, scorecard};

#[test]
fn scenario_files_are_well_formed_and_cover_the_use_cases() {
    let scenarios = load_scenarios();
    assert!(scenarios.len() >= 20, "{} scenarios", scenarios.len());
    assert_eq!(harness::lint(&scenarios), Vec::<String>::new());
    let categories: HashSet<&str> = scenarios.iter().map(|s| s.category.as_str()).collect();
    for needed in [
        "support_question",
        "sales_lead",
        "finance_query",
        "personal_scheduling",
        "client_requirement",
        "angry_customer",
        "prompt_injection",
        "needs_info",
        "no_action",
        "permissions",
        "limits",
    ] {
        assert!(categories.contains(needed), "missing category {needed}");
    }
}

#[tokio::test]
async fn golden_scenarios_hold_the_safety_invariants() {
    let mut outcomes = Vec::new();
    for s in load_scenarios() {
        let o = harness::run_scenario(&s, false).await;
        println!("{} {} -> {} ({} steps)", if o.passed() { "PASS" } else { "FAIL" }, o.id, o.class, o.steps);
        outcomes.push(o);
    }
    let card = scorecard("Offline golden eval (scripted planner, canned drafter)", &outcomes);
    println!("{card}");
    let failed: Vec<&str> = outcomes.iter().filter(|o| !o.passed()).map(|o| o.id.as_str()).collect();
    assert!(failed.is_empty(), "scenarios failed: {failed:?}\n{card}");
}
