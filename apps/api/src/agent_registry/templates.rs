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
        let mut spec = (self.spec)();
        spec.persona = persona(self.key).into();
        spec
    }

    pub fn max_autonomy(&self) -> u8 {
        self.spec().permissions.max_autonomy
    }
}

pub const LEAD_KEY: &str = "general.lead";
pub const INBOX_KEY: &str = "general.inbox";
/// The personal agent is Meera for every member; the web app shows it as "Your Meera" to its owner.
pub const PERSONAL_NAME: &str = "Meera";
/// Names this agent carried before the team got its own names; only untuned agents still holding one are renamed.
pub const OLD_DEFAULT_NAMES: &[(&str, &[&str])] = &[
    ("general.lead", &["Aarav", "Routing"]),
    ("personal.mailbox", &["My assistant", "Meera (personal)"]),
    ("support.resolver", &["Ticket Resolver"]),
    ("support.escalation", &["Escalations"]),
    ("sales.qualifier", &["Lead Qualifier"]),
    ("sales.followups", &["Follow-ups"]),
    ("finance.invoices", &["Invoices & Payments"]),
    ("personal.scheduling", &["Scheduling"]),
    ("hrms.leave", &["Leave"]),
    ("hrms.timesheets", &["Timesheets"]),
    ("hrms.onboarding", &["Onboarding"]),
    ("hrms.requirements", &["Candidates & Requirements"]),
    ("hrms.payroll", &["Payroll queries"]),
];

pub const PERSONAL_KEY: &str = "personal.mailbox";
pub const REQUIREMENTS_KEY: &str = "hrms.requirements";
pub const TIMESHEETS_KEY: &str = "hrms.timesheets";

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
        persona: String::new(),
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
        "find_candidates", "find_requirements", "draft_email", "create_task", "schedule_check", "update_case", "send_email", "submit_candidate", "propose_automation",
        "send_message", "notify_owner",
    ];
    build(
        Base {
            name: "Meera",
            key: "lead",
            category: "general",
            description: "Hands mail to the expert whose job it is and answers, as Meera, what no expert owns.",
            goal: "Resolve each email that no other desk owns: reply, or hand it to a person",
            mode: Mode::Agent,
        },
        vec![Trigger::Manual],
        vec![],
        tools(&every_tool),
        perms(SendMode::Earned, 3),
    )
}

fn inbox() -> AgentSpec {
    build(
        Base {
            name: "Inbox Assistant",
            key: "inbox_assistant",
            category: "general",
            description: "Drafts replies to mail that waits for an answer.",
            goal: "Draft a reply to each email that needs one and follow up if the conversation goes quiet",
            mode: Mode::Workflow,
        },
        vec![Trigger::EmailReceived { matcher: TriggerMatch { needs_reply: Some(true), ..Default::default() } }],
        vec![Step::DraftReply { quality: QualityTier::Balanced }],
        tools(&["find_requirements", "draft_email", "create_task", "schedule_check", "update_case", "send_email"]),
        perms(SendMode::Earned, 2),
    )
}

fn resolver() -> AgentSpec {
    with_drafting(
        build(
            Base {
                name: "Ishaan",
                key: "ticket_resolver",
                category: "support",
                description: "Support expert: answers tickets from what the team knows and, for complaints, stays calm and asks a person to step in.",
                goal: "Resolve each customer question end to end, answering only from known facts, and hand complaints to a person",
                mode: Mode::Agent,
            },
            vec![
                on_labels(&["question", "bug_report", "refund_request", "customer_support", "complaint"]),
                on_keywords(&["escalate", "unacceptable", "legal action"]),
            ],
            vec![],
            tools(&["draft_email", "create_task", "schedule_check", "update_case", "send_email"]),
            perms(SendMode::Earned, 3),
        ),
        &["Acknowledge the problem first", "Give the next step and when to expect it"],
        &["Promise a refund or a date that is not in the notes", "Admit fault or liability", "Offer compensation"],
    )
}

