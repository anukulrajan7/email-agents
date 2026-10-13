//! Direct Gmail backfill through Nango's proxy: imports the last N days with full bodies,
//! upgrades preview-only messages, runs once per source, and tolerates transient 403s.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use axum::{
    Json, Router,
    body::Body,
    extract::Path,
    http::{Request, StatusCode},
    routing::get,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use email_ai_api::{build_state, config::Config, db, http::router, jobs::worker};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

fn full_message(id: &str, subject: &str, body: &str) -> Value {
    json!({
        "id": id, "threadId": format!("t-{id}"), "internalDate": (db::now_ms() - 3_600_000).to_string(),
        "snippet": "preview…",
        "payload": {
            "mimeType": "multipart/mixed",
            "headers": [
                {"name": "From", "value": "Client Recruiter <hr@capgemini.example>"},
                {"name": "To", "value": "me@acme.com"},
                {"name": "Subject", "value": subject}
            ],
            "parts": [
                {"mimeType": "text/plain", "body": {"data": URL_SAFE_NO_PAD.encode(body)}},
                {"mimeType": "application/pdf", "filename": "jd.pdf", "body": {"attachmentId": "a", "size": 900}}
            ]
        }
    })
}

async fn spawn_mock(flaky_first: Arc<AtomicUsize>) -> String {
    let app = Router::new()
        .route(
            "/connections/{id}",
            get(|Path(id): Path<String>| async move {
                Json(json!({"connection_id": id, "provider_config_key": "google-mail", "provider": "google-mail",
                            "tags": {"organization_id": std::env::var("BF_WS").unwrap_or_default(),
                                     "end_user_id": std::env::var("BF_USER").unwrap_or_default()}}))
            }),
        )
        .route(
            "/proxy/gmail/v1/users/me/messages",
            get(|| async { Json(json!({"messages": [{"id": "m1"}, {"id": "m2"}, {"id": "m3"}], "resultSizeEstimate": 3})) }),
        )
        .route(
            "/proxy/gmail/v1/users/me/messages/{id}",
            get(move |Path(id): Path<String>| {
                let flaky = flaky_first.clone();
                async move {
                    // The very first full fetch fails with 403 (fresh connection); the retry succeeds.
                    if flaky.fetch_add(1, Ordering::SeqCst) == 0 {
                        return (StatusCode::FORBIDDEN, Json(json!({"error": "not yet"})));
                    }
                    let (subject, body) = match id.as_str() {
                        "m1" => ("Requirement: 3 Java developers, Pune", "Need 3 Java devs with Spring Boot, 5-8 yrs, Pune."),
                        "m2" => ("Timesheet for September", "Please find my timesheet attached. 168 hours."),
                        _ => ("Hello", "Just saying hi."),
                    };
                    (StatusCode::OK, Json(full_message(&id, subject, body)))
                }
            }),
        )
        .route("/records", get(|| async { Json(json!({"records": [], "next_cursor": null})) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

async fn call(app: &Router, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let req = Request::builder().method(method).uri(path).header("x-auth-token", "dev:admin@acme.test");
    let req = match body {
        Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn backfills_full_messages_once_with_retry() {
    let dir = tempfile::tempdir().unwrap();
    let flaky = Arc::new(AtomicUsize::new(0));
    let mut config = Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()));
    config.nango_api_url = spawn_mock(flaky.clone()).await;
    config.nango_api_key = "k".into();
    config.nango_gmail_model = "Message".into();
    let state = build_state(config).await.unwrap();
    db::migrate(&state.db).await.unwrap();
    let app = router(state.clone());

    let (_, ws) = call(&app, "POST", "/api/v1/workspaces", Some(json!({"name":"Acme"}))).await;
    let ws = ws["id"].as_str().unwrap().to_string();
    let (_, me) = call(&app, "GET", "/api/v1/me", None).await;
    // SAFETY: test-local env vars read by the mock (single test in this binary).
    unsafe {
        std::env::set_var("BF_WS", &ws);
        std::env::set_var("BF_USER", me["user"]["id"].as_str().unwrap());
    }
    let (s, src) = call(&app, "POST", &format!("/api/v1/workspaces/{ws}/sources/nango-connections"),
        Some(json!({"connection_id":"conn_bf","provider_config_key":"google-mail"}))).await;
    assert_eq!(s, StatusCode::OK);
    worker::drain(&state).await;

    let (_, msgs) = call(&app, "GET", &format!("/api/v1/workspaces/{ws}/messages"), None).await;
    let items = msgs["items"].as_array().unwrap();
    assert_eq!(items.len(), 3, "all three listed messages imported (first fetch retried)");
    let req = items.iter().find(|m| m["subject"] == "Requirement: 3 Java developers, Pune").unwrap();
    assert_eq!(req["from"], "hr@capgemini.example");
    assert_eq!(req["attachment_count"], 1);
    let (_, detail) = call(&app, "GET", &format!("/api/v1/workspaces/{ws}/messages/{}", req["id"].as_str().unwrap()), None).await;
    assert_eq!(detail["body_text"], "Need 3 Java devs with Spring Boot, 5-8 yrs, Pune.", "full body, not the snippet");

    // Runs once: a second sync doesn't list/fetch Gmail again.
    let fetched = flaky.load(Ordering::SeqCst);
    call(&app, "POST", &format!("/api/v1/workspaces/{ws}/sources/{}/sync", src["id"].as_str().unwrap()), None).await;
    worker::drain(&state).await;
    assert_eq!(flaky.load(Ordering::SeqCst), fetched);
}
