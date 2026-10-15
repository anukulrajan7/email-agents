//! Pack scope: the directory, escalation, scoping and metrics framework are available in every workspace;
//! billable policy, timesheets, submissions, portal reconciliation and matching belong to the staffing pack.

use crate::staffing_common;

use axum::http::StatusCode;
use email_ai_api::alerts;
use serde_json::json;
use staffing_common::*;

#[tokio::test]
async fn a_general_workspace_has_the_directory_escalation_and_widget_but_no_hr_features() {
    let h = harness().await;
    let ws = h.workspace(&["general"]).await;
    let src = h.mailbox(&ws, "asha@acme.test").await;
    let asha = h.person(&ws, json!({"name": "Asha", "emails": ["asha@acme.test"], "source_id": src, "daily_target": 10})).await;
    let asha_t = h.member(&ws, "asha@acme.test", "operator").await;
    h.ingest(&ws, &src, email("cust@shop.test", &["asha@acme.test"], "Q", "help", WED - 2 * HOUR), "g1").await.unwrap();
    h.ingest(&ws, &src, sent(email("asha@acme.test", &["cust@shop.test"], "Re: Q", "done", WED - HOUR)), "g1").await.unwrap();

    // Directory, scoped views, widget numbers (generic metrics), escalation chain and mailbox scoping all work.
    let (_, me) = h.get(&asha_t, &format!("{}/team/metrics/me", api(&ws))).await;
    let keys: Vec<&str> = me["metrics"]["metrics"].as_array().unwrap().iter().map(|m| m["key"].as_str().unwrap()).collect();
    assert_eq!((me["person"]["name"].as_str(), keys), (Some("Asha"), vec!["replies_sent", "avg_response_minutes"]));
    assert_eq!(h.get(&asha_t, &format!("{}/team/escalations", api(&ws))).await.1["chains"].as_array().unwrap().len(), 1);
    assert_eq!(h.get(OWNER, &format!("{}/messages?person_id={}&direction=all", api(&ws), asha["id"].as_str().unwrap())).await.1["items"].as_array().unwrap().len(), 2);
    let (_, access) = h.get(OWNER, &format!("{}/team/access", api(&ws))).await;
    assert_eq!(access["staffing"], false);

    // Every HR & recruiting endpoint says no.
    for path in ["staffing/submissions", "staffing/timesheets", "staffing/portal/mismatches"] {
        assert_eq!(h.get(OWNER, &format!("{}/{path}", api(&ws))).await.0, StatusCode::CONFLICT, "{path}");
    }
    for path in ["staffing/setup", "staffing/scan", "staffing/portal/reconcile"] {
        assert_eq!(h.call("POST", &format!("{}/{path}", api(&ws)), OWNER, None).await.0, StatusCode::CONFLICT, "{path}");
    }
    assert_eq!(h.call("POST", &format!("{}/staffing/portal/hours", api(&ws)), OWNER, Some(json!({"csv": "email,month,hours\n"}))).await.0, StatusCode::CONFLICT);

    // The HR settings are ignored here, so the billable mail policy cannot switch on.
    let hr = h.mailbox(&ws, "hr@acme.test").await;
    let body = json!({"tz_offset_minutes": 0, "manager_after_hours": null, "final_after_hours": null, "final_emails": [], "below_target_hour": 16,
                      "weekly_check_hour": 12, "hr_source_id": hr, "client_domains": ["x.com"]});
    let (s, saved) = h.call("PUT", &format!("{}/team/settings", api(&ws)), OWNER, Some(body)).await;
    assert_eq!(s, StatusCode::OK);
    assert!(saved.get("hr_source_id").is_none(), "{saved}");
    let stored: Option<String> = sqlx::query_scalar("SELECT hr_source_id FROM team_settings WHERE workspace_id = ?1").bind(&ws).fetch_one(&h.state.db).await.unwrap();
    assert!(stored.is_none());
    assert!(h.ingest(&ws, &src, email("ravi@client.test", &["asha@acme.test"], "Hi", "x", WED), "g2").await.is_some());

    // No timesheet or portal rule runs here, but generic below-target does.
    let r = alerts::evaluate_workspace(&h.state, &ws, WED + 8 * HOUR + 24 * DAY).await.unwrap();
    assert_eq!(r.raised, 0, "{r:?}");
    let (_, list) = h.get(OWNER, &format!("{}/alerts", api(&ws))).await;
    assert!(list["items"].as_array().unwrap().iter().all(|a| a["kind"] != "timesheet_missing" && a["kind"] != "portal_mismatch"));
}

#[tokio::test]
async fn a_staffing_workspace_gets_everything_at_once_with_defaults() {
    let h = harness().await;
    let ws = h.workspace(&["staffing"]).await;
    let (_, access) = h.get(OWNER, &format!("{}/team/access", api(&ws))).await;
    assert_eq!((access["staffing"].clone(), access["primary_metric"].as_str()), (json!(true), Some("submissions")));
    // No setup step: every module answers straight away, empty but valid.
    for path in ["team/people", "team/escalations", "team/metrics/team", "team/settings", "staffing/submissions", "staffing/timesheets", "staffing/portal/mismatches"] {
        assert_eq!(h.get(OWNER, &format!("{}/{path}", api(&ws))).await.0, StatusCode::OK, "{path}");
    }
    let (_, s) = h.get(OWNER, &format!("{}/team/settings", api(&ws))).await;
    assert_eq!((s["timesheet_grace_days"].as_i64(), s["hours_tolerance"].as_f64(), s["below_target_hour"].as_i64()), (Some(3), Some(0.5), Some(16)));
    let (s, p) = h.call("GET", &format!("{}/matching/requirements", api(&ws)), OWNER, None).await;
    assert_eq!(s, StatusCode::OK, "{p}");
    // The HRMS metric definitions replace the generic primary and still include the generic ones.
    let hr = h.mailbox(&ws, "hr@acme.test").await;
    let body = json!({"tz_offset_minutes": 0, "manager_after_hours": null, "final_after_hours": null, "final_emails": [], "below_target_hour": 16, "weekly_check_hour": 12, "hr_source_id": hr});
    assert_eq!(h.call("PUT", &format!("{}/team/settings", api(&ws)), OWNER, Some(body)).await.0, StatusCode::OK);
    let p = h.person(&ws, json!({"name": "P", "emails": ["p@acme.test"], "employee_type": "billable"})).await;
    assert_eq!(p["employee_type"], "billable");
    let (_, m) = h.get(OWNER, &format!("{}/team/metrics/people/{}", api(&ws), p["id"].as_str().unwrap())).await;
    let keys: Vec<&str> = m["metrics"].as_array().unwrap().iter().map(|m| m["key"].as_str().unwrap()).collect();
    assert_eq!(keys, vec!["replies_sent", "avg_response_minutes", "submissions", "interviews", "selected", "joined", "rejected", "interview_rate"]);
}
