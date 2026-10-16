//! Pipeline stages: fetch → normalize/persist → classify → workflow → draft.
//! Each stage writes its result and enqueues the next stage in one transaction whose
//! first statement is a write (so SQLite takes the write lock up front).

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{Job, kind};
use crate::{
    ai::ledger,
    ai::{
        self, AiError, ClassifyInput, ClassifyOutput, DraftInput, policy,
        understand::{UnderstandInput, Understanding},
    },
    platform::config::UnderstandMode,
    platform::audit::{self, Actor, Entry},
    platform::db::{new_id, now_ms},
    mail::ingest, learning::memory,
    platform::domain::{self, ClassificationSource, OTHER_LABEL, Priority, TAXONOMY_VERSION},
    understanding::labels,
    http::AppState,
    jobs, learning,
    mail::sources::resend::{FetchError, ReceivedEmail},
};

mod fetch;
mod classify;
mod persist;
mod workflow;
mod draft;
mod understanding;
mod retention;

use fetch::fetch_forwarded_email;
use classify::classify_email;
use persist::continue_pipeline;
pub use persist::persist_classification;
pub use persist::WriteMode;
use workflow::execute_workflow;
use draft::generate_draft;
pub use draft::create_draft_for_message;
pub use draft::create_agent_draft;
pub use draft::preview_draft;
pub use understanding::analyze_message;
use retention::backup_database;
use retention::purge_expired_events;
use persist::persist_classification_with;
use persist::enqueue_workflow;
use understanding::thread_digest;
use understanding::store_insight;

#[derive(Debug, thiserror::Error)]
pub enum JobError {
    #[error("{0}")]
    Retry(String),
    #[error("{0}")]
    Permanent(String),
}

impl JobError {
    pub fn is_permanent(&self) -> bool {
        matches!(self, JobError::Permanent(_))
    }
}

impl From<sqlx::Error> for JobError {
    fn from(e: sqlx::Error) -> Self {
        JobError::Retry(format!("database: {e}"))
    }
}

impl From<AiError> for JobError {
    fn from(e: AiError) -> Self {
        match e {
            AiError::Retryable(m) => JobError::Retry(m),
            AiError::Permanent(m) => JobError::Permanent(m),
        }
    }
}

/// AI stages run automatically only for new mail. For older mail they are skipped (no model
/// call, no tokens) and run when a person opens the email (`:open:` jobs) or asks (`:manual:`).
/// Plain labelling of newly imported mail still runs: it's one cheap call and fills the inbox.
async fn deferred_until_opened(state: &AppState, job: &Job, payload: &Value) -> Result<bool, JobError> {
    // Global inbound cap: automatic embedding waits; opening the email (`index:{id}:open`) re-runs it.
    let by_person = job.dedupe_key.contains(":open") || job.dedupe_key.contains(":manual:");
    if job.kind == kind::INDEX_MESSAGE
        && !by_person
        && !ledger::within_global_budget(&state.db, state.config.ai_global_daily_budget_usd).await
    {
        return Ok(true);
    }
    let deferrable = match job.kind.as_str() {
        kind::ANALYZE_MESSAGE | kind::LEARN_FROM_MESSAGE | kind::INDEX_MESSAGE | kind::RUN_AUTOMATIONS | kind::EXTRACT_RECORD
        | kind::EXECUTE_WORKFLOW => true,
        // Bulk re-labelling (after new labels) is deferred too; first-time labelling is not.
        kind::CLASSIFY_EMAIL => job.dedupe_key.contains(":recheck:") || job.dedupe_key.contains(":label:"),
        _ => false,
    };
    if !deferrable {
        return Ok(false);
    }
    let costs_tokens = matches!(
        job.kind.as_str(),
        kind::ANALYZE_MESSAGE | kind::LEARN_FROM_MESSAGE | kind::EXTRACT_RECORD | kind::RUN_AUTOMATIONS
    );
    ai_deferred(state, job, payload, costs_tokens).await
}

/// A person opened the email (`:open`) or asked for the work (`:manual`).
pub fn person_initiated(dedupe_key: &str) -> bool {
    [":open", ":manual"].iter().any(|m| dedupe_key.ends_with(m) || dedupe_key.contains(&format!("{m}:")))
}

/// True when AI work for this job's email should wait for a person: the email is older than
/// `AI_AUTO_HOURS`, or (for token-costing work) the workspace's daily budget is spent. Work a
/// person asked for is never deferred. Jobs that mix AI and non-AI work (attachments) call this
/// directly and skip only the AI part.
pub async fn ai_deferred(state: &AppState, job: &Job, payload: &Value, costs_tokens: bool) -> Result<bool, JobError> {
    if person_initiated(&job.dedupe_key) {
        return Ok(false);
    }
    if costs_tokens
        && let Some(ws) = &job.workspace_id
        && !ledger::within_budget(&state.db, state.config.ai_daily_budget_usd, ws).await
    {
        return Ok(true);
    }
    let Some(message_id) = payload.get("message_id").and_then(Value::as_str) else { return Ok(false) };
    let received: Option<i64> =
        sqlx::query_scalar("SELECT received_at FROM email_messages WHERE id = ?1").bind(message_id).fetch_optional(&state.db).await?;
    Ok(received.is_some_and(|at| at < now_ms() - state.config.ai_auto_hours * 3_600_000))
}

