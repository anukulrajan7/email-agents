//! Slice S9.2: nobody who mails a timesheet waits in silence. Every inbound mail a desk owns gets a reply duty with a deadline of
//! 4 working hours; if no reply has been composed by then Meera sends one short holding note through the same gate, and a person is
//! told one working day later if only that note went. Mail nobody should answer (machines, bulk, a closing "thanks", mail a person
//! already answered, held-back billable mail) is excused, with the reason on the row. Off is today's behaviour, compare only audits.

use std::sync::Arc;

use email_ai_api::{
    agents::triggers,
    hr::team::clock,
    jobs::{self, kind, worker},
    mail::ingest,
    platform::{
        db::now_ms,
        domain::NormalizedEmail,
        flags::{Key, Mode},
    },
};
use serde_json::{Value, json};

use crate::{common::*, staffing_common::Recorder};

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const RAVI: &str = "ravi@client.test";
const THREE_DAYS: i64 = 3 * 86_400_000;

fn last_month() -> (String, String) {
    let month = clock::previous_month(&clock::month_of(now_ms(), 0));
    (format!("{} {}", MONTHS[month[5..7].parse::<usize>().unwrap() - 1], &month[..4]), month)
}

fn sheet(month: &str) -> String {
    format!(
        "Employee ID: E1024\nEmployee Name: Ravi Kumar\nClient: Acme Corp\n\n| Date | Project | Hours |\n| --- | --- | --- |\n\
         | {month}-01 | Portal | 8 |\n| {month}-02 | Portal | 7.5 |\n| {month}-03 | Support | 8 |\n| Total | | 23.5 | \n"
    )
}

#[derive(Clone, Copy, PartialEq)]
enum Mbox {
    /// The operator's own mailbox: her own Meera writes, and she has agreed to routine replies.
    Personal,
    /// The shared team mailbox nobody owns: the lead Meera writes, and every reply asks.
    Shared,
}

struct World {
    env: Env,
    src: Mbox,
}

impl World {
    fn source(&self) -> String {
        if self.src == Mbox::Personal { self.env.op_src.clone() } else { self.env.shared_src.clone() }
    }

