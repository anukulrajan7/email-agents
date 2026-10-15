use super::*;

pub(super) struct ProposeAutomation;

impl ProposeAutomation {
    /// The suggestion as a plain-English job for the designer. Automatic sending is described as
    /// "reply for review" because a designed agent earns sending later; nothing here can switch it on.
    fn describe(input: &crate::automations::AutomationInput, reason: &str) -> String {
        use crate::automations::{Action, Condition, ContainsOp, LabelOp};
        let when: Vec<String> = input
            .conditions
            .iter()
            .map(|c| match c {
                Condition::Label { op: LabelOp::Is, value } => format!("the label is {value}"),
                Condition::Label { op: LabelOp::IsNot, value } => format!("the label is not {value}"),
                Condition::Priority { value, .. } => format!("the priority is {value:?}").to_lowercase(),
                Condition::NeedsReply { value, .. } => if *value { "it needs a reply".to_string() } else { "it needs no reply".to_string() },
                Condition::From { op: ContainsOp::Contains, value } => format!("the sender contains \"{value}\""),
                Condition::Subject { op: ContainsOp::Contains, value } => format!("the subject contains \"{value}\""),
            })
            .collect();
        let then: Vec<String> = input
            .actions
            .iter()
            .map(|a| match a {
                Action::DraftReply { instructions: Some(i) } => format!("draft a reply ({})", sentence(i, 160)),
                Action::DraftReply { instructions: None } => "draft a reply".to_string(),
                Action::SaveToGmailDrafts => "save the draft to Gmail Drafts".to_string(),
                Action::SendReply { .. } => "send the reply (here: draft it and ask a person before sending)".to_string(),
                Action::CreateReview { title: Some(t) } => format!("ask a person to review: {}", sentence(t, 100)),
                Action::CreateReview { title: None } => "ask a person to review".to_string(),
                Action::RemindIfNoReply { hours } => format!("remind a person if nobody replied after {hours} hours"),
            })
            .collect();
        let mut text = format!("An agent suggested a standing rule called \"{}\". ", sentence(input.name.trim(), 80));
        if !when.is_empty() {
            text.push_str(&format!("When {}: ", when.join(" and ")));
        }
        text.push_str(&format!("{}.", then.join(", then ")));
        if !reason.is_empty() {
            text.push_str(&format!(" Why: {reason}"));
        }
        text
    }

    fn parse(args: &Value) -> Result<crate::automations::AutomationInput, ToolError> {
        let input = json!({
            "name": args.get("name").cloned().unwrap_or(Value::Null),
            "enabled": true,
            "conditions": args.get("conditions").cloned().unwrap_or_else(|| json!([])),
            "actions": args.get("actions").cloned().unwrap_or_else(|| json!([])),
        });
        let input: crate::automations::AutomationInput = serde_json::from_value(input)
            .map_err(|e| ToolError::Failed(format!("The automation is not in the expected shape: {}", sentence(&e.to_string(), 160))))?;
        crate::automations::validate(&input.name, &input.conditions, &input.actions).map_err(ToolError::Failed)?;
        Ok(input)
    }
}

#[async_trait]
impl Tool for ProposeAutomation {
    fn name(&self) -> &'static str {
        "propose_automation"
    }
    fn description(&self) -> &'static str {
        "Suggest a standing rule for emails like this one. If an admin approves, it opens the agent designer; nothing runs until they review, dry-run and save it. \
         conditions: [{field: label, op: is|is_not, value: <label key>} | {field: priority, op: is, value: high|normal|low} | \
         {field: needs_reply, op: is, value: bool} | {field: from|subject, op: contains, value: text}]. \
         actions: [{type: draft_reply, instructions?} | {type: save_to_gmail_drafts} | {type: create_review, title?} | \
         {type: remind_if_no_reply, hours} | {type: send_reply, daily_cap, allowed_domains}]."
    }
    fn schema(&self) -> Value {
        object(
            json!({
                "name": {"type": "string", "description": "Short name, e.g. \"Acknowledge timesheets\""},
                "conditions": {"type": "array", "items": {"type": "object"}},
                "actions": {"type": "array", "items": {"type": "object"}},
                "reason": {"type": "string", "description": "One sentence: why this would help the team"}
            }),
            &["name", "actions"],
        )
    }
    fn risk(&self) -> Risk {
        // Never activates on its own: a person approves the suggestion.
        Risk::High
    }
    async fn preview(&self, _ctx: &ToolCtx<'_>, args: &Value) -> Result<Option<Preview>, ToolError> {
        let input = Self::parse(args)?;
        let reason = opt_str(args, "reason").map(|r| sentence(r, 240)).unwrap_or_default();
        Ok(Some(Preview { summary: format!("Suggests an automation: {}", sentence(input.name.trim(), 80)), body: reason, ..Default::default() }))
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let input = Self::parse(args)?;
        let Actor::User(admin_id) = ctx.actor else {
            return failed("Only an admin who approves the suggestion can open the agent designer.");
        };
        let reason = opt_str(args, "reason").map(|r| sentence(r, 240)).unwrap_or_default();
        let session = crate::agents::designer::session::open_from_proposal(ctx.state, ctx.ws(), admin_id, &input.name, &Self::describe(&input, &reason)).await?;
        audit::record(
            &mut *ctx.state.db.acquire().await?,
            Entry {
                workspace_id: ctx.ws(),
                actor: ctx.actor,
                action: "designer.session_opened",
                entity_type: "designer_session",
                entity_id: &session.id,
                message_id: None,
                metadata: json!({ "by": "agent", "case_id": ctx.case.id, "drafted": session.spec.is_some() }),
            },
        )
        .await?;
        let name = sentence(input.name.trim(), 80);
        Ok(ToolOutput {
            summary: format!("Opened the agent designer for \"{name}\". Nothing runs until an admin reviews, dry-runs and saves it"),
            value: json!({"designer_session_id": session.id, "drafted": session.spec.is_some()}),
            ..Default::default()
        })
    }
}
