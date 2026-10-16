//! Reply duty: "never silent". Every inbound mail a desk owns carries a duty to answer its sender within `reply_sla_hours` working
//! hours, unless the mail is one nobody should answer (then the duty is `excused`, with the reason). The timers are jobs on the
//! ordinary queue (`REPLY_SLA`, dedupe `sla:{message}:{stage}`) and an hourly sweep catches a timer that was lost:
//!
//! - `warn` at half the time: whoever can unblock it is told (an approval waiting, a stuck desk). Nothing goes to the sender.
//! - `hold` at the deadline: when nothing has been composed, Meera sends one short holding note through the same gate (a
//!   `Holding` request: no facts, no hours). It never replaces a real reply that is already composed, never goes from a personal
//!   mailbox, and never to someone we cannot tell is real.
//! - `escalate` one working day later: a person is told that only a note (or nothing) went out.
//!
//! A sent reply of any kind closes the duty; a person's own answer closes it as `answered_by_person`. Each stage runs once
//! (its time is written on the row) and only while the `handoff` flag acts.

use serde_json::{Value, json};
use sqlx::{SqliteConnection, SqlitePool};

use crate::{
    agents::handoff::{self, Asker, Fact, MessageKind, MessageRequest},
    alerts,
    hr::team::settings::{self, TeamSettings},
    http::AppState,
    jobs::{self, handlers::JobError, kind},
    platform::{
        audit::{self, Actor, Entry},
        db::{new_id, now_ms},
        flags::{self, Key},
        time::{self, add_working_minutes},
    },
};

/// The job key of the holding note's request: its own row, so it never replaces the desk's own request.
pub const HOLDING_JOB: &str = "sla:holding";
const HOUR_MS: i64 = 3_600_000;
const SWEEP_EVERY_MS: i64 = HOUR_MS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deadlines {
    pub warn: i64,
    pub due: i64,
}

/// Half the allowance and all of it, in working time from the moment the mail arrived.
pub fn deadlines(received_ms: i64, zone: &time::Zone, week: &time::WorkWeek, sla_hours: i64) -> Deadlines {
    let minutes = sla_hours.max(1) * 60;
    Deadlines { warn: add_working_minutes(zone, week, received_ms, minutes / 2), due: add_working_minutes(zone, week, received_ms, minutes) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Warn,
    Hold,
    Escalate,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Warn => "warn",
            Stage::Hold => "hold",
            Stage::Escalate => "escalate",
        }
    }

    fn parse(raw: &str) -> Option<Stage> {
        [Stage::Warn, Stage::Hold, Stage::Escalate].into_iter().find(|s| s.as_str() == raw)
    }
}

/// What decides whether a reply is owed, gathered once per mail.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    pub inbound: bool,
    pub suppressed: bool,
    pub signals: Vec<String>,
    pub sender: String,
    pub auto_kind: Option<String>,
    /// The connected mailbox and whoever connected it.
    pub own_addresses: Vec<String>,
    pub body: String,
    pub subject: String,
    pub has_attachments: bool,
    /// The last mail we sent on this conversation was an acknowledgement.
    pub last_ours_was_ack: bool,
    /// The owner's never-reply list (addresses and `@domains`).
    pub no_reply: Vec<String>,
    /// A person (not us) already wrote on the conversation after this mail.
    pub answered_by_person: bool,
}

const CLOSERS: [&str; 22] = [
    "thanks", "thank", "you", "ok", "okay", "noted", "ji", "done", "theek", "thik", "hai", "got", "it", "thx", "a", "lot", "much", "so", "regards", "best", "shukriya", "dhanyavaad",
];

/// What the sender wrote in this mail: the quoted history and everything under an "On ... wrote:" line is left out.
pub fn fresh_text(body: &str) -> String {
    let mut out = vec![];
    for line in body.lines() {
        let t = line.trim();
        let lower = t.to_lowercase();
        if (lower.starts_with("on ") && lower.ends_with("wrote:")) || lower.contains("original message") || lower.starts_with("-----") {
            break;
        }
        if !t.starts_with('>') {
            out.push(t);
        }
    }
    out.join("\n").trim().to_string()
}

