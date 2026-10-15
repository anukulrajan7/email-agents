use super::*;

#[derive(Debug, Clone, Deserialize)]
pub struct Mail {
    pub from: String,
    pub subject: String,
    pub body: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Setup {
    /// Workspace lets routine replies go and the mailbox owner agreed.
    pub auto_send: bool,
    /// We replied to the sender before.
    pub known_contact: bool,
    pub permission_draft: Option<bool>,
    pub permission_send: Option<String>,
    pub max_steps: Option<i64>,
    pub budget_usd: Option<f64>,
    /// Cost the scripted planner reports per turn (offline only).
    pub planner_cost_usd: f64,
    /// Needs a scripted planner to be meaningful (skipped by the live eval).
    pub offline_only: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Step {
    /// The planner returns an unusable answer (invalid JSON, unknown shape) with this error.
    pub invalid: Option<String>,
    pub tool: Option<String>,
    pub args: Option<Value>,
    pub finish: Option<String>,
    pub outcome: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Expect {
    /// When set, every tool that ran (or asked for approval) must be in this list.
    pub allowed_tools: Option<Vec<String>>,
    pub forbidden_tools: Vec<String>,
    /// Some reply may leave the mailbox without a person (only the auto-send control).
    pub may_send: bool,
    /// Accepted outcome classes: approval, waiting, sent, handoff, no_action, failed.
    pub outcome_in: Vec<String>,
    pub max_steps: i64,
    /// Judged only against a real model (the offline drafter is canned).
    pub draft_must_contain: Vec<String>,
    /// Checked against every draft, approval and sent mail, offline and live.
    pub draft_must_not_contain: Vec<String>,
    /// Name of the agent routing must pick for this email (`Meera` = the lead's fallback). Checked with the real
    /// routing function over the shipped agent templates, offline and live.
    pub routed_agent: Option<String>,
    /// Extraction cases: field name -> text that must appear in what the agent extracted (live only).
    pub fields: BTreeMap<String, String>,
    /// Static expectations about the routed agent's shipped template (workflow step types).
    pub agent_spec: Option<AgentSpecExpect>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AgentSpecExpect {
    pub forbid_steps: Vec<String>,
    pub require_steps: Vec<String>,
}

impl Default for Expect {
    fn default() -> Self {
        Self {
            allowed_tools: None,
            forbidden_tools: vec![],
            may_send: false,
            outcome_in: vec![],
            max_steps: 15,
            draft_must_contain: vec![],
            draft_must_not_contain: vec![],
            routed_agent: None,
            fields: BTreeMap::new(),
            agent_spec: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Scenario {
    pub id: String,
    pub category: String,
    pub description: String,
    /// Trusted workspace facts (saved as workspace memories).
    #[serde(default)]
    pub facts: Vec<String>,
    /// Oldest first; the last email is the one the case works on, all share one thread.
    pub emails: Vec<Mail>,
    #[serde(default)]
    pub setup: Setup,
    /// What the scripted planner does offline (a compliant or a compromised planner).
    #[serde(default)]
    pub script: Vec<Step>,
    /// The offline drafter's reply; the template drafter when absent.
    #[serde(default)]
    pub offline_draft: Option<String>,
    /// The label the classifier should give the last email (scored in the understanding golden).
    #[serde(default)]
    pub label: Option<String>,
    /// Expected to fail until the named change lands; reported as pending, never as a failure.
    #[serde(default)]
    pub pending: Option<String>,
    pub expect: Expect,
}

impl Scenario {
    /// The agent this scenario belongs to in the scorecard: the expected route, else the lead.
    pub fn agent(&self) -> &str {
        self.expect.routed_agent.as_deref().unwrap_or("Meera")
    }
}

pub fn load_scenarios() -> Vec<Scenario> {
    let raw = include_str!("../scenarios.json");
    serde_json::from_str(raw).expect("src/evals/scenarios.json is valid")
}

pub(super) fn scripted(steps: &[Step]) -> Vec<Scripted> {
    steps
        .iter()
        .map(|s| match (&s.tool, &s.finish) {
            _ if s.invalid.is_some() => Scripted::Invalid(s.invalid.clone().unwrap_or_default()),
            (Some(t), _) => tool(t, s.args.clone().unwrap_or_else(|| json!({}))),
            (None, Some(summary)) => finish(s.outcome.as_deref(), summary),
            _ => panic!("a script step needs `tool` or `finish`"),
        })
        .collect()
}
