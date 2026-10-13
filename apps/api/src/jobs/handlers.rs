//! Pipeline stages: fetch → normalize/persist → classify → workflow → draft.
//! Each stage writes its result and enqueues the next stage in one transaction whose
//! first statement is a write (so SQLite takes the write lock up front).

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{Job, kind};
use crate::{
    ai_ledger,
    ai::{self, AiError, ClassifyInput, ClassifyOutput, DraftInput, policy},
    audit::{self, Actor, Entry},
    db::{new_id, now_ms},
    ingest, memory,
    domain::{self, ClassificationSource, OTHER_LABEL, Priority, TAXONOMY_VERSION},
    labels,
    http::AppState,
    jobs,
    sources::resend::{FetchError, ReceivedEmail},
};

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
        && !ai_ledger::within_global_budget(&state.db, state.config.ai_global_daily_budget_usd).await
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
        && !ai_ledger::within_budget(&state.db, state.config.ai_daily_budget_usd, ws).await
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
        kind::SYNC_NANGO_SOURCE => crate::sources::nango::sync_source(state, &payload).await,
        kind::LEARN_FROM_DRAFT => learn_from_draft(state, &payload).await,
        kind::ANALYZE_MESSAGE => {
            let insight = analyze_message(state, &payload).await?;
            crate::agents::triggers::after_analysis(state, job, &str_field(&payload, "message_id")?, &insight).await
        }
        kind::CLEAN_STORED_TEXT => super::message_jobs::clean_stored_text(state).await,
        kind::REFETCH_GMAIL_HTML => super::message_jobs::refetch_gmail_html(state, &payload).await,
        kind::EXTRACT_RECORD => super::matching_jobs::extract_record(state, &payload).await,
        kind::EXTRACT_ATTACHMENTS => super::attachment_jobs::extract_attachments(state, job, &payload).await,
        kind::MATCH_RECORD => super::matching_jobs::match_record(state, job, &payload).await,
        kind::RUN_AUTOMATIONS => super::automation_jobs::run_automations(state, &payload).await,
        kind::REMIND_NO_REPLY => super::automation_jobs::remind_no_reply(state, &payload).await,
        kind::REGISTER_NANGO_CONNECTION => crate::sources::nango::register_from_webhook(state, &payload).await,
        kind::INDEX_MESSAGE | kind::INDEX_REPLY | kind::BACKFILL_KNOWLEDGE => super::knowledge_jobs::handle(state, job, &payload).await,
        kind::LEARN_FROM_MESSAGE => super::learning_jobs::learn_from_message(state, &payload).await,
        kind::LEARN_FROM_REPLY => super::learning_jobs::learn_from_reply(state, &payload).await,
        kind::EVALUATE_ALERTS => crate::alerts::evaluate_job(state, &payload).await,
        kind::SEND_MATCH_DIGEST => super::match_digest::digest_job(state, &payload).await,
        kind::AGENT_STEP => super::agent_jobs::agent_step(state, &payload).await,
        kind::SEND_DRAFT => crate::replies::send_scheduled(state, &payload).await,
        other => Err(JobError::Permanent(format!("unknown job kind {other}"))),
    }
}

