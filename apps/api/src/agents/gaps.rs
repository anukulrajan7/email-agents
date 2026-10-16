//! Tools an agent asked for that do not exist. Each wish is counted per workspace so the owner
//! sees what to build; nothing about the wish fails a case.

use sqlx::SqliteConnection;

use crate::platform::db::{new_id, now_ms};

pub struct NewGap<'a> {
    pub tool: &'a str,
    pub why: &'a str,
    pub agent_id: Option<&'a str>,
    pub message_id: Option<&'a str>,
    /// `designer`, `planner`, `rule` or `system`.
    pub source: &'a str,
}

/// Counts one more request for `gap.tool` (the first one creates the row).
pub async fn record(conn: &mut SqliteConnection, ws: &str, gap: NewGap<'_>) -> sqlx::Result<()> {
    let now = now_ms();
    sqlx::query(
        "INSERT INTO tool_gaps (id, workspace_id, tool, why, agent_id, message_id, source, first_seen_at, last_seen_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
         ON CONFLICT (workspace_id, tool) DO UPDATE SET
           count = count + 1, last_seen_at = ?8, why = ?4, agent_id = ?5, message_id = ?6",
    )
    .bind(new_id("gap"))
    .bind(ws)
    .bind(gap.tool)
    .bind(gap.why)
    .bind(gap.agent_id)
    .bind(gap.message_id)
    .bind(gap.source)
    .bind(now)
    .execute(conn)
    .await?;
    Ok(())
}
