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
    audit::{self, Actor, Entry},
    db::{iso, new_id, now_ms},
    http::AppState,
    jobs::{self, handlers::JobError, kind},
    mailer::{OutgoingEmail, kind as mail_kind},
    memory_learning::team_addresses,
};

pub const IMPORTANT_UNANSWERED: &str = "important_unanswered";
pub const APPROVAL_PENDING: &str = "approval_pending";
pub const AUTOMATION: &str = "automation";
pub const MAX_LEVEL: i64 = 3;
pub const EVALUATE_EVERY_MS: i64 = 15 * 60_000;
/// Older mail never raises alerts (avoids a flood after a mailbox backfill).
pub const LOOKBACK_MS: i64 = 7 * 86_400_000;
const HOUR_MS: i64 = 3_600_000;
const MAX_CANDIDATES: i64 = 200;

// ---------------------------------------------------------------- raise

pub struct NewAlert<'a> {
    pub workspace_id: &'a str,
    pub kind: &'a str,
    /// `high` or `normal` (anything else is stored as `normal`).
    pub severity: &'a str,
    pub title: String,
    pub body: String,
    pub message_id: Option<&'a str>,
    /// Level the alert is raised at (normally 1); part of the default dedupe key.
    pub level: i64,
    /// Overrides the default `kind:message_id:level` key.
    pub dedupe_key: Option<String>,
}

pub fn dedupe_key(kind: &str, message_id: Option<&str>, level: i64) -> String {
    format!("{kind}:{}:{level}", message_id.unwrap_or("-"))
}

/// Inserts the alert unless one with the same dedupe key exists (whatever its status).
/// Returns `(alert_id, created)`.
pub async fn raise(pool: &SqlitePool, a: NewAlert<'_>) -> sqlx::Result<(String, bool)> {
    let key = a.dedupe_key.clone().unwrap_or_else(|| dedupe_key(a.kind, a.message_id, a.level));
    let severity = if a.severity == "high" { "high" } else { "normal" };
    let level = a.level.clamp(1, MAX_LEVEL);
    let now = now_ms();
    let id = new_id("alr");
    let mut tx = pool.begin().await?;
    let created = sqlx::query(
        "INSERT INTO alerts (id, workspace_id, kind, severity, title, body, message_id, dedupe_key, escalation_level,
                             level_since, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, ?10)
         ON CONFLICT(workspace_id, dedupe_key) DO NOTHING",
    )
    .bind(&id)
    .bind(a.workspace_id)
    .bind(a.kind)
    .bind(severity)
    .bind(crate::sources::normalize::truncate_chars(&a.title, 200))
    .bind(crate::sources::normalize::truncate_chars(&a.body, 1_000))
    .bind(a.message_id)
    .bind(&key)
    .bind(level)
    .bind(now)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    let id: String = sqlx::query_scalar("SELECT id FROM alerts WHERE workspace_id = ?1 AND dedupe_key = ?2")
        .bind(a.workspace_id)
        .bind(&key)
        .fetch_one(&mut *tx)
        .await?;
    if created {
        audit::record(
            &mut tx,
            Entry {
                workspace_id: a.workspace_id,
                actor: Actor::System,
                action: "alert.raised",
                entity_type: "alert",
                entity_id: &id,
                message_id: a.message_id,
                metadata: json!({ "kind": a.kind, "severity": severity, "level": level }),
            },
        )
        .await?;
    }
    tx.commit().await?;
    Ok((id, created))
}

// ---------------------------------------------------------------- settings

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlertSettings {
    pub enabled: bool,
    pub recipients: Vec<String>,
    pub escalate_to: Vec<String>,
    pub unanswered_hours: i64,
    pub escalate_after_hours: i64,
    pub daily_digest: bool,
    pub digest_hour: i64,
}

fn valid_email(e: &str) -> bool {
    let Some((local, domain)) = e.split_once('@') else { return false };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains('@')
        && e.len() <= 254
        && !e.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | ',' | ';' | '"'))
}

