//! Deleting mail from Post Room only (the provider keeps its copy), one email, a selection, a thread, or
//! everything from a disconnected mailbox.
//!
//! `delete_batch` is the single place that knows what an email leaves behind. The caller holds a
//! `BEGIN IMMEDIATE` transaction and keeps batches small; the work is set-based over the batch's ids.
//! Re-running it for ids that are already gone does nothing, which is what makes the mailbox purge
//! idempotent and resumable.

use std::{
    collections::{BTreeSet, HashSet},
    time::{Duration, Instant},
};

use serde::Serialize;
use serde_json::{Value, json};
use sqlx::{SqliteConnection, SqlitePool};

use crate::{
    agent_registry::store,
    agents::case,
    audit::{self, Actor, Entry},
    auth::{AuthUser, require_role},
    db::now_ms,
    domain::Role,
    error::{AppError, AppResult},
    http::AppState,
    jobs::{self, handlers::JobError, kind},
    memory_learning::CONFIRMED,
    team::access::{Level, resolve},
};

/// Emails deleted per transaction: short write locks, so syncs and requests keep flowing.
pub const BATCH: usize = 25;
/// Most emails one request may delete.
pub const MAX_SELECTION: usize = 200;
/// One purge job run stops after this long and queues its own continuation (a job lease is two minutes).
const RUN_BUDGET: Duration = Duration::from_secs(45);
/// Pause between batches, so syncs and requests get the write lock in between.
const BATCH_PAUSE: Duration = Duration::from_millis(50);
/// A purge that only has mid-send replies left is retried this often...
const BUSY_RETRY_MS: i64 = 30_000;
/// ...and gives up (status `failed`, mail unblocked) after this long.
const GIVE_UP_MS: i64 = 30 * 60_000;
/// Tombstones only need to outlive the sync windows that could bring the email back.
const TOMBSTONE_TTL_MS: i64 = 180 * 86_400_000;

const DELETED_SUBJECT: &str = "Deleted email";
const CASE_NOTE: &str = "The email was deleted from Post Room";

#[derive(Debug, thiserror::Error)]
pub enum DeleteError {
    #[error("a reply to this email is being sent right now; try again in a moment")]
    Busy,
    #[error("database: {0}")]
    Db(#[from] sqlx::Error),
}

impl From<DeleteError> for AppError {
    fn from(e: DeleteError) -> Self {
        match e {
            DeleteError::Busy => AppError::Conflict(e.to_string()),
            DeleteError::Db(e) => e.into(),
        }
    }
}

impl From<DeleteError> for JobError {
    fn from(e: DeleteError) -> Self {
        JobError::Retry(e.to_string())
    }
}

/// What a deletion removed (ids and counts only).
#[derive(Debug, Default, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct Removed {
    pub emails: i64,
    pub attachments: i64,
    pub drafts: i64,
    pub summaries: i64,
    pub chunks: i64,
    pub cases_cancelled: i64,
}

impl Removed {
    fn add(&mut self, o: Removed) {
        self.emails += o.emails;
        self.attachments += o.attachments;
        self.drafts += o.drafts;
        self.summaries += o.summaries;
        self.chunks += o.chunks;
        self.cases_cancelled += o.cases_cancelled;
    }
}

#[derive(Clone, Copy)]
pub struct Opts<'a> {
    pub actor: Actor<'a>,
    /// Remember the provider message id so the next sync does not import it again.
    pub tombstone: bool,
    /// One `message.deleted` audit row per email (the mailbox purge writes a single `source.purged` row instead).
    pub audit: bool,
}

fn ids_json(ids: &[String]) -> String {
    json!(ids).to_string()
}

fn actor_id(actor: Actor<'_>) -> Option<&str> {
    match actor {
        Actor::User(id) => Some(id),
        Actor::System => None,
    }
}

#[derive(sqlx::FromRow)]
struct Doomed {
    id: String,
    source_id: String,
    thread_id: String,
    sender_address: String,
}

async fn scalar(sql: &str, arg: &str, conn: &mut SqliteConnection) -> sqlx::Result<i64> {
    sqlx::query_scalar(sql).bind(arg).fetch_one(conn).await
}

async fn column(conn: &mut SqliteConnection, sql: &str, ws: &str, ids: &str) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar(sql).bind(ws).bind(ids).fetch_all(conn).await
}

