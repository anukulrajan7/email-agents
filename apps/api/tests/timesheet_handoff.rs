//! Slice S7: one gate for mail to the sender. Rohan files a typed request, Meera writes the one reply from a template
//! (no model) through the one live draft and sends it at the level of the expert who asked. Off is today's behaviour,
//! compare logs what would be sent, on acts.

use std::sync::Arc;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use email_ai_api::{
    agents::handoff::{self, Asker, Filed, MessageKind, MessageRequest},
    hr::team::clock,
    jobs::{self, kind, worker},
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

/// `(month name with year, YYYY-MM)` of the month `back` months before the current one.
fn month_back(back: usize) -> (String, String) {
    let mut month = clock::month_of(now_ms(), 0);
    for _ in 0..back {
        month = clock::previous_month(&month);
    }
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
    /// The shared team mailbox nobody owns: the lead Meera writes.
    Shared,
}

struct World {
    env: Env,
    mail: Arc<Recorder>,
    rohan: String,
    src: Mbox,
}

impl World {
    fn token(&self) -> &'static str {
        if self.src == Mbox::Personal { OPERATOR } else { ADMIN }
    }

    fn source(&self) -> String {
        if self.src == Mbox::Personal { self.env.op_src.clone() } else { self.env.shared_src.clone() }
    }

    async fn receive(&self, gmail_id: &str, thread: &str, month_back_by: usize, from: &str) -> String {
        let (name, month) = month_back(month_back_by);
        let id = self.env.receive(&self.source(), gmail_id, thread, from, &format!("{name} timesheet"), &sheet(&month), "timesheet").await;
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

    async fn pending(&self) -> Vec<Value> {
        self.env.ok("GET", "/approvals?status=pending", self.token(), None).await["items"].as_array().unwrap().clone()
    }

    /// The text that reached Gmail: the sent message itself, or the draft update that carried it.
    fn sent_raw(&self) -> (Value, String) {
        let sends = self.env.sends();
        assert_eq!(sends.len(), 1, "sent exactly once: {sends:?}");
        let sent = match sends[0].get("raw") {
            Some(_) => sends[0].clone(),
            None => self.env.calls.lock().unwrap().iter().rev().find(|(k, _)| k.starts_with("update_draft:")).map(|(_, b)| b["message"].clone()).expect("the draft update"),
        };
        let raw = String::from_utf8(URL_SAFE_NO_PAD.decode(sent["raw"].as_str().unwrap()).unwrap()).unwrap();
        (sent, raw)
    }

    /// Makes the held send due and runs it.
    async fn release_hold(&self, message_id: &str) {
        let (draft, send_at): (String, i64) = sqlx::query_as("SELECT id, send_at FROM drafts WHERE workspace_id = ?1 AND message_id = ?2 AND send_at IS NOT NULL")
            .bind(&self.env.ws)
            .bind(message_id)
            .fetch_one(&self.env.state.db)
            .await
            .unwrap();
        assert!(send_at > now_ms() + 500_000, "held about ten minutes");
        let due = now_ms();
        sqlx::query("UPDATE drafts SET send_at = ?1 WHERE id = ?2").bind(due).bind(&draft).execute(&self.env.state.db).await.unwrap();
        let mut conn = self.env.state.db.acquire().await.unwrap();
        jobs::enqueue_at(&mut conn, kind::SEND_DRAFT, &format!("send_draft:{draft}:{due}"), Some(&self.env.ws), &json!({"workspace_id": self.env.ws, "draft_id": draft, "send_at": due}), 3, due)
            .await
            .unwrap();
        drop(conn);
        worker::drain(&self.env.state).await;
    }
}

async fn world(src: Mbox, mode: Option<Mode>, rohan_level: i64) -> World {
    let mut env = Env::new(Opts::packs(&["general", "staffing"]).config(|c| c.agent_auto_send_delay_secs = 600)).await;
    let mail = Arc::new(Recorder::default());
    env.state.alert_mailer = Some(mail.clone());
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
    World { env, mail, rohan, src }
}

#[tokio::test]
async fn with_the_flag_off_nothing_is_filed_or_drafted() {
    let w = world(Mbox::Personal, None, 3).await;
    let id = w.receive("gm-1", "th-1", 1, RAVI).await;
    assert!(w.env.requests(&id).await.is_empty());
    assert!(w.env.live_drafts(&id).await.is_empty());
    assert!(w.env.sends().is_empty());
    assert!(w.env.cases_by_job(&id).await.is_empty());
    assert!(w.pending().await.is_empty());
}

#[tokio::test]
async fn compare_mode_audits_what_would_be_sent_and_writes_nothing() {
    let w = world(Mbox::Personal, Some(Mode::Compare), 3).await;
    let id = w.receive("gm-1", "th-1", 1, RAVI).await;
    let compare = w.audits("ownership.compare").await;
    assert_eq!(compare.len(), 1, "{compare:?}");
    assert_eq!(compare[0]["would_send"], json!({"kind": "ack", "to": {"to": "sender"}, "issues": 0, "requester_level": 3, "goes_out_on_its_own": true}), "{}", compare[0]);
    assert!(w.env.requests(&id).await.is_empty(), "no request is filed");
    assert!(w.env.live_drafts(&id).await.is_empty(), "0 drafts");
    assert!(w.env.sends().is_empty() && w.pending().await.is_empty());
    assert!(w.env.cases_by_job(&id).await.is_empty(), "compare acts as today: no case");
}

#[tokio::test]
async fn a_clean_sheet_gets_one_ack_from_meera_in_the_thread_sent_once_at_rohans_level() {
    let w = world(Mbox::Personal, Some(Mode::On), 3).await;
    // Meera's own level does not hold back words that Rohan vouched for.
    let meera = w.env.personal_id(OPERATOR).await;
    sqlx::query("UPDATE agents SET autonomy_level = 0 WHERE id = ?1").bind(&meera).execute(&w.env.state.db).await.unwrap();
    let id = w.receive("gm-1", "th-1", 1, RAVI).await;

    let drafts = w.env.live_drafts(&id).await;
    assert_eq!(drafts.len(), 1, "one live draft: {drafts:?}");
    let (name, month) = month_back(1);
    assert!(drafts[0].2.contains(&format!("your timesheet for {name} (23.5 h) and recorded it")) && !drafts[0].2.contains(&month), "no ISO month: {}", drafts[0].2);
    assert!(drafts[0].2.starts_with("Hello,") && drafts[0].2.ends_with("Best regards,\nHR Operations"), "{}", drafts[0].2);
    let author: String = sqlx::query_scalar("SELECT lower(a.name) FROM drafts d JOIN agents a ON a.id = d.agent_id WHERE d.id = ?1").bind(&drafts[0].0).fetch_one(&w.env.state.db).await.unwrap();
    assert_eq!(author, "meera");
    let (origin, model, by): (String, String, String) = sqlx::query_as("SELECT origin_key, model_id, requested_by_json FROM drafts WHERE id = ?1").bind(&drafts[0].0).fetch_one(&w.env.state.db).await.unwrap();
    assert_eq!((origin, model), (format!("reply:{id}"), "reply-template-v1".to_string()), "written by the template, no model");
    assert!(by.contains("\"name\":\"Rohan\"") && by.contains("\"kind\":\"ack\""), "requested_by names Rohan: {by}");
    assert_eq!(w.env.requests(&id).await, [("rohan".to_string(), "ack".to_string(), "composed".to_string())]);
    assert_eq!(w.env.cases_by_job(&id).await, [("desk:timesheet".to_string(), "rohan".to_string()), ("reply".to_string(), "meera".to_string())]);
    let expert_drafts: i64 = w.env.count("SELECT COUNT(*) FROM drafts WHERE workspace_id = ?1 AND agent_id IS NOT NULL AND origin_key NOT LIKE 'reply:%'").await;
    assert_eq!(expert_drafts, 0, "no expert draft");

    assert!(w.env.sends().is_empty(), "held first, so a person can stop it");
    assert_eq!(w.audits("agent.send_held").await.len(), 1);
    assert!(w.pending().await.is_empty(), "level 3 needs no approval");
    w.release_hold(&id).await;
    let (sent, raw) = w.sent_raw();
    assert_eq!(sent["threadId"], "th-1", "in the sender's thread");
    assert!(raw.contains("To: ravi@client.test") && raw.contains("In-Reply-To: <gm-1@mail.example>") && raw.contains("References: <gm-1@mail.example>"), "{raw}");
    assert_eq!(w.env.requests(&id).await[0].2, "sent");
    assert_eq!(w.env.count("SELECT COUNT(*) FROM cases WHERE workspace_id = ?1 AND job = 'reply' AND status = 'done'").await, 1);
    assert!(w.mail.take().len() == 2, "the manager summary still goes by the alert mailer, not through Meera");
}

#[tokio::test]
async fn at_level_0_and_1_the_ack_waits_for_an_approval_that_says_who_asked() {
    for level in [0, 1] {
        let w = world(Mbox::Personal, Some(Mode::On), level).await;
        let id = w.receive("gm-1", "th-1", 1, RAVI).await;
        assert_eq!(w.env.live_drafts(&id).await.len(), 1, "level {level}");
        assert!(w.env.sends().is_empty(), "level {level}");
        let pending = w.pending().await;
        assert_eq!(pending.len(), 1, "level {level}");
        assert_eq!(pending[0]["handoff"]["line"], "Rohan asked Meera to tell the sender the timesheet is recorded", "{}", pending[0]);
        assert_eq!(pending[0]["handoff"]["from"][0]["name"], "Rohan");
        assert!(pending[0]["body"].as_str().unwrap().contains("recorded it"));
        let (s, decided) = w.env.req("POST", &format!("/approvals/{}", pending[0]["id"].as_str().unwrap()), w.token(), Some(json!({"decision": "approve"}))).await;
        assert_eq!(s, axum::http::StatusCode::OK, "{decided}");
        worker::drain(&w.env.state).await;
        assert_eq!(w.env.sends().len(), 1, "sent once after the approval");
    }
}

#[tokio::test]
async fn a_shared_mailbox_reply_is_written_by_the_lead_and_always_asks() {
    let w = world(Mbox::Shared, Some(Mode::On), 3).await;
    let id = w.receive("gm-1", "th-1", 1, RAVI).await;
    let cases = w.env.cases_by_job(&id).await;
    assert!(cases.contains(&("reply".to_string(), "meera".to_string())), "{cases:?}");
    let lead: String = sqlx::query_scalar("SELECT a.kind FROM cases c JOIN agents a ON a.id = c.agent_id WHERE c.message_id = ?1 AND c.job = 'reply'").bind(&id).fetch_one(&w.env.state.db).await.unwrap();
    assert_eq!(lead, "lead");
    assert_eq!(w.pending().await.len(), 1, "nobody owns this mailbox, so nobody has agreed to routine sends");
    assert!(w.env.sends().is_empty());
}

#[tokio::test]
async fn a_wrong_month_sheet_gets_one_correction_naming_the_expected_month() {
    let w = world(Mbox::Personal, Some(Mode::On), 1).await;
    let id = w.receive("gm-1", "th-1", 2, RAVI).await;
    let (expected, _) = month_back(1);
    let (got, _) = month_back(2);
    assert_eq!(w.env.requests(&id).await, [("rohan".to_string(), "correction".to_string(), "composed".to_string())]);
    let drafts = w.env.live_drafts(&id).await;
    assert_eq!(drafts.len(), 1);
    assert!(drafts[0].2.contains(&format!("Your timesheet is for {got}, but we expected {expected}")) && drafts[0].2.contains(&format!("Please send the one for {expected}")), "{}", drafts[0].2);
    assert!(!drafts[0].2.contains("recorded"), "never a 'recorded' mail for a wrong sheet: {}", drafts[0].2);
    let pending = w.pending().await;
    assert_eq!(pending[0]["handoff"]["line"], "Rohan asked Meera to ask the sender for a corrected sheet");
    assert_eq!(w.mail.take().len(), 0, "no manager notice either");
}

#[tokio::test]
async fn a_portal_mismatch_gets_one_question_with_both_figures() {
    let w = world(Mbox::Personal, Some(Mode::On), 1).await;
    let (_, month) = month_back(1);
    let ravi: String = sqlx::query_scalar("SELECT id FROM people WHERE workspace_id = ?1 AND name = 'Ravi Kumar'").bind(&w.env.ws).fetch_one(&w.env.state.db).await.unwrap();
    sqlx::query("INSERT INTO portal_hours (workspace_id, person_id, month, hours, imported_at) VALUES (?1, ?2, ?3, 168, 1)").bind(&w.env.ws).bind(&ravi).bind(&month).execute(&w.env.state.db).await.unwrap();
    let id = w.receive("gm-1", "th-1", 1, RAVI).await;
    assert_eq!(w.env.requests(&id).await, [("rohan".to_string(), "question".to_string(), "composed".to_string())]);
    let drafts = w.env.live_drafts(&id).await;
    assert_eq!(drafts.len(), 1);
    assert!(drafts[0].2.contains("Your sheet says 23.5 h but the portal shows 168 h for this period. Which one is right?"), "{}", drafts[0].2);
}

#[tokio::test]
async fn a_rerun_files_no_second_request_and_writes_no_second_draft() {
    let w = world(Mbox::Personal, Some(Mode::On), 3).await;
    let id = w.receive("gm-1", "th-1", 1, RAVI).await;
    let before = w.audits("handoff.requested").await.len();
    // The desk looks at the same sheet again, three ways.
    email_ai_api::hr::staffing::timesheet_records::desk::catch_up(&w.env.state, &w.env.ws, &id).await.unwrap();
    email_ai_api::agents::triggers::start_timesheet_case(&w.env.state, &w.env.ws, &id).await.unwrap();
    let mut conn = w.env.state.db.acquire().await.unwrap();
    jobs::enqueue(&mut conn, kind::RECORD_TIMESHEET, "again", Some(&w.env.ws), &json!({"message_id": id})).await.unwrap();
    drop(conn);
    worker::drain(&w.env.state).await;
    assert_eq!(w.env.requests(&id).await.len(), 1);
    assert_eq!(w.audits("handoff.requested").await.len(), before, "no new request");
    assert!(w.audits("handoff.replaced").await.is_empty());
    assert_eq!(w.env.live_drafts(&id).await.len(), 1);
    assert_eq!(w.env.cases_by_job(&id).await.len(), 2, "one desk case and one reply case");
    w.release_hold(&id).await;
    assert_eq!(w.env.sends().len(), 1, "one send");
    // After it was sent, filing the very same request again still changes nothing.
    email_ai_api::hr::staffing::timesheet_records::desk::catch_up(&w.env.state, &w.env.ws, &id).await.unwrap();
    worker::drain(&w.env.state).await;
    assert_eq!(w.env.sends().len(), 1);
}

#[tokio::test]
async fn a_changed_request_rewrites_the_unsent_reply_and_the_waiting_approval() {
    let w = world(Mbox::Personal, Some(Mode::On), 1).await;
    let id = w.receive("gm-1", "th-1", 1, RAVI).await;
    let before = w.pending().await;
    assert_eq!(before.len(), 1);
    let rohan = w.rohan.clone();
    let asker = Asker { agent_id: &rohan, case_id: None, job: "desk:timesheet" };
    let question = MessageRequest::template(MessageKind::Question, vec![], vec![handoff::IssueText { code: "portal_mismatch".into(), text: "Which figure is right?".into() }]);
    let filed = handoff::hand_to_meera(&w.env.state, &w.env.ws, &asker, &id, &question).await.unwrap();
    assert!(matches!(filed, Filed::Replaced { .. }), "{filed:?}");
    let drafts = w.env.live_drafts(&id).await;
    assert_eq!(drafts.len(), 1, "still one live draft");
    assert!(drafts[0].2.contains("Which figure is right?") && !drafts[0].2.contains("recorded it"), "{}", drafts[0].2);
    let after = w.pending().await;
    assert_eq!((after.len(), after[0]["id"].clone()), (1, before[0]["id"].clone()), "the same approval, with the new text");
    assert!(after[0]["body"].as_str().unwrap().contains("Which figure is right?"));
    assert_eq!(w.audits("handoff.replaced").await.len(), 1);
    assert_eq!(w.env.requests(&id).await.len(), 1);
}

#[tokio::test]
async fn a_resent_corrected_sheet_supersedes_the_old_unsent_reply() {
    let w = world(Mbox::Personal, Some(Mode::On), 1).await;
    let first = w.receive("gm-1", "th-1", 2, RAVI).await;
    assert_eq!(w.env.live_drafts(&first).await.len(), 1, "the correction waits for approval");
    assert_eq!(w.pending().await.len(), 1);
    sqlx::query("UPDATE email_messages SET received_at = received_at - 600000 WHERE id = ?1").bind(&first).execute(&w.env.state.db).await.unwrap();
    let second = w.receive("gm-2", "th-1", 1, RAVI).await;
    assert!(w.env.live_drafts(&first).await.is_empty(), "the old correction is retired");
    assert_eq!(w.env.live_drafts(&second).await.len(), 1);
    assert!(w.env.live_drafts(&second).await[0].2.contains("recorded it"), "the corrected sheet is acknowledged");
    assert_eq!(w.env.requests(&first).await[0].2, "superseded");
    let pending = w.pending().await;
    assert_eq!(pending.len(), 1, "only the new reply waits for a person: {pending:?}");
    assert_eq!(pending[0]["handoff"]["line"], "Rohan asked Meera to tell the sender the timesheet is recorded");
    let old_case: String = sqlx::query_scalar("SELECT status FROM cases WHERE workspace_id = ?1 AND message_id = ?2 AND job = 'reply'").bind(&w.env.ws).bind(&first).fetch_one(&w.env.state.db).await.unwrap();
    assert_eq!(old_case, "cancelled");
    assert_eq!(w.audits("handoff.superseded").await.len(), 1);
    assert!(w.env.sends().is_empty());
}

#[tokio::test]
async fn own_and_no_reply_mail_gets_nothing() {
    let w = world(Mbox::Personal, Some(Mode::On), 3).await;
    // A sheet from a no-reply address is recorded and the request is refused.
    let id = w.receive("gm-1", "th-1", 1, "noreply@client.test").await;
    email_ai_api::agents::triggers::start_timesheet_case(&w.env.state, &w.env.ws, &id).await.unwrap();
    worker::drain(&w.env.state).await;
    assert_eq!(w.env.requests(&id).await, [("rohan".to_string(), "ack".to_string(), "dropped".to_string())]);
    assert!(w.env.live_drafts(&id).await.is_empty());
    assert!(w.env.sends().is_empty() && w.pending().await.is_empty());
    assert_eq!(w.audits("handoff.dropped").await.len(), 1);
    assert_eq!(w.env.cases_by_job(&id).await, [("desk:timesheet".to_string(), "rohan".to_string())], "no Meera case");

    // Our own mail never starts a case, and a request for it is refused outright.
    let email = NormalizedEmail {
        source_message_id: "own-1".into(),
        rfc_message_id: Some("<own-1@mail.example>".into()),
        sender_address: RAVI.into(),
        recipients: vec!["team@acme.com".into()],
        subject: "[Post Room] digest".into(),
        body_text: sheet(&month_back(1).1),
        received_at: now_ms() - 3_600_000,
        bulk_signals: vec!["postroom".into()],
        ..Default::default()
    };
    let mut tx = w.env.state.db.begin().await.unwrap();
    let own = email_ai_api::mail::ingest::persist_message(&mut tx, &w.source(), &w.env.ws, &email, Some("th-own"), json!({})).await.unwrap().unwrap();
    tx.commit().await.unwrap();
    worker::drain(&w.env.state).await;
    w.env.set_understanding(&own, "timesheet", false, json!([])).await;
    let started = email_ai_api::agents::triggers::start_timesheet_case(&w.env.state, &w.env.ws, &own).await.unwrap();
    assert!(matches!(started, email_ai_api::agents::triggers::Started::NotAllowed(_)), "{started:?}");
    let rohan = w.rohan.clone();
    let filed = handoff::hand_to_meera(&w.env.state, &w.env.ws, &Asker { agent_id: &rohan, case_id: None, job: "desk:timesheet" }, &own, &MessageRequest::template(MessageKind::Ack, vec![], vec![])).await.unwrap();
    assert_eq!(filed, Filed::Dropped("it is Post Room's own mail"));
    assert!(w.env.live_drafts(&own).await.is_empty() && w.env.cases_by_job(&own).await.is_empty());
}

#[tokio::test]
async fn a_recipient_outside_the_conversation_is_refused() {
    let w = world(Mbox::Personal, Some(Mode::On), 3).await;
    let id = w.receive("gm-1", "th-1", 1, RAVI).await;
    let rohan = w.rohan.clone();
    let evil = MessageRequest { to: handoff::Recipient::Participant { address: "x@evil.test".into() }, ..MessageRequest::template(MessageKind::Ack, vec![], vec![]) };
    let filed = handoff::hand_to_meera(&w.env.state, &w.env.ws, &Asker { agent_id: &rohan, case_id: None, job: "other:job" }, &id, &evil).await.unwrap();
    assert_eq!(filed, Filed::Dropped("the address is not in this conversation"));
    assert_eq!(w.env.live_drafts(&id).await.len(), 1, "the real reply is untouched");
}

#[tokio::test]
async fn a_held_ack_is_stopped_when_rohan_is_demoted_before_it_is_due() {
    let w = world(Mbox::Personal, Some(Mode::On), 3).await;
    let id = w.receive("gm-1", "th-1", 1, RAVI).await;
    assert!(w.env.sends().is_empty(), "held");
    sqlx::query("UPDATE agents SET autonomy_level = 1 WHERE id = ?1").bind(&w.rohan).execute(&w.env.state.db).await.unwrap();
    w.release_hold(&id).await;
    assert!(w.env.sends().is_empty(), "the words were vouched for by Rohan at level 3; at level 1 a person decides");
}

#[tokio::test]
async fn a_declined_reply_ends_the_case_and_sends_nothing() {
    let w = world(Mbox::Personal, Some(Mode::On), 1).await;
    let id = w.receive("gm-1", "th-1", 1, RAVI).await;
    let pending = w.pending().await;
    let (s, decided) = w.env.req("POST", &format!("/approvals/{}", pending[0]["id"].as_str().unwrap()), w.token(), Some(json!({"decision": "decline", "note": "I will answer myself"}))).await;
    assert_eq!(s, axum::http::StatusCode::OK, "{decided}");
    worker::drain(&w.env.state).await;
    let (status, state): (String, String) = sqlx::query_as("SELECT status, state_json FROM cases WHERE workspace_id = ?1 AND message_id = ?2 AND job = 'reply'").bind(&w.env.ws).bind(&id).fetch_one(&w.env.state.db).await.unwrap();
    assert_eq!(status, "done", "{state}");
    assert!(!state.contains("\"wf\":\"planner\""), "the planner never takes a reply job over: {state}");
    assert!(w.env.sends().is_empty());
}

/// A timesheet email that arrives with a draft already standing on it: the draft is put there before the desk looks.
async fn sheet_with_a_draft_already_there(w: &World, status: &str, origin: &str, body: &str) -> String {
    let email = NormalizedEmail {
        source_message_id: "pre-1".into(),
        rfc_message_id: Some("<pre-1@mail.example>".into()),
        sender_address: RAVI.into(),
        sender_name: Some("Ravi Kumar".into()),
        recipients: vec!["team@acme.com".into()],
        subject: "Monthly hours".into(),
        body_text: sheet(&month_back(1).1),
        received_at: now_ms() - 3_600_000,
        ..Default::default()
    };
    let mut tx = w.env.state.db.begin().await.unwrap();
    let id = email_ai_api::mail::ingest::persist_message(&mut tx, &w.source(), &w.env.ws, &email, Some("th-pre"), json!({})).await.unwrap().unwrap();
    tx.commit().await.unwrap();
    worker::drain(&w.env.state).await;
    sqlx::query("INSERT INTO drafts (id, workspace_id, message_id, body_text, original_body_text, status, model_id, origin_key, created_at, updated_at) VALUES ('drf_pre', ?1, ?2, ?3, ?3, ?4, 'some-model', ?5, ?6, ?6)")
        .bind(&w.env.ws)
        .bind(&id)
        .bind(body)
        .bind(status)
        .bind(origin)
        .bind(now_ms())
        .execute(&w.env.state.db)
        .await
        .unwrap();
    w.env.set_understanding(&id, "timesheet", false, json!([])).await;
    email_ai_api::agents::triggers::start_timesheet_case(&w.env.state, &w.env.ws, &id).await.unwrap();
    worker::drain(&w.env.state).await;
    id
}

#[tokio::test]
async fn another_writer_retiring_meeras_draft_closes_its_request_case_and_approval() {
    use email_ai_api::replies::live_draft::{self, NewDraft};
    let w = world(Mbox::Personal, Some(Mode::On), 1).await;
    let first = w.receive("gm-1", "th-1", 1, RAVI).await;
    assert_eq!((w.env.live_drafts(&first).await.len(), w.pending().await.len()), (1, 1));
    sqlx::query("UPDATE email_messages SET received_at = received_at - 600000 WHERE id = ?1").bind(&first).execute(&w.env.state.db).await.unwrap();
    // A different writer (an agent's own draft step) writes the reply of a later message on the same thread.
    let later = w.env.receive(&w.source(), "gm-2", "th-1", RAVI, "Re: timesheet", "Any news?", "general").await;
    let mut tx = w.env.state.db.begin_with("BEGIN IMMEDIATE").await.unwrap();
    live_draft::write(
        &mut tx,
        live_draft::Mode::Replace { allow_person_edited: false },
        &NewDraft { workspace_id: &w.env.ws, message_id: &later, origin_key: "agent:other:1", body_text: "Hello,\n\nWe are on it.\n\nPriya", model_id: "m", memory_ids_json: "[]".into(), unanswered_json: "[]".into(), audit: json!({}) },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert!(w.env.live_drafts(&first).await.is_empty(), "retired");
    assert_eq!(w.env.requests(&first).await[0].2, "superseded", "its request closes with it");
    assert!(w.pending().await.is_empty(), "no approval is left pointing at nothing");
    let status: String = sqlx::query_scalar("SELECT status FROM cases WHERE workspace_id = ?1 AND message_id = ?2 AND job = 'reply'").bind(&w.env.ws).bind(&first).fetch_one(&w.env.state.db).await.unwrap();
    assert_eq!(status, "cancelled");
}

#[tokio::test]
async fn a_changed_request_during_a_held_send_is_not_merged_and_the_sent_mail_does_not_cover_it() {
    let w = world(Mbox::Personal, Some(Mode::On), 3).await;
    let id = w.receive("gm-1", "th-1", 1, RAVI).await;
    assert!(w.env.sends().is_empty(), "held");
    let rohan = w.rohan.clone();
    let question = MessageRequest::template(MessageKind::Correction, vec![], vec![handoff::IssueText { code: "wrong_period".into(), text: "Please send the sheet for the right month.".into() }]);
    let filed = handoff::hand_to_meera(&w.env.state, &w.env.ws, &Asker { agent_id: &rohan, case_id: None, job: "desk:timesheet" }, &id, &question).await.unwrap();
    assert!(matches!(filed, Filed::Replaced { .. }), "{filed:?}");
    assert_eq!(w.audits("handoff.not_merged").await.len(), 1, "the trail says the new words did not reach the held mail");
    assert!(w.env.live_drafts(&id).await[0].2.contains("recorded it"), "the held text is untouched");
    assert_eq!(w.env.requests(&id).await, [("rohan".to_string(), "correction".to_string(), "pending".to_string())]);
    w.release_hold(&id).await;
    let (_, raw) = w.sent_raw();
    assert!(raw.contains("recorded it") && !raw.contains("right month"), "{raw}");
    // The request the sent mail did not cover is not marked sent: it goes out as its own mail, through the same gate.
    let requests = w.env.requests(&id).await;
    assert_ne!(requests[0].2, "sent", "{requests:?}");
    let drafts = w.env.live_drafts(&id).await;
    assert_eq!(drafts.len(), 1, "the follow-up is the one live draft: {drafts:?}");
    assert!(drafts[0].2.contains("Please send the sheet for the right month."), "{}", drafts[0].2);
    assert_eq!(w.env.sends().len(), 1, "and it waits out its own hold");
}

#[tokio::test]
async fn a_draft_a_person_edited_is_never_rewritten_or_sent_on_its_own_and_the_request_waits() {
    let w = world(Mbox::Personal, Some(Mode::On), 3).await;
    let id = sheet_with_a_draft_already_there(&w, "edited", "manual:abc", "Hello,\n\nMy own words.\n\nPriya").await;
    let drafts = w.env.live_drafts(&id).await;
    assert_eq!((drafts.len(), drafts[0].2.as_str()), (1, "Hello,\n\nMy own words.\n\nPriya"));
    assert!(w.env.sends().is_empty(), "a person's edited draft never goes on its own, whatever Rohan's level");
    assert!(w.pending().await.is_empty(), "the reply job does not take a person's own draft to approval either");
    assert_eq!(w.env.requests(&id).await, [("rohan".to_string(), "ack".to_string(), "pending".to_string())], "not marked composed");
    assert_eq!(w.audits("handoff.not_merged").await.len(), 1);
    let note: Option<String> = sqlx::query_scalar("SELECT status_note FROM message_requests WHERE message_id = ?1").bind(&id).fetch_one(&w.env.state.db).await.unwrap();
    assert!(note.unwrap().contains("another draft stands"));
}

#[tokio::test]
async fn a_machine_draft_that_is_not_the_replys_keeps_its_origin_and_is_not_rewritten_as_the_experts_words() {
    let w = world(Mbox::Personal, Some(Mode::On), 3).await;
    let meera = w.env.personal_id(OPERATOR).await;
    sqlx::query("UPDATE agents SET autonomy_level = 0 WHERE id = ?1").bind(&meera).execute(&w.env.state.db).await.unwrap();
    let id = sheet_with_a_draft_already_there(&w, "suggested", "agent:c1:1", "Hello,\n\nAnother agent wrote this.\n\nPriya").await;
    assert!(w.pending().await.is_empty(), "the reply job never sends another writer's draft, so there is nothing to approve either");
    assert!(w.env.sends().is_empty());
    let drafts = w.env.live_drafts(&id).await;
    assert_eq!(drafts[0].2, "Hello,\n\nAnother agent wrote this.\n\nPriya", "left as it was");
    let (origin, model): (String, String) = sqlx::query_as("SELECT origin_key, model_id FROM drafts WHERE id = ?1").bind(&drafts[0].0).fetch_one(&w.env.state.db).await.unwrap();
    assert_eq!((origin.as_str(), model.as_str()), ("agent:c1:1", "some-model"));
    assert_eq!(w.env.requests(&id).await[0].2, "pending");
    assert!(w.audits("handoff.not_merged").await.len() == 1);
}

#[tokio::test]
async fn a_reply_a_person_already_sent_is_not_followed_by_a_second_mail_from_the_desk() {
    let w = world(Mbox::Personal, Some(Mode::On), 3).await;
    let id = sheet_with_a_draft_already_there(&w, "sent", "manual:sent", "Hello,\n\nAlready answered.\n\nPriya").await;
    assert!(w.env.live_drafts(&id).await.is_empty(), "no second draft after the person's reply");
    assert!(w.env.sends().is_empty());
    assert_eq!(w.env.requests(&id).await[0].2, "cancelled", "closed for good, so it stops counting for levels and payroll");
    assert_eq!(w.audits("handoff.cancelled").await.len(), 1);
}

#[tokio::test]
async fn a_request_the_desk_withdraws_takes_its_unsent_reply_with_it() {
    let w = world(Mbox::Personal, Some(Mode::On), 1).await;
    let id = w.receive("gm-1", "th-1", 2, RAVI).await;
    assert_eq!((w.env.live_drafts(&id).await.len(), w.pending().await.len()), (1, 1));
    let rohan = w.rohan.clone();
    let decision = email_ai_api::agents::handoff::Decision { outcome: email_ai_api::agents::handoff::Outcome::NothingToDo, message: None, manager_summary: None, waits: vec![], parked: None, asks: vec![] };
    email_ai_api::hr::staffing::timesheet_records::desk::hand_off(&w.env.state, &w.env.ws, &Asker { agent_id: &rohan, case_id: None, job: "desk:timesheet" }, &id, &decision, false).await.unwrap();
    assert_eq!(w.env.requests(&id).await[0].2, "cancelled");
    assert!(w.env.live_drafts(&id).await.is_empty());
    assert!(w.pending().await.is_empty());
    assert_eq!(w.audits("handoff.withdrawn").await.len(), 1);
}

#[tokio::test]
async fn mail_from_one_of_our_own_addresses_is_never_written_to() {
    let w = world(Mbox::Personal, Some(Mode::On), 3).await;
    let id = w.receive("gm-1", "th-1", 1, "priya@acme.com").await;
    let requests = w.env.requests(&id).await;
    assert!(requests.iter().all(|r| r.2 == "dropped"), "{requests:?}");
    assert!(w.env.live_drafts(&id).await.is_empty() && w.env.sends().is_empty());
}

#[tokio::test]
async fn the_reply_job_never_sends_a_pipeline_draft() {
    let w = world(Mbox::Personal, Some(Mode::On), 3).await;
    let id = sheet_with_a_draft_already_there(&w, "suggested", "draft:auto:1", "Hello,\n\nThanks for the sheet.\n\nPriya").await;
    let held: i64 = w.env.count("SELECT COUNT(*) FROM drafts WHERE workspace_id = ?1 AND send_at IS NOT NULL").await;
    assert_eq!(held, 0, "nothing scheduled");
    w.env.run_due_after(3_600_000).await;
    assert!(w.env.sends().is_empty(), "0 sends");
    let requests = w.env.requests(&id).await;
    assert_eq!(requests, [("rohan".to_string(), "ack".to_string(), "pending".to_string())]);
    let note: Option<String> = sqlx::query_scalar("SELECT status_note FROM message_requests WHERE message_id = ?1").bind(&id).fetch_one(&w.env.state.db).await.unwrap();
    assert!(note.unwrap().contains("another draft stands"));
    let reply_case: String = sqlx::query_scalar("SELECT status FROM cases WHERE workspace_id = ?1 AND message_id = ?2 AND job = 'reply'").bind(&w.env.ws).bind(&id).fetch_one(&w.env.state.db).await.unwrap();
    assert_eq!(reply_case, "done", "the reply job ends without sending");
}

fn refile_of(facts: &str, issues: &str) -> MessageRequest {
    MessageRequest::template(MessageKind::Ack, serde_json::from_str(facts).unwrap(), serde_json::from_str(issues).unwrap())
}

#[tokio::test]
async fn a_withdrawn_request_filed_again_with_the_same_words_asks_again_and_gets_one_ack() {
    let w = world(Mbox::Personal, Some(Mode::On), 1).await;
    let id = w.receive("gm-1", "th-1", 1, RAVI).await;
    let (facts, issues): (String, String) = sqlx::query_as("SELECT facts_json, issues_json FROM message_requests WHERE message_id = ?1").bind(&id).fetch_one(&w.env.state.db).await.unwrap();
    let rohan = w.rohan.clone();
    let asker = Asker { agent_id: &rohan, case_id: None, job: "desk:timesheet" };
    let none = email_ai_api::agents::handoff::Decision { outcome: email_ai_api::agents::handoff::Outcome::NothingToDo, message: None, manager_summary: None, waits: vec![], parked: None, asks: vec![] };
    email_ai_api::hr::staffing::timesheet_records::desk::hand_off(&w.env.state, &w.env.ws, &asker, &id, &none, false).await.unwrap();
    assert_eq!(w.env.requests(&id).await[0].2, "cancelled");
    assert!(w.env.live_drafts(&id).await.is_empty() && w.pending().await.is_empty());
    let filed = handoff::hand_to_meera(&w.env.state, &w.env.ws, &asker, &id, &refile_of(&facts, &issues)).await.unwrap();
    assert!(matches!(filed, Filed::Requested { .. }), "{filed:?}");
    worker::drain(&w.env.state).await;
    assert_eq!(w.env.requests(&id).await, [("rohan".to_string(), "ack".to_string(), "composed".to_string())]);
    assert_eq!(w.env.live_drafts(&id).await.len(), 1, "one ack again");
    assert_eq!(w.pending().await.len(), 1, "waiting for a person at level 1, once");
    assert_eq!(w.audits("handoff.requested").await.len(), 2);
}

#[tokio::test]
async fn withdrawing_a_held_reply_stops_the_hold() {
    let w = world(Mbox::Personal, Some(Mode::On), 3).await;
    let id = w.receive("gm-1", "th-1", 1, RAVI).await;
    let held: i64 = w.env.count("SELECT COUNT(*) FROM drafts WHERE workspace_id = ?1 AND send_at IS NOT NULL").await;
    assert_eq!(held, 1, "held");
    let rohan = w.rohan.clone();
    let none = email_ai_api::agents::handoff::Decision { outcome: email_ai_api::agents::handoff::Outcome::NothingToDo, message: None, manager_summary: None, waits: vec![], parked: None, asks: vec![] };
    email_ai_api::hr::staffing::timesheet_records::desk::hand_off(&w.env.state, &w.env.ws, &Asker { agent_id: &rohan, case_id: None, job: "desk:timesheet" }, &id, &none, false).await.unwrap();
    assert!(w.env.live_drafts(&id).await.is_empty(), "the held reply is gone");
    assert_eq!(w.audits("handoff.hold_stopped").await.len(), 1);
    w.env.run_due_after(3_600_000).await;
    assert!(w.env.sends().is_empty(), "it never goes out");
}

#[tokio::test]
async fn merging_a_newer_request_later_clears_the_warning_and_the_note() {
    let w = world(Mbox::Personal, Some(Mode::On), 1).await;
    let id = w.receive("gm-1", "th-1", 1, RAVI).await;
    let rohan = w.rohan.clone();
    let asker = Asker { agent_id: &rohan, case_id: None, job: "desk:timesheet" };
    let ask = |text: &str| MessageRequest::template(MessageKind::Question, vec![], vec![handoff::IssueText { code: "portal_mismatch".into(), text: text.into() }]);
    sqlx::query("UPDATE drafts SET status = 'edited' WHERE message_id = ?1").bind(&id).execute(&w.env.state.db).await.unwrap();
    handoff::hand_to_meera(&w.env.state, &w.env.ws, &asker, &id, &ask("Which figure is right?")).await.unwrap();
    assert_eq!(w.pending().await[0]["handoff"]["newer"], "Rohan has a newer request that is not in this reply");
    sqlx::query("UPDATE drafts SET status = 'suggested' WHERE message_id = ?1").bind(&id).execute(&w.env.state.db).await.unwrap();
    handoff::hand_to_meera(&w.env.state, &w.env.ws, &asker, &id, &ask("Which hours are right?")).await.unwrap();
    let pending = w.pending().await;
    assert!(pending[0]["handoff"]["newer"].is_null(), "{}", pending[0]);
    assert!(pending[0]["body"].as_str().unwrap().contains("Which hours are right?"));
    let (status, note): (String, Option<String>) = sqlx::query_as("SELECT status, status_note FROM message_requests WHERE message_id = ?1").bind(&id).fetch_one(&w.env.state.db).await.unwrap();
    assert_eq!((status.as_str(), note), ("composed", None));
}

#[tokio::test]
async fn a_request_filed_while_the_reply_waits_is_not_stranded_when_that_case_ends() {
    let w = world(Mbox::Personal, Some(Mode::On), 1).await;
    let id = w.receive("gm-1", "th-1", 1, RAVI).await;
    let rohan = w.rohan.clone();
    let late = MessageRequest::template(MessageKind::Question, vec![], vec![handoff::IssueText { code: "x".into(), text: "One more thing: which project?".into() }]);
    handoff::hand_to_meera(&w.env.state, &w.env.ws, &Asker { agent_id: &rohan, case_id: None, job: "other:job" }, &id, &late).await.unwrap();
    assert_eq!(w.env.cases_by_job(&id).await.iter().filter(|(j, _)| j.starts_with("reply")).count(), 1, "the case is busy, so the request waits");
    let pending = w.pending().await;
    let (s, v) = w.env.req("POST", &format!("/approvals/{}", pending[0]["id"].as_str().unwrap()), w.token(), Some(json!({"decision": "approve"}))).await;
    assert_eq!(s, axum::http::StatusCode::OK, "{v}");
    worker::drain(&w.env.state).await;
    let jobs: Vec<String> = w.env.cases_by_job(&id).await.into_iter().map(|(j, _)| j).collect();
    assert!(jobs.iter().any(|j| j == "reply#2"), "{jobs:?}");
    let requests = w.env.requests(&id).await;
    assert!(requests.iter().all(|r| r.2 != "pending"), "{requests:?}");
}

async fn refile_stored(w: &World, id: &str, job: &str) -> Filed {
    let (kind, facts, issues): (String, String, String) =
        sqlx::query_as("SELECT kind, facts_json, issues_json FROM message_requests WHERE message_id = ?1 AND job = ?2").bind(id).bind(job).fetch_one(&w.env.state.db).await.unwrap();
    let req = MessageRequest::template(MessageKind::parse(&kind).unwrap(), serde_json::from_str(&facts).unwrap(), serde_json::from_str(&issues).unwrap());
    let rohan = w.rohan.clone();
    handoff::hand_to_meera(&w.env.state, &w.env.ws, &Asker { agent_id: &rohan, case_id: None, job }, id, &req).await.unwrap()
}

#[tokio::test]
async fn a_superseded_request_is_not_revived_by_a_late_rerun_of_the_desk() {
    let w = world(Mbox::Personal, Some(Mode::On), 1).await;
    let first = w.receive("gm-1", "th-1", 2, RAVI).await;
    sqlx::query("UPDATE email_messages SET received_at = received_at - 600000 WHERE id = ?1").bind(&first).execute(&w.env.state.db).await.unwrap();
    let second = w.receive("gm-2", "th-1", 1, RAVI).await;
    assert_eq!(w.env.requests(&first).await[0].2, "superseded");
    let filed = refile_stored(&w, &first, "desk:timesheet").await;
    worker::drain(&w.env.state).await;
    assert_eq!(filed, Filed::Unchanged);
    assert_eq!(w.env.requests(&first).await[0].2, "superseded", "still closed");
    assert!(w.env.live_drafts(&first).await.is_empty(), "the old correction does not come back next to the newer reply");
    assert_eq!(w.env.live_drafts(&second).await.len(), 1);
    assert_eq!(w.pending().await.len(), 1);
}

#[tokio::test]
async fn a_request_closed_because_someone_else_answered_is_not_revived() {
    let w = world(Mbox::Personal, Some(Mode::On), 3).await;
    let id = sheet_with_a_draft_already_there(&w, "sent", "manual:sent", "Hello,\n\nAlready answered.\n\nPriya").await;
    assert_eq!(w.env.requests(&id).await[0].2, "cancelled");
    let cases_before = w.env.cases_by_job(&id).await.len();
    let filed = refile_stored(&w, &id, "desk:timesheet").await;
    worker::drain(&w.env.state).await;
    assert_eq!(filed, Filed::Unchanged);
    assert_eq!(w.env.requests(&id).await[0].2, "cancelled");
    assert_eq!(w.env.cases_by_job(&id).await.len(), cases_before, "no new reply case, so nothing is cancelled again on every rerun");
    assert_eq!(w.audits("handoff.cancelled").await.len(), 1);
}

#[tokio::test]
async fn a_request_the_template_cannot_write_is_set_aside_and_does_not_spawn_reply_cases() {
    let w = world(Mbox::Personal, Some(Mode::On), 1).await;
    let id = w.receive("gm-1", "th-1", 1, RAVI).await;
    let rohan = w.rohan.clone();
    let custom = MessageRequest { kind: MessageKind::Answer, body: Some("Here is the answer.".into()), ..MessageRequest::template(MessageKind::Answer, vec![], vec![]) };
    handoff::hand_to_meera(&w.env.state, &w.env.ws, &Asker { agent_id: &rohan, case_id: None, job: "other:job" }, &id, &custom).await.unwrap();
    let pending = w.pending().await;
    let (s, v) = w.env.req("POST", &format!("/approvals/{}", pending[0]["id"].as_str().unwrap()), w.token(), Some(json!({"decision": "approve"}))).await;
    assert_eq!(s, axum::http::StatusCode::OK, "{v}");
    worker::drain(&w.env.state).await;
    for _ in 0..4 {
        handoff::store::follow_up(&w.env.state, &w.env.ws, &id).await.unwrap();
        worker::drain(&w.env.state).await;
    }
    let reply_cases = w.env.cases_by_job(&id).await.into_iter().filter(|(j, _)| j.starts_with("reply")).count();
    assert!(reply_cases <= 2, "the first reply and at most one follow-up: {reply_cases}");
    let note: Option<String> = sqlx::query_scalar("SELECT status_note FROM message_requests WHERE message_id = ?1 AND job = 'other:job'").bind(&id).fetch_one(&w.env.state.db).await.unwrap();
    assert_eq!(note.as_deref(), Some("the words are not all template"));
}

#[tokio::test]
async fn withdrawing_never_cancels_a_send_a_person_asked_for() {
    let w = world(Mbox::Personal, Some(Mode::On), 3).await;
    let id = w.receive("gm-1", "th-1", 1, RAVI).await;
    sqlx::query("UPDATE drafts SET send_requested_by = ?1 WHERE message_id = ?2").bind(&w.env.op_id).bind(&id).execute(&w.env.state.db).await.unwrap();
    let rohan = w.rohan.clone();
    let none = email_ai_api::agents::handoff::Decision { outcome: email_ai_api::agents::handoff::Outcome::NothingToDo, message: None, manager_summary: None, waits: vec![], parked: None, asks: vec![] };
    email_ai_api::hr::staffing::timesheet_records::desk::hand_off(&w.env.state, &w.env.ws, &Asker { agent_id: &rohan, case_id: None, job: "desk:timesheet" }, &id, &none, false).await.unwrap();
    assert_eq!(w.env.live_drafts(&id).await.len(), 1, "the person's own send stays");
    assert!(w.audits("handoff.hold_stopped").await.is_empty());
    assert_eq!(w.audits("handoff.not_merged").await.len(), 1);
}

#[tokio::test]
async fn turning_the_flag_off_stops_follow_up_cases() {
    let w = world(Mbox::Personal, Some(Mode::On), 3).await;
    let id = w.receive("gm-1", "th-1", 1, RAVI).await;
    w.release_hold(&id).await;
    sqlx::query(
        "INSERT INTO message_requests (id, workspace_id, message_id, from_agent_id, job, kind, digest, status, created_at, updated_at) VALUES ('mrq_x', ?1, ?2, ?3, 'x:job', 'ack', 'd', 'pending', 1, 1)",
    )
    .bind(&w.env.ws)
    .bind(&id)
    .bind(&w.rohan)
    .execute(&w.env.state.db)
    .await
    .unwrap();
    w.env.flag(Key::Handoff, Mode::Off).await;
    handoff::store::follow_up(&w.env.state, &w.env.ws, &id).await.unwrap();
    assert!(!w.env.cases_by_job(&id).await.iter().any(|(j, _)| j == "reply#2"), "off: no new reply case");
    w.env.flag(Key::Handoff, Mode::On).await;
    handoff::store::follow_up(&w.env.state, &w.env.ws, &id).await.unwrap();
    assert!(w.env.cases_by_job(&id).await.iter().any(|(j, _)| j == "reply#2"), "on: the follow-up starts");
}
