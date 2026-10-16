//! The one writer of reply drafts. A message has at most one live (unsent) reply draft, and a thread has one
//! machine-written live draft: regenerating replaces the open draft in place, a newer message on the thread
//! retires the unsent machine draft on the earlier ones. Candidate-watch follow-ups (`watch:` origin) are outside
//! the rule. The database holds the line too (`uq_drafts_live`, migration 0058).
//!
//! `write` runs inside the caller's `BEGIN IMMEDIATE` transaction; `settle` (after commit) keeps the Gmail
//! Drafts folder in step.

use sqlx::{SqliteConnection, SqlitePool};

use super::*;

/// What to do when the message already has a live draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Leave it (and an already-sent reply) alone: nothing is written.
    KeepExisting,
    /// Rewrite it in place. A draft a person edited or approved is rewritten only when `allow_person_edited`
    /// (the person asked for a new draft); a scheduled or sending reply is never rewritten.
    Replace { allow_person_edited: bool },
}

/// The draft to write; the same shape for every caller.
pub struct NewDraft<'a> {
    pub workspace_id: &'a str,
    pub message_id: &'a str,
    pub origin_key: &'a str,
    pub body_text: &'a str,
    pub model_id: &'a str,
    pub memory_ids_json: String,
    pub unanswered_json: String,
    /// Audit metadata of `draft.generated` / `draft.regenerated`.
    pub audit: Value,
}

/// The unsent reply draft of a message, as it is now.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Live {
    pub id: String,
    pub body_text: String,
    pub status: String,
    pub provider_draft_id: Option<String>,
    /// A send is scheduled or under way.
    pub busy: bool,
}

impl Live {
    /// A person edited, approved or scheduled it: an agent must not touch it.
    pub fn person_is_working(&self) -> bool {
        self.busy || self.status != "suggested"
    }
}

/// An earlier machine draft removed because a newer message arrived on the thread.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Retired {
    pub draft_id: String,
    pub provider_draft_id: Option<String>,
    pub connection_id: Option<String>,
    pub provider_config_key: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Written {
    Inserted { id: String, retired: Vec<Retired> },
    /// Rewritten in place: same id and origin key, back to `suggested`. `provider_draft_id` is the Gmail draft to refresh.
    Replaced { id: String, provider_draft_id: Option<String>, retired: Vec<Retired> },
    Existing(Live),
    /// A send is scheduled or under way, so the draft was left as it is.
    Busy(Live),
    /// The email was already answered and nothing live is left; nothing was written.
    AlreadySent(String),
}

impl Written {
    /// The draft this call created or rewrote.
    pub fn changed(&self) -> Option<&str> {
        match self {
            Written::Inserted { id, .. } | Written::Replaced { id, .. } => Some(id),
            _ => None,
        }
    }

    /// The draft that now stands for the message, if any.
    pub fn id(&self) -> &str {
        match self {
            Written::Inserted { id, .. } | Written::Replaced { id, .. } | Written::AlreadySent(id) => id,
            Written::Existing(d) | Written::Busy(d) => &d.id,
        }
    }
}

const LIVE: &str = "SELECT id, body_text, status, provider_draft_id, (send_at IS NOT NULL OR send_attempt_id IS NOT NULL) AS busy
                    FROM drafts WHERE message_id = ?1 AND status != 'sent' AND (origin_key IS NULL OR substr(origin_key, 1, 6) != 'watch:')
                    ORDER BY created_at DESC, rowid DESC LIMIT 1";

/// The message's live draft.
pub async fn live(conn: &mut SqliteConnection, message_id: &str) -> sqlx::Result<Option<Live>> {
    sqlx::query_as(LIVE).bind(message_id).fetch_optional(&mut *conn).await
}

enum Step {
    Insert,
    Replace(Live),
    Done(Written),
}

