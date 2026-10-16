//! Slice S6: Rohan owns the timesheet job behind the `handoff` flag. Off is today's behaviour, compare logs what would
//! happen and acts as today, on records through Rohan's case, stores the decision and tells the manager once.

use std::sync::Arc;

use axum::http::StatusCode;
use email_ai_api::{
    agents::triggers,
    hr::{
        staffing::timesheet_records::{self as records, rules},
        team::clock,
    },
    platform::{
        db::now_ms,
        flags::{Key, Mode},
    },
};
use serde_json::{Value, json};

use crate::{common::*, staffing_common::Recorder};

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/// `(month name, YYYY-MM)` of the month `back` months before the current one.
fn month_back(back: usize) -> (String, String) {
    let mut month = clock::month_of(now_ms(), 0);
    for _ in 0..back {
        month = clock::previous_month(&month);
    }
    let name = MONTHS[month[5..7].parse::<usize>().unwrap() - 1];
    (format!("{name} {}", &month[..4]), month)
}

fn sheet(month: &str) -> String {
    format!(
        "Employee ID: E1024\nEmployee Name: Ravi Kumar\nClient: Acme Corp\n\n| Date | Project | Hours |\n| --- | --- | --- |\n\
         | {month}-01 | Portal | 8 |\n| {month}-02 | Portal | 7.5 |\n| {month}-03 | Support | 8 |\n| Total | | 23.5 |\n"
    )
}

struct Desk {
    env: Env,
    mail: Arc<Recorder>,
    rohan: String,
    manager_user: String,
}

async fn desk(mode: Option<Mode>) -> Desk {
    let mut env = Env::new(Opts::packs(&["general", "staffing"])).await;
    let mail = Arc::new(Recorder::default());
    env.state.alert_mailer = Some(mail.clone());
    let (s, hema) = env.req("POST", "/team/people", ADMIN, Some(json!({"name": "Hema Rao", "emails": ["priya@acme.test"]}))).await;
    assert_eq!(s, StatusCode::CREATED, "{hema}");
    let (s, ravi) = env.req("POST", "/team/people", ADMIN, Some(json!({"name": "Ravi Kumar", "emails": ["ravi@client.test"], "employee_code": "E1024", "manager_id": hema["id"]}))).await;
    assert_eq!(s, StatusCode::CREATED, "{ravi}");
    if let Some(mode) = mode {
        env.flag(Key::Handoff, mode).await;
    }
    let rohan = env.agent_id(ADMIN, "hrms.timesheets").await;
    sqlx::query("UPDATE agents SET autonomy_level = 0 WHERE id = ?1").bind(&rohan).execute(&env.state.db).await.unwrap();
    let manager_user = env.op_id.clone();
    Desk { env, mail, rohan, manager_user }
}

impl Desk {
    async fn receive(&self, gmail_id: &str, month_back_by: usize) -> String {
        let (name, month) = month_back(month_back_by);
        let id = self.env.receive(&self.env.shared_src, gmail_id, &format!("th-{gmail_id}"), "ravi@client.test", &format!("{name} timesheet"), &sheet(&month), "timesheet").await;
        self.env.set_understanding(&id, "timesheet", false, json!([])).await;
        worker_drain(&self.env).await;
        id
    }

    async fn count(&self, sql: &str, message_id: &str) -> i64 {
        sqlx::query_scalar(sql).bind(&self.env.ws).bind(message_id).fetch_one(&self.env.state.db).await.unwrap()
    }

    async fn notices(&self, message_id: &str) -> i64 {
        let _ = message_id;
        self.env.count("SELECT COUNT(*) FROM in_app_messages WHERE workspace_id = ?1 AND kind = 'notice' AND sender_agent_id IS NOT NULL").await
    }

    async fn summaries(&self) -> i64 {
        self.env.count("SELECT COUNT(*) FROM email_outbox WHERE kind = 'timesheet_summary' AND ?1 IS NOT NULL").await
    }

    async fn audits(&self, action: &str) -> Vec<Value> {
        let rows: Vec<String> = sqlx::query_scalar("SELECT metadata_json FROM audit_events WHERE workspace_id = ?1 AND action = ?2 ORDER BY created_at, rowid")
            .bind(&self.env.ws)
            .bind(action)
            .fetch_all(&self.env.state.db)
            .await
            .unwrap();
        rows.iter().map(|m| serde_json::from_str(m).unwrap()).collect()
    }

    async fn record_rows(&self) -> Vec<(String, Option<String>, Option<i64>)> {
        sqlx::query_as("SELECT status, person_id, summarized_at FROM timesheet_records WHERE workspace_id = ?1 ORDER BY created_at, rowid")
            .bind(&self.env.ws)
            .fetch_all(&self.env.state.db)
            .await
            .unwrap()
    }
}

async fn worker_drain(env: &Env) {
    email_ai_api::jobs::worker::drain(&env.state).await;
}

