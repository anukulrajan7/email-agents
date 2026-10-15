use super::*;

/// Everything the deletion needs to read from tables that can be big (jobs, assistant history, preferences).
/// Computed before the write lock is taken and applied by id inside it; a row that changed in between is skipped.
#[derive(Default)]
pub struct Prepared {
    /// What taught the agents something: the emails, their drafts, the approvals and feedback they produced.
    pub(super) refs: HashSet<String>,
    pub(super) jobs: Vec<String>,
    pub(super) plans: Vec<String>,
    pub(super) citations: Vec<(i64, String, String)>,
    pub(super) prefs: Vec<PrefChange>,
}

pub(super) enum PrefChange {
    Delete { id: String, old: String },
    Keep { id: String, old: String, kept: Vec<String> },
}

pub async fn prepare(db: &SqlitePool, ws: &str, ids: &[String]) -> sqlx::Result<Prepared> {
    let ids_arg = ids_json(ids);
    let mut conn = db.acquire().await?;
    let conn = &mut *conn;
    let draft_ids = column(conn, "SELECT id FROM drafts WHERE workspace_id = ?1 AND message_id IN (SELECT value FROM json_each(?2))", ws, &ids_arg).await?;
    let drafts_arg = ids_json(&draft_ids);
    let approval_ids = column(
        conn,
        "SELECT a.id FROM agent_approvals a JOIN cases c ON c.id = a.case_id
         WHERE a.workspace_id = ?1 AND c.message_id IN (SELECT value FROM json_each(?2))",
        ws,
        &ids_arg,
    )
    .await?;
    let feedback_ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM feedback_events WHERE workspace_id = ?1
           AND (message_id IN (SELECT value FROM json_each(?2)) OR draft_id IN (SELECT value FROM json_each(?3)))",
    )
    .bind(ws)
    .bind(&ids_arg)
    .bind(&drafts_arg)
    .fetch_all(&mut *conn)
    .await?;
    let refs: HashSet<String> = ids.iter().chain(&draft_ids).chain(&approval_ids).chain(&feedback_ids).cloned().collect();

    let jobs: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM jobs WHERE workspace_id = ?1 AND status != 'running'
           AND (json_extract(payload_json, '$.message_id') IN (SELECT value FROM json_each(?2))
                OR json_extract(payload_json, '$.draft_id') IN (SELECT value FROM json_each(?3)))",
    )
    .bind(ws)
    .bind(&ids_arg)
    .bind(&drafts_arg)
    .fetch_all(&mut *conn)
    .await?;
    let plans: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM assistant_plans WHERE workspace_id = ?1 AND used_at IS NULL
           AND EXISTS (SELECT 1 FROM json_each(?2) j WHERE instr(params_json, j.value) > 0)",
    )
    .bind(ws)
    .bind(&ids_arg)
    .fetch_all(&mut *conn)
    .await?;

    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, citations_json FROM assistant_messages
         WHERE workspace_id = ?1 AND citations_json != '[]'
           AND EXISTS (SELECT 1 FROM json_each(?2) j WHERE instr(citations_json, j.value) > 0)",
    )
    .bind(ws)
    .bind(&ids_arg)
    .fetch_all(&mut *conn)
    .await?;
    let mut citations = vec![];
    for (id, raw) in rows {
        let mut list: Vec<Value> = serde_json::from_str(&raw).unwrap_or_default();
        let mut changed = false;
        for c in &mut list {
            let cited = c.get("message_id").and_then(Value::as_str).is_some_and(|m| ids.iter().any(|i| i == m));
            if cited && let Some(o) = c.as_object_mut() {
                o.insert("subject".into(), json!(DELETED_SUBJECT));
                o.insert("deleted".into(), json!(true));
                o.remove("attachment_id");
                o.remove("attachment_name");
                changed = true;
            }
        }
        if changed {
            citations.push((id, raw, Value::Array(list).to_string()));
        }
    }

    let stored: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT id, evidence_json, source, status FROM agent_preferences WHERE workspace_id = ?1 AND evidence_json != '[]'",
    )
    .bind(ws)
    .fetch_all(&mut *conn)
    .await?;
    let mut prefs = vec![];
    for (id, old, source, status) in stored {
        let evidence: Vec<String> = serde_json::from_str(&old).unwrap_or_default();
        let kept: Vec<String> = evidence.iter().filter(|e| !refs.contains(base_ref(e))).cloned().collect();
        if kept.len() == evidence.len() {
            continue;
        }
        // Only a suggestion nobody approved disappears with its only evidence; an accepted one keeps working.
        if kept.is_empty() && source != "manual" && status == "suggested" {
            prefs.push(PrefChange::Delete { id, old });
        } else {
            prefs.push(PrefChange::Keep { id, old, kept });
        }
    }
    Ok(Prepared { refs, jobs, plans, citations, prefs })
}

