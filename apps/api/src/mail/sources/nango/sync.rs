use super::*;

#[derive(sqlx::FromRow)]
struct SourceForSync {
    workspace_id: String,
    status: String,
    external_connection_id: Option<String>,
    provider_config_key: Option<String>,
    sync_cursor: Option<String>,
    backfilled_at: Option<i64>,
}

async fn mark_source(state: &AppState, source_id: &str, status: &str, error: Option<&str>) -> sqlx::Result<()> {
    sqlx::query("UPDATE email_sources SET status = ?1, last_error = ?2 WHERE id = ?3 AND status != 'disconnected'")
        .bind(status)
        .bind(error)
        .bind(source_id)
        .execute(&state.db)
        .await?;
    Ok(())
}

pub async fn sync_source(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let source_id = payload
        .get("source_id")
        .and_then(Value::as_str)
        .ok_or_else(|| JobError::Permanent("payload missing source_id".into()))?
        .to_string();
    let src: SourceForSync = sqlx::query_as(
        "SELECT workspace_id, status, external_connection_id, provider_config_key, sync_cursor, backfilled_at
         FROM email_sources WHERE id = ?1 AND kind = ?2",
    )
    .bind(&source_id)
    .bind(GMAIL_KIND)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| JobError::Permanent("source not found".into()))?;
    if src.status == "disconnected" {
        return Ok(());
    }
    let (Some(connection_id), Some(pck)) = (src.external_connection_id, src.provider_config_key) else {
        return Err(JobError::Permanent("source has no Nango connection".into()));
    };

    // Learn the mailbox's own address first, so mail it sent is stored as outbound.
    jobs::message_jobs::learn_mailbox_address(state, &source_id, &connection_id, &pck).await;

    // One-time direct backfill from Gmail (the catalog sync only keeps ~100 recent).
    if src.backfilled_at.is_none() {
        match backfill_from_gmail(state, &source_id, &src.workspace_id, &connection_id, &pck).await {
            Ok(n) => tracing::info!(%source_id, imported_or_refreshed = n, "gmail backfill finished"),
            Err(e) => tracing::warn!(%source_id, error = %e, "gmail backfill failed; will retry next sync"),
        }
    } else {
        match incremental_from_gmail(state, &source_id, &src.workspace_id, &connection_id, &pck).await {
            Ok(n) => tracing::info!(%source_id, new = n, "gmail incremental pull finished"),
            Err(JobError::Permanent(e)) => {
                mark_source(state, &source_id, "degraded", Some(&e)).await?;
                return Err(JobError::Permanent(e));
            }
            Err(e) => tracing::warn!(%source_id, error = %e, "gmail incremental pull failed; Nango records still checked"),
        }
    }

    // Network first, outside any transaction: page through everything new.
    let initial = src.sync_cursor.is_none();
    let cutoff = now_ms() - state.config.nango_backfill_days * 86_400_000;
    let modified_after = iso(cutoff);
    let mut records: Vec<MappedRecord> = Vec::new();
    let mut page_cursor = src.sync_cursor.clone();
    let mut more_pages = false;
    for page_no in 0.. {
        if page_no == MAX_PAGES_PER_RUN {
            more_pages = true;
            break;
        }
        let page = match state
            .nango
            .list_records(&connection_id, &pck, &state.config.nango_gmail_model, page_cursor.as_deref(), Some(&modified_after))
            .await
        {
            Ok(p) => p,
            Err(e @ NangoError::Connection(_)) => {
                mark_source(state, &source_id, "degraded", Some(&e.to_string())).await?;
                return Err(e.into());
            }
            Err(e) => return Err(e.into()),
        };
        records.extend(page.records.iter().filter_map(|r| map_gmail_record(r, state.config.max_body_chars)));
        match page.next_cursor {
            Some(next) if !page.records.is_empty() => page_cursor = Some(next),
            _ => break,
        }
    }
    // Resume point = cursor of the last record seen (records arrive oldest-modified first).
    let new_cursor = records.iter().rev().find_map(|r| r.cursor.clone()).or(src.sync_cursor);

    // Deleted upstream: keep local history (reviews/audit reference it), just don't import.
    let mut candidates: Vec<MappedRecord> = records.into_iter().filter(|r| !r.deleted).collect();
    if initial {
        // Bounded backfill: newest N messages from the last D days.
        candidates.retain(|r| r.email.received_at >= cutoff);
        candidates.sort_by_key(|r| std::cmp::Reverse(r.email.received_at));
        candidates.truncate(state.config.nango_backfill_max);
    }

    // Skip messages we already have, then fetch full content for the new ones only.
    let ids: Vec<&str> = candidates.iter().map(|r| r.email.source_message_id.as_str()).collect();
    let existing: Vec<String> = sqlx::query_scalar(
        "SELECT source_message_id FROM email_messages WHERE source_id = ?1 AND source_message_id IN (SELECT value FROM json_each(?2))
         UNION SELECT provider_message_id FROM deleted_messages WHERE source_id = ?1 AND provider_message_id IN (SELECT value FROM json_each(?2))",
    )
    .bind(&source_id)
    .bind(json!(ids).to_string())
    .fetch_all(&state.db)
    .await?;
    let existing: std::collections::HashSet<String> = existing.into_iter().collect();
    candidates.retain(|r| !existing.contains(&r.email.source_message_id));
    enrich_with_full_messages(state, &connection_id, &pck, &mut candidates).await;

    let mut imported = 0usize;
    let chunks: Vec<&[MappedRecord]> = candidates.chunks(PERSIST_CHUNK).collect();
    let total_chunks = chunks.len().max(1);
    for i in 0..total_chunks {
        let chunk = chunks.get(i).copied().unwrap_or(&[]);
        let last = i + 1 == total_chunks;
        let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
        // Write first (takes the lock); the cursor only advances with the final chunk, so a
        // crash mid-way re-imports from the old cursor and dedupe absorbs the repeats.
        sqlx::query(
            "UPDATE email_sources SET last_synced_at = ?1, sync_cursor = CASE WHEN ?2 THEN ?3 ELSE sync_cursor END
             WHERE id = ?4",
        )
        .bind(now_ms())
        .bind(last)
        .bind(&new_cursor)
        .bind(&source_id)
        .execute(&mut *tx)
        .await?;
        for r in chunk {
            let created = ingest::persist_message(
                &mut tx,
                &source_id,
                &src.workspace_id,
                &r.email,
                r.thread_id.as_deref(),
                json!({ "provider": GMAIL_KIND }),
            )
            .await?;
            imported += usize::from(created.is_some());
        }
        tx.commit().await?;
    }
    mark_source(state, &source_id, "connected", None).await?;
    jobs::message_jobs::enqueue_html_refetch(state, &src.workspace_id, &source_id).await?;
    if more_pages {
        let mut conn = state.db.acquire().await?;
        enqueue_sync(&mut conn, &src.workspace_id, &source_id, "continue").await?;
    }
    tracing::info!(%source_id, imported, initial, "nango sync finished");
    state.jobs_notify.notify_one();
    Ok(())
}

