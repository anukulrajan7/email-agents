use super::*;

pub(super) struct UpdateCase;

#[async_trait]
impl Tool for UpdateCase {
    fn name(&self) -> &'static str {
        "update_case"
    }
    fn description(&self) -> &'static str {
        "Record progress on the case and, when known, its outcome."
    }
    fn schema(&self) -> Value {
        object(
            json!({"status_note": {"type": "string"}, "outcome": {"type": "string", "enum": crate::agents::OUTCOMES}}),
            &["status_note"],
        )
    }
    fn risk(&self) -> Risk {
        Risk::Low
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let note = sentence(str_arg(args, "status_note")?, 240);
        let outcome = opt_str(args, "outcome");
        if outcome.is_some_and(|o| !crate::agents::OUTCOMES.contains(&o)) {
            return failed("outcome must be one of submitted, interview, rejected, placed, closed");
        }
        sqlx::query("UPDATE cases SET status_note = ?1, outcome = COALESCE(?2, outcome), updated_at = ?3 WHERE id = ?4 AND workspace_id = ?5")
            .bind(&note)
            .bind(outcome)
            .bind(now_ms())
            .bind(&ctx.case.id)
            .bind(ctx.ws())
            .execute(&ctx.state.db)
            .await?;
        let summary = match outcome {
            Some(o) => format!("Recorded the outcome as {o}: {note}"),
            None => format!("Progress: {note}"),
        };
        Ok(ToolOutput { summary, value: json!({"status_note": note, "outcome": outcome}), ..Default::default() })
    }
}