pub(super) async fn apply_preferences(conn: &mut SqliteConnection, prefs: &[PrefChange]) -> sqlx::Result<()> {
    for p in prefs {
        match p {
            PrefChange::Delete { id, old } => {
                sqlx::query("DELETE FROM agent_preferences WHERE id = ?1 AND evidence_json = ?2 AND status = 'suggested'")
                    .bind(id)
                    .bind(old)
                    .execute(&mut *conn)
                    .await?;
            }
            PrefChange::Keep { id, old, kept } => {
                sqlx::query("UPDATE agent_preferences SET evidence_json = ?1, evidence_count = ?2 WHERE id = ?3 AND evidence_json = ?4")
                    .bind(Value::from(kept.clone()).to_string())
                    .bind(i64::try_from(kept.len()).unwrap_or(i64::MAX).max(1))
                    .bind(id)
                    .bind(old)
                    .execute(&mut *conn)
                    .await?;
            }
        }
    }
    Ok(())
}

pub(super) async fn apply_citations(conn: &mut SqliteConnection, citations: &[(i64, String, String)]) -> sqlx::Result<()> {
    for (id, old, new) in citations {
        sqlx::query("UPDATE assistant_messages SET citations_json = ?1 WHERE id = ?2 AND citations_json = ?3")
            .bind(new)
            .bind(id)
            .bind(old)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// A sender's contact card follows the mail that is left: counts recomputed, the card dropped when none remains.
pub(super) async fn refresh_contact(conn: &mut SqliteConnection, ws: &str, email: &str) -> sqlx::Result<()> {
    let (count, first, last): (i64, Option<i64>, Option<i64>) = sqlx::query_as(
        "SELECT COUNT(*), MIN(received_at), MAX(received_at) FROM email_messages
         WHERE workspace_id = ?1 AND sender_address = ?2 AND direction = 'inbound' AND suppressed = 0",
    )
    .bind(ws)
    .bind(email)
    .fetch_one(&mut *conn)
    .await?;
    if count == 0 {
        sqlx::query("DELETE FROM contacts WHERE workspace_id = ?1 AND email = ?2").bind(ws).bind(email).execute(&mut *conn).await?;
    } else {
        sqlx::query(
            "UPDATE contacts SET message_count = ?3, first_seen_at = ?4, last_seen_at = ?5 WHERE workspace_id = ?1 AND email = ?2",
        )
        .bind(ws)
        .bind(email)
        .bind(count)
        .bind(first)
        .bind(last)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// True when sync must not store this email: the user deleted it, or its mailbox is being purged.
pub async fn is_blocked(conn: &mut SqliteConnection, source_id: &str, provider_message_id: &str) -> sqlx::Result<bool> {
    let hit: Option<i64> = sqlx::query_scalar(
        "SELECT 1 WHERE EXISTS (SELECT 1 FROM deleted_messages WHERE source_id = ?1 AND provider_message_id = ?2)
            OR EXISTS (SELECT 1 FROM email_sources WHERE id = ?1 AND purge_status = 'running')",
    )
    .bind(source_id)
    .bind(provider_message_id)
    .fetch_optional(conn)
    .await?;
    Ok(hit.is_some())
}
