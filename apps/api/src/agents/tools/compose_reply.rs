use super::*;
use crate::replies::compose::{self, Composed, Meera};

pub(super) struct ComposeReply;

#[async_trait]
impl Tool for ComposeReply {
    fn name(&self) -> &'static str {
        "compose_reply"
    }
    fn description(&self) -> &'static str {
        "Write the one reply to this email from the requests the team's experts filed for it. Fixed wording from facts, no model; the reply is the one live draft of the email."
    }
    fn schema(&self) -> Value {
        object(json!({"message_id": {"type": "string", "description": "The email being answered"}}), &["message_id"])
    }
    fn risk(&self) -> Risk {
        Risk::Low
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let msg = message_in_ws(ctx, str_arg(args, "message_id")?).await?;
        if ctx.case.shadow {
            return Ok(ToolOutput { summary: "Observe mode: would have written the reply; nothing was done".into(), value: json!({"simulated": true}), ..Default::default() });
        }
        let Some(agent_id) = ctx.case.agent_id.clone() else { return failed("This case has no agent to write the reply.") };
        let name: String = sqlx::query_scalar("SELECT name FROM agents WHERE id = ?1 AND workspace_id = ?2").bind(&agent_id).bind(ctx.ws()).fetch_one(&ctx.state.db).await?;
        Ok(match compose::compose_message(ctx.state, ctx.ws(), &msg.id, &Meera { id: agent_id, name }).await? {
            Composed::Written { draft_id, line, askers, level, kind } => ToolOutput {
                summary: format!("Wrote the reply {} asked for ({}) to {}", askers.first().map_or("the team".to_string(), |a| a.name.clone()), kind.as_str(), msg.sender_address),
                value: json!({"draft_id": draft_id, "kind": kind, "governing_level": level}),
                state: Some(json!({
                    "last_draft_id": draft_id,
                    "governing_level": level,
                    "handoff": {"line": line, "from": askers.iter().map(|a| json!({"agent_id": a.agent_id, "name": a.name, "kind": a.kind, "job": a.job})).collect::<Vec<_>>()},
                })),
                ..Default::default()
            },
            Composed::Existing { draft_id } => ToolOutput {
                summary: "A reply draft of this team already stands for this email; it was kept".into(),
                value: json!({"draft_id": draft_id}),
                state: Some(json!({"last_draft_id": draft_id})),
                ..Default::default()
            },
            // Another writer's draft is never sent by this job: the reply job ends and the request waits for a person.
            Composed::Foreign => ToolOutput { summary: "Another draft already stands for this email; the team's request waits and nothing was sent".into(), value: json!({"nothing": true}), ..Default::default() },
            Composed::Sent => ToolOutput { summary: "This email was already answered; nothing more to write".into(), value: json!({"nothing": true}), ..Default::default() },
            Composed::Nothing(why) => ToolOutput { summary: format!("Nothing to write: {why}"), value: json!({"nothing": true}), ..Default::default() },
        })
    }
}
