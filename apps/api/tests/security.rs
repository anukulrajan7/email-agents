//! Security hardening: invitation-only sign-up, account pre-claiming, password reset, workspace and
//! test-email caps, per-IP auth limits, security headers, sliding sessions, draft-context isolation for
//! automated drafts, auto-send trust, the global inbound AI budget and backups.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use email_ai_api::{
    ai::{
        AiError, Classifier, ClassifyInput, ClassifyOutput, DraftInput, DraftOutput, Drafter,
        fallback::{RulesClassifier, TemplateDrafter},
        memory_extractor::LearnDirection,
    },
    automations, backup, build_state,
    config::{AuthMode, Config},
    db::{self, now_ms},
    http::{AppState, router},
    jobs::worker,
    mailer::MemoryMailer,
    memory, memory_learning, sessions,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const PW: &str = "correct horse battery";
const ADMIN: &str = "dev:admin@acme.test";

struct H {
    state: AppState,
    app: Router,
    mail: Arc<MemoryMailer>,
    _dir: tempfile::TempDir,
}

async fn harness(configure: impl FnOnce(&mut Config)) -> H {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()));
    configure(&mut config);
    let mut state = build_state(config).await.unwrap();
    db::migrate(&state.db).await.unwrap();
    let mail = Arc::new(MemoryMailer::default());
    if state.config.auth_mode == AuthMode::Password {
        state.mailer = Some(mail.clone());
    }
    H { app: router(state.clone()), state, mail, _dir: dir }
}

async fn password_app(configure: impl FnOnce(&mut Config)) -> H {
    harness(|c| {
        c.auth_mode = AuthMode::Password;
        configure(c);
    })
    .await
}

impl H {
    async fn send(&self, method: &str, path: &str, token: Option<&str>, ip: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(path);
        if let Some(t) = token {
            req = req.header("x-auth-token", t);
        }
        if let Some(ip) = ip {
            req = req.header("x-forwarded-for", ip);
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

    async fn call(&self, method: &str, path: &str, token: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
        self.send(method, path, token, None, body).await
    }

    /// The 6-digit code in the newest email to `to`.
    fn last_code(&self, to: &str) -> String {
        let sent = self.mail.sent.lock().unwrap();
        let text = &sent.iter().rev().find(|(t, _, _)| t == to).unwrap_or_else(|| panic!("no mail to {to}")).2;
        text.split_whitespace()
            .map(|w| w.trim_end_matches('.'))
            .find(|w| w.len() == 6 && w.chars().all(|c| c.is_ascii_digit()))
            .unwrap()
            .to_string()
    }

    fn mails_to(&self, to: &str) -> usize {
        self.mail.sent.lock().unwrap().iter().filter(|(t, _, _)| t == to).count()
    }

    async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(&self.state.db).await.unwrap()
    }

    async fn user_id(&self, email: &str) -> String {
        sqlx::query_scalar("SELECT id FROM users WHERE email = ?1").bind(email).fetch_one(&self.state.db).await.unwrap()
    }

    /// Signs up and verifies a password account; returns its session token.
    async fn verified_account(&self, email: &str) -> String {
        let (s, _) = self.call("POST", "/api/v1/auth/signup", None, Some(json!({"email": email, "password": PW}))).await;
        assert_eq!(s, StatusCode::ACCEPTED, "{email}");
        let code = self.last_code(email);
        let (s, v) = self.call("POST", "/api/v1/auth/verify", None, Some(json!({"email": email, "code": code}))).await;
        assert_eq!(s, StatusCode::OK);
        v["token"].as_str().unwrap().to_string()
    }

    async fn dev_workspace(&self) -> (String, String) {
        let (_, ws) = self.call("POST", "/api/v1/workspaces", Some(ADMIN), Some(json!({"name": "Acme"}))).await;
        let ws = ws["id"].as_str().unwrap().to_string();
        let (_, src) = self.call("POST", &format!("/api/v1/workspaces/{ws}/sources/forwarding"), Some(ADMIN), None).await;
        (ws, src["id"].as_str().unwrap().to_string())
    }

    async fn email(&self, ws: &str, src: &str, from: &str, subject: &str, body: &str) -> String {
        let (s, _) = self
            .call("POST", &format!("/api/v1/workspaces/{ws}/sources/{src}/test-email"), Some(ADMIN),
                Some(json!({"from": from, "subject": subject, "body_text": body})))
            .await;
        assert_eq!(s, StatusCode::ACCEPTED);
        worker::drain(&self.state).await;
        sqlx::query_scalar("SELECT id FROM email_messages WHERE workspace_id = ?1 AND subject = ?2")
            .bind(ws)
            .bind(subject)
            .fetch_one(&self.state.db)
            .await
            .unwrap()
    }
}

// ---------------------------------------------------------------- 1. sign-up, caps

#[tokio::test]
async fn signup_is_closed_except_bootstrap_allow_lists_and_invites() {
    let h = harness(|c| {
        c.auth_mode = AuthMode::Password;
        c.signup_allowed_emails = vec!["vip@partner.test".into()];
    })
    .await;
    let signup = |email: &'static str| h.call("POST", "/api/v1/auth/signup", None, Some(json!({"email": email, "password": PW})));
    // With an allow-list configured, even the first account must be on it.
    let (s, err) = signup("first@nowhere.test").await;
    assert_eq!((s, err["error"]["message"].as_str()), (StatusCode::FORBIDDEN, Some("Sign-up is by invitation")));
    assert_eq!(signup("VIP@partner.test").await.0, StatusCode::ACCEPTED, "allowed; verification code sent");

    // No allow-list: the very first account bootstraps; everyone else needs an invitation.
    let h = harness(|c| c.auth_mode = AuthMode::Password).await;
    let mut state = h.state.clone();
    state.mailer = None;
    let app = router(state);
    let post = |email: &str| {
        let req = Request::post("/api/v1/auth/signup")
            .header("content-type", "application/json")
            .body(Body::from(json!({"email": email, "password": PW}).to_string()))
            .unwrap();
        app.clone().oneshot(req)
    };
    let res = post("founder@acme.test").await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED, "bootstrap admin");
    let token = serde_json::from_slice::<Value>(&res.into_body().collect().await.unwrap().to_bytes()).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(post("stranger@acme.test").await.unwrap().status(), StatusCode::FORBIDDEN);

