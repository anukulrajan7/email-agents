use super::*;

#[derive(sqlx::FromRow)]
struct DraftRow {
    id: String,
    body_text: String,
    status: String,
    subject: String,
    sender_address: String,
    thread_id: String,
}

pub(super) async fn draft_preview(ctx: &ToolCtx<'_>, draft_id: &str) -> Result<Preview, ToolError> {
    let d: DraftRow = sqlx::query_as(
        "SELECT d.id, d.body_text, d.status, m.subject, m.sender_address, m.thread_id
         FROM drafts d JOIN email_messages m ON m.id = d.message_id WHERE d.id = ?1 AND d.workspace_id = ?2",
    )
    .bind(draft_id)
    .bind(ctx.ws())
    .fetch_optional(&ctx.state.db)
    .await?
    .ok_or_else(|| ToolError::Failed("That draft was not found in this workspace.".into()))?;
    Ok(Preview {
        summary: format!("Send reply to {}", d.sender_address),
        to: Some(d.sender_address),
        subject: Some(replies::reply_subject(&d.subject)),
        body: d.body_text,
        draft_id: Some(d.id),
        draft_status: Some(d.status),
        thread_id: Some(d.thread_id),
        ..Default::default()
    })
}

pub(super) struct SendEmail;

#[async_trait]
impl Tool for SendEmail {
    fn name(&self) -> &'static str {
        "send_email"
    }
    fn description(&self) -> &'static str {
        "Send a drafted reply from the connected mailbox. A person approves it first."
    }
    fn schema(&self) -> Value {
        object(json!({"draft_id": {"type": "string"}}), &["draft_id"])
    }
    fn risk(&self) -> Risk {
        Risk::High
    }
    async fn preview(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<Option<Preview>, ToolError> {
        Ok(Some(draft_preview(ctx, str_arg(args, "draft_id")?).await?))
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let preview = draft_preview(ctx, str_arg(args, "draft_id")?).await?;
        let draft_id = preview.draft_id.clone().unwrap_or_default();
        let scheduled = replies::send_draft_held(ctx.state, ctx.ws(), &draft_id, ctx.actor, ctx.hold_secs).await?;
        Ok(ToolOutput {
            summary: format!("Reply to {} is sending (can be undone for a few seconds)", preview.to.unwrap_or_default()),
            value: json!({"scheduled": true, "draft_id": draft_id, "send_at": scheduled.send_at, "send_at_ms": scheduled.send_at_ms}),
            state: Some(json!({"last_sent_draft_id": draft_id})),
            ..Default::default()
        })
    }
}
