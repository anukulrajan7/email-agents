//! Replying from the connected mailbox: save the draft into Gmail's Drafts, or send it.
//! Threaded into the original conversation and audited. People trigger these from the UI;
//! automations and agents call the same `save_draft` / `send_draft` core, so the guards are shared.
//!
//! Sending is two-step so it can be undone: `send_draft` runs the safety check and schedules the
//! send `SEND_UNDO_SECONDS` ahead; the `SEND_DRAFT` job re-checks and calls Gmail. Each Gmail call
//! carries a durable attempt id (our own Message-ID). If Gmail's answer is ambiguous (timeout, 5xx)
//! the attempt stays open and is reconciled against Gmail before any retry, so a reply is never sent twice.

use std::sync::LazyLock;

use axum::{
    Json,
    extract::{Path, State},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use lettre::message::{Mailbox, Message, header::ContentType};
use regex::Regex;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    ai::policy,
    audit::{self, Actor, Entry},
    auth::{AuthUser, require_role},
    db::{iso, new_id, now_ms},
    domain::{Capabilities, Role},
    error::{AppError, AppResult},
    http::AppState,
    jobs::{self, handlers::JobError, kind},
    send_safety::{self, CheckInput},
    sources::nango::NangoError,
};

#[derive(sqlx::FromRow)]
pub struct ReplyContext {
    pub draft_id: String,
    pub message_id: String,
    pub body_text: String,
    pub status: String,
    pub provider_draft_id: Option<String>,
    pub send_at: Option<i64>,
    pub send_requested_by: Option<String>,
    pub send_attempt_id: Option<String>,
    pub send_started_at: Option<i64>,
    pub subject: String,
    pub sender_address: String,
    pub rfc_message_id: Option<String>,
    pub thread_id: String,
    pub direction: String,
    pub original_body: String,
    pub source_id: String,
    pub provider_thread_id: Option<String>,
    pub source_kind: String,
    pub capabilities_json: String,
    pub connection_id: Option<String>,
    pub provider_config_key: Option<String>,
}

pub async fn load(state: &AppState, ws: &str, draft_id: &str) -> AppResult<ReplyContext> {
    sqlx::query_as(
        "SELECT d.id AS draft_id, d.message_id, d.body_text, d.status, d.provider_draft_id,
                d.send_at, d.send_requested_by, d.send_attempt_id, d.send_started_at,
                m.subject, m.sender_address, m.rfc_message_id, m.thread_id, m.direction, m.body_text AS original_body,
                m.source_id, t.provider_thread_id,
                s.kind AS source_kind, s.capabilities_json, s.external_connection_id AS connection_id, s.provider_config_key
         FROM drafts d
         JOIN email_messages m ON m.id = d.message_id
         JOIN email_threads t ON t.id = m.thread_id
         JOIN email_sources s ON s.id = m.source_id
         WHERE d.id = ?1 AND d.workspace_id = ?2",
    )
    .bind(draft_id)
    .bind(ws)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)
}

pub fn capabilities(ctx: &ReplyContext) -> Capabilities {
    serde_json::from_str(&ctx.capabilities_json).unwrap_or_else(|_| crate::sources::capabilities(&ctx.source_kind))
}

pub fn connection(ctx: &ReplyContext) -> AppResult<(&str, &str)> {
    match (ctx.connection_id.as_deref(), ctx.provider_config_key.as_deref()) {
        (Some(c), Some(p)) => Ok((c, p)),
        _ => Err(AppError::Conflict("this email didn't come from a connected mailbox".into())),
    }
}

/// Longest subject we put on a reply (characters).
pub const MAX_SUBJECT_CHARS: usize = 250;

/// One header line: CR/LF and other control characters (from untrusted inbound mail) become spaces.
pub fn header_safe(value: &str) -> String {
    value.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().to_string()
}

pub fn reply_subject(subject: &str) -> String {
    let s = header_safe(subject);
    let s = if s.to_lowercase().starts_with("re:") { s } else { format!("Re: {s}") };
    crate::sources::normalize::truncate_chars(&s, MAX_SUBJECT_CHARS)
}

static MSG_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^<[^\s<>]+>$").expect("valid regex"));

/// `In-Reply-To` / `References` value, only when it is a well-formed `<id>`; anything else is dropped.
pub fn safe_message_id(raw: &str) -> Option<String> {
    let v = raw.trim();
    (v.len() <= 998 && MSG_ID.is_match(v)).then(|| v.to_string())
}

/// RFC 5322 reply, base64url-encoded for Gmail's `raw` field.
pub fn build_raw(from: &str, ctx_to: &str, subject: &str, in_reply_to: Option<&str>, body: &str) -> AppResult<String> {
    build_raw_with_id(from, ctx_to, subject, in_reply_to, None, body)
}