    let call = |method: &str, path: String, body: Option<Value>| {
        let req = Request::builder().method(method).uri(path).header("x-auth-token", token.clone());
        let req = match body {
            Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
            None => req.body(Body::empty()).unwrap(),
        };
        app.clone().oneshot(req)
    };
    let res = call("POST", "/api/v1/workspaces".into(), Some(json!({"name": "Acme"}))).await.unwrap();
    let ws = serde_json::from_slice::<Value>(&res.into_body().collect().await.unwrap().to_bytes()).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let res = call("POST", format!("/api/v1/workspaces/{ws}/members"), Some(json!({"email": "Guest@Partner.test", "role": "operator"})))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED, "no account yet: a pending invitation");
    assert_eq!(h.count("SELECT COUNT(*) FROM invitations").await, 1);
    assert_eq!(post("guest@partner.test").await.unwrap().status(), StatusCode::CREATED, "invited emails may sign up");
    assert_eq!(h.count("SELECT COUNT(*) FROM invitations").await, 0);
    let role: String = sqlx::query_scalar(
        "SELECT m.role FROM memberships m JOIN users u ON u.id = m.user_id WHERE u.email = 'guest@partner.test' AND m.workspace_id = ?1",
    )
    .bind(&ws)
    .fetch_one(&h.state.db)
    .await
    .unwrap();
    assert_eq!(role, "operator");
}

#[tokio::test]
async fn workspaces_per_user_are_capped() {
    let h = harness(|_| {}).await;
    for i in 0..5 {
        let (s, _) = h.call("POST", "/api/v1/workspaces", Some(ADMIN), Some(json!({"name": format!("W{i}")}))).await;
        assert_eq!(s, StatusCode::CREATED);
    }
    let (s, err) = h.call("POST", "/api/v1/workspaces", Some(ADMIN), Some(json!({"name": "One too many"}))).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{err}");
    assert_eq!(h.count("SELECT COUNT(*) FROM workspaces").await, 5, "the rejected workspace was rolled back");
    assert_eq!(h.call("POST", "/api/v1/workspaces", Some("dev:other@acme.test"), Some(json!({"name": "Mine"}))).await.0, StatusCode::CREATED);
}

