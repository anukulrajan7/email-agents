//! Send safety: one check every outgoing reply passes (manual send, automations, agent approvals
//! all go through `replies::send_draft`, and the scheduled send re-checks before Gmail is called).
//!
//! Deterministic checks first (recipient, subject, length, gaps, placeholders, duplicates,
//! attachments, unknown links/phones), then one cheap judge call with three typed questions.
//! `blocking` issues stop the send (409 `send_blocked`); `warnings` are shown to the person.

use std::{collections::HashSet, sync::LazyLock};

use axum::{
    Json,
    extract::{Path, State},
};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::{
    ai::judge::{self, Question},
    auth::{AuthUser, require_role},
    db::now_ms,
    domain::Role,
    error::{AppError, AppResult},
    http::AppState,
    replies::{self, ReplyContext},
};

pub const MIN_BODY_CHARS: usize = 20;
pub const MAX_BODY_CHARS: usize = 4_000;
/// Another reply in the same thread within this window counts as a duplicate.
pub const DUPLICATE_WINDOW_MS: i64 = 10 * 60_000;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Issue {
    pub code: &'static str,
    pub message: String,
    /// What the person can do about it.
    pub fix: Option<String>,
}

fn issue(code: &'static str, message: impl Into<String>, fix: &str) -> Issue {
    Issue { code, message: message.into(), fix: (!fix.is_empty()).then(|| fix.to_string()) }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SafetyReport {
    pub blocking: Vec<Issue>,
    pub warnings: Vec<Issue>,
}

impl SafetyReport {
    pub fn is_blocked(&self) -> bool {
        !self.blocking.is_empty()
    }

    /// 409 `send_blocked` with the report attached.
    pub fn into_error(self) -> AppError {
        let first = self.blocking.first().map(|i| i.message.clone()).unwrap_or_default();
        let details = serde_json::json!({ "blocking": self.blocking, "warnings": self.warnings });
        AppError::SendBlocked(format!("this reply can't be sent yet: {first}"), details)
    }
}

// ---------------------------------------------------------------- deterministic checks

static EMAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9.!#$%&'*+/=?^_`{|}~-]+@[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?(?:\.[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?)+$").expect("valid regex"));
static NO_REPLY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:.*(?:no-?reply|do-?not-?reply|donotreply)|mailer-daemon|postmaster|bounces?|notifications?|notify|newsletters?|alerts?|updates|marketing|news)(?:[.+-].*)?@")
        .expect("valid regex")
});
static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\{\{[^}]*\}?\}?|\{[a-z_][a-z0-9_ ]{0,30}\}|<[a-z_][a-z0-9_ ]{0,30}>|\[[a-z][a-z0-9_ ]{0,30}\]|\bx{3,}\b|\btbd\b")
        .expect("valid regex")
});
static ATTACHMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(?:attached|attachments?|enclosed|pfa)\b").expect("valid regex"));
static LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\b(?:https?://|www\.)[^\s<>()\[\]"']+"#).expect("valid regex"));
static PHONE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\+?\d[\d\s().-]{6,}\d").expect("valid regex"));
static DATE_LIKE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:\d{4}[./-]\d{1,2}[./-]\d{1,2}|\d{1,2}[./-]\d{1,2}[./-]\d{2,4})$").expect("valid regex"));

/// Bare address from `Name <a@b.com>` or `a@b.com`, lower-cased.
pub fn bare_address(raw: &str) -> String {
    let raw = raw.trim();
    let inner = match (raw.rfind('<'), raw.rfind('>')) {
        (Some(s), Some(e)) if s < e => &raw[s + 1..e],
        _ => raw,
    };
    inner.trim().to_lowercase()
}

pub fn valid_address(addr: &str) -> bool {
    addr.len() <= 254 && EMAIL.is_match(addr)
}

pub fn is_automated_address(addr: &str) -> bool {
    NO_REPLY.is_match(addr)
}

