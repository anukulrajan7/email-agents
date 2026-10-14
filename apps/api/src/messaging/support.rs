//! Vendor and bank support cases. Meera finds the company's support address in the email (or the contacts you
//! already have) and never guesses one; the first email always waits for the person's OK; a ticket tracks the
//! case until the company's answer says it is done.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::{
    db::{iso, new_id, now_ms},
    error::{AppError, AppResult},
    http::AppState,
    send_safety::{bare_address, is_automated_address, valid_address},
    sources::normalize::truncate_chars,
};

static ADDRESS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+").expect("valid regex"));
static SUPPORTISH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:support|help|helpdesk|care|customer[._-]?(?:care|service|support)|service|services|contact|grievance|assist|enquir(?:y|ies)|queries|query|cards?|customercare|feedback)(?:[._+-].*)?@")
        .expect("valid regex")
});
static RESOLVED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:resolved|has been (?:refunded|reversed|credited|processed|fixed|activated|completed|corrected)|have (?:refunded|reversed|credited|processed|fixed|corrected)|we(?:'ve| have) (?:refunded|reversed|credited|processed|fixed|resolved|corrected)|issue (?:is )?(?:now )?(?:resolved|fixed)|closing (?:this|the) (?:ticket|case|request)|no further action)\b",
    )
    .expect("valid regex")
});
static NOT_RESOLVED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(?:not|n't|yet to be|un)\s*(?:yet\s*)?(?:been\s*)?(?:resolved|refunded|processed|fixed|credited)\b").expect("valid regex"));

/// Does the company's reply say the case is closed?
pub fn looks_resolved(text: &str) -> bool {
    RESOLVED.is_match(text) && !NOT_RESOLVED.is_match(text)
}

#[derive(Debug, Clone)]
pub struct SupportAddress {
    pub address: String,
    /// Plain words for the confirm card.
    pub source: &'static str,
    pub company: String,
}

fn company_of(name: Option<&str>, address: &str) -> String {
    if let Some(n) = name.map(str::trim).filter(|n| !n.is_empty() && !n.contains('@')) {
        return truncate_chars(n, 60);
    }
    let domain = address.rsplit('@').next().unwrap_or(address);
    let core = domain.split('.').next().unwrap_or(domain);
    let mut chars = core.chars();
    chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_else(|| core.to_string())
}

/// The company's support address, taken from the email or a contact in the same domain. `None` when it is not there:
/// the address is never made up from a domain.
pub async fn find_address(db: &SqlitePool, ws: &str, message_id: &str) -> sqlx::Result<Option<SupportAddress>> {
    let row: Option<(String, Option<String>, String)> = sqlx::query_as(
        "SELECT sender_address, sender_name, body_text FROM email_messages WHERE id = ?1 AND workspace_id = ?2 AND suppressed = 0",
    )
    .bind(message_id)
    .bind(ws)
    .fetch_optional(db)
    .await?;
    let Some((sender, name, body)) = row else { return Ok(None) };
    let sender = bare_address(&sender);
    let company = company_of(name.as_deref(), &sender);
    let ok = |a: &str| valid_address(a) && !is_automated_address(a) && SUPPORTISH.is_match(a);
    if ok(&sender) {
        return Ok(Some(SupportAddress { address: sender, source: "the sender of that email", company }));
    }
    for m in ADDRESS.find_iter(&body) {
        let candidate = bare_address(m.as_str().trim_end_matches('.'));
        if ok(&candidate) {
            let company = company_of(name.as_deref(), &candidate);
            return Ok(Some(SupportAddress { address: candidate, source: "written in that email", company }));
        }
    }
    let domain = sender.rsplit('@').next().unwrap_or_default().to_string();
    if !domain.is_empty() {
        let contacts: Vec<String> = sqlx::query_scalar("SELECT email FROM contacts WHERE workspace_id = ?1 AND lower(email) LIKE ?2 ORDER BY message_count DESC")
            .bind(ws)
            .bind(format!("%@{domain}"))
            .fetch_all(db)
            .await?;
        if let Some(found) = contacts.into_iter().map(|c| bare_address(&c)).find(|c| ok(c)) {
            return Ok(Some(SupportAddress { address: found, source: "one of your contacts at that company", company }));
        }
    }
    Ok(None)
}

pub struct NewTicket<'a> {
    pub ws: &'a str,
    pub user_id: &'a str,
    pub watch_id: &'a str,
    pub company: &'a str,
    pub address: &'a str,
    pub address_source: &'a str,
    pub subject: &'a str,
    pub source_message_id: Option<&'a str>,
}

pub async fn create_ticket(conn: &mut sqlx::SqliteConnection, t: NewTicket<'_>) -> sqlx::Result<String> {
    let id = new_id("tkt");
    let now = now_ms();
    sqlx::query(
        "INSERT INTO support_tickets (id, workspace_id, user_id, watch_id, company, support_address, address_source, subject, source_message_id,
                                      status, last_summary, last_update_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'open', 'Waiting for your OK to write to them.', ?10, ?10)",
    )
    .bind(&id)
    .bind(t.ws)
    .bind(t.user_id)
    .bind(t.watch_id)
    .bind(truncate_chars(t.company, 80))
    .bind(t.address)
    .bind(t.address_source)
    .bind(truncate_chars(t.subject, 200))
    .bind(t.source_message_id)
    .bind(now)
    .execute(conn)
    .await?;
    Ok(id)
}

/// The caller's tickets, newest update first.
pub async fn list(state: &AppState, ws: &str, user_id: &str) -> sqlx::Result<Vec<Value>> {
    let rows: Vec<(String, String, String, String, String, String, i64)> = sqlx::query_as(
        "SELECT id, company, support_address, subject, status, last_summary, last_update_at FROM support_tickets
         WHERE workspace_id = ?1 AND user_id = ?2 ORDER BY last_update_at DESC, id DESC LIMIT 50",
    )
    .bind(ws)
    .bind(user_id)
    .fetch_all(&state.db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, company, address, subject, status, summary, at)| {
            json!({ "id": id, "company": company, "address": address, "subject": subject, "status": status, "last_summary": summary, "updated_at": iso(at) })
        })
        .collect())
}

/// The person marks a ticket resolved, or reopens it.
pub async fn set_status(state: &AppState, ws: &str, user_id: &str, id: &str, status: &str) -> AppResult<()> {
    if !matches!(status, "open" | "waiting" | "resolved") {
        return Err(AppError::BadRequest("status must be open, waiting or resolved".into()));
    }
    let done = sqlx::query("UPDATE support_tickets SET status = ?1, last_update_at = ?2 WHERE id = ?3 AND workspace_id = ?4 AND user_id = ?5")
        .bind(status)
        .bind(now_ms())
        .bind(id)
        .bind(ws)
        .bind(user_id)
        .execute(&state.db)
        .await?
        .rows_affected();
    if done == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolution_is_read_from_the_reply() {
        assert!(looks_resolved("Hi, the amount has been refunded to your card."));
        assert!(looks_resolved("We've resolved this. Closing this ticket."));
        assert!(!looks_resolved("Your refund has not been processed yet."));
        assert!(!looks_resolved("We are looking into it and will update you."));
    }

    #[test]
    fn support_addresses_look_like_support() {
        assert!(SUPPORTISH.is_match("customercare@hdfcbank.com"));
        assert!(SUPPORTISH.is_match("support@vendor.io"));
        assert!(!SUPPORTISH.is_match("ravi@vendor.io"));
    }
}
