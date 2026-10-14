//! Timesheets as records: read from pasted mail and attached sheets, matched to people, replaced not duplicated,
//! kept after the email is gone, and never answered by an agent.

mod staffing_common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::http::StatusCode;
use email_ai_api::{
    ai::{
        AiError,
        routing::SheetTier,
        timesheet_reader::{ModelSheet, TimesheetReader},
    },
    jobs::{handlers, worker},
    staffing::{
        policy,
        timesheet_parse::{Entry, Identity, Sheet},
        timesheet_records as records,
    },
};
use serde_json::json;
use staffing_common::*;

const SEP_TABLE: &str = "Employee ID: E1024\nEmployee Name: Ravi Kumar\nClient: Acme Corp\n\n| Date | Project | Hours |\n| --- | --- | --- |\n\
    | 2026-09-01 | Portal | 8 |\n| 2026-09-02 | Portal | 7.5 |\n| 2026-09-03 | Support | 8 |\n| Total | | 23.5 |\n";
/// 2026-10-02 09:00 UTC.
const OCT2: i64 = 1_790_931_600_000;

struct Fixture {
    h: H,
    ws: String,
    src: String,
    ravi: String,
}

async fn fixture() -> Fixture {
    let h = harness().await;
    let ws = h.workspace(&["staffing"]).await;
    let src = h.mailbox(&ws, "hr@acme.test").await;
    let ravi = h.person(&ws, json!({"name": "Ravi Kumar", "emails": ["ravi@client.test"], "employee_code": "E1024", "client": "Acme Corp"})).await["id"].as_str().unwrap().to_string();
    h.person(&ws, json!({"name": "Priya Shah", "emails": ["priya@acme.test"], "employee_code": "E2000"})).await;
    Fixture { h, ws, src, ravi }
}

impl Fixture {
    async fn timesheet(&self, from: &str, subject: &str, body: &str, at: i64, thread: &str) -> String {
        let id = self.h.ingest(&self.ws, &self.src, email(from, &["hr@acme.test"], subject, body, at), thread).await.unwrap();
        self.h.classify(&id, "timesheet").await;
        id
    }

    async fn record(&self, id: &str) -> records::Run {
        records::record_message(&self.h.state, &self.ws, id).await.unwrap()
    }

    async fn rows(&self) -> Vec<(String, Option<String>, String, Option<String>, String, Option<f64>)> {
        sqlx::query_as("SELECT id, person_id, status, replaces_id, match_method, total_hours FROM timesheet_records ORDER BY created_at, rowid")
            .fetch_all(&self.h.state.db)
            .await
            .unwrap()
    }
}

