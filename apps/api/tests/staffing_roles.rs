//! The super admin and role-scoped access, enforced on the server for every level.

use crate::staffing_common;

use axum::http::StatusCode;
use serde_json::{Value, json};
use staffing_common::*;

fn settings() -> Value {
    json!({"tz_offset_minutes": 330, "manager_after_hours": 12, "final_after_hours": 12, "final_emails": [], "below_target_hour": 16, "weekly_check_hour": 12})
}

#[tokio::test]
async fn each_level_gets_exactly_its_own_powers() {
    let h = harness().await;
    let ws = h.workspace(&["staffing"]).await;
    let admin = h.member(&ws, "admin2@acme.test", "admin").await;
    let neha = h.member(&ws, "neha@acme.test", "operator").await;
    let priya = h.member(&ws, "priya@acme.test", "operator").await;
    let stranger = "dev:stranger@else.test";
    h.call("GET", "/api/v1/me", stranger, None).await;
    let lead = h.person(&ws, json!({"name": "Neha", "emails": ["neha@acme.test"], "role": "team lead"})).await;
    let rec = h.person(&ws, json!({"name": "Priya", "emails": ["priya@acme.test"], "manager_id": lead["id"]})).await;
    let team = format!("{}/team", api(&ws));

    // Levels.
    let level = |t: &'static str| {
        let (h, team) = (&h, team.clone());
        async move { h.get(t, &format!("{team}/access")).await }
    };
    let (s, a) = level(OWNER).await;
    assert_eq!((s, a["level"].as_str()), (StatusCode::OK, Some("super_admin")));
    assert_eq!(h.get(&admin, &format!("{team}/access")).await.1["level"], "admin");
    let (_, a) = h.get(&neha, &format!("{team}/access")).await;
    assert_eq!((a["level"].as_str(), a["is_manager"].as_bool(), a["report_count"].as_i64()), (Some("operator"), Some(true), Some(1)));
    let (_, a) = h.get(&priya, &format!("{team}/access")).await;
    assert_eq!((a["level"].as_str(), a["is_manager"].as_bool(), a["person"]["name"].as_str()), (Some("operator"), Some(false), Some("Priya")));
    assert_eq!(h.get(stranger, &format!("{team}/access")).await.0, StatusCode::FORBIDDEN);

    // Writes: only the super admin. Everyone else is told why.
    let new_person = json!({"name": "Z", "emails": ["z@acme.test"]});
    for (who, token) in [("admin", &admin), ("manager", &neha), ("recruiter", &priya), ("stranger", &stranger.to_string())] {
        assert_eq!(h.call("POST", &format!("{team}/people"), token, Some(new_person.clone())).await.0, StatusCode::FORBIDDEN, "{who} create");
        assert_eq!(h.call("PATCH", &format!("{team}/people/{}", rec["id"].as_str().unwrap()), token, Some(json!({"daily_target": 99}))).await.0, StatusCode::FORBIDDEN, "{who} patch");
        assert_eq!(h.call("DELETE", &format!("{team}/people/{}", rec["id"].as_str().unwrap()), token, None).await.0, StatusCode::FORBIDDEN, "{who} delete");
        assert_eq!(h.call("POST", &format!("{team}/people/import"), token, Some(json!({"csv": "name,email\nA,a@acme.test\n"}))).await.0, StatusCode::FORBIDDEN, "{who} import");
        assert_eq!(h.call("PUT", &format!("{team}/settings"), token, Some(settings())).await.0, StatusCode::FORBIDDEN, "{who} settings");
        assert_eq!(h.call("POST", &format!("{}/staffing/scan", api(&ws)), token, None).await.0, StatusCode::FORBIDDEN, "{who} scan");
    }
    let (s, e) = h.call("POST", &format!("{team}/people"), &admin, Some(new_person.clone())).await;
    assert!(e["error"]["message"].as_str().unwrap().contains("super admin"), "{s} {e}");
    assert_eq!(h.call("PUT", &format!("{team}/settings"), OWNER, Some(settings())).await.0, StatusCode::OK);
    assert_eq!(h.call("POST", &format!("{team}/people"), OWNER, Some(new_person)).await.0, StatusCode::CREATED);
    // Everybody can read the settings (the escalation chain is not a secret from the people in it).
    assert_eq!(h.get(&priya, &format!("{team}/settings")).await.1["tz_offset_minutes"], 330);

    // Reads: scoped by the manager chain, even for an admin who is not the super admin.
    let n = |t: &str| {
        let t = t.to_string();
        let (h, team) = (&h, team.clone());
        async move { h.get(&t, &format!("{team}/people")).await.1["items"].as_array().unwrap().len() }
    };
    assert_eq!((n(OWNER).await, n(&neha).await, n(&priya).await, n(&admin).await), (3, 2, 1, 0));
    assert_eq!(h.get(&priya, &format!("{team}/people/{}", lead["id"].as_str().unwrap())).await.0, StatusCode::NOT_FOUND, "a report cannot read upward");
    assert_eq!(h.get(&neha, &format!("{team}/people/{}", rec["id"].as_str().unwrap())).await.0, StatusCode::OK);

    // Escalation chains are scoped the same way.
    let chains = |t: &str| {
        let t = t.to_string();
        let (h, team) = (&h, team.clone());
        async move { h.get(&t, &format!("{team}/escalations")).await.1["chains"].as_array().unwrap().len() }
    };
    assert_eq!((chains(OWNER).await, chains(&neha).await, chains(&priya).await), (3, 2, 1));
}

