//! System agent templates, versioned in code like packs. A template is never stored: an agent row
//! records which template and version it came from, and its specs live in `agent_versions`.

use super::spec::{
    AgentSpec, Budget, scopes_for_tools, Drafting, ExtractField, FieldType, MemoryScope, MemorySpec, Mode, Permissions, QualityTier, SendMode, Step, Trigger,
    TriggerMatch,
};

pub struct AgentTemplate {
    pub key: &'static str,
    pub category: &'static str,
    /// The pack that brings this agent. Personal agents come with every member, under `general`.
    pub pack: &'static str,
    pub version: u32,
    /// `lead`, `system`: a template never makes a custom agent.
    pub kind: &'static str,
    /// `workspace` or `personal`.
    pub scope: &'static str,
    /// Level a freshly seeded agent starts at.
    pub initial_level: i64,
    pub name: &'static str,
    pub avatar: &'static str,
    pub color: &'static str,
    pub spec: fn() -> AgentSpec,
}

impl AgentTemplate {
    pub fn spec(&self) -> AgentSpec {
        (self.spec)()
    }

    pub fn max_autonomy(&self) -> u8 {
        self.spec().permissions.max_autonomy
    }
}

pub const LEAD_KEY: &str = "general.lead";
pub const INBOX_KEY: &str = "general.inbox";
pub const PERSONAL_KEY: &str = "personal.mailbox";
pub const REQUIREMENTS_KEY: &str = "hrms.requirements";

const READ_TOOLS: &[&str] = &["read_email", "read_thread", "recall_memory", "search_mail"];

