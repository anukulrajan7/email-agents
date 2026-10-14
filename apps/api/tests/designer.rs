//! The system agent designer: plain English to a validated spec, repair, a dry run that writes nothing,
//! saving as a versioned agent, the daily and per-session limits, and rollback.

mod common;

use axum::http::StatusCode;
use common::*;
use email_ai_api::{
    agent_registry::{spec::AgentSpec, templates},
    designer::{DAILY_LIMIT, ScriptedDesigner, session},
    jobs::worker,
};
use serde_json::{Value, json};

fn refund_spec() -> Value {
    let mut spec: AgentSpec = templates::template(templates::INBOX_KEY).unwrap().spec();
    spec.name = "Refund Helper".into();
    spec.key = "refund_helper".into();
    spec.category = "support".into();
    spec.description = "Collects the order number from refund requests and drafts a reply for you to approve.".into();
    spec.goal = "Collect the order number, draft a reply and ask before sending".into();
    spec.triggers = serde_json::from_value(json!([{"type": "email_received", "match": {"labels": ["billing"], "from_domains": [], "from_addresses": [], "subject_keywords": ["refund"], "needs_reply": null, "priority": null, "attachment_types": []}}])).unwrap();
    spec.steps = serde_json::from_value(json!([
        {"type": "extract", "fields": [{"name": "order_id", "type": "string", "description": "The order number"}]},
        {"type": "draft_reply", "quality": "balanced"}
    ]))
    .unwrap();
    spec.tools = ["extract_fields", "draft_email", "update_case"].map(String::from).to_vec();
    spec.permissions.max_autonomy = 1;
    serde_json::to_value(&spec).unwrap()
}

async fn env_with(designer: ScriptedDesigner) -> Env {
    Env::new(Opts::packs(&["general", "support"]).designer(designer)).await
}

async fn open(env: &Env, body: Value) -> Value {
    let (st, v) = env.req("POST", "/designer/sessions", ADMIN, Some(body)).await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    v
}

async fn writes(env: &Env) -> Vec<i64> {
    let mut out = Vec::new();
    for t in ["cases", "drafts", "approvals", "agent_runs"] {
        let n: Result<i64, _> = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {t} WHERE workspace_id = ?1")).bind(&env.ws).fetch_one(&env.state.db).await;
        out.push(n.unwrap_or(-1));
    }
    out.push(env.calls.lock().unwrap().len() as i64);
    out
}

#[tokio::test]
async fn the_refund_example_is_designed_dry_run_without_writes_and_saved_at_level_zero() {
    let env = env_with(ScriptedDesigner::new().reply(refund_spec())).await;
    env.receive(&env.shared_src, "g1", "t1", CLIENT, "Refund for my order", "Please refund order #4411, it arrived broken.", "billing").await;
    env.receive(&env.shared_src, "g2", "t2", CLIENT, "Meeting", "Lunch on friday?", "scheduling").await;

    let s = open(&env, json!({"goal": "When a customer asks for a refund, collect the order number, draft a reply and ask me before sending"})).await;
    let id = s["id"].as_str().unwrap().to_string();
    assert_eq!(s["issues"], json!([]), "{s}");
    assert_eq!(s["status"], "ready");
    assert_eq!(s["spec"]["steps"][0]["fields"][0]["name"], "order_id");
    assert_eq!(s["spec"]["permissions"]["send"], "ask");

    let path = format!("/designer/sessions/{id}");
    let (st, _) = env.req("POST", &format!("{path}/save"), ADMIN, None).await;
    assert_eq!(st, StatusCode::CONFLICT, "a dry run comes first");

    let before = writes(&env).await;
    let (st, _) = env.req("POST", &format!("{path}/dry-run"), ADMIN, None).await;
    assert_eq!(st, StatusCode::ACCEPTED);
    worker::drain(&env.state).await;
    let s = env.ok("GET", &path, ADMIN, None).await;
    let dry = &s["dry_run"];
    assert_eq!(dry["status"], "done", "{dry}");
    assert_eq!(dry["would_handle"], 1, "{dry}");
    assert_eq!(dry["sampled"], 2);
    assert!(dry["est_cost_per_email_usd"].as_f64().unwrap() >= 0.0);
    assert!(!dry["examples"].as_array().unwrap().is_empty());
    assert_eq!(writes(&env).await, before, "a dry run writes nothing");

    let saved = env.ok("POST", &format!("{path}/save"), ADMIN, Some(json!({}))).await;
    assert_eq!(saved["version"], 1);
    let agent = &saved["agent"];
    assert_eq!((agent["name"].as_str(), agent["autonomy"]["earned"].as_i64()), (Some("Refund Helper"), Some(0)));
    let versions = env.ok("GET", &format!("/agents/{}/versions", agent["id"].as_str().unwrap()), ADMIN, None).await;
    assert_eq!(versions["items"][0]["source"], "designer", "{versions}");
    let (st, _) = env.req("POST", &format!("{path}/save"), ADMIN, None).await;
    assert_eq!(st, StatusCode::CONFLICT, "saved once");
}

