//! The metrics framework: targets vs achieved per person, from exact SQL. Nothing here uses a model.
//!
//! A metric is a `MetricDef`: a key, a label, a unit and one SQL query over `(workspace, person, from, to)`.
//! Every workspace gets the generic definitions (replies sent, average response time); a pack adds its own
//! (`crate::hr::staffing::metrics::DEFS`). The primary metric is the one daily and weekly targets count.

use serde::Serialize;
use sqlx::SqlitePool;

use super::{
    clock::{self, DAY_MS, Windows},
    people::Person,
    settings,
};
use crate::platform::packs;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    Count,
    /// 0..1
    Rate,
    Minutes,
}

/// Window values are passed as `?3` (from, inclusive) and `?4` (to, exclusive); `?1` workspace, `?2` person.
#[derive(Debug, Clone, Copy)]
pub struct MetricDef {
    pub key: &'static str,
    pub label: &'static str,
    pub unit: Unit,
    /// The metric daily and weekly targets count. The last enabled primary wins.
    pub primary: bool,
    /// A rate of two other metrics: `(numerator key, denominator key)`. Teams recompute it from sums.
    pub ratio: Option<(&'static str, &'static str)>,
    /// One row, one column, a REAL or NULL.
    pub sql: &'static str,
}

/// The person's sent mail: from any of their addresses, or from the mailbox linked to them.
macro_rules! sent_by_person {
    () => {
        "m.workspace_id = ?1 AND m.direction = 'outbound' AND m.received_at >= ?3 AND m.received_at < ?4
         AND (lower(m.sender_address) IN (SELECT email FROM person_emails WHERE person_id = ?2)
              OR m.source_id IN (SELECT source_id FROM people WHERE id = ?2 AND source_id IS NOT NULL))
         AND EXISTS (SELECT 1 FROM email_messages i WHERE i.thread_id = m.thread_id AND i.direction = 'inbound'
                     AND i.received_at < m.received_at)"
    };
}

pub const GENERIC_DEFS: &[MetricDef] = &[
    MetricDef {
        key: "replies_sent",
        label: "Replies sent",
        unit: Unit::Count,
        primary: true,
        ratio: None,
        sql: concat!("SELECT CAST(COUNT(*) AS REAL) FROM email_messages m WHERE ", sent_by_person!()),
    },
    MetricDef {
        key: "avg_response_minutes",
        label: "Average response time",
        unit: Unit::Minutes,
        primary: false,
        ratio: None,
        sql: concat!(
            "SELECT AVG((m.received_at - (SELECT MAX(i.received_at) FROM email_messages i WHERE i.thread_id = m.thread_id
                                           AND i.direction = 'inbound' AND i.received_at < m.received_at)) / 60000.0)
             FROM email_messages m WHERE ",
            sent_by_person!()
        ),
    },
];

/// Generic definitions plus the ones from every enabled pack.
pub fn defs_for(pack_keys: &[&str]) -> Vec<MetricDef> {
    let mut defs = GENERIC_DEFS.to_vec();
    if pack_keys.contains(&packs::STAFFING) {
        let staffing = crate::hr::staffing::metrics::DEFS;
        if staffing.iter().any(|d| d.primary) {
            defs.iter_mut().for_each(|d| d.primary = false);
        }
        defs.extend_from_slice(staffing);
    }
    defs
}

