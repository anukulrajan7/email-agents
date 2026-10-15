//! Alerts engine: deterministic rules over stored mail, escalation, and batched email delivery.
//!
//! Every 15 minutes `EVALUATE_ALERTS` runs per workspace (`evaluate_workspace`):
//! 1. Snoozed alerts whose snooze ended reopen (and are re-sent).
//! 2. Open alerts whose condition no longer holds are auto-resolved (reply sent, decision made, label changed).
//! 3. Rules raise alerts for mail received in the last 7 days and older than `unanswered_hours`:
//!    - `approval_pending`: label `approval`/`leave`, no closed review item and no reply.
//!    - `important_unanswered`: priority high OR (needs a reply and label not `other`), no reply. Mail sent by
//!      the team is never alerted on; `approval`/`leave` mail is governed only by `approval_pending`.
//! 4. Open alerts unchanged for `escalate_after_hours` go up one level (max 3) and are sent to `escalate_to`.
//! 5. Pending notifications are sent, one email per recipient per run (level 1 → recipients, 2+ → escalate_to).
//! 6. Daily digest of open alerts once per UTC day, in the run that falls in `digest_hour` (UTC).
//!
//! `raise` is public and idempotent by dedupe key (`kind:message_id:level`), so other modules (automations)
//! can raise alerts too; they are delivered by the next evaluation run.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::{
    ai::judge,
    platform::audit::{self, Actor, Entry},
    platform::db::{iso, new_id, now_ms},
    http::AppState,
    jobs::{self, handlers::JobError, kind},
    platform::mailer::{OutgoingEmail, kind as mail_kind},
    learning::memory_learning::team_addresses,
};

mod raise;
mod settings;
mod rules;
mod evaluate;
mod delivery;

pub use raise::NewAlert;
pub use raise::dedupe_key;
pub use raise::raise;
pub use settings::AlertSettings;
pub use settings::valid_email;
pub use settings::load_settings;
pub use settings::save_settings;
pub use evaluate::EvalReport;
pub use evaluate::evaluate_workspace;
pub use delivery::AlertLine;
pub use delivery::deep_link;
pub use delivery::escape_html;
pub use delivery::render_email;
use rules::firing;
use rules::raise_rule;
use rules::rule_alert;
use rules::worth_alerting;
use delivery::deliver;
use delivery::digest;

pub const IMPORTANT_UNANSWERED: &str = "important_unanswered";
pub const APPROVAL_PENDING: &str = "approval_pending";
pub const AUTOMATION: &str = "automation";
pub const MAX_LEVEL: i64 = 3;
pub const EVALUATE_EVERY_MS: i64 = 15 * 60_000;
/// Older mail never raises alerts (avoids a flood after a mailbox backfill).
pub const LOOKBACK_MS: i64 = 7 * 86_400_000;
const HOUR_MS: i64 = 3_600_000;
const MAX_CANDIDATES: i64 = 200;

/// Enqueues one evaluation per workspace per 15-minute bucket (called from the worker's maintenance tick).
pub async fn schedule(state: &AppState) -> sqlx::Result<()> {
    let bucket = now_ms() / EVALUATE_EVERY_MS;
    let workspaces: Vec<String> = sqlx::query_scalar("SELECT id FROM workspaces").fetch_all(&state.db).await?;
    let mut conn = state.db.acquire().await?;
    for ws in workspaces {
        jobs::enqueue_with_attempts(
            &mut conn,
            kind::EVALUATE_ALERTS,
            &format!("alerts:{ws}:{bucket}"),
            Some(&ws),
            &json!({ "workspace_id": ws }),
            3,
        )
        .await?;
    }
    Ok(())
}

pub async fn evaluate_job(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let ws = payload
        .get("workspace_id")
        .and_then(Value::as_str)
        .ok_or_else(|| JobError::Permanent("payload missing workspace_id".into()))?;
    let r = evaluate_workspace(state, ws, now_ms()).await?;
    if r != EvalReport::default() {
        tracing::info!(workspace_id = %ws, raised = r.raised, resolved = r.resolved, escalated = r.escalated,
            emails = r.emails_sent, digest = r.digest_sent, "alerts evaluated");
    }
    Ok(())
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct AlertRow {
    pub id: String,
    pub kind: String,
    pub severity: String,
    pub title: String,
    pub body: String,
    pub message_id: Option<String>,
    pub message_subject: Option<String>,
    pub status: String,
    pub escalation_level: i64,
    pub notified_json: String,
    pub snoozed_until: Option<i64>,
    pub created_at: i64,
    pub resolved_at: Option<i64>,
}

pub const ALERT_SELECT: &str = "SELECT a.id, a.kind, a.severity, a.title, a.body, a.message_id, m.subject AS message_subject,
    a.status, a.escalation_level, a.notified_json, a.snoozed_until, a.created_at, a.resolved_at
    FROM alerts a LEFT JOIN email_messages m ON m.id = a.message_id";

pub fn alert_json(a: AlertRow) -> Value {
    json!({
        "id": a.id, "kind": a.kind, "severity": a.severity, "title": a.title, "body": a.body,
        "message_id": a.message_id, "message_subject": a.message_subject, "status": a.status,
        "escalation_level": a.escalation_level,
        "notified": serde_json::from_str::<Vec<String>>(&a.notified_json).unwrap_or_default(),
        "snoozed_until": a.snoozed_until.map(iso),
        "created_at": iso(a.created_at), "resolved_at": a.resolved_at.map(iso),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> AlertSettings {
        AlertSettings {
            enabled: true,
            recipients: vec![" Boss@Acme.com ".into(), "boss@acme.com".into()],
            escalate_to: vec![],
            unanswered_hours: 4,
            escalate_after_hours: 24,
            daily_digest: true,
            digest_hour: 9,
        }
    }

    #[test]
    fn settings_validation() {
        assert_eq!(settings().validated().unwrap().recipients, vec!["boss@acme.com"]);
        assert!(AlertSettings { recipients: vec!["nope".into()], ..settings() }.validated().is_err());
        assert!(AlertSettings { recipients: vec!["a b@c.co".into()], ..settings() }.validated().is_err());
        assert!(AlertSettings { unanswered_hours: 0, ..settings() }.validated().is_err());
        assert!(AlertSettings { digest_hour: 24, ..settings() }.validated().is_err());
        assert!(AlertSettings { escalate_after_hours: 721, ..settings() }.validated().is_err());
    }

    #[test]
    fn renders_batched_email_with_links_and_escaping() {
        let lines = vec![
            AlertLine { title: "No reply yet: <Hours>".into(), body: "p@x.co wrote".into(), link: "https://app/w/ws/inbox/m1".into(), level: 1 },
            AlertLine { title: "Waiting for a decision: Leave".into(), body: "b".into(), link: "https://app/w/ws/inbox/m2".into(), level: 2 },
        ];
        let (subject, text, html) = render_email("Acme", "https://app", "ws", &lines, None);
        assert_eq!(subject, "[Post Room] 2 emails need attention");
        assert!(text.contains("Open: https://app/w/ws/inbox/m1") && text.contains("Escalated: Waiting"));
        assert!(html.contains("&lt;Hours&gt;") && html.contains(r#"href="https://app/w/ws/inbox/m2""#));
        assert_eq!(dedupe_key("automation", None, 1), "automation:-:1");
    }
}
pub mod messaging;
