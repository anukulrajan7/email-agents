//! A new email from a person's own mailbox. It is checked, held for a few seconds (ten minutes when nobody asked
//! for it), sent through Gmail under a durable attempt id, and recorded. A reply to an email is still a draft;
//! this is for mail that starts a conversation (or follows one up).

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use lettre::message::{Mailbox as LettreMailbox, Message, header::ContentType};
use reqwest::Method;
use serde_json::{Value, json};

use super::recipients::{self, Kind};
use crate::{
    audit::{self, Actor, Entry},
    db::{iso, new_id, now_ms},
    domain::Capabilities,
    error::{AppError, AppResult},
    http::AppState,
    jobs::{self, handlers::JobError, kind},
    replies::{attempt_message_id, header_safe, MAX_SUBJECT_CHARS},
    send_safety::{self, Issue, SafetyReport, bare_address, valid_address},
    sources::{nango::NangoError, normalize::truncate_chars},
};

pub const SENT_ACTION: &str = "message.email_sent";
pub const AUTO_SENT_ACTION: &str = "message.auto_sent";
/// An open attempt younger than this may not be visible in Gmail yet: never call it "not sent".
const RECONCILE_GRACE_MS: i64 = 120_000;

#[derive(Debug, Clone)]
pub struct Mailbox {
    pub source_id: String,
    pub connection_id: String,
    pub config_key: String,
    pub address: Option<String>,
}