/// Like `build_raw`, with our own `Message-ID` (the send attempt id) so the send can be found in Gmail later.
pub fn build_raw_with_id(
    from: &str,
    ctx_to: &str,
    subject: &str,
    in_reply_to: Option<&str>,
    message_id: Option<&str>,
    body: &str,
) -> AppResult<String> {
    let bad = |e: String| AppError::BadRequest(e);
    let from: Mailbox = header_safe(from).parse().map_err(|_| bad("connected mailbox address is invalid".into()))?;
    let to: Mailbox = header_safe(ctx_to).parse().map_err(|_| bad("the original sender's address is invalid".into()))?;
    let mut builder = Message::builder().from(from).to(to).subject(reply_subject(subject));
    if let Some(id) = in_reply_to.and_then(safe_message_id) {
        builder = builder.in_reply_to(id.clone()).references(id);
    }
    if let Some(id) = message_id.and_then(safe_message_id) {
        builder = builder.message_id(Some(id));
    }
    let msg = builder
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_string())
        .map_err(|e| AppError::Internal(anyhow::anyhow!("building reply: {e}")))?;
    Ok(URL_SAFE_NO_PAD.encode(msg.formatted()))
}

pub async fn mailbox_address(state: &AppState, conn: &str, pck: &str) -> AppResult<String> {
    let profile = state.nango.gmail_call(Method::GET, conn, pck, "profile", None).await?;
    profile
        .get("emailAddress")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or(AppError::Unavailable)
}

async fn raw_for(state: &AppState, ctx: &ReplyContext, conn: &str, pck: &str, message_id: Option<&str>) -> AppResult<Value> {
    if ctx.body_text.trim().is_empty() {
        return Err(AppError::BadRequest("the draft is empty".into()));
    }
    let from = mailbox_address(state, conn, pck).await?;
    let raw = build_raw_with_id(&from, &ctx.sender_address, &ctx.subject, ctx.rfc_message_id.as_deref(), message_id, &ctx.body_text)?;
    let mut message = json!({ "raw": raw });
    if let Some(t) = &ctx.provider_thread_id {
        message["threadId"] = json!(t);
    }
    Ok(message)
}

/// Saves (or updates) the reply in the mailbox's Drafts folder. Returns the provider draft id.
pub async fn save_draft(state: &AppState, ws: &str, draft_id: &str, actor: Actor<'_>) -> AppResult<String> {
    let ctx = load(state, ws, draft_id).await?;
    if !policy::can_create_provider_draft(&capabilities(&ctx)) {
        return Err(AppError::Conflict("this mailbox can't hold drafts; copy the text instead".into()));
    }
    if ctx.status == "sent" {
        return Err(AppError::Conflict("this reply was already sent".into()));
    }
    if ctx.send_at.is_some() || ctx.send_attempt_id.is_some() {
        return Err(AppError::Conflict("this reply is being sent; undo the send first".into()));
    }
    let (conn, pck) = connection(&ctx)?;
    let message = raw_for(state, &ctx, conn, pck, None).await?;
    let result = match &ctx.provider_draft_id {
        Some(existing) => {
            state.nango.gmail_call(Method::PUT, conn, pck, &format!("drafts/{existing}"), Some(&json!({ "message": message }))).await
        }
        None => state.nango.gmail_call(Method::POST, conn, pck, "drafts", Some(&json!({ "message": message }))).await,
    }?;
    let provider_draft_id = result.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query("UPDATE drafts SET provider_draft_id = ?1, updated_at = ?2 WHERE id = ?3")
        .bind(&provider_draft_id)
        .bind(now_ms())
        .bind(&ctx.draft_id)
        .execute(&mut *tx)
        .await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: ws,
            actor,
            action: "draft.saved_to_mailbox",
            entity_type: "draft",
            entity_id: &ctx.draft_id,
            message_id: Some(&ctx.message_id),
            metadata: json!({ "provider_draft_id": provider_draft_id }),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(provider_draft_id)
}

pub async fn save_to_provider(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    crate::team::scope::require_draft(&state.db, &ws, &user, &id).await?;
    let provider_draft_id = save_draft(&state, &ws, &id, Actor::User(&user.id)).await?;
    Ok(Json(json!({ "status": "saved", "provider_draft_id": provider_draft_id })))
}

#[derive(Deserialize)]
pub struct SendBody {
    /// The UI sends `true` only after the person confirms recipient + subject.
    confirm: bool,
}

/// True while the draft still has a gap the AI could not fill ("[NEEDS: ...]").
pub fn has_open_gaps(body: &str) -> bool {
    body.contains("[NEEDS:")
}

/// Result of `send_draft`: the reply goes out at `send_at` unless someone undoes it.
#[derive(Debug, Clone, Serialize)]
pub struct ScheduledSend {
    pub status: &'static str,
    pub draft_id: String,
    pub send_at: String,
    #[serde(skip)]
    pub send_at_ms: i64,
}

// ---------------------------------------------------------------- reconciliation

