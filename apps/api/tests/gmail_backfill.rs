//! Direct Gmail backfill through Nango's proxy: imports the last N days with full bodies,
//! upgrades preview-only messages, runs once per source, and tolerates transient 403s.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use std::sync::Mutex;

use axum::{
    Json, Router,
    body::Body,
    extract::Path,
    http::{Request, StatusCode},
    routing::{get, post, put},
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

type Calls = Arc<Mutex<Vec<(String, Value)>>>;

async fn spawn_mock(flaky_first: Arc<AtomicUsize>, calls: Calls) -> String {
    let (c1, c2, c3, c4) = (calls.clone(), calls.clone(), calls.clone(), calls);
    let app = Router::new()
        .route("/proxy/gmail/v1/users/me/profile", get(|| async { Json(json!({"emailAddress": "me@acme.com"})) }))
        .route("/proxy/gmail/v1/users/me/drafts", post(move |Json(b): Json<Value>| async move {
            c1.lock().unwrap().push(("create_draft".into(), b));
            Json(json!({"id": "gdraft1"}))
        }))
        .route("/proxy/gmail/v1/users/me/drafts/{id}", put(move |Path(id): Path<String>, Json(b): Json<Value>| async move {
            c2.lock().unwrap().push((format!("update_draft:{id}"), b));
            Json(json!({"id": id}))
        }))
        .route("/proxy/gmail/v1/users/me/drafts/send", post(move |Json(b): Json<Value>| async move {
            c3.lock().unwrap().push(("send_draft".into(), b));
            Json(json!({"id": "gmsg-sent"}))
        }))
        .route("/proxy/gmail/v1/users/me/messages/send", post(move |Json(b): Json<Value>| async move {
            c4.lock().unwrap().push(("send_message".into(), b));
            Json(json!({"id": "gmsg-direct"}))
        }))
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
            get(|| async {
                // A new email "arrives" in Gmail once the test flips NEW_MAIL.
                let mut ids = vec![json!({"id": "m1"}), json!({"id": "m2"}), json!({"id": "m3"})];
                if std::env::var("BF_NEW_MAIL").is_ok() {
                    ids.insert(0, json!({"id": "m4"}));
                }
                Json(json!({"messages": ids}))
            }),
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
                        "m4" => ("Brand new email", "This arrived after the first sync."),
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
    let calls: Calls = Arc::default();
    config.nango_api_url = spawn_mock(flaky.clone(), calls.clone()).await;
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

    // Reply from the mailbox: the timesheet email got a workflow draft.
    let ts = items.iter().find(|m| m["subject"] == "Timesheet for September").unwrap();
    let (_, detail) = call(&app, "GET", &format!("/api/v1/workspaces/{ws}/messages/{}", ts["id"].as_str().unwrap()), None).await;
    assert_eq!((detail["source"]["can_create_provider_draft"].as_bool(), detail["source"]["can_send"].as_bool()), (Some(true), Some(true)));
    let draft_id = detail["draft"]["id"].as_str().unwrap().to_string();

    // The default "reply to questions" automation already saved each reply into Gmail Drafts.
    assert_eq!(calls.lock().unwrap().iter().filter(|(k, _)| k == "create_draft").count(), 3);
    calls.lock().unwrap().clear();
    sqlx::query("UPDATE drafts SET provider_draft_id = NULL").execute(&state.db).await.unwrap();

    let (s, saved) = call(&app, "POST", &format!("/api/v1/workspaces/{ws}/drafts/{draft_id}/provider-draft"), None).await;
    assert_eq!((s, saved["provider_draft_id"].as_str()), (StatusCode::OK, Some("gdraft1")));
    {
        let calls = calls.lock().unwrap();
        let (kind, body) = &calls[0];
        assert_eq!(kind, "create_draft");
        assert_eq!(body["message"]["threadId"], "t-m2", "reply stays in the Gmail thread");
        let raw = String::from_utf8(URL_SAFE_NO_PAD.decode(body["message"]["raw"].as_str().unwrap()).unwrap()).unwrap();
        assert!(raw.contains("Subject: Re: Timesheet for September") && raw.contains("hr@capgemini.example"));
    }

    // Sending needs explicit confirmation, happens once, and is audited.
    assert_eq!(call(&app, "POST", &format!("/api/v1/workspaces/{ws}/drafts/{draft_id}/send"), Some(json!({"confirm": false}))).await.0, StatusCode::BAD_REQUEST);
    let (s, sent) = call(&app, "POST", &format!("/api/v1/workspaces/{ws}/drafts/{draft_id}/send"), Some(json!({"confirm": true}))).await;
    assert_eq!((s, sent["status"].as_str()), (StatusCode::OK, Some("sent")));
    assert_eq!(call(&app, "POST", &format!("/api/v1/workspaces/{ws}/drafts/{draft_id}/send"), Some(json!({"confirm": true}))).await.0, StatusCode::CONFLICT);
    let kinds: Vec<String> = calls.lock().unwrap().iter().map(|(k, _)| k.clone()).collect();
    assert_eq!(kinds, ["create_draft", "update_draft:gdraft1", "send_draft"]);
    let (_, detail) = call(&app, "GET", &format!("/api/v1/workspaces/{ws}/messages/{}", ts["id"].as_str().unwrap()), None).await;
    assert_eq!(detail["draft"]["status"], "sent");
    assert!(detail["audit"].as_array().unwrap().iter().any(|a| a["action"] == "draft.sent"));
    assert_eq!(detail["replied"], true, "a sent reply marks the email as replied");
    let (_, list) = call(&app, "GET", &format!("/api/v1/workspaces/{ws}/messages?sort=importance"), None).await;
    let row = list["items"].as_array().unwrap().iter().find(|m| m["id"] == ts["id"]).unwrap();
    assert_eq!(row["replied"], true);

    // A new email arrives in Gmail after the first sync: the next sync (or the 5-minute
    // poll) imports it straight from Gmail, without waiting for Nango's hourly sync.
    unsafe { std::env::set_var("BF_NEW_MAIL", "1") };
    call(&app, "POST", &format!("/api/v1/workspaces/{ws}/sources/{}/sync", src["id"].as_str().unwrap()), None).await;
    worker::drain(&state).await;
    let (_, msgs) = call(&app, "GET", &format!("/api/v1/workspaces/{ws}/messages"), None).await;
    let subjects: Vec<&str> = msgs["items"].as_array().unwrap().iter().map(|m| m["subject"].as_str().unwrap()).collect();
    assert_eq!(subjects.len(), 4, "{subjects:?}");
    assert!(subjects.contains(&"Brand new email"));
}
