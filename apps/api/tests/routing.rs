//! Routing end to end: the route recorded for every inbound email, precedence, observing (shadow)
//! agents and manual reassignment.

mod common;

use axum::http::StatusCode;
use common::*;
use email_ai_api::{
    agent_registry::routing::Source,
    agents::planner::{ScriptedPlanner, call as tool, finish},
};
use serde_json::{Value, json};

async fn set_level(env: &Env, template_key: &str, level: i64) {
    sqlx::query("UPDATE agents SET autonomy_level = ?1, autonomy_cap = MAX(autonomy_cap, ?1) WHERE workspace_id = ?2 AND template_key = ?3")
        .bind(level)
        .bind(&env.ws)
        .bind(template_key)
        .execute(&env.state.db)
        .await
        .unwrap();
}

async fn route_of(env: &Env, token: &str, message_id: &str) -> Value {
    env.ok("GET", &format!("/messages/{message_id}/route"), token, None).await["route"].clone()
}

#[tokio::test]
async fn every_inbound_email_gets_a_recorded_route() {
    let env = Env::new(Opts::packs(&["general", "support"])).await;
    let msg = env.receive(&env.shared_src, "m1", "t1", CLIENT, "Where is my parcel?", "Hello, where is my parcel?", "question").await;
    let route = route_of(&env, ADMIN, &msg).await;
    assert!(route["agent_id"].is_string(), "the pipeline routed it by itself: {route}");
    assert_eq!(env.count("SELECT COUNT(*) FROM message_routes WHERE workspace_id = ?1").await, 1);
}

#[tokio::test]
async fn precedence_is_manual_thread_rule_label_personal_generic_fallback() {
    let env = Env::new(Opts::packs(&["general", "support"])).await;
    set_level(&env, "support.resolver", 1).await;
    let resolver = env.agent_id(ADMIN, "support.resolver").await;
    let lead = env.agent_id(ADMIN, "general.lead").await;
    let mine = env.personal_id(OPERATOR).await;

    let label = env.receive(&env.shared_src, "m1", "t1", CLIENT, "Where is my parcel?", "Hello", "question").await;
    let r = env.route(&label).await;
    assert_eq!((r.route.agent_id.as_str(), r.route.source, r.route.shadow), (resolver.as_str(), Source::Label, false));

    let rule = env.receive(&env.shared_src, "m2", "t2", CLIENT, "This is unacceptable", "Hello", "question").await;
    assert_eq!((env.route(&rule).await.route.agent_id, env.route(&rule).await.route.source), (resolver.clone(), Source::Rule), "a keyword rule beats a label trigger");

    let generic = env.receive(&env.shared_src, "m3", "t3", CLIENT, "Hello there", "Hello", "general").await;
    assert_eq!((env.route(&generic).await.route.agent_id, env.route(&generic).await.route.source), (lead.clone(), Source::Fallback), "Meera answers what no expert owns");

    let owned = env.receive(&env.op_src, "m4", "t4", CLIENT, "Hello there", "Hello", "general").await;
    assert_eq!((env.route(&owned).await.route.agent_id, env.route(&owned).await.route.source), (mine.clone(), Source::Personal), "mail in a member's own mailbox goes to their personal agent");

    let quiet = env.receive(&env.shared_src, "m5", "t5", CLIENT, "FYI", "Hello", "general").await;
    env.set_understanding(&quiet, "general", false, json!([])).await;
    assert_eq!((env.route(&quiet).await.route.agent_id, env.route(&quiet).await.route.source), (lead.clone(), Source::Fallback));

    // Thread: an open case in the conversation keeps the conversation with its agent.
    let case_id = env.start(&label).await;
    assert_eq!(env.case(ADMIN, &case_id).await["agent"]["name"], "Ishaan");
    let reply = env.receive(&env.shared_src, "m6", "t1", CLIENT, "Re: Where is my parcel?", "Any news?", "general").await;
    assert_eq!((env.route(&reply).await.route.agent_id, env.route(&reply).await.route.source), (resolver.clone(), Source::Thread));

    // Manual beats everything and survives the next computed route.
    let out = env.ok("POST", &format!("/messages/{reply}/route"), OPERATOR, Some(json!({"agent_id": lead}))).await;
    assert_eq!((out["route"]["source"].as_str(), out["route"]["agent_id"].as_str()), (Some("manual"), Some(lead.as_str())));
    assert_eq!((env.route(&reply).await.route.agent_id, env.route(&reply).await.route.source), (lead.clone(), Source::Manual));
    assert_eq!(route_of(&env, ADMIN, &reply).await["source"], "manual");
    assert_eq!(env.count("SELECT COUNT(*) FROM audit_events WHERE workspace_id = ?1 AND action = 'message.route_corrected'").await, 1);
    let (s, _) = env.req("POST", &format!("/messages/{reply}/route"), OPERATOR, Some(json!({"agent_id": "agt_missing"}))).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let paused = env.ok("PATCH", &format!("/agents/{resolver}"), ADMIN, Some(json!({"status": "paused"}))).await;
    assert_eq!(paused["status"], "paused");
    let (s, _) = env.req("POST", &format!("/messages/{reply}/route"), OPERATOR, Some(json!({"agent_id": resolver}))).await;
    assert_eq!(s, StatusCode::CONFLICT, "only active agents take mail");
    let after_pause = env.route(&label).await;
    assert_eq!((after_pause.route.agent_id.as_str(), after_pause.route.source), (lead.as_str(), Source::Thread), "a paused agent is skipped; the conversation's other route is followed");
}