/// A short thank-you and nothing else: at most 8 words, all of them courtesy words, no question, no file.
pub fn is_closer(body: &str, has_attachments: bool) -> bool {
    let text = fresh_text(body);
    if has_attachments || text.contains('?') || text.is_empty() {
        return false;
    }
    if text.trim() == "👍" {
        return true;
    }
    let words: Vec<String> = text.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect();
    !words.is_empty() && words.len() <= 8 && words.iter().all(|w| CLOSERS.contains(&w.as_str()))
}

/// What a refused request says when the duty of its mail is excused.
pub fn excuse_words(code: &str) -> &'static str {
    match code {
        "own_mail" => "it is Post Room's own mail",
        "bulk" | "machine_sender" | "auto_reply" => "it is automatic or bulk mail",
        "bounce" => "it is a delivery failure notice",
        "own_address" => "the sender is one of our own addresses",
        "closer_after_ack" => "it only says thanks after our acknowledgement",
        "owner_no_reply" => "the owner asked for no automatic reply to this sender",
        "answered_by_person" => "a person already answered it",
        _ => "no reply is owed to this mail",
    }
}

/// The reason this mail's duty was excused, if it was.
pub async fn excused(db: &SqlitePool, ws: &str, message_id: &str) -> sqlx::Result<Option<String>> {
    Ok(sqlx::query_scalar::<_, Option<String>>("SELECT excuse FROM reply_obligations WHERE workspace_id = ?1 AND message_id = ?2 AND status = 'excused'").bind(ws).bind(message_id).fetch_optional(db).await?.flatten())
}

/// Is this sender on the owner's never-reply list?
pub fn listed(sender: &str, list: &[String]) -> bool {
    let address = crate::replies::send_safety::bare_address(sender);
    let domain = address.rsplit_once('@').map(|(_, d)| format!("@{d}")).unwrap_or_default();
    list.iter().any(|l| l.eq_ignore_ascii_case(&address) || (l.starts_with('@') && l.eq_ignore_ascii_case(&domain)))
}

/// Why no reply is owed, if so. The reasons are checked in this order and stored on the row.
pub fn excuse_reason(f: &Facts) -> Option<&'static str> {
    let has = |s: &str| f.signals.iter().any(|x| x == s);
    if crate::mail::system_mail::is_own(&f.signals) {
        return Some("own_mail");
    }
    if !f.inbound {
        return Some("not_inbound");
    }
    if f.suppressed {
        return Some("held_back");
    }
    let bare = crate::replies::send_safety::bare_address(&f.sender);
    if f.own_addresses.iter().any(|o| crate::replies::send_safety::bare_address(o) == bare) {
        return Some("own_address");
    }
    if has("list_unsubscribe") || has("precedence") || has("gmail_category") {
        return Some("bulk");
    }
    if crate::mail::bots::is_automated_address(&f.sender) || has("no_reply_sender") {
        return Some("machine_sender");
    }
    match f.auto_kind.as_deref() {
        Some("dsn") => return Some("bounce"),
        Some("ooo") => return Some("auto_reply"),
        _ => {}
    }
    if has("auto_submitted") || has("auto_reply") || crate::mail::bots::is_auto_reply_subject(&f.subject) {
        return Some("auto_reply");
    }
    if has("delivery_status") || crate::mail::bots::is_bounce_subject(&f.subject) {
        return Some("bounce");
    }
    if f.last_ours_was_ack && is_closer(&f.body, f.has_attachments) {
        return Some("closer_after_ack");
    }
    if listed(&f.sender, &f.no_reply) {
        return Some("owner_no_reply");
    }
    if f.answered_by_person {
        return Some("answered_by_person");
    }
    None
}

#[derive(sqlx::FromRow)]
struct MessageRow {
    thread_id: String,
    sender_address: String,
    direction: String,
    suppressed: bool,
    bulk_signals: String,
    auto_kind: Option<String>,
    subject: String,
    body_text: String,
    received_at: i64,
    attachments: i64,
}

pub struct Assessment {
    pub excuse: Option<&'static str>,
    pub thread_id: String,
    pub sender: String,
    pub received_at: i64,
    pub subject: String,
}

