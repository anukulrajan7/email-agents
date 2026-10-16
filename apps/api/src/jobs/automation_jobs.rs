//! Runs automations after a message is classified and understood, and the delayed
//! "no reply yet" reminder. Every (automation, message) pair runs at most once.

use serde::Serialize;
use serde_json::{Value, json};

use super::{
    DEFAULT_MAX_ATTEMPTS, enqueue_at,
    handlers::{self, JobError},
    kind,
};
use crate::{
    platform::audit::{self, Actor, Entry},
    automations::{self, Action, Automation, CONTEXT_SELECT, MessageContext},
    platform::db::{new_id, now_ms},
    platform::error::AppError,
    http::AppState,
    replies,
};

const DAY_MS: i64 = 86_400_000;
/// Auto-send needs a classification the model is sure about.
const AUTO_SEND_MIN_CONFIDENCE: f64 = 0.85;

#[derive(Debug, Clone, Serialize)]
pub struct ActionResult {
    #[serde(rename = "type")]
    pub kind: String,
    pub status: &'static str,
    pub detail: Option<String>,
}

fn done(kind: &str, detail: impl Into<String>) -> ActionResult {
    ActionResult { kind: kind.into(), status: "done", detail: Some(detail.into()) }
}

fn skipped(kind: &str, detail: impl Into<String>) -> ActionResult {
    ActionResult { kind: kind.into(), status: "skipped", detail: Some(detail.into()) }
}

fn failed(kind: &str, detail: impl Into<String>) -> ActionResult {
    ActionResult { kind: kind.into(), status: "failed", detail: Some(detail.into()) }
}

fn app_error_detail(e: &AppError) -> String {
    match e {
        AppError::Internal(_) => "unexpected error".to_string(),
        other => other.to_string(),
    }
}

