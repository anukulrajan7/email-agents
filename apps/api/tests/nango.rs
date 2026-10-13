//! Nango Gmail vertical slice against a mock Nango API: connect session, server-side
//! connection ownership checks, bounded backfill, cursor-based incremental sync,
//! signed webhooks, and disconnect.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query},
    http::{Request, StatusCode},
    routing::{get, post},
};
use email_ai_api::{
    build_state,
    config::Config,
    db,
    http::{AppState, router},
    jobs::worker,
    sources::nango::sign_webhook,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

/// connection_id -> (organization_id, end_user_id) tags the mock reports.
type Tags = Arc<Mutex<HashMap<String, (String, String)>>>;

#[derive(Clone, Default)]
struct Mock {
    tags: Tags,
    sessions: Arc<Mutex<Vec<Value>>>,
}

fn rec(id: &str, thread: &str, days_ago: i64, subject: &str, cursor: &str, deleted: bool) -> Value {
    json!({
        "id": id, "threadId": thread, "sender": "Priya <priya@acme.com>", "recipients": ["hr@acme.com"],
        "date": db::iso(db::now_ms() - days_ago * 86_400_000), "subject": subject, "body": "Hello HR",
        "attachments": [],
        "_nango_metadata": {"deleted_at": if deleted { json!("2026-10-12T00:00:00Z") } else { Value::Null },
                            "last_action": if deleted { "DELETED" } else { "ADDED" }, "cursor": cursor}
    })
}

async fn spawn_mock(mock: Mock) -> String {
    let m1 = mock.clone();
    let m2 = mock.clone();
    let app = Router::new()
        .route(
            "/connect/sessions",
            post(move |Json(body): Json<Value>| {
                let m = m1.clone();
                async move {
                    m.sessions.lock().unwrap().push(body);
                    (StatusCode::CREATED, Json(json!({"data": {"token": "nst_123", "expires_at": "2026-10-13T12:00:00Z"}})))
                }
            }),
        )
        .route(
            "/connections/{id}",
            get(move |Path(id): Path<String>, Query(q): Query<HashMap<String, String>>| {
                let m = m2.clone();
                async move {
                    match m.tags.lock().unwrap().get(&id) {
                        Some((org, user)) => (
                            StatusCode::OK,
                            Json(json!({
                                "connection_id": id, "provider_config_key": q.get("provider_config_key"),
                                "provider": "google-mail", "tags": {"organization_id": org, "end_user_id": user}
                            })),
                        ),
                        None => (StatusCode::NOT_FOUND, Json(json!({"error": {"code": "not_found"}}))),
                    }
                }
            })
            .delete(|| async { Json(json!({})) }),
        )
        .route(
            "/records",
            get(|Query(q): Query<HashMap<String, String>>| async move {
                let (records, next) = match q.get("cursor").map(String::as_str) {
                    None => (
                        vec![
                            rec("m1", "t1", 1, "Leave request for Friday", "rc1", false),
                            rec("m2", "t1", 1, "Re: Leave request for Friday", "rc2", false),
                            rec("old", "t9", 40, "Ancient timesheet", "rc2b", false),
                        ],
                        Some("p2"),
                    ),
                    Some("p2") => (vec![rec("gone", "t2", 1, "Deleted", "rc3", true)], None),
                    Some("rc3") => (vec![rec("m4", "t4", 0, "September timesheet", "rc4", false)], None),
                    _ => (vec![], None),
                };
                Json(json!({"records": records, "next_cursor": next}))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

struct H {
    state: AppState,
    app: Router,
    mock: Mock,
    _dir: tempfile::TempDir,
}

async fn harness(configured: bool) -> H {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()));
    let mock = Mock::default();
    config.nango_api_url = spawn_mock(mock.clone()).await;
    if configured {
        config.nango_api_key = "nango_secret".into();
    }
    let state = build_state(config).await.unwrap();
    db::migrate(&state.db).await.unwrap();
    H { app: router(state.clone()), state, mock, _dir: dir }
}

impl H {
    async fn call(&self, method: &str, path: &str, token: &str, body: Option<Value>) -> (StatusCode, Value) {
        let req = Request::builder().method(method).uri(path).header("authorization", format!("Bearer {token}"));
        let req = match body {
            Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
            None => req.body(Body::empty()).unwrap(),
        };
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn webhook(&self, body: &Value, secret: &str) -> (StatusCode, Value) {
        let raw = body.to_string();
        let req = Request::post("/webhooks/nango")
            .header("x-nango-hmac-sha256", sign_webhook(secret, raw.as_bytes()))
            .body(Body::from(raw))
            .unwrap();
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(&self.state.db).await.unwrap()
    }

    /// Workspace + admin user id.
    async fn setup(&self) -> (String, String) {
        let (_, ws) = self.call("POST", "/api/v1/workspaces", ADMIN, Some(json!({"name":"Acme"}))).await;
        let (_, me) = self.call("GET", "/api/v1/me", ADMIN, None).await;
        (ws["id"].as_str().unwrap().into(), me["user"]["id"].as_str().unwrap().into())
    }
}

const ADMIN: &str = "dev:admin@acme.test";

#[tokio::test]
async fn unconfigured_nango_is_reported_honestly() {
    let h = harness(false).await;
    let (ws, _) = h.setup().await;
    let (s, body) = h.call("POST", &format!("/api/v1/workspaces/{ws}/sources/nango-session"), ADMIN, Some(json!({"provider":"gmail"}))).await;
    assert_eq!((s, body["error"]["code"].as_str()), (StatusCode::NOT_IMPLEMENTED, Some("not_implemented")));
}

#[tokio::test]
async fn gmail_connect_backfill_and_incremental_sync() {
    let h = harness(true).await;
    let (ws, admin_id) = h.setup().await;
    let base = format!("/api/v1/workspaces/{ws}/sources");

    // Session is scoped to this workspace + user and to the Gmail integration only.
    let (s, session) = h.call("POST", &format!("{base}/nango-session"), ADMIN, Some(json!({"provider":"gmail"}))).await;
    assert_eq!((s, session["token"].as_str()), (StatusCode::OK, Some("nst_123")));
    let sent = h.mock.sessions.lock().unwrap()[0].clone();
    assert_eq!(sent["tags"]["organization_id"], ws.as_str());
    assert_eq!(sent["tags"]["end_user_id"], admin_id.as_str());
    assert_eq!(sent["allowed_integrations"], json!(["google-mail"]));
    let (s, _) = h.call("POST", &format!("{base}/nango-session"), ADMIN, Some(json!({"provider":"microsoft"}))).await;
    assert_eq!(s, StatusCode::NOT_IMPLEMENTED);

    // A connection tagged for another workspace is refused, even if the browser claims it.
    h.mock.tags.lock().unwrap().insert("conn_foreign".into(), ("ws_someone_else".into(), admin_id.clone()));
    let (s, _) = h
        .call("POST", &format!("{base}/nango-connections"), ADMIN, Some(json!({"connection_id":"conn_foreign","provider_config_key":"google-mail"})))
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = h
        .call("POST", &format!("{base}/nango-connections"), ADMIN, Some(json!({"connection_id":"conn_unknown","provider_config_key":"google-mail"})))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    h.mock.tags.lock().unwrap().insert("conn_ok".into(), (ws.clone(), admin_id.clone()));
    let (s, src) = h
        .call("POST", &format!("{base}/nango-connections"), ADMIN, Some(json!({"connection_id":"conn_ok","provider_config_key":"google-mail"})))
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!((src["kind"].as_str(), src["status"].as_str()), (Some("nango_gmail"), Some("connected")));
    assert_eq!(src["capabilities"]["sync"], true);
    assert_eq!(src["capabilities"]["provider_drafts"], false);
    let src_id = src["id"].as_str().unwrap().to_string();

    // Initial backfill: m1 + m2 (same Gmail thread), not the 40-day-old one, not the deleted one.
    worker::drain(&h.state).await;
    assert_eq!(h.count("SELECT COUNT(*) FROM email_messages").await, 2);
    assert_eq!(h.count("SELECT COUNT(*) FROM email_threads").await, 1, "grouped by provider thread");
    let cursor: String = sqlx::query_scalar("SELECT sync_cursor FROM email_sources WHERE id = ?1")
        .bind(&src_id)
        .fetch_one(&h.state.db)
        .await
        .unwrap();
    assert_eq!(cursor, "rc3");
    let (_, list) = h.call("GET", &format!("/api/v1/workspaces/{ws}/messages?category=leave"), ADMIN, None).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 2);

    // Manual sync resumes from the cursor: only m4 is new; it is a timesheet → workflow runs.
    let (s, _) = h.call("POST", &format!("{base}/{src_id}/sync"), ADMIN, None).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    worker::drain(&h.state).await;
    assert_eq!(h.count("SELECT COUNT(*) FROM email_messages").await, 3);
    assert_eq!(h.count("SELECT COUNT(*) FROM review_items").await, 1);

    // Signed sync webhook enqueues another (no-op) sync; replay is deduped; bad signature rejected.
    let event = json!({"type":"sync","connectionId":"conn_ok","providerConfigKey":"google-mail","syncName":"emails",
                       "model":"GmailEmail","success":true,"modifiedAfter":"2026-10-13T00:00:00Z",
                       "responseResults":{"added":1,"updated":0,"deleted":0}});
    assert_eq!(h.webhook(&event, "wrong").await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(h.webhook(&event, "nango-test-secret").await.1["status"], "accepted");
    assert_eq!(h.webhook(&event, "nango-test-secret").await.1["status"], "duplicate");
    worker::drain(&h.state).await;
    assert_eq!(h.count("SELECT COUNT(*) FROM email_messages").await, 3, "no duplicates on re-sync");

    // Refresh failure marks the source degraded.
    let fail = json!({"type":"auth","operation":"refresh","connectionId":"conn_ok","providerConfigKey":"google-mail","success":false});
    h.webhook(&fail, "nango-test-secret").await;
    let (_, sources) = h.call("GET", &base, ADMIN, None).await;
    let gmail = sources["items"].as_array().unwrap().iter().find(|s| s["kind"] == "nango_gmail").unwrap().clone();
    assert_eq!(gmail["status"], "degraded");

    // Disconnect stops syncing.
    let (s, d) = h.call("POST", &format!("{base}/{src_id}/disconnect"), ADMIN, None).await;
    assert_eq!((s, d["status"].as_str()), (StatusCode::OK, Some("disconnected")));
    assert_eq!(h.call("POST", &format!("{base}/{src_id}/sync"), ADMIN, None).await.0, StatusCode::CONFLICT);
}

#[tokio::test]
async fn auth_webhook_registers_connection_in_background() {
    let h = harness(true).await;
    let (ws, admin_id) = h.setup().await;
    h.mock.tags.lock().unwrap().insert("conn_wh".into(), (ws.clone(), admin_id));
    let event = json!({"type":"auth","operation":"creation","connectionId":"conn_wh","providerConfigKey":"google-mail",
                       "provider":"google-mail","environment":"DEV","success":true,
                       "tags":{"organization_id": ws, "end_user_id": "spoofable"}});
    assert_eq!(h.webhook(&event, "nango-test-secret").await.1["status"], "accepted");
    worker::drain(&h.state).await;
    assert_eq!(h.count("SELECT COUNT(*) FROM email_sources WHERE kind = 'nango_gmail' AND status = 'connected'").await, 1);
    assert_eq!(h.count("SELECT COUNT(*) FROM email_messages").await, 2);
}