const ENRICH_CONCURRENCY: usize = 8;

/// One-time: the last N days of mail, straight from Gmail. Marks the source backfilled.
async fn backfill_from_gmail(
    state: &AppState,
    source_id: &str,
    workspace_id: &str,
    connection_id: &str,
    pck: &str,
) -> Result<usize, JobError> {
    let q = format!("newer_than:{}d -in:chats", state.config.nango_backfill_days);
    let changed =
        pull_from_gmail(state, source_id, workspace_id, connection_id, pck, &q, state.config.nango_backfill_max, "backfill").await?;
    sqlx::query("UPDATE email_sources SET backfilled_at = ?1 WHERE id = ?2").bind(now_ms()).bind(source_id).execute(&state.db).await?;
    Ok(changed)
}

/// Every sync: mail newer than the newest stored message (minus 1 h overlap; dedupe
/// absorbs repeats). Independent of Nango's hourly template sync.
async fn incremental_from_gmail(
    state: &AppState,
    source_id: &str,
    workspace_id: &str,
    connection_id: &str,
    pck: &str,
) -> Result<usize, JobError> {
    let newest: Option<i64> = sqlx::query_scalar("SELECT MAX(received_at) FROM email_messages WHERE source_id = ?1")
        .bind(source_id)
        .fetch_one(&state.db)
        .await?;
    let since_secs = newest.map(|ms| ms / 1000 - 3600).unwrap_or_else(|| now_ms() / 1000 - 86_400);
    let q = format!("after:{since_secs} -in:chats");
    pull_from_gmail(state, source_id, workspace_id, connection_id, pck, &q, 200, "incremental").await
}

