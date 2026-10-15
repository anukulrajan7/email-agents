use super::*;

pub(super) fn ids_json(ids: &[String]) -> String {
    json!(ids).to_string()
}

fn actor_id(actor: Actor<'_>) -> Option<&str> {
    match actor {
        Actor::User(id) => Some(id),
        Actor::System => None,
    }
}

#[derive(sqlx::FromRow)]
struct Doomed {
    id: String,
    source_id: String,
    thread_id: String,
    sender_address: String,
}

async fn scalar(sql: &str, arg: &str, conn: &mut SqliteConnection) -> sqlx::Result<i64> {
    sqlx::query_scalar(sql).bind(arg).fetch_one(conn).await
}

pub(super) async fn column(conn: &mut SqliteConnection, sql: &str, ws: &str, ids: &str) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar(sql).bind(ws).bind(ids).fetch_all(conn).await
}

/// Deletes the emails (of this workspace) and everything derived from them. Ids that no longer exist are skipped.
pub async fn delete_batch(conn: &mut SqliteConnection, ws: &str, ids: &[String], opts: Opts<'_>, prep: &Prepared) -> Result<Removed, DeleteError> {
    let ids_arg = ids_json(ids);
    let doomed: Vec<Doomed> = sqlx::query_as(
        "SELECT id, source_id, thread_id, sender_address FROM email_messages
         WHERE workspace_id = ?1 AND id IN (SELECT value FROM json_each(?2))",
    )
    .bind(ws)
    .bind(&ids_arg)
    .fetch_all(&mut *conn)
    .await?;
    if doomed.is_empty() {
        return Ok(Removed::default());
    }
    let ids: Vec<String> = doomed.iter().map(|d| d.id.clone()).collect();
    let ids_arg = ids_json(&ids);

    let busy: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM drafts WHERE message_id IN (SELECT value FROM json_each(?1)) AND status != 'sent' AND send_attempt_id IS NOT NULL",
    )
    .bind(&ids_arg)
    .fetch_one(&mut *conn)
    .await?;
    if busy > 0 {
        return Err(DeleteError::Busy);
    }

    let now = now_ms();
    let mut removed = Removed { emails: ids.len() as i64, ..Removed::default() };
    removed.attachments = scalar("SELECT COUNT(*) FROM attachments WHERE message_id IN (SELECT value FROM json_each(?1))", &ids_arg, conn).await?;
    removed.drafts = scalar("SELECT COUNT(*) FROM drafts WHERE message_id IN (SELECT value FROM json_each(?1))", &ids_arg, conn).await?;
    removed.summaries =
        scalar("SELECT COUNT(*) FROM message_insights WHERE message_id IN (SELECT value FROM json_each(?1))", &ids_arg, conn).await?;

    let draft_ids = column(
        conn,
        "SELECT id FROM drafts WHERE workspace_id = ?1 AND message_id IN (SELECT value FROM json_each(?2))",
        ws,
        &ids_arg,
    )
    .await?;
    let drafts_arg = ids_json(&draft_ids);

    if opts.tombstone {
        sqlx::query(
            "INSERT OR IGNORE INTO deleted_messages (workspace_id, source_id, provider_message_id, deleted_by, deleted_at)
             SELECT workspace_id, source_id, source_message_id, ?2, ?3 FROM email_messages
             WHERE workspace_id = ?1 AND id IN (SELECT value FROM json_each(?4))",
        )
        .bind(ws)
        .bind(actor_id(opts.actor))
        .bind(now)
        .bind(&ids_arg)
        .execute(&mut *conn)
        .await?;
    }

    removed.cases_cancelled = detach_cases(conn, ws, &ids, &ids_arg).await?;
    forget_learning(conn, ws, &ids, &prep.refs).await?;
    apply_preferences(conn, &prep.prefs).await?;
    sqlx::query(
        "DELETE FROM feedback_events WHERE workspace_id = ?1
           AND (message_id IN (SELECT value FROM json_each(?2)) OR draft_id IN (SELECT value FROM json_each(?3)))",
    )
    .bind(ws)
    .bind(&ids_arg)
    .bind(&drafts_arg)
    .execute(&mut *conn)
    .await?;

    removed.chunks = sqlx::query(
        "DELETE FROM knowledge_chunks WHERE workspace_id = ?1 AND (
           message_id IN (SELECT value FROM json_each(?2))
           OR (kind = 'email' AND ref_id IN (SELECT value FROM json_each(?2)))
           OR (kind = 'attachment' AND ref_id IN (SELECT id FROM attachments WHERE message_id IN (SELECT value FROM json_each(?2))))
           OR (kind = 'reply' AND ref_id IN (SELECT value FROM json_each(?3))))",
    )
    .bind(ws)
    .bind(&ids_arg)
    .bind(&drafts_arg)
    .execute(&mut *conn)
    .await?
    .rows_affected() as i64;

    clear_references(conn, ws, &ids_arg, prep).await?;
    apply_citations(conn, &prep.citations).await?;

    sqlx::query("DELETE FROM email_messages WHERE workspace_id = ?1 AND id IN (SELECT value FROM json_each(?2))")
        .bind(ws)
        .bind(&ids_arg)
        .execute(&mut *conn)
        .await?;

    let threads: BTreeSet<&str> = doomed.iter().map(|d| d.thread_id.as_str()).collect();
    for thread in threads {
        sqlx::query(
            "DELETE FROM email_threads WHERE id = ?1 AND workspace_id = ?2
               AND NOT EXISTS (SELECT 1 FROM email_messages WHERE thread_id = ?1)",
        )
        .bind(thread)
        .bind(ws)
        .execute(&mut *conn)
        .await?;
    }
    let senders: BTreeSet<&str> = doomed.iter().map(|d| d.sender_address.as_str()).collect();
    for sender in senders {
        refresh_contact(conn, ws, sender).await?;
    }

    if opts.audit {
        for d in &doomed {
            audit::record(
                &mut *conn,
                Entry {
                    workspace_id: ws,
                    actor: opts.actor,
                    action: "message.deleted",
                    entity_type: "message",
                    entity_id: &d.id,
                    message_id: Some(&d.id),
                    metadata: json!({ "source_id": d.source_id, "thread_id": d.thread_id }),
                },
            )
            .await?;
        }
    }
    Ok(removed)
}

