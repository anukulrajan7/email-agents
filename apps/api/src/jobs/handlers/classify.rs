use super::*;

pub(super) async fn classify_email(state: &AppState, job: &Job, payload: &Value) -> Result<(), JobError> {
    let message_id = str_field(payload, "message_id")?;
    if crate::jobs::message_jobs::is_outbound(state, &message_id).await? {
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

    // Bulk and automated mail is filed by rules: no model call. A person's request (reprocess, opening the email) skips this.
    let full_treatment = job.dedupe_key.contains(":reprocess:") || payload.get("full").and_then(Value::as_bool) == Some(true);
    if !full_treatment && let Some(signals) = crate::mail::bulk::should_file(&state.db, &msg.workspace_id, &message_id).await? {
        return file_as_bulk(state, job, &msg, &message_id, &signals).await;
    }

    let attachment_names: Vec<String> = sqlx::query_scalar("SELECT filename FROM attachments WHERE message_id = ?1")
        .bind(&message_id)
        .fetch_all(&state.db)
        .await?;
    let automated_signals: String =
        sqlx::query_scalar("SELECT bulk_signals FROM email_messages WHERE id = ?1").bind(&message_id).fetch_optional(&state.db).await?.unwrap_or_default();
    let automated = crate::learning::memory_learning::is_automated_sender(&crate::hr::staffing::policy::address_of(&msg.sender_address))
        || serde_json::from_str::<Vec<String>>(&automated_signals).is_ok_and(|s| !s.is_empty());
    if !automated
        && crate::hr::staffing::timesheet_records::looks_like_timesheet(&msg.subject, &attachment_names)
        && labels::is_active(&state.db, &msg.workspace_id, crate::hr::staffing::timesheet_records::LABEL).await?
    {
        let out = ClassifyOutput {
            category: crate::hr::staffing::timesheet_records::LABEL.into(),
            priority: Priority::Normal,
            requires_reply: false,
            confidence: Some(0.95),
            source: ClassificationSource::Fallback,
            model_id: Some(crate::hr::staffing::timesheet_records::MODEL_ID.into()),
            model_version: None,
        };
        persist_classification_with(state, &message_id, &out, Actor::System, Some(&job.id), WriteMode::Upsert, None, false).await?;
        state.jobs_notify.notify_one();
        return Ok(());
    }
    let sender_history = memory::context_for(&state.db, &msg.workspace_id, &msg.sender_address, "").await?.sender_history;
    // Over the global inbound cap, automatic labelling uses the free rules (no model, no embedding).
    let over_global_cap = !job.dedupe_key.contains(":reprocess:")
        && !ledger::within_global_budget(&state.db, state.config.ai_global_daily_budget_usd).await;
    let examples = if over_global_cap {
        vec![]
    } else {
        crate::learning::knowledge::label_examples(&state.db, state.embedder.as_ref(), &msg.workspace_id, &message_id, &msg.subject, &msg.body_text).await
    };
    let input = ClassifyInput {
        examples,
        body_text: ai::body::model_body(&msg.subject, &msg.body_text),
        subject: msg.subject,
        sender: msg.sender_address,
        attachment_names,
        sender_history,
        labels: labels::active(&state.db, &msg.workspace_id).await?,
    };
    let mode = state.config.understand_mode;
    let mut understood: Option<StoredUnderstanding> = None;
    let out = if over_global_cap {
        tracing::warn!("global daily AI budget reached: labelling with rules");
        ai::Classifier::classify(&ai::fallback::RulesClassifier, &input).await?
    } else {
        // v2: one call labels and reads the email. Any failure falls back to today's calls.
        let v2 = if mode == UnderstandMode::On { understand_for_classify(state, &message_id, &input).await } else { None };
        match v2 {
            Some((out, stored)) => {
                understood = Some(stored);
                out
            }
            None => {
                let out = state.classifier.classify(&input).await?;
                if mode == UnderstandMode::Shadow {
                    shadow_compare(state, &msg.workspace_id, &message_id, &input, &out).await;
                }
                out
            }
        }
    };
    persist_classification_with(state, &message_id, &out, Actor::System, Some(&job.id), WriteMode::Upsert, understood.as_ref(), false).await?;
    state.jobs_notify.notify_one();
    Ok(())
}

/// The v2 input for one email: its new part, attachments, thread digest and the workspace taxonomy.
async fn understand_input(
    state: &AppState,
    message_id: &str,
    input: &ClassifyInput,
) -> Result<(UnderstandInput, Option<String>), JobError> {
    let received_at_ms: i64 = sqlx::query_scalar("SELECT received_at FROM email_messages WHERE id = ?1")
        .bind(message_id)
        .fetch_one(&state.db)
        .await?;
    let items = crate::mail::attachments::load_prompt_items(&state.db, message_id).await?;
    let attachments_block = crate::mail::attachments::prompt_block(&items);
    let attachments_hash = (!attachments_block.is_empty()).then(|| format!("{:x}", Sha256::digest(attachments_block.as_bytes())));
    Ok((
        UnderstandInput {
            subject: input.subject.clone(),
            sender: input.sender.clone(),
            received_at_ms,
            new_part: input.body_text.clone(),
            attachments_block,
            attachment_names: input.attachment_names.clone(),
            labels: input.labels.clone(),
            examples: input.examples.clone(),
            sender_history: input.sender_history.clone(),
            thread_digest: thread_digest(state, message_id).await?,
        },
        attachments_hash,
    ))
}

async fn understand_for_classify(state: &AppState, message_id: &str, input: &ClassifyInput) -> Option<(ClassifyOutput, StoredUnderstanding)> {
    if !state.understander.available() {
        return None;
    }
    let result = async {
        let (ui, attachments_hash) = understand_input(state, message_id, input).await.map_err(|e| e.to_string())?;
        let (u, model_id) = state.understander.understand(&ui).await.map_err(|e| e.to_string())?;
        Ok::<_, String>((u, model_id, attachments_hash))
    }
    .await;
    match result {
        Ok((u, model_id, attachments_hash)) => Some((u.classify_output(&model_id), StoredUnderstanding { understanding: u, model_id, attachments_hash })),
        Err(e) => {
            tracing::warn!(error = %e, %message_id, "understanding v2 failed: falling back to separate classify and analyze calls");
            None
        }
    }
}

/// Shadow mode: run v2 beside the production label and record whether they agree. Never affects the email.
async fn shadow_compare(state: &AppState, workspace_id: &str, message_id: &str, input: &ClassifyInput, v1: &ClassifyOutput) {
    if !state.understander.available() {
        return;
    }
    let v2 = match understand_input(state, message_id, input).await {
        Ok((ui, _)) => state.understander.understand(&ui).await,
        Err(e) => Err(AiError::Retryable(e.to_string())),
    };
    if let Err(e) = ai::understand::record_shadow(&state.db, workspace_id, message_id, v1, &v2).await {
        tracing::warn!(error = %e, "could not record the understanding shadow comparison");
    }
}

/// Files bulk mail with the rules label and an empty, no-reply insight. Nothing else runs on it except
/// indexing for search.
async fn file_as_bulk(state: &AppState, job: &Job, msg: &MessageForAi, message_id: &str, signals: &[String]) -> Result<(), JobError> {
    let active = labels::active(&state.db, &msg.workspace_id).await?;
    let category = labels::coerce(&active, "newsletters");
    let out = ClassifyOutput {
        category,
        priority: Priority::Low,
        requires_reply: false,
        confidence: Some(0.99),
        source: ClassificationSource::Fallback,
        model_id: Some(crate::mail::bulk::MODEL_ID.into()),
        model_version: None,
    };
    let written = persist_classification_with(state, message_id, &out, Actor::System, Some(&job.id), WriteMode::Upsert, None, true).await?;
    if written.is_some() {
        tracing::info!(%message_id, ?signals, "filed as bulk mail by rules: no model call");
    }
    state.jobs_notify.notify_one();
    Ok(())
}