/// Deletes the emails (of this workspace) and everything derived from them. Ids that no longer exist are skipped.
pub async fn delete_batch(conn: &mut SqliteConnection, ws: &str, ids: &[String], opts: Opts<'_>, prep: &Prepared) -> Result<Removed, DeleteError> {
    let ids_arg = ids_json(ids);
    let doomed: Vec<Doomed> = sqlx::query_as(
        "SELECT id, source_id, thread_id, sender_address FROM email_messages
         WHERE workspace_id = ?1 AND id IN (SELECT value FROM json_each(?2))",
    )
    .bind(ws)
    .bind(&ids_arg)
    .fetch_all(&mut *conn)
    .await?;
    if doomed.is_empty() {
        return Ok(Removed::default());
    }
    let ids: Vec<String> = doomed.iter().map(|d| d.id.clone()).collect();
    let ids_arg = ids_json(&ids);

    let busy: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM drafts WHERE message_id IN (SELECT value FROM json_each(?1)) AND status != 'sent' AND send_attempt_id IS NOT NULL",
    )
    .bind(&ids_arg)
    .fetch_one(&mut *conn)
    .await?;
    if busy > 0 {
        return Err(DeleteError::Busy);
    }

    let now = now_ms();
    let mut removed = Removed { emails: ids.len() as i64, ..Removed::default() };
    removed.attachments = scalar("SELECT COUNT(*) FROM attachments WHERE message_id IN (SELECT value FROM json_each(?1))", &ids_arg, conn).await?;
    removed.drafts = scalar("SELECT COUNT(*) FROM drafts WHERE message_id IN (SELECT value FROM json_each(?1))", &ids_arg, conn).await?;
    removed.summaries =
        scalar("SELECT COUNT(*) FROM message_insights WHERE message_id IN (SELECT value FROM json_each(?1))", &ids_arg, conn).await?;

    let draft_ids = column(
        conn,
        "SELECT id FROM drafts WHERE workspace_id = ?1 AND message_id IN (SELECT value FROM json_each(?2))",
        ws,
        &ids_arg,
    )
    .await?;
    let drafts_arg = ids_json(&draft_ids);

    if opts.tombstone {
        sqlx::query(
            "INSERT OR IGNORE INTO deleted_messages (workspace_id, source_id, provider_message_id, deleted_by, deleted_at)
             SELECT workspace_id, source_id, source_message_id, ?2, ?3 FROM email_messages
             WHERE workspace_id = ?1 AND id IN (SELECT value FROM json_each(?4))",
        )
        .bind(ws)
        .bind(actor_id(opts.actor))
        .bind(now)
        .bind(&ids_arg)
        .execute(&mut *conn)
        .await?;
    }

    removed.cases_cancelled = detach_cases(conn, ws, &ids, &ids_arg).await?;
    forget_learning(conn, ws, &ids, &prep.refs).await?;
    apply_preferences(conn, &prep.prefs).await?;
    sqlx::query(
        "DELETE FROM feedback_events WHERE workspace_id = ?1
           AND (message_id IN (SELECT value FROM json_each(?2)) OR draft_id IN (SELECT value FROM json_each(?3)))",
    )
    .bind(ws)
    .bind(&ids_arg)
    .bind(&drafts_arg)
    .execute(&mut *conn)
    .await?;

    removed.chunks = sqlx::query(
        "DELETE FROM knowledge_chunks WHERE workspace_id = ?1 AND (
           message_id IN (SELECT value FROM json_each(?2))
           OR (kind = 'email' AND ref_id IN (SELECT value FROM json_each(?2)))
           OR (kind = 'attachment' AND ref_id IN (SELECT id FROM attachments WHERE message_id IN (SELECT value FROM json_each(?2))))
           OR (kind = 'reply' AND ref_id IN (SELECT value FROM json_each(?3))))",
    )
    .bind(ws)
    .bind(&ids_arg)
    .bind(&drafts_arg)
    .execute(&mut *conn)
    .await?
    .rows_affected() as i64;

    clear_references(conn, ws, &ids_arg, prep).await?;
    apply_citations(conn, &prep.citations).await?;

    sqlx::query("DELETE FROM email_messages WHERE workspace_id = ?1 AND id IN (SELECT value FROM json_each(?2))")
        .bind(ws)
        .bind(&ids_arg)
        .execute(&mut *conn)
        .await?;

    let threads: BTreeSet<&str> = doomed.iter().map(|d| d.thread_id.as_str()).collect();
    for thread in threads {
        sqlx::query(
            "DELETE FROM email_threads WHERE id = ?1 AND workspace_id = ?2
               AND NOT EXISTS (SELECT 1 FROM email_messages WHERE thread_id = ?1)",
        )
        .bind(thread)
        .bind(ws)
        .execute(&mut *conn)
        .await?;
    }
    let senders: BTreeSet<&str> = doomed.iter().map(|d| d.sender_address.as_str()).collect();
    for sender in senders {
        refresh_contact(conn, ws, sender).await?;
    }

    if opts.audit {
        for d in &doomed {
            audit::record(
                &mut *conn,
                Entry {
                    workspace_id: ws,
                    actor: opts.actor,
                    action: "message.deleted",
                    entity_type: "message",
                    entity_id: &d.id,
                    message_id: Some(&d.id),
                    metadata: json!({ "source_id": d.source_id, "thread_id": d.thread_id }),
                },
            )
            .await?;
        }
    }
    Ok(removed)
}

