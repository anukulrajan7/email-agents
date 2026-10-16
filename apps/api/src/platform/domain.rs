use serde::{Deserialize, Serialize};

pub const TAXONOMY_VERSION: &str = "taxonomy-v1";

macro_rules! str_enum {
    ($name:ident { $($variant:ident => $s:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name { $(#[serde(rename = $s)] $variant),+ }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];
            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $s),+ }
            }
            pub fn parse(s: &str) -> Option<Self> {
                match s { $($s => Some($name::$variant),)+ _ => None }
            }
        }
    };
}

str_enum!(Priority { Low => "low", Normal => "normal", High => "high" });

str_enum!(Role { Admin => "admin", Operator => "operator" });

str_enum!(ClassificationSource { Jev => "jev", Llm => "llm", Manual => "manual", Fallback => "fallback" });

/// The always-present fallback label. It cannot be archived.
pub const OTHER_LABEL: &str = "other";

/// A workspace label as the AI and the UI see it. `description` is both help text for
/// people and the instruction given to the classifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Label {
    pub key: String,
    pub name: String,
    pub description: String,
}

pub struct LabelSeed {
    pub key: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub color: &'static str,
}

pub struct LabelTemplate {
    pub key: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub labels: &'static [LabelSeed],
}

pub const OTHER_SEED: LabelSeed = LabelSeed {
    key: OTHER_LABEL,
    name: "Other",
    description: "Spam, newsletters, automated notifications, or anything that fits no other label.",
    color: "rule",
};

/// HR / staffing labels (the `hr` template and the `staffing` pack).
pub const HR_LABELS: &[LabelSeed] = &[
    LabelSeed { key: "timesheet", name: "Timesheet", description: "An employee or contractor submits, corrects or asks about a timesheet or hours worked.", color: "slate" },
    LabelSeed { key: "leave", name: "Leave", description: "A request, approval or question about leave, vacation, sick days or time off.", color: "sage" },
    LabelSeed { key: "candidate_referral", name: "Candidate referral", description: "Someone refers or recommends a job candidate, or sends a CV for a referral.", color: "plum" },
    LabelSeed { key: "client_requirement", name: "Client requirement", description: "A client or recruiter shares a new job requirement, open position or job description to be filled.", color: "postmark" },
    LabelSeed { key: "approval", name: "Approval", description: "A request for a manager or HR approval that is not leave or timesheet related.", color: "ochre" },
    LabelSeed { key: "payroll", name: "Payroll", description: "Salary, payslips, reimbursements, tax forms or payment issues.", color: "teal" },
    LabelSeed { key: "general", name: "General HR", description: "A legitimate HR question or notice that fits no specific label above.", color: "ink" },
    OTHER_SEED,
];

/// Business inbox labels (the `general` template and pack).
pub const GENERAL_LABELS: &[LabelSeed] = &[
    LabelSeed { key: "action_required", name: "Action required", description: "The sender is waiting for us to do something specific: approve, sign, send, decide or fix.", color: "postmark" },
    LabelSeed { key: "question", name: "Question", description: "Someone asks a question that needs an answer but no other action.", color: "slate" },
    LabelSeed { key: "customer_support", name: "Customer support", description: "A customer reports a problem, complaint or needs help with our product or service.", color: "plum" },
    LabelSeed { key: "sales_lead", name: "Sales lead", description: "A potential customer asks about pricing, a demo, a quote or buying from us.", color: "sage" },
    LabelSeed { key: "billing", name: "Billing", description: "Invoices, receipts, payment reminders, refunds or subscription changes.", color: "teal" },
    LabelSeed { key: "scheduling", name: "Scheduling", description: "Meeting requests, calendar invites, reschedules or availability questions.", color: "ochre" },
    LabelSeed { key: "fyi", name: "FYI", description: "Updates or announcements worth reading that need no reply.", color: "ink" },
    OTHER_SEED,
];

