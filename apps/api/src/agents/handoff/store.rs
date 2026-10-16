//! Requests an expert filed for Meera, and the filing itself. One row per (message, asking agent, job): filing the same
//! request again changes nothing, a changed request replaces the unsent row, and a request filed after its reply went out
//! is noted (`handoff.late`) and never sent twice. Reading is always scoped by workspace.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{SqliteConnection, SqlitePool};

use super::{Fact, IssueText, MessageKind, MessageRequest, REPLY_JOB, Recipient, no_reply_reason, recipient_refusal};
use crate::{
    agents::{
        case::{self, NewCase},
        registry::{runtime, store as agents},
    },
    http::AppState,
    platform::{
        audit::{self, Actor, Entry},
        db::{new_id, now_ms},
    },
};

/// The agent filing the request, and the case it is working in.
#[derive(Debug, Clone, Copy)]
pub struct Asker<'a> {
    pub agent_id: &'a str,
    pub case_id: Option<&'a str>,
    pub job: &'a str,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Stored {
    pub id: String,
    pub message_id: String,
    pub from_agent_id: String,
    pub job: String,
    pub kind: String,
    pub recipient_json: String,
    pub facts_json: String,
    pub issues_json: String,
    pub instructions: Option<String>,
    pub body: Option<String>,
    pub requester_level: i64,
    pub status: String,
    pub draft_id: Option<String>,
    pub reply_case_id: Option<String>,
}

impl Stored {
    pub fn kind(&self) -> MessageKind {
        MessageKind::parse(&self.kind).unwrap_or(MessageKind::Holding)
    }

    pub fn recipient(&self) -> Recipient {
        serde_json::from_str(&self.recipient_json).unwrap_or(Recipient::Sender)
    }

    pub fn facts(&self) -> Vec<Fact> {
        serde_json::from_str(&self.facts_json).unwrap_or_default()
    }

    pub fn issues(&self) -> Vec<IssueText> {
        serde_json::from_str(&self.issues_json).unwrap_or_default()
    }
}

const COLUMNS: &str = "id, message_id, from_agent_id, job, kind, recipient_json, facts_json, issues_json, instructions, body, requester_level, status, draft_id, reply_case_id";

/// What filing did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Filed {
    /// A new request; Meera's reply case is `case_id`.
    Requested { case_id: Option<String> },
    /// The request changed while its reply was unsent: the row was replaced and the unsent draft rewritten.
    Replaced { case_id: Option<String> },
    /// The same request was filed before; nothing changed.
    Unchanged,
    /// The reply already went out; the changed request is only recorded.
    Late,
    /// Nobody is written to (our own mail, a no-reply sender, a recipient that is not allowed).
    Dropped(&'static str),
}

fn digest(req: &MessageRequest) -> String {
    let material = json!({"to": req.to, "kind": req.kind, "facts": req.facts, "issues": req.issues, "instructions": req.instructions, "body": req.body, "never_do": req.never_do});
    Sha256::digest(material.to_string().as_bytes()).iter().take(12).map(|b| format!("{b:02x}")).collect()
}

#[derive(sqlx::FromRow)]
struct MessageRow {
    sender_address: String,
    bulk_signals: String,
    direction: String,
    thread_id: String,
    subject: String,
}

async fn audit_request(conn: &mut SqliteConnection, ws: &str, id: &str, message_id: &str, action: &str, metadata: Value) -> sqlx::Result<()> {
    audit::record(conn, Entry { workspace_id: ws, actor: Actor::System, action, entity_type: "message_request", entity_id: id, message_id: Some(message_id), metadata }).await
}