/// Cases of these emails: open ones are cancelled (held sends released first), every one keeps its row with
/// the email link removed and what it read of the email blanked. Returns how many were cancelled.
async fn detach_cases(conn: &mut SqliteConnection, ws: &str, ids: &[String], ids_arg: &str) -> sqlx::Result<i64> {
    for id in ids {
        store::release_held_sends(conn, ws, None, Some(id)).await?;
    }
    let cases: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, status FROM cases WHERE workspace_id = ?1 AND message_id IN (SELECT value FROM json_each(?2))",
    )
    .bind(ws)
    .bind(ids_arg)
    .fetch_all(&mut *conn)
    .await?;
    let mut cancelled = 0;
    for (case_id, status) in &cases {
        if case::is_terminal(status) {
            continue;
        }
        if case::set_status(conn, ws, case_id, "cancelled").await? {
            cancelled += 1;
        }
        sqlx::query("UPDATE cases SET next_wake_at = NULL WHERE id = ?1").bind(case_id).execute(&mut *conn).await?;
        sqlx::query(
            "UPDATE agent_approvals SET status = 'declined', decided_at = ?2, note = ?3 WHERE case_id = ?1 AND status = 'pending'",
        )
        .bind(case_id)
        .bind(now_ms())
        .bind(CASE_NOTE)
        .execute(&mut *conn)
        .await?;
    }
    sqlx::query(
        "UPDATE case_steps SET input_json = 'null', output_json = 'null'
         WHERE case_id IN (SELECT id FROM cases WHERE workspace_id = ?1 AND message_id IN (SELECT value FROM json_each(?2)))",
    )
    .bind(ws)
    .bind(ids_arg)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "UPDATE agent_approvals SET body = '', input_json = '{}'
         WHERE action IN ('send_email', 'send_message')
           AND case_id IN (SELECT id FROM cases WHERE workspace_id = ?1 AND message_id IN (SELECT value FROM json_each(?2)))",
    )
    .bind(ws)
    .bind(ids_arg)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "UPDATE cases SET message_id = NULL, state_json = '{}', status_note = ?3, updated_at = ?4
         WHERE workspace_id = ?1 AND message_id IN (SELECT value FROM json_each(?2))",
    )
    .bind(ws)
    .bind(ids_arg)
    .bind(CASE_NOTE)
    .bind(now_ms())
    .execute(&mut *conn)
    .await?;
    Ok(cancelled)
}

fn base_ref(evidence: &str) -> &str {
    evidence.split('#').next().unwrap_or(evidence)
}

/// Memories taught only by these emails go; ones also confirmed elsewhere (or typed by a person) lose just this source.
async fn forget_learning(conn: &mut SqliteConnection, ws: &str, ids: &[String], refs: &HashSet<String>) -> sqlx::Result<()> {
    let refs_arg = json!(refs.iter().collect::<Vec<_>>()).to_string();
    let ids_arg = ids_json(ids);
    let memories: Vec<(String, String, String, i64, f64, Option<String>)> = sqlx::query_as(
        "SELECT id, sources_json, source, pinned, confidence, evidence_message_id FROM memories
         WHERE workspace_id = ?1 AND (
           evidence_message_id IN (SELECT value FROM json_each(?2))
           OR EXISTS (SELECT 1 FROM json_each(memories.sources_json) j WHERE json_extract(j.value, '$.id') IN (SELECT value FROM json_each(?3))))",
    )
    .bind(ws)
    .bind(&ids_arg)
    .bind(&refs_arg)
    .fetch_all(&mut *conn)
    .await?;
    for (id, sources_json, source, pinned, confidence, evidence) in memories {
        let sources: Vec<Value> = serde_json::from_str(&sources_json).unwrap_or_default();
        let kept: Vec<Value> = sources
            .into_iter()
            .filter(|s| !s.get("id").and_then(Value::as_str).is_some_and(|i| refs.contains(base_ref(i))))
            .collect();
        let taught_only_by_these = kept.is_empty() && source == "learned" && pinned == 0 && confidence < CONFIRMED;
        if taught_only_by_these {
            sqlx::query("DELETE FROM memories WHERE id = ?1").bind(&id).execute(&mut *conn).await?;
        } else {
            let evidence = evidence.filter(|e| !ids.contains(e));
            sqlx::query("UPDATE memories SET sources_json = ?1, evidence_message_id = ?2 WHERE id = ?3")
                .bind(Value::Array(kept).to_string())
                .bind(evidence)
                .bind(&id)
                .execute(&mut *conn)
                .await?;
        }
    }
    Ok(())
}

