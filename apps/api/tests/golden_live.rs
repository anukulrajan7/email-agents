//! Golden-set quality eval against the real configured models. Never runs in CI:
//!
//! ```text
//! OPENROUTER_API_KEY=... cargo test --test golden_live -- --ignored --nocapture
//! GOLDEN_FILTER=inj_ cargo test --test golden_live -- --ignored --nocapture   # a subset by id prefix
//! GOLDEN_WRITE_SCORECARD=1 ...                                                # also record the run in private/evals/scorecard.md
//! GOLDEN_BUDGET_USD=2 ...                                                     # stop once this much is spent (default 2)
//! GOLDEN_AS_LEAD=1 ...                                                        # run every scenario as the lead, not as its own expert
//! ```
//!
//! It runs the same scenarios as the offline eval (minus the ones that need a scripted planner),
//! each as the expert it belongs to (the shipped template installed as the working agent), scores them
//! against the same expectations plus the safety invariants, and prints a scorecard.
//! Sends go to a local mock Gmail, so nothing real leaves the machine, but model calls cost money.

use crate::golden_harness as harness;

use email_ai_api::{
    ai::{
        body::model_body,
        openrouter::OpenRouterClient,
        understand::{OpenRouterUnderstander, UnderstandInput, Understander},
    },
    evals::safety::{LiveModels, RunOptions},
};
use harness::{
    agent_scorecard, agent_scorecard_md, label_accuracy, label_accuracy_md, label_set, live_config, load_scenarios, run_scenario_with, scorecard, spec_for,
    write_scorecard,
};

#[tokio::test]
#[ignore = "calls real models and costs money; run with --ignored"]
async fn golden_scenarios_against_the_real_planner() {
    if live_config("sqlite://unused").is_none() {
        println!("OPENROUTER_API_KEY is not set: nothing to evaluate.");
        return;
    }
    let filter = std::env::var("GOLDEN_FILTER").unwrap_or_default();
    let budget: f64 = std::env::var("GOLDEN_BUDGET_USD").ok().and_then(|v| v.parse().ok()).unwrap_or(2.0);
    let as_lead = std::env::var("GOLDEN_AS_LEAD").is_ok();
    let mut outcomes = Vec::new();
    let mut spent = 0.0;
    for s in load_scenarios().into_iter().filter(|s| !s.setup.offline_only && s.id.starts_with(&filter)) {
        if spent >= budget {
            println!("Budget of ${budget:.2} reached after {} scenarios; stopping.", outcomes.len());
            break;
        }
        let options = RunOptions { live: Some(LiveModels::Env), spec: if as_lead { None } else { spec_for(&s) } };
        let o = run_scenario_with(&s, &options).await;
        spent += o.total_cost_usd.max(o.case_cost_usd);
        println!(
            "{} {} [{}] -> {} ({} steps, ${:.5}){}",
            if o.passed() { "PASS" } else if o.pending.is_some() { "PEND" } else { "FAIL" },
            o.id,
            o.agent,
            o.class,
            o.steps,
            o.case_cost_usd,
            if o.passed() { String::new() } else { format!(" {:?}", o.failures) }
        );
        outcomes.push(o);
    }
    let card = scorecard("Live golden eval (real planner, drafter, critic)", &outcomes);
    println!("{card}");
    println!("{}", agent_scorecard("Live golden eval", &outcomes, true));
    write_scorecard(
        "live golden eval",
        &format!(
            "Real planner, drafter and critic, each scenario run as its own expert{}; sends go to a mock Gmail. Quality misses are reported, only SAFETY failures fail the test.\n\n{}",
            if as_lead { " (GOLDEN_AS_LEAD: all as the lead)" } else { "" },
            agent_scorecard_md(&outcomes, true)
        ),
    );
    let safety: Vec<&str> = outcomes.iter().filter(|o| o.failures.iter().any(|f| f.starts_with("SAFETY"))).map(|o| o.id.as_str()).collect();
    assert!(safety.is_empty(), "safety invariants broken in: {safety:?}");
}

#[tokio::test]
#[ignore = "calls real models and costs money; run with --ignored"]
async fn golden_labels_against_the_real_reader() {
    let Some(config) = live_config("sqlite://unused") else {
        println!("OPENROUTER_API_KEY is not set: nothing to evaluate.");
        return;
    };
    let model = [&config.openrouter_understand_model_id, &config.openrouter_classifier_model_id, &config.openrouter_draft_model_id]
        .into_iter()
        .find(|m| !m.is_empty())
        .cloned()
        .unwrap_or_default();
    let reader = OpenRouterUnderstander::new(OpenRouterClient::new(reqwest::Client::new(), &config.openrouter_api_url, &config.openrouter_api_key), &model);
    let filter = std::env::var("GOLDEN_FILTER").unwrap_or_default();
    let (mut pairs, mut errors) = (Vec::new(), 0);
    for s in load_scenarios().iter().filter(|s| s.label.is_some() && s.id.starts_with(&filter)) {
        let mail = s.emails.last().unwrap();
        let input = UnderstandInput {
            subject: mail.subject.clone(),
            sender: mail.from.clone(),
            received_at_ms: 1_791_900_000_000,
            new_part: model_body(&mail.subject, &mail.body),
            attachments_block: String::new(),
            attachment_names: vec![],
            labels: label_set(s),
            examples: vec![],
            sender_history: None,
            thread_digest: None,
        };
        match reader.understand(&input).await {
            Ok((u, _)) => {
                let want = s.label.clone().unwrap_or_default();
                println!("{} {} expected {want}, got {}", if u.label == want { "OK  " } else { "MISS" }, s.id, u.label);
                pairs.push((want, u.label));
            }
            Err(e) => {
                errors += 1;
                println!("ERR  {}: {e}", s.id);
            }
        }
    }
    let table = label_accuracy_md(&label_accuracy(&pairs));
    println!("\nLabel accuracy, model {model} ({errors} errors)\n{table}");
    write_scorecard(
        "labelling, real reader",
        &format!("The understanding model `{model}` on the golden emails that name a label ({errors} calls failed).\n\n{table}"),
    );
}
