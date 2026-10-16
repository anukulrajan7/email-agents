//! Workspace-level team settings: time zone, escalation delays and final contacts (every workspace);
//! the HR mailbox, client domains and portal tolerance (HR & recruiting pack).

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::json;
use sqlx::SqlitePool;

use crate::{alerts::valid_email, platform::db::now_ms};

/// How often a timesheet is expected from each person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cadence {
    Monthly,
    Weekly,
    Daily,
}

impl Cadence {
    pub fn as_str(self) -> &'static str {
        match self {
            Cadence::Monthly => "monthly",
            Cadence::Weekly => "weekly",
            Cadence::Daily => "daily",
        }
    }

    pub fn parse(raw: &str) -> Option<Cadence> {
        match raw {
            "monthly" => Some(Cadence::Monthly),
            "weekly" => Some(Cadence::Weekly),
            "daily" => Some(Cadence::Daily),
            _ => None,
        }
    }
}

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
    pub timesheet_cadence: Cadence,
    /// The most hours one day of a sheet may hold; `None` = no limit.
    pub max_hours_per_day: Option<f64>,
    pub require_signature: bool,
    pub route_alerts_by_owner: bool,
    pub restrict_mail_to_team: bool,
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
            timesheet_cadence: Cadence::Monthly,
            max_hours_per_day: None,
            require_signature: false,
            route_alerts_by_owner: false,
            restrict_mail_to_team: false,
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
    timesheet_cadence: String,
    max_hours_per_day: Option<f64>,
    require_signature: bool,
    route_alerts_by_owner: bool,
    restrict_mail_to_team: bool,
}

pub async fn load(db: &SqlitePool, workspace_id: &str) -> sqlx::Result<TeamSettings> {
    let row: Option<Row> = sqlx::query_as(
        "SELECT tz_offset_minutes, manager_after_hours, final_after_hours, final_emails_json, below_target_hour,
                weekly_check_hour, hr_source_id, client_domains_json, timesheet_grace_days, hours_tolerance,
                timesheet_cadence, max_hours_per_day, require_signature, route_alerts_by_owner, restrict_mail_to_team
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
        timesheet_cadence: Cadence::parse(&r.timesheet_cadence).unwrap_or(Cadence::Monthly),
        max_hours_per_day: r.max_hours_per_day,
        require_signature: r.require_signature,
        route_alerts_by_owner: r.route_alerts_by_owner,
        restrict_mail_to_team: r.restrict_mail_to_team,
    }))
}

/// A missing field is left alone; `null` clears it.
fn nullable<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Some(Option::deserialize(d)?))
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
    pub timesheet_cadence: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub max_hours_per_day: Option<Option<f64>>,
    pub require_signature: Option<bool>,
    pub route_alerts_by_owner: Option<bool>,
    pub restrict_mail_to_team: Option<bool>,
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
            route_alerts_by_owner: self.route_alerts_by_owner.unwrap_or(current.route_alerts_by_owner),
            restrict_mail_to_team: staffing && self.restrict_mail_to_team.unwrap_or(current.restrict_mail_to_team),
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
            if let Some(raw) = &self.timesheet_cadence {
                out.timesheet_cadence = Cadence::parse(raw).ok_or("timesheet_cadence must be monthly, weekly or daily")?;
            }
            if let Some(max) = self.max_hours_per_day {
                if max.is_some_and(|m| !(m > 0.0 && m <= 24.0)) {
                    return Err("max_hours_per_day must be more than 0 and at most 24".into());
                }
                out.max_hours_per_day = max;
            }
            out.require_signature = self.require_signature.unwrap_or(current.require_signature);
        }
        Ok(out)
    }
}

