use super::*;

/// Earlier messages of this email's conversation as a short digest (newest four, oldest first), or `None` when
/// the email starts the conversation.
pub(super) async fn thread_digest(state: &AppState, message_id: &str) -> Result<Option<String>, JobError> {
    let rows: Vec<(String, Option<String>, String, i64, String, String)> = sqlx::query_as(
        "SELECT p.sender_address, p.sender_name, p.direction, p.received_at, p.subject, p.body_text
         FROM email_messages m JOIN email_messages p ON p.thread_id = m.thread_id AND p.id != m.id AND p.received_at < m.received_at
         WHERE m.id = ?1 ORDER BY p.received_at DESC LIMIT 4",
    )
    .bind(message_id)
    .fetch_all(&state.db)
    .await?;
    let entries: Vec<ai::body::DigestEntry> = rows
        .into_iter()
        .rev()
        .map(|(address, name, direction, received_at_ms, subject, body)| ai::body::DigestEntry {
            who: if direction == "outbound" { "us".into() } else { name.filter(|n| !n.trim().is_empty()).unwrap_or(address) },
            received_at_ms,
            subject,
            body,
        })
        .collect();
    Ok(Some(ai::body::thread_digest(&entries, THREAD_DIGEST_CHARS)).filter(|d| !d.is_empty()))
}

const THREAD_DIGEST_CHARS: usize = 700;

/// Returns the stored insight, computing and storing it on first use (idempotent).
/// `summary, asks_json, deadline, needs_reply, missing_json, attachments_hash` of a stored insight.
type StoredInsight = (String, String, Option<String>, bool, String, Option<String>);

/// One analysis at a time per email in this process: a second caller waits, then finds the stored insight.
static ANALYSIS_LOCKS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>> =
    std::sync::OnceLock::new();

async fn lock_analysis(message_id: &str) -> tokio::sync::OwnedMutexGuard<()> {
    let lock = {
        let mut map = ANALYSIS_LOCKS.get_or_init(Default::default).lock().expect("analysis locks");
        if map.len() > 2_048 {
            map.retain(|_, l| std::sync::Arc::strong_count(l) > 1);
        }
        map.entry(message_id.to_string()).or_default().clone()
    };
    lock.lock_owned().await
}

