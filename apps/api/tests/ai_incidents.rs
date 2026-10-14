//! The AI incident log, the demotion it triggers, the health report and the spike alerts.

mod common;

use axum::http::StatusCode;
use common::*;
use email_ai_api::{
    ai_health,
    ai_ledger::{self, AiContext, CallMeta, Usage},
    db::{new_id, now_ms},
    incidents::{self, Kind, NewIncident},
};
use serde_json::json;

async fn set_level(env: &Env, agent: &str, level: i64) {
    sqlx::query("UPDATE agents SET autonomy_level = ?1, autonomy_cap = 3 WHERE id = ?2").bind(level).bind(agent).execute(&env.state.db).await.unwrap();
}

async fn level(env: &Env, agent: &str) -> i64 {
    sqlx::query_scalar("SELECT autonomy_level FROM agents WHERE id = ?1").bind(agent).fetch_one(&env.state.db).await.unwrap()
}

async fn call_row(env: &Env, task: &str, cost: f64, latency: i64, at: i64) {
    sqlx::query("INSERT INTO ai_calls (id, workspace_id, task, model, cost_usd, latency_ms, ok, created_at) VALUES (?1, ?2, ?3, 'm', ?4, ?5, 1, ?6)")
        .bind(new_id("aic"))
        .bind(&env.ws)
        .bind(task)
        .bind(cost)
        .bind(latency)
        .bind(at)
        .execute(&env.state.db)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_leak_on_an_agent_is_logged_without_mail_text_and_lowers_its_level_once() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let lead = env.agent_id(ADMIN, "general.lead").await;
    set_level(&env, &lead, 2).await;

    let first = incidents::record(
        &env.state.db,
        NewIncident::new(&env.ws, Kind::LeakBlocked, json!({"items": 2, "body_text": "Dear Ravi, your salary is 90000", "why": "x".repeat(400)}))
            .agent(Some(&lead))
            .dedupe("leak:d1"),
    )
    .await;
    assert!(first.is_some());
    assert_eq!(level(&env, &lead).await, 1, "a leak costs the agent a level");

    let again = incidents::record(&env.state.db, NewIncident::new(&env.ws, Kind::LeakBlocked, json!({})).agent(Some(&lead)).dedupe("leak:d1")).await;
    assert!(again.is_none(), "the same key records once");
    incidents::record(&env.state.db, NewIncident::new(&env.ws, Kind::SendBlocked, json!({"codes": ["possible_leak"]})).severity("high").agent(Some(&lead)).dedupe("blocked:d1")).await;
    assert_eq!(level(&env, &lead).await, 1, "one burst costs one level");

    let stored: String = sqlx::query_scalar("SELECT details_json FROM ai_incidents WHERE id = ?1").bind(first.unwrap()).fetch_one(&env.state.db).await.unwrap();
    assert!(!stored.contains("salary") && !stored.contains("Ravi"), "{stored}");
    assert!(stored.len() < 400);
    let changes: i64 = env.count("SELECT COUNT(*) FROM autonomy_changes WHERE workspace_id = ?1 AND reason LIKE 'AI incident%'").await;
    assert_eq!(changes, 1);
}

#[tokio::test]
async fn low_severity_incidents_are_logged_but_do_not_demote() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let lead = env.agent_id(ADMIN, "general.lead").await;
    set_level(&env, &lead, 2).await;
    incidents::record(&env.state.db, NewIncident::new(&env.ws, Kind::UnsupportedClaim, json!({"claims": 1})).agent(Some(&lead))).await.unwrap();
    assert_eq!(level(&env, &lead).await, 2);
}

#[tokio::test]
async fn an_unusable_model_answer_becomes_a_schema_failure_incident_on_the_case_agent() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let lead = env.agent_id(ADMIN, "general.lead").await;
    let msg = env.receive(&env.shared_src, "g1", "t1", CLIENT, "Hello", "Please confirm the meeting.", "other").await;
    let case_id = new_id("cas");
    sqlx::query(
        "INSERT INTO cases (id, workspace_id, kind, title, goal, status, state_json, priority, step_count, max_steps, spent_usd, budget_usd, error_streak, message_id, agent_id, created_at, updated_at)
         VALUES (?1, ?2, 'inbox', 't', 'g', 'running', '{}', 'normal', 0, 5, 0, 1, 0, ?3, ?4, ?5, ?5)",
    )
    .bind(&case_id)
    .bind(&env.ws)
    .bind(&msg)
    .bind(&lead)
    .bind(now_ms())
    .execute(&env.state.db)
    .await
    .unwrap();
    let ctx = AiContext { workspace_id: Some(env.ws.clone()), message_id: Some(msg.clone()), case_id: Some(case_id.clone()), attempt: 1 };
    let row = ai_ledger::with_context(ctx, ai_ledger::record_full(&env.state.db, "agent_plan", "m", Usage::default(), &CallMeta::default(), 100, None)).await.unwrap();
    ai_ledger::mark_invalid(&env.state.db, &row, "schema", "not json").await;
    ai_ledger::mark_invalid(&env.state.db, &row, "schema", "not json").await;

    let (kind, agent, case): (String, Option<String>, Option<String>) =
        sqlx::query_as("SELECT kind, agent_id, case_id FROM ai_incidents WHERE workspace_id = ?1").bind(&env.ws).fetch_one(&env.state.db).await.unwrap();
    assert_eq!((kind.as_str(), agent.as_deref(), case.as_deref()), ("schema_failure", Some(lead.as_str()), Some(case_id.as_str())));
}

