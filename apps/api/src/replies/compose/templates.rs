//! The words of a template reply. Every sentence here is fixed; the only variable text is typed facts (the period and
//! the hours of a sheet) and the issue texts the rules wrote, used exactly as they were written.

use crate::agents::handoff::{Fact, IssueText};

fn fact<'a>(facts: &'a [Fact], label: &str) -> Option<&'a str> {
    facts.iter().find(|f| f.label == label).map(|f| f.value.as_str()).filter(|v| !v.trim().is_empty())
}

/// One sheet per `Name`, `Period`, `Hours` run of facts.
fn rows(facts: &[Fact]) -> Vec<Vec<&Fact>> {
    let mut rows: Vec<Vec<&Fact>> = vec![];
    for f in facts {
        if f.label == "Name" || rows.is_empty() {
            rows.push(vec![]);
        }
        if let Some(row) = rows.last_mut() {
            row.push(f);
        }
    }
    rows
}

fn hours_of(row: &[&Fact]) -> Option<String> {
    row.iter().find(|f| f.label == "Hours").map(|f| f.value.clone()).filter(|v| v != "no total" && !v.trim().is_empty())
}

pub fn ack(facts: &[Fact]) -> String {
    let rows = rows(facts);
    if rows.len() > 1 {
        let lines: Vec<String> = rows
            .iter()
            .map(|row| {
                let get = |l: &str| row.iter().find(|f| f.label == l).map(|f| f.value.as_str()).unwrap_or_default();
                let tail = [Some(get("Period").to_string()).filter(|p| !p.is_empty()), hours_of(row)].into_iter().flatten().collect::<Vec<_>>().join(", ");
                format!("- {}: {tail}", get("Name"))
            })
            .collect();
        return format!("Thank you. We have received these timesheets and recorded them:\n\n{}", lines.join("\n"));
    }
    let hours = rows.first().and_then(|r| hours_of(r));
    match (fact(facts, "Period"), hours) {
        (Some(period), Some(hours)) => format!("Thank you. We have received your timesheet for {period} ({hours}) and recorded it."),
        (Some(period), None) => format!("Thank you. We have received your timesheet for {period} and recorded it."),
        _ => "Thank you. We have received your timesheet and recorded it.".to_string(),
    }
}

fn list(issues: &[IssueText]) -> String {
    issues.iter().map(|i| format!("- {}", i.text.trim())).collect::<Vec<_>>().join("\n")
}

pub fn correction(issues: &[IssueText]) -> String {
    match issues {
        [] => "Thank you for sending your timesheet. We could not accept it yet; please send it again.".to_string(),
        [one] => format!("Thank you for sending your timesheet, but we could not accept it yet.\n\n{}", one.text.trim()),
        many => format!("Thank you for sending your timesheet, but we could not accept it yet:\n\n{}", list(many)),
    }
}

pub fn question(issues: &[IssueText]) -> String {
    match issues {
        [] => "Thank you for sending your timesheet. We have a question about it; could you check it and reply?".to_string(),
        [one] => format!("Thank you for sending your timesheet. We have a question before we go on.\n\n{}", one.text.trim()),
        many => format!("Thank you for sending your timesheet. We have a few questions before we go on:\n\n{}", list(many)),
    }
}

pub fn holding() -> String {
    "Thank you for your message. We have it and are looking into it, and we will get back to you.".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(label: &str, value: &str) -> Fact {
        Fact { label: label.into(), value: value.into() }
    }

    #[test]
    fn ack_names_the_period_and_hours_with_no_iso() {
        let text = ack(&[f("Name", "Ravi Kumar"), f("Period", "September 2026"), f("Hours", "23.5 h")]);
        assert_eq!(text, "Thank you. We have received your timesheet for September 2026 (23.5 h) and recorded it.");
        assert!(!regex::Regex::new(r"\d{4}-\d{2}").unwrap().is_match(&text), "{text}");
    }

    #[test]
    fn an_ack_without_a_total_does_not_invent_one() {
        let text = ack(&[f("Name", "Ravi"), f("Period", "September 2026"), f("Hours", "no total")]);
        assert_eq!(text, "Thank you. We have received your timesheet for September 2026 and recorded it.");
        assert!(ack(&[]).contains("received your timesheet"));
    }

    #[test]
    fn several_sheets_are_listed_one_per_person() {
        let text = ack(&[f("Name", "Ravi"), f("Period", "September 2026"), f("Hours", "23.5 h"), f("Name", "Hema"), f("Period", "September 2026"), f("Hours", "20 h")]);
        assert!(text.contains("- Ravi: September 2026, 23.5 h") && text.contains("- Hema: September 2026, 20 h"), "{text}");
    }

    #[test]
    fn correction_lists_each_issue_text_exactly() {
        let one = vec![IssueText { code: "wrong_period".into(), text: "Your timesheet is for August 2026, but we expected September 2026. Please send the one for September 2026.".into() }];
        assert!(correction(&one).ends_with("Please send the one for September 2026."), "{}", correction(&one));
        let two = vec![one[0].clone(), IssueText { code: "over_max_per_day".into(), text: "The sheet has more than 12 hours on 2 Sep.".into() }];
        let text = correction(&two);
        assert!(text.contains("- Your timesheet is for August 2026") && text.contains("- The sheet has more than 12 hours on 2 Sep."), "{text}");
    }

    #[test]
    fn a_question_carries_both_figures_as_written() {
        let q = vec![IssueText { code: "portal_mismatch".into(), text: "Your sheet says 23.5 h but the portal shows 168 h for this period. Which one is right?".into() }];
        let text = question(&q);
        assert!(text.contains("23.5 h") && text.contains("168 h") && text.contains("Which one is right?"), "{text}");
    }
}