/// Our Message-ID for one send attempt.
pub fn attempt_message_id(attempt_id: &str) -> String {
    format!("<{attempt_id}@postroom.mail>")
}

/// An open attempt younger than this may not be visible in Gmail yet: never call it "not sent".
pub const RECONCILE_GRACE_MS: i64 = 120_000;

#[derive(Debug, Clone, PartialEq)]
pub enum Reconciled {
    /// Gmail has it (provider message id).
    Sent(String),
    /// Safe to try again.
    NotSent,
    /// Can't tell yet; don't send.
    Unsure(String),
}

/// Decides from a Gmail thread (format=metadata) whether our attempt went out: our Message-ID is
/// there, or a SENT message appeared after the attempt started (conservative: never resend then).
pub fn sent_in_thread(thread: &Value, message_id: &str, started_at: i64) -> Option<String> {
    let messages = thread.get("messages").and_then(Value::as_array)?;
    messages.iter().find_map(|m| {
        let id = m.get("id").and_then(Value::as_str)?.to_string();
        let ours = m
            .pointer("/payload/headers")
            .and_then(Value::as_array)
            .is_some_and(|hs| {
                hs.iter().any(|h| {
                    h.get("name").and_then(Value::as_str).is_some_and(|n| n.eq_ignore_ascii_case("message-id"))
                        && h.get("value").and_then(Value::as_str).is_some_and(|v| v.trim().eq_ignore_ascii_case(message_id))
                })
            });
        let sent_after = m.get("labelIds").and_then(Value::as_array).is_some_and(|l| l.iter().any(|x| x == "SENT"))
            && m.get("internalDate")
                .and_then(|d| d.as_str().and_then(|s| s.parse::<i64>().ok()).or_else(|| d.as_i64()))
                .is_some_and(|at| at >= started_at - 60_000);
        (ours || sent_after).then_some(id)
    })
}

pub async fn reconcile(state: &AppState, ctx: &ReplyContext) -> Reconciled {
    let Some(attempt) = ctx.send_attempt_id.as_deref() else { return Reconciled::NotSent };
    let started = ctx.send_started_at.unwrap_or(0);
    let Ok((conn, pck)) = connection(ctx) else { return Reconciled::Unsure("no mailbox connection".into()) };
    let mid = attempt_message_id(attempt);
    let query = format!("rfc822msgid:{} in:anywhere", mid.trim_matches(['<', '>']));
    match state.nango.gmail_list_ids(conn, pck, &query, None).await {
        Ok((ids, _)) => {
            if let Some(id) = ids.into_iter().next() {
                return Reconciled::Sent(id);
            }
        }
        Err(e) => return Reconciled::Unsure(format!("Gmail search failed: {e}")),
    }
    if let Some(tid) = ctx.provider_thread_id.as_deref().filter(|t| !t.is_empty() && t.chars().all(|c| c.is_ascii_alphanumeric())) {
        let path = format!("threads/{tid}?format=metadata&metadataHeaders=Message-ID");
        match state.nango.gmail_call(Method::GET, conn, pck, &path, None).await {
            Ok(thread) => {
                if let Some(id) = sent_in_thread(&thread, &mid, started) {
                    return Reconciled::Sent(id);
                }
            }
            Err(e) => return Reconciled::Unsure(format!("Gmail thread read failed: {e}")),
        }
    }
    if now_ms() - started < RECONCILE_GRACE_MS {
        return Reconciled::Unsure("the last attempt is too recent to be sure".into());
    }
    Reconciled::NotSent
}

async fn clear_attempt(state: &AppState, draft_id: &str, attempt: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE drafts SET send_attempt_id = NULL, send_started_at = NULL WHERE id = ?1 AND send_attempt_id = ?2 AND status != 'sent'")
        .bind(draft_id)
        .bind(attempt)
        .execute(&state.db)
        .await?;
    Ok(())
}

