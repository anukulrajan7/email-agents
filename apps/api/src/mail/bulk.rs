//! Deterministic filter for bulk and automated mail. Header evidence is stored at ingest
//! (`email_messages.bulk_signals`); at labelling time a message is filed without any model call only
//! when there are at least two signals, the sender is not someone we know, and the content is not
//! billing-, legal- or security-like. A person opening the email always gets the full treatment.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;
use sqlx::SqlitePool;

/// Model id on rules-written insights and classifications.
pub const MODEL_ID: &str = "bulk-rules-v1";
/// Signals needed (header evidence plus a no-reply style sender) before mail is filed without a model.
pub const MIN_SIGNALS: usize = 2;

pub const LIST_UNSUBSCRIBE: &str = "list_unsubscribe";
pub const PRECEDENCE: &str = "precedence";
pub const AUTO_SUBMITTED: &str = "auto_submitted";
pub const GMAIL_CATEGORY: &str = "gmail_category";
pub const AUTO_REPLY: &str = "auto_reply";
pub const DELIVERY_STATUS: &str = "delivery_status";
pub const NO_REPLY_SENDER: &str = "no_reply_sender";

const GMAIL_CATEGORIES: &[&str] = &["CATEGORY_PROMOTIONS", "CATEGORY_SOCIAL", "CATEGORY_UPDATES", "CATEGORY_FORUMS"];

/// Senders such as `billing@` or `payroll@` act on money, contracts or accounts: never filed away.
const SAFE_LOCAL_PARTS: &[&str] =
    &["billing", "invoice", "invoices", "accounts", "accounting", "payments", "payment", "finance", "payroll", "receivables", "payables", "ar", "ap", "legal", "security", "orders", "tax"];

static BILLING_LIKE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(invoices?|receipts?|billing|bills?|payments?|payable|receivable|overdue|past due|amount due|balance due|statements?|remittance|refunds?|purchase order|tax|gst|vat|payslips?|salary|payroll|contracts?|agreements?|renewal|subscription|security alert|verification code|one-time|password|action required|legal|court|subpoena)\b",
    )
    .expect("valid pattern")
});
static REPLY_SUBJECT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^\s*(re|fwd?|aw|wg)\s*:").expect("valid pattern"));

/// Header-derived signals. `header` looks a header up by name; `label_ids` are the provider's labels (Gmail).
pub fn header_signals(header: &dyn Fn(&str) -> Option<String>, label_ids: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    if header("List-Unsubscribe").is_some() || header("List-Unsubscribe-Post").is_some() {
        out.push(LIST_UNSUBSCRIBE.to_string());
    }
    if header("Precedence").is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "bulk" | "list" | "junk")) {
        out.push(PRECEDENCE.to_string());
    }
    if header("Auto-Submitted").is_some_and(|v| !v.trim().eq_ignore_ascii_case("no")) {
        out.push(AUTO_SUBMITTED.to_string());
    }
    if header("X-Autoreply").is_some() || header("X-Autorespond").is_some() {
        out.push(AUTO_REPLY.to_string());
    }
    if header("Content-Type").is_some_and(|v| {
        let v = v.to_ascii_lowercase();
        v.contains("report-type=delivery-status") || v.contains("multipart/report")
    }) {
        out.push(DELIVERY_STATUS.to_string());
    }
    if header("X-PostRoom").is_some() {
        out.push(crate::mail::system_mail::SIGNAL.to_string());
    }
    if label_ids.iter().any(|l| GMAIL_CATEGORIES.contains(&l.as_str())) {
        out.push(GMAIL_CATEGORY.to_string());
    }
    out
}

/// Merges new signals into an existing list without duplicates.
pub fn merge(into: &mut Vec<String>, more: Vec<String>) {
    for s in more {
        if !into.contains(&s) {
            into.push(s);
        }
    }
}

/// Headers as a lookup: an object (`{"List-Unsubscribe": "…"}`) or an array of `{name, value}`.
pub fn header_lookup(headers: &Value) -> impl Fn(&str) -> Option<String> + '_ {
    move |name: &str| match headers {
        Value::Object(map) => map
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .and_then(|(_, v)| v.as_str().map(str::to_string))
            .filter(|v| !v.trim().is_empty()),
        Value::Array(items) => items
            .iter()
            .find(|h| h.get("name").and_then(Value::as_str).is_some_and(|n| n.eq_ignore_ascii_case(name)))
            .and_then(|h| h.get("value").and_then(Value::as_str))
            .map(str::to_string)
            .filter(|v| !v.trim().is_empty()),
        _ => None,
    }
}