/// Everything the deterministic checks need, gathered once.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    /// Where the reply goes (the original sender of the email being answered).
    pub recipient: String,
    /// Our own addresses: the connected mailbox and whoever connected it (lower-case).
    pub own_addresses: Vec<String>,
    pub original_subject: String,
    pub outgoing_subject: String,
    /// The email being answered was sent by us, so a reply would go to ourselves.
    pub replying_to_outbound: bool,
    /// Text the reply may quote links/phones from: the thread, team notes, reply style, signature.
    pub known_text: String,
    /// Someone already replied in this thread within `DUPLICATE_WINDOW_MS`.
    pub recent_reply_in_thread: bool,
    /// Another reply in this thread is scheduled to send.
    pub other_scheduled_in_thread: bool,
}

pub fn check_recipient(f: &Facts) -> Option<Issue> {
    let to = bare_address(&f.recipient);
    let fix = "Reply from Gmail instead, or open the original email from the client.";
    if !valid_address(&to) {
        return Some(issue("invalid_recipient", format!("The recipient address \"{}\" isn't a valid email address.", f.recipient.trim()), fix));
    }
    if f.replying_to_outbound || f.own_addresses.iter().any(|o| bare_address(o) == to) {
        return Some(issue("own_mailbox", format!("This reply would go to your own mailbox ({to})."), "Open the client's email in the thread and reply to that one."));
    }
    if is_automated_address(&to) {
        return Some(issue("automated_recipient", format!("{to} is an automated address; nobody will read the reply."), fix));
    }
    None
}

pub fn check_subject(f: &Facts) -> Option<Issue> {
    let original = f.original_subject.trim();
    let outgoing = f.outgoing_subject.trim();
    if !outgoing.to_lowercase().starts_with("re:") || !outgoing.to_lowercase().contains(&original.to_lowercase()) {
        return Some(issue("subject_mismatch", format!("The subject \"{outgoing}\" doesn't match the original \"{original}\"."), "Reply from the original email so the subject stays \"Re: …\"."));
    }
    None
}

pub fn check_length(body: &str) -> Option<Issue> {
    let n = body.trim().chars().count();
    if n == 0 {
        Some(issue("empty_body", "The reply is empty.", "Write the reply first."))
    } else if n < MIN_BODY_CHARS {
        Some(issue("too_short", format!("The reply is only {n} characters."), "Write at least a full sentence."))
    } else if n > MAX_BODY_CHARS {
        Some(issue("too_long", format!("The reply is {n} characters; the limit is {MAX_BODY_CHARS}."), "Shorten the reply."))
    } else {
        None
    }
}

pub fn check_gaps(body: &str) -> Option<Issue> {
    replies::has_open_gaps(body).then(|| {
        issue("open_gaps", "The reply still has [NEEDS: …] gaps the AI couldn't fill.", "Answer each [NEEDS: …] or remove it.")
    })
}

/// Unresolved template placeholders: `{{x}}`, `{name}`, `<name>`, `[Name]`, `XXX`, `TBD`.
pub fn find_placeholders(body: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    PLACEHOLDER
        .find_iter(body)
        .map(|m| m.as_str().to_string())
        .filter(|m| !m.starts_with("[NEEDS") && seen.insert(m.clone()))
        .take(5)
        .collect()
}

pub fn check_placeholders(body: &str) -> Option<Issue> {
    let found = find_placeholders(body);
    (!found.is_empty()).then(|| {
        issue("placeholder", format!("The reply still has template placeholders: {}.", found.join(", ")), "Replace each placeholder with the real value.")
    })
}

pub fn check_duplicate(f: &Facts) -> Option<Issue> {
    if f.other_scheduled_in_thread {
        return Some(issue("duplicate", "Another reply in this conversation is already scheduled to send.", "Undo that send first, or wait for it."));
    }
    f.recent_reply_in_thread.then(|| {
        issue("duplicate", "Someone already replied in this conversation in the last 10 minutes.", "Read the latest reply in the thread before sending another.")
    })
}

pub fn mentions_attachment(body: &str) -> bool {
    ATTACHMENT.is_match(body)
}

fn normalize_link(link: &str) -> String {
    let l = link.trim_end_matches(['.', ',', ';', ':', '!', '?', ')']).to_lowercase();
    let l = l.trim_start_matches("https://").trim_start_matches("http://").trim_start_matches("www.");
    let l = l.split(['?', '#']).next().unwrap_or_default();
    l.trim_end_matches('/').to_string()
}

