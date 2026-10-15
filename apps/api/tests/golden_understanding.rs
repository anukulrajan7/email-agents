//! Offline understanding golden set : new-content-only bodies, the bulk filter and the
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
use harness::{label_accuracy, label_accuracy_md, label_set, load_scenarios, write_scorecard};
use serde_json::Value;

use crate::golden_harness as harness;

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

/// The labelled set is the golden scenarios that name a label (the same emails the agents are graded on).
/// Without a model this scores the keyword baseline (`RulesUnderstander`); `golden_live` scores the real reader.
#[tokio::test]
async fn labelled_emails_have_valid_labels_and_the_keyword_baseline_is_recorded() {
    use email_ai_api::ai::understand::Understander;
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut problems = Vec::new();
    for s in load_scenarios().iter().filter(|s| s.label.is_some()) {
        let want = s.label.clone().unwrap_or_default();
        let labels = label_set(s);
        if !labels.iter().any(|l| l.key == want) {
            problems.push(format!("{}: label {want} is not in {}'s taxonomy", s.id, s.agent()));
            continue;
        }
        let mail = s.emails.last().unwrap();
        let input = UnderstandInput {
            subject: mail.subject.clone(),
            sender: mail.from.clone(),
            received_at_ms: 1_791_900_000_000,
            new_part: model_body(&mail.subject, &mail.body),
            attachments_block: String::new(),
            attachment_names: vec![],
            labels,
            examples: vec![],
            sender_history: None,
            thread_digest: None,
        };
        let (u, _) = email_ai_api::ai::understand::RulesUnderstander.understand(&input).await.unwrap();
        pairs.push((want, u.label));
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    assert!(pairs.len() >= 50, "{} labelled emails", pairs.len());
    let by = label_accuracy(&pairs);
    let table = label_accuracy_md(&by);
    println!("\nLabel accuracy, keyword baseline (rules-v1)\n{table}");
    write_scorecard(
        "labelling, keyword baseline",
        &format!("The offline keyword classifier on the golden emails that name a label. The real reader is scored live (`golden_live`).\n\n{table}"),
    );
}
