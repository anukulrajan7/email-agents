//! Ask and report back. A watch is something Meera asked on a person's behalf and is waiting on: a reply (an email
//! in the thread, an in-app reply, or a finished desk case) wakes it and the answer is posted into that person's
//! Meera conversation, with the email it came from. If nobody answers within the nudge time she nudges once and
//! tells the person; a later answer still gets through.

use serde_json::{Value, json};
use sqlx::SqliteConnection;

use super::recipients::Kind;
use crate::{
    agents::{injection, sentence},
    assistant::{command::Citation, plain_answer},
    audit::{self, Actor, Entry},
    db::{iso, new_id, now_ms},
    error::{AppError, AppResult},
    http::AppState,
    jobs::{self, handlers::JobError, kind},
    knowledge,
};

pub const DEFAULT_NUDGE_HOURS: i64 = 24;
pub const MAX_NUDGE_HOURS: i64 = 24 * 14;
const ANSWER_WORDS: usize = 70;

pub struct NewWatch<'a> {
    pub ws: &'a str,
    pub user_id: &'a str,
    pub agent_id: Option<&'a str>,
    /// `ask`, `ticket` or `desk`.
    pub kind: &'a str,
    pub title: &'a str,
    /// `email`, `in_app` or `desk`.
    pub channel: &'a str,
    pub counterpart: &'a str,
    pub outbound_email_id: Option<&'a str>,
    pub thread_key: Option<&'a str>,
    pub source_id: Option<&'a str>,
    pub case_id: Option<&'a str>,
    pub question: &'a str,
    pub nudge_hours: i64,
}

/// Starts watching and schedules the one nudge.
pub async fn create(conn: &mut SqliteConnection, w: NewWatch<'_>) -> sqlx::Result<String> {
    let id = new_id("wch");
    let now = now_ms();
    let hours = w.nudge_hours.clamp(1, MAX_NUDGE_HOURS);
    let nudge_at = now + hours * 3_600_000;
    sqlx::query(
        "INSERT INTO watches (id, workspace_id, user_id, agent_id, kind, title, channel, counterpart, status, outbound_email_id, thread_key,
                              source_id, case_id, question, nudge_hours, nudge_after_at, last_update_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'waiting', ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?16)",
    )
    .bind(&id)
    .bind(w.ws)
    .bind(w.user_id)
    .bind(w.agent_id)
    .bind(w.kind)
    .bind(sentence(w.title, 160))
    .bind(w.channel)
    .bind(sentence(w.counterpart, 120))
    .bind(w.outbound_email_id)
    .bind(w.thread_key)
    .bind(w.source_id)
    .bind(w.case_id)
    .bind(sentence(w.question, 500))
    .bind(hours)
    .bind(nudge_at)
    .bind(now)
    .execute(&mut *conn)
    .await?;
    if let Some(out) = w.outbound_email_id {
        sqlx::query("UPDATE outbound_emails SET watch_id = ?1 WHERE id = ?2").bind(&id).bind(out).execute(&mut *conn).await?;
    }
    enqueue_nudge(conn, w.ws, &id, nudge_at).await?;
    Ok(id)
}

async fn enqueue_nudge(conn: &mut SqliteConnection, ws: &str, watch_id: &str, at: i64) -> sqlx::Result<()> {
    jobs::enqueue_at(
        conn,
        kind::WATCH_NUDGE,
        &format!("watch_nudge:{watch_id}"),
        Some(ws),
        &json!({ "workspace_id": ws, "watch_id": watch_id }),
        jobs::DEFAULT_MAX_ATTEMPTS,
        at,
    )
    .await?;
    Ok(())
}

