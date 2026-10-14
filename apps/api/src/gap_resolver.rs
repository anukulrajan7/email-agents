//! Fills `[NEEDS: …]` gaps from data the system can read before a person is asked.
//! "Any openings?" questions are answered from the open requirements (staffing). Whatever is left is
//! rewritten so nobody sees a blank and no model text reaches outgoing mail: questions for the sender
//! become one clear question, facts the system can't know become "we'll confirm the <vetted topic> shortly",
//! and the topics are kept as an internal flag (`drafts.unanswered_json`).

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
    if MONEY_WORDS.iter().any(|w| contains_phrase(&c, w)) {
        return GapKind::Other;
    }
    if OPENING_WORDS.iter().any(|w| c.contains(w)) { GapKind::JobOpenings } else { GapKind::Other }
}

const JOB_SEEKER_LABELS: &[&str] = &["candidate_referral", "job_seeker", "candidate"];
const WORK_WORDS: &[&str] = &["role", "roles", "opportunit", "placement", "position", "job", "work for", "requirement", "vacanc", "hiring"];

/// The understanding step labelled the email as a job seeker's: a gap about work or roles is an openings question
/// even when it uses none of the keywords (never for money or dates).
pub fn job_seeker_asks_for_work(category: &str, question: &str, line: &str) -> bool {
    if !JOB_SEEKER_LABELS.contains(&category) {
        return false;
    }
    let c = format!("{question} {line}").to_lowercase();
    !MONEY_WORDS.iter().any(|w| contains_phrase(&c, w)) && WORK_WORDS.iter().any(|w| c.contains(w))
}

const GENERIC_QUESTIONS: &[&str] = &["answer", "answers", "details", "info", "it", "this", "that", "reply"];

/// Classifies the gap by the model's own question first; the surrounding line only when that is too vague.
pub fn classify_gap(question: &str, line: &str) -> GapKind {
    let q = question.trim().to_lowercase();
    if q.split_whitespace().count() >= 2 || (!q.is_empty() && !GENERIC_QUESTIONS.contains(&q.as_str())) {
        classify(&q)
    } else {
        classify(line)
    }
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

/// The sentence a job seeker gets when nothing is open. Its presence in a sent reply starts the candidate watch.
pub const NONE_PHRASE: &str = "We don't have a matching opening right now";

fn answer(found: &[&Opening]) -> String {
    if found.is_empty() {
        return format!("Thanks for sharing your profile. {NONE_PHRASE}; we've saved your details and, as soon as a matching role comes in, we'll send it to you.");
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

static FILLER: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\s*\b(?:we|the team)(?: will|'ll)? (?:review|look into|check)[^.!?\n]*get back to you[^.!?\n]*[.!?]").expect("valid regex")
});

/// Drops "we will review what we have and get back to you" once the draft gave a real answer.
pub fn strip_filler(body: &str) -> String {
    FILLER.replace_all(body, "").to_string()
}

/// Rewrites job-opening gaps using `openings`. Returns the new body and the ids it cited.
pub fn resolve_text(body: &str, email_text: &str, openings: &[Opening], category: &str) -> (String, Vec<String>) {
    let mut text = body.to_string();
    let mut cited = Vec::new();
    let mut from = 0;
    while let Some(rel) = text[from..].find(OPEN) {
        let start = from + rel;
        let Some(close) = text[start..].find(']') else { break };
        let end = start + close + 1;
        let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
        let line_end = text[end..].find('\n').map_or(text.len(), |i| end + i);
        let line = text[line_start..line_end].to_string();
        let question = text[start + OPEN.len()..end - 1].trim().to_string();
        let found = pick_openings(openings, &line, email_text);
        if (classify_gap(&question, &line).answerable_by_none() || job_seeker_asks_for_work(category, &question, &line)) && (!found.is_empty() || openings.is_empty()) {
            cited.extend(found.iter().map(|o| o.id.clone()));
            let answer = answer(&found);
            let (s, e) = (sentence_start(&text, start), sentence_end(&text, end));
            text.replace_range(s..e, &answer);
            from = s + answer.len();
        } else {
            from = end;
        }
    }
    (text, cited)
}

const HARD_WORDS: &[&str] = &[
    "amount", "invoice", "price", "pricing", "cost", "fee", "rate", "salary", "ctc", "compensation", "budget", "payment", "refund",
    "discount", "date", "deadline", "contract", "legal", "terms", "offer", "penalty", "liability", "guarantee",
];

/// A topic the system must not paper over: an amount, a date or a legal commitment needs a person.
pub fn is_hard(topic: &str) -> bool {
    let t = topic.to_lowercase();
    t.chars().any(|c| c.is_ascii_digit() || matches!(c, '\u{20b9}' | '$' | '\u{20ac}' | '\u{a3}')) || HARD_WORDS.iter().any(|w| contains_phrase(&t, w))
}

/// Parses `drafts.unanswered_json`; anything malformed counts as "something is unconfirmed" (fail closed).
pub fn parse_unanswered(json: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(json).unwrap_or_else(|_| vec!["details that could not be read".to_string()])
}

/// One rewritten gap. `raw` is internal (sanitized model text, for the owner's note); `phrase` is the
/// vetted wording that may appear in mail.
#[derive(Debug, Clone, PartialEq)]
pub struct Gap {
    pub raw: String,
    pub phrase: &'static str,
    pub ask_sender: bool,
}

static EMAIL_OR_URL: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)(https?://\S+|www\.\S+|[a-z0-9._%+-]+@[a-z0-9.-]+\.[a-z]{2,})").expect("valid regex"));
static ASKS_SENDER: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(could you|can you|would you|will you|please (?:share|send|confirm|provide|let us know|tell)|let us know|kindly)\b").expect("valid regex")
});

