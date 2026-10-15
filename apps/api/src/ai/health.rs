//! The AI health report behind the admin page: incidents over time, cost per email, latency and error
//! rates per task, the understanding-v2 shadow result and today's budget. Also the spike checks that
//! raise an alert when spend or latency jumps. Labels are plain task names, never model names.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::{
    ai::understand::{self, ShadowReport},
    ai_ledger::{AUTOMATIC_TASKS, task_label},
    alerts::{self, NewAlert},
    incidents::{self, ALL_KINDS, IncidentRow, Kind, NewIncident},
    metrics::{DAY_MS, percentile, ratio},
};

/// Spend today must pass this many times the 7-day daily average, and the floor, to count as a spike.
pub const COST_SPIKE_FACTOR: f64 = 2.0;
pub const COST_SPIKE_FLOOR_USD: f64 = 0.50;
/// Recent p95 must pass this many times the baseline p95, and the floor, to count as slow.
pub const LATENCY_SPIKE_FACTOR: f64 = 2.0;
pub const LATENCY_SPIKE_FLOOR_MS: f64 = 2_000.0;
const LATENCY_MIN_RECENT: usize = 20;
const LATENCY_MIN_BASELINE: usize = 50;
const HOUR_MS: i64 = 3_600_000;
const MAX_OPEN_SHOWN: i64 = 100;

/// Spend today against the previous seven days' total. Pure.
const COST_MIN_DAYS: i64 = 3;

pub fn cost_spike(today_usd: f64, prior_7d_usd: f64, days_with_data: i64) -> bool {
    prior_7d_usd > 0.0 && days_with_data >= COST_MIN_DAYS && today_usd > COST_SPIKE_FLOOR_USD && today_usd > COST_SPIKE_FACTOR * (prior_7d_usd / 7.0)
}

/// The last hour's p95 against the baseline p95 for one task. Pure.
pub fn latency_spike(recent: &[f64], baseline: &[f64]) -> Option<(f64, f64)> {
    if recent.len() < LATENCY_MIN_RECENT || baseline.len() < LATENCY_MIN_BASELINE {
        return None;
    }
    let (r, b) = (percentile(recent, 95.0)?, percentile(baseline, 95.0)?);
    (r > LATENCY_SPIKE_FLOOR_MS && r > LATENCY_SPIKE_FACTOR * b).then_some((r, b))
}

fn round4(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0
}

fn round6(v: f64) -> f64 {
    (v * 1_000_000.0).round() / 1_000_000.0
}

fn day_start(ms: i64) -> i64 {
    ms - ms.rem_euclid(DAY_MS)
}

fn date_of(ms: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(ms / 1000).map(|t| t.date().to_string()).unwrap_or_default()
}

#[derive(Debug, Serialize)]
pub struct KindCount {
    pub kind: &'static str,
    pub label: &'static str,
    pub severity: &'static str,
    pub total: i64,
    pub open: i64,
}

#[derive(Debug, Serialize)]
pub struct DayIncidents {
    pub date: String,
    pub counts: BTreeMap<&'static str, i64>,
    pub total: i64,
}

#[derive(Debug, Serialize)]
pub struct OpenIncident {
    pub id: String,
    pub kind: String,
    pub label: &'static str,
    pub severity: String,
    pub agent_id: Option<String>,
    pub agent_name: Option<String>,
    pub case_id: Option<String>,
    pub message_id: Option<String>,
    pub details: Value,
    pub created_at: i64,
}

#[derive(Debug, Serialize)]
pub struct CostHealth {
    pub today_usd: f64,
    pub per_email_today_usd: f64,
    pub per_email_7d_usd: f64,
    pub avg_daily_7d_usd: f64,
    pub spike: bool,
}

#[derive(Debug, Serialize)]
pub struct TaskHealth {
    pub task: String,
    pub label: &'static str,
    pub calls: i64,
    pub p50_ms: Option<f64>,
    pub p95_ms: Option<f64>,
    pub error_rate: f64,
    pub refusal_rate: f64,
    pub schema_failure_rate: f64,
}

#[derive(Debug, Serialize)]
pub struct BudgetHealth {
    pub daily_usd: f64,
    pub used_usd: f64,
    pub used_pct: f64,
}

