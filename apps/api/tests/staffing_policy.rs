//! Gap 4: billable policy. Billable people's mail arrives only through the HR / timesheet mailbox; non-billable
//! people keep their full mailbox; a billable person with no timesheet three days after month end raises
//! `timesheet_missing`, which resolves when the timesheet arrives.

use crate::staffing_common;

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

async fn suppressed(h: &H, ws: &str, id: &str) -> bool {
    let _ = ws;
    sqlx::query_scalar::<_, bool>("SELECT suppressed FROM email_messages WHERE id = ?1").bind(id).fetch_one(&h.state.db).await.unwrap()
}

#[tokio::test]
async fn the_billable_gate_only_hides_mail_a_billable_person_sent_and_never_deletes_it() {
    let h = harness().await;
    let ws = h.workspace(&["staffing"]).await;
    let hr = h.mailbox(&ws, "hr@acme.test").await;
    let priya_box = h.mailbox(&ws, "priya@acme.test").await;
    h.call("PUT", &format!("{}/team/settings", api(&ws)), OWNER, Some(settings(json!({"hr_source_id": hr})))).await;
    h.person(&ws, json!({"name": "Priya Sharma", "emails": ["priya@acme.test"], "employee_type": "non_billable", "source_id": priya_box})).await;
    let ravi = h.person(&ws, json!({"name": "Ravi Kumar", "emails": ["ravi@client.test"], "employee_type": "billable", "client": "TCS"})).await;
    let list = || async { h.get(OWNER, &format!("{}/messages?direction=all", api(&ws))).await.1["items"].as_array().unwrap().iter().map(|m| m["subject"].as_str().unwrap().to_string()).collect::<Vec<_>>() };

    // Kept: a client copying the consultant, a team list the consultant is on, and the recruiter's sent CV mail copying him.
    let mut cc = email("client@tcs.com", &["priya@acme.test"], "Cc kept", "x", OCT5);
    cc.cc = vec!["ravi@client.test".into()];
    assert!(!suppressed(&h, &ws, &h.ingest(&ws, &priya_box, cc, "k1").await.unwrap()).await);
    let list_mail = email("team-list@acme.test", &["priya@acme.test", "ravi@client.test"], "List kept", "x", OCT5 + 1);
    assert!(!suppressed(&h, &ws, &h.ingest(&ws, &priya_box, list_mail, "k2").await.unwrap()).await);
    let mut sent_cv = sent(with_files(email("priya@acme.test", &["hr@capgemini.com"], "Profile", "x", OCT5 + 2), &["Z_CV.pdf"]));
    sent_cv.cc = vec!["ravi@client.test".into()];
    assert!(!suppressed(&h, &ws, &h.ingest(&ws, &priya_box, sent_cv, "k3").await.unwrap()).await);
    let to_ravi = sent(email("priya@acme.test", &["ravi@client.test"], "Outbound to Ravi kept", "x", OCT5 + 3));
    assert!(!suppressed(&h, &ws, &h.ingest(&ws, &priya_box, to_ravi, "k4").await.unwrap()).await);
    assert_eq!(list().await.len(), 4);

    // Hidden but stored: Ravi himself wrote it, or it was forwarded on his behalf.
    let own = h.ingest(&ws, &priya_box, email("ravi@client.test", &["priya@acme.test"], "Hidden direct", "private", OCT5 + 4), "g1").await.unwrap();
    let mut fwd = email("forwarder@client.test", &["priya@acme.test"], "Hidden forwarded", "private", OCT5 + 5);
    fwd.original_sender_address = Some("Ravi <ravi@client.test>".into());
    let forwarded = h.ingest(&ws, &priya_box, fwd, "g2").await.unwrap();
    assert!(suppressed(&h, &ws, &own).await && suppressed(&h, &ws, &forwarded).await);
    assert_eq!(list().await.len(), 4, "not shown in the inbox");
    assert_eq!(h.get(OWNER, &format!("{}/messages/{own}", api(&ws))).await.0, StatusCode::NOT_FOUND);
    let audits: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE action = 'message.suppressed'").fetch_one(&h.state.db).await.unwrap();
    assert_eq!(audits, 2);
    let jobs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE payload_json LIKE ?1").bind(format!("%{own}%")).fetch_one(&h.state.db).await.unwrap();
    assert_eq!(jobs, 0, "suppressed mail is never processed");
    // Through the HR mailbox Ravi's mail is read normally.
    assert!(!suppressed(&h, &ws, &h.ingest(&ws, &hr, email("ravi@client.test", &["hr@acme.test"], "September timesheet", "Total hours: 168", OCT5 + 6), "b1").await.unwrap()).await);

    // Moving the HR mailbox does not release mail that still qualifies; Ravi no longer being billable does.
    let other = h.mailbox(&ws, "hr2@acme.test").await;
    assert_eq!(h.call("PUT", &format!("{}/team/settings", api(&ws)), OWNER, Some(settings(json!({"hr_source_id": other})))).await.0, StatusCode::OK);
    assert!(suppressed(&h, &ws, &own).await);
    let (st, body) = h.call("PATCH", &format!("{}/team/people/{}", api(&ws), ravi["id"].as_str().unwrap()), OWNER, Some(json!({"employee_type": "non_billable"}))).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(!suppressed(&h, &ws, &own).await);
    assert!(list().await.contains(&"Hidden direct".to_string()));
}