fn local_part(address: &str) -> String {
    address.rsplit_once('@').map(|(l, _)| l).unwrap_or(address).to_ascii_lowercase()
}

/// True for money, legal or account-security content, or a sender that handles those.
pub fn billing_like(sender: &str, subject: &str, new_part: &str) -> bool {
    let local = local_part(sender);
    let local = local.split(['+', '.', '-', '_']).next().unwrap_or_default();
    if SAFE_LOCAL_PARTS.contains(&local) {
        return true;
    }
    let head = crate::mail::sources::normalize::truncate_chars(new_part, 1_500);
    BILLING_LIKE.is_match(subject) || BILLING_LIKE.is_match(&head)
}

/// Someone we have dealt with: we wrote to them, a reply to them was sent, a person labelled their mail,
/// or the workspace holds a memory about them.
pub async fn is_known_contact(db: &SqlitePool, workspace_id: &str, sender: &str) -> sqlx::Result<bool> {
    let sender = sender.trim().to_ascii_lowercase();
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM email_messages WHERE workspace_id = ?1 AND direction = 'outbound' AND instr(lower(recipients_json), ?2) > 0)
             OR EXISTS (SELECT 1 FROM drafts d JOIN email_messages m ON m.id = d.message_id
                        WHERE d.workspace_id = ?1 AND d.status = 'sent' AND lower(m.sender_address) = ?2)
             OR EXISTS (SELECT 1 FROM classifications c JOIN email_messages m ON m.id = c.message_id
                        WHERE m.workspace_id = ?1 AND lower(m.sender_address) = ?2 AND c.source = 'manual')
             OR EXISTS (SELECT 1 FROM memories WHERE workspace_id = ?1 AND status = 'active' AND lower(contact_email) = ?2)",
    )
    .bind(workspace_id)
    .bind(sender)
    .fetch_one(db)
    .await
}

/// Pure part of the decision: enough signals, a reply-less subject and nothing billing-like.
pub fn qualifies(stored: &[String], sender: &str, subject: &str, new_part: &str) -> bool {
    if stored.iter().any(|s| s == crate::mail::system_mail::SIGNAL) {
        return false;
    }
    let mut signals = stored.to_vec();
    if crate::learning::memory_learning::is_automated_sender(sender) {
        merge(&mut signals, vec![NO_REPLY_SENDER.to_string()]);
    }
    signals.len() >= MIN_SIGNALS && !REPLY_SUBJECT.is_match(subject) && !billing_like(sender, subject, new_part)
}

/// Signals to file this message under, or `None` when it needs the models (or a person).
pub async fn should_file(db: &SqlitePool, workspace_id: &str, message_id: &str) -> sqlx::Result<Option<Vec<String>>> {
    let row: Option<(String, String, String, String)> =
        sqlx::query_as("SELECT sender_address, subject, body_text, bulk_signals FROM email_messages WHERE id = ?1")
            .bind(message_id)
            .fetch_optional(db)
            .await?;
    let Some((sender, subject, body, stored)) = row else { return Ok(None) };
    let stored: Vec<String> = serde_json::from_str(&stored).unwrap_or_default();
    if stored.is_empty() {
        return Ok(None);
    }
    let new_part = crate::ai::body::model_body(&subject, &body);
    if !qualifies(&stored, &sender, &subject, &new_part) || is_known_contact(db, workspace_id, &sender).await? {
        return Ok(None);
    }
    let mut signals = stored;
    if crate::learning::memory_learning::is_automated_sender(&sender) {
        merge(&mut signals, vec![NO_REPLY_SENDER.to_string()]);
    }
    Ok(Some(signals))
}

