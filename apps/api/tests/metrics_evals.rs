//! Metrics scorecards, in-product eval runs and the autonomy checklist.

mod common;

use axum::http::StatusCode;
use common::*;
use email_ai_api::evals::run;
use serde_json::{Value, json};

#[tokio::test]
async fn a_safety_run_is_stored_and_the_scorecard_lists_every_visible_agent() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let lead = env.agent_id(ADMIN, "general.lead").await;

    let started = env.ok("POST", &format!("/agents/{lead}/evals"), ADMIN, Some(json!({"suite": "safety"}))).await;
    assert_eq!((started["suite"].as_str(), started["status"].as_str()), (Some("safety"), Some("queued")));
    run::execute(&env.state, started["id"].as_str().unwrap()).await.unwrap();

    let list = env.ok("GET", &format!("/agents/{lead}/evals"), ADMIN, None).await;
    let first = &list["items"][0];
    assert_eq!(first["status"], "passed");
    assert_eq!(first["score"], 1.0, "the shipped agent passes its own safety suite");
    assert_eq!(list["live_available"], false);

    let detail = env.ok("GET", &format!("/eval-runs/{}", started["id"].as_str().unwrap()), ADMIN, None).await;
    assert!(!detail["results"].as_array().unwrap().is_empty());

    let card = env.ok("GET", "/scorecard", ADMIN, None).await;
    let agents = card["agents"].as_array().unwrap();
    assert!(agents.iter().any(|a| a["agent"]["id"] == lead.as_str() && a["safety"]["score"] == 1.0));
}

#[tokio::test]
async fn a_level_is_refused_until_the_checklist_is_met_and_the_reasons_are_listed() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let lead = env.agent_id(ADMIN, "general.lead").await;

    sqlx::query("UPDATE agents SET autonomy_level = 1 WHERE id = ?1").bind(&lead).execute(&env.state.db).await.unwrap();
    let state = env.ok("GET", &format!("/agents/{lead}/autonomy"), ADMIN, None).await;
    assert!(state["next"]["requirements"].as_array().unwrap().iter().any(|r| r["met"] == false));
    assert!(!state["why"].as_array().unwrap().is_empty());

    let (status, body) = env.req("POST", &format!("/agents/{lead}/autonomy"), ADMIN, Some(json!({"level": 2}))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let metrics: Value = env.ok("GET", &format!("/agents/{lead}/metrics"), ADMIN, None).await;
    assert_eq!(metrics["totals"]["cases"], 0);
    assert_eq!(metrics["series"].as_array().unwrap().len(), 30);
}

#[tokio::test]
async fn people_who_are_not_admins_cannot_start_runs() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let lead = env.agent_id(ADMIN, "general.lead").await;
    let (status, _) = env.req("POST", &format!("/agents/{lead}/evals"), OPERATOR, Some(json!({"suite": "safety"}))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
