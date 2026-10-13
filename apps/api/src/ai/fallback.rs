//! Keyword classifier and template drafter used when no model keys are configured
//! (local dev, tests). Results are stored with `source = 'fallback'` so they are never
//! confused with a model decision.

use async_trait::async_trait;

use super::{AiError, Classifier, ClassifyInput, ClassifyOutput, DraftInput, DraftOutput, Drafter};
use crate::domain::{Category, ClassificationSource, Priority};

pub struct RulesClassifier;

const RULES: &[(Category, &[&str])] = &[
    (Category::Timesheet, &["timesheet", "time sheet", "hours worked", "timecard", "time card"]),
    (Category::Leave, &["leave request", "vacation", "annual leave", "sick leave", "time off", "pto", "day off"]),
    (Category::CandidateReferral, &["referral", "refer a", "referring", "candidate", "resume", "cv attached"]),
    (Category::Payroll, &["payslip", "payroll", "salary", "reimbursement", "tax form"]),
    (Category::Approval, &["approval", "approve", "sign off", "sign-off"]),
];

#[async_trait]
impl Classifier for RulesClassifier {
    async fn classify(&self, input: &ClassifyInput) -> Result<ClassifyOutput, AiError> {
        let haystack = format!("{} {} {}", input.subject, input.attachment_names.join(" "), input.body_text).to_lowercase();
        let category = RULES
            .iter()
            .find(|(_, words)| words.iter().any(|w| haystack.contains(w)))
            .map(|(c, _)| *c)
            .unwrap_or(Category::General);
        let priority = if ["urgent", "asap", "immediately", "today"].iter().any(|w| haystack.contains(w)) {
            Priority::High
        } else {
            Priority::Normal
        };
        Ok(ClassifyOutput {
            category,
            priority,
            requires_reply: category != Category::Other,
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
    let line = match input.category {
        Category::Timesheet => "Thank you for submitting your timesheet. We have received it and it is now with the HR team for review. We will get back to you if anything needs clarification.",
        Category::Leave => "Thank you for your leave request. We have received it and will review it shortly.",
        Category::CandidateReferral => "Thank you for the referral. We have received the candidate's details and the recruiting team will review them.",
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
            })
            .await
            .unwrap();
        assert_eq!(out.category, Category::Timesheet);
        assert_eq!(out.source, ClassificationSource::Fallback);
    }
}
