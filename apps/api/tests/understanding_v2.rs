//! Email understanding v2: one call replaces classify + analyze + judges (UNDERSTAND_V2=on), runs beside
//! them and records label agreement (shadow), or does not run at all (off).

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use email_ai_api::{
    ai::{
        AiError, Analyst, Classifier, ClassifyInput, ClassifyOutput, Insight,
        judge::FixedJudge,
        understand::{UnderstandInput, Understander, Understanding, parse_understanding, shadow_report},
    },
    build_state,
    config::{Config, UnderstandMode},
    db,
    domain::{ClassificationSource, Priority},
    http::{AppState, router},
    jobs::worker,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const ADMIN: &str = "dev:admin@acme.test";

#[derive(Default)]
struct Calls {
    classify: AtomicUsize,
    analyze: AtomicUsize,
    understand: AtomicUsize,
}

struct V1Classifier(Arc<Calls>, &'static str);

#[async_trait]
impl Classifier for V1Classifier {
    async fn classify(&self, _input: &ClassifyInput) -> Result<ClassifyOutput, AiError> {
        self.0.classify.fetch_add(1, Ordering::SeqCst);
        Ok(ClassifyOutput {
            category: self.1.into(),
            priority: Priority::Normal,
            requires_reply: true,
            confidence: Some(0.9),
            source: ClassificationSource::Llm,
            model_id: Some("v1-model".into()),
            model_version: None,
        })
    }
}

struct V1Analyst(Arc<Calls>);

#[async_trait]
impl Analyst for V1Analyst {
    async fn analyze(&self, subject: &str, _sender: &str, _body: &str) -> Result<(Insight, String), AiError> {
        self.0.analyze.fetch_add(1, Ordering::SeqCst);
        Ok((Insight { summary: format!("v1 about {subject}"), needs_reply: true, asks: vec!["v1 ask".into()], ..Default::default() }, "v1-model".into()))
    }
}

/// Answers with a fixed model reply that goes through the real verification, or fails.
struct V2 {
    calls: Arc<Calls>,
    reply: Value,
    fail: bool,
    available: bool,
}

#[async_trait]
impl Understander for V2 {
    fn available(&self) -> bool {
        self.available
    }
    async fn understand(&self, input: &UnderstandInput) -> Result<(Understanding, String), AiError> {
        self.calls.understand.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            return Err(AiError::Retryable("model returned invalid JSON".into()));
        }
        Ok((parse_understanding(&self.reply.to_string(), input)?, "v2-model".into()))
    }
}

fn reply(label: &str) -> Value {
    json!({
        "label": label, "label_confidence": 0.94, "priority": "normal", "needs_reply": true, "intent": "request",
        "sentiment": "calm", "summary": "Priya asks to approve leave.",
        "asks": [
            {"text": "Approve leave 20-24 Oct", "kind": "request", "quote": "Can I take leave from 20 to 24 October?"},
            {"text": "Invented ask", "kind": "request", "quote": "please wire the money today"}
        ],
        "commitments": [], "deadlines": [{"what": "leave", "when_text": "20 to 24 October", "when_iso": null}],
        "entities": [], "missing_info": ["manager name"],
        "facts_to_remember": [{"category": "contact", "about_counterpart": true, "subject": "priya@example.com", "key": "role",
                               "content": "Priya is the HR manager at Acme.", "confidence": 0.9}],
        "alert_signals": [], "injection_signals": [], "language": "en"
    })
}

struct Env {
    state: AppState,
    app: Router,
    base: String,
    source: String,
    calls: Arc<Calls>,
    judge: Arc<FixedJudge>,
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

async fn setup(mode: UnderstandMode, v2_label: &str, fail: bool, available: bool) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()));
    config.understand_mode = mode;
    let mut state = build_state(config).await.unwrap();
    db::migrate(&state.db).await.unwrap();
    let calls = Arc::new(Calls::default());
    let judge = Arc::new(FixedJudge::new());
    state.classifier = Arc::new(V1Classifier(calls.clone(), "leave"));
    state.analyst = Arc::new(V1Analyst(calls.clone()));
    state.judge = judge.clone();
    state.understander = Arc::new(V2 { calls: calls.clone(), reply: reply(v2_label), fail, available });
    let app = router(state.clone());
    let (_, ws) = call(&app, "POST", "/api/v1/workspaces", Some(json!({"name":"Acme","template":"hr"}))).await;
    let base = format!("/api/v1/workspaces/{}", ws["id"].as_str().unwrap());
    let (_, src) = call(&app, "POST", &format!("{base}/sources/forwarding"), None).await;
    let source = src["id"].as_str().unwrap().to_string();
    Env { state, app, base, source, calls, judge, _dir: dir }
}

impl Env {
    async fn email(&self, subject: &str) -> String {
        let body = "Hi team,\nCan I take leave from 20 to 24 October?\nThanks,\nPriya";
        call(&self.app, "POST", &format!("{}/sources/{}/test-email", self.base, self.source), Some(json!({"subject": subject, "body_text": body})))
            .await;
        worker::drain(&self.state).await;
        sqlx::query_scalar("SELECT id FROM email_messages WHERE subject = ?1").bind(subject).fetch_one(&self.state.db).await.unwrap()
    }

    fn n(&self) -> (usize, usize, usize) {
        (self.calls.classify.load(Ordering::SeqCst), self.calls.analyze.load(Ordering::SeqCst), self.calls.understand.load(Ordering::SeqCst))
    }
}

