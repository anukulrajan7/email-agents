//! Reads for the scorecard. Every query is scoped by workspace.

use std::collections::HashMap;

use serde::Serialize;
use sqlx::SqlitePool;

use super::{Counts, DAY_MS, Totals, day_label, day_of, edit_distance, ratio, round4};
use crate::db::now_ms;

pub const MAX_DAYS: i64 = 180;

pub fn clamp_days(days: Option<i64>) -> i64 {
    days.unwrap_or(30).clamp(1, MAX_DAYS)
}

/// First day (inclusive) of a window of `days` days ending today.
pub fn first_day(days: i64) -> i64 {
    day_of(now_ms()) - days + 1
}

/// One measurement tied to an agent.
#[derive(Debug, Clone)]
pub struct Sample {
    pub workspace_id: String,
    pub agent_id: String,
    pub agent_version: i64,
    pub at: i64,
    pub value: f64,
}

/// Edit distance of every decided draft an agent made, by the time it was decided.
pub async fn edit_samples(db: &SqlitePool, ws: Option<&str>, from_ms: i64, until_ms: i64) -> sqlx::Result<Vec<Sample>> {
    let rows: Vec<(String, String, i64, i64, String, String)> = sqlx::query_as(
        "SELECT d.workspace_id, d.agent_id, COALESCE(r.agent_version, ag.active_version, 0), d.updated_at, d.original_body_text, d.body_text
         FROM drafts d
         LEFT JOIN message_routes r ON r.message_id = d.message_id
         LEFT JOIN agents ag ON ag.id = d.agent_id
         WHERE d.agent_id IS NOT NULL AND d.original_body_text IS NOT NULL AND d.status IN ('edited','approved','sent')
           AND d.updated_at >= ?2 AND d.updated_at < ?3 AND (?1 IS NULL OR d.workspace_id = ?1)",
    )
    .bind(ws)
    .bind(from_ms)
    .bind(until_ms)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(workspace_id, agent_id, agent_version, at, before, after)| Sample {
            workspace_id,
            agent_id,
            agent_version,
            at,
            value: edit_distance(&before, &after),
        })
        .collect())
}

/// Milliseconds from an email arriving to its case finishing, for finished cases.
pub async fn latency_samples(db: &SqlitePool, ws: Option<&str>, from_ms: i64, until_ms: i64) -> sqlx::Result<Vec<Sample>> {
    let rows: Vec<(String, String, i64, i64, i64)> = sqlx::query_as(
        "SELECT c.workspace_id, c.agent_id, COALESCE(c.agent_version, 0), c.updated_at, c.updated_at - m.received_at
         FROM cases c JOIN email_messages m ON m.id = c.message_id
         WHERE c.agent_id IS NOT NULL AND c.status = 'done' AND c.shadow = 0
           AND c.updated_at >= ?2 AND c.updated_at < ?3 AND (?1 IS NULL OR c.workspace_id = ?1)",
    )
    .bind(ws)
    .bind(from_ms)
    .bind(until_ms)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(workspace_id, agent_id, agent_version, at, ms)| Sample { workspace_id, agent_id, agent_version, at, value: ms.max(0) as f64 })
        .collect())
}

#[derive(sqlx::FromRow)]
struct DailyRow {
    agent_id: String,
    day: i64,
    emails_routed: i64,
    cases_started: i64,
    cases_done: i64,
    cases_failed: i64,
    cases_cancelled: i64,
    approvals_requested: i64,
    approvals_approved: i64,
    approvals_edited: i64,
    approvals_declined: i64,
    drafts_created: i64,
    drafts_sent: i64,
    edit_distance_sum: f64,
    edit_distance_n: i64,
    auto_sends: i64,
    auto_sends_stopped: i64,
    incidents: i64,
    label_corrections: i64,
    labels_total: i64,
    shadow_agreement_sum: f64,
    shadow_agreement_n: i64,
    cost_usd: f64,
}

async fn daily_rows(db: &SqlitePool, ws: &str, agent_id: Option<&str>, from_day: i64) -> sqlx::Result<Vec<DailyRow>> {
    sqlx::query_as(
        "SELECT agent_id, day, SUM(emails_routed) AS emails_routed, SUM(cases_started) AS cases_started, SUM(cases_done) AS cases_done,
                SUM(cases_failed) AS cases_failed, SUM(cases_cancelled) AS cases_cancelled,
                SUM(approvals_requested) AS approvals_requested, SUM(approvals_approved) AS approvals_approved,
                SUM(approvals_edited) AS approvals_edited, SUM(approvals_declined) AS approvals_declined,
                SUM(drafts_created) AS drafts_created, SUM(drafts_sent) AS drafts_sent,
                SUM(edit_distance_sum) AS edit_distance_sum, SUM(edit_distance_n) AS edit_distance_n,
                SUM(auto_sends) AS auto_sends, SUM(auto_sends_stopped) AS auto_sends_stopped, SUM(incidents) AS incidents,
                SUM(label_corrections) AS label_corrections, SUM(labels_total) AS labels_total,
                SUM(shadow_agreement_sum) AS shadow_agreement_sum, SUM(shadow_agreement_n) AS shadow_agreement_n,
                SUM(cost_usd) AS cost_usd
         FROM agent_metrics_daily WHERE workspace_id = ?1 AND day >= ?2 AND (?3 IS NULL OR agent_id = ?3)
         GROUP BY agent_id, day",
    )
    .bind(ws)
    .bind(from_day)
    .bind(agent_id)
    .fetch_all(db)
    .await
}

