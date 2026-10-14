//! Models read what the sender wrote this time: quoted history, signatures and disclaimers stay out of
//! classify, analyze, draft and memory prompts; earlier messages arrive as a short thread digest.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use email_ai_api::{
    ai::{
        AiError, Analyst, Classifier, ClassifyInput, ClassifyOutput, Insight,
    },
    build_state,
    config::Config,
    db::{self, now_ms},
    domain::ClassificationSource,
    http::{AppState, router},
    jobs::{handlers, worker},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const ADMIN: &str = "dev:admin@acme.test";

#[derive(Default)]
struct Seen {
    analyzed: Mutex<Vec<(String, Option<String>)>>,
    classified: Mutex<Vec<String>>,
}

struct RecordingAnalyst(Arc<Seen>);

#[async_trait]
impl Analyst for RecordingAnalyst {
    async fn analyze(&self, subject: &str, sender: &str, body: &str) -> Result<(Insight, String), AiError> {
        self.analyze_in_thread(subject, sender, body, None).await
    }
    async fn analyze_in_thread(&self, subject: &str, _s: &str, body: &str, digest: Option<&str>) -> Result<(Insight, String), AiError> {
        self.0.analyzed.lock().unwrap().push((body.to_string(), digest.map(str::to_string)));
        Ok((Insight { summary: format!("About {subject}"), needs_reply: true, asks: vec!["ask".into()], ..Default::default() }, "test".into()))
    }
}

struct RecordingClassifier(Arc<Seen>);

#[async_trait]
impl Classifier for RecordingClassifier {
    async fn classify(&self, input: &ClassifyInput) -> Result<ClassifyOutput, AiError> {
        self.0.classified.lock().unwrap().push(input.body_text.clone());
        Ok(ClassifyOutput {
            category: "other".into(),
            priority: email_ai_api::domain::Priority::Normal,
            requires_reply: true,
            confidence: Some(0.95),
            source: ClassificationSource::Llm,
            model_id: Some("test".into()),
            model_version: None,
        })
    }
}

struct Env {
    state: AppState,
    app: Router,
    ws: String,
    base: String,
    source: String,
    seen: Arc<Seen>,
    _dir: tempfile::TempDir,
}

async fn call(app: &Router, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let req = Request::builder().method(method).uri(path).header("x-auth-token", ADMIN);
    let req = match body {
        Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn setup() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()));
    let mut state = build_state(config).await.unwrap();
    db::migrate(&state.db).await.unwrap();
    let seen = Arc::new(Seen::default());
    state.analyst = Arc::new(RecordingAnalyst(seen.clone()));
    state.classifier = Arc::new(RecordingClassifier(seen.clone()));
    let app = router(state.clone());
    let (_, ws) = call(&app, "POST", "/api/v1/workspaces", Some(json!({"name":"Acme"}))).await;
    let ws = ws["id"].as_str().unwrap().to_string();
    let base = format!("/api/v1/workspaces/{ws}");
    let (_, src) = call(&app, "POST", &format!("{base}/sources/forwarding"), None).await;
    let source = src["id"].as_str().unwrap().to_string();
    Env { state, app, ws, base, source, seen, _dir: dir }
}

impl Env {
    async fn email(&self, subject: &str, body: &str) -> String {
        call(&self.app, "POST", &format!("{}/sources/{}/test-email", self.base, self.source), Some(json!({"subject": subject, "body_text": body})))
            .await;
        worker::drain(&self.state).await;
        sqlx::query_scalar("SELECT id FROM email_messages WHERE subject = ?1 AND workspace_id = ?2")
            .bind(subject)
            .bind(&self.ws)
            .fetch_one(&self.state.db)
            .await
            .unwrap()
    }
}

const REPLY: &str = "Yes, Friday works. Please send the signed form.\n\nBest regards,\nPriya Sharma\nHR Manager, Acme\n+91 98765 43210\n\nOn Tue, Oct 13, 2026 at 11:30 AM Support <support@acme.example.com>\nwrote:\n\n> Can we meet Friday?\n> Also we need the form.\n";

#[tokio::test]
async fn classify_and_analyze_read_only_the_new_part() {
    let env = setup().await;
    env.email("Re: Meeting", REPLY).await;
    let classified = env.seen.classified.lock().unwrap().clone();
    let analyzed = env.seen.analyzed.lock().unwrap().clone();
    assert_eq!(classified, ["Yes, Friday works. Please send the signed form."]);
    assert_eq!(analyzed.len(), 1);
    assert_eq!(analyzed[0].0, "Yes, Friday works. Please send the signed form.");
    assert_eq!(analyzed[0].1, None, "the first message of a conversation has no digest");
}

#[tokio::test]
async fn later_messages_get_a_thread_digest_of_the_earlier_ones() {
    let env = setup().await;
    let first = env.email("Payroll", "Where is my payslip for September?\n\nThanks,\nPriya").await;
    let second = env.email("Re: Payroll", "Thanks, found it.\n\n> Where is my payslip?").await;
    sqlx::query("UPDATE email_messages SET thread_id = (SELECT thread_id FROM email_messages WHERE id = ?1), received_at = ?3 WHERE id = ?2")
        .bind(&first)
        .bind(&second)
        .bind(now_ms() + 1_000)
        .execute(&env.state.db)
        .await
        .unwrap();
    sqlx::query("DELETE FROM message_insights WHERE message_id = ?1").bind(&second).execute(&env.state.db).await.unwrap();

    handlers::analyze_message(&env.state, &json!({"message_id": second})).await.unwrap();
    let analyzed = env.seen.analyzed.lock().unwrap().clone();
    let (body, digest) = analyzed.last().unwrap().clone();
    assert_eq!(body, "Thanks, found it.");
    let digest = digest.expect("earlier message summarised");
    assert!(digest.contains("Where is my payslip for September?") && !digest.contains("Thanks,\nPriya"), "{digest}");
}
