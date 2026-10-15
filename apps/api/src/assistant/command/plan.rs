use super::*;

impl Session<'_> {
    async fn add_plan(&mut self, action: &str, summary: String, params: Value) -> ToolResult {
        if let Some(existing) = self.plan.iter().find(|p| p.action == action && p.params == params) {
            return Ok(json!({ "plan_item_id": existing.id, "status": "already prepared" }));
        }
        if self.plan.len() >= MAX_PLAN_ITEMS {
            return Err(failed("too many actions in one request; ask for fewer"));
        }
        let id = new_id("apl");
        let now = now_ms();
        sqlx::query("DELETE FROM assistant_plans WHERE expires_at < ?1 AND used_at IS NULL").bind(now - 86_400_000).execute(&self.state.db).await?;
        sqlx::query(
            "INSERT INTO assistant_plans (id, workspace_id, user_id, action, summary, params_json, expires_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(&id)
        .bind(self.ws)
        .bind(&self.user.id)
        .bind(action)
        .bind(&summary)
        .bind(params.to_string())
        .bind(now + PLAN_TTL_MS)
        .bind(now)
        .execute(&self.state.db)
        .await?;
        self.plan.push(PlanCard { id: id.clone(), action: action.into(), summary, params });
        Ok(json!({ "plan_item_id": id, "status": "waiting for the person to confirm; nothing has been done yet" }))
    }

    pub(super) async fn draft_reply(&mut self, args: &Value) -> ToolResult {
        let id = text_arg(args, "message_id").ok_or_else(|| failed("message_id is required"))?;
        let row = self.mail_by_id(id).await?.ok_or_else(|| failed("no email with that id"))?;
        let instructions = text_arg(args, "instructions").map(|i| truncate_chars(i, MAX_INSTRUCTION_CHARS));
        let with = if instructions.is_some() { " with your instructions" } else { "" };
        let summary = format!("Draft a reply to “{}”{with}", sentence(&row.subject, 80));
        self.add_plan("draft_reply", summary, json!({ "message_id": row.id, "instructions": instructions })).await
    }

    pub(super) async fn start_agent(&mut self, args: &Value) -> ToolResult {
        if !self.state.planner.enabled() {
            return Err(failed("agents are not set up yet"));
        }
        match text_arg(args, "kind") {
            Some("requirement") if !self.features.requirement_agent => Err(failed("requirements are not part of this workspace")),
            Some("requirement") => {
                let id = text_arg(args, "requirement_id").ok_or_else(|| failed("requirement_id is required"))?;
                let found: Option<(String, Option<String>)> =
                    sqlx::query_as("SELECT role_title, client FROM requirements WHERE id = ?1 AND workspace_id = ?2")
                        .bind(id)
                        .bind(self.ws)
                        .fetch_optional(&self.state.db)
                        .await?;
                let (role, client) = found.ok_or_else(|| failed("no requirement with that id"))?;
                let name = match client.as_deref().filter(|c| !c.trim().is_empty()) {
                    Some(c) => format!("{role} at {c}"),
                    None => role,
                };
                let summary = format!("Start an agent on the requirement “{}”", sentence(&name, 80));
                self.add_plan("start_agent", summary, json!({ "kind": "requirement", "requirement_id": id })).await
            }
            Some("inbox") => {
                let id = text_arg(args, "message_id").ok_or_else(|| failed("message_id is required"))?;
                let row = self.mail_by_id(id).await?.filter(|m| m.direction == "inbound").ok_or_else(|| failed("no inbound email with that id"))?;
                if let Some(why) = crate::agents::triggers::personal_refusal(self.state, self.ws, &row.id, &self.user.id).await? {
                    return Err(failed(why));
                }
                let summary = format!("Start an agent on “{}”", sentence(&row.subject, 80));
                self.add_plan("start_agent", summary, json!({ "kind": "inbox", "message_id": row.id })).await
            }
            _ => Err(failed("kind must be requirement or inbox")),
        }
    }

    pub(super) async fn approval_card(&mut self, args: &Value, approve: bool) -> ToolResult {
        let id = text_arg(args, "approval_id").ok_or_else(|| failed("approval_id is required"))?;
        let approval = approvals::get(&self.state.db, self.ws, &self.user.id, id).await?.ok_or_else(|| failed("no approval with that id"))?;
        if approval.status != "pending" {
            return Err(failed("that approval was already decided"));
        }
        let verb = if approve { "Approve" } else { "Decline" };
        let summary = format!("{verb}: {}", sentence(&approval.summary, 140));
        let note = text_arg(args, "note").map(|n| truncate_chars(n, 300));
        let params = if approve { json!({ "approval_id": approval.id }) } else { json!({ "approval_id": approval.id, "note": note }) };
        self.add_plan(if approve { "approve" } else { "decline" }, summary, params).await
    }

    pub(super) async fn create_automation(&mut self, args: &Value) -> ToolResult {
        if self.role != Role::Admin {
            return Err(failed("only admins can create automations"));
        }
        let input: AutomationInput = serde_json::from_value(args.clone()).map_err(|e| failed(format!("invalid automation: {e}")))?;
        if input.actions.iter().any(|a| matches!(a, Action::SendReply { .. })) {
            return Err(failed("automatic sending cannot be set up from the assistant; it is done on the Automations page"));
        }
        automations::validate(&input.name, &input.conditions, &input.actions).map_err(|e| failed(format!("invalid automation: {e}")))?;
        let steps: Vec<String> = input.actions.iter().map(|a| a.type_name().replace('_', " ")).collect();
        let summary = format!("Create the automation “{}”: {}", sentence(&input.name, 60), steps.join(", then "));
        let params = json!({ "name": input.name.trim(), "enabled": input.enabled, "conditions": input.conditions, "actions": input.actions });
        self.add_plan("create_automation", summary, params).await
    }

    pub(super) async fn send_message(&mut self, args: &Value) -> ToolResult {
        let channel = match text_arg(args, "channel") {
            Some("email") => "email",
            Some("in_app") => "in_app",
            _ => return Err(failed("channel must be email or in_app")),
        };
        let to = text_arg(args, "to").ok_or_else(|| failed("to is required"))?;
        let subject = text_arg(args, "subject").ok_or_else(|| failed("subject is required"))?;
        let body = text_arg(args, "body").ok_or_else(|| failed("body is required"))?;
        let body = truncate_chars(body, crate::alerts::messaging::inapp::MAX_BODY_CHARS);
        let target = messaging::recipients::resolve(&self.state.db, self.ws, to).await.map_err(failed)?;
        let team = matches!(target, messaging::recipients::Target::Team { .. });
        let channel = if team { "in_app" } else { channel };
        if channel == "email" {
            let person = target.people().into_iter().next().ok_or_else(|| failed("there is nobody to send it to"))?;
            if person.address.is_none() {
                return Err(failed(format!("I do not have an email address for {}", person.label)));
            }
            messaging::email::mailbox_for(self.state, self.ws, Some(&self.user.id)).await.map_err(|e| failed(e.to_string()))?;
            let report = messaging::email::check(person.address.as_deref().unwrap_or_default(), subject, &body, std::slice::from_ref(&self.user.email));
            if let Some(first) = report.blocking.first() {
                return Err(failed(first.message.clone()));
            }
        } else if target.people().iter().any(|p| p.user_id.is_none()) {
            return Err(failed("in-app messages go to workspace members; use channel email for people outside the workspace"));
        }
        let ask = args.get("ask").and_then(Value::as_bool) == Some(true);
        let wait_hours = args.get("wait_hours").and_then(Value::as_i64).filter(|h| (1..=messaging::watch::MAX_NUDGE_HOURS).contains(h)).unwrap_or(messaging::watch::DEFAULT_NUDGE_HOURS);
        let people = target.people();
        let outside = people.iter().any(|p| !p.kind.is_internal());
        let audience = if outside { "outside your workspace: you decide" } else { "inside your workspace" };
        let how = if channel == "email" { "Email" } else { "Message" };
        let address = people.first().and_then(|p| p.address.clone()).filter(|_| channel == "email");
        let summary = format!(
            "{how} {}{}: “{}”{}",
            target.label(),
            address.as_deref().map(|a| format!(" ({a})")).unwrap_or_default(),
            sentence(subject, 80),
            if ask { ", then bring back the answer" } else { "" }
        );
        let params = json!({
            "channel": channel, "to": to, "subject": subject, "body": body, "ask": ask, "wait_hours": wait_hours,
            "pinned": messaging::recipients::pin(&target),
            "preview": {
                "to": target.label(), "address": address, "audience": audience, "outside": outside, "subject": subject, "body": body,
                "source": people.first().map(|p| messaging::recipients::source_words(to, p)),
            },
        });
        let sent_to = address.clone();
        let added = self.add_plan("send_message", summary, params).await?;
        if let Some(address) = sent_to {
            let now = now_ms();
            sqlx::query(
                "UPDATE assistant_plans SET expires_at = ?1 WHERE workspace_id = ?2 AND user_id = ?3 AND action = 'draft_reply'
                   AND used_at IS NULL AND expires_at > ?1
                   AND json_extract(params_json, '$.message_id') IN
                       (SELECT id FROM email_messages WHERE workspace_id = ?2 AND lower(sender_address) = lower(?4))",
            )
            .bind(now)
            .bind(self.ws)
            .bind(&self.user.id)
            .bind(address)
            .execute(&self.state.db)
            .await?;
        }
        Ok(added)
    }

    pub(super) async fn contact_support(&mut self, args: &Value) -> ToolResult {
        let id = text_arg(args, "message_id").ok_or_else(|| failed("message_id is required"))?;
        let subject = text_arg(args, "subject").ok_or_else(|| failed("subject is required"))?;
        let body = truncate_chars(text_arg(args, "body").ok_or_else(|| failed("body is required"))?, messaging::inapp::MAX_BODY_CHARS);
        let row = self.mail_by_id(id).await?.ok_or_else(|| failed("no email with that id"))?;
        let found = messaging::support::find_address(&self.state.db, self.ws, &row.id).await?;
        let given = text_arg(args, "support_address").map(crate::replies::send_safety::bare_address);
        let (address, source, company) = match (given, found) {
            (Some(a), found) => {
                if !crate::replies::send_safety::valid_address(&a) {
                    return Err(failed("that is not a valid email address"));
                }
                let from_person = self.request.to_lowercase().contains(&a);
                let from_email = found.as_ref().is_some_and(|f| f.address == a);
                if !from_person && !from_email {
                    return Err(failed("I can only write to an address the person gave me or one that is in the email. Ask them which address to use"));
                }
                let company = text_arg(args, "company").map(str::to_string).or_else(|| found.map(|f| f.company)).unwrap_or_else(|| a.rsplit('@').next().unwrap_or(&a).to_string());
                (a, if from_person { "the address you gave me" } else { "written in that email" }, company)
            }
            (None, Some(f)) => (f.address, f.source, text_arg(args, "company").map(str::to_string).unwrap_or(f.company)),
            (None, None) => {
                return Err(failed("there is no support address in that email or the person's contacts; ask the person for the address, do not guess one"));
            }
        };
        messaging::email::mailbox_for(self.state, self.ws, Some(&self.user.id)).await.map_err(|e| failed(e.to_string()))?;
        let report = messaging::email::check(&address, subject, &body, std::slice::from_ref(&self.user.email));
        if let Some(first) = report.blocking.first() {
            return Err(failed(first.message.clone()));
        }
        self.see(&row.id);
        let wait_hours = args.get("wait_hours").and_then(Value::as_i64).filter(|h| (1..=messaging::watch::MAX_NUDGE_HOURS).contains(h)).unwrap_or(messaging::watch::DEFAULT_NUDGE_HOURS);
        let summary = format!("Write to {company} support ({address}, {source}): “{}”", sentence(subject, 80));
        let params = json!({
            "message_id": row.id, "company": company, "address": address, "address_source": source, "subject": subject, "body": body, "wait_hours": wait_hours,
            "preview": { "to": format!("{company} support"), "address": address, "audience": "outside your workspace", "outside": true, "subject": subject, "body": body, "source": source },
        });
        self.add_plan("contact_support", summary, params).await
    }

    pub(super) async fn hand_to_desk(&mut self, args: &Value) -> ToolResult {
        let id = text_arg(args, "message_id").ok_or_else(|| failed("message_id is required"))?;
        let row = self.mail_by_id(id).await?.filter(|m| m.direction == "inbound").ok_or_else(|| failed("no inbound email with that id"))?;
        if let Some(why) = crate::agents::triggers::personal_refusal(self.state, self.ws, &row.id, &self.user.id).await? {
            return Err(failed(why));
        }
        let note = text_arg(args, "note").map(|n| truncate_chars(n, MAX_INSTRUCTION_CHARS)).unwrap_or_default();
        let summary = format!("Hand “{}” to the team desk and tell you what happens", sentence(&row.subject, 80));
        self.add_plan("hand_to_desk", summary, json!({ "message_id": row.id, "note": note })).await
    }

    pub(super) async fn snooze_alert(&mut self, args: &Value) -> ToolResult {
        let id = text_arg(args, "alert_id").ok_or_else(|| failed("alert_id is required"))?;
        let hours = args.get("hours").and_then(Value::as_i64).filter(|h| (1..=168).contains(h)).ok_or_else(|| failed("hours must be 1 to 168"))?;
        let found: Option<(String, String)> = sqlx::query_as("SELECT title, status FROM alerts WHERE id = ?1 AND workspace_id = ?2")
            .bind(id)
            .bind(self.ws)
            .fetch_optional(&self.state.db)
            .await?;
        let (title, status) = found.ok_or_else(|| failed("no alert with that id"))?;
        if status == "resolved" {
            return Err(failed("that alert is already resolved"));
        }
        let summary = format!("Snooze the alert “{}” for {hours} hours", sentence(&title, 80));
        self.add_plan("snooze_alert", summary, json!({ "alert_id": id, "hours": hours })).await
    }
}