/// The mailbox a message goes out from: the person's own, or (for a workspace agent) the shared one.
pub async fn mailbox_for(state: &AppState, ws: &str, owner: Option<&str>) -> AppResult<Mailbox> {
    #[derive(sqlx::FromRow)]
    struct SourceRow {
        id: String,
        external_connection_id: Option<String>,
        provider_config_key: Option<String>,
        mailbox_address: Option<String>,
        capabilities_json: String,
    }
    let rows: Vec<SourceRow> = sqlx::query_as(
        "SELECT id, external_connection_id, provider_config_key, mailbox_address, capabilities_json FROM email_sources
         WHERE workspace_id = ?1 AND kind = 'nango_gmail' AND status = 'connected'
           AND ((?2 IS NOT NULL AND connected_by_user_id = ?2) OR (?2 IS NULL AND connected_by_user_id IS NULL))
         ORDER BY created_at, id",
    )
    .bind(ws)
    .bind(owner)
    .fetch_all(&state.db)
    .await?;
    for r in rows {
        let caps: Capabilities = serde_json::from_str(&r.capabilities_json).unwrap_or_else(|_| crate::sources::capabilities("nango_gmail"));
        if let (Some(connection_id), Some(config_key)) = (r.external_connection_id, r.provider_config_key)
            && crate::ai::policy::can_send(&caps)
        {
            return Ok(Mailbox { source_id: r.id, connection_id, config_key, address: r.mailbox_address.filter(|a| !a.is_empty()) });
        }
    }
    Err(AppError::Conflict(if owner.is_some() {
        "Connect your own Gmail mailbox first: I send email from your mailbox.".into()
    } else {
        "No shared mailbox that can send is connected.".into()
    }))
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Outbound {
    pub id: String,
    pub workspace_id: String,
    pub source_id: String,
    pub owner_user_id: Option<String>,
    pub agent_id: Option<String>,
    pub to_address: String,
    pub to_label: String,
    pub subject: String,
    pub body_text: String,
    pub recipient_kind: String,
    pub authored_by: String,
    pub approval_reason: Option<String>,
    pub status: String,
    pub auto_sent: bool,
    pub send_at: Option<i64>,
    pub send_requested_by: Option<String>,
    pub send_error: Option<String>,
    pub send_attempt_id: Option<String>,
    pub send_started_at: Option<i64>,
    pub provider_message_id: Option<String>,
    pub provider_thread_id: Option<String>,
    pub rfc_message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub watch_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

const COLUMNS: &str = "id, workspace_id, source_id, owner_user_id, agent_id, to_address, to_label, subject, body_text, recipient_kind,
    authored_by, approval_reason, status, auto_sent, send_at, send_requested_by, send_error, send_attempt_id, send_started_at,
    provider_message_id, provider_thread_id, rfc_message_id, in_reply_to, watch_id, created_at, updated_at";

pub async fn get(state: &AppState, ws: &str, id: &str) -> sqlx::Result<Option<Outbound>> {
    sqlx::query_as(&format!("SELECT {COLUMNS} FROM outbound_emails WHERE id = ?1 AND workspace_id = ?2"))
        .bind(id)
        .bind(ws)
        .fetch_optional(&state.db)
        .await
}

fn issue(code: &'static str, message: impl Into<String>, fix: &str) -> Issue {
    Issue { code, message: message.into(), fix: (!fix.is_empty()).then(|| fix.to_string()) }
}

/// The checks every new email passes before it is held, and again before it goes out.
pub fn check(to: &str, subject: &str, body: &str, own: &[String]) -> SafetyReport {
    let mut blocking = vec![];
    let to = bare_address(to);
    if !valid_address(&to) {
        blocking.push(issue("invalid_recipient", format!("\"{to}\" isn't a valid email address."), "Check the address."));
    } else if own.iter().any(|o| bare_address(o) == to) {
        blocking.push(issue("own_mailbox", "This email would go to your own mailbox.", "Pick another recipient."));
    } else if send_safety::is_automated_address(&to) {
        blocking.push(issue("automated_recipient", format!("{to} is an automated address; nobody will read it."), "Find a person's address."));
    }
    let subject = header_safe(subject);
    if subject.is_empty() || subject.chars().count() > MAX_SUBJECT_CHARS {
        blocking.push(issue("bad_subject", format!("The subject must be 1 to {MAX_SUBJECT_CHARS} characters."), "Write a short subject."));
    }
    blocking.extend(send_safety::check_gaps(body));
    blocking.extend(send_safety::check_placeholders(body));
    blocking.extend(send_safety::check_length(body));
    SafetyReport { blocking, warnings: vec![] }
}

pub struct NewEmail<'a> {
    pub ws: &'a str,
    pub owner_user_id: Option<&'a str>,
    pub agent_id: Option<&'a str>,
    pub to_address: &'a str,
    pub to_label: &'a str,
    pub kind: Kind,
    pub subject: &'a str,
    pub body: &'a str,
    /// `agent` or `person`: a person's own words are never sent on their own.
    pub authored_by: &'a str,
    pub approval_reason: Option<&'a str>,
    pub in_reply_to: Option<&'a str>,
    pub provider_thread_id: Option<&'a str>,
}

/// Stores the email, waiting for a person's OK or for `schedule`. Nothing is sent.
pub async fn create(state: &AppState, new: NewEmail<'_>) -> AppResult<Outbound> {
    let mailbox = mailbox_for(state, new.ws, new.owner_user_id).await?;
    let owner_email: Option<String> = match new.owner_user_id {
        Some(u) => sqlx::query_scalar("SELECT email FROM users WHERE id = ?1").bind(u).fetch_optional(&state.db).await?,
        None => None,
    };
    let own: Vec<String> = mailbox.address.iter().cloned().chain(owner_email).collect();
    let report = check(new.to_address, new.subject, new.body, &own);
    if report.is_blocked() {
        return Err(report.into_error());
    }
    let id = new_id("out");
    let now = now_ms();
    sqlx::query(
        "INSERT INTO outbound_emails (id, workspace_id, source_id, owner_user_id, agent_id, to_address, to_label, subject, body_text,
                                      recipient_kind, authored_by, approval_reason, status, in_reply_to, provider_thread_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'awaiting_approval', ?13, ?14, ?15, ?15)",
    )
    .bind(&id)
    .bind(new.ws)
    .bind(&mailbox.source_id)
    .bind(new.owner_user_id)
    .bind(new.agent_id)
    .bind(bare_address(new.to_address))
    .bind(truncate_chars(new.to_label, 120))
    .bind(header_safe(new.subject))
    .bind(new.body.trim())
    .bind(new.kind.as_str())
    .bind(new.authored_by)
    .bind(new.approval_reason)
    .bind(new.in_reply_to)
    .bind(new.provider_thread_id)
    .bind(now)
    .execute(&state.db)
    .await?;
    get(state, new.ws, &id).await?.ok_or(AppError::NotFound)
}

#[derive(Debug, Clone)]
pub struct Scheduled {
    pub id: String,
    pub send_at_ms: i64,
}

