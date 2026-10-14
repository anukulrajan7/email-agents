//! When a message may go out without a person. Pure rules first (so each is easy to test), then the one
//! lookup that needs the database: how far the sender's personal agent has earned the right to send on its own.

use std::sync::LazyLock;

use regex::Regex;

use serde_json::Value;

use super::recipients::{self, Kind};
use crate::{
    agent_profiles::AgentContext,
    agent_registry::runtime::CaseAgent,
    agents::{
        autonomy::{self, LevelInputs},
        case::Case,
        injection,
    },
    http::AppState,
};

/// Money, banks and security: never sent without a person, whoever it goes to.
static MONEY_SECURITY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(₹|\$|€|£|\b(inr|usd|eur|rs\.?|lakhs?|crores?|salary|salaries|payroll|pay|paid|payments?|amount|invoices?|refunds?|prices?|pricing|quote[ds]?|discount|fees?|costs?|owe[sd]?|rupees?|dollars?|nda|confidential|visa|gdpr|harassment|legal|contracts?|wire|transfers?|neft|rtgs|imps|upi|ifsc|swift|iban|bank|banking|account (number|no\.?)|a/c|card (number|no\.?|ending)|credit card|debit card|cvv|otp|one[- ]time (password|code)|pin|password|passcode|passwords|credentials?|api[- ]keys?|secrets?|tokens?|2fa|two[- ]factor|login|sign[- ]in)\b)",
    )
    .expect("valid regex")
});

/// Why this text touches money, a bank or security, if it does.
pub fn sensitive(text: &str) -> Option<&'static str> {
    MONEY_SECURITY.is_match(text).then_some("it mentions money, a bank or security details")
}

/// What decides whether a message needs a person.
#[derive(Debug, Clone)]
pub struct Facts<'a> {
    /// The kind of every recipient (a team has one per member).
    pub recipients: &'a [Kind],
    /// The sender's effective autonomy level.
    pub level: i64,
    pub subject: &'a str,
    pub body: &'a str,
    pub attachments_out: bool,
    /// Inbound text that tried to instruct the agent, if any.
    pub injected: Option<&'static str>,
    /// A person wrote the text.
    pub person_authored: bool,
}

/// Why a person has to approve this message first, if they do. Every rule but the level holds at every level.
pub fn approval_reason(f: &Facts<'_>) -> Option<String> {
    if f.person_authored {
        return Some("a message a person wrote is never sent on its own".into());
    }
    if let Some(why) = f.injected {
        return Some(format!("the conversation has text that tries to instruct the agent ({why})"));
    }
    if f.recipients.is_empty() {
        return Some("there is nobody to send it to".into());
    }
    if f.recipients.iter().any(|k| !k.is_internal()) {
        return Some("it goes outside your workspace, or to an address nobody here has written to".into());
    }
    if let Some(why) = sensitive(f.subject).or_else(|| sensitive(f.body)) {
        return Some(why.into());
    }
    if f.attachments_out {
        return Some("it would send attachments".into());
    }
    if f.level < 2 {
        return Some("this assistant has not earned sending on its own yet".into());
    }
    None
}

/// Injection signals in text the agent wrote or was shown: the same gate the replies use.
pub fn injection_signal(texts: &[&str]) -> Option<&'static str> {
    texts.iter().find_map(|t| injection::scan(t))
}

/// The effective level of a person's own assistant, 0 when it is paused or missing.
pub async fn personal_level(state: &AppState, ws: &str, user_id: &str) -> sqlx::Result<i64> {
    let Some(row) = crate::agent_registry::store::personal_of(&state.db, ws, user_id).await? else { return Ok(0) };
    let (spec, _) = crate::agent_registry::versions::active_spec_in(&state.db, &row).await?;
    let routine_ok = crate::agent_profiles::user_profile(&state.db, ws, user_id).await?.is_none_or(|p| p.routine_ok());
    Ok(autonomy::effective_level(LevelInputs {
        earned: row.autonomy_level,
        cap: row.autonomy_cap,
        spec_max: i64::from(spec.permissions.max_autonomy),
        env_max: state.config.agent_max_autonomy,
        profile_asks_first: !routine_ok,
        active: row.status == "active",
    })
    .level)
}

