use super::*;

/// What follows the attempt id in our Message-ID.
const MESSAGE_ID_SUFFIX: &str = "@postroom.mail>";

/// Our Message-ID for one send attempt.
pub fn attempt_message_id(attempt_id: &str) -> String {
    format!("<{attempt_id}{MESSAGE_ID_SUFFIX}")
}

/// An open attempt younger than this may not be visible in Gmail yet: never call it "not sent".
pub const RECONCILE_GRACE_MS: i64 = 120_000;

#[derive(Debug, Clone, PartialEq)]
pub enum Reconciled {
    /// Gmail has it (provider message id).
    Sent(String),
    /// Safe to try again.
    NotSent,
    /// Can't tell yet; don't send.
    Unsure(String),
}

/// Decides from a Gmail thread (format=metadata) whether our attempt went out: our Message-ID is
/// there, or a SENT message appeared after the attempt started (conservative: never resend then).
pub fn sent_in_thread(thread: &Value, message_id: &str, started_at: i64) -> Option<String> {
    let messages = thread.get("messages").and_then(Value::as_array)?;
    messages.iter().find_map(|m| {
        let id = m.get("id").and_then(Value::as_str)?.to_string();
        let ours = m
            .pointer("/payload/headers")
            .and_then(Value::as_array)
            .is_some_and(|hs| {
                hs.iter().any(|h| {
                    h.get("name").and_then(Value::as_str).is_some_and(|n| n.eq_ignore_ascii_case("message-id"))
                        && h.get("value").and_then(Value::as_str).is_some_and(|v| v.trim().eq_ignore_ascii_case(message_id))
                })
            });
        let sent_after = m.get("labelIds").and_then(Value::as_array).is_some_and(|l| l.iter().any(|x| x == "SENT"))
            && m.get("internalDate")
                .and_then(|d| d.as_str().and_then(|s| s.parse::<i64>().ok()).or_else(|| d.as_i64()))
                .is_some_and(|at| at >= started_at - 60_000);
        (ours || sent_after).then_some(id)
    })
}

pub async fn reconcile(state: &AppState, ctx: &ReplyContext) -> Reconciled {
    let Some(attempt) = ctx.send_attempt_id.as_deref() else { return Reconciled::NotSent };
    let started = ctx.send_started_at.unwrap_or(0);
    let Ok((conn, pck)) = connection(ctx) else { return Reconciled::Unsure("no mailbox connection".into()) };
    let mid = attempt_message_id(attempt);
    let query = format!("rfc822msgid:{} in:anywhere", mid.trim_matches(['<', '>']));
    match state.nango.gmail_list_ids(conn, pck, &query, None).await {
        Ok((ids, _)) => {
            if let Some(id) = ids.into_iter().next() {
                return Reconciled::Sent(id);
            }
        }
        Err(e) => return Reconciled::Unsure(format!("Gmail search failed: {e}")),
    }
    if let Some(tid) = ctx.provider_thread_id.as_deref().filter(|t| !t.is_empty() && t.chars().all(|c| c.is_ascii_alphanumeric())) {
        let path = format!("threads/{tid}?format=metadata&metadataHeaders=Message-ID");
        match state.nango.gmail_call(Method::GET, conn, pck, &path, None).await {
            Ok(thread) => {
                if let Some(id) = sent_in_thread(&thread, &mid, started) {
                    return Reconciled::Sent(id);
                }
            }
            Err(e) => return Reconciled::Unsure(format!("Gmail thread read failed: {e}")),
        }
    }
    if now_ms() - started < RECONCILE_GRACE_MS {
        return Reconciled::Unsure("the last attempt is too recent to be sure".into());
    }
    Reconciled::NotSent
}

pub(super) async fn clear_attempt(state: &AppState, draft_id: &str, attempt: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE drafts SET send_attempt_id = NULL, send_started_at = NULL WHERE id = ?1 AND send_attempt_id = ?2 AND status != 'sent'")
        .bind(draft_id)
        .bind(attempt)
        .execute(&state.db)
        .await?;
    Ok(())
}

