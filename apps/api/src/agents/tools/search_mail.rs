use super::*;

pub(super) struct SearchMail;

#[async_trait]
impl Tool for SearchMail {
    fn name(&self) -> &'static str {
        "search_mail"
    }
    fn description(&self) -> &'static str {
        "Search this workspace's emails and attachments by meaning and keywords."
    }
    fn schema(&self) -> Value {
        object(json!({"query": {"type": "string"}, "limit": {"type": "integer", "description": "1 to 10, default 5"}}), &["query"])
    }
    fn risk(&self) -> Risk {
        Risk::Read
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let query = str_arg(args, "query")?;
        let hits = knowledge::hybrid_search(
            &ctx.state.db,
            ctx.state.embedder.as_ref(),
            ctx.ws(),
            query,
            knowledge::Scope::Documents,
            limit_arg(args, 5, 10) as usize,
        )
        .await?;
        let items: Vec<Value> = hits
            .iter()
            .map(|h| json!({"message_id": h.message_id, "kind": h.kind, "snippet": fenced(&knowledge::snippet(knowledge::chunk_body(&h.text), query, 240), THREAD_EXCERPT_WORDS)}))
            .collect();
        Ok(ToolOutput {
            summary: format!("Searched mail for \"{}\" and found {} result{}", sentence(query, 60), items.len(), plural(items.len())),
            value: json!({"items": items}),
            ..Default::default()
        })
    }
}
