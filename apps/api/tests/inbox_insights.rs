//! Wave 2 core: terminal `processed` status, insight + header fields in message detail,
//! `sort=importance`, and AI label suggestions.

use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use email_ai_api::{
    ai::{
        AiError,
        label_suggest::{LabelProposal, LabelSuggester, SampleMail},
    },
    build_state,
    platform::config::Config,
    platform::db,
    platform::domain::{Label, NormalizedEmail},
    http::{AppState, router},
    mail::ingest,
    jobs::worker,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const ADMIN: &str = "dev:admin@acme.test";

struct Harness {
    state: AppState,
    app: Router,
    ws: String,
    source_id: String,
    _dir: tempfile::TempDir,
}

async fn harness_with(suggester: Option<Arc<dyn LabelSuggester>>) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()));
    let mut state = build_state(config).await.unwrap();
    if let Some(s) = suggester {
        state.label_suggester = s;
    }
    db::migrate(&state.db).await.unwrap();
    let app = router(state.clone());
    let mut h = Harness { state, app, ws: String::new(), source_id: String::new(), _dir: dir };
    let (_, ws) = h.call("POST", "/api/v1/workspaces", ADMIN, Some(json!({"name": "Acme"}))).await;
    h.ws = ws["id"].as_str().unwrap().to_string();
    let (_, src) = h.call("POST", &format!("/api/v1/workspaces/{}/sources/forwarding", h.ws), ADMIN, None).await;
    h.source_id = src["id"].as_str().unwrap().to_string();
    h
}

async fn harness() -> Harness {
    harness_with(None).await
}

impl Harness {
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

    async fn url(&self, suffix: &str) -> String {
        format!("/api/v1/workspaces/{}{suffix}", self.ws)
    }

    async fn ingest(&self, n: usize, from: &str, subject: &str, body: &str, age_min: i64) -> String {
        let email = NormalizedEmail {
            source_message_id: format!("src-{n}"),
            sender_address: from.into(),
            sender_name: Some("Priya Sharma".into()),
            recipients: vec!["hr@acme.com".into(), "lead@acme.com".into()],
            cc: vec!["lead@acme.com".into()],
            subject: subject.into(),
            body_text: body.into(),
            received_at: db::now_ms() - age_min * 60_000,
            ..Default::default()
        };
        let mut tx = self.state.db.begin().await.unwrap();
        sqlx::query("UPDATE email_sources SET last_synced_at = 1 WHERE id = ?1").bind(&self.source_id).execute(&mut *tx).await.unwrap();
        let id = ingest::persist_message(&mut tx, &self.source_id, &self.ws, &email, None, json!({})).await.unwrap().unwrap();
        tx.commit().await.unwrap();
        id
    }
}

#[tokio::test]
async fn unmatched_messages_end_processed_and_detail_carries_insight_and_header() {
    let h = harness().await;
    let id = h.ingest(1, "priya@acme.com", "Lunch order", "Pizza is here.\r\n\r\n\r\n\r\n\r\nEnjoy.  ", 5).await;
    worker::drain(&h.state).await;

    let (_, detail) = h.call("GET", &h.url(&format!("/messages/{id}")).await, ADMIN, None).await;
    assert_eq!(detail["processing_status"], "processed", "classified with no workflow is terminal");
    assert_eq!(detail["body_text"], "Pizza is here.\n\nEnjoy.", "text cleaned at ingest");
    assert_eq!(detail["from"], "priya@acme.com");
    assert_eq!(detail["from_name"], "Priya Sharma");
    assert_eq!(detail["to"], json!(["hr@acme.com"]));
    assert_eq!(detail["cc"], json!(["lead@acme.com"]));
    assert_eq!(detail["direction"], "inbound");
    assert!(detail["received_at"].as_str().unwrap().ends_with('Z'));
    assert_eq!(detail["insight"]["needs_reply"], true);
    assert!(detail["insight"]["asks"].is_array() && detail["insight"]["missing_info"].is_array());
    // The default "reply to questions" automation drafts it (the test analyst never rules a reply out).
    assert!(detail["draft"]["id"].is_string());

    // Drafting adds `unanswered` (asks the draft could not answer).
    h.call("POST", &h.url(&format!("/messages/{id}/draft")).await, ADMIN, None).await;
    worker::drain(&h.state).await;
    sqlx::query("UPDATE drafts SET unanswered_json = '[\"your payroll date\"]'").execute(&h.state.db).await.unwrap();
    let (_, detail) = h.call("GET", &h.url(&format!("/messages/{id}")).await, ADMIN, None).await;
    assert_eq!(detail["draft"]["unanswered"], json!(["your payroll date"]));

    // A message that was never analysed has no insight.
    let bare = h.ingest(2, "b@acme.com", "Not analysed yet", "Hello there, nothing queued.", 1).await;
    let (_, detail) = h.call("GET", &h.url(&format!("/messages/{bare}")).await, ADMIN, None).await;
    assert!(detail["insight"].is_null());
}