#[tokio::test]
async fn missing_timesheets_alert_the_person_then_up_the_chain_and_resolve_on_arrival() {
    let h = harness().await;
    let ws = h.workspace(&["staffing"]).await;
    let hr = h.mailbox(&ws, "hr@acme.test").await;
    h.call("PUT", &format!("{}/team/settings", api(&ws)), OWNER, Some(settings(json!({"hr_source_id": hr, "manager_after_hours": 24, "final_after_hours": 24, "final_emails": ["hr-head@acme.test"]})))).await;
    let neha = h.person(&ws, json!({"name": "Neha Rao", "emails": ["neha@acme.test"], "role": "account manager"})).await;
    let mk = |name: &str, email: &str, joined: &str| json!({"name": name, "emails": [email], "employee_type": "billable", "manager_id": neha["id"], "joining_date": joined});
    h.person(&ws, mk("Ravi Kumar", "ravi@client.test", "2026-01-12")).await;
    h.person(&ws, mk("Dev Patel", "dev@client.test", "2026-01-12")).await;
    h.person(&ws, mk("New Joiner", "new@client.test", "2026-10-01")).await;
    h.person(&ws, json!({"name": "Priya Sharma", "emails": ["priya@acme.test"], "employee_type": "non_billable"})).await;
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


#[tokio::test]
async fn a_timesheet_counts_only_for_the_month_it_is_for() {
    let h = harness().await;
    let ws = h.workspace(&["staffing"]).await;
    let hr = h.mailbox(&ws, "hr@acme.test").await;
    h.call("PUT", &format!("{}/team/settings", api(&ws)), OWNER, Some(settings(json!({"hr_source_id": hr})))).await;
    let ravi = h.person(&ws, json!({"name": "Ravi", "emails": ["ravi@client.test"], "employee_type": "billable", "joining_date": "2026-01-12"})).await;
    let pid = ravi["id"].as_str().unwrap();
    let sep2 = OCT5 - 33 * DAY; // 2 Sep 12:00 UTC
    let aug = h.ingest(&ws, &hr, email("ravi@client.test", &["hr@acme.test"], "Timesheet", "Total hours: 150", sep2), "a1").await.unwrap();
    h.classify(&aug, "timesheet").await;
    let get = |m: &'static str| {
        let (db, ws, pid) = (h.state.db.clone(), ws.clone(), pid.to_string());
        async move { email_ai_api::hr::staffing::policy::timesheet_mail(&db, &ws, &pid, m).await.unwrap() }
    };
    // Sent 2 Sep, inside August's three-day grace: it is August's.
    assert_eq!(get("2026-08").await.unwrap().hours, Some(150.0));
    assert!(get("2026-09").await.is_none(), "an August timesheet never stands in for September's");
    // A named month wins over the send date: "September timesheet" sent on 1 Oct is September's, "Timesheet Sept (late)" too.
    let sept = h.ingest(&ws, &hr, email("ravi@client.test", &["hr@acme.test"], "September timesheet", "Total hours: 160", OCT5 - 4 * DAY), "a2").await.unwrap();
    h.classify(&sept, "timesheet").await;
    assert_eq!(get("2026-09").await.unwrap().hours, Some(160.0));
    assert_eq!(get("2026-08").await.unwrap().hours, Some(150.0));

    // Portal hours for August are never compared with September's mail.
    h.call("POST", &format!("{}/staffing/portal/hours", api(&ws)), OWNER, Some(json!({"csv": "email,month,hours\nravi@client.test,2026-08,150\n"}))).await;
    let r = email_ai_api::hr::staffing::portal::reconcile(&h.state.db, &ws, OCT5).await.unwrap();
    assert!(r.mismatches.is_empty(), "{r:?}");
    assert_eq!(r.matched, 1);
    h.call("POST", &format!("{}/staffing/portal/hours", api(&ws)), OWNER, Some(json!({"csv": "email,month,hours\nravi@client.test,2026-08,152\n"}))).await;
    let r = email_ai_api::hr::staffing::portal::reconcile(&h.state.db, &ws, OCT5).await.unwrap();
    assert_eq!(r.mismatches.len(), 1, "152 portal vs 150 mailed for August");
    assert_eq!(r.mismatches[0].detail["email_hours"], 150.0);
}

#[test]
fn periods_from_subjects_and_dates() {
    use email_ai_api::hr::staffing::policy::period_of;
    assert_eq!(period_of("Timesheet", OCT5 - 33 * DAY, 0, 3), "2026-08");
    assert_eq!(period_of("Timesheet", OCT5 - 20 * DAY, 0, 3), "2026-09");
    assert_eq!(period_of("Sept timesheet", OCT5, 0, 3), "2026-09");
    assert_eq!(period_of("December hours", OCT5, 0, 3), "2025-12");
    assert_eq!(period_of("hours 2026-07", OCT5, 0, 3), "2026-07");
    assert_eq!(period_of("March 2025 timesheet", OCT5, 0, 3), "2025-03");
}
