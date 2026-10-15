use super::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlertSettings {
    pub enabled: bool,
    pub recipients: Vec<String>,
    pub escalate_to: Vec<String>,
    pub unanswered_hours: i64,
    pub escalate_after_hours: i64,
    pub daily_digest: bool,
    pub digest_hour: i64,
}

pub fn valid_email(e: &str) -> bool {
    let Some((local, domain)) = e.split_once('@') else { return false };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains('@')
        && e.len() <= 254
        && !e.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | ',' | ';' | '"'))
}

fn clean_list(list: &[String], field: &str) -> Result<Vec<String>, String> {
    let mut seen = HashSet::new();
    let mut out = vec![];
    for e in list {
        let e = e.trim().to_lowercase();
        if !valid_email(&e) {
            return Err(format!("{field}: '{e}' is not a valid email address"));
        }
        if seen.insert(e.clone()) {
            out.push(e);
        }
    }
    if out.len() > 20 {
        return Err(format!("{field}: at most 20 addresses"));
    }
    Ok(out)
}

impl AlertSettings {
    /// Normalises addresses (trim, lower-case, dedupe) and checks ranges.
    pub fn validated(mut self) -> Result<Self, String> {
        self.recipients = clean_list(&self.recipients, "recipients")?;
        self.escalate_to = clean_list(&self.escalate_to, "escalate_to")?;
        if !(1..=168).contains(&self.unanswered_hours) {
            return Err("unanswered_hours must be 1-168".into());
        }
        if !(1..=720).contains(&self.escalate_after_hours) {
            return Err("escalate_after_hours must be 1-720".into());
        }
        if !(0..=23).contains(&self.digest_hour) {
            return Err("digest_hour must be 0-23 (UTC)".into());
        }
        Ok(self)
    }
}

async fn admin_emails(pool: &SqlitePool, workspace_id: &str) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar(
        "SELECT lower(u.email) FROM memberships mb JOIN users u ON u.id = mb.user_id
         WHERE mb.workspace_id = ?1 AND mb.role = 'admin' ORDER BY u.email",
    )
    .bind(workspace_id)
    .fetch_all(pool)
    .await
}

#[derive(sqlx::FromRow)]
struct SettingsRow {
    enabled: bool,
    recipients_json: Option<String>,
    escalate_to_json: String,
    unanswered_hours: i64,
    escalate_after_hours: i64,
    daily_digest: bool,
    digest_hour: i64,
}

/// Stored settings, or the defaults (enabled, admins, 4h, 24h, digest at 09 UTC).
pub async fn load_settings(pool: &SqlitePool, workspace_id: &str) -> sqlx::Result<AlertSettings> {
    let row: Option<SettingsRow> = sqlx::query_as(
        "SELECT enabled, recipients_json, escalate_to_json, unanswered_hours, escalate_after_hours, daily_digest, digest_hour
         FROM alert_settings WHERE workspace_id = ?1",
    )
    .bind(workspace_id)
    .fetch_optional(pool)
    .await?;
    let r = row.unwrap_or(SettingsRow {
        enabled: true,
        recipients_json: None,
        escalate_to_json: "[]".into(),
        unanswered_hours: 4,
        escalate_after_hours: 24,
        daily_digest: true,
        digest_hour: 9,
    });
    let recipients = match r.recipients_json.and_then(|j| serde_json::from_str::<Vec<String>>(&j).ok()) {
        Some(list) => list,
        None => admin_emails(pool, workspace_id).await?,
    };
    Ok(AlertSettings {
        enabled: r.enabled,
        recipients,
        escalate_to: serde_json::from_str(&r.escalate_to_json).unwrap_or_default(),
        unanswered_hours: r.unanswered_hours,
        escalate_after_hours: r.escalate_after_hours,
        daily_digest: r.daily_digest,
        digest_hour: r.digest_hour,
    })
}

pub async fn save_settings(pool: &SqlitePool, workspace_id: &str, s: &AlertSettings) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO alert_settings (workspace_id, enabled, recipients_json, escalate_to_json, unanswered_hours,
                                     escalate_after_hours, daily_digest, digest_hour, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(workspace_id) DO UPDATE SET
           enabled = excluded.enabled, recipients_json = excluded.recipients_json, escalate_to_json = excluded.escalate_to_json,
           unanswered_hours = excluded.unanswered_hours, escalate_after_hours = excluded.escalate_after_hours,
           daily_digest = excluded.daily_digest, digest_hour = excluded.digest_hour, updated_at = excluded.updated_at",
    )
    .bind(workspace_id)
    .bind(s.enabled)
    .bind(json!(s.recipients).to_string())
    .bind(json!(s.escalate_to).to_string())
    .bind(s.unanswered_hours)
    .bind(s.escalate_after_hours)
    .bind(s.daily_digest)
    .bind(s.digest_hour)
    .bind(now_ms())
    .execute(pool)
    .await?;
    Ok(())
}
