use super::*;

async fn mark_processed_if_classified(state: &AppState, message_id: &str) -> Result<(), JobError> {
    sqlx::query("UPDATE email_messages SET processing_status = 'processed' WHERE id = ?1 AND processing_status = 'classified'")
        .bind(message_id)
        .execute(&state.db)
        .await?;
    Ok(())
}

pub(super) async fn execute_workflow(state: &AppState, payload: &Value) -> Result<(), JobError> {
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
    if category == crate::hr::staffing::timesheet_records::LABEL && crate::platform::packs::enabled(&state.db, &workspace_id, crate::platform::packs::STAFFING).await? {
        // Under the `handoff` flag Rohan's case owns the sheet: it records it and asks a person to look when it needs one.
        let desk_case: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM cases WHERE workspace_id = ?1 AND message_id = ?2 AND job = ?3 AND shadow = 0 AND status NOT IN ('failed', 'cancelled') AND json_extract(state_json, '$.desk') = 1)",
        )
        .bind(&workspace_id)
        .bind(&message_id)
        .bind(crate::agents::ownership::DESK_JOB)
        .fetch_one(&state.db)
        .await?;
        if desk_case && crate::platform::flags::mode(&state.db, &state.config, &workspace_id, crate::platform::flags::Key::Handoff).await?.acts() {
            return mark_processed_if_classified(state, &message_id).await;
        }
        // A timesheet that was recorded cleanly needs no review item; one that needs a look gets the usual one below.
        let run = crate::hr::staffing::timesheet_records::record_message(state, &workspace_id, &message_id).await?;
        if run.deferred {
            return Err(JobError::Retry("the timesheet could not be read by the model yet".into()));
        }
        let nothing_recorded = run.records.is_empty() && !run.waiting_for_files;
        if !nothing_recorded && !run.needs_attention() {
            return mark_processed_if_classified(state, &message_id).await;
        }
    }
    let hr_workflows = crate::platform::packs::has_feature(&state.db, &workspace_id, crate::platform::packs::FEATURE_HR_WORKFLOWS).await?;
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
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
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
         SELECT ?1, ?2, ?3, ?4, ?5, ?6, 'open', ?7
         WHERE NOT EXISTS (SELECT 1 FROM review_items WHERE message_id = ?3 AND status = 'open' AND kind = 'agent_task' AND title = 'Check: Timesheet' AND ?5 = 'check_timesheet')",
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
