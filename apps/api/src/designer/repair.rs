//! One design request: ask the model, normalise, validate, and when there are issues send back only
//! `{previous_spec, issues}` to be fixed, at most `MAX_REPAIRS` times. What is still wrong after that
//! goes to the person as editable form errors.

use super::{DesignRequest, Designer, MAX_REPAIRS, prompt, validate::{self, WsCtx}};
use crate::{
    agent_registry::spec::{AgentSpec, Issue},
    ai::AiError,
};

#[derive(Debug)]
pub struct Outcome {
    /// The last answer that could be read, normalised. `None` when no answer ever parsed.
    pub spec: Option<AgentSpec>,
    pub issues: Vec<Issue>,
    /// Repair rounds used.
    pub repairs: usize,
    /// Model calls made (first answer plus repairs).
    pub calls: usize,
    pub cost_usd: f64,
}

fn parse(raw: &str) -> Result<AgentSpec, Issue> {
    serde_json::from_str::<AgentSpec>(raw.trim()).map_err(|e| Issue {
        path: String::new(),
        code: "bad_shape",
        message: format!("the answer is not a valid spec: {}", crate::agents::sentence(&e.to_string(), 240)),
    })
}

/// Runs the design loop. `budget_usd` is what the session may still spend: no repair starts once it is used up.
/// A failure of the first call is an error; a failure of a repair call keeps what was already read.
pub async fn run(designer: &dyn Designer, ctx: &WsCtx, first: DesignRequest, budget_usd: f64) -> Result<Outcome, AiError> {
    let mut request = first.clone();
    let mut outcome = Outcome { spec: None, issues: vec![], repairs: 0, calls: 0, cost_usd: 0.0 };
    loop {
        let reply = match designer.design(&request).await {
            Ok(r) => r,
            Err(e) if outcome.calls == 0 => return Err(e),
            Err(e) => {
                tracing::warn!(error = %e, "designer repair call failed; keeping the last answer");
                return Ok(outcome);
            }
        };
        outcome.calls += 1;
        outcome.cost_usd += reply.cost_usd;
        let previous = match parse(&reply.json) {
            Ok(spec) => {
                let spec = validate::normalise(spec, ctx);
                outcome.issues = validate::validate(&spec, ctx);
                let text = serde_json::to_string_pretty(&spec).unwrap_or_default();
                outcome.spec = Some(spec);
                text
            }
            Err(issue) => {
                outcome.issues = vec![issue];
                reply.json.clone()
            }
        };
        if outcome.issues.is_empty() || outcome.repairs >= MAX_REPAIRS || outcome.cost_usd >= budget_usd {
            return Ok(outcome);
        }
        outcome.repairs += 1;
        request = DesignRequest { system: first.system.clone(), user: prompt::repair_user(&previous, &outcome.issues), schema: first.schema.clone() };
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::{
        agent_registry::templates::{INBOX_KEY, template},
        designer::{ScriptedDesigner, schema::spec_schema, validate::tests_support::ctx},
    };

    fn good() -> Value {
        let mut spec = template(INBOX_KEY).unwrap().spec();
        spec.key = "refund_helper".into();
        spec.name = "Refund helper".into();
        serde_json::to_value(spec).unwrap()
    }

    fn request() -> DesignRequest {
        DesignRequest { system: "s".into(), user: "u".into(), schema: spec_schema() }
    }

    #[tokio::test]
    async fn a_valid_answer_needs_no_repair() {
        let d = ScriptedDesigner::new().reply(good());
        let o = run(&d, &ctx(), request(), 1.0).await.unwrap();
        assert!(o.issues.is_empty() && o.repairs == 0 && o.calls == 1);
        assert_eq!(o.spec.unwrap().key, "refund_helper");
    }

    #[tokio::test]
    async fn an_invalid_answer_is_repaired_with_only_the_issues() {
        let mut bad = good();
        bad["triggers"][0]["match"]["labels"] = json!(["nope"]);
        let d = ScriptedDesigner::new().reply(bad).reply(good());
        let o = run(&d, &ctx(), request(), 1.0).await.unwrap();
        assert!(o.issues.is_empty() && o.repairs == 1 && o.calls == 2);
        let second = &d.requests()[1];
        assert!(second.user.contains("<issues>") && second.user.contains("unknown_label") && second.user.contains("<previous_spec>"));
        assert!(!second.user.contains("<conversation>"), "a repair carries the spec and the issues, nothing else");
    }

    #[tokio::test]
    async fn an_unrepairable_answer_stops_after_two_repairs() {
        let mut bad = good();
        bad["triggers"][0]["match"]["labels"] = json!(["nope"]);
        let d = ScriptedDesigner::new().reply(bad.clone()).reply(bad.clone()).reply(bad.clone()).reply(good());
        let o = run(&d, &ctx(), request(), 1.0).await.unwrap();
        assert_eq!((o.repairs, o.calls, d.calls()), (2, 3, 3), "the fourth answer is never asked for");
        assert_eq!(o.issues.iter().map(|i| i.code).collect::<Vec<_>>(), ["unknown_label"]);
        assert!(o.spec.is_some(), "the spec is kept so the person can edit it");
    }

    #[tokio::test]
    async fn a_garbled_answer_is_a_shape_issue_and_the_budget_stops_repairs() {
        let d = ScriptedDesigner::new().with_cost(0.06).reply_raw("not json at all").reply_raw("{\"name\": 3}");
        let o = run(&d, &ctx(), request(), 0.10).await.unwrap();
        assert!(o.spec.is_none() && o.issues[0].code == "bad_shape");
        assert_eq!(o.calls, 2, "after $0.12 spent no third call starts");
    }

    #[tokio::test]
    async fn a_failed_first_call_is_an_error_but_a_failed_repair_keeps_the_answer() {
        let d = ScriptedDesigner::new().fail(AiError::Permanent("down".into()));
        assert!(run(&d, &ctx(), request(), 1.0).await.is_err());
        let mut bad = good();
        bad["permissions"]["max_autonomy"] = json!(9);
        bad["triggers"][0]["match"]["labels"] = json!(["nope"]);
        let d = ScriptedDesigner::new().reply(bad).fail(AiError::Retryable("blip".into()));
        let o = run(&d, &ctx(), request(), 1.0).await.unwrap();
        assert!(o.spec.is_some() && !o.issues.is_empty());
        assert_eq!(o.spec.unwrap().permissions.max_autonomy, 2, "normalised before it was kept");
    }
}