impl DailyRow {
    fn add_to(&self, c: &mut Counts) {
        c.routed += self.emails_routed;
        c.cases += self.cases_started;
        c.done += self.cases_done;
        c.failed += self.cases_failed;
        c.cancelled += self.cases_cancelled;
        c.approvals_requested += self.approvals_requested;
        c.approved += self.approvals_approved;
        c.edited += self.approvals_edited;
        c.declined += self.approvals_declined;
        c.drafts_created += self.drafts_created;
        c.drafts_sent += self.drafts_sent;
        c.edit_sum += self.edit_distance_sum;
        c.edit_n += self.edit_distance_n;
        c.auto_sends += self.auto_sends;
        c.stopped += self.auto_sends_stopped;
        c.incidents += self.incidents;
        c.label_corrections += self.label_corrections;
        c.labels_total += self.labels_total;
        c.shadow_sum += self.shadow_agreement_sum;
        c.shadow_n += self.shadow_agreement_n;
        c.cost_usd += self.cost_usd;
    }
}

/// Summed counters per agent over the last `days` days (today included).
pub async fn counts(db: &SqlitePool, ws: &str, agent_id: Option<&str>, days: i64) -> sqlx::Result<HashMap<String, Counts>> {
    let mut out: HashMap<String, Counts> = HashMap::new();
    for row in daily_rows(db, ws, agent_id, first_day(days)).await? {
        row.add_to(out.entry(row.agent_id.clone()).or_default());
    }
    Ok(out)
}

fn group(samples: Vec<Sample>) -> HashMap<String, Vec<f64>> {
    let mut out: HashMap<String, Vec<f64>> = HashMap::new();
    for s in samples {
        out.entry(s.agent_id).or_default().push(s.value);
    }
    out
}

/// Scorecard numbers per agent over the last `days` days. Medians and percentiles come from the
/// raw samples, everything else from the daily rollup.
pub async fn totals(db: &SqlitePool, ws: &str, agent_id: Option<&str>, days: i64) -> sqlx::Result<HashMap<String, Totals>> {
    let counts = counts(db, ws, agent_id, days).await?;
    let from = first_day(days) * DAY_MS;
    let until = now_ms() + DAY_MS;
    let mut edits = group(edit_samples(db, Some(ws), from, until).await?);
    let mut lats = group(latency_samples(db, Some(ws), from, until).await?);
    Ok(counts
        .into_iter()
        .map(|(agent, c)| {
            let median = super::percentile(&edits.remove(&agent).unwrap_or_default(), 50.0);
            let lat = lats.remove(&agent).unwrap_or_default();
            let t = Totals::from_counts(&c, median, &lat);
            (agent, t)
        })
        .collect())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SeriesPoint {
    pub date: String,
    pub approval_rate: Option<f64>,
    pub edit_distance: Option<f64>,
    pub cost_usd: f64,
    pub cases: i64,
}

/// One point per day of the window, oldest first, zero days included.
pub async fn series(db: &SqlitePool, ws: &str, agent_id: &str, days: i64) -> sqlx::Result<Vec<SeriesPoint>> {
    let first = first_day(days);
    let by_day: HashMap<i64, DailyRow> = daily_rows(db, ws, Some(agent_id), first).await?.into_iter().map(|r| (r.day, r)).collect();
    Ok((first..first + days)
        .map(|day| match by_day.get(&day) {
            Some(r) => SeriesPoint {
                date: day_label(day),
                approval_rate: ratio(r.approvals_approved, r.approvals_approved + r.approvals_declined).map(round4),
                edit_distance: (r.edit_distance_n > 0).then(|| round4(r.edit_distance_sum / r.edit_distance_n as f64)),
                cost_usd: (r.cost_usd * 1e6).round() / 1e6,
                cases: r.cases_started,
            },
            None => SeriesPoint { date: day_label(day), approval_rate: None, edit_distance: None, cost_usd: 0.0, cases: 0 },
        })
        .collect())
}