/// Records a confirmed send (idempotent): status, audit, indexing, learning.
async fn finalize_sent(state: &AppState, ws: &str, ctx: &ReplyContext, provider_message_id: &str, actor: Actor<'_>) -> AppResult<()> {
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let changed = sqlx::query(
        "UPDATE drafts SET status = 'sent', provider_message_id = ?1, send_at = NULL, send_error = NULL, updated_at = ?2
         WHERE id = ?3 AND status != 'sent'",
    )
    .bind(provider_message_id)
    .bind(now_ms())
    .bind(&ctx.draft_id)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if !changed {
        return Ok(());
    }
    jobs::knowledge_jobs::enqueue_index_reply(&mut tx, ws, &ctx.draft_id, "sent").await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: ws,
            actor,
            action: "draft.sent",
            entity_type: "draft",
            entity_id: &ctx.draft_id,
            message_id: Some(&ctx.message_id),
            metadata: json!({ "to": "original_sender", "provider_message_id": provider_message_id }),
        },
    )
    .await?;
    let promised: Option<String> = sqlx::query_scalar("SELECT unanswered_json FROM drafts WHERE id = ?1").bind(&ctx.draft_id).fetch_optional(&mut *tx).await?;
    let promised = promised.map(|j| crate::gap_resolver::parse_unanswered(&j)).unwrap_or_default();
    if !promised.is_empty() && ctx.body_text.to_lowercase().contains("we'll confirm") {
        let owner: Option<String> = sqlx::query_scalar("SELECT connected_by_user_id FROM email_sources WHERE id = ?1").bind(&ctx.source_id).fetch_optional(&mut *tx).await?.flatten();
        let title = crate::sources::normalize::truncate_chars(&format!("Follow up: you told {} we'd confirm {}", ctx.sender_address, promised.join(", ")), 200);
        sqlx::query(
            "INSERT INTO review_items (id, workspace_id, message_id, workflow_run_id, kind, title, status, assigned_user_id, created_at)
             VALUES (?1, ?2, ?3, NULL, 'agent_task', ?4, 'open', ?5, ?6)",
        )
        .bind(new_id("rev"))
        .bind(ws)
        .bind(&ctx.message_id)
        .bind(title)
        .bind(owner)
        .bind(now_ms())
        .execute(&mut *tx)
        .await?;
    }
    if ctx.body_text.to_lowercase().contains(&crate::gap_resolver::NONE_PHRASE.to_lowercase()) {
        crate::candidate_watch::queue_enrol(&mut tx, ws, &ctx.message_id, &ctx.draft_id).await?;
    }
    tx.commit().await?;
    if let Err(e) = jobs::learning_jobs::enqueue_reply_learning(&state.db, &ctx.draft_id).await {
        tracing::warn!(error = %e, "could not schedule learning from the sent reply");
    }
    state.jobs_notify.notify_one();
    Ok(())
}

// ---------------------------------------------------------------- the actual Gmail send

#[derive(Debug)]
pub enum SendFailure {
    /// Nothing went out; the attempt was released. `retry` = worth trying again later.
    NotSent { error: String, retry: bool },
    /// Gmail's answer was ambiguous; the attempt stays open until reconciled.
    Unknown(String),
    /// Someone else holds the send (another attempt, undone, already sent).
    Skipped,
}

/// Sends now, in the original thread, under a fresh attempt id. Callers have run the safety check.
pub async fn send_now(state: &AppState, ws: &str, ctx: &ReplyContext, actor: Actor<'_>) -> Result<String, SendFailure> {
    let not_sent = |e: AppError, retry| SendFailure::NotSent { error: e.to_string(), retry };
    let (conn, pck) = connection(ctx).map_err(|e| not_sent(e, false))?;
    let attempt = new_id("snd");
    let claimed = sqlx::query(
        "UPDATE drafts SET send_attempt_id = ?1, send_started_at = ?2
         WHERE id = ?3 AND workspace_id = ?4 AND status != 'sent' AND send_attempt_id IS NULL AND send_at IS ?5",
    )
    .bind(&attempt)
    .bind(now_ms())
    .bind(&ctx.draft_id)
    .bind(ws)
    .bind(ctx.send_at)
    .execute(&state.db)
    .await
    .map_err(|e| not_sent(e.into(), true))?
    .rows_affected()
        == 1;
    if !claimed {
        return Err(SendFailure::Skipped);
    }
    let attempt_ref = attempt.as_str();
    let release = |error: AppError, retry: bool| async move {
        if let Err(e) = clear_attempt(state, &ctx.draft_id, attempt_ref).await {
            tracing::error!(error = %e, "could not release a send attempt");
        }
        SendFailure::NotSent { error: error.to_string(), retry }
    };
    let mid = attempt_message_id(&attempt);
    // Everything before the final send call can't have sent anything.
    let message = match raw_for(state, ctx, conn, pck, Some(&mid)).await {
        Ok(m) => m,
        Err(e) => {
            let retry = matches!(e, AppError::Unavailable);
            return Err(release(e, retry).await);
        }
    };
    let (path, body) = match &ctx.provider_draft_id {
        Some(gid) => {
            if let Err(e) =
                state.nango.gmail_call(Method::PUT, conn, pck, &format!("drafts/{gid}"), Some(&json!({ "message": message }))).await
            {
                let retry = matches!(e, NangoError::Unavailable(_));
                return Err(release(e.into(), retry).await);
            }
            ("drafts/send", json!({ "id": gid }))
        }
        None => ("messages/send", message),
    };
    match state.nango.gmail_call(Method::POST, conn, pck, path, Some(&body)).await {
        Ok(sent) => {
            let provider_message_id = sent.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
            if let Err(e) = finalize_sent(state, ws, ctx, &provider_message_id, actor).await {
                // Sent, but not recorded: leave the attempt open so the next run reconciles it as sent.
                tracing::error!(error = %e, "reply sent but recording it failed");
                return Err(SendFailure::Unknown(format!("sent, but recording failed: {e}")));
            }
            Ok(provider_message_id)
        }
        // Timeout / 5xx / 429 / bad response body: Gmail may have sent it.
        Err(NangoError::Unavailable(m)) => Err(SendFailure::Unknown(m)),
        // A clear rejection (4xx) before anything went out.
        Err(e) => Err(release(e.into(), false).await),
    }
}

