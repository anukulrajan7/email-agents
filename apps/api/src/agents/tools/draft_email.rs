use super::*;

/// Stable short key of a draft's instructions, so retrying the same step finds the draft it already made.
fn instruction_key(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect()
}

pub(super) struct DraftEmail;

#[async_trait]
impl Tool for DraftEmail {
    fn name(&self) -> &'static str {
        "draft_email"
    }
    fn description(&self) -> &'static str {
        "Draft a reply to an email in this case's conversation. Put every fact the reply must contain (candidate names, scores, next steps) in instructions. Never put a sign-off or your own name in instructions; the mailbox owner's signature is added automatically."
    }
    fn schema(&self) -> Value {
        object(
            json!({
                "message_id": {"type": "string", "description": "The email to reply to"},
                "instructions": {"type": "string", "description": "What the reply must say"},
                "quality": {"type": "string", "enum": ["standard", "premium"]}
            }),
            &["message_id"],
        )
    }
    fn risk(&self) -> Risk {
        Risk::Low
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let msg = message_in_ws(ctx, str_arg(args, "message_id")?).await?;
        if msg.direction != "inbound" {
            return failed("Replies can only answer an email we received.");
        }
        if ctx.case.thread_id.as_deref().is_some_and(|t| t != msg.thread_id) {
            return failed("Replies can only go to this case's conversation.");
        }
        let instructions = opt_str(args, "instructions");
        // Without new instructions, reuse an unsent reply the system or an agent suggested for this
        // email. A reply a person wrote or edited is never reused or claimed by an agent.
        let reused: Option<(String, String)> = match instructions {
            Some(_) => None,
            None if ctx.case.shadow => None,
            None => sqlx::query_as(
                "SELECT id, body_text FROM drafts
                 WHERE message_id = ?1 AND workspace_id = ?2 AND status = 'suggested' AND origin_key IS NOT NULL
                   AND substr(origin_key, 1, 6) != 'watch:' AND send_at IS NULL
                 ORDER BY created_at DESC, id DESC LIMIT 1",
            )
            .bind(&msg.id)
            .bind(ctx.ws())
            .fetch_optional(&ctx.state.db)
            .await?,
        };
        let (draft_id, body) = match reused.clone() {
            Some(d) => d,
            None => {
                if ctx.case.trigger.as_deref() != Some("manual")
                    && crate::mail::system_mail::is_own_message(&ctx.state.db, ctx.ws(), &msg.id).await?
                {
                    return failed("This is Post Room's own mail; no reply is needed.");
                }
                let (agent, _) = crate::agents::registry::runtime::context_for_case(&ctx.state.db, &ctx.state.config, ctx.case).await?;
                let quality = match opt_str(args, "quality") {
                    _ if agent.agent.quality == "best" => Some(DraftQuality::Premium),
                    Some("premium") => Some(DraftQuality::Premium),
                    Some("standard") => Some(DraftQuality::Standard),
                    _ if ctx.case.kind == "requirement" => Some(DraftQuality::Premium),
                    _ => None,
                };
                let style = agent.draft_style(&ctx.state.config);
                let owner_said = ctx.case.state().get("owner_said").and_then(|o| {
                    Some((o.get("user_id")?.as_str()?.to_string(), o.get("text")?.as_str()?.to_string()))
                });
                if ctx.case.shadow {
                    let body = crate::agents::profiles::with_draft_style(style, handlers::preview_draft(ctx.state, &msg.id, quality)).await?;
                    if body.trim().is_empty() {
                        return failed("The writer judged that this email needs no reply.");
                    }
                    let gaps = replies::has_open_gaps(&body);
                    let stand_in = format!("shadow:{}:{}", ctx.case.id, ctx.turn);
                    return Ok(ToolOutput {
                        summary: format!("Observe mode: wrote a reply to {} (kept here, not saved as a draft)", msg.sender_address),
                        value: json!({"draft_id": stand_in, "body": body, "has_open_questions": gaps, "shadow": true}),
                        state: Some(json!({"last_draft_id": stand_in})),
                        ..Default::default()
                    });
                }
                let person_is_working = crate::replies::live_draft::live(&mut *ctx.state.db.acquire().await?, &msg.id).await?.is_some_and(|d| d.person_is_working());
                if person_is_working {
                    return failed("A person is already working on a reply.");
                }
                let origin = match instructions {
                    Some(text) => format!("agent:{}:{}:{}", ctx.case.id, msg.id, instruction_key(text)),
                    None => format!("agent:{}:{}", ctx.case.id, ctx.turn),
                };
                crate::agents::profiles::with_draft_style(style, handlers::create_agent_draft(ctx.state, &msg.id, instructions, quality, &origin, ctx.case.agent_id.as_deref(), owner_said.as_ref().map(|(u, t)| (u.as_str(), t.as_str()))))
                    .await?
                    .ok_or_else(|| ToolError::Failed("The writer judged that this email needs no reply.".into()))?
            }
        };
        if let Some(agent_id) = &ctx.case.agent_id {
            sqlx::query("UPDATE drafts SET agent_id = ?1 WHERE id = ?2 AND workspace_id = ?3 AND agent_id IS NULL AND status = 'suggested' AND origin_key IS NOT NULL")
                .bind(agent_id)
                .bind(&draft_id)
                .bind(ctx.ws())
                .execute(&ctx.state.db)
                .await?;
        }
        let gaps = replies::has_open_gaps(&body);
        let verb = if reused.is_some() { "Used the suggested reply to" } else { "Drafted a reply to" };
        Ok(ToolOutput {
            summary: format!("{verb} {}{}", msg.sender_address, if gaps { " (it still has open questions)" } else { "" }),
            state: Some(json!({"last_draft_id": draft_id})),
            value: json!({"draft_id": draft_id, "body": body, "has_open_questions": gaps}),
            ..Default::default()
        })
    }
}