fn sanitize(raw: &str) -> String {
    let no_links = EMAIL_OR_URL.replace_all(raw, "");
    crate::sources::normalize::truncate_chars(&no_links.split_whitespace().collect::<Vec<_>>().join(" "), 80)
}

/// What we may ask the sender for, in our own words. `None` when the gap names no known essential.
fn sender_phrase(q: &str) -> Option<&'static str> {
    const TABLE: &[(&[&str], &str)] = &[
        (&["location", "city", "work location"], "the location"),
        (&["budget", "rate card", "billing rate"], "the budget"),
        (&["start date", "joining date", "start", "joining", "when"], "the start date"),
        (&["positions", "headcount", "number of", "openings", "how many"], "the number of positions"),
        (&["duration", "contract length"], "the duration"),
        (&["notice period", "notice"], "the notice period"),
        (&["experience", "years"], "the experience required"),
        (&["skills", "tech stack", "technology", "stack"], "the key skills"),
        (&["name", "manager", "approver"], "the name of the right contact"),
        (&["contact", "phone", "email"], "the best way to reach you"),
    ];
    TABLE.iter().find(|(words, _)| words.iter().any(|w| contains_phrase(q, w))).map(|(_, p)| *p)
}

/// What the team will confirm later, in our own words. Digits or currency in the gap become "the amount".
fn internal_phrase(q: &str) -> &'static str {
    const TABLE: &[(&[&str], &str)] = &[
        (&["price", "pricing", "cost", "fee", "rate", "quote"], "the pricing"),
        (&["salary", "ctc", "compensation", "pay"], "the compensation"),
        (&["payment", "invoice", "refund", "discount"], "the payment details"),
        (&["date", "deadline", "timeline", "when", "schedule", "eta"], "the timeline"),
        (&["contract", "terms", "legal", "policy", "sla", "retention"], "the terms"),
        (&["opening", "openings", "vacancy", "role", "position", "job"], "the current openings"),
        (&["availability", "available", "slot"], "the availability"),
    ];
    if q.chars().any(|c| c.is_ascii_digit() || matches!(c, '\u{20b9}' | '$' | '\u{20ac}' | '\u{a3}')) {
        return "the amount";
    }
    TABLE.iter().find(|(words, _)| words.iter().any(|w| contains_phrase(q, w))).map_or("the details", |(_, p)| *p)
}

