//! Gmail mail the mailbox itself sent is stored but never triaged (direction), and legacy Gmail rows
//! stored without HTML get it back through a bounded, idempotent re-fetch.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, State},
    http::{Request, StatusCode},
    routing::get,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use email_ai_api::{
    jobs::worker,
    build_state,
    config::Config,
    db,
    domain::NormalizedEmail,
    http::{AppState, router},
    ingest, jobs,
    threads::thread_has_reply_after,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

#[derive(Default)]
struct Gmail {
    listed: Vec<String>,
    messages: HashMap<String, Value>,
    fetches: HashMap<String, usize>,
}

type Shared = Arc<Mutex<Gmail>>;

async fn spawn_mock(shared: Shared) -> String {
    let app = Router::new()
        .route("/proxy/gmail/v1/users/me/profile", get(|| async { Json(json!({"emailAddress": "Me@Acme.com"})) }))
        .route(
            "/proxy/gmail/v1/users/me/messages",
            get(|State(g): State<Shared>| async move {
                let ids: Vec<Value> = g.lock().unwrap().listed.iter().map(|id| json!({"id": id})).collect();
                Json(json!({"messages": ids}))
            }),
        )
        .route(
            "/proxy/gmail/v1/users/me/messages/{id}",
            get(|State(g): State<Shared>, Path(id): Path<String>| async move {
                let mut g = g.lock().unwrap();
                *g.fetches.entry(id.clone()).or_default() += 1;
                match g.messages.get(&id) {
                    Some(m) => (StatusCode::OK, Json(m.clone())),
                    None => (StatusCode::NOT_FOUND, Json(json!({}))),
                }
            }),
        )
        .route("/records", get(|| async { Json(json!({"records": [], "next_cursor": null})) }))
        .with_state(shared);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

fn b64(s: &str) -> String {
    URL_SAFE_NO_PAD.encode(s)
}

fn gmail_message(id: &str, thread: &str, from: &str, subject: &str, labels: &[&str], age_ms: i64, parts: Value) -> Value {
    json!({
        "id": id, "threadId": thread, "labelIds": labels, "internalDate": (db::now_ms() - age_ms).to_string(),
        "payload": {
            "mimeType": "multipart/mixed",
            "headers": [
                {"name": "From", "value": from},
                {"name": "To", "value": "Hr Team <hr@acme.com>"},
                {"name": "Cc", "value": "Lead <lead@acme.com>"},
                {"name": "Subject", "value": subject}
            ],
            "parts": parts
        }
    })
}

fn plain(text: &str) -> Value {
    json!([{"mimeType": "text/plain", "body": {"data": b64(text)}}])
}

struct Harness {
    state: AppState,
    app: Router,
    gmail: Shared,
    ws: String,
    source_id: String,
    _dir: tempfile::TempDir,
}

impl Harness {
    async fn call(&self, method: &str, path: &str) -> (StatusCode, Value) {
        let req = Request::builder().method(method).uri(path).header("x-auth-token", "dev:admin@acme.test");
        let res = self.app.clone().oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn sync(&self) {
        let (s, _) = self.call("POST", &format!("/api/v1/workspaces/{}/sources/{}/sync", self.ws, self.source_id)).await;
        assert_eq!(s, StatusCode::ACCEPTED);
        worker::drain(&self.state).await;
    }

    async fn messages(&self, query: &str) -> Vec<Value> {
        let (_, v) = self.call("GET", &format!("/api/v1/workspaces/{}/messages{query}", self.ws)).await;
        v["items"].as_array().unwrap().clone()
    }

    async fn id_of(&self, source_message_id: &str) -> String {
        sqlx::query_scalar("SELECT id FROM email_messages WHERE source_message_id = ?1")
            .bind(source_message_id)
            .fetch_one(&self.state.db)
            .await
            .unwrap()
    }

    /// Stores a message the way an older version would have: through ingest, with no HTML.
    async fn legacy(&self, source_message_id: &str, thread: &str, sender: &str, text: &str) {
        let email = NormalizedEmail {
            source_message_id: source_message_id.into(),
            sender_address: sender.into(),
            subject: "Legacy mail".into(),
            body_text: text.into(),
            received_at: db::now_ms() - 3 * 3_600_000,
            ..Default::default()
        };
        let mut tx = self.state.db.begin().await.unwrap();
        sqlx::query("UPDATE email_sources SET last_synced_at = 1 WHERE id = ?1").bind(&self.source_id).execute(&mut *tx).await.unwrap();
        ingest::persist_message(&mut tx, &self.source_id, &self.ws, &email, Some(thread), json!({})).await.unwrap().unwrap();
        tx.commit().await.unwrap();
    }
}

async fn refetch(h: &Harness) {
    jobs::message_jobs::refetch_gmail_html(&h.state, &json!({ "source_id": h.source_id })).await.unwrap();
}

async fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let gmail: Shared = Arc::default();
    let mut config = Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()));
    config.nango_api_url = spawn_mock(gmail.clone()).await;
    config.nango_api_key = "k".into();
    config.nango_gmail_model = "Message".into();
    let state = build_state(config).await.unwrap();
    db::migrate(&state.db).await.unwrap();
    let app = router(state.clone());

    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/workspaces")
        .header("x-auth-token", "dev:admin@acme.test")
        .header("content-type", "application/json")
        .body(Body::from(json!({"name": "Acme"}).to_string()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let ws: Value = serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let ws = ws["id"].as_str().unwrap().to_string();
    let source_id = "src_gmail_test".to_string();
    sqlx::query(
        "INSERT INTO email_sources (id, workspace_id, kind, status, external_connection_id, provider_config_key,
                                    capabilities_json, created_at, backfilled_at)
         VALUES (?1, ?2, 'nango_gmail', 'connected', 'conn_x', 'google-mail', ?3, ?4, ?4)",
    )
    .bind(&source_id)
    .bind(&ws)
    .bind(serde_json::to_string(&email_ai_api::sources::capabilities("nango_gmail")).unwrap())
    .bind(db::now_ms())
    .execute(&state.db)
    .await
    .unwrap();
    Harness { state, app, gmail, ws, source_id, _dir: dir }
}

#[tokio::test]
async fn sent_mail_is_stored_but_never_triaged_and_marks_threads_answered() {
    let h = harness().await;
    // Stored before the mailbox address was known: looks inbound until the next sync learns it.
    h.legacy("legacy-self", "t-old", "me@acme.com", "A note I sent myself earlier.").await;
    sqlx::query("DELETE FROM jobs WHERE kind = 'classify_email'").execute(&h.state.db).await.unwrap();
    {
        let mut g = h.gmail.lock().unwrap();
        g.listed = vec!["in1".into(), "sent1".into(), "self1".into()];
        g.messages.insert(
            "in1".into(),
            gmail_message("in1", "thread1", "Priya Sharma <priya@acme.com>", "Leave next week", &["INBOX"], 2 * 3_600_000, plain("Can I take Friday off?")),
        );
        g.messages.insert(
            "sent1".into(),
            gmail_message("sent1", "thread1", "Someone <alias@acme.com>", "Re: Leave next week", &["SENT"], 3_600_000, plain("Approved, enjoy.")),
        );
        g.messages.insert(
            "self1".into(),
            gmail_message("self1", "thread9", "Me <ME@acme.com>", "Notes", &["INBOX"], 30 * 60_000, plain("Reminder to self.")),
        );
    }
    h.sync().await;

    let inbound = h.messages("").await;
    let subjects: Vec<&str> = inbound.iter().map(|m| m["subject"].as_str().unwrap()).collect();
    assert_eq!(subjects, ["Leave next week"], "default list is inbound only: {subjects:?}");
    assert_eq!(inbound[0]["direction"], "inbound");
    assert_eq!(h.messages("?direction=all").await.len(), 4);
    let outbound = h.messages("?direction=outbound").await;
    assert_eq!(outbound.len(), 3, "SENT label, own address, and the legacy row marked once the address was learned");
    assert!(outbound.iter().all(|m| m["direction"] == "outbound" && m["processing_status"] == "processed"));
    let (s, _) = h.call("GET", &format!("/api/v1/workspaces/{}/messages?direction=sideways", h.ws)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "unknown direction is rejected");

    let mailbox: String =
        sqlx::query_scalar("SELECT mailbox_address FROM email_sources WHERE id = ?1").bind(&h.source_id).fetch_one(&h.state.db).await.unwrap();
    assert_eq!(mailbox, "me@acme.com");

    // Outgoing mail is never classified, analysed or drafted against, and contacts skip the mailbox itself.
    let triaged: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM classifications c JOIN email_messages m ON m.id = c.message_id WHERE m.direction = 'outbound')
              + (SELECT COUNT(*) FROM message_insights i JOIN email_messages m ON m.id = i.message_id WHERE m.direction = 'outbound')
              + (SELECT COUNT(*) FROM drafts d JOIN email_messages m ON m.id = d.message_id WHERE m.direction = 'outbound')",
    )
    .fetch_one(&h.state.db)
    .await
    .unwrap();
    assert_eq!(triaged, 0);
    let inbound_classified: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM classifications").fetch_one(&h.state.db).await.unwrap();
    assert_eq!(inbound_classified, 1, "only the inbound message was classified");

    // The inbound message's thread has a reply after it was received; an unrelated thread does not.
    let in_id = h.id_of("in1").await;
    let received: i64 = sqlx::query_scalar("SELECT received_at FROM email_messages WHERE id = ?1").bind(&in_id).fetch_one(&h.state.db).await.unwrap();
    assert!(thread_has_reply_after(&h.state.db, &h.ws, "thread1", received).await.unwrap());
    assert!(!thread_has_reply_after(&h.state.db, &h.ws, "thread1", db::now_ms()).await.unwrap(), "reply must be after the given time");
    assert!(!thread_has_reply_after(&h.state.db, &h.ws, "no-such-thread", 0).await.unwrap());

    let (_, detail) = h.call("GET", &format!("/api/v1/workspaces/{}/messages/{in_id}", h.ws)).await;
    assert_eq!(detail["from_name"], "Priya Sharma");
    assert_eq!(detail["from"], "priya@acme.com");
    assert_eq!(detail["to"], json!(["hr@acme.com"]));
    assert_eq!(detail["cc"], json!(["lead@acme.com"]));
    assert_eq!(detail["direction"], "inbound");
}

