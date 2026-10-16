//! Slice S9.3 and S9.4: a sheet that lacks what the owner requires (a name, an employee ID, an email, a period) is not guessed at. The sender
//! is asked for exactly the missing items, once, in the thread, through Meera's gate; one reminder, then a person. When they reply in the
//! thread the answers are read (patterns first, Hinglish included) and applied under the trust rules: only the person themself, their
//! manager or HR fill EMPTY profile fields, anything else waits for the owner, and bank or ID numbers are never taken by mail. Every change
//! is an `identity_changes` row with undo. Off is today's behaviour, compare only audits.

use std::sync::Arc;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use email_ai_api::{
    hr::team::clock,
    jobs::{self, kind, worker},
    platform::{
        db::now_ms,
        flags::{Key, Mode},
    },
};
use serde_json::{Value, json};

use crate::{common::*, staffing_common::Recorder};

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const RAVI: &str = "ravi@client.test";
const HEMA: &str = "hema@client.test";

fn last_month() -> (String, String) {
    let month = clock::previous_month(&clock::month_of(now_ms(), 0));
    (format!("{} {}", MONTHS[month[5..7].parse::<usize>().unwrap() - 1], &month[..4]), month)
}

fn sheet(name: Option<&str>, code: Option<&str>) -> String {
    let (_, month) = last_month();
    let id = code.map(|c| format!("Employee ID: {c}\n")).unwrap_or_default();
    let who = name.map(|n| format!("Employee Name: {n}\n")).unwrap_or_default();
    format!("{id}{who}Client: Acme Corp\n\n| Date | Project | Hours |\n| --- | --- | --- |\n| {month}-01 | Portal | 8 |\n| {month}-02 | Portal | 7.5 |\n| {month}-03 | Support | 8 |\n| Total | | 23.5 |\n")
}

struct World {
    env: Env,
    hema: String,
    ravi: String,
}