#[tokio::test]
async fn an_observing_agent_runs_a_shadow_case_beside_the_real_one_and_writes_no_drafts() {
    let script = vec![
        tool("read_email", json!({"message_id": "$message_id"})),
        tool("draft_email", json!({"message_id": "$message_id", "instructions": "Answer the question"})),
        tool("send_email", json!({"draft_id": "$draft_id"})),
        tool("create_task", json!({"title": "Check the parcel"})),
        finish(None, "Done."),
    ];
    let env = Env::new(Opts::packs(&["general", "support"]).planner(ScriptedPlanner::new().script("shadow", script).script("inbox", vec![tool("draft_email", json!({"message_id": "$message_id"})), finish(None, "Drafted.")]))).await;
    set_level(&env, "support.resolver", 0).await;
    let resolver = env.agent_id(ADMIN, "support.resolver").await;
    let inbox = env.agent_id(ADMIN, "general.lead").await;
    let msg = env.receive(&env.shared_src, "m1", "t1", CLIENT, "Where is my parcel?", "Hello, where is my parcel?", "question").await;

    let routed = env.route(&msg).await;
    assert_eq!((routed.route.agent_id.as_str(), routed.route.shadow, routed.route.handler_id.as_str()), (resolver.as_str(), true, inbox.as_str()));
    assert_eq!(route_of(&env, ADMIN, &msg).await["shadow"], true);

    let real_case = env.start(&msg).await;
    let cases = env.cases_of(&msg).await;
    assert_eq!(cases.len(), 2, "a real case and an observing one: {cases:?}");
    let (shadow_id, shadow_agent, shadow_flag) = cases[1].clone();
    assert_eq!((shadow_agent.as_deref(), shadow_flag), (Some(resolver.as_str()), true));
    assert_eq!(cases[0].1.as_deref(), Some(inbox.as_str()));

    // The real case is Meera's, the lead's: she drafts while the expert only observes.
    let real = env.case(ADMIN, &real_case).await;
    assert_eq!((real["status"].as_str(), real["shadow"].as_bool(), real["agent"]["name"].as_str()), (Some("done"), Some(false), Some("Meera")), "{real}");
    // The observing case ran the planner, but nothing it did left a mark.
    let shadow = env.case(ADMIN, &shadow_id).await;
    assert_eq!(shadow["shadow"], true);
    let tools: Vec<String> = shadow["steps"].as_array().unwrap().iter().filter_map(|s| s["tool"].as_str().map(str::to_string)).collect();
    assert_eq!(tools, ["read_email", "draft_email", "send_email", "create_task"]);
    let draft_step = shadow["steps"].as_array().unwrap().iter().find(|s| s["tool"] == "draft_email").unwrap();
    assert_eq!(draft_step["output"]["shadow"], true);
    assert!(draft_step["output"]["body"].as_str().unwrap().contains("Thanks for the update"), "the body stays in the step output");
    assert!(draft_step["output"]["draft_id"].as_str().unwrap().starts_with("shadow:"), "a stand-in id, not a draft");
    assert!(shadow["steps"].as_array().unwrap().iter().filter(|s| s["tool"] == "send_email" || s["tool"] == "create_task").all(|s| s["output"]["simulated"] == true));
    assert_eq!(env.count(&format!("SELECT COUNT(*) FROM drafts WHERE workspace_id = ?1 AND agent_id = '{resolver}'")).await, 0, "shadow cases write no drafts");
    let prompts = env.planner.systems().join("\n");
    assert!(prompts.contains("You are Ishaan, the support expert: a patient problem-solver"), "the desk's persona reaches the planner prompt: {prompts}");
    assert!(prompts.contains("Never sign them with your own name"), "mail goes out in the owner's voice: {prompts}");
    assert_eq!(env.count("SELECT COUNT(*) FROM drafts WHERE workspace_id = ?1").await, 1, "only the real case drafted");
    assert_eq!(env.count(&format!("SELECT COUNT(*) FROM agent_approvals WHERE workspace_id = ?1 AND agent_id = '{resolver}'")).await, 0);
    assert_eq!(env.count("SELECT COUNT(*) FROM review_items WHERE workspace_id = ?1 AND kind = 'agent_task'").await, 0, "no task was created");
    assert!(env.sends().is_empty());
    assert_eq!(shadow["status"], "done", "an observation ends by itself");

    // Trial cases stay out of the default list.
    let default_list = env.ok("GET", "/cases", ADMIN, None).await;
    assert_eq!(default_list["items"].as_array().unwrap().len(), 1);
    let all = env.ok("GET", "/cases?include_shadow=true", ADMIN, None).await;
    assert_eq!(all["items"].as_array().unwrap().len(), 2);
    let by_agent = env.ok("GET", &format!("/cases?agent_id={resolver}&include_shadow=true"), ADMIN, None).await;
    assert_eq!(by_agent["items"].as_array().unwrap().len(), 1);

    // Starting it again changes nothing: one real and one observing case per message.
    env.start(&msg).await;
    assert_eq!(env.cases_of(&msg).await.len(), 2);
}
