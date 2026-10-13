//! Durable job queue on SQLite. Delivery is at-least-once: every handler must be idempotent.

pub mod automation_jobs;
pub mod handlers;
pub mod knowledge_jobs;
pub mod matching_jobs;
pub mod worker;

use rand::Rng;
use serde_json::Value;
use sqlx::{FromRow, SqliteConnection, SqlitePool};

use crate::db::{new_id, now_ms};

pub const LEASE_MS: i64 = 120_000;

pub mod kind {
    pub const FETCH_FORWARDED_EMAIL: &str = "fetch_forwarded_email";
    pub const CLASSIFY_EMAIL: &str = "classify_email";
    pub const EXECUTE_WORKFLOW: &str = "execute_workflow";
    pub const GENERATE_DRAFT: &str = "generate_draft";
    pub const PURGE_EXPIRED_EVENTS: &str = "purge_expired_events";
    pub const SYNC_NANGO_SOURCE: &str = "sync_nango_source";
    pub const REGISTER_NANGO_CONNECTION: &str = "register_nango_connection";
    pub const LEARN_FROM_DRAFT: &str = "learn_from_draft";
    pub const EXTRACT_RECORD: &str = "extract_record";
    pub const MATCH_RECORD: &str = "match_record";
    pub const RUN_AUTOMATIONS: &str = "run_automations";
    pub const REMIND_NO_REPLY: &str = "remind_no_reply";
    pub const ANALYZE_MESSAGE: &str = "analyze_message";
    pub const INDEX_MESSAGE: &str = "index_message";
    pub const INDEX_REPLY: &str = "index_reply";
    pub const BACKFILL_KNOWLEDGE: &str = "backfill_knowledge";
}

#[derive(Debug, Clone, FromRow)]
pub struct Job {
    pub id: String,
    pub workspace_id: Option<String>,
    pub kind: String,
    pub dedupe_key: String,
    pub payload_json: String,
    pub status: String,
    pub attempts: i64,
    pub max_attempts: i64,
    pub run_at: i64,
    pub last_error: Option<String>,
    pub updated_at: i64,
}

impl Job {
    pub fn payload(&self) -> Value {
        serde_json::from_str(&self.payload_json).unwrap_or(Value::Null)
    }
}

const JOB_COLUMNS: &str = "id, workspace_id, kind, dedupe_key, payload_json, status, attempts, max_attempts, run_at, last_error, updated_at";

/// Inserts a job unless one with the same dedupe key exists. A `dead` job with the same
/// key is revived, so an explicit re-request can recover a dead-lettered stage.
/// Returns true if a job was created or revived.
pub async fn enqueue(
    conn: &mut SqliteConnection,
    kind: &str,
    dedupe_key: &str,
    workspace_id: Option<&str>,
    payload: &Value,
) -> sqlx::Result<bool> {
    enqueue_with_attempts(conn, kind, dedupe_key, workspace_id, payload, DEFAULT_MAX_ATTEMPTS).await
}

pub const DEFAULT_MAX_ATTEMPTS: i64 = 5;
/// Provider fetches ride out longer outages (~30 min with the 5 min backoff cap).
pub const PROVIDER_MAX_ATTEMPTS: i64 = 12;

pub async fn enqueue_with_attempts(
    conn: &mut SqliteConnection,
    kind: &str,
    dedupe_key: &str,
    workspace_id: Option<&str>,
    payload: &Value,
    max_attempts: i64,
) -> sqlx::Result<bool> {
    enqueue_at(conn, kind, dedupe_key, workspace_id, payload, max_attempts, now_ms()).await
}