fn phones(text: &str) -> Vec<(String, String)> {
    PHONE
        .find_iter(text)
        .map(|m| m.as_str().trim().to_string())
        .filter(|m| !DATE_LIKE.is_match(m))
        .map(|m| {
            let digits: String = m.chars().filter(char::is_ascii_digit).collect();
            (m, digits)
        })
        .filter(|(raw, d)| (8..=15).contains(&d.len()) && (d.len() >= 10 || raw.starts_with('+')))
        .collect()
}

/// Links and phone numbers in the reply that appear nowhere in `known` (thread + team notes).
pub fn unknown_contacts(body: &str, known: &str) -> Vec<String> {
    let known_lower = known.to_lowercase();
    let known_links: Vec<String> = LINK.find_iter(known).map(|m| normalize_link(m.as_str())).collect();
    let mut out = Vec::new();
    for m in LINK.find_iter(body) {
        let n = normalize_link(m.as_str());
        if !n.is_empty() && !known_links.iter().any(|k| k == &n) && !known_lower.contains(&n) && !out.contains(&m.as_str().to_string()) {
            out.push(m.as_str().to_string());
        }
    }
    let known_phones: Vec<String> = phones(known).into_iter().map(|(_, d)| d).collect();
    let tail = |d: &str| d[d.len().saturating_sub(10)..].to_string();
    for (raw, digits) in phones(body) {
        if !known_phones.iter().any(|k| tail(k) == tail(&digits)) && !out.contains(&raw) {
            out.push(raw);
        }
    }
    out
}

/// All deterministic checks. Pure, so every rule is unit-tested.
pub fn deterministic(f: &Facts, body: &str) -> SafetyReport {
    let mut r = SafetyReport::default();
    r.blocking.extend(
        [check_recipient(f), check_subject(f), check_length(body), check_gaps(body), check_placeholders(body), check_duplicate(f)]
            .into_iter()
            .flatten(),
    );
    if mentions_attachment(body) {
        r.warnings.push(issue(
            "attachment_mentioned",
            "The reply mentions an attachment, but replies sent from Post Room carry no files.",
            "Remove the mention, or send the file from Gmail.",
        ));
    }
    let unknown = unknown_contacts(body, &f.known_text);
    if !unknown.is_empty() {
        r.warnings.push(issue(
            "unknown_contact",
            format!("These links or phone numbers aren't in the email or team notes: {}.", unknown.join(", ")),
            "Double-check them, or remove them.",
        ));
    }
    r
}

// ---------------------------------------------------------------- judge review

pub const Q_PROMISES: &str = "promises";
pub const Q_PROFESSIONAL: &str = "professional";
pub const Q_ANSWERS_ASK: &str = "answers_ask";
/// Below this, the reply probably misses the sender's main ask.
const MIN_ANSWER_SCORE: f64 = 0.5;

pub fn review_questions() -> [Question; 3] {
    [
        Question::yes_no(
            Q_PROMISES,
            "Does the <reply> promise approvals, payments, money amounts, dates or decisions that are not stated in the <email> or the <team_notes>?",
        ),
        Question::yes_no(Q_PROFESSIONAL, "Is the tone of the <reply> professional and polite?"),
        Question::score(Q_ANSWERS_ASK, "How well does the <reply> answer the main ask of the <email>? 0 = not at all, 1 = fully."),
    ]
}

