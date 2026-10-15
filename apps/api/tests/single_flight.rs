//! Single flight: one model reading per email. Only the analysis job calls the analyst; the workflow,
//! automations and memory learning are queued after it stored its result; concurrent callers share it.

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
    ai::{AiError, Analyst, Insight},
    build_state,
    platform::config::Config,
    platform::db,
    http::{AppState, router},
    jobs::{handlers, worker},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const ADMIN: &str = "dev:admin@acme.test";

#[derive(Default)]
struct CountingAnalyst(AtomicUsize);

#[async_trait]
impl Analyst for CountingAnalyst {
    async fn analyze(&self, subject: &str, _sender: &str, _body: &str) -> Result<(Insight, String), AiError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        Ok((
            Insight { summary: format!("About {subject}"), needs_reply: true, asks: vec!["Please review".into()], ..Default::default() },
            "test".into(),
        ))
    }
}

struct Env {
    state: AppState,
    app: Router,
    base: String,
    source: String,
    analyst: Arc<CountingAnalyst>,
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
    let analyst = Arc::new(CountingAnalyst::default());
    state.analyst = analyst.clone();
    let app = router(state.clone());
    let (_, ws) = call(&app, "POST", "/api/v1/workspaces", Some(json!({"name":"Acme","template":"hr"}))).await;
    let base = format!("/api/v1/workspaces/{}", ws["id"].as_str().unwrap());
    let (_, src) = call(&app, "POST", &format!("{base}/sources/forwarding"), None).await;
    let source = src["id"].as_str().unwrap().to_string();
    Env { state, app, base, source, analyst, _dir: dir }
}

impl Env {
    async fn email(&self, subject: &str, body: &str) -> String {
        call(&self.app, "POST", &format!("{}/sources/{}/test-email", self.base, self.source), Some(json!({"subject": subject, "body_text": body})))
            .await;
        worker::drain(&self.state).await;
        sqlx::query_scalar("SELECT id FROM email_messages WHERE subject = ?1").bind(subject).fetch_one(&self.state.db).await.unwrap()
    }

    async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(&self.state.db).await.unwrap()
    }
}

#[tokio::test]
async fn one_analysis_per_message_even_with_automations_and_a_workflow() {
    let env = setup().await;
    // HR workspaces ship the "reply to questions" automation; a timesheet also has a workflow.
    let id = env.email("September timesheet", "Hi, please find my timesheet for September. Can you confirm receipt?").await;

    assert_eq!(env.analyst.0.load(Ordering::SeqCst), 1, "classify, workflow draft, automations and learning all read the stored insight");
    assert_eq!(env.count(&format!("SELECT COUNT(*) FROM message_insights WHERE message_id = '{id}'")).await, 1);
    let automations = env.count("SELECT COUNT(*) FROM automation_runs").await;
    let drafts = env.count("SELECT COUNT(*) FROM drafts").await;
    assert!(automations + drafts >= 1, "downstream stages still ran after the analysis (automation runs {automations}, drafts {drafts})");
}

#[tokio::test]
async fn downstream_jobs_are_queued_only_by_the_analysis_job() {
    let env = setup().await;
    call(&env.app, "POST", &format!("{}/sources/{}/test-email", env.base, env.source), Some(json!({"subject": "Leave", "body_text": "Can I take leave next week?"})))
        .await;
    // Run exactly the fetch and the classification: the analysis is queued, nothing else reads the insight yet.
    for _ in 0..2 {
        let job = email_ai_api::jobs::claim(&env.state.db, "t").await.unwrap().unwrap();
        worker::process(&env.state, "t", job).await;
    }
    let kinds: Vec<String> =
        sqlx::query_scalar("SELECT kind FROM jobs WHERE status = 'pending' ORDER BY kind").fetch_all(&env.state.db).await.unwrap();
    assert!(kinds.contains(&"analyze_message".to_string()), "{kinds:?}");
    for later in ["run_automations", "execute_workflow", "learn_from_message"] {
        assert!(!kinds.contains(&later.to_string()), "{later} must wait for the analysis: {kinds:?}");
    }
    worker::drain(&env.state).await;
    assert_eq!(env.analyst.0.load(Ordering::SeqCst), 1);
    let ran: i64 = env.count("SELECT COUNT(*) FROM jobs WHERE kind IN ('run_automations','learn_from_message') AND status = 'succeeded'").await;
    assert_eq!(ran, 2);
}

#[tokio::test]
async fn concurrent_callers_share_one_analysis() {
    let env = setup().await;
    let id = env.email("Payslip", "Where is my payslip?").await;
    sqlx::query("DELETE FROM message_insights WHERE message_id = ?1").bind(&id).execute(&env.state.db).await.unwrap();
    let before = env.analyst.0.load(Ordering::SeqCst);

    let payload = json!({"message_id": id});
    let results = futures_join(&env.state, &payload).await;
    assert!(results.iter().all(|r| r.summary == "About Payslip."));
    assert_eq!(env.analyst.0.load(Ordering::SeqCst) - before, 1, "draft, automations, agent tools and the job raced: one reading");
}

async fn futures_join(state: &AppState, payload: &Value) -> Vec<Insight> {
    let mut tasks = Vec::new();
    for _ in 0..6 {
        let (state, payload) = (state.clone(), payload.clone());
        tasks.push(tokio::spawn(async move { handlers::analyze_message(&state, &payload).await.unwrap() }));
    }
    let mut out = Vec::new();
    for t in tasks {
        out.push(t.await.unwrap());
    }
    out
}

#[tokio::test]
async fn relabelling_an_analysed_email_still_runs_the_workflow_without_a_second_reading() {
    let env = setup().await;
    let id = env.email("Leave request", "Can I take leave next week?").await;
    let runs_before = env.count("SELECT COUNT(*) FROM jobs WHERE kind = 'execute_workflow'").await;
    let (s, _) = call(&env.app, "PATCH", &format!("{}/messages/{id}/classification", env.base), Some(json!({"category": "leave"}))).await;
    assert!(s.is_success(), "{s}");
    worker::drain(&env.state).await;
    assert_eq!(env.analyst.0.load(Ordering::SeqCst), 1);
    assert!(env.count("SELECT COUNT(*) FROM jobs WHERE kind = 'execute_workflow'").await > runs_before);
}