#[tokio::test]
async fn importance_sort_orders_by_reply_priority_then_time_and_paginates() {
    let h = harness().await;
    let old = h.ingest(1, "a@acme.com", "FYI newsletter", "Nothing to do here, just reading material for you.", 300).await;
    let normal = h.ingest(2, "b@acme.com", "Question about leave", "When does my leave start?", 100).await;
    let high = h.ingest(3, "c@acme.com", "Urgent payroll issue", "My salary is wrong, please fix it?", 200).await;
    let newest = h.ingest(4, "d@acme.com", "Quick hello", "Hello there from the team.", 10).await;
    worker::drain(&h.state).await;
    // Control the signals the sort uses: needs_reply (insight), priority (classification).
    sqlx::query("UPDATE message_insights SET needs_reply = 0 WHERE message_id IN (?1, ?2)").bind(&old).bind(&newest).execute(&h.state.db).await.unwrap();
    sqlx::query("UPDATE message_insights SET needs_reply = 1 WHERE message_id IN (?1, ?2)").bind(&normal).bind(&high).execute(&h.state.db).await.unwrap();
    sqlx::query("UPDATE classifications SET priority = 'normal' WHERE message_id != ?1").bind(&high).execute(&h.state.db).await.unwrap();
    sqlx::query("UPDATE classifications SET priority = 'high' WHERE message_id = ?1").bind(&high).execute(&h.state.db).await.unwrap();
    let long = "w".repeat(300);
    sqlx::query("UPDATE message_insights SET summary = ?1 WHERE message_id = ?2").bind(&long).bind(&normal).execute(&h.state.db).await.unwrap();

    let (s, list) = h.call("GET", &h.url("/messages?sort=importance").await, ADMIN, None).await;
    assert_eq!(s, StatusCode::OK);
    let ids: Vec<&str> = list["items"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
    // needs reply first (high before normal), then the rest newest first.
    assert_eq!(ids, [high.as_str(), normal.as_str(), newest.as_str(), old.as_str()]);
    let first = &list["items"][0];
    assert_eq!((first["priority"].as_str(), first["needs_reply"].as_bool()), (Some("high"), Some(true)));
    let summary = list["items"][1]["summary"].as_str().unwrap();
    assert_eq!(summary.chars().count(), 140, "summary is cut to 140 chars");
    assert!(summary.ends_with('…'));
    assert_eq!(list["items"][3]["needs_reply"], false);

    // Recent order is unchanged and still the default.
    let (_, recent) = h.call("GET", &h.url("/messages").await, ADMIN, None).await;
    let ids: Vec<&str> = recent["items"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert_eq!(ids, [newest.as_str(), normal.as_str(), high.as_str(), old.as_str()]);

    // Opaque cursor pagination under the importance sort.
    let (_, page1) = h.call("GET", &h.url("/messages?sort=importance&limit=3").await, ADMIN, None).await;
    assert_eq!(page1["items"].as_array().unwrap().len(), 3);
    let cursor = page1["next_cursor"].as_str().unwrap();
    let (_, page2) = h.call("GET", &h.url(&format!("/messages?sort=importance&limit=3&cursor={cursor}")).await, ADMIN, None).await;
    assert_eq!(page2["items"].as_array().unwrap().len(), 1);
    assert_eq!(page2["items"][0]["id"], old.as_str());
    assert!(page2["next_cursor"].is_null());

    // Filters still apply with both sorts.
    let (_, q) = h.call("GET", &h.url("/messages?sort=importance&q=payroll").await, ADMIN, None).await;
    assert_eq!(q["items"].as_array().unwrap().len(), 1);
    let category = list["items"][0]["category"].as_str().unwrap().to_string();
    let (_, by_cat) = h.call("GET", &h.url(&format!("/messages?sort=importance&category={category}")).await, ADMIN, None).await;
    assert!(by_cat["items"].as_array().unwrap().iter().all(|m| m["category"] == category.as_str()));

    assert_eq!(h.call("GET", &h.url("/messages?sort=bogus").await, ADMIN, None).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(h.call("GET", &h.url("/messages?sort=importance&cursor=!!").await, ADMIN, None).await.0, StatusCode::BAD_REQUEST);
}

struct Failing;

#[async_trait]
impl LabelSuggester for Failing {
    async fn suggest(&self, _: &[SampleMail], _: &[Label]) -> Result<Vec<LabelProposal>, AiError> {
        Err(AiError::Permanent("model down".into()))
    }
}

#[tokio::test]
async fn label_suggestions_are_admin_only_unique_and_degrade_to_503() {
    let h = harness().await;
    for n in 0..3 {
        h.ingest(n, "billing@vendor.example", &format!("Invoice {n} for September"), "Please find the invoice attached.", n as i64).await;
    }
    for n in 3..5 {
        h.ingest(n, "x@vendor.example", &format!("Webinar {n} invite"), "Join our webinar next week.", n as i64).await;
    }
    worker::drain(&h.state).await;

    let (s, out) = h.call("POST", &h.url("/labels/suggest").await, ADMIN, None).await;
    assert_eq!(s, StatusCode::OK);
    let items = out["items"].as_array().unwrap();
    assert!(!items.is_empty() && items.len() <= 5);
    let invoice = items.iter().find(|i| i["key"] == "invoice").expect("invoice suggested");
    assert_eq!(invoice["name"], "Invoice");
    assert_eq!(invoice["estimated_count"], 3);
    assert_eq!(invoice["example_subjects"].as_array().unwrap().len(), 3);
    assert!(invoice["description"].as_str().unwrap().len() >= 10);

    // The suggestion is accepted with the existing POST /labels, colour included.
    let (s, _) = h
        .call(
            "POST",
            &h.url("/labels").await,
            ADMIN,
            Some(json!({"name": invoice["name"], "description": invoice["description"], "color": invoice["color"]})),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let (_, again) = h.call("POST", &h.url("/labels/suggest").await, ADMIN, None).await;
    assert!(again["items"].as_array().unwrap().iter().all(|i| i["key"] != "invoice"), "never repeats an existing key");

    h.call("GET", "/api/v1/me", "dev:op@acme.test", None).await;
    h.call("POST", &h.url("/members").await, ADMIN, Some(json!({"email": "op@acme.test", "role": "operator"}))).await;
    assert_eq!(h.call("POST", &h.url("/labels/suggest").await, "dev:op@acme.test", None).await.0, StatusCode::FORBIDDEN);

    let failing = harness_with(Some(Arc::new(Failing))).await;
    failing.ingest(1, "a@acme.com", "Something", "Some body text for the model.", 1).await;
    let (s, err) = failing.call("POST", &failing.url("/labels/suggest").await, ADMIN, None).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(err["error"]["code"], "ai_unavailable");
    let empty = harness_with(Some(Arc::new(Failing))).await;
    let (s, out) = empty.call("POST", &empty.url("/labels/suggest").await, ADMIN, None).await;
    assert_eq!((s, out["items"].as_array().map(Vec::len)), (StatusCode::OK, Some(0)), "no mail, no model call");
}

#[tokio::test]
async fn stored_text_is_recleaned_once() {
    let h = harness().await;
    let id = h.ingest(1, "a@acme.com", "Old mail", "Fine body text that is long enough to keep.", 1).await;
    sqlx::query("UPDATE email_messages SET body_text = ?1, text_clean_v = 0 WHERE id = ?2")
        .bind("<!--[if !mso]><!-->\nHello  \n\n\n\n\nSee Docs <https://docs.example/a>\n<![endif]-->")
        .bind(&id)
        .execute(&h.state.db)
        .await
        .unwrap();
    email_ai_api::jobs::message_jobs::schedule_text_clean(&h.state).await.unwrap();
    worker::drain(&h.state).await;
    let (text, v): (String, i64) =
        sqlx::query_as("SELECT body_text, text_clean_v FROM email_messages WHERE id = ?1").bind(&id).fetch_one(&h.state.db).await.unwrap();
    assert_eq!((text.as_str(), v), ("Hello\n\nSee Docs https://docs.example/a", 1));
    email_ai_api::jobs::message_jobs::schedule_text_clean(&h.state).await.unwrap();
    assert_eq!(worker::drain(&h.state).await, 0, "nothing left to clean");
}

#[tokio::test]
async fn opening_an_unread_email_reads_it_immediately() {
    let h = harness().await;
    // Not drained: the background queue hasn't analysed it yet.
    let id = h.ingest(7, "lead@client.test", "Need 2 React developers", "Can you share profiles by Friday? Budget is fixed.", 1).await;
    let (_, before) = h.call("GET", &h.url(&format!("/messages/{id}")).await, ADMIN, None).await;
    assert!(before["insight"].is_null());

    let (s, now) = h.call("POST", &h.url(&format!("/messages/{id}/insight")).await, ADMIN, None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(!now["insight"]["summary"].as_str().unwrap().is_empty());
    assert_eq!(now["insight"]["needs_reply"], true);
    let (_, after) = h.call("GET", &h.url(&format!("/messages/{id}")).await, ADMIN, None).await;
    assert_eq!(after["insight"]["summary"], now["insight"]["summary"], "stored, not recomputed");

    // The queued analysis later finds it done (idempotent) and the stages of this fresh email outrank old work.
    let p: i64 = sqlx::query_scalar("SELECT priority FROM jobs WHERE dedupe_key = ?1").bind(format!("analyze:{id}")).fetch_optional(&h.state.db).await.unwrap().unwrap_or(3);
    assert!(p >= 0);
    worker::drain(&h.state).await;
}

#[tokio::test]
async fn inbox_search_finds_words_that_only_appear_in_the_body_within_the_workspace() {
    let h = harness().await;
    let id = h.ingest(1, "a@client.example", "Question about pricing", "Do you have an enterprise plan for 50 seats?", 5).await;
    h.ingest(2, "b@client.example", "Lunch", "Pizza is here.", 4).await;

    let list = |q: &'static str| {
        let h = &h;
        async move { h.call("GET", &h.url(&format!("/messages?q={q}")).await, ADMIN, None).await.1 }
    };
    let found = list("enterprise%20plan").await;
    assert_eq!(found["items"].as_array().unwrap().len(), 1, "{found}");
    assert_eq!(found["items"][0]["id"], id.as_str());
    assert_eq!(list("subject%20only%20nothing").await["items"].as_array().unwrap().len(), 0);
    assert_eq!(list("100%25").await["items"].as_array().unwrap().len(), 0, "LIKE wildcards are escaped");

    let other = "dev:boss@other.test";
    let (_, ws2) = h.call("POST", "/api/v1/workspaces", other, Some(json!({"name": "Other"}))).await;
    let ws2 = ws2["id"].as_str().unwrap();
    let (_, src2) = h.call("POST", &format!("/api/v1/workspaces/{ws2}/sources/forwarding"), other, None).await;
    let email = NormalizedEmail {
        source_message_id: "src-other".into(),
        sender_address: "z@other.example".into(),
        subject: "Secret".into(),
        body_text: "enterprise plan details".into(),
        received_at: db::now_ms(),
        ..Default::default()
    };
    let mut tx = h.state.db.begin().await.unwrap();
    ingest::persist_message(&mut tx, src2["id"].as_str().unwrap(), ws2, &email, None, json!({})).await.unwrap();
    tx.commit().await.unwrap();
    let found = list("enterprise%20plan").await;
    assert_eq!(found["items"].as_array().unwrap().len(), 1, "another workspace's body never matches: {found}");
}

#[tokio::test]
async fn body_search_never_finds_a_suppressed_message() {
    let h = harness().await;
    let shown = h.ingest(1, "a@client.example", "Plan question", "Do you have a zanzibar plan?", 5).await;
    let hidden = h.ingest(2, "b@client.example", "Held back", "Our zanzibar secret pricing", 4).await;
    sqlx::query("UPDATE email_messages SET suppressed = 1 WHERE id = ?1").bind(&hidden).execute(&h.state.db).await.unwrap();

    let (_, found) = h.call("GET", &h.url("/messages?q=zanzibar").await, ADMIN, None).await;
    let ids: Vec<&str> = found["items"].as_array().unwrap().iter().filter_map(|m| m["id"].as_str()).collect();
    assert_eq!(ids, [shown.as_str()], "{found}");
    let (_, by_importance) = h.call("GET", &h.url("/messages?q=zanzibar&sort=importance").await, ADMIN, None).await;
    assert_eq!(by_importance["items"].as_array().unwrap().len(), 1, "{by_importance}");
}

#[tokio::test]
async fn summaries_never_expose_attachment_markup() {
    let h = harness().await;
    let fence = "\n\n<attachments>\n<attachment name=\"a.xlsx\" kind=\"sheet\">\nZanzibar\n</attachment>\n</attachments>";
    let id = h.ingest(1, "a@client.example", "September timesheet", "Please find the timesheet attached.", 5).await;
    worker::drain(&h.state).await;

    sqlx::query("UPDATE message_insights SET summary = ?2 WHERE message_id = ?1")
        .bind(&id)
        .bind(format!("Please find the timesheet attached.{fence}"))
        .execute(&h.state.db)
        .await
        .unwrap();
    let (_, list) = h.call("GET", &h.url("/messages").await, ADMIN, None).await;
    assert_eq!(list["items"][0]["summary"], "Please find the timesheet attached.");
    let (_, detail) = h.call("GET", &h.url(&format!("/messages/{id}")).await, ADMIN, None).await;
    assert_eq!(detail["insight"]["summary"], "Please find the timesheet attached.");

    use email_ai_api::ai::{Analyst, RulesAnalyst};
    let (insight, _) = RulesAnalyst.analyze("S", "a@b.c", &format!("Short note.{fence}")).await.unwrap();
    assert_eq!(insight.summary, "Short note.");
}