/// Rows that name an email without a foreign key: blank the link (or the copied content), drop queued work.
async fn clear_references(conn: &mut SqliteConnection, ws: &str, ids_arg: &str, prep: &Prepared) -> sqlx::Result<()> {
    for sql in [
        "UPDATE ai_calls SET message_id = NULL WHERE workspace_id = ?1 AND message_id IN (SELECT value FROM json_each(?2))",
        "UPDATE ai_incidents SET message_id = NULL, details_json = '{}' WHERE workspace_id = ?1 AND message_id IN (SELECT value FROM json_each(?2))",
        "UPDATE watches SET answer_message_id = NULL, answer_text = NULL WHERE workspace_id = ?1 AND answer_message_id IN (SELECT value FROM json_each(?2))",
        "UPDATE support_tickets SET source_message_id = NULL WHERE workspace_id = ?1 AND source_message_id IN (SELECT value FROM json_each(?2))",
        "UPDATE submissions SET outcome_message_id = NULL WHERE workspace_id = ?1 AND outcome_message_id IN (SELECT value FROM json_each(?2))",
        "UPDATE inbound_events SET payload_json = '{}'
         WHERE id IN (SELECT json_extract(metadata_json, '$.event_id') FROM audit_events
                      WHERE workspace_id = ?1 AND action = 'message.received' AND message_id IN (SELECT value FROM json_each(?2)))",
    ] {
        sqlx::query(sql).bind(ws).bind(ids_arg).execute(&mut *conn).await?;
    }
    sqlx::query("DELETE FROM jobs WHERE status != 'running' AND id IN (SELECT value FROM json_each(?1))")
        .bind(ids_json(&prep.jobs))
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM assistant_plans WHERE used_at IS NULL AND id IN (SELECT value FROM json_each(?1))")
        .bind(ids_json(&prep.plans))
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Everything the deletion needs to read from tables that can be big (jobs, assistant history, preferences).
/// Computed before the write lock is taken and applied by id inside it; a row that changed in between is skipped.
#[derive(Default)]
pub struct Prepared {
    /// What taught the agents something: the emails, their drafts, the approvals and feedback they produced.
    refs: HashSet<String>,
    jobs: Vec<String>,
    plans: Vec<String>,
    citations: Vec<(i64, String, String)>,
    prefs: Vec<PrefChange>,
}

enum PrefChange {
    Delete { id: String, old: String },
    Keep { id: String, old: String, kept: Vec<String> },
}

pub async fn prepare(db: &SqlitePool, ws: &str, ids: &[String]) -> sqlx::Result<Prepared> {
    let ids_arg = ids_json(ids);
    let mut conn = db.acquire().await?;
    let conn = &mut *conn;
    let draft_ids = column(conn, "SELECT id FROM drafts WHERE workspace_id = ?1 AND message_id IN (SELECT value FROM json_each(?2))", ws, &ids_arg).await?;
    let drafts_arg = ids_json(&draft_ids);
    let approval_ids = column(
        conn,
        "SELECT a.id FROM agent_approvals a JOIN cases c ON c.id = a.case_id
         WHERE a.workspace_id = ?1 AND c.message_id IN (SELECT value FROM json_each(?2))",
        ws,
        &ids_arg,
    )
    .await?;
    let feedback_ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM feedback_events WHERE workspace_id = ?1
           AND (message_id IN (SELECT value FROM json_each(?2)) OR draft_id IN (SELECT value FROM json_each(?3)))",
    )
    .bind(ws)
    .bind(&ids_arg)
    .bind(&drafts_arg)
    .fetch_all(&mut *conn)
    .await?;
    let refs: HashSet<String> = ids.iter().chain(&draft_ids).chain(&approval_ids).chain(&feedback_ids).cloned().collect();

    let jobs: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM jobs WHERE workspace_id = ?1 AND status != 'running'
           AND (json_extract(payload_json, '$.message_id') IN (SELECT value FROM json_each(?2))
                OR json_extract(payload_json, '$.draft_id') IN (SELECT value FROM json_each(?3)))",
    )
    .bind(ws)
    .bind(&ids_arg)
    .bind(&drafts_arg)
    .fetch_all(&mut *conn)
    .await?;
    let plans: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM assistant_plans WHERE workspace_id = ?1 AND used_at IS NULL
           AND EXISTS (SELECT 1 FROM json_each(?2) j WHERE instr(params_json, j.value) > 0)",
    )
    .bind(ws)
    .bind(&ids_arg)
    .fetch_all(&mut *conn)
    .await?;

    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, citations_json FROM assistant_messages
         WHERE workspace_id = ?1 AND citations_json != '[]'
           AND EXISTS (SELECT 1 FROM json_each(?2) j WHERE instr(citations_json, j.value) > 0)",
    )
    .bind(ws)
    .bind(&ids_arg)
    .fetch_all(&mut *conn)
    .await?;
    let mut citations = vec![];
    for (id, raw) in rows {
        let mut list: Vec<Value> = serde_json::from_str(&raw).unwrap_or_default();
        let mut changed = false;
        for c in &mut list {
            let cited = c.get("message_id").and_then(Value::as_str).is_some_and(|m| ids.iter().any(|i| i == m));
            if cited && let Some(o) = c.as_object_mut() {
                o.insert("subject".into(), json!(DELETED_SUBJECT));
                o.insert("deleted".into(), json!(true));
                o.remove("attachment_id");
                o.remove("attachment_name");
                changed = true;
            }
        }
        if changed {
            citations.push((id, raw, Value::Array(list).to_string()));
        }
    }

    let stored: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT id, evidence_json, source, status FROM agent_preferences WHERE workspace_id = ?1 AND evidence_json != '[]'",
    )
    .bind(ws)
    .fetch_all(&mut *conn)
    .await?;
    let mut prefs = vec![];
    for (id, old, source, status) in stored {
        let evidence: Vec<String> = serde_json::from_str(&old).unwrap_or_default();
        let kept: Vec<String> = evidence.iter().filter(|e| !refs.contains(base_ref(e))).cloned().collect();
        if kept.len() == evidence.len() {
            continue;
        }
        // Only a suggestion nobody approved disappears with its only evidence; an accepted one keeps working.
        if kept.is_empty() && source != "manual" && status == "suggested" {
            prefs.push(PrefChange::Delete { id, old });
        } else {
            prefs.push(PrefChange::Keep { id, old, kept });
        }
    }
    Ok(Prepared { refs, jobs, plans, citations, prefs })
}

async fn apply_preferences(conn: &mut SqliteConnection, prefs: &[PrefChange]) -> sqlx::Result<()> {
    for p in prefs {
        match p {
            PrefChange::Delete { id, old } => {
                sqlx::query("DELETE FROM agent_preferences WHERE id = ?1 AND evidence_json = ?2 AND status = 'suggested'")
                    .bind(id)
                    .bind(old)
                    .execute(&mut *conn)
                    .await?;
            }
            PrefChange::Keep { id, old, kept } => {
                sqlx::query("UPDATE agent_preferences SET evidence_json = ?1, evidence_count = ?2 WHERE id = ?3 AND evidence_json = ?4")
                    .bind(Value::from(kept.clone()).to_string())
                    .bind(i64::try_from(kept.len()).unwrap_or(i64::MAX).max(1))
                    .bind(id)
                    .bind(old)
                    .execute(&mut *conn)
                    .await?;
            }
        }
    }
    Ok(())
}