fn message_id_of(payload: &Value) -> Result<String, JobError> {
    payload
        .get("message_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| JobError::Permanent("payload missing message_id".into()))
}

// ---------------------------------------------------------------- run

pub async fn run_automations(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let message_id = message_id_of(payload)?;
    let info: Option<(String, String)> = sqlx::query_as(
        "SELECT m.workspace_id, s.status FROM email_messages m JOIN email_sources s ON s.id = m.source_id WHERE m.id = ?1",
    )
    .bind(&message_id)
    .fetch_optional(&state.db)
    .await?;
    let Some((workspace_id, source_status)) = info else {
        return Ok(());
    };
    if source_status == "disconnected" || crate::mail::system_mail::is_own_message(&state.db, &workspace_id, &message_id).await? {
        return Ok(());
    }
    // Conditions read needs_reply from the insight, so make sure it exists first.
    handlers::analyze_message(state, payload).await?;

    let candidates: Vec<Automation> = automations::list(&state.db, &workspace_id)
        .await?
        .into_iter()
        .filter(|a| a.enabled && a.trigger == automations::TRIGGER_EMAIL_RECEIVED)
        .collect();
    if candidates.is_empty() {
        return Ok(());
    }
    let ctx: Option<MessageContext> = sqlx::query_as(&format!("{CONTEXT_SELECT} WHERE m.id = ?1"))
        .bind(&message_id)
        .fetch_optional(&state.db)
        .await?;
    let Some(ctx) = ctx else {
        return Ok(());
    };

    for automation in candidates {
        if !automations::matches_all(&automation.conditions, &ctx) {
            continue;
        }
        let already: Option<i64> =
            sqlx::query_scalar("SELECT 1 FROM automation_runs WHERE automation_id = ?1 AND message_id = ?2")
                .bind(&automation.id)
                .bind(&message_id)
                .fetch_optional(&state.db)
                .await?;
        if already.is_some() {
            continue;
        }
        let results = execute(state, &workspace_id, &automation, &ctx).await?;
        record_run(state, &workspace_id, &automation, &message_id, &results).await?;
    }
    Ok(())
}

async fn record_run(
    state: &AppState,
    workspace_id: &str,
    automation: &Automation,
    message_id: &str,
    results: &[ActionResult],
) -> Result<(), JobError> {
    let status = if results.iter().any(|r| r.status == "failed") {
        "failed"
    } else if results.iter().all(|r| r.status == "skipped") {
        "skipped"
    } else {
        "completed"
    };
    let now = now_ms();
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let created = sqlx::query(
        "INSERT OR IGNORE INTO automation_runs (id, workspace_id, automation_id, message_id, status, actions_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(new_id("arun"))
    .bind(workspace_id)
    .bind(&automation.id)
    .bind(message_id)
    .bind(status)
    .bind(serde_json::to_string(results).unwrap_or_else(|_| "[]".into()))
    .bind(now)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if created {
        sqlx::query("UPDATE automations SET run_count = run_count + 1, last_run_at = ?1 WHERE id = ?2")
            .bind(now)
            .bind(&automation.id)
            .execute(&mut *tx)
            .await?;
        audit::record(
            &mut tx,
            Entry {
                workspace_id,
                actor: Actor::System,
                action: "automation.ran",
                entity_type: "automation",
                entity_id: &automation.id,
                message_id: Some(message_id),
                metadata: json!({ "status": status }),
            },
        )
        .await?;
    }
    tx.commit().await?;
    if created && status == "failed" {
        // A failed automation must reach a person, not just the run log.
        crate::alerts::raise(
            &state.db,
            crate::alerts::NewAlert {
                workspace_id,
                kind: "automation",
                severity: "normal",
                title: format!("Automation failed: {}", automation.name),
                body: "One or more steps failed. Open the automation's runs to see which step and why.".into(),
                message_id: Some(message_id),
                level: 1,
                dedupe_key: Some(format!("automation_failed:{}:{message_id}", automation.id)),
            },
        )
        .await?;
    }
    Ok(())
}

/// Runs the actions in order. Provider failures are recorded per action; only transient
/// AI errors bubble up so the whole job retries (drafts are reused, sends are claim-guarded).
async fn execute(
    state: &AppState,
    workspace_id: &str,
    automation: &Automation,
    ctx: &MessageContext,
) -> Result<Vec<ActionResult>, JobError> {
    let mut results = Vec::with_capacity(automation.actions.len());
    let mut draft_id: Option<String> = None;
    let auto_reply = crate::replies::reply_settings::load(&state.db, &state.config, workspace_id).await?.auto_reply;
    for action in &automation.actions {
        let kind_name = action.type_name();
        let result = match action {
            Action::DraftReply { .. } if !auto_reply => {
                skipped(kind_name, "automatic drafts are off: open the email and generate a draft")
            }
            Action::DraftReply { instructions } => {
                match handlers::create_draft_for_message(state, &ctx.id, instructions.as_deref()).await {
                    Ok(Some(id)) => {
                        draft_id = Some(id);
                        done(kind_name, "draft ready")
                    }
                    Ok(None) => skipped(kind_name, "no reply needed"),
                    Err(JobError::Retry(e)) => return Err(JobError::Retry(e)),
                    Err(JobError::Permanent(e)) => failed(kind_name, e),
                }
            }
            Action::SaveToGmailDrafts => save_to_gmail_drafts(state, workspace_id, draft_id.as_deref()).await?,
            Action::SendReply { daily_cap, allowed_domains } => {
                send_reply(state, workspace_id, automation, ctx, draft_id.as_deref(), *daily_cap, allowed_domains).await?
            }
            Action::CreateReview { title } => create_review(state, workspace_id, automation, ctx, title.as_deref()).await?,
            Action::RemindIfNoReply { hours } => schedule_reminder(state, workspace_id, automation, ctx, *hours).await?,
        };
        results.push(result);
    }
    Ok(results)
}

// ---------------------------------------------------------------- actions

async fn save_to_gmail_drafts(state: &AppState, workspace_id: &str, draft_id: Option<&str>) -> Result<ActionResult, JobError> {
    const K: &str = "save_to_gmail_drafts";
    let Some(draft_id) = draft_id else {
        return Ok(skipped(K, "no draft to save"));
    };
    let rc = match replies::load(state, workspace_id, draft_id).await {
        Ok(rc) => rc,
        Err(e) => return Ok(failed(K, app_error_detail(&e))),
    };
    if !crate::ai::policy::can_create_provider_draft(&replies::capabilities(&rc)) {
        return Ok(skipped(K, "this mailbox can't hold drafts"));
    }
    if rc.status == "sent" {
        return Ok(skipped(K, "reply was already sent"));
    }
    Ok(match replies::save_draft(state, workspace_id, draft_id, Actor::System).await {
        Ok(_) => done(K, "saved to Gmail Drafts"),
        Err(e) => failed(K, app_error_detail(&e)),
    })
}

/// `Err(reason)` when a classification is not trustworthy enough to send on its own.
pub fn auto_send_trust(confidence: Option<f64>, source: &str, model_id: Option<&str>) -> Result<(), &'static str> {
    if source == "fallback" && model_id == Some("error-fallback") {
        return Err("the label came from the error fallback");
    }
    if source == "manual" {
        return Ok(());
    }
    match confidence {
        Some(c) if c >= AUTO_SEND_MIN_CONFIDENCE => Ok(()),
        _ => Err("classification confidence is below 85%"),
    }
}

fn sender_domain(address: &str) -> String {
    address.rsplit('@').next().unwrap_or_default().trim_matches(|c: char| c == '>' || c.is_whitespace()).to_lowercase()
}

fn is_automated_sender(address: &str) -> bool {
    let local = address.split('@').next().unwrap_or_default().to_lowercase();
    ["noreply", "no-reply", "donotreply", "do-not-reply", "mailer-daemon", "postmaster"].iter().any(|p| local.contains(p))
}

async fn send_reply(
    state: &AppState,
    workspace_id: &str,
    automation: &Automation,
    ctx: &MessageContext,
    draft_id: Option<&str>,
    daily_cap: u32,
    allowed_domains: &[String],
) -> Result<ActionResult, JobError> {
    const K: &str = "send_reply";
    let Some(draft_id) = draft_id else {
        return Ok(skipped(K, "no draft to send"));
    };
    let draft: Option<(String, String, String, bool)> =
        sqlx::query_as(
            "SELECT body_text, unanswered_json, status,
                    (status = 'suggested' AND send_at IS NULL AND COALESCE(origin_key, '') LIKE 'automation-draft:%') AS untouched
             FROM drafts WHERE id = ?1 AND workspace_id = ?2",
        )
            .bind(draft_id)
            .bind(workspace_id)
            .fetch_optional(&state.db)
            .await?;
    let Some((body, unanswered, status, untouched)) = draft else {
        return Ok(skipped(K, "draft not found"));
    };
    if status == "sent" {
        return Ok(skipped(K, "reply was already sent"));
    }
    if !untouched {
        return Ok(skipped(K, "a person has edited or scheduled this draft; it is not sent automatically"));
    }
    if replies::has_open_gaps(&body) || !matches!(unanswered.trim(), "" | "[]") {
        return Ok(skipped(K, "draft has unanswered questions ([NEEDS: ...])"));
    }
    let thread_id: Option<String> = sqlx::query_scalar("SELECT thread_id FROM email_messages WHERE id = ?1 AND workspace_id = ?2")
        .bind(&ctx.id)
        .bind(workspace_id)
        .fetch_optional(&state.db)
        .await?;
    if let Some(why) = crate::agents::injection::scan_conversation(&state.db, workspace_id, thread_id.as_deref(), Some(&ctx.id)).await? {
        crate::ai::incidents::record(
            &state.db,
            crate::ai::incidents::NewIncident::new(workspace_id, crate::ai::incidents::Kind::InjectionDetected, json!({"gate": "auto_reply", "why": why}))
                .message(Some(&ctx.id))
                .dedupe(format!("injection:auto:{}", ctx.id)),
        )
        .await;
        return Ok(skipped(K, format!("not sent on its own: {why}")));
    }
    let domain = sender_domain(&ctx.sender_address);
    if !allowed_domains.iter().any(|d| d.trim().to_lowercase() == domain) {
        return Ok(skipped(K, "sender's domain is not in allowed_domains"));
    }
    if crate::automations::is_free_mail_domain(&domain) {
        return Ok(skipped(K, "the sender uses a free email provider"));
    }
    if !crate::automations::has_human_reply(&state.db, workspace_id, &ctx.sender_address).await? {
        return Ok(skipped(K, "nobody on the team has replied to this sender yet; the first reply is sent by a person"));
    }
    let classification: Option<(Option<f64>, String, Option<String>)> =
        sqlx::query_as("SELECT confidence, source, model_id FROM classifications WHERE message_id = ?1")
            .bind(&ctx.id)
            .fetch_optional(&state.db)
            .await?;
    let Some((confidence, source, model_id)) = classification else {
        return Ok(skipped(K, "the email has no classification"));
    };
    if let Err(reason) = auto_send_trust(confidence, &source, model_id.as_deref()) {
        return Ok(skipped(K, reason));
    }
    let day_start = now_ms() - now_ms().rem_euclid(DAY_MS);
    let sent_today: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events
         WHERE workspace_id = ?1 AND action = 'automation.sent_reply' AND entity_id = ?2 AND created_at >= ?3",
    )
    .bind(workspace_id)
    .bind(&automation.id)
    .bind(day_start)
    .fetch_one(&state.db)
    .await?;
    if sent_today >= i64::from(daily_cap) {
        return Ok(skipped(K, format!("daily cap of {daily_cap} reached")));
    }
    let rc = match replies::load(state, workspace_id, draft_id).await {
        Ok(rc) => rc,
        Err(e) => return Ok(failed(K, app_error_detail(&e))),
    };
    if !crate::ai::policy::can_send(&replies::capabilities(&rc)) {
        return Ok(skipped(K, "this source can't send"));
    }
    if is_automated_sender(&ctx.sender_address) {
        return Ok(skipped(K, "the sender is an automated address"));
    }
    let Ok((conn, pck)) = replies::connection(&rc) else {
        return Ok(skipped(K, "this email didn't come from a connected mailbox"));
    };
    match replies::mailbox_address(state, conn, pck).await {
        Ok(mailbox) if mailbox.trim().eq_ignore_ascii_case(ctx.sender_address.trim()) => {
            return Ok(skipped(K, "the sender is this mailbox itself"));
        }
        Ok(_) => {}
        Err(e) => return Ok(failed(K, format!("couldn't verify the mailbox address: {}", app_error_detail(&e)))),
    }
    match replies::send_draft(state, workspace_id, draft_id, Actor::System).await {
        Ok(_) => {
            let mut conn = state.db.acquire().await?;
            audit::record(
                &mut conn,
                Entry {
                    workspace_id,
                    actor: Actor::System,
                    action: "automation.sent_reply",
                    entity_type: "automation",
                    entity_id: &automation.id,
                    message_id: Some(&ctx.id),
                    metadata: json!({ "draft_id": draft_id }),
                },
            )
            .await?;
            Ok(done(K, "reply checked and scheduled to send"))
        }
        Err(AppError::DraftIncomplete(m)) | Err(AppError::SendBlocked(m, _)) => Ok(skipped(K, m)),
        Err(e) => Ok(failed(K, app_error_detail(&e))),
    }
}