// ---------------------------------------------------------------- schedule / undo

fn requested_by<'a>(actor: Actor<'a>) -> Option<&'a str> {
    match actor {
        Actor::User(u) => Some(u),
        Actor::System => None,
    }
}

/// Checks the reply and schedules it `SEND_UNDO_SECONDS` ahead. Every send path (person,
/// automation, agent approval) goes through here, so these guards cannot be bypassed.
pub async fn send_draft(state: &AppState, ws: &str, draft_id: &str, actor: Actor<'_>) -> AppResult<ScheduledSend> {
    send_draft_held(state, ws, draft_id, actor, 0).await
}

/// Like `send_draft`, with the send held at least `hold_secs` from now. The hold is part of the same
/// claim, so the reply is never scheduled earlier than the hold.
async fn record_send_blocked(state: &AppState, ws: &str, ctx: &ReplyContext, report: &send_safety::SafetyReport, automated: bool) {
    let agent_id: Option<String> = if automated {
        sqlx::query_scalar("SELECT agent_id FROM drafts WHERE id = ?1 AND workspace_id = ?2")
            .bind(&ctx.draft_id)
            .bind(ws)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten()
            .flatten()
    } else {
        None
    };
    let codes: Vec<&str> = report.blocking.iter().map(|i| i.code).collect();
    let severity = crate::incidents::send_block_severity(&codes);
    crate::incidents::record(
        &state.db,
        crate::incidents::NewIncident::new(ws, crate::incidents::Kind::SendBlocked, serde_json::json!({"codes": codes, "automated": automated}))
            .severity(severity)
            .agent(agent_id.as_deref())
            .message(Some(&ctx.message_id))
            .dedupe(format!("blocked:{}:{}", ctx.draft_id, codes.join(","))),
    )
    .await;
}

pub async fn send_draft_held(state: &AppState, ws: &str, draft_id: &str, actor: Actor<'_>, hold_secs: i64) -> AppResult<ScheduledSend> {
    let mut ctx = load(state, ws, draft_id).await?;
    if ctx.status == "sent" {
        return Err(AppError::Conflict("this reply was already sent".into()));
    }
    if ctx.send_at.is_some() {
        return Err(AppError::Conflict("this reply is already scheduled to send".into()));
    }
    // Text that can never go out is refused first, whatever the source.
    if let Some(gaps) = send_safety::check_gaps(&ctx.body_text) {
        return Err(send_safety::SafetyReport { blocking: vec![gaps], warnings: vec![] }.into_error());
    }
    if !policy::can_send(&capabilities(&ctx)) {
        return Err(AppError::Conflict("this source can't send; copy the draft into your mail client".into()));
    }
    connection(&ctx)?;
    if let Some(attempt) = ctx.send_attempt_id.clone() {
        match reconcile(state, &ctx).await {
            Reconciled::Sent(id) => {
                finalize_sent(state, ws, &ctx, &id, actor).await?;
                return Err(AppError::Conflict("this reply was already sent".into()));
            }
            Reconciled::NotSent => {
                clear_attempt(state, &ctx.draft_id, &attempt).await?;
                ctx = load(state, ws, draft_id).await?;
            }
            Reconciled::Unsure(why) => {
                tracing::warn!(draft_id, %why, "previous send attempt not reconciled yet");
                return Err(AppError::Conflict(
                    "we're still confirming whether the previous send went out; check Gmail's Sent folder or try again in a few minutes".into(),
                ));
            }
        }
    }
    let automated = matches!(actor, Actor::System);
    let mut report = send_safety::check(state, &CheckInput { actor_user: requested_by(actor), workspace_id: ws, reply: &ctx, body: &ctx.body_text, automated }).await?;
    // An automatic send gets one repair of an unsupported claim; a person's send never needs it (warning only).
    if report.is_blocked() && automated && crate::repair::apply(state, ws, &ctx, &report).await? {
        ctx = load(state, ws, draft_id).await?;
        report = send_safety::check(state, &CheckInput { actor_user: requested_by(actor), workspace_id: ws, reply: &ctx, body: &ctx.body_text, automated }).await?;
    }
    if report.is_blocked() {
        if automated {
            crate::repair::hand_off(state, ws, &ctx, &report).await?;
        }
        record_send_blocked(state, ws, &ctx, &report, automated).await;
        return Err(report.into_error());
    }
    let now = now_ms();
    let send_at = now + state.config.send_undo_seconds.max(hold_secs).max(0) * 1_000;
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let claimed = sqlx::query(
        "UPDATE drafts SET send_at = ?1, send_requested_by = ?2, send_error = NULL, updated_at = ?3
         WHERE id = ?4 AND workspace_id = ?5 AND status != 'sent' AND send_at IS NULL AND send_attempt_id IS NULL",
    )
    .bind(send_at)
    .bind(requested_by(actor))
    .bind(now)
    .bind(&ctx.draft_id)
    .bind(ws)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if !claimed {
        return Err(AppError::Conflict("this reply is already being sent".into()));
    }
    jobs::enqueue_at(
        &mut tx,
        kind::SEND_DRAFT,
        &format!("send_draft:{}:{send_at}", ctx.draft_id),
        Some(ws),
        &json!({ "workspace_id": ws, "draft_id": ctx.draft_id, "send_at": send_at }),
        jobs::PROVIDER_MAX_ATTEMPTS,
        send_at,
    )
    .await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: ws,
            actor,
            action: "draft.send_scheduled",
            entity_type: "draft",
            entity_id: &ctx.draft_id,
            message_id: Some(&ctx.message_id),
            metadata: json!({ "send_at": send_at, "warnings": report.warnings.iter().map(|w| w.code).collect::<Vec<_>>() }),
        },
    )
    .await?;
    tx.commit().await?;
    state.jobs_notify.notify_one();
    Ok(ScheduledSend { status: "scheduled", draft_id: ctx.draft_id, send_at: iso(send_at), send_at_ms: send_at })
}