/// The email went out: its Gmail thread is what a reply will arrive in. A ticket starts waiting.
pub async fn email_sent(conn: &mut SqliteConnection, ws: &str, outbound_id: &str, source_id: &str, thread: Option<&str>) -> sqlx::Result<()> {
    let now = now_ms();
    let watch: Option<String> = sqlx::query_scalar(
        "UPDATE watches SET source_id = ?1, provider_thread_id = COALESCE(?2, provider_thread_id), last_update_at = ?3
         WHERE workspace_id = ?4 AND outbound_email_id = ?5 RETURNING id",
    )
    .bind(source_id)
    .bind(thread)
    .bind(now)
    .bind(ws)
    .bind(outbound_id)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(watch_id) = watch {
        sqlx::query("UPDATE support_tickets SET status = 'waiting', last_update_at = ?1 WHERE workspace_id = ?2 AND watch_id = ?3 AND status = 'open'")
            .bind(now)
            .bind(ws)
            .bind(&watch_id)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

const LISTENING: &str = "(w.status IN ('waiting','nudged') OR (w.kind = 'ticket' AND w.status = 'replied'))";

async fn wake(conn: &mut SqliteConnection, ws: &str, watch_ids: Vec<String>, what: &str, reference: &str) -> sqlx::Result<()> {
    for watch_id in watch_ids {
        jobs::enqueue(
            conn,
            kind::WATCH_REPLY,
            &format!("watch_reply:{watch_id}:{reference}"),
            Some(ws),
            &json!({ "workspace_id": ws, "watch_id": watch_id, "what": what, "ref": reference }),
        )
        .await?;
    }
    Ok(())
}

/// An email arrived in a stored thread: a watch on that Gmail thread wakes.
pub async fn on_inbound_email(conn: &mut SqliteConnection, ws: &str, source_id: &str, thread_id: &str, message_id: &str) -> sqlx::Result<()> {
    let ids: Vec<String> = sqlx::query_scalar(&format!(
        "SELECT w.id FROM watches w JOIN email_threads t ON t.provider_thread_id = w.provider_thread_id AND t.source_id = w.source_id
         WHERE w.workspace_id = ?1 AND w.source_id = ?2 AND t.id = ?3 AND {LISTENING}"
    ))
    .bind(ws)
    .bind(source_id)
    .bind(thread_id)
    .fetch_all(&mut *conn)
    .await?;
    wake(conn, ws, ids, "email", message_id).await
}

/// A recipient answered an in-app message in this thread.
pub async fn wake_for_reply(conn: &mut SqliteConnection, ws: &str, thread_key: &str, reply_id: &str) -> sqlx::Result<()> {
    let ids: Vec<String> = sqlx::query_scalar(&format!(
        "SELECT w.id FROM watches w WHERE w.workspace_id = ?1 AND w.thread_key = ?2 AND w.channel = 'in_app' AND {LISTENING}"
    ))
    .bind(ws)
    .bind(thread_key)
    .fetch_all(&mut *conn)
    .await?;
    wake(conn, ws, ids, "in_app", reply_id).await
}

/// A case ended: whoever handed it to a desk hears about it.
pub async fn on_case_ended(conn: &mut SqliteConnection, ws: &str, case_id: &str) -> sqlx::Result<()> {
    let ids: Vec<String> =
        sqlx::query_scalar(&format!("SELECT w.id FROM watches w WHERE w.workspace_id = ?1 AND w.case_id = ?2 AND {LISTENING}"))
            .bind(ws)
            .bind(case_id)
            .fetch_all(&mut *conn)
            .await?;
    wake(conn, ws, ids, "case", case_id).await
}

#[derive(sqlx::FromRow)]
pub struct Watch {
    pub id: String,
    pub workspace_id: String,
    pub user_id: String,
    pub agent_id: Option<String>,
    pub kind: String,
    pub title: String,
    pub channel: String,
    pub counterpart: String,
    pub status: String,
    pub outbound_email_id: Option<String>,
    pub thread_key: Option<String>,
    pub case_id: Option<String>,
    pub question: String,
    pub nudge_hours: i64,
    pub nudged_at: Option<i64>,
    pub answer_text: Option<String>,
    pub last_update_at: i64,
}

const WATCH_COLUMNS: &str = "id, workspace_id, user_id, agent_id, kind, title, channel, counterpart, status, outbound_email_id, thread_key, case_id,
    question, nudge_hours, nudged_at, answer_text, last_update_at";

pub async fn get(state: &AppState, ws: &str, id: &str) -> sqlx::Result<Option<Watch>> {
    sqlx::query_as(&format!("SELECT {WATCH_COLUMNS} FROM watches WHERE id = ?1 AND workspace_id = ?2")).bind(id).bind(ws).fetch_optional(&state.db).await
}

fn job_fields(payload: &Value) -> Result<(String, String), JobError> {
    let field = |k: &str| payload.get(k).and_then(Value::as_str).map(str::to_string);
    match (field("workspace_id"), field("watch_id")) {
        (Some(ws), Some(id)) => Ok((ws, id)),
        _ => Err(JobError::Permanent("payload missing workspace_id/watch_id".into())),
    }
}

struct Answer {
    who: String,
    text: String,
    citation: Option<Citation>,
    links: Value,
    warning: Option<&'static str>,
}

async fn read_answer(state: &AppState, w: &Watch, what: &str, reference: &str) -> sqlx::Result<Option<Answer>> {
    match what {
        "email" => {
            let row: Option<(String, Option<String>, String, String)> = sqlx::query_as(
                "SELECT sender_address, sender_name, subject, body_text FROM email_messages WHERE id = ?1 AND workspace_id = ?2 AND suppressed = 0",
            )
            .bind(reference)
            .bind(&w.workspace_id)
            .fetch_optional(&state.db)
            .await?;
            Ok(row.map(|(address, name, subject, body)| {
                let clean = knowledge::clean_body(&body);
                Answer {
                    who: name.filter(|n| !n.trim().is_empty()).unwrap_or(address),
                    warning: injection::scan(&clean),
                    text: plain_answer(&clean, ANSWER_WORDS),
                    citation: Some(Citation { message_id: reference.to_string(), subject, attachment_id: None, attachment_name: None }),
                    links: json!([{ "kind": "message", "id": reference, "label": "Open their reply" }]),
                }
            }))
        }
        "in_app" => {
            let row: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
                "SELECT m.body, u.name, u.email FROM in_app_messages m LEFT JOIN users u ON u.id = m.sender_user_id
                 WHERE m.id = ?1 AND m.workspace_id = ?2 AND m.recipient_user_id = ?3",
            )
            .bind(reference)
            .bind(&w.workspace_id)
            .bind(&w.user_id)
            .fetch_optional(&state.db)
            .await?;
            Ok(row.map(|(body, name, email)| Answer {
                who: name.filter(|n| !n.trim().is_empty()).or(email).unwrap_or_else(|| w.counterpart.clone()),
                warning: injection::scan(&body),
                text: plain_answer(&body, ANSWER_WORDS),
                citation: None,
                links: json!([{ "kind": "inbox", "id": reference, "label": "Open the reply" }]),
            }))
        }
        "case" => {
            let row: Option<(String, Option<String>, Option<String>, String)> = sqlx::query_as(
                "SELECT status, status_note, failure_reason, title FROM cases WHERE id = ?1 AND workspace_id = ?2",
            )
            .bind(reference)
            .bind(&w.workspace_id)
            .fetch_optional(&state.db)
            .await?;
            Ok(row.map(|(status, note, failure, _)| Answer {
                who: w.counterpart.clone(),
                warning: None,
                text: match status.as_str() {
                    "done" => note.unwrap_or_else(|| "It is finished.".into()),
                    _ => failure.or(note).unwrap_or_else(|| "It could not be finished.".into()),
                },
                citation: None,
                links: json!([{ "kind": "case", "id": reference, "label": "Open the case" }]),
            }))
        }
        _ => Ok(None),
    }
}

/// `WATCH_REPLY`: post the answer into the person's Meera conversation.
pub async fn reply_job(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let (ws, id) = job_fields(payload)?;
    let what = payload.get("what").and_then(Value::as_str).unwrap_or_default();
    let reference = payload.get("ref").and_then(Value::as_str).unwrap_or_default();
    let Some(w) = get(state, &ws, &id).await? else { return Ok(()) };
    if w.status == "closed" || (w.status == "replied" && w.kind != "ticket") {
        return Ok(());
    }
    let Some(answer) = read_answer(state, &w, what, reference).await? else { return Ok(()) };
    let resolved = w.kind == "ticket" && super::support::looks_resolved(&answer.text);
    let mut text = match w.kind.as_str() {
        "ticket" => format!("{} replied about “{}”: {}", answer.who, w.title, answer.text),
        "desk" => format!("{} finished “{}”: {}", answer.who, w.title, answer.text),
        _ => format!("{} answered your question “{}”: {}", answer.who, w.title, answer.text),
    };
    if answer.warning.is_some() {
        text.push_str(" Their message also contains text that tries to give me instructions, so I ignored that part.");
    }
    if resolved {
        text.push_str(" That sounds resolved, so I marked it done. Tell me if it is not.");
    }
    let now = now_ms();
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let updated = sqlx::query(
        "UPDATE watches SET status = 'replied', answer_text = ?1, answer_message_id = ?2, answered_at = ?3, last_update_at = ?3
         WHERE id = ?4 AND status != 'closed'",
    )
    .bind(&answer.text)
    .bind(reference)
    .bind(now)
    .bind(&id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if updated == 0 {
        return Ok(());
    }
    if w.kind == "ticket" {
        sqlx::query("UPDATE support_tickets SET status = ?1, last_summary = ?2, last_update_at = ?3 WHERE workspace_id = ?4 AND watch_id = ?5")
            .bind(if resolved { "resolved" } else { "open" })
            .bind(sentence(&answer.text, 300))
            .bind(now)
            .bind(&ws)
            .bind(&id)
            .execute(&mut *tx)
            .await?;
    }
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &ws,
            actor: Actor::System,
            action: "message.answer_reported",
            entity_type: "watch",
            entity_id: &id,
            message_id: (what == "email").then_some(reference),
            metadata: json!({ "kind": w.kind, "what": what, "resolved": resolved, "instructions_ignored": answer.warning.is_some() }),
        },
    )
    .await?;
    tx.commit().await?;
    let citations: Vec<Citation> = answer.citation.into_iter().collect();
    super::report(state, &ws, &w.user_id, &text, &citations, &answer.links).await?;
    let headline = if w.kind == "ticket" { format!("{} replied", w.counterpart) } else { format!("{} answered", answer.who) };
    super::inapp::notify_owner(state, &ws, &w.user_id, w.agent_id.as_deref(), &headline, &sentence(&text, 300), json!({ "kind": "meera" }), false).await?;
    Ok(())
}