async fn put_file_text(h: &H, message_id: &str, text: &str) {
    sqlx::query("UPDATE attachments SET text_content = ?2, text_status = 'extracted', extract_status = 'done', extract_kind = 'sheet' WHERE message_id = ?1")
        .bind(message_id)
        .bind(text)
        .execute(&h.state.db)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_pasted_timesheet_becomes_a_record_and_reading_it_again_changes_nothing() {
    let f = fixture().await;
    let id = f.timesheet("ravi@client.test", "September timesheet", SEP_TABLE, OCT2, "t1").await;
    let run = f.record(&id).await;
    assert_eq!(run.records.len(), 1);
    let r = &run.records[0];
    assert_eq!((r.status.as_str(), r.person_id.as_deref(), r.total_hours), ("recorded", Some(f.ravi.as_str()), Some(23.5)));
    assert_eq!((r.period_start.as_str(), r.period_end.as_str()), ("2026-09-01", "2026-09-30"));
    let again = f.record(&id).await;
    assert_eq!(again.records.len(), 1);
    assert!(!again.records[0].created, "the same email is not recorded twice");
    assert_eq!(f.rows().await.len(), 1);
    let entries: String = sqlx::query_scalar("SELECT entries_json FROM timesheet_records").fetch_one(&f.h.state.db).await.unwrap();
    assert!(entries.contains("\"hours\":7.5") && entries.contains("Support"));
}

#[tokio::test]
async fn an_attached_sheet_is_read_from_its_extracted_text() {
    let f = fixture().await;
    let id = f.h.ingest(&f.ws, &f.src, with_files(email("ravi@client.test", &["hr@acme.test"], "Hours", "see attached", OCT2), &["timesheet-september.xlsx"]), "t2").await.unwrap();
    f.h.classify(&id, "timesheet").await;
    let waiting = f.record(&id).await;
    assert!(waiting.waiting_for_files && waiting.records.is_empty(), "the file has not been read yet");
    put_file_text(&f.h, &id, &format!("## Sheet: Sep (3 rows × 3 columns)\n{}", SEP_TABLE.split("\n\n").nth(1).unwrap())).await;
    let run = f.record(&id).await;
    assert_eq!(run.records.len(), 1);
    let (source, kind, file): (String, String, String) =
        sqlx::query_as("SELECT source, attachment_kind, attachment_filename FROM timesheet_records").fetch_one(&f.h.state.db).await.unwrap();
    assert_eq!((source.as_str(), kind.as_str(), file.as_str()), ("attachment", "Excel", "timesheet-september.xlsx"));
    assert_eq!(run.records[0].total_hours, Some(23.5));
}

#[tokio::test]
async fn a_new_sheet_for_the_same_person_and_period_replaces_the_old_one() {
    let f = fixture().await;
    let first = f.timesheet("ravi@client.test", "September timesheet", SEP_TABLE, OCT2, "t1").await;
    f.record(&first).await;
    let fixed = SEP_TABLE.replace("| 2026-09-03 | Support | 8 |", "| 2026-09-03 | Support | 6 |").replace("23.5", "21.5");
    let second = f.timesheet("ravi@client.test", "September timesheet (corrected)", &fixed, OCT2 + DAY, "t2").await;
    f.record(&second).await;
    let rows = f.rows().await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].2, "replaced");
    assert_eq!((rows[1].2.as_str(), rows[1].3.as_deref(), rows[1].5), ("recorded", Some(rows[0].0.as_str()), Some(21.5)));
    // Reading the older email again never brings it back over the newer one.
    sqlx::query("DELETE FROM timesheet_records WHERE id = ?1").bind(&rows[0].0).execute(&f.h.state.db).await.unwrap();
    f.record(&first).await;
    let rows = f.rows().await;
    assert_eq!(rows.iter().filter(|r| r.2 == "replaced").count(), 1);
    assert_eq!(rows.iter().filter(|r| r.2 == "recorded").count(), 1);
}

#[tokio::test]
async fn days_that_do_not_add_up_need_a_check_and_a_missing_period_uses_the_email_month() {
    let f = fixture().await;
    let bad = SEP_TABLE.replace("23.5", "30");
    let id = f.timesheet("ravi@client.test", "September timesheet", &bad, OCT2, "t1").await;
    let r = &f.record(&id).await.records[0];
    assert_eq!(r.status, "needs_check");
    assert!(r.issues.iter().any(|i| i["code"] == "total_mismatch"));

    let id = f.timesheet("ravi@client.test", "Timesheet", "Hi, my hours for the month. Total hours: 160", OCT2 + DAY, "t2").await;
    let r = &f.record(&id).await.records[0];
    assert_eq!((r.total_hours, r.status.as_str()), (Some(160.0), "recorded"));
    assert_eq!((r.period_start.as_str(), r.period_end.as_str()), ("2026-09-01", "2026-09-30"), "two days into October is still September's timesheet");
    assert!(r.issues.iter().any(|i| i["code"] == "period_assumed"));

    let id = f.timesheet("ravi@client.test", "Timesheet", "Hi, no numbers in this one", OCT2 + 2 * DAY, "t3").await;
    let r = &f.record(&id).await.records[0];
    assert_eq!((r.total_hours, r.status.as_str()), (None, "needs_check"), "nothing is invented");
}

