//! Gap 1: the people directory. Owner writes, validation, manager chain, CSV import with a validation
//! report (and dry run), HR fields only with the staffing pack.

mod staffing_common;

use axum::http::StatusCode;
use serde_json::json;
use staffing_common::*;

#[tokio::test]
async fn owner_manages_the_directory_with_validation() {
    let h = harness().await;
    let ws = h.workspace(&["staffing"]).await;
    let base = format!("{}/team/people", api(&ws));

    let lead = h
        .person(&ws, json!({"name": "Neha Rao", "emails": ["Neha@Acme.test"], "role": "team lead", "joining_date": "2025-01-06",
                            "employee_type": "non_billable", "daily_target": 0, "weekly_target": 0}))
        .await;
    assert_eq!(lead["emails"], json!(["neha@acme.test"]));
    let priya = h
        .person(&ws, json!({"name": "Priya Sharma", "emails": ["priya@acme.test", "priya.s@acme.test"], "role": "recruiter",
                            "manager_id": lead["id"], "joining_date": "2026-03-02", "daily_target": 5, "weekly_target": 25,
                            "employee_type": "non_billable", "client": "Capgemini"}))
        .await;
    assert_eq!((priya["daily_target"].as_i64(), priya["manager_id"].clone()), (Some(5), lead["id"].clone()));

    // An address belongs to one person; bad shapes are named.
    let (s, e) = h.call("POST", &base, OWNER, Some(json!({"name": "Dup", "emails": ["priya.s@acme.test"]}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(e["error"]["message"].as_str().unwrap().contains("already belongs to Priya Sharma"), "{e}");
    for (body, field) in [
        (json!({"name": "X", "emails": ["nope"]}), "emails"),
        (json!({"name": "X", "emails": ["x@acme.test"], "joining_date": "14/10/2026"}), "joining_date"),
        (json!({"name": "X", "emails": ["x@acme.test"], "daily_target": -1}), "daily_target"),
        (json!({"name": "X", "emails": ["x@acme.test"], "manager_id": "per_missing"}), "manager_id"),
        (json!({"name": "", "emails": ["x@acme.test"]}), "name"),
    ] {
        let (s, e) = h.call("POST", &base, OWNER, Some(body)).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{field}");
        assert!(e["error"]["message"].as_str().unwrap().starts_with(field), "{e}");
    }

    // The manager chain cannot loop.
    let (s, e) = h.call("PATCH", &format!("{base}/{}", lead["id"].as_str().unwrap()), OWNER, Some(json!({"manager_id": priya["id"]}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(e["error"]["message"].as_str().unwrap().contains("loop"), "{e}");

    // Patch: missing keeps, null clears.
    let id = priya["id"].as_str().unwrap();
    let (s, p) = h.call("PATCH", &format!("{base}/{id}"), OWNER, Some(json!({"weekly_target": 30, "client": null}))).await;
    assert_eq!(s, StatusCode::OK, "{p}");
    assert_eq!((p["weekly_target"].as_i64(), p["daily_target"].as_i64(), p["client"].clone()), (Some(30), Some(5), json!(null)));

    // Archive frees the address and hides the person.
    let (s, _) = h.call("DELETE", &format!("{base}/{id}"), OWNER, None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (_, list) = h.get(OWNER, &base).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    h.person(&ws, json!({"name": "Priya Again", "emails": ["priya@acme.test"]})).await;
}

#[tokio::test]
async fn billable_people_cannot_link_their_own_mailbox() {
    let h = harness().await;
    let ws = h.workspace(&["staffing"]).await;
    let own = h.mailbox(&ws, "ravi@gmail.test").await;
    let hr = h.mailbox(&ws, "hr@acme.test").await;
    // Billable needs a connected HR mailbox to exist first.
    let (s, e) = h.call("POST", &format!("{}/team/people", api(&ws)), OWNER, Some(json!({"name": "Early", "emails": ["early@client.test"], "employee_type": "billable"}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(e["error"]["message"].as_str().unwrap().contains("connected HR mailbox"), "{e}");
    let (s, _) = h.call("PUT", &format!("{}/team/settings", api(&ws)), OWNER, Some(settings(json!({"hr_source_id": hr})))).await;
    assert_eq!(s, StatusCode::OK);
    let (s, e) = h
        .call("POST", &format!("{}/team/people", api(&ws)), OWNER, Some(json!({"name": "Ravi", "emails": ["ravi@client.test"], "employee_type": "billable", "source_id": own})))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(e["error"]["message"].as_str().unwrap().contains("only through the HR mailbox"), "{e}");
    // The HR mailbox is the one place a billable person may be linked. It cannot be unset while they exist.
    h.person(&ws, json!({"name": "Ravi", "emails": ["ravi@client.test"], "employee_type": "billable", "source_id": hr})).await;
    let (s, e) = h.call("PUT", &format!("{}/team/settings", api(&ws)), OWNER, Some(settings(json!({"hr_source_id": null})))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(e["error"]["message"].as_str().unwrap().contains("must stay set"), "{e}");
    sqlx::query("UPDATE email_sources SET status = 'disconnected' WHERE id = ?1").bind(&hr).execute(&h.state.db).await.unwrap();
    assert_eq!(h.call("PUT", &format!("{}/team/settings", api(&ws)), OWNER, Some(settings(json!({"hr_source_id": hr})))).await.0, StatusCode::BAD_REQUEST);
}

pub fn settings(extra: serde_json::Value) -> serde_json::Value {
    let mut s = json!({"tz_offset_minutes": 0, "manager_after_hours": null, "final_after_hours": null, "final_emails": [],
                       "below_target_hour": 16, "weekly_check_hour": 12});
    for (k, v) in extra.as_object().unwrap() {
        s[k] = v.clone();
    }
    s
}

#[tokio::test]
async fn csv_import_reports_every_problem_and_dry_run_changes_nothing() {
    let h = harness().await;
    let ws = h.workspace(&["staffing"]).await;
    let hr = h.mailbox(&ws, "hr@acme.test").await;
    h.call("PUT", &format!("{}/team/settings", api(&ws)), OWNER, Some(settings(json!({"hr_source_id": hr})))).await;
    let url = format!("{}/team/people/import", api(&ws));
    let csv = "name,email,employee_type,client,manager_email,role,joining_date,daily_target,weekly_target\n\
               Neha Rao,neha@acme.test,non-billable,,,team lead,2025-01-06,,\n\
               Priya Sharma,priya@acme.test;priya.s@acme.test,Non Billable,Capgemini,neha@acme.test,recruiter,2026-03-02,5,25\n\
               Ravi Kumar,ravi@client.test,billable,TCS,neha@acme.test,consultant,2026-01-12,,\n\
               Bad Email,not-an-email,,,,,,,\n\
               Bad Date,bd@acme.test,,,,,31/12/2026,,\n\
               Bad Type,bt@acme.test,contractor,,,,,,\n\
               Bad Target,btg@acme.test,,,,,,abc,\n\
               Ghost Manager,gm@acme.test,,,nobody@acme.test,,,,\n";
    let (s, dry) = h.call("POST", &url, OWNER, Some(json!({"csv": csv, "dry_run": true}))).await;
    assert_eq!(s, StatusCode::OK, "{dry}");
    assert_eq!((dry["dry_run"].clone(), dry["created"].as_i64(), dry["skipped"].as_i64(), dry["total_rows"].as_i64()), (json!(true), Some(4), Some(4), Some(8)));
    let (_, list) = h.get(OWNER, &format!("{}/team/people", api(&ws))).await;
    assert!(list["items"].as_array().unwrap().is_empty(), "a dry run must not write");

    let (_, real) = h.call("POST", &url, OWNER, Some(json!({"csv": csv}))).await;
    assert_eq!((real["created"].as_i64(), real["updated"].as_i64(), real["skipped"].as_i64()), (Some(4), Some(0), Some(4)));
    let errors = real["errors"].as_array().unwrap();
    let at = |row: i64| errors.iter().find(|e| e["row"] == row).unwrap_or_else(|| panic!("no error for row {row}: {real}"));
    assert_eq!(at(5)["field"], "emails");
    assert_eq!(at(6)["field"], "joining_date");
    assert_eq!(at(7)["field"], "employee_type");
    assert_eq!(at(8)["field"], "daily_target");
    assert_eq!(at(9)["field"], "manager_email");

    let (_, list) = h.get(OWNER, &format!("{}/team/people", api(&ws))).await;
    let items = list["items"].as_array().unwrap();
    let priya = items.iter().find(|p| p["name"] == "Priya Sharma").unwrap();
    let neha = items.iter().find(|p| p["name"] == "Neha Rao").unwrap();
    assert_eq!(priya["manager_id"], neha["id"]);
    assert_eq!(items.iter().find(|p| p["name"] == "Ravi Kumar").unwrap()["employee_type"], "billable");

    // Re-importing updates by email instead of duplicating.
    let (_, again) = h.call("POST", &url, OWNER, Some(json!({"csv": "name,email,weekly_target\nPriya Sharma,priya.s@acme.test,40\n"}))).await;
    assert_eq!((again["created"].as_i64(), again["updated"].as_i64()), (Some(0), Some(1)));
    let (_, list) = h.get(OWNER, &format!("{}/team/people", api(&ws))).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 4, "the row with an unknown manager is still created, with an error listed");
    assert_eq!(list["items"].as_array().unwrap().iter().find(|p| p["name"] == "Priya Sharma").unwrap()["weekly_target"], 40);

    let (s, e) = h.call("POST", &url, OWNER, Some(json!({"csv": "full_name\nX\n"}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(e["error"]["message"].as_str().unwrap().contains("name and email"), "{e}");
}

#[tokio::test]
async fn hr_fields_exist_only_with_the_staffing_pack() {
    let h = harness().await;
    let ws = h.workspace(&["general"]).await;
    let base = format!("{}/team/people", api(&ws));
    let (s, e) = h.call("POST", &base, OWNER, Some(json!({"name": "A", "emails": ["a@acme.test"], "employee_type": "billable"}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(e["error"]["message"].as_str().unwrap().contains("HR & recruiting pack"), "{e}");
    let p = h.person(&ws, json!({"name": "A", "emails": ["a@acme.test"], "role": "support", "daily_target": 20})).await;
    assert!(p.get("employee_type").is_none() && p.get("client").is_none(), "{p}");
    // The staffing endpoints answer 409 here.
    let (s, _) = h.get(OWNER, &format!("{}/staffing/submissions", api(&ws))).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, _) = h.get(OWNER, &format!("{}/staffing/timesheets", api(&ws))).await;
    assert_eq!(s, StatusCode::CONFLICT);
}