/// Files `req` for Meera and makes sure her reply case exists. Safe to call again with the same request.
pub async fn hand_to_meera(state: &AppState, ws: &str, asker: &Asker<'_>, message_id: &str, req: &MessageRequest) -> sqlx::Result<Filed> {
    let db = &state.db;
    let Some(m) = sqlx::query_as::<_, MessageRow>(
        "SELECT sender_address, bulk_signals, direction, thread_id, subject FROM email_messages WHERE id = ?1 AND workspace_id = ?2",
    )
    .bind(message_id)
    .bind(ws)
    .fetch_optional(db)
    .await?
    else {
        return Ok(Filed::Dropped("the email is gone"));
    };
    let signals: Vec<String> = serde_json::from_str(&m.bulk_signals).unwrap_or_default();
    let thread_senders: Vec<String> = sqlx::query_scalar("SELECT DISTINCT lower(sender_address) FROM email_messages WHERE workspace_id = ?1 AND thread_id = ?2 AND direction = 'inbound'")
        .bind(ws)
        .bind(&m.thread_id)
        .fetch_all(db)
        .await?;
    let own: Vec<String> = sqlx::query_scalar(
        "SELECT lower(s.mailbox_address) FROM email_messages m JOIN email_sources s ON s.id = m.source_id WHERE m.id = ?1 AND s.mailbox_address IS NOT NULL
         UNION SELECT lower(u.email) FROM email_messages m JOIN email_sources s ON s.id = m.source_id JOIN users u ON u.id = s.connected_by_user_id WHERE m.id = ?1",
    )
    .bind(message_id)
    .fetch_all(db)
    .await?;
    let refusal = if m.direction != "inbound" {
        Some("it is not an email we received")
    } else {
        no_reply_reason(&m.sender_address, &signals, &own)
            .or_else(|| recipient_refusal(&req.to, &thread_senders, &[]))
            .or((req.to != Recipient::Sender).then_some("only the sender is written to for now"))
    };
    let level = runtime::level_of(db, &state.config, ws, asker.agent_id, message_id).await?;
    let digest = digest(req);
    let now = now_ms();
    let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
    let existing: Option<(String, String, String)> =
        sqlx::query_as("SELECT id, digest, status FROM message_requests WHERE workspace_id = ?1 AND message_id = ?2 AND from_agent_id = ?3 AND job = ?4")
            .bind(ws)
            .bind(message_id)
            .bind(asker.agent_id)
            .bind(asker.job)
            .fetch_optional(&mut *tx)
            .await?;
    let status = if refusal.is_some() { "dropped" } else { "pending" };
    let (id, outcome) = match existing {
        Some((id, old, _)) if old == digest => {
            tx.commit().await?;
            return finish(state, ws, message_id, &id, Filed::Unchanged, refusal).await;
        }
        Some((id, _, was)) => {
            sqlx::query(
                "UPDATE message_requests SET from_case_id = ?2, kind = ?3, recipient_json = ?4, facts_json = ?5, issues_json = ?6, instructions = ?7, never_do_json = ?8,
                        body = ?9, requester_level = ?10, digest = ?11, status = CASE WHEN status = 'sent' THEN status ELSE ?12 END, status_note = ?13, updated_at = ?14
                 WHERE id = ?1",
            )
            .bind(&id)
            .bind(asker.case_id)
            .bind(req.kind.as_str())
            .bind(json!(req.to).to_string())
            .bind(json!(req.facts).to_string())
            .bind(json!(req.issues).to_string())
            .bind(&req.instructions)
            .bind(json!(req.never_do).to_string())
            .bind(&req.body)
            .bind(level)
            .bind(&digest)
            .bind(status)
            .bind(refusal)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            let late = was == "sent";
            let action = if late { "handoff.late" } else if refusal.is_some() { "handoff.dropped" } else { "handoff.replaced" };
            audit_request(&mut tx, ws, &id, message_id, action, json!({"kind": req.kind, "was": was, "reason": refusal, "from_agent_id": asker.agent_id, "job": asker.job})).await?;
            (id, if late { Filed::Late } else { Filed::Replaced { case_id: None } })
        }
        None => {
            let id = new_id("mrq");
            sqlx::query(
                "INSERT INTO message_requests (id, workspace_id, message_id, from_agent_id, job, from_case_id, kind, recipient_json, facts_json, issues_json, instructions,
                                               never_do_json, body, requester_level, digest, status, status_note, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?18)",
            )
            .bind(&id)
            .bind(ws)
            .bind(message_id)
            .bind(asker.agent_id)
            .bind(asker.job)
            .bind(asker.case_id)
            .bind(req.kind.as_str())
            .bind(json!(req.to).to_string())
            .bind(json!(req.facts).to_string())
            .bind(json!(req.issues).to_string())
            .bind(&req.instructions)
            .bind(json!(req.never_do).to_string())
            .bind(&req.body)
            .bind(level)
            .bind(&digest)
            .bind(status)
            .bind(refusal)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            let action = if refusal.is_some() { "handoff.dropped" } else { "handoff.requested" };
            audit_request(&mut tx, ws, &id, message_id, action, json!({"kind": req.kind, "reason": refusal, "from_agent_id": asker.agent_id, "job": asker.job, "level": level})).await?;
            (id, Filed::Requested { case_id: None })
        }
    };
    tx.commit().await?;
    finish(state, ws, message_id, &id, outcome, refusal).await
}

