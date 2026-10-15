//! Alerts engine: rules fire after time passes (injected `now`), idempotent raise, batched email delivery
//! (recording double, no real email), escalation, auto-resolve, snooze, settings validation, test email, digest.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use email_ai_api::{
    alerts::{self, NewAlert},
    build_state,
    platform::config::Config,
    platform::db::{self, now_ms},
    http::{AppState, router},
    jobs::worker,
    platform::mailer::Mailer,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const ADMIN: &str = "dev:admin@acme.test";
const OPERATOR: &str = "dev:op@acme.test";
const HOUR: i64 = 3_600_000;

#[derive(Default)]
struct Recorder {
    sent: Mutex<Vec<(String, String, String, String)>>,
}

#[async_trait]
impl Mailer for Recorder {
    async fn send(&self, to: &str, subject: &str, text: &str, html: &str) -> anyhow::Result<()> {
        self.sent.lock().unwrap().push((to.into(), subject.into(), text.into(), html.into()));
        Ok(())
    }
}

impl Recorder {
    fn take(&self) -> Vec<(String, String, String, String)> {
        std::mem::take(&mut *self.sent.lock().unwrap())
    }
}

struct H {
    state: AppState,
    app: Router,
    mail: Arc<Recorder>,
    _dir: tempfile::TempDir,
}

async fn harness() -> H {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()));
    let mut state = build_state(config).await.unwrap();
    db::migrate(&state.db).await.unwrap();
    let mail = Arc::new(Recorder::default());
    state.alert_mailer = Some(mail.clone());
    H { app: router(state.clone()), state, mail, _dir: dir }
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

    /// Alert recipients must be verified members: dev identities sign in once, then join.
    async fn add_members(&self, ws: &str, emails: &[&str]) {
        for email in emails {
            self.call("GET", "/api/v1/me", &format!("dev:{email}"), None).await;
            let (s, _) = self
                .call("POST", &format!("/api/v1/workspaces/{ws}/members"), ADMIN, Some(json!({"email": email, "role": "operator"})))
                .await;
            assert_eq!(s, StatusCode::OK, "{email}");
        }
    }

    async fn workspace(&self) -> (String, String) {
        let (_, ws) = self.call("POST", "/api/v1/workspaces", ADMIN, Some(json!({"name":"Acme","template":"hr"}))).await;
        let ws = ws["id"].as_str().unwrap().to_string();
        let (_, src) = self.call("POST", &format!("/api/v1/workspaces/{ws}/sources/forwarding"), ADMIN, None).await;
        (ws, src["id"].as_str().unwrap().to_string())
    }

    async fn email(&self, ws: &str, src: &str, subject: &str, body: &str) -> String {
        let (s, _) = self
            .call("POST", &format!("/api/v1/workspaces/{ws}/sources/{src}/test-email"), ADMIN,
                Some(json!({"from": "Priya Sharma <priya.sharma@example.com>", "subject": subject, "body_text": body})))
            .await;
        assert_eq!(s, StatusCode::ACCEPTED);
        worker::drain(&self.state).await;
        let (_, list) = self.call("GET", &format!("/api/v1/workspaces/{ws}/messages?limit=1"), ADMIN, None).await;
        list["items"][0]["id"].as_str().unwrap().to_string()
    }

    async fn alerts(&self, ws: &str, status: &str) -> Value {
        self.call("GET", &format!("/api/v1/workspaces/{ws}/alerts?status={status}"), ADMIN, None).await.1
    }

    async fn eval(&self, ws: &str, now: i64) -> alerts::EvalReport {
        alerts::evaluate_workspace(&self.state, ws, now).await.unwrap()
    }
}

fn alert_for<'a>(list: &'a Value, message_id: &str) -> &'a Value {
    list["items"].as_array().unwrap().iter().find(|a| a["message_id"] == message_id).unwrap_or_else(|| panic!("{list}"))
}

/// A reply from the team in the same thread (as Gmail sync would store a sent message).
async fn team_reply(h: &H, message_id: &str) {
    sqlx::query(
        "INSERT INTO email_messages (id, workspace_id, source_id, thread_id, source_message_id, sender_address, subject,
                                     body_text, received_at, created_at, processing_status)
         SELECT 'msg_reply_' || id, workspace_id, source_id, thread_id, 'reply-' || id, 'admin@acme.test', 'Re: ' || subject,
                'Thanks, done.', received_at + 1000, received_at + 1000, 'classified'
         FROM email_messages WHERE id = ?1",
    )
    .bind(message_id)
    .execute(&h.state.db)
    .await
    .unwrap();
}

