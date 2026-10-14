//! Append-only feedback events, written in the same transaction as the action they describe.

use serde::Serialize;
use serde_json::{Value, json};
use sqlx::{SqliteConnection, SqlitePool};

use super::scope;
use crate::db::{iso, new_id, now_ms};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    DraftApproved,
    DraftEdited,
    DraftDiscarded,
    ApprovalApproved,
    ApprovalEdited,
    ApprovalDeclined,
    LabelConfirmed,
    LabelCorrected,
    RouteCorrected,
    AutoSendStopped,
    AutoSendReported,
    CaseCancelled,
    PreferenceAccepted,
    PreferenceRejected,
    MemoryConfirmed,
    MemoryArchived,
    ShadowScored,
    SafetyBlocked,
}

impl Kind {
    pub const ALL: [Kind; 18] = [
        Kind::DraftApproved,
        Kind::DraftEdited,
        Kind::DraftDiscarded,
        Kind::ApprovalApproved,
        Kind::ApprovalEdited,
        Kind::ApprovalDeclined,
        Kind::LabelConfirmed,
        Kind::LabelCorrected,
        Kind::RouteCorrected,
        Kind::AutoSendStopped,
        Kind::AutoSendReported,
        Kind::CaseCancelled,
        Kind::PreferenceAccepted,
        Kind::PreferenceRejected,
        Kind::MemoryConfirmed,
        Kind::MemoryArchived,
        Kind::ShadowScored,
        Kind::SafetyBlocked,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::DraftApproved => "draft_approved",
            Kind::DraftEdited => "draft_edited",
            Kind::DraftDiscarded => "draft_discarded",
            Kind::ApprovalApproved => "approval_approved",
            Kind::ApprovalEdited => "approval_edited",
            Kind::ApprovalDeclined => "approval_declined",
            Kind::LabelConfirmed => "label_confirmed",
            Kind::LabelCorrected => "label_corrected",
            Kind::RouteCorrected => "route_corrected",
            Kind::AutoSendStopped => "auto_send_stopped",
            Kind::AutoSendReported => "auto_send_reported",
            Kind::CaseCancelled => "case_cancelled",
            Kind::PreferenceAccepted => "preference_accepted",
            Kind::PreferenceRejected => "preference_rejected",
            Kind::MemoryConfirmed => "memory_confirmed",
            Kind::MemoryArchived => "memory_archived",
            Kind::ShadowScored => "shadow_scored",
            Kind::SafetyBlocked => "safety_blocked",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// What happened. Only ids, labels and numbers: never bodies.
pub struct Signal<'a> {
    pub workspace_id: &'a str,
    pub kind: Kind,
    /// Defaults to the agent the message was routed to.
    pub agent_id: Option<&'a str>,
    pub actor_user_id: Option<&'a str>,
    pub message_id: Option<&'a str>,
    pub draft_id: Option<&'a str>,
    pub case_id: Option<&'a str>,
    pub approval_id: Option<&'a str>,
    pub edit_distance: Option<f64>,
    pub before: Value,
    pub after: Value,
    pub note: Option<&'a str>,
}

impl<'a> Signal<'a> {
    pub fn new(workspace_id: &'a str, kind: Kind) -> Self {
        Self {
            workspace_id,
            kind,
            agent_id: None,
            actor_user_id: None,
            message_id: None,
            draft_id: None,
            case_id: None,
            approval_id: None,
            edit_distance: None,
            before: json!({}),
            after: json!({}),
            note: None,
        }
    }