/// After filing: a dropped request ends here; otherwise Meera's reply case exists, and a changed request whose reply was
/// already written is merged into that unsent reply again.
async fn finish(state: &AppState, ws: &str, message_id: &str, id: &str, outcome: Filed, refusal: Option<&'static str>) -> sqlx::Result<Filed> {
    if let Some(why) = refusal {
        return Ok(Filed::Dropped(why));
    }
    if outcome == Filed::Late {
        return Ok(Filed::Late);
    }
    let case_id = ensure_reply_case(state, ws, message_id, id).await?;
    match outcome {
        Filed::Requested { .. } => Ok(Filed::Requested { case_id }),
        Filed::Replaced { .. } => {
            let composed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM message_requests WHERE workspace_id = ?1 AND message_id = ?2 AND draft_id IS NOT NULL AND status IN ('pending', 'composed'))")
                .bind(ws)
                .bind(message_id)
                .fetch_one(&state.db)
                .await?;
            if composed {
                crate::replies::compose::refresh(state, ws, message_id).await?;
            }
            Ok(Filed::Replaced { case_id })
        }
        other => Ok(other),
    }
}

/// Meera's reply case for this message: her personal agent when the mail arrived in a member's own mailbox, else the lead.
/// One case per message; `None` when no Meera is active (the request then waits and is audited).
async fn ensure_reply_case(state: &AppState, ws: &str, message_id: &str, request_id: &str) -> sqlx::Result<Option<String>> {
    let ctx = crate::agents::profiles::context_for_message(&state.db, &state.config, ws, message_id).await?;
    let personal = match ctx.owner.as_ref() {
        Some(o) => agents::personal_of(&state.db, ws, &o.user_id).await?.filter(|a| a.status == "active"),
        None => None,
    };
    let meera = match personal {
        Some(p) => Some(p),
        None => agents::lead(&state.db, ws).await?.filter(|a| a.status == "active"),
    };
    let Some(meera) = meera else {
        let mut conn = state.db.acquire().await?;
        audit_request(&mut conn, ws, request_id, message_id, "handoff.waiting", json!({"reason": "no active Meera to write the reply"})).await?;
        return Ok(None);
    };
    let Some(m) = sqlx::query_as::<_, MessageRow>("SELECT sender_address, bulk_signals, direction, thread_id, subject FROM email_messages WHERE id = ?1 AND workspace_id = ?2")
        .bind(message_id)
        .bind(ws)
        .fetch_optional(&state.db)
        .await?
    else {
        return Ok(None);
    };
    let title = if m.subject.trim().is_empty() { "(no subject)".to_string() } else { m.subject.clone() };
    let job = reply_job_for(&state.db, ws, message_id).await?;
    let mut conn = state.db.acquire().await?;
    let (case_id, _) = case::create(
        &mut conn,
        &state.config,
        NewCase {
            workspace_id: ws,
            kind: "inbox",
            title,
            goal: format!("Write and send the reply the team asked for to {}", m.sender_address),
            priority: "normal".into(),
            message_id: Some(message_id),
            requirement_id: None,
            thread_id: Some(&m.thread_id),
            state: json!({"from": m.sender_address, "subject": m.subject, "reply": true}),
            created_by: None,
            agent_user_id: ctx.owner.as_ref().map(|o| o.user_id.as_str()),
            agent_id: Some(&meera.id),
            agent_version: Some(meera.active_version.unwrap_or(0)),
            trigger: Some("handoff"),
            shadow: false,
            job: &job,
        },
    )
    .await?;
    sqlx::query("UPDATE message_requests SET reply_case_id = ?1 WHERE workspace_id = ?2 AND message_id = ?3 AND status IN ('pending', 'composed') AND reply_case_id IS NULL")
        .bind(&case_id)
        .bind(ws)
        .bind(message_id)
        .execute(&mut *conn)
        .await?;
    drop(conn);
    state.jobs_notify.notify_one();
    Ok(Some(case_id))
}