fn str_field(payload: &Value, key: &str) -> Result<String, JobError> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| JobError::Permanent(format!("payload missing {key}")))
}

pub async fn handle(state: &AppState, job: &Job) -> Result<(), JobError> {
    let payload = job.payload();
    if deferred_until_opened(state, job, &payload).await? {
        tracing::debug!(kind = %job.kind, key = %job.dedupe_key, "AI deferred until someone opens the email (old mail or daily budget reached)");
        return Ok(());
    }
    match job.kind.as_str() {
        kind::FETCH_FORWARDED_EMAIL => fetch_forwarded_email(state, &payload).await,
        kind::CLASSIFY_EMAIL => classify_email(state, job, &payload).await,
        kind::EXECUTE_WORKFLOW => execute_workflow(state, &payload).await,
        kind::GENERATE_DRAFT => generate_draft(state, job, &payload).await,
        kind::PURGE_EXPIRED_EVENTS => purge_expired_events(state).await,
        kind::BACKUP_DATABASE => backup_database(state).await,
        kind::SYNC_NANGO_SOURCE => crate::mail::sources::nango::sync_source(state, &payload).await,
        kind::LEARN_FROM_DRAFT | kind::LEARN_FROM_REPLY | kind::LEARN_FROM_SIGNAL => super::learning_jobs::learn_from_signal(state, &payload).await,
        kind::ANALYZE_MESSAGE => {
            let insight = analyze_message(state, &payload).await?;
            continue_pipeline(state, job, &payload).await?;
            crate::agents::triggers::after_analysis(state, job, &str_field(&payload, "message_id")?, &insight).await
        }
        kind::CLEAN_STORED_TEXT => super::message_jobs::clean_stored_text(state).await,
        kind::REFETCH_GMAIL_HTML => super::message_jobs::refetch_gmail_html(state, &payload).await,
        kind::EXTRACT_RECORD => super::matching_jobs::extract_record(state, &payload).await,
        kind::EXTRACT_ATTACHMENTS => super::attachment_jobs::extract_attachments(state, job, &payload).await,
        kind::MATCH_RECORD => super::matching_jobs::match_record(state, job, &payload).await,
        kind::RUN_AUTOMATIONS => super::automation_jobs::run_automations(state, &payload).await,
        kind::REMIND_NO_REPLY => super::automation_jobs::remind_no_reply(state, &payload).await,
        kind::REGISTER_NANGO_CONNECTION => crate::mail::sources::nango::register_from_webhook(state, &payload).await,
        kind::INDEX_MESSAGE | kind::INDEX_REPLY | kind::BACKFILL_KNOWLEDGE => super::knowledge_jobs::handle(state, job, &payload).await,
        kind::LEARN_FROM_MESSAGE => super::learning_jobs::learn_from_message(state, &payload).await,
        kind::EVALUATE_ALERTS => crate::alerts::evaluate_job(state, &payload).await,
        kind::SEND_MATCH_DIGEST => super::match_digest::digest_job(state, &payload).await,
        kind::AGENT_STEP => super::agent_jobs::agent_step(state, job, &payload).await,
        kind::AGENT_SCHEDULE_TICK => crate::agents::triggers::schedule_tick_job(state, job).await,
        kind::SEND_DRAFT => crate::replies::send_scheduled(state, &payload).await,
        kind::ENROL_CANDIDATE_WATCH => crate::hr::candidate_watch::enrol_job(state, &payload).await,
        kind::NOTIFY_CANDIDATE_WATCH => crate::hr::candidate_watch::notify_job(state, &payload).await,
        kind::SEND_OUTBOUND => crate::alerts::messaging::email::run_scheduled(state, &payload).await,
        kind::WATCH_REPLY => crate::alerts::messaging::watch::reply_job(state, &payload).await,
        kind::WATCH_NUDGE => crate::alerts::messaging::watch::nudge_job(state, &payload).await,
        kind::INAPP_DUE => crate::alerts::messaging::inapp::due_job(state, &payload).await,
        kind::PURGE_SOURCE => crate::mail::deletion::purge_source_job(state, &payload).await,
        kind::STAFFING_SCAN => crate::hr::staffing::scan_job(state, &payload).await,
        kind::RECORD_TIMESHEET => crate::hr::staffing::record_job(state, &payload).await,
        kind::PERSON_LINKED => crate::agents::plan::person_linked_job(state, &payload).await,
        kind::INFO_REQUEST_DUE => crate::hr::team::info_requests::due_job(state, &payload).await,
        kind::REPLY_SLA => crate::agents::supervisor::sla::stage_job(state, &payload).await,
        kind::REPLY_SLA_SWEEP => crate::agents::supervisor::sla::sweep_job(state, &payload).await,
        kind::REFRESH_IDENTITIES => crate::hr::team::identity::unresolved::refresh_job(state, &payload).await,
        kind::RUN_EVAL => super::eval_jobs::run_eval(state, &payload).await,
        kind::ROLLUP_AGENT_METRICS => super::eval_jobs::rollup_agent_metrics(state).await,
        kind::DESIGNER_DRY_RUN => super::designer_jobs::designer_dry_run(state, job, &payload).await,
        other => Err(JobError::Permanent(format!("unknown job kind {other}"))),
    }
}