const ABBREVIATIONS: &[&str] = &["mr", "mrs", "ms", "dr", "prof", "sr", "jr", "vs", "etc", "no", "inc", "ltd", "approx", "st"];

/// True when the `.`/`!`/`?` at `text[at..]` ends a sentence (not "Mr.", "e.g.", an initial or a number).
fn ends_sentence(text: &str, at: usize) -> bool {
    let c = text[at..].chars().next().unwrap_or(' ');
    if !matches!(c, '.' | '!' | '?') {
        return false;
    }
    if !text[at + 1..].chars().next().is_none_or(|n| n.is_whitespace()) {
        return false;
    }
    if c != '.' {
        return true;
    }
    let word: String = text[..at].chars().rev().take_while(|ch| !ch.is_whitespace()).collect::<Vec<_>>().into_iter().rev().collect();
    let lower = word.to_lowercase();
    !(ABBREVIATIONS.contains(&lower.as_str()) || lower.contains('.') || lower.chars().count() == 1 || lower.chars().all(|ch| ch.is_ascii_digit()))
}

/// Start of the sentence that ends at or after `pos` within `text` (a position right after a sentence end, or the line start).
fn sentence_start(text: &str, pos: usize) -> usize {
    let line_start = text[..pos].rfind('\n').map_or(0, |i| i + 1);
    let mut start = line_start;
    for (i, _) in text[line_start..pos].char_indices() {
        if ends_sentence(text, line_start + i) {
            start = line_start + i + 1;
        }
    }
    start += text[start..pos].len() - text[start..pos].trim_start().len();
    start
}

/// End (exclusive) of the sentence containing `pos`, within its line.
fn sentence_end(text: &str, pos: usize) -> usize {
    let line_end = text[pos..].find('\n').map_or(text.len(), |i| pos + i);
    for (i, _) in text[pos..line_end].char_indices() {
        if ends_sentence(text, pos + i) {
            return pos + i + 1;
        }
    }
    line_end
}