/// Reads what decides the duty of a mail. `None` when the mail is gone.
pub async fn assess(state: &AppState, ws: &str, message_id: &str, cfg: &TeamSettings) -> sqlx::Result<Option<Assessment>> {
    let Some(m) = sqlx::query_as::<_, MessageRow>(
        "SELECT thread_id, sender_address, direction, suppressed, bulk_signals, auto_kind, subject, body_text, received_at,
                (SELECT COUNT(*) FROM attachments a WHERE a.message_id = email_messages.id) AS attachments
         FROM email_messages WHERE id = ?1 AND workspace_id = ?2",
    )
    .bind(message_id)
    .bind(ws)
    .fetch_optional(&state.db)
    .await?
    else {
        return Ok(None);
    };
    let own: Vec<String> = sqlx::query_scalar(
        "SELECT lower(s.mailbox_address) FROM email_messages m JOIN email_sources s ON s.id = m.source_id WHERE m.id = ?1 AND s.mailbox_address IS NOT NULL
         UNION SELECT lower(u.email) FROM email_messages m JOIN email_sources s ON s.id = m.source_id JOIN users u ON u.id = s.connected_by_user_id WHERE m.id = ?1",
    )
    .bind(message_id)
    .fetch_all(&state.db)
    .await?;
    let last_ack: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM message_requests r WHERE r.workspace_id = ?1 AND r.kind = 'ack' AND r.draft_id = (
             SELECT d.id FROM drafts d JOIN email_messages x ON x.id = d.message_id
             WHERE x.thread_id = ?2 AND d.status = 'sent' AND d.updated_at < ?3 ORDER BY d.updated_at DESC, d.rowid DESC LIMIT 1))",
    )
    .bind(ws)
    .bind(&m.thread_id)
    .bind(now_ms() + 1)
    .fetch_one(&state.db)
    .await?;
    let answered = answered_by_person(&state.db, ws, &m.thread_id, m.received_at).await?;
    let facts = Facts {
        inbound: m.direction == "inbound",
        suppressed: m.suppressed,
        signals: serde_json::from_str(&m.bulk_signals).unwrap_or_default(),
        sender: m.sender_address.clone(),
        auto_kind: m.auto_kind,
        own_addresses: own,
        body: m.body_text,
        subject: m.subject.clone(),
        has_attachments: m.attachments > 0,
        last_ours_was_ack: last_ack,
        no_reply: cfg.no_reply_senders.clone(),
        answered_by_person: answered,
    };
    Ok(Some(Assessment { excuse: excuse_reason(&facts), thread_id: m.thread_id, sender: m.sender_address, received_at: m.received_at, subject: m.subject }))
}

/// Someone wrote on the conversation from our mailbox after `since`, and it was not a mail we sent ourselves.
async fn answered_by_person(db: &SqlitePool, ws: &str, thread_id: &str, since: i64) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM email_messages o WHERE o.workspace_id = ?1 AND o.thread_id = ?2 AND o.direction = 'outbound' AND o.received_at >= ?3
                         AND NOT EXISTS (SELECT 1 FROM drafts d WHERE d.rfc_message_id IS NOT NULL AND d.rfc_message_id = o.rfc_message_id))",
    )
    .bind(ws)
    .bind(thread_id)
    .bind(since)
    .fetch_one(db)
    .await
}

async fn audit_duty(conn: &mut SqliteConnection, ws: &str, message_id: &str, action: &str, metadata: Value) -> sqlx::Result<()> {
    audit::record(conn, Entry { workspace_id: ws, actor: Actor::System, action, entity_type: "message", entity_id: message_id, message_id: Some(message_id), metadata }).await
}

/// Starts the reply duty of a mail a desk owns (the `handoff` flag acts). One row per mail: opening again changes nothing. An
/// excused mail gets a row too, with the reason. Returns the excuse, if any.
pub async fn open(state: &AppState, ws: &str, message_id: &str, agent_id: &str, job: &str) -> sqlx::Result<Option<&'static str>> {
    if !flags::mode(&state.db, &state.config, ws, Key::Handoff).await?.acts() {
        return Ok(None);
    }
    let cfg = settings::load(&state.db, ws).await?;
    let Some(a) = assess(state, ws, message_id, &cfg).await? else { return Ok(None) };
    let now = now_ms();
    let d = deadlines(a.received_at, &cfg.zone(), &cfg.work_week(), cfg.reply_sla_hours);
    let status = if a.excuse.is_some() { "excused" } else { "open" };
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let made = sqlx::query(
        "INSERT INTO reply_obligations (id, workspace_id, message_id, thread_id, agent_id, job, sender_address, status, excuse, warn_at, due_at, closed_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)
         ON CONFLICT(workspace_id, message_id) DO NOTHING",
    )
    .bind(new_id("dty"))
    .bind(ws)
    .bind(message_id)
    .bind(&a.thread_id)
    .bind(agent_id)
    .bind(job)
    .bind(&a.sender)
    .bind(status)
    .bind(a.excuse)
    .bind(d.warn)
    .bind(d.due)
    .bind(a.excuse.map(|_| now))
    .bind(now)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if made {
        audit_duty(&mut tx, ws, message_id, if a.excuse.is_some() { "duty.excused" } else { "duty.opened" }, json!({"excuse": a.excuse, "warn_at": d.warn, "due_at": d.due, "job": job})).await?;
        if a.excuse.is_none() {
            for (stage, at) in [(Stage::Warn, d.warn), (Stage::Hold, d.due)] {
                enqueue_stage(&mut tx, ws, message_id, stage, at).await?;
            }
        }
    }
    tx.commit().await?;
    state.jobs_notify.notify_one();
    Ok(a.excuse)
}

