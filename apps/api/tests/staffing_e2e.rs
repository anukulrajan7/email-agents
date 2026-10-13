//! The owner's seven requirements (R1-R7 in docs/shrikant-verification.md), each run end to end through the
//! HTTP API and the real job pipeline with a seeded company: people from a CSV, mail, attachments, portal CSVs.

mod staffing_common;

use axum::http::StatusCode;
use email_ai_api::{alerts, db::now_ms, jobs::worker, staffing::metrics as hrms};
use serde_json::{Value, json};
use staffing_common::*;

const OCT5: i64 = 1_791_201_600_000;

struct Co {
    h: H,
    ws: String,
    intake: String,
    hr_box: String,
    priya_box: String,
    neha_box: String,
    priya_t: String,
    neha_t: String,
    ids: std::collections::HashMap<String, String>,
}

/// The staffing company: Neha leads Priya (recruiter), Ravi and Dev are billable consultants.
async fn company() -> Co {
    let h = harness().await;
    let ws = h.workspace(&["staffing"]).await;
    let neha_t = h.member(&ws, "neha@acme.test", "operator").await;
    let priya_t = h.member(&ws, "priya@acme.test", "operator").await;
    let (_, src) = h.call("POST", &format!("{}/sources/forwarding", api(&ws)), OWNER, None).await;
    let intake = src["id"].as_str().unwrap().to_string();
    let hr_box = h.mailbox(&ws, "hr@acme.test").await;
    let priya_box = h.mailbox(&ws, "priya@acme.test").await;
    let neha_box = h.mailbox(&ws, "neha@acme.test").await;
    let csv = "name,email,employee_type,client,manager_email,role,joining_date,daily_target,weekly_target,mailbox\n\
               Neha Rao,neha@acme.test,non_billable,,,team lead,2025-01-06,,,neha@acme.test\n\
               Priya Sharma,priya@acme.test,non_billable,,neha@acme.test,recruiter,2026-03-02,5,25,priya@acme.test\n\
               Ravi Kumar,ravi@client.test,billable,TCS,neha@acme.test,consultant,2026-01-12,,,\n\
               Dev Patel,dev@client.test,billable,Capgemini,neha@acme.test,consultant,2026-01-12,,,\n";
    let (s, r) = h.call("POST", &format!("{}/team/people/import", api(&ws)), OWNER, Some(json!({"csv": csv}))).await;
    assert_eq!((s, r["created"].as_i64(), r["errors"].clone()), (StatusCode::OK, Some(4), json!([])), "{r}");
    let (_, list) = h.get(OWNER, &format!("{}/team/people", api(&ws))).await;
    let ids = list["items"].as_array().unwrap().iter().map(|p| (p["name"].as_str().unwrap().split(' ').next().unwrap().to_string(), p["id"].as_str().unwrap().to_string())).collect();
    let settings = json!({"tz_offset_minutes": 0, "manager_after_hours": 24, "final_after_hours": 24, "final_emails": ["hr-head@acme.test"], "below_target_hour": 16,
                          "weekly_check_hour": 12, "hr_source_id": hr_box, "client_domains": [], "timesheet_grace_days": 3, "hours_tolerance": 0.5});
    assert_eq!(h.call("PUT", &format!("{}/team/settings", api(&ws)), OWNER, Some(settings)).await.0, StatusCode::OK);
    h.call("PUT", &format!("{}/alert-settings", api(&ws)), OWNER, Some(json!({"enabled": true, "recipients": ["owner@acme.test"], "escalate_to": [], "unanswered_hours": 4, "escalate_after_hours": 24, "daily_digest": false, "digest_hour": 9}))).await;
    Co { h, ws, intake, hr_box, priya_box, neha_box, priya_t, neha_t, ids }
}

async fn forwarded(co: &Co, from: &str, subject: &str, body: &str) {
    let (s, _) = co.h.call("POST", &format!("{}/sources/{}/test-email", api(&co.ws), co.intake), OWNER, Some(json!({"from": from, "subject": subject, "body_text": body}))).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    worker::drain(&co.h.state).await;
}

/// A referral and a requirement arrive and become an explained match (the R7 pipeline).
async fn matching_pipeline(co: &Co) {
    forwarded(co, "Friend <friend@gmail.com>", "Referral - Rahul Verma", "Hi, referring my friend.\nName: Rahul Verma\n6 years in Java, SpringBoot, Microservices.\nPune. Notice period 30 days.\nrahul.v@mail.com").await;
    forwarded(co, "Recruiter <hr@capgemini.com>", "Requirement: Java Developer", "Need 3 developers with Java, Spring Boot and Microservices. 5-8 yrs. Location Pune.").await;
}

fn metric(m: &Value, key: &str) -> Value {
    m["metrics"].as_array().unwrap().iter().find(|v| v["key"] == key).cloned().unwrap_or_else(|| panic!("{key} in {m}"))
}

