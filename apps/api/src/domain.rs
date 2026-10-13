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

str_enum!(Category {
    Timesheet => "timesheet",
    Leave => "leave",
    CandidateReferral => "candidate_referral",
    Approval => "approval",
    Payroll => "payroll",
    General => "general",
    Other => "other",
});

str_enum!(Priority { Low => "low", Normal => "normal", High => "high" });

str_enum!(Role { Admin => "admin", Operator => "operator" });

str_enum!(ClassificationSource { Jev => "jev", Llm => "llm", Manual => "manual", Fallback => "fallback" });

impl Category {
    /// Plain-language definition, also used as the Jev choice criteria.
    pub fn description(self) -> &'static str {
        match self {
            Category::Timesheet => "An employee or contractor submits, corrects or asks about a timesheet or hours worked.",
            Category::Leave => "A request, approval or question about leave, vacation, sick days or time off.",
            Category::CandidateReferral => "Someone refers or recommends a job candidate, or sends a CV for a referral.",
            Category::Approval => "A request for a manager or HR approval that is not leave or timesheet related.",
            Category::Payroll => "Salary, payslips, reimbursements, tax forms or payment issues.",
            Category::General => "A legitimate HR question or notice that fits no specific category above.",
            Category::Other => "Spam, newsletters, automated notifications or anything not HR related.",
        }
    }
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
    pub subject: String,
    pub body_text: String,
    pub body_html: Option<String>,
    pub received_at: i64,
    pub attachments: Vec<AttachmentMeta>,
}

#[derive(Debug, Clone, Default)]
pub struct AttachmentMeta {
    pub filename: String,
    pub media_type: Option<String>,
    pub size_bytes: Option<i64>,
}

pub struct WorkflowTemplate {
    pub key: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub category: Category,
    pub review_kind: &'static str,
    pub review_title: &'static str,
    /// P0 ships only the timesheet template; others are visible but cannot be enabled yet.
    pub available: bool,
}

pub const WORKFLOW_TEMPLATES: &[WorkflowTemplate] = &[
    WorkflowTemplate {
        key: "timesheet_review",
        name: "Timesheet review",
        description: "When an email is classified as a timesheet, create a review item and an acknowledgement draft. Nothing is sent.",
        category: Category::Timesheet,
        review_kind: "check_timesheet",
        review_title: "Check submitted timesheet",
        available: true,
    },
    WorkflowTemplate {
        key: "leave_request",
        name: "Leave request",
        description: "Create a leave review item and an acknowledgement draft.",
        category: Category::Leave,
        review_kind: "check_leave_request",
        review_title: "Review leave request",
        available: false,
    },
    WorkflowTemplate {
        key: "candidate_referral",
        name: "Candidate referral",
        description: "Create a referral review item and a thank-you draft.",
        category: Category::CandidateReferral,
        review_kind: "check_referral",
        review_title: "Review candidate referral",
        available: false,
    },
];

pub fn template(key: &str) -> Option<&'static WorkflowTemplate> {
    WORKFLOW_TEMPLATES.iter().find(|t| t.key == key)
}
