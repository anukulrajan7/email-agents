//! `AgentSpec`: the contract every agent package builds on. A spec is plain data (triggers, steps,
//! tools, permissions, memory, drafting rules, budget) that code validates before anything runs.
//! Unknown fields are rejected, so a typo or an injected field never silently does something.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const CATEGORIES: &[&str] = &["general", "support", "sales", "finance", "personal", "hrms", "custom"];
pub const MAX_AUTONOMY: u8 = 3;
pub const MAX_NAME_CHARS: usize = 60;
pub const PRIORITIES: &[&str] = &["low", "normal", "high"];
pub const NOTIFY_TARGETS: &[&str] = &["owner", "team"];

static SLUG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z][a-z0-9_]{1,63}$").expect("valid regex"));
static DOMAIN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?(\.[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?)+$").expect("valid regex"));
static ATTACHMENT_TYPE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9./+-]{0,31}$").expect("valid regex"));

/// What an agent may touch at all. A tool outside the agent's scopes is never offered or run, so a
/// read-only agent (`read_mail` only) cannot draft, label or send whatever its prompt says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// Read mail, threads, memory and search.
    ReadMail,
    /// Write drafts.
    Draft,
    /// Send mail, submit candidates.
    Send,
    /// Organise work: tasks and case notes.
    Label,
    /// Wake later, propose automations.
    Schedule,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::ReadMail => "read_mail",
            Scope::Draft => "draft",
            Scope::Send => "send",
            Scope::Label => "label",
            Scope::Schedule => "schedule",
        }
    }
}

/// The scope a tool needs.
pub fn scope_of_tool(tool: &str) -> Scope {
    match tool {
        "draft_email" => Scope::Draft,
        "send_email" | "submit_candidate" | "send_message" => Scope::Send,
        "save_to_gmail_drafts" => Scope::Draft,
        "create_task" | "update_case" | "label_email" | "notify_owner" => Scope::Label,
        "schedule_check" | "propose_automation" => Scope::Schedule,
        _ => Scope::ReadMail,
    }
}