fn clean_list(list: &[String], field: &str) -> Result<Vec<String>, String> {
    let mut seen = HashSet::new();
    let mut out = vec![];
    for e in list {
        let e = e.trim().to_lowercase();
        if !valid_email(&e) {
            return Err(format!("{field}: '{e}' is not a valid email address"));
        }
        if seen.insert(e.clone()) {
            out.push(e);
        }
    }
    if out.len() > 20 {
        return Err(format!("{field}: at most 20 addresses"));
    }
    Ok(out)
}

impl AlertSettings {
    /// Normalises addresses (trim, lower-case, dedupe) and checks ranges.
    pub fn validated(mut self) -> Result<Self, String> {
        self.recipients = clean_list(&self.recipients, "recipients")?;
        self.escalate_to = clean_list(&self.escalate_to, "escalate_to")?;
        if !(1..=168).contains(&self.unanswered_hours) {
            return Err("unanswered_hours must be 1-168".into());
        }
        if !(1..=720).contains(&self.escalate_after_hours) {
            return Err("escalate_after_hours must be 1-720".into());
        }
        if !(0..=23).contains(&self.digest_hour) {
            return Err("digest_hour must be 0-23 (UTC)".into());
        }
        Ok(self)
    }
}

async fn admin_emails(pool: &SqlitePool, workspace_id: &str) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar(
        "SELECT lower(u.email) FROM memberships mb JOIN users u ON u.id = mb.user_id
         WHERE mb.workspace_id = ?1 AND mb.role = 'admin' ORDER BY u.email",
    )
    .bind(workspace_id)
    .fetch_all(pool)
    .await
}

#[derive(sqlx::FromRow)]
struct SettingsRow {
    enabled: bool,
    recipients_json: Option<String>,
    escalate_to_json: String,
    unanswered_hours: i64,
    escalate_after_hours: i64,
    daily_digest: bool,
    digest_hour: i64,
}

/// Stored settings, or the defaults (enabled, admins, 4h, 24h, digest at 09 UTC).
pub async fn load_settings(pool: &SqlitePool, workspace_id: &str) -> sqlx::Result<AlertSettings> {
    let row: Option<SettingsRow> = sqlx::query_as(
        "SELECT enabled, recipients_json, escalate_to_json, unanswered_hours, escalate_after_hours, daily_digest, digest_hour
         FROM alert_settings WHERE workspace_id = ?1",
    )
    .bind(workspace_id)
    .fetch_optional(pool)
    .await?;
    let r = row.unwrap_or(SettingsRow {
        enabled: true,
        recipients_json: None,
        escalate_to_json: "[]".into(),
        unanswered_hours: 4,
        escalate_after_hours: 24,
        daily_digest: true,
        digest_hour: 9,
    });
    let recipients = match r.recipients_json.and_then(|j| serde_json::from_str::<Vec<String>>(&j).ok()) {
        Some(list) => list,
        None => admin_emails(pool, workspace_id).await?,
    };
    Ok(AlertSettings {
        enabled: r.enabled,
        recipients,
        escalate_to: serde_json::from_str(&r.escalate_to_json).unwrap_or_default(),
        unanswered_hours: r.unanswered_hours,
        escalate_after_hours: r.escalate_after_hours,
        daily_digest: r.daily_digest,
        digest_hour: r.digest_hour,
    })
}

pub async fn save_settings(pool: &SqlitePool, workspace_id: &str, s: &AlertSettings) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO alert_settings (workspace_id, enabled, recipients_json, escalate_to_json, unanswered_hours,
                                     escalate_after_hours, daily_digest, digest_hour, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(workspace_id) DO UPDATE SET
           enabled = excluded.enabled, recipients_json = excluded.recipients_json, escalate_to_json = excluded.escalate_to_json,
           unanswered_hours = excluded.unanswered_hours, escalate_after_hours = excluded.escalate_after_hours,
           daily_digest = excluded.daily_digest, digest_hour = excluded.digest_hour, updated_at = excluded.updated_at",
    )
    .bind(workspace_id)
    .bind(s.enabled)
    .bind(json!(s.recipients).to_string())
    .bind(json!(s.escalate_to).to_string())
    .bind(s.unanswered_hours)
    .bind(s.escalate_after_hours)
    .bind(s.daily_digest)
    .bind(s.digest_hour)
    .bind(now_ms())
    .execute(pool)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------- rules