/// Called once when a job is dead-lettered: surface the failure instead of pretending success.
pub async fn on_dead(state: &AppState, job: &Job, error: &str) {
    let payload = job.payload();
    let result: Result<(), JobError> = async {
        match job.kind.as_str() {
            kind::PURGE_SOURCE => {
                let source_id = str_field(&payload, "source_id")?;
                crate::mail::deletion::purge_failed(&state.db, &source_id, "The deletion stopped after repeated errors. Disconnect again to retry.").await?;
                Ok(())
            }
            kind::CLASSIFY_EMAIL => {
                // Route to `other` + needs_review so a human sees it.
                let message_id = str_field(&payload, "message_id")?;
                let fallback = ClassifyOutput {
                    category: OTHER_LABEL.to_string(),
                    priority: Priority::Normal,
                    requires_reply: true,
                    confidence: None,
                    source: ClassificationSource::Fallback,
                    model_id: Some("error-fallback".into()),
                    model_version: None,
                };
                persist_classification(state, &message_id, &fallback, Actor::System, None, WriteMode::OnlyIfAbsent)
                    .await?;
                set_message_error(state, &message_id, error).await
            }
            kind::GENERATE_DRAFT => {
                let message_id = str_field(&payload, "message_id")?;
                // Only a workflow draft failure fails the message; a manual "regenerate" does not.
                if let Some(run_id) = payload.get("workflow_run_id").and_then(Value::as_str) {
                    sqlx::query("UPDATE workflow_runs SET status = 'failed', error = ?1 WHERE id = ?2")
                        .bind(crate::mail::sources::normalize::truncate_chars(error, 500))
                        .bind(run_id)
                        .execute(&state.db)
                        .await?;
                    sqlx::query("UPDATE email_messages SET processing_status = 'failed', processing_error = ?1 WHERE id = ?2")
                        .bind(crate::mail::sources::normalize::truncate_chars(error, 500))
                        .bind(&message_id)
                        .execute(&state.db)
                        .await?;
                }
                Ok(())
            }
            kind::EXECUTE_WORKFLOW => {
                let message_id = str_field(&payload, "message_id")?;
                set_message_error(state, &message_id, error).await
            }
            kind::SEND_DRAFT => {
                crate::replies::give_up_scheduled(state, &payload, error).await?;
                Ok(())
            }
            kind::SEND_OUTBOUND => crate::alerts::messaging::email::give_up_scheduled(state, &payload, error).await,
            kind::FETCH_FORWARDED_EMAIL => {
                if let Some(event_id) = payload.get("event_id").and_then(Value::as_str) {
                    sqlx::query("UPDATE inbound_events SET status = 'rejected' WHERE id = ?1")
                        .bind(event_id)
                        .execute(&state.db)
                        .await?;
                }
                Ok(())
            }
            kind::AGENT_STEP => super::agent_jobs::on_dead(state, job, &payload, error).await,
            // The pipeline must not stall behind a failed reading: the next stages run and surface the failure themselves.
            kind::ANALYZE_MESSAGE => continue_pipeline(state, job, &payload).await,
            kind::RUN_EVAL => super::eval_jobs::on_dead(state, &payload, error).await,
            kind::DESIGNER_DRY_RUN => super::designer_jobs::on_dead(state, job, &payload).await,
            _ => Ok(()),
        }
    }
    .await;
    if let Err(e) = result {
        tracing::error!(job_id = %job.id, error = %e, "dead-letter handler failed");
    }
}

async fn set_message_error(state: &AppState, message_id: &str, error: &str) -> Result<(), JobError> {
    sqlx::query("UPDATE email_messages SET processing_error = ?1 WHERE id = ?2")
        .bind(crate::mail::sources::normalize::truncate_chars(error, 500))
        .bind(message_id)
        .execute(&state.db)
        .await?;
    Ok(())
}

#[derive(sqlx::FromRow)]
struct MessageForAi {
    workspace_id: String,
    subject: String,
    sender_address: String,
    body_text: String,
}

async fn load_message(state: &AppState, message_id: &str) -> Result<MessageForAi, JobError> {
    sqlx::query_as("SELECT workspace_id, subject, sender_address, body_text FROM email_messages WHERE id = ?1")
        .bind(message_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| JobError::Permanent("message not found".into()))
}

/// A v2 reading stored together with the classification it produced.
pub struct StoredUnderstanding {
    understanding: Understanding,
    model_id: String,
    attachments_hash: Option<String>,
}

