//! Gap 6: layered escalation. An alert about a person goes to them, then their manager, then the final contacts,
//! with configurable delays. Works in every workspace; `below_target` is a generic rule.

use crate::staffing_common;

use email_ai_api::{alerts, platform::db::now_ms};
use serde_json::{Value, json};
use staffing_common::*;

const FRI_13: i64 = 1_792_155_600_000;

fn settings(manager: i64, finals: i64) -> Value {
    json!({"tz_offset_minutes": 0, "manager_after_hours": manager, "final_after_hours": finals, "final_emails": ["hr@acme.test"],
           "below_target_hour": 16, "weekly_check_hour": 12, "route_alerts_by_owner": true})
}

fn recipients(h: &H) -> Vec<String> {
    h.mail.take().into_iter().map(|m| m.0).collect()
}

#[tokio::test]
async fn unanswered_mail_goes_owner_then_manager_then_final_contacts() {
    let h = harness().await;
    // A plain business workspace: the escalation chain is not an HR feature.
    let ws = h.workspace(&["general"]).await;
    let src = h.mailbox(&ws, "asha@acme.test").await;
    let ben = h.person(&ws, json!({"name": "Ben Ortiz", "emails": ["ben@acme.test"], "role": "support lead"})).await;
    h.person(&ws, json!({"name": "Asha Nair", "emails": ["asha@acme.test"], "role": "support", "source_id": src, "manager_id": ben["id"]})).await;
    h.call("PUT", &format!("{}/team/settings", api(&ws)), OWNER, Some(settings(2, 3))).await;
    h.call("PUT", &format!("{}/alert-settings", api(&ws)), OWNER, Some(json!({"enabled": true, "recipients": ["owner@acme.test"], "escalate_to": [], "unanswered_hours": 4, "escalate_after_hours": 24, "daily_digest": false, "digest_hour": 9}))).await;

    let t0 = now_ms();
    let m = h.ingest(&ws, &src, email("cust@shop.test", &["asha@acme.test"], "Refund needed urgently", "Please answer", t0 - 5 * HOUR), "e1").await.unwrap();
    sqlx::query("INSERT INTO classifications (message_id, category, priority, requires_reply, confidence, needs_review, source, taxonomy_version, updated_at) VALUES (?1, 'question', 'high', 1, 1.0, 0, 'manual', 'taxonomy-v1', 1)")
        .bind(&m).execute(&h.state.db).await.unwrap();

    alerts::evaluate_workspace(&h.state, &ws, t0).await.unwrap();
    assert_eq!(recipients(&h), vec!["asha@acme.test"], "the mailbox owner first, not the workspace admins");
    alerts::evaluate_workspace(&h.state, &ws, t0 + HOUR).await.unwrap();
    assert!(recipients(&h).is_empty(), "nothing moves before the manager delay");
    alerts::evaluate_workspace(&h.state, &ws, t0 + 3 * HOUR).await.unwrap();
    assert_eq!(recipients(&h), vec!["ben@acme.test"]);
    alerts::evaluate_workspace(&h.state, &ws, t0 + 5 * HOUR).await.unwrap();
    assert!(recipients(&h).is_empty());
    alerts::evaluate_workspace(&h.state, &ws, t0 + 7 * HOUR).await.unwrap();
    assert_eq!(recipients(&h), vec!["hr@acme.test"]);
    let (_, list) = h.get(OWNER, &format!("{}/alerts", api(&ws))).await;
    let a = &list["items"][0];
    assert_eq!((a["escalation_level"].as_i64(), a["notified"].clone()), (Some(3), json!(["asha@acme.test", "ben@acme.test", "hr@acme.test"])));

    // The Escalations view shows each person's chain.
    let (_, e) = h.get(OWNER, &format!("{}/team/escalations", api(&ws))).await;
    let asha = e["chains"].as_array().unwrap().iter().find(|c| c["person"]["name"] == "Asha Nair").unwrap();
    let steps: Vec<(String, Vec<String>)> = asha["steps"].as_array().unwrap().iter().map(|s| (s["label"].as_str().unwrap().into(), s["emails"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().into()).collect())).collect();
    assert_eq!(steps, vec![("Owner".into(), vec!["asha@acme.test".into()]), ("Manager".into(), vec!["ben@acme.test".into()]), ("Final contacts".into(), vec!["hr@acme.test".into()])]);
    assert_eq!((e["manager_after_hours"].as_i64(), e["final_after_hours"].as_i64()), (Some(2), Some(3)));
}

