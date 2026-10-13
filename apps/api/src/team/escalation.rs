//! Layered escalation: an alert about a person goes to them first, then to their manager, then to the
//! final contacts (HR or the admins). Delays are settings. Also raises the generic `below_target` alert.

use serde::Serialize;
use sqlx::SqlitePool;

use super::{clock, metrics, people, settings};
use crate::{
    alerts::{self, NewAlert},
    http::AppState,
    packs,
};

pub const BELOW_TARGET: &str = "below_target";

/// Hours an alert waits at the owner, and then at the manager: `(manager_after, final_after)`.
pub async fn delays(db: &SqlitePool, workspace_id: &str, default_hours: i64) -> sqlx::Result<(i64, i64)> {
    let s = settings::load(db, workspace_id).await?;
    Ok((s.manager_after_hours.unwrap_or(default_hours), s.final_after_hours.unwrap_or(default_hours)))
}

pub async fn set_alert_owner(db: &SqlitePool, alert_id: &str, person_id: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE alerts SET person_id = ?1 WHERE id = ?2").bind(person_id).bind(alert_id).execute(db).await?;
    Ok(())
}

/// The owner of a mailbox's mail is the person linked to that mailbox.
pub async fn tag_owner_for_message(db: &SqlitePool, workspace_id: &str, alert_id: &str, message_id: &str) -> sqlx::Result<()> {
    let owner: Option<String> = sqlx::query_scalar(
        "SELECT p.id FROM email_messages m JOIN people p ON p.source_id = m.source_id AND p.workspace_id = m.workspace_id
         WHERE m.id = ?1 AND m.workspace_id = ?2 AND p.active = 1 ORDER BY p.created_at, p.id LIMIT 1",
    )
    .bind(message_id)
    .bind(workspace_id)
    .fetch_optional(db)
    .await?;
    if let Some(p) = owner {
        set_alert_owner(db, alert_id, &p).await?;
    }
    Ok(())
}

pub struct PersonAlert<'a> {
    pub workspace_id: &'a str,
    pub person_id: &'a str,
    pub kind: &'a str,
    pub severity: &'a str,
    pub title: String,
    pub body: String,
    pub dedupe_key: String,
    pub message_id: Option<&'a str>,
    /// The evaluation clock; the escalation delay counts from here.
    pub now: i64,
}

/// Idempotent by `dedupe_key`; returns `(alert_id, created)`.
pub async fn raise_for_person(db: &SqlitePool, a: PersonAlert<'_>) -> sqlx::Result<(String, bool)> {
    let (id, created) = alerts::raise(
        db,
        NewAlert {
            workspace_id: a.workspace_id,
            kind: a.kind,
            severity: a.severity,
            title: a.title,
            body: a.body,
            message_id: a.message_id,
            level: 1,
            dedupe_key: Some(a.dedupe_key),
        },
    )
    .await?;
    if created {
        set_alert_owner(db, &id, a.person_id).await?;
        sqlx::query("UPDATE alerts SET level_since = ?1, created_at = ?1, updated_at = ?1 WHERE id = ?2").bind(a.now).bind(&id).execute(db).await?;
    }
    Ok((id, created))
}

async fn primary_email(db: &SqlitePool, workspace_id: &str, person_id: &str) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar(
        "SELECT e.email FROM person_emails e JOIN people p ON p.id = e.person_id
         WHERE e.workspace_id = ?1 AND e.person_id = ?2 AND p.active = 1 ORDER BY e.position LIMIT 1",
    )
    .bind(workspace_id)
    .bind(person_id)
    .fetch_optional(db)
    .await
}

/// Final contacts: the configured list, else the alert recipients (admins by default).
async fn final_contacts(db: &SqlitePool, workspace_id: &str, fallback: &[String]) -> sqlx::Result<Vec<String>> {
    let s = settings::load(db, workspace_id).await?;
    Ok(if s.final_emails.is_empty() { fallback.to_vec() } else { s.final_emails })
}