/// The job key of the reply case to use for a message: the first one is `reply`; once it is done (a first mail went out) and
/// a request still has no words written, a follow-up case `reply#N` writes the next mail. A reply case that was cancelled or
/// failed is never reopened this way.
async fn reply_job_for(db: &SqlitePool, ws: &str, message_id: &str) -> sqlx::Result<String> {
    let cases: Vec<(String, String)> = sqlx::query_as(
        "SELECT job, status FROM cases WHERE workspace_id = ?1 AND message_id = ?2 AND shadow = 0 AND (job = ?3 OR job LIKE 'reply#%') ORDER BY created_at DESC, rowid DESC",
    )
    .bind(ws)
    .bind(message_id)
    .bind(REPLY_JOB)
    .fetch_all(db)
    .await?;
    let Some((latest, status)) = cases.first() else { return Ok(REPLY_JOB.to_string()) };
    if status == "done" && open_for_message(db, ws, message_id).await?.iter().any(|r| r.status == "pending") {
        return Ok(format!("reply#{}", cases.len() + 1));
    }
    Ok(latest.clone())
}

/// The requests Meera still has to write for a message, oldest first.
pub async fn open_for_message(db: &SqlitePool, ws: &str, message_id: &str) -> sqlx::Result<Vec<Stored>> {
    sqlx::query_as(&format!("SELECT {COLUMNS} FROM message_requests WHERE workspace_id = ?1 AND message_id = ?2 AND status IN ('pending', 'composed') ORDER BY created_at, rowid"))
        .bind(ws)
        .bind(message_id)
        .fetch_all(db)
        .await
}

/// The level the reply of this message runs at: the lowest captured level of its live requests (template words).
/// `None` when nothing was asked, so the agent's own level stands.
pub async fn governing_for_message(db: &SqlitePool, ws: &str, message_id: Option<&str>) -> sqlx::Result<Option<i64>> {
    let Some(message_id) = message_id else { return Ok(None) };
    let levels: Vec<i64> = sqlx::query_scalar("SELECT requester_level FROM message_requests WHERE workspace_id = ?1 AND message_id = ?2 AND status IN ('pending', 'composed', 'sent')")
        .bind(ws)
        .bind(message_id)
        .fetch_all(db)
        .await?;
    Ok((!levels.is_empty()).then(|| super::governing_level(&levels, super::Author::Template, 3)))
}

/// The same rule as `governing_for_message` but with each asking agent's level as of now: a held send that comes due
/// checks that nobody who vouched for the words has been demoted or paused since.
pub async fn governing_now(db: &SqlitePool, config: &crate::platform::config::Config, ws: &str, message_id: &str) -> sqlx::Result<Option<i64>> {
    let agents: Vec<String> = sqlx::query_scalar("SELECT DISTINCT from_agent_id FROM message_requests WHERE workspace_id = ?1 AND message_id = ?2 AND status IN ('pending', 'composed', 'sent')")
        .bind(ws)
        .bind(message_id)
        .fetch_all(db)
        .await?;
    if agents.is_empty() {
        return Ok(None);
    }
    let mut levels = vec![];
    for agent in &agents {
        levels.push(runtime::level_of(db, config, ws, agent, message_id).await?);
    }
    Ok(Some(super::governing_level(&levels, super::Author::Template, 3)))
}