fn nothing_went_out(d: &Desk) {
    assert!(d.mail.take().is_empty(), "no email left");
    assert!(d.env.sends().is_empty(), "nothing was sent to anybody");
}

#[tokio::test]
async fn with_the_flag_off_the_desk_records_and_does_nothing_else() {
    let d = desk(None).await;
    let id = d.receive("gm-1", 1).await;
    let rows = d.record_rows().await;
    assert_eq!(rows.len(), 1, "recorded directly, as today");
    assert_eq!((rows[0].0.as_str(), rows[0].2), ("recorded", None));
    assert!(d.env.cases_by_job(&id).await.is_empty(), "no case for a timesheet when the flag is off");
    assert_eq!(d.notices(&id).await, 0);
    assert_eq!(d.summaries().await, 0);
    assert!(d.audits("ownership.decided").await.is_empty() && d.audits("ownership.compare").await.is_empty());
    assert!(d.env.live_drafts(&id).await.is_empty());
    nothing_went_out(&d);
}

#[tokio::test]
async fn compare_mode_logs_the_decision_and_acts_as_today() {
    let d = desk(Some(Mode::Compare)).await;
    let id = d.receive("gm-1", 1).await;
    let rows = d.record_rows().await;
    assert_eq!((rows.len(), rows[0].0.as_str(), rows[0].2), (1, "recorded", None), "the record is made as today and left as it is");
    assert!(d.env.cases_by_job(&id).await.is_empty());
    assert_eq!(d.notices(&id).await, 0);
    assert_eq!(d.summaries().await, 0);
    assert!(d.audits("ownership.decided").await.is_empty());
    let compare = d.audits("ownership.compare").await;
    assert_eq!(compare.len(), 1, "one row however many times the sheet was read: {compare:?}");
    assert_eq!(compare[0]["decision"]["message"]["kind"], "ack");
    assert_eq!(compare[0]["would_assign"], json!({"job": "desk:timesheet", "agent": "Rohan"}));
    assert_eq!(compare[0]["would_tell"], 2, "the manager and the final contact would be told");
    assert!(d.env.live_drafts(&id).await.is_empty());
    nothing_went_out(&d);
}

#[tokio::test]
async fn rohan_owns_a_timesheet_at_level_0_and_records_it() {
    let d = desk(Some(Mode::On)).await;
    let id = d.receive("gm-1", 1).await;
    assert_eq!(d.record_rows().await.len(), 1);
    assert_eq!(d.env.cases_by_job(&id).await, [("desk:timesheet".to_string(), "rohan".to_string()), ("reply".to_string(), "meera".to_string())], "Rohan owns the sheet; the reply is Meera's job");
    let expert_drafts: i64 = d.env.count("SELECT COUNT(*) FROM drafts d JOIN agents a ON a.id = d.agent_id WHERE d.workspace_id = ?1 AND a.template_key = 'hrms.timesheets'").await;
    assert_eq!(expert_drafts, 0, "Rohan writes no draft");
    assert_eq!(d.env.live_drafts(&id).await.len(), 1, "Meera's one reply waits for an approval, since Rohan is at level 0");
    let decided = d.audits("ownership.decided").await;
    assert_eq!(decided.len(), 1);
    assert_eq!(decided[0]["jobs"], json!([{"job": "desk:timesheet", "agent_id": d.rohan}]));
    assert_eq!(d.count("SELECT COUNT(*) FROM cases WHERE workspace_id = ?1 AND message_id = ?2 AND shadow = 1", &id).await, 0);
}

#[tokio::test]
async fn one_case_per_message_and_job_even_when_triggered_twice() {
    let d = desk(Some(Mode::On)).await;
    let id = d.receive("gm-1", 1).await;
    let again = triggers::start_timesheet_case(&d.env.state, &d.env.ws, &id).await.unwrap();
    assert!(matches!(again, triggers::Started::Case(_, false)), "{again:?}");
    assert_eq!(d.env.cases_by_job(&id).await.iter().filter(|(job, _)| job == "desk:timesheet").count(), 1);
    let dup = sqlx::query(
        "INSERT INTO cases (id, workspace_id, kind, title, goal, status, state_json, priority, max_steps, budget_usd, message_id, created_at, updated_at, job)
         VALUES ('dup', ?1, 'inbox', 't', 'g', 'running', '{}', 'normal', 5, 0.1, ?2, 1, 1, 'desk:timesheet')",
    )
    .bind(&d.env.ws)
    .bind(&id)
    .execute(&d.env.state.db)
    .await;
    assert!(dup.is_err(), "the unique index holds (workspace, kind, message, job)");
}