/// True if someone on the team answered in this thread after `after_ms`.
///
/// Column-free heuristic: a later message in the same thread sent from a team address (a workspace member, or
/// the user who connected the mailbox, whose address is the mailbox owner's for Gmail), or a reply sent from
/// Post Room (`drafts.status = 'sent'`) for any message in the thread after `after_ms`.
/// Replied = outbound mail / sent draft in the thread (direction column), or mail from a team
/// address (covers send-as aliases Gmail didn't label SENT).
async fn thread_has_reply_after(db: &SqlitePool, workspace_id: &str, thread_key: &str, after_ms: i64) -> sqlx::Result<bool> {
    Ok(crate::threads::thread_has_reply_after(db, workspace_id, thread_key, after_ms).await?
        || team_sent_after(db, workspace_id, thread_key, after_ms).await?)
}

async fn team_sent_after(db: &SqlitePool, workspace_id: &str, thread_key: &str, after_ms: i64) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS(
           SELECT 1 FROM email_messages r JOIN email_sources s ON s.id = r.source_id
           WHERE r.workspace_id = ?1 AND r.thread_id = ?2 AND r.received_at > ?3
             AND (lower(r.sender_address) IN (SELECT lower(u.email) FROM memberships mb JOIN users u ON u.id = mb.user_id
                                              WHERE mb.workspace_id = ?1)
                  OR lower(r.sender_address) = (SELECT lower(u.email) FROM users u WHERE u.id = s.connected_by_user_id))
         ) OR EXISTS(
           SELECT 1 FROM drafts d JOIN email_messages m ON m.id = d.message_id
           WHERE d.workspace_id = ?1 AND m.thread_id = ?2 AND d.status = 'sent' AND d.updated_at > ?3
         )",
    )
    .bind(workspace_id)
    .bind(thread_key)
    .bind(after_ms)
    .fetch_one(db)
    .await
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct Candidate {
    id: String,
    thread_id: String,
    subject: String,
    sender_address: String,
    received_at: i64,
    priority: String,
    summary: Option<String>,
}

/// Messages matching a rule, received in `[from_ms, to_ms]`, optionally limited to `only_ids`, with no reply.
async fn firing(
    pool: &SqlitePool,
    workspace_id: &str,
    rule: &str,
    from_ms: i64,
    to_ms: i64,
    only_ids: Option<&[String]>,
    team: &[String],
) -> sqlx::Result<Vec<Candidate>> {
    let condition = match rule {
        APPROVAL_PENDING => {
            "c.category IN ('approval', 'leave')
             AND NOT EXISTS (SELECT 1 FROM review_items r WHERE r.message_id = m.id AND r.status = 'closed')"
        }
        IMPORTANT_UNANSWERED => {
            "(c.priority = 'high' OR (COALESCE(i.needs_reply, c.requires_reply) = 1 AND c.category != 'other'))
             AND c.category NOT IN ('approval', 'leave')"
        }
        _ => return Ok(vec![]),
    };
    let rows: Vec<Candidate> = sqlx::query_as(&format!(
        "SELECT m.id, m.thread_id, m.subject, m.sender_address, m.received_at, c.priority, i.summary
         FROM email_messages m
         JOIN classifications c ON c.message_id = m.id
         LEFT JOIN message_insights i ON i.message_id = m.id
         WHERE m.workspace_id = ?1 AND m.received_at >= ?2 AND m.received_at <= ?3
           AND (?4 IS NULL OR m.id IN (SELECT value FROM json_each(?4)))
           AND lower(m.sender_address) NOT IN (SELECT value FROM json_each(?5))
           AND {condition}
         ORDER BY m.received_at DESC LIMIT ?6"
    ))
    .bind(workspace_id)
    .bind(from_ms)
    .bind(to_ms)
    .bind(only_ids.map(|i| json!(i).to_string()))
    .bind(json!(team).to_string())
    .bind(MAX_CANDIDATES)
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for c in rows {
        if !thread_has_reply_after(pool, workspace_id, &c.thread_id, c.received_at).await? {
            out.push(c);
        }
    }
    Ok(out)
}

