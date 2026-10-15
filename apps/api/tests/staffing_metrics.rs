//! Gap 3 and 8: exact-SQL metrics. Per person (today, this week, since joining, interview rate), per team, the
//! role-scoped API, and the generic metrics every other pack gets. The library functions are the hook for Ask Meera.

use crate::staffing_common;

use axum::http::StatusCode;
use email_ai_api::staffing::metrics as hrms;
use serde_json::{Value, json};
use staffing_common::*;

fn metric(m: &Value, key: &str) -> Value {
    m["metrics"].as_array().unwrap().iter().find(|v| v["key"] == key).cloned().unwrap_or_else(|| panic!("{key} in {m}"))
}

#[tokio::test]
async fn person_and_team_numbers_come_from_exact_sql() {
    let h = harness().await;
    let ws = h.workspace(&["staffing"]).await;
    let src = h.mailbox(&ws, "log@acme.test").await;
    let neha = h.person(&ws, json!({"name": "Neha Rao", "emails": ["neha@acme.test"], "role": "team lead"})).await;
    let priya = h.person(&ws, json!({"name": "Priya Sharma", "emails": ["priya@acme.test"], "role": "recruiter", "manager_id": neha["id"],
                                     "joining_date": "2026-03-02", "daily_target": 5, "weekly_target": 25})).await;
    let karan = h.person(&ws, json!({"name": "Karan Mehta", "emails": ["karan@acme.test"], "role": "recruiter", "manager_id": neha["id"],
                                     "joining_date": "2026-05-04", "daily_target": 5, "weekly_target": 25})).await;
    let (p, k) = (priya["id"].as_str().unwrap(), karan["id"].as_str().unwrap());

    // Wednesday 14 Oct 10:00 UTC. Priya: 2 today, 3 earlier this week, 5 last week, 1 before she joined, 1 dismissed.
    let rows: [(i64, &str, bool, &str); 12] = [
        (WED - HOUR, "pending", false, "active"),
        (WED - 2 * HOUR, "interview", true, "active"),
        (WED - 2 * DAY, "pending", false, "active"),
        (WED - 2 * DAY + HOUR, "rejected", false, "active"),
        (WED - 2 * DAY + 2 * HOUR, "selected", true, "active"),
        (WED - 8 * DAY, "joined", true, "active"),
        (WED - 9 * DAY, "rejected", true, "active"),
        (WED - 9 * DAY + HOUR, "pending", false, "active"),
        (WED - 10 * DAY, "pending", false, "active"),
        (WED - 10 * DAY + HOUR, "pending", false, "active"),
        (WED - 250 * DAY, "pending", false, "active"),
        (WED - HOUR, "pending", false, "dismissed"),
    ];
    for (n, (at, outcome, interviewed, status)) in rows.iter().enumerate() {
        submission(&h, &ws, p, &src, n, *at, outcome, *interviewed, status).await;
    }
    submission(&h, &ws, k, &src, 100, WED - 3 * HOUR, "pending", false, "active").await;

    let m = hrms::person_metrics(&h.state.db, &ws, p, WED).await.unwrap().unwrap();
    let m = serde_json::to_value(m).unwrap();
    assert_eq!((m["primary_metric"].as_str(), m["today"].as_f64(), m["week"].as_f64()), (Some("submissions"), Some(2.0), Some(5.0)));
    assert_eq!((m["daily_target"].as_i64(), m["weekly_target"].as_i64()), (Some(5), Some(25)));
    let subs = metric(&m, "submissions");
    assert_eq!((subs["today"].as_f64(), subs["week"].as_f64(), subs["since_joining"].as_f64()), (Some(2.0), Some(5.0), Some(10.0)), "the pre-joining and dismissed rows are excluded");
    let rate = metric(&m, "interview_rate");
    assert_eq!((rate["today"].as_f64(), rate["week"].as_f64(), rate["since_joining"].as_f64()), (Some(0.5), Some(0.4), Some(0.4)));
    for (key, expected) in [("interviews", 4.0), ("selected", 2.0), ("joined", 1.0), ("rejected", 2.0)] {
        assert_eq!(metric(&m, key)["since_joining"].as_f64(), Some(expected), "{key}");
    }
    // Generic metrics are there too, and empty rather than made up.
    assert_eq!(metric(&m, "replies_sent")["since_joining"].as_f64(), Some(0.0));
    assert!(metric(&m, "avg_response_minutes")["since_joining"].is_null());

    // A new joiner with no submissions has no rate, not 0%.
    let none = serde_json::to_value(hrms::person_metrics(&h.state.db, &ws, karan["id"].as_str().unwrap(), WED - 40 * DAY).await.unwrap().unwrap()).unwrap();
    assert!(metric(&none, "interview_rate")["since_joining"].is_null());

    // Team of Neha: sums of counts, the rate recomputed from the sums.
    let t = serde_json::to_value(hrms::team_metrics(&h.state.db, &ws, Some(neha["id"].as_str().unwrap()), WED).await.unwrap()).unwrap();
    assert_eq!((t["members"].as_array().unwrap().len(), t["today"].as_f64(), t["week"].as_f64()), (2, Some(3.0), Some(6.0)));
    assert_eq!((t["daily_target"].as_i64(), t["weekly_target"].as_i64()), (Some(10), Some(50)));
    assert_eq!(metric(&t, "interview_rate")["since_joining"].as_f64(), Some(0.36), "4 interviews / 11 submissions");
    // Everyone (no manager) includes the manager herself.
    let all = hrms::team_metrics(&h.state.db, &ws, None, WED).await.unwrap();
    assert_eq!(all.members.len(), 3);

    // The lookup Ask Meera would use to resolve a name.
    let found = hrms::find_people(&h.state.db, &ws, "priy").await.unwrap();
    assert_eq!((found.len(), found[0].id.as_str()), (1, p));
    assert!(hrms::find_people(&h.state.db, &ws, "").await.unwrap().is_empty());
    assert!(hrms::person_metrics(&h.state.db, "ws_other", p, WED).await.unwrap().is_none(), "never across workspaces");
}