#[tokio::test]
async fn sent_draft_counts_as_a_reply() {
    let h = harness().await;
    h.legacy("m1", "thread-d", "priya@acme.com", "Question for HR here.").await;
    let id = h.id_of("m1").await;
    let received: i64 = sqlx::query_scalar("SELECT received_at FROM email_messages WHERE id = ?1").bind(&id).fetch_one(&h.state.db).await.unwrap();
    assert!(!thread_has_reply_after(&h.state.db, &h.ws, "thread-d", received).await.unwrap());
    sqlx::query(
        "INSERT INTO drafts (id, workspace_id, message_id, body_text, status, created_at, updated_at)
         VALUES ('drf_x', ?1, ?2, 'Thanks', 'sent', ?3, ?3)",
    )
    .bind(&h.ws)
    .bind(&id)
    .bind(db::now_ms())
    .execute(&h.state.db)
    .await
    .unwrap();
    assert!(thread_has_reply_after(&h.state.db, &h.ws, "thread-d", received).await.unwrap());
}

#[tokio::test]
async fn legacy_gmail_rows_get_their_html_back_once() {
    let h = harness().await;
    h.legacy("old1", "t-old1", "billing@vendor.example", "[image: logo]").await;
    h.legacy("old2", "t-old2", "ops@vendor.example", "Plain text only mail, nothing fancy in here at all.").await;
    // Older versions stored no HTML; mark them as such (new rows default to "done").
    sqlx::query("UPDATE email_messages SET html_refetch_tries = 0, content_complete = 1").execute(&h.state.db).await.unwrap();
    let html = "<!--[if mso]><p>Outlook only</p><![endif]--><div><p>Your invoice <b>#123</b> is ready.</p>\
<p><a href=\"https://pay.example/inv/123\">Pay now</a></p></div>";
    {
        let mut g = h.gmail.lock().unwrap();
        // multipart/mixed > multipart/related > multipart/alternative > text + html.
        g.messages.insert(
            "old1".into(),
            gmail_message(
                "old1",
                "t-old1",
                "Billing <billing@vendor.example>",
                "Legacy mail",
                &["INBOX"],
                3 * 3_600_000,
                json!([{"mimeType": "multipart/related", "parts": [
                    {"mimeType": "multipart/alternative", "parts": [
                        {"mimeType": "text/plain", "body": {"data": b64("[image: logo]\n<https://x.example/t.png>")}},
                        {"mimeType": "text/html", "body": {"data": b64(html)}}
                    ]},
                    {"mimeType": "image/png", "filename": "logo.png", "body": {"attachmentId": "a1", "size": 10}}
                ]}]),
            ),
        );
        g.messages.insert(
            "old2".into(),
            gmail_message("old2", "t-old2", "ops@vendor.example", "Legacy mail", &["INBOX"], 3 * 3_600_000, plain("Plain text only mail, nothing fancy in here at all.")),
        );
    }
    h.sync().await;

    let id = h.id_of("old1").await;
    let (_, detail) = h.call("GET", &format!("/api/v1/workspaces/{}/messages/{id}", h.ws)).await;
    let safe = detail["body_html_safe"].as_str().expect("html was filled in");
    assert!(safe.contains("Your invoice") && safe.contains("https://pay.example/inv/123"), "{safe}");
    assert!(!safe.contains("Outlook only"), "conditional comment content is not shown: {safe}");
    assert_eq!(detail["body_text"], "Your invoice #123 is ready.\n\nPay now https://pay.example/inv/123");
    let id2 = h.id_of("old2").await;
    let (_, detail2) = h.call("GET", &format!("/api/v1/workspaces/{}/messages/{id2}", h.ws)).await;
    assert!(detail2["body_html_safe"].is_null(), "a plain-text-only mail stays without HTML");

    // Idempotent: both rows are done, so running the job again fetches nothing more for them.
    let before: HashMap<String, usize> = h.gmail.lock().unwrap().fetches.clone();
    refetch(&h).await;
    let after = h.gmail.lock().unwrap().fetches.clone();
    assert_eq!(before.get("old1"), after.get("old1"));
    assert_eq!(before.get("old2"), after.get("old2"));
    let tries: Vec<i64> = sqlx::query_scalar("SELECT html_refetch_tries FROM email_messages ORDER BY source_message_id")
        .fetch_all(&h.state.db)
        .await
        .unwrap();
    assert_eq!(tries, [99, 99]);
}

