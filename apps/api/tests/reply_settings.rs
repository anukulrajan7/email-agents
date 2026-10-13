//! Workspace reply settings (auto drafting, style, signature) and tiered drafting.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use email_ai_api::{
    ai::{AiError, DraftInput, DraftOutput, DraftQuality, Drafter, routing::DEFAULT_REPLY_STYLE},
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

#[derive(Default)]
struct Recorder {
    seen: Mutex<Vec<DraftInput>>,
}

#[async_trait]
impl Drafter for Recorder {
    async fn draft(&self, input: &DraftInput) -> Result<DraftOutput, AiError> {
        self.seen.lock().unwrap().push(input.clone());
        Ok(DraftOutput { body_text: "Hello,\n\nThanks.\n\nBest regards,\nTeam".into(), model_id: "recorder".into(), unanswered: vec![] })
    }
}

struct Env {
    state: AppState,
    app: Router,
    base: String,
    source: String,
    recorder: Arc<Recorder>,
    _dir: tempfile::TempDir,
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

async fn setup_with(tweak: impl FnOnce(&mut Config)) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()));
    tweak(&mut config);
    let mut state = build_state(config).await.unwrap();
    let recorder = Arc::new(Recorder::default());
    state.drafter = recorder.clone();
    db::migrate(&state.db).await.unwrap();
    let app = router(state.clone());
    let (_, ws) = call(&app, "POST", "/api/v1/workspaces", ADMIN, Some(json!({"name":"Acme","template":"hr"}))).await;
    let base = format!("/api/v1/workspaces/{}", ws["id"].as_str().unwrap());
    call(&app, "GET", "/api/v1/me", OPERATOR, None).await;
    let (s, _) = call(&app, "POST", &format!("{base}/members"), ADMIN, Some(json!({"email":"op@acme.test","role":"operator"}))).await;
    assert_eq!(s, StatusCode::OK);
    let (_, src) = call(&app, "POST", &format!("{base}/sources/forwarding"), ADMIN, None).await;
    let source = src["id"].as_str().unwrap().to_string();
    Env { state, app, base, source, recorder, _dir: dir }
}

impl Env {
    async fn email(&self, subject: &str, body: &str) -> String {
        call(
            &self.app,
            "POST",
            &format!("{}/sources/{}/test-email", self.base, self.source),
            ADMIN,
            Some(json!({"subject": subject, "body_text": body})),
        )
        .await;
        worker::drain(&self.state).await;
        let (_, msgs) = call(&self.app, "GET", &format!("{}/messages", self.base), ADMIN, None).await;
        msgs["items"].as_array().unwrap().iter().find(|m| m["subject"] == subject).unwrap()["id"].as_str().unwrap().to_string()
    }

    async fn settings_path(&self) -> String {
        format!("{}/reply-settings", self.base)
    }

    async fn tiers(&self, message_id: &str) -> Vec<String> {
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT json_extract(metadata_json, '$.tier') FROM audit_events
             WHERE action = 'draft.generated' AND message_id = ?1 ORDER BY rowid",
        )
        .bind(message_id)
        .fetch_all(&self.state.db)
        .await
        .unwrap();
        rows
    }

    async fn draft_count(&self) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM drafts").fetch_one(&self.state.db).await.unwrap()
    }
}