fn tools(extra: &[&str]) -> Vec<String> {
    READ_TOOLS.iter().chain(extra).map(|t| t.to_string()).collect()
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn on_labels(labels: &[&str]) -> Trigger {
    Trigger::EmailReceived { matcher: TriggerMatch { labels: strings(labels), ..Default::default() } }
}

fn on_keywords(words: &[&str]) -> Trigger {
    Trigger::EmailReceived { matcher: TriggerMatch { subject_keywords: strings(words), ..Default::default() } }
}

fn field(name: &str, field_type: FieldType, description: &str) -> ExtractField {
    ExtractField { name: name.into(), field_type, description: description.into() }
}

struct Base {
    name: &'static str,
    key: &'static str,
    category: &'static str,
    description: &'static str,
    goal: &'static str,
    mode: Mode,
}

fn build(b: Base, triggers: Vec<Trigger>, steps: Vec<Step>, tools: Vec<String>, permissions: Permissions) -> AgentSpec {
    AgentSpec {
        scopes: scopes_for_tools(&tools),
        name: b.name.into(),
        key: b.key.into(),
        category: b.category.into(),
        description: b.description.into(),
        goal: b.goal.into(),
        mode: b.mode,
        triggers,
        steps,
        tools,
        permissions,
        memory: MemorySpec::default(),
        drafting: Drafting::default(),
        budget: Budget::default(),
        questions_for_user: vec![],
        assumptions: vec![],
    }
}

fn perms(send: SendMode, max_autonomy: u8) -> Permissions {
    Permissions { draft: true, send, max_autonomy, daily_send_cap: 20 }
}

fn with_drafting(mut spec: AgentSpec, rules: &[&str], never_do: &[&str]) -> AgentSpec {
    spec.drafting.rules = strings(rules);
    spec.drafting.never_do = strings(never_do);
    spec
}

fn lead() -> AgentSpec {
    let every_tool = [
        "find_candidates", "draft_email", "create_task", "schedule_check", "update_case", "send_email", "submit_candidate", "propose_automation",
    ];
    build(
        Base {
            name: "Meera",
            key: "lead",
            category: "general",
            description: "Routes mail, answers what no desk owns and keeps the team's voice.",
            goal: "Resolve each email that no other desk owns: reply, or hand it to a person",
            mode: Mode::Agent,
        },
        vec![Trigger::Manual],
        vec![],
        tools(&every_tool),
        perms(SendMode::Earned, 2),
    )
}

fn inbox() -> AgentSpec {
    build(
        Base {
            name: "Inbox Assistant",
            key: "inbox_assistant",
            category: "general",
            description: "Drafts replies to mail that waits for an answer and reminds you when nobody replied.",
            goal: "Draft a reply to each email that needs one and follow up if the conversation goes quiet",
            mode: Mode::Workflow,
        },
        vec![Trigger::EmailReceived { matcher: TriggerMatch { needs_reply: Some(true), ..Default::default() } }],
        vec![Step::DraftReply { quality: QualityTier::Balanced }, Step::RemindIfNoReply { hours: 48 }],
        tools(&["draft_email", "create_task", "schedule_check", "update_case", "send_email"]),
        perms(SendMode::Earned, 2),
    )
}

fn resolver() -> AgentSpec {
    with_drafting(
        build(
            Base {
                name: "Ticket Resolver",
                key: "ticket_resolver",
                category: "support",
                description: "Resolves one customer question end to end from what the team already knows.",
                goal: "Resolve one customer question end to end, answering only from known facts",
                mode: Mode::Agent,
            },
            vec![on_labels(&["question", "bug_report", "refund_request", "customer_support"])],
            vec![],
            tools(&["draft_email", "create_task", "schedule_check", "update_case", "send_email"]),
            perms(SendMode::Earned, 3),
        ),
        &["Acknowledge the problem first", "Give the next step and when to expect it"],
        &["Promise a refund or a date that is not in the notes"],
    )
}

fn escalation() -> AgentSpec {
    with_drafting(
        build(
            Base {
                name: "Escalations",
                key: "escalations",
                category: "support",
                description: "Handles complaints and angry customers: asks a person to step in and drafts a calm reply. Never sends on its own.",
                goal: "Escalate each complaint to a person and prepare a calm, honest draft",
                mode: Mode::Workflow,
            },
            vec![on_labels(&["complaint"]), on_keywords(&["escalate", "unacceptable", "legal action"])],
            vec![Step::CreateReview { title: "Customer complaint needs a person".into() }, Step::DraftReply { quality: QualityTier::Best }],
            tools(&["draft_email", "create_task", "update_case"]),
            perms(SendMode::Ask, 1),
        ),
        &["Apologise for the experience, never for facts we have not checked"],
        &["Admit fault or liability", "Offer compensation"],
    )
}

fn qualifier() -> AgentSpec {
    build(
        Base {
            name: "Lead Qualifier",
            key: "lead_qualifier",
            category: "sales",
            description: "Reads new enquiries, pulls out budget, timeline and need, and prepares a reply for review.",
            goal: "Qualify each new sales enquiry and prepare a reply for a person to review",
            mode: Mode::Workflow,
        },
        vec![on_labels(&["sales_lead"])],
        vec![
            Step::Extract {
                fields: vec![
                    field("budget", FieldType::String, "What they plan to spend, as they wrote it"),
                    field("timeline", FieldType::String, "When they want to start"),
                    field("need", FieldType::String, "What they want to achieve"),
                ],
            },
            Step::DraftReply { quality: QualityTier::Balanced },
            Step::CreateReview { title: "Review the qualified lead".into() },
        ],
        tools(&["extract_fields", "draft_email", "create_task", "update_case", "send_email"]),
        perms(SendMode::Earned, 2),
    )
}

fn followups() -> AgentSpec {
    let mut spec = build(
        Base {
            name: "Follow-ups",
            key: "follow_ups",
            category: "sales",
            description: "Keeps deals moving: follows up on proposals and quotes that went quiet.",
            goal: "Follow up on conversations about proposals and quotes that have gone quiet",
            mode: Mode::Agent,
        },
        vec![on_labels(&["follow_up"]), Trigger::Schedule { every_hours: 24 }],
        vec![],
        tools(&["draft_email", "create_task", "schedule_check", "update_case", "send_email"]),
        perms(SendMode::Earned, 2),
    );
    spec.drafting.never_do = strings(&["Offer a discount"]);
    spec
}

fn invoices() -> AgentSpec {
    with_drafting(
        build(
            Base {
                name: "Invoices & Payments",
                key: "invoices_payments",
                category: "finance",
                description: "Reads invoices and payment mail, acknowledges them and asks a person to approve. Never promises a payment.",
                goal: "Capture each invoice or payment notice, acknowledge it and hand it to a person",
                mode: Mode::Workflow,
            },
            vec![on_labels(&["invoice", "payment", "expense", "billing"])],
            vec![
                Step::Extract {
                    fields: vec![
                        field("vendor", FieldType::String, "Who sent the invoice"),
                        field("invoice_number", FieldType::String, "The invoice number"),
                        field("amount", FieldType::String, "The total with currency, as written"),
                        field("due_date", FieldType::Date, "When it is due"),
                    ],
                },
                Step::DraftReply { quality: QualityTier::Balanced },
                Step::CreateReview { title: "Approve this invoice".into() },
            ],
            tools(&["extract_fields", "draft_email", "create_task", "update_case"]),
            perms(SendMode::Ask, 1),
        ),
        &["Acknowledge receipt only"],
        &["Promise a payment date", "Confirm an amount that is not on the invoice"],
    )
}

fn scheduling() -> AgentSpec {
    with_drafting(
        build(
            Base {
                name: "Scheduling",
                key: "scheduling",
                category: "personal",
                description: "Answers meeting requests with the slots you gave it, and marks anything it does not know.",
                goal: "Reply to each meeting request, proposing slots only when they are known",
                mode: Mode::Workflow,
            },
            vec![on_labels(&["scheduling"]), on_keywords(&["meeting", "reschedule", "availability"])],
            vec![Step::DraftReply { quality: QualityTier::Fast }, Step::CreateReview { title: "Confirm the meeting slot".into() }],
            tools(&["draft_email", "create_task", "update_case", "send_email"]),
            perms(SendMode::Earned, 2),
        ),
        &["Use [NEEDS: slot] when no free time is known"],
        &["Accept a meeting without a known free slot"],
    )
}

fn hr_desk(name: &'static str, key: &'static str, description: &'static str, goal: &'static str, triggers: Vec<Trigger>, send: SendMode, max: u8) -> AgentSpec {
    let extra: &[&str] = if send == SendMode::Earned {
        &["draft_email", "create_task", "update_case", "send_email"]
    } else {
        &["draft_email", "create_task", "update_case"]
    };
    build(
        Base { name, key, category: "hrms", description, goal, mode: Mode::Workflow },
        triggers,
        vec![Step::DraftReply { quality: QualityTier::Balanced }, Step::CreateReview { title: format!("Check: {name}") }],
        tools(extra),
        perms(send, max),
    )
}

fn leave() -> AgentSpec {
    hr_desk(
        "Leave",
        "leave_desk",
        "Handles leave requests and questions about time off. Approvals are always a person's call.",
        "Acknowledge each leave request and prepare it for a manager's decision",
        vec![on_labels(&["leave"])],
        SendMode::Ask,
        1,
    )
}

fn timesheets() -> AgentSpec {
    hr_desk(
        "Timesheets",
        "timesheets_desk",
        "Acknowledges timesheets and chases missing or unclear hours.",
        "Acknowledge each timesheet and flag anything missing",
        vec![on_labels(&["timesheet"])],
        SendMode::Earned,
        2,
    )
}

fn onboarding() -> AgentSpec {
    hr_desk(
        "Onboarding",
        "onboarding_desk",
        "Collects new-joiner documents and walks them through the checklist.",
        "Collect a new joiner's documents and keep their checklist moving",
        vec![on_keywords(&["onboarding", "new joiner", "joining formalities"])],
        SendMode::Ask,
        1,
    )
}

fn requirements() -> AgentSpec {
    let mut spec = build(
        Base {
            name: "Candidates & Requirements",
            key: "candidates_requirements",
            category: "hrms",
            description: "Works a client's job requirement: shortlists candidates, asks for approval, submits and follows up.",
            goal: "Shortlist candidates for each client requirement, get approval, submit and follow up until an outcome",
            mode: Mode::Agent,
        },
        vec![on_labels(&["client_requirement", "candidate_referral"])],
        vec![],
        tools(&["find_candidates", "draft_email", "create_task", "schedule_check", "update_case", "send_email", "submit_candidate"]),
        perms(SendMode::Ask, 1),
    );
    spec.drafting.quality_tier = QualityTier::Best;
    spec.drafting.never_do = strings(&["Share a candidate's contact details before approval"]);
    spec
}

fn payroll() -> AgentSpec {
    let mut spec = hr_desk(
        "Payroll queries",
        "payroll_desk",
        "Acknowledges payroll questions and routes them to a person. Never states amounts.",
        "Acknowledge each payroll question and hand it to the payroll owner",
        vec![on_labels(&["payroll"])],
        SendMode::Ask,
        1,
    );
    spec.drafting.never_do = strings(&["State a salary, rate or payment amount", "Promise a payment date"]);
    spec
}

fn mailbox() -> AgentSpec {
    let mut spec = build(
        Base {
            name: "My assistant",
            key: "my_assistant",
            category: "personal",
            description: "Works on your own mailbox, in your voice, using your profile and preferences.",
            goal: "Handle the mail in the owner's own mailbox: draft in their voice, hand over what needs them",
            mode: Mode::Workflow,
        },
        vec![Trigger::Manual],
        vec![Step::DraftReply { quality: QualityTier::Balanced }, Step::RemindIfNoReply { hours: 48 }],
        tools(&["draft_email", "create_task", "schedule_check", "update_case", "send_email"]),
        perms(SendMode::Earned, 2),
    );
    spec.memory = MemorySpec { scope: MemoryScope::Own, ..MemorySpec::default() };
    spec
}

macro_rules! template {
    ($key:expr, $cat:expr, $pack:expr, $kind:expr, $scope:expr, $level:expr, $name:expr, $avatar:expr, $color:expr, $spec:expr) => {
        AgentTemplate {
            key: $key,
            category: $cat,
            pack: $pack,
            version: 1,
            kind: $kind,
            scope: $scope,
            initial_level: $level,
            name: $name,
            avatar: $avatar,
            color: $color,
            spec: $spec,
        }
    };
}

pub const TEMPLATES: &[AgentTemplate] = &[
    template!(LEAD_KEY, "general", "general", "lead", "workspace", 1, "Meera", "🌼", "ochre", lead),
    template!(INBOX_KEY, "general", "general", "system", "workspace", 1, "Inbox Assistant", "📥", "slate", inbox),
    template!("support.resolver", "support", "support", "system", "workspace", 0, "Ticket Resolver", "🎧", "teal", resolver),
    template!("support.escalation", "support", "support", "system", "workspace", 0, "Escalations", "🚨", "postmark", escalation),
    template!("sales.qualifier", "sales", "sales", "system", "workspace", 0, "Lead Qualifier", "🎯", "sage", qualifier),
    template!("sales.followups", "sales", "sales", "system", "workspace", 0, "Follow-ups", "🔔", "ochre", followups),
    template!("finance.invoices", "finance", "finance", "system", "workspace", 0, "Invoices & Payments", "🧾", "teal", invoices),
    template!("personal.scheduling", "personal", "personal", "system", "workspace", 0, "Scheduling", "📅", "ochre", scheduling),
    template!("hrms.leave", "hrms", "staffing", "system", "workspace", 0, "Leave", "🌴", "sage", leave),
    template!("hrms.timesheets", "hrms", "staffing", "system", "workspace", 0, "Timesheets", "⏱", "slate", timesheets),
    template!("hrms.onboarding", "hrms", "staffing", "system", "workspace", 0, "Onboarding", "🧭", "plum", onboarding),
    template!(REQUIREMENTS_KEY, "hrms", "staffing", "system", "workspace", 1, "Candidates & Requirements", "🧩", "postmark", requirements),
    template!("hrms.payroll", "hrms", "staffing", "system", "workspace", 0, "Payroll queries", "💼", "teal", payroll),
    template!(PERSONAL_KEY, "personal", "general", "system", "personal", 1, "My assistant", "🙋", "sage", mailbox),
];

pub fn template(key: &str) -> Option<&'static AgentTemplate> {
    TEMPLATES.iter().find(|t| t.key == key)
}