/// True when this `send_message` may go without a person: the mailbox owner agreed to routine sends, the agent is on
/// and has earned level 2, today's cap is not reached, the conversation passed the injection gate and every rule above holds.
pub async fn may_send_on_its_own(state: &AppState, case: &Case, ctx: &AgentContext, agent: &CaseAgent, args: &Value) -> sqlx::Result<bool> {
    let Some(owner) = ctx.owner.as_ref().filter(|o| o.routine_ok()) else { return Ok(false) };
    if autonomy::standing_block(ctx.agent.enabled, &ctx.agent.permissions.send, agent.level.level).is_some() {
        return Ok(false);
    }
    let cap = state.config.agent_auto_send_daily_cap.min(i64::from(agent.spec.permissions.daily_send_cap));
    if super::email::auto_sent_today(state, &case.workspace_id, &owner.user_id).await? >= cap {
        tracing::info!(case_id = %case.id, "daily routine-send cap reached; asking a person");
        return Ok(false);
    }
    let text = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let (to, subject, body) = (text("to"), text("subject"), text("body"));
    let Ok(target) = recipients::resolve(&state.db, &case.workspace_id, &to).await else { return Ok(false) };
    let kinds = target.kinds();
    let injected = match injection::scan_case(&state.db, case).await? {
        Some(why) => Some(why),
        None => injection_signal(&[&subject, &body]),
    };
    let facts = Facts { recipients: &kinds, level: agent.level.level, subject: &subject, body: &body, attachments_out: false, injected, person_authored: false };
    match approval_reason(&facts) {
        None => Ok(true),
        Some(why) => {
            tracing::info!(case_id = %case.id, why, "message needs a person");
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts<'a>(recipients: &'a [Kind], level: i64, body: &'a str) -> Facts<'a> {
        Facts { recipients, level, subject: "Order 123", body, attachments_out: false, injected: None, person_authored: false }
    }

    #[test]
    fn internal_recipients_send_on_their_own_from_level_two() {
        let internal = [Kind::Member];
        for (level, auto) in [(0, false), (1, false), (2, true), (3, true)] {
            assert_eq!(approval_reason(&facts(&internal, level, "Why is order 123 late?")).is_none(), auto, "level {level}");
        }
        assert!(approval_reason(&facts(&[Kind::Directory, Kind::Member], 2, "Lunch at one?")).is_none());
    }

    #[test]
    fn external_and_new_addresses_always_need_a_person() {
        for kind in [Kind::KnownExternal, Kind::NewExternal] {
            for level in 0..=3 {
                assert!(approval_reason(&facts(&[kind], level, "Hello")).is_some(), "{kind:?} at level {level}");
            }
        }
        assert!(approval_reason(&facts(&[Kind::Member, Kind::NewExternal], 3, "Hello")).is_some());
    }

    #[test]
    fn money_bank_and_security_always_need_a_person() {
        for body in ["Please pay ₹5,000", "What is the bank account number?", "Send me the password", "The OTP is 123", "Refund the invoice"] {
            for level in 0..=3 {
                assert!(approval_reason(&facts(&[Kind::Member], level, body)).is_some(), "{body} at level {level}");
            }
        }
    }

    #[test]
    fn attachments_injection_and_human_drafts_always_need_a_person() {
        let internal = [Kind::Member];
        assert!(approval_reason(&Facts { attachments_out: true, ..facts(&internal, 3, "See file") }).is_some());
        assert!(approval_reason(&Facts { injected: Some("it tells the agent to ignore its earlier instructions"), ..facts(&internal, 3, "Hi") }).is_some());
        assert!(approval_reason(&Facts { person_authored: true, ..facts(&internal, 3, "Hi") }).is_some());
        assert!(approval_reason(&facts(&[], 3, "Hi")).is_some());
    }
}