/// `WATCH_NUDGE`: nobody answered in time. Nudge once, then tell the person.
pub async fn nudge_job(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let (ws, id) = job_fields(payload)?;
    let Some(w) = get(state, &ws, &id).await? else { return Ok(()) };
    if w.status != "waiting" || w.nudged_at.is_some() {
        return Ok(());
    }
    let claimed = sqlx::query("UPDATE watches SET status = 'nudged', nudged_at = ?1, last_update_at = ?1 WHERE id = ?2 AND status = 'waiting' AND nudged_at IS NULL")
        .bind(now_ms())
        .bind(&id)
        .execute(&state.db)
        .await?
        .rows_affected()
        == 1;
    if !claimed {
        return Ok(());
    }
    let hours = w.nudge_hours;
    let waited = if hours % 24 == 0 && hours >= 24 { format!("{} day{}", hours / 24, if hours == 24 { "" } else { "s" }) } else { format!("{hours} hours") };
    let done = match w.channel.as_str() {
        "in_app" => nudge_in_app(state, &w).await?,
        "email" => nudge_email(state, &w).await?,
        _ => format!("It is still with {}.", w.counterpart),
    };
    let text = format!("No answer yet on “{}” after {waited}. {done}", w.title);
    super::report(state, &ws, &w.user_id, &text, &[], &json!([])).await?;
    super::inapp::notify_owner(state, &ws, &w.user_id, w.agent_id.as_deref(), &format!("Still waiting: {}", sentence(&w.title, 80)), &text, json!({ "kind": "meera" }), false).await?;
    let mut conn = state.db.acquire().await?;
    audit::record(
        &mut conn,
        Entry {
            workspace_id: &ws,
            actor: Actor::System,
            action: "message.nudged",
            entity_type: "watch",
            entity_id: &id,
            message_id: None,
            metadata: json!({ "kind": w.kind, "channel": w.channel }),
        },
    )
    .await?;
    Ok(())
}