/// Cheap gate before a new `important_unanswered` alert: high priority always alerts; otherwise
/// one cached judge question ("important/time-sensitive?") cuts false alerts. Fails open.
async fn worth_alerting(state: &AppState, workspace_id: &str, c: &Candidate) -> sqlx::Result<bool> {
    if c.priority == "high" {
        return Ok(true);
    }
    let key = dedupe_key(IMPORTANT_UNANSWERED, Some(&c.id), 1);
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM alerts WHERE workspace_id = ?1 AND dedupe_key = ?2)")
        .bind(workspace_id)
        .bind(&key)
        .fetch_one(&state.db)
        .await?;
    if exists {
        return Ok(true);
    }
    let body: String = sqlx::query_scalar("SELECT body_text FROM email_messages WHERE id = ?1")
        .bind(&c.id)
        .fetch_optional(&state.db)
        .await?
        .unwrap_or_default();
    let context = judge::email_context(&c.sender_address, &c.subject, c.summary.as_deref(), &body);
    let scope = format!("alert_important:v1:{workspace_id}");
    let verdict = match judge::ask_cached(&state.db, state.judge.as_ref(), &scope, &context, &[judge::important_question()]).await {
        Ok(a) => a.yes_no(judge::IMPORTANT),
        Err(e) => {
            tracing::debug!(error = %e, "alert gate unavailable; alerting");
            None
        }
    };
    Ok(judge::alert_gate(false, verdict))
}

fn hours_ago(now: i64, at: i64) -> i64 {
    ((now - at) / HOUR_MS).max(0)
}

fn rule_alert<'a>(workspace_id: &'a str, rule: &'a str, c: &'a Candidate, now: i64) -> NewAlert<'a> {
    let h = hours_ago(now, c.received_at);
    let (title, body) = if rule == APPROVAL_PENDING {
        (format!("Waiting for a decision: {}", c.subject), format!("{} asked {h} hours ago. No decision has been recorded yet.", c.sender_address))
    } else {
        let summary = c.summary.as_deref().map(|s| format!(" {s}")).unwrap_or_default();
        (format!("No reply yet: {}", c.subject), format!("{} wrote {h} hours ago and nobody has replied.{summary}", c.sender_address))
    };
    NewAlert {
        workspace_id,
        kind: rule,
        severity: if c.priority == "high" { "high" } else { "normal" },
        title,
        body,
        message_id: Some(&c.id),
        level: 1,
        dedupe_key: None,
    }
}

// ---------------------------------------------------------------- evaluation

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EvalReport {
    pub raised: usize,
    pub resolved: usize,
    pub escalated: usize,
    pub reopened: u64,
    pub emails_sent: usize,
    pub digest_sent: bool,
}

async fn resolve(pool: &SqlitePool, workspace_id: &str, id: &str, message_id: Option<&str>, now: i64) -> sqlx::Result<bool> {
    let mut tx = pool.begin().await?;
    let done = sqlx::query(
        "UPDATE alerts SET status = 'resolved', resolved_at = ?1, resolved_by = 'system', snoozed_until = NULL, updated_at = ?1
         WHERE id = ?2 AND status != 'resolved'",
    )
    .bind(now)
    .bind(id)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if done {
        audit::record(
            &mut tx,
            Entry {
                workspace_id,
                actor: Actor::System,
                action: "alert.auto_resolved",
                entity_type: "alert",
                entity_id: id,
                message_id,
                metadata: json!({}),
            },
        )
        .await?;
    }
    tx.commit().await?;
    Ok(done)
}