#[tokio::test]
async fn a_clean_sheet_is_summarised_to_the_manager_once_and_the_decision_is_stored() {
    let d = desk(Some(Mode::On)).await;
    let id = d.receive("gm-1", 1).await;
    assert_eq!(d.notices(&id).await, 2, "one in-app notice each for the manager and the final contact");
    let mails = d.mail.take();
    assert_eq!(mails.len(), 2, "{mails:?}");
    let mut to: Vec<&str> = mails.iter().map(|m| m.0.as_str()).collect();
    to.sort();
    assert_eq!(to, ["admin@acme.test", "priya@acme.test"]);
    assert!(mails[0].1.contains("Ravi Kumar") && mails[0].2.contains("23.5 h"), "{:?}", mails[0]);
    let kind: i64 = d.env.count("SELECT COUNT(*) FROM email_outbox WHERE kind = 'timesheet_summary' AND ?1 IS NOT NULL").await;
    assert_eq!(kind, 2);
    let manager_notice: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM in_app_messages WHERE recipient_user_id = ?1 AND kind = 'notice'").bind(&d.manager_user).fetch_one(&d.env.state.db).await.unwrap();
    assert_eq!(manager_notice, 1);
    assert!(d.record_rows().await[0].2.is_some(), "claimed");

    let case_id = &d.env.cases_of(&id).await[0].0;
    let case = d.env.case(ADMIN, case_id).await;
    assert_eq!(case["state"]["decision"]["message"]["kind"], "ack", "{case}");
    assert_eq!(case["state"]["decision"]["message"]["facts"][2], json!({"label": "Hours", "value": "23.5 h"}));
    assert_eq!(d.env.count("SELECT COUNT(*) FROM drafts WHERE workspace_id = ?1").await, 1, "Meera's one acknowledgement, waiting for approval at level 0");
    assert!(d.env.sends().is_empty(), "nothing goes to the sender without a person at level 0");

    // Recording again, and starting the case again, tell nobody twice.
    records::record_message(&d.env.state, &d.env.ws, &id).await.unwrap();
    triggers::start_timesheet_case(&d.env.state, &d.env.ws, &id).await.unwrap();
    sqlx::query("INSERT OR IGNORE INTO jobs (id, kind, dedupe_key, workspace_id, payload_json, status, run_at, created_at, updated_at, attempts, max_attempts) SELECT 'again', 'record_timesheet', 'again', ?1, ?2, 'pending', 1, 1, 1, 0, 3")
        .bind(&d.env.ws)
        .bind(json!({"message_id": id}).to_string())
        .execute(&d.env.state.db)
        .await
        .ok();
    worker_drain(&d.env).await;
    assert_eq!(d.notices(&id).await, 2);
    assert!(d.mail.take().is_empty());
}

#[tokio::test]
async fn a_wrong_month_sheet_is_needs_check_with_no_manager_notice() {
    let d = desk(Some(Mode::On)).await;
    let id = d.receive("gm-1", 2).await;
    let rows = d.record_rows().await;
    assert_eq!((rows.len(), rows[0].0.as_str(), rows[0].2), (1, "needs_check", None));
    let issues: String = sqlx::query_scalar("SELECT issues_json FROM timesheet_records WHERE workspace_id = ?1").bind(&d.env.ws).fetch_one(&d.env.state.db).await.unwrap();
    assert!(issues.contains("wrong_period"), "{issues}");
    assert_eq!(d.notices(&id).await, 0);
    nothing_went_out(&d);
    let case = d.env.case(ADMIN, &d.env.cases_of(&id).await[0].0).await;
    assert_eq!(case["state"]["decision"]["message"]["kind"], "correction", "{case}");
    assert!(case["state"]["decision"]["message"]["issues"][0]["text"].as_str().unwrap().contains(&month_back(2).0));
    assert_eq!(d.env.live_drafts(&id).await.len(), 1, "Meera writes the correction; at level 0 a person approves it");
    let tasks = d.count("SELECT COUNT(*) FROM review_items WHERE workspace_id = ?1 AND message_id = ?2 AND kind = 'agent_task'", &id).await;
    assert_eq!(tasks, 1, "a person is asked to look, once");
}

#[tokio::test]
async fn old_mail_is_recorded_and_tells_nobody() {
    let d = desk(Some(Mode::On)).await;
    let (name, month) = month_back(1);
    let email = email_ai_api::platform::domain::NormalizedEmail {
        source_message_id: "old-1".into(),
        sender_address: "ravi@client.test".into(),
        recipients: vec!["team@acme.com".into()],
        subject: format!("{name} timesheet"),
        body_text: sheet(&month),
        received_at: now_ms() - 5 * 86_400_000,
        ..Default::default()
    };
    let mut tx = d.env.state.db.begin().await.unwrap();
    let id = email_ai_api::mail::ingest::persist_message(&mut tx, &d.env.shared_src, &d.env.ws, &email, Some("old-thread"), json!({})).await.unwrap().unwrap();
    tx.commit().await.unwrap();
    worker_drain(&d.env).await;
    assert_eq!(d.record_rows().await.len(), 1, "the backfill still records it");
    assert!(d.env.cases_by_job(&id).await.is_empty());
    assert_eq!(d.notices(&id).await, 0);
    nothing_went_out(&d);
}