/// Checks and schedules the reply; it goes out after the undo window.
pub async fn send(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
    Json(body): Json<SendBody>,
) -> AppResult<Json<ScheduledSend>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    crate::team::scope::require_draft(&state.db, &ws, &user, &id).await?;
    if !body.confirm {
        return Err(AppError::BadRequest("sending needs explicit confirmation".into()));
    }
    Ok(Json(send_draft(&state, &ws, &id, Actor::User(&user.id)).await?))
}

/// Cancels a scheduled send. 409 once Gmail has been called (sending or sent).
pub async fn undo_send(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    crate::team::scope::require_draft(&state.db, &ws, &user, &id).await?;
    // Read before the update clears it: an agent's own send (nobody asked for it) that a person stops is a learning signal.
    let sender: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT message_id, agent_id, send_requested_by FROM drafts WHERE id = ?1 AND workspace_id = ?2 AND send_at IS NOT NULL",
    )
    .bind(&id)
    .bind(&ws)
    .fetch_optional(&state.db)
    .await?;
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let undone = sqlx::query(
        "UPDATE drafts SET send_at = NULL, send_requested_by = NULL, updated_at = ?1
         WHERE id = ?2 AND workspace_id = ?3 AND status != 'sent' AND send_at IS NOT NULL AND send_attempt_id IS NULL",
    )
    .bind(now_ms())
    .bind(&id)
    .bind(&ws)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if !undone {
        let row: Option<(String, Option<i64>, Option<String>)> =
            sqlx::query_as("SELECT status, send_at, send_attempt_id FROM drafts WHERE id = ?1 AND workspace_id = ?2")
                .bind(&id)
                .bind(&ws)
                .fetch_optional(&mut *tx)
                .await?;
        return Err(match row {
            None => AppError::NotFound,
            Some((s, _, _)) if s == "sent" => AppError::Conflict("too late: this reply was already sent".into()),
            Some((_, _, Some(_))) => AppError::Conflict("too late: this reply is being sent".into()),
            Some(_) => AppError::Conflict("this reply isn't scheduled to send".into()),
        });
    }
    let message_id: String = sqlx::query_scalar("SELECT message_id FROM drafts WHERE id = ?1").bind(&id).fetch_one(&mut *tx).await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &ws,
            actor: Actor::User(&user.id),
            action: "draft.send_undone",
            entity_type: "draft",
            entity_id: &id,
            message_id: Some(&message_id),
            metadata: json!({}),
        },
    )
    .await?;
    if let Some((_, Some(agent_id), None)) = &sender {
        crate::learning::hooks::auto_send_stopped(&mut tx, &ws, &user.id, &id, &message_id, Some(agent_id)).await?;
    }
    tx.commit().await?;
    Ok(Json(json!({ "status": "cancelled", "draft_id": id })))
}

// ---------------------------------------------------------------- the SEND_DRAFT job

fn job_err(e: AppError) -> JobError {
    match e {
        AppError::Unavailable | AppError::AiUnavailable | AppError::Internal(_) => JobError::Retry(e.to_string()),
        other => JobError::Permanent(other.to_string()),
    }
}