async fn apply_citations(conn: &mut SqliteConnection, citations: &[(i64, String, String)]) -> sqlx::Result<()> {
    for (id, old, new) in citations {
        sqlx::query("UPDATE assistant_messages SET citations_json = ?1 WHERE id = ?2 AND citations_json = ?3")
            .bind(new)
            .bind(id)
            .bind(old)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// A sender's contact card follows the mail that is left: counts recomputed, the card dropped when none remains.
async fn refresh_contact(conn: &mut SqliteConnection, ws: &str, email: &str) -> sqlx::Result<()> {
    let (count, first, last): (i64, Option<i64>, Option<i64>) = sqlx::query_as(
        "SELECT COUNT(*), MIN(received_at), MAX(received_at) FROM email_messages
         WHERE workspace_id = ?1 AND sender_address = ?2 AND direction = 'inbound' AND suppressed = 0",
    )
    .bind(ws)
    .bind(email)
    .fetch_one(&mut *conn)
    .await?;
    if count == 0 {
        sqlx::query("DELETE FROM contacts WHERE workspace_id = ?1 AND email = ?2").bind(ws).bind(email).execute(&mut *conn).await?;
    } else {
        sqlx::query(
            "UPDATE contacts SET message_count = ?3, first_seen_at = ?4, last_seen_at = ?5 WHERE workspace_id = ?1 AND email = ?2",
        )
        .bind(ws)
        .bind(email)
        .bind(count)
        .bind(first)
        .bind(last)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// True when sync must not store this email: the user deleted it, or its mailbox is being purged.
pub async fn is_blocked(conn: &mut SqliteConnection, source_id: &str, provider_message_id: &str) -> sqlx::Result<bool> {
    let hit: Option<i64> = sqlx::query_scalar(
        "SELECT 1 WHERE EXISTS (SELECT 1 FROM deleted_messages WHERE source_id = ?1 AND provider_message_id = ?2)
            OR EXISTS (SELECT 1 FROM email_sources WHERE id = ?1 AND purge_status = 'running')",
    )
    .bind(source_id)
    .bind(provider_message_id)
    .fetch_optional(conn)
    .await?;
    Ok(hit.is_some())
}

// ---------------------------------------------------------------- who may delete what

/// 404 unless every email is visible to the user and deletable by them: inside their wall, and (for operators)
/// either in a shared mailbox or in their own. Admins may delete from any mailbox of the workspace.
/// Nothing is revealed about ids the user may not touch.
pub async fn authorize(db: &SqlitePool, ws: &str, user: &AuthUser, ids: &[String]) -> AppResult<()> {
    let access = resolve(db, ws, user).await?;
    let wall = crate::team::scope::wall(db, ws, &access).await?.map(|w| json!(w).to_string());
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT m.id, s.connected_by_user_id FROM email_messages m JOIN email_sources s ON s.id = m.source_id
         WHERE m.workspace_id = ?1 AND m.suppressed = 0 AND m.id IN (SELECT value FROM json_each(?2))
           AND (?3 IS NULL OR m.source_id IN (SELECT value FROM json_each(?3)))",
    )
    .bind(ws)
    .bind(ids_json(ids))
    .bind(wall)
    .fetch_all(db)
    .await?;
    let found: HashSet<&str> = rows.iter().map(|(id, _)| id.as_str()).collect();
    if ids.iter().any(|id| !found.contains(id.as_str())) {
        return Err(AppError::NotFound);
    }
    let someone_elses = |owner: &Option<String>| owner.as_deref().is_some_and(|o| o != user.id);
    if access.level == Level::Operator && rows.iter().any(|(_, owner)| someone_elses(owner)) {
        return Err(AppError::NotFound);
    }
    Ok(())
}

/// The whole conversation an email belongs to, suppressed mail included (same mailbox, same permission).
pub async fn thread_of(db: &SqlitePool, ws: &str, message_id: &str) -> AppResult<Vec<String>> {
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM email_messages WHERE workspace_id = ?1
           AND thread_id = (SELECT thread_id FROM email_messages WHERE id = ?2 AND workspace_id = ?1)
         ORDER BY received_at, id",
    )
    .bind(ws)
    .bind(message_id)
    .fetch_all(db)
    .await?;
    if ids.is_empty() { Err(AppError::NotFound) } else { Ok(ids) }
}

/// Emails of this list with a reply being sent at this moment.
pub async fn busy_ids(db: &SqlitePool, ws: &str, ids: &[String]) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar(
        "SELECT DISTINCT d.message_id FROM drafts d WHERE d.workspace_id = ?1 AND d.message_id IN (SELECT value FROM json_each(?2))
           AND d.status != 'sent' AND d.send_attempt_id IS NOT NULL",
    )
    .bind(ws)
    .bind(ids_json(ids))
    .fetch_all(db)
    .await
}

/// Replies a person scheduled for these emails; deleting the emails cancels them.
pub async fn scheduled_replies(db: &SqlitePool, ws: &str, ids: &[String]) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM drafts WHERE workspace_id = ?1 AND message_id IN (SELECT value FROM json_each(?2))
           AND status != 'sent' AND send_at IS NOT NULL AND send_requested_by IS NOT NULL",
    )
    .bind(ws)
    .bind(ids_json(ids))
    .fetch_one(db)
    .await
}