#[tokio::test]
async fn the_manager_approves_and_an_outsider_cannot() {
    let d = desk(Some(Mode::On)).await;
    d.receive("gm-1", 1).await;
    let record: String = sqlx::query_scalar("SELECT id FROM timesheet_records WHERE workspace_id = ?1").bind(&d.env.ws).fetch_one(&d.env.state.db).await.unwrap();
    let path = format!("/staffing/timesheet-records/{record}/approve");
    let (s, _) = d.env.req("POST", &path, OUTSIDER, None).await;
    assert!(matches!(s, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND), "not a member: {s}");
    let stranger = "dev:stranger@acme.test";
    d.env.req("GET", "/me", stranger, None).await;
    let (s, _) = d.env.req("POST", "/members", ADMIN, Some(json!({"email": "stranger@acme.test", "role": "operator"}))).await;
    assert!(s.is_success(), "{s}");
    let (s, _) = d.env.req("POST", &path, stranger, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "a member who is not this person's manager");
    let approved: Option<String> = sqlx::query_scalar("SELECT approved_by FROM timesheet_records WHERE id = ?1").bind(&record).fetch_one(&d.env.state.db).await.unwrap();
    assert!(approved.is_none());
    let (s, body) = d.env.req("POST", &path, OPERATOR, None).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let (by, at): (Option<String>, Option<i64>) = sqlx::query_as("SELECT approved_by, approved_at FROM timesheet_records WHERE id = ?1").bind(&record).fetch_one(&d.env.state.db).await.unwrap();
    assert_eq!(by.as_deref(), Some(d.manager_user.as_str()));
    assert!(at.is_some());
    assert_eq!(d.audits("timesheet.approved").await.len(), 1);
}

#[tokio::test]
async fn the_new_rule_settings_round_trip_and_unknown_fields_are_still_rejected() {
    let env = Env::new(Opts::packs(&["general", "staffing"])).await;
    let current = env.ok("GET", "/team/settings", ADMIN, None).await;
    assert_eq!((current["timesheet_cadence"].as_str(), current["max_hours_per_day"].is_null(), current["require_signature"].as_bool()), (Some("monthly"), true, Some(false)));
    let mut body = current.clone();
    body.as_object_mut().unwrap().remove("sources");
    body["timesheet_cadence"] = json!("weekly");
    body["max_hours_per_day"] = json!(10.5);
    body["require_signature"] = json!(true);
    let saved = env.ok("PUT", "/team/settings", ADMIN, Some(body.clone())).await;
    assert_eq!((saved["timesheet_cadence"].as_str(), saved["max_hours_per_day"].as_f64(), saved["require_signature"].as_bool()), (Some("weekly"), Some(10.5), Some(true)));
    body["max_hours_per_day"] = Value::Null;
    let cleared = env.ok("PUT", "/team/settings", ADMIN, Some(body.clone())).await;
    assert!(cleared["max_hours_per_day"].is_null() && cleared["timesheet_cadence"] == "weekly");
    body.as_object_mut().unwrap().remove("max_hours_per_day");
    body["timesheet_cadence"] = json!("daily");
    let kept = env.ok("PUT", "/team/settings", ADMIN, Some(body.clone())).await;
    assert!(kept["max_hours_per_day"].is_null(), "an absent field is left alone");
    for (field, value) in [("timesheet_cadence", json!("hourly")), ("max_hours_per_day", json!(25)), ("max_hours_per_day", json!(0))] {
        let mut bad = body.clone();
        bad[field] = value;
        let (s, _) = env.req("PUT", "/team/settings", ADMIN, Some(bad)).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{field}");
    }
    let mut unknown = body.clone();
    unknown["sources"] = json!([]);
    let (s, _) = env.req("PUT", "/team/settings", ADMIN, Some(unknown)).await;
    assert_ne!(s, StatusCode::OK, "the read-only `sources` is still refused");
    let (s, _) = env.req("PUT", "/team/settings", OPERATOR, Some(body)).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "only the super admin edits the rules");
}

