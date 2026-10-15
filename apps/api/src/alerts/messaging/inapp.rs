//! In-app messages and notices. A message is only ever read by its recipient (or, in the sent view, its sender):
//! every query here filters on `recipient_user_id` or `sender_user_id`, and a held message stays hidden from the
//! recipient until its delivery time and for good once the sender stops it.

use serde_json::{Value, json};
use sqlx::{SqliteConnection, SqlitePool};

use crate::{
    platform::audit::{self, Actor, Entry},
    platform::db::{iso, new_id, now_ms},
    platform::error::{AppError, AppResult},
    http::AppState,
    jobs::handlers::JobError,
    platform::mailer::OutgoingEmail,
    mail::sources::normalize::truncate_chars,
};

pub const MAX_BODY_CHARS: usize = 4_000;
pub const MAX_SUBJECT_CHARS: usize = 200;

pub struct NewMessage<'a> {
    pub ws: &'a str,
    pub thread_key: Option<&'a str>,
    pub sender_user_id: Option<&'a str>,
    pub sender_agent_id: Option<&'a str>,
    pub recipient: &'a str,
    pub team_id: Option<&'a str>,
    /// `message` or `notice`.
    pub kind: &'a str,
    pub subject: &'a str,
    pub body: &'a str,
    pub link: Value,
    /// `assistant`, `agent`, `person` or `system`.
    pub via: &'a str,
    pub deliver_at: i64,
}

/// Stores one message for one recipient. Returns `(id, thread_key)`.
pub async fn insert(conn: &mut SqliteConnection, m: NewMessage<'_>) -> sqlx::Result<(String, String)> {
    let id = new_id("imsg");
    let thread_key = m.thread_key.map(str::to_string).unwrap_or_else(|| id.clone());
    sqlx::query(
        "INSERT INTO in_app_messages (id, workspace_id, thread_key, sender_user_id, sender_agent_id, recipient_user_id, team_id, kind,
                                      subject, body, link_json, via, deliver_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
    )
    .bind(&id)
    .bind(m.ws)
    .bind(&thread_key)
    .bind(m.sender_user_id)
    .bind(m.sender_agent_id)
    .bind(m.recipient)
    .bind(m.team_id)
    .bind(m.kind)
    .bind(truncate_chars(m.subject.trim(), MAX_SUBJECT_CHARS))
    .bind(truncate_chars(m.body.trim(), MAX_BODY_CHARS))
    .bind(m.link.to_string())
    .bind(m.via)
    .bind(m.deliver_at)
    .bind(now_ms())
    .execute(conn)
    .await?;
    Ok((id, thread_key))
}

#[derive(sqlx::FromRow)]
struct InboxRow {
    id: String,
    thread_key: String,
    kind: String,
    subject: String,
    body: String,
    link_json: String,
    sender_name: Option<String>,
    sender_email: Option<String>,
    sender_agent_id: Option<String>,
    read_at: Option<i64>,
    deliver_at: i64,
}

const VISIBLE: &str = "m.deliver_at <= ?3 AND m.cancelled_at IS NULL AND m.needs_check = 0";

/// The caller's own messages and notices, newest first.
pub async fn inbox(db: &SqlitePool, ws: &str, user_id: &str) -> sqlx::Result<Vec<Value>> {
    let rows: Vec<InboxRow> = sqlx::query_as(&format!(
        "SELECT m.id, m.thread_key, m.kind, m.subject, m.body, m.link_json, u.name AS sender_name, u.email AS sender_email,
                m.sender_agent_id, m.read_at, m.deliver_at
         FROM in_app_messages m LEFT JOIN users u ON u.id = m.sender_user_id
         WHERE m.workspace_id = ?1 AND m.recipient_user_id = ?2 AND {VISIBLE}
         ORDER BY m.deliver_at DESC, m.id DESC LIMIT 100"
    ))
    .bind(ws)
    .bind(user_id)
    .bind(now_ms())
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let from = r.sender_name.filter(|n| !n.trim().is_empty()).or(r.sender_email).unwrap_or_else(|| "Meera".into());
            json!({
                "id": r.id, "thread_key": r.thread_key, "kind": r.kind, "subject": r.subject, "body": r.body,
                "from": from, "from_assistant": r.sender_agent_id.is_some(),
                "link": serde_json::from_str::<Value>(&r.link_json).unwrap_or(Value::Null),
                "read": r.read_at.is_some(), "at": iso(r.deliver_at), "can_reply": r.kind == "message",
            })
        })
        .collect())
}