/// One evaluation pass for a workspace at time `now` (injectable for tests).
pub async fn evaluate_workspace(state: &AppState, workspace_id: &str, now: i64) -> sqlx::Result<EvalReport> {
    let pool = &state.db;
    let mut report = EvalReport::default();
    let settings = load_settings(pool, workspace_id).await?;
    let team = team_addresses(pool, workspace_id).await?;

    report.reopened = sqlx::query(
        "UPDATE alerts SET status = 'open', snoozed_until = NULL, level_since = ?1, updated_at = ?1,
                notified_level = escalation_level - 1
         WHERE workspace_id = ?2 AND status = 'snoozed' AND snoozed_until <= ?1",
    )
    .bind(now)
    .bind(workspace_id)
    .execute(pool)
    .await?
    .rows_affected();

    // Auto-resolve rule alerts whose condition cleared (any age; the rule's time window doesn't matter here).
    let live: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT id, kind, message_id FROM alerts
         WHERE workspace_id = ?1 AND status != 'resolved' AND message_id IS NOT NULL AND kind IN (?2, ?3)",
    )
    .bind(workspace_id)
    .bind(IMPORTANT_UNANSWERED)
    .bind(APPROVAL_PENDING)
    .fetch_all(pool)
    .await?;
    for rule in [IMPORTANT_UNANSWERED, APPROVAL_PENDING] {
        let mine: Vec<&(String, String, String)> = live.iter().filter(|(_, k, _)| k == rule).collect();
        if mine.is_empty() {
            continue;
        }
        let ids: Vec<String> = mine.iter().map(|(_, _, m)| m.clone()).collect();
        let still: HashSet<String> =
            firing(pool, workspace_id, rule, 0, i64::MAX, Some(&ids), &team).await?.into_iter().map(|c| c.id).collect();
        for (id, _, message_id) in mine {
            if !still.contains(message_id) && resolve(pool, workspace_id, id, Some(message_id), now).await? {
                report.resolved += 1;
            }
        }
    }

    if !settings.enabled {
        return Ok(report);
    }

    let cutoff = now - settings.unanswered_hours * HOUR_MS;
    let from = now - LOOKBACK_MS;
    let approvals = firing(pool, workspace_id, APPROVAL_PENDING, from, cutoff, None, &team).await?;
    let approval_ids: HashSet<&str> = approvals.iter().map(|c| c.id.as_str()).collect();
    let important = firing(pool, workspace_id, IMPORTANT_UNANSWERED, from, cutoff, None, &team).await?;
    for c in &approvals {
        report.raised += raise(pool, rule_alert(workspace_id, APPROVAL_PENDING, c, now)).await?.1 as usize;
    }
    for c in important.iter().filter(|c| !approval_ids.contains(c.id.as_str())) {
        if !worth_alerting(state, workspace_id, c).await? {
            continue;
        }
        report.raised += raise(pool, rule_alert(workspace_id, IMPORTANT_UNANSWERED, c, now)).await?.1 as usize;
    }

    let escalated: Vec<(String, Option<String>, i64)> = sqlx::query_as(
        "UPDATE alerts SET escalation_level = escalation_level + 1, severity = 'high', level_since = ?1, updated_at = ?1
         WHERE workspace_id = ?2 AND status = 'open' AND escalation_level < ?3 AND level_since <= ?4
         RETURNING id, message_id, escalation_level",
    )
    .bind(now)
    .bind(workspace_id)
    .bind(MAX_LEVEL)
    .bind(now - settings.escalate_after_hours * HOUR_MS)
    .fetch_all(pool)
    .await?;
    if !escalated.is_empty() {
        let mut conn = pool.acquire().await?;
        for (id, message_id, level) in &escalated {
            audit::record(
                &mut conn,
                Entry {
                    workspace_id,
                    actor: Actor::System,
                    action: "alert.escalated",
                    entity_type: "alert",
                    entity_id: id,
                    message_id: message_id.as_deref(),
                    metadata: json!({ "level": level }),
                },
            )
            .await?;
        }
    }
    report.escalated = escalated.len();

    report.emails_sent = deliver(state, workspace_id, &settings).await?;
    if settings.daily_digest && (now / HOUR_MS) % 24 == settings.digest_hour {
        report.digest_sent = digest(state, workspace_id, &settings, now).await?;
    }
    Ok(report)
}

// ---------------------------------------------------------------- delivery

#[derive(sqlx::FromRow)]
struct Pending {
    id: String,
    title: String,
    body: String,
    message_id: Option<String>,
    escalation_level: i64,
    notified_json: String,
}

/// One alert in an email.
pub struct AlertLine {
    pub title: String,
    pub body: String,
    pub link: String,
    pub level: i64,
}

