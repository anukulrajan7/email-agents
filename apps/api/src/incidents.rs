//! The AI incident log. Every detection point (injection gate, leak and send checks, unsupported claims,
//! malformed or refused model answers, spend and latency spikes) writes one row here. Details carry ids and
//! short enum-like facts only, never email bodies. A high-severity incident on an agent lowers its
//! autonomy level at once through the same demotion the hourly sweep uses.

use serde::Serialize;
use serde_json::{Map, Value, json};
use sqlx::SqlitePool;

use crate::{
    agent_registry::store,
    db::{new_id, now_ms},
    evals::gate,
    sources::normalize::truncate_chars,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    LeakBlocked,
    InjectionDetected,
    Refusal,
    SchemaFailure,
    UnsupportedClaim,
    SendBlocked,
    CostSpike,
    LatencySpike,
}

pub const ALL_KINDS: [Kind; 8] = [
    Kind::LeakBlocked,
    Kind::InjectionDetected,
    Kind::Refusal,
    Kind::SchemaFailure,
    Kind::UnsupportedClaim,
    Kind::SendBlocked,
    Kind::CostSpike,
    Kind::LatencySpike,
];

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::LeakBlocked => "leak_blocked",
            Kind::InjectionDetected => "injection_detected",
            Kind::Refusal => "refusal",
            Kind::SchemaFailure => "schema_failure",
            Kind::UnsupportedClaim => "unsupported_claim",
            Kind::SendBlocked => "send_blocked",
            Kind::CostSpike => "cost_spike",
            Kind::LatencySpike => "latency_spike",
        }
    }

    /// Plain-English name for the health page.
    pub fn label(self) -> &'static str {
        match self {
            Kind::LeakBlocked => "Possible leak blocked",
            Kind::InjectionDetected => "Instructions hidden in mail",
            Kind::Refusal => "AI declined a request",
            Kind::SchemaFailure => "Unusable AI answer",
            Kind::UnsupportedClaim => "Claim not backed by facts",
            Kind::SendBlocked => "Send blocked",
            Kind::CostSpike => "Spend spike",
            Kind::LatencySpike => "Slow answers",
        }
    }

    /// High: a person-visible harm was only just avoided, so the agent loses a level.
    pub fn severity(self) -> &'static str {
        match self {
            Kind::LeakBlocked | Kind::SendBlocked => "high",
            Kind::InjectionDetected | Kind::CostSpike | Kind::LatencySpike => "medium",
            Kind::Refusal | Kind::SchemaFailure | Kind::UnsupportedClaim => "low",
        }
    }
}

pub struct NewIncident<'a> {
    pub workspace_id: &'a str,
    pub agent_id: Option<&'a str>,
    pub case_id: Option<&'a str>,
    pub message_id: Option<&'a str>,
    pub kind: Kind,
    pub details: Value,
    /// Same key twice in a workspace records once.
    pub dedupe_key: Option<String>,
}

impl<'a> NewIncident<'a> {
    pub fn new(workspace_id: &'a str, kind: Kind, details: Value) -> Self {
        Self { workspace_id, agent_id: None, case_id: None, message_id: None, kind, details, dedupe_key: None }
    }

    pub fn agent(mut self, agent_id: Option<&'a str>) -> Self {
        self.agent_id = agent_id;
        self
    }

    pub fn case(mut self, case_id: Option<&'a str>) -> Self {
        self.case_id = case_id;
        self
    }

    pub fn message(mut self, message_id: Option<&'a str>) -> Self {
        self.message_id = message_id;
        self
    }

    pub fn dedupe(mut self, key: impl Into<String>) -> Self {
        self.dedupe_key = Some(key.into());
        self
    }
}

/// One burst of incidents (a leak that also blocks the send) costs one level, not two.
const DEMOTION_COOLDOWN_MS: i64 = 10 * 60_000;

const BODY_KEYS: [&str; 8] = ["body", "body_text", "text", "html", "content", "subject", "draft", "snippet"];

/// Keeps details small and free of mail content: body-like keys are dropped, long strings cut.
pub fn scrub(details: Value) -> Value {
    match details {
        Value::Object(map) => {
            let kept: Map<String, Value> =
                map.into_iter().filter(|(k, _)| !BODY_KEYS.contains(&k.as_str())).map(|(k, v)| (k, scrub(v))).collect();
            Value::Object(kept)
        }
        Value::String(s) => Value::String(truncate_chars(&s, 200)),
        Value::Array(items) => Value::Array(items.into_iter().take(20).map(scrub).collect()),
        other => other,
    }
}

/// Records an incident and applies the demotion rule. Never fails the caller: problems are logged.
/// Do not call while holding a transaction; this takes its own short ones.
pub async fn record(db: &SqlitePool, i: NewIncident<'_>) -> Option<String> {
    match try_record(db, &i).await {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!(error = %e, kind = i.kind.as_str(), "failed to record AI incident");
            None
        }
    }
}

