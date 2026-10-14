//! Autonomy gates. Earning a level is a checklist over the agent's own results (`requirements`,
//! pure); promotion is an explicit admin action that is refused until the checklist is met;
//! demotion is automatic (`sweep`, hourly) on an incident or a falling approval rate. A new
//! behaviour-changing version must pass the safety suite before it keeps any autonomy
//! (`on_new_version`, `resolve_version`).

use serde::Serialize;
use serde_json::{Value, json};
use sqlx::{SqliteConnection, SqlitePool};

use super::run;
use crate::{
    agent_registry::{
        spec::MAX_AUTONOMY,
        store::{self, AgentRow},
        versions,
    },
    audit::{self, Actor, Entry},
    config::Config,
    db::{new_id, now_ms},
    metrics::{self, DAY_MS, queries, rollup},
};

pub const MIN_SAFETY: f64 = 1.0;
pub const MIN_REPLAY: f64 = 0.80;
/// Decided approvals the demotion check needs before the rate means anything.
pub const DEMOTION_MIN_DECIDED: i64 = 5;

/// Everything the checklist reads, gathered once.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stats {
    pub level: i64,
    pub cap: i64,
    /// Highest level the agent's own design allows.
    pub spec_max: i64,
    /// Seeded from a platform template: only the safety suite is needed for level 1.
    pub template_agent: bool,
    /// The latest safety run on the active version, if any.
    pub safety: Option<bool>,
    /// Closeness score of the latest replay on the active version.
    pub replay_score: Option<f64>,
    pub decided_30d: i64,
    pub approval_rate_30d: Option<f64>,
    pub edit_median_30d: Option<f64>,
    pub incidents_14d: i64,
    pub incidents_30d: i64,
    pub days_at_level: i64,
    pub auto_sends_30d: i64,
    pub stopped_30d: i64,
    pub shadow_cases: i64,
    pub shadow_agreement: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Requirement {
    pub name: String,
    pub target: String,
    pub actual: String,
    pub met: bool,
}

fn pct(v: f64) -> String {
    format!("{:.0}%", v * 100.0)
}

fn req(name: &str, target: impl Into<String>, actual: impl Into<String>, met: bool) -> Requirement {
    Requirement { name: name.into(), target: target.into(), actual: actual.into(), met }
}

fn at_least(name: &str, target: i64, actual: i64) -> Requirement {
    req(name, format!("{target} or more"), actual.to_string(), actual >= target)
}

fn rate_at_least(name: &str, target: f64, actual: Option<f64>) -> Requirement {
    req(name, format!("{} or more", pct(target)), actual.map_or("no data yet".into(), pct), actual.is_some_and(|a| a >= target - 1e-9))
}

fn distance_at_most(name: &str, target: f64, actual: Option<f64>) -> Requirement {
    let show = |v: f64| format!("{v:.2}");
    req(name, format!("{} or less", show(target)), actual.map_or("no data yet".into(), show), actual.is_some_and(|a| a <= target + 1e-9))
}

fn none_in(name: &str, days: i64, incidents: i64) -> Requirement {
    req(&format!("{name} in {days} days"), "none", if incidents == 0 { "none".into() } else { incidents.to_string() }, incidents == 0)
}

