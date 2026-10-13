//! Per-person mailbox scoping for the inbox: narrows a workspace's mail to one mailbox or to the
//! mailbox linked to one person. The result is a list of source ids the message query must stay inside.

use sqlx::SqlitePool;

use super::access::Access;
use crate::error::{AppError, AppResult};

/// `None` = no filter. `Some(ids)` = only mail stored under these mailboxes (an empty list matches nothing).
pub async fn source_filter(
    db: &SqlitePool,
    workspace_id: &str,
    access: &Access,
    source_id: Option<&str>,
    person_id: Option<&str>,
) -> AppResult<Option<Vec<String>>> {
    let source_id = source_id.filter(|s| !s.is_empty());
    let person_id = person_id.filter(|s| !s.is_empty());
    if source_id.is_none() && person_id.is_none() {
        return Ok(None);
    }
    let mut allowed: Option<Vec<String>> = None;
    if let Some(s) = source_id {
        let found: Option<String> = sqlx::query_scalar("SELECT id FROM email_sources WHERE id = ?1 AND workspace_id = ?2")
            .bind(s)
            .bind(workspace_id)
            .fetch_optional(db)
            .await?;
        allowed = Some(found.into_iter().collect());
        if allowed.as_ref().is_some_and(Vec::is_empty) {
            return Err(AppError::BadRequest("unknown mailbox".into()));
        }
    }
    if let Some(p) = person_id {
        access.require_visible(p)?;
        let linked: Vec<String> = sqlx::query_scalar(
            "SELECT source_id FROM people WHERE id = ?1 AND workspace_id = ?2 AND source_id IS NOT NULL AND active = 1",
        )
        .bind(p)
        .bind(workspace_id)
        .fetch_all(db)
        .await?;
        let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM people WHERE id = ?1 AND workspace_id = ?2")
            .bind(p)
            .bind(workspace_id)
            .fetch_optional(db)
            .await?;
        if exists.is_none() {
            return Err(AppError::BadRequest("unknown person".into()));
        }
        allowed = Some(match allowed {
            Some(a) => a.into_iter().filter(|s| linked.contains(s)).collect(),
            None => linked,
        });
    }
    Ok(allowed)
}
