//! The system agent designer: plain English to a validated spec, repair, a dry run that writes nothing,
//! saving as a versioned agent, the daily and per-session limits, and rollback.

use crate::common;

use axum::http::StatusCode;
use common::*;
use email_ai_api::{
    agents::registry::{spec::AgentSpec, templates},
    agents::designer::{DAILY_LIMIT, ScriptedDesigner, session},
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
    env.install_inbox().await;
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

    let (st, body) = env.req("POST", &format!("{path}/save"), ADMIN, Some(json!({}))).await;
    assert_eq!(st, StatusCode::CONFLICT, "it would take mail from the Inbox Assistant: {body}");
    assert!(body["error"]["message"].as_str().unwrap().contains("Inbox Assistant"));
    let saved = env.ok("POST", &format!("{path}/save"), ADMIN, Some(json!({"confirm_steals": true}))).await;
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

async fn dry_run_and_wait(env: &Env, id: &str) {
    env.req("POST", &format!("/designer/sessions/{id}/dry-run"), ADMIN, None).await;
    worker::drain(&env.state).await;
}

#[tokio::test]
async fn racing_requests_cannot_go_past_the_daily_limit() {
    let mut script = ScriptedDesigner::new();
    for _ in 0..30 {
        script = script.reply(refund_spec());
    }
    let env = env_with(script).await;
    let url = format!("/api/v1/workspaces/{}/designer/sessions", env.ws);
    let tasks: Vec<_> = (0..30)
        .map(|_| {
            let (app, url) = (env.app.clone(), url.clone());
            tokio::spawn(async move { call(&app, "POST", &url, ADMIN, Some(json!({"goal": "refunds"}))).await.0 })
        })
        .collect();
    let mut created = 0;
    let mut refused = 0;
    for t in tasks {
        match t.await.unwrap() {
            StatusCode::CREATED => created += 1,
            StatusCode::TOO_MANY_REQUESTS => refused += 1,
            other => panic!("unexpected {other}"),
        }
    }
    assert_eq!((created, refused), (DAILY_LIMIT, 30 - DAILY_LIMIT), "exactly the daily limit got through");
    assert_eq!(env.designer.calls() as i64, DAILY_LIMIT, "refused requests never reached the model");
}

#[tokio::test]
async fn a_session_has_one_designer_call_at_a_time_and_failed_calls_are_recorded() {
    let env = env_with(ScriptedDesigner::new().reply(refund_spec()).fail(email_ai_api::ai::AiError::Permanent("down".into()))).await;
    let s = open(&env, json!({"goal": "refunds"})).await;
    let id = s["id"].as_str().unwrap();
    sqlx::query("INSERT INTO designer_calls (id, workspace_id, session_id, user_id, kind, status, created_at) VALUES ('dcl_busy', ?1, ?2, 'u', 'design', 'pending', ?3)")
        .bind(&env.ws)
        .bind(id)
        .bind(email_ai_api::platform::db::now_ms())
        .execute(&env.state.db)
        .await
        .unwrap();
    let (st, _) = env.req("POST", &format!("/designer/sessions/{id}/messages"), ADMIN, Some(json!({"text": "again"}))).await;
    assert_eq!(st, StatusCode::CONFLICT);
    sqlx::query("UPDATE designer_calls SET status = 'done'").execute(&env.state.db).await.unwrap();
    let (st, _) = env.req("POST", &format!("/designer/sessions/{id}/messages"), ADMIN, Some(json!({"text": "again"}))).await;
    assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(env.count("SELECT COUNT(*) FROM designer_calls WHERE workspace_id = ?1 AND status = 'failed'").await, 1, "a failed call still counts");
    assert_eq!(env.count("SELECT COUNT(*) FROM designer_calls WHERE workspace_id = ?1 AND status = 'pending'").await, 0);
}

struct Bumper(sqlx::SqlitePool);

#[async_trait::async_trait]
impl email_ai_api::agents::designer::Designer for Bumper {
    async fn design(&self, _: &email_ai_api::agents::designer::DesignRequest) -> Result<email_ai_api::agents::designer::DesignReply, email_ai_api::ai::AiError> {
        sqlx::query("UPDATE designer_sessions SET rev = rev + 1").execute(&self.0).await.unwrap();
        Ok(email_ai_api::agents::designer::DesignReply { json: refund_spec().to_string(), cost_usd: 0.01 })
    }
}

#[tokio::test]
async fn a_design_turn_that_lost_a_race_does_not_overwrite_the_newer_session() {
    let env = env_with(ScriptedDesigner::new().reply(refund_spec())).await;
    let s = open(&env, json!({"goal": "refunds"})).await;
    let id = s["id"].as_str().unwrap().to_string();
    let mut racing = env.state.clone();
    racing.designer = std::sync::Arc::new(Bumper(env.state.db.clone()));
    let err = session::design_turn(&racing, &env.ws, &id, "shorter please").await;
    assert!(matches!(err, Err(email_ai_api::platform::error::AppError::Conflict(_))), "{err:?}");
    let after = session::get(&env.state.db, &env.ws, &id).await.unwrap().unwrap();
    assert_eq!(after.turns.len(), 2, "the losing turn wrote nothing");
    assert!(after.cost_usd >= 0.01, "but what it cost is still charged");
}

#[tokio::test]
async fn dry_runs_are_metered_rate_limited_and_claimed_once() {
    let env = env_with(ScriptedDesigner::new().reply(refund_spec())).await;
    env.receive(&env.shared_src, "g1", "t1", CLIENT, "Refund for my order", "Please refund order #4411.", "billing").await;
    let s = open(&env, json!({"goal": "refunds"})).await;
    let id = s["id"].as_str().unwrap().to_string();
    let path = format!("/designer/sessions/{id}/dry-run");

    let (a, b) = tokio::join!(env.req("POST", &path, ADMIN, None), env.req("POST", &path, ADMIN, None));
    assert_eq!((a.0, b.0), (StatusCode::ACCEPTED, StatusCode::ACCEPTED));
    assert_eq!(env.count("SELECT COUNT(*) FROM designer_calls WHERE workspace_id = ?1 AND kind = 'dry_run'").await, 1, "two requests, one run");
    worker::drain(&env.state).await;
    let after = env.ok("GET", &format!("/designer/sessions/{id}"), ADMIN, None).await;
    assert_eq!(after["dry_run"]["status"], "done");
    assert!(after["cost_usd"].as_f64().unwrap() > 0.0, "the run is charged to the session: {after}");
    let call_cost: f64 = sqlx::query_scalar("SELECT cost_usd FROM designer_calls WHERE kind = 'dry_run'").fetch_one(&env.state.db).await.unwrap();
    assert!(call_cost > 0.0);

    for _ in 0..4 {
        dry_run_and_wait(&env, &id).await;
    }
    let (st, body) = env.req("POST", &path, ADMIN, None).await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "{body}");
}

