//! When the agent may send a reply without asking. Only `send_email`, only when the workspace
//! lets routine replies go, the mailbox owner agreed ("Let routine replies go"), the daily cap
//! isn't reached and fixed rules pass; anything else asks a person. Submissions always ask.

use std::sync::LazyLock;

use regex::Regex;

use super::{case::Case, tools::Preview};
use crate::{agent_profiles::AgentContext, http::AppState, replies};

/// Money, rates or rejections: never sent without a person.
static SENSITIVE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(₹|\$|€|£|\b(inr|usd|eur|rs\.?|lakhs?|lpa|ctc|salary|salaries|rates?|payment|invoice|budget|offer letter|unfortunately|regret|reject(ed|ion)?|not (been )?(selected|shortlisted)|decline[ds]?)\b)",
    )
    .expect("valid regex")
});

/// Facts the fixed send rules check.
#[derive(Debug, Clone)]
pub struct SendFacts<'a> {
    pub to: &'a str,
    /// Sender of the first inbound email in the conversation.
    pub original_sender: Option<&'a str>,
    pub case_thread: Option<&'a str>,
    pub draft_thread: Option<&'a str>,
    pub body: &'a str,
    /// We wrote to this address before.
    pub known_contact: bool,
}

/// Why this send must go to a person, if it must. Pure; the hard rules already ran.
pub fn blocker(f: &SendFacts<'_>) -> Option<&'static str> {
    let to = f.to.trim().to_lowercase();
    if f.body.trim().is_empty() || replies::has_open_gaps(f.body) {
        return Some("the reply still has open questions");
    }
    match (f.case_thread, f.draft_thread) {
        (Some(c), Some(d)) if c == d => {}
        _ => return Some("it is not in this case's conversation"),
    }
    if f.original_sender.map(|s| s.trim().to_lowercase()) != Some(to.clone()) {
        return Some("it does not go to the person who started the conversation");
    }
    if !f.known_contact {
        return Some("it is the first email to this address");
    }
    if SENSITIVE.is_match(f.body) {
        return Some("it mentions money, rates or a decision about someone");
    }
    None
}

pub const AUTO_SENT_ACTION: &str = "agent.sent_without_approval";

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

/// True when this `send_email` may go without a person: the workspace allows routine sends,
/// the mailbox owner chose "let routine replies go", today's cap isn't reached, and every
/// fixed rule passes.
pub async fn may_send_on_its_own(state: &AppState, case: &Case, ctx: &AgentContext, preview: &Preview) -> sqlx::Result<bool> {
    let Some(owner) = ctx.owner.as_ref().filter(|o| o.routine_ok()) else { return Ok(false) };
    if !ctx.agent.enabled || ctx.agent.permissions.send != "auto" {
        return Ok(false);
    }
    if sent_today(state, &case.workspace_id, &owner.user_id).await? >= state.config.agent_auto_send_daily_cap {
        tracing::info!(case_id = %case.id, "daily routine-send cap reached; asking a person");
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
    let facts = SendFacts {
        to: &to,
        original_sender: original_sender.as_deref(),
        case_thread: case.thread_id.as_deref(),
        draft_thread: preview.thread_id.as_deref(),
        body: &preview.body,
        known_contact,
    };
    match blocker(&facts) {
        None => Ok(true),
        Some(why) => {
            tracing::info!(case_id = %case.id, why, "routine send needs a person");
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(to: &'static str, body: &'static str) -> SendFacts<'static> {
        SendFacts {
            to,
            original_sender: Some("ravi@client.com"),
            case_thread: Some("t1"),
            draft_thread: Some("t1"),
            body,
            known_contact: true,
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
}
