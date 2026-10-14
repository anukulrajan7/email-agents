//! Recomputes `agent_metrics_daily` from the source tables. Idempotent: the rows of the window are
//! deleted and rebuilt in one transaction, so running it twice gives identical rows.

use std::collections::BTreeMap;

use sqlx::SqlitePool;

use super::{DAY_MS, day_of, percentile, queries};
use crate::{
    agents::autonomy::AUTO_SENT_ACTION,
    db::now_ms,
    metrics::queries::Sample,
};

type Key = (String, String, i64, i64);

#[derive(Default)]
struct Day {
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
    edit_sum: f64,
    edit_n: i64,
    auto_sends: i64,
    stopped: i64,
    cost_usd: f64,
    latencies: Vec<f64>,
}

struct Rows<'a> {
    map: BTreeMap<Key, Day>,
    from_ms: i64,
    until_ms: i64,
    ws: Option<&'a str>,
}

impl Rows<'_> {
    /// The row for an event at `at`, when the event falls inside the window.
    fn at(&mut self, ws: &str, agent: Option<&str>, version: i64, at: i64) -> Option<&mut Day> {
        let agent = agent?;
        if at < self.from_ms || at >= self.until_ms || self.ws.is_some_and(|w| w != ws) {
            return None;
        }
        Some(self.map.entry((ws.to_string(), agent.to_string(), version, day_of(at))).or_default())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stop {
    pub workspace_id: String,
    pub agent_id: Option<String>,
    pub agent_version: i64,
    pub at: i64,
}

/// A person undoing a send the agent was holding is a stopped auto-send, and an incident.
pub async fn stopped_sends(db: &SqlitePool, ws: Option<&str>, from_ms: i64, until_ms: i64) -> sqlx::Result<Vec<Stop>> {
    let rows: Vec<(String, Option<String>, i64, i64)> = sqlx::query_as(
        "SELECT e.workspace_id, COALESCE(d.agent_id, c.agent_id), COALESCE(c.agent_version, 0), e.created_at
         FROM audit_events e
         LEFT JOIN drafts d ON d.id = e.entity_id
         LEFT JOIN cases c ON c.id = (SELECT s.entity_id FROM audit_events s
                                      WHERE s.action = ?4 AND json_extract(s.metadata_json, '$.draft_id') = e.entity_id LIMIT 1)
         WHERE e.action = 'draft.send_undone' AND e.created_at >= ?2 AND e.created_at < ?3 AND (?1 IS NULL OR e.workspace_id = ?1)
           AND EXISTS (SELECT 1 FROM audit_events h WHERE h.action = 'agent.send_held' AND h.entity_id = e.entity_id)",
    )
    .bind(ws)
    .bind(from_ms)
    .bind(until_ms)
    .bind(AUTO_SENT_ACTION)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(|(workspace_id, agent_id, agent_version, at)| Stop { workspace_id, agent_id, agent_version, at }).collect())
}

type ApprovalRow = (String, String, i64, String, i64, Option<i64>, bool);

