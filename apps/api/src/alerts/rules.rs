use super::*;

/// True if someone on the team answered in this thread after `after_ms`.
///
/// Column-free heuristic: a later message in the same thread sent from a team address (a workspace member, or
/// the user who connected the mailbox, whose address is the mailbox owner's for Gmail), or a reply sent from
/// Post Room (`drafts.status = 'sent'`) for any message in the thread after `after_ms`.
/// Replied = outbound mail / sent draft in the thread (direction column), or mail from a team
/// address (covers send-as aliases Gmail didn't label SENT).
async fn thread_has_reply_after(db: &SqlitePool, workspace_id: &str, thread_key: &str, after_ms: i64) -> sqlx::Result<bool> {
    Ok(crate::mail::threads::thread_has_reply_after(db, workspace_id, thread_key, after_ms).await?
        || team_sent_after(db, workspace_id, thread_key, after_ms).await?)
}

async fn team_sent_after(db: &SqlitePool, workspace_id: &str, thread_key: &str, after_ms: i64) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS(
           SELECT 1 FROM email_messages r JOIN email_sources s ON s.id = r.source_id
           WHERE r.workspace_id = ?1 AND r.thread_id = ?2 AND r.suppressed = 0 AND r.received_at > ?3
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
pub(super) struct Candidate {
    pub(super) id: String,
    thread_id: String,
    subject: String,
    sender_address: String,
    received_at: i64,
    priority: String,
    summary: Option<String>,
}

/// Messages matching a rule, received in `[from_ms, to_ms]`, optionally limited to `only_ids`, with no reply.
pub(super) async fn firing(
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
    let not_own = crate::mail::system_mail::not_own_sql("m");
    let rows: Vec<Candidate> = sqlx::query_as(&format!(
        "SELECT m.id, m.thread_id, m.subject, m.sender_address, m.received_at, c.priority, i.summary
         FROM email_messages m
         JOIN classifications c ON c.message_id = m.id
         LEFT JOIN message_insights i ON i.message_id = m.id
         WHERE m.workspace_id = ?1 AND m.received_at >= ?2 AND m.received_at <= ?3
           AND (?4 IS NULL OR m.id IN (SELECT value FROM json_each(?4)))
           AND lower(m.sender_address) NOT IN (SELECT value FROM json_each(?5))
           AND {not_own}
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
pub(super) async fn worth_alerting(state: &AppState, workspace_id: &str, c: &Candidate) -> sqlx::Result<bool> {
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
    // Understanding v2 already decided what is time-sensitive while reading the email: no judge call.
    if state.config.understand_mode == crate::platform::config::UnderstandMode::On
        && let Some(u) = crate::ai::understand::stored(&state.db, &c.id).await?
    {
        return Ok(u.important());
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

pub(super) fn rule_alert<'a>(workspace_id: &'a str, rule: &'a str, c: &'a Candidate, now: i64) -> NewAlert<'a> {
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

/// Raises a rule alert and gives it an owner (the person linked to the mailbox) for the escalation chain.
pub(super) async fn raise_rule(pool: &SqlitePool, alert: NewAlert<'_>, message_id: &str) -> sqlx::Result<bool> {
    let workspace_id = alert.workspace_id;
    let (id, created) = raise(pool, alert).await?;
    if created {
        crate::hr::team::escalation::tag_owner_for_message(pool, workspace_id, &id, message_id).await?;
    }
    Ok(created)
}