#[tokio::test]
async fn settings_get_put_validation_and_roles() {
    let env = setup_with(|c| c.auto_drafts = false).await;
    let path = env.settings_path().await;

    let (s, v) = call(&env.app, "GET", &path, OPERATOR, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!((v["auto_reply"].clone(), v["reply_instructions"].clone(), v["signature"].clone()), (json!(false), json!(""), Value::Null));
    assert_eq!(v["default_reply_style"], DEFAULT_REPLY_STYLE);

    let body = json!({"auto_reply": true, "reply_instructions": "  Warm and short.  ", "signature": "  The Acme Team "});
    assert_eq!(call(&env.app, "PUT", &path, OPERATOR, Some(body.clone())).await.0, StatusCode::FORBIDDEN);
    let (s, v) = call(&env.app, "PUT", &path, ADMIN, Some(body)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        (v["auto_reply"].clone(), v["reply_instructions"].clone(), v["signature"].clone()),
        (json!(true), json!("Warm and short."), json!("The Acme Team"))
    );
    let (_, v) = call(&env.app, "GET", &path, OPERATOR, None).await;
    assert_eq!((v["auto_reply"].clone(), v["signature"].clone()), (json!(true), json!("The Acme Team")));

    let too_long = json!({"auto_reply": true, "reply_instructions": "x".repeat(2001), "signature": null});
    assert_eq!(call(&env.app, "PUT", &path, ADMIN, Some(too_long)).await.0, StatusCode::BAD_REQUEST);
    let long_sig = json!({"auto_reply": true, "reply_instructions": "", "signature": "x".repeat(201)});
    assert_eq!(call(&env.app, "PUT", &path, ADMIN, Some(long_sig)).await.0, StatusCode::BAD_REQUEST);

    // A blank signature clears it.
    let (_, v) = call(&env.app, "PUT", &path, ADMIN, Some(json!({"auto_reply": false, "reply_instructions": "", "signature": "  "}))).await;
    assert_eq!((v["auto_reply"].clone(), v["signature"].clone()), (json!(false), Value::Null));

    // Other workspaces' settings are not readable by an outsider.
    assert_eq!(call(&env.app, "GET", &path, "dev:stranger@x.test", None).await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn config_default_applies_until_the_workspace_decides() {
    let env = setup_with(|c| c.auto_drafts = true).await;
    let path = env.settings_path().await;
    let (_, v) = call(&env.app, "GET", &path, ADMIN, None).await;
    assert_eq!(v["auto_reply"], true);
}

#[tokio::test]
async fn workspace_auto_reply_overrides_server_default() {
    let env = setup_with(|c| c.auto_drafts = false).await;
    let first = env.email("Can you send the offer letter?", "Hi, can you send the offer letter today?").await;
    assert_eq!(env.draft_count().await, 0, "server default is off: no draft for {first}");

    let path = env.settings_path().await;
    let (s, _) = call(&env.app, "PUT", &path, ADMIN, Some(json!({"auto_reply": true, "reply_instructions": "", "signature": null}))).await;
    assert_eq!(s, StatusCode::OK);
    let second = env.email("Where is my payslip?", "Hi, where can I find my payslip for last month?").await;
    let (_, detail) = call(&env.app, "GET", &format!("{}/messages/{second}", env.base), ADMIN, None).await;
    assert!(detail["draft"]["id"].is_string(), "workspace auto_reply=true drafts automatically");

    // And the reverse: workspace off beats server on.
    let off = setup_with(|c| c.auto_drafts = true).await;
    call(&off.app, "PUT", &off.settings_path().await, ADMIN, Some(json!({"auto_reply": false, "reply_instructions": "", "signature": null}))).await;
    off.email("Can you send the offer letter?", "Hi, can you send the offer letter today?").await;
    assert_eq!(off.draft_count().await, 0);
}

#[tokio::test]
async fn reply_style_signature_and_label_meaning_reach_the_drafter() {
    let env = setup_with(|_| {}).await;

    // Without team instructions the default business context is used.
    env.email("Can you send the offer letter?", "Hi, can you send the offer letter today?").await;
    {
        let seen = env.recorder.seen.lock().unwrap();
        // The workspace agent's voice (seeded with the standard guidance) writes every draft.
        assert!(seen.last().unwrap().reply_instructions.contains(DEFAULT_REPLY_STYLE));
        assert_eq!(seen.last().unwrap().signature, None);
        assert!(!seen.last().unwrap().category_description.is_empty(), "label meaning is passed along");
    }

    let path = env.settings_path().await;
    let body = json!({"auto_reply": true, "reply_instructions": "Warm and short. Never discuss salary.", "signature": "The Acme Team"});
    assert_eq!(call(&env.app, "PUT", &path, ADMIN, Some(body)).await.0, StatusCode::OK);
    env.email("Where is my payslip?", "Hi, where can I find my payslip for last month?").await;
    let seen = env.recorder.seen.lock().unwrap();
    let last = seen.last().unwrap();
    assert!(last.reply_instructions.ends_with("Team reply instructions:\nWarm and short. Never discuss salary."), "{}", last.reply_instructions);
    assert_eq!(last.signature.as_deref(), Some("The Acme Team"));
}

#[tokio::test]
async fn better_draft_request_routes_to_premium() {
    let env = setup_with(|c| c.auto_drafts = false).await;
    let id = env.email("Where is my payslip?", "Hi, where can I find my payslip for last month?").await;
    let url = format!("{}/messages/{id}/draft", env.base);

    // No body still works and picks the automatic tier for a routine email.
    assert!(call(&env.app, "POST", &url, ADMIN, None).await.0.is_success());
    worker::drain(&env.state).await;
    assert_eq!(env.tiers(&id).await, ["standard"]);
    assert_eq!(env.recorder.seen.lock().unwrap().last().unwrap().quality, DraftQuality::Standard);

    let (s, _) = call(&env.app, "POST", &url, OPERATOR, Some(json!({"quality": "premium"}))).await;
    assert!(s.is_success());
    worker::drain(&env.state).await;
    assert_eq!(env.tiers(&id).await, ["standard", "premium"]);
    assert_eq!(env.recorder.seen.lock().unwrap().last().unwrap().quality, DraftQuality::Premium);

    assert_eq!(call(&env.app, "POST", &url, ADMIN, Some(json!({"quality": "gold"}))).await.0, StatusCode::BAD_REQUEST);
}
