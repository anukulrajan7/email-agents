use super::*;

pub(super) struct RecallMemory;

#[async_trait]
impl Tool for RecallMemory {
    fn name(&self) -> &'static str {
        "recall_memory"
    }
    fn description(&self) -> &'static str {
        "What the team knows about a contact (email address) or a topic/client name."
    }
    fn schema(&self) -> Value {
        object(json!({"about": {"type": "string", "description": "An email address, client or topic"}}), &["about"])
    }
    fn risk(&self) -> Risk {
        Risk::Read
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let about = str_arg(args, "about")?.to_lowercase();
        let agent = {
            let mut conn = ctx.state.db.acquire().await?;
            crate::learning::scope::resolve(&mut conn, ctx.ws(), ctx.case.agent_id.as_deref(), None).await?
        };
        let view = agent.agent_id.clone().map(|agent_id| memory::AgentMemory { agent_id, scope: agent.memory });
        let (sender, topic) = if about.contains('@') { (about.as_str(), "") } else { ("", about.as_str()) };
        let mem = memory::context_for_agent(&ctx.state.db, ctx.ws(), sender, topic, false, view.as_ref()).await?;
        let notes = mem.lines();
        Ok(ToolOutput {
            summary: format!("Recalled {} note{} about {}", notes.len(), plural(notes.len()), sentence(&about, 60)),
            value: json!({"notes": notes, "history": mem.sender_history}),
            ..Default::default()
        })
    }
}
