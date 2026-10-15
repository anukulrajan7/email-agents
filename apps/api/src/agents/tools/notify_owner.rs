use super::*;

pub(super) struct NotifyOwner;

#[async_trait]
impl Tool for NotifyOwner {
    fn name(&self) -> &'static str {
        "notify_owner"
    }
    fn description(&self) -> &'static str {
        "Tell the person this assistant works for something, in their Post Room notifications (for example \"Support replied\" or \"3 emails need attention\"). \
         Set email_line to true to also email them a one-line version."
    }
    fn schema(&self) -> Value {
        object(
            json!({
                "subject": {"type": "string", "description": "A short headline"},
                "body": {"type": "string", "description": "One or two plain sentences"},
                "email_line": {"type": "boolean"}
            }),
            &["subject"],
        )
    }
    fn risk(&self) -> Risk {
        Risk::Low
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let subject = str_arg(args, "subject")?;
        if ctx.case.shadow {
            return Ok(ToolOutput {
                summary: "Observe mode: would have notified the owner; nothing was done".into(),
                value: json!({"simulated": true}),
                ..Default::default()
            });
        }
        let Some(owner) = ctx.case.agent_user_id.as_deref() else {
            return failed("This case has no owner to notify.");
        };
        let link = ctx.case.message_id.as_deref().map_or(json!({"kind": "meera"}), |m| json!({"kind": "message", "id": m}));
        let id = crate::alerts::messaging::inapp::notify_owner(
            ctx.state,
            ctx.ws(),
            owner,
            ctx.case.agent_id.as_deref(),
            subject,
            opt_str(args, "body").unwrap_or_default(),
            link,
            args.get("email_line").and_then(Value::as_bool) == Some(true),
        )
        .await?;
        Ok(ToolOutput { summary: format!("Told the owner: {}", sentence(subject, 80)), value: json!({"notified": true, "notice_id": id}), ..Default::default() })
    }
}