pub async fn save(db: &SqlitePool, workspace_id: &str, s: &TeamSettings) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO team_settings (workspace_id, tz_offset_minutes, manager_after_hours, final_after_hours, final_emails_json,
                below_target_hour, weekly_check_hour, hr_source_id, client_domains_json, timesheet_grace_days, hours_tolerance, route_alerts_by_owner,
                restrict_mail_to_team, timesheet_cadence, max_hours_per_day, require_signature, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
         ON CONFLICT(workspace_id) DO UPDATE SET
           tz_offset_minutes = excluded.tz_offset_minutes, manager_after_hours = excluded.manager_after_hours,
           final_after_hours = excluded.final_after_hours, final_emails_json = excluded.final_emails_json,
           below_target_hour = excluded.below_target_hour, weekly_check_hour = excluded.weekly_check_hour,
           hr_source_id = excluded.hr_source_id, client_domains_json = excluded.client_domains_json,
           timesheet_grace_days = excluded.timesheet_grace_days, hours_tolerance = excluded.hours_tolerance,
           route_alerts_by_owner = excluded.route_alerts_by_owner, restrict_mail_to_team = excluded.restrict_mail_to_team,
           timesheet_cadence = excluded.timesheet_cadence, max_hours_per_day = excluded.max_hours_per_day,
           require_signature = excluded.require_signature, updated_at = excluded.updated_at",
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
    .bind(s.route_alerts_by_owner)
    .bind(s.restrict_mail_to_team)
    .bind(s.timesheet_cadence.as_str())
    .bind(s.max_hours_per_day)
    .bind(s.require_signature)
    .bind(now_ms())
    .execute(db)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> SettingsInput {
        serde_json::from_value(serde_json::json!({"tz_offset_minutes": 0, "below_target_hour": 16, "weekly_check_hour": 12})).unwrap()
    }

    #[test]
    fn the_rule_settings_default_validate_and_clear() {
        let current = TeamSettings::default();
        assert_eq!((current.timesheet_cadence, current.max_hours_per_day, current.require_signature), (Cadence::Monthly, None, false));
        let set: SettingsInput = serde_json::from_value(serde_json::json!({
            "tz_offset_minutes": 0, "below_target_hour": 16, "weekly_check_hour": 12,
            "timesheet_cadence": "weekly", "max_hours_per_day": 10.0, "require_signature": true
        }))
        .unwrap();
        let next = set.validated(&current, true).unwrap();
        assert_eq!((next.timesheet_cadence, next.max_hours_per_day, next.require_signature), (Cadence::Weekly, Some(10.0), true));
        let cleared: SettingsInput = serde_json::from_value(serde_json::json!({"tz_offset_minutes": 0, "below_target_hour": 16, "weekly_check_hour": 12, "max_hours_per_day": null})).unwrap();
        let after = cleared.validated(&next, true).unwrap();
        assert_eq!((after.max_hours_per_day, after.timesheet_cadence, after.require_signature), (None, Cadence::Weekly, true), "null clears the limit, absent fields stay");
        assert_eq!(input().validated(&next, true).unwrap().max_hours_per_day, Some(10.0), "absent leaves the limit");
        assert_eq!(input().validated(&next, false).unwrap().timesheet_cadence, Cadence::Weekly, "other workspaces keep their values");
    }

    #[test]
    fn bad_rule_values_are_refused() {
        let with = |field: &str, value: serde_json::Value| -> SettingsInput {
            let mut body = serde_json::json!({"tz_offset_minutes": 0, "below_target_hour": 16, "weekly_check_hour": 12});
            body[field] = value;
            serde_json::from_value(body).unwrap()
        };
        let current = TeamSettings::default();
        assert!(with("timesheet_cadence", serde_json::json!("hourly")).validated(&current, true).is_err());
        assert!(with("max_hours_per_day", serde_json::json!(0)).validated(&current, true).is_err());
        assert!(with("max_hours_per_day", serde_json::json!(24.5)).validated(&current, true).is_err());
        assert!(with("max_hours_per_day", serde_json::json!(24)).validated(&current, true).is_ok());
    }
}
