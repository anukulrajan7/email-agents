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
    let refusal = if m.direction != "inbound" {
        Some("it is not an email we received")
    } else {
        no_reply_reason(&m.sender_address, &signals)
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
            let composed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM message_requests WHERE workspace_id = ?1 AND message_id = ?2 AND draft_id IS NOT NULL AND status = 'pending')")
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
            job: REPLY_JOB,
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
pub async fn governing_now(state: &AppState, ws: &str, message_id: &str) -> sqlx::Result<Option<i64>> {
    let agents: Vec<String> = sqlx::query_scalar("SELECT DISTINCT from_agent_id FROM message_requests WHERE workspace_id = ?1 AND message_id = ?2 AND status IN ('pending', 'composed', 'sent')")
        .bind(ws)
        .bind(message_id)
        .fetch_all(&state.db)
        .await?;
    if agents.is_empty() {
        return Ok(None);
    }
    let mut levels = vec![];
    for agent in &agents {
        levels.push(runtime::level_of(&state.db, &state.config, ws, agent, message_id).await?);
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

/// The reply was sent or scheduled: its requests are done.
pub async fn mark_sent(conn: &mut SqliteConnection, ws: &str, message_id: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE message_requests SET status = 'sent', updated_at = ?3 WHERE workspace_id = ?1 AND message_id = ?2 AND status IN ('pending', 'composed')")
        .bind(ws)
        .bind(message_id)
        .bind(now_ms())
        .execute(conn)
        .await?;
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
                sqlx::query("UPDATE agent_approvals SET status = 'declined', decided_at = ?2, note = 'Replaced by a newer reply' WHERE case_id = ?1 AND status = 'pending'")
                    .bind(&case_id)
                    .bind(now_ms())
                    .execute(&mut *conn)
                    .await?;
                case::set_status(&mut *conn, ws, &case_id, "cancelled").await?;
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