/// Cases of these emails: open ones are cancelled (held sends released first), every one keeps its row with
/// the email link removed and what it read of the email blanked. Returns how many were cancelled.
async fn detach_cases(conn: &mut SqliteConnection, ws: &str, ids: &[String], ids_arg: &str) -> sqlx::Result<i64> {
    for id in ids {
        store::release_held_sends(conn, ws, None, Some(id)).await?;
    }
    let cases: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, status FROM cases WHERE workspace_id = ?1 AND message_id IN (SELECT value FROM json_each(?2))",
    )
    .bind(ws)
    .bind(ids_arg)
    .fetch_all(&mut *conn)
    .await?;
    let mut cancelled = 0;
    for (case_id, status) in &cases {
        if case::is_terminal(status) {
            continue;
        }
        if case::set_status(conn, ws, case_id, "cancelled").await? {
            cancelled += 1;
        }
        sqlx::query("UPDATE cases SET next_wake_at = NULL WHERE id = ?1").bind(case_id).execute(&mut *conn).await?;
        sqlx::query(
            "UPDATE agent_approvals SET status = 'declined', decided_at = ?2, note = ?3 WHERE case_id = ?1 AND status = 'pending'",
        )
        .bind(case_id)
        .bind(now_ms())
        .bind(CASE_NOTE)
        .execute(&mut *conn)
        .await?;
    }
    sqlx::query(
        "UPDATE case_steps SET input_json = 'null', output_json = 'null'
         WHERE case_id IN (SELECT id FROM cases WHERE workspace_id = ?1 AND message_id IN (SELECT value FROM json_each(?2)))",
    )
    .bind(ws)
    .bind(ids_arg)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "UPDATE agent_approvals SET body = '', input_json = '{}'
         WHERE action IN ('send_email', 'send_message')
           AND case_id IN (SELECT id FROM cases WHERE workspace_id = ?1 AND message_id IN (SELECT value FROM json_each(?2)))",
    )
    .bind(ws)
    .bind(ids_arg)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "UPDATE cases SET message_id = NULL, state_json = '{}', status_note = ?3, updated_at = ?4
         WHERE workspace_id = ?1 AND message_id IN (SELECT value FROM json_each(?2))",
    )
    .bind(ws)
    .bind(ids_arg)
    .bind(CASE_NOTE)
    .bind(now_ms())
    .execute(&mut *conn)
    .await?;
    Ok(cancelled)
}

