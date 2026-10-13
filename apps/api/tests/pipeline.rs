//! Integration tests for the acceptance list in the spec: auth/tenancy, verified webhooks,
//! idempotent ingestion, job claiming/retries/recovery, and the timesheet vertical slice.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    extract::Path,
    http::{Request, StatusCode},
    routing::get,
};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use email_ai_api::{
    ai::{AiError, Classifier, ClassifyInput, ClassifyOutput},
    build_state,
    config::Config,
    db,
    http::{AppState, router},
    jobs::{self, worker},
    sources::resend::sign,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

struct Harness {
    state: AppState,
    app: Router,
    _dir: tempfile::TempDir,
}

async fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", dir.path().join("test.db").display());
    let mut config = Config::for_tests(&url);
    config.resend_api_key = "re_test".into();
    config.resend_api_url = spawn_mock_resend().await;
    let state = build_state(config).await.unwrap();
    db::migrate(&state.db).await.unwrap();
    Harness { app: router(state.clone()), state, _dir: dir }
}

/// Mock of `GET /emails/receiving/{id}`. Every id resolves to a timesheet email addressed
/// to the alias encoded in the id: `<anything>__<alias-local-part>`.
async fn spawn_mock_resend() -> String {
    let app = Router::new().route(
        "/emails/receiving/{id}",
        get(|Path(id): Path<String>| async move {
            let (_, token) = id.split_once("__").unwrap_or(("", ""));
            axum::Json(json!({
                "id": id,
                "from": "Team Lead <lead@acme.com>",
                "to": [format!("inbox+{token}@inbound.test")],
                "subject": "Fwd: September timesheet",
                "text": "FYI\n\n---------- Forwarded message ---------\nFrom: Priya <priya@acme.com>\nSubject: September timesheet\n\nPlease review my timesheet.",
                "html": "<p>ignored when text exists</p>",
                "message_id": "<abc@acme.com>",
                "created_at": "2026-10-13T11:30:00.000Z",
                "attachments": [{"id":"a1","filename":"timesheet.pdf","content_type":"application/pdf","size":1234}]
            }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

impl Harness {
    async fn call(&self, method: &str, path: &str, token: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(path);
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        let req = match body {
            Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
            None => req.body(Body::empty()).unwrap(),
        };
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn webhook(&self, delivery_id: &str, body: &Value, tamper: bool) -> (StatusCode, Value) {
        let raw = body.to_string();
        let ts = (db::now_ms() / 1000).to_string();
        let key = B64.decode(self.state.config.resend_webhook_secret.trim_start_matches("whsec_")).unwrap();
        let sig = B64.encode(sign(&key, delivery_id, &ts, raw.as_bytes()));
        let sent = if tamper { raw.replace("email.received", "email.receivee") } else { raw };
        let req = Request::post("/webhooks/resend")
            .header("svix-id", delivery_id)
            .header("svix-timestamp", ts)
            .header("svix-signature", format!("v1,{sig}"))
            .body(Body::from(sent))
            .unwrap();
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    /// Creates a workspace + forwarding source; returns (workspace_id, alias token).
    async fn workspace_with_alias(&self, admin: &str) -> (String, String) {
        let (s, ws) = self.call("POST", "/api/v1/workspaces", Some(admin), Some(json!({"name":"Acme HR"}))).await;
        assert_eq!(s, StatusCode::CREATED);
        let ws = ws["id"].as_str().unwrap().to_string();
        let (s, src) = self.call("POST", &format!("/api/v1/workspaces/{ws}/sources/forwarding"), Some(admin), None).await;
        assert_eq!(s, StatusCode::OK);
        let addr = src["inbound_address"].as_str().unwrap();
        let token = addr.trim_start_matches("inbox+").split('@').next().unwrap().to_string();
        (ws, token)
    }

    async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(&self.state.db).await.unwrap()
    }
}

fn received(email_id: &str) -> Value {
    json!({"type":"email.received","created_at":"2026-10-13T11:30:00Z","data":{"email_id": email_id}})
}

const ADMIN: &str = "dev:admin@acme.test";
const OUTSIDER: &str = "dev:outsider@other.test";

#[tokio::test]
async fn migrations_are_idempotent_and_health_works() {
    let h = harness().await;
    db::migrate(&h.state.db).await.unwrap();
    assert_eq!(h.call("GET", "/health/live", None, None).await.0, StatusCode::OK);
    assert_eq!(h.call("GET", "/health/ready", None, None).await.0, StatusCode::OK);
    let mode: String = sqlx::query_scalar("PRAGMA journal_mode").fetch_one(&h.state.db).await.unwrap();
    assert_eq!(mode, "wal");
    let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys").fetch_one(&h.state.db).await.unwrap();
    assert_eq!(fk, 1);
}

#[tokio::test]
async fn tenancy_is_enforced_server_side() {
    let h = harness().await;
    assert_eq!(h.call("GET", "/api/v1/me", None, None).await.0, StatusCode::UNAUTHORIZED);
    let (ws, _) = h.workspace_with_alias(ADMIN).await;

    let (s, body) = h.call("GET", &format!("/api/v1/workspaces/{ws}/messages"), Some(OUTSIDER), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(body["error"]["code"], "forbidden");
    assert!(!body["error"]["request_id"].as_str().unwrap().is_empty());

    // Operators can read but not administer.
    h.call("GET", "/api/v1/me", Some("dev:op@acme.test"), None).await;
    let (s, _) = h
        .call("POST", &format!("/api/v1/workspaces/{ws}/members"), Some(ADMIN), Some(json!({"email":"op@acme.test","role":"operator"})))
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(h.call("GET", &format!("/api/v1/workspaces/{ws}/messages"), Some("dev:op@acme.test"), None).await.0, StatusCode::OK);
    assert_eq!(
        h.call("POST", &format!("/api/v1/workspaces/{ws}/sources/forwarding"), Some("dev:op@acme.test"), None).await.0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn webhook_rejects_unsigned_and_tampered_requests() {
    let h = harness().await;
    let req = Request::post("/webhooks/resend").body(Body::from(received("x").to_string())).unwrap();
    assert_eq!(h.app.clone().oneshot(req).await.unwrap().status(), StatusCode::UNAUTHORIZED);
    assert_eq!(h.webhook("msg_t", &received("x"), true).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(h.count("SELECT COUNT(*) FROM inbound_events").await, 0);
}

#[tokio::test]
async fn timesheet_vertical_slice_end_to_end() {
    let h = harness().await;
    let (ws, token) = h.workspace_with_alias(ADMIN).await;
    let email_id = format!("em1__{token}");

    // Same delivery retried + the same email delivered again under a new delivery ID.
    assert_eq!(h.webhook("msg_1", &received(&email_id), false).await.1["status"], "accepted");
    assert_eq!(h.webhook("msg_1", &received(&email_id), false).await.1["status"], "duplicate");
    assert_eq!(h.webhook("msg_2", &received(&email_id), false).await.1["status"], "accepted");
    worker::drain(&h.state).await;

    assert_eq!(h.count("SELECT COUNT(*) FROM email_messages").await, 1, "forwarded twice appears once");
    assert_eq!(h.count("SELECT COUNT(*) FROM review_items").await, 1);
    assert_eq!(h.count("SELECT COUNT(*) FROM drafts").await, 1);
    assert_eq!(h.count("SELECT COUNT(*) FROM jobs WHERE status != 'succeeded'").await, 0);

    let (s, list) = h.call("GET", &format!("/api/v1/workspaces/{ws}/messages"), Some(ADMIN), None).await;
    assert_eq!(s, StatusCode::OK);
    let item = &list["items"][0];
    assert_eq!(item["category"], "timesheet");
    assert_eq!(item["processing_status"], "workflow_completed");
    assert_eq!(item["attachment_count"], 1);
    assert_eq!(item["original_from"], "priya@acme.com");
    let id = item["id"].as_str().unwrap();

    let (_, detail) = h.call("GET", &format!("/api/v1/workspaces/{ws}/messages/{id}"), Some(ADMIN), None).await;
    assert_eq!(detail["from"], "lead@acme.com");
    assert_eq!(detail["source"]["can_create_provider_draft"], false);
    assert_eq!(detail["classification"]["source"], "fallback");
    assert_eq!(detail["classification"]["needs_review"], true);
    assert_eq!(detail["review"]["status"], "open");
    assert_eq!(detail["workflow_run"]["status"], "completed");
    assert_eq!(detail["draft"]["status"], "suggested");
    let actions: Vec<&str> = detail["audit"].as_array().unwrap().iter().map(|a| a["action"].as_str().unwrap()).collect();
    assert_eq!(
        actions,
        ["message.received", "message.classified", "workflow.started", "review.created", "draft.generated", "workflow.completed"]
    );

    // Operator edits + approves the draft; "sent" cannot be set by clients.
    let draft_id = detail["draft"]["id"].as_str().unwrap();
    let path = format!("/api/v1/workspaces/{ws}/drafts/{draft_id}");
    let (s, d) = h.call("PATCH", &path, Some(ADMIN), Some(json!({"body_text":"Thanks, received!"}))).await;
    assert_eq!((s, d["status"].as_str()), (StatusCode::OK, Some("edited")));
    assert_eq!(h.call("PATCH", &path, Some(ADMIN), Some(json!({"status":"sent"}))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(h.call("PATCH", &path, Some(OUTSIDER), Some(json!({"status":"approved"}))).await.0, StatusCode::FORBIDDEN);

    // Reprocessing re-runs the pipeline without duplicating the run, review or draft.
    let (s, _) = h.call("POST", &format!("/api/v1/workspaces/{ws}/messages/{id}/reprocess"), Some(ADMIN), None).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    worker::drain(&h.state).await;
    assert_eq!(h.count("SELECT COUNT(*) FROM workflow_runs").await, 1);
    assert_eq!(h.count("SELECT COUNT(*) FROM review_items").await, 1);
    assert_eq!(h.count("SELECT COUNT(*) FROM drafts").await, 1);

    // Close the review.
    let (_, reviews) = h.call("GET", &format!("/api/v1/workspaces/{ws}/reviews?status=open"), Some(ADMIN), None).await;
    let rid = reviews["items"][0]["id"].as_str().unwrap();
    let (s, r) = h.call("PATCH", &format!("/api/v1/workspaces/{ws}/reviews/{rid}"), Some(ADMIN), Some(json!({"status":"closed"}))).await;
    assert_eq!((s, r["status"].as_str()), (StatusCode::OK, Some("closed")));
}

#[tokio::test]
async fn manual_override_triggers_workflow_once() {
    let h = harness().await;
    let (ws, token) = h.workspace_with_alias(ADMIN).await;
    let src: String = sqlx::query_scalar("SELECT id FROM email_sources").fetch_one(&h.state.db).await.unwrap();
    let _ = token;
    let (s, _) = h
        .call(
            "POST",
            &format!("/api/v1/workspaces/{ws}/sources/{src}/test-email"),
            Some(ADMIN),
            Some(json!({"subject":"Quick question","body_text":"When is the holiday party?"})),
        )
        .await;
    assert_eq!(s, StatusCode::ACCEPTED);
    worker::drain(&h.state).await;
    let (_, list) = h.call("GET", &format!("/api/v1/workspaces/{ws}/messages"), Some(ADMIN), None).await;
    assert_eq!(list["items"][0]["category"], "general");
    assert_eq!(h.count("SELECT COUNT(*) FROM review_items").await, 0);

    let id = list["items"][0]["id"].as_str().unwrap().to_string();
    let path = format!("/api/v1/workspaces/{ws}/messages/{id}/classification");
    let (s, c) = h.call("PATCH", &path, Some(ADMIN), Some(json!({"category":"timesheet"}))).await;
    assert_eq!((s, c["source"].as_str(), c["needs_review"].as_bool()), (StatusCode::OK, Some("manual"), Some(false)));
    h.call("PATCH", &path, Some(ADMIN), Some(json!({"category":"timesheet"}))).await;
    worker::drain(&h.state).await;
    assert_eq!(h.count("SELECT COUNT(*) FROM review_items").await, 1);
    assert_eq!(h.count("SELECT COUNT(*) FROM drafts").await, 1);

    // A later model run never overwrites the human decision.
    h.call("POST", &format!("/api/v1/workspaces/{ws}/messages/{id}/reprocess"), Some(ADMIN), None).await;
    worker::drain(&h.state).await;
    let src: String = sqlx::query_scalar("SELECT source FROM classifications").fetch_one(&h.state.db).await.unwrap();
    assert_eq!(src, "manual");
}

#[tokio::test]
async fn unknown_alias_is_rejected_not_ingested() {
    let h = harness().await;
    h.workspace_with_alias(ADMIN).await;
    h.webhook("msg_x", &received("em__notarealalias"), false).await;
    worker::drain(&h.state).await;
    assert_eq!(h.count("SELECT COUNT(*) FROM email_messages").await, 0);
    assert_eq!(h.count("SELECT COUNT(*) FROM inbound_events WHERE status = 'rejected'").await, 1);
}

#[tokio::test]
async fn pagination_is_stable() {
    let h = harness().await;
    let (ws, token) = h.workspace_with_alias(ADMIN).await;
    for i in 0..5 {
        h.webhook(&format!("d{i}"), &received(&format!("em{i}__{token}")), false).await;
    }
    worker::drain(&h.state).await;
    let mut seen = Vec::new();
    let mut cursor = String::new();
    loop {
        let (_, page) = h
            .call("GET", &format!("/api/v1/workspaces/{ws}/messages?limit=2&cursor={cursor}"), Some(ADMIN), None)
            .await;
        seen.extend(page["items"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap().to_string()));
        match page["next_cursor"].as_str() {
            Some(c) => cursor = c.to_string(),
            None => break,
        }
    }
    let mut unique = seen.clone();
    unique.dedup();
    assert_eq!((seen.len(), unique.len()), (5, 5));
}

// ---------------------------------------------------------------- jobs

#[tokio::test]
async fn concurrent_workers_never_claim_the_same_job() {
    let h = harness().await;
    {
        let mut conn = h.state.db.acquire().await.unwrap();
        for i in 0..30 {
            jobs::enqueue(&mut conn, "noop", &format!("k{i}"), None, &json!({})).await.unwrap();
        }
    }
    let mut tasks = Vec::new();
    for w in 0..6 {
        let pool = h.state.db.clone();
        tasks.push(tokio::spawn(async move {
            let mut mine = Vec::new();
            while let Some(job) = jobs::claim(&pool, &format!("w{w}")).await.unwrap() {
                mine.push(job.id);
            }
            mine
        }));
    }
    let mut all = Vec::new();
    for t in tasks {
        all.extend(t.await.unwrap());
    }
    let total = all.len();
    all.sort();
    all.dedup();
    assert_eq!((total, all.len()), (30, 30));
}

#[tokio::test]
async fn expired_lease_is_recovered_after_crash() {
    let h = harness().await;
    let mut conn = h.state.db.acquire().await.unwrap();
    jobs::enqueue(&mut conn, "noop", "crash", None, &json!({})).await.unwrap();
    drop(conn);
    let first = jobs::claim(&h.state.db, "worker-a").await.unwrap().unwrap();
    assert!(jobs::claim(&h.state.db, "worker-b").await.unwrap().is_none(), "live lease is exclusive");
    sqlx::query("UPDATE jobs SET locked_until = 0").execute(&h.state.db).await.unwrap();
    // Re-claimed by the *same* worker process after the lease expired.
    let second = jobs::claim(&h.state.db, "worker-a").await.unwrap().unwrap();
    assert_eq!((&first.id, second.attempts), (&second.id, 2));
    // The stale first attempt's late completion is fenced out by the attempt number.
    jobs::complete(&h.state.db, &first, "worker-a").await.unwrap();
    assert_eq!(h.count("SELECT COUNT(*) FROM jobs WHERE status = 'running'").await, 1);
    jobs::complete(&h.state.db, &second, "worker-a").await.unwrap();
    assert_eq!(h.count("SELECT COUNT(*) FROM jobs WHERE status = 'succeeded'").await, 1);
}

struct FlakyClassifier(Arc<AtomicUsize>);

#[async_trait]
impl Classifier for FlakyClassifier {
    async fn classify(&self, _: &ClassifyInput) -> Result<ClassifyOutput, AiError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(AiError::Retryable("model unavailable".into()))
    }
}

#[tokio::test]
async fn model_failures_retry_then_dead_letter_to_review() {
    let mut h = harness().await;
    let calls = Arc::new(AtomicUsize::new(0));
    h.state.classifier = Arc::new(FlakyClassifier(calls.clone()));
    h.app = router(h.state.clone());
    let (ws, token) = h.workspace_with_alias(ADMIN).await;

    // Webhook still succeeds quickly while AI is down.
    assert_eq!(h.webhook("d1", &received(&format!("e__{token}")), false).await.0, StatusCode::OK);
    for _ in 0..10 {
        worker::drain(&h.state).await;
        sqlx::query("UPDATE jobs SET run_at = 0 WHERE status = 'pending'").execute(&h.state.db).await.unwrap();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 5, "max_attempts");

    let (_, dead) = h.call("GET", &format!("/api/v1/workspaces/{ws}/jobs?status=dead"), Some(ADMIN), None).await;
    let dead = dead["items"].as_array().unwrap();
    assert_eq!(dead.len(), 1);
    assert_eq!(dead[0]["kind"], "classify_email");

    let (_, list) = h.call("GET", &format!("/api/v1/workspaces/{ws}/messages"), Some(ADMIN), None).await;
    assert_eq!(list["items"][0]["category"], "other");
    assert_eq!(list["items"][0]["needs_review"], true);

    // Admin retry revives it.
    let job_id = dead[0]["id"].as_str().unwrap();
    let (s, j) = h.call("POST", &format!("/api/v1/workspaces/{ws}/jobs/{job_id}/retry"), Some(ADMIN), None).await;
    assert_eq!((s, j["status"].as_str()), (StatusCode::OK, Some("pending")));
}

#[tokio::test]
async fn serves_spa_with_deep_link_fallback_but_json_404_for_api() {
    let dir = tempfile::tempdir().unwrap();
    let web = dir.path().join("web");
    std::fs::create_dir_all(web.join("assets")).unwrap();
    std::fs::write(web.join("index.html"), "<title>Post Room</title>").unwrap();
    std::fs::write(web.join("assets/app.js"), "console.log(1)").unwrap();
    let mut config = Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()));
    config.web_dist_dir = web.display().to_string();
    let state = build_state(config).await.unwrap();
    db::migrate(&state.db).await.unwrap();
    let app = router(state);
    let get = |p: &str| Request::get(p).body(Body::empty()).unwrap();

    for path in ["/", "/w/ws_1/inbox/msg_1", "/assets/app.js"] {
        let res = app.clone().oneshot(get(path)).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{path}");
    }
    let res = app.clone().oneshot(get("/w/ws_1/inbox")).await.unwrap();
    let body = res.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&body).contains("Post Room"));
    assert_eq!(app.clone().oneshot(get("/api/v1/nope")).await.unwrap().status(), StatusCode::NOT_FOUND);
    assert_eq!(app.oneshot(get("/api/v1/me")).await.unwrap().status(), StatusCode::UNAUTHORIZED);
}
