//! Gap 5: portal reconciliation. CSV imports with a validation report, exact joins against timesheet mail and
//! detected submissions, mismatch alerts that clear when the data agrees.

mod staffing_common;

use axum::http::StatusCode;
use email_ai_api::staffing::portal;
use serde_json::{Value, json};
use staffing_common::*;

const OCT5: i64 = 1_791_201_600_000;
const SEP14: i64 = OCT5 - 21 * DAY;

fn find<'a>(v: &'a Value, person: &str, kind: &str) -> Option<&'a Value> {
    v.as_array()?.iter().find(|m| m["person_name"] == person && m["kind"] == kind)
}

#[tokio::test]
async fn portal_data_is_joined_exactly_and_mismatches_alert_then_clear() {
    let h = harness().await;
    let ws = h.workspace(&["staffing"]).await;
    let hr = h.mailbox(&ws, "hr@acme.test").await;
    let priya_box = h.mailbox(&ws, "priya@acme.test").await;
    h.call("PUT", &format!("{}/team/settings", api(&ws)), OWNER, Some(json!({"tz_offset_minutes": 0, "manager_after_hours": null, "final_after_hours": null, "final_emails": [],
        "below_target_hour": 16, "weekly_check_hour": 12, "hr_source_id": hr, "client_domains": [], "timesheet_grace_days": 3, "hours_tolerance": 0.5}))).await;
    let mut ids = std::collections::HashMap::new();
    for (name, email, kind) in [("Ravi", "ravi@client.test", "billable"), ("Dev", "dev@client.test", "billable"), ("Mani", "mani@client.test", "billable"), ("Zoya", "zoya@client.test", "billable"), ("Priya", "priya@acme.test", "non_billable")] {
        let mut body = json!({"name": name, "emails": [email], "employee_type": kind, "joining_date": "2026-01-12"});
        if name == "Priya" {
            body["source_id"] = json!(priya_box);
        }
        ids.insert(name, h.person(&ws, body).await["id"].as_str().unwrap().to_string());
    }

    // Timesheet mail: Ravi says 152 hours, Dev 168, Zoya 150. Mani sent none.
    for (n, (from, text)) in [("ravi@client.test", "Total hours: 152"), ("dev@client.test", "Total hours: 168"), ("zoya@client.test", "Total hours: 150")].into_iter().enumerate() {
        let m = h.ingest(&ws, &hr, email(from, &["hr@acme.test"], "September timesheet", text, OCT5 - 4 * DAY + n as i64), &format!("ts{n}")).await.unwrap();
        h.classify(&m, "timesheet").await;
    }
    // Sent submissions in September: Rahul on the 14th and Kiran on the 16th.
    submission(&h, &ws, &ids["Priya"], &priya_box, 1, SEP14, "pending", false, "active").await;
    submission(&h, &ws, &ids["Priya"], &priya_box, 2, SEP14 + 2 * DAY, "pending", false, "active").await;
    sqlx::query("UPDATE submissions SET candidate_key = 'rahul verma', candidate_name = 'Rahul Verma' WHERE id = ?1").bind(format!("sub_{}_1", ids["Priya"])).execute(&h.state.db).await.unwrap();
    sqlx::query("UPDATE submissions SET candidate_key = 'kiran rao', candidate_name = 'Kiran Rao' WHERE id = ?1").bind(format!("sub_{}_2", ids["Priya"])).execute(&h.state.db).await.unwrap();

    // Import: the report names every bad row; good rows are kept.
    let hours = "email,month,hours\nravi@client.test,2026-09,160\ndev@client.test,2026-09,168\nmani@client.test,2026-09,120\n\
                 ghost@client.test,2026-09,100\ndev@client.test,Sept,100\ndev@client.test,2026-08,-5\n";
    let url = format!("{}/staffing/portal", api(&ws));
    let (s, r) = h.call("POST", &format!("{url}/hours"), OWNER, Some(json!({"csv": hours, "dry_run": true}))).await;
    assert_eq!((s, r["created"].as_i64(), r["skipped"].as_i64()), (StatusCode::OK, Some(3), Some(3)), "{r}");
    let (_, r) = h.call("POST", &format!("{url}/hours"), OWNER, Some(json!({"csv": hours}))).await;
    let errs: Vec<(i64, &str)> = r["errors"].as_array().unwrap().iter().map(|e| (e["row"].as_i64().unwrap(), e["field"].as_str().unwrap())).collect();
    assert_eq!(errs, vec![(5, "email"), (6, "month"), (7, "hours")]);
    let subs = "email,date,candidate,client\npriya@acme.test,2026-09-14,Verma Rahul,Capgemini\npriya@acme.test,2026-09-15,Anita Desai,Capgemini\npriya@acme.test,bad,Someone,x\n";
    let (_, r) = h.call("POST", &format!("{url}/submissions"), OWNER, Some(json!({"csv": subs}))).await;
    assert_eq!((r["created"].as_i64(), r["skipped"].as_i64()), (Some(2), Some(1)));

    // Reconcile as of 5 Oct (after the grace period).
    let raised = portal::reconcile_and_alert(&h.state, &ws, OCT5).await.unwrap();
    assert_eq!(raised, 5);
    let (_, m) = h.get(OWNER, &format!("{url}/mismatches")).await;
    let items = &m["items"];
    assert_eq!(items.as_array().unwrap().len(), 5, "{m}");
    let ravi = find(items, "Ravi", "hours_differ").unwrap();
    assert_eq!((ravi["detail"]["portal_hours"].as_f64(), ravi["detail"]["email_hours"].as_f64()), (Some(160.0), Some(152.0)));
    assert!(find(items, "Mani", "missing_in_email").is_some() && find(items, "Zoya", "missing_in_portal").is_some());
    assert!(find(items, "Dev", "hours_differ").is_none(), "equal hours match");
    assert_eq!(find(items, "Priya", "submissions_missing_in_portal").unwrap()["detail"]["candidates"], json!(["Kiran Rao"]));
    assert_eq!(find(items, "Priya", "submissions_missing_in_email").unwrap()["detail"]["candidates"], json!(["Anita Desai"]));
    let (_, alerts) = h.get(OWNER, &format!("{}/alerts", api(&ws))).await;
    assert_eq!(alerts["items"].as_array().unwrap().iter().filter(|a| a["kind"] == "portal_mismatch").count(), 5);

    // Idempotent.
    assert_eq!(portal::reconcile_and_alert(&h.state, &ws, OCT5 + HOUR).await.unwrap(), 0);

    // The portal is corrected: Ravi's hours now agree, so that mismatch and its alert close.
    h.call("POST", &format!("{url}/hours"), OWNER, Some(json!({"csv": "email,month,hours\nravi@client.test,2026-09,152.2\n"}))).await;
    portal::reconcile_and_alert(&h.state, &ws, OCT5 + 2 * HOUR).await.unwrap();
    let (_, m) = h.get(OWNER, &format!("{url}/mismatches")).await;
    assert_eq!(find(&m["items"], "Ravi", "hours_differ").unwrap()["status"], "resolved", "within the 0.5 h tolerance");
    let (_, alerts) = h.get(OWNER, &format!("{}/alerts", api(&ws))).await;
    assert_eq!(alerts["items"].as_array().unwrap().iter().filter(|a| a["kind"] == "portal_mismatch").count(), 4);
}

