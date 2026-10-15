use super::*;

pub(super) struct ReadEmail;

#[async_trait]
impl Tool for ReadEmail {
    fn name(&self) -> &'static str {
        "read_email"
    }
    fn description(&self) -> &'static str {
        "Read one email: what it asks (summary, asks, deadline) and a short fenced excerpt. Pass excerpt: true for a longer one."
    }
    fn schema(&self) -> Value {
        object(json!({"message_id": {"type": "string"}, "excerpt": {"type": "boolean"}}), &["message_id"])
    }
    fn risk(&self) -> Risk {
        Risk::Read
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let msg = message_in_ws(ctx, str_arg(args, "message_id")?).await?;
        let insight = if msg.direction == "inbound" {
            Some(handlers::analyze_message(ctx.state, &json!({"message_id": msg.id})).await?)
        } else {
            None
        };
        Ok(ToolOutput {
            summary: format!("Read the email \"{}\" from {}", sentence(&msg.subject, 80), msg.sender_address),
            value: json!({
                "message_id": msg.id, "from": msg.sender_address, "subject": msg.subject, "received_at": iso(msg.received_at),
                "direction": msg.direction,
                "excerpt": fenced(&msg.body_text, if args.get("excerpt").and_then(Value::as_bool) == Some(true) { LONG_EXCERPT_WORDS } else { EXCERPT_WORDS }),
                "insight": insight,
            }),
            ..Default::default()
        })
    }
}