#[tokio::test]
async fn rules_raise_batch_escalate_and_auto_resolve() {
    let h = harness().await;
    let (ws, src) = h.workspace().await;
    let base = format!("/api/v1/workspaces/{ws}");
    let t0 = now_ms();
    let timesheet = h.email(&ws, &src, "September timesheet", "Hi, my timesheet is attached. Can you confirm you got it?").await;
    let leave = h.email(&ws, &src, "Leave request for next week", "Hi, I would like annual leave from Monday to Wednesday.").await;

    // Defaults: enabled, admins, 4h, 24h, digest at 9 UTC.
    let (s, settings) = h.call("GET", &format!("{base}/alert-settings"), ADMIN, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(settings, json!({"enabled": true, "recipients": ["admin@acme.test"], "escalate_to": [], "unanswered_hours": 4,
        "escalate_after_hours": 24, "daily_digest": true, "digest_hour": 9}));
    // Escalations go to the boss; digest off so the injected clock can't land in the digest hour.
    h.add_members(&ws, &["boss@acme.test"]).await;
    let (s, _) = h
        .call("PUT", &format!("{base}/alert-settings"), ADMIN, Some(json!({"enabled": true, "recipients": ["admin@acme.test"],
            "escalate_to": ["Boss@Acme.test"], "unanswered_hours": 4, "escalate_after_hours": 24, "daily_digest": false, "digest_hour": 9})))
        .await;
    assert_eq!(s, StatusCode::OK);

    // Too fresh: nothing fires.
    let r = h.eval(&ws, t0 + HOUR).await;
    assert_eq!(r.raised, 0);
    assert!(h.mail.take().is_empty());

    // After unanswered_hours: one alert per message, one batched email per recipient.
    let at = t0 + 5 * HOUR;
    let r = h.eval(&ws, at).await;
    assert_eq!((r.raised, r.emails_sent), (2, 1));
    let sent = h.mail.take();
    assert_eq!(sent.len(), 1);
    let (to, subject, text, html) = &sent[0];
    assert_eq!((to.as_str(), subject.as_str()), ("admin@acme.test", "[Post Room] 2 emails need attention"));
    assert!(text.contains(&format!("https://mail.anukulraj.tech/w/{ws}/inbox/{timesheet}")), "{text}");
    assert!(html.contains(&format!("https://mail.anukulraj.tech/w/{ws}/inbox/{leave}")), "{html}");

    let list = h.alerts(&ws, "open").await;
    assert_eq!(list["open_count"], 2);
    let a = alert_for(&list, &timesheet);
    assert_eq!((a["kind"].as_str(), a["status"].as_str(), a["escalation_level"].as_i64()), (Some("important_unanswered"), Some("open"), Some(1)));
    assert_eq!(a["notified"], json!(["admin@acme.test"]));
    assert_eq!(a["message_subject"], "September timesheet");
    assert_eq!(alert_for(&list, &leave)["kind"], "approval_pending", "approval wins over important for the same email");

    // Idempotent: same run again raises and sends nothing.
    let r = h.eval(&ws, at + 60_000).await;
    assert_eq!((r.raised, r.emails_sent), (0, 0));

    // Escalation goes to escalate_to after escalate_after_hours.
    let r = h.eval(&ws, at + 25 * HOUR).await;
    assert_eq!((r.escalated, r.emails_sent), (2, 1));
    let sent = h.mail.take();
    assert_eq!(sent[0].0, "boss@acme.test");
    assert!(sent[0].2.contains("Escalated: No reply yet: September timesheet"), "{}", sent[0].2);
    let list = h.alerts(&ws, "open").await;
    let a = alert_for(&list, &timesheet);
    assert_eq!((a["escalation_level"].as_i64(), a["severity"].as_str()), (Some(2), Some("high")));
    assert_eq!(a["notified"], json!(["admin@acme.test", "boss@acme.test"]));

    // Auto-resolve: a team reply in the thread clears "unanswered"; closing the review clears "approval".
    team_reply(&h, &timesheet).await;
    let (_, reviews) = h.call("GET", &format!("{base}/reviews?status=open"), ADMIN, None).await;
    let review = reviews["items"].as_array().unwrap().iter().find(|r| r["message_id"] == leave.as_str()).unwrap();
    let rid = review["id"].as_str().unwrap();
    h.call("PATCH", &format!("{base}/reviews/{rid}"), ADMIN, Some(json!({"status": "closed"}))).await;
    let r = h.eval(&ws, at + 26 * HOUR).await;
    assert_eq!((r.resolved, r.raised), (2, 0));
    let resolved = h.alerts(&ws, "resolved").await;
    assert_eq!(resolved["items"].as_array().unwrap().len(), 2);
    assert!(resolved["items"][0]["resolved_at"].is_string());
    assert_eq!(h.alerts(&ws, "open").await["open_count"], 0);

    // The team's own sent mail never alerts.
    let r = h.eval(&ws, at + 40 * HOUR).await;
    assert_eq!(r.raised, 0);
}

#[tokio::test]
async fn patch_snooze_settings_validation_and_test_email() {
    let h = harness().await;
    let (ws, src) = h.workspace().await;
    let base = format!("/api/v1/workspaces/{ws}");
    h.call("GET", "/api/v1/me", OPERATOR, None).await;
    let (s, _) = h.call("POST", &format!("{base}/members"), ADMIN, Some(json!({"email": "op@acme.test", "role": "operator"}))).await;
    assert!(s.is_success(), "{s}");
    let no_digest = json!({"enabled": true, "recipients": ["admin@acme.test"], "escalate_to": [], "unanswered_hours": 4,
                           "escalate_after_hours": 24, "daily_digest": false, "digest_hour": 9});
    h.call("PUT", &format!("{base}/alert-settings"), ADMIN, Some(no_digest)).await;
    let t0 = now_ms();
    let msg = h.email(&ws, &src, "Timesheet question", "Is my timesheet approved? Please let me know.").await;
    h.eval(&ws, t0 + 5 * HOUR).await;
    h.mail.take();
    let id = h.alerts(&ws, "open").await["items"][0]["id"].as_str().unwrap().to_string();

    // Operators snooze and resolve; bad input is rejected.
    let (s, a) = h.call("PATCH", &format!("{base}/alerts/{id}"), OPERATOR, Some(json!({"status": "open", "snooze_hours": 2}))).await;
    assert_eq!((s, a["status"].as_str()), (StatusCode::OK, Some("snoozed")));
    assert!(a["snoozed_until"].is_string());
    assert_eq!(h.alerts(&ws, "open").await["open_count"], 0);
    assert_eq!(h.call("PATCH", &format!("{base}/alerts/{id}"), OPERATOR, Some(json!({"snooze_hours": 0}))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(h.call("PATCH", &format!("{base}/alerts/{id}"), OPERATOR, Some(json!({"status": "done"}))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(h.call("PATCH", &format!("{base}/alerts/nope"), OPERATOR, Some(json!({"status": "resolved"}))).await.0, StatusCode::NOT_FOUND);

    // Snooze ends: reopened and sent again.
    let r = h.eval(&ws, now_ms() + 3 * HOUR).await;
    assert_eq!((r.reopened, r.emails_sent), (1, 1));
    assert_eq!(h.mail.take()[0].1, "[Post Room] No reply yet: Timesheet question");

    let (s, a) = h.call("PATCH", &format!("{base}/alerts/{id}"), OPERATOR, Some(json!({"status": "resolved"}))).await;
    assert_eq!((s, a["status"].as_str()), (StatusCode::OK, Some("resolved")));
    // A person's resolve is final: the rule does not re-raise the same alert.
    let r = h.eval(&ws, t0 + 6 * HOUR).await;
    assert_eq!(r.raised, 0);
    assert_eq!(alert_for(&h.alerts(&ws, "all").await, &msg)["status"], "resolved");

    // Settings: admin only, validated; recipients must be verified members.
    h.add_members(&ws, &["a@acme.test", "b@acme.test"]).await;
    let good =json!({"enabled": true, "recipients": ["a@acme.test", "b@acme.test"], "escalate_to": [], "unanswered_hours": 2,
                      "escalate_after_hours": 8, "daily_digest": true, "digest_hour": 7});
    assert_eq!(h.call("PUT", &format!("{base}/alert-settings"), OPERATOR, Some(good.clone())).await.0, StatusCode::FORBIDDEN);
    for bad in [
        json!({"recipients": ["not-an-email"]}),
        json!({"recipients": ["stranger@elsewhere.test"]}),
        json!({"escalate_to": ["stranger@elsewhere.test"]}),
        json!({"unanswered_hours": 0}),
        json!({"escalate_after_hours": 1000}),
        json!({"digest_hour": 24}),
    ] {
        let mut body = good.clone();
        for (k, v) in bad.as_object().unwrap() {
            body[k] = v.clone();
        }
        assert_eq!(h.call("PUT", &format!("{base}/alert-settings"), ADMIN, Some(body)).await.0, StatusCode::BAD_REQUEST);
    }
    let mut unknown = good.clone();
    unknown["surprise"] = json!(1);
    assert!(h.call("PUT", &format!("{base}/alert-settings"), ADMIN, Some(unknown)).await.0.is_client_error());
    let (s, saved) = h.call("PUT", &format!("{base}/alert-settings"), ADMIN, Some(good.clone())).await;
    assert_eq!((s, &saved), (StatusCode::OK, &good));
    assert_eq!(h.call("GET", &format!("{base}/alert-settings"), OPERATOR, None).await.1, good);

    // Test email goes through the delivery trait.
    assert_eq!(h.call("POST", &format!("{base}/alert-settings/test"), OPERATOR, None).await.0, StatusCode::FORBIDDEN);
    let (s, r) = h.call("POST", &format!("{base}/alert-settings/test"), ADMIN, None).await;
    assert_eq!((s, r), (StatusCode::OK, json!({"sent_to": ["a@acme.test", "b@acme.test"]})));
    let sent = h.mail.take();
    assert_eq!(sent.len(), 2);
    assert!(sent[0].1.starts_with("[Post Room] Test alert"));
    // One test per workspace per 10 minutes.
    assert_eq!(h.call("POST", &format!("{base}/alert-settings/test"), ADMIN, None).await.0, StatusCode::TOO_MANY_REQUESTS);
    assert!(h.mail.take().is_empty());

    // Disabled: rules don't raise.
    let mut off = good.clone();
    off["enabled"] = json!(false);
    h.call("PUT", &format!("{base}/alert-settings"), ADMIN, Some(off)).await;
    h.email(&ws, &src, "Another timesheet", "Here is another timesheet. Did you receive it?").await;
    assert_eq!(h.eval(&ws, now_ms() + 5 * HOUR).await.raised, 0);
}

#[tokio::test]
async fn raise_is_idempotent_and_digest_is_daily() {
    let h = harness().await;
    let (ws, _) = h.workspace().await;
    let new = || NewAlert {
        workspace_id: &ws,
        kind: alerts::AUTOMATION,
        severity: "normal",
        title: "Automation needs attention".into(),
        body: "A rule could not send a reply.".into(),
        message_id: None,
        level: 1,
        dedupe_key: None,
    };
    let (id1, created1) = alerts::raise(&h.state.db, new()).await.unwrap();
    let (id2, created2) = alerts::raise(&h.state.db, new()).await.unwrap();
    assert_eq!((created1, created2, id1 == id2), (true, false, true));

    // Daily digest: only in the digest hour (UTC), once per day.
    let day = now_ms() / 86_400_000 + 1;
    let off_hour = day * 86_400_000 + 8 * HOUR;
    let digest_hour = day * 86_400_000 + 9 * HOUR + 5 * 60_000;
    let r = h.eval(&ws, off_hour).await;
    assert!(!r.digest_sent);
    h.mail.take();
    let r = h.eval(&ws, digest_hour).await;
    assert!(r.digest_sent);
    let sent = h.mail.take();
    assert!(sent.iter().any(|(_, s, t, _)| s == "[Post Room] Daily digest: 1 open alert" && t.contains(&format!("/w/{ws}/alerts"))), "{sent:?}");
    assert!(!h.eval(&ws, digest_hour + 15 * 60_000).await.digest_sent);

    // Without a mail transport, the test endpoint says so.
    let mut state = h.state.clone();
    state.alert_mailer = None;
    let app = router(state);
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/workspaces/{ws}/alert-settings/test"))
        .header("x-auth-token", ADMIN)
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.oneshot(req).await.unwrap().status(), StatusCode::CONFLICT);
}
