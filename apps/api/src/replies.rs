//! Replying from the connected mailbox: save the draft into Gmail's Drafts, or send it.
//! Both are explicit user actions (never automatic), threaded into the original
//! conversation, and audited.

use axum::{
    Json,
    extract::{Path, State},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use lettre::message::{Mailbox, Message, header::ContentType};
use reqwest::Method;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    ai::policy,
    audit::{self, Actor, Entry},
    auth::{AuthUser, require_role},
    db::now_ms,
    domain::{Capabilities, Role},
    error::{AppError, AppResult},
    http::AppState,
};

#[derive(sqlx::FromRow)]
struct ReplyContext {
    draft_id: String,
    message_id: String,
    body_text: String,
    status: String,
    provider_draft_id: Option<String>,
    subject: String,
    sender_address: String,
    rfc_message_id: Option<String>,
    provider_thread_id: Option<String>,
    source_kind: String,
    capabilities_json: String,
    connection_id: Option<String>,
    provider_config_key: Option<String>,
}

async fn load(state: &AppState, ws: &str, draft_id: &str) -> AppResult<ReplyContext> {
    sqlx::query_as(
        "SELECT d.id AS draft_id, d.message_id, d.body_text, d.status, d.provider_draft_id,
                m.subject, m.sender_address, m.rfc_message_id, t.provider_thread_id,
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

fn capabilities(ctx: &ReplyContext) -> Capabilities {
    serde_json::from_str(&ctx.capabilities_json).unwrap_or_else(|_| crate::sources::capabilities(&ctx.source_kind))
}

fn connection(ctx: &ReplyContext) -> AppResult<(&str, &str)> {
    match (ctx.connection_id.as_deref(), ctx.provider_config_key.as_deref()) {
        (Some(c), Some(p)) => Ok((c, p)),
        _ => Err(AppError::Conflict("this email didn't come from a connected mailbox".into())),
    }
}

pub fn reply_subject(subject: &str) -> String {
    let s = subject.trim();
    if s.to_lowercase().starts_with("re:") { s.to_string() } else { format!("Re: {s}") }
}

/// RFC 5322 reply, base64url-encoded for Gmail's `raw` field.
pub fn build_raw(from: &str, ctx_to: &str, subject: &str, in_reply_to: Option<&str>, body: &str) -> AppResult<String> {
    let bad = |e: String| AppError::BadRequest(e);
    let from: Mailbox = from.parse().map_err(|_| bad("connected mailbox address is invalid".into()))?;
    let to: Mailbox = ctx_to.parse().map_err(|_| bad("the original sender's address is invalid".into()))?;
    let mut builder = Message::builder().from(from).to(to).subject(reply_subject(subject));
    if let Some(id) = in_reply_to {
        builder = builder.in_reply_to(id.to_string()).references(id.to_string());
    }
    let msg = builder
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_string())
        .map_err(|e| AppError::Internal(anyhow::anyhow!("building reply: {e}")))?;
    Ok(URL_SAFE_NO_PAD.encode(msg.formatted()))
}

async fn mailbox_address(state: &AppState, conn: &str, pck: &str) -> AppResult<String> {
    let profile = state.nango.gmail_call(Method::GET, conn, pck, "profile", None).await?;
    profile
        .get("emailAddress")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or(AppError::Unavailable)
}

async fn raw_for(state: &AppState, ctx: &ReplyContext, conn: &str, pck: &str) -> AppResult<Value> {
    if ctx.body_text.trim().is_empty() {
        return Err(AppError::BadRequest("the draft is empty".into()));
    }
    let from = mailbox_address(state, conn, pck).await?;
    let raw = build_raw(&from, &ctx.sender_address, &ctx.subject, ctx.rfc_message_id.as_deref(), &ctx.body_text)?;
    let mut message = json!({ "raw": raw });
    if let Some(t) = &ctx.provider_thread_id {
        message["threadId"] = json!(t);
    }
    Ok(message)
}

/// Saves (or updates) the reply in the mailbox's Drafts folder.
pub async fn save_to_provider(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let ctx = load(&state, &ws, &id).await?;
    if !policy::can_create_provider_draft(&capabilities(&ctx)) {
        return Err(AppError::Conflict("this mailbox can't hold drafts; copy the text instead".into()));
    }
    if ctx.status == "sent" {
        return Err(AppError::Conflict("this reply was already sent".into()));
    }
    let (conn, pck) = connection(&ctx)?;
    let message = raw_for(&state, &ctx, conn, pck).await?;
    let result = match &ctx.provider_draft_id {
        Some(existing) => {
            state.nango.gmail_call(Method::PUT, conn, pck, &format!("drafts/{existing}"), Some(&json!({ "message": message }))).await
        }
        None => state.nango.gmail_call(Method::POST, conn, pck, "drafts", Some(&json!({ "message": message }))).await,
    }?;
    let gmail_draft_id = result.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE drafts SET provider_draft_id = ?1, updated_at = ?2 WHERE id = ?3")
        .bind(&gmail_draft_id)
        .bind(now_ms())
        .bind(&ctx.draft_id)
        .execute(&mut *tx)
        .await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &ws,
            actor: Actor::User(&user.id),
            action: "draft.saved_to_mailbox",
            entity_type: "draft",
            entity_id: &ctx.draft_id,
            message_id: Some(&ctx.message_id),
            metadata: json!({ "provider_draft_id": gmail_draft_id }),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "status": "saved", "provider_draft_id": gmail_draft_id })))
}

