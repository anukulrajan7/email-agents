use super::*;

pub(super) struct ReadThread;

#[async_trait]
impl Tool for ReadThread {
    fn name(&self) -> &'static str {
        "read_thread"
    }
    fn description(&self) -> &'static str {
        "Read the whole conversation an email belongs to, oldest first, including replies we sent."
    }
    fn schema(&self) -> Value {
        object(json!({"message_id": {"type": "string"}}), &["message_id"])
    }
    fn risk(&self) -> Risk {
        Risk::Read
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let msg = message_in_ws(ctx, str_arg(args, "message_id")?).await?;
        let rows: Vec<MessageRow> = sqlx::query_as(
            "SELECT id, subject, sender_address, body_text, received_at, direction, thread_id
             FROM email_messages WHERE thread_id = ?1 AND workspace_id = ?2 AND suppressed = 0 ORDER BY received_at, id LIMIT 30",
        )
        .bind(&msg.thread_id)
        .bind(ctx.ws())
        .fetch_all(&ctx.state.db)
        .await?;
        let sent: Vec<(String, String, i64)> = sqlx::query_as(
            "SELECT d.id, d.body_text, d.updated_at FROM drafts d JOIN email_messages m ON m.id = d.message_id
             WHERE m.thread_id = ?1 AND d.workspace_id = ?2 AND d.status = 'sent' ORDER BY d.updated_at LIMIT 20",
        )
        .bind(&msg.thread_id)
        .bind(ctx.ws())
        .fetch_all(&ctx.state.db)
        .await?;
        let mut entries: Vec<(i64, Value)> = rows
            .iter()
            .map(|m| {
                (m.received_at, json!({
                    "message_id": m.id, "from": m.sender_address, "direction": m.direction, "subject": m.subject,
                    "received_at": iso(m.received_at),
                    "excerpt": fenced(&m.body_text, THREAD_EXCERPT_WORDS),
                }))
            })
            .collect();
        entries.extend(sent.into_iter().map(|(id, body, at)| {
            (at, json!({"draft_id": id, "from": "us", "direction": "sent_reply", "received_at": iso(at),
                        "excerpt": fenced(&body, THREAD_EXCERPT_WORDS)}))
        }));
        entries.sort_by_key(|(at, _)| *at);
        let inbound = rows.iter().filter(|m| m.direction == "inbound").count();
        Ok(ToolOutput {
            summary: format!("Read the conversation ({} email{}, {} from the other side)", entries.len(), plural(entries.len()), inbound),
            value: json!({"thread_id": msg.thread_id, "messages": entries.into_iter().map(|(_, v)| v).collect::<Vec<_>>()}),
            ..Default::default()
        })
    }
}