#[tokio::test]
async fn r7_referrals_match_requirements_and_a_sent_cv_links_to_both() {
    let co = company().await;
    let (h, ws) = (&co.h, &co.ws);
    matching_pipeline(&co).await;
    let (_, matches) = h.get(OWNER, &format!("{}/matching/matches", api(ws))).await;
    assert_eq!((matches["items"][0]["candidate"]["name"].as_str(), matches["items"][0]["score"].as_i64()), (Some("Rahul Verma"), Some(90)));

    // Priya submits Rahul to Capgemini: the submission links to the same candidate and requirement.
    h.ingest(ws, &co.priya_box, sent(with_files(email("priya@acme.test", &["hr@capgemini.com"], "Java Developer profile - Rahul Verma", "Please find Rahul's profile.", now_ms() - 60_000), &["Rahul_Verma_CV.pdf"])), "p1").await.unwrap();
    let (_, scan) = h.call("POST", &format!("{}/staffing/scan", api(ws)), OWNER, None).await;
    assert_eq!(scan["created"], 1, "{scan}");
    let (_, subs) = h.get(OWNER, &format!("{}/staffing/submissions", api(ws))).await;
    let s = &subs["items"][0];
    assert_eq!(s["link_status"], "auto");
    assert_eq!(s["candidate_id"], matches["items"][0]["candidate"]["id"]);
    assert_eq!(s["requirement_id"], matches["items"][0]["requirement"]["id"]);
}

#[tokio::test]
async fn r1_a_recruiter_sees_today_and_this_week_against_target_and_a_manager_sees_the_team() {
    let co = company().await;
    let (h, ws) = (&co.h, &co.ws);
    matching_pipeline(&co).await;
    let t = now_ms();
    for (n, (cv, subject)) in [("Rahul_Verma_CV.pdf", "Java Developer profile - Rahul Verma"), ("Anita_Desai_CV.pdf", "Profile - Anita Desai"), ("Kiran_Rao_CV.docx", "Profile - Kiran Rao")].into_iter().enumerate() {
        h.ingest(ws, &co.priya_box, sent(with_files(email("priya@acme.test", &["hr@capgemini.com"], subject, "Profile attached.", t - 600_000 + n as i64), &[cv])), &format!("p{n}")).await.unwrap();
    }
    // Noise that must not count: a CV to a candidate's own Gmail, and a reply with no CV.
    h.ingest(ws, &co.priya_box, sent(with_files(email("priya@acme.test", &["rahul@gmail.com"], "Your CV", "check", t - 500_000), &["Rahul_Verma_CV.pdf"])), "px").await.unwrap();
    h.ingest(ws, &co.priya_box, sent(email("priya@acme.test", &["hr@capgemini.com"], "Re: Profile - Anita Desai", "Any update?", t - 400_000)), "p1").await.unwrap();
    assert_eq!(h.call("POST", &format!("{}/staffing/scan", api(ws)), OWNER, None).await.1["created"], 3);

    let (s, me) = h.get(&co.priya_t, &format!("{}/team/metrics/me", api(ws))).await;
    assert_eq!(s, StatusCode::OK);
    let m = &me["metrics"];
    assert_eq!((m["primary_metric"].as_str(), m["today"].as_f64(), m["daily_target"].as_i64(), m["weekly_target"].as_i64()), (Some("submissions"), Some(3.0), Some(5), Some(25)));
    assert!(m["week"].as_f64().unwrap() >= 3.0);

    // The client interviews Anita: quality (interview rate) moves, counts do not.
    h.ingest(ws, &co.priya_box, email("hr@capgemini.com", &["priya@acme.test"], "Re: Profile - Anita Desai", "Thanks, the panel would like to schedule an interview.", t - 100_000), "p1").await.unwrap();
    h.call("POST", &format!("{}/staffing/scan", api(ws)), OWNER, None).await;
    let (_, me) = h.get(&co.priya_t, &format!("{}/team/metrics/me", api(ws))).await;
    assert_eq!(metric(&me["metrics"], "submissions")["since_joining"].as_f64(), Some(3.0));
    assert_eq!(metric(&me["metrics"], "interview_rate")["since_joining"].as_f64(), Some(0.33));

    // The team lead sees her team; the recruiter only herself; someone outside the directory gets no widget.
    let (_, team) = h.get(&co.neha_t, &format!("{}/team/metrics/team", api(ws))).await;
    assert_eq!((team["members"].as_array().unwrap().len(), team["today"].as_f64(), team["daily_target"].as_i64()), (3, Some(3.0), Some(5)), "Priya plus the two consultants");
    let (_, mine) = h.get(&co.priya_t, &format!("{}/team/people", api(ws))).await;
    assert_eq!(mine["items"].as_array().unwrap().len(), 1);
    let (_, nobody) = h.get(OWNER, &format!("{}/team/metrics/me", api(ws))).await;
    assert!(nobody["person"].is_null());
    // Before 4 pm Priya is not flagged; after it, 3 of 5 raises a below-target alert to her first.
    let w = email_ai_api_clock_day(t);
    let r = alerts::evaluate_workspace(&h.state, ws, w + 17 * HOUR).await.unwrap();
    assert!(r.raised >= 1, "{r:?}");
    let (_, list) = h.get(OWNER, &format!("{}/alerts", api(ws))).await;
    assert!(list["items"].as_array().unwrap().iter().any(|a| a["kind"] == "below_target" && a["title"] == "Below target today: Priya Sharma"), "{list}");
}

