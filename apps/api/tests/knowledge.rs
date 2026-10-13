//! Knowledge store: indexing through the real pipeline (test-email route + worker), hybrid
//! keyword + meaning search, tenant isolation, idempotent re-indexing, backfill, and RAG
//! examples reaching the classifier and the drafter.

use std::sync::{Arc, Mutex};

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
    },
    build_state,
    config::Config,
    db,
    http::{AppState, router},
    jobs::{knowledge_jobs, worker},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const ADMIN: &str = "dev:admin@acme.test";
const OTHER: &str = "dev:admin@other.test";

#[derive(Default)]
struct Seen {
    classify: Mutex<Vec<ClassifyInput>>,
    draft: Mutex<Vec<DraftInput>>,
}

struct RecordingClassifier(Arc<Seen>);

#[async_trait]
impl Classifier for RecordingClassifier {
    async fn classify(&self, input: &ClassifyInput) -> Result<ClassifyOutput, AiError> {
        self.0.classify.lock().unwrap().push(input.clone());
        RulesClassifier.classify(input).await
    }
}

struct RecordingDrafter(Arc<Seen>);

#[async_trait]
impl Drafter for RecordingDrafter {
    async fn draft(&self, input: &DraftInput) -> Result<DraftOutput, AiError> {
        self.0.draft.lock().unwrap().push(input.clone());
        TemplateDrafter.draft(input).await
    }
}

struct H {
    state: AppState,
    app: Router,
    seen: Arc<Seen>,
    _dir: tempfile::TempDir,
}

async fn harness() -> H {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()));
    let mut state = build_state(config).await.unwrap();
    db::migrate(&state.db).await.unwrap();
    let seen = Arc::new(Seen::default());
    state.classifier = Arc::new(RecordingClassifier(seen.clone()));
    state.drafter = Arc::new(RecordingDrafter(seen.clone()));
    H { app: router(state.clone()), state, seen, _dir: dir }
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

    async fn workspace(&self, token: &str) -> (String, String) {
        let (_, ws) = self.call("POST", "/api/v1/workspaces", token, Some(json!({"name":"Acme"}))).await;
        let ws = ws["id"].as_str().unwrap().to_string();
        let (_, src) = self.call("POST", &format!("/api/v1/workspaces/{ws}/sources/forwarding"), token, None).await;
        (ws, src["id"].as_str().unwrap().to_string())
    }

    /// Sends a test email through the full pipeline; returns its message id.
    async fn email(&self, token: &str, ws: &str, src: &str, subject: &str, body: &str) -> String {
        let path = format!("/api/v1/workspaces/{ws}/sources/{src}/test-email");
        let (s, _) = self.call("POST", &path, token, Some(json!({"subject": subject, "body_text": body}))).await;
        assert_eq!(s, StatusCode::ACCEPTED);
        worker::drain(&self.state).await;
        sqlx::query_scalar("SELECT id FROM email_messages WHERE workspace_id = ?1 AND subject = ?2")
            .bind(ws)
            .bind(subject)
            .fetch_one(&self.state.db)
            .await
            .unwrap()
    }

    async fn search(&self, token: &str, ws: &str, q: &str) -> Vec<Value> {
        let q = q.replace(' ', "%20");
        let (s, body) = self.call("GET", &format!("/api/v1/workspaces/{ws}/search?q={q}&limit=5"), token, None).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        body["items"].as_array().unwrap().clone()
    }

    async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(&self.state.db).await.unwrap()
    }
}

fn subjects(items: &[Value]) -> Vec<&str> {
    items.iter().map(|i| i["subject"].as_str().unwrap()).collect()
}

