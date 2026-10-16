//! Post Room's own system mail (alert digests, "No reply yet" notices) lands in the owner's connected
//! mailbox like any other message. It must never get a draft, a case, an automation run or an alert.
//! Two independent proofs: the `X-PostRoom` header we stamp on outgoing mail (stored at ingest as the
//! `postroom` signal) and the sender being our own SMTP account. The subject is never consulted:
//! anyone, including a manager replying "Re: [Post Room] ...", can type it.

use sqlx::SqlitePool;

use crate::platform::config::Config;

/// Stored in `email_messages.bulk_signals` when the message carries our `X-PostRoom` header.
pub const SIGNAL: &str = "postroom";

fn address_of(value: &str) -> String {
    let inner = match (value.rfind('<'), value.rfind('>')) {
        (Some(open), Some(close)) if open < close => &value[open + 1..close],
        _ => value,
    };
    inner.trim().to_ascii_lowercase()
}

/// Lowercased addresses Post Room sends from: the `smtp_from` address and the SMTP login when it is an address.
pub fn own_senders(config: &Config) -> Vec<String> {
    let mut out = vec![address_of(&config.smtp_from)];
    let username = config.smtp_username.trim();
    if username.contains('@') {
        out.push(address_of(username));
    }
    out.retain(|a| !a.is_empty());
    out.dedup();
    out
}

/// True when the message carries our header signal or comes from one of our own addresses.
pub fn is_own(config: &Config, sender: &str, signals: &[String]) -> bool {
    if signals.iter().any(|s| s == SIGNAL) {
        return true;
    }
    let sender = sender.trim().to_ascii_lowercase();
    !sender.is_empty() && own_senders(config).contains(&sender)
}

pub async fn is_own_message(db: &SqlitePool, config: &Config, workspace_id: &str, message_id: &str) -> sqlx::Result<bool> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT sender_address, bulk_signals FROM email_messages WHERE id = ?1 AND workspace_id = ?2")
            .bind(message_id)
            .bind(workspace_id)
            .fetch_optional(db)
            .await?;
    Ok(row.is_some_and(|(sender, stored)| {
        let signals: Vec<String> = serde_json::from_str(&stored).unwrap_or_default();
        is_own(config, &sender, &signals)
    }))
}

/// SQL predicate (for a query over `email_messages` aliased `alias`) that excludes our own mail.
/// `own_param` is a placeholder (e.g. `?7`) bound to `json!(own_senders(config)).to_string()`.
pub fn not_own_sql(alias: &str, own_param: &str) -> String {
    format!(
        "(lower({alias}.sender_address) NOT IN (SELECT value FROM json_each({own_param}))
          AND {alias}.bulk_signals NOT LIKE '%\"{SIGNAL}\"%')"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(from: &str, username: &str) -> Config {
        let mut c = Config::for_tests("sqlite::memory:");
        c.smtp_from = from.into();
        c.smtp_username = username.into();
        c
    }

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|i| (*i).to_string()).collect()
    }

    #[test]
    fn the_address_is_parsed_from_a_display_name_form() {
        assert_eq!(own_senders(&config("Post Room <No-Reply@X.test>", "")), ["no-reply@x.test"]);
    }

    #[test]
    fn a_bare_address_works() {
        assert_eq!(own_senders(&config("no-reply@x.test", "")), ["no-reply@x.test"]);
    }

    #[test]
    fn the_smtp_username_counts_only_when_it_is_an_address() {
        assert_eq!(own_senders(&config("Post Room <no-reply@x.test>", "Mailer@X.test")), ["no-reply@x.test", "mailer@x.test"]);
        assert_eq!(own_senders(&config("", "mailer@x.test")), ["mailer@x.test"], "falls back to the username");
        assert!(own_senders(&config("", "apikey")).is_empty(), "a plain login is not an address");
        assert_eq!(own_senders(&config("a@x.test", "a@x.test")), ["a@x.test"], "no duplicates");
    }

    #[test]
    fn the_sender_match_ignores_case() {
        let c = config("Post Room <no-reply@x.test>", "");
        assert!(is_own(&c, "NO-REPLY@x.test", &[]));
        assert!(is_own(&c, " no-reply@x.test ", &[]));
    }

    #[test]
    fn the_header_signal_alone_is_enough() {
        assert!(is_own(&config("", ""), "anyone@elsewhere.test", &s(&[SIGNAL])));
    }

    #[test]
    fn a_stranger_using_the_post_room_subject_is_not_own() {
        let c = config("Post Room <no-reply@x.test>", "");
        assert!(!is_own(&c, "manager@client.test", &[]));
        assert!(!is_own(&c, "", &[]), "an empty sender never matches an empty config");
        assert!(!is_own(&config("", ""), "", &[]));
    }
}