fn base_url(state: &AppState) -> String {
    state.config.app_base_url.trim_end_matches('/').to_string()
}

pub fn deep_link(base: &str, workspace_id: &str, message_id: Option<&str>) -> String {
    match message_id {
        Some(m) => format!("{base}/w/{workspace_id}/inbox/{m}"),
        None => format!("{base}/w/{workspace_id}/alerts"),
    }
}

async fn workspace_name(pool: &SqlitePool, workspace_id: &str) -> sqlx::Result<String> {
    Ok(sqlx::query_scalar("SELECT name FROM workspaces WHERE id = ?1")
        .bind(workspace_id)
        .fetch_optional(pool)
        .await?
        .unwrap_or_else(|| "your workspace".into()))
}

/// Sends every pending notification, batched into one email per recipient. Returns emails sent.
async fn deliver(state: &AppState, workspace_id: &str, settings: &AlertSettings) -> sqlx::Result<usize> {
    let Some(mailer) = state.alert_mailer.clone() else {
        return Ok(0);
    };
    let pending: Vec<Pending> = sqlx::query_as(
        "SELECT id, title, body, message_id, escalation_level, notified_json FROM alerts
         WHERE workspace_id = ?1 AND status = 'open' AND notified_level < escalation_level ORDER BY created_at",
    )
    .bind(workspace_id)
    .fetch_all(&state.db)
    .await?;
    if pending.is_empty() {
        return Ok(0);
    }
    let base = base_url(state);
    let ws_name = workspace_name(&state.db, workspace_id).await?;
    let mut by_recipient: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, p) in pending.iter().enumerate() {
        let targets = if p.escalation_level <= 1 || settings.escalate_to.is_empty() { &settings.recipients } else { &settings.escalate_to };
        for t in targets {
            by_recipient.entry(t.as_str()).or_default().push(i);
        }
    }
    let mut delivered: Vec<Vec<String>> = vec![vec![]; pending.len()];
    let mut sent = 0;
    for (to, idxs) in by_recipient {
        let lines: Vec<AlertLine> = idxs
            .iter()
            .map(|&i| AlertLine {
                title: pending[i].title.clone(),
                body: pending[i].body.clone(),
                link: deep_link(&base, workspace_id, pending[i].message_id.as_deref()),
                level: pending[i].escalation_level,
            })
            .collect();
        let (subject, text, html) = render_email(&ws_name, &base, workspace_id, &lines, None);
        let email = OutgoingEmail { kind: mail_kind::ALERT, to, subject: &subject, text: &text, html: &html };
        match crate::mailer::deliver(&state.db, mailer.as_ref(), email).await {
            Ok(()) => {
                sent += 1;
                for &i in &idxs {
                    delivered[i].push(to.to_string());
                }
            }
            Err(e) => tracing::warn!(error = %e, "alert email failed; will retry next run"),
        }
    }
    for (p, to) in pending.iter().zip(delivered) {
        if to.is_empty() {
            continue;
        }
        let mut notified: Vec<String> = serde_json::from_str(&p.notified_json).unwrap_or_default();
        for t in to {
            if !notified.contains(&t) {
                notified.push(t);
            }
        }
        sqlx::query("UPDATE alerts SET notified_level = ?1, notified_json = ?2, updated_at = ?3 WHERE id = ?4")
            .bind(p.escalation_level)
            .bind(json!(notified).to_string())
            .bind(now_ms())
            .bind(&p.id)
            .execute(&state.db)
            .await?;
    }
    Ok(sent)
}