#[derive(Debug, Serialize)]
pub struct HealthReport {
    pub days: i64,
    pub by_kind: Vec<KindCount>,
    pub by_day: Vec<DayIncidents>,
    pub open_total: i64,
    pub open: Vec<OpenIncident>,
    pub cost: CostHealth,
    pub tasks: Vec<TaskHealth>,
    pub shadow: ShadowReport,
    pub budget: BudgetHealth,
}

fn view(row: IncidentRow, agent_names: &HashMap<String, String>) -> OpenIncident {
    let label = ALL_KINDS.iter().find(|k| k.as_str() == row.kind).map_or("Other", |k| k.label());
    OpenIncident {
        label,
        agent_name: row.agent_id.as_ref().and_then(|a| agent_names.get(a).cloned()),
        details: serde_json::from_str(&row.details_json).unwrap_or(Value::Null),
        id: row.id,
        kind: row.kind,
        severity: row.severity,
        agent_id: row.agent_id,
        case_id: row.case_id,
        message_id: row.message_id,
        created_at: row.created_at,
    }
}

pub async fn report(db: &SqlitePool, ws: &str, days: i64, daily_budget_usd: f64) -> sqlx::Result<HealthReport> {
    let days = days.clamp(1, 90);
    let now = crate::db::now_ms();
    let today = day_start(now);
    let since = today - (days - 1) * DAY_MS;

    let kind_rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT kind, COUNT(*), COALESCE(SUM(resolved_at IS NULL), 0) FROM ai_incidents
         WHERE workspace_id = ?1 AND created_at >= ?2 GROUP BY kind",
    )
    .bind(ws)
    .bind(since)
    .fetch_all(db)
    .await?;
    let by_kind = ALL_KINDS
        .iter()
        .map(|k| {
            let (total, open) = kind_rows.iter().find(|(n, _, _)| n == k.as_str()).map_or((0, 0), |(_, t, o)| (*t, *o));
            KindCount { kind: k.as_str(), label: k.label(), severity: k.severity(), total, open }
        })
        .collect();

    let day_rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT strftime('%Y-%m-%d', created_at / 1000, 'unixepoch'), kind, COUNT(*) FROM ai_incidents
         WHERE workspace_id = ?1 AND created_at >= ?2 GROUP BY 1, 2",
    )
    .bind(ws)
    .bind(since)
    .fetch_all(db)
    .await?;
    let by_day = (0..days)
        .map(|i| {
            let date = date_of(since + i * DAY_MS);
            let mut counts = BTreeMap::new();
            for (_, kind, n) in day_rows.iter().filter(|(d, _, _)| *d == date) {
                if let Some(k) = ALL_KINDS.iter().find(|k| k.as_str() == kind) {
                    counts.insert(k.as_str(), *n);
                }
            }
            let total = counts.values().sum();
            DayIncidents { date, counts, total }
        })
        .collect();

    let open_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ai_incidents WHERE workspace_id = ?1 AND resolved_at IS NULL")
        .bind(ws)
        .fetch_one(db)
        .await?;
    let names: HashMap<String, String> = sqlx::query_as::<_, (String, String)>("SELECT id, name FROM agents WHERE workspace_id = ?1")
        .bind(ws)
        .fetch_all(db)
        .await?
        .into_iter()
        .collect();
    let open = incidents::list(db, ws, true, MAX_OPEN_SHOWN).await?.into_iter().map(|r| view(r, &names)).collect();

    let (shadow, cost, tasks) = (understand::shadow_report(db, ws).await?, cost_health(db, ws, now).await?, task_health(db, ws, now).await?);
    let used = cost.today_usd;
    Ok(HealthReport {
        days,
        by_kind,
        by_day,
        open_total,
        open,
        cost,
        tasks,
        shadow,
        budget: BudgetHealth {
            daily_usd: daily_budget_usd,
            used_usd: used,
            used_pct: if daily_budget_usd > 0.0 { round4(used / daily_budget_usd) } else { 0.0 },
        },
    })
}

fn placeholders(n: usize) -> String {
    vec!["?"; n].join(",")
}

