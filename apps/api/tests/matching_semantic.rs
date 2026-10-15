//! Semantic matching (M4): skill synonyms and related skills, vector recall of a candidate with no skill
//! overlap, degraded matching when embeddings fail, and the daily match digest (recording mailer, no real email).

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use email_ai_api::{
    ai::{AiError, embeddings::Embedder},
    build_state,
    platform::config::Config,
    platform::db::{self, now_ms},
    http::{AppState, router},
    jobs::{match_digest, worker},
    platform::mailer::Mailer,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const ADMIN: &str = "dev:admin@acme.test";

#[derive(Default)]
struct Recorder {
    sent: Mutex<Vec<(String, String, String)>>,
}

#[async_trait]
impl Mailer for Recorder {
    async fn send(&self, to: &str, subject: &str, text: &str, _html: &str) -> anyhow::Result<()> {
        self.sent.lock().unwrap().push((to.into(), subject.into(), text.into()));
        Ok(())
    }
}

struct DownEmbedder;

#[async_trait]
impl Embedder for DownEmbedder {
    async fn embed(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, AiError> {
        Err(AiError::Permanent("embedding provider rejected the request".into()))
    }
}

struct H {
    state: AppState,
    app: Router,
    mail: Arc<Recorder>,
    ws: String,
    src: String,
    _dir: tempfile::TempDir,
}

async fn harness(embedder: Option<Arc<dyn Embedder>>) -> H {
    let dir = tempfile::tempdir().unwrap();
    let mut state = build_state(Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()))).await.unwrap();
    db::migrate(&state.db).await.unwrap();
    let mail = Arc::new(Recorder::default());
    state.alert_mailer = Some(mail.clone());
    if let Some(e) = embedder {
        state.embedder = e;
    }
    let app = router(state.clone());
    let mut h = H { state, app, mail, ws: String::new(), src: String::new(), _dir: dir };
    let (_, ws) = h.call("POST", "/api/v1/workspaces", Some(json!({"name":"Wokay","template":"hr"}))).await;
    h.ws = ws["id"].as_str().unwrap().to_string();
    let (_, src) = h.call("POST", &format!("/api/v1/workspaces/{}/sources/forwarding", h.ws), None).await;
    h.src = src["id"].as_str().unwrap().to_string();
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

    async fn email(&self, from: &str, subject: &str, body: &str) {
        let (s, _) = self
            .call("POST", &format!("/api/v1/workspaces/{}/sources/{}/test-email", self.ws, self.src),
                Some(json!({"from": from, "subject": subject, "body_text": body})))
            .await;
        assert_eq!(s, StatusCode::ACCEPTED);
        worker::drain(&self.state).await;
    }

    async fn matches(&self) -> Vec<Value> {
        let (_, m) = self.call("GET", &format!("/api/v1/workspaces/{}/matching/matches", self.ws), None).await;
        m["items"].as_array().unwrap().clone()
    }

    async fn full_stack_requirement_and_referrals(&self) {
        self.email("Recruiter <hr@infosys.com>", "Requirement: Full Stack Developer",
            "Need a full stack developer with React, Spring Boot. 4-7 yrs. Location Bangalore.").await;
        self.email("Friend <friend@gmail.com>", "Referral - Vikram Shah",
            "Name: Vikram Shah\n5 years with ReactJS, SpringBoot. Bangalore. Immediate joiner.").await;
        self.email("Friend <friend@gmail.com>", "Referral - Neha Gupta",
            "Name: Neha Gupta\n5 years with Next.js and Spring. Bangalore.").await;
    }
}

fn reasons(m: &Value) -> Vec<String> {
    m["reasons"].as_array().unwrap().iter().map(|r| r["text"].as_str().unwrap().to_string()).collect()
}

fn by_name<'a>(items: &'a [Value], name: &str) -> Option<&'a Value> {
    items.iter().find(|m| m["candidate"]["name"] == name)
}

#[tokio::test]
async fn synonyms_match_fully_and_related_skills_earn_half() {
    let h = harness(None).await;
    h.full_stack_requirement_and_referrals().await;
    let items = h.matches().await;

    let vikram = by_name(&items, "Vikram Shah").expect("synonym CV matched");
    let r = reasons(vikram);
    assert!(r.contains(&"Has 2/2 must-have skills: react, spring boot".to_string()), "{r:?}");
    assert!(!r.iter().any(|t| t.starts_with("Missing")), "{r:?}");
    assert!(r.iter().any(|t| t.contains("(similarity 0.")), "{r:?}");
    assert!(vikram["score"].as_i64().unwrap() >= 85, "50 skills + 20 exp + 15 location + semantic");

    let neha = by_name(&items, "Neha Gupta").expect("related skills matched");
    let r = reasons(neha);
    assert!(r.contains(&"Has related skill: next.js (for react)".to_string()), "{r:?}");
    assert!(r.contains(&"Has related skill: spring (for spring boot)".to_string()), "{r:?}");
    assert!(neha["score"].as_i64().unwrap() < vikram["score"].as_i64().unwrap());

    let (_, cands) = h.call("GET", &format!("/api/v1/workspaces/{}/matching/candidates", h.ws), None).await;
    let vikram_skills = cands["items"].as_array().unwrap().iter().find(|c| c["name"] == "Vikram Shah").unwrap()["skills"].clone();
    assert_eq!(vikram_skills, json!(["spring boot", "react"]), "normalised at extraction");
}

