//! What code checks before a designed spec reaches a person, on top of `spec::validate_basic`:
//! the workspace's labels, packs and agents, the autonomy ceiling, tools for every step, and text
//! that asks for things no agent may do. `normalise` first removes what is not allowed; it only
//! ever tightens, never adds a tool, a scope, a step or a level.

use std::sync::LazyLock;

use regex::Regex;

use crate::agent_registry::{
    runtime::MATCHING_TOOLS,
    spec::{self, AgentSpec, CATEGORIES, Issue, MAX_NAME_CHARS, SendMode, Step, Trigger},
};

/// The highest level a designed (custom) agent may reach.
pub const CUSTOM_CEILING: u8 = 2;
/// Most an agent may spend on one case, unless the workspace sets a higher AI budget per case.
pub const MAX_CASE_BUDGET_USD: f64 = 0.50;

#[derive(Debug, Clone)]
pub struct LabelBrief {
    pub key: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct AgentBrief {
    pub key: String,
    pub name: String,
    pub category: String,
    pub triggers: String,
}

/// Everything about the workspace that validation and the designer prompt need.
#[derive(Debug, Clone, Default)]
pub struct WsCtx {
    /// Pack keys, e.g. `general`, `staffing`.
    pub packs: Vec<String>,
    pub labels: Vec<LabelBrief>,
    pub agents: Vec<AgentBrief>,
    /// The HR & recruiting pack's candidate matching is on.
    pub matching: bool,
    pub ceiling: u8,
    pub max_case_budget_usd: f64,
    /// The spec being edited, when the session edits an existing agent.
    pub editing: Option<AgentSpec>,
}

fn category_for_pack(pack: &str) -> &str {
    if pack == crate::packs::STAFFING { "hrms" } else { pack }
}

impl WsCtx {
    fn category_allowed(&self, category: &str) -> bool {
        matches!(category, "custom" | "general") || self.packs.iter().any(|p| category_for_pack(p) == category)
    }

    fn has_label(&self, key: &str) -> bool {
        self.labels.iter().any(|l| l.key == key)
    }
}

pub fn trigger_summary(spec: &AgentSpec) -> String {
    let parts: Vec<String> = spec
        .triggers
        .iter()
        .map(|t| match t {
            Trigger::EmailReceived { matcher } => {
                let mut bits: Vec<String> = Vec::new();
                if !matcher.labels.is_empty() {
                    bits.push(format!("labels {}", matcher.labels.join("/")));
                }
                if !matcher.from_domains.is_empty() {
                    bits.push(format!("from {}", matcher.from_domains.join("/")));
                }
                if !matcher.from_addresses.is_empty() {
                    bits.push(format!("from {}", matcher.from_addresses.join("/")));
                }
                if !matcher.subject_keywords.is_empty() {
                    bits.push(format!("subject has {}", matcher.subject_keywords.join("/")));
                }
                if bits.is_empty() { "any email that needs a reply".to_string() } else { bits.join(", ") }
            }
            Trigger::Schedule { every_hours } => format!("every {every_hours}h"),
            Trigger::Manual => "started by hand".to_string(),
        })
        .collect();
    parts.join("; ")
}

static NEGATED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^\s*(never|do not|don't|dont|avoid|no|not)\b").expect("valid regex"));
static FORBIDDEN: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        (r"(?i)\b(forward(ing|ed)?|bcc|cc)\b", "forwards mail or copies other people in"),
        (r"(?i)\b(new|unknown|other|additional|extra|more) recipients?\b|\badd (the )?recipients?\b", "writes to people who are not already in the conversation"),
        (r"(?i)\b(send|attach|include)\b[^.]{0,20}\battach(ment)?s?\b|\battach (the |any |a )?(file|document|invoice|pdf|report)s?\b", "sends attachments out"),
        (r"(?i)https?://|\b(include|add|share|send|paste)\b[^.]{0,12}\blinks?\b", "puts links in replies"),
        (
            r"(?i)\bwithout (asking|approval|review|checking|confirm\w*|a person)\b|\bskip(ping)? (the )?(approval|review)\b|\bauto[- ]?(send|approve)\b|\b(no|ignore) (the )?approval\b|\bdon'?t ask\b",
            "weakens the approval step",
        ),
        (
            r"(?i)\b(approve|authori[sz]e|issue|process|grant|pay|transfer|wire|release)\b[^.]{0,30}\b(refunds?|payments?|money|funds|invoices?|discounts?|credits?)\b|\bpromise\b[^.]{0,30}\b(refunds?|discounts?|payments?|money)\b",
            "decides about money",
        ),
    ]
    .into_iter()
    .map(|(re, why)| (Regex::new(re).expect("valid regex"), why))
    .collect()
});