#[tokio::test]
async fn api_views_follow_the_manager_chain() {
    let h = harness().await;
    let ws = h.workspace(&["staffing"]).await;
    let neha_t = h.member(&ws, "neha@acme.test", "operator").await;
    let priya_t = h.member(&ws, "priya@acme.test", "operator").await;
    let karan_t = h.member(&ws, "karan@acme.test", "operator").await;
    let outsider_t = h.member(&ws, "sam@acme.test", "admin").await;
    let neha = h.person(&ws, json!({"name": "Neha Rao", "emails": ["neha@acme.test"], "role": "team lead"})).await;
    let priya = h.person(&ws, json!({"name": "Priya Sharma", "emails": ["priya@acme.test"], "manager_id": neha["id"], "daily_target": 5})).await;
    let karan = h.person(&ws, json!({"name": "Karan Mehta", "emails": ["karan@acme.test"], "manager_id": neha["id"]})).await;
    let base = format!("{}/team", api(&ws));
    let ids = |v: &Value| -> Vec<String> { v["items"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap().to_string()).collect() };

    // The directory: owner everyone, manager her team, recruiter herself, a person outside the chain nobody.
    assert_eq!(ids(&h.get(OWNER, &format!("{base}/people")).await.1).len(), 3);
    assert_eq!(ids(&h.get(&neha_t, &format!("{base}/people")).await.1).len(), 3);
    assert_eq!(ids(&h.get(&priya_t, &format!("{base}/people")).await.1), vec!["Priya Sharma"]);
    assert!(ids(&h.get(&outsider_t, &format!("{base}/people")).await.1).is_empty(), "an admin who is not the super admin sees no one by default");

    // Metrics: own numbers, reports for a manager, 404 for someone else's.
    let (s, me) = h.get(&priya_t, &format!("{base}/metrics/me")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(me["person"]["name"], "Priya Sharma");
    assert_eq!(metric(&me["metrics"], "submissions")["key"], "submissions");
    assert_eq!(h.get(&priya_t, &format!("{base}/metrics/people/{}", karan["id"].as_str().unwrap())).await.0, StatusCode::NOT_FOUND);
    assert_eq!(h.get(&neha_t, &format!("{base}/metrics/people/{}", karan["id"].as_str().unwrap())).await.0, StatusCode::OK);
    assert_eq!(h.get(&karan_t, &format!("{base}/metrics/people/{}", priya["id"].as_str().unwrap())).await.0, StatusCode::NOT_FOUND);
    let (_, none) = h.get(&outsider_t, &format!("{base}/metrics/me")).await;
    assert!(none["person"].is_null() && none["metrics"].is_null(), "no widget for someone outside the directory");

    // Team view: a manager sees the people below her; a recruiter has no team; the owner sees everyone.
    let members = |v: &Value| v["members"].as_array().unwrap().len();
    assert_eq!(members(&h.get(&neha_t, &format!("{base}/metrics/team")).await.1), 2);
    assert_eq!(members(&h.get(&priya_t, &format!("{base}/metrics/team")).await.1), 0);
    assert_eq!(members(&h.get(&outsider_t, &format!("{base}/metrics/team")).await.1), 0);
    assert_eq!(members(&h.get(OWNER, &format!("{base}/metrics/team")).await.1), 3);
    assert_eq!(h.get(&priya_t, &format!("{base}/metrics/team?manager_id={}", neha["id"].as_str().unwrap())).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn other_packs_get_generic_metrics_with_the_same_framework() {
    let h = harness().await;
    let ws = h.workspace(&["general"]).await;
    let src = h.mailbox(&ws, "asha@acme.test").await;
    let asha = h.person(&ws, json!({"name": "Asha Nair", "emails": ["asha@acme.test"], "role": "support", "source_id": src, "daily_target": 20, "joining_date": "2026-01-05"})).await;
    // Two customer threads: answered after 60 and 30 minutes. A third reply goes to a thread nobody wrote in first: not a reply.
    for (n, (asked, answered)) in [(WED - 3 * HOUR, WED - 2 * HOUR), (WED - 5 * HOUR, WED - 5 * HOUR + 30 * 60_000)].into_iter().enumerate() {
        h.ingest(&ws, &src, email("cust@shop.test", &["asha@acme.test"], &format!("Question {n}"), "help", asked), &format!("g{n}")).await.unwrap();
        h.ingest(&ws, &src, sent(email("asha@acme.test", &["cust@shop.test"], &format!("Re: Question {n}"), "done", answered)), &format!("g{n}")).await.unwrap();
    }
    h.ingest(&ws, &src, sent(email("asha@acme.test", &["boss@acme.test"], "Standalone note", "fyi", WED - HOUR)), "g9").await.unwrap();

    let m = serde_json::to_value(hrms::person_metrics(&h.state.db, &ws, asha["id"].as_str().unwrap(), WED).await.unwrap().unwrap()).unwrap();
    assert_eq!((m["primary_metric"].as_str(), m["today"].as_f64(), m["daily_target"].as_i64()), (Some("replies_sent"), Some(2.0), Some(20)));
    assert_eq!(metric(&m, "avg_response_minutes")["today"].as_f64(), Some(45.0));
    let keys: Vec<&str> = m["metrics"].as_array().unwrap().iter().map(|v| v["key"].as_str().unwrap()).collect();
    assert_eq!(keys, vec!["replies_sent", "avg_response_minutes"], "no HR & recruiting metrics outside that pack");
    let (_, access) = h.get(OWNER, &format!("{}/team/access", api(&ws))).await;
    assert_eq!((access["staffing"].clone(), access["primary_metric"].as_str()), (json!(false), Some("replies_sent")));
}
