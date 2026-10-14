//! What the owner themselves said or typed counts as a supported fact: Ask Meera turns, draft instructions
//! and their own edits. It is stored on the draft with its author, so one member's words never support
//! another member's send. Text from the sender (including quoted or forwarded text) never lands here.

use sqlx::{SqliteConnection, SqlitePool};

use crate::{assistant::history, db::now_ms, sources::normalize::truncate_chars};

pub const MAX_CHARS: usize = 2_000;

/// The person's own recent turns in the Meera conversation (the window used for a draft), oldest first.
pub async fn conversation(db: &SqlitePool, ws: &str, user_id: &str) -> sqlx::Result<String> {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT text FROM assistant_messages WHERE workspace_id = ?1 AND user_id = ?2 AND role = 'user' ORDER BY id DESC LIMIT ?3",
    )
    .bind(ws)
    .bind(user_id)
    .bind(history::PLANNER_TURNS)
    .fetch_all(db)
    .await?;
    let mut lines: Vec<String> = rows.iter().map(|t| truncate_chars(t.trim(), 500)).collect();
    lines.reverse();
    Ok(truncate_chars(&lines.join("\n"), MAX_CHARS))
}

fn sentences(text: &str) -> Vec<String> {
    text.split(['\n', '.', '!', '?']).map(|s| s.trim().to_string()).filter(|s| s.len() > 3).collect()
}

/// Sentences in `new` that were not in `old`: what the person typed into the draft themselves.
pub fn added_by_edit(old: &str, new: &str) -> String {
    let before: Vec<String> = sentences(old).iter().map(|s| s.to_lowercase()).collect();
    sentences(new).into_iter().filter(|s| !before.contains(&s.to_lowercase())).collect::<Vec<_>>().join(". ")
}

pub fn merge(existing: Option<&str>, add: &str) -> String {
    let add = add.trim();
    match existing.map(str::trim).filter(|e| !e.is_empty()) {
        Some(e) if add.is_empty() || e.contains(add) => e.to_string(),
        Some(e) => truncate_chars(&format!("{e}\n{add}"), MAX_CHARS),
        None => truncate_chars(add, MAX_CHARS),
    }
}

/// Adds the author's words to a draft. With no new words, a retry or second draft of the same email keeps the
/// provenance of the earlier one.
pub async fn record(conn: &mut SqliteConnection, ws: &str, draft_id: &str, message_id: &str, user_id: Option<&str>, add: Option<&str>) -> sqlx::Result<()> {
    let add = add.map(str::trim).filter(|a| !a.is_empty());
    if let (Some(user), Some(add)) = (user_id, add) {
        let current: Option<String> = sqlx::query_scalar("SELECT instructions_by_user FROM drafts WHERE id = ?1").bind(draft_id).fetch_one(&mut *conn).await?;
        sqlx::query("UPDATE drafts SET instructions_by_user = ?1, instructions_user_id = COALESCE(instructions_user_id, ?2) WHERE id = ?3")
            .bind(merge(current.as_deref(), add))
            .bind(user)
            .bind(draft_id)
            .execute(&mut *conn)
            .await?;
        return Ok(());
    }
    let earlier: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT instructions_by_user, instructions_user_id FROM drafts
         WHERE message_id = ?1 AND workspace_id = ?2 AND id != ?3 AND instructions_by_user IS NOT NULL ORDER BY created_at DESC, rowid DESC LIMIT 1",
    )
    .bind(message_id)
    .bind(ws)
    .bind(draft_id)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some((text, author)) = earlier {
        sqlx::query("UPDATE drafts SET instructions_by_user = COALESCE(instructions_by_user, ?1), instructions_user_id = COALESCE(instructions_user_id, ?2), updated_at = ?3 WHERE id = ?4")
            .bind(text)
            .bind(author)
            .bind(now_ms())
            .bind(draft_id)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
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

    #[test]
    fn merge_keeps_both_and_dedupes() {
        assert_eq!(merge(Some("a fact"), "b fact"), "a fact\nb fact");
        assert_eq!(merge(Some("a fact"), "a fact"), "a fact");
        assert_eq!(merge(None, "  x  "), "x");
    }
}