#[tokio::test]
async fn the_creator_is_super_admin_and_existing_workspaces_are_backfilled() {
    let h = harness().await;
    let ws = h.workspace(&["general"]).await;
    let owner: String = sqlx::query_scalar("SELECT user_id FROM workspace_owners WHERE workspace_id = ?1").bind(&ws).fetch_one(&h.state.db).await.unwrap();
    let creator: String = sqlx::query_scalar("SELECT id FROM users WHERE email = 'owner@acme.test'").fetch_one(&h.state.db).await.unwrap();
    assert_eq!(owner, creator);

    // The migration's backfill picks the first admin of a workspace that has no owner row.
    let second = h.member(&ws, "second@acme.test", "admin").await;
    sqlx::query("DELETE FROM workspace_owners").execute(&h.state.db).await.unwrap();
    sqlx::query(
        "INSERT OR IGNORE INTO workspace_owners (workspace_id, user_id, created_at)
         SELECT workspace_id, user_id, 1 FROM memberships
         WHERE role = 'admin' AND rowid IN (SELECT MIN(rowid) FROM memberships WHERE role = 'admin' GROUP BY workspace_id)",
    )
    .execute(&h.state.db)
    .await
    .unwrap();
    let again: String = sqlx::query_scalar("SELECT user_id FROM workspace_owners WHERE workspace_id = ?1").bind(&ws).fetch_one(&h.state.db).await.unwrap();
    assert_eq!(again, creator, "the first admin, not the second");
    // And with no owner row at all the first admin still counts as the super admin.
    sqlx::query("DELETE FROM workspace_owners").execute(&h.state.db).await.unwrap();
    assert_eq!(h.get(OWNER, &format!("{}/team/access", api(&ws))).await.1["level"], "super_admin");
    assert_eq!(h.get(&second, &format!("{}/team/access", api(&ws))).await.1["level"], "admin");
}