pub async fn unread_count(db: &SqlitePool, ws: &str, user_id: &str) -> sqlx::Result<i64> {
    sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM in_app_messages m WHERE m.workspace_id = ?1 AND m.recipient_user_id = ?2 AND m.read_at IS NULL AND {VISIBLE}"
    ))
    .bind(ws)
    .bind(user_id)
    .bind(now_ms())
    .fetch_one(db)
    .await
}

pub async fn mark_read(db: &SqlitePool, ws: &str, user_id: &str, id: &str) -> AppResult<()> {
    let done = sqlx::query(
        "UPDATE in_app_messages SET read_at = COALESCE(read_at, ?1)
         WHERE id = ?2 AND workspace_id = ?3 AND recipient_user_id = ?4 AND deliver_at <= ?1 AND cancelled_at IS NULL AND needs_check = 0",
    )
    .bind(now_ms())
    .bind(id)
    .bind(ws)
    .bind(user_id)
    .execute(db)
    .await?
    .rows_affected();
    if done == 0 { Err(AppError::NotFound) } else { Ok(()) }
}

/// What the caller sent (through Meera or by replying): one row per message, with where it stands.
pub async fn sent(db: &SqlitePool, ws: &str, user_id: &str) -> sqlx::Result<Vec<Value>> {
    #[derive(sqlx::FromRow)]
    struct SentRow {
        id: String,
        subject: String,
        body: String,
        via: String,
        name: Option<String>,
        email: Option<String>,
        deliver_at: i64,
        cancelled_at: Option<i64>,
        needs_check: bool,
        read_at: Option<i64>,
    }
    let rows: Vec<SentRow> = sqlx::query_as(
        "SELECT m.id, m.subject, m.body, m.via, u.name AS name, u.email AS email, m.deliver_at, m.cancelled_at, m.needs_check, m.read_at
         FROM in_app_messages m JOIN users u ON u.id = m.recipient_user_id
         WHERE m.workspace_id = ?1 AND m.sender_user_id = ?2 AND m.kind = 'message' ORDER BY m.created_at DESC, m.id DESC LIMIT 100",
    )
    .bind(ws)
    .bind(user_id)
    .fetch_all(db)
    .await?;
    let now = now_ms();
    Ok(rows
        .into_iter()
        .map(|r| {
            let SentRow { id, subject, body, via, name, email, deliver_at, cancelled_at, needs_check, read_at } = r;
            let status = if cancelled_at.is_some() {
                "stopped"
            } else if deliver_at > now || needs_check {
                "held"
            } else if read_at.is_some() {
                "read"
            } else {
                "delivered"
            };
            json!({
                "id": id, "channel": "in_app", "to": name.filter(|n| !n.trim().is_empty()).or(email).unwrap_or_default(),
                "subject": subject, "body": body, "via": via, "status": status, "at": iso(deliver_at), "can_stop": status == "held",
            })
        })
        .collect())
}