pub const LABEL_TEMPLATES: &[LabelTemplate] = &[
    LabelTemplate {
        key: "hr",
        name: "HR operations",
        description: "Timesheets, leave, referrals, approvals and payroll for an HR team inbox.",
        labels: HR_LABELS,
    },
    LabelTemplate {
        key: "general",
        name: "General inbox",
        description: "A business inbox: things to act on, questions, customers, leads, billing and noise.",
        labels: GENERAL_LABELS,
    },
    LabelTemplate {
        key: "blank",
        name: "Start blank",
        description: "Only the Other label. Add your own labels with descriptions.",
        labels: &[OTHER_SEED],
    },
];

pub fn label_template(key: &str) -> Option<&'static LabelTemplate> {
    LABEL_TEMPLATES.iter().find(|t| t.key == key)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Capabilities {
    pub read: bool,
    pub sync: bool,
    pub provider_labels: bool,
    pub provider_drafts: bool,
    pub send: bool,
}

impl Capabilities {
    pub fn forwarding() -> Self {
        Self { read: true, sync: false, provider_labels: false, provider_drafts: false, send: false }
    }
}

/// Provider-agnostic message produced by every source adapter.
#[derive(Debug, Clone, Default)]
pub struct NormalizedEmail {
    pub source_message_id: String,
    pub rfc_message_id: Option<String>,
    pub sender_address: String,
    /// Display name from the From header, when present ("Priya Sharma").
    pub sender_name: Option<String>,
    pub original_sender_address: Option<String>,
    pub recipients: Vec<String>,
    /// Cc addresses. For Gmail they are also part of `recipients`.
    pub cc: Vec<String>,
    /// Sent by the connected mailbox itself (Gmail SENT label): stored, never triaged.
    pub outbound: bool,
    pub subject: String,
    pub body_text: String,
    pub body_html: Option<String>,
    pub received_at: i64,
    pub attachments: Vec<AttachmentMeta>,
    /// True when only a provider preview (snippet) is known, not the full body.
    pub preview_only: bool,
    /// Header evidence of bulk or automated mail (`bulk::header_signals`).
    pub bulk_signals: Vec<String>,
    /// The Message-ID this mail answers (`In-Reply-To`), and every earlier one in its chain (`References`).
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    /// What the receiving server concluded about the sender: `{"dmarc": "pass", "spf": "fail"}` (`mail::headers::auth_results`).
    pub auth: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default)]
pub struct AttachmentMeta {
    pub filename: String,
    pub media_type: Option<String>,
    pub size_bytes: Option<i64>,
    /// Provider handle for downloading the bytes later (Gmail attachmentId).
    pub provider_ref: Option<String>,
}

pub struct WorkflowTemplate {
    pub key: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// Label key this workflow runs on.
    pub label_key: &'static str,
    pub review_kind: &'static str,
    pub review_title: &'static str,
    /// Whether the template is implemented. It also needs its label to be active.
    pub available: bool,
}

pub const WORKFLOW_TEMPLATES: &[WorkflowTemplate] = &[
    WorkflowTemplate {
        key: "timesheet_review",
        name: "Timesheet review",
        description: "When an email is labelled Timesheet, create a review item and an acknowledgement draft. Nothing is sent.",
        label_key: "timesheet",
        review_kind: "check_timesheet",
        review_title: "Check submitted timesheet",
        available: true,
    },
    WorkflowTemplate {
        key: "leave_request",
        name: "Leave request",
        description: "When an email is labelled Leave, create a review item and an acknowledgement draft. Nothing is sent.",
        label_key: "leave",
        review_kind: "check_leave_request",
        review_title: "Review leave request",
        available: true,
    },
    WorkflowTemplate {
        key: "candidate_referral",
        name: "Candidate referral",
        description: "When an email is labelled Candidate referral, create a review item and a thank-you draft. Nothing is sent.",
        label_key: "candidate_referral",
        review_kind: "check_referral",
        review_title: "Review candidate referral",
        available: true,
    },
];

pub fn template(key: &str) -> Option<&'static WorkflowTemplate> {
    WORKFLOW_TEMPLATES.iter().find(|t| t.key == key)
}
