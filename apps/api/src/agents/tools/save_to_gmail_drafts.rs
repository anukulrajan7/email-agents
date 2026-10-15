use super::*;

pub(super) struct SaveToGmailDrafts;

#[async_trait]
impl Tool for SaveToGmailDrafts {
    fn name(&self) -> &'static str {
        "save_to_gmail_drafts"
    }
    fn description(&self) -> &'static str {
        "Save a drafted reply to the mailbox's Drafts folder so a person can finish it there. Nothing is sent."
    }
    fn schema(&self) -> Value {
        object(json!({"draft_id": {"type": "string"}}), &["draft_id"])
    }
    fn risk(&self) -> Risk {
        Risk::Low
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let draft_id = str_arg(args, "draft_id")?;
        if ctx.case.shadow {
            return Ok(ToolOutput {
                summary: "Observe mode: would have saved the draft to Gmail Drafts; nothing was done".into(),
                value: json!({"simulated": true}),
                ..Default::default()
            });
        }
        let rc = replies::load(ctx.state, ctx.ws(), draft_id).await?;
        if !crate::ai::policy::can_create_provider_draft(&replies::capabilities(&rc)) {
            return Ok(ToolOutput {
                summary: "This mailbox cannot hold Gmail drafts, so the draft stays in Post Room".into(),
                value: json!({"saved": false}),
                ..Default::default()
            });
        }
        let provider_draft_id = replies::save_draft(ctx.state, ctx.ws(), draft_id, ctx.actor).await?;
        Ok(ToolOutput { summary: "Saved the draft to Gmail Drafts".into(), value: json!({"saved": true, "provider_draft_id": provider_draft_id}), ..Default::default() })
    }
}
