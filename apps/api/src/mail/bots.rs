//! Machines writing mail: a no-reply sender, an out-of-office answer, a delivery failure. Ingest and send share these rules, so
//! what we store about an inbound message and what we refuse to write to are the same.

use std::sync::LazyLock;

use regex::Regex;

use crate::mail::bulk;

static NO_REPLY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:.*(?:no-?reply|do-?not-?reply|donotreply)|mailer-daemon|postmaster|bounces?|notifications?|notify|newsletters?|alerts?|updates|marketing|news)(?:[.+-].*)?@")
        .expect("valid regex")
});
static BOUNCE_SUBJECT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(?:undeliverable|undelivered mail|delivery status notification|mail delivery (?:failed|failure|subsystem)|returned mail|delivery has failed|failure notice|message not delivered)")
        .expect("valid regex")
});
static AUTO_SUBJECT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(?:(?:automatic|auto)[ -]?(?:reply|response|answer)|auto\s*:|autoreply|out of (?:the )?office|away from (?:the )?office|on (?:annual )?leave|vacation (?:reply|response))")
        .expect("valid regex")
});

/// A sender address that is a machine (`noreply@`, `mailer-daemon@`, `notifications@` ...).
pub fn is_automated_address(addr: &str) -> bool {
    NO_REPLY.is_match(addr)
}

pub fn is_bounce_subject(subject: &str) -> bool {
    BOUNCE_SUBJECT.is_match(subject)
}

pub fn is_auto_reply_subject(subject: &str) -> bool {
    AUTO_SUBJECT.is_match(subject)
}

/// What kind of machine answer an inbound message is, as `email_messages.auto_kind` stores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoKind {
    /// An out-of-office or other automatic reply: nobody has read our mail yet.
    Ooo,
    /// A delivery failure notice about mail we sent.
    Dsn,
}

impl AutoKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AutoKind::Ooo => "ooo",
            AutoKind::Dsn => "dsn",
        }
    }
}

/// A bounce is a mailer-daemon, a delivery-status report or a failure subject; an automatic answer says so in `Auto-Submitted`,
/// `X-Autoreply` or its subject. A no-reply address alone is neither: it is only a sender nobody writes to.
pub fn auto_kind(sender: &str, subject: &str, signals: &[String]) -> Option<AutoKind> {
    let has = |s: &str| signals.iter().any(|x| x == s);
    let local = sender.split('@').next().unwrap_or_default().to_lowercase();
    if matches!(local.as_str(), "mailer-daemon" | "postmaster") || has(bulk::DELIVERY_STATUS) || is_bounce_subject(subject) {
        return Some(AutoKind::Dsn);
    }
    (has(bulk::AUTO_SUBMITTED) || has(bulk::AUTO_REPLY) || is_auto_reply_subject(subject)).then_some(AutoKind::Ooo)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|i| (*i).to_string()).collect()
    }

    #[test]
    fn no_reply_addresses_are_machines() {
        for a in ["noreply@acme.test", "no-reply@acme.test", "mailer-daemon@acme.test", "notifications@x.io", "do-not-reply@x.io"] {
            assert!(is_automated_address(a), "{a}");
        }
        assert!(!is_automated_address("ravi@acme.test") && !is_automated_address("hr@acme.test"));
    }

    #[test]
    fn auto_kind_tells_an_out_of_office_from_a_bounce_and_ignores_people() {
        assert_eq!(auto_kind("ravi@x.test", "Automatic reply: on leave till 20 Oct", &[]), Some(AutoKind::Ooo));
        assert_eq!(auto_kind("ravi@x.test", "Re: timesheet", &s(&["auto_submitted"])), Some(AutoKind::Ooo));
        assert_eq!(auto_kind("mailer-daemon@x.test", "Re: timesheet", &[]), Some(AutoKind::Dsn));
        assert_eq!(auto_kind("ravi@x.test", "Undeliverable: timesheet", &[]), Some(AutoKind::Dsn));
        assert_eq!(auto_kind("ravi@x.test", "Re: timesheet", &s(&["delivery_status"])), Some(AutoKind::Dsn));
        assert_eq!(auto_kind("ravi@x.test", "Re: timesheet", &[]), None);
        assert_eq!(auto_kind("noreply@x.test", "Your timesheet was approved", &[]), None, "a no-reply sender is not itself an automatic answer");
    }
}
