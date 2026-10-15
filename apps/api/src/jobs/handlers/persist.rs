use super::*;

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
    persist_classification_with(state, message_id, out, actor, trigger, mode, None, false).await
}

/// `bulk`: filed by rules. The insight is written here (nothing needs a model) and only indexing is queued.
/// `understanding`: a v2 reading made by the same call as the label; its insight is stored in the same transaction.
#[allow(clippy::too_many_arguments)]
pub(super) async fn persist_classification_with(
    state: &AppState,
    message_id: &str,
    out: &ClassifyOutput,
    actor: Actor<'_>,
    trigger: Option<&str>,
    mode: WriteMode,
    understanding: Option<&StoredUnderstanding>,
    bulk: bool,
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
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
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
    if bulk {
        let summary = {
            let subject: String = sqlx::query_scalar("SELECT subject FROM email_messages WHERE id = ?1").bind(message_id).fetch_one(&mut *tx).await?;
            crate::mail::sources::normalize::truncate_chars(subject.trim(), 200)
        };
        sqlx::query(
            "INSERT OR IGNORE INTO message_insights (message_id, summary, asks_json, deadline, needs_reply, missing_json, source, model_id, created_at)
             VALUES (?1, ?2, '[]', NULL, 0, '[]', 'rules', ?3, ?4)",
        )
        .bind(message_id)
        .bind(summary)
        .bind(crate::mail::bulk::MODEL_ID)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE email_messages SET processing_status = 'processed' WHERE id = ?1 AND processing_status = 'classified'")
            .bind(message_id)
            .execute(&mut *tx)
            .await?;
        crate::jobs::knowledge_jobs::enqueue_index_message(&mut tx, &workspace_id, message_id).await?;
        tx.commit().await?;
        return Ok(Some(needs_review));
    }
    if let Actor::User(user_id) = actor
        && matches!(out.source, ClassificationSource::Manual)
    {
        // A person set the label: it confirms the model's pick or corrects it.
        let model_pick: Option<String> =
            sqlx::query_scalar("SELECT ai_category FROM classifications WHERE message_id = ?1").bind(message_id).fetch_one(&mut *tx).await?;
        learning::hooks::label_decision(&mut tx, &workspace_id, user_id, message_id, model_pick.as_deref(), &out.category).await?;
    }
    let trigger = trigger.map(str::to_string).unwrap_or_else(|| format!("{}:{}", out.category, now));
    let analyzed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM message_insights WHERE message_id = ?1)")
        .bind(message_id)
        .fetch_one(&mut *tx)
        .await?;
    if let Some(u) = understanding {
        // The same call that labelled the email also read it: store the insight with the label, in one transaction.
        store_insight(&mut tx, message_id, &u.understanding.insight(), &u.model_id, u.attachments_hash.as_deref(), Some(&u.understanding), true).await?;
    }
    if analyzed {
        // Re-labelled mail already has its insight: the downstream stages can follow right away.
        enqueue_after_analysis(&mut tx, &workspace_id, message_id, &trigger).await?;
    } else {
        // Single flight: only the analysis job reads the email with a model. It enqueues the workflow,
        // automations and memory learning once its result is stored.
        jobs::enqueue(
            &mut tx,
            kind::ANALYZE_MESSAGE,
            &format!("analyze:{message_id}"),
            Some(&workspace_id),
            &json!({ "message_id": message_id, "trigger": trigger }),
        )
        .await?;
    }
    crate::jobs::knowledge_jobs::enqueue_index_message(&mut tx, &workspace_id, message_id).await?;
    // Requirement / referral emails also become structured records for matching (staffing pack only).
    if crate::jobs::matching_jobs::is_extractable(&out.category)
        && crate::platform::packs::has_feature(&mut *tx, &workspace_id, crate::platform::packs::FEATURE_MATCHING).await?
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
    if out.category == crate::hr::staffing::timesheet_records::LABEL && crate::platform::packs::enabled(&mut *tx, &workspace_id, crate::platform::packs::STAFFING).await? {
        jobs::enqueue(
            &mut tx,
            kind::RECORD_TIMESHEET,
            &format!("timesheet:{message_id}:{trigger}"),
            Some(&workspace_id),
            &json!({ "message_id": message_id }),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(Some(needs_review))
}

/// The stages that read the stored insight, queued only after it exists: workflow, automations, memory learning.
async fn enqueue_after_analysis(
    conn: &mut sqlx::SqliteConnection,
    workspace_id: &str,
    message_id: &str,
    trigger: &str,
) -> sqlx::Result<()> {
    enqueue_workflow(conn, workspace_id, message_id, trigger).await?;
    jobs::enqueue(conn, kind::RUN_AUTOMATIONS, &format!("automations:{message_id}"), Some(workspace_id), &json!({ "message_id": message_id }))
        .await?;
    jobs::enqueue(conn, kind::LEARN_FROM_MESSAGE, &format!("learn:{message_id}"), Some(workspace_id), &json!({ "message_id": message_id }))
        .await?;
    Ok(())
}

/// After a first analysis (payload carries the classification `trigger`): hand the email to the next stages.
pub(super) async fn continue_pipeline(state: &AppState, job: &Job, payload: &Value) -> Result<(), JobError> {
    let (Some(trigger), Some(ws)) = (payload.get("trigger").and_then(Value::as_str), job.workspace_id.as_deref()) else {
        return Ok(());
    };
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    enqueue_after_analysis(&mut tx, ws, &str_field(payload, "message_id")?, trigger).await?;
    tx.commit().await?;
    state.jobs_notify.notify_one();
    Ok(())
}

pub(super) async fn enqueue_workflow(
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
