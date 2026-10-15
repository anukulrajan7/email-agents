use super::*;

pub struct NewAlert<'a> {
    pub workspace_id: &'a str,
    pub kind: &'a str,
    /// `high` or `normal` (anything else is stored as `normal`).
    pub severity: &'a str,
    pub title: String,
    pub body: String,
    pub message_id: Option<&'a str>,
    /// Level the alert is raised at (normally 1); part of the default dedupe key.
    pub level: i64,
    /// Overrides the default `kind:message_id:level` key.
    pub dedupe_key: Option<String>,
}

pub fn dedupe_key(kind: &str, message_id: Option<&str>, level: i64) -> String {
    format!("{kind}:{}:{level}", message_id.unwrap_or("-"))
}

/// Inserts the alert unless one with the same dedupe key exists (whatever its status).
/// Returns `(alert_id, created)`.
pub async fn raise(pool: &SqlitePool, a: NewAlert<'_>) -> sqlx::Result<(String, bool)> {
    let key = a.dedupe_key.clone().unwrap_or_else(|| dedupe_key(a.kind, a.message_id, a.level));
    let severity = if a.severity == "high" { "high" } else { "normal" };
    let level = a.level.clamp(1, MAX_LEVEL);
    let now = now_ms();
    let id = new_id("alr");
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let created = sqlx::query(
        "INSERT INTO alerts (id, workspace_id, kind, severity, title, body, message_id, dedupe_key, escalation_level,
                             level_since, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, ?10)
         ON CONFLICT(workspace_id, dedupe_key) DO NOTHING",
    )
    .bind(&id)
    .bind(a.workspace_id)
    .bind(a.kind)
    .bind(severity)
    .bind(crate::mail::sources::normalize::truncate_chars(&a.title, 200))
    .bind(crate::mail::sources::normalize::truncate_chars(&a.body, 1_000))
    .bind(a.message_id)
    .bind(&key)
    .bind(level)
    .bind(now)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    let id: String = sqlx::query_scalar("SELECT id FROM alerts WHERE workspace_id = ?1 AND dedupe_key = ?2")
        .bind(a.workspace_id)
        .bind(&key)
        .fetch_one(&mut *tx)
        .await?;
    if created {
        audit::record(
            &mut tx,
            Entry {
                workspace_id: a.workspace_id,
                actor: Actor::System,
                action: "alert.raised",
                entity_type: "alert",
                entity_id: &id,
                message_id: a.message_id,
                metadata: json!({ "kind": a.kind, "severity": severity, "level": level }),
            },
        )
        .await?;
    }
    tx.commit().await?;
    Ok((id, created))
}