#[tokio::test]
async fn the_person_is_matched_by_id_then_email_then_name_and_a_forwarder_never_owns_the_sheet() {
    let f = fixture().await;
    let method = |id: String| {
        let db = f.h.state.db.clone();
        async move { sqlx::query_as::<_, (Option<String>, String)>("SELECT person_id, match_method FROM timesheet_records WHERE message_id = ?1").bind(id).fetch_one(&db).await.unwrap() }
    };
    // By employee ID, although the sender is someone else entirely.
    let a = f.timesheet("somebody@else.test", "Timesheet Sept", "Employee ID: e1024\nTotal hours: 150", OCT2, "a").await;
    f.record(&a).await;
    assert_eq!(method(a).await, (Some(f.ravi.clone()), "code".into()));
    // By the sender's email when the sheet names no one.
    let b = f.timesheet("ravi@client.test", "Timesheet Sept", "Total hours: 150", OCT2 + 1, "b").await;
    f.record(&b).await;
    assert_eq!(method(b).await, (Some(f.ravi.clone()), "email".into()));
    // By an email written in the sheet.
    let c = f.timesheet("hr-desk@acme.test", "Timesheet Sept", "Email: Ravi@Client.test\nTotal hours: 150", OCT2 + 2, "c").await;
    f.record(&c).await;
    assert_eq!(method(c).await, (Some(f.ravi.clone()), "email".into()));
    // By name only.
    let d = f.timesheet("hr-desk@acme.test", "Timesheet Sept", "Employee Name: Kumar, Ravi\nTotal hours: 150", OCT2 + 3, "d").await;
    f.record(&d).await;
    assert_eq!(method(d).await, (Some(f.ravi.clone()), "name".into()));
    // Forwarded by Priya, the manager: the sheet names Ravi.
    let e = f.timesheet("priya@acme.test", "Fwd: Timesheet Sept", "Employee Name: Ravi Kumar\nTotal hours: 150", OCT2 + 4, "e").await;
    f.record(&e).await;
    assert_eq!(method(e).await, (Some(f.ravi.clone()), "name".into()), "the employee in the sheet, not the forwarder");
    // A name nobody has, forwarded by Priya: not Priya's.
    let g = f.timesheet("priya@acme.test", "Fwd: Timesheet Sept", "Employee Name: Dinesh Rao\nTotal hours: 150", OCT2 + 5, "g").await;
    let run = f.record(&g).await;
    assert_eq!(method(g).await, (None, "none".into()));
    let r = &run.records[0];
    assert!(r.needs_attention() && r.issues.iter().any(|i| i["message"] == "Who is this? (Dinesh Rao)"));
}