async fn nudge_in_app(state: &AppState, w: &Watch) -> Result<String, JobError> {
    let Some(key) = w.thread_key.as_deref() else { return Ok("I could not find who to nudge.".into()) };
    let recipients: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT recipient_user_id FROM in_app_messages WHERE workspace_id = ?1 AND thread_key = ?2 AND sender_user_id = ?3",
    )
    .bind(&w.workspace_id)
    .bind(key)
    .bind(&w.user_id)
    .fetch_all(&state.db)
    .await?;
    let body = format!("A gentle reminder about this question: {}", w.question);
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    for r in &recipients {
        super::inapp::insert(
            &mut tx,
            super::inapp::NewMessage {
                ws: &w.workspace_id,
                thread_key: Some(key),
                sender_user_id: Some(&w.user_id),
                sender_agent_id: w.agent_id.as_deref(),
                recipient: r,
                team_id: None,
                kind: "message",
                subject: &format!("Reminder: {}", sentence(&w.title, 120)),
                body: &body,
                link: Value::Null,
                via: "agent",
                deliver_at: now_ms(),
            },
        )
        .await?;
    }
    tx.commit().await?;
    Ok(format!("I nudged {} once. I will tell you as soon as they answer.", w.counterpart))
}

async fn nudge_email(state: &AppState, w: &Watch) -> Result<String, JobError> {
    let Some(out) = &w.outbound_email_id else { return Ok("I could not find the email to follow up.".into()) };
    let Some(sent) = super::email::get(state, &w.workspace_id, out).await? else { return Ok("I could not find the email to follow up.".into()) };
    let person = super::recipients::classify(&state.db, &w.workspace_id, &sent.to_address).await?;
    if let (Kind::Member, Some(uid)) = (person.kind, person.user_id.as_deref()) {
        let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
        super::inapp::insert(
            &mut tx,
            super::inapp::NewMessage {
                ws: &w.workspace_id,
                thread_key: None,
                sender_user_id: Some(&w.user_id),
                sender_agent_id: w.agent_id.as_deref(),
                recipient: uid,
                team_id: None,
                kind: "message",
                subject: &format!("Reminder: {}", sentence(&sent.subject, 120)),
                body: &format!("A gentle reminder about my email “{}”: {}", sentence(&sent.subject, 80), w.question),
                link: Value::Null,
                via: "agent",
                deliver_at: now_ms(),
            },
        )
        .await?;
        tx.commit().await?;
        return Ok(format!("I nudged {} once. I will tell you as soon as they answer.", w.counterpart));
    }
    let body = format!(
        "Hello,\n\nI am following up on my earlier email “{}”. Could you let me know where this stands?\n\nThank you.",
        sentence(&sent.subject, 120)
    );
    let reason = "it is a follow-up to someone outside your workspace";
    let follow_up = super::email::create(
        state,
        super::email::NewEmail {
            ws: &w.workspace_id,
            owner_user_id: Some(&w.user_id),
            agent_id: w.agent_id.as_deref(),
            to_address: &sent.to_address,
            to_label: &sent.to_label,
            kind: person.kind,
            subject: &format!("Re: {}", sent.subject.trim_start_matches("Re: ")),
            body: &body,
            authored_by: "agent",
            approval_reason: Some(reason),
            in_reply_to: sent.rfc_message_id.as_deref(),
            provider_thread_id: sent.provider_thread_id.as_deref(),
        },
    )
    .await
    .map_err(|e| match e {
        AppError::Internal(_) | AppError::Unavailable => JobError::Retry(e.to_string()),
        other => JobError::Permanent(other.to_string()),
    });
    match follow_up {
        Ok(f) => {
            sqlx::query("UPDATE outbound_emails SET watch_id = ?1 WHERE id = ?2").bind(&w.id).bind(&f.id).execute(&state.db).await?;
            Ok(format!("I wrote a short follow-up to {}. It needs your OK before it goes, because {reason}: open Messages to send it.", w.counterpart))
        }
        Err(JobError::Retry(m)) => Err(JobError::Retry(m)),
        Err(_) => Ok("I could not prepare a follow-up automatically.".into()),
    }
}