#[tokio::test]
async fn a_dry_run_for_an_older_spec_is_dropped() {
    let env = env_with(ScriptedDesigner::new().reply(refund_spec())).await;
    let s = open(&env, json!({"goal": "refunds"})).await;
    let id = s["id"].as_str().unwrap().to_string();
    env.req("POST", &format!("/designer/sessions/{id}/dry-run"), ADMIN, None).await;
    let mut other = refund_spec();
    other["description"] = json!("Something else entirely.");
    env.ok("PUT", &format!("/designer/sessions/{id}/spec"), ADMIN, Some(json!({"spec": other}))).await;
    worker::drain(&env.state).await;
    let after = env.ok("GET", &format!("/designer/sessions/{id}"), ADMIN, None).await;
    assert_eq!(after["dry_run"], Value::Null, "the finished report of the old spec was not stored");
}

#[tokio::test]
async fn a_personal_agents_session_only_sees_its_owners_mailbox_and_has_its_own_quota() {
    let mut personal: AgentSpec = templates::template(templates::PERSONAL_KEY).unwrap().spec();
    personal.description = "Drafts my replies carefully.".into();
    let spec = serde_json::to_value(&personal).unwrap();
    let mut script = ScriptedDesigner::new();
    for _ in 0..8 {
        script = script.reply(spec.clone());
    }
    let env = env_with(script).await;
    env.receive(&env.shared_src, "g1", "t1", CLIENT, "Shared mailbox mail", "Hello team", "question").await;
    let mine = env.receive(&env.op_src, "g2", "t2", CLIENT, "Mail to Priya", "Hello Priya", "question").await;
    let agent_id = env.personal_id(OPERATOR).await;

    let (st, s) = env.req("POST", "/designer/sessions", OPERATOR, Some(json!({"goal": "be careful", "agent_id": agent_id}))).await;
    assert_eq!(st, StatusCode::CREATED, "{s}");
    let id = s["id"].as_str().unwrap().to_string();
    assert_eq!(s["issues"], json!([]), "{s}");
    let shared: String = sqlx::query_scalar("SELECT id FROM email_messages WHERE workspace_id = ?1 AND source_id = 'src_shared'").bind(&env.ws).fetch_one(&env.state.db).await.unwrap();
    let (st, _) = env.req("POST", &format!("/designer/sessions/{id}/dry-run"), OPERATOR, Some(json!({"message_ids": [shared]}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "someone else's mailbox cannot be picked");
    env.req("POST", &format!("/designer/sessions/{id}/dry-run"), OPERATOR, Some(json!({"message_ids": [mine]}))).await;
    worker::drain(&env.state).await;
    let after = env.ok("GET", &format!("/designer/sessions/{id}"), OPERATOR, None).await;
    assert_eq!(after["dry_run"]["sampled"], 1, "{after}");
    env.req("POST", &format!("/designer/sessions/{id}/dry-run"), OPERATOR, None).await;
    worker::drain(&env.state).await;
    let after = env.ok("GET", &format!("/designer/sessions/{id}"), OPERATOR, None).await;
    assert_eq!(after["dry_run"]["sampled"], 1, "the default sample is the owner's mailbox too");

    for n in 0..4 {
        let (st, _) = env.req("POST", &format!("/designer/sessions/{id}/messages"), OPERATOR, Some(json!({"text": format!("tweak {n}")}))).await;
        assert_eq!(st, StatusCode::OK);
    }
    let (st, _) = env.req("POST", &format!("/designer/sessions/{id}/messages"), OPERATOR, Some(json!({"text": "one more"}))).await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "5 a day for a member's own agent");
    assert_eq!(session::calls_today(&env.state.db, &env.ws).await.unwrap(), 0, "the workspace quota is untouched");
}

#[tokio::test]
async fn only_an_admin_approver_opens_a_designer_session_from_a_proposal() {
    let env = env_with(ScriptedDesigner::new().reply(refund_spec())).await;
    let err = session::open_from_proposal(&env.state, &env.ws, &env.op_id, "Refund helper", "Collect order numbers").await;
    assert!(matches!(err, Err(email_ai_api::platform::error::AppError::Forbidden)));
    assert_eq!(env.count("SELECT COUNT(*) FROM designer_sessions WHERE workspace_id = ?1").await, 0);
}

#[tokio::test]
async fn saving_an_edit_of_an_agent_that_changed_since_is_refused() {
    let env = env_with(ScriptedDesigner::new().reply(refund_spec())).await;
    let s = open(&env, json!({"goal": "refunds"})).await;
    let id = s["id"].as_str().unwrap().to_string();
    dry_run_and_wait(&env, &id).await;
    let saved = env.ok("POST", &format!("/designer/sessions/{id}/save"), ADMIN, Some(json!({"confirm_steals": true}))).await;
    let agent_id = saved["agent"]["id"].as_str().unwrap().to_string();

    let mut edit = refund_spec();
    edit["description"] = json!("A second version.");
    env.designer.push(edit.clone());
    let s = open(&env, json!({"goal": "tune it", "agent_id": agent_id})).await;
    let edit_id = s["id"].as_str().unwrap().to_string();
    dry_run_and_wait(&env, &edit_id).await;

    let mut other = refund_spec();
    other["description"] = json!("Changed by someone else meanwhile.");
    env.designer.push(other);
    let s2 = open(&env, json!({"goal": "tune it too", "agent_id": agent_id})).await;
    let other_id = s2["id"].as_str().unwrap().to_string();
    dry_run_and_wait(&env, &other_id).await;
    env.ok("POST", &format!("/designer/sessions/{other_id}/save"), ADMIN, Some(json!({"confirm_steals": true}))).await;

    let (st, body) = env.req("POST", &format!("/designer/sessions/{edit_id}/save"), ADMIN, Some(json!({"confirm_steals": true}))).await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
}

struct CostlyExtractor;

#[async_trait::async_trait]
impl email_ai_api::ai::field_extract::FieldExtractor for CostlyExtractor {
    async fn extract(&self, _: &str, _: &str, _: &[email_ai_api::agents::registry::spec::ExtractField]) -> Result<email_ai_api::ai::field_extract::Extracted, email_ai_api::ai::AiError> {
        Ok(email_ai_api::ai::field_extract::Extracted { found: [("order_id".to_string(), "4411".to_string())].into(), cost_usd: 0.0042 })
    }
}

#[tokio::test]
async fn reading_details_is_charged_to_the_case_and_a_retried_draft_is_not_duplicated() {
    let mut env = env_with(ScriptedDesigner::new().reply(refund_spec())).await;
    env.state.field_extractor = std::sync::Arc::new(CostlyExtractor);
    let s = open(&env, json!({"goal": "refunds"})).await;
    let id = s["id"].as_str().unwrap().to_string();
    dry_run_and_wait(&env, &id).await;
    env.ok("POST", &format!("/designer/sessions/{id}/save"), ADMIN, Some(json!({"start_level": 1, "confirm_steals": true}))).await;
    let msg = env.receive(&env.shared_src, "g1", "t1", CLIENT, "Refund for my order", "Please refund order #4411.", "billing").await;
    let case_id = env.start(&msg).await;
    let case = env.case(ADMIN, &case_id).await;
    let extract = case["steps"].as_array().unwrap().iter().find(|s| s["tool"] == "extract_fields").expect("an extract step");
    assert_eq!(extract["status"], "ok", "{case}");
    assert!(case["spent_usd"].as_f64().unwrap() >= 0.0042, "the reading cost reached the case: {case}");

    let drafts = || env.count("SELECT COUNT(*) FROM drafts WHERE workspace_id = ?1");
    let before = drafts().await;
    assert_eq!(before, 1);
    let origin: String = sqlx::query_scalar("SELECT origin_key FROM drafts WHERE workspace_id = ?1").bind(&env.ws).fetch_one(&env.state.db).await.unwrap();
    assert!(origin.starts_with(&format!("agent:{case_id}:{msg}:")), "{origin}");
}

const PLAIN_REQUESTS: [&str; 4] = [
    "create a agent that observe looks for time sheet per day, capture data invoicing and send me summary",
    "agent for refunds",
    "reply to job seekers and send matching roles",
    "har din timesheet check karo aur invoice banao",
];

fn without_types(mut spec: Value) -> Value {
    for step in spec["steps"].as_array_mut().unwrap() {
        step.as_object_mut().unwrap().remove("type");
        for field in step.get_mut("fields").and_then(Value::as_array_mut).into_iter().flatten() {
            field.as_object_mut().unwrap().remove("type");
        }
    }
    spec
}

#[tokio::test]
async fn short_and_foreign_requests_become_a_ready_agent() {
    for goal in PLAIN_REQUESTS {
        let env = env_with(ScriptedDesigner::new().reply(refund_spec())).await;
        let s = open(&env, json!({"goal": goal})).await;
        assert_eq!((s["status"].as_str(), s["issues"].clone()), (Some("ready"), json!([])), "{goal}: {s}");
    }
}

#[tokio::test]
async fn a_step_without_a_type_is_inferred_saved_and_shown_as_an_assumption() {
    for goal in PLAIN_REQUESTS {
        let env = env_with(ScriptedDesigner::new().reply(without_types(refund_spec()))).await;
        let s = open(&env, json!({"goal": goal})).await;
        assert_eq!(s["status"], "ready", "{goal}: {s}");
        assert_eq!(s["spec"]["steps"][0]["type"], "extract");
        assert_eq!(s["spec"]["steps"][0]["fields"][0]["type"], "string");
        assert_eq!(s["spec"]["steps"][1]["type"], "draft_reply");
        let notes = s["assumptions"].as_array().unwrap();
        assert!(notes.iter().any(|n| n.as_str().unwrap().contains("had no type")), "{notes:?}");
        assert!(s["spec"]["permissions"]["max_autonomy"].as_i64().unwrap() <= 1, "inference never widens permissions");
    }
}

#[tokio::test]
async fn unreadable_answers_try_the_fallback_then_fail_cleanly_and_can_be_retried() {
    let designer = ScriptedDesigner::new();
    for _ in 0..4 {
        designer.push_raw("{\"name\": \"x\", \"steps\": [{\"oops\": 1}]}");
    }
    designer.push(refund_spec());
    let env = env_with(designer).await;
    let s = open(&env, json!({"goal": PLAIN_REQUESTS[0]})).await;
    let id = s["id"].as_str().unwrap().to_string();
    assert_eq!(s["status"], "failed", "{s}");
    assert_eq!(s["issues"][0]["code"], "could_not_build");
    assert!(s["issues"][0]["message"].as_str().unwrap().contains("couldn't build that agent"));
    assert!(s["turns"][1]["text"].as_str().unwrap().contains("one or two sentences"));
    let (st, _) = env.req("POST", &format!("/designer/sessions/{id}/save"), ADMIN, Some(json!({}))).await;
    assert_eq!(st, StatusCode::CONFLICT);

    let s = env.ok("POST", &format!("/designer/sessions/{id}/messages"), ADMIN, Some(json!({"text": PLAIN_REQUESTS[0]}))).await;
    assert_eq!(s["status"], "ready", "{s}");
}
