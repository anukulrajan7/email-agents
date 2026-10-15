//! Shared harness for the team / staffing integration tests.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use email_ai_api::{
    build_state,
    platform::config::Config,
    platform::db,
    platform::domain::{AttachmentMeta, NormalizedEmail},
    http::{AppState, router},
    mail::ingest,
    platform::mailer::Mailer,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

pub const OWNER: &str = "dev:owner@acme.test";
pub const HOUR: i64 = 3_600_000;
pub const DAY: i64 = 86_400_000;
/// Wednesday 2026-10-14 10:00 UTC.
pub const WED: i64 = 1_791_972_000_000;

#[derive(Default)]
pub struct Recorder {
    pub sent: Mutex<Vec<(String, String, String)>>,
}

#[async_trait]
impl Mailer for Recorder {
    async fn send(&self, to: &str, subject: &str, text: &str, _html: &str) -> anyhow::Result<()> {
        self.sent.lock().unwrap().push((to.into(), subject.into(), text.into()));
        Ok(())
    }
}

impl Recorder {
    pub fn take(&self) -> Vec<(String, String, String)> {
        std::mem::take(&mut *self.sent.lock().unwrap())
    }
}

pub struct H {
    pub state: AppState,
    pub app: Router,
    pub mail: Arc<Recorder>,
    pub _dir: tempfile::TempDir,
}

pub async fn harness() -> H {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()));
    let mut state = build_state(config).await.unwrap();
    db::migrate(&state.db).await.unwrap();
    let mail = Arc::new(Recorder::default());
    state.alert_mailer = Some(mail.clone());
    H { app: router(state.clone()), state, mail, _dir: dir }
}

pub fn email(from: &str, to: &[&str], subject: &str, body: &str, at: i64) -> NormalizedEmail {
    NormalizedEmail {
        source_message_id: format!("m-{subject}-{at}-{from}-{}", to.join(",")),
        sender_address: from.into(),
        recipients: to.iter().map(|s| s.to_string()).collect(),
        subject: subject.into(),
        body_text: body.into(),
        received_at: at,
        ..Default::default()
    }
}

pub fn with_files(mut e: NormalizedEmail, files: &[&str]) -> NormalizedEmail {
    e.attachments = files
        .iter()
        .map(|f| AttachmentMeta { filename: (*f).into(), media_type: Some("application/pdf".into()), size_bytes: Some(1000), provider_ref: None })
        .collect();
    e
}

pub fn sent(mut e: NormalizedEmail) -> NormalizedEmail {
    e.outbound = true;
    e
}

impl H {
    pub async fn call(&self, method: &str, path: &str, token: &str, body: Option<Value>) -> (StatusCode, Value) {
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

    /// A workspace owned by `OWNER` with these packs.
    pub async fn workspace(&self, packs: &[&str]) -> String {
        let (s, ws) = self.call("POST", "/api/v1/workspaces", OWNER, Some(json!({"name": "Acme", "packs": packs}))).await;
        assert_eq!(s, StatusCode::CREATED, "{ws}");
        ws["id"].as_str().unwrap().to_string()
    }

    /// Signs the user in once, then adds them to the workspace.
    pub async fn member(&self, ws: &str, email: &str, role: &str) -> String {
        let token = format!("dev:{email}");
        self.call("GET", "/api/v1/me", &token, None).await;
        let (s, b) = self.call("POST", &format!("/api/v1/workspaces/{ws}/members"), OWNER, Some(json!({"email": email, "role": role}))).await;
        assert!(s.is_success(), "{email}: {b}");
        token
    }

    pub async fn person(&self, ws: &str, body: Value) -> Value {
        let (s, p) = self.call("POST", &format!("/api/v1/workspaces/{ws}/team/people"), OWNER, Some(body)).await;
        assert_eq!(s, StatusCode::CREATED, "{p}");
        p
    }

    /// A connected Gmail mailbox (no network).
    pub async fn mailbox(&self, ws: &str, address: &str) -> String {
        let id = format!("src_{}", address.replace(['@', '.'], "_"));
        sqlx::query(
            "INSERT INTO email_sources (id, workspace_id, kind, status, external_connection_id, mailbox_address, created_at)
             VALUES (?1, ?2, 'nango_gmail', 'connected', ?1, ?3, 1)",
        )
        .bind(&id)
        .bind(ws)
        .bind(address)
        .execute(&self.state.db)
        .await
        .unwrap();
        id
    }

    /// Stores a message through the same path every mailbox adapter uses. `None` = the pack policy dropped it.
    pub async fn ingest(&self, ws: &str, src: &str, e: NormalizedEmail, thread: &str) -> Option<String> {
        let mut tx = self.state.db.begin().await.unwrap();
        sqlx::query("UPDATE workspaces SET name = name WHERE id = ?1").bind(ws).execute(&mut *tx).await.unwrap();
        let id = ingest::persist_message(&mut tx, src, ws, &e, Some(thread), json!({})).await.unwrap();
        tx.commit().await.unwrap();
        id
    }

    pub async fn classify(&self, message_id: &str, category: &str) {
        sqlx::query(
            "INSERT OR REPLACE INTO classifications (message_id, category, priority, requires_reply, confidence, needs_review, source,
                                                     taxonomy_version, updated_at)
             VALUES (?1, ?2, 'normal', 0, 1.0, 0, 'manual', 'taxonomy-v1', 1)",
        )
        .bind(message_id)
        .bind(category)
        .execute(&self.state.db)
        .await
        .unwrap();
    }

    pub async fn get(&self, token: &str, path: &str) -> (StatusCode, Value) {
        self.call("GET", path, token, None).await
    }
}

pub fn api(ws: &str) -> String {
    format!("/api/v1/workspaces/{ws}")
}

/// A submission row backed by a real stored message, so every foreign key holds.
#[allow(clippy::too_many_arguments)]
pub async fn submission(h: &H, ws: &str, person: &str, src: &str, n: usize, sent_at: i64, outcome: &str, interviewed: bool, status: &str) {
    let msg = h.ingest(ws, src, sent(with_files(email("x@acme.test", &["hr@client.test"], &format!("profile {n}"), "body", sent_at), &["cv.pdf"])), &format!("th{n}")).await.unwrap();
    let thread: String = sqlx::query_scalar("SELECT thread_id FROM email_messages WHERE id = ?1").bind(&msg).fetch_one(&h.state.db).await.unwrap();
    sqlx::query(
        "INSERT INTO submissions (id, workspace_id, person_id, message_id, thread_id, sent_at, client_domain, attachment_name, candidate_key,
                                  outcome, reached_interview, status, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'client.test', 'cv.pdf', ?7, ?8, ?9, ?10, 1, 1)",
    )
    .bind(format!("sub_{person}_{n}"))
    .bind(ws)
    .bind(person)
    .bind(&msg)
    .bind(thread)
    .bind(sent_at)
    .bind(format!("cand{n}"))
    .bind(outcome)
    .bind(interviewed)
    .bind(status)
    .execute(&h.state.db)
    .await
    .unwrap();
}