/// Lists Gmail message ids matching `q`, fetches the new ones in full and stores them;
/// already-stored preview-only messages are upgraded. Returns how many changed.
#[allow(clippy::too_many_arguments)]
async fn pull_from_gmail(
    state: &AppState,
    source_id: &str,
    workspace_id: &str,
    connection_id: &str,
    pck: &str,
    q: &str,
    cap: usize,
    via: &str,
) -> Result<usize, JobError> {
    let mut ids: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let (page, next) = state.nango.gmail_list_ids(connection_id, pck, q, token.as_deref()).await?;
        ids.extend(page);
        if ids.len() >= cap || next.is_none() {
            break;
        }
        token = next;
    }
    ids.truncate(cap);
    // Skip what we already have with a full body (incremental runs re-list an hour of overlap).
    let known: Vec<String> = sqlx::query_scalar(
        "SELECT source_message_id FROM email_messages WHERE source_id = ?1 AND content_complete = 1
           AND source_message_id IN (SELECT value FROM json_each(?2))
         UNION SELECT provider_message_id FROM deleted_messages WHERE source_id = ?1 AND provider_message_id IN (SELECT value FROM json_each(?2))",
    )
    .bind(source_id)
    .bind(json!(ids).to_string())
    .fetch_all(&state.db)
    .await?;
    let known: std::collections::HashSet<String> = known.into_iter().collect();
    ids.retain(|id| !known.contains(id));

    // Reuse the enrichment path: start from bare records, fill from format=full.
    let mut records: Vec<MappedRecord> = ids
        .into_iter()
        .filter_map(|id| map_gmail_record(&json!({ "id": id }), state.config.max_body_chars))
        .collect();
    enrich_with_full_messages(state, connection_id, pck, &mut records).await;
    records.retain(|r| !r.email.sender_address.is_empty()); // fetch failed → skip, retry next time

    let mut changed = 0usize;
    for chunk in records.chunks(PERSIST_CHUNK) {
        let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query("UPDATE email_sources SET last_synced_at = ?1 WHERE id = ?2")
            .bind(now_ms())
            .bind(source_id)
            .execute(&mut *tx)
            .await?;
        for r in chunk {
            let created = ingest::persist_message(
                &mut tx,
                source_id,
                workspace_id,
                &r.email,
                r.thread_id.as_deref(),
                json!({ "provider": GMAIL_KIND, "via": via }),
            )
            .await?;
            if created.is_some() || ingest::refresh_content(&mut tx, source_id, &r.email).await?.is_some() {
                changed += 1;
            }
        }
        tx.commit().await?;
    }
    state.jobs_notify.notify_one();
    Ok(changed)
}

/// Fetches bodies + attachment metadata through Nango's proxy, 8 at a time, outside any
/// transaction. A failed fetch keeps the snippet rather than failing the whole sync.
async fn enrich_with_full_messages(state: &AppState, connection_id: &str, pck: &str, records: &mut [MappedRecord]) {
    let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(ENRICH_CONCURRENCY));
    let mut tasks = tokio::task::JoinSet::new();
    for (i, r) in records.iter().enumerate() {
        let (nango, permits) = (state.nango.clone(), permits.clone());
        let (conn, pck, id) = (connection_id.to_string(), pck.to_string(), r.email.source_message_id.clone());
        tasks.spawn(async move {
            let _permit = permits.acquire_owned().await.ok()?;
            match nango.gmail_message_full(&conn, &pck, &id).await {
                Ok(full) => Some((i, full)),
                Err(e) => {
                    tracing::warn!(error = %e, "could not fetch full Gmail message; keeping snippet");
                    None
                }
            }
        });
    }
    while let Some(done) = tasks.join_next().await {
        if let Ok(Some((i, full))) = done {
            // A bare record (backfill) gets everything from the full message: headers,
            // thread, date; a catalog record keeps its headers and gains body + attachments.
            if records[i].email.sender_address.is_empty()
                && let Some(mapped) = map_gmail_record(&full, state.config.max_body_chars)
            {
                let cursor = records[i].cursor.take();
                records[i] = MappedRecord { cursor, ..mapped };
            } else if let Some(payload) = full.get("payload") {
                apply_gmail_payload(&mut records[i].email, payload, state.config.max_body_chars);
                records[i].email.outbound |= has_sent_label(&full);
            }
        }
    }
}