#[tokio::test]
async fn indexes_mail_and_finds_it_by_keyword_and_meaning() {
    let h = harness().await;
    let (ws, src) = h.workspace(ADMIN).await;
    h.email(ADMIN, &ws, &src, "September timesheet", "Please find attached my hours for September. 168 in total.").await;
    let leave = h.email(ADMIN, &ws, &src, "Annual leave next week", "I would like to take leave from Monday to Friday.").await;
    h.email(ADMIN, &ws, &src, "Office party", "Cake in the kitchen at four, everyone welcome.").await;

    assert_eq!(h.count("SELECT COUNT(*) FROM knowledge_chunks WHERE kind = 'email'").await, 3);
    assert_eq!(h.count("SELECT COUNT(*) FROM knowledge_chunks WHERE embedded = 0").await, 0);
    assert_eq!(h.count("SELECT COUNT(*) FROM knowledge_vec").await, h.count("SELECT COUNT(*) FROM knowledge_chunks").await);

    // Keyword: exact term, with a snippet around it.
    let found = h.search(ADMIN, &ws, "leave").await;
    assert_eq!(found[0]["subject"], "Annual leave next week");
    assert_eq!((found[0]["kind"].as_str(), found[0]["message_id"].as_str()), (Some("email"), Some(leave.as_str())));
    assert!(found[0]["snippet"].as_str().unwrap().contains("leave"));
    assert!(found[0]["score"].as_f64().unwrap() > 0.0);

    // Meaning: a misspelling no keyword index matches still finds the timesheet.
    assert_eq!(h.count("SELECT COUNT(*) FROM knowledge_fts WHERE knowledge_fts MATCH '\"timeshet\"'").await, 0);
    let found = h.search(ADMIN, &ws, "timeshet").await;
    assert_eq!(found[0]["subject"], "September timesheet", "{found:?}");

    // FTS operators in user input are plain words; blank query is empty.
    assert_eq!(h.call("GET", &format!("/api/v1/workspaces/{ws}/search?q=NEAR(%22*"), ADMIN, None).await.0, StatusCode::OK);
    assert!(h.search(ADMIN, &ws, "%20").await.is_empty());

    // Re-indexing unchanged content is a no-op (same chunk rows), and it is all idempotent.
    let before = h.count("SELECT COALESCE(SUM(id), 0) FROM knowledge_chunks").await;
    let mut conn = h.state.db.acquire().await.unwrap();
    knowledge_jobs::enqueue_reindex_message(&mut conn, &ws, &leave, "test").await.unwrap();
    drop(conn);
    worker::drain(&h.state).await;
    assert_eq!(h.count("SELECT COALESCE(SUM(id), 0) FROM knowledge_chunks").await, before);

    // Changed content replaces the chunks and their vectors.
    sqlx::query("UPDATE email_messages SET body_text = 'Leave cancelled, sorry.' WHERE id = ?1")
        .bind(&leave)
        .execute(&h.state.db)
        .await
        .unwrap();
    let mut conn = h.state.db.acquire().await.unwrap();
    knowledge_jobs::enqueue_reindex_message(&mut conn, &ws, &leave, "edited").await.unwrap();
    drop(conn);
    worker::drain(&h.state).await;
    let text: String = sqlx::query_scalar("SELECT text FROM knowledge_chunks WHERE ref_id = ?1")
        .bind(&leave)
        .fetch_one(&h.state.db)
        .await
        .unwrap();
    assert_eq!(text, "Annual leave next week\nLeave cancelled, sorry.");
    assert_eq!(h.count("SELECT COUNT(*) FROM knowledge_vec").await, 3);

    // Deleting chunks removes their vectors (trigger).
    let mut conn = h.state.db.acquire().await.unwrap();
    email_ai_api::knowledge::delete_message(&mut conn, &leave).await.unwrap();
    drop(conn);
    assert_eq!(h.count("SELECT COUNT(*) FROM knowledge_vec").await, 2);
    assert_eq!(h.count("SELECT COUNT(*) FROM knowledge_fts WHERE knowledge_fts MATCH 'cancelled'").await, 0);
}