#[tokio::test]
async fn the_health_report_is_admin_only_and_lists_open_incidents_until_resolved() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let lead = env.agent_id(ADMIN, "general.lead").await;
    let id = incidents::record(&env.state.db, NewIncident::new(&env.ws, Kind::Refusal, json!({"source": "planner"})).agent(Some(&lead))).await.unwrap();

    let (status, _) = env.req("GET", "/ai-health", OPERATOR, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = env.req("GET", "/ai-incidents", OUTSIDER, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let report = env.ok("GET", "/ai-health?days=7", ADMIN, None).await;
    assert_eq!(report["open_total"], 1);
    assert_eq!(report["open"][0]["id"], id.as_str());
    assert_eq!(report["open"][0]["label"], "AI declined a request");
    assert!(report["open"][0]["agent_name"].as_str().is_some());
    assert_eq!(report["by_day"].as_array().unwrap().len(), 7);
    let refusal = report["by_kind"].as_array().unwrap().iter().find(|k| k["kind"] == "refusal").unwrap();
    assert_eq!((refusal["total"].as_i64(), refusal["open"].as_i64()), (Some(1), Some(1)));
    assert!(report["budget"]["daily_usd"].as_f64().unwrap() > 0.0);
    assert!(report["shadow"]["ready_to_switch"].is_boolean());
    assert!(!report.to_string().to_lowercase().contains("gpt") && !report.to_string().contains("deepseek"));

    let (status, _) = env.req("POST", &format!("/ai-incidents/{id}/resolve"), OPERATOR, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    env.ok("POST", &format!("/ai-incidents/{id}/resolve"), ADMIN, None).await;
    let (status, _) = env.req("POST", &format!("/ai-incidents/{id}/resolve"), ADMIN, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "already resolved");
    let after = env.ok("GET", "/ai-health", ADMIN, None).await;
    assert_eq!(after["open_total"], 0);
    let all = env.ok("GET", "/ai-incidents?status=all", ADMIN, None).await;
    assert_eq!(all["items"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn cost_per_email_latency_and_error_rates_come_from_the_ledger() {
    let env = Env::new(Opts::packs(&["general"])).await;
    env.receive(&env.shared_src, "g1", "t1", CLIENT, "Hello", "Hi there.", "other").await;
    sqlx::query("DELETE FROM ai_calls").execute(&env.state.db).await.unwrap();
    let now = now_ms();
    for ms in [100, 200, 300, 400, 5_000] {
        call_row(&env, "classify", 0.001, ms, now - 1_000).await;
    }
    sqlx::query("INSERT INTO ai_calls (id, workspace_id, task, model, cost_usd, latency_ms, ok, error_class, created_at) VALUES (?1, ?2, 'classify', 'm', 0, 10, 0, 'refusal', ?3)")
        .bind(new_id("aic"))
        .bind(&env.ws)
        .bind(now - 500)
        .execute(&env.state.db)
        .await
        .unwrap();
    let report = env.ok("GET", "/ai-health", ADMIN, None).await;
    let task = report["tasks"].as_array().unwrap().iter().find(|t| t["task"] == "classify").unwrap();
    assert_eq!(task["calls"], 6);
    assert_eq!(task["p50_ms"], 300.0);
    assert_eq!(task["p95_ms"], 5_000.0);
    assert!((task["error_rate"].as_f64().unwrap() - 0.1667).abs() < 0.001);
    assert!((task["refusal_rate"].as_f64().unwrap() - 0.1667).abs() < 0.001);
    assert!(report["cost"]["per_email_today_usd"].as_f64().unwrap() > 0.0);
}

#[tokio::test]
async fn a_spend_spike_raises_one_alert_and_one_incident_per_day() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let now = now_ms();
    for d in 1..=7 {
        call_row(&env, "classify", 0.01, 100, now - d * 86_400_000).await;
    }
    assert_eq!(ai_health::evaluate_spikes(&env.state.db, &env.ws, now).await.unwrap(), 0);
    call_row(&env, "draft", 3.0, 100, now - 1_000).await;
    assert_eq!(ai_health::evaluate_spikes(&env.state.db, &env.ws, now).await.unwrap(), 1);
    assert_eq!(ai_health::evaluate_spikes(&env.state.db, &env.ws, now).await.unwrap(), 0);
    assert_eq!(env.count("SELECT COUNT(*) FROM alerts WHERE workspace_id = ?1 AND kind = 'ai_cost_spike'").await, 1);
    assert_eq!(env.count("SELECT COUNT(*) FROM ai_incidents WHERE workspace_id = ?1 AND kind = 'cost_spike'").await, 1);
    let report = env.ok("GET", "/ai-health", ADMIN, None).await;
    assert_eq!(report["cost"]["spike"], true);
}

#[tokio::test]
async fn a_latency_spike_needs_a_clear_jump_over_the_week_before() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let now = now_ms();
    for i in 0..60 {
        call_row(&env, "draft", 0.0, 1_000, now - 2 * 3_600_000 - i * 1_000).await;
    }
    for i in 0..25 {
        call_row(&env, "draft", 0.0, 6_000, now - 1_000 - i * 1_000).await;
    }
    assert_eq!(ai_health::evaluate_spikes(&env.state.db, &env.ws, now).await.unwrap(), 1);
    assert_eq!(env.count("SELECT COUNT(*) FROM alerts WHERE workspace_id = ?1 AND kind = 'ai_latency_spike'").await, 1);
    assert_eq!(env.count("SELECT COUNT(*) FROM ai_incidents WHERE workspace_id = ?1 AND kind = 'latency_spike'").await, 1);
}

#[tokio::test]
async fn one_burst_costs_exactly_one_level_even_when_incidents_race_and_the_sweep_runs() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let lead = env.agent_id(ADMIN, "general.lead").await;
    set_level(&env, &lead, 3).await;
    let mut tasks = vec![];
    for i in 0..6 {
        let (db, ws, lead) = (env.state.db.clone(), env.ws.clone(), lead.clone());
        tasks.push(tokio::spawn(async move {
            incidents::record(&db, NewIncident::new(&ws, Kind::LeakBlocked, json!({"n": i})).agent(Some(&lead)).dedupe(format!("race:{i}"))).await
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    assert_eq!(level(&env, &lead).await, 2);
    assert_eq!(env.count("SELECT COUNT(*) FROM autonomy_changes WHERE workspace_id = ?1 AND to_level < from_level").await, 1);
    let evidence: String = sqlx::query_scalar("SELECT evidence_json FROM autonomy_changes WHERE workspace_id = ?1 AND to_level < from_level")
        .bind(&env.ws)
        .fetch_one(&env.state.db)
        .await
        .unwrap();
    assert!(evidence.contains("incident_ids"), "{evidence}");

    incidents::record(&env.state.db, NewIncident::new(&env.ws, Kind::LeakBlocked, json!({})).agent(Some(&lead)).dedupe("late")).await;
    email_ai_api::evals::gate::sweep(&env.state.db).await.unwrap();
    assert_eq!(level(&env, &lead).await, 2, "the sweep does not charge the same burst again");
}

#[tokio::test]
async fn a_send_block_is_high_only_for_real_safety_findings() {
    assert_eq!(incidents::send_block_severity(&["duplicate"]), "medium");
    assert_eq!(incidents::send_block_severity(&["too_short", "open_gaps", "automated_recipient", "ai_review_unavailable"]), "medium");
    assert_eq!(incidents::send_block_severity(&["duplicate", "possible_leak"]), "high");
    assert_eq!(incidents::send_block_severity(&["unsupported_promise"]), "high");
}

#[tokio::test]
async fn a_spend_spike_needs_a_baseline_of_at_least_three_days() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let now = now_ms();
    call_row(&env, "classify", 0.01, 100, now - 86_400_000).await;
    call_row(&env, "classify", 0.01, 100, now - 2 * 86_400_000).await;
    call_row(&env, "draft", 3.0, 100, now - 1_000).await;
    assert_eq!(ai_health::evaluate_spikes(&env.state.db, &env.ws, now).await.unwrap(), 0);
    call_row(&env, "classify", 0.01, 100, now - 3 * 86_400_000).await;
    assert_eq!(ai_health::evaluate_spikes(&env.state.db, &env.ws, now).await.unwrap(), 1);
}