fn join_phrases(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => (*one).to_string(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// Rewrites every remaining `[NEEDS: …]` so nobody sees a blank. The whole sentence is dropped; the
/// mail gets "Could you please share <essentials>?" for questions to the sender and
/// "We'll confirm <topic> shortly." for facts only the team knows. Only vetted wording is used.
pub fn soften(body: &str) -> (String, Vec<Gap>) {
    let mut text = body.to_string();
    let mut gaps: Vec<Gap> = Vec::new();
    while let Some((start, end, line)) = next_gap(&text) {
        let bullet = line.trim_start().starts_with(['-', '\u{2022}', '*']);
        let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
        let (from, to) = if bullet {
            (line_start, text[end..].find('\n').map_or(text.len(), |i| end + i + 1))
        } else {
            (sentence_start(&text, start), sentence_end(&text, end))
        };
        let to = to.min(text.len());
        let sentence = text[from..to].to_lowercase();
        let mut region = text[from..to].to_string();
        while let Some((s0, e0, _)) = next_gap(&region) {
            let question = region[s0 + OPEN.len()..e0 - 1].trim().to_string();
            let q_lc = question.to_lowercase();
            let vague = q_lc.split_whitespace().count() < 2 && GENERIC_QUESTIONS.contains(&q_lc.as_str());
            let basis = if vague { line.to_lowercase() } else { q_lc.clone() };
            let asks = ASKS_SENDER.is_match(&sentence);
            let (phrase, ask_sender) = if asks {
                (sender_phrase(&basis).unwrap_or("a few more details"), true)
            } else {
                (internal_phrase(&basis), false)
            };
            let raw = if vague {
                sanitize(line.replace(&region[s0..e0], "").trim_start_matches(['-', '*']).trim().trim_end_matches([':', '?']))
            } else {
                sanitize(&question)
            };
            gaps.push(Gap { raw, phrase, ask_sender });
            region.replace_range(s0..e0, "");
        }
        text.replace_range(from..to, "");
    }
    if gaps.is_empty() {
        return (text, gaps);
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
    let mut asks: Vec<&str> = Vec::new();
    let mut confirms: Vec<&str> = Vec::new();
    for g in &gaps {
        let list = if g.ask_sender { &mut asks } else { &mut confirms };
        if !list.contains(&g.phrase) {
            list.push(g.phrase);
        }
    }
    let mut added = Vec::new();
    if !asks.is_empty() {
        added.push(format!("Could you please share {}?", join_phrases(&asks)));
    }
    if !confirms.is_empty() {
        added.push(format!("We'll confirm {} shortly.", join_phrases(&confirms)));
    }
    let note = added.join(" ");
    let mut paras: Vec<&str> = cleaned.trim().split("\n\n").collect();
    let at = if paras.len() >= 3 { paras.len() - 1 } else { paras.len() };
    paras.insert(at, &note);
    (paras.join("\n\n"), gaps)
}

/// What the resolver may read for one draft.
pub struct Sources<'a> {
    pub ws: &'a str,
    pub staffing: bool,
    pub subject: &'a str,
    pub email_body: &'a str,
    /// The email's label (what the understanding step decided it is), e.g. `candidate_referral` for a job seeker.
    pub category: &'a str,
    /// Staff-written team notes and contact facts the drafter was given (trusted).
    pub notes: &'a [String],
}

const NOTE_STOP: &[&str] = &["what", "which", "when", "where", "your", "from", "with", "that", "this", "have", "about", "does", "need", "needs", "answer"];

fn note_words(text: &str) -> Vec<String> {
    text.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() >= 4 && !NOTE_STOP.contains(w)).map(str::to_string).collect()
}

/// The staff note that answers this gap: it must share at least two meaningful words with the question
/// (one when the question itself has only one or two).
pub fn note_for<'a>(question: &str, notes: &'a [String]) -> Option<&'a String> {
    let q = note_words(question);
    if q.is_empty() {
        return None;
    }
    let need = if q.len() <= 2 { 1 } else { 2 };
    notes
        .iter()
        .map(|n| (n, note_words(n).iter().filter(|w| q.contains(w)).count()))
        .filter(|(_, hits)| *hits >= need)
        .max_by_key(|(_, hits)| *hits)
        .map(|(n, _)| n)
}

/// Fills gaps from the team's own notes and contact facts: the gap's sentence becomes the note.
/// Gaps that ask the sender for something are left alone.
pub fn fill_from_notes(body: &str, notes: &[String]) -> (String, usize) {
    let mut text = body.to_string();
    let mut from = 0;
    let mut filled = 0;
    while let Some(rel) = text[from..].find(OPEN) {
        let start = from + rel;
        let Some(close) = text[start..].find(']') else { break };
        let end = start + close + 1;
        let s = sentence_start(&text, start);
        let e = sentence_end(&text, end);
        let question = text[start + OPEN.len()..end - 1].trim().to_string();
        let asks = ASKS_SENDER.is_match(&text[s..e].to_lowercase());
        let vague = question.split_whitespace().count() < 2 && GENERIC_QUESTIONS.contains(&question.to_lowercase().as_str());
        let basis = if vague { text[s..e].replace(&text[start..end], "") } else { question };
        match (!asks).then(|| note_for(&basis, notes)).flatten() {
            Some(note) => {
                let mut fact = note.trim().trim_end_matches('.').to_string();
                fact.push('.');
                text.replace_range(s..e, &fact);
                from = s + fact.len();
                filled += 1;
            }
            None => from = end,
        }
    }
    (text, filled)
}

