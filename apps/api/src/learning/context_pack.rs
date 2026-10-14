//! The verified context pack of one draft: the thread, the writer's profile, the learned preferences and the
//! facts with where they came from. The critic checks the finished draft against it: a number, date, amount or
//! address that is in none of them is not a fact we know, so the sentence becomes a `[NEEDS: ...]` for a person.

use std::{collections::HashSet, sync::LazyLock};

use regex::Regex;

use crate::ai::{DraftInput, DraftOutput};

/// Digits (with `.`, `,`, `:`, `/`, `-` inside), optionally with a currency sign or percent.
static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\p{Sc}]?\d[\d.,:/\-]*\d%?|[\p{Sc}]?\d%?").expect("valid regex"));
static EMAIL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)[\w.+-]+@[\w-]+(?:\.[\w-]+)+").expect("valid regex"));
static LIST_MARKER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*\(?\d{1,2}[.)]\s").expect("valid regex"));
/// Ordinal suffixes: "3rd" and "3" are the same claim.
static ORDINAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(\d)(?:st|nd|rd|th)\b").expect("valid regex"));

fn normalise(token: &str) -> String {
    token.trim_matches(|c: char| !c.is_alphanumeric() && !matches!(c, '%')).replace(',', "").to_lowercase()
}

fn claims_in(line: &str) -> Vec<String> {
    let line = ORDINAL.replace_all(line, "$1");
    let mut out: Vec<String> = EMAIL.find_iter(&line).map(|m| m.as_str().to_lowercase()).collect();
    let without_emails = EMAIL.replace_all(&line, " ");
    out.extend(NUMBER.find_iter(&without_emails).map(|m| normalise(m.as_str())).filter(|t| !t.is_empty()));
    out
}

/// What the writer was given, as plain text with its source.
#[derive(Debug, Clone, Default)]
pub struct ContextPack {
    known: HashSet<String>,
    pub sources: Vec<&'static str>,
}

impl ContextPack {
    /// The pack is exactly what the drafter sees, except past replies: they are style references, not verified facts.
    pub fn from_input(input: &DraftInput) -> Self {
        let mut texts: Vec<(&'static str, String)> = vec![
            ("thread", format!("{}\n{}\n{}", input.subject, input.sender, input.body_text)),
            ("profile", format!("{}\n{}\n{}", input.reply_instructions, input.signature.clone().unwrap_or_default(), input.default_sign_off)),
        ];
        if let Some(i) = &input.insight {
            texts.push(("understanding", format!("{}\n{}\n{}", i.summary, i.asks.join("\n"), i.deadline.clone().unwrap_or_default())));
        }
        if !input.writer_preferences.is_empty() {
            texts.push(("preferences", input.writer_preferences.join("\n")));
        }
        if !input.memory_notes.is_empty() {
            texts.push(("facts", input.memory_notes.join("\n")));
        }
        if let Some(h) = &input.sender_history {
            texts.push(("history", h.clone()));
        }
        let mut known = HashSet::new();
        let mut sources = Vec::new();
        for (source, text) in texts {
            let claims = claims_in(&text);
            let present = text.split_whitespace().next().is_some();
            known.extend(claims);
            if present {
                sources.push(source);
            }
        }
        Self { known, sources }
    }

    /// Numbers, dates, amounts and addresses in `body` that nothing in the pack supports.
    pub fn unsupported(&self, body: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for line in body.lines().filter(|l| !LIST_MARKER.is_match(l) && !l.contains("[NEEDS")) {
            for c in claims_in(line) {
                if !self.known.contains(&c) && !out.contains(&c) {
                    out.push(c);
                }
            }
        }
        out
    }
}