    fn token(&self) -> &'static str {
        if self.src == Mbox::Personal { OPERATOR } else { ADMIN }
    }

    /// A sheet for last month, read through the whole pipeline.
    async fn receive(&self, gmail_id: &str, thread: &str, from: &str) -> String {
        let (name, month) = last_month();
        let id = self.env.receive(&self.source(), gmail_id, thread, from, &format!("{name} timesheet"), &sheet(&month), "timesheet").await;
        self.env.set_understanding(&id, "timesheet", false, json!([])).await;
        worker::drain(&self.env.state).await;
        id
    }

    /// A sheet whose desk case never runs (its job was lost): the duty is open and nothing is composed.
    async fn stuck(&self, gmail_id: &str, thread: &str, from: &str) -> String {
        let id = self.receive(gmail_id, thread, from).await;
        self.env.flag(Key::Handoff, Mode::On).await;
        triggers::start_timesheet_case(&self.env.state, &self.env.ws, &id).await.unwrap();
        sqlx::query("DELETE FROM jobs WHERE kind = 'agent_step'").execute(&self.env.state.db).await.unwrap();
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

    async fn pending(&self) -> Vec<Value> {
        self.env.ok("GET", "/approvals?status=pending", self.token(), None).await["items"].as_array().unwrap().clone()
    }

    async fn approve_all(&self) {
        for item in self.pending().await {
            let (s, v) = self.env.req("POST", &format!("/approvals/{}", item["id"].as_str().unwrap()), self.token(), Some(json!({"decision": "approve"}))).await;
            assert_eq!(s, axum::http::StatusCode::OK, "{v}");
        }
        worker::drain(&self.env.state).await;
    }

    async fn alerts(&self, prefix: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM alerts WHERE workspace_id = ?1 AND dedupe_key LIKE ?2 || '%'").bind(&self.env.ws).bind(prefix).fetch_one(&self.env.state.db).await.unwrap()
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
}

async fn world(src: Mbox, mode: Option<Mode>, rohan_level: i64) -> World {
    let mut env = Env::new(Opts::packs(&["general", "staffing"]).config(|c| c.agent_auto_send_delay_secs = 600)).await;
    env.state.alert_mailer = Some(Arc::new(Recorder::default()));
    let (s, hema) = env.req("POST", "/team/people", ADMIN, Some(json!({"name": "Hema Rao", "emails": ["priya@acme.test"]}))).await;
    assert_eq!(s, axum::http::StatusCode::CREATED, "{hema}");
    let (s, ravi) = env.req("POST", "/team/people", ADMIN, Some(json!({"name": "Ravi Kumar", "emails": [RAVI], "employee_code": "E1024", "manager_id": hema["id"]}))).await;
    assert_eq!(s, axum::http::StatusCode::CREATED, "{ravi}");
    env.ok("PUT", "/agent", ADMIN, Some(json!({"permissions": {"send": "auto"}}))).await;
    env.ok("PUT", "/me/agent-profile", OPERATOR, Some(json!({"display_name": "Priya", "approvals": {"send_email": "routine_ok"}}))).await;
    if let Some(mode) = mode {
        env.flag(Key::Handoff, mode).await;
    }
    let rohan = env.agent_id(ADMIN, "hrms.timesheets").await;
    sqlx::query("UPDATE agents SET autonomy_level = ?2, autonomy_cap = 3 WHERE id = ?1").bind(&rohan).bind(rohan_level).execute(&env.state.db).await.unwrap();
    World { env, src }
}

type Duty = (String, Option<String>, i64, i64, Option<i64>);

impl Env {
    /// (status, excuse, warn_at, due_at, final_due_at) of the reply duty of a message.
    async fn duty(&self, message_id: &str) -> Option<Duty> {
        sqlx::query_as("SELECT status, excuse, warn_at, due_at, final_due_at FROM reply_obligations WHERE workspace_id = ?1 AND message_id = ?2")
            .bind(&self.ws)
            .bind(message_id)
            .fetch_optional(&self.state.db)
            .await
            .unwrap()
    }

    async fn holdings(&self, message_id: &str) -> Vec<(String, String)> {
        sqlx::query_as("SELECT job, status FROM message_requests WHERE workspace_id = ?1 AND message_id = ?2 AND kind = 'holding' ORDER BY created_at, rowid").bind(&self.ws).bind(message_id).fetch_all(&self.state.db).await.unwrap()
    }
}

#[tokio::test]
async fn with_the_flag_off_no_duty_is_kept() {
    let w = world(Mbox::Personal, None, 3).await;
    let id = w.receive("gm-1", "th-1", RAVI).await;
    assert_eq!(w.env.duty(&id).await, None);
    assert_eq!(w.env.count("SELECT COUNT(*) FROM reply_obligations WHERE workspace_id = ?1").await, 0);
    assert!(w.env.sends().is_empty());
}

#[tokio::test]
async fn compare_mode_audits_the_duty_it_would_keep_and_keeps_none() {
    let w = world(Mbox::Personal, Some(Mode::Compare), 3).await;
    let id = w.receive("gm-1", "th-1", RAVI).await;
    assert_eq!(w.env.duty(&id).await, None, "compare writes nothing");
    let compare = w.audits("ownership.compare").await;
    assert_eq!(compare.len(), 1, "{compare:?}");
    assert_eq!(compare[0]["would_owe_reply"], json!({"excused": null}), "{}", compare[0]);
    assert!(w.env.holdings(&id).await.is_empty() && w.env.sends().is_empty());
    let noreply = w.receive("gm-2", "th-2", "noreply@client.test").await;
    email_ai_api::hr::staffing::timesheet_records::record_message(&w.env.state, &w.env.ws, &noreply).await.unwrap();
    email_ai_api::hr::staffing::timesheet_records::desk::compare(&w.env.state, &w.env.ws, &noreply).await.unwrap();
    let compare = w.audits("ownership.compare").await;
    assert_eq!(compare.last().unwrap()["would_owe_reply"], json!({"excused": "machine_sender"}), "{noreply}");
}

#[tokio::test]
async fn a_desk_owned_sheet_gets_a_duty_due_in_4_working_hours() {
    let w = world(Mbox::Personal, Some(Mode::On), 1).await;
    let id = w.receive("gm-1", "th-1", RAVI).await;
    let (status, excuse, warn_at, due_at, final_due_at) = w.env.duty(&id).await.expect("a duty");
    assert_eq!((status.as_str(), excuse, final_due_at), ("open", None, None), "the ack waits for approval, so the duty stays open");
    let received: i64 = sqlx::query_scalar("SELECT received_at FROM email_messages WHERE id = ?1").bind(&id).fetch_one(&w.env.state.db).await.unwrap();
    let hour = 3_600_000;
    assert!(warn_at >= received + 2 * hour && warn_at < due_at, "warn at half of the deadline in working time: {warn_at} {due_at}");
    assert!(due_at >= received + 4 * hour && due_at <= received + 4 * hour + 20 * hour, "4 working hours, nights and Sundays not counted: {}", (due_at - received) / hour);
    for stage in ["warn", "hold"] {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE kind = 'reply_sla' AND dedupe_key = ?1").bind(format!("sla:{id}:{stage}")).fetch_one(&w.env.state.db).await.unwrap();
        assert_eq!(n, 1, "{stage} is a timer on the jobs queue");
    }
    // Looking at the same mail again keeps the one duty.
    triggers::start_timesheet_case(&w.env.state, &w.env.ws, &id).await.unwrap();
    assert_eq!(w.env.count("SELECT COUNT(*) FROM reply_obligations WHERE workspace_id = ?1").await, 1);
}

#[tokio::test]
async fn a_sent_ack_closes_the_duty() {
    let w = world(Mbox::Personal, Some(Mode::On), 3).await;
    let id = w.receive("gm-1", "th-1", RAVI).await;
    assert_eq!(w.env.duty(&id).await.unwrap().0, "open", "composed is not yet sent");
    w.release_hold(&id).await;
    assert_eq!(w.env.sends().len(), 1);
    assert_eq!(w.env.duty(&id).await.unwrap().0, "sent");
    let reply: Option<String> = sqlx::query_scalar("SELECT reply_draft_id FROM reply_obligations WHERE message_id = ?1").bind(&id).fetch_one(&w.env.state.db).await.unwrap();
    assert!(reply.is_some(), "the duty names the reply that met it");
    // The timers that are still queued find nothing to do.
    w.env.run_due_after(THREE_DAYS).await;
    assert_eq!(w.env.sends().len(), 1);
    assert!(w.env.holdings(&id).await.is_empty());
}

#[tokio::test]
async fn a_stuck_desk_gets_a_warning_and_then_one_holding_note_through_the_gate() {
    let w = world(Mbox::Shared, None, 3).await;
    let id = w.stuck("gm-1", "th-1", RAVI).await;
    assert_eq!(w.env.duty(&id).await.unwrap().0, "open");
    assert!(w.env.requests(&id).await.is_empty() && w.env.live_drafts(&id).await.is_empty(), "nothing was composed: the desk is stuck");
    w.env.run_due_after(THREE_DAYS).await;
    assert_eq!(w.alerts("sla_warn:").await, 1, "the warning goes to whoever can unblock it, once");
    assert_eq!(w.env.holdings(&id).await, [("sla:holding".to_string(), "composed".to_string())], "one holding request, written by Meera");
    let drafts = w.env.live_drafts(&id).await;
    assert_eq!(drafts.len(), 1, "{drafts:?}");
    assert!(drafts[0].2.contains("We have received") && drafts[0].2.contains("will confirm by") && !drafts[0].2.contains("recorded"), "{}", drafts[0].2);
    assert!(!drafts[0].2.chars().any(|c| c == '₹') && !drafts[0].2.contains("hours"), "a holding note carries no facts: {}", drafts[0].2);
    assert!(w.env.sends().is_empty(), "the shared mailbox always asks first");
    let pending = w.pending().await;
    assert_eq!(pending.len(), 1);
    w.approve_all().await;
    assert_eq!(w.env.sends().len(), 1, "the holding note went once");
    let duty = w.env.duty(&id).await.unwrap();
    assert_eq!(duty.0, "holding_sent");
    assert!(duty.4.is_some(), "a person is told one working day later if only this went out");
}

#[tokio::test]
async fn one_holding_reply_per_duty_however_often_the_timers_and_the_sweep_fire() {
    let w = world(Mbox::Shared, None, 3).await;
    let id = w.stuck("gm-1", "th-1", RAVI).await;
    w.env.run_due_after(THREE_DAYS).await;
    w.approve_all().await;
    for _ in 0..3 {
        email_ai_api::agents::supervisor::sla::sweep(&w.env.state, &w.env.ws).await.unwrap();
        w.env.run_due_after(THREE_DAYS).await;
    }
    assert_eq!(w.env.holdings(&id).await.len(), 1, "{:?}", w.env.holdings(&id).await);
    assert_eq!(w.env.sends().len(), 1);
    assert_eq!(w.alerts("sla_warn:").await, 1);
}

#[tokio::test]
async fn a_reply_already_waiting_for_approval_gets_a_warning_and_no_second_mail() {
    let w = world(Mbox::Personal, Some(Mode::On), 1).await;
    let id = w.receive("gm-1", "th-1", RAVI).await;
    assert_eq!(w.pending().await.len(), 1, "the ack waits for approval");
    w.env.run_due_after(THREE_DAYS).await;
    assert!(w.env.holdings(&id).await.is_empty(), "a reply is composed: it is not replaced by a note that says less");
    assert_eq!(w.alerts("sla_warn:").await, 1, "the approver is told it is waiting");
    assert_eq!(w.pending().await.len(), 1);
    assert!(w.env.sends().is_empty());
    let skipped = w.audits("duty.hold_skipped").await;
    assert_eq!(skipped.len(), 1, "{skipped:?}");
    assert_eq!(skipped[0]["why"], "a reply is already waiting");
}

#[tokio::test]
async fn a_personal_mailbox_is_warned_and_never_answered_by_an_automatic_holding_note() {
    let w = world(Mbox::Personal, None, 3).await;
    let id = w.stuck("gm-1", "th-1", RAVI).await;
    w.env.run_due_after(THREE_DAYS).await;
    assert!(w.env.holdings(&id).await.is_empty() && w.env.sends().is_empty());
    assert_eq!(w.alerts("sla_warn:").await, 1, "the member who owns the mailbox is warned");
    assert_eq!(w.audits("duty.hold_skipped").await[0]["why"], "it is a personal mailbox");
}

#[tokio::test]
async fn a_closing_thanks_after_our_ack_is_excused_with_a_reason_and_real_mail_is_not() {
    let w = world(Mbox::Personal, Some(Mode::On), 3).await;
    let id = w.receive("gm-1", "th-1", RAVI).await;
    w.release_hold(&id).await;
    assert_eq!(w.env.sends().len(), 1, "the ack");
    let thanks = w.env.reply_to(&id, RAVI, "ok thanks").await;
    assert_eq!(w.env.duty(&thanks).await.map(|d| (d.0, d.1)), Some(("excused".into(), Some("closer_after_ack".into()))));
    assert!(w.env.requests(&thanks).await.is_empty() && w.env.live_drafts(&thanks).await.is_empty());
    let doubt = w.env.reply_to(&id, RAVI, "ok thanks, but the hours look wrong?").await;
    assert_eq!(w.env.duty(&doubt).await.map(|d| d.0), Some("open".into()), "a question is real mail");
}

#[tokio::test]
async fn machine_bulk_and_owner_listed_senders_are_excused_and_get_nothing() {
    let w = world(Mbox::Personal, Some(Mode::On), 3).await;
    let machine = w.receive("gm-1", "th-1", "noreply@client.test").await;
    triggers::start_timesheet_case(&w.env.state, &w.env.ws, &machine).await.unwrap();
    worker::drain(&w.env.state).await;
    assert_eq!(w.env.duty(&machine).await.map(|d| (d.0, d.1)), Some(("excused".into(), Some("machine_sender".into()))));
    // Bulk mail: the list headers say so.
    let (name, month) = last_month();
    let email = NormalizedEmail {
        source_message_id: "gm-2".into(),
        rfc_message_id: Some("<gm-2@mail.example>".into()),
        sender_address: "payroll-bot@client.test".into(),
        recipients: vec!["team@acme.com".into()],
        subject: format!("{name} timesheet"),
        body_text: sheet(&month),
        received_at: now_ms() - 3_600_000,
        bulk_signals: vec!["list_unsubscribe".into()],
        ..Default::default()
    };
    let mut tx = w.env.state.db.begin().await.unwrap();
    let bulk = ingest::persist_message(&mut tx, &w.source(), &w.env.ws, &email, Some("th-2"), json!({})).await.unwrap().unwrap();
    tx.commit().await.unwrap();
    worker::drain(&w.env.state).await;
    w.env.set_understanding(&bulk, "timesheet", false, json!([])).await;
    triggers::start_timesheet_case(&w.env.state, &w.env.ws, &bulk).await.unwrap();
    worker::drain(&w.env.state).await;
    assert_eq!(w.env.duty(&bulk).await.map(|d| (d.0, d.1)), Some(("excused".into(), Some("bulk".into()))));
    // The owner's own "don't reply" list.
    w.env.ok("PUT", "/team/settings", ADMIN, Some(json!({"tz_offset_minutes": 0, "below_target_hour": 16, "weekly_check_hour": 12, "no_reply_senders": ["@listed.test"]}))).await;
    let listed = w.receive("gm-3", "th-3", "someone@listed.test").await;
    assert_eq!(w.env.duty(&listed).await.map(|d| (d.0, d.1)), Some(("excused".into(), Some("owner_no_reply".into()))));
    assert!(w.env.live_drafts(&machine).await.is_empty() && w.env.live_drafts(&bulk).await.is_empty() && w.env.live_drafts(&listed).await.is_empty());
    assert!(w.env.sends().is_empty());
    w.env.run_due_after(THREE_DAYS).await;
    assert!(w.env.sends().is_empty() && w.env.holdings(&listed).await.is_empty(), "nothing is owed, so nothing goes out later either");
}

#[tokio::test]
async fn an_out_of_office_answer_and_our_own_mail_owe_no_reply() {
    let w = world(Mbox::Personal, Some(Mode::On), 3).await;
    let (_, month) = last_month();
    let mut ids = vec![];
    for (n, subject, signals) in [(1, "Automatic reply: on leave till 20 Oct", vec![]), (2, "[Post Room] digest", vec!["postroom".to_string()])] {
        let email = NormalizedEmail {
            source_message_id: format!("gm-{n}"),
            rfc_message_id: Some(format!("<gm-{n}@mail.example>")),
            sender_address: RAVI.into(),
            recipients: vec!["team@acme.com".into()],
            subject: subject.into(),
            body_text: sheet(&month),
            received_at: now_ms() - 3_600_000,
            bulk_signals: signals,
            ..Default::default()
        };
        let mut tx = w.env.state.db.begin().await.unwrap();
        let id = ingest::persist_message(&mut tx, &w.source(), &w.env.ws, &email, Some(&format!("th-{n}")), json!({})).await.unwrap().unwrap();
        tx.commit().await.unwrap();
        worker::drain(&w.env.state).await;
        w.env.set_understanding(&id, "timesheet", false, json!([])).await;
        let _ = triggers::start_timesheet_case(&w.env.state, &w.env.ws, &id).await.unwrap();
        worker::drain(&w.env.state).await;
        ids.push(id);
    }
    assert_eq!(w.env.duty(&ids[0]).await.map(|d| (d.0, d.1)), Some(("excused".into(), Some("auto_reply".into()))));
    assert_eq!(w.env.duty(&ids[1]).await, None, "our own mail is nobody's work: no row, no case, no mail");
    assert!(w.env.cases_by_job(&ids[1]).await.is_empty());
    assert!(w.env.sends().is_empty() && w.env.live_drafts(&ids[0]).await.is_empty());
}

#[tokio::test]
async fn held_back_mail_is_suppressed_with_no_case_no_draft_and_no_duty() {
    let w = world(Mbox::Personal, Some(Mode::On), 3).await;
    w.env.ok("PUT", "/team/settings", ADMIN, Some(json!({"tz_offset_minutes": 0, "below_target_hour": 16, "weekly_check_hour": 12, "hr_source_id": w.env.shared_src}))).await;
    let (s, p) = w.env.req("POST", "/team/people", ADMIN, Some(json!({"name": "Bina Das", "emails": ["bina@client.test"], "employee_type": "billable"}))).await;
    assert_eq!(s, axum::http::StatusCode::CREATED, "{p}");
    let id = w.receive("gm-1", "th-1", "bina@client.test").await;
    let (suppressed, visible): (bool, i64) = sqlx::query_as("SELECT suppressed, (SELECT COUNT(*) FROM classifications WHERE message_id = ?1) FROM email_messages WHERE id = ?1").bind(&id).fetch_one(&w.env.state.db).await.unwrap();
    assert!(suppressed, "kept out of sight");
    assert_eq!(visible, 0, "not even classified");
    assert!(w.env.cases_by_job(&id).await.is_empty() && w.env.live_drafts(&id).await.is_empty());
    assert_eq!(w.env.duty(&id).await, None, "no reply is owed: the person hears from the HR mailbox, not from this one");
    assert!(w.env.sends().is_empty());
}

#[tokio::test]
async fn a_person_who_answers_first_closes_the_duty_and_no_note_follows() {
    let w = world(Mbox::Shared, None, 3).await;
    let id = w.stuck("gm-1", "th-1", RAVI).await;
    // Someone replies from Gmail itself, then the mail syncs back as sent mail on the thread.
    let thread: String = sqlx::query_scalar("SELECT thread_id FROM email_messages WHERE id = ?1").bind(&id).fetch_one(&w.env.state.db).await.unwrap();
    let provider: String = sqlx::query_scalar("SELECT provider_thread_id FROM email_threads WHERE id = ?1").bind(&thread).fetch_one(&w.env.state.db).await.unwrap();
    let sent = NormalizedEmail {
        source_message_id: "out-1".into(),
        rfc_message_id: Some("<out-1@gmail.example>".into()),
        sender_address: "team@acme.com".into(),
        recipients: vec![RAVI.into()],
        subject: "Re: timesheet".into(),
        body_text: "Got it, thanks Ravi.".into(),
        received_at: now_ms(),
        outbound: true,
        ..Default::default()
    };
    let mut tx = w.env.state.db.begin().await.unwrap();
    ingest::persist_message(&mut tx, &w.env.shared_src, &w.env.ws, &sent, Some(&provider), json!({})).await.unwrap().unwrap();
    tx.commit().await.unwrap();
    assert_eq!(w.env.duty(&id).await.unwrap().0, "answered_by_person");
    w.env.run_due_after(THREE_DAYS).await;
    assert!(w.env.holdings(&id).await.is_empty() && w.env.sends().is_empty() && w.pending().await.is_empty());
}

#[tokio::test]
async fn it_escalates_after_one_more_working_day_when_only_the_holding_note_went() {
    let w = world(Mbox::Shared, None, 3).await;
    let id = w.stuck("gm-1", "th-1", RAVI).await;
    w.env.run_due_after(THREE_DAYS).await;
    w.approve_all().await;
    assert_eq!(w.env.duty(&id).await.unwrap().0, "holding_sent");
    assert_eq!(w.alerts("sla_escalate:").await, 0, "not yet");
    w.env.run_due_after(THREE_DAYS).await;
    assert_eq!(w.env.duty(&id).await.unwrap().0, "escalated");
    assert_eq!(w.alerts("sla_escalate:").await, 1, "a person is told once");
    w.env.run_due_after(THREE_DAYS).await;
    assert_eq!(w.alerts("sla_escalate:").await, 1);
    assert_eq!(w.env.sends().len(), 1, "no second mail to the sender");
}

#[tokio::test]
async fn the_sweep_catches_a_lost_timer() {
    let w = world(Mbox::Shared, None, 3).await;
    let id = w.stuck("gm-1", "th-1", RAVI).await;
    sqlx::query("DELETE FROM jobs WHERE kind = 'reply_sla'").execute(&w.env.state.db).await.unwrap();
    sqlx::query("UPDATE reply_obligations SET warn_at = warn_at - ?1, due_at = due_at - ?1").bind(THREE_DAYS).execute(&w.env.state.db).await.unwrap();
    assert!(w.env.holdings(&id).await.is_empty());
    email_ai_api::agents::supervisor::sla::sweep(&w.env.state, &w.env.ws).await.unwrap();
    worker::drain(&w.env.state).await;
    assert_eq!(w.env.holdings(&id).await.len(), 1, "the sweep ran the stage whose timer was lost");
    assert_eq!(w.alerts("sla_warn:").await, 1);
}

#[tokio::test]
async fn a_stranger_on_a_free_mailbox_gets_no_holding_note_and_a_person_is_told_instead() {
    let w = world(Mbox::Shared, None, 3).await;
    let id = w.stuck("gm-1", "th-1", "stranger@gmail.com").await;
    w.env.run_due_after(THREE_DAYS).await;
    assert!(w.env.holdings(&id).await.is_empty() && w.env.sends().is_empty());
    assert_eq!(w.audits("duty.hold_skipped").await[0]["why"], "the sender is not someone we can tell is real");
    w.env.run_due_after(THREE_DAYS).await;
    assert_eq!(w.alerts("sla_escalate:").await, 1, "never silent: the owner is told nobody could answer");
}

#[tokio::test]
async fn turning_the_flag_off_stops_the_timers() {
    let w = world(Mbox::Shared, None, 3).await;
    let id = w.stuck("gm-1", "th-1", RAVI).await;
    w.env.flag(Key::Handoff, Mode::Off).await;
    w.env.run_due_after(THREE_DAYS).await;
    assert!(w.env.holdings(&id).await.is_empty() && w.env.sends().is_empty());
    assert_eq!(w.alerts("sla_").await, 0);
}

#[tokio::test]
async fn the_reply_time_settings_round_trip_and_bad_values_are_refused() {
    let w = world(Mbox::Shared, None, 3).await;
    let base = json!({"tz_offset_minutes": 0, "below_target_hour": 16, "weekly_check_hour": 12});
    let with = |extra: Value| {
        let mut b = base.clone();
        b.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        b
    };
    let saved = w.env.ok("PUT", "/team/settings", ADMIN, Some(with(json!({"timezone": "Asia/Kolkata", "reply_sla_hours": 6, "work_start_hour": 10, "work_end_hour": 18, "work_days": [0, 1, 2, 3, 4], "holding_mode": "off", "no_reply_senders": ["Ravi@X.com", "@y.com"]})))).await;
    assert_eq!((saved["timezone"].as_str(), saved["reply_sla_hours"].as_i64(), saved["holding_mode"].as_str()), (Some("Asia/Kolkata"), Some(6), Some("off")));
    assert_eq!(saved["no_reply_senders"], json!(["ravi@x.com", "@y.com"]));
    for bad in [json!({"timezone": "Mars/Base"}), json!({"reply_sla_hours": 0}), json!({"work_start_hour": 18, "work_end_hour": 10}), json!({"work_days": []}), json!({"holding_mode": "sometimes"}), json!({"no_reply_senders": ["not an address"]})] {
        let (s, v) = w.env.req("PUT", "/team/settings", ADMIN, Some(with(bad.clone()))).await;
        assert_eq!(s, axum::http::StatusCode::BAD_REQUEST, "{bad}: {v}");
    }
    let kept = w.env.ok("GET", "/team/settings", ADMIN, None).await;
    assert_eq!(kept["reply_sla_hours"], 6, "a refused change leaves the settings alone");
}