async fn decide(conn: &mut SqliteConnection, message_id: &str, mode: Mode) -> sqlx::Result<Step> {
    Ok(match (live(conn, message_id).await?, mode) {
        (Some(d), Mode::KeepExisting) => Step::Done(Written::Existing(d)),
        (Some(d), _) if d.busy => Step::Done(Written::Busy(d)),
        (Some(d), Mode::Replace { allow_person_edited }) if allow_person_edited || d.status == "suggested" => Step::Replace(d),
        (Some(d), Mode::Replace { .. }) => Step::Done(Written::Existing(d)),
        (None, Mode::KeepExisting) => {
            let sent: Option<String> = sqlx::query_scalar(
                "SELECT id FROM drafts WHERE message_id = ?1 AND status = 'sent' AND (origin_key IS NULL OR substr(origin_key, 1, 6) != 'watch:')
                 ORDER BY created_at DESC, rowid DESC LIMIT 1",
            )
            .bind(message_id)
            .fetch_optional(&mut *conn)
            .await?;
            sent.map_or(Step::Insert, |id| Step::Done(Written::AlreadySent(id)))
        }
        (None, Mode::Replace { .. }) => Step::Insert,
    })
}

/// What `write` would answer without writing, when that needs no new draft: lets a caller skip the model.
/// `None` means a draft would be written.
pub async fn settled(db: &SqlitePool, message_id: &str, mode: Mode) -> sqlx::Result<Option<Written>> {
    let mut conn = db.acquire().await?;
    Ok(match decide(&mut conn, message_id, mode).await? {
        Step::Done(w) => Some(w),
        Step::Insert | Step::Replace(_) => None,
    })
}

/// Writes the reply draft of a message. Call inside `BEGIN IMMEDIATE`, then `settle` after the commit.
pub async fn write(tx: &mut SqliteConnection, mode: Mode, d: &NewDraft<'_>) -> sqlx::Result<Written> {
    let now = now_ms();
    match decide(tx, d.message_id, mode).await? {
        Step::Done(w) => Ok(w),
        Step::Replace(old) => {
            sqlx::query(
                "UPDATE drafts SET body_text = ?2, original_body_text = ?2, status = 'suggested', model_id = ?3, memory_ids_json = ?4,
                                   unanswered_json = ?5, edit_distance = NULL, updated_at = ?6
                 WHERE id = ?1",
            )
            .bind(&old.id)
            .bind(d.body_text)
            .bind(d.model_id)
            .bind(&d.memory_ids_json)
            .bind(&d.unanswered_json)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            record_audit(tx, d, "draft.regenerated", &old.id).await?;
            let retired = retire_earlier(tx, d.workspace_id, d.message_id).await?;
            Ok(Written::Replaced { id: old.id, provider_draft_id: old.provider_draft_id, retired })
        }
        Step::Insert => {
            let id = new_id("drf");
            let inserted = sqlx::query(
                "INSERT OR IGNORE INTO drafts (id, workspace_id, message_id, body_text, original_body_text, status, model_id,
                                               origin_key, memory_ids_json, unanswered_json, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4, 'suggested', ?5, ?6, ?7, ?8, ?9, ?9)",
            )
            .bind(&id)
            .bind(d.workspace_id)
            .bind(d.message_id)
            .bind(d.body_text)
            .bind(d.model_id)
            .bind(d.origin_key)
            .bind(&d.memory_ids_json)
            .bind(&d.unanswered_json)
            .bind(now)
            .execute(&mut *tx)
            .await?
            .rows_affected()
                == 1;
            if !inserted {
                // The same request already wrote its draft (a retry); that draft stands.
                let existing: Live = sqlx::query_as(
                    "SELECT id, body_text, status, provider_draft_id, (send_at IS NOT NULL OR send_attempt_id IS NOT NULL) AS busy FROM drafts WHERE origin_key = ?1",
                )
                .bind(d.origin_key)
                .fetch_one(&mut *tx)
                .await?;
                return Ok(if existing.status == "sent" { Written::AlreadySent(existing.id) } else { Written::Existing(existing) });
            }
            record_audit(tx, d, "draft.generated", &id).await?;
            let retired = retire_earlier(tx, d.workspace_id, d.message_id).await?;
            Ok(Written::Inserted { id, retired })
        }
    }
}

async fn record_audit(tx: &mut SqliteConnection, d: &NewDraft<'_>, action: &str, draft_id: &str) -> sqlx::Result<()> {
    audit::record(
        tx,
        Entry {
            workspace_id: d.workspace_id,
            actor: Actor::System,
            action,
            entity_type: "draft",
            entity_id: draft_id,
            message_id: Some(d.message_id),
            metadata: d.audit.clone(),
        },
    )
    .await
}