/// Holds the email for at least the undo window (`hold_secs` when longer) and queues the send. `auto` = nobody
/// asked for this send: a person's own words are refused, and the send is checked again when it comes due.
pub async fn schedule(state: &AppState, ws: &str, id: &str, actor: Actor<'_>, hold_secs: i64, auto: bool) -> AppResult<Scheduled> {
    let row = get(state, ws, id).await?.ok_or(AppError::NotFound)?;
    if row.status != "awaiting_approval" {
        return Err(AppError::Conflict(match row.status.as_str() {
            "sent" => "this email was already sent".into(),
            "scheduled" => "this email is already on its way".into(),
            _ => "this email can no longer be sent".into(),
        }));
    }
    if auto && row.authored_by == "person" {
        return Err(AppError::Conflict("an email a person wrote is never sent on its own".into()));
    }
    let own = own_addresses(state, &row).await?;
    let report = check(&row.to_address, &row.subject, &row.body_text, &own);
    if report.is_blocked() {
        return Err(report.into_error());
    }
    let duplicate: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM outbound_emails WHERE workspace_id = ?1 AND to_address = ?2 AND subject = ?3 AND id != ?4
                          AND status IN ('scheduled','sent') AND created_at > ?5)",
    )
    .bind(ws)
    .bind(&row.to_address)
    .bind(&row.subject)
    .bind(id)
    .bind(now_ms() - send_safety::DUPLICATE_WINDOW_MS)
    .fetch_one(&state.db)
    .await?;
    if duplicate {
        return Err(SafetyReport { blocking: vec![issue("duplicate", "The same email just went to this address.", "Wait a few minutes.")], warnings: vec![] }.into_error());
    }
    let now = now_ms();
    let send_at = now + state.config.send_undo_seconds.max(hold_secs).max(0) * 1_000;
    let requested_by = match actor {
        Actor::User(u) => Some(u),
        Actor::System => None,
    };
    let mut tx = state.db.begin().await?;
    let claimed = sqlx::query(
        "UPDATE outbound_emails SET status = 'scheduled', send_at = ?1, send_requested_by = ?2, auto_sent = ?3, send_error = NULL, updated_at = ?4
         WHERE id = ?5 AND workspace_id = ?6 AND status = 'awaiting_approval'",
    )
    .bind(send_at)
    .bind(requested_by)
    .bind(auto)
    .bind(now)
    .bind(id)
    .bind(ws)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if !claimed {
        return Err(AppError::Conflict("this email is already on its way".into()));
    }
    jobs::enqueue_at(
        &mut tx,
        kind::SEND_OUTBOUND,
        &format!("send_outbound:{id}:{send_at}"),
        Some(ws),
        &json!({ "workspace_id": ws, "outbound_id": id, "send_at": send_at }),
        jobs::PROVIDER_MAX_ATTEMPTS,
        send_at,
    )
    .await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: ws,
            actor,
            action: "message.email_scheduled",
            entity_type: "outbound_email",
            entity_id: id,
            message_id: None,
            metadata: json!({ "recipient_kind": row.recipient_kind, "auto": auto, "send_at": send_at, "authored_by": row.authored_by }),
        },
    )
    .await?;
    if auto {
        audit::record(
            &mut tx,
            Entry {
                workspace_id: ws,
                actor: Actor::System,
                action: AUTO_SENT_ACTION,
                entity_type: "outbound_email",
                entity_id: id,
                message_id: None,
                metadata: json!({ "channel": "email", "owner": row.owner_user_id, "hold_secs": (send_at - now) / 1000 }),
            },
        )
        .await?;
    }
    tx.commit().await?;
    state.jobs_notify.notify_one();
    Ok(Scheduled { id: id.to_string(), send_at_ms: send_at })
}

async fn own_addresses(state: &AppState, row: &Outbound) -> sqlx::Result<Vec<String>> {
    let mailbox: Option<String> = sqlx::query_scalar("SELECT mailbox_address FROM email_sources WHERE id = ?1").bind(&row.source_id).fetch_optional(&state.db).await?.flatten();
    let owner: Option<String> = match &row.owner_user_id {
        Some(u) => sqlx::query_scalar("SELECT email FROM users WHERE id = ?1").bind(u).fetch_optional(&state.db).await?,
        None => None,
    };
    Ok(mailbox.into_iter().chain(owner).collect())
}

