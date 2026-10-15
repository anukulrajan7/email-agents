use super::*;

pub(super) struct LabelEmail;

#[async_trait]
impl Tool for LabelEmail {
    fn name(&self) -> &'static str {
        "label_email"
    }
    fn description(&self) -> &'static str {
        "Give an email one of this workspace's labels (a person's own label is never overwritten)."
    }
    fn schema(&self) -> Value {
        object(json!({"message_id": {"type": "string"}, "label": {"type": "string", "description": "A label key"}}), &["message_id", "label"])
    }
    fn risk(&self) -> Risk {
        Risk::Low
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let msg = message_in_ws(ctx, str_arg(args, "message_id")?).await?;
        let label = str_arg(args, "label")?;
        if ctx.case.shadow {
            return Ok(ToolOutput {
                summary: format!("Observe mode: would have labelled it {label}; nothing was done"),
                value: json!({"simulated": true}),
                ..Default::default()
            });
        }
        if !crate::understanding::labels::is_active(&ctx.state.db, ctx.ws(), label).await? {
            return failed("That label does not exist in this workspace.");
        }
        let existing: Option<(String, String, bool, String)> =
            sqlx::query_as("SELECT category, priority, requires_reply, source FROM classifications WHERE message_id = ?1")
                .bind(&msg.id)
                .fetch_optional(&ctx.state.db)
                .await?;
        let Some((category, priority, requires_reply, source)) = existing else { return failed("This email has not been labelled yet.") };
        if source == "manual" {
            return Ok(ToolOutput {
                summary: format!("Kept the label a person chose ({category})"),
                value: json!({"label": category, "kept": true}),
                ..Default::default()
            });
        }
        let out = crate::ai::ClassifyOutput {
            category: label.to_string(),
            priority: crate::platform::domain::Priority::parse(&priority).unwrap_or(crate::platform::domain::Priority::Normal),
            requires_reply,
            confidence: None,
            source: crate::platform::domain::ClassificationSource::Llm,
            model_id: Some("agent".into()),
            model_version: None,
        };
        handlers::persist_classification(ctx.state, &msg.id, &out, ctx.actor, None, handlers::WriteMode::Upsert).await?;
        Ok(ToolOutput { summary: format!("Labelled the email {label}"), value: json!({"label": label}), ..Default::default() })
    }
}
