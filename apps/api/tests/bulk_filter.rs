//! Deterministic bulk filter: header evidence + unknown sender + not billing-like means the message is
//! filed by rules with no model call. Known contacts, one-signal mail and invoices go through the models.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use email_ai_api::{
    ai::{AiError, Analyst, Classifier, ClassifyInput, ClassifyOutput, Insight},
    build_state,
    config::Config,
    db::{self, new_id, now_ms},
    domain::{ClassificationSource, NormalizedEmail, Priority},
    http::{AppState, router},
    ingest,
    jobs::worker,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const ADMIN: &str = "dev:admin@acme.test";

#[derive(Default)]
struct Calls {
    classify: AtomicUsize,
    analyze: AtomicUsize,
}

struct CountingClassifier(Arc<Calls>);

#[async_trait]
impl Classifier for CountingClassifier {
    async fn classify(&self, input: &ClassifyInput) -> Result<ClassifyOutput, AiError> {
        self.0.classify.fetch_add(1, Ordering::SeqCst);
        let _ = input;
        Ok(ClassifyOutput {
            category: "other".into(),
            priority: Priority::Normal,
            requires_reply: true,
            confidence: Some(0.95),
            source: ClassificationSource::Llm,
            model_id: Some("test".into()),
            model_version: None,
        })
    }
}

struct CountingAnalyst(Arc<Calls>);

#[async_trait]
impl Analyst for CountingAnalyst {
    async fn analyze(&self, subject: &str, _sender: &str, _body: &str) -> Result<(Insight, String), AiError> {
        self.0.analyze.fetch_add(1, Ordering::SeqCst);
        Ok((Insight { summary: format!("About {subject}"), needs_reply: true, ..Default::default() }, "test".into()))
    }
}

struct Env {
    state: AppState,
    app: Router,
    ws: String,
    source: String,
    calls: Arc<Calls>,
    _dir: tempfile::TempDir,
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

async fn setup() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()));
    let mut state = build_state(config).await.unwrap();
    db::migrate(&state.db).await.unwrap();
    let calls = Arc::new(Calls::default());
    state.classifier = Arc::new(CountingClassifier(calls.clone()));
    state.analyst = Arc::new(CountingAnalyst(calls.clone()));
    let app = router(state.clone());
    let (_, ws) = call(&app, "POST", "/api/v1/workspaces", Some(json!({"name":"Acme","template":"hr"}))).await;
    let ws = ws["id"].as_str().unwrap().to_string();
    let (_, src) = call(&app, "POST", &format!("/api/v1/workspaces/{ws}/sources/forwarding"), None).await;
    let source = src["id"].as_str().unwrap().to_string();
    Env { state, app, ws, source, calls, _dir: dir }
}

impl Env {
    async fn receive(&self, from: &str, subject: &str, body: &str, signals: &[&str]) -> String {
        let email = NormalizedEmail {
            source_message_id: new_id("gm"),
            sender_address: from.into(),
            recipients: vec!["me@acme.com".into()],
            subject: subject.into(),
            body_text: body.into(),
            received_at: now_ms(),
            bulk_signals: signals.iter().map(|s| (*s).to_string()).collect(),
            ..Default::default()
        };
        let mut tx = self.state.db.begin().await.unwrap();
        let id = ingest::persist_message(&mut tx, &self.source, &self.ws, &email, Some(&new_id("thr")), json!({})).await.unwrap().unwrap();
        tx.commit().await.unwrap();
        worker::drain(&self.state).await;
        id
    }

    async fn model_calls(&self) -> (usize, usize) {
        (self.calls.classify.load(Ordering::SeqCst), self.calls.analyze.load(Ordering::SeqCst))
    }

    async fn classification(&self, id: &str) -> (String, bool, String, Option<String>) {
        sqlx::query_as("SELECT category, requires_reply, source, model_id FROM classifications WHERE message_id = ?1")
            .bind(id)
            .fetch_one(&self.state.db)
            .await
            .unwrap()
    }
}

const NEWSLETTER: [&str; 2] = ["list_unsubscribe", "gmail_category"];

