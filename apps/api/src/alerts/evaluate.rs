use super::*;

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
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
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
        report.raised += raise_rule(pool, rule_alert(workspace_id, APPROVAL_PENDING, c, now), &c.id).await? as usize;
    }
    for c in important.iter().filter(|c| !approval_ids.contains(c.id.as_str())) {
        if !worth_alerting(state, workspace_id, c).await? {
            continue;
        }
        report.raised += raise_rule(pool, rule_alert(workspace_id, IMPORTANT_UNANSWERED, c, now), &c.id).await? as usize;
    }
    report.raised += crate::hr::team::evaluate_alerts(state, workspace_id, now).await?;

    // Alerts about a person wait at the owner, then at the manager, for their own configurable delays.
    let (manager_hours, final_hours) = crate::hr::team::escalation::delays(pool, workspace_id, settings.escalate_after_hours).await?;
    let escalated: Vec<(String, Option<String>, i64)> = sqlx::query_as(
        "UPDATE alerts SET escalation_level = escalation_level + 1, severity = 'high', level_since = ?1, updated_at = ?1
         WHERE workspace_id = ?2 AND status = 'open' AND escalation_level < ?3
           AND level_since <= ?1 - 3600000 * CASE WHEN person_id IS NULL THEN ?4 WHEN escalation_level = 1 THEN ?5 ELSE ?6 END
         RETURNING id, message_id, escalation_level",
    )
    .bind(now)
    .bind(workspace_id)
    .bind(MAX_LEVEL)
    .bind(settings.escalate_after_hours)
    .bind(manager_hours)
    .bind(final_hours)
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

    report.raised += crate::ai::health::evaluate_spikes(pool, workspace_id, now).await?;
    report.emails_sent = deliver(state, workspace_id, &settings).await?;
    if settings.daily_digest && (now / HOUR_MS) % 24 == settings.digest_hour {
        report.digest_sent = digest(state, workspace_id, &settings, now).await?;
    }
    Ok(report)
}