async fn create_review(
    state: &AppState,
    workspace_id: &str,
    automation: &Automation,
    ctx: &MessageContext,
    title: Option<&str>,
) -> Result<ActionResult, JobError> {
    let title = title.map(str::trim).filter(|t| !t.is_empty()).unwrap_or(&automation.name);
    let created = insert_review(state, workspace_id, &automation.id, &ctx.id, "automation", title).await?;
    Ok(if created { done("create_review", "added to Reviews") } else { skipped("create_review", "already in Reviews") })
}

async fn insert_review(
    state: &AppState,
    workspace_id: &str,
    automation_id: &str,
    message_id: &str,
    review_kind: &str,
    title: &str,
) -> Result<bool, JobError> {
    let review_id = new_id("rev");
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let created = sqlx::query(
        "INSERT OR IGNORE INTO review_items (id, workspace_id, message_id, workflow_run_id, kind, title, status, created_at, automation_id)
         VALUES (?1, ?2, ?3, NULL, ?4, ?5, 'open', ?6, ?7)",
    )
    .bind(&review_id)
    .bind(workspace_id)
    .bind(message_id)
    .bind(review_kind)
    .bind(crate::mail::sources::normalize::truncate_chars(title, 200))
    .bind(now_ms())
    .bind(automation_id)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if created {
        audit::record(
            &mut tx,
            Entry {
                workspace_id,
                actor: Actor::System,
                action: "review.created",
                entity_type: "review_item",
                entity_id: &review_id,
                message_id: Some(message_id),
                metadata: json!({ "kind": review_kind, "automation_id": automation_id }),
            },
        )
        .await?;
    }
    tx.commit().await?;
    Ok(created)
}

