//! The missing-data loop. A sheet that lacks what the owner requires gets one question to its sender (Meera writes it, in the thread,
//! through the same gate as every reply): exactly the missing items, numbered. The question is remembered here, with the Message-ID it
//! went out with, so that a reply in the thread is recognised as the answer. One reminder if nobody answers, then a person is asked
//! and nothing more is mailed. Nobody is mailed more than three times (the ask, a re-ask for what is still missing, a reminder).
//!
//! States: `asking` (filed, not yet sent) → `waiting` (sent) → `applying` (an answer is being read) → `waiting_owner` (a proposed
//! profile change needs the owner) → `done`; or `escalated` (no answer) or `cancelled`. `apply` holds the answer side.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{SqliteConnection, SqlitePool};

use crate::{
    agents::handoff::{self, Asker, Fact, Field, IssueText, MISSING_DATA, MessageKind, MessageRequest},
    alerts,
    hr::team::settings,
    http::AppState,
    jobs::{self, handlers::JobError, kind},
    platform::{
        audit::{self, Actor, Entry},
        db::{new_id, now_ms},
        flags::{self, Key},
    },
};

pub mod apply;

/// Most mails one loop sends to the sender: the ask, a re-ask for what is still missing, one reminder.
pub const MAX_MAILS: i64 = 3;
/// The job key of a reminder's request: its own row beside the ask.
pub const REMINDER_JOB: &str = "desk:timesheet:reminder";

/// One thing asked, and what became of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Asked {
    pub key: String,
    pub label: String,
    /// `asked`, `answered` (a value was taken from a reply and applied), `pending_owner` (waits for the owner), `declined`, `conflict`.
    pub status: String,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub from_message_id: Option<String>,
    #[serde(default)]
    pub from_class: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    /// For a change that waits for the owner: what it would do (`{person_id, field, old, new}`).
    #[serde(default)]
    pub proposal: Option<Value>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct InfoRequest {
    pub id: String,
    pub thread_id: String,
    pub origin_message_id: String,
    pub asked_to: String,
    pub asked_by_agent_id: Option<String>,
    pub fields_json: String,
    pub state: String,
    pub round: i64,
    pub reminders: i64,
    pub mails_sent: i64,
    pub next_at: Option<i64>,
    pub ask_rfc_ids_json: String,
}

pub const COLUMNS: &str = "id, thread_id, origin_message_id, asked_to, asked_by_agent_id, fields_json, state, round, reminders, mails_sent, next_at, ask_rfc_ids_json";

impl InfoRequest {
    pub fn fields(&self) -> Vec<Asked> {
        serde_json::from_str(&self.fields_json).unwrap_or_default()
    }

    /// The things still asked, as fields.
    pub fn open_fields(&self) -> Vec<Field> {
        self.fields().iter().filter(|f| f.status == "asked").filter_map(|f| Field::parse(&f.key)).collect()
    }

    pub fn ask_rfc_ids(&self) -> Vec<String> {
        serde_json::from_str(&self.ask_rfc_ids_json).unwrap_or_default()
    }

    pub fn is_open(&self) -> bool {
        matches!(self.state.as_str(), "asking" | "waiting" | "applying" | "waiting_owner")
    }
}

/// The request that is still going (or the latest finished one) for a sheet email.
pub async fn latest_for(db: &SqlitePool, ws: &str, origin_message_id: &str) -> sqlx::Result<Option<InfoRequest>> {
    sqlx::query_as(&format!("SELECT {COLUMNS} FROM info_requests WHERE workspace_id = ?1 AND origin_message_id = ?2 ORDER BY created_at DESC, rowid DESC LIMIT 1"))
        .bind(ws)
        .bind(origin_message_id)
        .fetch_optional(db)
        .await
}

pub async fn get(db: &SqlitePool, ws: &str, id: &str) -> sqlx::Result<Option<InfoRequest>> {
    sqlx::query_as(&format!("SELECT {COLUMNS} FROM info_requests WHERE workspace_id = ?1 AND id = ?2")).bind(ws).bind(id).fetch_optional(db).await
}

async fn audit_request(conn: &mut SqliteConnection, ws: &str, id: &str, message_id: &str, action: &str, metadata: Value) -> sqlx::Result<()> {
    audit::record(conn, Entry { workspace_id: ws, actor: Actor::System, action, entity_type: "info_request", entity_id: id, message_id: Some(message_id), metadata }).await
}