#[tokio::test]
async fn a_sheet_whose_files_are_still_being_read_waits_and_wakes_when_they_are_read() {
    use email_ai_api::platform::domain::{AttachmentMeta, NormalizedEmail};
    let d = desk(Some(Mode::On)).await;
    let (name, month) = month_back(1);
    let email = NormalizedEmail {
        source_message_id: "files-1".into(),
        sender_address: "ravi@client.test".into(),
        recipients: vec!["team@acme.com".into()],
        subject: format!("{name} timesheet"),
        body_text: "see attached".into(),
        received_at: now_ms() - 3_600_000,
        attachments: vec![AttachmentMeta { filename: "timesheet.xlsx".into(), media_type: Some("application/vnd.ms-excel".into()), size_bytes: Some(1000), provider_ref: None }],
        ..Default::default()
    };
    let mut tx = d.env.state.db.begin().await.unwrap();
    let id = email_ai_api::mail::ingest::persist_message(&mut tx, &d.env.shared_src, &d.env.ws, &email, Some("files-thread"), json!({})).await.unwrap().unwrap();
    tx.commit().await.unwrap();
    sqlx::query("DELETE FROM jobs WHERE kind != 'agent_step'").execute(&d.env.state.db).await.unwrap();
    sqlx::query(
        "INSERT OR REPLACE INTO classifications (message_id, category, priority, requires_reply, confidence, needs_review, source, taxonomy_version, updated_at)
         VALUES (?1, 'timesheet', 'normal', 0, 1.0, 0, 'manual', 'taxonomy-v1', 1)",
    )
    .bind(&id)
    .execute(&d.env.state.db)
    .await
    .unwrap();

    let started = triggers::start_timesheet_case(&d.env.state, &d.env.ws, &id).await.unwrap();
    let triggers::Started::Case(case_id, true) = started else { panic!("{started:?}") };
    worker_drain(&d.env).await;
    let case = d.env.case(ADMIN, &case_id).await;
    assert_eq!(case["status"], "waiting_event", "{case}");
    assert_eq!(case["state"]["record_waits"], 1);
    assert!(d.record_rows().await.is_empty());
    assert_eq!(d.notices(&id).await, 0);

    sqlx::query("UPDATE attachments SET text_content = ?2, text_status = 'extracted', extract_status = 'done', extract_kind = 'sheet' WHERE message_id = ?1")
        .bind(&id)
        .bind(sheet(&month))
        .execute(&d.env.state.db)
        .await
        .unwrap();
    let mut conn = d.env.state.db.acquire().await.unwrap();
    triggers::wake_for_message(&mut conn, &d.env.ws, &id).await.unwrap();
    drop(conn);
    worker_drain(&d.env).await;
    let rows = d.record_rows().await;
    assert_eq!((rows.len(), rows[0].0.as_str()), (1, "recorded"));
    assert_eq!(d.notices(&id).await, 2);
    assert_eq!(d.env.case(ADMIN, &case_id).await["status"], "done");
}

#[tokio::test]
async fn a_sheet_that_differs_from_the_portal_asks_the_manager_and_stores_a_question_for_the_sender() {
    let d = desk(Some(Mode::On)).await;
    let (_, month) = month_back(1);
    let ravi: String = sqlx::query_scalar("SELECT id FROM people WHERE workspace_id = ?1 AND name = 'Ravi Kumar'").bind(&d.env.ws).fetch_one(&d.env.state.db).await.unwrap();
    sqlx::query("INSERT INTO portal_hours (workspace_id, person_id, month, hours, imported_at) VALUES (?1, ?2, ?3, 168, 1)").bind(&d.env.ws).bind(&ravi).bind(&month).execute(&d.env.state.db).await.unwrap();
    let id = d.receive("gm-1", 1).await;
    assert_eq!(d.notices(&id).await, 2, "the manager decides, so the manager is told");
    let mails = d.mail.take();
    assert!(mails[0].2.contains("23.5 h on the sheet, 168 h on the portal"), "{:?}", mails[0]);
    let case = d.env.case(ADMIN, &d.env.cases_of(&id).await[0].0).await;
    assert_eq!(case["state"]["decision"]["message"]["kind"], "question", "{case}");
    assert_eq!(d.record_rows().await[0].0, "needs_check");
    assert_eq!(d.count("SELECT COUNT(*) FROM review_items WHERE workspace_id = ?1 AND message_id = ?2 AND kind = 'agent_task'", &id).await, 0, "the summary is the ask, no second task");
    assert!(d.env.sends().is_empty());
}

fn failing_mailer() -> Arc<dyn email_ai_api::platform::mailer::Mailer> {
    struct Down;
    #[async_trait::async_trait]
    impl email_ai_api::platform::mailer::Mailer for Down {
        async fn send(&self, _: &str, _: &str, _: &str, _: &str) -> anyhow::Result<()> {
            Err(anyhow::anyhow!("smtp is down"))
        }
    }
    Arc::new(Down)
}

#[tokio::test]
async fn a_person_can_still_start_a_reply_case_beside_the_desk_case() {
    let d = desk(Some(Mode::On)).await;
    let id = d.receive("gm-1", 1).await;
    assert_eq!(d.env.cases_by_job(&id).await.len(), 2, "the desk case and Meera's reply case");
    let manual = triggers::start_inbox_case(&d.env.state, &d.env.ws, &id, Some(&d.env.admin_id)).await.unwrap();
    assert!(matches!(manual, triggers::Started::Case(_, true)), "{manual:?}");
    let cases = d.env.cases_by_job(&id).await;
    assert_eq!(cases.len(), 3, "the desk case, Meera's reply case and the person's own case: {cases:?}");
    assert!(cases.contains(&("desk:timesheet".to_string(), "rohan".to_string())) && cases.contains(&("main".to_string(), "meera".to_string())), "{cases:?}");
}