/// Why this text asks for something an agent may never do, if it does. A rule that forbids it ("Never forward…") is fine.
pub fn forbidden_reason(text: &str) -> Option<&'static str> {
    if NEGATED.is_match(text) {
        return None;
    }
    FORBIDDEN.iter().find(|(re, _)| re.is_match(text)).map(|(_, why)| *why)
}

fn issue(path: impl Into<String>, code: &'static str, message: impl Into<String>) -> Issue {
    Issue { path: path.into(), code, message: message.into() }
}

/// Every problem with this spec in this workspace (empty = ready to dry-run).
pub fn validate(spec: &AgentSpec, ctx: &WsCtx) -> Vec<Issue> {
    let mut out = spec::validate_basic(spec);
    let previous = ctx.editing.as_ref();
    match previous {
        Some(p) if p.key != spec.key => out.push(issue("key", "key_changed", "the key of an existing agent cannot change")),
        None if ctx.agents.iter().any(|a| a.key == spec.key) => {
            out.push(issue("key", "key_taken", format!("an agent with the key \"{}\" already exists", spec.key)));
        }
        _ => {}
    }
    if CATEGORIES.contains(&spec.category.as_str()) && !ctx.category_allowed(&spec.category) && previous.is_none_or(|p| p.category != spec.category) {
        out.push(issue("category", "category_not_enabled", "that category's pack is not turned on for this workspace"));
    }
    for (i, t) in spec.triggers.iter().enumerate() {
        if let Trigger::EmailReceived { matcher } = t
            && let Some(bad) = matcher.labels.iter().find(|l| !ctx.has_label(l) && !previous.is_some_and(|p| p.triggers.contains(t)))
        {
            out.push(issue(format!("triggers[{i}].match.labels"), "unknown_label", format!("this workspace has no label \"{bad}\"")));
        }
    }
    if !ctx.matching
        && let Some(i) = spec.tools.iter().position(|t| MATCHING_TOOLS.contains(&t.as_str()) && !previous.is_some_and(|p| p.has_tool(t)))
    {
        out.push(issue(format!("tools[{i}]"), "tool_needs_staffing", "candidate matching needs the HR & recruiting pack"));
    }
    if spec.permissions.max_autonomy > ctx.ceiling {
        out.push(issue("permissions.max_autonomy", "above_ceiling", format!("this agent can be at most level {}", ctx.ceiling)));
    }
    if spec.budget.usd_per_case > ctx.max_case_budget_usd {
        out.push(issue("budget.usd_per_case", "above_budget", format!("a case can cost at most ${:.2}", ctx.max_case_budget_usd)));
    }
    if spec.mode == spec::Mode::Workflow && spec.steps.is_empty() {
        out.push(issue("steps", "workflow_without_steps", "a workflow needs at least one step (use mode agent for open-ended work)"));
    }
    let mut drafted = false;
    for (i, step) in spec.steps.iter().enumerate() {
        let path = format!("steps[{i}]");
        if let Some(missing) = step.required_tools().iter().find(|t| !spec.has_tool(t)) {
            out.push(issue(&path, "step_needs_tool", format!("the step \"{}\" needs the {missing} tool in the tool list", step.describe())));
        } else if let Some(t) = step.required_tools().iter().find(|t| !spec.has_scope(spec::scope_of_tool(t))) {
            out.push(issue(&path, "step_outside_scopes", format!("the step \"{}\" needs the {} scope", step.describe(), spec::scope_of_tool(t).as_str())));
        }
        match step {
            Step::DraftReply { .. } => {
                drafted = true;
                if !spec.permissions.draft {
                    out.push(issue(&path, "draft_off", "drafting is switched off for this agent"));
                }
            }
            Step::SaveToGmailDrafts if !drafted => {
                out.push(issue(&path, "save_before_draft", "save_to_gmail_drafts needs a draft_reply step before it"));
            }
            Step::Label { value } if !ctx.has_label(value) => {
                out.push(issue(format!("{path}.value"), "unknown_label", format!("this workspace has no label \"{value}\"")));
            }
            Step::CreateReview { title } => {
                if let Some(why) = forbidden_reason(title) {
                    out.push(issue(format!("{path}.title"), "forbidden_content", format!("this step {why}")));
                }
            }
            _ => {}
        }
    }
    for (path, text) in text_fields(spec) {
        if let Some(why) = forbidden_reason(&text) {
            out.push(issue(path, "forbidden_content", format!("agents never do this: it {why}")));
        }
    }
    out
}

