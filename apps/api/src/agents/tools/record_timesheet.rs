use super::*;

pub(super) struct RecordTimesheet;

#[async_trait]
impl Tool for RecordTimesheet {
    fn name(&self) -> &'static str {
        "record_timesheet"
    }
    fn description(&self) -> &'static str {
        "Read a timesheet from an email (the text, or its attached files) and keep it as a record for the person and period. \
         A new timesheet for the same period replaces the old one. Nothing is sent."
    }
    fn schema(&self) -> Value {
        object(json!({"message_id": {"type": "string", "description": "The timesheet email"}}), &["message_id"])
    }
    fn risk(&self) -> Risk {
        Risk::Low
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let msg = message_in_ws(ctx, str_arg(args, "message_id")?).await?;
        if msg.direction != "inbound" {
            return failed("Timesheets can only be read from an email we received.");
        }
        if ctx.case.shadow {
            return Ok(ToolOutput {
                summary: "Observe mode: would have recorded the timesheet; nothing was done".into(),
                value: json!({"simulated": true}),
                ..Default::default()
            });
        }
        use crate::hr::staffing::timesheet_records as records;
        let run = records::record_message(ctx.state, ctx.ws(), &msg.id).await?;
        let desk = ctx.case.state().get("desk").and_then(Value::as_bool) == Some(true)
            && crate::platform::flags::mode(&ctx.state.db, &ctx.state.config, ctx.ws(), crate::platform::flags::Key::Handoff).await?.acts();
        if desk {
            return desk_run(ctx, &msg.id, run).await;
        }
        let clean = !run.records.is_empty() && !run.needs_attention();
        let summary = match run.records.as_slice() {
            [] if run.deferred => "The timesheet could not be read just now; it is tried again later".to_string(),
            [] if run.waiting_for_files => "The attached files are still being read; the timesheet is recorded when they are".to_string(),
            [] => "No timesheet data was found in this email".to_string(),
            records => records
                .iter()
                .map(|r| {
                    let who = r.person_name.clone().unwrap_or_else(|| "someone not yet matched to a person".into());
                    let hours = r.total_hours.map_or("no total".to_string(), |h| format!("{h} h"));
                    format!("Recorded {who}, {} to {}, {hours} ({})", r.period_start, r.period_end, r.status.replace('_', " "))
                })
                .collect::<Vec<_>>()
                .join("; "),
        };
        Ok(ToolOutput {
            summary,
            value: json!({"records": run.records, "waiting_for_files": run.waiting_for_files}),
            state: Some(json!({"timesheet_clean": clean})),
            ..Default::default()
        })
    }
}

/// How many times the desk looks again for files that are still being read, 15 minutes apart.
const MAX_WAITS: i64 = 8;
const WAIT_MS: i64 = 15 * 60_000;

/// The timesheet desk's run under the `handoff` flag: the records are checked against the owner's rules, the decision is
/// kept in the case for the sender's reply, and the manager is told once. Nothing is sent to the sender.
async fn desk_run(ctx: &ToolCtx<'_>, message_id: &str, run: crate::hr::staffing::timesheet_records::Run) -> Result<ToolOutput, ToolError> {
    use crate::hr::staffing::timesheet_records::{desk, rules::{MessageKind, Outcome}, summary};
    let waits = ctx.case.state().get("record_waits").and_then(Value::as_i64).unwrap_or(0);
    if run.records.is_empty() && (run.waiting_for_files || run.deferred) && waits < MAX_WAITS {
        return Ok(ToolOutput {
            summary: "The timesheet's files are still being read; looking again in 15 minutes".into(),
            value: json!({"waiting": true}),
            state: Some(json!({"record_waits": waits + 1})),
            sleep_until: Some(now_ms() + WAIT_MS),
            repeat: true,
            ..Default::default()
        });
    }
    let db = &ctx.state.db;
    let review = desk::review(db, ctx.ws(), message_id).await?;
    let mut review = review;
    let statuses = desk::apply(db, ctx.ws(), &mut review).await?;
    let mut conn = db.acquire().await?;
    if review.signature_gap {
        desk::record_signature_gap(&mut conn, ctx.ws(), ctx.case.agent_id.as_deref(), message_id).await?;
    }
    desk::audit_decision(&mut conn, ctx.ws(), message_id, &ctx.case.id, &review).await?;
    drop(conn);
    let told = summary::summarize(ctx.state, ctx.ws(), ctx.case.agent_id.as_deref(), &review.rows, &review.decision).await?;
    let clean = !review.rows.is_empty() && statuses.iter().all(|s| s == "recorded") && review.all_matched();
    // The manager's summary is the ask only when the manager is the one to decide (a portal question); a "recorded"
    // summary never replaces the review item for a sheet that needs a look.
    let manager_decides = told > 0 && review.decision.message.as_ref().is_some_and(|m| m.kind == MessageKind::Question);
    let verdict = match (&review.decision.outcome, review.decision.message.as_ref().map(|m| m.kind)) {
        (Outcome::NothingToDo, _) => "No timesheet data was found in this email".to_string(),
        (Outcome::NeedsPerson { .. }, _) => "Recorded. Someone needs to say who this sheet is for".to_string(),
        (_, Some(MessageKind::Ack)) => format!("Recorded {} sheet(s); they follow the rules", review.rows.len()),
        (_, Some(MessageKind::Correction)) => "Recorded, but the sheet breaks a rule and needs a corrected copy".to_string(),
        (_, _) => "Recorded, but the sheet and the portal differ".to_string(),
    };
    Ok(ToolOutput {
        summary: if told > 0 { format!("{verdict}. The manager was told") } else { verdict },
        value: json!({"records": run.records, "decision": review.decision}),
        state: Some(json!({"decision": review.decision, "record_ids": review.record_ids(), "timesheet_clean": clean, "manager_notified": told > 0, "manager_decides": manager_decides})),
        ..Default::default()
    })
}
