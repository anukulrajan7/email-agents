use super::super::{
    case::{Case, StepRow},
    critic::RuleContext,
};
use crate::http::AppState;

/// Steps whose full input/output is shown to the planner; older ones appear as their summary only.
const DETAILED_STEPS: usize = 8;
const OUTPUT_CHARS: usize = 900;

fn clip(v: &str, max: usize) -> String {
    crate::mail::sources::normalize::truncate_chars(v, max)
}

/// The planner's view of the case, ordered for prompt caching: what never changes first (the
/// case header), then the append-only step list, then what changes every turn (the latest
/// detailed results, the case facts) and the turn and budget line last.
pub fn render_prompt(case: &Case, steps: &[StepRow], turn: i64) -> String {
    let mut out = format!(
        "Case: {}\nKind: {}\nGoal: {}\nRequirement id: {}\nCase email id: {}\n\nSteps so far (oldest first):\n",
        case.title,
        case.kind,
        case.goal,
        case.requirement_id.as_deref().unwrap_or("none"),
        case.message_id.as_deref().unwrap_or("none"),
    );
    if steps.is_empty() {
        out.push_str("(none yet)\n");
    }
    for s in steps {
        let tool = s.tool.as_deref().map(|t| format!(" {t}")).unwrap_or_default();
        out.push_str(&format!("#{} [{}{tool}] {}: {}\n", s.seq, s.kind, s.status, s.summary));
    }
    let detailed_from = steps.len().saturating_sub(DETAILED_STEPS);
    let mut latest = String::new();
    for s in &steps[detailed_from..] {
        if s.input_json != "null" {
            latest.push_str(&format!("#{} input: {}\n", s.seq, clip(&s.input_json, 400)));
        }
        if s.output_json != "null" {
            latest.push_str(&format!("#{} result: {}\n", s.seq, clip(&s.output_json, OUTPUT_CHARS)));
        }
    }
    if !latest.is_empty() {
        out.push_str("\nLatest details:\n");
        out.push_str(&latest);
    }
    out.push_str(&format!("\nCase facts: {}\n", clip(&case.state_json, 2_000)));
    out.push_str(&format!(
        "\nThis is turn {turn} of at most {}. Budget left: ${:.3}.\nCall the single best next tool, or finish.",
        case.max_steps,
        (case.budget_usd - case.spent_usd).max(0.0)
    ));
    out
}

pub async fn rule_context(state: &AppState, case: &Case) -> sqlx::Result<RuleContext> {
    let allowed_recipients: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT lower(sender_address) FROM email_messages
         WHERE workspace_id = ?1 AND direction = 'inbound'
           AND (thread_id = ?2 OR (?2 IS NULL AND id = ?3))",
    )
    .bind(&case.workspace_id)
    .bind(&case.thread_id)
    .bind(&case.message_id)
    .fetch_all(&state.db)
    .await?;
    let declined_draft_ids: Vec<String> =
        sqlx::query_scalar("SELECT draft_id FROM agent_approvals WHERE case_id = ?1 AND status = 'declined' AND draft_id IS NOT NULL")
            .bind(&case.id)
            .fetch_all(&state.db)
            .await?;
    Ok(RuleContext { case_thread_id: case.thread_id.clone(), allowed_recipients, declined_draft_ids })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case() -> Case {
        Case {
            id: "case_1".into(),
            workspace_id: "ws".into(),
            kind: "requirement".into(),
            title: "Java Developer at Capgemini".into(),
            goal: "Shortlist up to 3 candidates".into(),
            status: "running".into(),
            outcome: None,
            status_note: None,
            failure_reason: None,
            state_json: r#"{"client":"Capgemini"}"#.into(),
            priority: "normal".into(),
            step_count: 2,
            max_steps: 15,
            spent_usd: 0.01,
            budget_usd: 0.10,
            error_streak: 0,
            message_id: Some("msg_1".into()),
            requirement_id: Some("req_1".into()),
            thread_id: Some("thr_1".into()),
            next_wake_at: None,
            agent_user_id: None,
            agent_id: None,
            agent_version: None,
            trigger: None,
            outcome_detail: None,
            shadow: false,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn steps_row(i: i64) -> StepRow {
        StepRow {
            seq: i,
            turn: i,
            kind: "tool".into(),
            tool: Some("search_mail".into()),
            summary: format!("step {i}"),
            input_json: r#"{"query":"java"}"#.into(),
            output_json: format!(r#"{{"marker":"out{i}"}}"#),
            status: "ok".into(),
            cost_usd: 0.0,
            created_at: 0,
        }
    }

    #[test]
    fn prompt_shows_goal_facts_and_recent_detail() {
        let steps: Vec<StepRow> = (1..=10).map(steps_row).collect();
        let p = render_prompt(&case(), &steps, 11);
        assert!(p.contains("Goal: Shortlist up to 3 candidates") && p.contains("Requirement id: req_1") && p.contains("Capgemini"));
        assert!(p.contains("#1 [tool search_mail] ok: step 1"));
        assert!(!p.contains("out1\"") && !p.contains("out2\""), "old steps are summarised only");
        assert!(p.contains("out3") && p.contains("out10"));
        assert!(p.contains("turn 11 of at most 15") && p.contains("Budget left: $0.090"));
        let (header, steps_at, facts_at, turn_at) = (p.find("Case: Java").unwrap(), p.find("Steps so far").unwrap(), p.find("Case facts").unwrap(), p.find("This is turn").unwrap());
        assert!(header < steps_at && steps_at < facts_at && facts_at < turn_at, "stable parts first, volatile last");
        let later = render_prompt(&case(), &(1..=11).map(|i| StepRow { seq: i, ..steps_row(i) }).collect::<Vec<_>>(), 12);
        let prefix = &p[..p.find("\nLatest details").unwrap()];
        assert!(later.starts_with(prefix), "the header and earlier steps do not change when a step is added");
    }
}
