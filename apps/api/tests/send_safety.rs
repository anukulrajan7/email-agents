//! Send safety end to end: the pre-send check, blocked sends (placeholder, [NEEDS], wrong recipient,
//! duplicate, judge "promises"), warning-only sends, scheduled send + undo, the delayed send firing,
//! no double send on an ambiguous Gmail error, the judge alert gate, and OTP mail retry/failure.
//! Gmail is a local mock behind the Nango proxy; no real email is sent.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use axum::{
    Json, Router,
    body::Body,
    extract::{Path, State},
    http::{Request, StatusCode},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use email_ai_api::{
    ai::judge::{FixedJudge, Verdict},
    alerts,
    platform::audit::Actor,
    build_state,
    platform::config::{AuthMode, Config},
    platform::db::{self, now_ms},
    platform::error::AppError,
    http::{AppState, router},
    jobs::worker,
    platform::mailer::Mailer,
    replies,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const ADMIN: &str = "dev:admin@acme.test";
const MAILBOX: &str = "hr@acme.com";
const CLIENT: &str = "priya.sharma@client.example";
const GOOD: &str = "Hello Priya,\n\nThanks, we received your September timesheet and passed it to payroll.\n\nBest regards,\nHR Operations";

#[derive(Clone, Copy, PartialEq)]
enum SendMode {
    Ok,
    /// Gmail sends it but the proxy answers 503 (timeout-like).
    SentBut503,
    /// 503 and nothing was sent.
    Fail503,
    /// Clear rejection before sending.
    Reject400,
}

struct Mock {
    mode: SendMode,
    sends: Vec<Value>,
    /// What a `rfc822msgid:` search returns.
    search_hit: Option<String>,
}

type Shared = Arc<Mutex<Mock>>;

async fn spawn_gmail(mock: Shared) -> String {
    let app = Router::new()
        .route("/proxy/gmail/v1/users/me/profile", get(|| async { Json(json!({ "emailAddress": MAILBOX })) }))
        .route(
            "/proxy/gmail/v1/users/me/messages/send",
            post(|State(m): State<Shared>, Json(b): Json<Value>| async move {
                let mut m = m.lock().unwrap();
                match m.mode {
                    SendMode::Ok => {
                        m.sends.push(b);
                        (StatusCode::OK, Json(json!({ "id": format!("gmsg-{}", m.sends.len()) })))
                    }
                    SendMode::SentBut503 => {
                        m.sends.push(b);
                        (StatusCode::SERVICE_UNAVAILABLE, Json(json!({})))
                    }
                    SendMode::Fail503 => (StatusCode::SERVICE_UNAVAILABLE, Json(json!({}))),
                    SendMode::Reject400 => (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid" }))),
                }
            }),
        )
        .route(
            "/proxy/gmail/v1/users/me/messages",
            get(|State(m): State<Shared>| async move {
                match m.lock().unwrap().search_hit.clone() {
                    Some(id) => Json(json!({ "messages": [{ "id": id }] })),
                    None => Json(json!({})),
                }
            }),
        )
        .route("/proxy/gmail/v1/users/me/threads/{id}", get(|Path(_id): Path<String>| async { Json(json!({ "messages": [] })) }))
        .with_state(mock);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

struct H {
    state: AppState,
    app: Router,
    mock: Shared,
    judge: Arc<FixedJudge>,
    ws: String,
    msg: String,
    _dir: tempfile::TempDir,
}

fn reviewing_judge() -> FixedJudge {
    FixedJudge::new()
        .with("promises", Verdict::YesNo(false), "")
        .with("professional", Verdict::YesNo(true), "")
        .with("answers_ask", Verdict::Score(0.9), "")
}

async fn harness(undo_seconds: i64) -> H {
    let dir = tempfile::tempdir().unwrap();
    let mock: Shared = Arc::new(Mutex::new(Mock { mode: SendMode::Ok, sends: vec![], search_hit: None }));
    let mut config = Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()));
    config.nango_api_url = spawn_gmail(mock.clone()).await;
    config.nango_api_key = "k".into();
    config.send_undo_seconds = undo_seconds;
    let mut state = build_state(config).await.unwrap();
    db::migrate(&state.db).await.unwrap();
    let judge = Arc::new(reviewing_judge());
    state.judge = judge.clone();
    let app = router(state.clone());
    let mut h = H { state, app, mock, judge, ws: String::new(), msg: String::new(), _dir: dir };

    let (_, ws) = h.call("POST", "/api/v1/workspaces", Some(json!({"name": "Acme", "template": "hr"}))).await;
    h.ws = ws["id"].as_str().unwrap().to_string();
    let (_, src) = h.call("POST", &format!("/api/v1/workspaces/{}/sources/forwarding", h.ws), None).await;
    let src = src["id"].as_str().unwrap().to_string();
    let (s, _) = h
        .call(
            "POST",
            &format!("/api/v1/workspaces/{}/sources/{src}/test-email", h.ws),
            Some(json!({"from": format!("Priya Sharma <{CLIENT}>"), "subject": "September hours",
                        "body_text": "Hi, my timesheet is attached. Can you confirm you got it? Call me on +91 98765 43210."})),
        )
        .await;
    assert_eq!(s, StatusCode::ACCEPTED);
    worker::drain(&h.state).await;
    let (_, list) = h.call("GET", &format!("/api/v1/workspaces/{}/messages?limit=1", h.ws), None).await;
    h.msg = list["items"][0]["id"].as_str().unwrap().to_string();
    // Make it a connected Gmail mailbox that can send.
    sqlx::query(
        "UPDATE email_sources SET kind = 'nango_gmail', external_connection_id = 'conn_t', provider_config_key = 'google-mail',
           capabilities_json = '{\"read\":true,\"sync\":true,\"provider_labels\":false,\"provider_drafts\":true,\"send\":true}'
         WHERE workspace_id = ?1",
    )
    .bind(&h.ws)
    .execute(&h.state.db)
    .await
    .unwrap();
    sqlx::query("UPDATE email_threads SET provider_thread_id = 'thr1' WHERE workspace_id = ?1").bind(&h.ws).execute(&h.state.db).await.unwrap();
    h
}

impl H {
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let req = Request::builder().method(method).uri(path).header("x-auth-token", ADMIN);
        let req = match body {
            Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
            None => req.body(Body::empty()).unwrap(),
        };
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn draft(&self, body: &str) -> String {
        let id = format!("drf_{}", uuid_like());
        sqlx::query(
            "INSERT INTO drafts (id, workspace_id, message_id, body_text, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 'edited', ?5, ?5)",
        )
        .bind(&id)
        .bind(&self.ws)
        .bind(&self.msg)
        .bind(body)
        .bind(now_ms())
        .execute(&self.state.db)
        .await
        .unwrap();
        id
    }

    async fn send(&self, draft: &str) -> (StatusCode, Value) {
        self.call("POST", &format!("/api/v1/workspaces/{}/drafts/{draft}/send", self.ws), Some(json!({"confirm": true}))).await
    }

    async fn undo(&self, draft: &str) -> (StatusCode, Value) {
        self.call("POST", &format!("/api/v1/workspaces/{}/drafts/{draft}/undo-send", self.ws), None).await
    }

    async fn check(&self, draft: &str, body: Option<Value>) -> Value {
        let (s, r) = self.call("POST", &format!("/api/v1/workspaces/{}/drafts/{draft}/check", self.ws), body).await;
        assert_eq!(s, StatusCode::OK, "{r}");
        r
    }

    async fn draft_row(&self, draft: &str) -> (String, Option<i64>, Option<String>, Option<String>, Option<String>) {
        sqlx::query_as("SELECT status, send_at, send_attempt_id, send_error, provider_message_id FROM drafts WHERE id = ?1")
            .bind(draft)
            .fetch_one(&self.state.db)
            .await
            .unwrap()
    }

    fn sends(&self) -> usize {
        self.mock.lock().unwrap().sends.len()
    }

    /// Makes every pending job due now (instead of waiting for backoff / the undo window).
    async fn run_due_now(&self) {
        sqlx::query("UPDATE jobs SET run_at = 0 WHERE status = 'pending'").execute(&self.state.db).await.unwrap();
        worker::drain(&self.state).await;
    }
}

fn uuid_like() -> String {
    static N: AtomicUsize = AtomicUsize::new(0);
    format!("{}_{}", now_ms(), N.fetch_add(1, Ordering::SeqCst))
}

fn codes(list: &Value) -> Vec<String> {
    list.as_array().unwrap().iter().map(|i| i["code"].as_str().unwrap().to_string()).collect()
}

#[tokio::test]
async fn check_reports_blocking_issues_and_warnings() {
    let h = harness(0).await;
    let clean = h.draft(GOOD).await;
    let r = h.check(&clean, None).await;
    assert_eq!((codes(&r["blocking"]), codes(&r["warnings"])), (vec![], vec![]), "{r}");

    let bad = h.draft("Dear {name},\n\nYour leave starts [NEEDS: start date]. Regards, [Your Name]").await;
    let r = h.check(&bad, None).await;
    assert!(codes(&r["blocking"]).contains(&"placeholder".to_string()) && codes(&r["blocking"]).contains(&"open_gaps".to_string()), "{r}");
    assert!(r["blocking"][0]["fix"].is_string());

    // Unsaved editor text is checked instead of the stored draft.
    let r = h
        .check(&clean, Some(json!({"body_text": format!("{GOOD}\nThe form is attached; see https://forms.example/x or call +91 98765 43210.")})))
        .await;
    assert_eq!(codes(&r["blocking"]), Vec::<String>::new(), "{r}");
    assert_eq!(codes(&r["warnings"]), ["attachment_mentioned", "unknown_contact"], "{r}");
    let unknown = r["warnings"][1]["message"].as_str().unwrap();
    assert!(unknown.contains("forms.example") && !unknown.contains("98765"), "the phone is in the original email: {unknown}");
}

#[tokio::test]
async fn blocked_sends_never_reach_gmail() {
    let h = harness(0).await;
    let blocked = |r: &Value, code: &str| {
        assert_eq!(r["error"]["code"], "send_blocked", "{r}");
        assert!(codes(&r["error"]["blocking"]).contains(&code.to_string()), "{code}: {r}");
    };
    let (s, r) = h.send(&h.draft("Dear {{first_name}}, thanks for the timesheet. We will process it soon.").await).await;
    assert_eq!(s, StatusCode::CONFLICT);
    blocked(&r, "placeholder");
    let (s, r) = h.send(&h.draft("Hi Priya, the payroll cut-off is [NEEDS: cut-off date]. Thanks!").await).await;
    assert_eq!(s, StatusCode::CONFLICT);
    blocked(&r, "open_gaps");

    // Wrong recipient: an automated address, then our own mailbox.
    let good = h.draft(GOOD).await;
    for (sender, code) in [("no-reply@client.example", "automated_recipient"), (MAILBOX, "own_mailbox")] {
        sqlx::query("UPDATE email_messages SET sender_address = ?1 WHERE id = ?2").bind(sender).bind(&h.msg).execute(&h.state.db).await.unwrap();
        let (s, r) = h.send(&good).await;
        assert_eq!(s, StatusCode::CONFLICT);
        blocked(&r, code);
    }
    sqlx::query("UPDATE email_messages SET sender_address = ?1 WHERE id = ?2").bind(CLIENT).bind(&h.msg).execute(&h.state.db).await.unwrap();

    // Duplicate: the first reply goes out; a second one in the same thread within 10 minutes is blocked.
    assert_eq!(h.send(&good).await.0, StatusCode::OK);
    let other = h.draft("Hello again Priya, just confirming we have your timesheet. Regards, HR").await;
    let (s, r) = h.send(&other).await;
    assert_eq!(s, StatusCode::CONFLICT);
    blocked(&r, "duplicate");
    worker::drain(&h.state).await;
    assert_eq!(h.sends(), 1);
    let (s, r) = h.send(&other).await;
    assert_eq!(s, StatusCode::CONFLICT);
    blocked(&r, "duplicate");
    assert_eq!(h.sends(), 1, "only the first reply was sent");
}

#[tokio::test]
async fn warning_only_send_goes_out_threaded() {
    let h = harness(0).await;
    let d = h.draft(&format!("{GOOD}\nPS: the attached policy explains the cut-off.")).await;
    let (s, r) = h.send(&d).await;
    assert_eq!((s, r["status"].as_str()), (StatusCode::OK, Some("scheduled")), "{r}");
    worker::drain(&h.state).await;
    let (status, send_at, attempt, err, gmsg) = h.draft_row(&d).await;
    assert_eq!((status.as_str(), send_at, err, gmsg.as_deref()), ("sent", None, None, Some("gmsg-1")));
    assert!(attempt.is_some(), "the attempt id stays as the record of what went out");
    let sent = h.mock.lock().unwrap().sends[0].clone();
    assert_eq!(sent["threadId"], "thr1");
    let raw = String::from_utf8(URL_SAFE_NO_PAD.decode(sent["raw"].as_str().unwrap()).unwrap()).unwrap();
    assert!(raw.contains("Subject: Re: September hours") && raw.contains(CLIENT) && raw.contains("@postroom.mail>"), "{raw}");
    let (_, detail) = h.call("GET", &format!("/api/v1/workspaces/{}/messages/{}", h.ws, h.msg), None).await;
    assert!(detail["audit"].as_array().unwrap().iter().any(|a| a["action"] == "draft.send_scheduled"));
}

#[tokio::test]
async fn judge_promise_blocks_and_missing_review_blocks_only_automated_sends() {
    let h = harness(0).await;
    h.judge.set("promises", Verdict::YesNo(true), "promises payment by Friday");
    let d = h.draft("Hello Priya,\n\nYour payment of 50,000 will be credited by Friday.\n\nRegards, HR").await;
    // A person's send is their decision: the finding is a warning, never a block.
    let r = h.check(&d, None).await;
    assert_eq!(codes(&r["warnings"]), ["unsupported_promise"], "{r}");
    assert!(r["blocking"].as_array().unwrap().is_empty());
    assert_eq!(h.judge.calls(), 1);
    // An automatic send still stops on it.
    let err = replies::send_draft(&h.state, &h.ws, &d, Actor::System).await.unwrap_err();
    match err {
        AppError::SendBlocked(_, details) => assert_eq!(codes(&details["blocking"]), ["unsupported_promise"]),
        other => panic!("{other:?}"),
    }

    // Judge down: a person still gets a warning and can send; an automated send is held.
    h.judge.fail(true);
    let d2 = h.draft(GOOD).await;
    let err = replies::send_draft(&h.state, &h.ws, &d2, Actor::System).await.unwrap_err();
    match err {
        AppError::SendBlocked(_, details) => assert_eq!(codes(&details["blocking"]), ["ai_review_unavailable"]),
        other => panic!("{other:?}"),
    }
    let r = h.check(&d2, None).await;
    assert_eq!(codes(&r["warnings"]), ["ai_review_unavailable"]);
    assert_eq!(h.send(&d2).await.0, StatusCode::OK);
}

#[tokio::test]
async fn scheduled_send_can_be_undone_then_fires_after_the_delay() {
    let h = harness(30).await;
    let d = h.draft(GOOD).await;
    let (s, r) = h.send(&d).await;
    assert_eq!((s, r["status"].as_str()), (StatusCode::OK, Some("scheduled")));
    let (_, detail) = h.call("GET", &format!("/api/v1/workspaces/{}/messages/{}", h.ws, h.msg), None).await;
    assert_eq!(detail["draft"]["status"], "scheduled", "{}", detail["draft"]);
    assert!(detail["draft"]["send_at"].is_string());
    // Not due yet; edits are refused while scheduled.
    worker::drain(&h.state).await;
    assert_eq!(h.sends(), 0);
    let (s, _) = h.call("PATCH", &format!("/api/v1/workspaces/{}/drafts/{d}", h.ws), Some(json!({"body_text": "changed text here"}))).await;
    assert_eq!(s, StatusCode::CONFLICT);

    let (s, r) = h.undo(&d).await;
    assert_eq!((s, r["status"].as_str()), (StatusCode::OK, Some("cancelled")));
    assert_eq!(h.undo(&d).await.0, StatusCode::CONFLICT, "nothing left to undo");
    h.run_due_now().await;
    assert_eq!(h.sends(), 0, "the undone send never fires");

    // Schedule again; once the delay passes the job sends exactly once, and undo is too late.
    assert_eq!(h.send(&d).await.0, StatusCode::OK);
    h.run_due_now().await;
    assert_eq!(h.sends(), 1);
    assert_eq!(h.draft_row(&d).await.0, "sent");
    let (s, r) = h.undo(&d).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert!(r["error"]["message"].as_str().unwrap().contains("already sent"));
}

#[tokio::test]
async fn ambiguous_gmail_error_is_reconciled_not_resent() {
    let h = harness(0).await;
    // Gmail sent it but the answer was a 503: the attempt stays open, nothing is retried blindly.
    h.mock.lock().unwrap().mode = SendMode::SentBut503;
    let d = h.draft(GOOD).await;
    assert_eq!(h.send(&d).await.0, StatusCode::OK);
    worker::drain(&h.state).await;
    let (status, _, attempt, _, _) = h.draft_row(&d).await;
    assert_eq!((status.as_str(), attempt.is_some()), ("edited", true));
    assert_eq!(h.undo(&d).await.0, StatusCode::CONFLICT, "too late to undo while sending");
    // The retry finds our Message-ID in Gmail and records it as sent.
    {
        let mut m = h.mock.lock().unwrap();
        m.mode = SendMode::Ok;
        m.search_hit = Some("gmsg-recovered".into());
    }
    h.run_due_now().await;
    let (status, _, _, _, gmsg) = h.draft_row(&d).await;
    assert_eq!((status.as_str(), gmsg.as_deref()), ("sent", Some("gmsg-recovered")));
    assert_eq!(h.sends(), 1, "never sent twice");
}

#[tokio::test]
async fn ambiguous_error_with_nothing_sent_retries_only_after_the_grace_period() {
    let h = harness(0).await;
    h.mock.lock().unwrap().mode = SendMode::Fail503;
    let d = h.draft(GOOD).await;
    assert_eq!(h.send(&d).await.0, StatusCode::OK);
    worker::drain(&h.state).await;
    h.mock.lock().unwrap().mode = SendMode::Ok;
    // Too soon to call it "not sent": still no new send.
    h.run_due_now().await;
    assert_eq!((h.sends(), h.draft_row(&d).await.2.is_some()), (0, true));
    // After the grace period Gmail still has nothing: one new attempt goes out.
    sqlx::query("UPDATE drafts SET send_started_at = send_started_at - 600000 WHERE id = ?1").bind(&d).execute(&h.state.db).await.unwrap();
    h.run_due_now().await;
    assert_eq!((h.sends(), h.draft_row(&d).await.0.as_str()), (1, "sent"));
}

#[tokio::test]
async fn clear_rejection_releases_the_attempt_and_alerts() {
    let h = harness(0).await;
    h.mock.lock().unwrap().mode = SendMode::Reject400;
    let d = h.draft(GOOD).await;
    assert_eq!(h.send(&d).await.0, StatusCode::OK);
    worker::drain(&h.state).await;
    let (status, send_at, attempt, err, _) = h.draft_row(&d).await;
    assert_eq!((status.as_str(), send_at, attempt), ("edited", None, None));
    assert!(err.unwrap().starts_with("Not sent"));
    let (_, list) = h.call("GET", &format!("/api/v1/workspaces/{}/alerts?status=open", h.ws), None).await;
    assert!(list["items"].as_array().unwrap().iter().any(|a| a["title"].as_str().unwrap().starts_with("Reply not sent")), "{list}");
    let (_, detail) = h.call("GET", &format!("/api/v1/workspaces/{}/messages/{}", h.ws, h.msg), None).await;
    assert_eq!((detail["draft"]["id"].as_str(), detail["draft"]["status"].as_str()), (Some(d.as_str()), Some("edited")));
    assert!(detail["draft"]["send_error"].as_str().unwrap().starts_with("Not sent"), "{}", detail["draft"]);
    // The person can fix things and send again.
    h.mock.lock().unwrap().mode = SendMode::Ok;
    assert_eq!(h.send(&d).await.0, StatusCode::OK);
    worker::drain(&h.state).await;
    assert_eq!(h.draft_row(&d).await.0, "sent");
}

#[tokio::test]
async fn judge_gates_important_unanswered_alerts() {
    let h = harness(0).await;
    let important = |r: &Value| r["items"].as_array().unwrap().iter().filter(|a| a["kind"] == "important_unanswered").count();
    // Make sure the email is a normal-priority "needs reply" candidate.
    sqlx::query("UPDATE classifications SET priority = 'normal', requires_reply = 1, category = 'timesheet' WHERE message_id = ?1")
        .bind(&h.msg)
        .execute(&h.state.db)
        .await
        .unwrap();
    sqlx::query("UPDATE message_insights SET needs_reply = 1 WHERE message_id = ?1").bind(&h.msg).execute(&h.state.db).await.unwrap();
    h.judge.set("important", Verdict::YesNo(false), "routine confirmation");
    let later = now_ms() + 5 * 3_600_000;
    alerts::evaluate_workspace(&h.state, &h.ws, later).await.unwrap();
    alerts::evaluate_workspace(&h.state, &h.ws, later + 1_000).await.unwrap();
    let (_, list) = h.call("GET", &format!("/api/v1/workspaces/{}/alerts?status=open", h.ws), None).await;
    assert_eq!(important(&list), 0, "{list}");
    assert_eq!(h.judge.calls(), 1, "the verdict is cached between runs");
    // A different verdict for new material raises it; high priority always alerts.
    sqlx::query("UPDATE classifications SET priority = 'high' WHERE message_id = ?1").bind(&h.msg).execute(&h.state.db).await.unwrap();
    alerts::evaluate_workspace(&h.state, &h.ws, later + 2_000).await.unwrap();
    let (_, list) = h.call("GET", &format!("/api/v1/workspaces/{}/alerts?status=open", h.ws), None).await;
    assert_eq!(important(&list), 1);
}

// ---------------------------------------------------------------- transactional email

struct Flaky {
    fail_first: usize,
    calls: AtomicUsize,
}

#[async_trait]
impl Mailer for Flaky {
    async fn send(&self, _to: &str, _s: &str, _t: &str, _h: &str) -> anyhow::Result<()> {
        if self.calls.fetch_add(1, Ordering::SeqCst) < self.fail_first { anyhow::bail!("421 try later") } else { Ok(()) }
    }
}

async fn auth_app(fail_first: usize) -> (Router, AppState, Arc<Flaky>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()));
    config.auth_mode = AuthMode::Password;
    let mut state = build_state(config).await.unwrap();
    db::migrate(&state.db).await.unwrap();
    let mailer = Arc::new(Flaky { fail_first, calls: AtomicUsize::new(0) });
    state.mailer = Some(mailer.clone());
    (router(state.clone()), state, mailer, dir)
}

