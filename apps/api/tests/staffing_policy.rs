//! Gap 4: billable policy. Billable people's mail arrives only through the HR / timesheet mailbox; non-billable
//! people keep their full mailbox; a billable person with no timesheet three days after month end raises
//! `timesheet_missing`, which resolves when the timesheet arrives.

mod staffing_common;

use axum::http::StatusCode;
use email_ai_api::alerts;
use serde_json::json;
use staffing_common::*;

fn settings(extra: serde_json::Value) -> serde_json::Value {
    let mut s = json!({"tz_offset_minutes": 0, "manager_after_hours": null, "final_after_hours": null, "final_emails": [],
                       "below_target_hour": 16, "weekly_check_hour": 12});
    for (k, v) in extra.as_object().unwrap() {
        s[k] = v.clone();
    }
    s
}

/// Monday 5 Oct 2026 12:00 UTC: September's timesheets were due on 4 Oct.
const OCT5: i64 = 1_791_201_600_000;

#[tokio::test]
async fn billable_mail_only_enters_through_the_hr_mailbox() {
    let h = harness().await;
    let ws = h.workspace(&["staffing"]).await;
    let hr = h.mailbox(&ws, "hr@acme.test").await;
    let priya_box = h.mailbox(&ws, "priya@acme.test").await;
    h.person(&ws, json!({"name": "Priya Sharma", "emails": ["priya@acme.test"], "employee_type": "non_billable", "source_id": priya_box})).await;
    h.person(&ws, json!({"name": "Ravi Kumar", "emails": ["ravi@client.test"], "employee_type": "billable", "client": "TCS"})).await;

    // No HR mailbox named yet: the policy cannot know where billable mail belongs, so nothing is dropped.
    assert!(h.ingest(&ws, &priya_box, email("ravi@client.test", &["priya@acme.test"], "Hello", "hi", OCT5), "a1").await.is_some());

    let (s, _) = h.call("PUT", &format!("{}/team/settings", api(&ws)), OWNER, Some(settings(json!({"hr_source_id": hr})))).await;
    assert_eq!(s, StatusCode::OK);

    // Billable Ravi's mail in a personal mailbox, in either direction, is never stored.
    assert!(h.ingest(&ws, &priya_box, email("ravi@client.test", &["priya@acme.test"], "Client issue", "details", OCT5 + 1000), "a2").await.is_none());
    assert!(h.ingest(&ws, &priya_box, sent(email("priya@acme.test", &["Ravi Kumar <ravi@client.test>"], "Re: Client issue", "ok", OCT5 + 2000)), "a2").await.is_none());
    // Everything else in that mailbox is read as usual (non-billable = full mailbox).
    assert!(h.ingest(&ws, &priya_box, email("customer@shop.test", &["priya@acme.test"], "Quote", "price?", OCT5 + 3000), "a3").await.is_some());
    assert!(h.ingest(&ws, &priya_box, sent(email("priya@acme.test", &["colleague@acme.test"], "Lunch", "?", OCT5 + 4000)), "a4").await.is_some());
    // Through the HR mailbox Ravi's timesheet is read.
    assert!(h.ingest(&ws, &hr, email("ravi@client.test", &["hr@acme.test"], "September timesheet", "Total hours: 168", OCT5 + 5000), "b1").await.is_some());

    let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM email_messages WHERE workspace_id = ?1 AND subject = 'Client issue'").bind(&ws).fetch_one(&h.state.db).await.unwrap();
    assert_eq!(stored, 0);
}