#[tokio::test]
async fn search_is_tenant_isolated() {
    let h = harness().await;
    let (ws1, src1) = h.workspace(ADMIN).await;
    let (ws2, src2) = h.workspace(OTHER).await;
    h.email(ADMIN, &ws1, &src1, "Leave request for Priya", "Leave from Monday please.").await;
    h.email(OTHER, &ws2, &src2, "Secret payroll leave figures", "Payroll leave figures for the board.").await;

    let mine = h.search(ADMIN, &ws1, "payroll leave figures").await;
    assert_eq!(subjects(&mine), vec!["Leave request for Priya"]);
    let theirs = h.search(OTHER, &ws2, "leave").await;
    assert_eq!(subjects(&theirs), vec!["Secret payroll leave figures"]);
    assert_eq!(h.call("GET", &format!("/api/v1/workspaces/{ws2}/search?q=leave"), ADMIN, None).await.0, StatusCode::FORBIDDEN);

    // RAG examples are isolated too: workspace 2's confirmed label never reaches workspace 1.
    let id2: String = sqlx::query_scalar("SELECT id FROM email_messages WHERE workspace_id = ?1").bind(&ws2).fetch_one(&h.state.db).await.unwrap();
    let (s, _) = h
        .call("PATCH", &format!("/api/v1/workspaces/{ws2}/messages/{id2}/classification"), OTHER, Some(json!({"category":"payroll"})))
        .await;
    assert_eq!(s, StatusCode::OK);
    h.email(ADMIN, &ws1, &src1, "Payroll leave figures question", "Where are the payroll leave figures?").await;
    let last = h.seen.classify.lock().unwrap().last().cloned().unwrap();
    assert!(last.examples.is_empty(), "{:?}", last.examples);
}

#[tokio::test]
async fn trusted_labels_and_approved_replies_reach_the_models() {
    let h = harness().await;
    let (ws, src) = h.workspace(ADMIN).await;
    let base = format!("/api/v1/workspaces/{ws}");

    // Empty store: classification still works, with no examples.
    let first = h.email(ADMIN, &ws, &src, "Hours for September", "My hours for September are 160, see attached sheet.").await;
    assert!(h.seen.classify.lock().unwrap()[0].examples.is_empty());
    // An unrelated email labelled only by rules is not a trustworthy example.
    h.email(ADMIN, &ws, &src, "Hours sheet reminder", "Reminder: hours sheet due.").await;

    // A person labels the first one: it becomes a few-shot example for similar mail.
    let (s, _) = h.call("PATCH", &format!("{base}/messages/{first}/classification"), ADMIN, Some(json!({"category":"timesheet"}))).await;
    assert_eq!(s, StatusCode::OK);
    worker::drain(&h.state).await;

    // And a person approves a reply to it: it becomes a drafting example.
    let (s, _) = h.call("POST", &format!("{base}/messages/{first}/draft"), ADMIN, None).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    worker::drain(&h.state).await;
    let draft_id: String = sqlx::query_scalar("SELECT id FROM drafts WHERE message_id = ?1 ORDER BY created_at DESC, id DESC LIMIT 1")
        .bind(&first)
        .fetch_one(&h.state.db)
        .await
        .unwrap();
    let approved = "Hi, thanks for your hours. Payroll closes on the 25th.\n\nHR Team, Acme";
    let (s, _) = h
        .call("PATCH", &format!("{base}/drafts/{draft_id}"), ADMIN, Some(json!({"body_text": approved, "status": "approved"})))
        .await;
    assert_eq!(s, StatusCode::OK);
    worker::drain(&h.state).await;
    assert_eq!(h.count("SELECT COUNT(*) FROM knowledge_chunks WHERE kind = 'reply' AND embedded = 1").await, 1);
    let found = h.search(ADMIN, &ws, "payroll closes").await;
    assert!(found.iter().all(|i| i["kind"] == "email"), "replies are not search results: {found:?}");

    let second = h.email(ADMIN, &ws, &src, "Hours for October", "My hours for October are 150.").await;
    let input = h.seen.classify.lock().unwrap().last().cloned().unwrap();
    assert_eq!(input.subject, "Hours for October");
    assert_eq!(input.examples.len(), 1, "{:?}", input.examples);
    assert_eq!((input.examples[0].subject.as_str(), input.examples[0].label.as_str()), ("Hours for September", "timesheet"));
    assert!(input.examples[0].snippet.starts_with("My hours for September"));

    let (s, _) = h.call("POST", &format!("{base}/messages/{second}/draft"), ADMIN, None).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    worker::drain(&h.state).await;
    let draft = h.seen.draft.lock().unwrap().last().cloned().unwrap();
    assert_eq!(draft.subject, "Hours for October");
    assert_eq!(draft.examples.len(), 1);
    assert_eq!((draft.examples[0].subject.as_str(), draft.examples[0].reply.as_str()), ("Hours for September", approved));

    // Never its own reply as an example.
    let (s, _) = h.call("POST", &format!("{base}/messages/{first}/draft"), ADMIN, None).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    worker::drain(&h.state).await;
    assert!(h.seen.draft.lock().unwrap().last().unwrap().examples.is_empty());
}