/// Deletes in short transactions of `BATCH` emails each. All-or-nothing about busy replies: if any email has a
/// reply being sent right now, nothing is deleted. Idempotent: a retry skips what is already gone.
pub async fn delete_emails(db: &SqlitePool, ws: &str, ids: &[String], opts: Opts<'_>) -> Result<Removed, DeleteError> {
    if !busy_ids(db, ws, ids).await?.is_empty() {
        return Err(DeleteError::Busy);
    }
    let mut total = Removed::default();
    for chunk in ids.chunks(BATCH) {
        let prep = prepare(db, ws, chunk).await?;
        let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
        total.add(delete_batch(&mut tx, ws, chunk, opts, &prep).await?);
        tx.commit().await?;
    }
    Ok(total)
}

/// Removes tombstones old enough that no sync window can bring the email back.
pub async fn prune_tombstones(db: &SqlitePool) -> sqlx::Result<u64> {
    Ok(sqlx::query("DELETE FROM deleted_messages WHERE deleted_at < ?1").bind(now_ms() - TOMBSTONE_TTL_MS).execute(db).await?.rows_affected())
}

// ---------------------------------------------------------------- disconnecting a mailbox

/// What disconnecting a mailbox will permanently delete from Post Room.
pub async fn source_counts(db: &SqlitePool, ws: &str, source_id: &str) -> sqlx::Result<Removed> {
    let mut conn = db.acquire().await?;
    let (emails, attachments, drafts, summaries, chunks): (i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM email_messages WHERE workspace_id = ?1 AND source_id = ?2),
                (SELECT COUNT(*) FROM attachments a JOIN email_messages m ON m.id = a.message_id WHERE m.workspace_id = ?1 AND m.source_id = ?2),
                (SELECT COUNT(*) FROM drafts d JOIN email_messages m ON m.id = d.message_id WHERE m.workspace_id = ?1 AND m.source_id = ?2),
                (SELECT COUNT(*) FROM message_insights i JOIN email_messages m ON m.id = i.message_id WHERE m.workspace_id = ?1 AND m.source_id = ?2),
                (SELECT COUNT(*) FROM knowledge_chunks c JOIN email_messages m ON m.id = c.message_id WHERE m.workspace_id = ?1 AND m.source_id = ?2)",
    )
    .bind(ws)
    .bind(source_id)
    .fetch_one(&mut *conn)
    .await?;
    Ok(Removed { emails, attachments, drafts, summaries, chunks, cases_cancelled: 0 })
}

/// `(scheduled replies a person set up, support tickets still open or waiting)` that disconnecting a mailbox ends.
pub async fn source_loose_ends(db: &SqlitePool, ws: &str, source_id: &str) -> sqlx::Result<(i64, i64)> {
    sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM drafts d JOIN email_messages m ON m.id = d.message_id
                 WHERE m.workspace_id = ?1 AND m.source_id = ?2 AND d.status != 'sent' AND d.send_at IS NOT NULL AND d.send_requested_by IS NOT NULL),
                (SELECT COUNT(*) FROM support_tickets t JOIN watches w ON w.id = t.watch_id
                 WHERE w.workspace_id = ?1 AND w.source_id = ?2 AND t.status IN ('open', 'waiting'))",
    )
    .bind(ws)
    .bind(source_id)
    .fetch_one(db)
    .await
}

/// Marks the source as being purged and queues the background job. Runs inside the caller's transaction
/// (the same one that disconnects the mailbox). Safe to call again: a purge already running keeps its
/// original totals.
pub async fn start_purge(conn: &mut SqliteConnection, ws: &str, source_id: &str, requested_by: &str) -> sqlx::Result<()> {
    let started = sqlx::query(
        "UPDATE email_sources SET purge_status = 'running', purge_done = 0,
                purge_total = (SELECT COUNT(*) FROM email_messages WHERE source_id = ?1),
                purge_counts_json = ?3, purge_started_at = ?4, purge_error = NULL
         WHERE id = ?1 AND workspace_id = ?2 AND (purge_status IS NULL OR purge_status = 'failed')",
    )
    .bind(source_id)
    .bind(ws)
    .bind(counts_in_tx(conn, ws, source_id).await?)
    .bind(now_ms())
    .execute(&mut *conn)
    .await?
    .rows_affected()
        == 1;
    if !started {
        let running: Option<String> =
            sqlx::query_scalar("SELECT purge_status FROM email_sources WHERE id = ?1 AND workspace_id = ?2").bind(source_id).bind(ws).fetch_optional(&mut *conn).await?.flatten();
        if running.as_deref() != Some("running") {
            return Ok(());
        }
    }
    jobs::enqueue_with_attempts(
        conn,
        kind::PURGE_SOURCE,
        &if started { format!("purge_source:{source_id}:start:{}", now_ms()) } else { format!("purge_source:{source_id}:{}", now_ms() / 60_000) },
        Some(ws),
        &json!({ "source_id": source_id, "requested_by": requested_by }),
        jobs::PROVIDER_MAX_ATTEMPTS,
    )
    .await?;
    Ok(())
}