/// Maps judge answers to issues. `automated` = no person reviews this send, so a missing review blocks.
pub fn review_issues(answers: Result<&judge::Answers, &str>, automated: bool) -> SafetyReport {
    let mut r = SafetyReport::default();
    let unavailable = |why: &str| {
        issue(
            "ai_review_unavailable",
            format!("The AI review couldn't run ({why}). Read the reply carefully before sending."),
            if automated { "Send it yourself after reading it, or try again later." } else { "" },
        )
    };
    let a = match answers {
        Ok(a) => a,
        Err(why) => {
            if automated { r.blocking.push(unavailable(why)) } else { r.warnings.push(unavailable(why)) }
            return r;
        }
    };
    match a.yes_no(Q_PROMISES) {
        Some(true) => {
            let why = a.reason(Q_PROMISES);
            let why = if why.is_empty() { String::new() } else { format!(" ({why})") };
            if automated {
                r.blocking.push(issue(
                    "unsupported_promise",
                    format!("The reply may promise an approval, payment, date or decision that the email and team notes don't support{why}."),
                    "Remove or soften the promise, or add the fact to team notes first.",
                ));
            } else {
                r.warnings.push(issue(
                    "unsupported_promise",
                    format!("This mentions something the email and your notes don't back up{why} - double-check before sending."),
                    "",
                ));
            }
        }
        Some(false) => {}
        None if automated => r.blocking.push(unavailable("no answer about promises")),
        None => r.warnings.push(unavailable("no answer about promises")),
    }
    if a.yes_no(Q_PROFESSIONAL) == Some(false) {
        r.warnings.push(issue("tone", format!("The tone may not be professional. {}", a.reason(Q_PROFESSIONAL)).trim().to_string(), "Reword the reply."));
    }
    if a.score(Q_ANSWERS_ASK).is_some_and(|s| s < MIN_ANSWER_SCORE) {
        r.warnings.push(issue(
            "misses_ask",
            format!("The reply may not answer what the sender asked. {}", a.reason(Q_ANSWERS_ASK)).trim().to_string(),
            "Check the sender's question and answer it directly.",
        ));
    }
    r
}

// ---------------------------------------------------------------- the check

pub struct CheckInput<'a> {
    pub workspace_id: &'a str,
    pub reply: &'a ReplyContext,
    /// The text to check (the stored draft, or unsaved edits from the send dialog).
    pub body: &'a str,
    /// No person looks at this send (automation, or the system acting alone).
    pub automated: bool,
}

async fn gather(state: &AppState, input: &CheckInput<'_>) -> AppResult<(Facts, Vec<Issue>, String)> {
    let r = input.reply;
    let ws = input.workspace_id;
    let mut warnings = vec![];
    let mut own: Vec<String> = sqlx::query_scalar(
        "SELECT lower(u.email) FROM email_sources s JOIN users u ON u.id = s.connected_by_user_id WHERE s.id = ?1",
    )
    .bind(&r.source_id)
    .fetch_all(&state.db)
    .await?;
    if let Ok((conn, pck)) = replies::connection(r) {
        match replies::mailbox_address(state, conn, pck).await {
            Ok(mailbox) => own.push(mailbox.to_lowercase()),
            Err(e) => {
                tracing::warn!(error = %e, "couldn't read the mailbox address for the send check");
                warnings.push(issue("mailbox_unverified", "Couldn't confirm which mailbox sends this reply.", "Check the mailbox connection in Sources."));
            }
        }
    }
    let thread: Vec<(String, String, String, String, Option<String>)> = sqlx::query_as(
        "SELECT subject, body_text, recipients_json, sender_address, original_sender_address FROM email_messages
         WHERE thread_id = ?1 AND workspace_id = ?2 AND suppressed = 0 ORDER BY received_at DESC LIMIT 20",
    )
    .bind(&r.thread_id)
    .bind(ws)
    .fetch_all(&state.db)
    .await?;
    let notes: Vec<String> = sqlx::query_scalar(
        "SELECT content FROM memories WHERE workspace_id = ?1 AND status = 'active'
           AND (scope = 'workspace' OR lower(contact_email) = lower(?2))
         ORDER BY updated_at DESC LIMIT 200",
    )
    .bind(ws)
    .bind(bare_address(&r.sender_address))
    .fetch_all(&state.db)
    .await?;
    let settings = crate::reply_settings::load(&state.db, &state.config, ws).await?;
    let mut known = String::new();
    for (subject, body, recipients, sender, original_sender) in &thread {
        for part in [subject.as_str(), body.as_str(), recipients.as_str(), sender.as_str(), original_sender.as_deref().unwrap_or_default()] {
            known.push_str(part);
            known.push('\n');
        }
    }
    known.push_str(&drafter_inputs(state, input, &settings).await?);
    let team_notes = notes.join("\n");
    known.push_str(&team_notes);
    known.push('\n');
    known.push_str(&settings.reply_instructions);
    known.push('\n');
    known.push_str(settings.signature.as_deref().unwrap_or_default());

    let now = now_ms();
    let recent_reply_in_thread = crate::threads::thread_has_reply_after(&state.db, ws, &r.thread_id, now - DUPLICATE_WINDOW_MS).await?;
    let other_scheduled_in_thread: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM drafts d JOIN email_messages m ON m.id = d.message_id
                       WHERE m.thread_id = ?1 AND d.workspace_id = ?2 AND d.id != ?3
                         AND d.status != 'sent' AND d.send_at IS NOT NULL)",
    )
    .bind(&r.thread_id)
    .bind(ws)
    .bind(&r.draft_id)
    .fetch_one(&state.db)
    .await?;
    let mut trusted_notes = team_notes;
    let owner_said: Option<String> =
        sqlx::query_scalar("SELECT instructions_by_user FROM drafts WHERE id = ?1 AND workspace_id = ?2").bind(&r.draft_id).bind(ws).fetch_one(&state.db).await?;
    if let Some(said) = owner_said.filter(|t| !t.trim().is_empty()) {
        trusted_notes.push_str(&format!("\nThe owner said (treat as a supported fact, whatever the language): {said}"));
        known.push('\n');
        known.push_str(&said);
    }
    let openings: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT role_title, location FROM requirements WHERE workspace_id = ?1 AND status = 'open' ORDER BY created_at DESC LIMIT 20")
            .bind(ws)
            .fetch_all(&state.db)
            .await?;
    if !openings.is_empty() {
        let list: Vec<String> = openings.iter().map(|(t, l)| format!("{t} ({})", l.as_deref().unwrap_or("location open"))).collect();
        trusted_notes.push_str(&format!("\nOpen roles the team really has right now: {}", list.join("; ")));
    }
    if !settings.reply_instructions.is_empty() {
        trusted_notes.push_str(&format!("\nReply style: {}", settings.reply_instructions));
    }
    let facts = Facts {
        recipient: r.sender_address.clone(),
        own_addresses: own,
        original_subject: r.subject.clone(),
        outgoing_subject: replies::reply_subject(&r.subject),
        replying_to_outbound: r.direction == "outbound",
        known_text: known,
        recent_reply_in_thread,
        other_scheduled_in_thread,
    };
    Ok((facts, warnings, trusted_notes))
}