/// Marks the requests behind a written reply as composed and remembers the draft.
pub async fn mark_composed(conn: &mut SqliteConnection, ws: &str, ids: &[String], draft_id: &str) -> sqlx::Result<()> {
    for id in ids {
        sqlx::query("UPDATE message_requests SET status = 'composed', draft_id = ?3, updated_at = ?4 WHERE id = ?1 AND workspace_id = ?2 AND status IN ('pending', 'composed')")
            .bind(id)
            .bind(ws)
            .bind(draft_id)
            .bind(now_ms())
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// A reply went out: the requests whose words are in that draft are done. A request that changed after the words were written
/// (it is `pending` again) or was never merged stays open: the mail that went out does not say what it now asks.
pub async fn mark_sent(conn: &mut SqliteConnection, ws: &str, draft_id: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE message_requests SET status = 'sent', status_note = NULL, updated_at = ?3 WHERE workspace_id = ?1 AND draft_id = ?2 AND status = 'composed'")
        .bind(ws)
        .bind(draft_id)
        .bind(now_ms())
        .execute(conn)
        .await?;
    Ok(())
}

/// After a send: a request that is still open (changed or filed after its words were written) gets a follow-up mail through
/// the same gate, written by the next reply case.
pub async fn follow_up(state: &AppState, ws: &str, message_id: &str) -> sqlx::Result<()> {
    let open: Option<String> = sqlx::query_scalar("SELECT id FROM message_requests WHERE workspace_id = ?1 AND message_id = ?2 AND status = 'pending' ORDER BY created_at LIMIT 1")
        .bind(ws)
        .bind(message_id)
        .fetch_optional(&state.db)
        .await?;
    if let Some(id) = open {
        ensure_reply_case(state, ws, message_id, &id).await?;
    }
    Ok(())
}

/// A request that could not be merged into the reply that stands (a person is working on it, it is being sent, or it went out):
/// it stays pending with a note for the person, and the trail says so.
pub async fn not_merged(conn: &mut SqliteConnection, ws: &str, message_id: &str, ids: &[String], why: &str, case_id: Option<&str>) -> sqlx::Result<()> {
    let mut first_changed: Option<&String> = None;
    for id in ids {
        let changed = sqlx::query("UPDATE message_requests SET status_note = ?3 WHERE id = ?1 AND workspace_id = ?2 AND status = 'pending' AND COALESCE(status_note, '') != ?3")
            .bind(id)
            .bind(ws)
            .bind(why)
            .execute(&mut *conn)
            .await?
            .rows_affected()
            == 1;
        if changed {
            first_changed.get_or_insert(id);
            audit_request(&mut *conn, ws, id, message_id, "handoff.not_merged", json!({"why": why})).await?;
        }
    }
    if let (Some(case_id), Some(first)) = (case_id, first_changed) {
        let asker: Option<String> =
            sqlx::query_scalar("SELECT a.name FROM message_requests r JOIN agents a ON a.id = r.from_agent_id WHERE r.id = ?1").bind(first).fetch_optional(&mut *conn).await?;
        let note = format!("{} has a newer request that is not in this reply", asker.unwrap_or_else(|| "An expert".into()));
        sqlx::query("UPDATE agent_approvals SET input_json = json_set(input_json, '$.handoff.newer', ?2) WHERE case_id = ?1 AND workspace_id = ?3 AND status = 'pending'")
            .bind(case_id)
            .bind(note)
            .bind(ws)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// The asking agent no longer wants anything said (its decision has no message now): its open request is cancelled. When no
/// other request is open, the unsent reply that carried it goes away with its case and approval; otherwise the rest is merged again.
pub async fn withdraw(state: &AppState, ws: &str, asker: &Asker<'_>, message_id: &str) -> sqlx::Result<()> {
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let gone: Vec<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "UPDATE message_requests SET status = 'cancelled', status_note = 'the desk no longer asks for this', updated_at = ?5
         WHERE workspace_id = ?1 AND message_id = ?2 AND from_agent_id = ?3 AND job = ?4 AND status IN ('pending', 'composed') RETURNING id, draft_id, reply_case_id",
    )
    .bind(ws)
    .bind(message_id)
    .bind(asker.agent_id)
    .bind(asker.job)
    .bind(now_ms())
    .fetch_all(&mut *tx)
    .await?;
    for (id, ..) in &gone {
        audit_request(&mut tx, ws, id, message_id, "handoff.withdrawn", json!({"from_agent_id": asker.agent_id, "job": asker.job})).await?;
    }
    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM message_requests WHERE workspace_id = ?1 AND message_id = ?2 AND status IN ('pending', 'composed')")
        .bind(ws)
        .bind(message_id)
        .fetch_one(&mut *tx)
        .await?;
    if remaining == 0 {
        for (_, draft_id, case_id) in &gone {
            if let Some(draft_id) = draft_id {
                sqlx::query("DELETE FROM drafts WHERE id = ?1 AND workspace_id = ?2 AND status = 'suggested' AND send_at IS NULL AND send_attempt_id IS NULL")
                    .bind(draft_id)
                    .bind(ws)
                    .execute(&mut *tx)
                    .await?;
            }
            if let Some(case_id) = case_id {
                close_case(&mut tx, ws, case_id, "Withdrawn: the desk no longer asks for this reply").await?;
            }
        }
    }
    tx.commit().await?;
    if remaining > 0 && !gone.is_empty() {
        crate::replies::compose::refresh(state, ws, message_id).await?;
    }
    Ok(())
}

/// Cancels a reply case that has not finished and declines the approval waiting on it (not as a mark against anyone).
async fn close_case(conn: &mut SqliteConnection, ws: &str, case_id: &str, note: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE agent_approvals SET status = 'declined', decided_at = ?2, note = ?3 WHERE case_id = ?1 AND workspace_id = ?4 AND status = 'pending'")
        .bind(case_id)
        .bind(now_ms())
        .bind(note)
        .bind(ws)
        .execute(&mut *conn)
        .await?;
    case::set_status(&mut *conn, ws, case_id, "cancelled").await?;
    Ok(())
}

/// Drafts that a newer message on the thread retired: their requests are superseded, their unfinished reply case is
/// cancelled and any approval still waiting on them is declined (not as a mark against anyone).
pub async fn supersede_drafts(conn: &mut SqliteConnection, ws: &str, draft_ids: &[String], by_message_id: &str) -> sqlx::Result<()> {
    for draft_id in draft_ids {
        let rows: Vec<(String, String, Option<String>)> =
            sqlx::query_as("SELECT id, message_id, reply_case_id FROM message_requests WHERE workspace_id = ?1 AND draft_id = ?2 AND status IN ('pending', 'composed')")
                .bind(ws)
                .bind(draft_id)
                .fetch_all(&mut *conn)
                .await?;
        for (id, message_id, case_id) in rows {
            sqlx::query("UPDATE message_requests SET status = 'superseded', status_note = 'a newer message arrived on the conversation', updated_at = ?2 WHERE id = ?1")
                .bind(&id)
                .bind(now_ms())
                .execute(&mut *conn)
                .await?;
            if let Some(case_id) = case_id {
                close_case(&mut *conn, ws, &case_id, "Replaced by a newer reply").await?;
            }
            audit_request(&mut *conn, ws, &id, &message_id, "handoff.superseded", json!({"by_message_id": by_message_id, "draft_id": draft_id})).await?;
        }
    }
    Ok(())
}

/// Display names of the agents behind some requests, in the same order.
pub async fn names(db: &SqlitePool, ws: &str, agent_ids: &[String]) -> sqlx::Result<Vec<String>> {
    let mut out = vec![];
    for id in agent_ids {
        let name: Option<String> = sqlx::query_scalar("SELECT name FROM agents WHERE id = ?1 AND workspace_id = ?2").bind(id).bind(ws).fetch_optional(db).await?;
        out.push(name.unwrap_or_else(|| "An agent".into()));
    }
    Ok(out)
}

/// Keeps a request that must not go out yet: a `waiting` row with no reply case and no draft. Filing the same request again
/// changes nothing; a changed one replaces the parked words; a request that was released or sent is left alone.
pub async fn park(state: &AppState, ws: &str, asker: &Asker<'_>, message_id: &str, req: &MessageRequest) -> sqlx::Result<bool> {
    let level = runtime::level_of(&state.db, &state.config, ws, asker.agent_id, message_id).await?;
    let (digest, now) = (digest(req), now_ms());
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let existing: Option<(String, String, String)> =
        sqlx::query_as("SELECT id, digest, status FROM message_requests WHERE workspace_id = ?1 AND message_id = ?2 AND from_agent_id = ?3 AND job = ?4")
            .bind(ws)
            .bind(message_id)
            .bind(asker.agent_id)
            .bind(asker.job)
            .fetch_optional(&mut *tx)
            .await?;
    let id = match existing {
        Some((_, old, status)) if old == digest || status != "waiting" => {
            tx.commit().await?;
            return Ok(false);
        }
        Some((id, ..)) => {
            sqlx::query("UPDATE message_requests SET kind = ?2, facts_json = ?3, issues_json = ?4, requester_level = ?5, digest = ?6, updated_at = ?7 WHERE id = ?1")
                .bind(&id)
                .bind(req.kind.as_str())
                .bind(json!(req.facts).to_string())
                .bind(json!(req.issues).to_string())
                .bind(level)
                .bind(&digest)
                .bind(now)
                .execute(&mut *tx)
                .await?;
            id
        }
        None => {
            let id = new_id("mrq");
            sqlx::query(
                "INSERT INTO message_requests (id, workspace_id, message_id, from_agent_id, job, kind, recipient_json, facts_json, issues_json, never_do_json, requester_level, digest,
                                               status, status_note, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, '[]', ?10, ?11, 'waiting', 'waiting for the person to be linked', ?12, ?12)",
            )
            .bind(&id)
            .bind(ws)
            .bind(message_id)
            .bind(asker.agent_id)
            .bind(asker.job)
            .bind(req.kind.as_str())
            .bind(json!(req.to).to_string())
            .bind(json!(req.facts).to_string())
            .bind(json!(req.issues).to_string())
            .bind(level)
            .bind(&digest)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            id
        }
    };
    audit_request(&mut tx, ws, &id, message_id, "handoff.parked", json!({"kind": req.kind, "from_agent_id": asker.agent_id, "job": asker.job})).await?;
    tx.commit().await?;
    Ok(true)
}

/// A parked request is ready: it becomes a normal pending request whose words the next filing replaces. `true` when one was waiting.
pub async fn unpark(state: &AppState, ws: &str, message_id: &str, job: &str) -> sqlx::Result<bool> {
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let released: Option<String> = sqlx::query_scalar(
        "UPDATE message_requests SET status = 'pending', digest = '', status_note = NULL, updated_at = ?4
         WHERE workspace_id = ?1 AND message_id = ?2 AND job = ?3 AND status = 'waiting' RETURNING id",
    )
    .bind(ws)
    .bind(message_id)
    .bind(job)
    .bind(now_ms())
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(id) = &released {
        audit_request(&mut tx, ws, id, message_id, "handoff.released", json!({"job": job})).await?;
    }
    tx.commit().await?;
    Ok(released.is_some())
}