/// The pure part of the pipeline, in order: job openings, filler removal, team notes, then the rewrite of
/// whatever is left. Returns the final text, the gaps it could not answer and how many it resolved.
pub fn finalize_text(body: &str, src: &Sources<'_>, openings: &[Opening]) -> (String, Vec<Gap>, usize) {
    let mut text = body.to_string();
    let mut resolved = 0;
    if src.staffing {
        let (t, _) = resolve_text(&text, &format!("{}\n{}", src.subject, src.email_body), openings, src.category);
        resolved += text.matches(OPEN).count() - t.matches(OPEN).count();
        text = if resolved > 0 { strip_filler(&t) } else { t };
    }
    let (t, from_notes) = fill_from_notes(&text, src.notes);
    resolved += from_notes;
    let (t, gaps) = soften(&t);
    (t, gaps, resolved)
}

/// Resolves what it can in a fresh draft, then rewrites what is left so no blank is ever shown.
/// Job-opening answers are staffing only: elsewhere "no openings" isn't something the system knows.
pub async fn resolve_draft(db: &SqlitePool, src: &Sources<'_>, draft: &mut crate::ai::DraftOutput) -> sqlx::Result<usize> {
    if !draft.body_text.contains(OPEN) {
        return Ok(0);
    }
    let openings: Vec<Opening> = if src.staffing {
        sqlx::query_as(
            "SELECT id, role_title, location, must_have_skills_json AS skills_json FROM requirements
             WHERE workspace_id = ?1 AND status = 'open' ORDER BY created_at DESC LIMIT 100",
        )
        .bind(src.ws)
        .fetch_all(db)
        .await?
    } else {
        vec![]
    };
    let (body, gaps, resolved) = finalize_text(&draft.body_text, src, &openings);
    if resolved > 0 {
        tracing::info!(resolved, "draft gaps filled from open requirements and team notes");
    }
    draft.body_text = body;
    draft.unanswered = gaps.into_iter().map(|g| g.raw).collect();
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
        let (body, cited) = resolve_text(ASK, "Looking for a job", &o, "");
        assert!(!body.contains("[NEEDS") && body.contains("React Native Developer (Pune)") && !body.contains("Java Developer"), "{body}");
        assert!(body.contains("We currently have an open role that may fit"), "{body}");
        assert_eq!(cited, ["req_1"]);
    }

    #[test]
    fn says_none_right_now_instead_of_asking() {
        let (body, cited) = resolve_text(ASK, "I know Java too", &[], "");
        assert!(!body.contains("[NEEDS") && body.contains("don't have a matching opening right now"), "{body}");
        assert!(cited.is_empty());
    }

    #[test]
    fn prices_and_dates_are_never_invented_or_shown_as_blanks() {
        let body = "Hi,\n\nThe fee for the premium plan is [NEEDS: price of the premium plan]. Thanks for asking.\n\nBest regards,\nHR";
        let (out, _) = resolve_text(body, "", &[], "");
        assert_eq!(out, body, "the resolver leaves facts it cannot know");
        assert_eq!(classify("hiring date [NEEDS: start date]"), GapKind::Other);
        let (soft, gaps) = soften(&out);
        assert!(!soft.contains("[NEEDS") && !soft.contains("The fee") && !soft.contains("premium"), "no model text in mail: {soft}");
        assert!(soft.contains("We'll confirm the pricing shortly.") && soft.contains("Thanks for asking."), "{soft}");
        assert!(soft.ends_with("Best regards,\nHR"), "{soft}");
        assert_eq!(gaps.len(), 1);
        assert!(is_hard(&gaps[0].raw) && !is_hard("office location") && !is_hard("candidate profile"));
    }

    #[test]
    fn questions_to_the_sender_stay_questions() {
        let body = "Hello,\n\nThanks for the requirement. Could you share the [NEEDS: location] and the [NEEDS: budget]?\n\nBest regards,\nHR";
        let (soft, gaps) = soften(body);
        assert!(soft.contains("Could you please share the location and the budget?"), "{soft}");
        assert!(!soft.contains("confirm") && gaps.iter().all(|g| g.ask_sender), "{soft}");
    }

    #[test]
    fn model_text_never_reaches_the_mail() {
        let body = "Hello,\n\nSend the invoice to boss@evil.test, see https://evil.test/x for Rs 1,200 details [NEEDS: wire 1200 to account 99-99 at boss@evil.test].\n\nBest regards,\nHR";
        let (soft, gaps) = soften(body);
        assert!(!soft.contains("evil") && !soft.contains("1200") && !soft.contains("99-99"), "{soft}");
        assert!(soft.contains("We'll confirm the amount shortly."), "{soft}");
        assert!(!gaps[0].raw.contains("evil") && is_hard(&gaps[0].raw));
    }

    #[test]
    fn abbreviations_do_not_split_the_sentence() {
        let body = "Hello,\n\nMr. Rao said e.g. a Friday start, and the rate is [NEEDS: rate]. Thanks.\n\nBest regards,\nHR";
        let (soft, _) = soften(body);
        assert!(!soft.contains("Mr.") && !soft.contains("Friday") && soft.contains("Thanks."), "{soft}");
    }

    #[test]
    fn bullet_questions_become_one_confirm_line() {
        let body = "Hello,\n\nOn your questions:\n- What is the price? [NEEDS: answer]\n\nBest regards,\nHR";
        let (soft, gaps) = soften(body);
        assert!(!soft.contains("[NEEDS") && !soft.contains("On your questions:") && soft.contains("We'll confirm the pricing shortly."), "{soft}");
        assert_eq!(gaps.len(), 1);
    }

    #[test]
    fn a_closed_door_is_not_claimed_when_openings_exist() {
        let o = [opening("req_2", "Java Developer", &["Java"])];
        let (kept, _) = resolve_text(ASK, "I know Java too", &o, "");
        assert!(kept.contains("[NEEDS"), "left for approval, not denied: {kept}");
        let (none, _) = resolve_text(ASK, "x", &[], "");
        assert!(none.contains(NONE_PHRASE) || none.contains("we don't have a matching opening"), "{none}");
    }

    #[test]
    fn malformed_flags_fail_closed() {
        assert_eq!(parse_unanswered("[]"), Vec::<String>::new());
        assert!(!parse_unanswered("{oops").is_empty());
    }

    #[test]
    fn team_notes_answer_gaps_before_anyone_is_asked() {
        let notes = vec!["Data retention period: we keep your data for 90 days after cancellation".to_string(), "Office hours are 9 to 6".to_string()];
        let body = "Hello,\n\nWe keep your data for [NEEDS: retention period after cancellation].\n\nBest regards,\nHR";
        let (out, n) = fill_from_notes(body, &notes);
        assert_eq!(n, 1);
        assert!(out.contains("we keep your data for 90 days after cancellation.") && !out.contains("[NEEDS"), "{out}");
        let (same, none) = fill_from_notes("Hi,\n\nThe price is [NEEDS: price of the premium plan].\n\nBest,\nHR", &notes);
        assert_eq!(none, 0);
        assert!(same.contains("[NEEDS"));
        let ask = "Hello,\n\nCould you share the [NEEDS: office hours]?\n\nBest,\nHR";
        assert_eq!(fill_from_notes(ask, &notes).1, 0, "questions to the sender are never answered from our notes");
    }

    #[test]
    fn a_job_seeker_label_makes_a_work_gap_an_openings_question() {
        assert!(job_seeker_asks_for_work("candidate_referral", "roles that suit a React dev", ""));
        assert!(!job_seeker_asks_for_work("candidate_referral", "salary for the role", ""));
        assert!(!job_seeker_asks_for_work("timesheet", "roles that suit a React dev", ""));
        let body = "Hello,\n\nYou asked about roles for you: [NEEDS: suitable roles].\n\nBest,\nHR";
        let (out, _) = resolve_text(body, "x", &[], "candidate_referral");
        assert!(out.contains("matching opening right now") || out.contains("matching opening"), "{out}");
    }
}
