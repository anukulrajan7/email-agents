//! Turns a merged reply into text when it is made only of template kinds. No model call, exact issue wording, and the
//! sign-off comes from the profile voice. Anything that needs free words returns `None` and goes to the drafter.

use super::{
    merge::{Merged, Section, job_words},
    templates,
};
use crate::agents::handoff::MessageKind;

fn section_text(s: &Section) -> Option<String> {
    Some(match s.kind {
        MessageKind::Ack => templates::ack(&s.facts),
        MessageKind::Correction => templates::correction(&s.issues),
        MessageKind::Question if s.issues.iter().any(|i| i.code == crate::agents::handoff::MISSING_DATA) => templates::ask_missing(&s.facts),
        MessageKind::Question => templates::question(&s.issues),
        MessageKind::Holding => templates::holding(&s.facts),
        MessageKind::Answer | MessageKind::Custom => return None,
    })
}

const CLOSINGS: [&str; 7] = ["best", "regards", "thanks", "thank you", "sincerely", "cheers", "warm"];

/// "Best regards," above the sign-off, unless the sign-off already is a closing line.
fn closing(sign_off: &str) -> String {
    let sign_off = sign_off.trim();
    let lower = sign_off.to_lowercase();
    if CLOSINGS.iter().any(|c| lower.starts_with(c)) { sign_off.to_string() } else { format!("Best regards,\n{sign_off}") }
}

/// The reply text, or `None` when its words are not all template.
pub fn render_template(m: &Merged, first_name: Option<&str>, sign_off: &str) -> Option<String> {
    if !m.is_template() || m.sections.is_empty() {
        return None;
    }
    let greeting = match first_name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => format!("Hi {n},"),
        None => "Hello,".to_string(),
    };
    let headed = m.sections.len() > 1;
    let mut parts = vec![greeting];
    for s in &m.sections {
        let text = section_text(s)?;
        parts.push(if headed { format!("{}:\n{text}", job_words(&s.job)) } else { text });
    }
    parts.push(closing(sign_off));
    Some(parts.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::handoff::{Fact, IssueText, Recipient};
    use crate::replies::compose::merge::Asked;

    fn merged(kind: MessageKind, facts: Vec<Fact>, issues: Vec<IssueText>) -> Merged {
        Merged {
            to: Recipient::Sender,
            kind,
            sections: vec![Section { job: "desk:timesheet".into(), kind, facts, issues, instructions: None, body: None }],
            asked: vec![Asked { request_id: "r".into(), agent_id: "a".into(), job: "desk:timesheet".into(), kind, level: 3 }],
        }
    }

    #[test]
    fn an_ack_reads_as_one_short_mail_signed_from_the_profile() {
        let m = merged(MessageKind::Ack, vec![Fact { label: "Period".into(), value: "September 2026".into() }, Fact { label: "Hours".into(), value: "23.5 h".into() }], vec![]);
        let text = render_template(&m, Some("Ravi"), "Priya, HR Operations").unwrap();
        assert_eq!(text, "Hi Ravi,\n\nThank you. We have received your timesheet for September 2026 (23.5 h) and recorded it.\n\nBest regards,\nPriya, HR Operations");
        assert_eq!(render_template(&m, None, "Regards, Priya").unwrap().lines().next(), Some("Hello,"));
        assert!(render_template(&m, None, "Regards, Priya").unwrap().ends_with("Regards, Priya"));
    }

    #[test]
    fn words_that_are_not_template_are_not_rendered() {
        let mut m = merged(MessageKind::Answer, vec![], vec![]);
        m.sections[0].body = Some("Hello".into());
        assert_eq!(render_template(&m, None, "X"), None);
    }

    #[test]
    fn two_jobs_get_plain_headings_never_an_agent_name() {
        let mut m = merged(MessageKind::Correction, vec![], vec![IssueText { code: "c".into(), text: "Please resend.".into() }]);
        m.sections.push(Section { job: "desk:timesheet".into(), kind: MessageKind::Ack, facts: vec![Fact { label: "Period".into(), value: "Sep".into() }], issues: vec![], instructions: None, body: None });
        let text = render_template(&m, None, "Team").unwrap();
        assert!(text.contains("Your timesheet:\nThank you for sending") && !text.contains("Rohan"), "{text}");
    }

    #[test]
    fn no_template_body_carries_money_or_a_sensitive_word() {
        let ack = merged(MessageKind::Ack, vec![Fact { label: "Period".into(), value: "September 2026".into() }, Fact { label: "Hours".into(), value: "168 h".into() }], vec![]);
        let correction = merged(MessageKind::Correction, vec![], vec![IssueText { code: "wrong_period".into(), text: "Your timesheet is for August 2026, but we expected September 2026. Please send the one for September 2026.".into() }]);
        let question = merged(MessageKind::Question, vec![], vec![IssueText { code: "portal_mismatch".into(), text: "Your sheet says 23.5 h but the portal shows 168 h for this period. Which one is right?".into() }]);
        let holding = merged(MessageKind::Holding, vec![], vec![]);
        let money = regex::Regex::new(r"(?i)[\u{20b9}$\u{20ac}\u{a3}]|\b(inr|usd|eur|rs)\.?\s?\d|\b(rupees?|dollars?|lakhs?)\b").unwrap();
        for m in [ack, correction, question, holding] {
            let text = render_template(&m, Some("Ravi"), "Best regards,\nHR Operations").unwrap();
            assert!(!money.is_match(&text), "no money digits: {text}");
            assert!(!crate::agents::autonomy::is_sensitive(&text), "nothing the floor would stop: {text}");
        }
    }
}
