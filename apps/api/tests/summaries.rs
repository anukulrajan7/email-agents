//! Summaries read like a person talking to the mailbox owner: golden fixtures of real-shaped Indian bank, card,
//! invoice and salary mail, with a scripted model that answers in the robotic form (or invents an amount).
//! The final text is made in code: ₹ grouping, "card ending NNNN", "12 Oct", amounts that are in the email.

use std::sync::Arc;

use async_trait::async_trait;
use email_ai_api::{
    ai::{
        AiError, Analyst, Insight,
        understand::{UnderstandInput, parse_understanding},
    },
    build_state,
    platform::config::Config,
    platform::db,
    platform::domain::NormalizedEmail,
    http::{AppState, router},
    mail::ingest,
};
use axum::{Router, body::Body, http::Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const ADMIN: &str = "dev:admin@acme.test";
/// 14 Oct 2025.
const RECEIVED: i64 = 1_760_400_000_000;

struct Fixture {
    name: &'static str,
    subject: &'static str,
    body: &'static str,
    robotic: &'static str,
    expected: &'static str,
}

const FIXTURES: &[Fixture] = &[
    Fixture {
        name: "bank debit",
        subject: "Transaction alert for your HDFC Bank account",
        body: "Dear Customer,\nINR 2,500.00 has been debited from your A/c XX1234 on 03-11-2025 towards UPI-RAHUL KUMAR. Avl Bal INR 41,200.00.\nIf this was not you, call 1800 202 6161.",
        robotic: "INR 2,500.00 debited from account XX1234 on 03-11-2025. Avl Bal INR 41,200.00.",
        expected: "₹2,500 was debited from your account ending 1234 on 3 Nov. Avl Bal ₹41,200.",
    },
    Fixture {
        name: "credit card spend",
        subject: "Alert: Transaction on your Credit Card",
        body: "Rs.8138.76 spent on your HDFC Bank Credit Card ending 4755 at AMAZON on 12-10-2025 14:02:11.\nAvailable limit Rs.1,91,861.24.",
        robotic: "INR 8,138.76 spent on credit card XX4755 at Amazon on 12-10-2025.",
        expected: "You spent ₹8,138.76 on your card ending 4755 at Amazon on 12 Oct.",
    },
    Fixture {
        name: "invoice due",
        subject: "Invoice 2231 from Acme Supplies",
        body: "Hello,\nPlease find invoice 2231 for Rs. 1,18,000.00 attached. Payment is due by 30 November 2025.\nRegards,\nAcme Supplies",
        robotic: "Invoice 2231 of INR 118000.00 from Acme Supplies due 30 November 2025.",
        expected: "Invoice 2231 of ₹1,18,000 from Acme Supplies due 30 Nov.",
    },
    Fixture {
        name: "salary credit",
        subject: "Salary credited",
        body: "Dear Employee,\nINR 85,000.00 has been credited to your account XXXXXXXX9876 on 01-10-2025 towards salary for September 2025.",
        robotic: "INR 85,000.00 credited to account XXXXXXXX9876 on 01-10-2025 (salary).",
        expected: "₹85,000 was credited to your account ending 9876 on 1 Oct (salary).",
    },
];

fn input(f: &Fixture) -> UnderstandInput {
    UnderstandInput {
        subject: f.subject.into(),
        sender: "alerts@bank.example".into(),
        received_at_ms: RECEIVED,
        new_part: f.body.into(),
        attachments_block: String::new(),
        attachment_names: vec![],
        labels: vec![],
        examples: vec![],
        sender_history: None,
        thread_digest: None,
    }
}

fn model_reply(summary: &str) -> String {
    json!({
        "label": "other", "label_confidence": 0.9, "priority": "normal", "needs_reply": false, "intent": "other",
        "sentiment": "calm", "summary": summary, "asks": [], "commitments": [], "deadlines": [], "entities": [],
        "missing_info": [], "facts_to_remember": [], "alert_signals": [], "injection_signals": [], "language": "en"
    })
    .to_string()
}

#[test]
fn robotic_model_summaries_are_rewritten_in_chat_voice() {
    for f in FIXTURES {
        let u = parse_understanding(&model_reply(f.robotic), &input(f)).unwrap();
        assert_eq!(u.summary, f.expected, "{}", f.name);
        assert!(!u.summary.contains("INR") && !u.summary.contains("XX"), "{}: {}", f.name, u.summary);
    }
}

#[test]
fn an_amount_the_email_does_not_contain_is_dropped_and_flagged() {
    let f = &FIXTURES[1];
    let u = parse_understanding(&model_reply("You spent ₹9,999 on your card ending 4755 at Amazon on 12 Oct."), &input(f)).unwrap();
    assert_eq!(u.summary, "You spent an amount on your card ending 4755 at Amazon on 12 Oct. Check the amount in the email.");
    assert!(!u.summary.contains("9,999"));
}

#[test]
fn a_good_summary_passes_through_unchanged() {
    for f in FIXTURES {
        let u = parse_understanding(&model_reply(f.expected), &input(f)).unwrap();
        assert_eq!(u.summary, f.expected, "{}: polishing twice must not change the text", f.name);
    }
}

#[test]
fn the_year_is_shown_only_when_it_is_not_the_year_of_the_email() {
    let mut f = input(&FIXTURES[1]);
    f.new_part = "Rs.500 spent on your Credit Card ending 4755 at SWIGGY on 12-10-2024.".into();
    let u = parse_understanding(&model_reply("INR 500 spent on credit card XX4755 at Swiggy on 12-10-2024."), &f).unwrap();
    assert_eq!(u.summary, "You spent ₹500 on your card ending 4755 at Swiggy on 12 Oct 2024.");
}

#[test]
fn rules_fallback_uses_the_same_voice() {
    for (f, expected) in [
        (&FIXTURES[1], "You spent ₹8,138.76 on your card ending 4755 at Amazon on 12 Oct."),
        (&FIXTURES[3], "Your salary of ₹85,000 was credited to your account ending 9876 on 1 Oct."),
    ] {
        let got = email_ai_api::platform::format::summary::rules_summary(f.subject, f.body, RECEIVED);
        assert_eq!(got, expected, "{}", f.name);
    }
}

// ---------------------------------------------------------------- the analyze job

struct RoboticAnalyst(&'static str);

#[async_trait]
impl Analyst for RoboticAnalyst {
    async fn analyze(&self, _subject: &str, _sender: &str, _body: &str) -> Result<(Insight, String), AiError> {
        Ok((Insight { summary: self.0.into(), needs_reply: false, ..Default::default() }, "scripted".into()))
    }
}

async fn call(app: &Router, method: &str, path: &str, body: Option<Value>) -> Value {
    let req = Request::builder().method(method).uri(path).header("x-auth-token", ADMIN);
    let req = match body {
        Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let res = app.clone().oneshot(req).await.unwrap();
    serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap_or(Value::Null)
}

async fn stored_summary(analyst: &'static str, f: &Fixture) -> String {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()));
    let mut state: AppState = build_state(config).await.unwrap();
    state.analyst = Arc::new(RoboticAnalyst(analyst));
    db::migrate(&state.db).await.unwrap();
    let app = router(state.clone());
    let ws = call(&app, "POST", "/api/v1/workspaces", Some(json!({"name": "Acme"}))).await["id"].as_str().unwrap().to_string();
    let source = call(&app, "POST", &format!("/api/v1/workspaces/{ws}/sources/forwarding"), None).await["id"].as_str().unwrap().to_string();
    let email = NormalizedEmail {
        source_message_id: "bank-1".into(),
        sender_address: "alerts@bank.example".into(),
        recipients: vec!["me@acme.com".into()],
        subject: f.subject.into(),
        body_text: f.body.into(),
        received_at: RECEIVED,
        ..Default::default()
    };
    let mut tx = state.db.begin().await.unwrap();
    sqlx::query("UPDATE email_sources SET last_synced_at = 1 WHERE id = ?1").bind(&source).execute(&mut *tx).await.unwrap();
    let id = ingest::persist_message(&mut tx, &source, &ws, &email, None, json!({})).await.unwrap().unwrap();
    tx.commit().await.unwrap();
    email_ai_api::jobs::handlers::analyze_message(&state, &json!({ "message_id": id })).await.unwrap();
    sqlx::query_scalar("SELECT summary FROM message_insights WHERE message_id = ?1").bind(&id).fetch_one(&state.db).await.unwrap()
}

#[tokio::test]
async fn the_analyze_job_stores_the_polished_summary() {
    for f in FIXTURES {
        assert_eq!(stored_summary(f.robotic, f).await, f.expected, "{}", f.name);
    }
}

#[tokio::test]
async fn the_analyze_job_never_stores_an_amount_that_is_not_in_the_email() {
    let stored = stored_summary("INR 70,000 was credited to your account XX9876 on 01-10-2025.", &FIXTURES[3]).await;
    assert_eq!(stored, "An amount was credited to your account ending 9876 on 1 Oct. Check the amount in the email.");
}
