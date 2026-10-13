//! Pipeline stages: fetch → normalize/persist → classify → workflow → draft.
//! Each stage writes its result and enqueues the next stage in one transaction whose
//! first statement is a write (so SQLite takes the write lock up front).

use serde_json::{Value, json};

use super::{Job, kind};
use crate::{
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

fn str_field(payload: &Value, key: &str) -> Result<String, JobError> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| JobError::Permanent(format!("payload missing {key}")))
}

pub async fn handle(state: &AppState, job: &Job) -> Result<(), JobError> {
    let payload = job.payload();
    match job.kind.as_str() {
        kind::FETCH_FORWARDED_EMAIL => fetch_forwarded_email(state, &payload).await,
        kind::CLASSIFY_EMAIL => classify_email(state, job, &payload).await,
        kind::EXECUTE_WORKFLOW => execute_workflow(state, &payload).await,
        kind::GENERATE_DRAFT => generate_draft(state, job, &payload).await,
        kind::PURGE_EXPIRED_EVENTS => purge_expired_events(state).await,
        kind::SYNC_NANGO_SOURCE => crate::sources::nango::sync_source(state, &payload).await,
        kind::LEARN_FROM_DRAFT => learn_from_draft(state, &payload).await,
        kind::ANALYZE_MESSAGE => analyze_message(state, &payload).await.map(|_| ()),
        kind::EXTRACT_RECORD => super::matching_jobs::extract_record(state, &payload).await,
        kind::MATCH_RECORD => super::matching_jobs::match_record(state, &payload).await,
        kind::REGISTER_NANGO_CONNECTION => crate::sources::nango::register_from_webhook(state, &payload).await,
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
            kind::FETCH_FORWARDED_EMAIL => {
                if let Some(event_id) = payload.get("event_id").and_then(Value::as_str) {
                    sqlx::query("UPDATE inbound_events SET status = 'rejected' WHERE id = ?1")
                        .bind(event_id)
                        .execute(&state.db)
                        .await?;
                }
                Ok(())
            }
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
    let input = ClassifyInput {
        subject: msg.subject,
        sender: msg.sender_address,
        body_text: msg.body_text,
        attachment_names,
        sender_history,
        labels: labels::active(&state.db, &msg.workspace_id).await?,
    };
    let out = state.classifier.classify(&input).await?;
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
           updated_at = excluded.updated_at
         WHERE classifications.source != 'manual' OR excluded.source = 'manual'"
        }
    };
    let mut tx = state.db.begin().await?;
    let written = sqlx::query(&format!(
        "INSERT INTO classifications
           (message_id, category, priority, requires_reply, confidence, needs_review, source, model_id, model_version,
            taxonomy_version, prompt_version, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
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
    // Requirement / referral emails also become structured records for matching.
    if super::matching_jobs::is_extractable(&out.category) {
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
    let Some(template) = domain::WORKFLOW_TEMPLATES.iter().find(|t| t.label_key == category && t.available)
    else {
        // No automation for this category: stays in the inbox for a human.
        return Ok(());
    };
    let workflow: Option<(String, bool)> =
        sqlx::query_as("SELECT id, enabled FROM workflows WHERE workspace_id = ?1 AND template_key = ?2")
            .bind(&workspace_id)
            .bind(template.key)
            .fetch_optional(&state.db)
            .await?;
    let Some((workflow_id, true)) = workflow else {
        return Ok(());
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
    let category: Option<String> = sqlx::query_scalar("SELECT category FROM classifications WHERE message_id = ?1")
        .bind(&message_id)
        .fetch_optional(&state.db)
        .await?;
    let category = category.unwrap_or_else(|| OTHER_LABEL.to_string());
    let insight = analyze_message(state, &json!({ "message_id": message_id })).await?;
    // Workflows don't draft replies to email that needs none (newsletters, notifications).
    if run_id.is_some() && !insight.needs_reply {
        return finish_without_draft(state, &msg.workspace_id, &message_id, run_id.as_deref(), "no reply needed").await;
    }
    let category_name = labels::active(&state.db, &msg.workspace_id)
        .await?
        .into_iter()
        .find(|l| l.key == category)
        .map(|l| l.name)
        .unwrap_or_else(|| category.clone());

    // Trusted memory (staff notes + learned sender profile), retrieved by sender and topic.
    let ctx = memory::context_for(&state.db, &msg.workspace_id, &msg.sender_address, &msg.subject).await?;
    let out = state
        .drafter
        .draft(&DraftInput {
            category,
            category_name,
            subject: msg.subject,
            sender: msg.sender_address,
            body_text: msg.body_text,
            memory_notes: ctx.lines(),
            sender_history: ctx.sender_history.clone(),
            insight: Some(insight),
        })
        .await?;
    if out.body_text.trim().is_empty() {
        return finish_without_draft(state, &msg.workspace_id, &message_id, run_id.as_deref(), "model judged no reply needed").await;
    }

    let now = now_ms();
    let draft_id = new_id("drf");
    let mut tx = state.db.begin().await?;
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
                metadata: json!({ "model_id": out.model_id, "prompt_version": ai::DRAFT_PROMPT_VERSION, "memories_used": ctx.ids().len() }),
            },
        )
        .await?;
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

// ---------------------------------------------------------------- understanding

/// Returns the stored insight, computing and storing it on first use (idempotent).
pub async fn analyze_message(state: &AppState, payload: &Value) -> Result<ai::Insight, JobError> {
    let message_id = str_field(payload, "message_id")?;
    let stored: Option<(String, String, Option<String>, bool, String)> = sqlx::query_as(
        "SELECT summary, asks_json, deadline, needs_reply, missing_json FROM message_insights WHERE message_id = ?1",
    )
    .bind(&message_id)
    .fetch_optional(&state.db)
    .await?;
    if let Some((summary, asks, deadline, needs_reply, missing)) = stored {
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
    let (mut insight, model_id) = state.analyst.analyze(&msg.subject, &msg.sender_address, &msg.body_text).await?;
    // Two independent signals must agree before we skip a reply.
    insight.needs_reply = insight.needs_reply || (requires_reply == Some(true) && !insight.asks.is_empty());
    let source = if model_id == "rules-v1" { "rules" } else { "llm" };
    sqlx::query(
        "INSERT INTO message_insights (message_id, summary, asks_json, deadline, needs_reply, missing_json, source, model_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) ON CONFLICT(message_id) DO NOTHING",
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
    tracing::info!(purged_payloads = purged, deleted_jobs = jobs, "retention purge");
    Ok(())
}