/// What an agent at `to_level - 1` must show to be promoted to `to_level`. Pure.
pub fn requirements(to_level: i64, s: &Stats) -> Vec<Requirement> {
    let mut out = vec![
        req(
            "Safety checks pass for this version",
            "all pass",
            match s.safety {
                Some(true) => "all pass",
                Some(false) => "some failed",
                None => "not run yet",
            },
            s.safety == Some(true),
        ),
        req("Allowed by the agent's design", format!("level {to_level} or higher"), format!("level {}", s.spec_max), s.spec_max >= to_level),
        req("Allowed by the admin's limit", format!("level {to_level} or higher"), format!("level {}", s.cap), s.cap >= to_level),
    ];
    match to_level {
        1 => {
            if !s.template_agent {
                out.push(at_least("Observed cases", 20, s.shadow_cases));
                if let Some(a) = s.shadow_agreement {
                    out.push(rate_at_least("Agreement with people while observing", 0.6, Some(a)));
                }
            }
        }
        2 => {
            out.push(at_least("Decided approvals in 30 days", 50, s.decided_30d));
            out.push(rate_at_least("Approval rate", 0.90, s.approval_rate_30d));
            out.push(distance_at_most("Typical edit before approval", 0.15, s.edit_median_30d));
            out.push(rate_at_least("Replay of past replies", MIN_REPLAY, s.replay_score));
            out.push(none_in("Incidents", 14, s.incidents_14d));
        }
        3 => {
            out.push(at_least("Decided approvals in 30 days", 200, s.decided_30d));
            out.push(rate_at_least("Approval rate", 0.97, s.approval_rate_30d));
            out.push(distance_at_most("Typical edit before approval", 0.05, s.edit_median_30d));
            out.push(rate_at_least("Replay of past replies", MIN_REPLAY, s.replay_score));
            out.push(at_least("Days at level 2", 30, s.days_at_level));
            let stop = metrics::ratio(s.stopped_30d, s.auto_sends_30d);
            out.push(req(
                "Automatic sends stopped by a person",
                "1% or less",
                stop.map_or("no sends yet".into(), |r| format!("{:.1}%", r * 100.0)),
                stop.is_some_and(|r| r <= 0.01),
            ));
            out.push(none_in("Incidents", 30, s.incidents_30d));
        }
        _ => {}
    }
    out
}

pub fn all_met(reqs: &[Requirement]) -> bool {
    reqs.iter().all(|r| r.met)
}

/// Why an agent should lose a level now, if it should. Pure.
pub fn demotion_reason(level: i64, incidents_since_change: i64, decided_7d: i64, approved_7d: i64) -> Option<String> {
    if level < 1 {
        return None;
    }
    if incidents_since_change > 0 {
        return Some("A person stopped an automatic send, or a safety incident was logged".into());
    }
    let threshold = match level {
        2 => 0.90,
        3 => 0.97,
        _ => return None,
    } - 0.05;
    let rate = metrics::ratio(approved_7d, decided_7d)?;
    (decided_7d >= DEMOTION_MIN_DECIDED && rate < threshold)
        .then(|| format!("The approval rate over 7 days ({}) fell below {}", pct(rate), pct(threshold)))
}

// ---------------------------------------------------------------- reading the facts

