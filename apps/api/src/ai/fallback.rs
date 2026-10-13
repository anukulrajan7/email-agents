//! Keyword classifier and template drafter used when no model keys are configured
//! (local dev, tests). Results are stored with `source = 'fallback'` so they are never
//! confused with a model decision.

use async_trait::async_trait;

use super::{AiError, Classifier, ClassifyInput, ClassifyOutput, DraftInput, DraftOutput, Drafter};
use crate::{
    domain::{ClassificationSource, OTHER_LABEL, Priority},
    labels,
};

pub struct RulesClassifier;

/// Extra keywords for the built-in template labels. Any other label is matched by the
/// words in its own name.
const KEYWORDS: &[(&str, &[&str])] = &[
    ("timesheet", &["timesheet", "time sheet", "hours worked", "timecard", "time card"]),
    ("leave", &["leave request", "vacation", "annual leave", "sick leave", "time off", "pto", "day off"]),
    ("candidate_referral", &["referral", "refer a", "referring", "candidate", "resume", "cv attached"]),
    ("payroll", &["payslip", "payroll", "salary", "reimbursement", "tax form"]),
    ("approval", &["approval", "approve", "sign off", "sign-off"]),
    ("billing", &["invoice", "receipt", "payment due", "refund", "subscription"]),
    ("sales_lead", &["pricing", "quote", "demo", "interested in buying"]),
    ("scheduling", &["meeting", "reschedule", "calendar", "availability"]),
    ("customer_support", &["not working", "issue with", "complaint", "help with", "error"]),
];

#[async_trait]
impl Classifier for RulesClassifier {
    async fn classify(&self, input: &ClassifyInput) -> Result<ClassifyOutput, AiError> {
        let haystack = format!("{} {} {}", input.subject, input.attachment_names.join(" "), input.body_text).to_lowercase();
        let matches = |key: &str, name: &str| {
            let builtin = KEYWORDS.iter().find(|(k, _)| *k == key).map(|(_, w)| *w).unwrap_or(&[]);
            builtin.iter().any(|w| haystack.contains(w))
                || name.split_whitespace().filter(|w| w.len() >= 4).any(|w| haystack.contains(&w.to_lowercase()))
        };
        let category = input
            .labels
            .iter()
            .filter(|l| l.key != OTHER_LABEL)
            .find(|l| matches(&l.key, &l.name))
            .map(|l| l.key.clone())
            .unwrap_or_else(|| labels::coerce(&input.labels, "general"));
        let priority = if ["urgent", "asap", "immediately", "today"].iter().any(|w| haystack.contains(w)) {
            Priority::High
        } else {
            Priority::Normal
        };
        Ok(ClassifyOutput {
            requires_reply: category != OTHER_LABEL,
            category,
            priority,
            confidence: None,
            source: ClassificationSource::Fallback,
            model_id: Some("rules-v1".into()),
            model_version: None,
        })
    }
}

pub struct TemplateDrafter;

pub fn template_body(input: &DraftInput) -> String {
    let greeting = "Hello,";
    let line = match input.category.as_str() {
        "timesheet" => "Thank you for submitting your timesheet. We have received it and it is now with the HR team for review. We will get back to you if anything needs clarification.",
        "leave" => "Thank you for your leave request. We have received it and will review it shortly.",
        "candidate_referral" => "Thank you for the referral. We have received the candidate's details and the recruiting team will review them.",
        _ => "Thank you for your email. We have received it and the HR team will get back to you.",
    };
    format!("{greeting}\n\n{line}\n\nBest regards,\nHR Operations")
}

#[async_trait]
impl Drafter for TemplateDrafter {
    async fn draft(&self, input: &DraftInput) -> Result<DraftOutput, AiError> {
        Ok(DraftOutput { body_text: template_body(input), model_id: "template-v1".into() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn classifies_timesheet() {
        let out = RulesClassifier
            .classify(&ClassifyInput {
                subject: "September timesheet".into(),
                sender: "a@b.co".into(),
                body_text: "Please review".into(),
                attachment_names: vec![],
                sender_history: None,
                labels: crate::domain::label_template("hr").unwrap().labels.iter().map(|l| crate::domain::Label {
                    key: l.key.into(), name: l.name.into(), description: l.description.into(),
                }).collect(),
                examples: vec![],
            })
            .await
            .unwrap();
        assert_eq!(out.category, "timesheet");
        assert_eq!(out.source, ClassificationSource::Fallback);
    }
}