#[tokio::test]
async fn a_sheet_that_needs_a_check_is_never_called_recorded_to_the_manager() {
    let d = desk(Some(Mode::On)).await;
    let id = d.receive("gm-1", 2).await;
    let review = records::desk::review(&d.env.state.db, &d.env.ws, &id).await.unwrap();
    assert_eq!(d.record_rows().await[0].0, "needs_check");
    let ack = rules::decision(&[rules::Checked { record_id: "r".into(), person: Some("Ravi Kumar".into()), period: ("2026-09-01".into(), "2026-09-30".into()), total: Some(23.5), issues: vec![] }]);
    let told = records::summary::summarize(&d.env.state, &d.env.ws, None, &review.rows, &ack).await.unwrap();
    assert_eq!((told, d.notices(&id).await), (0, 0), "an Ack summary needs every sheet recorded");
    nothing_went_out(&d);
}

#[tokio::test]
async fn a_summary_is_only_claimed_when_someone_can_be_told_and_never_goes_to_the_sender() {
    let mut d = desk(Some(Mode::On)).await;
    d.env.state.alert_mailer = None;
    // Everyone to tell is outside the workspace and there is no mailer: nothing is claimed, nobody is "notified".
    sqlx::query("UPDATE person_emails SET email = 'hema@nowhere.test' WHERE email = 'priya@acme.test'").execute(&d.env.state.db).await.unwrap();
    d.env.ok("PUT", "/team/settings", ADMIN, Some({
        let mut s = d.env.ok("GET", "/team/settings", ADMIN, None).await;
        s.as_object_mut().unwrap().remove("sources");
        s["final_emails"] = json!(["boss@nowhere.test"]);
        s
    })).await;
    let id = d.receive("gm-1", 1).await;
    let case = d.env.case(ADMIN, &d.env.cases_of(&id).await[0].0).await;
    assert_eq!(case["state"]["manager_notified"], false, "{case}");
    assert_eq!(d.record_rows().await[0].2, None, "not claimed, so a later run can still tell them");
    // A mailer that is down: the claim is given back.
    d.env.state.alert_mailer = Some(failing_mailer());
    let review = records::desk::review(&d.env.state.db, &d.env.ws, &id).await.unwrap();
    let told = records::summary::summarize(&d.env.state, &d.env.ws, None, &review.rows, &review.decision).await.unwrap();
    assert_eq!(told, 0);
    assert_eq!(d.record_rows().await[0].2, None, "every delivery failed, so the claim is released");
}

#[tokio::test]
async fn the_person_who_sent_the_sheet_is_never_a_summary_recipient() {
    let d = desk(Some(Mode::On)).await;
    d.env.ok("PUT", "/team/settings", ADMIN, Some({
        let mut s = d.env.ok("GET", "/team/settings", ADMIN, None).await;
        s.as_object_mut().unwrap().remove("sources");
        s["final_emails"] = json!(["ravi@client.test", "admin@acme.test"]);
        s
    })).await;
    let id = d.receive("gm-1", 1).await;
    let mails = d.mail.take();
    let mut to: Vec<&str> = mails.iter().map(|m| m.0.as_str()).collect();
    to.sort();
    assert_eq!(to, ["admin@acme.test", "priya@acme.test"], "Ravi sent the sheet and is not told about it");
    assert_eq!(d.notices(&id).await, 2);
}

#[tokio::test]
async fn compare_mode_does_not_write_a_route() {
    let d = desk(Some(Mode::Compare)).await;
    let id = d.receive("gm-1", 1).await;
    sqlx::query("DELETE FROM message_routes WHERE message_id = ?1").bind(&id).execute(&d.env.state.db).await.unwrap();
    sqlx::query("DELETE FROM audit_events WHERE action = 'ownership.compare'").execute(&d.env.state.db).await.unwrap();
    records::desk::compare(&d.env.state, &d.env.ws, &id).await.unwrap();
    assert_eq!(d.count("SELECT COUNT(*) FROM message_routes WHERE workspace_id = ?1 AND message_id = ?2", &id).await, 0, "compare only reads");
    assert_eq!(d.audits("ownership.compare").await.len(), 1);
}

#[tokio::test]
async fn the_signature_gap_is_counted_once_per_email() {
    let d = desk(Some(Mode::On)).await;
    d.env.ok("PUT", "/team/settings", ADMIN, Some({
        let mut s = d.env.ok("GET", "/team/settings", ADMIN, None).await;
        s.as_object_mut().unwrap().remove("sources");
        s["require_signature"] = json!(true);
        s
    })).await;
    let id = d.receive("gm-1", 1).await;
    let mut conn = d.env.state.db.acquire().await.unwrap();
    for _ in 0..3 {
        records::desk::record_signature_gap(&mut conn, &d.env.ws, None, &id).await.unwrap();
    }
    drop(conn);
    let count: i64 = d.env.count("SELECT count FROM tool_gaps WHERE workspace_id = ?1 AND tool = 'read_signature'").await;
    assert_eq!(count, 1);
}