/// Rebuilds the days `from_day..=to_day` (UTC) for one workspace, or for all of them. Returns the
/// number of rows written.
pub async fn run(db: &SqlitePool, ws: Option<&str>, from_day: i64, to_day: i64) -> sqlx::Result<usize> {
    let (from_ms, until_ms) = (from_day * DAY_MS, (to_day + 1) * DAY_MS);
    let mut rows = Rows { map: BTreeMap::new(), from_ms, until_ms, ws };

    let routes: Vec<(String, String, i64, i64)> = sqlx::query_as(
        "SELECT workspace_id, agent_id, agent_version, created_at FROM message_routes
         WHERE shadow = 0 AND created_at >= ?2 AND created_at < ?3 AND (?1 IS NULL OR workspace_id = ?1)",
    )
    .bind(ws)
    .bind(from_ms)
    .bind(until_ms)
    .fetch_all(db)
    .await?;
    for (w, agent, version, at) in routes {
        if let Some(d) = rows.at(&w, Some(&agent), version, at) {
            d.emails_routed += 1;
        }
    }

    let cases: Vec<(String, String, i64, String, i64, i64)> = sqlx::query_as(
        "SELECT workspace_id, agent_id, COALESCE(agent_version, 0), status, created_at, updated_at FROM cases
         WHERE agent_id IS NOT NULL AND (?1 IS NULL OR workspace_id = ?1)
           AND ((created_at >= ?2 AND created_at < ?3) OR (updated_at >= ?2 AND updated_at < ?3))",
    )
    .bind(ws)
    .bind(from_ms)
    .bind(until_ms)
    .fetch_all(db)
    .await?;
    for (w, agent, version, status, created, updated) in cases {
        if let Some(d) = rows.at(&w, Some(&agent), version, created) {
            d.cases_started += 1;
        }
        if let Some(d) = rows.at(&w, Some(&agent), version, updated) {
            match status.as_str() {
                "done" => d.cases_done += 1,
                "failed" => d.cases_failed += 1,
                "cancelled" => d.cases_cancelled += 1,
                _ => {}
            }
        }
    }
    for s in queries::latency_samples(db, ws, from_ms, until_ms).await? {
        if let Some(d) = rows.at(&s.workspace_id, Some(&s.agent_id), s.agent_version, s.at) {
            d.latencies.push(s.value);
        }
    }

    let approvals: Vec<ApprovalRow> = sqlx::query_as(
        "SELECT a.workspace_id, COALESCE(a.agent_id, c.agent_id), COALESCE(c.agent_version, 0), a.status, a.created_at, a.decided_at,
                COALESCE(d.original_body_text IS NOT NULL AND TRIM(d.original_body_text) != TRIM(d.body_text), 0)
         FROM agent_approvals a JOIN cases c ON c.id = a.case_id LEFT JOIN drafts d ON d.id = a.draft_id
         WHERE COALESCE(a.agent_id, c.agent_id) IS NOT NULL AND (?1 IS NULL OR a.workspace_id = ?1)
           AND ((a.created_at >= ?2 AND a.created_at < ?3) OR (a.decided_at >= ?2 AND a.decided_at < ?3))",
    )
    .bind(ws)
    .bind(from_ms)
    .bind(until_ms)
    .fetch_all(db)
    .await?;
    for (w, agent, version, status, created, decided, edited) in approvals {
        if let Some(d) = rows.at(&w, Some(&agent), version, created) {
            d.approvals_requested += 1;
        }
        if let Some(at) = decided
            && let Some(d) = rows.at(&w, Some(&agent), version, at)
        {
            match status.as_str() {
                "approved" => {
                    d.approvals_approved += 1;
                    d.approvals_edited += i64::from(edited);
                }
                "declined" => d.approvals_declined += 1,
                _ => {}
            }
        }
    }

    let drafts: Vec<(String, String, i64, String, i64, i64)> = sqlx::query_as(
        "SELECT d.workspace_id, d.agent_id, COALESCE(r.agent_version, ag.active_version, 0), d.status, d.created_at, d.updated_at
         FROM drafts d LEFT JOIN message_routes r ON r.message_id = d.message_id LEFT JOIN agents ag ON ag.id = d.agent_id
         WHERE d.agent_id IS NOT NULL AND (?1 IS NULL OR d.workspace_id = ?1)
           AND ((d.created_at >= ?2 AND d.created_at < ?3) OR (d.updated_at >= ?2 AND d.updated_at < ?3))",
    )
    .bind(ws)
    .bind(from_ms)
    .bind(until_ms)
    .fetch_all(db)
    .await?;
    for (w, agent, version, status, created, updated) in drafts {
        if let Some(d) = rows.at(&w, Some(&agent), version, created) {
            d.drafts_created += 1;
        }
        if status == "sent"
            && let Some(d) = rows.at(&w, Some(&agent), version, updated)
        {
            d.drafts_sent += 1;
        }
    }
    for s in queries::edit_samples(db, ws, from_ms, until_ms).await? {
        let Sample { workspace_id, agent_id, agent_version, at, value } = s;
        if let Some(d) = rows.at(&workspace_id, Some(&agent_id), agent_version, at) {
            d.edit_sum += value;
            d.edit_n += 1;
        }
    }

    let auto: Vec<(String, Option<String>, i64, i64)> = sqlx::query_as(
        "SELECT e.workspace_id, c.agent_id, COALESCE(c.agent_version, 0), e.created_at
         FROM audit_events e JOIN cases c ON c.id = e.entity_id
         WHERE e.action = ?4 AND e.entity_type = 'case' AND e.created_at >= ?2 AND e.created_at < ?3 AND (?1 IS NULL OR e.workspace_id = ?1)",
    )
    .bind(ws)
    .bind(from_ms)
    .bind(until_ms)
    .bind(AUTO_SENT_ACTION)
    .fetch_all(db)
    .await?;
    for (w, agent, version, at) in auto {
        if let Some(d) = rows.at(&w, agent.as_deref(), version, at) {
            d.auto_sends += 1;
        }
    }

    for stop in stopped_sends(db, ws, from_ms, until_ms).await? {
        if let Some(d) = rows.at(&stop.workspace_id, stop.agent_id.as_deref(), stop.agent_version, stop.at) {
            d.stopped += 1;
        }
    }

    // Spend: a call belongs to its case's agent, else to the agent the email was routed to. Eval
    // runs are not agent work.
    let cost: Vec<(String, Option<String>, i64, i64, f64)> = sqlx::query_as(
        "SELECT a.workspace_id, COALESCE(c.agent_id, r.agent_id), COALESCE(c.agent_version, r.agent_version, 0), a.created_at, a.cost_usd
         FROM ai_calls a LEFT JOIN cases c ON c.id = a.case_id LEFT JOIN message_routes r ON r.message_id = a.message_id
         WHERE a.workspace_id IS NOT NULL AND (a.message_id IS NULL OR a.message_id NOT LIKE 'eval:%')
           AND a.created_at >= ?2 AND a.created_at < ?3 AND (?1 IS NULL OR a.workspace_id = ?1)",
    )
    .bind(ws)
    .bind(from_ms)
    .bind(until_ms)
    .fetch_all(db)
    .await?;
    for (w, agent, version, at, usd) in cost {
        if let Some(d) = rows.at(&w, agent.as_deref(), version, at) {
            d.cost_usd += usd;
        }
    }

    let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query("DELETE FROM agent_metrics_daily WHERE day >= ?2 AND day <= ?3 AND (?1 IS NULL OR workspace_id = ?1)")
        .bind(ws)
        .bind(from_day)
        .bind(to_day)
        .execute(&mut *tx)
        .await?;
    let written = rows.map.len();
    for ((w, agent, version, day), d) in rows.map {
        sqlx::query(
            "INSERT INTO agent_metrics_daily
               (workspace_id, agent_id, agent_version, day, emails_routed, cases_started, cases_done, cases_failed, cases_cancelled,
                approvals_requested, approvals_approved, approvals_edited, approvals_declined, drafts_created, drafts_sent,
                edit_distance_sum, edit_distance_n, auto_sends, auto_sends_stopped, incidents, cost_usd, latency_p50_ms, latency_p95_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?19, ?20, ?21, ?22)",
        )
        .bind(&w)
        .bind(&agent)
        .bind(version)
        .bind(day)
        .bind(d.emails_routed)
        .bind(d.cases_started)
        .bind(d.cases_done)
        .bind(d.cases_failed)
        .bind(d.cases_cancelled)
        .bind(d.approvals_requested)
        .bind(d.approvals_approved)
        .bind(d.approvals_edited)
        .bind(d.approvals_declined)
        .bind(d.drafts_created)
        .bind(d.drafts_sent)
        .bind(d.edit_sum)
        .bind(d.edit_n)
        .bind(d.auto_sends)
        .bind(d.stopped)
        .bind(d.cost_usd)
        .bind(percentile(&d.latencies, 50.0).map(|v| v as i64))
        .bind(percentile(&d.latencies, 95.0).map(|v| v as i64))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(written)
}

/// The window the hourly job keeps fresh: yesterday and today.
pub async fn run_recent(db: &SqlitePool, ws: Option<&str>) -> sqlx::Result<usize> {
    let today = day_of(now_ms());
    run(db, ws, today - 1, today).await
}