#[tokio::test]
async fn failing_refetch_is_bounded() {
    let h = harness().await;
    h.legacy("gone1", "t-g", "a@vendor.example", "Some body that is long enough to be real text.").await;
    sqlx::query("UPDATE email_messages SET html_refetch_tries = 0, content_complete = 1").execute(&h.state.db).await.unwrap();
    for _ in 0..5 {
        refetch(&h).await;
    }
    let tries: i64 = sqlx::query_scalar("SELECT html_refetch_tries FROM email_messages").fetch_one(&h.state.db).await.unwrap();
    assert_eq!(tries, 3, "three tries, then left alone");
}

#[tokio::test]
async fn processed_status_is_accepted_on_every_pooled_connection() {
    let h = harness().await;
    h.legacy("m1", "t1", "a@vendor.example", "Some body that is long enough to be real text.").await;
    // The migrating connection must not keep the pre-migration CHECK in memory.
    let mut conns = Vec::new();
    for _ in 0..8 {
        conns.push(h.state.db.acquire().await.unwrap());
    }
    for conn in &mut conns {
        sqlx::query("UPDATE email_messages SET processing_status = 'processed'").execute(&mut **conn).await.unwrap();
        sqlx::query("UPDATE email_messages SET processing_status = 'workflow_running'").execute(&mut **conn).await.unwrap();
    }
}