async fn automatic_spend(db: &SqlitePool, ws: &str, from: i64, to: i64) -> sqlx::Result<(f64, i64)> {
    let sql = format!(
        "SELECT COALESCE(SUM(cost_usd), 0.0) FROM ai_calls WHERE workspace_id = ? AND created_at >= ? AND created_at < ? AND task IN ({})",
        placeholders(AUTOMATIC_TASKS.len())
    );
    let mut q = sqlx::query_scalar::<_, f64>(&sql).bind(ws).bind(from).bind(to);
    for t in AUTOMATIC_TASKS {
        q = q.bind(*t);
    }
    let usd = q.fetch_one(db).await?;
    let emails: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM email_messages WHERE workspace_id = ?1 AND direction = 'inbound' AND received_at >= ?2 AND received_at < ?3",
    )
    .bind(ws)
    .bind(from)
    .bind(to)
    .fetch_one(db)
    .await?;
    Ok((usd, emails))
}

async fn total_spend(db: &SqlitePool, ws: &str, from: i64, to: i64) -> sqlx::Result<f64> {
    sqlx::query_scalar("SELECT COALESCE(SUM(cost_usd), 0.0) FROM ai_calls WHERE workspace_id = ?1 AND created_at >= ?2 AND created_at < ?3")
        .bind(ws)
        .bind(from)
        .bind(to)
        .fetch_one(db)
        .await
}

pub async fn cost_health(db: &SqlitePool, ws: &str, now: i64) -> sqlx::Result<CostHealth> {
    let today = day_start(now);
    let end = now + 1;
    let today_usd = total_spend(db, ws, today, end).await?;
    let prior = total_spend(db, ws, today - 7 * DAY_MS, today).await?;
    let days_with_data: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT created_at / 86400000) FROM ai_calls WHERE workspace_id = ?1 AND created_at >= ?2 AND created_at < ?3 AND cost_usd > 0",
    )
    .bind(ws)
    .bind(today - 7 * DAY_MS)
    .bind(today)
    .fetch_one(db)
    .await?;
    let (auto_today, emails_today) = automatic_spend(db, ws, today, end).await?;
    let (auto_7d, emails_7d) = automatic_spend(db, ws, today - 6 * DAY_MS, end).await?;
    let per = |usd: f64, n: i64| if n > 0 { round6(usd / n as f64) } else { 0.0 };
    Ok(CostHealth {
        today_usd: round6(today_usd),
        per_email_today_usd: per(auto_today, emails_today),
        per_email_7d_usd: per(auto_7d, emails_7d),
        avg_daily_7d_usd: round6(prior / 7.0),
        spike: cost_spike(today_usd, prior, days_with_data),
    })
}

async fn call_rows(db: &SqlitePool, ws: &str, from: i64, to: i64) -> sqlx::Result<Vec<(String, i64, bool, Option<String>, Option<bool>)>> {
    sqlx::query_as(
        "SELECT task, latency_ms, ok, error_class, schema_ok FROM ai_calls
         WHERE workspace_id = ?1 AND created_at >= ?2 AND created_at < ?3 ORDER BY created_at DESC LIMIT 100000",
    )
    .bind(ws)
    .bind(from)
    .bind(to)
    .fetch_all(db)
    .await
}

pub async fn task_health(db: &SqlitePool, ws: &str, now: i64) -> sqlx::Result<Vec<TaskHealth>> {
    let rows = call_rows(db, ws, now - 7 * DAY_MS, now + 1).await?;
    #[derive(Default)]
    struct Acc {
        calls: i64,
        errors: i64,
        refusals: i64,
        schema: i64,
        latencies: Vec<f64>,
    }
    let mut by_task: BTreeMap<String, Acc> = BTreeMap::new();
    for (task, latency, ok, class, schema_ok) in rows {
        let a = by_task.entry(task).or_default();
        a.calls += 1;
        if ok {
            a.latencies.push(latency as f64);
        } else {
            a.errors += 1;
        }
        if class.as_deref() == Some("refusal") {
            a.refusals += 1;
        }
        if schema_ok == Some(false) {
            a.schema += 1;
        }
    }
    let rate = |n: i64, d: i64| round4(ratio(n, d).unwrap_or(0.0));
    Ok(by_task
        .into_iter()
        .map(|(task, a)| TaskHealth {
            label: task_label(&task),
            calls: a.calls,
            p50_ms: percentile(&a.latencies, 50.0),
            p95_ms: percentile(&a.latencies, 95.0),
            error_rate: rate(a.errors, a.calls),
            refusal_rate: rate(a.refusals, a.calls),
            schema_failure_rate: rate(a.schema, a.calls),
            task,
        })
        .collect())
}