/// Called once when a job is dead-lettered: surface the failure instead of pretending success.
pub async fn on_dead(state: &AppState, job: &Job, error: &str) {
    let payload = job.payload();
    let result: Result<(), JobError> = async {
        match job.kind.as_str() {
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
                        .bind(crate::sources::normalize::truncate_chars(error, 500))
                        .bind(run_id)
                        .execute(&state.db)
                        .await?;
                    sqlx::query("UPDATE email_messages SET processing_status = 'failed', processing_error = ?1 WHERE id = ?2")
                        .bind(crate::sources::normalize::truncate_chars(error, 500))
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
            kind::FETCH_FORWARDED_EMAIL => {
                if let Some(event_id) = payload.get("event_id").and_then(Value::as_str) {
                    sqlx::query("UPDATE inbound_events SET status = 'rejected' WHERE id = ?1")
                        .bind(event_id)
                        .execute(&state.db)
                        .await?;
                }
                Ok(())
            }
            kind::AGENT_STEP => super::agent_jobs::on_dead(state, &payload, error).await,
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
        .bind(crate::sources::normalize::truncate_chars(error, 500))
        .bind(message_id)
        .execute(&state.db)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------- fetch + persist

async fn fetch_forwarded_email(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let event_id = str_field(payload, "event_id")?;
    let provider = str_field(payload, "provider")?;

    let email: ReceivedEmail = if provider == "test" {
        let raw: String = sqlx::query_scalar("SELECT payload_json FROM inbound_events WHERE id = ?1")
            .bind(&event_id)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| JobError::Permanent("inbound event missing".into()))?;
        serde_json::from_str(&raw).map_err(|e| JobError::Permanent(format!("bad test payload: {e}")))?
    } else {
        // Network call happens outside any transaction.
        let email_id = str_field(payload, "email_id")?;
        state.resend.get_received_email(&email_id).await.map_err(|e| match e {
            FetchError::Retryable(m) => JobError::Retry(m),
            FetchError::Permanent(m) => JobError::Permanent(m),
        })?
    };

    // Resolve the workspace only from a validated alias we issued; never from headers.
    let mut source: Option<(String, String)> = None;
    for addr in email.delivery_addresses() {
        source = sqlx::query_as(
            "SELECT id, workspace_id FROM email_sources
             WHERE kind = 'forwarding' AND inbound_alias = ?1 AND status != 'disconnected'",
        )
        .bind(&addr)
        .fetch_optional(&state.db)
        .await?;
        if source.is_some() {
            break;
        }
    }
    let Some((source_id, workspace_id)) = source else {
        tracing::warn!(%event_id, "inbound email for unknown alias rejected");
        sqlx::query("UPDATE inbound_events SET status = 'rejected' WHERE id = ?1")
            .bind(&event_id)
            .execute(&state.db)
            .await?;
        return Ok(());
    };

    let mut email_norm = email.normalize(state.config.max_body_chars);
    if email_norm.source_message_id.is_empty() {
        email_norm.source_message_id = payload.get("email_id").and_then(Value::as_str).unwrap_or(&event_id).to_string();
    }

    let mut tx = state.db.begin().await?;
    // First statement is a write, so the dedupe check in persist_message runs under the write lock.
    sqlx::query("UPDATE inbound_events SET status = 'processed' WHERE id = ?1").bind(&event_id).execute(&mut *tx).await?;
    let created = ingest::persist_message(&mut tx, &source_id, &workspace_id, &email_norm, None, json!({ "event_id": event_id }))
        .await?;
    if created.is_some() {
        sqlx::query(
            "UPDATE email_sources SET status = 'connected', last_synced_at = ?1 WHERE id = ?2 AND status != 'disconnected'",
        )
        .bind(now_ms())
        .bind(&source_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    state.jobs_notify.notify_one();
    Ok(())
}

// ---------------------------------------------------------------- classify

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

async fn classify_email(state: &AppState, job: &Job, payload: &Value) -> Result<(), JobError> {
    let message_id = str_field(payload, "message_id")?;
    if super::message_jobs::is_outbound(state, &message_id).await? {
        return Ok(());
    }
    let msg = load_message(state, &message_id).await?;

    let existing: Option<String> = sqlx::query_scalar("SELECT source FROM classifications WHERE message_id = ?1")
        .bind(&message_id)
        .fetch_optional(&state.db)
        .await?;
    if existing.as_deref() == Some("manual") {
        // A human decision is never overwritten by a model; still re-run the workflow.
        let mut conn = state.db.acquire().await?;
        enqueue_workflow(&mut conn, &msg.workspace_id, &message_id, &job.id).await?;
        return Ok(());
    }

    let attachment_names: Vec<String> = sqlx::query_scalar("SELECT filename FROM attachments WHERE message_id = ?1")
        .bind(&message_id)
        .fetch_all(&state.db)
        .await?;
    let sender_history = memory::context_for(&state.db, &msg.workspace_id, &msg.sender_address, "").await?.sender_history;
    // Over the global inbound cap, automatic labelling uses the free rules (no model, no embedding).
    let over_global_cap = !job.dedupe_key.contains(":reprocess:")
        && !ai_ledger::within_global_budget(&state.db, state.config.ai_global_daily_budget_usd).await;
    let examples = if over_global_cap {
        vec![]
    } else {
        crate::knowledge::label_examples(&state.db, state.embedder.as_ref(), &msg.workspace_id, &message_id, &msg.subject, &msg.body_text).await
    };
    let input = ClassifyInput {
        examples,
        subject: msg.subject,
        sender: msg.sender_address,
        body_text: msg.body_text,
        attachment_names,
        sender_history,
        labels: labels::active(&state.db, &msg.workspace_id).await?,
    };
    let out = if over_global_cap {
        tracing::warn!("global daily AI budget reached: labelling with rules");
        ai::Classifier::classify(&ai::fallback::RulesClassifier, &input).await?
    } else {
        state.classifier.classify(&input).await?
    };
    persist_classification(state, &message_id, &out, Actor::System, Some(&job.id), WriteMode::Upsert).await?;
    state.jobs_notify.notify_one();
    Ok(())
}

/// Stores a classification (model, fallback or manual) and enqueues the workflow stage.
/// `trigger` makes the workflow job key unique per classification event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    /// Replace an existing classification, except that only a human can replace a human.
    Upsert,
    /// Write only if the message has no classification yet (dead-letter fallback).
    OnlyIfAbsent,
}

/// Returns `Some(needs_review)` if written, `None` if an existing (e.g. manual) decision was kept.
pub async fn persist_classification(
    state: &AppState,
    message_id: &str,
    out: &ClassifyOutput,
    actor: Actor<'_>,
    trigger: Option<&str>,
    mode: WriteMode,
) -> Result<Option<bool>, JobError> {
    let needs_review = policy::needs_review(out);
    let now = now_ms();
    let conflict = match mode {
        WriteMode::OnlyIfAbsent => "ON CONFLICT(message_id) DO NOTHING",
        // The WHERE makes "manual wins" atomic, closing the race with a concurrent model run.
        WriteMode::Upsert => {
            "ON CONFLICT(message_id) DO UPDATE SET
           category = excluded.category, priority = excluded.priority, requires_reply = excluded.requires_reply,
           confidence = excluded.confidence, needs_review = excluded.needs_review, source = excluded.source,
           model_id = excluded.model_id, model_version = excluded.model_version,
           taxonomy_version = excluded.taxonomy_version, prompt_version = excluded.prompt_version,
           updated_at = excluded.updated_at,
           ai_category = CASE WHEN excluded.source = 'manual'
             THEN COALESCE(classifications.ai_category, CASE WHEN classifications.source != 'manual' THEN classifications.category END)
             ELSE classifications.ai_category END,
           confirmed = CASE WHEN excluded.source = 'manual' THEN 1 ELSE classifications.confirmed END
         WHERE classifications.source != 'manual' OR excluded.source = 'manual'"
        }
    };
    let mut tx = state.db.begin().await?;
    let written = sqlx::query(&format!(
        "INSERT INTO classifications
           (message_id, category, priority, requires_reply, confidence, needs_review, source, model_id, model_version,
            taxonomy_version, prompt_version, updated_at, confirmed)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         {conflict}"
    ))
    .bind(message_id)
    .bind(&out.category)
    .bind(out.priority.as_str())
    .bind(out.requires_reply)
    .bind(out.confidence)
    .bind(needs_review)
    .bind(out.source.as_str())
    .bind(&out.model_id)
    .bind(&out.model_version)
    .bind(TAXONOMY_VERSION)
    .bind(matches!(out.source, ClassificationSource::Jev | ClassificationSource::Llm).then_some(ai::PROMPT_VERSION))
    .bind(now)
    .bind(matches!(out.source, ClassificationSource::Manual))
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if !written {
        tx.rollback().await?;
        return Ok(None);
    }

    let workspace_id: String = sqlx::query_scalar(
        "UPDATE email_messages SET processing_status = CASE
           WHEN processing_status IN ('pending', 'normalized', 'failed') THEN 'classified' ELSE processing_status END
         WHERE id = ?1 RETURNING workspace_id",
    )
    .bind(message_id)
    .fetch_one(&mut *tx)
    .await?;

    let (action, actor_meta) = match actor {
        Actor::User(_) => ("classification.overridden", "user"),
        Actor::System => ("message.classified", "system"),
    };
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &workspace_id,
            actor,
            action,
            entity_type: "classification",
            entity_id: message_id,
            message_id: Some(message_id),
            metadata: json!({
                "category": out.category, "priority": out.priority.as_str(),
                "source": out.source.as_str(), "needs_review": needs_review, "by": actor_meta,
                "model_id": out.model_id, "taxonomy_version": TAXONOMY_VERSION,
            }),
        },
    )
    .await?;
    let trigger = trigger.map(str::to_string).unwrap_or_else(|| format!("{}:{}", out.category, now));
    enqueue_workflow(&mut tx, &workspace_id, message_id, &trigger).await?;
    // Understand what the email asks (shown to people, used by the drafter).
    jobs::enqueue(&mut tx, kind::ANALYZE_MESSAGE, &format!("analyze:{message_id}"), Some(&workspace_id), &json!({ "message_id": message_id }))
        .await?;
    jobs::enqueue(&mut tx, kind::RUN_AUTOMATIONS, &format!("automations:{message_id}"), Some(&workspace_id), &json!({ "message_id": message_id }))
        .await?;
    super::knowledge_jobs::enqueue_index_message(&mut tx, &workspace_id, message_id).await?;
    jobs::enqueue(&mut tx, kind::LEARN_FROM_MESSAGE, &format!("learn:{message_id}"), Some(&workspace_id), &json!({ "message_id": message_id })).await?;
    // Requirement / referral emails also become structured records for matching (staffing pack only).
    if super::matching_jobs::is_extractable(&out.category)
        && crate::packs::has_feature(&mut *tx, &workspace_id, crate::packs::FEATURE_MATCHING).await?
    {
        jobs::enqueue(
            &mut tx,
            kind::EXTRACT_RECORD,
            &format!("extract:{message_id}:{}", out.category),
            Some(&workspace_id),
            &json!({ "message_id": message_id, "label": out.category }),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(Some(needs_review))
}

async fn enqueue_workflow(
    conn: &mut sqlx::SqliteConnection,
    workspace_id: &str,
    message_id: &str,
    trigger: &str,
) -> sqlx::Result<bool> {
    jobs::enqueue(
        conn,
        kind::EXECUTE_WORKFLOW,
        &format!("workflow:{message_id}:{trigger}"),
        Some(workspace_id),
        &json!({ "message_id": message_id }),
    )
    .await
}

// ---------------------------------------------------------------- workflow

/// Terminal state for a classified message that no workflow picked up (`classified` → `processed`).
async fn mark_processed_if_classified(state: &AppState, message_id: &str) -> Result<(), JobError> {
    sqlx::query("UPDATE email_messages SET processing_status = 'processed' WHERE id = ?1 AND processing_status = 'classified'")
        .bind(message_id)
        .execute(&state.db)
        .await?;
    Ok(())
}

async fn execute_workflow(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let message_id = str_field(payload, "message_id")?;
    let row: Option<(String, String, String)> = sqlx::query_as(
        "SELECT m.workspace_id, c.category, s.status
         FROM email_messages m
         JOIN classifications c ON c.message_id = m.id
         JOIN email_sources s ON s.id = m.source_id
         WHERE m.id = ?1",
    )
    .bind(&message_id)
    .fetch_optional(&state.db)
    .await?;
    let Some((workspace_id, category, source_status)) = row else {
        return Ok(());
    };
    if source_status == "disconnected" {
        return Ok(());
    }
    let hr_workflows = crate::packs::has_feature(&state.db, &workspace_id, crate::packs::FEATURE_HR_WORKFLOWS).await?;
    let Some(template) = domain::WORKFLOW_TEMPLATES.iter().find(|t| hr_workflows && t.label_key == category && t.available)
    else {
        // No workflow for this category: classified is the end of the line.
        return mark_processed_if_classified(state, &message_id).await;
    };
    let workflow: Option<(String, bool)> =
        sqlx::query_as("SELECT id, enabled FROM workflows WHERE workspace_id = ?1 AND template_key = ?2")
            .bind(&workspace_id)
            .bind(template.key)
            .fetch_optional(&state.db)
            .await?;
    let Some((workflow_id, true)) = workflow else {
        return mark_processed_if_classified(state, &message_id).await;
    };

    let now = now_ms();
    let mut tx = state.db.begin().await?;
    let created = sqlx::query(
        "INSERT OR IGNORE INTO workflow_runs (id, workspace_id, workflow_id, message_id, status, created_at)
         VALUES (?1, ?2, ?3, ?4, 'running', ?5)",
    )
    .bind(new_id("run"))
    .bind(&workspace_id)
    .bind(&workflow_id)
    .bind(&message_id)
    .bind(now)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    let (run_id, run_status): (String, String) =
        sqlx::query_as("SELECT id, status FROM workflow_runs WHERE workflow_id = ?1 AND message_id = ?2")
            .bind(&workflow_id)
            .bind(&message_id)
            .fetch_one(&mut *tx)
            .await?;
    if run_status == "completed" {
        tx.commit().await?;
        return Ok(());
    }
    if run_status == "failed" {
        sqlx::query("UPDATE workflow_runs SET status = 'running', error = NULL WHERE id = ?1")
            .bind(&run_id)
            .execute(&mut *tx)
            .await?;
    }
    if created {
        audit::record(
            &mut tx,
            Entry {
                workspace_id: &workspace_id,
                actor: Actor::System,
                action: "workflow.started",
                entity_type: "workflow_run",
                entity_id: &run_id,
                message_id: Some(&message_id),
                metadata: json!({ "template_key": template.key }),
            },
        )
        .await?;
    }
    let review_id = new_id("rev");
    let review_created = sqlx::query(
        "INSERT OR IGNORE INTO review_items (id, workspace_id, message_id, workflow_run_id, kind, title, status, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'open', ?7)",
    )
    .bind(&review_id)
    .bind(&workspace_id)
    .bind(&message_id)
    .bind(&run_id)
    .bind(template.review_kind)
    .bind(template.review_title)
    .bind(now)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if review_created {
        audit::record(
            &mut tx,
            Entry {
                workspace_id: &workspace_id,
                actor: Actor::System,
                action: "review.created",
                entity_type: "review_item",
                entity_id: &review_id,
                message_id: Some(&message_id),
                metadata: json!({ "kind": template.review_kind }),
            },
        )
        .await?;
    }
    jobs::enqueue(
        &mut tx,
        kind::GENERATE_DRAFT,
        &format!("draft:{message_id}:run:{run_id}"),
        Some(&workspace_id),
        &json!({ "message_id": message_id, "workflow_run_id": run_id }),
    )
    .await?;
    tx.commit().await?;
    state.jobs_notify.notify_one();
    Ok(())
}

// ---------------------------------------------------------------- draft

async fn generate_draft(state: &AppState, job: &Job, payload: &Value) -> Result<(), JobError> {
    let message_id = str_field(payload, "message_id")?;
    let run_id = payload.get("workflow_run_id").and_then(Value::as_str).map(str::to_string);
    let msg = load_message(state, &message_id).await?;
    // Automatic drafting can be switched off: then only a person's request (`:manual:`) drafts.
    let auto_reply = crate::reply_settings::load(&state.db, &state.config, &msg.workspace_id).await?.auto_reply;
    if !auto_reply && !job.dedupe_key.contains(":manual:") {
        return finish_without_draft(state, &msg.workspace_id, &message_id, run_id.as_deref(), "automatic drafts are off").await;
    }
    let insight = analyze_message(state, &json!({ "message_id": message_id })).await?;
    // Workflows don't draft replies to email that needs none (newsletters, notifications).
    if run_id.is_some() && !insight.needs_reply {
        return finish_without_draft(state, &msg.workspace_id, &message_id, run_id.as_deref(), "no reply needed").await;
    }
    // An automation may already have drafted this email (and saved it to the mailbox): keep that one.
    let automation_draft: Option<String> = match run_id {
        Some(_) => {
            sqlx::query_scalar("SELECT id FROM drafts WHERE message_id = ?1 AND origin_key LIKE 'automation-draft:%' LIMIT 1")
                .bind(&message_id)
                .fetch_optional(&state.db)
                .await?
        }
        None => None,
    };
    let requested = (payload.get("quality").and_then(Value::as_str) == Some("premium")).then_some(ai::DraftQuality::Premium);
    let by_person = job.dedupe_key.contains(":manual:");
    let generated = match automation_draft {
        Some(_) => None,
        None => Some(compose_draft(state, &message_id, &msg, insight, payload.get("instructions").and_then(Value::as_str), requested, !by_person).await?),
    };
    if let Some((out, _, _)) = &generated
        && out.body_text.trim().is_empty()
    {
        return finish_without_draft(state, &msg.workspace_id, &message_id, run_id.as_deref(), "model judged no reply needed").await;
    }

    let now = now_ms();
    let draft_id = new_id("drf");
    let mut tx = state.db.begin().await?;
    if let Some((out, ctx, quality)) = &generated {
        let inserted = sqlx::query(
            "INSERT OR IGNORE INTO drafts (id, workspace_id, message_id, body_text, original_body_text, status, model_id,
                                           origin_key, memory_ids_json, unanswered_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?4, 'suggested', ?5, ?6, ?8, ?9, ?7, ?7)",
        )
        .bind(&draft_id)
        .bind(&msg.workspace_id)
        .bind(&message_id)
        .bind(&out.body_text)
        .bind(&out.model_id)
        .bind(&job.dedupe_key)
        .bind(now)
        .bind(json!(ctx.ids()).to_string())
        .bind(json!(out.unanswered).to_string())
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        if inserted {
            audit::record(
                &mut tx,
                Entry {
                    workspace_id: &msg.workspace_id,
                    actor: Actor::System,
                    action: "draft.generated",
                    entity_type: "draft",
                    entity_id: &draft_id,
                    message_id: Some(&message_id),
                    metadata: json!({ "model_id": out.model_id, "prompt_version": ai::DRAFT_PROMPT_VERSION, "memories_used": ctx.ids().len(), "tier": quality.as_str() }),
                },
            )
            .await?;
            crate::memory_learning::record_use(&mut tx, &ctx.ids(), now).await?;
        }
    }
    if let Some(run_id) = &run_id {
        let completed = sqlx::query("UPDATE workflow_runs SET status = 'completed', error = NULL WHERE id = ?1 AND status != 'completed'")
            .bind(run_id)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            == 1;
        sqlx::query("UPDATE email_messages SET processing_status = 'workflow_completed', processing_error = NULL WHERE id = ?1")
            .bind(&message_id)
            .execute(&mut *tx)
            .await?;
        if completed {
            audit::record(
                &mut tx,
                Entry {
                    workspace_id: &msg.workspace_id,
                    actor: Actor::System,
                    action: "workflow.completed",
                    entity_type: "workflow_run",
                    entity_id: run_id,
                    message_id: Some(&message_id),
                    metadata: json!({}),
                },
            )
            .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

/// Runs the drafter with the workspace's labels, trusted memory and the email's insight.
/// `extra_instructions` (from an automation) is passed to the drafter as one more memory note.
/// `automated` (nobody asked for this draft): past replies only from the sender's own domain and
/// no auto-learned workspace-wide notes, so outside mail can't pull other clients' data into it.
async fn compose_draft(
    state: &AppState,
    message_id: &str,
    msg: &MessageForAi,
    insight: ai::Insight,
    extra_instructions: Option<&str>,
    requested: Option<ai::DraftQuality>,
    automated: bool,
) -> Result<(ai::DraftOutput, memory::MemoryContext, ai::DraftQuality), JobError> {
    let classification: Option<(String, String)> =
        sqlx::query_as("SELECT category, priority FROM classifications WHERE message_id = ?1")
            .bind(message_id)
            .fetch_optional(&state.db)
            .await?;
    let (category, priority) = classification.unwrap_or_else(|| (OTHER_LABEL.to_string(), Priority::Normal.as_str().to_string()));
    let (category_name, category_description) = labels::active(&state.db, &msg.workspace_id)
        .await?
        .into_iter()
        .find(|l| l.key == category)
        .map(|l| (l.name, l.description))
        .unwrap_or_else(|| (category.clone(), String::new()));
    let staffing = crate::packs::enabled(&state.db, &msg.workspace_id, crate::packs::STAFFING).await?;
    let quality =
        ai::routing::draft_quality(Priority::parse(&priority).unwrap_or(Priority::Normal), &category, Some(&insight), requested, staffing);
    let settings = crate::reply_settings::load(&state.db, &state.config, &msg.workspace_id).await?;
    let reply_instructions = if settings.reply_instructions.is_empty() {
        crate::packs::reply_style(&state.db, &msg.workspace_id).await?.to_string()
    } else {
        settings.reply_instructions
    };
    let default_sign_off = crate::packs::default_sign_off(&state.db, &msg.workspace_id).await?;
    let (reply_instructions, signature) =
        crate::agent_profiles::draft_voice(&state.db, &state.config, &msg.workspace_id, message_id, reply_instructions, settings.signature).await?;

    // Trusted memory (staff notes + learned sender profile), retrieved by sender and topic.
    let ctx = if automated {
        memory::context_for_automation(&state.db, &msg.workspace_id, &msg.sender_address, &msg.subject).await?
    } else {
        memory::context_for(&state.db, &msg.workspace_id, &msg.sender_address, &msg.subject).await?
    };
    let mut memory_notes = ctx.lines();
    if let Some(extra) = extra_instructions.map(str::trim).filter(|e| !e.is_empty()) {
        memory_notes.push(format!("Instruction for this reply: {extra}"));
    }
    let (db, embedder) = (&state.db, state.embedder.as_ref());
    let examples = if automated {
        let domain = msg.sender_address.rsplit_once('@').map(|(_, d)| d.to_lowercase()).unwrap_or_default();
        crate::knowledge::reply_examples_from_domain(db, embedder, &msg.workspace_id, message_id, &msg.subject, &msg.body_text, &domain).await
    } else {
        crate::knowledge::reply_examples(db, embedder, &msg.workspace_id, message_id, &msg.subject, &msg.body_text).await
    };
    let out = state
        .drafter
        .draft(&DraftInput {
            examples,
            category,
            category_name,
            category_description,
            subject: msg.subject.clone(),
            sender: msg.sender_address.clone(),
            body_text: msg.body_text.clone(),
            memory_notes,
            sender_history: ctx.sender_history.clone(),
            insight: Some(insight),
            quality,
            reply_instructions,
            signature,
            default_sign_off,
        })
        .await?;
    Ok((out, ctx, quality))
}

/// Drafts a reply for automations. Reuses the message's existing draft if there is one.
/// `Ok(None)` means the email needs no reply (or the model declined to write one).
pub async fn create_draft_for_message(
    state: &AppState,
    message_id: &str,
    extra_instructions: Option<&str>,
) -> Result<Option<String>, JobError> {
    let existing: Option<String> =
        sqlx::query_scalar("SELECT id FROM drafts WHERE message_id = ?1 ORDER BY created_at DESC, id DESC LIMIT 1")
            .bind(message_id)
            .fetch_optional(&state.db)
            .await?;
    if existing.is_some() {
        return Ok(existing);
    }
    let msg = load_message(state, message_id).await?;
    let insight = analyze_message(state, &json!({ "message_id": message_id })).await?;
    if !insight.needs_reply {
        return Ok(None);
    }
    let (out, ctx, quality) = compose_draft(state, message_id, &msg, insight, extra_instructions, None, true).await?;
    if out.body_text.trim().is_empty() {
        return Ok(None);
    }
    let origin_key = format!("automation-draft:{message_id}");
    let draft_id = new_id("drf");
    let now = now_ms();
    let mut tx = state.db.begin().await?;
    let inserted = sqlx::query(
        "INSERT OR IGNORE INTO drafts (id, workspace_id, message_id, body_text, original_body_text, status, model_id,
                                       origin_key, memory_ids_json, unanswered_json, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?4, 'suggested', ?5, ?6, ?8, ?9, ?7, ?7)",
    )
    .bind(&draft_id)
    .bind(&msg.workspace_id)
    .bind(message_id)
    .bind(&out.body_text)
    .bind(&out.model_id)
    .bind(&origin_key)
    .bind(now)
    .bind(json!(ctx.ids()).to_string())
    .bind(json!(out.unanswered).to_string())
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if inserted {
        audit::record(
            &mut tx,
            Entry {
                workspace_id: &msg.workspace_id,
                actor: Actor::System,
                action: "draft.generated",
                entity_type: "draft",
                entity_id: &draft_id,
                message_id: Some(message_id),
                metadata: json!({ "model_id": out.model_id, "prompt_version": ai::DRAFT_PROMPT_VERSION, "memories_used": ctx.ids().len(), "by": "automation", "tier": quality.as_str() }),
            },
        )
        .await?;
    }
    tx.commit().await?;
    if inserted {
        return Ok(Some(draft_id));
    }
    // A concurrent run (same origin key) won the insert; use its draft.
    Ok(sqlx::query_scalar("SELECT id FROM drafts WHERE origin_key = ?1").bind(&origin_key).fetch_optional(&state.db).await?)
}

/// Drafts a reply for an agent step. One draft per `origin_key` (re-runs reuse it); the agent picks
/// the instructions and tier. `Ok(None)` means the writer judged that no reply is needed.
pub async fn create_agent_draft(
    state: &AppState,
    message_id: &str,
    instructions: Option<&str>,
    requested: Option<ai::DraftQuality>,
    origin_key: &str,
) -> Result<Option<(String, String)>, JobError> {
    let existing: Option<(String, String)> = sqlx::query_as("SELECT id, body_text FROM drafts WHERE origin_key = ?1")
        .bind(origin_key)
        .fetch_optional(&state.db)
        .await?;
    if existing.is_some() {
        return Ok(existing);
    }
    let msg = load_message(state, message_id).await?;
    let insight = analyze_message(state, &json!({ "message_id": message_id })).await?;
    let (out, ctx, quality) = compose_draft(state, message_id, &msg, insight, instructions, requested, true).await?;
    if out.body_text.trim().is_empty() {
        return Ok(None);
    }
    let draft_id = new_id("drf");
    let now = now_ms();
    let mut tx = state.db.begin().await?;
    let inserted = sqlx::query(
        "INSERT OR IGNORE INTO drafts (id, workspace_id, message_id, body_text, original_body_text, status, model_id,
                                       origin_key, memory_ids_json, unanswered_json, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?4, 'suggested', ?5, ?6, ?8, ?9, ?7, ?7)",
    )
    .bind(&draft_id)
    .bind(&msg.workspace_id)
    .bind(message_id)
    .bind(&out.body_text)
    .bind(&out.model_id)
    .bind(origin_key)
    .bind(now)
    .bind(json!(ctx.ids()).to_string())
    .bind(json!(out.unanswered).to_string())
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if inserted {
        audit::record(
            &mut tx,
            Entry {
                workspace_id: &msg.workspace_id,
                actor: Actor::System,
                action: "draft.generated",
                entity_type: "draft",
                entity_id: &draft_id,
                message_id: Some(message_id),
                metadata: json!({ "model_id": out.model_id, "prompt_version": ai::DRAFT_PROMPT_VERSION, "memories_used": ctx.ids().len(), "by": "agent", "tier": quality.as_str() }),
            },
        )
        .await?;
    }
    tx.commit().await?;
    Ok(sqlx::query_as("SELECT id, body_text FROM drafts WHERE origin_key = ?1").bind(origin_key).fetch_optional(&state.db).await?)
}

/// Writes a reply without storing it, so people can try out the agent's instructions.
pub async fn preview_draft(state: &AppState, message_id: &str, requested: Option<ai::DraftQuality>) -> Result<String, JobError> {
    let msg = load_message(state, message_id).await?;
    let insight = analyze_message(state, &json!({ "message_id": message_id })).await?;
    let (out, _, _) = compose_draft(state, message_id, &msg, insight, None, requested, false).await?;
    Ok(out.body_text)
}

// ---------------------------------------------------------------- understanding

/// Returns the stored insight, computing and storing it on first use (idempotent).
/// `summary, asks_json, deadline, needs_reply, missing_json, attachments_hash` of a stored insight.
type StoredInsight = (String, String, Option<String>, bool, String, Option<String>);

pub async fn analyze_message(state: &AppState, payload: &Value) -> Result<ai::Insight, JobError> {
    let message_id = str_field(payload, "message_id")?;
    // Set when attachment text arrived after the first reading: read the email again with it, once per change.
    let refresh = payload.get("refresh_attachments").and_then(Value::as_bool).unwrap_or(false);
    let items = crate::attachments::load_prompt_items(&state.db, &message_id).await?;
    let attachments_block = crate::attachments::prompt_block(&items);
    let attachments_hash = (!attachments_block.is_empty()).then(|| format!("{:x}", Sha256::digest(attachments_block.as_bytes())));
    let stored: Option<StoredInsight> = sqlx::query_as(
        "SELECT summary, asks_json, deadline, needs_reply, missing_json, attachments_hash FROM message_insights WHERE message_id = ?1",
    )
    .bind(&message_id)
    .fetch_optional(&state.db)
    .await?;
    if let Some((summary, asks, deadline, needs_reply, missing, stored_hash)) = stored
        && (!refresh || attachments_hash.is_none() || stored_hash == attachments_hash)
    {
        return Ok(ai::Insight {
            summary,
            asks: serde_json::from_str(&asks).unwrap_or_default(),
            deadline,
            needs_reply,
            missing_info: serde_json::from_str(&missing).unwrap_or_default(),
        });
    }
    let msg = load_message(state, &message_id).await?;
    let requires_reply: Option<bool> = sqlx::query_scalar("SELECT requires_reply FROM classifications WHERE message_id = ?1")
        .bind(&message_id)
        .fetch_optional(&state.db)
        .await?;
    let analyst_body = if attachments_block.is_empty() {
        msg.body_text.clone()
    } else {
        format!("{}{attachments_block}", crate::sources::normalize::truncate_chars(&msg.body_text, ai::MAX_MODEL_INPUT_CHARS))
    };
    let (mut insight, model_id) = state.analyst.analyze(&msg.subject, &msg.sender_address, &analyst_body).await?;
    // Two independent signals must agree before we skip a reply; a cheap judge breaks real ties.
    let has_asks = !insight.asks.is_empty();
    let judge_says = if ai::judge::needs_reply_disputed(insight.needs_reply, requires_reply, has_asks) {
        let context = ai::judge::email_context(&msg.sender_address, &msg.subject, Some(&insight.summary), &msg.body_text);
        match state.judge.judge(&context, &[ai::judge::needs_reply_question()]).await {
            Ok(a) => a.yes_no(ai::judge::NEEDS_REPLY),
            Err(e) => {
                tracing::debug!(error = %e, "needs-reply tie-break unavailable; keeping the rule");
                None
            }
        }
    } else {
        None
    };
    insight.needs_reply = ai::judge::settle_needs_reply(insight.needs_reply, requires_reply, has_asks, judge_says);
    let source = if model_id == "rules-v1" { "rules" } else { "llm" };
    sqlx::query(
        "INSERT INTO message_insights (message_id, summary, asks_json, deadline, needs_reply, missing_json, source, model_id, created_at,
                                      attachments_hash)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(message_id) DO UPDATE SET summary = excluded.summary, asks_json = excluded.asks_json,
           deadline = excluded.deadline, needs_reply = excluded.needs_reply, missing_json = excluded.missing_json,
           source = excluded.source, model_id = excluded.model_id, created_at = excluded.created_at,
           attachments_hash = excluded.attachments_hash
         WHERE ?11",
    )
    .bind(&message_id)
    .bind(&insight.summary)
    .bind(json!(insight.asks).to_string())
    .bind(&insight.deadline)
    .bind(insight.needs_reply)
    .bind(json!(insight.missing_info).to_string())
    .bind(source)
    .bind(&model_id)
    .bind(now_ms())
    .bind(&attachments_hash)
    .bind(refresh)
    .execute(&state.db)
    .await?;
    Ok(insight)
}

async fn finish_without_draft(
    state: &AppState,
    workspace_id: &str,
    message_id: &str,
    run_id: Option<&str>,
    reason: &str,
) -> Result<(), JobError> {
    let mut tx = state.db.begin().await?;
    if let Some(run_id) = run_id {
        sqlx::query("UPDATE workflow_runs SET status = 'completed', error = NULL WHERE id = ?1").bind(run_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE email_messages SET processing_status = 'workflow_completed', processing_error = NULL WHERE id = ?1")
            .bind(message_id)
            .execute(&mut *tx)
            .await?;
    }
    audit::record(
        &mut tx,
        Entry {
            workspace_id,
            actor: Actor::System,
            action: "draft.skipped",
            entity_type: "message",
            entity_id: message_id,
            message_id: Some(message_id),
            metadata: json!({ "reason": reason }),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

// ---------------------------------------------------------------- learning

/// Compares the suggested draft with what an operator approved and stores up to three
/// reusable lessons as *suggestions*. Nothing is used until a person accepts it.
async fn learn_from_draft(state: &AppState, payload: &Value) -> Result<(), JobError> {
    // An approved reply also teaches durable facts (once per draft).
    super::learning_jobs::enqueue_reply_learning(&state.db, &str_field(payload, "draft_id")?).await?;
    let Some(learner) = state.learner.clone() else {
        return Ok(());
    };
    let draft_id = str_field(payload, "draft_id")?;
    let row: Option<(String, String, Option<String>, String, String)> = sqlx::query_as(
        "SELECT d.workspace_id, d.message_id, d.original_body_text, d.body_text, m.sender_address
         FROM drafts d JOIN email_messages m ON m.id = d.message_id WHERE d.id = ?1",
    )
    .bind(&draft_id)
    .fetch_optional(&state.db)
    .await?;
    let Some((workspace_id, message_id, Some(original), approved, sender)) = row else {
        return Ok(());
    };
    if memory::dedupe_key(&original) == memory::dedupe_key(&approved) {
        return Ok(());
    }
    let lessons = learner.learn(&original, &approved).await?;
    let suggestions = lessons
        .into_iter()
        .map(|l| memory::Suggestion { kind: l.kind, scope: l.scope, content: l.content })
        .collect();
    let stored = memory::store_suggestions(&state.db, &workspace_id, &sender, &message_id, suggestions).await?;
    tracing::info!(%draft_id, stored, "learned from draft edit");
    Ok(())
}

// ---------------------------------------------------------------- retention

async fn backup_database(state: &AppState) -> Result<(), JobError> {
    let Some(dir) = crate::backup::backup_dir(&state.config.database_url) else { return Ok(()) };
    let today = time::OffsetDateTime::now_utc().date();
    match crate::backup::run(&state.db, &dir, today, crate::backup::KEEP).await {
        Ok(Some(path)) => tracing::info!(path = %path.display(), "database backup written"),
        Ok(None) => {}
        Err(e) => return Err(JobError::Retry(format!("database backup failed: {e:#}"))),
    }
    Ok(())
}

async fn purge_expired_events(state: &AppState) -> Result<(), JobError> {
    let cutoff = now_ms() - state.config.email_retention_days * 86_400_000;
    let purged = sqlx::query("UPDATE inbound_events SET payload_json = '{}' WHERE created_at < ?1 AND payload_json != '{}'")
        .bind(cutoff)
        .execute(&state.db)
        .await?
        .rows_affected();
    let jobs = sqlx::query("DELETE FROM jobs WHERE status = 'succeeded' AND updated_at < ?1")
        .bind(cutoff)
        .execute(&state.db)
        .await?
        .rows_affected();
    sqlx::query("DELETE FROM judge_cache WHERE created_at < ?1").bind(cutoff).execute(&state.db).await?;
    sqlx::query("DELETE FROM email_outbox WHERE created_at < ?1").bind(cutoff).execute(&state.db).await?;
    let rate_events = crate::security::purge_rate_events(&state.db).await?;
    tracing::info!(purged_payloads = purged, deleted_jobs = jobs, rate_events, "retention purge");
    Ok(())
}
