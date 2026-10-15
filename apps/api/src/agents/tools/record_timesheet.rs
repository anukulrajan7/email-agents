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
        let run = crate::hr::staffing::timesheet_records::record_message(ctx.state, ctx.ws(), &msg.id).await?;
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