fn sentences(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = line.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        cur.push(*c);
        let end = matches!(c, '.' | '!' | '?') && chars.get(i + 1).is_none_or(|n| n.is_whitespace());
        if end {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// The critic. Every sentence stating something the pack doesn't support is replaced by `[NEEDS: confirm ...]`
/// (also listed in `unanswered`), so the draft cannot be sent until a person fills it in.
/// Returns what was found.
pub fn check_draft(pack: &ContextPack, out: &mut DraftOutput) -> Vec<String> {
    let unsupported = pack.unsupported(&out.body_text);
    if unsupported.is_empty() {
        return unsupported;
    }
    let bad: HashSet<&String> = unsupported.iter().collect();
    let mut rewritten: Vec<String> = Vec::new();
    for line in out.body_text.lines() {
        if LIST_MARKER.is_match(line) || line.contains("[NEEDS") || claims_in(line).iter().all(|c| !bad.contains(c)) {
            rewritten.push(line.to_string());
            continue;
        }
        let mut kept = String::new();
        for sentence in sentences(line) {
            let found: Vec<String> = claims_in(&sentence).into_iter().filter(|c| bad.contains(c)).collect();
            if found.is_empty() {
                kept.push_str(&sentence);
            } else {
                let ws = sentence.len() - sentence.trim_start().len();
                kept.push_str(&format!("{}[NEEDS: confirm {}]", &sentence[..ws], found.join(", ")));
            }
        }
        rewritten.push(kept);
    }
    out.body_text = rewritten.join("\n");
    for c in &unsupported {
        let item = format!("confirm {c}");
        if !out.unanswered.contains(&item) && out.unanswered.len() < 10 {
            out.unanswered.push(item);
        }
    }
    unsupported
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::{DraftQuality, Insight};

    fn input() -> DraftInput {
        DraftInput {
            category: "other".into(),
            category_name: "Other".into(),
            category_description: String::new(),
            subject: "Timesheet for September".into(),
            sender: "ravi@client.com".into(),
            body_text: "Hi, when are timesheets due? Our PO is 4471.".into(),
            memory_notes: vec!["[fact · team · learned] Timesheets are due by the 3rd of each month.".into()],
            writer_preferences: vec!["(length) Keep replies under 80 words".into()],
            sender_history: None,
            insight: Some(Insight { summary: "Asks the due date".into(), asks: vec!["when are timesheets due".into()], ..Default::default() }),
            examples: vec![],
            quality: DraftQuality::Standard,
            reply_instructions: String::new(),
            signature: None,
            default_sign_off: "Acme".into(),
        }
    }

    fn out(body: &str) -> DraftOutput {
        DraftOutput { body_text: body.into(), model_id: "t".into(), unanswered: vec![] }
    }

    #[test]
    fn supported_claims_pass_unchanged() {
        let pack = ContextPack::from_input(&input());
        let mut o = out("Hello Ravi,\n\nTimesheets are due on the 3rd. We have your PO 4471.\n\nAcme");
        assert!(check_draft(&pack, &mut o).is_empty());
        assert!(!o.body_text.contains("NEEDS"));
    }

    #[test]
    fn unsupported_claims_become_needs() {
        let pack = ContextPack::from_input(&input());
        let mut o = out("Hello Ravi,\n\nTimesheets are due on the 3rd. We ship in 5 days. Reach billing@evil.test.\n\nAcme");
        let found = check_draft(&pack, &mut o);
        assert_eq!(found, vec!["billing@evil.test".to_string(), "5".to_string()]);
        assert!(o.body_text.contains("Timesheets are due on the 3rd."), "{}", o.body_text);
        assert!(!o.body_text.contains("5 days") && !o.body_text.contains("Reach"), "{}", o.body_text);
        assert!(o.body_text.contains("[NEEDS: confirm 5]") && o.body_text.contains("[NEEDS: confirm billing@evil.test]"), "{}", o.body_text);
        assert!(crate::replies::has_open_gaps(&o.body_text));
        assert_eq!(o.unanswered, vec!["confirm billing@evil.test", "confirm 5"]);
    }

    #[test]
    fn list_numbering_and_existing_gaps_are_ignored() {
        let pack = ContextPack::from_input(&input());
        let mut o = out("1. First point\n2) Second point\nRate is [NEEDS: budget 9000].");
        assert!(check_draft(&pack, &mut o).is_empty());
    }

    #[test]
    fn records_where_the_pack_came_from() {
        let pack = ContextPack::from_input(&input());
        assert_eq!(pack.sources, vec!["thread", "profile", "understanding", "preferences", "facts"]);
    }
}