#[tokio::test]
async fn only_the_super_admin_imports_and_everyone_sees_their_own_mismatches() {
    let h = harness().await;
    let ws = h.workspace(&["staffing"]).await;
    let hr = h.mailbox(&ws, "hr@acme.test").await;
    h.call("PUT", &format!("{}/team/settings", api(&ws)), OWNER, Some(json!({"tz_offset_minutes": 0, "manager_after_hours": null, "final_after_hours": null, "final_emails": [],
        "below_target_hour": 16, "weekly_check_hour": 12, "hr_source_id": hr}))).await;
    let admin = h.member(&ws, "admin2@acme.test", "admin").await;
    let ravi_t = h.member(&ws, "ravi@client.test", "operator").await;
    let dev_t = h.member(&ws, "dev@client.test", "operator").await;
    h.person(&ws, json!({"name": "Ravi", "emails": ["ravi@client.test"], "employee_type": "billable", "joining_date": "2026-01-12"})).await;
    h.person(&ws, json!({"name": "Dev", "emails": ["dev@client.test"], "employee_type": "billable", "joining_date": "2026-01-12"})).await;
    let url = format!("{}/staffing/portal", api(&ws));
    for t in [&admin, &ravi_t] {
        assert_eq!(h.call("POST", &format!("{url}/hours"), t, Some(json!({"csv": "email,month,hours\nravi@client.test,2026-09,1\n"}))).await.0, StatusCode::FORBIDDEN);
        assert_eq!(h.call("POST", &format!("{url}/reconcile"), t, None).await.0, StatusCode::FORBIDDEN);
    }
    h.call("POST", &format!("{url}/hours"), OWNER, Some(json!({"csv": "email,month,hours\nravi@client.test,2026-09,100\ndev@client.test,2026-09,100\n"}))).await;
    portal::reconcile_and_alert(&h.state, &ws, OCT5).await.unwrap();
    let names = |v: &Value| v["items"].as_array().unwrap().iter().map(|m| m["person_name"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(names(&h.get(&ravi_t, &format!("{url}/mismatches")).await.1), vec!["Ravi"]);
    assert_eq!(names(&h.get(&dev_t, &format!("{url}/mismatches")).await.1), vec!["Dev"]);
    assert_eq!(names(&h.get(OWNER, &format!("{url}/mismatches")).await.1).len(), 2);
    assert!(names(&h.get(&admin, &format!("{url}/mismatches")).await.1).is_empty());
}

#[tokio::test]
async fn a_cleared_mismatch_reopens_at_level_one_when_it_comes_back_but_a_manual_resolve_sticks() {
    let h = harness().await;
    let ws = h.workspace(&["staffing"]).await;
    let hr = h.mailbox(&ws, "hr@acme.test").await;
    h.call("PUT", &format!("{}/team/settings", api(&ws)), OWNER, Some(json!({"tz_offset_minutes": 0, "manager_after_hours": null, "final_after_hours": null, "final_emails": [],
        "below_target_hour": 16, "weekly_check_hour": 12, "hr_source_id": hr}))).await;
    h.person(&ws, json!({"name": "Ravi", "emails": ["ravi@client.test"], "employee_type": "billable", "joining_date": "2026-01-12"})).await;
    let m = h.ingest(&ws, &hr, email("ravi@client.test", &["hr@acme.test"], "September timesheet", "Total hours: 152", OCT5 - 4 * DAY), "t1").await.unwrap();
    h.classify(&m, "timesheet").await;
    let url = format!("{}/staffing/portal/hours", api(&ws));
    let state = || async { sqlx::query_as::<_, (String, i64)>("SELECT status, escalation_level FROM alerts WHERE kind = 'portal_mismatch'").fetch_one(&h.state.db).await.unwrap() };

    h.call("POST", &url, OWNER, Some(json!({"csv": "email,month,hours\nravi@client.test,2026-09,160\n"}))).await;
    portal::reconcile_and_alert(&h.state, &ws, OCT5).await.unwrap();
    sqlx::query("UPDATE alerts SET escalation_level = 3").execute(&h.state.db).await.unwrap();
    h.call("POST", &url, OWNER, Some(json!({"csv": "email,month,hours\nravi@client.test,2026-09,152\n"}))).await;
    portal::reconcile_and_alert(&h.state, &ws, OCT5 + HOUR).await.unwrap();
    assert_eq!(state().await.0, "resolved");

    h.call("POST", &url, OWNER, Some(json!({"csv": "email,month,hours\nravi@client.test,2026-09,140\n"}))).await;
    let raised = portal::reconcile_and_alert(&h.state, &ws, OCT5 + 2 * HOUR).await.unwrap();
    assert_eq!(raised, 1);
    assert_eq!(state().await, ("open".to_string(), 1), "reopened at level 1, same alert");

    // Resolved by a person: the same mismatch does not reopen it.
    sqlx::query("UPDATE alerts SET status = 'resolved', resolved_by = 'usr_x'").execute(&h.state.db).await.unwrap();
    assert_eq!(portal::reconcile_and_alert(&h.state, &ws, OCT5 + 3 * HOUR).await.unwrap(), 0);
    assert_eq!(state().await.0, "resolved");
}
