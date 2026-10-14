//! When an agent may send a reply without asking. Only `send_email`, only when the workspace
//! lets routine replies go, the mailbox owner agreed ("Let routine replies go"), the agent has
//! earned level 2 or more, the daily cap isn't reached and the fixed rules pass; anything else
//! asks a person. Submissions always ask.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::json;

use super::{case::Case, tools::Preview};
use crate::{
    agent_profiles::AgentContext,
    agent_registry::runtime::CaseAgent,
    audit::{self, Actor, Entry},
    http::AppState,
    replies,
};

/// What limits an agent's level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LevelInputs {
    /// Earned through the evals gate.
    pub earned: i64,
    /// Granted by an admin.
    pub cap: i64,
    /// The spec's `max_autonomy`.
    pub spec_max: i64,
    /// `AGENT_MAX_AUTONOMY`.
    pub env_max: i64,
    /// The mailbox owner has not agreed to routine replies going out.
    pub profile_asks_first: bool,
    pub active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Effective {
    pub level: i64,
    /// `earned`, `admin_cap`, `spec`, `workspace` (the environment limit), `profile` or `paused`.
    pub limited_by: &'static str,
}

/// The lowest of every limit, and which one it was (the first one on a tie).
pub fn effective_level(i: LevelInputs) -> Effective {
    if !i.active {
        return Effective { level: 0, limited_by: "paused" };
    }
    let limits = [
        (i.earned, "earned"),
        (i.cap, "admin_cap"),
        (i.spec_max, "spec"),
        (i.env_max, "workspace"),
        (if i.profile_asks_first { 1 } else { 3 }, "profile"),
    ];
    let (level, limited_by) = limits.into_iter().fold((i64::MAX, "earned"), |best, next| if next.0 < best.0 { next } else { best });
    Effective { level: level.clamp(0, 3), limited_by }
}

/// Money, rates or rejections: never sent without a person.
static SENSITIVE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(₹|\$|€|£|\b(inr|usd|eur|rs\.?|lakhs?|lpa|ctc|salary|salaries|rates?|payment|invoice|budget|offer letter|unfortunately|regret|reject(ed|ion)?|not (been )?(selected|shortlisted)|decline[ds]?)\b)",
    )
    .expect("valid regex")
});

/// Words too common to mean anything when matching a "never do" rule against a draft.
const COMMON_WORDS: &[&str] = &[
    "should", "would", "could", "about", "before", "after", "without", "anything", "something", "someone", "another", "whether", "because",
    "through", "within", "never", "always", "other", "their", "there", "these", "those", "which", "where", "while", "being", "having",
    "customer", "customers", "person", "people", "email", "emails", "reply", "replies", "agent",
];

/// Does the draft touch something a "never do" rule forbids? Deliberately cautious: the rule's text,
/// a quoted phrase in it, or any of its significant words (six letters or more) in the draft sends
/// the reply to a person.
pub fn never_do_hit(body: &str, rules: &[String]) -> Option<String> {
    let body = body.to_lowercase();
    for rule in rules {
        let lower = rule.to_lowercase();
        let quoted = lower.split(['"', '\u{201c}', '\u{201d}']).skip(1).step_by(2).any(|q| !q.trim().is_empty() && body.contains(q.trim()));
        let words = lower
            .split(|c: char| !c.is_alphabetic())
            .filter(|w| w.chars().count() >= 6 && !COMMON_WORDS.contains(w))
            .any(|w| body.split(|c: char| !c.is_alphabetic()).any(|b| b == w));
        if quoted || words || (!lower.trim().is_empty() && body.contains(lower.trim())) {
            return Some(rule.clone());
        }
    }
    None
}

static EMAIL_ADDRESS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)[a-z0-9._%+-]+@[a-z0-9-]+(\.[a-z0-9-]+)+").expect("valid regex"));
static LONG_NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d(?:[\s-]?\d){8,}").expect("valid regex"));

/// The leak check, in code: the draft names an email address that is not part of this conversation
/// or our own, or carries a long number (account, card, phone). Such a reply goes to a person.
pub fn leaks(body: &str, known: &[String]) -> bool {
    LONG_NUMBER.is_match(body)
        || EMAIL_ADDRESS.find_iter(body).any(|m| !known.iter().any(|k| k.trim().eq_ignore_ascii_case(m.as_str().trim_end_matches('.'))))
}