/// A person stops an email that has not gone out yet (the one who asked for it, or the mailbox owner).
pub async fn stop(state: &AppState, ws: &str, id: &str, user_id: &str) -> AppResult<()> {
    let mut tx = state.db.begin().await?;
    let stopped = sqlx::query(
        "UPDATE outbound_emails SET status = 'cancelled', send_at = NULL, updated_at = ?1
         WHERE id = ?2 AND workspace_id = ?3 AND status IN ('scheduled','awaiting_approval') AND send_attempt_id IS NULL
           AND (owner_user_id = ?4 OR send_requested_by = ?4)",
    )
    .bind(now_ms())
    .bind(id)
    .bind(ws)
    .bind(user_id)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if !stopped {
        let status: Option<(String, Option<String>)> =
            sqlx::query_as("SELECT status, owner_user_id FROM outbound_emails WHERE id = ?1 AND workspace_id = ?2").bind(id).bind(ws).fetch_optional(&mut *tx).await?;
        return Err(match status {
            Some((_, owner)) if owner.as_deref() != Some(user_id) => AppError::NotFound,
            Some((s, _)) if s == "sent" => AppError::Conflict("too late: this email was already sent".into()),
            Some(_) => AppError::Conflict("this email can no longer be stopped".into()),
            None => AppError::NotFound,
        });
    }
    audit::record(
        &mut tx,
        Entry {
            workspace_id: ws,
            actor: Actor::User(user_id),
            action: "message.email_stopped",
            entity_type: "outbound_email",
            entity_id: id,
            message_id: None,
            metadata: json!({}),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// A person approves a waiting email (a follow-up, say): it is held for the undo window, then sent.
pub async fn approve(state: &AppState, ws: &str, id: &str, user_id: &str) -> AppResult<Scheduled> {
    let row = get(state, ws, id).await?.ok_or(AppError::NotFound)?;
    if row.owner_user_id.as_deref() != Some(user_id) {
        return Err(AppError::NotFound);
    }
    schedule(state, ws, id, Actor::User(user_id), 0, false).await
}

// ---------------------------------------------------------------- the send

/// RFC 5322 message for Gmail's `raw` field. A new conversation: no "Re:" is added.
pub fn build_raw_new(from: &str, to: &str, subject: &str, in_reply_to: Option<&str>, message_id: &str, body: &str) -> AppResult<String> {
    let bad = |e: String| AppError::BadRequest(e);
    let from: LettreMailbox = header_safe(from).parse().map_err(|_| bad("connected mailbox address is invalid".into()))?;
    let to: LettreMailbox = header_safe(to).parse().map_err(|_| bad("the recipient's address is invalid".into()))?;
    let mut builder = Message::builder().from(from).to(to).subject(truncate_chars(&header_safe(subject), MAX_SUBJECT_CHARS));
    if let Some(id) = in_reply_to.and_then(crate::replies::safe_message_id) {
        builder = builder.in_reply_to(id.clone()).references(id);
    }
    if let Some(id) = crate::replies::safe_message_id(message_id) {
        builder = builder.message_id(Some(id));
    }
    let msg = builder
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_string())
        .map_err(|e| AppError::Internal(anyhow::anyhow!("building email: {e}")))?;
    Ok(URL_SAFE_NO_PAD.encode(msg.formatted()))
}

enum Outcome {
    Sent { message_id: String, thread_id: Option<String> },
    NotSent,
    Unsure(String),
}

async fn reconcile(state: &AppState, row: &Outbound, mailbox: &Mailbox) -> Outcome {
    let Some(attempt) = row.send_attempt_id.as_deref() else { return Outcome::NotSent };
    let mid = attempt_message_id(attempt);
    let query = format!("rfc822msgid:{} in:anywhere", mid.trim_matches(['<', '>']));
    match state.nango.gmail_list_ids(&mailbox.connection_id, &mailbox.config_key, &query, None).await {
        Ok((ids, _)) => {
            if let Some(id) = ids.into_iter().next() {
                return Outcome::Sent { message_id: id, thread_id: None };
            }
        }
        Err(e) => return Outcome::Unsure(format!("Gmail search failed: {e}")),
    }
    if now_ms() - row.send_started_at.unwrap_or(0) < RECONCILE_GRACE_MS {
        return Outcome::Unsure("the last attempt is too recent to be sure".into());
    }
    Outcome::NotSent
}

fn job_err(e: AppError) -> JobError {
    match e {
        AppError::Unavailable | AppError::AiUnavailable | AppError::Internal(_) => JobError::Retry(e.to_string()),
        other => JobError::Permanent(other.to_string()),
    }
}

/// An automatic send is checked again when it comes due. Why it must wait for a person instead, if it must.
async fn auto_stopped(state: &AppState, row: &Outbound) -> sqlx::Result<Option<&'static str>> {
    if let Some(owner) = row.owner_user_id.as_deref()
        && super::rules::personal_level(state, &row.workspace_id, owner).await? < 2
    {
        return Ok(Some("the assistant no longer has permission to send on its own"));
    }
    let person = recipients::classify(&state.db, &row.workspace_id, &row.to_address).await?;
    if !person.kind.is_internal() {
        return Ok(Some("the recipient is not inside your workspace"));
    }
    Ok(None)
}

/// `SEND_OUTBOUND`: at `send_at`, send the email unless it was stopped.
pub async fn run_scheduled(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let field = |k: &str| payload.get(k).and_then(Value::as_str).map(str::to_string);
    let (Some(ws), Some(id), Some(send_at)) = (field("workspace_id"), field("outbound_id"), payload.get("send_at").and_then(Value::as_i64)) else {
        return Err(JobError::Permanent("payload missing workspace_id/outbound_id/send_at".into()));
    };
    let Some(mut row) = get(state, &ws, &id).await? else { return Ok(()) };
    if row.status != "scheduled" || row.send_at != Some(send_at) {
        return Ok(());
    }
    let mailbox = mailbox_for(state, &ws, row.owner_user_id.as_deref()).await.map_err(job_err)?;
    if let Some(attempt) = row.send_attempt_id.clone() {
        match reconcile(state, &row, &mailbox).await {
            Outcome::Sent { message_id, thread_id } => return finalize_sent(state, &row, &message_id, thread_id.as_deref()).await.map_err(job_err),
            Outcome::NotSent => {
                sqlx::query("UPDATE outbound_emails SET send_attempt_id = NULL, send_started_at = NULL WHERE id = ?1 AND send_attempt_id = ?2")
                    .bind(&id)
                    .bind(&attempt)
                    .execute(&state.db)
                    .await?;
                row = get(state, &ws, &id).await?.ok_or_else(|| JobError::Permanent("email vanished".into()))?;
            }
            Outcome::Unsure(why) => return Err(JobError::Retry(format!("confirming the previous send attempt: {why}"))),
        }
    }
    if row.send_requested_by.is_none()
        && let Some(reason) = auto_stopped(state, &row).await?
    {
        sqlx::query(
            "UPDATE outbound_emails SET status = 'awaiting_approval', send_at = NULL, auto_sent = 0, approval_reason = ?1, updated_at = ?2
             WHERE id = ?3 AND status = 'scheduled' AND send_attempt_id IS NULL",
        )
        .bind(format!("Held for you: {reason}."))
        .bind(now_ms())
        .bind(&id)
        .execute(&state.db)
        .await?;
        tracing::info!(outbound_id = %id, reason, "held automatic email needs a person");
        return Ok(());
    }
    let own = own_addresses(state, &row).await?;
    let report = check(&row.to_address, &row.subject, &row.body_text, &own);
    if report.is_blocked() {
        let reason = format!("Not sent: {}", report.blocking.iter().map(|i| i.message.as_str()).collect::<Vec<_>>().join(" "));
        return give_up(state, &row, &reason).await;
    }
    let attempt = new_id("snd");
    let claimed = sqlx::query(
        "UPDATE outbound_emails SET send_attempt_id = ?1, send_started_at = ?2
         WHERE id = ?3 AND workspace_id = ?4 AND status = 'scheduled' AND send_attempt_id IS NULL AND send_at IS ?5",
    )
    .bind(&attempt)
    .bind(now_ms())
    .bind(&id)
    .bind(&ws)
    .bind(send_at)
    .execute(&state.db)
    .await?
    .rows_affected()
        == 1;
    if !claimed {
        return Ok(());
    }
    let release = |state: AppState, id: String, attempt: String| async move {
        let _ = sqlx::query("UPDATE outbound_emails SET send_attempt_id = NULL, send_started_at = NULL WHERE id = ?1 AND send_attempt_id = ?2")
            .bind(id)
            .bind(attempt)
            .execute(&state.db)
            .await;
    };
    let from = match &mailbox.address {
        Some(a) => a.clone(),
        None => match crate::replies::mailbox_address(state, &mailbox.connection_id, &mailbox.config_key).await {
            Ok(a) => a,
            Err(e) => {
                release(state.clone(), id.clone(), attempt).await;
                return Err(job_err(e));
            }
        },
    };
    let mid = attempt_message_id(&attempt);
    let raw = match build_raw_new(&from, &row.to_address, &row.subject, row.in_reply_to.as_deref(), &mid, &row.body_text) {
        Ok(r) => r,
        Err(e) => {
            release(state.clone(), id.clone(), attempt).await;
            return Err(job_err(e));
        }
    };
    let mut message = json!({ "raw": raw });
    if let Some(t) = row.provider_thread_id.as_deref().filter(|t| !t.is_empty()) {
        message["threadId"] = json!(t);
    }
    sqlx::query("UPDATE outbound_emails SET rfc_message_id = ?1 WHERE id = ?2").bind(&mid).bind(&id).execute(&state.db).await?;
    match state.nango.gmail_call(Method::POST, &mailbox.connection_id, &mailbox.config_key, "messages/send", Some(&message)).await {
        Ok(sent) => {
            let message_id = sent.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
            let thread_id = sent.get("threadId").and_then(Value::as_str).map(str::to_string);
            let row = get(state, &ws, &id).await?.ok_or_else(|| JobError::Permanent("email vanished".into()))?;
            finalize_sent(state, &row, &message_id, thread_id.as_deref()).await.map_err(|e| {
                tracing::error!(error = %e, "email sent, but recording it failed");
                JobError::Retry(format!("sent, but recording failed: {e}"))
            })
        }
        Err(NangoError::Unavailable(m)) => Err(JobError::Retry(format!("Gmail's answer was unclear; will confirm before retrying: {m}"))),
        Err(e) => {
            release(state.clone(), id.clone(), attempt).await;
            give_up(state, &row, &format!("Not sent: {e}")).await
        }
    }
}

async fn finalize_sent(state: &AppState, row: &Outbound, provider_message_id: &str, thread_id: Option<&str>) -> AppResult<()> {
    let mut tx = state.db.begin().await?;
    let changed = sqlx::query(
        "UPDATE outbound_emails SET status = 'sent', provider_message_id = ?1, provider_thread_id = COALESCE(?2, provider_thread_id),
                send_at = NULL, send_error = NULL, updated_at = ?3
         WHERE id = ?4 AND status != 'sent'",
    )
    .bind(provider_message_id)
    .bind(thread_id)
    .bind(now_ms())
    .bind(&row.id)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if !changed {
        return Ok(());
    }
    let actor = match row.send_requested_by.as_deref() {
        Some(u) => Actor::User(u),
        None => Actor::System,
    };
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &row.workspace_id,
            actor,
            action: SENT_ACTION,
            entity_type: "outbound_email",
            entity_id: &row.id,
            message_id: None,
            metadata: json!({ "recipient_kind": row.recipient_kind, "auto": row.auto_sent, "authored_by": row.authored_by, "provider_message_id": provider_message_id }),
        },
    )
    .await?;
    let thread = thread_id.or(row.provider_thread_id.as_deref());
    super::watch::email_sent(&mut tx, &row.workspace_id, &row.id, &row.source_id, thread).await?;
    tx.commit().await?;
    if let Some(owner) = row.owner_user_id.as_deref() {
        let held = if row.auto_sent { " I held it for a few minutes first so you could stop it." } else { "" };
        let text = format!("I sent “{}” to {}.{held}", crate::agents::sentence(&row.subject, 80), row.to_label_or_address());
        let links = json!([{ "kind": "sent", "id": row.id, "label": "See what I sent" }]);
        if let Err(e) = super::report(state, &row.workspace_id, owner, &text, &[], &links).await {
            tracing::warn!(error = %e, "could not tell the person about the sent email");
        }
    }
    Ok(())
}