/// The scopes that exactly cover these tools (sorted, no repeats).
pub fn scopes_for_tools(tools: &[String]) -> Vec<Scope> {
    let mut scopes: Vec<Scope> = tools.iter().map(|t| scope_of_tool(t)).collect();
    scopes.sort();
    scopes.dedup();
    scopes
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Steps known in advance.
    Workflow,
    /// A planner loop picks the next tool.
    Agent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SendMode {
    Never,
    Ask,
    /// May go out without a person once the agent has earned autonomy level 2 or more.
    Earned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityTier {
    Fast,
    Balanced,
    Best,
}

impl QualityTier {
    pub fn as_str(self) -> &'static str {
        match self {
            QualityTier::Fast => "fast",
            QualityTier::Balanced => "balanced",
            QualityTier::Best => "best",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryScope {
    #[serde(rename = "own+shared")]
    OwnShared,
    #[serde(rename = "own")]
    Own,
    #[serde(rename = "shared")]
    Shared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearnFrom {
    Edits,
    Approvals,
    Labels,
    Routes,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerMatch {
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub from_domains: Vec<String>,
    #[serde(default)]
    pub from_addresses: Vec<String>,
    #[serde(default)]
    pub subject_keywords: Vec<String>,
    #[serde(default)]
    pub needs_reply: Option<bool>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub attachment_types: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Trigger {
    EmailReceived {
        #[serde(rename = "match", default)]
        matcher: TriggerMatch,
    },
    Schedule {
        every_hours: u32,
    },
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    String,
    Number,
    Date,
    Boolean,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtractField {
    pub name: String,
    #[serde(rename = "type")]
    pub field_type: FieldType,
    #[serde(default)]
    pub description: String,
}

fn balanced() -> QualityTier {
    QualityTier::Balanced
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Step {
    Extract { fields: Vec<ExtractField> },
    DraftReply {
        #[serde(default = "balanced")]
        quality: QualityTier,
    },
    CreateReview { title: String },
    RemindIfNoReply { hours: u32 },
    SaveToGmailDrafts,
    Label { value: String },
    StartCase,
    Notify { to: String },
}

impl Step {
    /// The tools a fixed workflow calls to carry this step out. A step only runs when its spec lists them.
    pub fn required_tools(&self) -> &'static [&'static str] {
        match self {
            Step::Extract { .. } => &["extract_fields"],
            Step::DraftReply { .. } => &["draft_email"],
            Step::CreateReview { .. } | Step::Notify { .. } => &["create_task"],
            Step::RemindIfNoReply { .. } => &["schedule_check", "create_task"],
            Step::SaveToGmailDrafts => &["save_to_gmail_drafts"],
            Step::Label { .. } => &["label_email"],
            Step::StartCase => &["update_case"],
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Step::Extract { fields } => format!("extract {}", fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", ")),
            Step::DraftReply { quality } => format!("draft a reply ({} quality)", quality.as_str()),
            Step::CreateReview { title } => format!("ask a person to review: {title}"),
            Step::RemindIfNoReply { hours } => format!("remind if there is no reply after {hours} hours"),
            Step::SaveToGmailDrafts => "save the draft to Gmail".into(),
            Step::Label { value } => format!("label it {value}"),
            Step::StartCase => "start a case".into(),
            Step::Notify { to } => format!("notify {to}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Permissions {
    #[serde(default = "yes")]
    pub draft: bool,
    pub send: SendMode,
    pub max_autonomy: u8,
    #[serde(default = "default_daily_send_cap")]
    pub daily_send_cap: u32,
}

fn default_scopes() -> Vec<Scope> {
    vec![Scope::ReadMail, Scope::Draft]
}

fn yes() -> bool {
    true
}

fn default_daily_send_cap() -> u32 {
    20
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemorySpec {
    pub scope: MemoryScope,
    #[serde(default)]
    pub learn_from: Vec<LearnFrom>,
}

impl Default for MemorySpec {
    fn default() -> Self {
        Self { scope: MemoryScope::OwnShared, learn_from: vec![LearnFrom::Edits, LearnFrom::Approvals, LearnFrom::Labels, LearnFrom::Routes] }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Drafting {
    #[serde(default)]
    pub rules: Vec<String>,
    #[serde(default)]
    pub never_do: Vec<String>,
    #[serde(default = "balanced")]
    pub quality_tier: QualityTier,
}

impl Default for Drafting {
    fn default() -> Self {
        Self { rules: vec![], never_do: vec![], quality_tier: QualityTier::Balanced }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    pub max_steps: u32,
    pub usd_per_case: f64,
}

impl Default for Budget {
    fn default() -> Self {
        Self { max_steps: 15, usd_per_case: 0.10 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSpec {
    pub name: String,
    pub key: String,
    pub category: String,
    #[serde(default)]
    pub description: String,
    pub goal: String,
    /// How this agent thinks: what it checks first, its tone and what it never does. Shapes judgement, never the signature.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub persona: String,
    pub mode: Mode,
    #[serde(default)]
    pub triggers: Vec<Trigger>,
    #[serde(default)]
    pub steps: Vec<Step>,
    #[serde(default)]
    pub tools: Vec<String>,
    /// Explicit scopes; every tool must be covered by one.
    #[serde(default = "default_scopes")]
    pub scopes: Vec<Scope>,
    pub permissions: Permissions,
    #[serde(default)]
    pub memory: MemorySpec,
    #[serde(default)]
    pub drafting: Drafting,
    #[serde(default)]
    pub budget: Budget,
    #[serde(default)]
    pub questions_for_user: Vec<String>,
    #[serde(default)]
    pub assumptions: Vec<String>,
}

impl AgentSpec {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a spec always serialises")
    }

    /// Stable fingerprint of the spec's content.
    pub fn hash(&self) -> String {
        Sha256::digest(self.to_json().as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn has_tool(&self, name: &str) -> bool {
        self.tools.iter().any(|t| t == name)
    }

    pub fn has_scope(&self, scope: Scope) -> bool {
        self.scopes.contains(&scope)
    }

    /// Tools the agent may call: listed in `tools` and covered by a scope.
    pub fn usable_tools(&self) -> Vec<String> {
        self.tools.iter().filter(|t| self.has_scope(scope_of_tool(t))).cloned().collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Issue {
    pub path: String,
    pub code: &'static str,
    pub message: String,
}

fn issue(path: impl Into<String>, code: &'static str, message: impl Into<String>) -> Issue {
    Issue { path: path.into(), code, message: message.into() }
}

pub fn is_slug(value: &str) -> bool {
    SLUG.is_match(value)
}

fn too_long(items: &[String], max_items: usize, max_chars: usize, path: &str, out: &mut Vec<Issue>) {
    if items.len() > max_items {
        out.push(issue(path, "too_many", format!("at most {max_items} items")));
    }
    for (i, item) in items.iter().enumerate() {
        let n = item.trim().chars().count();
        if n == 0 || n > max_chars {
            out.push(issue(format!("{path}[{i}]"), "bad_length", format!("each item must be 1 to {max_chars} characters")));
        }
    }
}

fn check_match(m: &TriggerMatch, path: &str, out: &mut Vec<Issue>) {
    if m.labels.len() > 20 || m.labels.iter().any(|l| !is_slug(l)) {
        out.push(issue(format!("{path}.labels"), "bad_labels", "up to 20 label keys, lower-case letters, digits and underscores"));
    }
    if m.from_domains.len() > 10 || m.from_domains.iter().any(|d| !DOMAIN.is_match(d)) {
        out.push(issue(format!("{path}.from_domains"), "bad_domains", "up to 10 lower-case domains such as client.com"));
    }
    if m.from_addresses.len() > 10 || m.from_addresses.iter().any(|a| a != &a.to_lowercase() || !a.contains('@') || a.contains(char::is_whitespace) || a.len() > 120) {
        out.push(issue(format!("{path}.from_addresses"), "bad_addresses", "up to 10 lower-case email addresses"));
    }
    too_long(&m.subject_keywords, 10, 60, &format!("{path}.subject_keywords"), out);
    if m.attachment_types.len() > 8 || m.attachment_types.iter().any(|t| !ATTACHMENT_TYPE.is_match(t)) {
        out.push(issue(format!("{path}.attachment_types"), "bad_attachment_types", "up to 8 lower-case types such as pdf or xlsx"));
    }
    if m.priority.as_deref().is_some_and(|p| !PRIORITIES.contains(&p)) {
        out.push(issue(format!("{path}.priority"), "bad_priority", "priority must be low, normal or high"));
    }
}

/// A field to extract needs a lower-case name and a description of at most 200 characters.
pub fn field_ok(f: &ExtractField) -> bool {
    is_slug(&f.name) && f.description.chars().count() <= 200
}

fn check_step(step: &Step, path: &str, out: &mut Vec<Issue>) {
    match step {
        Step::Extract { fields } => {
            if fields.is_empty() || fields.len() > 12 {
                out.push(issue(format!("{path}.fields"), "bad_fields", "extract 1 to 12 fields"));
            }
            for (i, f) in fields.iter().enumerate() {
                if !field_ok(f) {
                    out.push(issue(format!("{path}.fields[{i}]"), "bad_field", "a field needs a lower-case name and a description of at most 200 characters"));
                }
            }
        }
        Step::CreateReview { title } if title.trim().is_empty() || title.chars().count() > 160 => {
            out.push(issue(format!("{path}.title"), "bad_length", "the review title must be 1 to 160 characters"));
        }
        Step::RemindIfNoReply { hours } if !(1..=336).contains(hours) => {
            out.push(issue(format!("{path}.hours"), "bad_hours", "hours must be 1 to 336"));
        }
        Step::Label { value } if !is_slug(value) => {
            out.push(issue(format!("{path}.value"), "bad_label", "a label key is lower-case letters, digits and underscores"));
        }
        Step::Notify { to } if !NOTIFY_TARGETS.contains(&to.as_str()) => {
            out.push(issue(format!("{path}.to"), "bad_target", "notify owner or team"));
        }
        _ => {}
    }
}

/// Everything about a spec that needs no workspace: shape, bounds, known tools and coherent
/// permissions. Returns every issue found (empty = valid).
pub fn validate_basic(spec: &AgentSpec) -> Vec<Issue> {
    let mut out = Vec::new();
    if !is_slug(&spec.key) {
        out.push(issue("key", "bad_key", "the key is lower-case letters, digits and underscores, starting with a letter"));
    }
    let name_len = spec.name.trim().chars().count();
    if name_len == 0 || name_len > MAX_NAME_CHARS {
        out.push(issue("name", "bad_length", format!("the name must be 1 to {MAX_NAME_CHARS} characters")));
    }
    if spec.description.chars().count() > 500 {
        out.push(issue("description", "bad_length", "the description can be at most 500 characters"));
    }
    if spec.persona.chars().count() > 1200 {
        out.push(issue("persona", "bad_length", "the persona must be at most 1200 characters"));
    }
    let goal_len = spec.goal.trim().chars().count();
    if goal_len == 0 || goal_len > 300 {
        out.push(issue("goal", "bad_length", "the goal must be 1 to 300 characters"));
    }
    if !CATEGORIES.contains(&spec.category.as_str()) {
        out.push(issue("category", "bad_category", format!("category must be one of {}", CATEGORIES.join(", "))));
    }
    if spec.triggers.is_empty() || spec.triggers.len() > 8 {
        out.push(issue("triggers", "bad_triggers", "give 1 to 8 triggers"));
    }
    for (i, t) in spec.triggers.iter().enumerate() {
        match t {
            Trigger::EmailReceived { matcher } => check_match(matcher, &format!("triggers[{i}].match"), &mut out),
            Trigger::Schedule { every_hours } if !(1..=720).contains(every_hours) => {
                out.push(issue(format!("triggers[{i}].every_hours"), "bad_hours", "a schedule runs every 1 to 720 hours"));
            }
            _ => {}
        }
    }
    if spec.steps.len() > 12 {
        out.push(issue("steps", "too_many", "at most 12 steps"));
    }
    for (i, s) in spec.steps.iter().enumerate() {
        check_step(s, &format!("steps[{i}]"), &mut out);
    }
    let registry = crate::agents::tools::registry();
    if spec.tools.len() > 20 {
        out.push(issue("tools", "too_many", "at most 20 tools"));
    }
    for (i, tool) in spec.tools.iter().enumerate() {
        if registry.get(tool).is_none() {
            out.push(issue(format!("tools[{i}]"), "unknown_tool", format!("{tool} is not a tool agents can use")));
        } else if spec.tools[..i].contains(tool) {
            out.push(issue(format!("tools[{i}]"), "duplicate_tool", format!("{tool} is listed twice")));
        }
    }
    for (i, tool) in spec.tools.iter().enumerate() {
        let needed = scope_of_tool(tool);
        if !spec.has_scope(needed) && crate::agents::tools::registry().get(tool).is_some() {
            out.push(issue(format!("tools[{i}]"), "tool_outside_scopes", format!("{tool} needs the {} scope", needed.as_str())));
        }
    }
    if spec.scopes.is_empty() || spec.scopes.windows(2).any(|w| w[0] >= w[1]) {
        out.push(issue("scopes", "bad_scopes", "list each scope once, in the order read_mail, draft, send, label, schedule"));
    }
    let p = &spec.permissions;
    if p.send != SendMode::Never && !spec.has_scope(Scope::Send) && spec.has_tool("send_email") {
        out.push(issue("permissions.send", "send_without_scope", "sending needs the send scope"));
    }
    if p.max_autonomy > MAX_AUTONOMY {
        out.push(issue("permissions.max_autonomy", "bad_autonomy", "max_autonomy is 0 to 3"));
    }
    if p.daily_send_cap > 500 {
        out.push(issue("permissions.daily_send_cap", "bad_cap", "the daily send cap is at most 500"));
    }
    if p.send == SendMode::Earned && !spec.has_tool("send_email") {
        out.push(issue("permissions.send", "send_without_tool", "send 'earned' needs the send_email tool"));
    }
    if p.send == SendMode::Never && spec.has_tool("send_email") {
        out.push(issue("tools", "send_tool_without_permission", "send_email is listed but sending is never allowed"));
    }
    if !p.draft && spec.has_tool("draft_email") {
        out.push(issue("tools", "draft_tool_without_permission", "draft_email is listed but drafting is switched off"));
    }
    if p.send == SendMode::Earned && p.max_autonomy < 2 {
        out.push(issue("permissions.max_autonomy", "earned_needs_level_2", "sending on its own needs max_autonomy of at least 2"));
    }
    too_long(&spec.drafting.rules, 10, 200, "drafting.rules", &mut out);
    too_long(&spec.drafting.never_do, 10, 200, "drafting.never_do", &mut out);
    if !(1..=30).contains(&spec.budget.max_steps) {
        out.push(issue("budget.max_steps", "bad_budget", "max_steps is 1 to 30"));
    }
    if !(0.001..=1.0).contains(&spec.budget.usd_per_case) {
        out.push(issue("budget.usd_per_case", "bad_budget", "usd_per_case is 0.001 to 1.00"));
    }
    too_long(&spec.questions_for_user, 5, 300, "questions_for_user", &mut out);
    too_long(&spec.assumptions, 10, 300, "assumptions", &mut out);
    out
}

/// One line per validation issue, for API errors.
pub fn issues_message(issues: &[Issue]) -> String {
    issues.iter().map(|i| format!("{}: {}", i.path, i.message)).collect::<Vec<_>>().join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn spec_json() -> Value {
        json!({
            "name": "Ticket Resolver", "key": "ticket_resolver", "category": "support",
            "description": "Answers customer questions.", "goal": "Resolve one customer question end to end",
            "mode": "workflow",
            "triggers": [
                {"type": "email_received", "match": {"labels": ["question"], "from_domains": ["client.com"], "from_addresses": [], "subject_keywords": [],
                    "needs_reply": true, "priority": null, "attachment_types": []}},
                {"type": "schedule", "every_hours": 24}, {"type": "manual"}
            ],
            "steps": [
                {"type": "extract", "fields": [{"name": "order_id", "type": "string", "description": "The order number"}]},
                {"type": "draft_reply", "quality": "balanced"}, {"type": "create_review", "title": "Check the reply"},
                {"type": "remind_if_no_reply", "hours": 48}, {"type": "save_to_gmail_drafts"},
                {"type": "label", "value": "handled"}, {"type": "start_case"}, {"type": "notify", "to": "owner"}
            ],
            "tools": ["read_email", "read_thread", "recall_memory", "search_mail", "draft_email", "create_task", "schedule_check", "update_case", "send_email"],
            "scopes": ["read_mail", "draft", "send", "label", "schedule"],
            "permissions": {"draft": true, "send": "earned", "max_autonomy": 2, "daily_send_cap": 20},
            "memory": {"scope": "own+shared", "learn_from": ["edits", "approvals", "labels", "routes"]},
            "drafting": {"rules": ["Be brief"], "never_do": ["Promise refunds"], "quality_tier": "balanced"},
            "budget": {"max_steps": 15, "usd_per_case": 0.10},
            "questions_for_user": [], "assumptions": ["Orders look like #123"]
        })
    }

    #[test]
    fn the_contract_round_trips() {
        let spec: AgentSpec = serde_json::from_value(spec_json()).unwrap();
        assert_eq!(serde_json::to_value(&spec).unwrap(), spec_json());
        assert!(validate_basic(&spec).is_empty(), "{:?}", validate_basic(&spec));
        assert_eq!(spec.hash(), spec.clone().hash());
        assert_eq!(spec.hash().len(), 64);
    }

    #[test]
    fn unknown_fields_are_rejected_everywhere() {
        for (path, extra) in [("", "model"), ("/permissions", "bypass"), ("/budget", "x"), ("/drafting", "x"), ("/memory", "x")] {
            let mut v = spec_json();
            v.pointer_mut(if path.is_empty() { "" } else { path }).unwrap()[extra] = json!(1);
            assert!(serde_json::from_value::<AgentSpec>(v).is_err(), "{path}/{extra}");
        }
        let mut v = spec_json();
        v["triggers"][0]["match"]["regex"] = json!(".*");
        assert!(serde_json::from_value::<AgentSpec>(v).is_err());
        let mut v = spec_json();
        v["steps"][0]["extra"] = json!(1);
        assert!(serde_json::from_value::<AgentSpec>(v).is_err());
        let mut v = spec_json();
        v["steps"].as_array_mut().unwrap().push(json!({"type": "send_reply"}));
        assert!(serde_json::from_value::<AgentSpec>(v).is_err(), "there is no send_reply step");
    }

    fn codes(mutate: impl FnOnce(&mut AgentSpec)) -> Vec<&'static str> {
        let mut spec: AgentSpec = serde_json::from_value(spec_json()).unwrap();
        mutate(&mut spec);
        validate_basic(&spec).into_iter().map(|i| i.code).collect()
    }

    #[test]
    fn scopes_gate_tools() {
        let spec: AgentSpec = serde_json::from_value(spec_json()).unwrap();
        assert_eq!(spec.usable_tools().len(), 9);
        let read_only = AgentSpec { scopes: vec![Scope::ReadMail], ..spec.clone() };
        assert_eq!(read_only.usable_tools(), ["read_email", "read_thread", "recall_memory", "search_mail"]);
        assert_eq!(scopes_for_tools(&spec.tools), spec.scopes);
        assert_eq!(scope_of_tool("submit_candidate"), Scope::Send);
        assert_eq!(scope_of_tool("find_candidates"), Scope::ReadMail);
    }

    #[test]
    fn validation_catches_each_rule() {
        assert_eq!(codes(|s| s.key = "Bad Key".into()), ["bad_key"]);
        assert_eq!(codes(|s| s.name = " ".into()), ["bad_length"]);
        assert_eq!(codes(|s| s.category = "pirates".into()), ["bad_category"]);
        assert_eq!(codes(|s| s.triggers.clear()), ["bad_triggers"]);
        assert_eq!(codes(|s| s.tools.push("rm_rf".into())), ["unknown_tool"]);
        assert_eq!(codes(|s| s.tools.push("read_email".into())), ["duplicate_tool"]);
        assert_eq!(codes(|s| s.permissions.max_autonomy = 4), ["bad_autonomy"]);
        assert_eq!(codes(|s| s.scopes.retain(|c| *c != Scope::Send)), ["tool_outside_scopes", "send_without_scope"]);
        assert_eq!(codes(|s| s.scopes.retain(|c| *c != Scope::Schedule)), ["tool_outside_scopes"]);
        assert_eq!(codes(|s| s.scopes = vec![Scope::Draft, Scope::ReadMail, Scope::Send, Scope::Label, Scope::Schedule]), ["bad_scopes"]);
        assert_eq!(codes(|s| s.permissions.max_autonomy = 1), ["earned_needs_level_2"]);
        assert_eq!(codes(|s| s.tools.retain(|t| t != "send_email")), ["send_without_tool"]);
        assert_eq!(codes(|s| s.permissions.send = SendMode::Never), ["send_tool_without_permission"]);
        assert_eq!(codes(|s| s.budget.max_steps = 0), ["bad_budget"]);
        assert_eq!(codes(|s| s.budget.usd_per_case = 5.0), ["bad_budget"]);
        assert_eq!(codes(|s| s.triggers.push(Trigger::Schedule { every_hours: 0 })), ["bad_hours"]);
        assert_eq!(
            codes(|s| s.triggers.push(Trigger::EmailReceived { matcher: TriggerMatch { from_domains: vec!["not a domain".into()], ..Default::default() } })),
            ["bad_domains"]
        );
        assert_eq!(codes(|s| s.steps.push(Step::Notify { to: "everyone".into() })), ["bad_target"]);
        assert_eq!(codes(|s| s.drafting.never_do = vec!["x".into(); 11]), ["too_many"]);
    }
}