async fn enqueue_stage(conn: &mut SqliteConnection, ws: &str, message_id: &str, stage: Stage, at: i64) -> sqlx::Result<()> {
    jobs::enqueue_at(
        conn,
        kind::REPLY_SLA,
        &format!("sla:{message_id}:{}", stage.as_str()),
        Some(ws),
        &json!({"workspace_id": ws, "message_id": message_id, "stage": stage.as_str()}),
        jobs::DEFAULT_MAX_ATTEMPTS,
        at,
    )
    .await?;
    Ok(())
}

#[derive(sqlx::FromRow, Clone)]
struct Duty {
    id: String,
    message_id: String,
    thread_id: String,
    agent_id: Option<String>,
    sender_address: String,
    status: String,
    warn_at: i64,
    due_at: i64,
    final_due_at: Option<i64>,
    warned_at: Option<i64>,
    held_at: Option<i64>,
    escalated_at: Option<i64>,
}

const DUTY_COLUMNS: &str = "id, message_id, thread_id, agent_id, sender_address, status, warn_at, due_at, final_due_at, warned_at, held_at, escalated_at";

async fn load(db: &SqlitePool, ws: &str, message_id: &str) -> sqlx::Result<Option<Duty>> {
    sqlx::query_as(&format!("SELECT {DUTY_COLUMNS} FROM reply_obligations WHERE workspace_id = ?1 AND message_id = ?2")).bind(ws).bind(message_id).fetch_optional(db).await
}

async fn subject_of(db: &SqlitePool, message_id: &str) -> String {
    let subject: Option<String> = sqlx::query_scalar("SELECT subject FROM email_messages WHERE id = ?1").bind(message_id).fetch_optional(db).await.ok().flatten();
    let subject = subject.unwrap_or_default();
    if subject.trim().is_empty() { "(no subject)".into() } else { subject }
}

/// The `REPLY_SLA` job: runs one stage for one mail.
pub async fn stage_job(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let field = |k: &str| payload.get(k).and_then(Value::as_str);
    let (Some(ws), Some(message_id), Some(stage)) = (field("workspace_id"), field("message_id"), field("stage").and_then(Stage::parse)) else {
        return Err(JobError::Permanent("payload missing workspace_id, message_id or stage".into()));
    };
    run_stage(state, ws, message_id, stage).await?;
    Ok(())
}

/// Runs one stage if it is due and not yet done. Safe to call again and from two places (the timer and the sweep).
pub async fn run_stage(state: &AppState, ws: &str, message_id: &str, stage: Stage) -> sqlx::Result<()> {
    if !flags::mode(&state.db, &state.config, ws, Key::Handoff).await?.acts() {
        return Ok(());
    }
    let Some(duty) = load(&state.db, ws, message_id).await? else { return Ok(()) };
    let now = now_ms();
    let (column, allowed): (&str, &[&str]) = match stage {
        Stage::Warn => ("warned_at", &["open"]),
        Stage::Hold => ("held_at", &["open"]),
        Stage::Escalate => ("escalated_at", &["open", "holding_sent"]),
    };
    if !allowed.contains(&duty.status.as_str()) || (stage == Stage::Escalate && duty.final_due_at.is_none()) {
        return Ok(());
    }
    let claimed = sqlx::query(&format!("UPDATE reply_obligations SET {column} = ?1, updated_at = ?1 WHERE id = ?2 AND {column} IS NULL AND status IN ({})", allowed.iter().map(|s| format!("'{s}'")).collect::<Vec<_>>().join(",")))
        .bind(now)
        .bind(&duty.id)
        .execute(&state.db)
        .await?
        .rows_affected()
        == 1;
    if !claimed {
        return Ok(());
    }
    match stage {
        Stage::Warn => warn(state, ws, message_id, &duty).await,
        Stage::Hold => hold(state, ws, message_id, &duty).await,
        Stage::Escalate => escalate(state, ws, message_id, &duty).await,
    }
}