#[tokio::test]
async fn managers_decide_on_their_reports_submissions_and_nobody_else_does() {
    let h = harness().await;
    let ws = h.workspace(&["staffing"]).await;
    let neha_t = h.member(&ws, "neha@acme.test", "operator").await;
    let karan_t = h.member(&ws, "karan@acme.test", "operator").await;
    let src = h.mailbox(&ws, "log@acme.test").await;
    let neha = h.person(&ws, json!({"name": "Neha", "emails": ["neha@acme.test"]})).await;
    let priya = h.person(&ws, json!({"name": "Priya", "emails": ["priya@acme.test"], "manager_id": neha["id"]})).await;
    h.person(&ws, json!({"name": "Karan", "emails": ["karan@acme.test"]})).await;
    submission(&h, &ws, priya["id"].as_str().unwrap(), &src, 1, WED, "pending", false, "active").await;
    let url = format!("{}/staffing/submissions", api(&ws));
    let id = format!("sub_{}_1", priya["id"].as_str().unwrap());
    assert_eq!(h.get(&karan_t, &url).await.1["items"].as_array().unwrap().len(), 0);
    assert_eq!(h.get(&neha_t, &url).await.1["items"].as_array().unwrap().len(), 1);
    assert_eq!(h.call("PATCH", &format!("{url}/{id}"), &karan_t, Some(json!({"dismissed": true}))).await.0, StatusCode::NOT_FOUND);
    assert_eq!(h.call("PATCH", &format!("{url}/{id}"), &neha_t, Some(json!({"outcome": "interview"}))).await.0, StatusCode::OK);
    assert_eq!(h.get(&karan_t, &format!("{url}?person_id={}", priya["id"].as_str().unwrap())).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_owner_must_stay_an_admin_and_can_hand_the_role_over() {
    let h = harness().await;
    let ws = h.workspace(&["general"]).await;
    let second = h.member(&ws, "second@acme.test", "admin").await;
    let op = h.member(&ws, "op@acme.test", "operator").await;
    let team = format!("{}/team", api(&ws));
    // Demoting the super admin through the members route is refused.
    let (s, e) = h.call("POST", &format!("{}/members", api(&ws)), second.as_str(), Some(json!({"email": "owner@acme.test", "role": "operator"}))).await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");
    // Even if the membership were changed behind our back, an owner row without the admin role is no super admin.
    sqlx::query("UPDATE memberships SET role = 'operator' WHERE user_id = (SELECT id FROM users WHERE email = 'owner@acme.test')").execute(&h.state.db).await.unwrap();
    assert_eq!(h.get(OWNER, &format!("{team}/access")).await.1["level"], "operator");
    sqlx::query("UPDATE memberships SET role = 'admin' WHERE user_id = (SELECT id FROM users WHERE email = 'owner@acme.test')").execute(&h.state.db).await.unwrap();

    // Transfer: owner only, to an admin member only.
    assert_eq!(h.call("POST", &format!("{team}/ownership"), &second, Some(json!({"email": "second@acme.test"}))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(h.call("POST", &format!("{team}/ownership"), OWNER, Some(json!({"email": "op@acme.test"}))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(h.call("POST", &format!("{team}/ownership"), OWNER, Some(json!({"email": "nobody@acme.test"}))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(h.call("POST", &format!("{team}/ownership"), OWNER, Some(json!({"email": "second@acme.test"}))).await.0, StatusCode::OK);
    assert_eq!(h.get(&second, &format!("{team}/access")).await.1["level"], "super_admin");
    assert_eq!(h.get(OWNER, &format!("{team}/access")).await.1["level"], "admin", "the old owner stays an admin");
    assert_eq!(h.call("POST", &format!("{team}/people"), OWNER, Some(json!({"name": "Z", "emails": ["z@acme.test"]}))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(h.call("POST", &format!("{team}/people"), &second, Some(json!({"name": "Z", "emails": ["z@acme.test"]}))).await.0, StatusCode::CREATED);
    let _ = op;
}

#[tokio::test]
async fn visibility_is_applied_before_the_limit() {
    let h = harness().await;
    let ws = h.workspace(&["staffing"]).await;
    let src = h.mailbox(&ws, "log@acme.test").await;
    let karan_t = h.member(&ws, "karan@acme.test", "operator").await;
    let karan = h.person(&ws, json!({"name": "Karan", "emails": ["karan@acme.test"]})).await;
    let other = h.person(&ws, json!({"name": "Other", "emails": ["other@acme.test"]})).await;
    submission(&h, &ws, karan["id"].as_str().unwrap(), &src, 1, WED - 30 * DAY, "pending", false, "active").await;
    for n in 2..8 {
        submission(&h, &ws, other["id"].as_str().unwrap(), &src, n, WED - n as i64 * HOUR, "pending", false, "active").await;
    }
    let (_, v) = h.get(&karan_t, &format!("{}/staffing/submissions?limit=2", api(&ws))).await;
    assert_eq!(v["items"].as_array().unwrap().len(), 1, "his own older submission is not crowded out by 6 newer ones he may not see");
    assert_eq!(h.get(OWNER, &format!("{}/staffing/submissions?limit=2", api(&ws))).await.1["items"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn non_owners_do_not_see_contacts_and_the_mail_wall_follows_the_manager_chain_when_on() {
    let h = harness().await;
    let ws = h.workspace(&["staffing"]).await;
    let neha_t = h.member(&ws, "neha@acme.test", "operator").await;
    let priya_t = h.member(&ws, "priya@acme.test", "operator").await;
    let (nb, pb, ob) = (h.mailbox(&ws, "neha@acme.test").await, h.mailbox(&ws, "priya@acme.test").await, h.mailbox(&ws, "other@acme.test").await);
    let neha = h.person(&ws, json!({"name": "Neha", "emails": ["neha@acme.test"], "source_id": nb})).await;
    h.person(&ws, json!({"name": "Priya", "emails": ["priya@acme.test"], "manager_id": neha["id"], "source_id": pb})).await;
    h.person(&ws, json!({"name": "Other", "emails": ["other@acme.test"], "source_id": ob})).await;
    let team = format!("{}/team", api(&ws));
    let mut s = settings();
    s["final_emails"] = json!(["hr@acme.test"]);
    s["hr_source_id"] = json!(null);
    assert_eq!(h.call("PUT", &format!("{team}/settings"), OWNER, Some(s.clone())).await.0, StatusCode::OK);

    // Redaction.
    let (_, st) = h.get(&priya_t, &format!("{team}/settings")).await;
    assert_eq!((st["final_emails"].clone(), st.get("hr_source_id").is_none(), st["manager_after_hours"].as_i64()), (json!([]), true, Some(12)));
    assert_eq!(h.get(OWNER, &format!("{team}/settings")).await.1["final_emails"], json!(["hr@acme.test"]));
    let (_, esc) = h.get(&priya_t, &format!("{team}/escalations")).await;
    assert_eq!(esc["final_emails"], json!([]));
    assert!(esc["chains"][0]["steps"].as_array().unwrap().iter().all(|s| s["emails"].as_array().unwrap().is_empty()));
    assert_eq!(esc["chains"][0]["steps"][1]["name"], "Neha", "names stay, so the chain is still readable");

    // Wall off (default): everyone reads the shared inbox, as before.
    let m1 = h.ingest(&ws, &pb, email("c@x.test", &["priya@acme.test"], "For Priya", "x", WED), "w1").await.unwrap();
    h.ingest(&ws, &ob, email("c@x.test", &["other@acme.test"], "For Other", "x", WED + 1), "w2").await.unwrap();
    let subjects = |v: Value| v["items"].as_array().unwrap().iter().map(|m| m["subject"].as_str().unwrap().to_string()).collect::<Vec<_>>().len();
    let all = format!("{}/messages?direction=all", api(&ws));
    assert_eq!(subjects(h.get(&priya_t, &all).await.1), 2);

    // Wall on: operators only read their own subtree's mailboxes.
    s["restrict_mail_to_team"] = json!(true);
    assert_eq!(h.call("PUT", &format!("{team}/settings"), OWNER, Some(s)).await.0, StatusCode::OK);
    assert_eq!(subjects(h.get(&priya_t, &all).await.1), 1);
    assert_eq!(subjects(h.get(&neha_t, &all).await.1), 1, "Neha's subtree has her own and Priya's mailboxes, which hold one mail");
    assert_eq!(subjects(h.get(OWNER, &all).await.1), 2);
    let other_id: String = sqlx::query_scalar("SELECT id FROM email_messages WHERE subject = 'For Other'").fetch_one(&h.state.db).await.unwrap();
    assert_eq!(h.get(&priya_t, &format!("{}/messages/{other_id}", api(&ws))).await.0, StatusCode::NOT_FOUND);
    assert_eq!(h.get(&priya_t, &format!("{}/messages/{m1}", api(&ws))).await.0, StatusCode::OK);
    assert_eq!(h.get(&priya_t, &format!("{all}&source_id={ob}")).await.1["items"].as_array().unwrap().len(), 0, "a filter cannot step outside the wall");
}
