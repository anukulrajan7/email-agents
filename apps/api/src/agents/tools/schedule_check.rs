use super::*;

pub const MAX_SLEEP_HOURS: i64 = 14 * 24;

pub(super) struct ScheduleCheck;

#[async_trait]
impl Tool for ScheduleCheck {
    fn name(&self) -> &'static str {
        "schedule_check"
    }
    fn description(&self) -> &'static str {
        "Pause the case and look again after some hours (a reply in the conversation wakes it earlier)."
    }
    fn schema(&self) -> Value {
        object(json!({"hours": {"type": "integer", "description": "1 to 336"}, "reason": {"type": "string"}}), &["hours", "reason"])
    }
    fn risk(&self) -> Risk {
        Risk::Low
    }
    async fn run(&self, _ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let hours = args.get("hours").and_then(Value::as_i64).unwrap_or(0);
        if !(1..=MAX_SLEEP_HOURS).contains(&hours) {
            return failed("hours must be between 1 and 336");
        }
        let reason = sentence(str_arg(args, "reason")?, 160);
        let wake = now_ms() + hours * 3_600_000;
        Ok(ToolOutput {
            summary: format!("Will check again in {hours} hour{}: {reason}", plural(hours as usize)),
            value: json!({"wake_at": iso(wake)}),
            sleep_until: Some(wake),
            ..Default::default()
        })
    }
}