#[tokio::test]
async fn on_one_call_labels_reads_and_remembers_without_the_other_calls() {
    let env = setup(UnderstandMode::On, "leave", false, true).await;
    let id = env.email("Leave request").await;

    assert_eq!(env.n(), (0, 0, 1), "no classify, no analyze: one understanding call");
    assert_eq!(env.judge.calls(), 0, "no needs-reply judge");

    let (category, source, model, confidence, prompt): (String, String, String, f64, Option<String>) = sqlx::query_as(
        "SELECT category, source, model_id, confidence, prompt_version FROM classifications WHERE message_id = ?1",
    )
    .bind(&id)
    .fetch_one(&env.state.db)
    .await
    .unwrap();
    assert_eq!((category.as_str(), source.as_str(), model.as_str(), confidence), ("leave", "llm", "v2-model", 0.94));
    assert!(prompt.is_some());

    let (summary, asks, needs_reply, json_blob, version): (String, String, bool, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT summary, asks_json, needs_reply, understanding_json, understanding_version FROM message_insights WHERE message_id = ?1",
    )
    .bind(&id)
    .fetch_one(&env.state.db)
    .await
    .unwrap();
    assert_eq!((summary.as_str(), needs_reply), ("Priya asks to approve leave.", true));
    assert_eq!(asks, r#"["Approve leave 20-24 Oct"]"#, "the invented ask was dropped by the quote check");
    assert_eq!(version.as_deref(), Some("understand-v1"));
    assert!(json_blob.unwrap().contains("\"dropped_unverified\":1"));

    // The pipeline after the reading is unchanged: automations and memory learning ran from the stored result.
    let succeeded: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM jobs WHERE kind IN ('analyze_message','run_automations','learn_from_message','execute_workflow') AND status = 'succeeded'",
    )
    .fetch_one(&env.state.db)
    .await
    .unwrap();
    assert!(succeeded >= 3, "{succeeded}");
    let memories: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM memories WHERE content LIKE '%HR manager at Acme%'").fetch_one(&env.state.db).await.unwrap();
    assert_eq!(memories, 1, "incoming facts come from the same call");
}

#[tokio::test]
async fn on_falls_back_to_the_separate_calls_when_v2_fails() {
    let env = setup(UnderstandMode::On, "leave", true, true).await;
    let id = env.email("Leave request").await;
    assert_eq!(env.n(), (1, 1, 2), "v2 was tried for the label and for the reading, then classify and analyze ran");
    let model: String = sqlx::query_scalar("SELECT model_id FROM classifications WHERE message_id = ?1").bind(&id).fetch_one(&env.state.db).await.unwrap();
    assert_eq!(model, "v1-model");
    let summary: String = sqlx::query_scalar("SELECT summary FROM message_insights WHERE message_id = ?1").bind(&id).fetch_one(&env.state.db).await.unwrap();
    assert_eq!(summary, "v1 about Leave request");
}

#[tokio::test]
async fn on_without_a_real_understander_keeps_todays_calls() {
    let env = setup(UnderstandMode::On, "leave", false, false).await;
    env.email("Leave request").await;
    assert_eq!(env.n(), (1, 1, 0));
}

#[tokio::test]
async fn off_never_calls_v2() {
    let env = setup(UnderstandMode::Off, "leave", false, true).await;
    env.email("Leave request").await;
    assert_eq!(env.n(), (1, 1, 0));
}

#[tokio::test]
async fn shadow_keeps_production_and_records_label_agreement() {
    let env = setup(UnderstandMode::Shadow, "timesheet", false, true).await;
    let id = env.email("Leave request").await;
    assert_eq!(env.n(), (1, 1, 1), "production calls plus one shadow call");

    let model: String = sqlx::query_scalar("SELECT model_id FROM classifications WHERE message_id = ?1").bind(&id).fetch_one(&env.state.db).await.unwrap();
    assert_eq!(model, "v1-model", "v1 still decides in shadow mode");
    let (v1, v2, agree): (String, Option<String>, bool) =
        sqlx::query_as("SELECT v1_label, v2_label, label_agree FROM understanding_shadow WHERE message_id = ?1").bind(&id).fetch_one(&env.state.db).await.unwrap();
    assert_eq!((v1.as_str(), v2.as_deref(), agree), ("leave", Some("timesheet"), false));

    let ws: String = sqlx::query_scalar("SELECT workspace_id FROM email_messages WHERE id = ?1").bind(&id).fetch_one(&env.state.db).await.unwrap();
    let report = shadow_report(&env.state.db, &ws).await.unwrap();
    assert_eq!((report.compared, report.failed, report.label_agreement, report.ready_to_switch), (1, 0, 0.0, false));

    let (s, v) = call(&env.app, "GET", &format!("{}/understanding-shadow", env.base), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!((v["compared"].as_i64(), v["ready_to_switch"].as_bool()), (Some(1), Some(false)));
}

#[tokio::test]
async fn shadow_failures_count_against_readiness_and_never_break_the_email() {
    let env = setup(UnderstandMode::Shadow, "leave", true, true).await;
    let id = env.email("Leave request").await;
    let status: String = sqlx::query_scalar("SELECT processing_status FROM email_messages WHERE id = ?1").bind(&id).fetch_one(&env.state.db).await.unwrap();
    assert_ne!(status, "failed");
    let ws: String = sqlx::query_scalar("SELECT workspace_id FROM email_messages WHERE id = ?1").bind(&id).fetch_one(&env.state.db).await.unwrap();
    let report = shadow_report(&env.state.db, &ws).await.unwrap();
    assert_eq!((report.compared, report.failed, report.label_agreement), (0, 1, 0.0));
}