impl World {
    /// A sheet for last month from `from`, read through the whole pipeline.
    async fn receive(&self, gmail_id: &str, from: &str, name: Option<&str>, code: Option<&str>) -> String {
        let (month_name, _) = last_month();
        let id = self.env.receive(&self.env.op_src, gmail_id, &format!("th-{gmail_id}"), from, &format!("{month_name} timesheet"), &sheet(name, code), "timesheet").await;
        self.env.set_understanding(&id, "timesheet", false, json!([])).await;
        worker::drain(&self.env.state).await;
        id
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

    /// Makes the held send due and runs it.
    async fn release_hold(&self, message_id: &str) {
        let (draft, send_at): (String, i64) = sqlx::query_as("SELECT id, send_at FROM drafts WHERE workspace_id = ?1 AND message_id = ?2 AND send_at IS NOT NULL AND status != 'sent'")
            .bind(&self.env.ws)
            .bind(message_id)
            .fetch_one(&self.env.state.db)
            .await
            .unwrap();
        assert!(send_at > now_ms() + 500_000, "held about ten minutes");
        let due = now_ms();
        sqlx::query("UPDATE drafts SET send_at = ?1 WHERE id = ?2").bind(due).bind(&draft).execute(&self.env.state.db).await.unwrap();
        let mut conn = self.env.state.db.acquire().await.unwrap();
        jobs::enqueue_at(&mut conn, kind::SEND_DRAFT, &format!("send_draft:{draft}:{due}"), Some(&self.env.ws), &json!({"workspace_id": self.env.ws, "draft_id": draft, "send_at": due}), 3, due).await.unwrap();
        drop(conn);
        worker::drain(&self.env.state).await;
    }

    /// Ten minutes pass: a mail we sent on the thread no longer counts as "someone just replied".
    async fn age_replies(&self, message_id: &str) {
        sqlx::query("UPDATE email_messages SET received_at = received_at - 3600000 WHERE workspace_id = ?1 AND direction = 'outbound' AND thread_id = (SELECT thread_id FROM email_messages WHERE id = ?2)")
            .bind(&self.env.ws)
            .bind(message_id)
            .execute(&self.env.state.db)
            .await
            .unwrap();
        sqlx::query("UPDATE drafts SET updated_at = updated_at - 3600000 WHERE workspace_id = ?1 AND status = 'sent'").bind(&self.env.ws).execute(&self.env.state.db).await.unwrap();
    }

    fn sent_texts(&self) -> Vec<String> {
        self.env
            .sends()
            .iter()
            .filter_map(|s| s.get("raw").and_then(Value::as_str).map(|raw| String::from_utf8(URL_SAFE_NO_PAD.decode(raw).unwrap()).unwrap()))
            .collect()
    }

    /// (state, asked field keys, round, mails sent) of the open or latest info request of a message.
    async fn ask(&self, message_id: &str) -> Option<(String, Vec<String>, i64, i64)> {
        let row: Option<(String, String, i64, i64)> = sqlx::query_as("SELECT state, fields_json, round, mails_sent FROM info_requests WHERE workspace_id = ?1 AND origin_message_id = ?2 ORDER BY created_at DESC, rowid DESC")
            .bind(&self.env.ws)
            .bind(message_id)
            .fetch_optional(&self.env.state.db)
            .await
            .unwrap();
        row.map(|(state, fields, round, mails)| {
            let keys = serde_json::from_str::<Vec<Value>>(&fields).unwrap().iter().filter(|f| f["status"] == "asked").map(|f| f["key"].as_str().unwrap().to_string()).collect();
            (state, keys, round, mails)
        })
    }
}

/// Ravi has no employee ID on file; the owner requires one.
async fn world(mode: Option<Mode>, required: &[&str]) -> World {
    let mut env = Env::new(Opts::packs(&["general", "staffing"]).config(|c| c.agent_auto_send_delay_secs = 600)).await;
    env.state.alert_mailer = Some(Arc::new(Recorder::default()));
    let hema = env.person("Hema Rao", None, HEMA, None).await;
    let ravi = env.person("Ravi Kumar", None, RAVI, Some(&hema)).await;
    env.ok("PUT", "/agent", ADMIN, Some(json!({"permissions": {"send": "auto"}}))).await;
    env.ok("PUT", "/me/agent-profile", OPERATOR, Some(json!({"display_name": "Priya", "approvals": {"send_email": "routine_ok"}}))).await;
    env.ok("PUT", "/team/settings", ADMIN, Some(json!({"tz_offset_minutes": 0, "below_target_hour": 16, "weekly_check_hour": 12, "timesheet_required": required}))).await;
    if let Some(mode) = mode {
        env.flag(Key::Handoff, mode).await;
    }
    let rohan = env.agent_id(ADMIN, "hrms.timesheets").await;
    sqlx::query("UPDATE agents SET autonomy_level = 3, autonomy_cap = 3 WHERE id = ?1").bind(&rohan).execute(&env.state.db).await.unwrap();
    World { env, hema, ravi }
}

#[tokio::test]
async fn with_the_flag_off_a_sheet_without_an_id_is_recorded_and_nobody_is_asked() {
    let w = world(None, &["employee_code"]).await;
    let id = w.receive("gm-1", RAVI, Some("Ravi Kumar"), None).await;
    assert_eq!(w.ask(&id).await, None);
    assert!(w.env.requests(&id).await.is_empty() && w.env.sends().is_empty());
}

#[tokio::test]
async fn compare_mode_audits_the_ask_it_would_send_and_sends_nothing() {
    let w = world(Some(Mode::Compare), &["employee_code"]).await;
    let id = w.receive("gm-1", RAVI, Some("Ravi Kumar"), None).await;
    let compare = w.audits("ownership.compare").await;
    assert_eq!(compare.len(), 1, "{compare:?}");
    assert_eq!(compare[0]["would_ask"], json!(["employee_code"]), "{}", compare[0]);
    assert_eq!(w.ask(&id).await, None);
    assert!(w.env.requests(&id).await.is_empty() && w.env.live_drafts(&id).await.is_empty() && w.env.sends().is_empty());
}

#[tokio::test]
async fn a_sheet_without_an_id_asks_only_for_the_id_in_the_thread() {
    let w = world(Some(Mode::On), &["employee_code"]).await;
    let id = w.receive("gm-1", RAVI, Some("Ravi Kumar"), None).await;
    assert_eq!(w.env.requests(&id).await, [("rohan".to_string(), "question".to_string(), "composed".to_string())], "one ask, no ack");
    let drafts = w.env.live_drafts(&id).await;
    assert_eq!(drafts.len(), 1);
    assert!(drafts[0].2.contains("1. Your employee ID:") && !drafts[0].2.contains("2.") && !drafts[0].2.to_lowercase().contains("full name"), "exactly what is missing: {}", drafts[0].2);
    assert!(drafts[0].2.contains("reply to this mail"), "{}", drafts[0].2);
    assert_eq!(w.ask(&id).await, Some(("asking".into(), vec!["employee_code".into()], 1, 0)));
    let (status, request): (String, Option<String>) = sqlx::query_as("SELECT status, info_request_id FROM timesheet_records WHERE message_id = ?1").bind(&id).fetch_one(&w.env.state.db).await.unwrap();
    assert_eq!(status, "needs_check", "a sheet that lacks what we need is not recorded as fine");
    assert!(request.is_some(), "the record knows what it waits for");
    w.release_hold(&id).await;
    let texts = w.sent_texts();
    assert_eq!(texts.len(), 1);
    assert!(texts[0].contains("In-Reply-To: <gm-1@mail.example>") && texts[0].contains("To: ravi@client.test"), "{}", texts[0]);
    let (state, rfc_ids, next_at): (String, String, Option<i64>) = sqlx::query_as("SELECT state, ask_rfc_ids_json, next_at FROM info_requests WHERE origin_message_id = ?1").bind(&id).fetch_one(&w.env.state.db).await.unwrap();
    assert_eq!(state, "waiting");
    assert!(rfc_ids.contains("@postroom.mail>"), "the Message-ID the ask went out with is kept: {rfc_ids}");
    assert!(next_at.unwrap() > now_ms() + 20 * 3_600_000, "a reminder is due in about a day");
    assert_eq!(w.env.count("SELECT COUNT(*) FROM review_items WHERE workspace_id = ?1 AND kind = 'send_ack'").await, 0);
}

#[tokio::test]
async fn a_sheet_with_neither_name_nor_id_asks_for_both_in_one_mail() {
    let w = world(Some(Mode::On), &["identity", "period"]).await;
    let id = w.receive("gm-1", "newjoiner@client.test", None, None).await;
    let drafts = w.env.live_drafts(&id).await;
    assert_eq!(drafts.len(), 1, "{:?}", w.env.requests(&id).await);
    assert!(drafts[0].2.contains("1. Your full name:") && drafts[0].2.contains("2. Your employee ID:") && !drafts[0].2.contains("3."), "{}", drafts[0].2);
    assert_eq!(w.ask(&id).await.unwrap().1, ["name", "employee_code"]);
}

#[tokio::test]
async fn what_the_owner_does_not_require_is_never_asked() {
    let w = world(Some(Mode::On), &[]).await;
    let id = w.receive("gm-1", RAVI, Some("Ravi Kumar"), None).await;
    assert_eq!(w.ask(&id).await, None);
    assert_eq!(w.env.requests(&id).await, [("rohan".to_string(), "ack".to_string(), "composed".to_string())], "a clean sheet is just acknowledged");
}

#[tokio::test]
async fn asking_again_changes_nothing() {
    let w = world(Some(Mode::On), &["employee_code"]).await;
    let id = w.receive("gm-1", RAVI, Some("Ravi Kumar"), None).await;
    for _ in 0..2 {
        email_ai_api::hr::staffing::timesheet_records::desk::catch_up(&w.env.state, &w.env.ws, &id).await.unwrap();
        email_ai_api::agents::triggers::start_timesheet_case(&w.env.state, &w.env.ws, &id).await.unwrap();
        worker::drain(&w.env.state).await;
    }
    assert_eq!(w.env.requests(&id).await.len(), 1);
    assert_eq!(w.env.count("SELECT COUNT(*) FROM info_requests WHERE workspace_id = ?1").await, 1);
    assert_eq!(w.env.live_drafts(&id).await.len(), 1);
}

#[tokio::test]
async fn a_stranger_on_a_free_mailbox_is_not_asked_anything() {
    let w = world(Some(Mode::On), &["identity"]).await;
    let id = w.receive("gm-1", "someone@gmail.com", Some("Dinesh Rao"), None).await;
    assert!(w.env.requests(&id).await.is_empty() && w.env.live_drafts(&id).await.is_empty(), "no mail to an address we cannot tell is real");
    assert_eq!(w.ask(&id).await, None);
    assert_eq!(w.audits("handoff.ask_skipped").await.len(), 1);
}

#[tokio::test]
async fn no_answer_means_one_reminder_then_a_person_and_no_more_mail() {
    let w = world(Some(Mode::On), &["employee_code"]).await;
    let id = w.receive("gm-1", RAVI, Some("Ravi Kumar"), None).await;
    w.release_hold(&id).await;
    w.age_replies(&id).await;
    assert_eq!(w.sent_texts().len(), 1, "the ask");

    w.env.run_due_after(2 * 86_400_000).await;
    assert_eq!(w.ask(&id).await.map(|a| (a.0, a.3)), Some(("waiting".into(), 1)), "the reminder is composed, not yet sent");
    w.release_hold(&id).await;
    w.age_replies(&id).await;
    let texts = w.sent_texts();
    assert_eq!(texts.len(), 2);
    assert!(texts[1].contains("quick reminder") && texts[1].contains("1. Your employee ID:") && !texts[1].contains("2."), "{}", texts[1]);

    w.env.run_due_after(3 * 86_400_000).await;
    let (state, reason): (String, Option<String>) = sqlx::query_as("SELECT state, closed_reason FROM info_requests WHERE origin_message_id = ?1").bind(&id).fetch_one(&w.env.state.db).await.unwrap();
    assert_eq!((state.as_str(), reason.as_deref()), ("escalated", Some("no answer")));
    let items: Vec<String> = sqlx::query_scalar("SELECT title FROM review_items WHERE workspace_id = ?1 AND message_id = ?2 AND status = 'open'").bind(&w.env.ws).bind(&id).fetch_all(&w.env.state.db).await.unwrap();
    assert!(items.iter().any(|t| t.contains("did not answer") && t.contains("employee ID")), "a person is asked: {items:?}");
    assert_eq!(w.env.live_drafts(&id).await.len(), 0);
    w.env.run_due_after(10 * 86_400_000).await;
    assert_eq!(w.sent_texts().len(), 2, "nothing more goes to the sender");
    let _ = (&w.hema, &w.ravi);
}

#[tokio::test]
async fn the_required_details_and_the_waiting_times_are_the_owners_settings() {
    let w = world(None, &["identity", "period"]).await;
    let base = json!({"tz_offset_minutes": 0, "below_target_hour": 16, "weekly_check_hour": 12});
    let with = |extra: Value| {
        let mut b = base.clone();
        b.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        b
    };
    let saved = w.env.ok("PUT", "/team/settings", ADMIN, Some(with(json!({"timesheet_required": ["Name", "email", "name"], "missing_reminder_hours": 12, "missing_escalate_hours": 24, "missing_max_reminders": 0})))).await;
    assert_eq!(saved["timesheet_required"], json!(["name", "email"]));
    assert_eq!((saved["missing_reminder_hours"].as_i64(), saved["missing_escalate_hours"].as_i64(), saved["missing_max_reminders"].as_i64()), (Some(12), Some(24), Some(0)));
    for bad in [json!({"timesheet_required": ["salary"]}), json!({"missing_reminder_hours": 0}), json!({"missing_max_reminders": 5})] {
        let (s, v) = w.env.req("PUT", "/team/settings", ADMIN, Some(with(bad.clone()))).await;
        assert_eq!(s, axum::http::StatusCode::BAD_REQUEST, "{bad}: {v}");
    }
}
