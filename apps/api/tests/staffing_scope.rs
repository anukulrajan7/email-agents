//! Gap 7: per-person mailbox scoping. The inbox can be narrowed to one mailbox or to the mailbox linked to one
//! person, and the filter never leaks the other mailbox's mail.

use crate::staffing_common;

use axum::http::StatusCode;
use serde_json::{Value, json};
use staffing_common::*;

fn subjects(v: &Value) -> Vec<String> {
    let mut s: Vec<String> = v["items"].as_array().unwrap().iter().map(|m| m["subject"].as_str().unwrap().to_string()).collect();
    s.sort();
    s
}

#[tokio::test]
async fn two_mailboxes_stay_isolated_under_the_scope_filter() {
    let h = harness().await;
    let ws = h.workspace(&["general"]).await;
    let (a, b) = (h.mailbox(&ws, "asha@acme.test").await, h.mailbox(&ws, "ben@acme.test").await);
    let pa = h.person(&ws, json!({"name": "Asha", "emails": ["asha@acme.test"], "source_id": a})).await;
    let pb = h.person(&ws, json!({"name": "Ben", "emails": ["ben@acme.test"], "source_id": b})).await;
    let nobox = h.person(&ws, json!({"name": "Cy", "emails": ["cy@acme.test"]})).await;
    for n in 0..3 {
        h.ingest(&ws, &a, email("cust@shop.test", &["asha@acme.test"], &format!("A mail {n}"), "x", WED + n), &format!("a{n}")).await.unwrap();
        h.ingest(&ws, &b, email("cust@shop.test", &["ben@acme.test"], &format!("B mail {n}"), "x", WED + 10 + n), &format!("b{n}")).await.unwrap();
    }
    h.ingest(&ws, &a, sent(email("asha@acme.test", &["cust@shop.test"], "A reply", "x", WED + 50)), "a0").await.unwrap();
    let url = |q: &str| format!("{}/messages?direction=all&{q}", api(&ws));
    let hh = &h;
    let list = |q: String| async move { hh.get(OWNER, &q).await };

    let (s, all) = list(url("")).await;
    assert_eq!((s, subjects(&all).len()), (StatusCode::OK, 7));
    let only_a = subjects(&list(url(&format!("person_id={}", pa["id"].as_str().unwrap()))).await.1);
    assert_eq!(only_a, vec!["A mail 0", "A mail 1", "A mail 2", "A reply"]);
    let only_b = subjects(&list(url(&format!("source_id={b}"))).await.1);
    assert_eq!(only_b, vec!["B mail 0", "B mail 1", "B mail 2"]);
    assert!(only_a.iter().chain(&only_b).all(|s| s.starts_with('A') == only_a.contains(s)), "no overlap");
    assert_eq!(subjects(&list(url(&format!("person_id={}", pb["id"].as_str().unwrap()))).await.1), only_b);

    // Both filters together intersect: Asha's person with Ben's mailbox is nothing.
    assert!(subjects(&list(url(&format!("person_id={}&source_id={b}", pa["id"].as_str().unwrap()))).await.1).is_empty());
    // A person with no mailbox has no mail, not everyone's mail.
    assert!(subjects(&list(url(&format!("person_id={}", nobox["id"].as_str().unwrap()))).await.1).is_empty());
    // The scope holds with paging and the importance sort.
    let (_, page) = list(url(&format!("source_id={a}&limit=2"))).await;
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    let (_, next) = list(url(&format!("source_id={a}&limit=2&cursor={}", page["next_cursor"].as_str().unwrap()))).await;
    assert!(subjects(&next).iter().chain(subjects(&page).iter()).all(|s| s.starts_with('A')));
    assert!(subjects(&list(url(&format!("source_id={a}&sort=importance"))).await.1).iter().all(|s| s.starts_with('A')));
    // Unknown ids are refused rather than ignored.
    assert_eq!(list(url("source_id=src_nope")).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(list(url("person_id=per_nope")).await.0, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_member_can_only_scope_to_people_they_may_see_and_workspaces_never_mix() {
    let h = harness().await;
    let ws = h.workspace(&["general"]).await;
    let other = h.workspace(&["general"]).await;
    let (a, b) = (h.mailbox(&ws, "asha@acme.test").await, h.mailbox(&ws, "ben@acme.test").await);
    let asha_t = h.member(&ws, "asha@acme.test", "operator").await;
    let pa = h.person(&ws, json!({"name": "Asha", "emails": ["asha@acme.test"], "source_id": a})).await;
    let pb = h.person(&ws, json!({"name": "Ben", "emails": ["ben@acme.test"], "source_id": b})).await;
    h.ingest(&ws, &b, email("cust@shop.test", &["ben@acme.test"], "Ben private", "x", WED), "b1").await.unwrap();
    let q = |p: &Value| format!("{}/messages?person_id={}", api(&ws), p["id"].as_str().unwrap());
    assert_eq!(h.get(&asha_t, &q(&pb)).await.0, StatusCode::NOT_FOUND, "Asha cannot scope to Ben");
    assert_eq!(h.get(&asha_t, &q(&pa)).await.0, StatusCode::OK);
    // A source from another workspace is not a valid scope here.
    let foreign = h.mailbox(&other, "x@other.test").await;
    assert_eq!(h.get(OWNER, &format!("{}/messages?source_id={foreign}", api(&ws))).await.0, StatusCode::BAD_REQUEST);
}