/// Facts the fixed send rules check.
#[derive(Debug, Clone)]
pub struct SendFacts<'a> {
    pub to: &'a str,
    /// Sender of the first inbound email in the conversation.
    pub original_sender: Option<&'a str>,
    /// Everyone who wrote to us in the conversation (lower-case).
    pub participants: &'a [String],
    pub case_thread: Option<&'a str>,
    pub draft_thread: Option<&'a str>,
    pub body: &'a str,
    /// We wrote to this address before.
    pub known_contact: bool,
    /// The agent's effective level.
    pub level: i64,
    /// The spec's `drafting.never_do` rules.
    pub never_do: &'a [String],
    /// Inbound text that tried to instruct the agent (the injection gate), if any.
    pub injected: Option<&'static str>,
    /// Addresses the draft may mention: the conversation's and our own.
    pub known_addresses: &'a [String],
}

/// Why this send must go to a person, if it must. Pure; the hard rules already ran.
pub fn blocker(f: &SendFacts<'_>) -> Option<&'static str> {
    let to = f.to.trim().to_lowercase();
    if f.body.trim().is_empty() || replies::has_open_gaps(f.body) {
        return Some("the reply still has open questions");
    }
    if f.injected.is_some() {
        return Some("the conversation contains text that tries to instruct the agent");
    }
    if f.level < 2 {
        return Some("this agent has not earned sending on its own yet");
    }
    match (f.case_thread, f.draft_thread) {
        (Some(c), Some(d)) if c == d => {}
        _ => return Some("it is not in this case's conversation"),
    }
    let original = f.original_sender.map(|s| s.trim().to_lowercase());
    let participant = f.participants.iter().any(|p| p.trim().to_lowercase() == to);
    if f.level >= 3 {
        if !participant && original.as_deref() != Some(to.as_str()) {
            return Some("it goes to someone who is not in the conversation");
        }
    } else if original.as_deref() != Some(to.as_str()) {
        return Some("it does not go to the person who started the conversation");
    }
    if f.level < 3 && !f.known_contact {
        return Some("it is the first email to this address");
    }
    if SENSITIVE.is_match(f.body) {
        return Some("it mentions money, rates or a decision about someone");
    }
    if leaks(f.body, f.known_addresses) {
        return Some("it mentions an address or number that is not from this conversation");
    }
    if never_do_hit(f.body, f.never_do).is_some() {
        return Some("it touches something this agent must never do");
    }
    None
}

pub const AUTO_SENT_ACTION: &str = "agent.sent_without_approval";
pub const HELD_ACTION: &str = "agent.send_held";

/// Replies sent without asking today (UTC) for one mailbox owner.
pub async fn sent_today(state: &AppState, ws: &str, owner_id: &str) -> sqlx::Result<i64> {
    const DAY_MS: i64 = 86_400_000;
    let since = crate::db::now_ms() / DAY_MS * DAY_MS;
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events e JOIN cases c ON c.id = e.entity_id
         WHERE e.workspace_id = ?1 AND e.action = ?2 AND e.created_at >= ?3 AND c.agent_user_id = ?4",
    )
    .bind(ws)
    .bind(AUTO_SENT_ACTION)
    .bind(since)
    .bind(owner_id)
    .fetch_one(&state.db)
    .await
}

/// The standing conditions for sending without a person, shared by the decision to send and the check
/// when a held send comes due: the agent is switched on, its effective permission is "auto" and its level is 2 or more.
pub fn standing_block(enabled: bool, send: &str, level: i64) -> Option<&'static str> {
    if !enabled {
        return Some("the agent is switched off");
    }
    if send != "auto" {
        return Some("the agent no longer has permission to send on its own");
    }
    if level < 2 {
        return Some("the agent's level no longer allows sending on its own");
    }
    None
}

