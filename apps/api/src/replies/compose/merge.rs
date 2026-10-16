//! Merging the requests of one message into the replies Meera writes. Pure: requests in, replies out. One recipient gets
//! one reply; its kind is the highest-priority kind present; its sections run corrections, questions, answers, then
//! acknowledgements, each headed by the job in plain words when more than one job speaks. Facts and issues repeated by
//! another request are said once.

use crate::agents::handoff::{Author, Fact, IssueText, MessageKind, Recipient, Stored, governing_level};

#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub job: String,
    pub kind: MessageKind,
    pub facts: Vec<Fact>,
    pub issues: Vec<IssueText>,
    pub instructions: Option<String>,
    pub body: Option<String>,
}

/// Who asked for part of a reply.
#[derive(Debug, Clone, PartialEq)]
pub struct Asked {
    pub request_id: String,
    pub agent_id: String,
    pub job: String,
    pub kind: MessageKind,
    pub level: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Merged {
    pub to: Recipient,
    pub kind: MessageKind,
    pub sections: Vec<Section>,
    pub asked: Vec<Asked>,
}

impl Merged {
    /// Words from a template and typed facts only: nobody wrote free text, so no model is needed.
    pub fn is_template(&self) -> bool {
        self.sections.iter().all(|s| s.kind.is_template() && s.body.is_none() && s.instructions.is_none())
    }

    pub fn author(&self) -> Author {
        if self.sections.iter().any(|s| s.body.is_some()) {
            Author::Requester
        } else if self.is_template() {
            Author::Template
        } else {
            Author::Meera
        }
    }

    /// The level this reply is sent at; `meera_level` only matters for text she writes with a model.
    pub fn governing_level(&self, meera_level: i64) -> i64 {
        governing_level(&self.asked.iter().map(|a| a.level).collect::<Vec<_>>(), self.author(), meera_level)
    }
}

/// How a job is named to the correspondent: never an agent's name.
pub fn job_words(job: &str) -> &'static str {
    if job.contains("timesheet") { "Your timesheet" } else { "Your message" }
}

pub fn merge(requests: &[Stored]) -> Vec<Merged> {
    let mut groups: Vec<(Recipient, Vec<&Stored>)> = vec![];
    for r in requests {
        let to = r.recipient();
        match groups.iter_mut().find(|(t, _)| *t == to) {
            Some((_, list)) => list.push(r),
            None => groups.push((to, vec![r])),
        }
    }
    groups.into_iter().map(|(to, list)| merge_group(to, &list)).collect()
}

fn merge_group(to: Recipient, list: &[&Stored]) -> Merged {
    let has_real = list.iter().any(|r| r.kind() != MessageKind::Holding);
    let mut ordered: Vec<&Stored> = list.iter().copied().filter(|r| !has_real || r.kind() != MessageKind::Holding).collect();
    ordered.sort_by_key(|r| std::cmp::Reverse(r.kind().priority()));
    let kind = ordered.first().map_or(MessageKind::Holding, |r| r.kind());
    let (mut seen_facts, mut seen_issues): (Vec<Fact>, Vec<String>) = (vec![], vec![]);
    let mut sections: Vec<Section> = vec![];
    for r in &ordered {
        let facts: Vec<Fact> = r.facts().into_iter().filter(|f| !seen_facts.contains(f)).collect();
        let issues: Vec<IssueText> = r.issues().into_iter().filter(|i| !seen_issues.contains(&i.text)).collect();
        seen_facts.extend(facts.iter().cloned());
        seen_issues.extend(issues.iter().map(|i| i.text.clone()));
        let empty = facts.is_empty() && issues.is_empty() && r.body.is_none() && r.instructions.is_none();
        if empty && r.kind() != MessageKind::Holding && r.kind() != MessageKind::Ack {
            continue;
        }
        sections.push(Section { job: r.job.clone(), kind: r.kind(), facts, issues, instructions: r.instructions.clone(), body: r.body.clone() });
    }
    let asked = ordered
        .iter()
        .map(|r| Asked { request_id: r.id.clone(), agent_id: r.from_agent_id.clone(), job: r.job.clone(), kind: r.kind(), level: r.requester_level })
        .collect();
    Merged { to, kind, sections, asked }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(id: &str, agent: &str, job: &str, kind: MessageKind, facts: &[(&str, &str)], issues: &[&str], level: i64) -> Stored {
        Stored {
            id: id.into(),
            message_id: "m1".into(),
            from_agent_id: agent.into(),
            job: job.into(),
            kind: kind.as_str().into(),
            recipient_json: r#"{"to":"sender"}"#.into(),
            facts_json: serde_json::json!(facts.iter().map(|(l, v)| Fact { label: (*l).into(), value: (*v).into() }).collect::<Vec<_>>()).to_string(),
            issues_json: serde_json::json!(issues.iter().map(|t| IssueText { code: "x".into(), text: (*t).into() }).collect::<Vec<_>>()).to_string(),
            instructions: None,
            body: None,
            requester_level: level,
            status: "pending".into(),
            draft_id: None,
            reply_case_id: None,
        }
    }

    #[test]
    fn highest_kind_wins_and_facts_dedupe() {
        let a = stored("1", "rohan", "desk:timesheet", MessageKind::Ack, &[("Period", "September 2026")], &[], 3);
        let b = stored("2", "ananya", "desk:leave", MessageKind::Correction, &[("Period", "September 2026")], &["Please add the end date."], 2);
        let merged = merge(&[a, b]);
        assert_eq!(merged.len(), 1, "one recipient, one reply");
        let m = &merged[0];
        assert_eq!(m.kind, MessageKind::Correction);
        assert_eq!(m.sections.iter().map(|s| s.kind).collect::<Vec<_>>(), [MessageKind::Correction, MessageKind::Ack], "corrections first");
        assert_eq!(m.sections[0].facts.len(), 1);
        assert!(m.sections[1].facts.is_empty(), "the repeated fact is said once");
        assert_eq!(m.governing_level(0), 2, "the lowest requester");
    }

    #[test]
    fn holding_is_lowest_and_disappears_behind_a_real_message() {
        let holding = stored("1", "a", "j1", MessageKind::Holding, &[], &[], 3);
        let ack = stored("2", "b", "j2", MessageKind::Ack, &[("Period", "Sep")], &[], 3);
        let m = &merge(&[holding.clone(), ack])[0];
        assert_eq!((m.kind, m.sections.len()), (MessageKind::Ack, 1));
        assert_eq!(merge(&[holding])[0].kind, MessageKind::Holding);
    }

    #[test]
    fn different_recipients_get_different_replies() {
        let a = stored("1", "a", "j1", MessageKind::Ack, &[], &[], 3);
        let mut b = stored("2", "b", "j2", MessageKind::Ack, &[], &[], 3);
        b.recipient_json = r#"{"to":"participant","address":"hema@acme.test"}"#.into();
        assert_eq!(merge(&[a, b]).len(), 2);
    }

    #[test]
    fn a_body_written_by_an_expert_is_governed_by_the_expert_and_is_not_a_template() {
        let mut a = stored("1", "a", "j1", MessageKind::Answer, &[], &[], 2);
        a.body = Some("Done.".into());
        let m = &merge(&[a])[0];
        assert!(!m.is_template());
        assert_eq!(m.author(), Author::Requester);
        assert_eq!(m.governing_level(0), 2);
    }

    #[test]
    fn a_job_is_named_in_plain_words() {
        assert_eq!(job_words("desk:timesheet"), "Your timesheet");
        assert_eq!(job_words("main"), "Your message");
    }
}
