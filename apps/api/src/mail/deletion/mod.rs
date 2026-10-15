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
    agents::registry::store,
    agents::case,
    platform::audit::{self, Actor, Entry},
    platform::auth::{AuthUser, require_role},
    platform::db::now_ms,
    platform::domain::Role,
    platform::error::{AppError, AppResult},
    http::AppState,
    jobs::{self, handlers::JobError, kind},
    learning::memory_learning::CONFIRMED,
    hr::team::access::{Level, resolve},
};

mod batch;
mod prepare;
mod emails;
mod purge;

pub use batch::delete_batch;
pub use prepare::Prepared;
pub use prepare::prepare;
pub use prepare::is_blocked;
pub use emails::authorize;
pub use emails::thread_of;
pub use emails::busy_ids;
pub use emails::scheduled_replies;
pub use emails::delete_emails;
pub use emails::prune_tombstones;
pub use purge::source_counts;
pub use purge::source_loose_ends;
pub use purge::start_purge;
pub use purge::PurgeOutcome;
pub use purge::PurgeRun;
pub use purge::purge_run;
pub use purge::purge_source_job;
pub use purge::purge_failed;
pub use purge::authorize_source;
use prepare::apply_preferences;
use prepare::apply_citations;
use prepare::refresh_contact;
use batch::ids_json;
use batch::column;
use batch::base_ref;

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