#[tokio::test]
async fn test_emails_are_admin_only_and_capped_per_day() {
    let h = harness(|_| {}).await;
    let (ws, src) = h.dev_workspace().await;
    let path = format!("/api/v1/workspaces/{ws}/sources/{src}/test-email");
    h.call("GET", "/api/v1/me", Some("dev:op@acme.test"), None).await;
    h.call("POST", &format!("/api/v1/workspaces/{ws}/members"), Some(ADMIN), Some(json!({"email": "op@acme.test", "role": "operator"}))).await;
    assert_eq!(h.call("POST", &path, Some("dev:op@acme.test"), None).await.0, StatusCode::FORBIDDEN);
    for _ in 0..20 {
        assert_eq!(h.call("POST", &path, Some(ADMIN), None).await.0, StatusCode::ACCEPTED);
    }
    assert_eq!(h.call("POST", &path, Some(ADMIN), None).await.0, StatusCode::TOO_MANY_REQUESTS);
}

// ---------------------------------------------------------------- 2. pre-claiming, invitations, reset

#[tokio::test]
async fn an_unverified_signup_cannot_pre_claim_an_account() {
    let h = password_app(|c| c.signup_allowed_domains = vec!["acme.test".into()]).await;
    let victim = "victim@acme.test";
    // The attacker registers the victim's address first (can't read the code).
    assert_eq!(h.call("POST", "/api/v1/auth/signup", None, Some(json!({"email": victim, "password": "attacker password 1"}))).await.0, StatusCode::ACCEPTED);
    let uid = h.user_id(victim).await;
    let squatter = sessions::create_session(&h.state.db, &uid).await.unwrap().token;
    let stale_code = h.last_code(victim);

    // The real owner signs up later: not "already exists"; the squatter's password, codes and sessions die.
    assert_eq!(h.call("POST", "/api/v1/auth/signup", None, Some(json!({"email": victim, "password": PW}))).await.0, StatusCode::ACCEPTED);
    assert_eq!(h.call("GET", "/api/v1/me", Some(&squatter), None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(h.mails_to(victim), 2);
    let code = h.last_code(victim);
    if code != stale_code {
        let (s, _) = h.call("POST", "/api/v1/auth/verify", None, Some(json!({"email": victim, "code": stale_code}))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "codes from before the takeover are void");
    }
    // A session that appeared before verification does not survive it.
    let early = sessions::create_session(&h.state.db, &uid).await.unwrap().token;
    let (s, v) = h.call("POST", "/api/v1/auth/verify", None, Some(json!({"email": victim, "code": code}))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(h.call("GET", "/api/v1/me", Some(&early), None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(h.call("GET", "/api/v1/me", Some(v["token"].as_str().unwrap()), None).await.0, StatusCode::OK);

    let login = |pw: &'static str| h.call("POST", "/api/v1/auth/login", None, Some(json!({"email": victim, "password": pw})));
    assert_eq!(login("attacker password 1").await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(login(PW).await.0, StatusCode::OK);
    // Verified accounts are not overwritten.
    assert_eq!(h.call("POST", "/api/v1/auth/signup", None, Some(json!({"email": victim, "password": "another password"}))).await.0, StatusCode::CONFLICT);
}

#[tokio::test]
async fn members_must_be_verified_or_become_invitations() {
    let h = password_app(|c| c.signup_allowed_domains = vec!["acme.test".into()]).await;
    let admin = h.verified_account("admin@acme.test").await;
    let (_, ws) = h.call("POST", "/api/v1/workspaces", Some(&admin), Some(json!({"name": "Acme"}))).await;
    let ws = ws["id"].as_str().unwrap().to_string();

    // An unverified sign-up (possibly a squatter) is invited, not added.
    h.call("POST", "/api/v1/auth/signup", None, Some(json!({"email": "new@acme.test", "password": PW}))).await;
    let (s, v) = h.call("POST", &format!("/api/v1/workspaces/{ws}/members"), Some(&admin), Some(json!({"email": "new@acme.test", "role": "operator"}))).await;
    assert_eq!((s, v["invited"].as_bool()), (StatusCode::ACCEPTED, Some(true)));
    assert_eq!(h.count("SELECT COUNT(*) FROM memberships").await, 1);

    let code = h.last_code("new@acme.test");
    let (_, session) = h.call("POST", "/api/v1/auth/verify", None, Some(json!({"email": "new@acme.test", "code": code}))).await;
    let (_, me) = h.call("GET", "/api/v1/me", Some(session["token"].as_str().unwrap()), None).await;
    assert_eq!(me["workspaces"][0]["id"], ws.as_str(), "verification turned the invitation into a membership");
    assert_eq!(me["workspaces"][0]["role"], "operator");

    // A verified account joins directly.
    let other = h.verified_account("other@acme.test").await;
    let (s, _) = h.call("POST", &format!("/api/v1/workspaces/{ws}/members"), Some(&admin), Some(json!({"email": "other@acme.test", "role": "operator"}))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(h.call("GET", &format!("/api/v1/workspaces/{ws}/labels"), Some(&other), None).await.0, StatusCode::OK);
}

#[tokio::test]
async fn password_reset_sets_a_new_password_and_signs_everyone_out() {
    let h = password_app(|c| c.signup_allowed_domains = vec!["acme.test".into()]).await;
    let email = "meera@acme.test";
    let old_session = h.verified_account(email).await;

    // Unknown accounts get the same answer, and no email.
    let (s, v) = h.call("POST", "/api/v1/auth/password-reset/request", None, Some(json!({"email": "ghost@acme.test"}))).await;
    assert_eq!((s, v), (StatusCode::OK, json!({"status": "sent"})));
    let (s, _) = h.call("POST", "/api/v1/auth/password-reset/request", None, Some(json!({"email": email}))).await;
    assert_eq!(s, StatusCode::OK);
    for _ in 0..100 {
        if h.mail.sent.lock().unwrap().iter().any(|(_, subject, _)| subject.contains("password reset")) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(h.mail.sent.lock().unwrap().iter().all(|(to, _, _)| to != "ghost@acme.test"));
    let code = h.last_code(email);
    let wrong = if code == "000000" { "111111" } else { "000000" };
    let confirm = |code: &str, pw: &str| {
        h.call("POST", "/api/v1/auth/password-reset/confirm", None, Some(json!({"email": email, "code": code, "new_password": pw})))
    };
    assert_eq!(confirm(&code, "short").await.0, StatusCode::BAD_REQUEST);
    assert_eq!(confirm(wrong, "brand new password").await.0, StatusCode::BAD_REQUEST);
    let (s, v) = confirm(&code, "brand new password").await;
    assert_eq!((s, v), (StatusCode::OK, json!({"status": "password_reset"})));
    assert_eq!(confirm(&code, "another new password").await.0, StatusCode::BAD_REQUEST, "codes are single use");

    assert_eq!(h.call("GET", "/api/v1/me", Some(&old_session), None).await.0, StatusCode::UNAUTHORIZED);
    let login = |pw: &'static str| h.call("POST", "/api/v1/auth/login", None, Some(json!({"email": email, "password": pw})));
    assert_eq!(login(PW).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(login("brand new password").await.0, StatusCode::OK);
    // A verify code can't be used as a reset code.
    assert_eq!(h.count("SELECT COUNT(*) FROM email_codes WHERE purpose = 'password_reset'").await, 1);
}

// ---------------------------------------------------------------- 6. auth flooding, 8. headers + sessions

#[tokio::test]
async fn auth_endpoints_are_rate_limited_per_client_ip() {
    let h = password_app(|_| {}).await;
    let resend = |ip: &'static str| h.send("POST", "/api/v1/auth/resend-code", None, Some(ip), Some(json!({"email": "a@acme.test"})));
    for _ in 0..20 {
        assert_eq!(resend("203.0.113.7").await.0, StatusCode::ACCEPTED);
    }
    let (s, body) = resend("203.0.113.7").await;
    assert_eq!((s, body["error"]["code"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("rate_limited")));
    assert_eq!(resend("203.0.113.8").await.0, StatusCode::ACCEPTED, "other clients are unaffected");
    assert_eq!(h.send("GET", "/api/v1/auth/config", None, Some("203.0.113.7"), None).await.0, StatusCode::OK, "config is not limited");
}

#[tokio::test]
async fn every_response_carries_security_headers() {
    let h = harness(|_| {}).await;
    for path in ["/health/live", "/api/v1/nope", "/api/v1/me"] {
        let res = h.app.clone().oneshot(Request::get(path).body(Body::empty()).unwrap()).await.unwrap();
        let get = |n: &str| res.headers().get(n).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
        let csp = get("content-security-policy");
        assert!(csp.contains("default-src 'self'") && csp.contains("frame-ancestors 'none'") && csp.contains("img-src 'self' https: data:"), "{path}: {csp}");
        assert_eq!(get("strict-transport-security"), "max-age=31536000; includeSubDomains");
        assert_eq!(get("x-content-type-options"), "nosniff");
        assert_eq!(get("x-frame-options"), "DENY");
        assert_eq!(get("referrer-policy"), "strict-origin-when-cross-origin");
        assert!(get("permissions-policy").contains("camera=()"));
    }
}

#[tokio::test]
async fn sessions_last_seven_days_and_slide_on_use() {
    let h = password_app(|c| c.signup_allowed_domains = vec!["acme.test".into()]).await;
    let token = h.verified_account("s@acme.test").await;
    let ttl: i64 = h.count("SELECT expires_at - created_at FROM sessions").await;
    assert_eq!(ttl, sessions::SESSION_TTL_MS);
    assert_eq!(sessions::SESSION_TTL_MS, 7 * 86_400_000);
    let now = now_ms();
    sqlx::query("UPDATE sessions SET last_used_at = ?1, expires_at = ?2").bind(now - 3_600_000).bind(now + 60_000).execute(&h.state.db).await.unwrap();
    assert_eq!(h.call("GET", "/api/v1/me", Some(&token), None).await.0, StatusCode::OK);
    let expires: i64 = h.count("SELECT expires_at FROM sessions").await;
    assert!(expires > now + sessions::SESSION_TTL_MS - 60_000, "renewed on use");
}

// ---------------------------------------------------------------- 5. automated drafts and auto-send

#[derive(Default)]
struct Seen(Mutex<Vec<DraftInput>>);

struct RecordingDrafter(Arc<Seen>);

#[async_trait]
impl Drafter for RecordingDrafter {
    async fn draft(&self, input: &DraftInput) -> Result<DraftOutput, AiError> {
        self.0.0.lock().unwrap().push(input.clone());
        TemplateDrafter.draft(input).await
    }
}

#[tokio::test]
async fn automated_drafts_only_see_same_domain_replies_and_staff_notes() {
    let mut h = harness(|_| {}).await;
    let seen = Arc::new(Seen::default());
    h.state.drafter = Arc::new(RecordingDrafter(seen.clone()));
    h.app = router(h.state.clone());
    let (ws, src) = h.dev_workspace().await;
    let base = format!("/api/v1/workspaces/{ws}");

    // The team replies (by hand) to a client at acme.example.
    let first = h.email(&ws, &src, "Priya <priya@acme.example>", "September timesheet", "My timesheet for September: 160 hours.").await;
    h.call("POST", &format!("{base}/messages/{first}/draft"), Some(ADMIN), None).await;
    worker::drain(&h.state).await;
    let draft_id: String = sqlx::query_scalar("SELECT id FROM drafts WHERE message_id = ?1 ORDER BY created_at DESC, id DESC LIMIT 1")
        .bind(&first)
        .fetch_one(&h.state.db)
        .await
        .unwrap();
    let confidential = "Hi Priya, Acme's rate stays at 95 per hour.\n\nHR Team";
    h.call("PATCH", &format!("{base}/drafts/{draft_id}"), Some(ADMIN), Some(json!({"body_text": confidential, "status": "approved"}))).await;
    worker::drain(&h.state).await;

    // Notes: one written by staff, one auto-learned from a team reply (workspace-wide).
    h.call("POST", &format!("{base}/memories"), Some(ADMIN), Some(json!({"scope": "workspace", "kind": "fact", "content": "Payroll runs on the 28th."}))).await;
    let learned = memory_learning::LearnedFact {
        scope: "workspace",
        contact_email: None,
        kind: "fact",
        subject: "Acme".into(),
        key: "rate".into(),
        content: "Acme timesheets are billed weekly.".into(),
        confidence: 0.9,
    };
    memory_learning::store(&h.state.db, &ws, &first, "r1", LearnDirection::Reply, vec![learned], 500).await.unwrap();
    let all = memory::context_for(&h.state.db, &ws, "x@evil.example", "timesheet").await.unwrap().lines();
    let auto = memory::context_for_automation(&h.state.db, &ws, "x@evil.example", "timesheet").await.unwrap().lines();
    assert!(all.iter().any(|l| l.contains("billed weekly")) && all.iter().any(|l| l.contains("28th")), "{all:?}");
    assert!(auto.iter().any(|l| l.contains("28th")) && !auto.iter().any(|l| l.contains("billed weekly")), "{auto:?}");

    // An outsider's similar email: the workflow drafts on its own, without other clients' replies.
    seen.0.lock().unwrap().clear();
    let outsider = h.email(&ws, &src, "Eve <eve@evil.example>", "October timesheet", "My timesheet for October: 150 hours.").await;
    let automated = seen.0.lock().unwrap().last().cloned().expect("the timesheet workflow drafted a reply");
    assert!(automated.examples.is_empty(), "{:?}", automated.examples);
    assert!(!automated.memory_notes.iter().any(|n| n.contains("billed weekly")), "{:?}", automated.memory_notes);

    // A person asking for a draft gets the full context.
    h.call("POST", &format!("{base}/messages/{outsider}/draft"), Some(ADMIN), None).await;
    worker::drain(&h.state).await;
    let manual = seen.0.lock().unwrap().last().cloned().unwrap();
    assert_eq!(manual.examples.len(), 1);

    // Same client domain: its own past reply is a fair example even for automated drafts.
    seen.0.lock().unwrap().clear();
    h.email(&ws, &src, "Raj <raj@acme.example>", "November timesheet", "My timesheet for November: 140 hours.").await;
    let same_domain = seen.0.lock().unwrap().last().cloned().unwrap();
    assert_eq!(same_domain.examples.len(), 1, "{:?}", same_domain.examples);
    assert_eq!(same_domain.examples[0].reply, confidential);
}

#[tokio::test]
async fn auto_send_needs_a_business_domain_and_an_earlier_human_reply() {
    assert!(automations::is_free_mail_domain("gmail.com"));
    assert!(automations::is_free_mail_domain("Yahoo.co.in"));
    assert!(automations::is_free_mail_domain("proton.me"));
    assert!(!automations::is_free_mail_domain("acme.com"));

    let h = harness(|_| {}).await;
    let (ws, src) = h.dev_workspace().await;
    for domain in ["gmail.com", "hotmail.co.uk"] {
        let (s, err) = h
            .call("POST", &format!("/api/v1/workspaces/{ws}/automations"), Some(ADMIN), Some(json!({
                "name": "Auto", "enabled": true, "conditions": [],
                "actions": [{"type": "draft_reply"}, {"type": "send_reply", "daily_cap": 5, "allowed_domains": [domain]}]
            })))
            .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{domain}");
        assert!(err["error"]["message"].as_str().unwrap().contains("free email provider"), "{err}");
    }

    let msg = h.email(&ws, &src, "Priya <priya@acme.example>", "Question", "Hello, a quick question for you.").await;
    let db = &h.state.db;
    let human = || automations::has_human_reply(db, &ws, "Priya@acme.example");
    assert!(!human().await.unwrap());
    let audit = |actor: &'static str| {
        sqlx::query(
            "INSERT INTO audit_events (id, workspace_id, actor_type, actor_id, action, entity_type, entity_id, message_id, metadata_json, created_at)
             VALUES (?1, ?2, ?3, NULL, 'draft.sent', 'draft', 'd', ?4, '{}', 0)",
        )
        .bind(format!("aud_{actor}"))
        .bind(&ws)
        .bind(actor)
        .bind(&msg)
        .execute(db)
    };
    audit("system").await.unwrap();
    assert!(!human().await.unwrap(), "an automation's send doesn't count");
    audit("user").await.unwrap();
    assert!(human().await.unwrap());

    // Mail the team wrote from the mailbox counts too, unless Post Room sent it.
    let out = |id: &'static str, gmail_id: &'static str| {
        sqlx::query(
            "INSERT INTO email_messages (id, workspace_id, source_id, thread_id, source_message_id, sender_address, recipients_json, subject,
                                         body_text, received_at, created_at, processing_status, direction)
             SELECT ?1, workspace_id, source_id, thread_id, ?2, 'admin@acme.test', '[\"raj@acme.example\"]', 'Hi', 'Hi Raj', received_at,
                    created_at, 'processed', 'outbound'
             FROM email_messages WHERE id = ?3",
        )
        .bind(id)
        .bind(gmail_id)
        .bind(&msg)
        .execute(db)
    };
    out("msg_out_auto", "gm-1").await.unwrap();
    sqlx::query(
        "INSERT INTO drafts (id, workspace_id, message_id, body_text, status, provider_message_id, created_at, updated_at)
         VALUES ('drf_auto', ?1, ?2, 'x', 'sent', 'gm-1', 0, 0)",
    )
    .bind(&ws)
    .bind(&msg)
    .execute(db)
    .await
    .unwrap();
    assert!(!automations::has_human_reply(db, &ws, "raj@acme.example").await.unwrap());
    out("msg_out_human", "gm-2").await.unwrap();
    assert!(automations::has_human_reply(db, &ws, "raj@acme.example").await.unwrap());
}

// ---------------------------------------------------------------- 11. global inbound budget

struct CountingClassifier(Arc<AtomicUsize>);

#[async_trait]
impl Classifier for CountingClassifier {
    async fn classify(&self, input: &ClassifyInput) -> Result<ClassifyOutput, AiError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        RulesClassifier.classify(input).await
    }
}

#[tokio::test]
async fn global_daily_budget_falls_back_to_rules_and_defers_embedding() {
    let mut h = harness(|c| c.ai_global_daily_budget_usd = 1.0).await;
    let calls = Arc::new(AtomicUsize::new(0));
    h.state.classifier = Arc::new(CountingClassifier(calls.clone()));
    h.app = router(h.state.clone());
    let (ws, src) = h.dev_workspace().await;

    let under = h.email(&ws, &src, "Priya <priya@acme.example>", "First", "Hello there, first email.").await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(h.count(&format!("SELECT COUNT(*) FROM knowledge_chunks WHERE message_id = '{under}'")).await > 0);

    // Another workspace (or anyone) spent the global budget today.
    sqlx::query(
        "INSERT INTO ai_calls (id, workspace_id, task, model, cost_usd, created_at) VALUES ('aic_x', NULL, 'classify', 'm', 1.5, ?1)",
    )
    .bind(now_ms())
    .execute(&h.state.db)
    .await
    .unwrap();
    let over = h.email(&ws, &src, "Priya <priya@acme.example>", "Second", "Hello there, second email.").await;
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the model was not called");
    let source: String = sqlx::query_scalar("SELECT source FROM classifications WHERE message_id = ?1").bind(&over).fetch_one(&h.state.db).await.unwrap();
    assert_eq!(source, "fallback");
    let chunks = format!("SELECT COUNT(*) FROM knowledge_chunks WHERE message_id = '{over}'");
    assert_eq!(h.count(&chunks).await, 0, "embedding waits");

    // Opening the email indexes it.
    let (s, _) = h.call("POST", &format!("/api/v1/workspaces/{ws}/messages/{over}/insight"), Some(ADMIN), None).await;
    assert_eq!(s, StatusCode::OK);
    worker::drain(&h.state).await;
    assert!(h.count(&chunks).await > 0);
}

// ---------------------------------------------------------------- backups + file permissions

#[tokio::test]
async fn nightly_backup_is_a_private_copy_and_keeps_seven() {
    let h = harness(|_| {}).await;
    h.dev_workspace().await;
    let dir = backup::backup_dir(&h.state.config.database_url).unwrap();
    for day in 1..=9u8 {
        let date = time::Date::from_calendar_date(2026, time::Month::October, day).unwrap();
        assert!(backup::run(&h.state.db, &dir, date, backup::KEEP).await.unwrap().is_some());
    }
    let date = time::Date::from_calendar_date(2026, time::Month::October, 9).unwrap();
    assert!(backup::run(&h.state.db, &dir, date, backup::KEEP).await.unwrap().is_none(), "once per day");
    let mut names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    names.sort();
    assert_eq!(names.len(), 7, "{names:?}");
    assert_eq!(names[0], "email_ai-20261003.db");
    let copy = dir.join("email_ai-20261009.db");
    let pool = db::connect(&format!("sqlite://{}", copy.display())).await.unwrap();
    let workspaces: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workspaces").fetch_one(&pool).await.unwrap();
    assert_eq!(workspaces, 1, "the backup is a working database");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&copy).unwrap().permissions().mode() & 0o777, 0o600);
        let main = dir.parent().unwrap().join("t.db");
        assert_eq!(std::fs::metadata(main).unwrap().permissions().mode() & 0o777, 0o600, "new databases are owner-only");
    }
}