/// The scheduled send didn't go out: clear the schedule, say why on the draft, audit, and alert the team.
async fn hold(state: &AppState, ws: &str, ctx: &ReplyContext, action: &str, reason: &str, metadata: Value) -> Result<(), JobError> {
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query(
        "UPDATE drafts SET send_at = NULL, send_requested_by = NULL, send_error = ?1, updated_at = ?2
         WHERE id = ?3 AND status != 'sent'",
    )
    .bind(crate::sources::normalize::truncate_chars(reason, 500))
    .bind(now_ms())
    .bind(&ctx.draft_id)
    .execute(&mut *tx)
    .await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: ws,
            actor: Actor::System,
            action,
            entity_type: "draft",
            entity_id: &ctx.draft_id,
            message_id: Some(&ctx.message_id),
            metadata,
        },
    )
    .await?;
    tx.commit().await?;
    let alert = crate::alerts::NewAlert {
        workspace_id: ws,
        kind: crate::alerts::AUTOMATION,
        severity: "high",
        title: format!("Reply not sent: {}", ctx.subject),
        body: reason.to_string(),
        message_id: Some(&ctx.message_id),
        level: 1,
        dedupe_key: Some(format!("send_held:{}:{}", ctx.draft_id, ctx.send_at.unwrap_or_default())),
    };
    if let Err(e) = crate::alerts::raise(&state.db, alert).await {
        tracing::warn!(error = %e, "could not raise the held-send alert");
    }
    Ok(())
}

/// `SEND_DRAFT`: at `send_at`, reconcile any open attempt, re-run the safety check, then send.
pub async fn send_scheduled(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let field = |k: &str| payload.get(k).and_then(Value::as_str).map(str::to_string);
    let (Some(ws), Some(draft_id), Some(send_at)) =
        (field("workspace_id"), field("draft_id"), payload.get("send_at").and_then(Value::as_i64))
    else {
        return Err(JobError::Permanent("payload missing workspace_id/draft_id/send_at".into()));
    };
    let mut ctx = match load(state, &ws, &draft_id).await {
        Ok(c) => c,
        Err(AppError::NotFound) => return Ok(()),
        Err(e) => return Err(job_err(e)),
    };
    // Undone, rescheduled, or already sent: this job is stale.
    if ctx.status == "sent" || ctx.send_at != Some(send_at) {
        return Ok(());
    }
    let requester = ctx.send_requested_by.clone();
    let actor = match requester.as_deref() {
        Some(u) => Actor::User(u),
        None => Actor::System,
    };
    if let Some(attempt) = ctx.send_attempt_id.clone() {
        match reconcile(state, &ctx).await {
            Reconciled::Sent(id) => return finalize_sent(state, &ws, &ctx, &id, actor).await.map_err(job_err),
            Reconciled::NotSent => {
                clear_attempt(state, &draft_id, &attempt).await?;
                ctx = load(state, &ws, &draft_id).await.map_err(job_err)?;
            }
            Reconciled::Unsure(why) => return Err(JobError::Retry(format!("confirming the previous send attempt: {why}"))),
        }
    }
    if requester.is_none()
        && let Some(reason) = crate::candidate_watch::stop_reason(state, &ws, &ctx.draft_id).await.map_err(|e| job_err(e.into()))?
    {
        tracing::info!(draft_id = %ctx.draft_id, reason, "a watch email is cancelled");
        crate::candidate_watch::discard(state, &ws, &ctx.draft_id).await.map_err(|e| job_err(e.into()))?;
        return Ok(());
    }
    if requester.is_none()
        && let Some(reason) = crate::agents::autonomy::automated_send_stopped(state, &ws, &ctx.draft_id).await.map_err(|e| job_err(e.into()))?
    {
        sqlx::query("UPDATE drafts SET send_at = NULL, updated_at = ?1 WHERE id = ?2 AND workspace_id = ?3 AND send_attempt_id IS NULL")
            .bind(now_ms())
            .bind(&ctx.draft_id)
            .bind(&ws)
            .execute(&state.db)
            .await
            .map_err(|e| job_err(e.into()))?;
        tracing::info!(draft_id = %ctx.draft_id, reason, "held automatic send cancelled; it needs a person");
        return Ok(());
    }
    let input = CheckInput { actor_user: requester.as_deref(), workspace_id: &ws, reply: &ctx, body: &ctx.body_text, automated: requester.is_none() };
    let report = send_safety::check(state, &input).await.map_err(job_err)?;
    if report.is_blocked() {
        let reasons: Vec<&str> = report.blocking.iter().map(|i| i.message.as_str()).collect();
        let codes: Vec<&str> = report.blocking.iter().map(|i| i.code).collect();
        let reason = format!("Not sent: {}", reasons.join(" "));
        return hold(state, &ws, &ctx, "draft.send_blocked", &reason, json!({ "codes": codes })).await;
    }
    match send_now(state, &ws, &ctx, actor).await {
        Ok(_) | Err(SendFailure::Skipped) => Ok(()),
        Err(SendFailure::NotSent { error, retry: true }) => Err(JobError::Retry(error)),
        Err(SendFailure::NotSent { error, retry: false }) => Err(JobError::Permanent(error)),
        Err(SendFailure::Unknown(e)) => Err(JobError::Retry(format!("Gmail's answer was unclear; will confirm before retrying: {e}"))),
    }
}