async fn counts_in_tx(conn: &mut SqliteConnection, ws: &str, source_id: &str) -> sqlx::Result<String> {
    let (emails, attachments, drafts, summaries): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM email_messages WHERE workspace_id = ?1 AND source_id = ?2),
                (SELECT COUNT(*) FROM attachments a JOIN email_messages m ON m.id = a.message_id WHERE m.workspace_id = ?1 AND m.source_id = ?2),
                (SELECT COUNT(*) FROM drafts d JOIN email_messages m ON m.id = d.message_id WHERE m.workspace_id = ?1 AND m.source_id = ?2),
                (SELECT COUNT(*) FROM message_insights i JOIN email_messages m ON m.id = i.message_id WHERE m.workspace_id = ?1 AND m.source_id = ?2)",
    )
    .bind(ws)
    .bind(source_id)
    .fetch_one(conn)
    .await?;
    Ok(json!({ "emails": emails, "attachments": attachments, "drafts": drafts, "summaries": summaries }).to_string())
}

#[derive(Debug, PartialEq, Eq)]
pub enum PurgeOutcome {
    /// Everything is deleted and the source is cleaned up.
    Done,
    /// The time budget ran out with work left.
    MoreToDo,
    /// Only emails or sends that are mid-send are left; try again shortly.
    Waiting,
}

#[derive(Debug)]
pub struct PurgeRun {
    pub outcome: PurgeOutcome,
    pub batches: usize,
    /// The longest a batch held the write lock.
    pub max_lock: Duration,
    /// How long each batch held it.
    pub locks: Vec<Duration>,
}

/// Deletes the mailbox's emails batch by batch (reads first, then one short write transaction per batch),
/// then its source-scoped rows. Emails with a reply being sent right now are left for a later run.
pub async fn purge_run(state: &AppState, ws: &str, source_id: &str, requested_by: Option<&str>, budget: Duration, pause: Duration) -> Result<PurgeRun, JobError> {
    let started = Instant::now();
    let opts = Opts { actor: Actor::System, tombstone: false, audit: false };
    let mut run = PurgeRun { outcome: PurgeOutcome::Done, batches: 0, max_lock: Duration::ZERO, locks: vec![] };
    loop {
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT m.id FROM email_messages m WHERE m.source_id = ?1
               AND NOT EXISTS (SELECT 1 FROM drafts d WHERE d.message_id = m.id AND d.status != 'sent' AND d.send_attempt_id IS NOT NULL)
             ORDER BY m.id LIMIT ?2",
        )
        .bind(source_id)
        .bind(BATCH as i64)
        .fetch_all(&state.db)
        .await?;
        if ids.is_empty() {
            break;
        }
        let prep = prepare(&state.db, ws, &ids).await?;
        let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
        let locked = Instant::now();
        match delete_batch(&mut tx, ws, &ids, opts, &prep).await {
            Ok(_) => {}
            Err(DeleteError::Busy) => continue,
            Err(DeleteError::Db(e)) => return Err(e.into()),
        }
        sqlx::query(
            "UPDATE email_sources SET purge_done = MAX(0, COALESCE(purge_total, 0) - (SELECT COUNT(*) FROM email_messages WHERE source_id = ?1))
             WHERE id = ?1",
        )
        .bind(source_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        run.max_lock = run.max_lock.max(locked.elapsed());
        run.locks.push(locked.elapsed());
        run.batches += 1;
        if started.elapsed() >= budget {
            run.outcome = PurgeOutcome::MoreToDo;
            return Ok(run);
        }
        tokio::time::sleep(pause).await;
    }
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM email_messages WHERE source_id = ?1").bind(source_id).fetch_one(&state.db).await?;
    if left > 0 || !finish_purge(state, ws, source_id, requested_by).await? {
        run.outcome = PurgeOutcome::Waiting;
    }
    Ok(run)
}

/// The `purge_source` job. Resumable: a run that is cut off, runs out of budget or finds only busy sends is
/// continued by a new run. It never stays `running` for good: after `GIVE_UP_MS` it marks the purge failed.
pub async fn purge_source_job(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let source_id = payload
        .get("source_id")
        .and_then(Value::as_str)
        .ok_or_else(|| JobError::Permanent("payload missing source_id".into()))?
        .to_string();
    let requested_by = payload.get("requested_by").and_then(Value::as_str).map(str::to_string);
    let row: Option<(String, Option<String>, Option<i64>)> =
        sqlx::query_as("SELECT workspace_id, purge_status, purge_started_at FROM email_sources WHERE id = ?1").bind(&source_id).fetch_optional(&state.db).await?;
    let Some((ws, Some(status), started_at)) = row else { return Ok(()) };
    if status != "running" {
        return Ok(());
    }
    let run = purge_run(state, &ws, &source_id, requested_by.as_deref(), RUN_BUDGET, BATCH_PAUSE).await?;
    let next_at = match run.outcome {
        PurgeOutcome::Done => return Ok(()),
        PurgeOutcome::MoreToDo => now_ms(),
        PurgeOutcome::Waiting => {
            if now_ms() - started_at.unwrap_or_else(now_ms) > GIVE_UP_MS {
                purge_failed(&state.db, &source_id, "Some replies were still being sent, so the deletion could not finish. Disconnect again to retry.").await?;
                return Ok(());
            }
            now_ms() + BUSY_RETRY_MS
        }
    };
    let mut conn = state.db.acquire().await?;
    jobs::enqueue_at(
        &mut conn,
        kind::PURGE_SOURCE,
        &format!("purge_source:{source_id}:next:{next_at}"),
        Some(&ws),
        payload,
        jobs::PROVIDER_MAX_ATTEMPTS,
        next_at,
    )
    .await?;
    state.jobs_notify.notify_one();
    Ok(())
}

