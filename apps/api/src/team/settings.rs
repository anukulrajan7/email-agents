//! Workspace-level team settings: time zone, escalation delays and final contacts (every workspace);
//! the HR mailbox, client domains and portal tolerance (HR & recruiting pack).

use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::SqlitePool;

use crate::{alerts::valid_email, db::now_ms};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TeamSettings {
    pub tz_offset_minutes: i64,
    pub manager_after_hours: Option<i64>,
    pub final_after_hours: Option<i64>,
    pub final_emails: Vec<String>,
    pub below_target_hour: i64,
    pub weekly_check_hour: i64,
    pub hr_source_id: Option<String>,
    pub client_domains: Vec<String>,
    pub timesheet_grace_days: i64,
    pub hours_tolerance: f64,
}

impl Default for TeamSettings {
    fn default() -> Self {
        Self {
            tz_offset_minutes: 0,
            manager_after_hours: None,
            final_after_hours: None,
            final_emails: vec![],
            below_target_hour: 16,
            weekly_check_hour: 12,
            hr_source_id: None,
            client_domains: vec![],
            timesheet_grace_days: 3,
            hours_tolerance: 0.5,
        }
    }
}

#[derive(sqlx::FromRow)]
struct Row {
    tz_offset_minutes: i64,
    manager_after_hours: Option<i64>,
    final_after_hours: Option<i64>,
    final_emails_json: String,
    below_target_hour: i64,
    weekly_check_hour: i64,
    hr_source_id: Option<String>,
    client_domains_json: String,
    timesheet_grace_days: i64,
    hours_tolerance: f64,
}

pub async fn load(db: &SqlitePool, workspace_id: &str) -> sqlx::Result<TeamSettings> {
    let row: Option<Row> = sqlx::query_as(
        "SELECT tz_offset_minutes, manager_after_hours, final_after_hours, final_emails_json, below_target_hour,
                weekly_check_hour, hr_source_id, client_domains_json, timesheet_grace_days, hours_tolerance
         FROM team_settings WHERE workspace_id = ?1",
    )
    .bind(workspace_id)
    .fetch_optional(db)
    .await?;
    Ok(row.map_or_else(TeamSettings::default, |r| TeamSettings {
        tz_offset_minutes: r.tz_offset_minutes,
        manager_after_hours: r.manager_after_hours,
        final_after_hours: r.final_after_hours,
        final_emails: serde_json::from_str(&r.final_emails_json).unwrap_or_default(),
        below_target_hour: r.below_target_hour,
        weekly_check_hour: r.weekly_check_hour,
        hr_source_id: r.hr_source_id,
        client_domains: serde_json::from_str(&r.client_domains_json).unwrap_or_default(),
        timesheet_grace_days: r.timesheet_grace_days,
        hours_tolerance: r.hours_tolerance,
    }))
}

/// Request body. Staffing-only fields are ignored in other workspaces.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsInput {
    pub tz_offset_minutes: i64,
    pub manager_after_hours: Option<i64>,
    pub final_after_hours: Option<i64>,
    #[serde(default)]
    pub final_emails: Vec<String>,
    pub below_target_hour: i64,
    pub weekly_check_hour: i64,
    pub hr_source_id: Option<String>,
    #[serde(default)]
    pub client_domains: Vec<String>,
    pub timesheet_grace_days: Option<i64>,
    pub hours_tolerance: Option<f64>,
}

impl SettingsInput {
    /// Normalises and range-checks; `staffing` = the HR & recruiting pack is on.
    pub fn validated(self, current: &TeamSettings, staffing: bool) -> Result<TeamSettings, String> {
        let in_range = |v: i64, lo: i64, hi: i64, f: &str| (lo..=hi).contains(&v).then_some(v).ok_or(format!("{f} must be {lo}-{hi}"));
        let opt = |v: Option<i64>, f: &str| v.map(|h| in_range(h, 1, 720, f)).transpose();
        let mut finals = vec![];
        for e in &self.final_emails {
            let e = e.trim().to_lowercase();
            if !valid_email(&e) {
                return Err(format!("final_emails: '{e}' is not a valid email address"));
            }
            if !finals.contains(&e) {
                finals.push(e);
            }
        }
        if finals.len() > 20 {
            return Err("final_emails: at most 20 addresses".into());
        }
        let mut out = TeamSettings {
            tz_offset_minutes: in_range(self.tz_offset_minutes, -840, 840, "tz_offset_minutes")?,
            manager_after_hours: opt(self.manager_after_hours, "manager_after_hours")?,
            final_after_hours: opt(self.final_after_hours, "final_after_hours")?,
            final_emails: finals,
            below_target_hour: in_range(self.below_target_hour, 0, 23, "below_target_hour")?,
            weekly_check_hour: in_range(self.weekly_check_hour, 0, 23, "weekly_check_hour")?,
            ..current.clone()
        };
        if staffing {
            let mut domains = vec![];
            for d in &self.client_domains {
                let d = d.trim().trim_start_matches('@').to_lowercase();
                if !d.contains('.') || d.contains(char::is_whitespace) || d.contains('@') {
                    return Err(format!("client_domains: '{d}' is not a domain like acme.com"));
                }
                if !domains.contains(&d) {
                    domains.push(d);
                }
            }
            out.client_domains = domains;
            out.hr_source_id = self.hr_source_id.filter(|s| !s.is_empty());
            out.timesheet_grace_days = in_range(self.timesheet_grace_days.unwrap_or(current.timesheet_grace_days), 0, 28, "timesheet_grace_days")?;
            let tol = self.hours_tolerance.unwrap_or(current.hours_tolerance);
            if !(0.0..=100.0).contains(&tol) {
                return Err("hours_tolerance must be 0-100".into());
            }
            out.hours_tolerance = tol;
        }
        Ok(out)
    }
}

pub async fn save(db: &SqlitePool, workspace_id: &str, s: &TeamSettings) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO team_settings (workspace_id, tz_offset_minutes, manager_after_hours, final_after_hours, final_emails_json,
                below_target_hour, weekly_check_hour, hr_source_id, client_domains_json, timesheet_grace_days, hours_tolerance, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(workspace_id) DO UPDATE SET
           tz_offset_minutes = excluded.tz_offset_minutes, manager_after_hours = excluded.manager_after_hours,
           final_after_hours = excluded.final_after_hours, final_emails_json = excluded.final_emails_json,
           below_target_hour = excluded.below_target_hour, weekly_check_hour = excluded.weekly_check_hour,
           hr_source_id = excluded.hr_source_id, client_domains_json = excluded.client_domains_json,
           timesheet_grace_days = excluded.timesheet_grace_days, hours_tolerance = excluded.hours_tolerance,
           updated_at = excluded.updated_at",
    )
    .bind(workspace_id)
    .bind(s.tz_offset_minutes)
    .bind(s.manager_after_hours)
    .bind(s.final_after_hours)
    .bind(json!(s.final_emails).to_string())
    .bind(s.below_target_hour)
    .bind(s.weekly_check_hour)
    .bind(&s.hr_source_id)
    .bind(json!(s.client_domains).to_string())
    .bind(s.timesheet_grace_days)
    .bind(s.hours_tolerance)
    .bind(now_ms())
    .execute(db)
    .await?;
    Ok(())
}
