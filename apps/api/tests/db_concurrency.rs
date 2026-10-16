//! Concurrency stress: API writes, draft requests and several job workers share one SQLite file.
//! Nothing may surface "database is locked" (an HTTP 500 or a job error).

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
    http::router,
    jobs::worker,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const ADMIN: &str = "dev:admin@acme.test";

struct SlowAnalyst;

struct CountingSlowAnalyst(Arc<AtomicUsize>);

#[async_trait]
impl Analyst for CountingSlowAnalyst {
    async fn analyze(&self, subject: &str, _sender: &str, _body: &str) -> Result<(Insight, String), AiError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        Ok((Insight { summary: format!("About {subject}"), ..Default::default() }, "test".into()))
    }
}

#[async_trait]
impl Analyst for SlowAnalyst {
    async fn analyze(&self, subject: &str, _sender: &str, _body: &str) -> Result<(Insight, String), AiError> {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        Ok((Insight { summary: format!("About {subject}"), needs_reply: true, asks: vec!["Please review".into()], ..Default::default() }, "test".into()))
    }
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

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_writers_never_see_database_is_locked() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()));
    let mut state = build_state(config).await.unwrap();
    db::migrate(&state.db).await.unwrap();
    state.analyst = Arc::new(SlowAnalyst);
    let app = router(state.clone());
    let (_, ws) = call(&app, "POST", "/api/v1/workspaces", Some(json!({"name":"Acme","template":"hr"}))).await;
    let base = format!("/api/v1/workspaces/{}", ws["id"].as_str().unwrap());
    let (_, src) = call(&app, "POST", &format!("{base}/sources/forwarding"), None).await;
    let source = src["id"].as_str().unwrap().to_string();

    let mut tasks = Vec::new();
    for w in 0..3 {
        let state = state.clone();
        tasks.push(tokio::spawn(async move {
            for _ in 0..40 {
                worker::drain(&state).await;
                tokio::time::sleep(std::time::Duration::from_millis(2 + w)).await;
            }
            Vec::new()
        }));
    }
    for i in 0..12 {
        let (app, base, source) = (app.clone(), base.clone(), source.clone());
        tasks.push(tokio::spawn(async move {
            let mut failures = Vec::new();
            let subject = format!("Timesheet {i}");
            let (s, b) = call(&app, "POST", &format!("{base}/sources/{source}/test-email"), Some(json!({"subject": subject, "body_text": "Please confirm my timesheet. Can you reply?"}))).await;
            if s.is_server_error() {
                failures.push(format!("test-email {s} {b}"));
            }
            for round in 0..6 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                let (_, list) = call(&app, "GET", &format!("{base}/messages"), None).await;
                let id = list["items"].as_array().or(list.as_array()).and_then(|a| a.iter().find(|m| m["subject"] == subject.as_str())).and_then(|m| m["id"].as_str().map(str::to_string));
                let Some(id) = id else { continue };
                let calls = [
                    ("POST", format!("{base}/messages/{id}/draft"), None),
                    ("PATCH", format!("{base}/messages/{id}/classification"), Some(json!({"category": "leave"}))),
                    ("PUT", format!("{base}/reply-settings"), Some(json!({"auto_reply": false, "reply_instructions": format!("r{round}"), "signature": null}))),
                    ("POST", format!("{base}/messages/{id}/understand"), None),
                ];
                for (m, p, body) in calls {
                    let (s, b) = call(&app, m, &p, body).await;
                    if s.is_server_error() {
                        failures.push(format!("{m} {p} {s} {b}"));
                    }
                }
            }
            failures
        }));
    }
    let mut failures = Vec::new();
    for t in tasks {
        failures.extend(t.await.unwrap());
    }
    worker::drain(&state).await;
    let job_errors: Vec<String> =
        sqlx::query_scalar("SELECT kind || ': ' || last_error FROM jobs WHERE last_error LIKE '%locked%' OR last_error LIKE '%busy%'")
            .fetch_all(&state.db)
            .await
            .unwrap();
    assert!(failures.is_empty(), "server errors: {failures:#?}");
    assert!(job_errors.is_empty(), "jobs hit the lock: {job_errors:#?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_1s_lease_with_a_3s_handler_runs_once() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()));
    config.job_lease_ms = 1_000;
    let mut state = build_state(config).await.unwrap();
    db::migrate(&state.db).await.unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    state.analyst = Arc::new(CountingSlowAnalyst(calls.clone()));
    let app = router(state.clone());
    let (_, ws) = call(&app, "POST", "/api/v1/workspaces", Some(json!({"name":"Acme","template":"hr"}))).await;
    let base = format!("/api/v1/workspaces/{}", ws["id"].as_str().unwrap());
    let (_, src) = call(&app, "POST", &format!("{base}/sources/forwarding"), None).await;
    let source = src["id"].as_str().unwrap();
    let (s, _) = call(&app, "POST", &format!("{base}/sources/{source}/test-email"), Some(json!({"subject": "Slow one", "body_text": "Please confirm my timesheet."}))).await;
    assert!(s.is_success());

    let first = {
        let state = state.clone();
        tokio::spawn(async move { worker::drain(&state).await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(1_500)).await;
    worker::drain(&state).await;
    first.await.unwrap();

    assert_eq!(calls.load(Ordering::SeqCst), 1, "the 3 s handler ran once, not once per lease");
    let most_attempts: i64 = sqlx::query_scalar("SELECT MAX(attempts) FROM jobs").fetch_one(&state.db).await.unwrap();
    assert_eq!(most_attempts, 1, "no job was claimed a second time");
    let running: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE status = 'running'").fetch_one(&state.db).await.unwrap();
    assert_eq!(running, 0);
}
