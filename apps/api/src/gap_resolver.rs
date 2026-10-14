//! Fills `[NEEDS: …]` gaps from data the system can read before a person is asked.
//! Today: "any openings?" questions are answered from the open requirements (staffing), or with a
//! natural "none right now". Facts the system can't know (prices, dates, rates) stay `[NEEDS]`.

use sqlx::SqlitePool;

const OPEN: &str = "[NEEDS:";

/// What a gap asks for, and whether "nothing found" is itself a valid answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapKind {
    JobOpenings,
    Other,
}

impl GapKind {
    /// "None found" can be said to the sender without a person.
    pub fn answerable_by_none(self) -> bool {
        matches!(self, GapKind::JobOpenings)
    }
}

const OPENING_WORDS: &[&str] = &[
    "opening", "vacanc", "job posting", "job opportunit", "open position", "open role", "available position", "available role",
    "any jobs", "job openings", "hiring", "current jobs", "current positions",
];
const MONEY_WORDS: &[&str] = &["salary", "rate", "price", "pricing", "cost", "ctc", "fee", "budget", "date", "how much"];
const GENERIC_WORDS: &[&str] = &[
    "job", "jobs", "opening", "openings", "position", "positions", "role", "roles", "vacancy", "vacancies", "current", "currently",
    "available", "any", "there", "are", "you", "have", "work", "open", "posting", "postings", "opportunity", "opportunities",
    "the", "for", "and", "with", "your", "asked", "if", "whether", "is", "a", "an", "of", "to", "in", "on", "any", "needs", "answer",
    "hiring", "now", "right", "company", "team", "me", "my", "i", "do", "does", "please", "list", "details", "about", "which",
];
const TITLE_GENERIC: &[&str] = &["developer", "engineer", "senior", "junior", "lead", "sr", "jr", "the", "a", "software", "programmer"];

pub fn classify(context: &str) -> GapKind {
    let c = context.to_lowercase();
    if MONEY_WORDS.iter().any(|w| c.contains(w)) {
        return GapKind::Other;
    }
    if OPENING_WORDS.iter().any(|w| c.contains(w)) { GapKind::JobOpenings } else { GapKind::Other }
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Opening {
    pub id: String,
    pub role_title: String,
    pub location: Option<String>,
    pub skills_json: String,
}

impl Opening {
    fn skills(&self) -> Vec<String> {
        serde_json::from_str(&self.skills_json).unwrap_or_default()
    }
}

fn contains_phrase(text: &str, phrase: &str) -> bool {
    let phrase = phrase.trim();
    if phrase.len() < 2 {
        return false;
    }
    let mut from = 0;
    while let Some(at) = text[from..].find(phrase) {
        let start = from + at;
        let end = start + phrase.len();
        let before = text[..start].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
        let after = text[end..].chars().next().is_none_or(|c| !c.is_alphanumeric());
        if before && after {
            return true;
        }
        from = start + phrase.len().max(1);
    }
    false
}

fn matches_text(o: &Opening, text_lc: &str) -> bool {
    if o.skills().iter().any(|s| contains_phrase(text_lc, &s.to_lowercase())) {
        return true;
    }
    let title: Vec<String> = o
        .role_title
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '+' && c != '#')
        .filter(|w| !w.is_empty() && !TITLE_GENERIC.contains(w))
        .map(str::to_string)
        .collect();
    !title.is_empty() && contains_phrase(text_lc, &title.join(" "))
}

fn is_generic_ask(context_lc: &str) -> bool {
    !context_lc
        .split(|c: char| !c.is_alphanumeric() && c != '+' && c != '#')
        .any(|w| w.len() > 1 && !GENERIC_WORDS.contains(&w))
}

/// Openings that answer the gap: named skills/titles in the ask, or (for a generic ask) the email at large.
pub fn pick_openings<'a>(openings: &'a [Opening], ask_context: &str, email_text: &str) -> Vec<&'a Opening> {
    let ask = ask_context.to_lowercase();
    let text = if is_generic_ask(&ask) { email_text.to_lowercase() } else { ask };
    openings.iter().filter(|o| matches_text(o, &text)).collect()
}

fn answer(found: &[&Opening]) -> String {
    if found.is_empty() {
        return "Thanks for sharing your profile. We don't have a matching opening right now; we'll keep your details and reach out when one comes up.".into();
    }
    let list: Vec<String> = found
        .iter()
        .take(3)
        .map(|o| match o.location.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
            Some(l) => format!("{} ({l})", o.role_title.trim()),
            None => o.role_title.trim().to_string(),
        })
        .collect();
    format!("We currently have an open role that may fit: {}. Let us know if you'd like us to consider you.", list.join("; "))
}

