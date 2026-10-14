//! Offline understanding golden set (docs/evals.md): new-content-only bodies, the bulk filter and the
//! code-side verification of a model's reading, on real-world-shaped emails. No model is called; the live
//! variant would feed the same emails to the understanding call and grade the same expectations.

use email_ai_api::{
    ai::{
        body::model_body,
        understand::{UnderstandInput, parse_understanding},
    },
    bulk,
    domain::Label,
};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct Files {
    body: Vec<BodyCase>,
    bulk: Vec<BulkCase>,
    verify: Vec<VerifyCase>,
}

#[derive(Deserialize)]
struct BodyCase {
    id: String,
    subject: String,
    text: String,
    must_contain: Vec<String>,
    must_not_contain: Vec<String>,
}

#[derive(Deserialize)]
struct BulkCase {
    id: String,
    sender: String,
    subject: String,
    text: String,
    signals: Vec<String>,
    filed: bool,
}

#[derive(Deserialize)]
struct VerifyCase {
    id: String,
    received_at_ms: i64,
    subject: String,
    text: String,
    reply: Value,
    expect: Expect,
}

#[derive(Deserialize)]
struct Expect {
    asks: usize,
    deadlines: usize,
    dropped: u32,
    priority: String,
    first_deadline_text: Option<String>,
    first_deadline_iso: Option<String>,
}

fn files() -> Files {
    serde_json::from_str(include_str!("golden/understanding.json")).expect("tests/golden/understanding.json is valid")
}

#[test]
fn bodies_keep_what_is_new_and_lose_what_is_old() {
    let mut failures = Vec::new();
    for c in files().body {
        let out = model_body(&c.subject, &c.text);
        for needle in &c.must_contain {
            if !out.contains(needle.as_str()) {
                failures.push(format!("{}: lost {needle:?}\n{out}", c.id));
            }
        }
        for needle in &c.must_not_contain {
            if out.contains(needle.as_str()) {
                failures.push(format!("{}: still has {needle:?}\n{out}", c.id));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n---\n"));
}

#[test]
fn bulk_filter_files_newsletters_and_never_billing() {
    let mut failures = Vec::new();
    for c in files().bulk {
        let new_part = model_body(&c.subject, &c.text);
        let filed = bulk::qualifies(&c.signals, &c.sender, &c.subject, &new_part);
        if filed != c.filed {
            failures.push(format!("{}: filed = {filed}, expected {}", c.id, c.filed));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn verification_drops_what_the_email_never_said() {
    let mut failures = Vec::new();
    for c in files().verify {
        let input = UnderstandInput {
            subject: c.subject.clone(),
            sender: "priya@acme.com".into(),
            received_at_ms: c.received_at_ms,
            new_part: model_body(&c.subject, &c.text),
            attachments_block: String::new(),
            attachment_names: vec![],
            labels: vec![Label { key: "other".into(), name: "Other".into(), description: "d".into() }],
            examples: vec![],
            sender_history: None,
            thread_digest: None,
        };
        let u = parse_understanding(&c.reply.to_string(), &input).unwrap();
        let e = &c.expect;
        let got = (u.asks.len(), u.deadlines.len(), u.dropped_unverified, u.priority.as_str());
        if got != (e.asks, e.deadlines, e.dropped, e.priority.as_str()) {
            failures.push(format!("{}: got {got:?}, expected {:?}", c.id, (e.asks, e.deadlines, e.dropped, &e.priority)));
        }
        if let Some(text) = &e.first_deadline_text
            && u.deadlines.first().map(|d| &d.when_text) != Some(text)
        {
            failures.push(format!("{}: first deadline text {:?}", c.id, u.deadlines.first()));
        }
        if let Some(iso) = &e.first_deadline_iso
            && u.deadlines.first().and_then(|d| d.when_iso.as_ref()) != Some(iso)
        {
            failures.push(format!("{}: first deadline iso {:?}", c.id, u.deadlines.first()));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