async fn warn(state: &AppState, ws: &str, message_id: &str, duty: &Duty) -> sqlx::Result<()> {
    let waiting = reply_waiting(&state.db, ws, message_id).await?;
    let subject = subject_of(&state.db, message_id).await;
    let (title, body) = if waiting {
        (format!("Reply waiting for approval: {subject}"), format!("A reply to {} is written and waiting for someone to approve it. They have waited half of the time we promise.", duty.sender_address))
    } else {
        (format!("Reply due soon: {subject}"), format!("{} wrote to us and no reply is written yet. A reply is due within the hour.", duty.sender_address))
    };
    raise(state, ws, message_id, "sla_warn", 1, title, body).await?;
    let mut conn = state.db.acquire().await?;
    audit_duty(&mut conn, ws, message_id, "duty.warned", json!({"waiting_for_approval": waiting})).await
}

async fn raise(state: &AppState, ws: &str, message_id: &str, prefix: &str, level: i64, title: String, body: String) -> sqlx::Result<()> {
    alerts::raise(&state.db, alerts::NewAlert { workspace_id: ws, kind: alerts::AUTOMATION, severity: "high", title, body, message_id: Some(message_id), level, dedupe_key: Some(format!("{prefix}:{message_id}")) }).await?;
    Ok(())
}

/// A reply for this mail is written (or parked for a person) and has not gone out.
async fn reply_waiting(db: &SqlitePool, ws: &str, message_id: &str) -> sqlx::Result<bool> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM message_requests WHERE workspace_id = ?1 AND message_id = ?2 AND status IN ('pending', 'composed') AND kind != 'holding')")
        .bind(ws)
        .bind(message_id)
        .fetch_one(db)
        .await
}

async fn is_personal(state: &AppState, ws: &str, message_id: &str, hr_source: Option<&str>) -> sqlx::Result<bool> {
    let ctx = crate::agents::profiles::context_for_message(&state.db, &state.config, ws, message_id).await?;
    let source: Option<String> = sqlx::query_scalar("SELECT source_id FROM email_messages WHERE id = ?1").bind(message_id).fetch_optional(&state.db).await?;
    Ok(ctx.owner.is_some() && source.as_deref() != hr_source)
}

async fn skipped(state: &AppState, ws: &str, message_id: &str, why: &str) -> sqlx::Result<()> {
    let mut conn = state.db.acquire().await?;
    audit_duty(&mut conn, ws, message_id, "duty.hold_skipped", json!({"why": why})).await
}

async fn hold(state: &AppState, ws: &str, message_id: &str, duty: &Duty) -> sqlx::Result<()> {
    let cfg = settings::load(&state.db, ws).await?;
    let (zone, week) = (cfg.zone(), cfg.work_week());
    let final_due = add_working_minutes(&zone, &week, now_ms(), week.day_minutes());
    sqlx::query("UPDATE reply_obligations SET final_due_at = ?1 WHERE id = ?2").bind(final_due).bind(&duty.id).execute(&state.db).await?;
    let mut conn = state.db.acquire().await?;
    enqueue_stage(&mut conn, ws, message_id, Stage::Escalate, final_due).await?;
    drop(conn);
    let since: i64 = sqlx::query_scalar("SELECT received_at FROM email_messages WHERE id = ?1").bind(message_id).fetch_one(&state.db).await?;
    if answered_by_person(&state.db, ws, &duty.thread_id, since).await? {
        return close(&state.db, ws, message_id, "answered_by_person", None).await;
    }
    if reply_waiting(&state.db, ws, message_id).await? {
        return skipped(state, ws, message_id, "a reply is already waiting").await;
    }
    let holding_exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM message_requests WHERE workspace_id = ?1 AND message_id = ?2 AND kind = 'holding' AND status NOT IN ('dropped', 'cancelled', 'superseded'))")
        .bind(ws)
        .bind(message_id)
        .fetch_one(&state.db)
        .await?;
    if holding_exists {
        return skipped(state, ws, message_id, "a holding note was already filed").await;
    }
    if cfg.holding_mode != "auto" {
        return skipped(state, ws, message_id, "holding notes are switched off").await;
    }
    if is_personal(state, ws, message_id, cfg.hr_source_id.as_deref()).await? {
        return skipped(state, ws, message_id, "it is a personal mailbox").await;
    }
    let Some(agent_id) = duty.agent_id.as_deref() else { return skipped(state, ws, message_id, "no agent owns this mail").await };
    if !crate::hr::staffing::timesheet_records::desk::receipt_allowed(state, ws, message_id).await? {
        return skipped(state, ws, message_id, "the sender is not someone we can tell is real").await;
    }
    let mut facts = vec![Fact { label: "About".into(), value: "timesheet".into() }];
    facts.push(Fact { label: "Reply by".into(), value: time::day_label(&zone, final_due) });
    let filed = handoff::hand_to_meera(state, ws, &Asker { agent_id, case_id: None, job: HOLDING_JOB }, message_id, &MessageRequest::template(MessageKind::Holding, facts, vec![])).await?;
    let mut conn = state.db.acquire().await?;
    audit_duty(&mut conn, ws, message_id, "duty.holding_filed", json!({"filed": format!("{filed:?}"), "reply_by": final_due})).await
}