/// A gap in the body: byte range of the whole `[NEEDS: …]` token plus the line it sits in.
fn next_gap(body: &str) -> Option<(usize, usize, String)> {
    let start = body.find(OPEN)?;
    let end = start + body[start..].find(']')? + 1;
    let line_start = body[..start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = body[end..].find('\n').map_or(body.len(), |i| end + i);
    Some((start, end, body[line_start..line_end].to_string()))
}

/// Rewrites job-opening gaps using `openings`. Returns the new body and the ids it cited.
pub fn resolve_text(body: &str, email_text: &str, openings: &[Opening]) -> (String, Vec<String>) {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    let mut cited = Vec::new();
    while let Some((start, end, line)) = next_gap(rest) {
        out.push_str(&rest[..start]);
        let tail = &rest[end..];
        if classify(&line).answerable_by_none() {
            let found = pick_openings(openings, &line, email_text);
            cited.extend(found.iter().map(|o| o.id.clone()));
            let mut text = answer(&found);
            if out.trim_end().ends_with(':') {
                let mut chars = text.chars();
                text = chars.next().map(|c| c.to_lowercase().collect::<String>() + chars.as_str()).unwrap_or_default();
            }
            out.push_str(&text);
            rest = tail.strip_prefix('.').unwrap_or(tail);
        } else {
            out.push_str(&rest[start..end]);
            rest = tail;
        }
    }
    out.push_str(rest);
    (out, cited)
}

const HARD_WORDS: &[&str] = &[
    "amount", "invoice", "price", "pricing", "cost", "fee", "rate", "salary", "ctc", "budget", "payment", "refund", "discount", "date",
    "deadline", "contract", "legal", "terms", "offer", "penalty", "liability", "guarantee",
];

/// A topic the system must not paper over: an amount, a date or a legal commitment needs a person.
pub fn is_hard(topic: &str) -> bool {
    let t = topic.to_lowercase();
    t == "this" || t.chars().any(|c| c.is_ascii_digit() || matches!(c, '₹' | '$' | '€' | '£'))
        || HARD_WORDS.iter().any(|w| contains_phrase(&t, w))
}

fn topic_of(question: &str, line: &str) -> String {
    let q = question.trim().trim_end_matches('.').trim();
    let q = q.strip_prefix("confirm ").unwrap_or(q);
    if q.split_whitespace().count() >= 2 || (!q.is_empty() && !matches!(q.to_lowercase().as_str(), "answer" | "answers" | "details" | "info" | "it" | "this")) {
        return q.to_string();
    }
    let ask: String = line.replace(['[', ']'], "");
    let ask = ask.split(OPEN.trim_start_matches('[')).next().unwrap_or_default();
    let ask = ask.trim().trim_start_matches(['-', '•', '*']).trim().trim_end_matches([':', '?']).trim();
    if ask.is_empty() { "this".to_string() } else { format!("your question \"{ask}\"") }
}

fn join_topics(topics: &[String]) -> String {
    match topics {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// Rewrites every remaining `[NEEDS: …]` so nobody sees a blank: the sentence is dropped and one
/// "we'll confirm … shortly" line is added. Returns the new body and the topics it could not answer.
pub fn soften(body: &str) -> (String, Vec<String>) {
    let mut text = body.to_string();
    let mut topics: Vec<String> = Vec::new();
    while let Some((start, end, line)) = next_gap(&text) {
        let question = text[start + OPEN.len()..end - 1].to_string();
        let topic = topic_of(&question, &line);
        if !topics.contains(&topic) {
            topics.push(topic);
        }
        let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
        let bullet = line.trim_start().starts_with(['-', '•', '*']);
        let (cut_from, mut cut_to) = if bullet {
            (line_start, text[end..].find('\n').map_or(text.len(), |i| end + i + 1))
        } else {
            let boundary = [". ", "! ", "? "].iter().filter_map(|b| text[line_start..start].rfind(b).map(|i| line_start + i + 1)).max();
            (boundary.map_or(line_start, |b| b + 1), end)
        };
        if text[cut_to..].starts_with('.') {
            cut_to += 1;
        }
        text.replace_range(cut_from..cut_to.min(text.len()), "");
    }
    if topics.is_empty() {
        return (text, topics);
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut kept: Vec<&str> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let orphan_header = l.trim_end().ends_with(':') && lines.get(i + 1).is_none_or(|n| n.trim().is_empty());
        if !orphan_header && !(l.trim().is_empty() && kept.last().is_some_and(|k| k.trim().is_empty())) {
            kept.push(if l.trim().is_empty() { "" } else { l });
        }
    }
    let cleaned = kept.iter().map(|l| l.trim_end()).collect::<Vec<_>>().join("\n");
    let note = format!("We'll confirm {} shortly.", join_topics(&topics));
    let mut paras: Vec<&str> = cleaned.trim().split("\n\n").collect();
    let at = if paras.len() >= 3 { paras.len() - 1 } else { paras.len() };
    paras.insert(at, &note);
    (paras.join("\n\n"), topics)
}

/// Resolves what it can in a fresh draft, then rewrites what is left so no blank is ever shown.
/// Job-opening answers are staffing only: elsewhere "no openings" isn't something the system knows.
pub async fn resolve_draft(
    db: &SqlitePool,
    ws: &str,
    staffing: bool,
    subject: &str,
    email_body: &str,
    draft: &mut crate::ai::DraftOutput,
) -> sqlx::Result<usize> {
    if !draft.body_text.contains(OPEN) {
        return Ok(0);
    }
    let mut resolved = 0;
    if staffing {
        let openings: Vec<Opening> = sqlx::query_as(
            "SELECT id, role_title, location, must_have_skills_json AS skills_json FROM requirements
             WHERE workspace_id = ?1 AND status = 'open' ORDER BY created_at DESC LIMIT 100",
        )
        .bind(ws)
        .fetch_all(db)
        .await?;
        let (body, cited) = resolve_text(&draft.body_text, &format!("{subject}\n{email_body}"), &openings);
        resolved = draft.body_text.matches(OPEN).count() - body.matches(OPEN).count();
        if resolved > 0 {
            tracing::info!(resolved, requirements = ?cited, "draft gaps filled from open requirements");
            draft.body_text = body;
        }
    }
    let (body, topics) = soften(&draft.body_text);
    draft.body_text = body;
    draft.unanswered = topics;
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opening(id: &str, title: &str, skills: &[&str]) -> Opening {
        Opening { id: id.into(), role_title: title.into(), location: Some("Pune".into()), skills_json: serde_json::json!(skills).to_string() }
    }

    const ASK: &str = "Hello,\n\nYou asked if there are any React Native openings: [NEEDS: current React Native job openings]\n\nBest regards,\nHR";

    #[test]
    fn names_a_matching_open_requirement() {
        let o = [opening("req_1", "React Native Developer", &["React Native", "TypeScript"]), opening("req_2", "Java Developer", &["Java"])];
        let (body, cited) = resolve_text(ASK, "Looking for a job", &o);
        assert!(!body.contains("[NEEDS") && body.contains("React Native Developer (Pune)") && !body.contains("Java Developer"), "{body}");
        assert!(body.contains("openings: we currently have"), "{body}");
        assert_eq!(cited, ["req_1"]);
    }

    #[test]
    fn says_none_right_now_instead_of_asking() {
        let o = [opening("req_2", "Java Developer", &["Java"])];
        let (body, cited) = resolve_text(ASK, "I know Java too", &o);
        assert!(!body.contains("[NEEDS") && body.contains("don't have a matching opening right now"), "{body}");
        assert!(cited.is_empty());
    }

    #[test]
    fn prices_and_dates_are_never_invented_or_shown_as_blanks() {
        let body = "Hi,\n\nThe fee for the premium plan is [NEEDS: price of the premium plan]. Thanks for asking.\n\nBest regards,\nHR";
        let (out, _) = resolve_text(body, "", &[]);
        assert_eq!(out, body, "the resolver leaves facts it cannot know");
        assert_eq!(classify("hiring date [NEEDS: start date]"), GapKind::Other);
        let (soft, topics) = soften(&out);
        assert!(!soft.contains("[NEEDS") && !soft.contains("The fee"), "{soft}");
        assert!(soft.contains("We'll confirm price of the premium plan shortly.") && soft.contains("Thanks for asking."), "{soft}");
        assert!(soft.ends_with("Best regards,\nHR"), "{soft}");
        assert_eq!(topics, ["price of the premium plan"]);
        assert!(is_hard(&topics[0]) && !is_hard("office location"));
    }

    #[test]
    fn bullet_questions_become_one_confirm_line() {
        let body = "Hello,\n\nOn your questions:\n- What is the price? [NEEDS: answer]\n\nBest regards,\nHR";
        let (soft, topics) = soften(body);
        assert!(!soft.contains("[NEEDS") && !soft.contains("On your questions:") && soft.contains("your question \"What is the price\""), "{soft}");
        assert_eq!(topics.len(), 1);
    }
}