fn email_ai_api_clock_day(ms: i64) -> i64 {
    ms.div_euclid(DAY) * DAY
}

#[tokio::test]
async fn r2_billable_staff_mail_is_limited_to_the_hr_mailbox_and_missing_timesheets_alert() {
    let co = company().await;
    let (h, ws) = (&co.h, &co.ws);
    // Billable Ravi's client mail never enters a personal mailbox; his timesheet through HR does.
    assert!(h.ingest(ws, &co.neha_box, email("ravi@client.test", &["neha@acme.test"], "Client escalation", "private client details", OCT5 - 9 * DAY), "n1").await.is_none());
    let ts = h.ingest(ws, &co.hr_box, email("dev@client.test", &["hr@acme.test"], "September timesheet", "Total hours: 168", OCT5 - 4 * DAY), "h1").await.unwrap();
    h.classify(&ts, "timesheet").await;
    assert!(h.ingest(ws, &co.neha_box, email("customer@shop.test", &["neha@acme.test"], "Quote", "price?", OCT5 - 9 * DAY), "n2").await.is_some(), "non-billable: full mailbox");

    let r = alerts::evaluate_workspace(&h.state, ws, OCT5).await.unwrap();
    assert_eq!(r.raised, 1, "only Ravi is overdue: {r:?}");
    let (_, list) = h.get(OWNER, &format!("{}/alerts", api(ws))).await;
    assert_eq!(list["items"][0]["title"], "Timesheet missing: Ravi Kumar (2026-09)");
    assert_eq!(h.mail.take().iter().map(|m| m.0.clone()).collect::<Vec<_>>(), vec!["ravi@client.test"]);
}

#[tokio::test]
async fn r3_portal_mismatch_escalates_by_level_until_someone_fixes_it() {
    let co = company().await;
    let (h, ws) = (&co.h, &co.ws);
    for (who, text) in [("ravi", "Total hours: 152"), ("dev", "Total hours: 168")] {
        let m = h.ingest(ws, &co.hr_box, email(&format!("{who}@client.test"), &["hr@acme.test"], "September timesheet", text, OCT5 - 4 * DAY), &format!("t-{who}")).await.unwrap();
        h.classify(&m, "timesheet").await;
    }
    let (s, r) = h.call("POST", &format!("{}/staffing/portal/hours", api(ws)), OWNER, Some(json!({"csv": "email,month,hours\nravi@client.test,2026-09,160\ndev@client.test,2026-09,168\n"}))).await;
    assert_eq!((s, r["created"].as_i64()), (StatusCode::OK, Some(2)));
    alerts::evaluate_workspace(&h.state, ws, OCT5).await.unwrap();
    let (_, m) = h.get(OWNER, &format!("{}/staffing/portal/mismatches", api(ws))).await;
    assert_eq!((m["items"].as_array().unwrap().len(), m["items"][0]["person_name"].as_str(), m["items"][0]["kind"].as_str()), (1, Some("Ravi Kumar"), Some("hours_differ")));
    let to = |h: &H| h.mail.take().into_iter().map(|m| m.0).collect::<Vec<_>>();
    assert_eq!(to(h), vec!["ravi@client.test"], "level 1: the employee");
    alerts::evaluate_workspace(&h.state, ws, OCT5 + 25 * HOUR).await.unwrap();
    assert_eq!(to(h), vec!["neha@acme.test"], "level 2: the manager");
    alerts::evaluate_workspace(&h.state, ws, OCT5 + 50 * HOUR).await.unwrap();
    assert_eq!(to(h), vec!["hr-head@acme.test"], "level 3: HR");
    // The portal gets corrected: the mismatch and its alert close.
    h.call("POST", &format!("{}/staffing/portal/hours", api(ws)), OWNER, Some(json!({"csv": "email,month,hours\nravi@client.test,2026-09,152\n"}))).await;
    alerts::evaluate_workspace(&h.state, ws, OCT5 + 51 * HOUR).await.unwrap();
    assert_eq!(h.get(OWNER, &format!("{}/alerts", api(ws))).await.1["open_count"], 0);
}