#[tokio::test]
async fn a_manager_less_person_escalates_straight_to_the_final_contacts_and_bad_settings_are_refused() {
    let h = harness().await;
    let ws = h.workspace(&["general"]).await;
    let src = h.mailbox(&ws, "solo@acme.test").await;
    h.person(&ws, json!({"name": "Solo", "emails": ["solo@acme.test"], "source_id": src})).await;
    h.call("PUT", &format!("{}/team/settings", api(&ws)), OWNER, Some(settings(1, 1))).await;
    h.call("PUT", &format!("{}/alert-settings", api(&ws)), OWNER, Some(json!({"enabled": true, "recipients": ["owner@acme.test"], "escalate_to": [], "unanswered_hours": 4, "escalate_after_hours": 24, "daily_digest": false, "digest_hour": 9}))).await;
    let t0 = now_ms();
    let m = h.ingest(&ws, &src, email("cust@shop.test", &["solo@acme.test"], "Where is my order", "?", t0 - 6 * HOUR), "s1").await.unwrap();
    sqlx::query("INSERT INTO classifications (message_id, category, priority, requires_reply, confidence, needs_review, source, taxonomy_version, updated_at) VALUES (?1, 'question', 'high', 1, 1.0, 0, 'manual', 'taxonomy-v1', 1)")
        .bind(&m).execute(&h.state.db).await.unwrap();
    alerts::evaluate_workspace(&h.state, &ws, t0).await.unwrap();
    assert_eq!(recipients(&h), vec!["solo@acme.test"]);
    alerts::evaluate_workspace(&h.state, &ws, t0 + 2 * HOUR).await.unwrap();
    assert_eq!(recipients(&h), vec!["hr@acme.test"], "no manager: the final contacts take over");

    for bad in [json!({"manager_after_hours": 0}), json!({"final_emails": ["nope"]}), json!({"tz_offset_minutes": 900}), json!({"below_target_hour": 24})] {
        let mut s = settings(2, 3);
        for (k, v) in bad.as_object().unwrap() {
            s[k] = v.clone();
        }
        assert_eq!(h.call("PUT", &format!("{}/team/settings", api(&ws)), OWNER, Some(s)).await.0, 400, "{bad}");
    }
}

#[tokio::test]
async fn below_target_alerts_daily_and_weekly_and_clear_when_the_target_is_met() {
    let h = harness().await;
    let ws = h.workspace(&["general"]).await;
    let src = h.mailbox(&ws, "asha@acme.test").await;
    let ben = h.person(&ws, json!({"name": "Ben Ortiz", "emails": ["ben@acme.test"]})).await;
    h.person(&ws, json!({"name": "Asha Nair", "emails": ["asha@acme.test"], "source_id": src, "manager_id": ben["id"], "daily_target": 5, "weekly_target": 25, "joining_date": "2026-01-05"})).await;
    h.call("PUT", &format!("{}/alert-settings", api(&ws)), OWNER, Some(json!({"enabled": true, "recipients": ["owner@acme.test"], "escalate_to": [], "unanswered_hours": 4, "escalate_after_hours": 24, "daily_digest": false, "digest_hour": 9}))).await;
    let reply = |n: i64| {
        let ws = ws.clone();
        let src = src.clone();
        let h = &h;
        async move {
            let thread = format!("r{n}");
            h.ingest(&ws, &src, email("cust@shop.test", &["asha@acme.test"], &format!("Q{n}"), "help", WED - 4 * HOUR + n), &thread).await.unwrap();
            h.ingest(&ws, &src, sent(email("asha@acme.test", &["cust@shop.test"], &format!("Re: Q{n}"), "done", WED - 3 * HOUR + n)), &thread).await.unwrap();
        }
    };
    reply(1).await;
    reply(2).await;

    // Wednesday 10:00 is before the 16:00 check: quiet. At 17:00 two of five is below target.
    assert_eq!(alerts::evaluate_workspace(&h.state, &ws, WED).await.unwrap().raised, 0);
    let r = alerts::evaluate_workspace(&h.state, &ws, WED + 7 * HOUR).await.unwrap();
    assert_eq!(r.raised, 1);
    let (_, list) = h.get(OWNER, &format!("{}/alerts", api(&ws))).await;
    assert_eq!(list["items"][0]["kind"], "below_target");
    assert_eq!(list["items"][0]["title"], "Below target today: Asha Nair");
    assert_eq!(recipients(&h), vec!["asha@acme.test"]);
    assert_eq!(alerts::evaluate_workspace(&h.state, &ws, WED + 8 * HOUR).await.unwrap().raised, 0, "once per day");

    for n in 3..=5 {
        reply(n).await;
    }
    let r = alerts::evaluate_workspace(&h.state, &ws, WED + 9 * HOUR).await.unwrap();
    assert_eq!(r.raised, 0);
    assert_eq!(h.get(OWNER, &format!("{}/alerts", api(&ws))).await.1["open_count"], 0, "target met: the alert resolves itself");

    // Friday noon: this week's total (5) is under the weekly target (25).
    let r = alerts::evaluate_workspace(&h.state, &ws, FRI_13).await.unwrap();
    assert_eq!(r.raised, 1);
    let (_, list) = h.get(OWNER, &format!("{}/alerts", api(&ws))).await;
    assert_eq!(list["items"][0]["title"], "Below target this week: Asha Nair");
}

