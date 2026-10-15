//! Workspace labels: the taxonomy the AI classifies into, with human-written descriptions.

use sqlx::{SqliteConnection, SqlitePool};

use crate::{
    platform::db::{new_id, now_ms},
    platform::domain::{LabelTemplate, Label, OTHER_LABEL},
};

pub const MAX_ACTIVE_LABELS: i64 = 30;
pub const COLORS: &[&str] = &["slate", "sage", "plum", "ochre", "teal", "postmark", "ink", "rule"];

/// Active labels in display order. Always contains `other` (seeded as a system label).
pub async fn active(db: &SqlitePool, workspace_id: &str) -> sqlx::Result<Vec<Label>> {
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT key, name, description FROM labels WHERE workspace_id = ?1 AND archived = 0 ORDER BY position, created_at",
    )
    .bind(workspace_id)
    .fetch_all(db)
    .await?;
    let mut labels: Vec<Label> = rows.into_iter().map(|(key, name, description)| Label { key, name, description }).collect();
    if !labels.iter().any(|l| l.key == OTHER_LABEL) {
        labels.push(Label {
            key: OTHER_LABEL.into(),
            name: "Other".into(),
            description: "Anything that fits no other label.".into(),
        });
    }
    Ok(labels)
}

pub async fn is_active(db: &SqlitePool, workspace_id: &str, key: &str) -> sqlx::Result<bool> {
    if key == OTHER_LABEL {
        return Ok(true);
    }
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM labels WHERE workspace_id = ?1 AND key = ?2 AND archived = 0")
        .bind(workspace_id)
        .bind(key)
        .fetch_one(db)
        .await?;
    Ok(n > 0)
}

/// Keeps model output inside the workspace taxonomy: unknown keys become `other`.
pub fn coerce(labels: &[Label], key: &str) -> String {
    if labels.iter().any(|l| l.key == key) { key.to_string() } else { OTHER_LABEL.to_string() }
}

pub async fn seed(conn: &mut SqliteConnection, workspace_id: &str, template: &LabelTemplate) -> sqlx::Result<()> {
    let now = now_ms();
    for (i, l) in template.labels.iter().enumerate() {
        sqlx::query(
            "INSERT INTO labels (id, workspace_id, key, name, description, color, position, system, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )
        .bind(new_id("lbl"))
        .bind(workspace_id)
        .bind(l.key)
        .bind(l.name)
        .bind(l.description)
        .bind(l.color)
        .bind(i as i64)
        .bind(l.key == OTHER_LABEL)
        .bind(now)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// "Sales lead (EU)" → "sales_lead_eu".
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    out.trim_end_matches('_').chars().take(40).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_and_coercion() {
        assert_eq!(slugify("  Sales lead (EU) "), "sales_lead_eu");
        assert_eq!(slugify("Réunion"), "r_union");
        assert_eq!(slugify("!!!"), "");
        let labels = vec![Label { key: "billing".into(), name: "Billing".into(), description: "x".into() }];
        assert_eq!(coerce(&labels, "billing"), "billing");
        assert_eq!(coerce(&labels, "crypto"), "other");
    }
}
