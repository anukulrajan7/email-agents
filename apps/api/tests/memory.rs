//! Agent memory: learned contact profiles, user notes, FTS retrieval into drafts,
//! learning suggestions from approved edits (human-accepted only), and tenant isolation.

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
    ai::{AiError, LearnedLesson, Learner},
    build_state,
    config::Config,
    db,
    http::{AppState, router},
    jobs::worker,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

struct FakeLearner(Arc<AtomicUsize>);

#[async_trait]
impl Learner for FakeLearner {
    async fn learn(&self, _suggested: &str, _approved: &str) -> Result<Vec<LearnedLesson>, AiError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(vec![LearnedLesson { kind: "style".into(), scope: "workspace".into(), content: "Sign off as 'HR Team, Acme'.".into() }])
    }
}

struct H {
    state: AppState,
    app: Router,
    _dir: tempfile::TempDir,
}

const ADMIN: &str = "dev:admin@acme.test";

async fn harness(learner_calls: Arc<AtomicUsize>) -> H {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()));
    let mut state = build_state(config).await.unwrap();
    db::migrate(&state.db).await.unwrap();
    state.learner = Some(Arc::new(FakeLearner(learner_calls)));
    H { app: router(state.clone()), state, _dir: dir }
}

impl H {
    async fn call(&self, method: &str, path: &str, token: &str, body: Option<Value>) -> (StatusCode, Value) {
        let req = Request::builder().method(method).uri(path).header("x-auth-token", token);
        let req = match body {
            Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
            None => req.body(Body::empty()).unwrap(),
        };
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn workspace(&self, token: &str) -> (String, String) {
        let (_, ws) = self.call("POST", "/api/v1/workspaces", token, Some(json!({"name":"Acme"}))).await;
        let ws = ws["id"].as_str().unwrap().to_string();
        let (_, src) = self.call("POST", &format!("/api/v1/workspaces/{ws}/sources/forwarding"), token, None).await;
        (ws, src["id"].as_str().unwrap().to_string())
    }

    async fn email(&self, ws: &str, src: &str, body: Value) {
        let (s, _) = self.call("POST", &format!("/api/v1/workspaces/{ws}/sources/{src}/test-email"), ADMIN, Some(body)).await;
        assert_eq!(s, StatusCode::ACCEPTED);
        worker::drain(&self.state).await;
    }

    async fn latest_message(&self, ws: &str) -> Value {
        let (_, list) = self.call("GET", &format!("/api/v1/workspaces/{ws}/messages?limit=1"), ADMIN, None).await;
        let id = list["items"][0]["id"].as_str().unwrap();
        self.call("GET", &format!("/api/v1/workspaces/{ws}/messages/{id}"), ADMIN, None).await.1
    }
}

#[tokio::test]
async fn memory_learns_profiles_retrieves_notes_and_learns_from_edits() {
    let calls = Arc::new(AtomicUsize::new(0));
    let h = harness(calls.clone()).await;
    let (ws, src) = h.workspace(ADMIN).await;
    let base = format!("/api/v1/workspaces/{ws}");

    // 1. Contact profile is learned from the email itself.
    h.email(&ws, &src, json!({})).await; // default: "Priya Sharma <priya.sharma@example.com>", September timesheet
    let (_, contacts) = h.call("GET", &format!("{base}/contacts"), ADMIN, None).await;
    let priya = &contacts["items"][0];
    assert_eq!(priya["email"], "priya.sharma@example.com");
    assert_eq!(priya["display_name"], "Priya Sharma");
    assert_eq!(priya["message_count"], 1);
    assert_eq!(priya["top_categories"][0], json!({"category":"timesheet","count":1}));
    let detail = h.latest_message(&ws).await;
    assert_eq!(detail["contact"]["display_name"], "Priya Sharma");
    assert_eq!(detail["contact_memories"], json!([]));
    assert_eq!(detail["draft"]["memories_used"], json!([]));

    // 2. Users save memories; validation holds.
    let (s, team) = h
        .call("POST", &format!("{base}/memories"), ADMIN, Some(json!({"scope":"workspace","kind":"fact",
            "content":"Payroll runs on the 28th; never promise payment dates."})))
        .await;
    assert_eq!((s, team["status"].as_str(), team["source"].as_str()), (StatusCode::CREATED, Some("active"), Some("user")));
    let (s, about) = h
        .call("POST", &format!("{base}/memories"), ADMIN, Some(json!({"scope":"contact","contact_email":"Priya.Sharma@example.com",
            "kind":"preference","content":"Greet Priya by first name."})))
        .await;
    assert_eq!((s, about["contact_email"].as_str()), (StatusCode::CREATED, Some("priya.sharma@example.com")));
    assert_eq!(
        h.call("POST", &format!("{base}/memories"), ADMIN, Some(json!({"scope":"contact","kind":"fact","content":"x"}))).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        h.call("POST", &format!("{base}/memories"), ADMIN, Some(json!({"scope":"workspace","kind":"fact","content":"a".repeat(501)})))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(h.call("GET", &format!("{base}/memories"), "dev:out@sider.test", None).await.0, StatusCode::FORBIDDEN);

    // 3. Full-text search, with FTS operators in user input treated as plain words.
    let (_, found) = h.call("GET", &format!("{base}/memories?q=payroll"), ADMIN, None).await;
    assert_eq!(found["items"].as_array().unwrap().len(), 1);
    assert_eq!(h.call("GET", &format!("{base}/memories?q=NEAR(%22*"), ADMIN, None).await.0, StatusCode::OK);

    // 4. Approving an edited draft teaches a *suggestion* (never auto-active), deduped.
    let draft_id = detail["draft"]["id"].as_str().unwrap().to_string();
    let path = format!("{base}/drafts/{draft_id}");
    h.call("PATCH", &path, ADMIN, Some(json!({"body_text":"Hi Priya, thanks — received.\n\nHR Team, Acme","status":"approved"})))
        .await;
    worker::drain(&h.state).await;
    h.call("PATCH", &path, ADMIN, Some(json!({"body_text":"Hi Priya, thanks, received!\n\nHR Team, Acme","status":"approved"})))
        .await;
    worker::drain(&h.state).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let (_, suggested) = h.call("GET", &format!("{base}/memories?status=suggested"), ADMIN, None).await;
    let suggested = suggested["items"].as_array().unwrap().clone();
    assert_eq!(suggested.len(), 1, "same lesson twice is stored once");
    assert_eq!((suggested[0]["source"].as_str(), suggested[0]["evidence_message_id"].is_string()), (Some("learned"), true));
    let (_, active_before) = h.call("GET", &format!("{base}/memories"), ADMIN, None).await;
    assert_eq!(active_before["items"].as_array().unwrap().len(), 2, "suggestion not used until accepted");

    let sid = suggested[0]["id"].as_str().unwrap();
    let (s, accepted) = h.call("PATCH", &format!("{base}/memories/{sid}"), ADMIN, Some(json!({"status":"active"}))).await;
    assert_eq!((s, accepted["status"].as_str()), (StatusCode::OK, Some("active")));

    // 5. The next draft for Priya is given her note, the matching team fact and the accepted lesson.
    h.email(&ws, &src, json!({"from":"Priya Sharma <priya.sharma@example.com>","subject":"Payroll date question",
                               "body_text":"When does payroll run this month?"}))
        .await;
    let detail = h.latest_message(&ws).await;
    assert_eq!(detail["contact"]["message_count"], 2);
    assert_eq!(detail["contact_memories"].as_array().unwrap().len(), 1);
    let id = detail["id"].as_str().unwrap();
    assert_eq!(h.call("POST", &format!("{base}/messages/{id}/draft"), ADMIN, None).await.0, StatusCode::ACCEPTED);
    worker::drain(&h.state).await;
    let detail = h.latest_message(&ws).await;
    let used: Vec<String> =
        detail["draft"]["memories_used"].as_array().unwrap().iter().map(|m| m["content"].as_str().unwrap().to_string()).collect();
    assert!(used.contains(&"Greet Priya by first name.".to_string()), "{used:?}");
    assert!(used.iter().any(|u| u.starts_with("Payroll runs on the 28th")), "{used:?}");
    assert!(used.contains(&"Sign off as 'HR Team, Acme'.".to_string()), "{used:?}");
    assert_eq!(used[0], "Greet Priya by first name.", "contact notes come first");

    // 6. Archived memories are no longer used.
    let tid = team["id"].as_str().unwrap();
    h.call("PATCH", &format!("{base}/memories/{tid}"), ADMIN, Some(json!({"status":"archived"}))).await;
    h.call("POST", &format!("{base}/messages/{id}/draft"), ADMIN, None).await;
    worker::drain(&h.state).await;
    let detail = h.latest_message(&ws).await;
    let used = detail["draft"]["memories_used"].as_array().unwrap();
    assert!(used.iter().all(|m| !m["content"].as_str().unwrap().starts_with("Payroll")));
}

#[tokio::test]
async fn memories_are_tenant_isolated() {
    let h = harness(Arc::new(AtomicUsize::new(0))).await;
    let (ws1, _) = h.workspace(ADMIN).await;
    let (ws2, _) = h.workspace("dev:other@corp.test").await;
    let (_, mem) = h
        .call("POST", &format!("/api/v1/workspaces/{ws1}/memories"), ADMIN, Some(json!({"scope":"workspace","kind":"fact","content":"secret fact"})))
        .await;
    let mid = mem["id"].as_str().unwrap();
    let (s, _) = h
        .call("PATCH", &format!("/api/v1/workspaces/{ws2}/memories/{mid}"), "dev:other@corp.test", Some(json!({"status":"archived"})))
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (_, other) = h.call("GET", &format!("/api/v1/workspaces/{ws2}/memories?q=secret"), "dev:other@corp.test", None).await;
    assert_eq!(other["items"], json!([]));
}