pub fn lead_template() -> &'static AgentTemplate {
    template(LEAD_KEY).expect("the lead template exists")
}

pub fn personal_template() -> &'static AgentTemplate {
    template(PERSONAL_KEY).expect("the personal template exists")
}

/// Desks these packs bring (not the lead, not personal agents), in catalogue order.
pub fn for_packs<'a>(packs: &'a [&str]) -> impl Iterator<Item = &'static AgentTemplate> + 'a {
    TEMPLATES.iter().filter(move |t| t.kind == "system" && t.scope == "workspace" && packs.contains(&t.pack))
}

pub fn category_name(category: &str) -> &'static str {
    match category {
        "general" => "General",
        "support" => "Support",
        "sales" => "Sales",
        "finance" => "Finance",
        "personal" => "Personal",
        "hrms" => "HRMS",
        _ => "Custom",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{agent_registry::spec::validate_basic, packs};

    #[test]
    fn every_template_is_valid_and_consistent() {
        let all_labels: Vec<&str> = packs::PACKS.iter().flat_map(|p| p.labels.iter().map(|l| l.key)).collect();
        for t in TEMPLATES {
            let spec = t.spec();
            assert!(validate_basic(&spec).is_empty(), "{}: {:?}", t.key, validate_basic(&spec));
            assert_eq!(spec.category, t.category, "{}", t.key);
            assert_eq!(spec.name, t.name, "{}", t.key);
            assert!(packs::pack(t.pack).is_some(), "{}", t.key);
            assert!(t.initial_level <= i64::from(spec.permissions.max_autonomy), "{}", t.key);
            for trigger in &spec.triggers {
                if let Trigger::EmailReceived { matcher } = trigger {
                    for l in &matcher.labels {
                        assert!(all_labels.contains(&l.as_str()), "{}: unknown label {l}", t.key);
                    }
                }
            }
            if t.kind == "system" || t.kind == "lead" {
                assert!(matches!(t.scope, "workspace" | "personal"));
            }
        }
        let mut keys: Vec<&str> = TEMPLATES.iter().map(|t| t.key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), TEMPLATES.len(), "template keys are unique");
        let mut spec_keys: Vec<String> = TEMPLATES.iter().map(|t| t.spec().key).collect();
        spec_keys.sort();
        spec_keys.dedup();
        assert_eq!(spec_keys.len(), TEMPLATES.len(), "spec keys are unique");
    }

    #[test]
    fn rosters_per_pack() {
        let names = |packs: &[&str]| for_packs(packs).map(|t| t.key).collect::<Vec<_>>();
        assert_eq!(names(&["general"]), ["general.inbox"]);
        assert_eq!(names(&["support"]), ["support.resolver", "support.escalation"]);
        assert_eq!(names(&["sales"]), ["sales.qualifier", "sales.followups"]);
        assert_eq!(names(&["finance"]), ["finance.invoices"]);
        assert_eq!(names(&["personal"]), ["personal.scheduling"]);
        assert_eq!(names(&["staffing"]), ["hrms.leave", "hrms.timesheets", "hrms.onboarding", "hrms.requirements", "hrms.payroll"]);
        assert_eq!(template(LEAD_KEY).unwrap().kind, "lead");
        assert_eq!(template(PERSONAL_KEY).unwrap().scope, "personal");
        assert_eq!(template("support.escalation").unwrap().max_autonomy(), 1, "never sends on its own");
        assert_eq!(template("hrms.requirements").unwrap().spec().permissions.send, SendMode::Ask, "submissions always ask");
    }

    #[test]
    fn templates_match_the_migration_backfill() {
        let lead = template(LEAD_KEY).unwrap();
        let personal = template(PERSONAL_KEY).unwrap();
        assert_eq!((lead.name, lead.avatar, lead.color), ("Meera", "🌼", "ochre"));
        assert_eq!((personal.name, personal.avatar, personal.color), ("My assistant", "🙋", "sage"));
        assert_eq!(lead.spec().description, "Routes mail, answers what no desk owns and keeps the team's voice.");
        assert_eq!(personal.spec().description, "Works on your own mailbox, in your voice, using your profile and preferences.");
    }
}