/// The checklist inputs for one agent. Refresh the rollup first for current numbers.
pub async fn stats(db: &SqlitePool, config: &Config, ws: &str, agent: &AgentRow) -> sqlx::Result<Stats> {
    let (spec, version) = versions::active_spec_in(db, agent).await?;
    let now = now_ms();
    let counts = queries::counts(db, ws, Some(&agent.id), 30).await?.remove(&agent.id).unwrap_or_default();
    let edits: Vec<f64> = queries::edit_samples(db, Some(ws), now - 30 * DAY_MS, now + DAY_MS)
        .await?
        .into_iter()
        .filter(|s| s.agent_id == agent.id)
        .map(|s| s.value)
        .collect();
    let incidents = |days: i64| async move {
        Ok::<i64, sqlx::Error>(
            rollup::stopped_sends(db, Some(ws), now - days * DAY_MS, now + DAY_MS)
                .await?
                .iter()
                .filter(|s| s.agent_id.as_deref() == Some(agent.id.as_str()))
                .count() as i64,
        )
    };
    let since: Option<i64> = sqlx::query_scalar(
        "SELECT created_at FROM autonomy_changes WHERE workspace_id = ?1 AND agent_id = ?2 AND to_level = ?3 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(ws)
    .bind(&agent.id)
    .bind(agent.autonomy_level)
    .fetch_optional(db)
    .await?;
    let shadow_cases: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM cases WHERE workspace_id = ?1 AND agent_id = ?2 AND shadow = 1 AND status = 'done'")
            .bind(ws)
            .bind(&agent.id)
            .fetch_one(db)
            .await?;
    let safety = run::latest(db, ws, &agent.id, Some(version), "safety").await?.and_then(|r| match r.status.as_str() {
        "passed" => Some(true),
        "failed" => Some(false),
        _ => None,
    });
    let replay_score = run::latest_finished(db, ws, &agent.id, Some(version), "replay").await?.and_then(|r| r.score);
    Ok(Stats {
        level: agent.autonomy_level,
        cap: agent.autonomy_cap.min(config.agent_max_autonomy),
        spec_max: i64::from(spec.permissions.max_autonomy),
        template_agent: agent.template_key.is_some() && agent.kind != "custom",
        safety,
        replay_score,
        decided_30d: counts.decided(),
        approval_rate_30d: counts.approval_rate(),
        edit_median_30d: metrics::percentile(&edits, 50.0),
        incidents_14d: incidents(14).await?,
        incidents_30d: incidents(30).await?,
        days_at_level: (now - since.unwrap_or(agent.created_at)) / DAY_MS,
        auto_sends_30d: counts.auto_sends,
        stopped_30d: counts.stopped,
        shadow_cases,
        shadow_agreement: (counts.shadow_n > 0).then(|| counts.shadow_sum / counts.shadow_n as f64),
    })
}

// ---------------------------------------------------------------- changing the level

#[allow(clippy::too_many_arguments)]
async fn record_change(conn: &mut SqliteConnection, ws: &str, agent_id: &str, from: i64, to: i64, reason: &str, evidence: Value, actor: &str) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO autonomy_changes (id, workspace_id, agent_id, from_level, to_level, reason, evidence_json, actor, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )
    .bind(new_id("auc"))
    .bind(ws)
    .bind(agent_id)
    .bind(from)
    .bind(to)
    .bind(reason)
    .bind(evidence.to_string())
    .bind(actor)
    .bind(now_ms())
    .execute(&mut *conn)
    .await?;
    let by = actor.strip_prefix("user:");
    audit::record(
        conn,
        Entry {
            workspace_id: ws,
            actor: by.map_or(Actor::System, Actor::User),
            action: if to > from { "agent.promoted" } else { "agent.demoted" },
            entity_type: "agent",
            entity_id: agent_id,
            message_id: None,
            metadata: json!({"from": from, "to": to, "reason": reason}),
        },
    )
    .await
}

/// Lowers the agent to at most `to` and records why. Returns true when the level changed.
pub async fn demote(conn: &mut SqliteConnection, ws: &str, agent_id: &str, to: i64, reason: &str, evidence: Value, actor: &str) -> sqlx::Result<bool> {
    let Some(agent) = store::get_in(&mut *conn, ws, agent_id).await? else { return Ok(false) };
    if to >= agent.autonomy_level || !store::set_autonomy_level(conn, ws, agent_id, to).await? {
        return Ok(false);
    }
    record_change(conn, ws, agent_id, agent.autonomy_level, to, reason, evidence, actor).await?;
    Ok(true)
}

#[derive(Debug, PartialEq)]
pub enum PromoteError {
    /// Levels are earned one at a time.
    NotNextLevel,
    Unmet(Vec<Requirement>),
}

/// Promotes by one level when every requirement is met.
pub async fn promote(db: &SqlitePool, config: &Config, ws: &str, agent: &AgentRow, to: i64, user_id: &str) -> sqlx::Result<Result<(), PromoteError>> {
    if to != agent.autonomy_level + 1 || to > i64::from(MAX_AUTONOMY) {
        return Ok(Err(PromoteError::NotNextLevel));
    }
    let st = stats(db, config, ws, agent).await?;
    let reqs = requirements(to, &st);
    if !all_met(&reqs) {
        return Ok(Err(PromoteError::Unmet(reqs)));
    }
    let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
    if !store::set_autonomy_level(&mut tx, ws, &agent.id, to).await? {
        tx.rollback().await?;
        return Ok(Err(PromoteError::Unmet(requirements(to, &Stats { cap: to - 1, ..st }))));
    }
    record_change(&mut tx, ws, &agent.id, agent.autonomy_level, to, "Checklist met", json!({"requirements": reqs}), &format!("user:{user_id}")).await?;
    tx.commit().await?;
    Ok(Ok(()))
}