/// True when this `send_email` may go without a person: the workspace allows routine sends, the
/// mailbox owner chose "let routine replies go", the agent's effective level allows it, today's cap
/// isn't reached, the conversation passed the injection gate and every fixed rule passes.
pub async fn may_send_on_its_own(state: &AppState, case: &Case, ctx: &AgentContext, agent: &CaseAgent, preview: &Preview) -> sqlx::Result<bool> {
    let Some(owner) = ctx.owner.as_ref().filter(|o| o.routine_ok()) else { return Ok(false) };
    if standing_block(ctx.agent.enabled, &ctx.agent.permissions.send, agent.level.level).is_some() {
        return Ok(false);
    }
    let cap = state.config.agent_auto_send_daily_cap.min(i64::from(agent.spec.permissions.daily_send_cap));
    if sent_today(state, &case.workspace_id, &owner.user_id).await? >= cap {
        tracing::info!(case_id = %case.id, "daily routine-send cap reached; asking a person");
        return Ok(false);
    }
    let machine_written: bool = match preview.draft_id.as_deref() {
        Some(id) => sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM drafts WHERE id = ?1 AND workspace_id = ?2 AND status = 'suggested' AND origin_key IS NOT NULL)",
        )
        .bind(id)
        .bind(&case.workspace_id)
        .fetch_one(&state.db)
        .await?,
        None => false,
    };
    if !machine_written {
        tracing::info!(case_id = %case.id, "a reply a person wrote or edited never goes out on its own");
        return Ok(false);
    }
    let Some(to) = preview.to.as_deref().map(str::to_lowercase) else { return Ok(false) };
    let original_sender: Option<String> = sqlx::query_scalar(
        "SELECT lower(sender_address) FROM email_messages
         WHERE workspace_id = ?1 AND thread_id = ?2 AND direction = 'inbound' ORDER BY received_at, id LIMIT 1",
    )
    .bind(&case.workspace_id)
    .bind(&case.thread_id)
    .fetch_optional(&state.db)
    .await?;
    let participants: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT lower(sender_address) FROM email_messages
         WHERE workspace_id = ?1 AND thread_id = ?2 AND direction = 'inbound'",
    )
    .bind(&case.workspace_id)
    .bind(&case.thread_id)
    .fetch_all(&state.db)
    .await?;
    let known_contact: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM drafts d JOIN email_messages m ON m.id = d.message_id
                        WHERE d.workspace_id = ?1 AND d.status = 'sent' AND lower(m.sender_address) = ?2)
             OR EXISTS (SELECT 1 FROM email_messages o, json_each(o.recipients_json) r
                        WHERE o.workspace_id = ?1 AND o.direction = 'outbound' AND instr(lower(r.value), ?2) > 0)",
    )
    .bind(&case.workspace_id)
    .bind(&to)
    .fetch_one(&state.db)
    .await?;
    let injected = super::injection::scan_case(&state.db, case).await?;
    let mut known_addresses = participants.clone();
    known_addresses.push(to.clone());
    known_addresses.extend(ctx.mailbox.clone());
    known_addresses.push(owner.email.clone());
    let facts = SendFacts {
        to: &to,
        original_sender: original_sender.as_deref(),
        participants: &participants,
        case_thread: case.thread_id.as_deref(),
        draft_thread: preview.thread_id.as_deref(),
        body: &preview.body,
        known_contact,
        level: agent.level.level,
        never_do: &agent.spec.drafting.never_do,
        injected,
        known_addresses: &known_addresses,
    };
    match blocker(&facts) {
        None => Ok(true),
        Some(why) => {
            tracing::info!(case_id = %case.id, why, "routine send needs a person");
            Ok(false)
        }
    }
}

/// Notes in the audit trail that an automatic send was held until `send_at` so a person can stop it.
pub async fn record_hold(state: &AppState, ws: &str, draft_id: &str, send_at: i64, delay_secs: i64) -> sqlx::Result<()> {
    let mut conn = state.db.acquire().await?;
    audit::record(
        &mut conn,
        Entry {
            workspace_id: ws,
            actor: Actor::System,
            action: HELD_ACTION,
            entity_type: "draft",
            entity_id: draft_id,
            message_id: None,
            metadata: json!({ "send_at": send_at, "delay_secs": delay_secs }),
        },
    )
    .await
}