/// The sender stops a message that has not been delivered yet.
pub async fn stop(db: &SqlitePool, ws: &str, user_id: &str, id: &str) -> AppResult<()> {
    let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
    let done = sqlx::query(
        "UPDATE in_app_messages SET cancelled_at = ?1
         WHERE id = ?2 AND workspace_id = ?3 AND sender_user_id = ?4 AND (deliver_at > ?1 OR needs_check = 1) AND cancelled_at IS NULL",
    )
    .bind(now_ms())
    .bind(id)
    .bind(ws)
    .bind(user_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if done == 0 {
        return Err(AppError::Conflict("too late: this message was already delivered".into()));
    }
    super::watch::close_if_nothing_delivered(&mut tx, ws, id).await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: ws,
            actor: Actor::User(user_id),
            action: "message.in_app_stopped",
            entity_type: "in_app_message",
            entity_id: id,
            message_id: None,
            metadata: json!({}),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// The recipient answers a message: it goes back to the sender in the same thread, and wakes whatever Meera is
/// watching on that thread.
pub async fn reply(state: &AppState, ws: &str, user_id: &str, id: &str, body: &str) -> AppResult<String> {
    let body = body.trim();
    if body.is_empty() {
        return Err(AppError::BadRequest("write your reply first".into()));
    }
    if body.chars().count() > MAX_BODY_CHARS {
        return Err(AppError::BadRequest(format!("keep the reply under {MAX_BODY_CHARS} characters")));
    }
    let original: Option<(String, Option<String>, String, String)> = sqlx::query_as(
        "SELECT thread_key, sender_user_id, subject, kind FROM in_app_messages
         WHERE id = ?1 AND workspace_id = ?2 AND recipient_user_id = ?3 AND deliver_at <= ?4 AND cancelled_at IS NULL AND needs_check = 0",
    )
    .bind(id)
    .bind(ws)
    .bind(user_id)
    .bind(now_ms())
    .fetch_optional(&state.db)
    .await?;
    let (thread_key, sender, subject, kind) = original.ok_or(AppError::NotFound)?;
    let Some(sender) = sender.filter(|_| kind == "message") else {
        return Err(AppError::Conflict("this is a notice; there is nobody to reply to".into()));
    };
    let subject = if subject.to_lowercase().starts_with("re:") { subject } else { format!("Re: {subject}") };
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let (reply_id, _) = insert(
        &mut tx,
        NewMessage {
            ws,
            thread_key: Some(&thread_key),
            sender_user_id: Some(user_id),
            sender_agent_id: None,
            recipient: &sender,
            team_id: None,
            kind: "message",
            subject: &subject,
            body,
            link: Value::Null,
            via: "person",
            deliver_at: now_ms(),
        },
    )
    .await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: ws,
            actor: Actor::User(user_id),
            action: "message.in_app_replied",
            entity_type: "in_app_message",
            entity_id: &reply_id,
            message_id: None,
            metadata: json!({ "in_reply_to": id }),
        },
    )
    .await?;
    super::watch::wake_for_reply(&mut tx, ws, &thread_key, &reply_id).await?;
    tx.commit().await?;
    state.jobs_notify.notify_one();
    Ok(reply_id)
}

/// The notice itself, inside the caller's transaction.
#[allow(clippy::too_many_arguments)]
pub async fn notify_in(
    conn: &mut SqliteConnection,
    ws: &str,
    user_id: &str,
    agent_id: Option<&str>,
    subject: &str,
    body: &str,
    link: Value,
    email_line: bool,
) -> sqlx::Result<String> {
    let (id, _) = insert(
        conn,
        NewMessage {
            ws,
            thread_key: None,
            sender_user_id: None,
            sender_agent_id: agent_id,
            recipient: user_id,
            team_id: None,
            kind: "notice",
            subject,
            body,
            link,
            via: if agent_id.is_some() { "agent" } else { "system" },
            deliver_at: now_ms(),
        },
    )
    .await?;
    audit::record(
        conn,
        Entry {
            workspace_id: ws,
            actor: Actor::System,
            action: "message.owner_notified",
            entity_type: "in_app_message",
            entity_id: &id,
            message_id: None,
            metadata: json!({ "email_line": email_line }),
        },
    )
    .await?;
    Ok(id)
}