#[tokio::test]
async fn backfill_indexes_existing_mail_and_replies_in_bounded_steps() {
    let h = harness().await;
    let (ws, src) = h.workspace(ADMIN).await;
    let id = h.email(ADMIN, &ws, &src, "September timesheet", "Hours attached.").await;
    h.email(ADMIN, &ws, &src, "Leave request", "Friday off please.").await;
    let draft_id: String =
        sqlx::query_scalar("SELECT id FROM drafts WHERE message_id = ?1").bind(&id).fetch_one(&h.state.db).await.unwrap();
    sqlx::query("UPDATE drafts SET status = 'sent' WHERE id = ?1").bind(&draft_id).execute(&h.state.db).await.unwrap();
    sqlx::query("DELETE FROM knowledge_chunks").execute(&h.state.db).await.unwrap();
    assert_eq!(h.count("SELECT COUNT(*) FROM knowledge_vec").await, 0);

    assert!(knowledge_jobs::enqueue_backfill(&h.state.db).await.unwrap());
    // Simulate mail that predates the store: no index jobs ever ran for it.
    sqlx::query("DELETE FROM jobs WHERE kind = 'index_message'").execute(&h.state.db).await.unwrap();
    worker::drain(&h.state).await;
    assert_eq!(h.count("SELECT COUNT(*) FROM knowledge_chunks WHERE kind = 'email' AND embedded = 1").await, 2);
    assert_eq!(h.count("SELECT COUNT(*) FROM knowledge_chunks WHERE kind = 'reply' AND embedded = 1").await, 1);
    assert_eq!(h.count("SELECT COUNT(*) FROM jobs WHERE kind = 'backfill_knowledge' AND status = 'succeeded'").await, 2);
    assert!(!knowledge_jobs::enqueue_backfill(&h.state.db).await.unwrap(), "a finished backfill is not repeated");
}

#[tokio::test]
async fn new_label_rechecks_other_mail_and_approval_confirms() {
    let h = harness().await;
    let (ws, src) = h.workspace(ADMIN).await;
    let base = format!("/api/v1/workspaces/{ws}");
    let invoice = h.email(ADMIN, &ws, &src, "Invoice 4411 from Globex", "Amount due: 1,200. Bank transfer details inside.").await;
    sqlx::query("UPDATE classifications SET category = 'other', source = 'llm', confidence = 0.6 WHERE message_id = ?1")
        .bind(&invoice)
        .execute(&h.state.db)
        .await
        .unwrap();
    let before = h.seen.classify.lock().unwrap().len();

    let (s, _) = h
        .call("POST", &format!("{base}/labels"), ADMIN, Some(json!({"name":"Invoices","description":"Vendor invoices and payment requests"})))
        .await;
    assert_eq!(s, StatusCode::CREATED);
    worker::drain(&h.state).await;
    {
        let seen = h.seen.classify.lock().unwrap();
        assert_eq!(seen.len(), before + 1, "the other email was re-checked once");
        let last = seen.last().unwrap();
        assert_eq!(last.subject, "Invoice 4411 from Globex");
        assert!(last.labels.iter().any(|l| l.key == "invoices"));
    }

    let (s, _) = h.call("POST", &format!("{base}/messages/{invoice}/classification/approve"), ADMIN, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(h.count("SELECT confirmed FROM classifications").await, 1);
}