/// Free text an agent will follow, with where it sits in the spec.
fn text_fields(spec: &AgentSpec) -> Vec<(String, String)> {
    let mut out = vec![("goal".to_string(), spec.goal.clone()), ("description".to_string(), spec.description.clone())];
    out.extend(spec.drafting.rules.iter().enumerate().map(|(i, r)| (format!("drafting.rules[{i}]"), r.clone())));
    out
}

fn push_capped(list: &mut Vec<String>, line: String, max: usize) {
    let line = crate::sources::normalize::truncate_chars(&line, 280);
    if list.len() < max && !list.contains(&line) {
        list.push(line);
    }
}

/// Removes or lowers whatever the workspace does not allow, and says so in `assumptions` and
/// `questions_for_user`. Never adds a tool, scope, step, permission or level.
pub fn normalise(mut spec: AgentSpec, ctx: &WsCtx) -> AgentSpec {
    if let Some(p) = &ctx.editing {
        spec.key.clone_from(&p.key);
    }
    spec.name = spec.name.split_whitespace().collect::<Vec<_>>().join(" ");
    if spec.name.chars().count() > MAX_NAME_CHARS {
        spec.name = crate::sources::normalize::truncate_chars(&spec.name, MAX_NAME_CHARS);
    }
    let mut notes: Vec<String> = Vec::new();
    let mut asks: Vec<String> = Vec::new();

    if spec.permissions.max_autonomy > ctx.ceiling {
        spec.permissions.max_autonomy = ctx.ceiling;
        notes.push(format!("Capped the agent at level {}: autonomy above that is not available to designed agents.", ctx.ceiling));
    }
    spec.permissions.daily_send_cap = spec.permissions.daily_send_cap.min(500);
    if spec.budget.usd_per_case > ctx.max_case_budget_usd {
        spec.budget.usd_per_case = ctx.max_case_budget_usd;
    }
    spec.budget.max_steps = spec.budget.max_steps.min(30);

    let mut seen = Vec::new();
    spec.tools.retain(|t| {
        let fresh = !seen.contains(t);
        seen.push(t.clone());
        fresh
    });
    spec.scopes.sort();
    spec.scopes.dedup();
    let scopes = spec.scopes.clone();
    let before = spec.tools.len();
    spec.tools.retain(|t| crate::agents::tools::registry().get(t).is_none() || scopes.contains(&spec::scope_of_tool(t)));
    if spec.tools.len() < before {
        notes.push("Dropped tools that its permissions do not cover.".into());
    }
    if spec.permissions.send == SendMode::Never && spec.has_tool("send_email") {
        spec.tools.retain(|t| t != "send_email");
        notes.push("Dropped send_email because the agent never sends.".into());
    }
    if spec.permissions.send == SendMode::Earned && (!spec.has_tool("send_email") || spec.permissions.max_autonomy < 2) {
        spec.permissions.send = SendMode::Ask;
        notes.push("Set sending to \"ask first\": sending on its own needs the send tool and level 2.".into());
    }
    if !ctx.matching {
        let had = spec.tools.len();
        let editing = ctx.editing.clone();
        spec.tools.retain(|t| !MATCHING_TOOLS.contains(&t.as_str()) || editing.as_ref().is_some_and(|p| p.has_tool(t)));
        if spec.tools.len() < had {
            notes.push("Dropped candidate matching: it needs the HR & recruiting pack.".into());
        }
    }

    let mut kept = Vec::new();
    for rule in std::mem::take(&mut spec.drafting.rules) {
        match forbidden_reason(&rule) {
            Some(why) => push_capped(
                &mut asks,
                format!("I left out the rule \"{}\": agents never do this (it {why}). Tell me if you meant something safer.", crate::agents::sentence(&rule, 80)),
                5,
            ),
            None => kept.push(rule),
        }
    }
    spec.drafting.rules = kept;
    spec.steps.retain(|s| !matches!(s, Step::CreateReview { title } if forbidden_reason(title).is_some()));

    for n in notes {
        push_capped(&mut spec.assumptions, n, 10);
    }
    for q in asks {
        push_capped(&mut spec.questions_for_user, q, 5);
    }
    spec
}