/// Stops a purge for good: the source is no longer blocked for mail, and the person can start it again.
pub async fn purge_failed(db: &SqlitePool, source_id: &str, message: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE email_sources SET purge_status = 'failed', purge_error = ?2 WHERE id = ?1 AND purge_status = 'running'")
        .bind(source_id)
        .bind(message)
        .execute(db)
        .await?;
    Ok(())
}

/// Removes what only existed because of the mailbox and records the result. One short transaction.
/// `false` = not yet: a send from this mailbox is in flight.
async fn finish_purge(state: &AppState, ws: &str, source_id: &str, requested_by: Option<&str>) -> sqlx::Result<bool> {
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let counts: Option<String> = sqlx::query_scalar(
        "SELECT purge_counts_json FROM email_sources WHERE id = ?1 AND workspace_id = ?2 AND purge_status = 'running'",
    )
    .bind(source_id)
    .bind(ws)
    .fetch_optional(&mut *tx)
    .await?
    .flatten();
    let Some(counts) = counts else { return Ok(true) };
    let remaining: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM email_messages WHERE source_id = ?1").bind(source_id).fetch_one(&mut *tx).await?;
    if remaining > 0 {
        return Ok(false);
    }
    let sending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM outbound_emails WHERE workspace_id = ?1 AND source_id = ?2 AND status != 'sent' AND send_attempt_id IS NOT NULL",
    )
    .bind(ws)
    .bind(source_id)
    .fetch_one(&mut *tx)
    .await?;
    if sending > 0 {
        return Ok(false);
    }
    let outbound: Vec<String> = sqlx::query_scalar("SELECT id FROM outbound_emails WHERE workspace_id = ?1 AND source_id = ?2")
        .bind(ws)
        .bind(source_id)
        .fetch_all(&mut *tx)
        .await?;
    for id in &outbound {
        sqlx::query(
            "UPDATE agent_approvals SET status = CASE WHEN status = 'pending' THEN 'declined' ELSE status END,
                    decided_at = CASE WHEN status = 'pending' THEN ?3 ELSE decided_at END,
                    note = CASE WHEN status = 'pending' THEN ?4 ELSE note END, body = '', input_json = '{}'
             WHERE workspace_id = ?1 AND instr(input_json, ?2) > 0",
        )
        .bind(ws)
        .bind(id)
        .bind(now_ms())
        .bind(CASE_NOTE)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM jobs WHERE status != 'running' AND instr(payload_json, ?1) > 0")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("DELETE FROM email_threads WHERE source_id = ?1").bind(source_id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM support_tickets WHERE watch_id IN (SELECT id FROM watches WHERE workspace_id = ?1 AND source_id = ?2)")
        .bind(ws)
        .bind(source_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM watches WHERE workspace_id = ?1 AND source_id = ?2").bind(ws).bind(source_id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM outbound_emails WHERE workspace_id = ?1 AND source_id = ?2").bind(ws).bind(source_id).execute(&mut *tx).await?;
    sqlx::query("UPDATE people SET source_id = NULL WHERE workspace_id = ?1 AND source_id = ?2").bind(ws).bind(source_id).execute(&mut *tx).await?;
    sqlx::query("UPDATE team_settings SET hr_source_id = NULL WHERE workspace_id = ?1 AND hr_source_id = ?2")
        .bind(ws)
        .bind(source_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM deleted_messages WHERE source_id = ?1").bind(source_id).execute(&mut *tx).await?;
    sqlx::query("UPDATE jobs SET status = 'dead', last_error = 'source purged' WHERE status = 'pending' AND json_extract(payload_json, '$.source_id') = ?1 AND kind != ?2")
        .bind(source_id)
        .bind(kind::PURGE_SOURCE)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE email_sources SET status = 'disconnected', purge_status = 'done', purge_done = purge_total, purged_at = ?3,
                sync_cursor = NULL, external_connection_id = NULL, provider_config_key = NULL, mailbox_address = NULL,
                inbound_alias = NULL, last_error = NULL, last_synced_at = NULL
         WHERE id = ?1 AND workspace_id = ?2",
    )
    .bind(source_id)
    .bind(ws)
    .bind(now_ms())
    .execute(&mut *tx)
    .await?;
    let counts: Value = serde_json::from_str(&counts).unwrap_or_else(|_| json!({}));
    audit::record(
        &mut tx,
        Entry {
            workspace_id: ws,
            actor: requested_by.map_or(Actor::System, Actor::User),
            action: "source.purged",
            entity_type: "email_source",
            entity_id: source_id,
            message_id: None,
            metadata: counts,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(true)
}

/// May this user disconnect (and purge) this mailbox? Admins any; a member their own personal mailbox.
/// Someone else's personal mailbox is a 404.
pub async fn authorize_source(db: &SqlitePool, ws: &str, user: &AuthUser, source_id: &str) -> AppResult<()> {
    let role = require_role(db, ws, user, Role::Operator).await?;
    let owner: Option<Option<String>> =
        sqlx::query_scalar("SELECT connected_by_user_id FROM email_sources WHERE id = ?1 AND workspace_id = ?2")
            .bind(source_id)
            .bind(ws)
            .fetch_optional(db)
            .await?;
    let owner = owner.ok_or(AppError::NotFound)?;
    if role == Role::Admin || owner.as_deref() == Some(user.id.as_str()) {
        return Ok(());
    }
    Err(if owner.is_some() { AppError::NotFound } else { AppError::Forbidden })
}