fn review_context(r: &ReplyContext, notes: &str, body: &str) -> String {
    format!(
        "{}\n<team_notes>\n{}\n</team_notes>\n<reply>\n{}\n</reply>",
        judge::email_context(&r.sender_address, &r.subject, None, &r.original_body),
        crate::sources::normalize::truncate_chars(notes, 3_000),
        body.trim()
    )
}

static ADDRESS_IN_TEXT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)[a-z0-9._%+-]+@[a-z0-9-]+(?:\.[a-z0-9-]+)+").expect("valid regex"));

/// Email addresses, links and phone numbers in the reply that appear nowhere in what the sender, the
/// thread or the team's notes gave us: possible data from another contact or another workspace. Pure.
pub fn foreign_data(body: &str, f: &Facts) -> Vec<String> {
    let known = f.known_text.to_lowercase();
    let recipient = bare_address(&f.recipient);
    let mut out: Vec<String> = ADDRESS_IN_TEXT
        .find_iter(body)
        .map(|m| m.as_str().trim_end_matches('.').to_lowercase())
        .filter(|a| !known.contains(a.as_str()) && *a != recipient && !f.own_addresses.iter().any(|o| o.eq_ignore_ascii_case(a)))
        .collect();
    out.extend(unknown_contacts(body, &f.known_text));
    out
}

