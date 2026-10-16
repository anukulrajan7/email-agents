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

/// Most distinct tool names kept per workspace; a model inventing names cannot grow the table without limit.
pub const MAX_GAPS_PER_WORKSPACE: i64 = 200;
const MAX_NAME_CHARS: usize = 60;

/// Counts one more request for `gap.tool` (the first one creates the row). Past the cap, names already
/// recorded still count and new names are dropped.
pub async fn record(conn: &mut SqliteConnection, ws: &str, gap: NewGap<'_>) -> sqlx::Result<()> {
    let now = now_ms();
    let tool: String = gap.tool.chars().take(MAX_NAME_CHARS).collect();
    sqlx::query(
        "INSERT INTO tool_gaps (id, workspace_id, tool, why, agent_id, message_id, source, first_seen_at, last_seen_at)
         SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8
         WHERE (SELECT COUNT(*) FROM tool_gaps WHERE workspace_id = ?2) < ?9
            OR EXISTS (SELECT 1 FROM tool_gaps WHERE workspace_id = ?2 AND tool = ?3)
         ON CONFLICT (workspace_id, tool) DO UPDATE SET
           count = count + 1, last_seen_at = ?8, why = ?4, agent_id = ?5, message_id = ?6",
    )
    .bind(new_id("gap"))
    .bind(ws)
    .bind(&tool)
    .bind(gap.why)
    .bind(gap.agent_id)
    .bind(gap.message_id)
    .bind(gap.source)
    .bind(now)
    .bind(MAX_GAPS_PER_WORKSPACE)
    .execute(conn)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::{Connection, SqliteConnection};

    async fn conn() -> SqliteConnection {
        let mut c = SqliteConnection::connect("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE tool_gaps (id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, tool TEXT NOT NULL, why TEXT NOT NULL DEFAULT '', agent_id TEXT, message_id TEXT, source TEXT NOT NULL, count INTEGER NOT NULL DEFAULT 1, status TEXT NOT NULL DEFAULT 'open', first_seen_at INTEGER NOT NULL, last_seen_at INTEGER NOT NULL, UNIQUE (workspace_id, tool))")
            .execute(&mut c)
            .await
            .unwrap();
        c
    }

    fn gap(tool: &str) -> NewGap<'_> {
        NewGap { tool, why: "", agent_id: None, message_id: None, source: "planner" }
    }

    #[tokio::test]
    async fn the_table_is_capped_per_workspace_but_known_names_keep_counting() {
        let mut c = conn().await;
        for n in 0..MAX_GAPS_PER_WORKSPACE + 25 {
            record(&mut c, "ws1", gap(&format!("tool_{n}"))).await.unwrap();
        }
        record(&mut c, "ws1", gap("tool_0")).await.unwrap();
        record(&mut c, "ws2", gap("tool_new")).await.unwrap();
        let rows = |ws: &'static str| sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tool_gaps WHERE workspace_id = ?1").bind(ws);
        assert_eq!(rows("ws1").fetch_one(&mut c).await.unwrap(), MAX_GAPS_PER_WORKSPACE);
        assert_eq!(rows("ws2").fetch_one(&mut c).await.unwrap(), 1, "another workspace has its own room");
        let first: i64 = sqlx::query_scalar("SELECT count FROM tool_gaps WHERE workspace_id = 'ws1' AND tool = 'tool_0'").fetch_one(&mut c).await.unwrap();
        assert_eq!(first, 2);
    }

    #[tokio::test]
    async fn a_long_name_is_cut_to_sixty_characters_and_counts_as_one_row() {
        let mut c = conn().await;
        let long = "x".repeat(300);
        record(&mut c, "ws1", gap(&long)).await.unwrap();
        record(&mut c, "ws1", gap(&long)).await.unwrap();
        let (name, count): (String, i64) = sqlx::query_as("SELECT tool, count FROM tool_gaps").fetch_one(&mut c).await.unwrap();
        assert_eq!((name.chars().count(), count), (60, 2));
    }
}