#[derive(Deserialize)]
pub struct SendBody {
    /// The UI sends `true` only after the person confirms recipient + subject.
    confirm: bool,
}

/// Sends the reply from the connected mailbox, in the original thread.
pub async fn send(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
    Json(body): Json<SendBody>,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    if !body.confirm {
        return Err(AppError::BadRequest("sending needs explicit confirmation".into()));
    }
    let ctx = load(&state, &ws, &id).await?;
    if !policy::can_send(&capabilities(&ctx)) {
        return Err(AppError::Conflict("this source can't send; copy the draft into your mail client".into()));
    }
    // Claim the draft first so a double click can't send twice.
    let claimed = sqlx::query("UPDATE drafts SET status = 'sent', updated_at = ?1 WHERE id = ?2 AND workspace_id = ?3 AND status != 'sent'")
        .bind(now_ms())
        .bind(&ctx.draft_id)
        .bind(&ws)
        .execute(&state.db)
        .await?
        .rows_affected()
        == 1;
    if !claimed {
        return Err(AppError::Conflict("this reply was already sent".into()));
    }
    let (conn, pck) = connection(&ctx)?;
    let outcome = async {
        match &ctx.provider_draft_id {
            Some(gid) => {
                // Refresh the saved Gmail draft with the latest text, then send it.
                let message = raw_for(&state, &ctx, conn, pck).await?;
                state.nango.gmail_call(Method::PUT, conn, pck, &format!("drafts/{gid}"), Some(&json!({ "message": message }))).await?;
                Ok::<Value, AppError>(state.nango.gmail_call(Method::POST, conn, pck, "drafts/send", Some(&json!({ "id": gid }))).await?)
            }
            None => {
                let message = raw_for(&state, &ctx, conn, pck).await?;
                Ok(state.nango.gmail_call(Method::POST, conn, pck, "messages/send", Some(&message)).await?)
            }
        }
    }
    .await;
    let sent = match outcome {
        Ok(v) => v,
        Err(e) => {
            // Not sent: release the claim so the person can retry.
            sqlx::query("UPDATE drafts SET status = 'approved', updated_at = ?1 WHERE id = ?2")
                .bind(now_ms())
                .bind(&ctx.draft_id)
                .execute(&state.db)
                .await?;
            return Err(e);
        }
    };
    let gmail_message_id = sent.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE drafts SET provider_message_id = ?1 WHERE id = ?2").bind(&gmail_message_id).bind(&ctx.draft_id).execute(&mut *tx).await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &ws,
            actor: Actor::User(&user.id),
            action: "draft.sent",
            entity_type: "draft",
            entity_id: &ctx.draft_id,
            message_id: Some(&ctx.message_id),
            metadata: json!({ "to": "original_sender", "provider_message_id": gmail_message_id }),
        },
    )
    .await?;
    tx.commit().await?;
    if let Err(e) = crate::jobs::learning_jobs::enqueue_reply_learning(&state.db, &ctx.draft_id).await {
        tracing::warn!(error = %e, "could not schedule learning from the sent reply");
    }
    state.jobs_notify.notify_one();
    Ok(Json(json!({ "status": "sent", "provider_message_id": gmail_message_id })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_threaded_reply() {
        assert_eq!(reply_subject("Timesheet"), "Re: Timesheet");
        assert_eq!(reply_subject("RE: Timesheet"), "RE: Timesheet");
        let raw = build_raw("me@acme.com", "Priya <priya@acme.com>", "Timesheet", Some("<abc@mail>"), "Thanks!").unwrap();
        let text = String::from_utf8(URL_SAFE_NO_PAD.decode(raw).unwrap()).unwrap();
        assert!(text.contains("Subject: Re: Timesheet"));
        assert!(text.contains("In-Reply-To: <abc@mail>"));
        assert!(text.contains("References: <abc@mail>"));
        assert!(text.contains("To: Priya <priya@acme.com>"));
        assert!(text.ends_with("Thanks!") || text.contains("\r\n\r\nThanks!"));
    }
}