/// Why a held automatic send must not go out any more, if it must: the agent that wrote it was
/// paused or archived, switched off, its effective level fell below 2, or the workspace or its spec no longer lets routine
/// replies go. The send then waits for a person. Drafts that no agent wrote are not checked.
pub async fn automated_send_stopped(state: &AppState, ws: &str, draft_id: &str) -> sqlx::Result<Option<&'static str>> {
    let agent_id: Option<String> = sqlx::query_scalar("SELECT agent_id FROM drafts WHERE id = ?1 AND workspace_id = ?2")
        .bind(draft_id)
        .bind(ws)
        .fetch_optional(&state.db)
        .await?
        .flatten();
    let Some(agent_id) = agent_id else { return Ok(None) };
    let conversation: Option<(Option<String>, String)> = sqlx::query_as(
        "SELECT m.thread_id, m.id FROM drafts d JOIN email_messages m ON m.id = d.message_id WHERE d.id = ?1 AND d.workspace_id = ?2",
    )
    .bind(draft_id)
    .bind(ws)
    .fetch_optional(&state.db)
    .await?;
    if let Some((thread, message)) = conversation
        && super::injection::scan_conversation(&state.db, ws, thread.as_deref(), Some(&message)).await?.is_some()
    {
        return Ok(Some("mail in the conversation tries to instruct the agent"));
    }
    let Some(row) = crate::agent_registry::store::get_in(&state.db, ws, &agent_id).await? else { return Ok(Some("the agent no longer exists")) };
    if row.status != "active" {
        return Ok(Some("the agent is not active"));
    }
    let (spec, _) = crate::agent_registry::versions::active_spec_in(&state.db, &row).await?;
    let mailbox_owner: Option<String> = sqlx::query_scalar(
        "SELECT s.connected_by_user_id FROM drafts d JOIN email_messages m ON m.id = d.message_id
         JOIN email_sources s ON s.id = m.source_id WHERE d.id = ?1",
    )
    .bind(draft_id)
    .fetch_optional(&state.db)
    .await?
    .flatten();
    let routine_ok = match mailbox_owner {
        Some(owner) => crate::agent_profiles::user_profile(&state.db, ws, &owner).await?.is_some_and(|p| p.routine_ok()),
        None => false,
    };
    let level = effective_level(LevelInputs {
        earned: row.autonomy_level,
        cap: row.autonomy_cap,
        spec_max: i64::from(spec.permissions.max_autonomy),
        env_max: state.config.agent_max_autonomy,
        profile_asks_first: !routine_ok,
        active: true,
    });
    let workspace = crate::agent_profiles::agent(&state.db, &state.config, ws).await?;
    let send = if row.kind == "lead" {
        workspace.permissions.send.clone()
    } else {
        crate::agent_registry::runtime::effective_send(&workspace.permissions.send, spec.permissions.send).to_string()
    };
    if let Some(why) = standing_block(workspace.enabled, &send, level.level) {
        return Ok(Some(why));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(to: &'static str, body: &'static str) -> SendFacts<'static> {
        SendFacts {
            to,
            original_sender: Some("ravi@client.com"),
            participants: &[],
            case_thread: Some("t1"),
            draft_thread: Some("t1"),
            body,
            known_contact: true,
            level: 2,
            never_do: &[],
            injected: None,
            known_addresses: &[],
        }
    }

    #[test]
    fn routine_reply_to_the_original_sender_passes() {
        assert_eq!(blocker(&facts("Ravi@Client.com", "Hello Ravi,\n\nThanks, we got your leave request.\n\nMeera")), None);
    }

    #[test]
    fn risky_sends_need_a_person() {
        assert!(blocker(&facts("ravi@client.com", "Please confirm [NEEDS: dates]")).is_some());
        assert!(blocker(&facts("other@client.com", "Hello")).is_some(), "not the original sender");
        assert!(blocker(&SendFacts { draft_thread: Some("t2"), ..facts("ravi@client.com", "Hello") }).is_some(), "other thread");
        assert!(blocker(&SendFacts { case_thread: None, ..facts("ravi@client.com", "Hello") }).is_some());
        for body in ["The rate is ₹1,200 per hour.", "Unfortunately you were not selected.", "Your salary will be revised.", "We have to reject this."] {
            assert!(blocker(&facts("ravi@client.com", body)).is_some(), "{body}");
        }
        let first = SendFacts { known_contact: false, ..facts("ravi@client.com", "Hello") };
        assert!(blocker(&first).is_some(), "first email to this address");
    }

    #[test]
    fn effective_level_is_the_lowest_limit() {
        let base = LevelInputs { earned: 3, cap: 3, spec_max: 3, env_max: 3, profile_asks_first: false, active: true };
        assert_eq!(effective_level(base), Effective { level: 3, limited_by: "earned" });
        assert_eq!(effective_level(LevelInputs { cap: 2, ..base }), Effective { level: 2, limited_by: "admin_cap" });
        assert_eq!(effective_level(LevelInputs { spec_max: 1, ..base }), Effective { level: 1, limited_by: "spec" });
        assert_eq!(effective_level(LevelInputs { env_max: 2, ..base }), Effective { level: 2, limited_by: "workspace" });
        assert_eq!(effective_level(LevelInputs { profile_asks_first: true, ..base }), Effective { level: 1, limited_by: "profile" });
        assert_eq!(effective_level(LevelInputs { earned: 0, ..base }), Effective { level: 0, limited_by: "earned" });
        assert_eq!(effective_level(LevelInputs { earned: 1, cap: 2, spec_max: 3, env_max: 3, profile_asks_first: true, active: true }), Effective { level: 1, limited_by: "earned" });
        assert_eq!(effective_level(LevelInputs { active: false, ..base }), Effective { level: 0, limited_by: "paused" });
    }

    #[test]
    fn levels_below_two_never_send_on_their_own() {
        for level in [0, 1] {
            assert!(blocker(&SendFacts { level, ..facts("ravi@client.com", "Hello") }).is_some(), "level {level}");
        }
        assert_eq!(blocker(&facts("ravi@client.com", "Hello")), None);
    }

    #[test]
    fn level_three_replies_to_any_participant_but_never_to_strangers() {
        let participants = vec!["ravi@client.com".to_string(), "priya@client.com".to_string()];
        let l3 = |to| SendFacts { level: 3, participants: &participants, known_contact: false, ..facts(to, "Hello") };
        assert_eq!(blocker(&l3("priya@client.com")), None, "a participant, first email to them");
        assert_eq!(blocker(&l3("ravi@client.com")), None, "an inbound sender, first reply");
        assert!(blocker(&l3("stranger@evil.com")).is_some(), "a new recipient always asks");
        let l2 = SendFacts { level: 2, participants: &participants, ..facts("priya@client.com", "Hello") };
        assert!(blocker(&l2).is_some(), "level 2 only answers the original sender");
    }

    #[test]
    fn the_fixed_floor_holds_at_every_level() {
        let participants = vec!["ravi@client.com".to_string()];
        for level in [2, 3] {
            let at = |body| SendFacts { level, participants: &participants, ..facts("ravi@client.com", body) };
            assert!(blocker(&at("We agree on ₹5,000.")).is_some(), "money, level {level}");
            assert!(blocker(&at("Your offer letter is ready.")).is_some(), "decision, level {level}");
            assert!(blocker(&at("Which date? [NEEDS: date]")).is_some(), "open gap, level {level}");
            assert!(blocker(&SendFacts { injected: Some("it names a tool"), ..at("Hello") }).is_some(), "injection, level {level}");
            let rules = vec!["Promise a refund or a date that is not in the notes".to_string()];
            assert!(blocker(&SendFacts { never_do: &rules, ..at("We will process your refund today") }).is_some(), "never_do, level {level}");
            assert_eq!(blocker(&SendFacts { never_do: &rules, ..at("Thanks, we got your question") }), None);
        }
    }

    #[test]
    fn the_leak_check_is_a_code_rule() {
        let known = vec!["ravi@client.com".to_string(), "me@acme.com".to_string()];
        assert!(!leaks("Thanks Ravi, I will write to ravi@client.com again.", &known));
        assert!(leaks("Please also write to cfo@elsewhere.org", &known));
        assert!(leaks("The account number is 1234 5678 9012", &known));
        assert!(leaks("Call +91 98765 43210", &known));
        assert!(!leaks("Your order 12345 ships Monday.", &known));
        let facts = SendFacts { known_addresses: &known, ..facts("ravi@client.com", "Copy cfo@elsewhere.org on this") };
        assert!(blocker(&facts).is_some());
    }

    #[test]
    fn never_do_rules_match_text_quotes_and_key_words() {
        let rules = vec!["Offer a discount".to_string(), r#"Say "guaranteed""#.to_string()];
        assert!(never_do_hit("Happy to offer a discount", &rules).is_some());
        assert!(never_do_hit("There is a 10% discount today", &rules).is_some());
        assert!(never_do_hit("This is guaranteed to work", &rules).is_some());
        assert!(never_do_hit("Thanks for your note", &rules).is_none());
        assert!(never_do_hit("Anything", &[]).is_none());
    }
}