#[tokio::test]
async fn nobody_approves_their_own_sheet_and_a_replaced_sheet_cannot_be_approved() {
    let d = desk(Some(Mode::On)).await;
    d.receive("gm-1", 1).await;
    let record: String = sqlx::query_scalar("SELECT id FROM timesheet_records WHERE workspace_id = ?1").bind(&d.env.ws).fetch_one(&d.env.state.db).await.unwrap();
    let (s, p) = d.env.req("POST", "/team/people", ADMIN, Some(json!({"name": "Ada Admin", "emails": ["admin@acme.test"]}))).await;
    assert_eq!(s, StatusCode::CREATED, "{p}");
    sqlx::query("UPDATE timesheet_records SET person_id = ?1 WHERE id = ?2").bind(p["id"].as_str().unwrap()).bind(&record).execute(&d.env.state.db).await.unwrap();
    let path = format!("/staffing/timesheet-records/{record}/approve");
    let (s, _) = d.env.req("POST", &path, ADMIN, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "the super admin cannot approve their own sheet either");
    sqlx::query("UPDATE timesheet_records SET status = 'replaced', person_id = NULL WHERE id = ?1").bind(&record).execute(&d.env.state.db).await.unwrap();
    let (s, _) = d.env.req("POST", &path, ADMIN, None).await;
    assert_eq!(s, StatusCode::CONFLICT);
}

#[tokio::test]
async fn a_cancelled_desk_case_leaves_the_sheet_to_the_old_review_path() {
    let d = desk(Some(Mode::On)).await;
    let id = d.receive("gm-1", 2).await;
    sqlx::query("UPDATE cases SET status = 'cancelled' WHERE workspace_id = ?1 AND message_id = ?2").bind(&d.env.ws).bind(&id).execute(&d.env.state.db).await.unwrap();
    sqlx::query("DELETE FROM review_items WHERE message_id = ?1").bind(&id).execute(&d.env.state.db).await.unwrap();
    let mut conn = d.env.state.db.acquire().await.unwrap();
    email_ai_api::jobs::enqueue(&mut conn, email_ai_api::jobs::kind::EXECUTE_WORKFLOW, "wf-again", Some(&d.env.ws), &json!({"message_id": id})).await.unwrap();
    drop(conn);
    worker_drain(&d.env).await;
    let items: i64 = d.count("SELECT COUNT(*) FROM review_items WHERE workspace_id = ?1 AND message_id = ?2", &id).await;
    assert_eq!(items, 1, "a sheet that needs a look is not dropped because its desk case was cancelled");
}

#[tokio::test]
async fn a_sheet_recorded_after_the_desk_case_gave_up_waiting_is_still_checked_and_the_manager_told() {
    use email_ai_api::platform::domain::{AttachmentMeta, NormalizedEmail};
    let d = desk(Some(Mode::On)).await;
    let (name, month) = month_back(1);
    let email = NormalizedEmail {
        source_message_id: "late-files".into(),
        sender_address: "ravi@client.test".into(),
        recipients: vec!["team@acme.com".into()],
        subject: format!("{name} timesheet"),
        body_text: "see attached".into(),
        received_at: now_ms() - 3_600_000,
        attachments: vec![AttachmentMeta { filename: "timesheet.xlsx".into(), media_type: Some("application/vnd.ms-excel".into()), size_bytes: Some(1000), provider_ref: None }],
        ..Default::default()
    };
    let mut tx = d.env.state.db.begin().await.unwrap();
    let id = email_ai_api::mail::ingest::persist_message(&mut tx, &d.env.shared_src, &d.env.ws, &email, Some("late-thread"), json!({})).await.unwrap().unwrap();
    tx.commit().await.unwrap();
    sqlx::query("DELETE FROM jobs WHERE kind != 'agent_step'").execute(&d.env.state.db).await.unwrap();
    sqlx::query(
        "INSERT OR REPLACE INTO classifications (message_id, category, priority, requires_reply, confidence, needs_review, source, taxonomy_version, updated_at)
         VALUES (?1, 'timesheet', 'normal', 0, 1.0, 0, 'manual', 'taxonomy-v1', 1)",
    )
    .bind(&id)
    .execute(&d.env.state.db)
    .await
    .unwrap();
    let triggers::Started::Case(case_id, true) = triggers::start_timesheet_case(&d.env.state, &d.env.ws, &id).await.unwrap() else { panic!() };
    sqlx::query("UPDATE cases SET state_json = json_set(state_json, '$.record_waits', 8) WHERE id = ?1").bind(&case_id).execute(&d.env.state.db).await.unwrap();
    worker_drain(&d.env).await;
    assert_eq!(d.env.case(ADMIN, &case_id).await["status"], "done", "the case stopped waiting");
    assert!(d.record_rows().await.is_empty());

    sqlx::query("UPDATE attachments SET text_content = ?2, text_status = 'extracted', extract_status = 'done', extract_kind = 'sheet' WHERE message_id = ?1")
        .bind(&id)
        .bind(sheet(&month))
        .execute(&d.env.state.db)
        .await
        .unwrap();
    let mut conn = d.env.state.db.acquire().await.unwrap();
    email_ai_api::jobs::enqueue(&mut conn, email_ai_api::jobs::kind::RECORD_TIMESHEET, "late-record", Some(&d.env.ws), &json!({"message_id": id})).await.unwrap();
    drop(conn);
    worker_drain(&d.env).await;
    assert_eq!(d.record_rows().await.len(), 1);
    assert_eq!(d.notices(&id).await, 2, "the late sheet is summarised once");
    let mut conn = d.env.state.db.acquire().await.unwrap();
    email_ai_api::jobs::enqueue(&mut conn, email_ai_api::jobs::kind::RECORD_TIMESHEET, "late-record-2", Some(&d.env.ws), &json!({"message_id": id})).await.unwrap();
    drop(conn);
    worker_drain(&d.env).await;
    assert_eq!(d.notices(&id).await, 2, "and not again");
}

