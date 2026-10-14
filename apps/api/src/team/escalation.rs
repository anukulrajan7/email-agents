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
    // Owner routing is opt-in: the staffing pack always, anyone else through the team setting.
    if !packs::enabled(db, workspace_id, packs::STAFFING).await? && !settings::load(db, workspace_id).await?.route_alerts_by_owner {
        return Ok(());
    }
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
    /// The window a rolling alert is about.
    pub period_key: Option<&'a str>,
}

/// Idempotent by `dedupe_key`; returns `(alert_id, created)`.
pub async fn raise_for_person(db: &SqlitePool, a: PersonAlert<'_>) -> sqlx::Result<(String, bool)> {
    let (title, body) = (a.title.clone(), a.body.clone());
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
        sqlx::query("UPDATE alerts SET level_since = ?1, created_at = ?1, updated_at = ?1, period_key = ?3 WHERE id = ?2")
            .bind(a.now)
            .bind(&id)
            .bind(a.period_key)
            .execute(db)
            .await?;
        return Ok((id, true));
    }
    // The condition came back after the system had closed the alert: reopen it at level 1. A person's own
    // resolve is respected.
    let reopened = sqlx::query(
        "UPDATE alerts SET status = 'open', escalation_level = 1, notified_level = 0, notified_json = '[]', level_since = ?1, updated_at = ?1,
                resolved_at = NULL, resolved_by = NULL, snoozed_until = NULL, severity = ?2, title = ?3, body = ?4, period_key = ?5, person_id = ?6
         WHERE id = ?7 AND status = 'resolved' AND resolved_by = 'system'",
    )
    .bind(a.now)
    .bind(if a.severity == "high" { "high" } else { "normal" })
    .bind(&title)
    .bind(&body)
    .bind(a.period_key)
    .bind(a.person_id)
    .bind(&id)
    .execute(db)
    .await?
    .rows_affected()
        == 1;
    Ok((id, reopened))
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

/// One rolling `below_target` alert per person. It is raised (or reopened) after `below_target_hour` on a weekday when
/// today's count is under the daily target, or on Friday after `weekly_check_hour` when the week is under the weekly
/// target. It resolves itself when the target is met or its window closes.
pub async fn evaluate_below_target(state: &AppState, workspace_id: &str, now: i64) -> sqlx::Result<usize> {
    let db = &state.db;
    let s = settings::load(db, workspace_id).await?;
    let pack_keys = packs::enabled_keys(db, workspace_id).await?;
    let w = clock::windows(now, s.tz_offset_minutes);
    let mut raised = 0;
    for p in people::list(db, workspace_id, false).await? {
        let key = format!("{BELOW_TARGET}:{}", p.id);
        let open: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT id, period_key FROM alerts WHERE workspace_id = ?1 AND dedupe_key = ?2 AND status IN ('open', 'snoozed')",
        )
        .bind(workspace_id)
        .bind(&key)
        .fetch_optional(db)
        .await?;
        let started = p.joining_date.as_deref().is_none_or(|j| j <= w.date.as_str());
        let checks = [
            ("d", w.date.clone(), p.daily_target, (w.day_start, w.day_end), w.weekday < 5 && w.local_hour >= s.below_target_hour, "today"),
            ("w", w.week_date.clone(), p.weekly_target, (w.week_start, w.week_end), w.weekday == 4 && w.local_hour >= s.weekly_check_hour, "this week"),
        ];
        let mut open_still_below = false;
        let mut pick: Option<(String, f64, i64, &str, &'static str)> = None;
        if started {
            for (tag, period, target, (from, to), due, when) in checks {
                let Some(target) = target.filter(|t| *t > 0) else { continue };
                let period = format!("{tag}{period}");
                let (count, metric) = metrics::primary_count(db, workspace_id, &p.id, &pack_keys, from, to).await?;
                let below = count < target as f64;
                if open.as_ref().is_some_and(|(_, pk)| pk.as_deref() == Some(period.as_str())) {
                    open_still_below = below;
                }
                if pick.is_none() && due && below {
                    pick = Some((period, count, target, when, metric));
                }
            }
        }
        if let Some((id, _)) = &open {
            if !open_still_below {
                sqlx::query("UPDATE alerts SET status = 'resolved', resolved_at = ?1, resolved_by = 'system', snoozed_until = NULL, updated_at = ?1 WHERE id = ?2")
                    .bind(now)
                    .bind(id)
                    .execute(db)
                    .await?;
            } else if let Some((period, count, target, when, metric)) = &pick {
                let label = metrics::label_of(metric, &pack_keys).to_lowercase();
                sqlx::query("UPDATE alerts SET body = ?1, updated_at = ?2 WHERE id = ?3 AND period_key = ?4")
                    .bind(format!("{} has {} of {target} ({label}) {when}.", p.name, *count as i64))
                    .bind(now)
                    .bind(id)
                    .bind(period)
                    .execute(db)
                    .await?;
                continue;
            }
        }
        let Some((period, count, target, when, metric)) = pick else { continue };
        if open.is_some() && open_still_below {
            continue;
        }
        let label = metrics::label_of(metric, &pack_keys).to_lowercase();
        let (_, changed) = raise_for_person(
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
                period_key: Some(&period),
            },
        )
        .await?;
        raised += changed as usize;
    }
    Ok(raised)
}
