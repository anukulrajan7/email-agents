//! Candidate watch end to end: a job seeker told "we'll send matching roles" gets one email when a
//! matching requirement arrives; weekly cap, STOP and confidential clients are honoured.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::{
    Json, Router,
    body::Body,
    http::{Request, StatusCode},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use email_ai_api::{
    ai::{AiError, DraftInput, DraftOutput, Drafter},
    build_state,
    candidate_watch,
    config::Config,
    db,
    domain::NormalizedEmail,
    http::{AppState, router},
    ingest,
    jobs::worker,
    replies,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const ADMIN: &str = "dev:admin@acme.test";
const SEEKER: &str = "ananth@gmail.com";
const CLIENT: &str = "hr@capgemini.com";

type Sends = Arc<Mutex<Vec<Value>>>;

struct Seeker;

#[async_trait]
impl Drafter for Seeker {
    async fn draft(&self, _: &DraftInput) -> Result<DraftOutput, AiError> {
        Ok(DraftOutput {
            body_text: "Hello,\n\nThank you for reaching out about a Java role. You asked if there are any Java openings: [NEEDS: current Java job openings]. We will review what we have and get back to you.\n\nBest regards,\nAarav".into(),
            model_id: "test".into(),
            unanswered: vec!["current Java job openings".into()],
        })
    }
}

async fn spawn_gmail(sends: Sends) -> String {
    let (s1, s2) = (sends.clone(), sends);
    let app = Router::new()
        .route("/proxy/gmail/v1/users/me/profile", get(|| async { Json(json!({"emailAddress": "me@acme.com"})) }))
        .route("/proxy/gmail/v1/users/me/messages/send", post(move |Json(b): Json<Value>| async move {
            s1.lock().unwrap().push(b);
            Json(json!({"id": "gmsg"}))
        }))
        .route("/proxy/gmail/v1/users/me/drafts/send", post(move |Json(b): Json<Value>| async move {
            s2.lock().unwrap().push(b);
            Json(json!({"id": "gmsg"}))
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
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

fn qp_decode(raw: &[u8]) -> String {
    let mut out = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'=' && raw.get(i + 1) == Some(&b'\r') && raw.get(i + 2) == Some(&b'\n') {
            i += 3;
        } else if raw[i] == b'=' && i + 2 < raw.len() && raw[i + 1].is_ascii_hexdigit() && raw[i + 2].is_ascii_hexdigit() {
            out.push(u8::from_str_radix(std::str::from_utf8(&raw[i + 1..i + 3]).unwrap(), 16).unwrap());
            i += 3;
        } else {
            out.push(raw[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

struct Env {
    state: AppState,
    ws: String,
    src: String,
    sends: Sends,
    _dir: tempfile::TempDir,
}

async fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let sends: Sends = Arc::default();
    let mut config = Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()));
    config.nango_api_url = spawn_gmail(sends.clone()).await;
    config.nango_api_key = "k".into();
    let mut state = build_state(config).await.unwrap();
    db::migrate(&state.db).await.unwrap();
    state.drafter = Arc::new(Seeker);
    state.judge = Arc::new(email_ai_api::ai::mock::mock_judge());
    let app = router(state.clone());
    let (_, ws) = call(&app, "POST", "/api/v1/workspaces", Some(json!({"name": "Acme", "template": "hr"}))).await;
    let ws = ws["id"].as_str().unwrap().to_string();
    assert_eq!(call(&app, "POST", &format!("/api/v1/workspaces/{ws}/matching/setup"), None).await.0, StatusCode::OK);
    let src = "src_gmail_test".to_string();
    sqlx::query(
        "INSERT INTO email_sources (id, workspace_id, kind, status, external_connection_id, provider_config_key, capabilities_json, created_at, mailbox_address)
         VALUES (?1, ?2, 'nango_gmail', 'connected', 'conn_test', 'google-mail',
                 '{\"read\":true,\"sync\":true,\"provider_labels\":false,\"provider_drafts\":true,\"send\":true}', ?3, 'me@acme.com')",
    )
    .bind(&src)
    .bind(&ws)
    .bind(db::now_ms())
    .execute(&state.db)
    .await
    .unwrap();
    Env { state, ws, src, sends, _dir: dir }
}

impl Env {
    async fn receive(&self, id: &str, from: &str, subject: &str, body: &str) -> String {
        let email = NormalizedEmail {
            source_message_id: id.into(),
            rfc_message_id: Some(format!("<{id}@mail.example>")),
            sender_address: from.into(),
            recipients: vec!["me@acme.com".into()],
            subject: subject.into(),
            body_text: body.into(),
            received_at: db::now_ms() - 3_600_000,
            ..Default::default()
        };
        let mut tx = self.state.db.begin().await.unwrap();
        let mid = ingest::persist_message(&mut tx, &self.src, &self.ws, &email, Some(&format!("t-{id}")), json!({})).await.unwrap().unwrap();
        tx.commit().await.unwrap();
        worker::drain(&self.state).await;
        mid
    }

    fn to_seeker(&self) -> usize {
        self.sends
            .lock()
            .unwrap()
            .iter()
            .filter(|b| b["raw"].as_str().is_some_and(|r| String::from_utf8_lossy(&URL_SAFE_NO_PAD.decode(r).unwrap_or_default()).contains(SEEKER)))
            .count()
    }

    fn last_raw(&self) -> String {
        let sends = self.sends.lock().unwrap();
        let raw = URL_SAFE_NO_PAD.decode(sends.last().unwrap()["raw"].as_str().unwrap()).unwrap();
        qp_decode(&raw)
    }

    async fn watch(&self) -> (i64, Option<i64>) {
        sqlx::query_as("SELECT opted_out, last_notified_at FROM candidate_watch WHERE workspace_id = ?1 AND email = ?2")
            .bind(&self.ws)
            .bind(SEEKER)
            .fetch_one(&self.state.db)
            .await
            .unwrap()
    }

    async fn requirement(&self, id: &str, subject: &str) {
        self.receive(id, CLIENT, subject, "Need 2 developers with Java and Spring Boot. 5-8 yrs. Location Pune.").await;
        worker::drain(&self.state).await;
        worker::drain(&self.state).await;
    }

    async fn enroll_seeker(&self) -> String {
        let text = "Hi, I am a Java developer, 6 years with Spring Boot and Microservices. Pune. Notice period 30 days. Are there any Java openings?";
        let msg = self.receive("seeker", SEEKER, "Looking for a job", text).await;
        assert!(candidate_watch::enroll(&self.state, &self.ws, &msg, SEEKER, "Looking for a job", text).await.unwrap());
        msg
    }
}

#[tokio::test]
async fn the_job_seeker_reply_answers_directly_and_a_sent_reply_starts_the_watch() {
    let env = env().await;
    let msg = env.receive("seeker", SEEKER, "Looking for a job", "Hi, I am a Java developer with 6 years in Spring Boot. Are there any Java openings?").await;
    let (s, _) = call(&router(env.state.clone()), "POST", &format!("/api/v1/workspaces/{}/messages/{msg}/draft", env.ws), None).await;
    assert!(s.is_success(), "{s}");
    worker::drain(&env.state).await;
    let (draft_id, body): (String, String) = sqlx::query_as("SELECT id, body_text FROM drafts WHERE message_id = ?1").bind(&msg).fetch_one(&env.state.db).await.unwrap();
    assert!(body.contains("We don't have a matching opening right now") || body.contains("we don't have a matching opening right now"), "{body}");
    assert!(!body.contains("[NEEDS") && !body.to_lowercase().contains("get back to you") && !body.contains("Aarav"), "{body}");
    assert!(body.contains("as soon as a matching role comes in, we'll send it to you"), "{body}");

    replies::send_draft(&env.state, &env.ws, &draft_id, email_ai_api::audit::Actor::System).await.unwrap();
    worker::drain(&env.state).await;
    let (opted_out, notified) = env.watch().await;
    assert_eq!((opted_out, notified), (0, None), "the sent promise started the watch");
}

#[tokio::test]
async fn a_new_matching_requirement_emails_the_candidate_exactly_once() {
    let env = env().await;
    env.enroll_seeker().await;
    env.requirement("r1", "Requirement: Java Developer").await;
    assert_eq!(env.to_seeker(), 1, "one email to the job seeker");
    let raw = env.last_raw();
    assert!(raw.contains("Java Developer") && raw.contains("Capgemini") && raw.contains("Reply STOP"), "{raw}");
    let audited: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE action = 'candidate_watch.notified'").fetch_one(&env.state.db).await.unwrap();
    assert_eq!(audited, 1);
    worker::drain(&env.state).await;
    assert_eq!(env.to_seeker(), 1, "re-running changes nothing");
}

#[tokio::test]
async fn at_most_one_email_per_candidate_per_week() {
    let env = env().await;
    env.enroll_seeker().await;
    env.requirement("r1", "Requirement: Java Developer").await;
    env.requirement("r2", "Requirement: Senior Java Developer").await;
    assert_eq!(env.to_seeker(), 1, "the second requirement within a week sends nothing");
    sqlx::query("UPDATE drafts SET updated_at = updated_at - 8 * 86400000").execute(&env.state.db).await.unwrap();
    sqlx::query("UPDATE candidate_watch SET last_notified_at = ?1").bind(db::now_ms() - 8 * 86_400_000).execute(&env.state.db).await.unwrap();
    env.requirement("r3", "Requirement: Java Lead Developer").await;
    assert_eq!(env.to_seeker(), 2, "a week later the next match goes out");
}

#[tokio::test]
async fn stop_opts_the_candidate_out() {
    let env = env().await;
    env.enroll_seeker().await;
    env.receive("stop1", SEEKER, "Re: Looking for a job", "STOP").await;
    assert_eq!(env.watch().await.0, 1);
    env.requirement("r1", "Requirement: Java Developer").await;
    assert_eq!(env.to_seeker(), 0, "no email after STOP");
    let audited: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE action = 'candidate_watch.opted_out'").fetch_one(&env.state.db).await.unwrap();
    assert_eq!(audited, 1);
}

#[tokio::test]
async fn a_confidential_requirement_never_names_the_client() {
    let env = env().await;
    env.enroll_seeker().await;
    env.requirement("r1", "Confidential requirement: Java Developer").await;
    assert_eq!(env.to_seeker(), 1);
    let raw = env.last_raw();
    assert!(raw.contains("Java Developer") && !raw.contains("Capgemini"), "{raw}");
}

#[tokio::test]
async fn a_sent_confirm_later_promise_becomes_an_owner_follow_up() {
    let env = env().await;
    let msg = env.receive("q1", "buyer@acme.com", "Pricing", "What does the premium plan cost for us?").await;
    sqlx::query(
        "INSERT INTO drafts (id, workspace_id, message_id, body_text, status, unanswered_json, created_at, updated_at)
         VALUES ('drf_p', ?1, ?2, 'Hello,\n\nThanks for asking about the plan. We''ll confirm the pricing shortly.\n\nBest regards,\nHR', 'edited', '[\"plan price\"]', ?3, ?3)",
    )
    .bind(&env.ws)
    .bind(&msg)
    .bind(db::now_ms())
    .execute(&env.state.db)
    .await
    .unwrap();
    replies::send_draft(&env.state, &env.ws, "drf_p", email_ai_api::audit::Actor::System).await.unwrap();
    worker::drain(&env.state).await;
    let titles: Vec<String> = sqlx::query_scalar("SELECT title FROM review_items WHERE kind = 'agent_task' AND message_id = ?1").bind(&msg).fetch_all(&env.state.db).await.unwrap();
    assert_eq!(titles.len(), 1, "{titles:?}");
    assert!(titles[0].starts_with("Follow up: you told buyer@acme.com we'd confirm plan price"), "{titles:?}");
}