async fn post_json(app: &Router, path: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::builder().method("POST").uri(path).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn otp_email_is_retried_and_a_failure_is_surfaced() {
    const PW: &str = "correct horse battery";
    // Two SMTP failures, then success: the user never notices.
    let (app, state, mailer, _d) = auth_app(2).await;
    let (s, _) = post_json(&app, "/api/v1/auth/signup", json!({"email": "meera@acme.com", "password": PW})).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    assert_eq!(mailer.calls.load(Ordering::SeqCst), 3);
    let row: (String, i64, String) = sqlx::query_as("SELECT status, attempts, subject FROM email_outbox WHERE kind = 'otp'").fetch_one(&state.db).await.unwrap();
    assert_eq!(row, ("sent".into(), 3, "Verification code".into()), "the code itself is never stored");

    // SMTP down: the sign-in screen gets a clear error instead of waiting for a code.
    let (app, state, _mailer, _d) = auth_app(usize::MAX).await;
    let (s, err) = post_json(&app, "/api/v1/auth/signup", json!({"email": "ravi@acme.com", "password": PW})).await;
    assert_eq!((s, err["error"]["code"].as_str()), (StatusCode::SERVICE_UNAVAILABLE, Some("email_not_sent")), "{err}");
    assert!(err["error"]["message"].as_str().unwrap().contains("ravi@acme.com"));
    let (status, attempts): (String, i64) = sqlx::query_as("SELECT status, attempts FROM email_outbox").fetch_one(&state.db).await.unwrap();
    assert_eq!((status.as_str(), attempts), ("failed", 3));
    // The undelivered code doesn't throttle "resend": the next request tries again right away.
    let (s, _) = post_json(&app, "/api/v1/auth/resend-code", json!({"email": "ravi@acme.com"})).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM email_outbox").fetch_one(&state.db).await.unwrap(), 2);
}

// ---------------------------------------------------------------- leak check inputs

async fn automated_codes(h: &H, body: &str) -> Vec<String> {
    let d = h.draft(body).await;
    match replies::send_draft(&h.state, &h.ws, &d, Actor::System).await {
        Err(AppError::SendBlocked(_, details)) => codes(&details["blocking"]),
        _ => vec![],
    }
}

fn leaks(c: &[String]) -> bool {
    c.iter().any(|c| c == "possible_leak")
}

#[tokio::test]
async fn the_leak_check_knows_what_the_drafter_was_given() {
    let h = harness(0).await;
    email_ai_api::replies::reply_settings::save(
        &h.state.db,
        &h.ws,
        &email_ai_api::replies::reply_settings::ReplySettings {
            auto_reply: false,
            reply_instructions: String::new(),
            signature: Some("HR Operations, call +91 80 4567 8901, https://hr.acme.test/help".into()),
        },
    )
    .await
    .unwrap();
    sqlx::query("UPDATE email_messages SET recipients_json = ?1, body_text = body_text || ' See https://docs.client.test/file and invoice INV-77.' WHERE id = ?2")
        .bind(r#"["meena@acme.test"]"#)
        .bind(&h.msg)
        .execute(&h.state.db)
        .await
        .unwrap();

    let signed = format!("{GOOD}\nCall +91 80 4567 8901 or see https://hr.acme.test/help");
    assert!(!leaks(&automated_codes(&h, &signed).await), "a signature phone and link are the drafter's own");
    let cc = format!("{GOOD}\nI have copied meena@acme.test.");
    assert!(!leaks(&automated_codes(&h, &cc).await), "a colleague in To/Cc is known");
    let tracked = format!("{GOOD}\nThe file is at https://docs.client.test/file?utm_source=mail#top");
    assert!(!leaks(&automated_codes(&h, &tracked).await), "query strings and fragments are ignored");
    let total = format!("{GOOD}\nThe invoice total is 12345678 (₹1,25,00,000) for INV-77.");
    assert!(!leaks(&automated_codes(&h, &total).await), "short digit runs are amounts, not phone numbers");

    for foreign in [
        "Her number is +44 20 7946 0958",
        "Write to cfo@elsewhere.org",
        "See https://other-tenant.example/secret",
        "Call 9812345678 for details",
    ] {
        let c = automated_codes(&h, &format!("{GOOD}\n{foreign}")).await;
        assert!(leaks(&c), "{foreign}: {c:?}");
    }
}

#[tokio::test]
async fn a_reviewer_outage_blocking_ten_sends_demotes_nobody() {
    let h = harness(0).await;
    let agent: String = sqlx::query_scalar("SELECT id FROM agents WHERE workspace_id = ?1 LIMIT 1").bind(&h.ws).fetch_one(&h.state.db).await.unwrap();
    sqlx::query("UPDATE agents SET autonomy_level = 2, autonomy_cap = 3 WHERE id = ?1").bind(&agent).execute(&h.state.db).await.unwrap();
    h.judge.fail(true);
    for _ in 0..10 {
        let d = h.draft(GOOD).await;
        sqlx::query("UPDATE drafts SET agent_id = ?1 WHERE id = ?2").bind(&agent).bind(&d).execute(&h.state.db).await.unwrap();
        assert!(replies::send_draft(&h.state, &h.ws, &d, Actor::System).await.is_err());
    }
    let level: i64 = sqlx::query_scalar("SELECT autonomy_level FROM agents WHERE id = ?1").bind(&agent).fetch_one(&h.state.db).await.unwrap();
    assert_eq!(level, 2);
    let (n, high): (i64, i64) = sqlx::query_as("SELECT COUNT(*), COALESCE(SUM(severity = 'high'), 0) FROM ai_incidents WHERE kind = 'send_blocked'")
        .fetch_one(&h.state.db)
        .await
        .unwrap();
    assert_eq!((n, high), (10, 0));
}

#[tokio::test]
async fn the_judge_is_told_what_the_owner_said_and_which_roles_are_open() {
    let h = harness(0).await;
    let d = h.draft("Hello Ananth,\n\nA UK company is hiring React Native developers, and your 2 years fit well.\n\nRegards, HR").await;
    let me: String = sqlx::query_scalar("SELECT id FROM users WHERE email = 'admin@acme.test'").fetch_one(&h.state.db).await.unwrap();
    owner_said(&h, &d, "bol do UK wali company hiring kar rahi hai", &me).await;
    sqlx::query(
        "INSERT INTO requirements (id, workspace_id, message_id, client, role_title, location, status, source, created_at)
         VALUES ('req_x', ?1, ?2, 'Acme UK', 'React Native Developer', 'London', 'open', 'manual', ?3)",
    )
    .bind(&h.ws)
    .bind(&h.msg)
    .bind(now_ms())
    .execute(&h.state.db)
    .await
    .unwrap();
    h.check(&d, None).await;
    let context = h.judge.last_context();
    assert!(context.contains("The owner said") && context.contains("UK wali company"), "{context}");
    assert!(context.contains("React Native Developer (London)"), "{context}");
}

async fn open_role(h: &H, title: &str, location: &str) {
    sqlx::query(
        "INSERT INTO requirements (id, workspace_id, message_id, client, role_title, location, status, source, created_at)
         VALUES ('req_r', ?1, ?2, 'Acme UK', ?3, ?4, 'open', 'manual', ?5)",
    )
    .bind(&h.ws)
    .bind(&h.msg)
    .bind(title)
    .bind(location)
    .bind(now_ms())
    .execute(&h.state.db)
    .await
    .unwrap();
}

const ROLE_REPLY: &str = "Hello Priya,\n\nThanks for your note. We have a React Native Developer role open in London that fits your background.\n\nIf you are interested, reply and we will take it forward.\n\nBest regards,\nHR";

#[tokio::test]
async fn an_automatic_send_naming_a_real_open_role_goes_out_untouched() {
    let h = harness(0).await;
    open_role(&h, "React Native Developer", "London").await;
    h.judge.flag_unless_noted("React Native Developer", "It promises a role that the notes do not support");
    let d = h.draft(ROLE_REPLY).await;
    replies::send_draft(&h.state, &h.ws, &d, Actor::System).await.unwrap();
    let body: String = sqlx::query_scalar("SELECT body_text FROM drafts WHERE id = ?1").bind(&d).fetch_one(&h.state.db).await.unwrap();
    assert_eq!(body, ROLE_REPLY, "nothing was repaired");
}

#[tokio::test]
async fn an_invented_company_is_repaired_once_then_sent() {
    let h = harness(0).await;
    h.judge.flag_unless_noted("Zeta Corp", "It promises a hiring company Zeta Corp and a fit that are not supported");
    let invented = "Hello Priya,\n\nThanks for your note.\n\nZeta Corp is hiring and your profile is a fit for Zeta Corp.\n\nIf you are interested, reply and we will take it forward.\n\nBest regards,\nHR";
    let d = h.draft(invented).await;
    replies::send_draft(&h.state, &h.ws, &d, Actor::System).await.unwrap();
    let (body, unanswered): (String, String) = sqlx::query_as("SELECT body_text, unanswered_json FROM drafts WHERE id = ?1").bind(&d).fetch_one(&h.state.db).await.unwrap();
    assert!(!body.contains("Zeta") && body.contains("We'll confirm the details shortly.") && body.contains("Thanks for your note."), "{body}");
    assert!(unanswered.contains("details"), "{unanswered}");
    let (n, severity): (i64, String) = sqlx::query_as("SELECT COUNT(*), MAX(severity) FROM ai_incidents WHERE kind = 'unsupported_claim'").fetch_one(&h.state.db).await.unwrap();
    assert_eq!((n, severity.as_str()), (1, "low"));
    let tasks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM review_items WHERE kind = 'agent_task'").fetch_one(&h.state.db).await.unwrap();
    assert_eq!(tasks, 0);
}

#[tokio::test]
async fn a_draft_that_cannot_be_repaired_creates_exactly_one_review_task() {
    let h = harness(0).await;
    h.judge.flag_unless_noted("guarantee", "odd");
    let d = h.draft("Hello Priya,\n\nThanks for your note.\n\nWe guarantee a quick outcome for you.\n\nBest regards,\nHR").await;
    for _ in 0..2 {
        let err = replies::send_draft(&h.state, &h.ws, &d, Actor::System).await.unwrap_err();
        assert!(matches!(err, AppError::SendBlocked(..)), "{err:?}");
    }
    let tasks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM review_items WHERE kind = 'agent_task'").fetch_one(&h.state.db).await.unwrap();
    assert_eq!(tasks, 1, "one task, no retry loop");
    let body: String = sqlx::query_scalar("SELECT body_text FROM drafts WHERE id = ?1").bind(&d).fetch_one(&h.state.db).await.unwrap();
    assert!(body.contains("guarantee"), "left untouched");

    let money = h.draft("Hello Sam,\n\nThanks.\n\nYour payment of 50,000 will arrive on Friday for the project.\n\nBest regards,\nHR").await;
    h.judge.flag_unless_noted("payment", "It promises payment of 50,000 on Friday for the project");
    assert!(replies::send_draft(&h.state, &h.ws, &money, Actor::System).await.is_err());
    let body: String = sqlx::query_scalar("SELECT body_text FROM drafts WHERE id = ?1").bind(&money).fetch_one(&h.state.db).await.unwrap();
    assert!(body.contains("50,000"), "money claims are never auto-repaired");
}

// ---------------------------------------------------------------- the owner's own words are facts

const UK: &str = "Hello Priya,\n\nThanks for your note.\n\nA UK company is hiring React Native developers and you could be a good fit.\n\nBest regards,\nHR";

async fn admin_id(h: &H) -> String {
    sqlx::query_scalar("SELECT id FROM users WHERE email = 'admin@acme.test'").fetch_one(&h.state.db).await.unwrap()
}

async fn owner_said(h: &H, draft: &str, text: &str, author: &str) {
    sqlx::query("INSERT INTO draft_owner_words (workspace_id, draft_id, user_id, text, created_at) VALUES (?1, ?2, ?3, ?4, ?5)").bind(&h.ws).bind(draft).bind(author).bind(text).bind(now_ms()).execute(&h.state.db).await.unwrap();
}

fn warned(report: &Value, code: &str) -> bool {
    report["warnings"].as_array().unwrap().iter().any(|w| w["code"] == code)
}

#[tokio::test]
async fn what_the_owner_told_meera_supports_the_claim_and_an_invented_one_is_flagged() {
    let h = harness(0).await;
    h.judge.flag_unless_noted("UK company", "It promises a hiring company and fit that are not supported");
    let me = admin_id(&h).await;
    let d = h.draft(UK).await;
    assert!(warned(&h.check(&d, None).await, "unsupported_promise"), "no owner words and no data: flagged");
    owner_said(&h, &d, "tell Priya a UK company is hiring React Native devs", &me).await;
    let report = h.check(&d, None).await;
    assert!(report["warnings"].as_array().unwrap().is_empty() && report["blocking"].as_array().unwrap().is_empty(), "{report}");
    replies::send_draft(&h.state, &h.ws, &d, Actor::System).await.expect("an automatic send that follows the owner's request passes");
}

#[tokio::test]
async fn an_earlier_meera_turn_counts_when_it_is_about_this_email_and_only_the_persons_own_turns() {
    let h = harness(0).await;
    let me = admin_id(&h).await;
    sqlx::query("INSERT INTO users (id, external_auth_subject, email, created_at) VALUES ('usr_other', 'dev:other@acme.test', 'other@acme.test', 1)").execute(&h.state.db).await.unwrap();
    let long_ago = now_ms() - 30 * 86_400_000;
    let turns = [
        (me.as_str(), "FYI a UK company is hiring for the September timesheet crowd", long_ago),
        (me.as_str(), "what is the weather on Friday", long_ago),
        ("usr_other", "secret: Zeta Corp pays double", now_ms()),
        (me.as_str(), "reply to Priya please", now_ms()),
    ];
    for (user, text, at) in turns {
        sqlx::query("INSERT INTO assistant_messages (workspace_id, user_id, role, text, citations_json, plan_json, created_at, links_json) VALUES (?1, ?2, 'user', ?3, '[]', '[]', ?4, '[]')")
            .bind(&h.ws).bind(user).bind(text).bind(at).execute(&h.state.db).await.unwrap();
    }
    let said = email_ai_api::replies::owner_said::conversation(&h.state.db, &h.ws, &me, &h.msg).await.unwrap();
    assert!(said.contains("UK company is hiring") && said.contains("reply to Priya"), "an older turn about this email (it names the subject) and a turn after it arrived: {said}");
    assert!(!said.contains("weather"), "an old turn about something else is not evidence for this email: {said}");
    assert!(!said.contains("Zeta"), "another member's words are not mine: {said}");
}

#[tokio::test]
async fn the_owners_edit_is_the_source_and_the_check_is_fresh() {
    let h = harness(0).await;
    h.judge.flag_unless_noted("UK company", "It promises a hiring company that is not supported");
    let d = h.draft("Hello Priya,\n\nThanks for your note.\n\nBest regards,\nHR").await;
    assert!(h.check(&d, None).await["warnings"].as_array().unwrap().is_empty());
    // Unsaved edit in the editor: judged on the edited content, with the typed sentence as the owner's own.
    let unsaved = h.check(&d, Some(json!({"body_text": UK}))).await;
    assert!(!warned(&unsaved, "unsupported_promise"), "{unsaved}");
    // Saved edit: recorded with its author.
    let (s, _) = h.call("PATCH", &format!("/api/v1/workspaces/{}/drafts/{d}", h.ws), Some(json!({"body_text": UK}))).await;
    assert_eq!(s, StatusCode::OK);
    let (said, author): (String, String) = sqlx::query_as("SELECT text, user_id FROM draft_owner_words WHERE draft_id = ?1").bind(&d).fetch_one(&h.state.db).await.unwrap();
    assert!(said.contains("UK company is hiring") && !author.is_empty());
    assert!(!warned(&h.check(&d, None).await, "unsupported_promise"));
}

#[tokio::test]
async fn standing_info_counts_but_suggestions_do_not() {
    let h = harness(0).await;
    h.judge.flag_unless_noted("Zeta Corp", "It promises a hiring company that is not supported");
    let zeta = "Hello Priya,\n\nThanks for your note.\n\nZeta Corp is hiring and could suit you.\n\nBest regards,\nHR";
    let d = h.draft(zeta).await;
    assert!(warned(&h.check(&d, None).await, "unsupported_promise"));
    let pref = |status: &'static str, key: &'static str| {
        let (db, ws) = (h.state.db.clone(), h.ws.clone());
        async move {
            sqlx::query("INSERT INTO agent_preferences (id, workspace_id, category, content, status, source, dedupe_key, created_at, updated_at) VALUES (?1, ?2, 'process', 'We often place people at Zeta Corp', ?3, 'manual', ?4, 1, 1)")
                .bind(format!("pref_{key}")).bind(ws).bind(status).bind(key).execute(&db).await.unwrap();
        }
    };
    pref("suggested", "a").await;
    assert!(warned(&h.check(&d, None).await, "unsupported_promise"), "a suggestion is not a fact yet");
    sqlx::query("UPDATE agent_preferences SET status = 'active'").execute(&h.state.db).await.unwrap();
    assert!(!warned(&h.check(&d, None).await, "unsupported_promise"), "an accepted preference is");
    // Workspace team notes are shared.
    let d2 = h.draft(&zeta.replace("Zeta Corp", "Zeta Corp ")).await;
    sqlx::query("DELETE FROM agent_preferences").execute(&h.state.db).await.unwrap();
    sqlx::query("INSERT INTO memories (id, workspace_id, scope, kind, content, status, source, created_at, updated_at) VALUES ('mem_z', ?1, 'workspace', 'fact', 'Zeta Corp is a hiring partner', 'active', 'user', 1, 1)")
        .bind(&h.ws).execute(&h.state.db).await.unwrap();
    assert!(!warned(&h.check(&d2, None).await, "unsupported_promise"));
}

#[tokio::test]
async fn the_owner_wins_over_the_data_with_a_one_line_note() {
    let h = harness(0).await;
    let me = admin_id(&h).await;
    h.judge.set("contradicts_data", Verdict::YesNo(true), "Your note says Pune; the requirement says Mumbai");
    let d = h.draft(UK).await;
    owner_said(&h, &d, "the role is in Pune", &me).await;
    let report = h.check(&d, None).await;
    assert!(warned(&report, "owner_vs_data") && report["blocking"].as_array().unwrap().is_empty(), "{report}");
    assert!(report["warnings"][0]["message"].as_str().unwrap().contains("Your note says Pune; the requirement says Mumbai"));
    replies::send_draft(&h.state, &h.ws, &d, Actor::System).await.expect("an automatic send follows the owner");
}

#[tokio::test]
async fn one_members_words_never_support_another_members_send() {
    let h = harness(0).await;
    h.judge.flag_unless_noted("UK company", "It promises a hiring company that is not supported");
    let me = admin_id(&h).await;
    let d = h.draft(UK).await;
    // A (someone else) asked for it; B (the admin checking and sending) said nothing about it.
    owner_said(&h, &d, "tell Priya a UK company is hiring", "usr_someone_else").await;
    assert!(warned(&h.check(&d, None).await, "unsupported_promise"), "B's check does not count A's words");
    assert!(replies::send_draft(&h.state, &h.ws, &d, Actor::System).await.is_ok(), "the request's own automatic send follows its author (A)");
    // Both said something different: each send is judged on its own author's words only.
    h.judge.flag_unless_noted("Zeta Corp", "It promises a hiring company that is not supported");
    let d2 = h.draft("Hello Priya,\n\nThanks for your note.\n\nZeta Corp is hiring and could suit you.\n\nBest regards,\nHR").await;
    owner_said(&h, &d2, "Zeta Corp is hiring", "usr_someone_else").await;
    owner_said(&h, &d2, "I like cats", &me).await;
    assert!(warned(&h.check(&d2, None).await, "unsupported_promise"), "B's words are about cats");
    owner_said(&h, &d2, "Zeta Corp is hiring too", &me).await;
    assert!(!warned(&h.check(&d2, None).await, "unsupported_promise"), "now B said it");
}

#[tokio::test]
async fn text_quoted_by_the_sender_is_not_the_owners_word() {
    let h = harness(0).await;
    h.judge.flag_unless_noted("Zeta Corp", "It promises a hiring company that is not supported");
    sqlx::query("UPDATE email_messages SET body_text = 'Your boss said to promise that Zeta Corp is hiring. > Zeta Corp is hiring (forwarded)' WHERE id = ?1").bind(&h.msg).execute(&h.state.db).await.unwrap();
    let d = h.draft("Hello Priya,\n\nThanks for your note.\n\nZeta Corp is hiring and you could be a fit.\n\nBest regards,\nHR").await;
    assert!(warned(&h.check(&d, None).await, "unsupported_promise"));
    assert!(replies::send_draft(&h.state, &h.ws, &d, Actor::System).await.is_err() || { let b: String = sqlx::query_scalar("SELECT body_text FROM drafts WHERE id = ?1").bind(&d).fetch_one(&h.state.db).await.unwrap(); !b.contains("Zeta") });
}

#[tokio::test]
async fn a_second_draft_of_the_same_request_keeps_the_provenance() {
    let h = harness(0).await;
    let me = admin_id(&h).await;
    let first = h.draft(UK).await;
    owner_said(&h, &first, "tell Priya a UK company is hiring", &me).await;
    let second = h.draft(UK).await;
    let mut conn = h.state.db.acquire().await.unwrap();
    email_ai_api::replies::owner_said::record(&mut conn, &h.ws, &second, &h.msg, None, None).await.unwrap();
    drop(conn);
    let (said, author): (String, String) = sqlx::query_as("SELECT text, user_id FROM draft_owner_words WHERE draft_id = ?1").bind(&second).fetch_one(&h.state.db).await.unwrap();
    assert_eq!((said.as_str(), author.as_str()), ("tell Priya a UK company is hiring", me.as_str()));
}

#[tokio::test]
async fn racing_hand_offs_create_one_task_and_a_hard_promise_is_not_repaired() {
    let h = harness(0).await;
    let d = h.draft("Hello Priya,\n\nThanks for your note.\n\nWe guarantee a quick outcome for you.\n\nBest regards,\nHR").await;
    let ctx = replies::load(&h.state, &h.ws, &d).await.unwrap();
    let report = email_ai_api::replies::send_safety::SafetyReport {
        blocking: vec![email_ai_api::replies::send_safety::Issue { code: "unsupported_promise", message: "may promise (x)".into(), fix: None }],
        warnings: vec![],
    };
    let (a, b, c) = tokio::join!(
        email_ai_api::replies::repair::hand_off(&h.state, &h.ws, &ctx, &report),
        email_ai_api::replies::repair::hand_off(&h.state, &h.ws, &ctx, &report),
        email_ai_api::replies::repair::hand_off(&h.state, &h.ws, &ctx, &report)
    );
    assert!(a.is_ok() && b.is_ok() && c.is_ok());
    let tasks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM review_items WHERE kind = 'agent_task'").fetch_one(&h.state.db).await.unwrap();
    assert_eq!(tasks, 1);

    // A draft that already promises to confirm a hard fact is not made sendable by a repair; soft topics merge.
    h.judge.flag_unless_noted("Zeta Corp", "It promises a hiring company Zeta Corp and a profile fit");
    let body = "Hello Priya,\n\nThanks for your note about the timesheet.\n\nZeta Corp is hiring and your profile is a fit for Zeta Corp.\n\nIf you are interested, reply to this email.\n\nBest regards,\nHR";
    let hard = h.draft(body).await;
    sqlx::query("UPDATE drafts SET unanswered_json = '[\"invoice amount\"]' WHERE id = ?1").bind(&hard).execute(&h.state.db).await.unwrap();
    assert!(replies::send_draft(&h.state, &h.ws, &hard, Actor::System).await.is_err(), "a hard 'we'll confirm' promise stays with a person");
    let soft = h.draft(body).await;
    sqlx::query("UPDATE drafts SET unanswered_json = '[\"office hours\"]' WHERE id = ?1").bind(&soft).execute(&h.state.db).await.unwrap();
    replies::send_draft(&h.state, &h.ws, &soft, Actor::System).await.unwrap();
    let merged: String = sqlx::query_scalar("SELECT unanswered_json FROM drafts WHERE id = ?1").bind(&soft).fetch_one(&h.state.db).await.unwrap();
    assert!(merged.contains("office hours") && merged.contains("details"), "{merged}");
}