/// Records a confirmed send (idempotent): status, audit, indexing, learning.
pub(super) async fn finalize_sent(state: &AppState, ws: &str, ctx: &ReplyContext, provider_message_id: &str, actor: Actor<'_>) -> AppResult<()> {
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let changed = sqlx::query(
        "UPDATE drafts SET status = 'sent', provider_message_id = ?1, send_at = NULL, send_error = NULL, updated_at = ?2,
                rfc_message_id = CASE WHEN send_attempt_id IS NOT NULL THEN '<' || send_attempt_id || ?4 ELSE rfc_message_id END
         WHERE id = ?3 AND status != 'sent'",
    )
    .bind(provider_message_id)
    .bind(now_ms())
    .bind(&ctx.draft_id)
    .bind(MESSAGE_ID_SUFFIX)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if !changed {
        return Ok(());
    }
    jobs::knowledge_jobs::enqueue_index_reply(&mut tx, ws, &ctx.draft_id, "sent").await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: ws,
            actor,
            action: "draft.sent",
            entity_type: "draft",
            entity_id: &ctx.draft_id,
            message_id: Some(&ctx.message_id),
            metadata: json!({ "to": "original_sender", "provider_message_id": provider_message_id }),
        },
    )
    .await?;
    let promised: Option<String> = sqlx::query_scalar("SELECT unanswered_json FROM drafts WHERE id = ?1").bind(&ctx.draft_id).fetch_optional(&mut *tx).await?;
    let promised = promised.map(|j| crate::replies::gap_resolver::parse_unanswered(&j)).unwrap_or_default();
    if !promised.is_empty() && ctx.body_text.to_lowercase().contains("we'll confirm") {
        let owner: Option<String> = sqlx::query_scalar("SELECT connected_by_user_id FROM email_sources WHERE id = ?1").bind(&ctx.source_id).fetch_optional(&mut *tx).await?.flatten();
        let title = crate::mail::sources::normalize::truncate_chars(&format!("Follow up: you told {} we'd confirm {}", ctx.sender_address, promised.join(", ")), 200);
        sqlx::query(
            "INSERT INTO review_items (id, workspace_id, message_id, workflow_run_id, kind, title, status, assigned_user_id, created_at)
             VALUES (?1, ?2, ?3, NULL, 'agent_task', ?4, 'open', ?5, ?6)",
        )
        .bind(new_id("rev"))
        .bind(ws)
        .bind(&ctx.message_id)
        .bind(title)
        .bind(owner)
        .bind(now_ms())
        .execute(&mut *tx)
        .await?;
    }
    if ctx.body_text.to_lowercase().contains(&crate::replies::gap_resolver::NONE_PHRASE.to_lowercase()) {
        crate::hr::candidate_watch::queue_enrol(&mut tx, ws, &ctx.message_id, &ctx.draft_id).await?;
    }
    crate::agents::handoff::store::mark_sent(&mut tx, ws, &ctx.draft_id).await?;
    tx.commit().await?;
    if let Err(e) = crate::agents::handoff::store::follow_up(state, ws, &ctx.message_id).await {
        tracing::warn!(error = %e, "could not queue the follow-up for a request filed after the reply was written");
    }
    if let Err(e) = jobs::learning_jobs::enqueue_reply_learning(&state.db, &ctx.draft_id).await {
        tracing::warn!(error = %e, "could not schedule learning from the sent reply");
    }
    state.jobs_notify.notify_one();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconciles_from_thread_metadata() {
        let mid = attempt_message_id("snd_9");
        let thread = json!({"messages": [
            {"id": "g1", "labelIds": ["INBOX"], "internalDate": "1000", "payload": {"headers": [{"name": "Message-ID", "value": "<x@y>"}]}},
            {"id": "g2", "labelIds": ["SENT"], "internalDate": "5000", "payload": {"headers": [{"name": "Message-Id", "value": mid}]}}
        ]});
        assert_eq!(sent_in_thread(&thread, &mid, 10_000_000), Some("g2".into()), "our Message-ID found");
        let other = json!({"messages": [{"id": "g3", "labelIds": ["SENT"], "internalDate": "200000", "payload": {"headers": []}}]});
        assert_eq!(sent_in_thread(&other, &mid, 180_000), Some("g3".into()), "a SENT message after the attempt counts");
        assert_eq!(sent_in_thread(&other, &mid, 500_000), None, "older sent mail doesn't");
        assert_eq!(sent_in_thread(&json!({}), &mid, 0), None);
    }
}