pub async fn analyze_message(state: &AppState, payload: &Value) -> Result<ai::Insight, JobError> {
    let message_id = str_field(payload, "message_id")?;
    let _one_at_a_time = lock_analysis(&message_id).await;
    // Set when attachment text arrived after the first reading: read the email again with it, once per change.
    let refresh = payload.get("refresh_attachments").and_then(Value::as_bool).unwrap_or(false);
    let items = crate::mail::attachments::load_prompt_items(&state.db, &message_id).await?;
    let attachments_block = crate::mail::attachments::prompt_block(&items);
    let attachments_hash = (!attachments_block.is_empty()).then(|| format!("{:x}", Sha256::digest(attachments_block.as_bytes())));
    let stored: Option<StoredInsight> = sqlx::query_as(
        "SELECT summary, asks_json, deadline, needs_reply, missing_json, attachments_hash FROM message_insights WHERE message_id = ?1",
    )
    .bind(&message_id)
    .fetch_optional(&state.db)
    .await?;
    if let Some((summary, asks, deadline, needs_reply, missing, stored_hash)) = stored
        && (!refresh || attachments_hash.is_none() || stored_hash == attachments_hash)
    {
        return Ok(ai::Insight {
            summary,
            asks: serde_json::from_str(&asks).unwrap_or_default(),
            deadline,
            needs_reply,
            missing_info: serde_json::from_str(&missing).unwrap_or_default(),
        });
    }
    let msg = load_message(state, &message_id).await?;
    let requires_reply: Option<bool> = sqlx::query_scalar("SELECT requires_reply FROM classifications WHERE message_id = ?1")
        .bind(&message_id)
        .fetch_optional(&state.db)
        .await?;
    let new_part = ai::body::model_body(&msg.subject, &msg.body_text);
    let digest = thread_digest(state, &message_id).await?;

    // v2: the same single call that labels also reads; here only the reading is kept (the label is not touched).
    let mut understood: Option<(Understanding, String)> = None;
    if state.config.understand_mode == UnderstandMode::On && state.understander.available() {
        let labels = labels::active(&state.db, &msg.workspace_id).await?;
        let received_at_ms: i64 = sqlx::query_scalar("SELECT received_at FROM email_messages WHERE id = ?1").bind(&message_id).fetch_one(&state.db).await?;
        let ui = UnderstandInput {
            subject: msg.subject.clone(),
            sender: msg.sender_address.clone(),
            received_at_ms,
            new_part: new_part.clone(),
            attachments_block: attachments_block.clone(),
            attachment_names: items.iter().map(|(name, _, _)| name.clone()).collect(),
            labels,
            examples: vec![],
            sender_history: None,
            thread_digest: digest.clone(),
        };
        match state.understander.understand(&ui).await {
            Ok(r) => understood = Some(r),
            Err(e) => tracing::warn!(error = %e, %message_id, "understanding v2 failed: falling back to the analyst"),
        }
    }
    let (mut insight, model_id, understanding) = match understood {
        // One call decided needs_reply, so no tie-break judge.
        Some((u, model_id)) => (u.insight(), model_id, Some(u)),
        None => {
            let analyst_body = if attachments_block.is_empty() {
                new_part
            } else {
                format!("{}{attachments_block}", crate::mail::sources::normalize::truncate_chars(&new_part, ai::MAX_MODEL_INPUT_CHARS))
            };
            let (mut insight, model_id) =
                state.analyst.analyze_in_thread(&msg.subject, &msg.sender_address, &analyst_body, digest.as_deref()).await?;
            // Two independent signals must agree before we skip a reply; a cheap judge breaks real ties.
            let has_asks = !insight.asks.is_empty();
            let judge_says = if ai::judge::needs_reply_disputed(insight.needs_reply, requires_reply, has_asks) {
                let context = ai::judge::email_context(&msg.sender_address, &msg.subject, Some(&insight.summary), &msg.body_text);
                match state.judge.judge(&context, &[ai::judge::needs_reply_question()]).await {
                    Ok(a) => a.yes_no(ai::judge::NEEDS_REPLY),
                    Err(e) => {
                        tracing::debug!(error = %e, "needs-reply tie-break unavailable; keeping the rule");
                        None
                    }
                }
            } else {
                None
            };
            insight.needs_reply = ai::judge::settle_needs_reply(insight.needs_reply, requires_reply, has_asks, judge_says);
            (insight, model_id, None)
        }
    };
    insight.summary = crate::mail::attachments::strip_attachment_markup(&insight.summary).to_string();
    let received_at_ms: i64 = sqlx::query_scalar("SELECT received_at FROM email_messages WHERE id = ?1").bind(&message_id).fetch_one(&state.db).await?;
    let seen = format!("{}\n{}\n{attachments_block}", msg.subject, msg.body_text);
    insight.summary = crate::platform::format::summary::polish(&insight.summary, &seen, received_at_ms);
    let mut conn = state.db.acquire().await?;
    store_insight(&mut conn, &message_id, &insight, &model_id, attachments_hash.as_deref(), understanding.as_ref(), refresh).await?;
    Ok(insight)
}

/// Writes a message's insight. `overwrite` replaces an existing one (a refresh after attachment text, or a v2 reading
/// stored with its label); otherwise the first writer wins.
pub(super) async fn store_insight(
    conn: &mut sqlx::SqliteConnection,
    message_id: &str,
    insight: &ai::Insight,
    model_id: &str,
    attachments_hash: Option<&str>,
    understanding: Option<&Understanding>,
    overwrite: bool,
) -> sqlx::Result<()> {
    let source = if model_id == "rules-v1" { "rules" } else { "llm" };
    sqlx::query(
        "INSERT INTO message_insights (message_id, summary, asks_json, deadline, needs_reply, missing_json, source, model_id, created_at,
                                      attachments_hash, understanding_json, understanding_version)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?12, ?13)
         ON CONFLICT(message_id) DO UPDATE SET summary = excluded.summary, asks_json = excluded.asks_json,
           deadline = excluded.deadline, needs_reply = excluded.needs_reply, missing_json = excluded.missing_json,
           source = excluded.source, model_id = excluded.model_id, created_at = excluded.created_at,
           attachments_hash = excluded.attachments_hash, understanding_json = excluded.understanding_json,
           understanding_version = excluded.understanding_version
         WHERE ?11",
    )
    .bind(message_id)
    .bind(&insight.summary)
    .bind(json!(insight.asks).to_string())
    .bind(&insight.deadline)
    .bind(insight.needs_reply)
    .bind(json!(insight.missing_info).to_string())
    .bind(source)
    .bind(model_id)
    .bind(now_ms())
    .bind(attachments_hash)
    .bind(overwrite)
    .bind(understanding.map(|u| serde_json::to_string(u).unwrap_or_default()))
    .bind(understanding.map(|_| ai::understand::PROMPT_VERSION))
    .execute(conn)
    .await?;
    Ok(())
}