async fn try_record(db: &SqlitePool, i: &NewIncident<'_>) -> sqlx::Result<Option<String>> {
    let id = new_id("inc");
    let created = sqlx::query(
        "INSERT INTO ai_incidents (id, workspace_id, agent_id, case_id, message_id, kind, severity, details_json, dedupe_key, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(workspace_id, dedupe_key) WHERE dedupe_key IS NOT NULL DO NOTHING",
    )
    .bind(&id)
    .bind(i.workspace_id)
    .bind(i.agent_id)
    .bind(i.case_id)
    .bind(i.message_id)
    .bind(i.kind.as_str())
    .bind(i.kind.severity())
    .bind(scrub(i.details.clone()).to_string())
    .bind(&i.dedupe_key)
    .bind(now_ms())
    .execute(db)
    .await?
    .rows_affected()
        == 1;
    if !created {
        return Ok(None);
    }
    if let (Some(agent_id), "high") = (i.agent_id, i.kind.severity()) {
        demote_agent(db, i.workspace_id, agent_id, i.kind, &id).await?;
    }
    Ok(Some(id))
}

async fn demote_agent(db: &SqlitePool, ws: &str, agent_id: &str, kind: Kind, incident_id: &str) -> sqlx::Result<()> {
    let recently: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM autonomy_changes WHERE workspace_id = ?1 AND agent_id = ?2 AND to_level < from_level AND created_at > ?3)",
    )
    .bind(ws)
    .bind(agent_id)
    .bind(now_ms() - DEMOTION_COOLDOWN_MS)
    .fetch_one(db)
    .await?;
    if recently {
        return Ok(());
    }
    let mut tx = db.begin().await?;
    if let Some(agent) = store::get_in(&mut *tx, ws, agent_id).await?
        && agent.autonomy_level >= 1
    {
        let reason = format!("AI incident: {}", kind.label());
        gate::demote(&mut tx, ws, agent_id, agent.autonomy_level - 1, &reason, json!({"incident_id": incident_id, "kind": kind.as_str()}), "system")
            .await?;
    }
    tx.commit().await
}

/// High-severity, unresolved-or-not incidents on one agent after `since` (used by the hourly demotion sweep).
pub async fn high_since(db: &SqlitePool, ws: &str, agent_id: &str, since: i64) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT COUNT(*) FROM ai_incidents WHERE workspace_id = ?1 AND agent_id = ?2 AND severity = 'high' AND created_at > ?3")
        .bind(ws)
        .bind(agent_id)
        .bind(since)
        .fetch_one(db)
        .await
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct IncidentRow {
    pub id: String,
    pub kind: String,
    pub severity: String,
    pub agent_id: Option<String>,
    pub case_id: Option<String>,
    pub message_id: Option<String>,
    pub details_json: String,
    pub created_at: i64,
    pub resolved_at: Option<i64>,
}

pub async fn list(db: &SqlitePool, ws: &str, open_only: bool, limit: i64) -> sqlx::Result<Vec<IncidentRow>> {
    sqlx::query_as(
        "SELECT id, kind, severity, agent_id, case_id, message_id, details_json, created_at, resolved_at
         FROM ai_incidents WHERE workspace_id = ?1 AND (?2 = 0 OR resolved_at IS NULL)
         ORDER BY created_at DESC, id DESC LIMIT ?3",
    )
    .bind(ws)
    .bind(open_only)
    .bind(limit)
    .fetch_all(db)
    .await
}

/// Marks an incident handled. False when it does not exist in this workspace or was already resolved.
pub async fn resolve(db: &SqlitePool, ws: &str, id: &str, user_id: &str) -> sqlx::Result<bool> {
    Ok(sqlx::query("UPDATE ai_incidents SET resolved_at = ?3, resolved_by = ?4 WHERE id = ?1 AND workspace_id = ?2 AND resolved_at IS NULL")
        .bind(id)
        .bind(ws)
        .bind(now_ms())
        .bind(user_id)
        .execute(db)
        .await?
        .rows_affected()
        == 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrub_drops_mail_content_and_cuts_long_text() {
        let v = scrub(json!({"body_text": "secret", "why": "x".repeat(500), "n": 3, "nested": {"subject": "s", "ok": "fine"}}));
        assert!(v.get("body_text").is_none());
        assert_eq!(v["why"].as_str().unwrap().chars().count(), 200);
        assert_eq!(v["n"], 3);
        assert!(v["nested"].get("subject").is_none());
        assert_eq!(v["nested"]["ok"], "fine");
    }

    #[test]
    fn every_kind_has_a_distinct_name_and_a_severity() {
        let names: std::collections::HashSet<_> = ALL_KINDS.iter().map(|k| k.as_str()).collect();
        assert_eq!(names.len(), ALL_KINDS.len());
        assert_eq!(Kind::LeakBlocked.severity(), "high");
        assert_eq!(Kind::SchemaFailure.severity(), "low");
    }
}