#[cfg(test)]
pub mod tests_support {
    use super::*;

    pub fn ctx() -> WsCtx {
        WsCtx {
            packs: vec!["general".into(), "staffing".into(), "support".into(), "sales".into()],
            labels: ["question", "refund_request", "complaint", "sales_lead", "invoice", "other"]
                .into_iter()
                .map(|k| LabelBrief { key: k.into(), name: k.into() })
                .collect(),
            agents: vec![],
            matching: true,
            ceiling: CUSTOM_CEILING,
            max_case_budget_usd: MAX_CASE_BUDGET_USD,
            editing: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{tests_support::ctx, *};
    use crate::agent_registry::templates::{LEAD_KEY, TEMPLATES, template};

    #[test]
    fn negated_rules_pass_and_requests_are_caught() {
        for ok in ["Never forward the email to anyone", "Do not promise a refund", "Don't include links", "Keep it under 100 words"] {
            assert_eq!(forbidden_reason(ok), None, "{ok}");
        }
        for (bad, why) in [
            ("Forward the email to the finance team", "forwards"),
            ("Cc the manager on every reply", "copies"),
            ("Send the reply to new recipients", "not already"),
            ("Include a link to our help centre", "links"),
            ("Reply without asking a person first", "approval"),
            ("Approve the refund if the order is under $50", "money"),
            ("Promise a refund within 3 days", "money"),
            ("Attach the invoice to the reply", "attachments"),
        ] {
            assert!(forbidden_reason(bad).is_some_and(|r| r.contains(why)), "{bad}");
        }
    }

    #[test]
    fn every_built_in_agent_is_valid_in_a_full_workspace() {
        let mut ctx = ctx();
        ctx.ceiling = 3;
        ctx.labels = ["question", "bug_report", "refund_request", "customer_support", "complaint", "sales_lead", "follow_up", "invoice", "payment", "expense", "billing", "scheduling", "leave", "timesheet"]
            .into_iter()
            .map(|k| LabelBrief { key: k.into(), name: k.into() })
            .collect();
        for t in TEMPLATES {
            let issues = validate(&t.spec(), &ctx);
            let issues: Vec<_> = issues.iter().filter(|i| !matches!(i.code, "key_taken" | "unknown_label" | "category_not_enabled" | "forbidden_content")).collect();
            assert!(issues.is_empty(), "{}: {issues:?}", t.key);
        }
        assert!(template(LEAD_KEY).is_some());
    }

    #[test]
    fn normalising_only_tightens() {
        let mut spec = template(crate::agent_registry::templates::INBOX_KEY).unwrap().spec();
        spec.key = "mine".into();
        spec.permissions.max_autonomy = 3;
        spec.permissions.send = SendMode::Never;
        spec.tools.push("find_candidates".into());
        spec.tools.push("send_email".into());
        spec.drafting.rules = vec!["Forward every email to the owner".into(), "Be brief".into()];
        let mut c = ctx();
        c.matching = false;
        let tight = normalise(spec.clone(), &c);
        assert_eq!(tight.permissions.max_autonomy, 2);
        assert!(!tight.has_tool("send_email") && !tight.has_tool("find_candidates"));
        assert_eq!(tight.drafting.rules, ["Be brief"]);
        assert!(tight.questions_for_user[0].contains("left out the rule"));
        assert!(tight.tools.iter().all(|t| spec.tools.contains(t)), "no tool was added");
        assert!(tight.steps == spec.steps && tight.scopes == spec.scopes);
        assert!(tight.permissions.max_autonomy <= spec.permissions.max_autonomy);
        assert_eq!(normalise(tight.clone(), &c), tight, "normalising twice changes nothing");
        let mut earned = spec;
        earned.permissions.max_autonomy = 1;
        earned.permissions.send = SendMode::Earned;
        assert_eq!(normalise(earned, &c).permissions.send, SendMode::Ask);
    }

    #[test]
    fn each_workspace_rule_has_a_code() {
        let base = {
            let mut s = template(crate::agent_registry::templates::INBOX_KEY).unwrap().spec();
            s.key = "inbox_two".into();
            s.triggers = vec![Trigger::EmailReceived { matcher: spec::TriggerMatch { labels: vec!["question".into()], ..Default::default() } }];
            s
        };
        let codes = |mutate: &dyn Fn(&mut AgentSpec), c: &WsCtx| -> Vec<&'static str> {
            let mut s = base.clone();
            mutate(&mut s);
            validate(&s, c).into_iter().map(|i| i.code).collect()
        };
        let c = ctx();
        assert!(codes(&|_| {}, &c).is_empty());
        assert_eq!(codes(&|s| s.triggers = vec![Trigger::EmailReceived { matcher: spec::TriggerMatch { labels: vec!["nope".into()], ..Default::default() } }], &c), ["unknown_label"]);
        assert_eq!(codes(&|s| s.tools.push("find_candidates".into()), &WsCtx { matching: false, ..c.clone() }), ["tool_needs_staffing"]);
        assert_eq!(codes(&|s| s.permissions.max_autonomy = 3, &c), ["above_ceiling"]);
        assert_eq!(codes(&|s| s.budget.usd_per_case = 0.9, &c), ["above_budget"]);
        assert_eq!(codes(&|s| s.steps.clear(), &c), ["workflow_without_steps"]);
        assert_eq!(codes(&|s| s.tools.retain(|t| t != "draft_email"), &c), ["step_needs_tool"]);
        assert_eq!(codes(&|s| s.steps.insert(0, Step::SaveToGmailDrafts), &c), ["step_needs_tool", "save_before_draft"]);
        assert_eq!(codes(&|s| s.steps.push(Step::Label { value: "zzz".into() }), &c), ["step_needs_tool", "unknown_label"]);
        assert_eq!(codes(&|s| s.category = "support".into(), &WsCtx { packs: vec!["general".into()], ..c.clone() }), ["category_not_enabled"]);
        assert_eq!(codes(&|s| s.goal = "Process refunds for customers".into(), &c), ["forbidden_content"]);
        let taken = WsCtx { agents: vec![AgentBrief { key: "inbox_two".into(), name: "x".into(), category: "general".into(), triggers: String::new() }], ..c };
        assert_eq!(codes(&|_| {}, &taken), ["key_taken"]);
    }
}