/// Raises an alert and an incident when this workspace's spend or latency jumped. Called by the alerts pass.
/// Returns how many alerts were newly raised.
pub async fn evaluate_spikes(db: &SqlitePool, ws: &str, now: i64) -> sqlx::Result<usize> {
    let mut raised = 0;
    let cost = cost_health(db, ws, now).await?;
    let day = date_of(now);
    if cost.spike {
        let body = format!(
            "AI spend today is ${:.2}, more than twice the ${:.2} daily average of the last 7 days. Open AI health to see which work is costing more.",
            cost.today_usd, cost.avg_daily_7d_usd
        );
        let (_, created) = alerts::raise(
            db,
            NewAlert {
                workspace_id: ws,
                kind: "ai_cost_spike",
                severity: "high",
                title: "AI spend is unusually high today".into(),
                body,
                message_id: None,
                level: 1,
                dedupe_key: Some(format!("ai_cost_spike:{day}")),
            },
        )
        .await?;
        raised += usize::from(created);
        incidents::record(
            db,
            NewIncident::new(ws, Kind::CostSpike, json!({"today_usd": cost.today_usd, "avg_daily_7d_usd": cost.avg_daily_7d_usd}))
                .dedupe(format!("cost_spike:{day}")),
        )
        .await;
    }
    let cut = now - HOUR_MS;
    let mut recent: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut base: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let rows: Vec<(String, i64, bool)> = sqlx::query_as(
        "SELECT task, latency_ms, created_at >= ?2 FROM ai_calls
         WHERE workspace_id = ?1 AND created_at >= ?3 AND created_at <= ?4 AND ok = 1 AND task != 'embed'",
    )
    .bind(ws)
    .bind(cut)
    .bind(now - 7 * DAY_MS)
    .bind(now)
    .fetch_all(db)
    .await?;
    for (task, latency, is_recent) in rows {
        let bucket = if is_recent { &mut recent } else { &mut base };
        bucket.entry(task).or_default().push(latency as f64);
    }
    for (task, r) in &recent {
        let Some(b) = base.get(task) else { continue };
        let Some((p95, baseline)) = latency_spike(r, b) else { continue };
        let (_, created) = alerts::raise(
            db,
            NewAlert {
                workspace_id: ws,
                kind: "ai_latency_spike",
                severity: "normal",
                title: format!("{} are slower than usual", task_label(task)),
                body: format!(
                    "The slowest 5% of {} answers took about {:.1} s in the last hour, against {:.1} s over the past week.",
                    task_label(task).to_lowercase(),
                    p95 / 1000.0,
                    baseline / 1000.0
                ),
                message_id: None,
                level: 1,
                dedupe_key: Some(format!("ai_latency_spike:{task}:{day}")),
            },
        )
        .await?;
        raised += usize::from(created);
        incidents::record(
            db,
            NewIncident::new(ws, Kind::LatencySpike, json!({"task": task, "p95_ms": p95, "baseline_p95_ms": baseline}))
                .dedupe(format!("latency_spike:{task}:{day}")),
        )
        .await;
    }
    Ok(raised)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_spike_needs_double_the_average_and_the_floor() {
        assert!(cost_spike(2.5, 7.0, 7));
        assert!(!cost_spike(2.0, 7.0, 7));
        assert!(!cost_spike(1.9, 7.0, 7));
        assert!(!cost_spike(0.4, 1.0, 7));
        assert!(!cost_spike(5.0, 0.0, 0), "no baseline, no spike");
        assert!(!cost_spike(5.0, 0.7, 2), "fewer than three days of data");
        assert!(cost_spike(5.0, 0.7, 3));
    }

    #[test]
    fn latency_spike_needs_enough_calls_and_a_clear_jump() {
        let base: Vec<f64> = vec![1_000.0; 60];
        let slow: Vec<f64> = vec![5_000.0; 25];
        assert_eq!(latency_spike(&slow, &base), Some((5_000.0, 1_000.0)));
        assert_eq!(latency_spike(&slow[..5], &base), None);
        assert_eq!(latency_spike(&slow, &base[..10]), None);
        assert_eq!(latency_spike(&[1_500.0; 25], &base), None);
        assert_eq!(latency_spike(&[2_500.0; 25], &[2_000.0; 60]), None);
    }
}