/// A new draft on `message_id` retires the unsent machine drafts on the messages that came before it in the thread.
async fn retire_earlier(tx: &mut SqliteConnection, ws: &str, message_id: &str) -> sqlx::Result<Vec<Retired>> {
    let Some((thread_id, received_at)): Option<(String, i64)> =
        sqlx::query_as("SELECT thread_id, received_at FROM email_messages WHERE id = ?1").bind(message_id).fetch_optional(&mut *tx).await?
    else {
        return Ok(Vec::new());
    };
    retire_thread(tx, ws, &thread_id, message_id, received_at).await
}

/// Deletes the `suggested` machine drafts on the thread's messages received before `before` (not `except`) and
/// audits `draft.retired`. A draft a person edited, approved or scheduled, one asked for by a person, and watch
/// drafts are never touched.
pub async fn retire_thread(tx: &mut SqliteConnection, ws: &str, thread_id: &str, except: &str, before: i64) -> sqlx::Result<Vec<Retired>> {
    let doomed: Vec<(Retired, String)> = sqlx::query_as::<_, (String, Option<String>, Option<String>, Option<String>, String)>(
        "SELECT d.id, d.provider_draft_id, s.external_connection_id, s.provider_config_key, d.message_id
         FROM drafts d
         JOIN email_messages m ON m.id = d.message_id
         JOIN email_sources s ON s.id = m.source_id
         WHERE d.workspace_id = ?1 AND m.thread_id = ?2 AND m.id != ?3 AND m.received_at < ?4
           AND d.status = 'suggested' AND d.send_at IS NULL AND d.send_attempt_id IS NULL
           AND d.origin_key IS NOT NULL AND substr(d.origin_key, 1, 6) != 'watch:' AND instr(d.origin_key, ':manual:') = 0",
    )
    .bind(ws)
    .bind(thread_id)
    .bind(except)
    .bind(before)
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .map(|(draft_id, provider_draft_id, connection_id, provider_config_key, message_id)| (Retired { draft_id, provider_draft_id, connection_id, provider_config_key }, message_id))
    .collect();
    for (r, message_id) in &doomed {
        sqlx::query("DELETE FROM drafts WHERE id = ?1").bind(&r.draft_id).execute(&mut *tx).await?;
        audit::record(
            tx,
            Entry {
                workspace_id: ws,
                actor: Actor::System,
                action: "draft.retired",
                entity_type: "draft",
                entity_id: &r.draft_id,
                message_id: Some(message_id),
                metadata: json!({ "reason": "a newer message arrived on the thread", "newer_message_id": except }),
            },
        )
        .await?;
    }
    // A reply Meera wrote for an expert's request must not vanish silently: its request, case and approval close with it.
    let ids: Vec<String> = doomed.iter().map(|(r, _)| r.draft_id.clone()).collect();
    crate::agents::handoff::store::supersede_drafts(tx, ws, &ids, except).await?;
    Ok(doomed.into_iter().map(|(r, _)| r).collect())
}

/// After the commit: refresh the rewritten draft's Gmail copy and delete the retired drafts' Gmail copies.
/// Gmail trouble never fails the write; the next save reconciles the Drafts folder.
pub async fn settle(state: &AppState, written: &Written) {
    let (rewritten, retired) = match written {
        Written::Inserted { retired, .. } => (None, retired),
        Written::Replaced { id, provider_draft_id, retired } => (provider_draft_id.as_ref().map(|_| id.as_str()), retired),
        _ => return,
    };
    if let Some(id) = rewritten {
        let ws: Option<String> = sqlx::query_scalar("SELECT workspace_id FROM drafts WHERE id = ?1").bind(id).fetch_optional(&state.db).await.ok().flatten();
        if let Some(ws) = ws
            && let Err(e) = save_draft(state, &ws, id, Actor::System).await
        {
            tracing::warn!(draft_id = id, error = %e, "the regenerated draft could not be refreshed in the mailbox");
        }
    }
    for r in retired {
        if let (Some(gmail_id), Some(conn), Some(pck)) = (&r.provider_draft_id, &r.connection_id, &r.provider_config_key)
            && let Err(e) = state.nango.gmail_call(Method::DELETE, conn, pck, &format!("drafts/{gmail_id}"), None).await
        {
            tracing::info!(draft_id = %r.draft_id, error = %e, "the retired draft's mailbox copy may not have been removed");
        }
    }
}