pub fn primary_key(defs: &[MetricDef]) -> &'static str {
    defs.iter().rev().find(|d| d.primary).map_or("replies_sent", |d| d.key)
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MetricValue {
    pub key: &'static str,
    pub label: &'static str,
    pub unit: Unit,
    pub primary: bool,
    pub today: Option<f64>,
    pub week: Option<f64>,
    pub since_joining: Option<f64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PersonMetrics {
    pub person_id: String,
    pub name: String,
    pub role: Option<String>,
    pub joining_date: Option<String>,
    pub primary_metric: &'static str,
    pub daily_target: Option<i64>,
    pub weekly_target: Option<i64>,
    /// The primary metric today and this week (0 when there is nothing yet).
    pub today: f64,
    pub week: f64,
    pub metrics: Vec<MetricValue>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TeamMetrics {
    pub manager_id: Option<String>,
    pub primary_metric: &'static str,
    pub today: f64,
    pub week: f64,
    pub daily_target: i64,
    pub weekly_target: i64,
    pub metrics: Vec<MetricValue>,
    pub members: Vec<PersonMetrics>,
}

async fn one(db: &SqlitePool, sql: &str, workspace_id: &str, person_id: &str, from: i64, to: i64) -> sqlx::Result<Option<f64>> {
    let v: Option<Option<f64>> = sqlx::query_scalar(sql).bind(workspace_id).bind(person_id).bind(from).bind(to).fetch_optional(db).await?;
    Ok(v.flatten())
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Everything for one person at `now`, in the workspace's time zone.
pub async fn compute_person(db: &SqlitePool, workspace_id: &str, person: &Person, pack_keys: &[&str], now: i64, tz_min: i64) -> sqlx::Result<PersonMetrics> {
    let defs = defs_for(pack_keys);
    let w: Windows = clock::windows(now, tz_min);
    let joined = person.joining_date.as_deref().and_then(|d| clock::date_start_ms(d, tz_min)).unwrap_or(0);
    let primary = primary_key(&defs);
    let mut metrics = Vec::with_capacity(defs.len());
    for d in &defs {
        let today = one(db, d.sql, workspace_id, &person.id, w.day_start, w.day_end).await?;
        let week = one(db, d.sql, workspace_id, &person.id, w.week_start, w.week_end).await?;
        let since = one(db, d.sql, workspace_id, &person.id, joined, now.max(joined) + DAY_MS).await?;
        metrics.push(MetricValue {
            key: d.key,
            label: d.label,
            unit: d.unit,
            primary: d.key == primary,
            today: today.map(round2),
            week: week.map(round2),
            since_joining: since.map(round2),
        });
    }
    let p = metrics.iter().find(|m| m.primary);
    Ok(PersonMetrics {
        person_id: person.id.clone(),
        name: person.name.clone(),
        role: person.role.clone(),
        joining_date: person.joining_date.clone(),
        primary_metric: primary,
        daily_target: person.daily_target,
        weekly_target: person.weekly_target,
        today: p.and_then(|m| m.today).unwrap_or(0.0),
        week: p.and_then(|m| m.week).unwrap_or(0.0),
        metrics,
    })
}

/// Sums count metrics across members, recomputes ratio metrics from those sums and averages the rest.
pub fn combine(members: &[PersonMetrics], manager_id: Option<String>, pack_keys: &[&str]) -> TeamMetrics {
    let defs = defs_for(pack_keys);
    let primary = primary_key(&defs);
    let sum = |key: &str, f: fn(&MetricValue) -> Option<f64>| -> Option<f64> {
        let vals: Vec<f64> = members.iter().filter_map(|m| m.metrics.iter().find(|v| v.key == key).and_then(f)).collect();
        (!vals.is_empty()).then(|| vals.iter().sum())
    };
    let mean = |key: &str, f: fn(&MetricValue) -> Option<f64>| -> Option<f64> {
        let vals: Vec<f64> = members.iter().filter_map(|m| m.metrics.iter().find(|v| v.key == key).and_then(f)).collect();
        (!vals.is_empty()).then(|| vals.iter().sum::<f64>() / vals.len() as f64)
    };
    let ratio = |num: Option<f64>, den: Option<f64>| match (num, den) {
        (Some(n), Some(d)) if d > 0.0 => Some(n / d),
        _ => None,
    };
    let pick: [fn(&MetricValue) -> Option<f64>; 3] = [|v| v.today, |v| v.week, |v| v.since_joining];
    let metrics: Vec<MetricValue> = defs
        .iter()
        .map(|d| {
            let col = |i: usize| match (d.unit, d.ratio) {
                (_, Some((n, de))) => ratio(sum(n, pick[i]), sum(de, pick[i])),
                (Unit::Count, None) => sum(d.key, pick[i]),
                _ => mean(d.key, pick[i]),
            };
            MetricValue {
                key: d.key,
                label: d.label,
                unit: d.unit,
                primary: d.key == primary,
                today: col(0).map(round2),
                week: col(1).map(round2),
                since_joining: col(2).map(round2),
            }
        })
        .collect();
    let p = metrics.iter().find(|m| m.primary);
    TeamMetrics {
        manager_id,
        primary_metric: primary,
        today: p.and_then(|m| m.today).unwrap_or(0.0),
        week: p.and_then(|m| m.week).unwrap_or(0.0),
        daily_target: members.iter().filter_map(|m| m.daily_target).sum(),
        weekly_target: members.iter().filter_map(|m| m.weekly_target).sum(),
        metrics,
        members: members.to_vec(),
    }
}

/// Enabled pack keys and the workspace's UTC offset: what every metric call needs.
pub async fn context(db: &SqlitePool, workspace_id: &str) -> sqlx::Result<(Vec<&'static str>, i64)> {
    Ok((packs::enabled_keys(db, workspace_id).await?, settings::load(db, workspace_id).await?.tz_offset_minutes))
}

/// The primary metric for one window: what targets are measured against.
pub async fn primary_count(db: &SqlitePool, workspace_id: &str, person_id: &str, pack_keys: &[&str], from: i64, to: i64) -> sqlx::Result<(f64, &'static str)> {
    let defs = defs_for(pack_keys);
    let key = primary_key(&defs);
    let sql = defs.iter().find(|d| d.key == key).map_or(GENERIC_DEFS[0].sql, |d| d.sql);
    Ok((one(db, sql, workspace_id, person_id, from, to).await?.unwrap_or(0.0), key))
}

pub fn label_of(key: &str, pack_keys: &[&str]) -> &'static str {
    defs_for(pack_keys).iter().find(|d| d.key == key).map_or("Activity", |d| d.label)
}
