use super::*;

pub(super) struct CreateTask;

#[async_trait]
impl Tool for CreateTask {
    fn name(&self) -> &'static str {
        "create_task"
    }
    fn description(&self) -> &'static str {
        "Ask a person on the team to do something (shows in Reviews)."
    }
    fn schema(&self) -> Value {
        object(json!({"title": {"type": "string"}, "message_id": {"type": "string", "description": "Defaults to the case's email"}}), &["title"])
    }
    fn risk(&self) -> Risk {
        Risk::Low
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let title = sentence(str_arg(args, "title")?, 200);
        let Some(message_id) = opt_str(args, "message_id").or(ctx.case.message_id.as_deref()) else {
            return failed("A task needs an email to attach to.");
        };
        let msg = message_in_ws(ctx, message_id).await?;
        let review_id = new_id("rev");
        let mut tx = ctx.state.db.begin_with("BEGIN IMMEDIATE").await?;
        let created = sqlx::query(
            "INSERT INTO review_items (id, workspace_id, message_id, workflow_run_id, kind, title, status, created_at)
             SELECT ?1, ?2, ?3, NULL, 'agent_task', ?4, 'open', ?5
             WHERE NOT EXISTS (SELECT 1 FROM review_items WHERE message_id = ?3 AND kind = 'agent_task' AND title = ?4 AND status = 'open')",
        )
        .bind(&review_id)
        .bind(ctx.ws())
        .bind(&msg.id)
        .bind(&title)
        .bind(now_ms())
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        if created {
            audit::record(
                &mut tx,
                Entry {
                    workspace_id: ctx.ws(),
                    actor: Actor::System,
                    action: "review.created",
                    entity_type: "review_item",
                    entity_id: &review_id,
                    message_id: Some(&msg.id),
                    metadata: json!({"kind": "agent_task", "case_id": ctx.case.id}),
                },
            )
            .await?;
        }
        tx.commit().await?;
        Ok(ToolOutput {
            summary: format!("Asked the team: {title}"),
            value: json!({"created": created, "title": title}),
            ..Default::default()
        })
    }
}