// ---------------------------------------------------------------- what the person sees

type TicketBrief = (String, String, String, String, String);

pub async fn list(state: &AppState, ws: &str, user_id: &str) -> sqlx::Result<Vec<Value>> {
    let watches: Vec<Watch> = sqlx::query_as(&format!(
        "SELECT {WATCH_COLUMNS} FROM watches WHERE workspace_id = ?1 AND user_id = ?2 AND status != 'closed' ORDER BY last_update_at DESC, id DESC LIMIT 50"
    ))
    .bind(ws)
    .bind(user_id)
    .fetch_all(&state.db)
    .await?;
    let mut out = vec![];
    for w in watches {
        let ticket: Option<TicketBrief> = sqlx::query_as(
            "SELECT id, company, support_address, status, last_summary FROM support_tickets WHERE workspace_id = ?1 AND watch_id = ?2",
        )
        .bind(ws)
        .bind(&w.id)
        .fetch_optional(&state.db)
        .await?;
        let (label, key) = match (w.status.as_str(), ticket.as_ref().map(|t| t.3.as_str())) {
            (_, Some("resolved")) => ("Resolved", "resolved"),
            ("replied", Some("open")) => ("They replied: needs a look", "open"),
            (_, Some("open")) => ("About to write to them", "starting"),
            ("replied", _) => ("Answered", "answered"),
            ("nudged", _) => ("No answer yet: I nudged once", "nudged"),
            _ => ("Waiting for a reply", "waiting"),
        };
        out.push(json!({
            "id": w.id, "kind": w.kind, "title": w.title, "with": w.counterpart, "channel": w.channel,
            "status": key, "status_label": label, "answer": w.answer_text, "updated_at": iso(w.last_update_at),
            "ticket": ticket.map(|t| json!({ "id": t.0, "company": t.1, "address": t.2, "status": t.3, "last_summary": t.4 })),
        }));
    }
    Ok(out)
}

/// The person stops watching (and, for a ticket, calls it resolved).
pub async fn close(state: &AppState, ws: &str, user_id: &str, id: &str) -> AppResult<()> {
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let closed = sqlx::query("UPDATE watches SET status = 'closed', last_update_at = ?1 WHERE id = ?2 AND workspace_id = ?3 AND user_id = ?4 AND status != 'closed'")
        .bind(now_ms())
        .bind(id)
        .bind(ws)
        .bind(user_id)
        .execute(&mut *tx)
        .await?
        .rows_affected()
        == 1;
    if !closed {
        return Err(AppError::NotFound);
    }
    sqlx::query("UPDATE support_tickets SET status = 'resolved', last_update_at = ?1 WHERE workspace_id = ?2 AND watch_id = ?3")
        .bind(now_ms())
        .bind(ws)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    audit::record(
        &mut tx,
        Entry { workspace_id: ws, actor: Actor::User(user_id), action: "message.watch_closed", entity_type: "watch", entity_id: id, message_id: None, metadata: json!({}) },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}