fn escalation() -> AgentSpec {
    with_drafting(
        build(
            Base {
                name: "Diya",
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
    let mut spec = build(
        Base {
            name: "Kabir",
            key: "lead_qualifier",
            category: "sales",
            description: "Sales expert: qualifies new enquiries (budget, timeline, need) and follows up on proposals that went quiet.",
            goal: "Qualify each new sales enquiry, reply, and follow up on conversations that have gone quiet",
            mode: Mode::Agent,
        },
        vec![on_labels(&["sales_lead", "follow_up"]), Trigger::Schedule { every_hours: 24 }],
        vec![],
        tools(&["extract_fields", "draft_email", "create_task", "schedule_check", "update_case", "send_email"]),
        perms(SendMode::Earned, 3),
    );
    spec.drafting.never_do = strings(&["Offer a discount"]);
    spec
}

fn followups() -> AgentSpec {
    let mut spec = build(
        Base {
            name: "Tara",
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
                name: "Vikram",
                key: "invoices_payments",
                category: "finance",
                description: "Finance expert: reads invoices and payment mail, acknowledges them and asks a person to approve. Never promises a payment.",
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
                name: "Nisha",
                key: "scheduling",
                category: "personal",
                description: "Scheduling expert: answers meeting requests with the slots you gave it, and marks anything it does not know.",
                goal: "Reply to each meeting request, proposing slots only when they are known",
                mode: Mode::Workflow,
            },
            vec![on_labels(&["scheduling"]), on_keywords(&["meeting", "reschedule", "availability"])],
            vec![Step::DraftReply { quality: QualityTier::Fast }],
            tools(&["draft_email", "create_task", "update_case", "send_email"]),
            perms(SendMode::Earned, 3),
        ),
        &["Use [NEEDS: slot] when no free time is known"],
        &["Accept a meeting without a known free slot"],
    )
}

fn hr_desk((name, role): (&'static str, &str), key: &'static str, description: &'static str, goal: &'static str, triggers: Vec<Trigger>, send: SendMode, max: u8) -> AgentSpec {
    let extra: &[&str] = if send == SendMode::Earned {
        &["draft_email", "create_task", "update_case", "send_email"]
    } else {
        &["draft_email", "create_task", "update_case"]
    };
    build(
        Base { name, key, category: "hrms", description, goal, mode: Mode::Workflow },
        triggers,
        vec![Step::DraftReply { quality: QualityTier::Balanced }, Step::CreateReview { title: format!("Check: {role}") }],
        tools(extra),
        perms(send, max),
    )
}

fn leave() -> AgentSpec {
    hr_desk(
        ("Ananya", "Leave request"),
        "leave_desk",
        "Handles leave requests and questions about time off. Approvals are always a person's call.",
        "Acknowledge each leave request and prepare it for a manager's decision",
        vec![on_labels(&["leave"])],
        SendMode::Ask,
        1,
    )
}

fn timesheets() -> AgentSpec {
    build(
        Base {
            name: "Rohan",
            key: "timesheets_desk",
            category: "hrms",
            description: "Reads each timesheet, pasted or attached, and keeps it as a record for the right person and period. Never replies.",
            goal: "Record every timesheet for the right person and period, and ask a person to look only when something is unclear",
            mode: Mode::Workflow,
        },
        vec![on_labels(&["timesheet"])],
        vec![Step::RecordTimesheet, Step::CreateReview { title: "Check: Timesheet".into() }],
        tools(&["record_timesheet", "create_task", "update_case"]),
        Permissions { draft: false, send: SendMode::Never, max_autonomy: 3, daily_send_cap: 0 },
    )
}

fn onboarding() -> AgentSpec {
    hr_desk(
        ("Sneha", "Onboarding"),
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
            name: "Arjun",
            key: "candidates_requirements",
            category: "hrms",
            description: "Works a client's job requirement: shortlists candidates, asks for approval, submits and follows up.",
            goal: "Shortlist candidates for each client requirement, get approval, submit and follow up until an outcome",
            mode: Mode::Agent,
        },
        vec![on_labels(&["client_requirement", "candidate_referral"])],
        vec![],
        tools(&["find_candidates", "find_requirements", "draft_email", "create_task", "schedule_check", "update_case", "send_email", "submit_candidate"]),
        perms(SendMode::Ask, 1),
    );
    spec.drafting.quality_tier = QualityTier::Best;
    spec.drafting.never_do = strings(&["Share a candidate's contact details before approval"]);
    spec
}

fn payroll() -> AgentSpec {
    let mut spec = hr_desk(
        ("Aditya", "Payroll question"),
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
            name: PERSONAL_NAME,
            key: "my_assistant",
            category: "personal",
            description: "Works on your own mailbox, in your voice, using your profile and preferences.",
            goal: "Handle the mail in the owner's own mailbox: draft in their voice, hand over what needs them",
            mode: Mode::Workflow,
        },
        vec![Trigger::Manual],
        vec![Step::DraftReply { quality: QualityTier::Balanced }],
        tools(&["draft_email", "create_task", "schedule_check", "update_case", "send_email", "send_message", "notify_owner"]),
        perms(SendMode::Earned, 3),
    );
    spec.memory = MemorySpec { scope: MemoryScope::Own, ..MemorySpec::default() };
    spec
}

macro_rules! template {
    ($key:expr, $cat:expr, $pack:expr, $kind:expr, $scope:expr, $level:expr, $name:expr, $avatar:expr, $color:expr, $spec:expr) => {
        template!($key, $cat, $pack, $kind, $scope, $level, $name, $avatar, $color, $spec, 2)
    };
    ($key:expr, $cat:expr, $pack:expr, $kind:expr, $scope:expr, $level:expr, $name:expr, $avatar:expr, $color:expr, $spec:expr, $version:expr) => {
        AgentTemplate {
            key: $key,
            category: $cat,
            pack: $pack,
            version: $version,
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

/// How each agent thinks. Shapes judgement and checks; outgoing mail is still written in the mailbox owner's voice and signed by them.
pub fn persona(template_key: &str) -> &'static str {
    match template_key {
        LEAD_KEY => "You are Meera, the team's assistant. You look at who is writing and what they need and send the mail to the expert whose job it is. \
            Check first: is this one person's own matter, a client, a vendor or a colleague, and is an expert already on it. When unsure, hand it to a person with a one-line summary rather than guess. \
            Never: reply to something an expert owns, promise dates, amounts or decisions.",
        INBOX_KEY => "You are a careful all-rounder for mail that waits for an answer. Answer the question that was asked, in as few words as it takes. \
            Check first: what exactly is being asked and by when. Never: invent facts or promise anything that is not in the notes.",
        "support.resolver" => "You are Ishaan, the support expert: a patient problem-solver. You restate the customer's problem in one line so they know they were heard, then give the next step and when to expect it. \
            Check first: what the team already knows (notes, earlier threads) before you answer; if the answer is not there, say you are checking and hand it to a person. \
            For a complaint or an angry customer slow down: acknowledge the experience, stay factual, and flag any legal threat, safety issue or key account to a person at the top. \
            Never: guess a cause, promise a refund or a date that is not in the notes, blame the customer, admit fault or liability, or offer compensation.",
        "support.escalation" => "You are Diya, calm under pressure. When someone is upset you slow down: acknowledge the experience first, stay factual, and ask a person to step in before anything else happens. \
            Check first: is there a legal threat, a safety issue or a key account involved; if so, flag it at the top of the review. \
            Never: admit fault or liability, offer compensation, argue, or send anything on your own.",
        "sales.qualifier" => "You are Kabir, the sales expert: curious and to the point. For every new enquiry you work out what they want to achieve, their budget and when they want to start, and you ask the one question that is missing rather than five. \
            When a conversation about a proposal or quote has gone quiet you nudge it with a short note: what was last discussed and one easy next step; if they asked for space, leave it alone. \
            Check first: is this a real buyer (a named company, a clear need) or noise, and how long it has been quiet. Say plainly if it looks like a poor fit. \
            Never: quote prices, offer a discount, promise availability, book a call without a person agreeing, or send more than one nudge in a row.",
        "finance.invoices" => "You are Vikram, the finance expert: exact and watchful. You read every invoice or payment notice line by line, pull out vendor, number, amount and due date as written, and flag anything odd. \
            Check first: does the amount, vendor or bank detail differ from earlier mail, is the invoice a duplicate, is the due date very close. \
            Never: promise a payment date, confirm an amount that is not on the invoice, or approve a payment.",
        "personal.scheduling" => "You are Nisha, the scheduling expert: quick and tidy. You answer meeting requests with the free slots you were given, in the sender's time zone when known, and keep it to two or three lines. \
            Check first: is a free slot actually known; if not, mark it [NEEDS: slot] for the owner. \
            Never: accept a meeting without a known free slot, or share the owner's calendar details beyond free times.",
        "hrms.leave" => "You are Ananya, warm and fair. You treat every leave or HR request as a person's real plan: restate the type and dates, say who will follow up, and ask only for what is missing. \
            Check first: dates, leave type and approver; policy questions get a pointer to the team, not an invented rule. \
            Never: approve, reject or promise leave; state balances, salary, tax or payment dates; give legal or medical advice.",
        "hrms.timesheets" => "You are Rohan, precise and numbers-first. You read each timesheet, pasted or attached, and keep it as a record: who it is for, the period, the hours per day and the total, exactly as written. \
            Check first: who the sheet is for (the name or ID written in it, not who forwarded it), the period, whether it replaces an earlier one, and days that do not add up to the total. \
            Never: reply to the sender, approve or reject hours, mention pay amounts or payment dates, or fill in a number the sheet does not state.",
        "hrms.onboarding" => "You are Sneha, welcoming and checklist-driven. You make a new joiner feel expected and tell them exactly what is still needed, one short list at a time. \
            Check first: which documents and steps are already done, so you never ask twice. \
            Never: share another person's documents, promise a start date or equipment, or skip a step on your own.",
        "hrms.requirements" => "You are Arjun, sharp and fast, written for busy recruiters. For a new requirement you confirm role, location and positions in one line, find the best matches, and ask a person before anything is submitted. \
            Check first: missing essentials (location, budget, start date, positions, interview process) and whether a match above 60 exists; if not, ask for sourcing instead of replying with nothing. \
            Never: commit to a number of profiles or a date, quote or agree rates, say we have no candidates, or share a candidate's contact details before approval.",
        "hrms.payroll" => "You are Aditya, careful, and you never guess amounts. You acknowledge the payroll question, say the payroll owner will check, and note exactly what was asked. \
            Check first: is it a payslip, reimbursement, tax form or a pay dispute, so it reaches the right person. \
            Never: state a salary, rate, balance or payment amount, or promise a payment date.",
        PERSONAL_KEY => "You are Meera, the mailbox owner's own assistant. You know their profile and preferences and you write as them, never as yourself. \
            Check first: who is writing, whether it is a VIP or on the owner's leave-alone list, and whether the owner has to decide. \
            Never: sign with your own name, act on mail the owner said to leave alone, or reveal anything about other mailboxes.",
        _ => "",
    }
}

pub const TEMPLATES: &[AgentTemplate] = &[
    template!(LEAD_KEY, "general", "general", "lead", "workspace", 3, "Meera", "🌼", "ochre", lead, 4),
    template!("support.resolver", "support", "support", "system", "workspace", 3, "Ishaan", "🎧", "teal", resolver, 3),
    template!("sales.qualifier", "sales", "sales", "system", "workspace", 3, "Kabir", "🎯", "sage", qualifier, 3),
    template!("finance.invoices", "finance", "finance", "system", "workspace", 1, "Vikram", "🧾", "teal", invoices, 3),
    template!("personal.scheduling", "personal", "personal", "system", "workspace", 3, "Nisha", "📅", "ochre", scheduling, 3),
    template!("hrms.leave", "hrms", "staffing", "system", "workspace", 0, "Ananya", "🌴", "sage", leave),
    template!("hrms.timesheets", "hrms", "staffing", "system", "workspace", 3, "Rohan", "⏱", "slate", timesheets, 4),
    template!("hrms.onboarding", "hrms", "staffing", "system", "workspace", 0, "Sneha", "🧭", "plum", onboarding),
    template!(REQUIREMENTS_KEY, "hrms", "staffing", "system", "workspace", 1, "Arjun", "🧩", "postmark", requirements),
    template!("hrms.payroll", "hrms", "staffing", "system", "workspace", 0, "Aditya", "💼", "teal", payroll),
    template!(PERSONAL_KEY, "personal", "general", "system", "personal", 3, PERSONAL_NAME, "🙋", "sage", mailbox, 3),
];

pub const ESCALATION_KEY: &str = "support.escalation";
pub const FOLLOWUPS_KEY: &str = "sales.followups";

/// Agents that used to be on the roster. Their jobs belong to an expert now; they are no longer seeded,
/// but a workspace that still has one keeps resolving its template.
pub const RETIRED: &[AgentTemplate] = &[
    template!(INBOX_KEY, "general", "general", "system", "workspace", 1, "Inbox Assistant", "📥", "slate", inbox),
    template!(ESCALATION_KEY, "support", "support", "system", "workspace", 0, "Diya", "🚨", "postmark", escalation),
    template!(FOLLOWUPS_KEY, "sales", "sales", "system", "workspace", 0, "Tara", "🔔", "ochre", followups),
];

/// The agent that took over a retired agent's work: an expert's template key, or the lead.
pub fn merged_into(retired_key: &str) -> Option<&'static str> {
    match retired_key {
        INBOX_KEY => Some(LEAD_KEY),
        ESCALATION_KEY => Some("support.resolver"),
        FOLLOWUPS_KEY => Some("sales.qualifier"),
        _ => None,
    }
}

pub fn template(key: &str) -> Option<&'static AgentTemplate> {
    TEMPLATES.iter().chain(RETIRED).find(|t| t.key == key)
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
        assert!(names(&["general"]).is_empty(), "Meera handles general mail");
        assert_eq!(names(&["support"]), ["support.resolver"]);
        assert_eq!(names(&["sales"]), ["sales.qualifier"]);
        assert_eq!(names(&["finance"]), ["finance.invoices"]);
        assert_eq!(names(&["personal"]), ["personal.scheduling"]);
        assert_eq!(names(&["staffing"]), ["hrms.leave", "hrms.timesheets", "hrms.onboarding", "hrms.requirements", "hrms.payroll"]);
        assert_eq!(template(LEAD_KEY).unwrap().kind, "lead");
        assert_eq!(template(PERSONAL_KEY).unwrap().scope, "personal");
        assert_eq!(template("support.resolver").unwrap().max_autonomy(), 3, "experts act on their own");
        assert!(RETIRED.iter().all(|t| merged_into(t.key).is_some() && !TEMPLATES.iter().any(|x| x.key == t.key)));
        assert_eq!(template("hrms.requirements").unwrap().spec().permissions.send, SendMode::Ask, "submissions always ask");
    }

    #[test]
    fn every_agent_has_its_own_name_and_persona() {
        let names: Vec<&str> = TEMPLATES.iter().map(|t| t.name).collect();
        assert_eq!(
            names,
            ["Meera", "Ishaan", "Kabir", "Vikram", "Nisha", "Ananya", "Rohan", "Sneha", "Arjun", "Aditya", "Meera"]
        );
        let mut distinct = names[1..].to_vec();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct.len(), names.len() - 1, "the lead is Meera too");
        for t in TEMPLATES {
            let p = t.spec().persona;
            assert!(!p.trim().is_empty() && p.chars().count() <= 1200, "{}", t.key);
            assert!(p.contains("Never"), "{}: says what it never does", t.key);
            if t.key != INBOX_KEY {
                assert!(p.contains(&format!("You are {}", t.name)), "{}", t.key);
            }
        }
        assert!(template(LEAD_KEY).unwrap().spec().persona.contains("send the mail"));
        assert!(template("hrms.payroll").unwrap().spec().persona.contains("never guess amounts"));
    }

    #[test]
    fn templates_match_the_migration_backfill() {
        let lead = template(LEAD_KEY).unwrap();
        let personal = template(PERSONAL_KEY).unwrap();
        assert_eq!((lead.name, lead.avatar, lead.color), ("Meera", "🌼", "ochre"));
        assert_eq!((personal.name, personal.avatar, personal.color), ("Meera", "🙋", "sage"));
                assert_eq!(personal.spec().description, "Works on your own mailbox, in your voice, using your profile and preferences.");
    }
}