#[tokio::test]
async fn without_the_opt_in_a_general_workspace_keeps_todays_alert_routing_even_with_a_linked_mailbox() {
    let h = harness().await;
    let ws = h.workspace(&["general"]).await;
    let src = h.mailbox(&ws, "asha@acme.test").await;
    h.member(&ws, "boss@acme.test", "operator").await;
    h.person(&ws, json!({"name": "Asha", "emails": ["asha@acme.test"], "source_id": src})).await;
    // The default team settings: no owner routing.
    h.call("PUT", &format!("{}/alert-settings", api(&ws)), OWNER, Some(json!({"enabled": true, "recipients": ["owner@acme.test"], "escalate_to": ["boss@acme.test"],
        "unanswered_hours": 4, "escalate_after_hours": 24, "daily_digest": false, "digest_hour": 9}))).await;
    let t0 = now_ms();
    let m = h.ingest(&ws, &src, email("cust@shop.test", &["asha@acme.test"], "Where is my order", "?", t0 - 6 * HOUR), "s1").await.unwrap();
    sqlx::query("INSERT INTO classifications (message_id, category, priority, requires_reply, confidence, needs_review, source, taxonomy_version, updated_at) VALUES (?1, 'question', 'high', 1, 1.0, 0, 'manual', 'taxonomy-v1', 1)")
        .bind(&m).execute(&h.state.db).await.unwrap();
    alerts::evaluate_workspace(&h.state, &ws, t0).await.unwrap();
    assert_eq!(recipients(&h), vec!["owner@acme.test"], "level 1 goes to the alert recipients, not the mailbox owner");
    alerts::evaluate_workspace(&h.state, &ws, t0 + 25 * HOUR).await.unwrap();
    assert_eq!(recipients(&h), vec!["boss@acme.test"], "escalate_to is honoured");
    let owner: Option<String> = sqlx::query_scalar("SELECT person_id FROM alerts WHERE workspace_id = ?1").bind(&ws).fetch_one(&h.state.db).await.unwrap();
    assert!(owner.is_none());
}

#[tokio::test]
async fn below_target_is_one_rolling_alert_per_person() {
    let h = harness().await;
    let ws = h.workspace(&["general"]).await;
    let src = h.mailbox(&ws, "asha@acme.test").await;
    h.person(&ws, json!({"name": "Asha Nair", "emails": ["asha@acme.test"], "source_id": src, "daily_target": 5, "joining_date": "2026-01-05"})).await;
    h.call("PUT", &format!("{}/alert-settings", api(&ws)), OWNER, Some(json!({"enabled": true, "recipients": ["owner@acme.test"], "escalate_to": [], "unanswered_hours": 4, "escalate_after_hours": 24, "daily_digest": false, "digest_hour": 9}))).await;
    let count = || async { sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM alerts WHERE kind = 'below_target'").fetch_one(&h.state.db).await.unwrap() };
    let status = || async { sqlx::query_scalar::<_, String>("SELECT status FROM alerts WHERE kind = 'below_target'").fetch_one(&h.state.db).await.unwrap() };

    alerts::evaluate_workspace(&h.state, &ws, WED + 7 * HOUR).await.unwrap();
    assert_eq!((count().await, status().await.as_str()), (1, "open"));
    // The next morning the old window has closed: the alert resolves itself.
    alerts::evaluate_workspace(&h.state, &ws, WED + DAY + 2 * HOUR).await.unwrap();
    assert_eq!((count().await, status().await.as_str()), (1, "resolved"));
    // Thursday evening, still under target: the same alert reopens at level 1 rather than a second one appearing.
    let r = alerts::evaluate_workspace(&h.state, &ws, WED + DAY + 7 * HOUR).await.unwrap();
    assert_eq!(r.raised, 1);
    assert_eq!((count().await, status().await.as_str()), (1, "open"));
    let level: i64 = sqlx::query_scalar("SELECT escalation_level FROM alerts WHERE kind = 'below_target'").fetch_one(&h.state.db).await.unwrap();
    assert_eq!(level, 1);
    // A person who resolves it by hand is not nagged again within the same window.
    sqlx::query("UPDATE alerts SET status = 'resolved', resolved_by = 'usr_x' WHERE kind = 'below_target'").execute(&h.state.db).await.unwrap();
    alerts::evaluate_workspace(&h.state, &ws, WED + DAY + 8 * HOUR).await.unwrap();
    assert_eq!(status().await, "resolved");
}