/// A person opened a message that was filed by rules: drop the rules insight so the real reading runs.
/// Returns true when there was one.
pub async fn release_for_person(db: &SqlitePool, message_id: &str) -> sqlx::Result<bool> {
    Ok(sqlx::query("DELETE FROM message_insights WHERE message_id = ?1 AND model_id = ?2")
        .bind(message_id)
        .bind(MODEL_ID)
        .execute(db)
        .await?
        .rows_affected()
        > 0)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|i| (*i).to_string()).collect()
    }

    #[test]
    fn header_signals_cover_the_four_header_kinds() {
        let headers = json!({"list-unsubscribe": "<mailto:u@x.co>", "Precedence": "bulk", "Auto-Submitted": "auto-generated"});
        let look = header_lookup(&headers);
        assert_eq!(header_signals(&look, &s(&["INBOX", "CATEGORY_PROMOTIONS"])), [LIST_UNSUBSCRIBE, PRECEDENCE, AUTO_SUBMITTED, GMAIL_CATEGORY]);
        let plain = json!([{"name": "Auto-Submitted", "value": "no"}, {"name": "Precedence", "value": "first-class"}]);
        assert!(header_signals(&header_lookup(&plain), &s(&["CATEGORY_PERSONAL", "INBOX"])).is_empty());
        let array = json!([{"name": "List-Unsubscribe", "value": "<https://x.co/u>"}]);
        assert_eq!(header_signals(&header_lookup(&array), &[]), [LIST_UNSUBSCRIBE]);
    }

    #[test]
    fn a_newsletter_with_two_signals_qualifies() {
        assert!(qualifies(&s(&[LIST_UNSUBSCRIBE, GMAIL_CATEGORY]), "news@brand.example", "Our October picks", "Ten books you will love."));
        assert!(qualifies(&s(&[LIST_UNSUBSCRIBE]), "no-reply@brand.example", "Weekly digest", "Top stories this week."), "no-reply counts as the second signal");
    }

    #[test]
    fn one_signal_is_never_enough() {
        assert!(!qualifies(&s(&[LIST_UNSUBSCRIBE]), "sam@brand.example", "Hello", "Quick question about your order status."));
        assert!(!qualifies(&[], "no-reply@brand.example", "Hello", "Hi"));
    }

    #[test]
    fn billing_mail_with_an_unsubscribe_footer_is_not_filed_away() {
        let signals = s(&[LIST_UNSUBSCRIBE, GMAIL_CATEGORY]);
        assert!(!qualifies(&signals, "billing@vendor.example", "Your October statement", "Amount due: 1,200 INR by 28 Oct. Unsubscribe."));
        assert!(!qualifies(&signals, "billing@vendor.example", "Hello", "Thanks for being a customer."), "billing@ is allowlisted by itself");
        assert!(!qualifies(&signals, "noreply@vendor.example", "Invoice INV-2291 is ready", "Download your invoice."));
        assert!(!qualifies(&signals, "alerts@bank.example", "Security alert: new sign-in", "We noticed a sign-in."));
        assert!(!qualifies(&signals, "payroll+mail@corp.example", "October", "See attached."), "plus-addressing does not dodge the allowlist");
    }

    #[test]
    fn gmail_records_and_forwarded_mail_carry_their_signals_into_the_message() {
        let rec = json!({
            "id": "g1", "labelIds": ["INBOX", "CATEGORY_UPDATES"], "snippet": "hi",
            "payload": {"headers": [{"name": "From", "value": "News <news@brand.example>"},
                                    {"name": "List-Unsubscribe", "value": "<mailto:u@brand.example>"}]}
        });
        let mapped = crate::mail::sources::nango::map_gmail_record(&rec, 5_000).unwrap();
        assert_eq!(mapped.email.bulk_signals, [LIST_UNSUBSCRIBE, GMAIL_CATEGORY]);

        let forwarded: crate::mail::sources::resend::ReceivedEmail = serde_json::from_value(
            json!({"id": "r1", "from": "x@y.co", "headers": {"Precedence": "bulk", "Auto-Submitted": "auto-generated"}}),
        )
        .unwrap();
        assert_eq!(forwarded.normalize(5_000).bulk_signals, [PRECEDENCE, AUTO_SUBMITTED]);
    }

    #[test]
    fn our_own_header_becomes_a_signal() {
        let headers = json!({"x-postroom": "notice", "Auto-Submitted": "auto-generated"});
        assert_eq!(header_signals(&header_lookup(&headers), &[]), [AUTO_SUBMITTED, crate::mail::system_mail::SIGNAL]);
        assert!(header_signals(&header_lookup(&json!({})), &[]).is_empty());
    }

    #[test]
    fn the_postroom_signal_never_makes_mail_bulk() {
        let own = crate::mail::system_mail::SIGNAL;
        assert!(!qualifies(&s(&[own]), "someone@brand.example", "Our October picks", "Ten books you will love."), "the signal alone is not evidence");
        assert!(!qualifies(&s(&[own]), "no-reply@brand.example", "Weekly digest", "Top stories this week."), "nor with a no-reply sender");
        assert!(!qualifies(&s(&[own, AUTO_SUBMITTED]), "no-reply@postroom.test", "[Post Room] 2 emails need attention", "Two emails."), "our mail also carries Auto-Submitted");
    }

    #[test]
    fn replies_are_never_bulk() {
        assert!(!qualifies(&s(&[LIST_UNSUBSCRIBE, GMAIL_CATEGORY]), "news@brand.example", "Re: your question", "Following up."));
    }
}