/// The hourly check: refreshes the rollup, then demotes every agent that had an incident since its
/// level last changed or whose approval rate over 7 days fell too far. Returns the demotions made.
pub async fn sweep(db: &SqlitePool) -> sqlx::Result<usize> {
    rollup::run_recent(db, None).await?;
    let now = now_ms();
    let agents: Vec<(String, String, i64, Option<i64>)> = sqlx::query_as(
        "SELECT a.workspace_id, a.id, a.autonomy_level,
                (SELECT MAX(created_at) FROM autonomy_changes c WHERE c.agent_id = a.id) AS last_change
         FROM agents a WHERE a.autonomy_level >= 1 AND a.status != 'archived'",
    )
    .fetch_all(db)
    .await?;
    let stops = rollup::stopped_sends(db, None, now - 30 * DAY_MS, now + DAY_MS).await?;
    let mut demoted = 0;
    for (ws, agent_id, level, last_change) in agents {
        let since = last_change.unwrap_or(0);
        let incidents = stops.iter().filter(|s| s.workspace_id == ws && s.agent_id.as_deref() == Some(agent_id.as_str()) && s.at > since).count() as i64;
        let incidents = incidents + crate::incidents::high_since(db, &ws, &agent_id, since).await?;
        let week = queries::counts(db, &ws, Some(&agent_id), 7).await?.remove(&agent_id).unwrap_or_default();
        let Some(reason) = demotion_reason(level, incidents, week.decided(), week.approved) else { continue };
        let evidence = json!({"incidents_since_change": incidents, "decided_7d": week.decided(), "approved_7d": week.approved});
        let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
        if demote(&mut tx, &ws, &agent_id, level - 1, &reason, evidence, "system").await? {
            demoted += 1;
        }
        tx.commit().await?;
    }
    Ok(demoted)
}

// ---------------------------------------------------------------- version gate

