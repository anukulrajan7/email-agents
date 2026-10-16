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
    /// IANA name of the workspace's zone; `None` = not chosen (the fixed offset, else Asia/Kolkata, applies).
    pub timezone: Option<String>,
    /// Working hours to answer a mail in ("never silent").
    pub reply_sla_hours: i64,
    pub work_start_hour: i64,
    pub work_end_hour: i64,
    /// Monday = 0.
    pub work_days: Vec<i64>,
    /// `auto`: a holding note goes out by itself when nothing was composed in time; `off`: only people are told.
    pub holding_mode: String,
    /// Addresses and `@domains` the owner never wants an automatic reply to.
    pub no_reply_senders: Vec<String>,
    /// What a sheet must state, or the sender is asked: `identity`, `name`, `employee_code`, `email`, `period`.
    pub timesheet_required: Vec<String>,
    /// Hours before the one reminder, and (after it) before a person is told. Calendar hours.
    pub missing_reminder_hours: i64,
    pub missing_escalate_hours: i64,
    pub missing_max_reminders: i64,
}

/// What a sheet may be required to state.
pub const REQUIRED_KEYS: [&str; 5] = ["identity", "name", "employee_code", "email", "period"];

impl TeamSettings {
    /// The zone this workspace counts working time in.
    pub fn zone(&self) -> crate::platform::time::Zone {
        crate::platform::time::resolve(None, self.timezone.as_deref(), self.tz_offset_minutes)
    }

    pub fn work_week(&self) -> crate::platform::time::WorkWeek {
        crate::platform::time::WorkWeek::from_days(self.work_start_hour, self.work_end_hour, &self.work_days)
    }
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
            timezone: None,
            reply_sla_hours: 4,
            work_start_hour: 9,
            work_end_hour: 19,
            work_days: vec![0, 1, 2, 3, 4, 5],
            holding_mode: "auto".into(),
            no_reply_senders: vec![],
            timesheet_required: vec!["identity".into(), "period".into()],
            missing_reminder_hours: 24,
            missing_escalate_hours: 48,
            missing_max_reminders: 1,
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
    timezone: Option<String>,
    reply_sla_hours: i64,
    work_start_hour: i64,
    work_end_hour: i64,
    work_days_json: String,
    holding_mode: String,
    no_reply_json: String,
    timesheet_required_json: String,
    missing_reminder_hours: i64,
    missing_escalate_hours: i64,
    missing_max_reminders: i64,
}