/// Drops what the desk case did for a message so `catch_up` and the old workflow path start from the same place.
async fn forget_the_desk_case(d: &Desk, id: &str) {
    sqlx::query("UPDATE cases SET status = 'cancelled', state_json = json_remove(state_json, '$.record_ids') WHERE workspace_id = ?1 AND message_id = ?2 AND job = 'desk:timesheet'")
        .bind(&d.env.ws)
        .bind(id)
        .execute(&d.env.state.db)
        .await
        .unwrap();
    sqlx::query("DELETE FROM review_items WHERE message_id = ?1").bind(id).execute(&d.env.state.db).await.unwrap();
}

async fn run_the_old_workflow(d: &Desk, id: &str, key: &str) {
    let mut conn = d.env.state.db.acquire().await.unwrap();
    email_ai_api::jobs::enqueue(&mut conn, email_ai_api::jobs::kind::EXECUTE_WORKFLOW, key, Some(&d.env.ws), &json!({"message_id": id})).await.unwrap();
    drop(conn);
    worker_drain(&d.env).await;
}

#[tokio::test]
async fn catch_up_and_the_old_workflow_never_both_open_a_review_item() {
    // The workflow path first, then catch_up.
    let d = desk(Some(Mode::On)).await;
    let id = d.receive("gm-1", 2).await;
    forget_the_desk_case(&d, &id).await;
    run_the_old_workflow(&d, &id, "wf-first").await;
    records::desk::catch_up(&d.env.state, &d.env.ws, &id).await.unwrap();
    assert_eq!(d.count("SELECT COUNT(*) FROM review_items WHERE workspace_id = ?1 AND message_id = ?2", &id).await, 1, "one review item, workflow first");

    // And catch_up first, then the workflow path.
    let d = desk(Some(Mode::On)).await;
    let id = d.receive("gm-1", 2).await;
    forget_the_desk_case(&d, &id).await;
    records::desk::catch_up(&d.env.state, &d.env.ws, &id).await.unwrap();
    run_the_old_workflow(&d, &id, "wf-second").await;
    assert_eq!(d.count("SELECT COUNT(*) FROM review_items WHERE workspace_id = ?1 AND message_id = ?2", &id).await, 1, "one review item, catch_up first");
}

#[tokio::test]
async fn catch_up_leaves_a_portal_question_to_the_summary_the_manager_got() {
    let d = desk(Some(Mode::On)).await;
    let (_, month) = month_back(1);
    let ravi: String = sqlx::query_scalar("SELECT id FROM people WHERE workspace_id = ?1 AND name = 'Ravi Kumar'").bind(&d.env.ws).fetch_one(&d.env.state.db).await.unwrap();
    sqlx::query("INSERT INTO portal_hours (workspace_id, person_id, month, hours, imported_at) VALUES (?1, ?2, ?3, 168, 1)").bind(&d.env.ws).bind(&ravi).bind(&month).execute(&d.env.state.db).await.unwrap();
    let id = d.receive("gm-1", 1).await;
    forget_the_desk_case(&d, &id).await;
    sqlx::query("UPDATE timesheet_records SET summarized_at = NULL WHERE workspace_id = ?1").bind(&d.env.ws).execute(&d.env.state.db).await.unwrap();
    let before = d.notices(&id).await;
    records::desk::catch_up(&d.env.state, &d.env.ws, &id).await.unwrap();
    assert_eq!(d.notices(&id).await, before + 2, "the manager is told again, as it was never claimed");
    assert_eq!(d.count("SELECT COUNT(*) FROM review_items WHERE workspace_id = ?1 AND message_id = ?2", &id).await, 0, "the summary is the ask, so no extra review item");
}