/// Dead-lettered `SEND_DRAFT`: stop retrying, keep any open attempt (the next send reconciles it).
pub async fn give_up_scheduled(state: &AppState, payload: &Value, error: &str) -> Result<(), JobError> {
    let field = |k: &str| payload.get(k).and_then(Value::as_str).map(str::to_string);
    let (Some(ws), Some(draft_id)) = (field("workspace_id"), field("draft_id")) else { return Ok(()) };
    let ctx = match load(state, &ws, &draft_id).await {
        Ok(c) => c,
        Err(AppError::NotFound) => return Ok(()),
        Err(e) => return Err(job_err(e)),
    };
    if ctx.status == "sent" || ctx.send_at != payload.get("send_at").and_then(Value::as_i64) {
        return Ok(());
    }
    let reason = if ctx.send_attempt_id.is_some() {
        "We couldn't confirm whether this reply went out. Check Gmail's Sent folder before sending again.".to_string()
    } else {
        format!("Not sent: {error}")
    };
    hold(state, &ws, &ctx, "draft.send_failed", &reason, json!({ "unconfirmed": ctx.send_attempt_id.is_some() })).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(raw: &str) -> String {
        String::from_utf8(URL_SAFE_NO_PAD.decode(raw).unwrap()).unwrap()
    }

    #[test]
    fn builds_threaded_reply() {
        assert_eq!(reply_subject("Timesheet"), "Re: Timesheet");
        assert_eq!(reply_subject("RE: Timesheet"), "RE: Timesheet");
        let raw = build_raw("me@acme.com", "Priya <priya@acme.com>", "Timesheet", Some("<abc@mail>"), "Thanks!").unwrap();
        let text = decode(&raw);
        assert!(text.contains("Subject: Re: Timesheet"));
        assert!(text.contains("In-Reply-To: <abc@mail>"));
        assert!(text.contains("References: <abc@mail>"));
        assert!(text.contains("To: Priya <priya@acme.com>"));
        assert!(text.ends_with("Thanks!") || text.contains("\r\n\r\nThanks!"));
    }

    #[test]
    fn headers_from_inbound_mail_are_sanitized() {
        // CR/LF in the inbound subject can't inject headers.
        let evil = "Hello\r\nBcc: attacker@evil.com\r\nX-Evil: 1";
        assert!(!reply_subject(evil).contains('\n') && !reply_subject(evil).contains('\r'));
        let raw = build_raw("me@acme.com", "priya@acme.com", evil, Some("<ok@id>\r\nBcc: x@evil.com"), "Body text here").unwrap();
        let text = decode(&raw);
        assert!(!text.contains("\r\nBcc:") && !text.contains("\r\nX-Evil:"), "{text}");
        assert!(!text.contains("In-Reply-To"), "an invalid In-Reply-To is dropped");
        // Subject is capped.
        assert_eq!(reply_subject(&"x".repeat(1_000)).chars().count(), MAX_SUBJECT_CHARS);
        // Message-ID validation.
        assert_eq!(safe_message_id(" <a.b@c> "), Some("<a.b@c>".into()));
        for bad in ["a@b", "<a b@c>", "<a@b>\r\nX: y", "<<a@b>>", "<>", ""] {
            assert_eq!(safe_message_id(bad), None, "{bad:?}");
        }
        // Our attempt Message-ID is set.
        let raw = build_raw_with_id("me@acme.com", "p@acme.com", "S", None, Some(&attempt_message_id("snd_1")), "Body").unwrap();
        assert!(decode(&raw).contains("Message-ID: <snd_1@postroom.mail>"));
    }

    #[test]
    fn reconciles_from_thread_metadata() {
        let mid = attempt_message_id("snd_9");
        let thread = json!({"messages": [
            {"id": "g1", "labelIds": ["INBOX"], "internalDate": "1000", "payload": {"headers": [{"name": "Message-ID", "value": "<x@y>"}]}},
            {"id": "g2", "labelIds": ["SENT"], "internalDate": "5000", "payload": {"headers": [{"name": "Message-Id", "value": mid}]}}
        ]});
        assert_eq!(sent_in_thread(&thread, &mid, 10_000_000), Some("g2".into()), "our Message-ID found");
        let other = json!({"messages": [{"id": "g3", "labelIds": ["SENT"], "internalDate": "200000", "payload": {"headers": []}}]});
        assert_eq!(sent_in_thread(&other, &mid, 180_000), Some("g3".into()), "a SENT message after the attempt counts");
        assert_eq!(sent_in_thread(&other, &mid, 500_000), None, "older sent mail doesn't");
        assert_eq!(sent_in_thread(&json!({}), &mid, 0), None);
    }
}