#[tokio::test]
async fn linking_an_unknown_sheet_to_a_person_is_remembered_for_the_next_one() {
    let f = fixture().await;
    let first = f.timesheet("hr-desk@acme.test", "Timesheet Sept", "Employee ID: X-77\nEmployee Name: D. Rao\nTotal hours: 100", OCT2, "a").await;
    let run = f.record(&first).await;
    let rec_id = run.records[0].id.clone();
    assert_eq!(run.records[0].person_id, None);

    let base = format!("{}/staffing/timesheet-records", api(&f.ws));
    let (s, body) = f.h.call("POST", &format!("{base}/{rec_id}/link"), OWNER, Some(json!({"person_id": f.ravi}))).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!((body["person_id"].as_str(), body["match_method"].as_str(), body["status"].as_str()), (Some(f.ravi.as_str()), Some("manual"), Some("recorded")));
    assert!(!body["issues"].to_string().contains("Who is this"));

    let second = f.timesheet("hr-desk@acme.test", "Timesheet Oct", "Employee ID: x-77\nTotal hours: 90\nPeriod: 01-Oct-2026 to 31-Oct-2026", OCT2 + DAY, "b").await;
    let r = &f.record(&second).await.records[0];
    assert_eq!(r.person_id.as_deref(), Some(f.ravi.as_str()));
    let m: String = sqlx::query_scalar("SELECT match_method FROM timesheet_records WHERE id = ?1").bind(&r.id).fetch_one(&f.h.state.db).await.unwrap();
    assert_eq!(m, "alias");
    assert_eq!(f.h.call("POST", &format!("{base}/{rec_id}/link"), "dev:nobody@else.test", Some(json!({"person_id": f.ravi}))).await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_recorded_timesheet_counts_as_submitted_even_when_the_email_is_gone() {
    let f = fixture().await;
    let id = f.timesheet("ravi@client.test", "September timesheet", SEP_TABLE, OCT2, "t1").await;
    f.record(&id).await;
    let t = policy::timesheet_mail(&f.h.state.db, &f.ws, &f.ravi, "2026-09").await.unwrap().expect("submitted");
    assert_eq!(t.hours, Some(23.5));
    assert!(policy::timesheet_mail(&f.h.state.db, &f.ws, &f.ravi, "2026-08").await.unwrap().is_none());
}

#[tokio::test]
async fn records_survive_deleting_the_email_and_disconnecting_the_mailbox() {
    let f = fixture().await;
    let id = f.h.ingest(&f.ws, &f.src, with_files(email("ravi@client.test", &["hr@acme.test"], "September timesheet", "see attached", OCT2), &["timesheet-september.xlsx"]), "t1").await.unwrap();
    f.h.classify(&id, "timesheet").await;
    put_file_text(&f.h, &id, SEP_TABLE).await;
    f.record(&id).await;

    let (s, body) = f.h.call("POST", &format!("{}/sources/{}/disconnect", api(&f.ws), f.src), OWNER, Some(json!({"purge": true}))).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    worker::drain(&f.h.state).await;
    let mails: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM email_messages WHERE id = ?1").bind(&id).fetch_one(&f.h.state.db).await.unwrap();
    assert_eq!(mails, 0, "the email is purged");

    let (message_id, attachment_id, subject, file, total, entries): (Option<String>, Option<String>, String, String, f64, String) = sqlx::query_as(
        "SELECT message_id, attachment_id, email_subject, attachment_filename, total_hours, entries_json FROM timesheet_records",
    )
    .fetch_one(&f.h.state.db)
    .await
    .unwrap();
    assert_eq!((message_id, attachment_id), (None, None));
    assert_eq!((subject.as_str(), file.as_str(), total), ("September timesheet", "timesheet-september.xlsx", 23.5));
    assert!(entries.contains("2026-09-02"));
    let (s, rec) = f.h.get(OWNER, &format!("{}/staffing/timesheet-records?month=2026-09", api(&f.ws))).await;
    assert_eq!(s, StatusCode::OK);
    let item = &rec["items"][0];
    assert_eq!((item["email"]["available"].as_bool(), item["email"]["subject"].as_str()), (Some(false), Some("September timesheet")));
    let (_, detail) = f.h.get(OWNER, &format!("{}/staffing/timesheet-records/{}", api(&f.ws), item["id"].as_str().unwrap())).await;
    assert_eq!(detail["entries"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn deleting_one_email_keeps_its_record_too() {
    let f = fixture().await;
    let id = f.timesheet("ravi@client.test", "September timesheet", SEP_TABLE, OCT2, "t1").await;
    f.record(&id).await;
    let (s, body) = f.h.call("POST", &format!("{}/messages/delete", api(&f.ws)), OWNER, Some(json!({"ids": [id]}))).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let (message_id, n): (Option<String>, i64) =
        sqlx::query_as("SELECT message_id, (SELECT COUNT(*) FROM timesheet_records) FROM timesheet_records").fetch_one(&f.h.state.db).await.unwrap();
    assert_eq!((message_id, n), (None, 1));
}

#[tokio::test]
async fn the_grid_has_a_row_per_expected_person_with_hours_per_day_and_marks_who_is_missing() {
    let f = fixture().await;
    sqlx::query("UPDATE people SET employee_type = 'billable' WHERE workspace_id = ?1").bind(&f.ws).execute(&f.h.state.db).await.unwrap();
    let id = f.timesheet("ravi@client.test", "September timesheet", SEP_TABLE, OCT2, "t1").await;
    f.record(&id).await;
    let (s, g) = f.h.get(OWNER, &format!("{}/staffing/timesheet-grid?month=2026-09", api(&f.ws))).await;
    assert_eq!(s, StatusCode::OK, "{g}");
    assert_eq!(g["days"].as_array().unwrap().len(), 30);
    assert_eq!((g["days"][4]["weekday"].as_str(), g["days"][4]["weekend"].as_bool()), (Some("Sat"), Some(true)));
    let rows = g["rows"].as_array().unwrap();
    let ravi = rows.iter().find(|r| r["name"] == "Ravi Kumar").unwrap();
    assert_eq!((ravi["cells"]["2026-09-02"].as_f64(), ravi["total"].as_f64(), ravi["status"].as_str(), ravi["employee_code"].as_str()), (Some(7.5), Some(23.5), Some("recorded"), Some("E1024")));
    let priya = rows.iter().find(|r| r["name"] == "Priya Shah").unwrap();
    assert_eq!(priya["status"], "missing");
    assert_eq!(g["summary"], json!({"recorded": 1, "needs_check": 0, "missing": 1}));
    let (_, d) = f.h.get(OWNER, &format!("{}/messages/{id}", api(&f.ws))).await;
    assert_eq!(d["timesheet_records"][0]["total_hours"], 23.5);
}

struct Scripted {
    calls: Mutex<Vec<&'static str>>,
    cheap: Option<Sheet>,
    strong: Option<Sheet>,
}

#[async_trait]
impl TimesheetReader for Scripted {
    fn has(&self, _tier: SheetTier) -> bool {
        true
    }

    async fn read(&self, tier: SheetTier, _subject: &str, _text: &str, _year: i64) -> Result<ModelSheet, AiError> {
        self.calls.lock().unwrap().push(if tier == SheetTier::Cheap { "cheap" } else { "strong" });
        let sheet = if tier == SheetTier::Cheap { &self.cheap } else { &self.strong };
        sheet.clone().map(|sheet| ModelSheet { sheet, cost_usd: 0.001 }).ok_or(AiError::Permanent("nothing".into()))
    }
}

fn model_sheet(total: f64, days: &[(&str, f64)]) -> Sheet {
    Sheet {
        identity: Identity { name: Some("Ravi Kumar".into()), ..Default::default() },
        period: Some(("2026-09-01".into(), "2026-09-30".into())),
        total: Some(total),
        entries: days.iter().map(|(d, h)| Entry { date: Some((*d).into()), hours: *h, project: None, note: None }).collect(),
    }
}

#[tokio::test]
async fn a_model_reads_only_what_the_rules_could_not_and_the_strong_one_only_when_the_cheap_one_fails() {
    let mut f = fixture().await;
    let messy = "Ravi Kumar worked on 1st Sept eight hrs, 2nd Sept 8 hrs. That is 16 in all.";
    let reader = Arc::new(Scripted { calls: Mutex::new(vec![]), cheap: Some(model_sheet(16.0, &[("2026-09-01", 8.0), ("2026-09-02", 8.0)])), strong: None });
    f.h.state.timesheet_reader = reader.clone();
    let id = f.timesheet("ravi@client.test", "Timesheet", messy, OCT2, "m1").await;
    let r = &f.record(&id).await.records[0];
    assert_eq!((r.total_hours, r.status.as_str()), (Some(16.0), "recorded"));
    assert_eq!(*reader.calls.lock().unwrap(), ["cheap"]);

    // The rules read this one fine: no model call.
    let id = f.timesheet("ravi@client.test", "September timesheet", SEP_TABLE, OCT2 + DAY, "m2").await;
    f.record(&id).await;
    assert_eq!(reader.calls.lock().unwrap().len(), 1);

    // The cheap answer does not add up, so the strong one is asked; its answer is kept.
    let reader = Arc::new(Scripted {
        calls: Mutex::new(vec![]),
        cheap: Some(model_sheet(20.0, &[("2026-09-01", 8.0), ("2026-09-02", 8.0)])),
        strong: Some(model_sheet(16.0, &[("2026-09-01", 8.0), ("2026-09-02", 8.0)])),
    });
    f.h.state.timesheet_reader = reader.clone();
    let id = f.timesheet("ravi@client.test", "Timesheet again", "Ravi Kumar: 1 Sep 8 hrs; 2 Sep 8 hrs; summary 16 or maybe 20", OCT2 + 2 * DAY, "m3").await;
    let r = &f.record(&id).await.records[0];
    assert_eq!(r.total_hours, Some(16.0));
    assert_eq!(*reader.calls.lock().unwrap(), ["cheap", "strong"]);
}

#[tokio::test]
async fn the_desk_never_drafts_a_reply_for_a_timesheet_but_a_person_still_can() {
    let f = fixture().await;
    let id = f.timesheet("ravi@client.test", "September timesheet", SEP_TABLE, OCT2, "t1").await;
    f.record(&id).await;
    assert!(handlers::create_draft_for_message(&f.h.state, &id, None).await.unwrap().is_none());
    assert!(handlers::create_agent_draft(&f.h.state, &id, None, None, "agent:test", None, None).await.unwrap().is_none());
    let drafts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM drafts WHERE message_id = ?1").bind(&id).fetch_one(&f.h.state.db).await.unwrap();
    assert_eq!(drafts, 0);
    assert!(records::suppresses_auto_reply(&f.h.state.db, &f.ws, &id).await.unwrap());
    // Labelled but not yet read still counts as the desk's.
    let other = f.timesheet("ravi@client.test", "Timesheet", "x", OCT2 + 1, "t2").await;
    assert!(records::suppresses_auto_reply(&f.h.state.db, &f.ws, &other).await.unwrap());
    let plain = f.h.ingest(&f.ws, &f.src, email("x@client.test", &["hr@acme.test"], "Lunch", "hi", OCT2 + 2), "t3").await.unwrap();
    assert!(!records::suppresses_auto_reply(&f.h.state.db, &f.ws, &plain).await.unwrap());
}

#[tokio::test]
async fn mail_that_says_timesheet_gets_the_label_by_rule_unless_a_person_chose_another() {
    let f = fixture().await;
    let by_subject = f.h.ingest(&f.ws, &f.src, email("someone@client.test", &["hr@acme.test"], "My September time sheet", "attached soon", OCT2), "l1").await.unwrap();
    let by_file = f.h.ingest(&f.ws, &f.src, with_files(email("someone@client.test", &["hr@acme.test"], "Hello", "hi", OCT2 + 1), &["Timecard_Sep.pdf"]), "l2").await.unwrap();
    let manual = f.h.ingest(&f.ws, &f.src, email("someone@client.test", &["hr@acme.test"], "Timesheet question", "hi", OCT2 + 2), "l3").await.unwrap();
    f.h.classify(&manual, "leave").await;
    worker::drain(&f.h.state).await;
    let label = |id: String| {
        let db = f.h.state.db.clone();
        async move { sqlx::query_as::<_, (String, String)>("SELECT category, source FROM classifications WHERE message_id = ?1").bind(id).fetch_one(&db).await.unwrap() }
    };
    assert_eq!(label(by_subject.clone()).await, ("timesheet".into(), "fallback".into()));
    assert_eq!(label(by_file).await.0, "timesheet");
    assert_eq!(label(manual).await, ("leave".into(), "manual".into()));
    let jobs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE kind = 'record_timesheet' AND payload_json LIKE ?1").bind(format!("%{by_subject}%")).fetch_one(&f.h.state.db).await.unwrap();
    assert_eq!(jobs, 1, "labelling queues the reading");
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM timesheet_records WHERE message_id = ?1").bind(&by_subject).fetch_one(&f.h.state.db).await.unwrap();
    assert_eq!(n, 1, "and the job ran: a mail with no hours is recorded as one to check");
}

#[tokio::test]
async fn the_backfill_records_old_timesheet_mail_once() {
    let f = fixture().await;
    let id = f.timesheet("ravi@client.test", "September timesheet", SEP_TABLE, OCT2, "t1").await;
    assert_eq!(records::backfill(&f.h.state, &f.ws, 20).await.unwrap(), 1);
    assert_eq!(records::backfill(&f.h.state, &f.ws, 20).await.unwrap(), 0);
    let _ = id;
    assert_eq!(f.rows().await.len(), 1);
}

#[tokio::test]
async fn rohan_records_and_never_replies_and_the_upgrade_skips_tuned_agents() {
    let f = fixture().await;
    email_ai_api::agent_registry::seed::ensure_workspace(&f.h.state.db, &f.ws).await.unwrap();
    let agent: (String, i64) = sqlx::query_as("SELECT id, template_version FROM agents WHERE workspace_id = ?1 AND template_key = 'hrms.timesheets'")
        .bind(&f.ws)
        .fetch_one(&f.h.state.db)
        .await
        .unwrap();
    assert_eq!(agent.1, 4);
    let spec: String = sqlx::query_scalar("SELECT spec_json FROM agent_versions WHERE agent_id = ?1 ORDER BY version DESC LIMIT 1").bind(&agent.0).fetch_one(&f.h.state.db).await.unwrap();
    let steps = serde_json::from_str::<serde_json::Value>(&spec).unwrap()["steps"].clone();
    assert_eq!(steps, json!([{"type": "record_timesheet"}, {"type": "create_review", "title": "Check: Timesheet"}]));
    assert!(!spec.contains("draft_reply") && !spec.contains("draft_email") && !spec.contains("send_email"), "{spec}");

    // An agent still on the old template follows the upgrade; a tuned one keeps its own spec.
    sqlx::query("UPDATE agents SET template_version = 3 WHERE id = ?1").bind(&agent.0).execute(&f.h.state.db).await.unwrap();
    email_ai_api::agent_registry::seed::ensure_workspace(&f.h.state.db, &f.ws).await.unwrap();
    let after: (i64, i64) = sqlx::query_as("SELECT template_version, active_version FROM agents WHERE id = ?1").bind(&agent.0).fetch_one(&f.h.state.db).await.unwrap();
    assert_eq!(after.0, 4);
    sqlx::query("UPDATE agents SET template_version = 3, tuned = 1 WHERE id = ?1").bind(&agent.0).execute(&f.h.state.db).await.unwrap();
    email_ai_api::agent_registry::seed::ensure_workspace(&f.h.state.db, &f.ws).await.unwrap();
    let kept: (i64, i64) = sqlx::query_as("SELECT template_version, active_version FROM agents WHERE id = ?1").bind(&agent.0).fetch_one(&f.h.state.db).await.unwrap();
    assert_eq!(kept, (3, after.1), "a tuned agent is left alone");
}

#[tokio::test]
async fn employee_ids_are_unique_editable_and_imported_from_csv() {
    let f = fixture().await;
    let base = format!("{}/team/people", api(&f.ws));
    let (s, body) = f.h.call("POST", &base, OWNER, Some(json!({"name": "Dev Patel", "emails": ["dev@acme.test"], "employee_code": "e1024"}))).await;
    assert!(!s.is_success() && body.to_string().contains("already belongs to Ravi Kumar"), "{body}");
    let (s, body) = f.h.call("PATCH", &format!("{base}/{}", f.ravi), OWNER, Some(json!({"employee_code": "RK-1"}))).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["employee_code"], "RK-1");
    let csv = "name,email,employee_id\nDev Patel,dev@acme.test,DP-9\nRavi Kumar,ravi@client.test,RK-2\n";
    let (s, body) = f.h.call("POST", &format!("{base}/import"), OWNER, Some(json!({"csv": csv, "dry_run": false}))).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!((body["created"].as_i64(), body["updated"].as_i64()), (Some(1), Some(1)));
    let code: String = sqlx::query_scalar("SELECT employee_code FROM people WHERE id = ?1").bind(&f.ravi).fetch_one(&f.h.state.db).await.unwrap();
    assert_eq!(code, "RK-2");
}