/// Who an alert about `person_id` goes to at `level`: 1 the person, 2 their manager (else the final contacts),
/// 3 the final contacts. `fallback` = the workspace's alert recipients.
pub async fn recipients(db: &SqlitePool, workspace_id: &str, person_id: &str, level: i64, fallback: &[String]) -> sqlx::Result<Vec<String>> {
    let manager: Option<String> = sqlx::query_scalar("SELECT manager_id FROM people WHERE id = ?1 AND workspace_id = ?2")
        .bind(person_id)
        .bind(workspace_id)
        .fetch_optional(db)
        .await?
        .flatten();
    let list = match level {
        ..=1 => primary_email(db, workspace_id, person_id).await?.into_iter().collect(),
        2 => match manager {
            Some(m) => primary_email(db, workspace_id, &m).await?.into_iter().collect(),
            None => vec![],
        },
        _ => vec![],
    };
    if list.is_empty() { final_contacts(db, workspace_id, fallback).await } else { Ok(list) }
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Step {
    pub level: i64,
    pub label: &'static str,
    pub name: Option<String>,
    pub emails: Vec<String>,
}

/// The chain for one person, as the Escalations page shows it.
pub async fn chain(db: &SqlitePool, workspace_id: &str, person_id: &str, fallback: &[String]) -> sqlx::Result<Vec<Step>> {
    let Some(person) = people::get(db, workspace_id, person_id).await? else { return Ok(vec![]) };
    let manager = match &person.manager_id {
        Some(m) => people::get(db, workspace_id, m).await?,
        None => None,
    };
    Ok(vec![
        Step { level: 1, label: "Owner", name: Some(person.name.clone()), emails: recipients(db, workspace_id, person_id, 1, fallback).await? },
        Step { level: 2, label: "Manager", name: manager.map(|m| m.name), emails: recipients(db, workspace_id, person_id, 2, fallback).await? },
        Step { level: 3, label: "Final contacts", name: None, emails: recipients(db, workspace_id, person_id, 3, fallback).await? },
    ])
}

// ---------------------------------------------------------------- below target

/// Raises `below_target` for people short of their daily target after `below_target_hour` on weekdays, and of
/// their weekly target on Friday after `weekly_check_hour`; resolves them once the target is reached.
pub async fn evaluate_below_target(state: &AppState, workspace_id: &str, now: i64) -> sqlx::Result<usize> {
    let db = &state.db;
    let s = settings::load(db, workspace_id).await?;
    let pack_keys = packs::enabled_keys(db, workspace_id).await?;
    let w = clock::windows(now, s.tz_offset_minutes);
    let mut raised = 0;
    for p in people::list(db, workspace_id, false).await? {
        let started = p.joining_date.as_deref().is_none_or(|j| j <= w.date.as_str());
        if !started {
            continue;
        }
        let checks = [
            ("d", w.date.clone(), p.daily_target, (w.day_start, w.day_end), w.weekday < 5 && w.local_hour >= s.below_target_hour, "today"),
            ("w", w.week_date.clone(), p.weekly_target, (w.week_start, w.week_end), w.weekday == 4 && w.local_hour >= s.weekly_check_hour, "this week"),
        ];
        for (tag, period, target, (from, to), due, when) in checks {
            let Some(target) = target.filter(|t| *t > 0) else { continue };
            let key = alerts::dedupe_key(BELOW_TARGET, Some(&format!("{}:{tag}{period}", p.id)), 1);
            let (count, metric) = metrics::primary_count(db, workspace_id, &p.id, &pack_keys, from, to).await?;
            if count >= target as f64 {
                sqlx::query(
                    "UPDATE alerts SET status = 'resolved', resolved_at = ?1, resolved_by = 'system', snoozed_until = NULL, updated_at = ?1
                     WHERE workspace_id = ?2 AND dedupe_key = ?3 AND status != 'resolved'",
                )
                .bind(now)
                .bind(workspace_id)
                .bind(&key)
                .execute(db)
                .await?;
                continue;
            }
            if !due {
                continue;
            }
            let label = metrics::label_of(metric, &pack_keys).to_lowercase();
            let (_, created) = raise_for_person(
                db,
                PersonAlert {
                    workspace_id,
                    person_id: &p.id,
                    kind: BELOW_TARGET,
                    severity: "normal",
                    title: format!("Below target {when}: {}", p.name),
                    body: format!("{} has {} of {target} ({label}) {when}.", p.name, count as i64),
                    dedupe_key: key,
                    message_id: None,
                    now,
                },
            )
            .await?;
            raised += created as usize;
        }
    }
    Ok(raised)
}