pub async fn load(db: &SqlitePool, workspace_id: &str) -> sqlx::Result<TeamSettings> {
    let row: Option<Row> = sqlx::query_as(
        "SELECT tz_offset_minutes, manager_after_hours, final_after_hours, final_emails_json, below_target_hour,
                weekly_check_hour, hr_source_id, client_domains_json, timesheet_grace_days, hours_tolerance,
                timesheet_cadence, max_hours_per_day, require_signature, route_alerts_by_owner, restrict_mail_to_team,
                timezone, reply_sla_hours, work_start_hour, work_end_hour, work_days_json, holding_mode, no_reply_json,
                timesheet_required_json, missing_reminder_hours, missing_escalate_hours, missing_max_reminders
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
        timezone: r.timezone,
        reply_sla_hours: r.reply_sla_hours,
        work_start_hour: r.work_start_hour,
        work_end_hour: r.work_end_hour,
        work_days: serde_json::from_str(&r.work_days_json).unwrap_or_else(|_| TeamSettings::default().work_days),
        holding_mode: r.holding_mode,
        no_reply_senders: serde_json::from_str(&r.no_reply_json).unwrap_or_default(),
        timesheet_required: serde_json::from_str(&r.timesheet_required_json).unwrap_or_else(|_| TeamSettings::default().timesheet_required),
        missing_reminder_hours: r.missing_reminder_hours,
        missing_escalate_hours: r.missing_escalate_hours,
        missing_max_reminders: r.missing_max_reminders,
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
    #[serde(default, deserialize_with = "nullable")]
    pub timezone: Option<Option<String>>,
    pub reply_sla_hours: Option<i64>,
    pub work_start_hour: Option<i64>,
    pub work_end_hour: Option<i64>,
    pub work_days: Option<Vec<i64>>,
    pub holding_mode: Option<String>,
    pub no_reply_senders: Option<Vec<String>>,
    pub timesheet_required: Option<Vec<String>>,
    pub missing_reminder_hours: Option<i64>,
    pub missing_escalate_hours: Option<i64>,
    pub missing_max_reminders: Option<i64>,
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
        if let Some(tz) = &self.timezone {
            out.timezone = match tz.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
                None => None,
                Some(name) if crate::platform::time::zone::is_known(name) => Some(name.to_string()),
                Some(_) => return Err("timezone must be a name like Asia/Kolkata".into()),
            };
        }
        out.reply_sla_hours = in_range(self.reply_sla_hours.unwrap_or(current.reply_sla_hours), 1, 72, "reply_sla_hours")?;
        out.work_start_hour = in_range(self.work_start_hour.unwrap_or(current.work_start_hour), 0, 23, "work_start_hour")?;
        out.work_end_hour = in_range(self.work_end_hour.unwrap_or(current.work_end_hour), 1, 24, "work_end_hour")?;
        if out.work_start_hour >= out.work_end_hour {
            return Err("work_end_hour must be after work_start_hour".into());
        }
        if let Some(days) = &self.work_days {
            let mut days: Vec<i64> = days.iter().copied().filter(|d| (0..7).contains(d)).collect();
            days.sort_unstable();
            days.dedup();
            if days.is_empty() {
                return Err("work_days needs at least one day (Monday is 0)".into());
            }
            out.work_days = days;
        }
        if let Some(mode) = &self.holding_mode {
            if !matches!(mode.as_str(), "auto" | "off") {
                return Err("holding_mode must be auto or off".into());
            }
            out.holding_mode = mode.clone();
        }
        if let Some(list) = &self.no_reply_senders {
            let mut clean: Vec<String> = vec![];
            for raw in list {
                let v = raw.trim().to_lowercase();
                if v.is_empty() || v.contains(char::is_whitespace) || !(v.starts_with('@') && v.contains('.') || v.contains('@') && !v.starts_with('@')) {
                    return Err(format!("no_reply_senders: '{v}' is not an address like ravi@x.com or a domain like @x.com"));
                }
                if !clean.contains(&v) {
                    clean.push(v);
                }
            }
            if clean.len() > 200 {
                return Err("no_reply_senders: at most 200 entries".into());
            }
            out.no_reply_senders = clean;
        }
        if staffing {
            if let Some(keys) = &self.timesheet_required {
                let mut clean: Vec<String> = vec![];
                for k in keys {
                    let k = k.trim().to_lowercase();
                    if !REQUIRED_KEYS.contains(&k.as_str()) {
                        return Err(format!("timesheet_required: '{k}' is not one of {}", REQUIRED_KEYS.join(", ")));
                    }
                    if !clean.contains(&k) {
                        clean.push(k);
                    }
                }
                out.timesheet_required = clean;
            }
            out.missing_reminder_hours = in_range(self.missing_reminder_hours.unwrap_or(current.missing_reminder_hours), 1, 720, "missing_reminder_hours")?;
            out.missing_escalate_hours = in_range(self.missing_escalate_hours.unwrap_or(current.missing_escalate_hours), 1, 720, "missing_escalate_hours")?;
            out.missing_max_reminders = in_range(self.missing_max_reminders.unwrap_or(current.missing_max_reminders), 0, 2, "missing_max_reminders")?;
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
                restrict_mail_to_team, timesheet_cadence, max_hours_per_day, require_signature, updated_at,
                timezone, reply_sla_hours, work_start_hour, work_end_hour, work_days_json, holding_mode, no_reply_json,
                timesheet_required_json, missing_reminder_hours, missing_escalate_hours, missing_max_reminders)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28)
         ON CONFLICT(workspace_id) DO UPDATE SET
           tz_offset_minutes = excluded.tz_offset_minutes, manager_after_hours = excluded.manager_after_hours,
           final_after_hours = excluded.final_after_hours, final_emails_json = excluded.final_emails_json,
           below_target_hour = excluded.below_target_hour, weekly_check_hour = excluded.weekly_check_hour,
           hr_source_id = excluded.hr_source_id, client_domains_json = excluded.client_domains_json,
           timesheet_grace_days = excluded.timesheet_grace_days, hours_tolerance = excluded.hours_tolerance,
           route_alerts_by_owner = excluded.route_alerts_by_owner, restrict_mail_to_team = excluded.restrict_mail_to_team,
           timesheet_cadence = excluded.timesheet_cadence, max_hours_per_day = excluded.max_hours_per_day,
           require_signature = excluded.require_signature, updated_at = excluded.updated_at,
           timezone = excluded.timezone, reply_sla_hours = excluded.reply_sla_hours, work_start_hour = excluded.work_start_hour,
           work_end_hour = excluded.work_end_hour, work_days_json = excluded.work_days_json, holding_mode = excluded.holding_mode,
           no_reply_json = excluded.no_reply_json, timesheet_required_json = excluded.timesheet_required_json,
           missing_reminder_hours = excluded.missing_reminder_hours, missing_escalate_hours = excluded.missing_escalate_hours,
           missing_max_reminders = excluded.missing_max_reminders",
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
    .bind(&s.timezone)
    .bind(s.reply_sla_hours)
    .bind(s.work_start_hour)
    .bind(s.work_end_hour)
    .bind(json!(s.work_days).to_string())
    .bind(&s.holding_mode)
    .bind(json!(s.no_reply_senders).to_string())
    .bind(json!(s.timesheet_required).to_string())
    .bind(s.missing_reminder_hours)
    .bind(s.missing_escalate_hours)
    .bind(s.missing_max_reminders)
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
