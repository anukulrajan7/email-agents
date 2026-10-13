//! HR & recruiting metrics, plugged into `crate::team::metrics`, and the read-only functions the Ask Meera
//! assistant calls for exact numbers. Nothing here writes, authorises or uses a model: the caller decides
//! whether the person asking may see the answer (use `crate::team::access` for that).
//!
//! ```ignore
//! let me = staffing::metrics::find_people(&db, ws, "priya").await?;          // who is "Priya"?
//! let m  = staffing::metrics::person_metrics(&db, ws, &me[0].id, now).await?; // today / week / since joining
//! let t  = staffing::metrics::team_metrics(&db, ws, Some(manager_id), now).await?;
//! ```

use serde::Serialize;
use sqlx::SqlitePool;

use crate::team::{
    access,
    metrics::{self, MetricDef, PersonMetrics, TeamMetrics, Unit},
    people,
};

macro_rules! window {
    () => {
        "FROM submissions s WHERE s.workspace_id = ?1 AND s.person_id = ?2 AND s.status = 'active' AND s.sent_at >= ?3 AND s.sent_at < ?4"
    };
}

/// Submissions are the primary metric: daily and weekly targets count them. Interview rate is the quality metric.
pub const DEFS: &[MetricDef] = &[
    MetricDef { key: "submissions", label: "Submissions", unit: Unit::Count, primary: true, ratio: None, sql: concat!("SELECT CAST(COUNT(*) AS REAL) ", window!()) },
    MetricDef {
        key: "interviews",
        label: "Interviews",
        unit: Unit::Count,
        primary: false,
        ratio: None,
        sql: concat!("SELECT CAST(COALESCE(SUM(s.reached_interview), 0) AS REAL) ", window!()),
    },
    MetricDef {
        key: "selected",
        label: "Selected",
        unit: Unit::Count,
        primary: false,
        ratio: None,
        sql: concat!("SELECT CAST(COALESCE(SUM(s.outcome IN ('selected', 'joined')), 0) AS REAL) ", window!()),
    },
    MetricDef {
        key: "joined",
        label: "Joined",
        unit: Unit::Count,
        primary: false,
        ratio: None,
        sql: concat!("SELECT CAST(COALESCE(SUM(s.outcome = 'joined'), 0) AS REAL) ", window!()),
    },
    MetricDef {
        key: "rejected",
        label: "Rejected",
        unit: Unit::Count,
        primary: false,
        ratio: None,
        sql: concat!("SELECT CAST(COALESCE(SUM(s.outcome = 'rejected'), 0) AS REAL) ", window!()),
    },
    MetricDef {
        key: "interview_rate",
        label: "Interview rate",
        unit: Unit::Rate,
        primary: false,
        ratio: Some(("interviews", "submissions")),
        sql: concat!("SELECT CASE WHEN COUNT(*) = 0 THEN NULL ELSE CAST(SUM(s.reached_interview) AS REAL) / COUNT(*) END ", window!()),
    },
];

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PersonRef {
    pub id: String,
    pub name: String,
    pub role: Option<String>,
}

/// People whose name or email contains `query` (case-insensitive), best matches first. For resolving "Priya".
pub async fn find_people(db: &SqlitePool, workspace_id: &str, query: &str) -> sqlx::Result<Vec<PersonRef>> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Ok(vec![]);
    }
    let like = format!("%{}%", q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
    let rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT p.id, p.name, p.role FROM people p
         WHERE p.workspace_id = ?1 AND p.active = 1
           AND (lower(p.name) LIKE ?2 ESCAPE '\\' OR EXISTS (SELECT 1 FROM person_emails e WHERE e.person_id = p.id AND e.email LIKE ?2 ESCAPE '\\'))
         ORDER BY (lower(p.name) = ?3) DESC, lower(p.name) LIMIT 10",
    )
    .bind(workspace_id)
    .bind(like)
    .bind(&q)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(|(id, name, role)| PersonRef { id, name, role }).collect())
}

/// One person's numbers today, this week and since joining. `None` if the person is not in this workspace.
pub async fn person_metrics(db: &SqlitePool, workspace_id: &str, person_id: &str, now_ms: i64) -> sqlx::Result<Option<PersonMetrics>> {
    let Some(person) = people::get(db, workspace_id, person_id).await? else { return Ok(None) };
    let (packs, tz) = metrics::context(db, workspace_id).await?;
    Ok(Some(metrics::compute_person(db, workspace_id, &person, &packs, now_ms, tz).await?))
}

/// A team: everyone below `manager_id` in the manager chain (not the manager), or all active people when `None`.
pub async fn team_metrics(db: &SqlitePool, workspace_id: &str, manager_id: Option<&str>, now_ms: i64) -> sqlx::Result<TeamMetrics> {
    let (packs, tz) = metrics::context(db, workspace_id).await?;
    let members = match manager_id {
        Some(m) => {
            let below = access::subtree(db, workspace_id, m).await?;
            people::list(db, workspace_id, false).await?.into_iter().filter(|p| p.id != m && below.contains(&p.id)).collect()
        }
        None => people::list(db, workspace_id, false).await?,
    };
    team_of(db, workspace_id, members, manager_id, &packs, tz, now_ms).await
}

pub async fn team_of(
    db: &SqlitePool,
    workspace_id: &str,
    members: Vec<people::Person>,
    manager_id: Option<&str>,
    packs: &[&str],
    tz: i64,
    now_ms: i64,
) -> sqlx::Result<TeamMetrics> {
    let mut rows = Vec::with_capacity(members.len());
    for p in &members {
        rows.push(metrics::compute_person(db, workspace_id, p, packs, now_ms, tz).await?);
    }
    Ok(metrics::combine(&rows, manager_id.map(str::to_string), packs))
}