    pub fn agent(mut self, id: Option<&'a str>) -> Self {
        self.agent_id = id;
        self
    }
    pub fn actor(mut self, id: &'a str) -> Self {
        self.actor_user_id = Some(id);
        self
    }
    pub fn message(mut self, id: Option<&'a str>) -> Self {
        self.message_id = id;
        self
    }
    pub fn draft(mut self, id: Option<&'a str>) -> Self {
        self.draft_id = id;
        self
    }
    pub fn case(mut self, id: &'a str) -> Self {
        self.case_id = Some(id);
        self
    }
    pub fn approval(mut self, id: &'a str) -> Self {
        self.approval_id = Some(id);
        self
    }
    pub fn distance(mut self, d: Option<f64>) -> Self {
        self.edit_distance = d;
        self
    }
    pub fn change(mut self, before: Value, after: Value) -> Self {
        self.before = before;
        self.after = after;
        self
    }
    pub fn note(mut self, note: Option<&'a str>) -> Self {
        self.note = note;
        self
    }
}

const MAX_NOTE_CHARS: usize = 300;

/// Writes one signal on `conn` (use the transaction of the user's action). Returns its id.
pub async fn record(conn: &mut SqliteConnection, s: Signal<'_>) -> sqlx::Result<String> {
    let scope = scope::resolve(conn, s.workspace_id, s.agent_id, s.message_id).await?;
    let version: Option<i64> = match &scope.agent_id {
        Some(id) => sqlx::query_scalar("SELECT active_version FROM agents WHERE id = ?1").bind(id).fetch_optional(&mut *conn).await?.flatten(),
        None => None,
    };
    let id = new_id("fbk");
    let note = s.note.map(|n| crate::sources::normalize::truncate_chars(n.trim(), MAX_NOTE_CHARS)).filter(|n| !n.is_empty());
    sqlx::query(
        "INSERT INTO feedback_events (id, workspace_id, agent_id, agent_version, actor_user_id, owner_user_id, kind, message_id, draft_id,
                                      case_id, approval_id, edit_distance, before_json, after_json, note, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
    )
    .bind(&id)
    .bind(s.workspace_id)
    .bind(&scope.agent_id)
    .bind(version)
    .bind(s.actor_user_id)
    .bind(&scope.owner_user_id)
    .bind(s.kind.as_str())
    .bind(s.message_id)
    .bind(s.draft_id)
    .bind(s.case_id)
    .bind(s.approval_id)
    .bind(s.edit_distance)
    .bind(s.before.to_string())
    .bind(s.after.to_string())
    .bind(note)
    .bind(now_ms())
    .execute(&mut *conn)
    .await?;
    Ok(id)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SignalRow {
    pub id: String,
    pub kind: String,
    pub agent_id: Option<String>,
    pub owner_user_id: Option<String>,
    pub message_id: Option<String>,
    pub edit_distance: Option<f64>,
    pub note: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SignalView {
    pub id: String,
    pub kind: String,
    pub agent_id: Option<String>,
    pub message_id: Option<String>,
    pub edit_distance: Option<f64>,
    pub note: Option<String>,
    pub created_at: String,
}

impl From<SignalRow> for SignalView {
    fn from(r: SignalRow) -> Self {
        Self { id: r.id, kind: r.kind, agent_id: r.agent_id, message_id: r.message_id, edit_distance: r.edit_distance, note: r.note, created_at: iso(r.created_at) }
    }
}

pub struct Filter<'a> {
    /// Hides signals of other members' personal agents.
    pub viewer: &'a str,
    pub agent_id: Option<&'a str>,
    pub kind: Option<&'a str>,
    /// Only signals about (or by) this person.
    pub person: Option<&'a str>,
    pub since: i64,
    pub limit: i64,
}

/// Newest first.
pub async fn recent(db: &SqlitePool, ws: &str, f: Filter<'_>) -> sqlx::Result<Vec<SignalRow>> {
    sqlx::query_as(&format!(
        "SELECT id, kind, agent_id, owner_user_id, message_id, edit_distance, note, created_at FROM feedback_events f
         WHERE workspace_id = ?1 AND (?2 IS NULL OR agent_id = ?2) AND (?3 IS NULL OR kind = ?3) AND created_at >= ?4
           AND (?6 IS NULL OR owner_user_id = ?6 OR actor_user_id = ?6) AND {}
         ORDER BY created_at DESC, id DESC LIMIT ?5",
        crate::agent_registry::store::hide_foreign_personal("f", "?7")
    ))
    .bind(ws)
    .bind(f.agent_id)
    .bind(f.kind)
    .bind(f.since)
    .bind(f.limit)
    .bind(f.person)
    .bind(f.viewer)
    .fetch_all(db)
    .await
}

pub async fn counts_by_kind(db: &SqlitePool, ws: &str, agent_id: Option<&str>, since: i64) -> sqlx::Result<Vec<(String, i64)>> {
    sqlx::query_as(
        "SELECT kind, COUNT(*) FROM feedback_events WHERE workspace_id = ?1 AND (?2 IS NULL OR agent_id = ?2) AND created_at >= ?3
         GROUP BY kind ORDER BY kind",
    )
    .bind(ws)
    .bind(agent_id)
    .bind(since)
    .fetch_all(db)
    .await
}
