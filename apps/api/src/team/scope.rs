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
    let wall = wall(db, workspace_id, access).await?;
    if source_id.is_none() && person_id.is_none() {
        return Ok(wall);
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
    Ok(match (allowed, wall) {
        (Some(a), Some(w)) => Some(a.into_iter().filter(|s| w.contains(s)).collect()),
        (a, w) => a.or(w),
    })
}

/// The access wall. With the staffing pack's `restrict_mail_to_team` on, operators (not admins, not the super
/// admin) can only read mailboxes linked to people in their own manager-chain subtree. `None` = no wall.
pub async fn wall(db: &SqlitePool, workspace_id: &str, access: &Access) -> AppResult<Option<Vec<String>>> {
    if access.level != super::access::Level::Operator {
        return Ok(None);
    }
    if !super::settings::load(db, workspace_id).await?.restrict_mail_to_team || !crate::staffing::enabled(db, workspace_id).await? {
        return Ok(None);
    }
    let Some(visible) = access.visible() else { return Ok(None) };
    let ids = serde_json::json!(visible.iter().collect::<Vec<_>>()).to_string();
    Ok(Some(
        sqlx::query_scalar(
            "SELECT DISTINCT source_id FROM people WHERE workspace_id = ?1 AND active = 1 AND source_id IS NOT NULL
               AND id IN (SELECT value FROM json_each(?2))",
        )
        .bind(workspace_id)
        .bind(ids)
        .fetch_all(db)
        .await?,
    ))
}