#[tokio::test]
async fn a_newsletter_is_filed_by_rules_with_no_model_call_and_no_downstream_work() {
    let env = setup().await;
    let id = env.receive("news@brand.example", "Our October picks", "Ten books you will love. Questions? Just reply!", &NEWSLETTER).await;

    assert_eq!(env.model_calls().await, (0, 0));
    let (category, requires_reply, source, model_id) = env.classification(&id).await;
    assert!(["newsletters", "other"].contains(&category.as_str()), "{category}");
    assert_eq!((requires_reply, source.as_str(), model_id.as_deref()), (false, "fallback", Some("bulk-rules-v1")));
    let (needs_reply, insight_model): (bool, String) =
        sqlx::query_as("SELECT needs_reply, model_id FROM message_insights WHERE message_id = ?1").bind(&id).fetch_one(&env.state.db).await.unwrap();
    assert_eq!((needs_reply, insight_model.as_str()), (false, "bulk-rules-v1"));
    let status: String = sqlx::query_scalar("SELECT processing_status FROM email_messages WHERE id = ?1").bind(&id).fetch_one(&env.state.db).await.unwrap();
    assert_eq!(status, "processed");
    let ran: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM jobs WHERE kind IN ('analyze_message','run_automations','execute_workflow','learn_from_message','agent_step')",
    )
    .fetch_one(&env.state.db)
    .await
    .unwrap();
    assert_eq!(ran, 0, "no analysis, automations, workflow, learning or agent for bulk mail");
    let indexed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE kind = 'index_message'").fetch_one(&env.state.db).await.unwrap();
    assert_eq!(indexed, 1, "it is still searchable");
}

#[tokio::test]
async fn a_no_reply_sender_counts_as_the_second_signal() {
    let env = setup().await;
    env.receive("no-reply@brand.example", "Your weekly digest", "Top stories.", &["list_unsubscribe"]).await;
    assert_eq!(env.model_calls().await, (0, 0));
}

#[tokio::test]
async fn billing_mail_with_an_unsubscribe_footer_still_gets_the_models() {
    let env = setup().await;
    env.receive("billing@vendor.example", "Your October statement", "Amount due 1,200 INR by 28 Oct.\nUnsubscribe from billing emails.", &NEWSLETTER).await;
    assert_eq!(env.model_calls().await, (1, 1), "billing@ with list-unsubscribe + category headers is not a newsletter");
}

#[tokio::test]
async fn an_invoice_from_an_ordinary_address_is_not_filed_away_either() {
    let env = setup().await;
    env.receive("ana@vendor.example", "Invoice INV-2291 for October", "Please find the invoice. Unsubscribe.", &NEWSLETTER).await;
    assert_eq!(env.model_calls().await, (1, 1));
}

#[tokio::test]
async fn one_signal_or_a_known_contact_is_never_filed_away() {
    let env = setup().await;
    env.receive("sam@brand.example", "A quick hello", "Hi there, quick question about your offer.", &["list_unsubscribe"]).await;
    assert_eq!(env.model_calls().await, (1, 1), "one signal");

    // We have written to this sender before: a known contact.
    let out = NormalizedEmail {
        source_message_id: new_id("gm"),
        sender_address: "me@acme.com".into(),
        recipients: vec!["promo@shop.example".into()],
        subject: "Order question".into(),
        body_text: "Where is my order?".into(),
        received_at: now_ms() - 10_000,
        outbound: true,
        ..Default::default()
    };
    let mut tx = env.state.db.begin().await.unwrap();
    ingest::persist_message(&mut tx, &env.source, &env.ws, &out, Some("t-out"), json!({})).await.unwrap().unwrap();
    tx.commit().await.unwrap();
    env.receive("promo@shop.example", "Your order has shipped", "Track your parcel.", &NEWSLETTER).await;
    assert_eq!(env.model_calls().await, (2, 2), "known contact");
}

#[tokio::test]
async fn replies_are_not_bulk() {
    let env = setup().await;
    env.receive("news@brand.example", "Re: your subscription question", "Thanks for asking about your plan.", &NEWSLETTER).await;
    assert_eq!(env.model_calls().await, (1, 1));
}

#[tokio::test]
async fn opening_a_filed_email_gives_it_the_full_treatment() {
    let env = setup().await;
    let id = env.receive("news@brand.example", "Our October picks", "Ten books you will love.", &NEWSLETTER).await;
    assert_eq!(env.model_calls().await, (0, 0));

    let (s, v) = call(&env.app, "POST", &format!("/api/v1/workspaces/{}/messages/{id}/insight", env.ws), None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    worker::drain(&env.state).await;
    let (classify, analyze) = env.model_calls().await;
    assert_eq!((classify, analyze), (1, 1), "classified by the model and read once");
    let (_, _, source, model_id) = env.classification(&id).await;
    assert_eq!((source.as_str(), model_id.as_deref()), ("llm", Some("test")));
}