async fn escalate(state: &AppState, ws: &str, message_id: &str, duty: &Duty) -> sqlx::Result<()> {
    let subject = subject_of(&state.db, message_id).await;
    let held = duty.status == "holding_sent";
    let (title, body) = if held {
        (format!("Only a holding note went out: {subject}"), format!("{} has had only a holding note for a working day. Please reply to them.", duty.sender_address))
    } else {
        (format!("Nobody could reply: {subject}"), format!("{} wrote to us and no reply has gone out for more than a working day. Please reply to them yourself.", duty.sender_address))
    };
    raise(state, ws, message_id, "sla_escalate", 2, title, body).await?;
    sqlx::query("UPDATE reply_obligations SET status = 'escalated', updated_at = ?2 WHERE id = ?1 AND status IN ('open', 'holding_sent')").bind(&duty.id).bind(now_ms()).execute(&state.db).await?;
    let mut conn = state.db.acquire().await?;
    audit_duty(&mut conn, ws, message_id, "duty.escalated", json!({"only_a_holding_note": held})).await
}

async fn close(db: &SqlitePool, ws: &str, message_id: &str, status: &str, draft_id: Option<&str>) -> sqlx::Result<()> {
    let mut conn = db.acquire().await?;
    close_in(&mut conn, ws, message_id, status, draft_id).await
}

async fn close_in(conn: &mut SqliteConnection, ws: &str, message_id: &str, status: &str, draft_id: Option<&str>) -> sqlx::Result<()> {
    let closed = sqlx::query(
        "UPDATE reply_obligations SET status = ?3, reply_draft_id = COALESCE(?4, reply_draft_id), closed_at = ?5, updated_at = ?5
         WHERE workspace_id = ?1 AND message_id = ?2 AND status IN ('open', 'holding_sent', 'escalated')",
    )
    .bind(ws)
    .bind(message_id)
    .bind(status)
    .bind(draft_id)
    .bind(now_ms())
    .execute(&mut *conn)
    .await?
    .rows_affected()
        == 1;
    if closed {
        audit_duty(conn, ws, message_id, "duty.closed", json!({"status": status})).await?;
    }
    Ok(())
}

/// A reply of ours went out (called in the send's own transaction). A holding note only moves an open duty to `holding_sent`; any
/// other reply, or a person's own send, closes it.
pub async fn on_sent(conn: &mut SqliteConnection, ws: &str, message_id: &str, draft_id: &str) -> sqlx::Result<()> {
    let kinds: Vec<String> = sqlx::query_scalar("SELECT kind FROM message_requests WHERE workspace_id = ?1 AND draft_id = ?2 AND status = 'sent'").bind(ws).bind(draft_id).fetch_all(&mut *conn).await?;
    if kinds.is_empty() {
        let ours: bool = sqlx::query_scalar("SELECT COALESCE(substr(origin_key, 1, 6) = 'reply:', 0) FROM drafts WHERE id = ?1").bind(draft_id).fetch_one(&mut *conn).await?;
        return close_in(conn, ws, message_id, if ours { "sent" } else { "answered_by_person" }, Some(draft_id)).await;
    }
    if kinds.iter().all(|k| k == "holding") {
        let moved = sqlx::query("UPDATE reply_obligations SET status = 'holding_sent', reply_draft_id = ?3, updated_at = ?4 WHERE workspace_id = ?1 AND message_id = ?2 AND status = 'open'")
            .bind(ws)
            .bind(message_id)
            .bind(draft_id)
            .bind(now_ms())
            .execute(&mut *conn)
            .await?
            .rows_affected()
            == 1;
        if moved {
            audit_duty(conn, ws, message_id, "duty.holding_sent", json!({"draft_id": draft_id})).await?;
        }
        return Ok(());
    }
    close_in(conn, ws, message_id, "sent", Some(draft_id)).await
}