impl Outbound {
    pub fn to_label_or_address(&self) -> String {
        if self.to_label.trim().is_empty() || self.to_label == self.to_address {
            self.to_address.clone()
        } else {
            format!("{} ({})", self.to_label, self.to_address)
        }
    }

    pub fn to_json(&self) -> Value {
        let status = match self.status.as_str() {
            "awaiting_approval" => "needs_you",
            other => other,
        };
        json!({
            "id": self.id, "channel": "email", "to": self.to_label_or_address(), "subject": self.subject, "body": self.body_text,
            "status": status, "reason": self.approval_reason, "error": self.send_error, "auto": self.auto_sent,
            "at": self.send_at.map(iso).unwrap_or_else(|| iso(self.updated_at)),
            "can_stop": self.status == "scheduled" && self.send_attempt_id.is_none(),
            "can_approve": self.status == "awaiting_approval",
        })
    }
}

/// The email will not go out: say why, tell the owner and keep the email for a person to look at.
async fn give_up(state: &AppState, row: &Outbound, reason: &str) -> Result<(), JobError> {
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "UPDATE outbound_emails SET status = 'blocked', send_at = NULL, send_error = ?1, updated_at = ?2
         WHERE id = ?3 AND status = 'scheduled'",
    )
    .bind(truncate_chars(reason, 500))
    .bind(now_ms())
    .bind(&row.id)
    .execute(&mut *tx)
    .await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &row.workspace_id,
            actor: Actor::System,
            action: "message.email_blocked",
            entity_type: "outbound_email",
            entity_id: &row.id,
            message_id: None,
            metadata: json!({ "unconfirmed": row.send_attempt_id.is_some() }),
        },
    )
    .await?;
    tx.commit().await?;
    if let Some(owner) = row.owner_user_id.as_deref() {
        let subject = format!("Email not sent: {}", crate::agents::sentence(&row.subject, 80));
        let link = json!({ "kind": "sent", "id": row.id });
        if let Err(e) = super::inapp::notify_owner(state, &row.workspace_id, owner, row.agent_id.as_deref(), &subject, reason, link, false).await {
            tracing::warn!(error = %e, "could not tell the owner an email was not sent");
        }
    }
    Ok(())
}