/// Claims today's digest (once per UTC day) and sends all open alerts to the recipients.
async fn digest(state: &AppState, workspace_id: &str, settings: &AlertSettings, now: i64) -> sqlx::Result<bool> {
    let Some(mailer) = state.alert_mailer.clone() else {
        return Ok(false);
    };
    let day = now / 86_400_000;
    let claimed = sqlx::query(
        "INSERT INTO alert_settings (workspace_id, last_digest_day, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(workspace_id) DO UPDATE SET last_digest_day = excluded.last_digest_day
         WHERE alert_settings.last_digest_day IS NULL OR alert_settings.last_digest_day < excluded.last_digest_day",
    )
    .bind(workspace_id)
    .bind(day)
    .bind(now)
    .execute(&state.db)
    .await?
    .rows_affected()
        == 1;
    if !claimed {
        return Ok(false);
    }
    let open: Vec<(String, String, Option<String>, i64)> = sqlx::query_as(
        "SELECT title, body, message_id, escalation_level FROM alerts WHERE workspace_id = ?1 AND status = 'open'
         ORDER BY severity = 'high' DESC, created_at LIMIT 50",
    )
    .bind(workspace_id)
    .fetch_all(&state.db)
    .await?;
    if open.is_empty() {
        return Ok(false);
    }
    let base = base_url(state);
    let ws_name = workspace_name(&state.db, workspace_id).await?;
    let lines: Vec<AlertLine> = open
        .into_iter()
        .map(|(title, body, message_id, level)| AlertLine { title, body, link: deep_link(&base, workspace_id, message_id.as_deref()), level })
        .collect();
    let (subject, text, html) = render_email(&ws_name, &base, workspace_id, &lines, Some("Daily digest"));
    let mut any = false;
    for to in &settings.recipients {
        let email = OutgoingEmail { kind: mail_kind::ALERT_DIGEST, to, subject: &subject, text: &text, html: &html };
        match crate::mailer::deliver(&state.db, mailer.as_ref(), email).await {
            Ok(()) => any = true,
            Err(e) => tracing::warn!(error = %e, "digest email failed"),
        }
    }
    Ok(any)
}

pub fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

/// Plain-text + simple HTML alert email. `heading` = `Some("Daily digest")` for the digest.
pub fn render_email(workspace_name: &str, base: &str, workspace_id: &str, lines: &[AlertLine], heading: Option<&str>) -> (String, String, String) {
    let title_of = |l: &AlertLine| if l.level > 1 { format!("Escalated: {}", l.title) } else { l.title.clone() };
    let subject = match (heading, lines) {
        (Some(h), _) => format!("[Post Room] {h}: {} open alert{}", lines.len(), if lines.len() == 1 { "" } else { "s" }),
        (None, [one]) => format!("[Post Room] {}", title_of(one)),
        (None, _) => format!("[Post Room] {} emails need attention", lines.len()),
    };
    let settings_link = format!("{base}/w/{workspace_id}/alerts");
    let intro = match heading {
        Some(_) => format!("Open alerts in {workspace_name}:"),
        None => format!("{} in {workspace_name} need{} attention:", if lines.len() == 1 { "One thing".to_string() } else { format!("{} things", lines.len()) }, if lines.len() == 1 { "s" } else { "" }),
    };
    let mut text = format!("Hi,\n\n{intro}\n\n");
    for l in lines {
        text.push_str(&format!("- {}\n  {}\n  Open: {}\n\n", title_of(l), l.body, l.link));
    }
    text.push_str(&format!("--\nPost Room alerts. All alerts and settings: {settings_link}\n"));

    let items: String = lines
        .iter()
        .map(|l| {
            format!(
                r#"<li style="margin:0 0 16px"><a href="{}" style="color:#34557A;font-weight:600">{}</a><br><span style="color:#5E5A50">{}</span></li>"#,
                escape_html(&l.link),
                escape_html(&title_of(l)),
                escape_html(&l.body)
            )
        })
        .collect();
    let html = format!(
        r#"<div style="font-family:Arial,sans-serif;background:#F3EFE6;padding:32px;color:#1C1B18">
<div style="height:6px;background:repeating-linear-gradient(135deg,#B73A26 0 10px,#F3EFE6 10px 20px,#34557A 20px 30px,#F3EFE6 30px 40px)"></div>
<h1 style="font-family:Georgia,serif;font-weight:600;font-size:22px;margin:24px 0 8px">{}</h1>
<ul style="padding-left:18px;margin:16px 0">{items}</ul>
<p style="font-size:12px;color:#5E5A50"><a href="{}" style="color:#5E5A50">All alerts and settings</a></p>
</div>"#,
        escape_html(&intro),
        escape_html(&settings_link)
    );
    (subject, text, html)
}

// ---------------------------------------------------------------- scheduling

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

// ---------------------------------------------------------------- API shape

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