#[tokio::test]
async fn vector_recall_finds_a_similar_profile_without_skill_overlap() {
    let h = harness(None).await;
    // Svelte is not in the taxonomy: Kiran shares no skill with the requirement, only the kind of work.
    h.email("Friend <friend@gmail.com>", "Referral - Kiran Rao",
        "Name: Kiran Rao\nFrontend engineer building responsive web interfaces and design systems with Svelte. 4 years. Hyderabad.").await;
    h.email("Recruiter <hr@tcs.com>", "Requirement: Frontend Engineer",
        "Looking for a frontend engineer to build responsive web interfaces and design systems with Vue. 3-6 yrs. Location Hyderabad.").await;
    // Same experience and city, unrelated work: recalled (only one requirement exists) but scored below 40.
    h.email("Friend <friend@gmail.com>", "Referral - Meena Iyer",
        "Name: Meena Iyer\nPayroll accountant handling GST filings, invoices and audits. 4 years. Hyderabad.").await;

    let items = h.matches().await;
    let kiran = by_name(&items, "Kiran Rao").expect("recalled by profile similarity");
    let r = reasons(kiran);
    assert!(r.contains(&"Missing: vue".to_string()), "no skill overlap: {r:?}");
    assert!(
        r.iter().any(|t| t.starts_with("Profile closely matches the role (similarity 0.") || t.starts_with("Profile is somewhat similar to the role (similarity 0.")),
        "{r:?}"
    );
    assert!(kiran["score"].as_i64().unwrap() >= 40);
    assert!(by_name(&items, "Meena Iyer").is_none(), "dissimilar profile stays below the threshold: {items:?}");

    let profiles: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM match_profiles").fetch_one(&h.state.db).await.unwrap();
    let vectors: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM match_profile_vec").fetch_one(&h.state.db).await.unwrap();
    assert_eq!((profiles, vectors), (3, 3), "one profile embedding per record");

    // Re-processing unchanged records does not re-embed (same content hash) and keeps one match.
    let before: Vec<i64> = sqlx::query_scalar("SELECT updated_at FROM match_profiles ORDER BY id").fetch_all(&h.state.db).await.unwrap();
    let (_, msgs) = h.call("GET", &format!("/api/v1/workspaces/{}/messages", h.ws), None).await;
    for msg in msgs["items"].as_array().unwrap() {
        h.call("POST", &format!("/api/v1/workspaces/{}/messages/{}/reprocess", h.ws, msg["id"].as_str().unwrap()), None).await;
    }
    worker::drain(&h.state).await;
    let after: Vec<i64> = sqlx::query_scalar("SELECT updated_at FROM match_profiles ORDER BY id").fetch_all(&h.state.db).await.unwrap();
    assert_eq!(before, after);
    assert_eq!(h.matches().await.len(), 1);
}

#[tokio::test]
async fn embedding_outage_degrades_to_skill_matching() {
    let h = harness(Some(Arc::new(DownEmbedder))).await;
    h.full_stack_requirement_and_referrals().await;
    let items = h.matches().await;
    let vikram = by_name(&items, "Vikram Shah").expect("still matched on skills");
    assert_eq!(vikram["score"].as_i64(), Some(93), "50 + 20 + 15 + half of the semantic 15");
    assert!(!reasons(vikram).iter().any(|t| t.contains("similarity")));
    let profiles: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM match_profiles").fetch_one(&h.state.db).await.unwrap();
    assert_eq!(profiles, 0);
}

#[tokio::test]
async fn daily_digest_sends_new_strong_matches_once_per_day() {
    let h = harness(None).await;
    let now = now_ms();
    assert_eq!(match_digest::send_match_digest(&h.state, &h.ws, now).await.unwrap(), 0, "nothing new: nothing sent");
    assert!(h.mail.sent.lock().unwrap().is_empty());

    h.full_stack_requirement_and_referrals().await;
    let items = h.matches().await;
    let strong: Vec<&Value> = items.iter().filter(|m| m["score"].as_i64().unwrap() >= 70).collect();
    assert!(!strong.is_empty());
    // A recruiter's decision takes a match out of the digest.
    let weak_or_decided = by_name(&items, "Neha Gupta").unwrap();
    h.call("PATCH", &format!("/api/v1/workspaces/{}/matching/matches/{}", h.ws, weak_or_decided["id"].as_str().unwrap()),
        Some(json!({"status": "dismissed"}))).await;

    let now = now_ms();
    assert_eq!(match_digest::send_match_digest(&h.state, &h.ws, now).await.unwrap(), 1);
    let sent = h.mail.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    let (to, subject, text) = &sent[0];
    assert_eq!(to, "admin@acme.test");
    assert_eq!(subject, "[Post Room] 1 new strong match for 1 requirement");
    let req_id = items[0]["requirement"]["id"].as_str().unwrap();
    assert!(text.contains(&format!("https://mail.anukulraj.tech/w/{}/matching?req={req_id}", h.ws)), "{text}");
    assert!(text.contains("Full Stack Developer (Infosys)") && text.contains("Vikram Shah"), "{text}");
    assert!(!text.contains("Neha Gupta"), "dismissed matches are left out");

    assert_eq!(match_digest::send_match_digest(&h.state, &h.ws, now).await.unwrap(), 0, "once per day");
    assert_eq!(h.mail.sent.lock().unwrap().len(), 1);
    assert_eq!(match_digest::send_match_digest(&h.state, &h.ws, now + 2 * 86_400_000).await.unwrap(), 0, "no new matches later");

    // The worker schedules one digest job per workspace per day.
    match_digest::schedule(&h.state).await.unwrap();
    match_digest::schedule(&h.state).await.unwrap();
    let jobs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE kind = 'send_match_digest' AND workspace_id = ?1")
        .bind(&h.ws)
        .fetch_one(&h.state.db)
        .await
        .unwrap();
    assert_eq!(jobs, 1);
}