/// A mail we did not send ourselves went out from the mailbox on this conversation: a person answered. Called when sent mail syncs in.
pub async fn note_outbound(conn: &mut SqliteConnection, ws: &str, thread_id: &str, rfc_message_id: Option<&str>, at: i64) -> sqlx::Result<()> {
    let ours: bool = match rfc_message_id {
        Some(id) => sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM drafts WHERE rfc_message_id = ?1)").bind(id).fetch_one(&mut *conn).await?,
        None => false,
    };
    if ours {
        return Ok(());
    }
    let open: Vec<String> = sqlx::query_scalar(
        "SELECT o.message_id FROM reply_obligations o JOIN email_messages m ON m.id = o.message_id
         WHERE o.workspace_id = ?1 AND o.thread_id = ?2 AND o.status IN ('open', 'holding_sent', 'escalated') AND m.received_at <= ?3",
    )
    .bind(ws)
    .bind(thread_id)
    .bind(at)
    .fetch_all(&mut *conn)
    .await?;
    for message_id in open {
        close_in(conn, ws, &message_id, "answered_by_person", None).await?;
    }
    Ok(())
}

/// Runs every due stage that has no timer left (a lost job, a restart). Returns how many stages ran.
pub async fn sweep(state: &AppState, ws: &str) -> sqlx::Result<usize> {
    if !flags::mode(&state.db, &state.config, ws, Key::Handoff).await?.acts() {
        return Ok(0);
    }
    let asked = crate::hr::team::info_requests::sweep(state, ws).await?;
    let now = now_ms();
    let rows: Vec<Duty> = sqlx::query_as(&format!("SELECT {DUTY_COLUMNS} FROM reply_obligations WHERE workspace_id = ?1 AND status IN ('open', 'holding_sent') ORDER BY due_at"))
        .bind(ws)
        .fetch_all(&state.db)
        .await?;
    let mut ran = asked;
    for d in rows {
        // A mail can owe several stages at once (the warning and the note); run them in order.
        let due = [
            (Stage::Warn, d.warned_at.is_none() && d.status == "open" && d.warn_at <= now),
            (Stage::Hold, d.held_at.is_none() && d.status == "open" && d.due_at <= now),
            (Stage::Escalate, d.escalated_at.is_none() && d.final_due_at.is_some_and(|at| at <= now)),
        ];
        for (stage, is_due) in due {
            if is_due {
                run_stage(state, ws, &d.message_id, stage).await?;
                ran += 1;
            }
        }
    }
    Ok(ran)
}

/// The `REPLY_SLA_SWEEP` job.
pub async fn sweep_job(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let ws = payload.get("workspace_id").and_then(Value::as_str).ok_or_else(|| JobError::Permanent("payload missing workspace_id".into()))?;
    sweep(state, ws).await?;
    Ok(())
}