pub(super) fn base_ref(evidence: &str) -> &str {
    evidence.split('#').next().unwrap_or(evidence)
}

/// Memories taught only by these emails go; ones also confirmed elsewhere (or typed by a person) lose just this source.
async fn forget_learning(conn: &mut SqliteConnection, ws: &str, ids: &[String], refs: &HashSet<String>) -> sqlx::Result<()> {
    let refs_arg = json!(refs.iter().collect::<Vec<_>>()).to_string();
    let ids_arg = ids_json(ids);
    let memories: Vec<(String, String, String, i64, f64, Option<String>)> = sqlx::query_as(
        "SELECT id, sources_json, source, pinned, confidence, evidence_message_id FROM memories
         WHERE workspace_id = ?1 AND (
           evidence_message_id IN (SELECT value FROM json_each(?2))
           OR EXISTS (SELECT 1 FROM json_each(memories.sources_json) j WHERE json_extract(j.value, '$.id') IN (SELECT value FROM json_each(?3))))",
    )
    .bind(ws)
    .bind(&ids_arg)
    .bind(&refs_arg)
    .fetch_all(&mut *conn)
    .await?;
    for (id, sources_json, source, pinned, confidence, evidence) in memories {
        let sources: Vec<Value> = serde_json::from_str(&sources_json).unwrap_or_default();
        let kept: Vec<Value> = sources
            .into_iter()
            .filter(|s| !s.get("id").and_then(Value::as_str).is_some_and(|i| refs.contains(base_ref(i))))
            .collect();
        let taught_only_by_these = kept.is_empty() && source == "learned" && pinned == 0 && confidence < CONFIRMED;
        if taught_only_by_these {
            sqlx::query("DELETE FROM memories WHERE id = ?1").bind(&id).execute(&mut *conn).await?;
        } else {
            let evidence = evidence.filter(|e| !ids.contains(e));
            sqlx::query("UPDATE memories SET sources_json = ?1, evidence_message_id = ?2 WHERE id = ?3")
                .bind(Value::Array(kept).to_string())
                .bind(evidence)
                .bind(&id)
                .execute(&mut *conn)
                .await?;
        }
    }
    Ok(())
}

/// Rows that name an email without a foreign key: blank the link (or the copied content), drop queued work.
async fn clear_references(conn: &mut SqliteConnection, ws: &str, ids_arg: &str, prep: &Prepared) -> sqlx::Result<()> {
    for sql in [
        "UPDATE ai_calls SET message_id = NULL WHERE workspace_id = ?1 AND message_id IN (SELECT value FROM json_each(?2))",
        "UPDATE ai_incidents SET message_id = NULL, details_json = '{}' WHERE workspace_id = ?1 AND message_id IN (SELECT value FROM json_each(?2))",
        "UPDATE watches SET answer_message_id = NULL, answer_text = NULL WHERE workspace_id = ?1 AND answer_message_id IN (SELECT value FROM json_each(?2))",
        "UPDATE support_tickets SET source_message_id = NULL WHERE workspace_id = ?1 AND source_message_id IN (SELECT value FROM json_each(?2))",
        "UPDATE submissions SET outcome_message_id = NULL WHERE workspace_id = ?1 AND outcome_message_id IN (SELECT value FROM json_each(?2))",
        "UPDATE inbound_events SET payload_json = '{}'
         WHERE id IN (SELECT json_extract(metadata_json, '$.event_id') FROM audit_events
                      WHERE workspace_id = ?1 AND action = 'message.received' AND message_id IN (SELECT value FROM json_each(?2)))",
    ] {
        sqlx::query(sql).bind(ws).bind(ids_arg).execute(&mut *conn).await?;
    }
    sqlx::query("DELETE FROM jobs WHERE status != 'running' AND id IN (SELECT value FROM json_each(?1))")
        .bind(ids_json(&prep.jobs))
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM assistant_plans WHERE used_at IS NULL AND id IN (SELECT value FROM json_each(?1))")
        .bind(ids_json(&prep.plans))
        .execute(&mut *conn)
        .await?;
    Ok(())
}