async fn schedule_reminder(
    state: &AppState,
    workspace_id: &str,
    automation: &Automation,
    ctx: &MessageContext,
    hours: u32,
) -> Result<ActionResult, JobError> {
    let run_at = now_ms() + i64::from(hours) * 3_600_000;
    let mut conn = state.db.acquire().await?;
    enqueue_at(
        &mut conn,
        kind::REMIND_NO_REPLY,
        &format!("remind:{}:{}", automation.id, ctx.id),
        Some(workspace_id),
        &json!({ "automation_id": automation.id, "message_id": ctx.id }),
        DEFAULT_MAX_ATTEMPTS,
        run_at,
    )
    .await?;
    Ok(done("remind_if_no_reply", format!("will check for a reply in {hours}h")))
}

// ---------------------------------------------------------------- reminder

/// Fires when `remind_if_no_reply` is due: if nobody answered the thread, open a review item.
pub async fn remind_no_reply(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let message_id = message_id_of(payload)?;
    let automation_id = payload
        .get("automation_id")
        .and_then(Value::as_str)
        .ok_or_else(|| JobError::Permanent("payload missing automation_id".into()))?;
    let automation: Option<String> = sqlx::query_scalar("SELECT workspace_id FROM automations WHERE id = ?1")
        .bind(automation_id)
        .fetch_optional(&state.db)
        .await?;
    let message: Option<(String, String, String, String, i64)> = sqlx::query_as(
        "SELECT workspace_id, subject, sender_address, thread_id, received_at FROM email_messages WHERE id = ?1",
    )
    .bind(&message_id)
    .fetch_optional(&state.db)
    .await?;
    let (Some(workspace_id), Some((message_ws, subject, sender, thread_id, received_at))) = (automation, message) else {
        return Ok(());
    };
    if workspace_id != message_ws {
        return Ok(());
    }
    // Answered = our side replied in the thread (outbound mail or a sent draft), not just anyone writing again.
    let answered = crate::mail::threads::thread_has_reply_after(&state.db, &workspace_id, &thread_id, received_at).await?;
    if answered {
        return Ok(());
    }
    let title = format!("No reply yet: {subject}");
    if insert_review(state, &workspace_id, automation_id, &message_id, "reminder", &title).await? {
        crate::alerts::raise(
            &state.db,
            crate::alerts::NewAlert {
                workspace_id: &workspace_id,
                kind: "automation",
                severity: "high",
                body: format!("Nobody has replied to {sender} yet."),
                title,
                message_id: Some(&message_id),
                level: 1,
                dedupe_key: Some(format!("reminder:{automation_id}:{message_id}")),
            },
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_send_needs_a_trusted_label() {
        assert!(auto_send_trust(Some(0.9), "llm", Some("m")).is_ok());
        assert!(auto_send_trust(Some(0.85), "jev", None).is_ok());
        assert!(auto_send_trust(Some(0.84), "llm", None).is_err());
        assert!(auto_send_trust(None, "fallback", Some("rules-v1")).is_err());
        assert!(auto_send_trust(Some(0.99), "fallback", Some("error-fallback")).is_err());
        assert!(auto_send_trust(None, "manual", None).is_ok());
    }

    #[test]
    fn sender_helpers() {
        assert_eq!(sender_domain("Priya@Acme.com"), "acme.com");
        assert!(is_automated_sender("no-reply@acme.com"));
        assert!(!is_automated_sender("priya@acme.com"));
    }
}