#[tokio::test]
async fn r4_every_mail_is_read_immediately_but_deep_reading_of_old_mail_waits_until_it_is_opened() {
    let co = company().await;
    let (h, ws) = (&co.h, &co.ws);
    let t = now_ms();
    let mut ids = vec![];
    for (age, subject) in [(0, "New request"), (5 * DAY, "Last week's thread"), (20 * DAY, "Old thread from a backfill")] {
        let id = h.ingest(ws, &co.neha_box, email("customer@shop.test", &["neha@acme.test"], subject, "Hello, can you send the quote for 40 licences please?", t - age), &format!("r4-{age}")).await.unwrap();
        ids.push(id);
    }
    worker::drain(&h.state).await;
    let has = |table: &'static str, id: String| {
        let db = h.state.db.clone();
        async move { sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM {table} WHERE message_id = ?1")).bind(id).fetch_one(&db).await.unwrap() > 0 }
    };
    for id in &ids {
        assert!(has("classifications", id.clone()).await, "every mail is labelled on arrival, whatever its age");
    }
    assert!(has("message_insights", ids[0].clone()).await);
    assert!(!has("message_insights", ids[2].clone()).await, "GAP: old mail is not summarised until someone opens it (AI_AUTO_HOURS)");
    let (s, _) = h.call("POST", &format!("{}/messages/{}/insight", api(ws), ids[2]), OWNER, None).await;
    assert_eq!(s, StatusCode::OK);
    worker::drain(&h.state).await;
    assert!(has("message_insights", ids[2].clone()).await);
}

#[tokio::test]
async fn r5_each_persons_mailbox_is_their_own_scope() {
    let co = company().await;
    let (h, ws) = (&co.h, &co.ws);
    h.ingest(ws, &co.priya_box, email("client@capgemini.com", &["priya@acme.test"], "For Priya", "x", WED), "q1").await.unwrap();
    h.ingest(ws, &co.neha_box, email("client@tcs.com", &["neha@acme.test"], "For Neha", "x", WED), "q2").await.unwrap();
    let subjects = |v: Value| v["items"].as_array().unwrap().iter().map(|m| m["subject"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    let path = |p: &str| format!("{}/messages?direction=all&person_id={}", api(ws), co.ids[p]);
    assert_eq!(subjects(h.get(OWNER, &path("Priya")).await.1), vec!["For Priya"]);
    assert_eq!(subjects(h.get(&co.neha_t, &path("Neha")).await.1), vec!["For Neha"]);
    // Neha manages Priya so she may scope to her; Priya may not scope to Neha.
    assert_eq!(subjects(h.get(&co.neha_t, &path("Priya")).await.1), vec!["For Priya"]);
    assert_eq!(h.get(&co.priya_t, &path("Neha")).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn r6_exact_answers_for_what_ask_meera_needs_to_say() {
    let co = company().await;
    let (h, ws) = (&co.h, &co.ws);
    // "How many submissions did Priya do since joining, and what % got interviews?"
    let person = hrms::find_people(&h.state.db, ws, "priya").await.unwrap();
    assert_eq!(person.len(), 1);
    for (n, (cv, thread)) in [("Rahul_Verma_CV.pdf", "m1"), ("Anita_Desai_CV.pdf", "m2"), ("Kiran_Rao_CV.pdf", "m3"), ("Meera_Joshi_CV.pdf", "m4")].into_iter().enumerate() {
        h.ingest(ws, &co.priya_box, sent(with_files(email("priya@acme.test", &["hr@capgemini.com"], &format!("Profile {n}"), "attached", OCT5 - 10 * DAY + n as i64), &[cv])), thread).await.unwrap();
    }
    h.ingest(ws, &co.priya_box, email("hr@capgemini.com", &["priya@acme.test"], "Re: Profile 0", "Please arrange an interview slot.", OCT5 - 9 * DAY), "m1").await.unwrap();
    h.ingest(ws, &co.priya_box, email("hr@capgemini.com", &["priya@acme.test"], "Re: Profile 1", "We will schedule an interview.", OCT5 - 9 * DAY), "m2").await.unwrap();
    email_ai_api::staffing::submissions::scan_workspace(&h.state, ws, OCT5).await.unwrap();
    email_ai_api::staffing::submissions::update_outcomes(&h.state.db, ws, OCT5).await.unwrap();
    let m = serde_json::to_value(hrms::person_metrics(&h.state.db, ws, &person[0].id, OCT5).await.unwrap().unwrap()).unwrap();
    assert_eq!(metric(&m, "submissions")["since_joining"].as_f64(), Some(4.0));
    assert_eq!(metric(&m, "interview_rate")["since_joining"].as_f64(), Some(0.5));
    assert_eq!(metric(&m, "interviews")["since_joining"].as_f64(), Some(2.0));
}