/// Hourly (from the worker's maintenance pass): one sweep per workspace that has a duty still running.
pub async fn schedule(state: &AppState) -> sqlx::Result<()> {
    let bucket = now_ms() / SWEEP_EVERY_MS;
    let workspaces: Vec<String> = sqlx::query_scalar("SELECT workspace_id FROM reply_obligations WHERE status IN ('open', 'holding_sent') UNION SELECT workspace_id FROM info_requests WHERE state = 'waiting'").fetch_all(&state.db).await?;
    let mut conn = state.db.acquire().await?;
    for ws in workspaces {
        jobs::enqueue_with_attempts(&mut conn, kind::REPLY_SLA_SWEEP, &format!("sla_sweep:{ws}:{bucket}"), Some(&ws), &json!({"workspace_id": ws}), 3).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hr::team::clock::{DAY_MS, parse_date};

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|i| (*i).to_string()).collect()
    }

    fn mail() -> Facts {
        Facts { inbound: true, sender: "ravi@client.test".into(), subject: "Re: timesheet".into(), body: "my id is EMP-1088".into(), ..Default::default() }
    }

    #[test]
    fn deadline_is_4_working_hours() {
        let zone = time::resolve(None, Some("Asia/Kolkata"), 0);
        let week = time::WorkWeek::standard();
        // Friday 16 Oct 2026, 16:00 in Kolkata: three working hours that day, one more on Saturday morning.
        let at = parse_date("2026-10-16").unwrap() * DAY_MS + (16 * 60 - 330) * 60_000;
        let d = deadlines(at, &zone, &week, 4);
        assert_eq!(d.warn, at + 2 * HOUR_MS, "half of it");
        assert_eq!(d.due, parse_date("2026-10-17").unwrap() * DAY_MS + (10 * 60 - 330) * 60_000);
    }

    #[test]
    fn a_reply_is_owed_unless_one_of_the_reasons_applies() {
        assert_eq!(excuse_reason(&mail()), None);
        assert_eq!(excuse_reason(&Facts { signals: s(&["postroom"]), ..mail() }), Some("own_mail"));
        assert_eq!(excuse_reason(&Facts { inbound: false, ..mail() }), Some("not_inbound"));
        assert_eq!(excuse_reason(&Facts { suppressed: true, ..mail() }), Some("held_back"));
        assert_eq!(excuse_reason(&Facts { own_addresses: s(&["Ravi <ravi@client.test>"]), ..mail() }), Some("own_address"));
        assert_eq!(excuse_reason(&Facts { signals: s(&["list_unsubscribe"]), ..mail() }), Some("bulk"));
        assert_eq!(excuse_reason(&Facts { sender: "noreply@client.test".into(), ..mail() }), Some("machine_sender"));
        assert_eq!(excuse_reason(&Facts { auto_kind: Some("ooo".into()), ..mail() }), Some("auto_reply"));
        assert_eq!(excuse_reason(&Facts { subject: "Automatic reply: away".into(), ..mail() }), Some("auto_reply"));
        assert_eq!(excuse_reason(&Facts { auto_kind: Some("dsn".into()), ..mail() }), Some("bounce"));
        assert_eq!(excuse_reason(&Facts { no_reply: s(&["@client.test"]), ..mail() }), Some("owner_no_reply"));
        assert_eq!(excuse_reason(&Facts { answered_by_person: true, ..mail() }), Some("answered_by_person"));
    }

    #[test]
    fn a_thank_you_after_our_ack_is_excused_but_only_after_an_ack() {
        let thanks = Facts { body: "ok thanks\n\nOn Mon, 5 Oct 2026 Hema wrote:\n> Thank you. We recorded it.".into(), ..mail() };
        assert_eq!(excuse_reason(&thanks), None, "we did not just send an ack");
        assert_eq!(excuse_reason(&Facts { last_ours_was_ack: true, ..thanks.clone() }), Some("closer_after_ack"));
        for real in ["ok thanks, but the hours look wrong?", "thanks, here is the corrected sheet", "ok", "thank you so much for the quick help with everything today"] {
            let f = Facts { last_ours_was_ack: true, body: real.into(), ..mail() };
            assert_eq!(excuse_reason(&f).is_some(), real == "ok", "{real}");
        }
        assert_eq!(excuse_reason(&Facts { last_ours_was_ack: true, has_attachments: true, body: "thanks".into(), ..mail() }), None, "a file is real mail");
        assert!(is_closer("theek hai ji", false) && is_closer("👍", false) && !is_closer("", false));
    }

    #[test]
    fn the_owners_list_matches_an_address_or_a_whole_domain() {
        assert!(listed("Ravi <Ravi@Client.test>", &s(&["ravi@client.test"])));
        assert!(listed("x@client.test", &s(&["@client.test"])));
        assert!(!listed("x@other.test", &s(&["@client.test", "y@client.test"])));
    }

    #[test]
    fn stages_are_named_in_the_job_key() {
        for st in [Stage::Warn, Stage::Hold, Stage::Escalate] {
            assert_eq!(Stage::parse(st.as_str()), Some(st));
        }
        assert_eq!(Stage::parse("hold!"), None);
    }
}