/// May the desk file a new ask for this sheet? Not while a loop is going (it is filed already, or an answer or the owner is being waited for)
/// and never after one ended without an answer: asking again and again is how a loop starts.
pub async fn may_ask(db: &SqlitePool, ws: &str, origin_message_id: &str) -> sqlx::Result<bool> {
    Ok(match latest_for(db, ws, origin_message_id).await? {
        None => true,
        Some(r) => matches!(r.state.as_str(), "asking" | "waiting"),
    })
}

/// Remembers (or updates) what is asked of the sender of a sheet email. The sheets of the email learn what they wait for.
pub async fn upsert_asks(state: &AppState, ws: &str, origin_message_id: &str, agent_id: &str, asks: &[Field]) -> sqlx::Result<String> {
    let (thread_id, sender): (String, String) = sqlx::query_as("SELECT thread_id, sender_address FROM email_messages WHERE id = ?1 AND workspace_id = ?2").bind(origin_message_id).bind(ws).fetch_one(&state.db).await?;
    let asked_to = crate::replies::send_safety::bare_address(&sender);
    let now = now_ms();
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let open: Option<InfoRequest> = sqlx::query_as(&format!("SELECT {COLUMNS} FROM info_requests WHERE workspace_id = ?1 AND origin_message_id = ?2 AND state IN ('asking', 'waiting') LIMIT 1"))
        .bind(ws)
        .bind(origin_message_id)
        .fetch_optional(&mut *tx)
        .await?;
    let id = match open {
        Some(r) => {
            let mut fields = r.fields();
            fields.retain(|f| f.status != "asked" || asks.iter().any(|a| a.key() == f.key));
            for a in asks {
                if !fields.iter().any(|f| f.key == a.key() && f.status == "asked") {
                    fields.push(Asked { key: a.key().into(), label: a.label().into(), status: "asked".into(), value: None, from_message_id: None, from_class: None, source: None, proposal: None });
                }
            }
            sqlx::query("UPDATE info_requests SET fields_json = ?2, updated_at = ?3 WHERE id = ?1").bind(&r.id).bind(json!(fields).to_string()).bind(now).execute(&mut *tx).await?;
            r.id
        }
        None => {
            let id = new_id("inf");
            let fields: Vec<Asked> = asks.iter().map(|a| Asked { key: a.key().into(), label: a.label().into(), status: "asked".into(), value: None, from_message_id: None, from_class: None, source: None, proposal: None }).collect();
            sqlx::query(
                "INSERT INTO info_requests (id, workspace_id, thread_id, origin_message_id, asked_to, asked_by_agent_id, fields_json, state, round, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'asking', 1, ?8, ?8)",
            )
            .bind(&id)
            .bind(ws)
            .bind(&thread_id)
            .bind(origin_message_id)
            .bind(&asked_to)
            .bind(agent_id)
            .bind(json!(fields).to_string())
            .bind(now)
            .execute(&mut *tx)
            .await?;
            audit_request(&mut tx, ws, &id, origin_message_id, "info_request.opened", json!({"asks": asks.iter().map(|a| a.key()).collect::<Vec<_>>()})).await?;
            id
        }
    };
    sqlx::query("UPDATE timesheet_records SET info_request_id = ?1 WHERE workspace_id = ?2 AND message_id = ?3 AND status != 'replaced'").bind(&id).bind(ws).bind(origin_message_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(id)
}

/// The decision no longer asks for anything (the sheet was corrected, or the owner stopped requiring it): a loop that is only asking or
/// waiting is closed as complete.
pub async fn settle_complete(state: &AppState, ws: &str, origin_message_id: &str) -> sqlx::Result<()> {
    let closed: Vec<String> = sqlx::query_scalar(
        "UPDATE info_requests SET state = 'done', closed_reason = 'complete', updated_at = ?3 WHERE workspace_id = ?1 AND origin_message_id = ?2 AND state IN ('asking', 'waiting') RETURNING id",
    )
    .bind(ws)
    .bind(origin_message_id)
    .bind(now_ms())
    .fetch_all(&state.db)
    .await?;
    let mut conn = state.db.acquire().await?;
    for id in closed {
        audit_request(&mut conn, ws, &id, origin_message_id, "info_request.closed", json!({"reason": "complete"})).await?;
    }
    Ok(())
}

/// A mail went out for this draft (called in the send's own transaction). If it was an ask or a reminder, the loop waits for the answer
/// and remembers the Message-ID the mail carries.
pub async fn on_sent(conn: &mut SqliteConnection, ws: &str, message_id: &str, draft_id: &str) -> sqlx::Result<()> {
    let sent: Option<(String,)> = sqlx::query_as(
        "SELECT r.facts_json FROM message_requests r WHERE r.workspace_id = ?1 AND r.draft_id = ?2 AND r.status = 'sent' AND r.kind = 'question' AND r.issues_json LIKE '%\"missing_data\"%' LIMIT 1",
    )
    .bind(ws)
    .bind(draft_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((facts,)) = sent else { return Ok(()) };
    let reminder = facts.contains("\"Reminder\"");
    let thread_id: String = sqlx::query_scalar("SELECT thread_id FROM email_messages WHERE id = ?1").bind(message_id).fetch_one(&mut *conn).await?;
    let rfc: Option<String> = sqlx::query_scalar("SELECT rfc_message_id FROM drafts WHERE id = ?1").bind(draft_id).fetch_one(&mut *conn).await?;
    let Some(row): Option<InfoRequest> = sqlx::query_as(&format!("SELECT {COLUMNS} FROM info_requests WHERE workspace_id = ?1 AND thread_id = ?2 AND state IN ('asking', 'waiting') ORDER BY created_at DESC LIMIT 1"))
        .bind(ws)
        .bind(&thread_id)
        .fetch_optional(&mut *conn)
        .await?
    else {
        return Ok(());
    };
    let (reminder_hours, escalate_hours): (i64, i64) = sqlx::query_as("SELECT missing_reminder_hours, missing_escalate_hours FROM team_settings WHERE workspace_id = ?1").bind(ws).fetch_optional(&mut *conn).await?.unwrap_or((24, 48));
    let mut ids = row.ask_rfc_ids();
    ids.extend(rfc);
    let next_at = now_ms() + if reminder { escalate_hours } else { reminder_hours } * 3_600_000;
    sqlx::query("UPDATE info_requests SET state = 'waiting', mails_sent = mails_sent + 1, ask_rfc_ids_json = ?2, next_at = ?3, updated_at = ?4 WHERE id = ?1")
        .bind(&row.id)
        .bind(json!(ids).to_string())
        .bind(next_at)
        .bind(now_ms())
        .execute(&mut *conn)
        .await?;
    enqueue_due(conn, ws, &row.id, row.mails_sent + 1, next_at).await?;
    audit_request(conn, ws, &row.id, &row.origin_message_id, "info_request.asked", json!({"mails_sent": row.mails_sent + 1, "reminder": reminder})).await
}

async fn enqueue_due(conn: &mut SqliteConnection, ws: &str, id: &str, n: i64, at: i64) -> sqlx::Result<()> {
    jobs::enqueue_at(conn, kind::INFO_REQUEST_DUE, &format!("info:{id}:{n}"), Some(ws), &json!({"workspace_id": ws, "info_request_id": id}), jobs::DEFAULT_MAX_ATTEMPTS, at).await?;
    Ok(())
}

/// The `INFO_REQUEST_DUE` job.
pub async fn due_job(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let field = |k: &str| payload.get(k).and_then(Value::as_str);
    let (Some(ws), Some(id)) = (field("workspace_id"), field("info_request_id")) else { return Err(JobError::Permanent("payload missing workspace_id or info_request_id".into())) };
    run_due(state, ws, id).await?;
    Ok(())
}

/// Nobody answered in time: one reminder (when the owner allows it and fewer than three mails went out), else a person is asked.
pub async fn run_due(state: &AppState, ws: &str, id: &str) -> sqlx::Result<()> {
    if !flags::mode(&state.db, &state.config, ws, Key::Handoff).await?.acts() {
        return Ok(());
    }
    let Some(r) = get(&state.db, ws, id).await? else { return Ok(()) };
    if r.state != "waiting" {
        return Ok(());
    }
    let cfg = settings::load(&state.db, ws).await?;
    let now = now_ms();
    let claimed = sqlx::query("UPDATE info_requests SET state = 'applying', updated_at = ?2 WHERE id = ?1 AND state = 'waiting'").bind(id).bind(now).execute(&state.db).await?.rows_affected() == 1;
    if !claimed {
        return Ok(());
    }
    let open = r.open_fields();
    if r.reminders < cfg.missing_max_reminders && r.mails_sent < MAX_MAILS && !open.is_empty() && r.asked_by_agent_id.is_some() {
        let agent = r.asked_by_agent_id.clone().unwrap_or_default();
        let mut facts: Vec<Fact> = open.iter().map(|f| Fact { label: "Ask".into(), value: f.label().into() }).collect();
        facts.push(Fact { label: "Reminder".into(), value: "yes".into() });
        let request = MessageRequest::template(MessageKind::Question, facts, vec![IssueText { code: MISSING_DATA.into(), text: "A reminder: we still need these details".into() }]);
        let filed = handoff::hand_to_meera(state, ws, &Asker { agent_id: &agent, case_id: None, job: REMINDER_JOB }, &r.origin_message_id, &request).await?;
        let next_at = now + cfg.missing_escalate_hours * 3_600_000;
        sqlx::query("UPDATE info_requests SET state = 'waiting', reminders = reminders + 1, next_at = ?2, updated_at = ?3 WHERE id = ?1").bind(id).bind(next_at).bind(now).execute(&state.db).await?;
        let mut conn = state.db.acquire().await?;
        enqueue_due(&mut conn, ws, id, r.mails_sent + 100, next_at).await?;
        return audit_request(&mut conn, ws, id, &r.origin_message_id, "info_request.reminded", json!({"filed": format!("{filed:?}")})).await;
    }
    escalate(state, ws, &r, "no answer").await
}

/// A person takes over: the loop ends, a review item says what is missing, and nothing more is mailed to the sender.
pub async fn escalate(state: &AppState, ws: &str, r: &InfoRequest, reason: &str) -> sqlx::Result<()> {
    let now = now_ms();
    sqlx::query("UPDATE info_requests SET state = 'escalated', closed_reason = ?2, next_at = NULL, updated_at = ?3 WHERE id = ?1").bind(&r.id).bind(reason).bind(now).execute(&state.db).await?;
    let who: Option<String> = sqlx::query_scalar("SELECT sender_name FROM email_messages WHERE id = ?1").bind(&r.origin_message_id).fetch_optional(&state.db).await?.flatten();
    let who = who.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| r.asked_to.clone());
    let missing = r.open_fields().iter().map(|f| f.short()).collect::<Vec<_>>().join(", ");
    let title = format!("{who} did not answer: {missing}");
    sqlx::query(
        "INSERT INTO review_items (id, workspace_id, message_id, workflow_run_id, kind, title, status, created_at)
         SELECT ?1, ?2, ?3, NULL, 'agent_task', ?4, 'open', ?5 WHERE NOT EXISTS (SELECT 1 FROM review_items WHERE message_id = ?3 AND status = 'open' AND title = ?4)",
    )
    .bind(new_id("rev"))
    .bind(ws)
    .bind(&r.origin_message_id)
    .bind(&title)
    .bind(now)
    .execute(&state.db)
    .await?;
    alerts::raise(
        &state.db,
        alerts::NewAlert { workspace_id: ws, kind: alerts::AUTOMATION, severity: "high", title: title.clone(), body: format!("We asked {} for these details and have stopped asking. Please get them another way.", r.asked_to), message_id: Some(&r.origin_message_id), level: 2, dedupe_key: Some(format!("info_escalate:{}", r.id)) },
    )
    .await?;
    let mut conn = state.db.acquire().await?;
    audit_request(&mut conn, ws, &r.id, &r.origin_message_id, "info_request.escalated", json!({"reason": reason, "missing": missing})).await
}

/// Every waiting loop whose time has come (a lost timer, a restart). Returns how many were run.
pub async fn sweep(state: &AppState, ws: &str) -> sqlx::Result<usize> {
    let due: Vec<String> = sqlx::query_scalar("SELECT id FROM info_requests WHERE workspace_id = ?1 AND state = 'waiting' AND next_at IS NOT NULL AND next_at <= ?2").bind(ws).bind(now_ms()).fetch_all(&state.db).await?;
    for id in &due {
        run_due(state, ws, id).await?;
    }
    Ok(due.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_asked_fields_are_open() {
        let fields = vec![
            Asked { key: "name".into(), label: "x".into(), status: "answered".into(), value: Some("Ravi".into()), from_message_id: None, from_class: None, source: None, proposal: None },
            Asked { key: "employee_code".into(), label: "y".into(), status: "asked".into(), value: None, from_message_id: None, from_class: None, source: None, proposal: None },
        ];
        let r = InfoRequest { id: "i".into(), thread_id: "t".into(), origin_message_id: "m".into(), asked_to: "a@x".into(), asked_by_agent_id: None, fields_json: json!(fields).to_string(), state: "waiting".into(), round: 1, reminders: 0, mails_sent: 1, next_at: None, ask_rfc_ids_json: "[\"<a@b>\"]".into() };
        assert_eq!(r.open_fields(), [Field::EmployeeCode]);
        assert_eq!(r.ask_rfc_ids(), ["<a@b>"]);
        assert!(r.is_open());
    }
}
