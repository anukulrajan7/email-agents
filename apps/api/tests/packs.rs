//! Packs: new workspaces are plain business inboxes; HR behaviour belongs to the staffing pack.

use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use email_ai_api::{
    agents::planner::{ScriptedPlanner, call as tool, finish},
    build_state,
    config::Config,
    db,
    http::{AppState, router},
    jobs::worker,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const ADMIN: &str = "dev:admin@acme.test";
const OPERATOR: &str = "dev:op@acme.test";

async fn setup() -> (AppState, Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut state = build_state(Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()))).await.unwrap();
    db::migrate(&state.db).await.unwrap();
    state.planner = Arc::new(ScriptedPlanner::new().script("requirement", vec![tool("recall_memory", json!({"about": "x@y.z"})), finish(None, "Done")]));
    (state.clone(), router(state), dir)
}

async fn call(app: &Router, method: &str, path: &str, token: &str, body: Option<Value>) -> (StatusCode, Value) {
    let req = Request::builder().method(method).uri(path).header("x-auth-token", token);
    let req = match body {
        Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn keys(v: &Value) -> Vec<String> {
    v["items"].as_array().unwrap().iter().map(|l| l["key"].as_str().unwrap().to_string()).collect()
}

async fn create(app: &Router, body: Value) -> String {
    let (s, ws) = call(app, "POST", "/api/v1/workspaces", ADMIN, Some(body)).await;
    assert_eq!(s, StatusCode::CREATED);
    ws["id"].as_str().unwrap().to_string()
}

async fn labels(app: &Router, ws: &str) -> Vec<String> {
    keys(&call(app, "GET", &format!("/api/v1/workspaces/{ws}/labels"), ADMIN, None).await.1)
}

async fn receive(app: &Router, state: &AppState, ws: &str, src: &str, subject: &str, body: &str) {
    let (s, _) = call(app, "POST", &format!("/api/v1/workspaces/{ws}/sources/{src}/test-email"), ADMIN,
        Some(json!({"from": "Recruiter <hr@capgemini.com>", "subject": subject, "body_text": body}))).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    worker::drain(state).await;
}

async fn count(state: &AppState, table: &str, ws: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table} WHERE workspace_id = ?1")).bind(ws).fetch_one(&state.db).await.unwrap()
}

const REQUIREMENT: &str = "Need 3 developers with Java, Spring Boot and Microservices. 5-8 yrs. Location Pune.";

#[tokio::test]
async fn new_workspace_is_general_with_no_hr_behaviour() {
    let (state, app, _d) = setup().await;
    let ws = create(&app, json!({"name": "Acme"})).await;
    let base = format!("/api/v1/workspaces/{ws}");

    let all = labels(&app, &ws).await;
    assert!(all.contains(&"sales_lead".to_string()) && all.contains(&"other".to_string()));
    for hr in ["timesheet", "leave", "candidate_referral", "client_requirement", "payroll"] {
        assert!(!all.contains(&hr.to_string()), "{hr} must not be seeded");
    }
    let (_, wfs) = call(&app, "GET", &format!("{base}/workflows"), ADMIN, None).await;
    assert!(wfs["items"].as_array().unwrap().is_empty());
    let (_, packs) = call(&app, "GET", &format!("{base}/packs"), ADMIN, None).await;
    assert_eq!(packs["enabled"], json!(["general"]));
    let (_, me) = call(&app, "GET", "/api/v1/me", ADMIN, None).await;
    assert_eq!(me["workspaces"][0]["packs"], json!(["general"]));
    let (_, rs) = call(&app, "GET", &format!("{base}/reply-settings"), ADMIN, None).await;
    assert!(!rs["default_reply_style"].as_str().unwrap().contains("HR"));
    let (_, presets) = call(&app, "GET", &format!("{base}/automations/presets"), ADMIN, None).await;
    assert!(!presets.to_string().contains("timesheet"));
    // Matching setup can't sneak the HR labels in.
    assert_eq!(call(&app, "POST", &format!("{base}/matching/setup"), ADMIN, None).await.0, StatusCode::CONFLICT);
    assert!(!labels(&app, &ws).await.contains(&"client_requirement".to_string()));

    let (_, src) = call(&app, "POST", &format!("{base}/sources/forwarding"), ADMIN, None).await;
    let src = src["id"].as_str().unwrap();
    receive(&app, &state, &ws, src, "Requirement: Java Developer", REQUIREMENT).await;
    receive(&app, &state, &ws, src, "Referral - Rahul Verma", "Name: Rahul Verma\n6 years in Java, SpringBoot.\nPune.\nrahul.v@mail.com").await;
    assert_eq!(count(&state, "requirements", &ws).await, 0);
    assert_eq!(count(&state, "candidates", &ws).await, 0);
    assert_eq!(count(&state, "cases", &ws).await, 0);
    assert_eq!(count(&state, "email_messages", &ws).await, 2);
}

#[tokio::test]
async fn enabling_staffing_restores_hr_behaviour_and_disabling_keeps_data() {
    let (state, app, _d) = setup().await;
    let ws = create(&app, json!({"name": "Acme"})).await;
    let base = format!("/api/v1/workspaces/{ws}");
    let (_, src) = call(&app, "POST", &format!("{base}/sources/forwarding"), ADMIN, None).await;
    let src = src["id"].as_str().unwrap().to_string();

    let (s, v) = call(&app, "PUT", &format!("{base}/packs"), ADMIN, Some(json!({"packs": ["general", "staffing"]}))).await;
    assert_eq!((s, v["enabled"].clone()), (StatusCode::OK, json!(["general", "staffing"])));
    let all = labels(&app, &ws).await;
    for hr in ["timesheet", "leave", "candidate_referral", "client_requirement"] {
        assert!(all.contains(&hr.to_string()), "{hr} missing");
    }
    assert_eq!(all.last().map(String::as_str), Some("other"));
    let (_, wfs) = call(&app, "GET", &format!("{base}/workflows"), ADMIN, None).await;
    assert!(!wfs["items"].as_array().unwrap().is_empty());
    let (_, rs) = call(&app, "GET", &format!("{base}/reply-settings"), ADMIN, None).await;
    assert!(rs["default_reply_style"].as_str().unwrap().contains("HR"));

    receive(&app, &state, &ws, &src, "Requirement: Java Developer", REQUIREMENT).await;
    assert_eq!(count(&state, "requirements", &ws).await, 1);
    assert_eq!(count(&state, "cases", &ws).await, 1);

    let (s, v) = call(&app, "PUT", &format!("{base}/packs"), ADMIN, Some(json!({"packs": ["general"]}))).await;
    assert_eq!((s, v["enabled"].clone()), (StatusCode::OK, json!(["general"])));
    assert!(labels(&app, &ws).await.contains(&"timesheet".to_string()));
    assert_eq!(count(&state, "requirements", &ws).await, 1);
    receive(&app, &state, &ws, &src, "Requirement: Python Developer", "Need 2 developers with Python, Django. 3-5 yrs. Location Chennai.").await;
    assert_eq!(count(&state, "requirements", &ws).await, 1);
    assert_eq!(count(&state, "cases", &ws).await, 1);
}

#[tokio::test]
async fn pack_choice_at_creation_and_catalogue() {
    let (_state, app, _d) = setup().await;
    let (s, cat) = call(&app, "GET", "/api/v1/packs", ADMIN, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(keys(&cat), ["general", "staffing", "support", "sales", "finance", "personal"]);
    assert!(cat["items"][1]["features"].as_array().unwrap().iter().any(|f| f == "matching"));

    let support = create(&app, json!({"name": "Help", "packs": ["support"]})).await;
    let all = labels(&app, &support).await;
    assert!(all.contains(&"bug_report".to_string()) && !all.contains(&"billing".to_string()) && !all.contains(&"timesheet".to_string()));

    let hr = create(&app, json!({"name": "People", "template": "hr"})).await;
    assert!(labels(&app, &hr).await.contains(&"timesheet".to_string()));
    let (_, v) = call(&app, "GET", &format!("/api/v1/workspaces/{hr}/packs"), ADMIN, None).await;
    assert_eq!(v["enabled"], json!(["staffing"]));

    for bad in [json!({"name": "X", "packs": ["nope"]}), json!({"name": "X", "packs": []})] {
        assert_eq!(call(&app, "POST", "/api/v1/workspaces", ADMIN, Some(bad)).await.0, StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn only_admins_change_packs() {
    let (_state, app, _d) = setup().await;
    let ws = create(&app, json!({"name": "Acme"})).await;
    let base = format!("/api/v1/workspaces/{ws}");
    call(&app, "GET", "/api/v1/me", OPERATOR, None).await;
    let (s, _) = call(&app, "POST", &format!("{base}/members"), ADMIN, Some(json!({"email": "op@acme.test", "role": "operator"}))).await;
    assert!(s.is_success());

    assert_eq!(call(&app, "GET", &format!("{base}/packs"), OPERATOR, None).await.0, StatusCode::OK);
    assert_eq!(call(&app, "PUT", &format!("{base}/packs"), OPERATOR, Some(json!({"packs": ["staffing"]}))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&app, "PUT", &format!("{base}/packs"), "dev:stranger@x.test", Some(json!({"packs": ["staffing"]}))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&app, "PUT", &format!("{base}/packs"), ADMIN, Some(json!({"packs": ["bogus"]}))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&app, "PUT", &format!("{base}/packs"), ADMIN, Some(json!({"packs": []}))).await.0, StatusCode::BAD_REQUEST);
    assert!(!labels(&app, &ws).await.contains(&"timesheet".to_string()));
}

#[tokio::test]
async fn migration_backfills_existing_workspaces_onto_staffing() {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::connect(&format!("sqlite://{}", dir.path().join("t.db").display())).await.unwrap();
    let mut before = sqlx::migrate!("./migrations");
    before.migrations = before.migrations.iter().filter(|m| m.version < 32).cloned().collect::<Vec<_>>().into();
    before.run(&pool).await.unwrap();
    for id in ["ws_old1", "ws_old2"] {
        sqlx::query("INSERT INTO workspaces (id, name, created_at) VALUES (?1, 'Old', 1)").bind(id).execute(&pool).await.unwrap();
    }
    db::migrate(&pool).await.unwrap();

    let rows: Vec<(String, String)> = sqlx::query_as("SELECT workspace_id, pack_key FROM workspace_packs ORDER BY workspace_id, pack_key")
        .fetch_all(&pool)
        .await
        .unwrap();
    let expect: Vec<(String, String)> = ["ws_old1", "ws_old2"]
        .iter()
        .flat_map(|w| ["general", "staffing"].map(|k| (w.to_string(), k.to_string())))
        .collect();
    assert_eq!(rows, expect);
    for w in ["ws_old1", "ws_old2"] {
        assert!(email_ai_api::packs::enabled(&pool, w, email_ai_api::packs::STAFFING).await.unwrap());
    }
}