/// Tells a person something in their Post Room notifications, and (if asked) emails them a one-line version.
/// Returns the notice id.
#[allow(clippy::too_many_arguments)]
pub async fn notify_owner(
    state: &AppState,
    ws: &str,
    user_id: &str,
    agent_id: Option<&str>,
    subject: &str,
    body: &str,
    link: Value,
    email_line: bool,
) -> sqlx::Result<String> {
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let id = notify_in(&mut tx, ws, user_id, agent_id, subject, body, link, email_line).await?;
    tx.commit().await?;
    if email_line && let Some(mailer) = &state.alert_mailer {
        let email: Option<String> = sqlx::query_scalar("SELECT email FROM users WHERE id = ?1").bind(user_id).fetch_optional(&state.db).await?;
        if let Some(to) = email {
            let line = truncate_chars(subject.trim(), MAX_SUBJECT_CHARS);
            let text = format!("{line}\n\nOpen Post Room to read more.");
            let html = format!("<p>{}</p><p>Open Post Room to read more.</p>", crate::alerts::escape_html(&line));
            let out = OutgoingEmail { kind: crate::platform::mailer::kind::ALERT, to: &to, subject: &line, text: &text, html: &html };
            if let Err(e) = crate::platform::mailer::deliver(&state.db, mailer.as_ref(), out).await {
                tracing::warn!(error = %e, "notice email line not sent");
            }
        }
    }
    Ok(id)
}

/// `INAPP_DUE`: an automatic in-app message is checked again when it comes due. It is delivered only if the
/// sender's assistant is still on at level 2 or more and the recipient is still in the workspace.
pub async fn due_job(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let field = |k: &str| payload.get(k).and_then(Value::as_str).map(str::to_string);
    let (Some(ws), Some(id)) = (field("workspace_id"), field("message_id")) else {
        return Err(JobError::Permanent("payload missing workspace_id/message_id".into()));
    };
    let row: Option<(Option<String>, String, String)> = sqlx::query_as(
        "SELECT sender_user_id, recipient_user_id, subject FROM in_app_messages WHERE id = ?1 AND workspace_id = ?2 AND needs_check = 1 AND cancelled_at IS NULL",
    )
    .bind(&id)
    .bind(&ws)
    .fetch_optional(&state.db)
    .await?;
    let Some((sender, recipient, subject)) = row else { return Ok(()) };
    let why = match &sender {
        Some(owner) if super::rules::personal_level(state, &ws, owner).await? < 2 => Some("the assistant no longer has permission to send on its own"),
        None => Some("there is nobody to check it for"),
        _ => None,
    };
    let why = match why {
        Some(w) => Some(w),
        None if super::recipients::member(&state.db, &ws, &recipient).await?.is_none() => Some("the recipient is no longer in your workspace"),
        None => None,
    };
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    match why {
        None => {
            sqlx::query("UPDATE in_app_messages SET needs_check = 0 WHERE id = ?1 AND needs_check = 1 AND cancelled_at IS NULL").bind(&id).execute(&mut *tx).await?;
        }
        Some(why) => {
            let held = sqlx::query("UPDATE in_app_messages SET cancelled_at = ?1, needs_check = 0 WHERE id = ?2 AND needs_check = 1 AND cancelled_at IS NULL")
                .bind(now_ms())
                .bind(&id)
                .execute(&mut *tx)
                .await?
                .rows_affected();
            if held == 1 {
                super::watch::close_if_nothing_delivered(&mut tx, &ws, &id).await?;
                audit::record(
                    &mut tx,
                    Entry {
                        workspace_id: &ws,
                        actor: Actor::System,
                        action: "message.in_app_held",
                        entity_type: "in_app_message",
                        entity_id: &id,
                        message_id: None,
                        metadata: json!({ "reason": why }),
                    },
                )
                .await?;
                if let Some(owner) = sender.as_deref() {
                    let text = format!("I did not send “{}”: {why}. Ask me again if you still want it to go.", crate::agents::sentence(&subject, 80));
                    notify_in(&mut tx, &ws, owner, None, "A message was not sent", &text, json!({ "kind": "meera" }), false).await?;
                    super::report_in(&mut tx, &ws, owner, &text, &[], &json!([])).await?;
                }
            }
        }
    }
    tx.commit().await?;
    Ok(())
}