/// Dead-lettered `SEND_OUTBOUND`.
pub async fn give_up_scheduled(state: &AppState, payload: &Value, error: &str) -> Result<(), JobError> {
    let field = |k: &str| payload.get(k).and_then(Value::as_str).map(str::to_string);
    let (Some(ws), Some(id)) = (field("workspace_id"), field("outbound_id")) else { return Ok(()) };
    let Some(row) = get(state, &ws, &id).await? else { return Ok(()) };
    if row.status != "scheduled" {
        return Ok(());
    }
    let reason = if row.send_attempt_id.is_some() {
        "We couldn't confirm whether this email went out. Check Gmail's Sent folder before sending it again.".to_string()
    } else {
        format!("Not sent: {error}")
    };
    give_up(state, &row, &reason).await
}

/// The caller's emails (as owner), newest first.
pub async fn list_for(state: &AppState, ws: &str, user_id: &str) -> sqlx::Result<Vec<Outbound>> {
    sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM outbound_emails WHERE workspace_id = ?1 AND owner_user_id = ?2 ORDER BY created_at DESC, id DESC LIMIT 100"
    ))
    .bind(ws)
    .bind(user_id)
    .fetch_all(&state.db)
    .await
}

/// Emails sent on their own today for one person, against the daily cap.
pub async fn auto_sent_today(state: &AppState, ws: &str, owner: &str) -> sqlx::Result<i64> {
    const DAY_MS: i64 = 86_400_000;
    let since = now_ms() / DAY_MS * DAY_MS;
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE workspace_id = ?1 AND action = ?2 AND created_at >= ?3 AND json_extract(metadata_json, '$.owner') = ?4",
    )
    .bind(ws)
    .bind(AUTO_SENT_ACTION)
    .bind(since)
    .bind(owner)
    .fetch_one(&state.db)
    .await
}
