use super::*;

pub(super) struct ExtractFields;

#[async_trait]
impl Tool for ExtractFields {
    fn name(&self) -> &'static str {
        "extract_fields"
    }
    fn description(&self) -> &'static str {
        "Read named details (an order number, a budget, a date) out of one email. Details the email does not state come back missing. \
         fields: [{name, type: string|number|date|boolean, description}]."
    }
    fn schema(&self) -> Value {
        object(
            json!({
                "message_id": {"type": "string", "description": "The email to read"},
                "fields": {"type": "array", "items": {"type": "object"}, "description": "1 to 12 details to find"}
            }),
            &["message_id", "fields"],
        )
    }
    fn risk(&self) -> Risk {
        Risk::Read
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let msg = message_in_ws(ctx, str_arg(args, "message_id")?).await?;
        if msg.direction != "inbound" {
            return failed("Details can only be read from an email we received.");
        }
        let fields: Vec<crate::agents::registry::spec::ExtractField> = serde_json::from_value(args.get("fields").cloned().unwrap_or(Value::Null))
            .map_err(|_| ToolError::Failed("The details to find are not in the expected shape.".into()))?;
        if fields.is_empty() || fields.len() > 12 {
            return failed("Ask for 1 to 12 details.");
        }
        if !fields.iter().all(crate::agents::registry::spec::field_ok) {
            return failed("Each detail needs a lower-case name and a description of at most 200 characters.");
        }
        let extracted = ctx
            .state
            .field_extractor
            .extract(&msg.subject, &knowledge::clean_body(&msg.body_text), &fields)
            .await
            .map_err(|_| ToolError::Failed("The details could not be read right now.".into()))?;
        let found = extracted.found;
        let missing: Vec<&str> = fields.iter().map(|f| f.name.as_str()).filter(|n| !found.contains_key(*n)).collect();
        let list = |names: Vec<String>| if names.is_empty() { "nothing".to_string() } else { names.join(", ") };
        Ok(ToolOutput {
            summary: format!(
                "Read the email: found {}; missing {}",
                list(found.iter().map(|(k, v)| format!("{k} = {v}")).collect()),
                list(missing.iter().map(|m| m.to_string()).collect())
            ),
            value: json!({"found": found, "missing": missing}),
            state: Some(json!({"extracted": found, "extract_missing": missing})),
            cost_usd: extracted.cost_usd,
            ..Default::default()
        })
    }
}
