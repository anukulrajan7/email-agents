//! What the owner themselves said or typed counts as a supported fact: Ask Meera turns, draft instructions
//! and their own edits. Each author's words are stored separately (`draft_owner_words`), and a send is
//! supported only by its own author's words. Text from the sender (including quoted or forwarded text)
//! never lands here.

use sqlx::{SqliteConnection, SqlitePool};

use crate::{assistant::history, platform::db::now_ms, mail::sources::normalize::truncate_chars};

pub const MAX_CHARS: usize = 2_000;

/// The person's own recent turns in the Meera conversation that are about this email (they mention its sender
/// or subject) or came after it arrived, oldest first.
pub async fn conversation(db: &SqlitePool, ws: &str, user_id: &str, message_id: &str) -> sqlx::Result<String> {
    let msg: Option<(i64, String, String)> =
        sqlx::query_as("SELECT received_at, sender_address, subject FROM email_messages WHERE id = ?1 AND workspace_id = ?2").bind(message_id).bind(ws).fetch_optional(db).await?;
    let Some((received_at, sender, subject)) = msg else { return Ok(String::new()) };
    let sender = sender.to_lowercase();
    let local = sender.split('@').next().unwrap_or_default().to_string();
    let domain = sender.split('@').nth(1).unwrap_or_default().to_string();
    let subject_words: Vec<String> = subject.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() >= 5).map(str::to_string).collect();
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT text, created_at FROM assistant_messages WHERE workspace_id = ?1 AND user_id = ?2 AND role = 'user' ORDER BY id DESC LIMIT ?3",
    )
    .bind(ws)
    .bind(user_id)
    .bind(history::PLANNER_TURNS)
    .fetch_all(db)
    .await?;
    let mut lines: Vec<String> = rows
        .into_iter()
        .filter(|(text, at)| {
            let t = text.to_lowercase();
            *at >= received_at
                || t.contains(&sender)
                || (local.len() >= 4 && t.contains(&local))
                || (domain.len() >= 4 && !is_free_mail(&domain) && t.contains(domain.split('.').next().unwrap_or(&domain)))
                || subject_words.iter().any(|w| t.contains(w.as_str()))
        })
        .map(|(t, _)| truncate_chars(t.trim(), 500))
        .collect();
    lines.reverse();
    Ok(truncate_chars(&lines.join("\n"), MAX_CHARS))
}

fn is_free_mail(domain: &str) -> bool {
    matches!(domain, "gmail.com" | "googlemail.com" | "yahoo.com" | "outlook.com" | "hotmail.com" | "icloud.com")
}

fn sentences(text: &str) -> Vec<String> {
    text.split(['\n', '.', '!', '?']).map(|s| s.trim().to_string()).filter(|s| s.len() > 3).collect()
}

/// Sentences in `new` that were not in `old`: what the person typed into the draft themselves.
pub fn added_by_edit(old: &str, new: &str) -> String {
    let before: Vec<String> = sentences(old).iter().map(|s| s.to_lowercase()).collect();
    sentences(new).into_iter().filter(|s| !before.contains(&s.to_lowercase())).collect::<Vec<_>>().join(". ")
}

/// Adds the author's words to a draft. With no new words, a retry or second draft of the same email keeps the
/// provenance of the earlier one (every author's rows are copied).
pub async fn record(conn: &mut SqliteConnection, ws: &str, draft_id: &str, message_id: &str, user_id: Option<&str>, add: Option<&str>) -> sqlx::Result<()> {
    let add = add.map(str::trim).filter(|a| !a.is_empty());
    if let (Some(user), Some(add)) = (user_id, add) {
        sqlx::query("INSERT OR IGNORE INTO draft_owner_words (workspace_id, draft_id, user_id, text, created_at) VALUES (?1, ?2, ?3, ?4, ?5)")
            .bind(ws)
            .bind(draft_id)
            .bind(user)
            .bind(truncate_chars(add, MAX_CHARS))
            .bind(now_ms())
            .execute(&mut *conn)
            .await?;
        return Ok(());
    }
    sqlx::query(
        "INSERT OR IGNORE INTO draft_owner_words (workspace_id, draft_id, user_id, text, created_at)
         SELECT ?1, ?2, w.user_id, w.text, ?3 FROM draft_owner_words w
         WHERE w.draft_id = (SELECT d.id FROM drafts d WHERE d.message_id = ?4 AND d.workspace_id = ?1 AND d.id != ?2
                               AND EXISTS (SELECT 1 FROM draft_owner_words x WHERE x.draft_id = d.id)
                             ORDER BY d.created_at DESC, d.rowid DESC LIMIT 1)",
    )
    .bind(ws)
    .bind(draft_id)
    .bind(now_ms())
    .bind(message_id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// The words that support this send. A person's send is supported only by their own words; an automatic send
/// follows the person who made the request (the first author).
pub async fn for_send(db: &SqlitePool, draft_id: &str, actor: Option<&str>) -> sqlx::Result<Vec<String>> {
    match actor {
        Some(user) => {
            sqlx::query_scalar("SELECT text FROM draft_owner_words WHERE draft_id = ?1 AND user_id = ?2 ORDER BY id").bind(draft_id).bind(user).fetch_all(db).await
        }
        None => {
            sqlx::query_scalar(
                "SELECT text FROM draft_owner_words WHERE draft_id = ?1
                   AND user_id = (SELECT user_id FROM draft_owner_words WHERE draft_id = ?1 ORDER BY id LIMIT 1) ORDER BY id",
            )
            .bind(draft_id)
            .fetch_all(db)
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_person_typed_is_what_they_added() {
        let added = added_by_edit("Hello,\n\nThanks for writing.\n\nBest,\nHR", "Hello,\n\nThanks for writing. A UK company is hiring React Native devs.\n\nBest,\nHR");
        assert!(added.contains("UK company is hiring") && !added.contains("Thanks for writing"), "{added}");
        assert_eq!(added_by_edit("same text here", "same text here"), "");
    }
}