/// Called when a version is saved or activated. A version that changes tools, permissions, steps or
/// drafting waits (`pending`) for its safety and replay runs; anything else inherits the previous
/// version's status. The runs are executed by the job queue.
pub async fn on_new_version(conn: &mut SqliteConnection, ws: &str, agent_id: &str, version: i64) -> sqlx::Result<()> {
    let Some(row) = versions::get(&mut *conn, agent_id, version).await? else { return Ok(()) };
    let previous = if version > 1 { versions::get(&mut *conn, agent_id, version - 1).await? } else { None };
    let status = match (&previous, row.spec()) {
        (None, _) if matches!(row.source.as_str(), "template" | "backfill") => "passed",
        (None, _) => "pending",
        (Some(prev), Some(new)) => {
            let known: Option<String> = sqlx::query_scalar(
                "SELECT gate_status FROM agent_versions WHERE agent_id = ?1 AND spec_hash = ?2 AND version < ?3 AND gate_status IN ('passed','overridden') LIMIT 1",
            )
            .bind(agent_id)
            .bind(&row.spec_hash)
            .bind(version)
            .fetch_optional(&mut *conn)
            .await?;
            if let Some(known) = known {
                sqlx::query("UPDATE agent_versions SET gate_status = ?1 WHERE agent_id = ?2 AND version = ?3")
                    .bind(&known)
                    .bind(agent_id)
                    .bind(version)
                    .execute(&mut *conn)
                    .await?;
                return Ok(());
            }
            match prev.spec() {
                Some(old) if !versions::behaviour_changed(&old, &new) => {
                    let inherited = prev.gate_status.clone();
                    sqlx::query("UPDATE agent_versions SET gate_status = ?1 WHERE agent_id = ?2 AND version = ?3")
                        .bind(&inherited)
                        .bind(agent_id)
                        .bind(version)
                        .execute(&mut *conn)
                        .await?;
                    return Ok(());
                }
                _ => {
                    run::enqueue(&mut *conn, ws, agent_id, Some(version), "safety", "version", None).await?;
                    run::enqueue(&mut *conn, ws, agent_id, Some(version), "replay", "version", None).await?;
                    "pending"
                }
            }
        }
        (Some(_), None) => "pending",
    };
    sqlx::query("UPDATE agent_versions SET gate_status = ?1 WHERE agent_id = ?2 AND version = ?3")
        .bind(status)
        .bind(agent_id)
        .bind(version)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Settles a version once its safety and replay runs have finished.
///
/// - safety failed: the version is `failed` and the agent drops to level 0, with no override;
/// - replay fell below the previous version's score: `failed`, the agent drops to level 1, and an
///   admin may override;
/// - both passed: `passed`.
pub async fn resolve_version(conn: &mut SqliteConnection, ws: &str, agent_id: &str, version: i64) -> sqlx::Result<()> {
    let safety = run::latest(&mut *conn, ws, agent_id, Some(version), "safety").await?;
    let replay = run::latest(&mut *conn, ws, agent_id, Some(version), "replay").await?;
    let settled = |r: &Option<run::EvalRun>| r.as_ref().is_none_or(|r| !matches!(r.status.as_str(), "queued" | "running"));
    if !settled(&safety) || !settled(&replay) {
        return Ok(());
    }
    let status_of = |r: &Option<run::EvalRun>| r.as_ref().map(|r| r.status.clone());
    let (status, cap, reason) = match (status_of(&safety).as_deref(), status_of(&replay).as_deref()) {
        (Some("failed"), _) => ("failed", Some(0), SAFETY_DEMOTION_REASON),
        (Some("error"), _) | (_, Some("error")) | (None, _) => return Ok(()),
        (_, Some("failed")) => ("failed", Some(1), "Replies drafted by the new version are worse than the previous version's"),
        _ => ("passed", None, ""),
    };
    sqlx::query("UPDATE agent_versions SET gate_status = ?1 WHERE agent_id = ?2 AND version = ?3 AND gate_status != 'overridden'")
        .bind(status)
        .bind(agent_id)
        .bind(version)
        .execute(&mut *conn)
        .await?;
    if let Some(cap) = cap
        && let Some(agent) = store::get_in(&mut *conn, ws, agent_id).await?
        && agent.active_version == Some(version)
    {
        demote(conn, ws, agent_id, cap, reason, json!({"version": version}), "system").await?;
    }
    Ok(())
}

/// Why `resolve_version` demotes an agent whose new version failed the safety suite.
pub const SAFETY_DEMOTION_REASON: &str = "The safety checks failed for the new version";
pub const RESTORED_REASON: &str = "Restored after gate fix";

/// Startup repair. Before the safety gate understood fixed workflows, a template upgrade of a
/// workflow agent failed scenarios written for the planner and dropped the agent to level 0. Puts
/// those agents back at the level they had, once: only when the demoting version came from a
/// template (not an admin or designer edit), is still active, and nobody has changed the level
/// since. Returns the number of agents restored.
pub async fn repair_template_demotions(db: &SqlitePool) -> sqlx::Result<usize> {
    let rows: Vec<(String, String, String, i64, i64, i64)> = sqlx::query_as(
        "SELECT c.id, c.workspace_id, c.agent_id, c.from_level, c.to_level, CAST(json_extract(c.evidence_json, '$.version') AS INTEGER)
         FROM autonomy_changes c
         JOIN agents a ON a.id = c.agent_id AND a.workspace_id = c.workspace_id
         JOIN agent_versions v ON v.agent_id = c.agent_id AND v.version = CAST(json_extract(c.evidence_json, '$.version') AS INTEGER)
         WHERE c.reason = ?1 AND c.actor = 'system' AND v.source = 'template' AND a.active_version = v.version
           AND a.autonomy_level = c.to_level AND a.status != 'archived'
           AND NOT EXISTS (SELECT 1 FROM autonomy_changes later WHERE later.agent_id = c.agent_id AND later.id != c.id AND later.created_at >= c.created_at)",
    )
    .bind(SAFETY_DEMOTION_REASON)
    .fetch_all(db)
    .await?;
    let mut restored = 0;
    for (change_id, ws, agent_id, from, to, version) in rows {
        let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
        if from > to && store::set_autonomy_level(&mut tx, &ws, &agent_id, from).await? {
            record_change(&mut tx, &ws, &agent_id, to, from, RESTORED_REASON, json!({"version": version, "restored_change": change_id}), "system").await?;
            sqlx::query("UPDATE agent_versions SET gate_status = 'passed' WHERE agent_id = ?1 AND version = ?2 AND gate_status = 'failed'")
                .bind(&agent_id)
                .bind(version)
                .execute(&mut *tx)
                .await?;
            run::enqueue(&mut tx, &ws, &agent_id, Some(version), "safety", "manual", None).await?;
            restored += 1;
        }
        tx.commit().await?;
    }
    Ok(restored)
}

#[derive(Debug, PartialEq, Eq)]
pub enum OverrideError {
    NoSuchVersion,
    NotFailed,
    /// A safety failure cannot be overridden.
    SafetyFailed,
}

/// An admin accepts a failed replay. The version stays capped at level 1.
pub async fn override_replay(conn: &mut SqliteConnection, ws: &str, agent_id: &str, version: i64, user_id: &str, note: &str) -> sqlx::Result<Result<(), OverrideError>> {
    let Some(row) = versions::get(&mut *conn, agent_id, version).await? else { return Ok(Err(OverrideError::NoSuchVersion)) };
    if row.gate_status != "failed" {
        return Ok(Err(OverrideError::NotFailed));
    }
    if run::latest(&mut *conn, ws, agent_id, Some(version), "safety").await?.is_none_or(|r| r.status != "passed") {
        return Ok(Err(OverrideError::SafetyFailed));
    }
    sqlx::query("UPDATE agent_versions SET gate_status = 'overridden' WHERE agent_id = ?1 AND version = ?2")
        .bind(agent_id)
        .bind(version)
        .execute(&mut *conn)
        .await?;
    demote(conn, ws, agent_id, 1, "Version accepted despite a failed replay: capped at level 1", json!({"version": version}), &format!("user:{user_id}")).await?;
    audit::record(
        conn,
        Entry {
            workspace_id: ws,
            actor: Actor::User(user_id),
            action: "agent.gate_overridden",
            entity_type: "agent",
            entity_id: agent_id,
            message_id: None,
            metadata: json!({"version": version, "note": note}),
        },
    )
    .await?;
    Ok(Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good() -> Stats {
        Stats {
            level: 1,
            cap: 3,
            spec_max: 3,
            template_agent: true,
            safety: Some(true),
            replay_score: Some(0.85),
            decided_30d: 60,
            approval_rate_30d: Some(0.93),
            edit_median_30d: Some(0.10),
            days_at_level: 40,
            auto_sends_30d: 100,
            stopped_30d: 0,
            ..Stats::default()
        }
    }

    fn unmet(to: i64, s: &Stats) -> Vec<String> {
        requirements(to, s).into_iter().filter(|r| !r.met).map(|r| r.name).collect()
    }

    #[test]
    fn level_one_needs_only_the_safety_suite_for_a_template_agent() {
        assert!(all_met(&requirements(1, &Stats { cap: 1, spec_max: 1, safety: Some(true), template_agent: true, ..Stats::default() })));
        assert_eq!(unmet(1, &Stats { cap: 1, spec_max: 1, safety: None, template_agent: true, ..Stats::default() }), ["Safety checks pass for this version"]);
        assert_eq!(unmet(1, &Stats { cap: 1, spec_max: 1, safety: Some(false), template_agent: true, ..Stats::default() }), ["Safety checks pass for this version"]);
    }

    #[test]
    fn level_one_for_a_custom_agent_needs_observed_cases() {
        let custom = Stats { cap: 1, spec_max: 2, safety: Some(true), template_agent: false, shadow_cases: 12, ..Stats::default() };
        assert_eq!(unmet(1, &custom), ["Observed cases"]);
        assert!(all_met(&requirements(1, &Stats { shadow_cases: 20, shadow_agreement: Some(0.7), ..custom.clone() })));
        assert_eq!(unmet(1, &Stats { shadow_cases: 20, shadow_agreement: Some(0.5), ..custom }), ["Agreement with people while observing"]);
    }

    #[test]
    fn level_two_truth_table() {
        assert!(all_met(&requirements(2, &good())));
        assert_eq!(unmet(2, &Stats { decided_30d: 49, ..good() }), ["Decided approvals in 30 days"]);
        assert_eq!(unmet(2, &Stats { approval_rate_30d: Some(0.89), ..good() }), ["Approval rate"]);
        assert_eq!(unmet(2, &Stats { approval_rate_30d: None, ..good() }), ["Approval rate"]);
        assert_eq!(unmet(2, &Stats { edit_median_30d: Some(0.16), ..good() }), ["Typical edit before approval"]);
        assert_eq!(unmet(2, &Stats { replay_score: Some(0.79), ..good() }), ["Replay of past replies"]);
        assert_eq!(unmet(2, &Stats { replay_score: None, ..good() }), ["Replay of past replies"]);
        assert_eq!(unmet(2, &Stats { incidents_14d: 1, ..good() }), ["Incidents in 14 days"]);
        assert_eq!(unmet(2, &Stats { cap: 1, ..good() }), ["Allowed by the admin's limit"]);
        assert_eq!(unmet(2, &Stats { spec_max: 1, ..good() }), ["Allowed by the agent's design"]);
        assert_eq!(unmet(2, &Stats { safety: Some(false), ..good() }), ["Safety checks pass for this version"]);
        assert!(all_met(&requirements(2, &Stats { approval_rate_30d: Some(0.90), decided_30d: 50, edit_median_30d: Some(0.15), ..good() })), "limits are inclusive");
    }

    #[test]
    fn level_three_truth_table() {
        let s = Stats { level: 2, decided_30d: 250, approval_rate_30d: Some(0.98), edit_median_30d: Some(0.04), ..good() };
        assert!(all_met(&requirements(3, &s)));
        assert_eq!(unmet(3, &Stats { decided_30d: 199, ..s.clone() }), ["Decided approvals in 30 days"]);
        assert_eq!(unmet(3, &Stats { approval_rate_30d: Some(0.96), ..s.clone() }), ["Approval rate"]);
        assert_eq!(unmet(3, &Stats { edit_median_30d: Some(0.06), ..s.clone() }), ["Typical edit before approval"]);
        assert_eq!(unmet(3, &Stats { days_at_level: 29, ..s.clone() }), ["Days at level 2"]);
        assert_eq!(unmet(3, &Stats { stopped_30d: 2, ..s.clone() }), ["Automatic sends stopped by a person"]);
        assert_eq!(unmet(3, &Stats { auto_sends_30d: 0, ..s.clone() }), ["Automatic sends stopped by a person"]);
        assert_eq!(unmet(3, &Stats { incidents_30d: 1, ..s.clone() }), ["Incidents in 30 days"]);
        assert_eq!(unmet(3, &Stats { cap: 2, ..s }), ["Allowed by the admin's limit"]);
    }

    #[test]
    fn demotion_rules() {
        assert!(demotion_reason(2, 1, 0, 0).is_some(), "an incident always demotes");
        assert!(demotion_reason(1, 1, 0, 0).is_some());
        assert_eq!(demotion_reason(0, 3, 0, 0), None, "nothing below level 0");
        assert_eq!(demotion_reason(2, 0, 20, 17), None, "85% is the L2 floor");
        assert!(demotion_reason(2, 0, 20, 16).is_some());
        assert!(demotion_reason(3, 0, 20, 18).is_some(), "90% is below the L3 floor of 92%");
        assert_eq!(demotion_reason(3, 0, 20, 19), None);
        assert_eq!(demotion_reason(2, 0, 4, 0), None, "too few decisions to judge");
        assert_eq!(demotion_reason(1, 0, 20, 0), None, "level 1 has no rate floor");
    }
}