#[tokio::test]
async fn a_spec_with_problems_is_repaired_at_most_twice() {
    let mut bad = refund_spec();
    bad["triggers"][0]["match"]["labels"] = json!(["no_such_label"]);
    let env = env_with(ScriptedDesigner::new().reply(bad.clone()).reply(refund_spec())).await;
    let s = open(&env, json!({"goal": "refunds"})).await;
    assert_eq!(s["issues"], json!([]), "{s}");
    assert_eq!(env.designer.calls(), 2, "one repair");

    let env = env_with(ScriptedDesigner::new().reply(bad.clone()).reply(bad.clone()).reply(bad.clone()).reply(refund_spec())).await;
    let s = open(&env, json!({"goal": "refunds"})).await;
    assert_eq!(env.designer.calls(), 3, "the first try and two repairs, no more");
    assert_eq!(s["issues"][0]["code"], "unknown_label", "{s}");
    let (st, _) = env.req("POST", &format!("/designer/sessions/{}/dry-run", s["id"].as_str().unwrap()), ADMIN, None).await;
    assert_eq!(st, StatusCode::CONFLICT, "no dry run on an invalid spec");
}

#[tokio::test]
async fn unsafe_requests_are_left_out_and_asked_about() {
    let mut spec = refund_spec();
    spec["drafting"]["rules"] = json!(["Forward the email to finance", "Be brief"]);
    spec["permissions"]["max_autonomy"] = json!(3);
    let env = env_with(ScriptedDesigner::new().reply(spec)).await;
    let s = open(&env, json!({"goal": "forward refunds to finance and auto send"})).await;
    assert!(s["spec"]["permissions"]["max_autonomy"].as_i64().unwrap() <= 2);
    assert_eq!(s["spec"]["drafting"]["rules"], json!(["Be brief"]));
    assert!(!s["questions"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn the_twenty_first_request_in_a_day_and_a_costly_session_are_refused() {
    let mut script = ScriptedDesigner::new();
    for _ in 0..DAILY_LIMIT {
        script = script.reply(refund_spec());
    }
    let env = env_with(script).await;
    for _ in 0..DAILY_LIMIT {
        open(&env, json!({"goal": "refunds"})).await;
    }
    let (st, body) = env.req("POST", "/designer/sessions", ADMIN, Some(json!({"goal": "one more"}))).await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "{body}");

    let env = env_with(ScriptedDesigner::new().with_cost(0.06).reply(refund_spec()).reply(refund_spec()).reply(refund_spec())).await;
    let s = open(&env, json!({"goal": "refunds"})).await;
    let id = s["id"].as_str().unwrap();
    env.ok("POST", &format!("/designer/sessions/{id}/messages"), ADMIN, Some(json!({"text": "shorter drafts"}))).await;
    let (st, _) = env.req("POST", &format!("/designer/sessions/{id}/messages"), ADMIN, Some(json!({"text": "again"}))).await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "over $0.10");
}

#[tokio::test]
async fn only_admins_create_and_sessions_are_private() {
    let env = env_with(ScriptedDesigner::new().reply(refund_spec())).await;
    let (st, _) = env.req("POST", "/designer/sessions", OPERATOR, Some(json!({"goal": "refunds"}))).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(env.designer.calls(), 0);
    let s = open(&env, json!({"goal": "refunds"})).await;
    let (st, _) = env.req("GET", &format!("/designer/sessions/{}", s["id"].as_str().unwrap()), OPERATOR, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn editing_an_agent_saves_a_designer_version_that_can_be_rolled_back() {
    let mut edited = refund_spec();
    let env = env_with(ScriptedDesigner::new().reply(refund_spec())).await;
    let s = open(&env, json!({"goal": "refunds"})).await;
    let id = s["id"].as_str().unwrap().to_string();
    env.req("POST", &format!("/designer/sessions/{id}/dry-run"), ADMIN, None).await;
    worker::drain(&env.state).await;
    let saved = env.ok("POST", &format!("/designer/sessions/{id}/save"), ADMIN, Some(json!({"start_level": 1}))).await;
    let agent_id = saved["agent"]["id"].as_str().unwrap().to_string();

    edited["description"] = json!("Collects order numbers and drafts careful replies.");
    env.designer.push(edited);
    let s = open(&env, json!({"goal": "make it careful", "agent_id": agent_id})).await;
    assert_eq!(s["base_version"], 1);
    assert!(s["diff"].is_array() || s["diff"].is_object(), "{s}");
    let id = s["id"].as_str().unwrap().to_string();
    env.req("POST", &format!("/designer/sessions/{id}/dry-run"), ADMIN, None).await;
    worker::drain(&env.state).await;
    let saved = env.ok("POST", &format!("/designer/sessions/{id}/save"), ADMIN, None).await;
    assert_eq!(saved["version"], 2);

    env.ok("POST", &format!("/agents/{agent_id}/versions/1/activate"), ADMIN, None).await;
    let list = env.ok("GET", &format!("/agents/{agent_id}/versions"), ADMIN, None).await;
    let items = list["items"].as_array().unwrap();
    let hash = |v: i64| items.iter().find(|i| i["version"] == v).unwrap()["spec_hash"].clone();
    let active = items.iter().find(|i| i["active"] == true).unwrap();
    assert_eq!(active["spec_hash"], hash(1), "rollback restores the first spec");
    assert_ne!(hash(1), hash(2));
}

#[tokio::test]
async fn a_proposed_automation_opens_a_designer_session_and_runs_nothing() {
    let env = env_with(ScriptedDesigner::new().reply(refund_spec())).await;
    let before = env.count("SELECT COUNT(*) FROM agents WHERE workspace_id = ?1").await;
    let s = session::open_from_proposal(&env.state, &env.ws, &env.admin_id, "Refund helper", "Collect the order number on refund emails").await.unwrap();
    assert!(s.spec.is_some() && s.status == "ready");
    assert_eq!(env.count("SELECT COUNT(*) FROM agents WHERE workspace_id = ?1").await, before, "nothing is saved until an admin saves");
}