/// Like `enqueue_with_attempts`, but the job is not claimable before `run_at` (unix ms).
pub async fn enqueue_at(
    conn: &mut SqliteConnection,
    kind: &str,
    dedupe_key: &str,
    workspace_id: Option<&str>,
    payload: &Value,
    max_attempts: i64,
    run_at: i64,
) -> sqlx::Result<bool> {
    let now = now_ms();
    let res = sqlx::query(
        "INSERT INTO jobs (id, workspace_id, kind, dedupe_key, payload_json, status, max_attempts, run_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?7, ?8, ?6, ?6)
         ON CONFLICT(dedupe_key) DO UPDATE SET
           status = 'pending', attempts = 0, run_at = excluded.run_at, last_error = NULL,
           locked_by = NULL, locked_until = NULL, updated_at = excluded.updated_at
         WHERE jobs.status = 'dead'",
    )
    .bind(new_id("job"))
    .bind(workspace_id)
    .bind(kind)
    .bind(dedupe_key)
    .bind(payload.to_string())
    .bind(now)
    .bind(max_attempts)
    .bind(run_at)
    .execute(conn)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Atomically claims one due job (or one whose lease expired) in a single statement,
/// so two workers can never both own the same live job.
pub async fn claim(pool: &SqlitePool, worker_id: &str) -> sqlx::Result<Option<Job>> {
    let now = now_ms();
    sqlx::query_as::<_, Job>(&format!(
        "UPDATE jobs
         SET status = 'running', locked_by = ?1, locked_until = ?2, attempts = attempts + 1, updated_at = ?3
         WHERE id = (
           SELECT id FROM jobs
           WHERE attempts < max_attempts AND (
             (status = 'pending' AND run_at <= ?3)
             OR (status = 'running' AND locked_until <= ?3)
           )
           ORDER BY run_at, created_at
           LIMIT 1
         )
         RETURNING {JOB_COLUMNS}"
    ))
    .bind(worker_id)
    .bind(now + LEASE_MS)
    .bind(now)
    .fetch_optional(pool)
    .await
}

/// `(locked_by, attempts)` is the claim token: it fences out a stale task whose lease
/// expired and was re-claimed, even by the same worker process.
pub async fn complete(pool: &SqlitePool, job: &Job, worker_id: &str) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE jobs SET status = 'succeeded', locked_by = NULL, locked_until = NULL, last_error = NULL, updated_at = ?3
         WHERE id = ?1 AND locked_by = ?2 AND attempts = ?4 AND status = 'running'",
    )
    .bind(&job.id)
    .bind(worker_id)
    .bind(now_ms())
    .bind(job.attempts)
    .execute(pool)
    .await?;
    Ok(())
}

/// Exponential backoff with jitter: ~2s, 4s, 8s ... capped at 5 minutes.
pub fn backoff_ms(attempt: i64) -> i64 {
    let base = 2_000i64.saturating_mul(1i64 << attempt.clamp(1, 16).saturating_sub(1)).min(300_000);
    base + rand::thread_rng().gen_range(0..1_000)
}

/// Returns true if the job is now dead.
pub async fn fail(pool: &SqlitePool, job: &Job, worker_id: &str, error: &str, permanent: bool) -> sqlx::Result<bool> {
    let now = now_ms();
    let dead = permanent || job.attempts >= job.max_attempts;
    let error = crate::sources::normalize::truncate_chars(error, 500);
    let res = sqlx::query(
        "UPDATE jobs SET status = ?1, run_at = ?2, last_error = ?3, locked_by = NULL, locked_until = NULL, updated_at = ?4
         WHERE id = ?5 AND locked_by = ?6 AND attempts = ?7 AND status = 'running'",
    )
    .bind(if dead { "dead" } else { "pending" })
    .bind(if dead { now } else { now + backoff_ms(job.attempts) })
    .bind(&error)
    .bind(now)
    .bind(&job.id)
    .bind(worker_id)
    .bind(job.attempts)
    .execute(pool)
    .await?;
    Ok(dead && res.rows_affected() > 0)
}

/// Expired leases with no attempts left would never be claimed again; dead-letter them.
pub async fn sweep_exhausted(pool: &SqlitePool) -> sqlx::Result<Vec<Job>> {
    let now = now_ms();
    sqlx::query_as::<_, Job>(&format!(
        "UPDATE jobs SET status = 'dead', locked_by = NULL, locked_until = NULL,
           last_error = COALESCE(last_error, 'lease expired'), updated_at = ?1
         WHERE status = 'running' AND locked_until <= ?1 AND attempts >= max_attempts
         RETURNING {JOB_COLUMNS}"
    ))
    .bind(now)
    .fetch_all(pool)
    .await
}

pub async fn list(pool: &SqlitePool, workspace_id: &str, status: &str) -> sqlx::Result<Vec<Job>> {
    sqlx::query_as::<_, Job>(&format!(
        "SELECT {JOB_COLUMNS} FROM jobs WHERE workspace_id = ?1 AND status = ?2 ORDER BY updated_at DESC LIMIT 200"
    ))
    .bind(workspace_id)
    .bind(status)
    .fetch_all(pool)
    .await
}

pub async fn retry_dead(conn: &mut SqliteConnection, workspace_id: &str, job_id: &str) -> sqlx::Result<Option<Job>> {
    let now = now_ms();
    sqlx::query_as::<_, Job>(&format!(
        "UPDATE jobs SET status = 'pending', attempts = 0, run_at = ?1, last_error = NULL, updated_at = ?1
         WHERE id = ?2 AND workspace_id = ?3 AND status = 'dead'
         RETURNING {JOB_COLUMNS}"
    ))
    .bind(now)
    .bind(job_id)
    .bind(workspace_id)
    .fetch_optional(conn)
    .await
}

#[cfg(test)]
mod tests {
    use super::backoff_ms;

    #[test]
    fn backoff_grows_and_caps() {
        assert!((2_000..3_000).contains(&backoff_ms(1)));
        assert!((8_000..9_000).contains(&backoff_ms(3)));
        assert!((300_000..301_000).contains(&backoff_ms(30)));
    }
}
