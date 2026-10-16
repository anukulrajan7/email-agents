//! Post Room's own system mail (alert digests, "No reply yet" notices) lands in the owner's connected
//! mailbox like any other message. It must never get a draft, a case, an automation run or an alert.
//! The one proof is the `X-PostRoom` header we stamp on outgoing mail, stored at ingest as the
//! `postroom` signal. The sender address is never consulted (the SMTP account is a real mailbox the
//! owner also writes from) and neither is the subject (anyone can type "[Post Room]").

use sqlx::SqlitePool;

/// Stored in `email_messages.bulk_signals` when the message carries our `X-PostRoom` header.
pub const SIGNAL: &str = "postroom";

pub fn is_own(signals: &[String]) -> bool {
    signals.iter().any(|s| s == SIGNAL)
}

pub async fn is_own_message(db: &SqlitePool, workspace_id: &str, message_id: &str) -> sqlx::Result<bool> {
    let stored: Option<String> = sqlx::query_scalar("SELECT bulk_signals FROM email_messages WHERE id = ?1 AND workspace_id = ?2")
        .bind(message_id)
        .bind(workspace_id)
        .fetch_optional(db)
        .await?;
    Ok(stored.is_some_and(|stored| is_own(&serde_json::from_str::<Vec<String>>(&stored).unwrap_or_default())))
}

/// SQL predicate (for a query over `email_messages` aliased `alias`) that excludes our own mail.
pub fn not_own_sql(alias: &str) -> String {
    format!("{alias}.bulk_signals NOT LIKE '%\"{SIGNAL}\"%'")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|i| (*i).to_string()).collect()
    }

    #[test]
    fn the_header_signal_makes_mail_own() {
        assert!(is_own(&s(&[SIGNAL])));
        assert!(is_own(&s(&["auto_submitted", SIGNAL])));
    }

    #[test]
    fn mail_without_our_header_is_not_own_whoever_sent_it() {
        assert!(!is_own(&[]));
        assert!(!is_own(&s(&["auto_submitted", "no_reply_sender"])));
    }

    #[test]
    fn the_test_pack_header_is_not_our_header() {
        use crate::mail::bulk::{header_lookup, header_signals};
        let headers = json!({"X-PostRoom-Test": "1"});
        assert!(!is_own(&header_signals(&header_lookup(&headers), &[])));
    }
}