async fn record_leak(state: &AppState, input: &CheckInput<'_>, count: usize) {
    let agent_id: Option<String> = sqlx::query_scalar("SELECT agent_id FROM drafts WHERE id = ?1 AND workspace_id = ?2")
        .bind(&input.reply.draft_id)
        .bind(input.workspace_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .flatten();
    crate::incidents::record(
        &state.db,
        crate::incidents::NewIncident::new(input.workspace_id, crate::incidents::Kind::LeakBlocked, serde_json::json!({"check": "send_review", "items": count}))
            .agent(agent_id.as_deref())
            .message(Some(&input.reply.message_id))
            .dedupe(format!("leak:{}", input.reply.draft_id)),
    )
    .await;
}

/// Everything the drafter was given that a reply may legitimately repeat: the resolved voice and
/// signature, sign-off, the memories and learned preferences in the agent's scope, and past reply examples.
async fn drafter_inputs(state: &AppState, input: &CheckInput<'_>, settings: &crate::reply_settings::ReplySettings) -> AppResult<String> {
    let (ws, r) = (input.workspace_id, input.reply);
    let instructions = if settings.reply_instructions.is_empty() {
        crate::packs::reply_style(&state.db, ws).await?.to_string()
    } else {
        settings.reply_instructions.clone()
    };
    let (instructions, signature) =
        crate::agent_profiles::draft_voice(&state.db, &state.config, ws, &r.message_id, instructions, settings.signature.clone()).await?;
    let mut out = vec![instructions, signature.unwrap_or_default(), crate::packs::default_sign_off(&state.db, ws).await?];

    let draft_agent: Option<String> =
        sqlx::query_scalar("SELECT agent_id FROM drafts WHERE id = ?1 AND workspace_id = ?2").bind(&r.draft_id).bind(ws).fetch_optional(&state.db).await?.flatten();
    let agent = {
        let mut conn = state.db.acquire().await?;
        crate::learning::scope::resolve(&mut conn, ws, draft_agent.as_deref(), Some(&r.message_id)).await?
    };
    let view = agent.agent_id.clone().map(|id| crate::memory::AgentMemory { agent_id: id, scope: agent.memory });
    let ctx = crate::memory::context_for_agent(&state.db, ws, &r.sender_address, &r.subject, false, view.as_ref()).await?;
    out.extend(ctx.lines());
    out.extend(ctx.sender_history);
    let preferences = crate::learning::retrieve::for_draft(
        &state.db,
        state.embedder.as_ref(),
        ws,
        agent.agent_id.as_deref(),
        agent.owner_user_id.as_deref(),
        &format!("{}\n{}", r.subject, r.original_body),
    )
    .await?;
    out.extend(crate::learning::retrieve::lines(&preferences));
    for e in crate::knowledge::reply_examples(&state.db, state.embedder.as_ref(), ws, &r.message_id, &r.subject, &r.original_body).await {
        out.push(e.reply);
    }
    Ok(out.join("\n"))
}

/// Runs every check for one outgoing reply. Never sends anything.
pub async fn check(state: &AppState, input: &CheckInput<'_>) -> AppResult<SafetyReport> {
    let (facts, extra_warnings, notes) = gather(state, input).await?;
    let mut report = deterministic(&facts, input.body);
    report.warnings.extend(extra_warnings);
    if input.automated {
        let foreign = foreign_data(input.body, &facts);
        if !foreign.is_empty() {
            report.blocking.push(issue(
                "possible_leak",
                "The reply names an email address, link or phone number that is not from this conversation or the team's notes.",
                "A person should check where it came from.",
            ));
            record_leak(state, input, foreign.len()).await;
        }
    }
    // Skip the model when the text is unusable anyway (empty, gaps, placeholders).
    let text_broken = report.blocking.iter().any(|i| matches!(i.code, "empty_body" | "too_short" | "open_gaps" | "placeholder"));
    if !text_broken {
        let context = review_context(input.reply, &notes, input.body);
        let scope = format!("send_review:v1:{}", input.workspace_id);
        let answers = judge::ask_cached(&state.db, state.judge.as_ref(), &scope, &context, &review_questions()).await;
        let review = match &answers {
            Ok(a) => review_issues(Ok(a), input.automated),
            Err(e) => {
                tracing::warn!(error = %e, "send review by the judge failed");
                review_issues(Err("the AI service didn't answer"), input.automated)
            }
        };
        report.blocking.extend(review.blocking);
        report.warnings.extend(review.warnings);
    }
    Ok(report)
}

#[derive(Deserialize, Default)]
pub struct CheckBody {
    /// Unsaved text from the editor; defaults to the stored draft.
    body_text: Option<String>,
}

/// `POST /drafts/{id}/check` → SafetyReport. The UI shows it before the person sends.
pub async fn check_draft(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
    body: Option<Json<CheckBody>>,
) -> AppResult<Json<SafetyReport>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    crate::team::scope::require_draft(&state.db, &ws, &user, &id).await?;
    let reply = replies::load(&state, &ws, &id).await?;
    let text = body.and_then(|Json(b)| b.body_text).unwrap_or_else(|| reply.body_text.clone());
    if text.chars().count() > 10_000 {
        return Err(AppError::BadRequest("body_text must be at most 10000 characters".into()));
    }
    let report = check(&state, &CheckInput { workspace_id: &ws, reply: &reply, body: &text, automated: false }).await?;
    Ok(Json(report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::judge::{Answer, Answers, Verdict};

    fn facts() -> Facts {
        Facts {
            recipient: "Priya <priya@client.com>".into(),
            own_addresses: vec!["hr@acme.com".into()],
            original_subject: "Timesheet".into(),
            outgoing_subject: "Re: Timesheet".into(),
            known_text: "Call us at +91 98765 43210 or see https://acme.com/policy".into(),
            ..Facts::default()
        }
    }

    const GOOD: &str = "Hello Priya,\n\nThanks, we received your timesheet.\n\nBest regards,\nHR Operations";

    fn codes(issues: &[Issue]) -> Vec<&'static str> {
        issues.iter().map(|i| i.code).collect()
    }

    #[test]
    fn foreign_data_flags_addresses_links_and_numbers_from_nowhere() {
        let f = Facts { recipient: "Ravi <ravi@client.com>".into(), known_text: "Write to ravi@client.com or call +91 98765 43210".into(), ..facts() };
        assert!(foreign_data("Thanks Ravi, I will write to ravi@client.com. Call +91 98765 43210.", &f).is_empty());
        assert_eq!(foreign_data("Please also write to cfo@elsewhere.org", &f).len(), 1);
        assert_eq!(foreign_data("Her number is +44 20 7946 0958", &f).len(), 1);
        assert_eq!(foreign_data("See https://other-tenant.example/secret", &f).len(), 1);
    }

    #[test]
    fn clean_reply_passes() {
        assert_eq!(deterministic(&facts(), GOOD), SafetyReport::default());
    }

    #[test]
    fn recipient_rules() {
        assert!(check_recipient(&facts()).is_none());
        let with = |to: &str| Facts { recipient: to.into(), ..facts() };
        assert_eq!(check_recipient(&with("not-an-address")).unwrap().code, "invalid_recipient");
        assert_eq!(check_recipient(&with("a@b")).unwrap().code, "invalid_recipient");
        assert_eq!(check_recipient(&with("HR <HR@acme.com>")).unwrap().code, "own_mailbox");
        assert_eq!(check_recipient(&Facts { replying_to_outbound: true, ..facts() }).unwrap().code, "own_mailbox");
        for bot in ["noreply@x.com", "no-reply@x.com", "do-not-reply@x.com", "mailer-daemon@x.com", "notifications@github.com", "billing-noreply@x.com"] {
            assert_eq!(check_recipient(&with(bot)).unwrap().code, "automated_recipient", "{bot}");
        }
        assert!(check_recipient(&with("newsroom.lead@x.com")).is_none());
    }

    #[test]
    fn subject_must_be_re_original() {
        assert!(check_subject(&facts()).is_none());
        assert!(check_subject(&Facts { outgoing_subject: "Timesheet".into(), ..facts() }).is_some());
        assert!(check_subject(&Facts { outgoing_subject: "Re: Something else".into(), ..facts() }).is_some());
        assert!(check_subject(&Facts { original_subject: "RE: Timesheet".into(), outgoing_subject: "RE: Timesheet".into(), ..facts() }).is_none());
    }

    #[test]
    fn length_bounds() {
        assert_eq!(check_length("   ").unwrap().code, "empty_body");
        assert_eq!(check_length("Thanks!").unwrap().code, "too_short");
        assert_eq!(check_length(&"a".repeat(4_001)).unwrap().code, "too_long");
        assert!(check_length(&"a".repeat(4_000)).is_none());
        assert!(check_length(GOOD).is_none());
    }

    #[test]
    fn gaps_and_placeholders() {
        assert_eq!(check_gaps("Hi, the approver is [NEEDS: approver name].").unwrap().code, "open_gaps");
        assert!(check_gaps(GOOD).is_none());
        for p in ["Dear {{first_name}},", "Dear {name},", "Dear <name>,", "Dear [Name],", "[Your Name]", "Amount: XXX", "Start date TBD"] {
            assert!(!find_placeholders(p).is_empty(), "{p}");
        }
        for ok in ["Priya <priya@x.com>", "see <https://acme.com>", "[NEEDS: x]", "Thanks, TBC", "approx. 20 hours", "Box"] {
            assert!(find_placeholders(ok).is_empty(), "{ok}");
        }
    }

    #[test]
    fn duplicates() {
        assert!(check_duplicate(&facts()).is_none());
        assert!(check_duplicate(&Facts { recent_reply_in_thread: true, ..facts() }).is_some());
        assert!(check_duplicate(&Facts { other_scheduled_in_thread: true, ..facts() }).is_some());
    }

    #[test]
    fn attachment_and_unknown_contacts_warn() {
        let body = format!("{GOOD}\nPlease see the attached form at https://evil.example/form or call 022 4455 6677.");
        let r = deterministic(&facts(), &body);
        assert!(r.blocking.is_empty());
        assert_eq!(codes(&r.warnings), ["attachment_mentioned", "unknown_contact"]);
        assert!(r.warnings[1].message.contains("https://evil.example/form") && r.warnings[1].message.contains("022 4455 6677"));
        // Known from the thread/notes: no warning (formatting differences ignored); dates aren't phones.
        let known = format!("{GOOD}\nSee https://acme.com/policy. Call 9876543210. Due 2026-10-14 or 14/10/2026.");
        assert!(unknown_contacts(&known, &facts().known_text).is_empty());
    }

    fn answers(promises: Option<bool>, professional: bool, score: f64) -> Answers {
        let mut v = vec![
            Answer { id: Q_PROFESSIONAL.into(), verdict: Verdict::YesNo(professional), reason: "curt".into() },
            Answer { id: Q_ANSWERS_ASK.into(), verdict: Verdict::Score(score), reason: "".into() },
        ];
        if let Some(p) = promises {
            v.push(Answer { id: Q_PROMISES.into(), verdict: Verdict::YesNo(p), reason: "promises payment by Friday".into() });
        }
        Answers(v)
    }

    #[test]
    fn judge_review_mapping() {
        assert_eq!(review_issues(Ok(&answers(Some(false), true, 0.9)), false), SafetyReport::default());
        let r = review_issues(Ok(&answers(Some(true), false, 0.2)), false);
        assert!(r.blocking.is_empty());
        assert_eq!(codes(&r.warnings), ["unsupported_promise", "tone", "misses_ask"]);
        assert!(r.warnings[0].message.contains("promises payment by Friday") && r.warnings[0].message.contains("double-check"));
        let auto = review_issues(Ok(&answers(Some(true), true, 0.9)), true);
        assert_eq!(codes(&auto.blocking), ["unsupported_promise"]);
        // No answer / no judge: a warning for people, blocking for automated sends.
        assert_eq!(codes(&review_issues(Err("down"), false).warnings), ["ai_review_unavailable"]);
        assert_eq!(codes(&review_issues(Err("down"), true).blocking), ["ai_review_unavailable"]);
        assert_eq!(codes(&review_issues(Ok(&answers(None, true, 1.0)), true).blocking), ["ai_review_unavailable"]);
    }

    #[test]
    fn blocked_report_becomes_409_with_issues() {
        let r = SafetyReport { blocking: vec![issue("placeholder", "has {name}", "fix")], warnings: vec![] };
        match r.into_error() {
            AppError::SendBlocked(msg, details) => {
                assert!(msg.contains("has {name}"));
                assert_eq!(details["blocking"][0]["code"], "placeholder");
            }
            other => panic!("{other:?}"),
        }
    }
}