#[tokio::test]
async fn missing_timesheets_alert_the_person_then_up_the_chain_and_resolve_on_arrival() {
    let h = harness().await;
    let ws = h.workspace(&["staffing"]).await;
    let hr = h.mailbox(&ws, "hr@acme.test").await;
    let neha = h.person(&ws, json!({"name": "Neha Rao", "emails": ["neha@acme.test"], "role": "account manager"})).await;
    let mk = |name: &str, email: &str, joined: &str| json!({"name": name, "emails": [email], "employee_type": "billable", "manager_id": neha["id"], "joining_date": joined});
    h.person(&ws, mk("Ravi Kumar", "ravi@client.test", "2026-01-12")).await;
    h.person(&ws, mk("Dev Patel", "dev@client.test", "2026-01-12")).await;
    h.person(&ws, mk("New Joiner", "new@client.test", "2026-10-01")).await;
    h.person(&ws, json!({"name": "Priya Sharma", "emails": ["priya@acme.test"], "employee_type": "non_billable"})).await;
    h.call("PUT", &format!("{}/team/settings", api(&ws)), OWNER, Some(settings(json!({"hr_source_id": hr, "manager_after_hours": 24, "final_after_hours": 24, "final_emails": ["hr-head@acme.test"]})))).await;
    h.call("PUT", &format!("{}/alert-settings", api(&ws)), OWNER, Some(json!({"enabled": true, "recipients": ["owner@acme.test"], "escalate_to": [], "unanswered_hours": 4, "escalate_after_hours": 24, "daily_digest": false, "digest_hour": 9}))).await;

    // Dev sent September's timesheet on 30 Sep.
    let m = h.ingest(&ws, &hr, email("dev@client.test", &["hr@acme.test"], "September timesheet", "Please find my timesheet. Total hours: 168", OCT5 - 5 * DAY), "ts1").await.unwrap();
    h.classify(&m, "timesheet").await;

    // 2 Oct: inside the 3-day grace period.
    let two = OCT5 - 3 * DAY;
    
    assert_eq!(alerts::evaluate_workspace(&h.state, &ws, two).await.unwrap().raised, 0);

    // 5 Oct: Ravi is overdue. Dev sent his; New Joiner joined in October; Priya is non-billable.
    let r = alerts::evaluate_workspace(&h.state, &ws, OCT5).await.unwrap();
    assert_eq!(r.raised, 1, "{r:?}");
    let (_, list) = h.get(OWNER, &format!("{}/alerts", api(&ws))).await;
    let a = &list["items"][0];
    assert_eq!((a["kind"].as_str(), a["severity"].as_str()), (Some("timesheet_missing"), Some("high")));
    assert!(a["title"].as_str().unwrap().contains("Ravi Kumar"), "{a}");
    let sent_mail = h.mail.take();
    assert_eq!(sent_mail.iter().map(|m| m.0.as_str()).collect::<Vec<_>>(), vec!["ravi@client.test"], "the person is told first");

    // The Timesheets view says the same.
    let (_, ts) = h.get(OWNER, &format!("{}/staffing/timesheets?month=2026-09", api(&ws))).await;
    let status = |name: &str| ts["items"].as_array().unwrap().iter().find(|r| r["name"] == name).map(|r| r["status"].as_str().unwrap().to_string());
    assert_eq!((status("Ravi Kumar").as_deref(), status("Dev Patel").as_deref(), status("New Joiner"), status("Priya Sharma")), (Some("missing"), Some("received"), None, None));
    assert_eq!(ts["missing"], 1);
    assert_eq!(ts["items"].as_array().unwrap().iter().find(|r| r["name"] == "Dev Patel").unwrap()["timesheet"]["hours"], 168.0);

    // Not resolved: it moves to the manager after a day, then to the final contacts.
    alerts::evaluate_workspace(&h.state, &ws, OCT5 + 25 * HOUR).await.unwrap();
    assert_eq!(h.mail.take().iter().map(|m| m.0.clone()).collect::<Vec<_>>(), vec!["neha@acme.test"]);
    alerts::evaluate_workspace(&h.state, &ws, OCT5 + 50 * HOUR).await.unwrap();
    assert_eq!(h.mail.take().iter().map(|m| m.0.clone()).collect::<Vec<_>>(), vec!["hr-head@acme.test"]);

    // Ravi's timesheet finally arrives: the alert closes by itself.
    let m = h.ingest(&ws, &hr, email("ravi@client.test", &["hr@acme.test"], "Timesheet Sept (late)", "Total hours: 160", OCT5 + 60 * HOUR), "ts2").await.unwrap();
    h.classify(&m, "timesheet").await;
    alerts::evaluate_workspace(&h.state, &ws, OCT5 + 61 * HOUR).await.unwrap();
    let (_, open) = h.get(OWNER, &format!("{}/alerts", api(&ws))).await;
    assert_eq!(open["open_count"], 0);
    let (_, resolved) = h.get(OWNER, &format!("{}/alerts?status=resolved", api(&ws))).await;
    assert_eq!(resolved["items"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn timesheet_rules_do_not_exist_in_other_workspaces() {
    let h = harness().await;
    let ws = h.workspace(&["general"]).await;
    let r = alerts::evaluate_workspace(&h.state, &ws, OCT5).await.unwrap();
    assert_eq!(r.raised, 0);
    let (s, _) = h.get(OWNER, &format!("{}/staffing/timesheets", api(&ws))).await;
    assert_eq!(s, StatusCode::CONFLICT);
}

