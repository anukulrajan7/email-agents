use super::*;

pub(super) async fn fetch_forwarded_email(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let event_id = str_field(payload, "event_id")?;
    let provider = str_field(payload, "provider")?;

    let email: ReceivedEmail = if provider == "test" {
        let raw: String = sqlx::query_scalar("SELECT payload_json FROM inbound_events WHERE id = ?1")
            .bind(&event_id)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| JobError::Permanent("inbound event missing".into()))?;
        serde_json::from_str(&raw).map_err(|e| JobError::Permanent(format!("bad test payload: {e}")))?
    } else {
        // Network call happens outside any transaction.
        let email_id = str_field(payload, "email_id")?;
        state.resend.get_received_email(&email_id).await.map_err(|e| match e {
            FetchError::Retryable(m) => JobError::Retry(m),
            FetchError::Permanent(m) => JobError::Permanent(m),
        })?
    };

    // Resolve the workspace only from a validated alias we issued; never from headers.
    let mut source: Option<(String, String)> = None;
    for addr in email.delivery_addresses() {
        source = sqlx::query_as(
            "SELECT id, workspace_id FROM email_sources
             WHERE kind = 'forwarding' AND inbound_alias = ?1 AND status != 'disconnected'",
        )
        .bind(&addr)
        .fetch_optional(&state.db)
        .await?;
        if source.is_some() {
            break;
        }
    }
    let Some((source_id, workspace_id)) = source else {
        tracing::warn!(%event_id, "inbound email for unknown alias rejected");
        sqlx::query("UPDATE inbound_events SET status = 'rejected' WHERE id = ?1")
            .bind(&event_id)
            .execute(&state.db)
            .await?;
        return Ok(());
    };

    let mut email_norm = email.normalize(state.config.max_body_chars);
    if email_norm.source_message_id.is_empty() {
        email_norm.source_message_id = payload.get("email_id").and_then(Value::as_str).unwrap_or(&event_id).to_string();
    }

    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    // First statement is a write, so the dedupe check in persist_message runs under the write lock.
    sqlx::query("UPDATE inbound_events SET status = 'processed' WHERE id = ?1").bind(&event_id).execute(&mut *tx).await?;
    let created = ingest::persist_message(&mut tx, &source_id, &workspace_id, &email_norm, None, json!({ "event_id": event_id }))
        .await?;
    if created.is_some() {
        sqlx::query(
            "UPDATE email_sources SET status = 'connected', last_synced_at = ?1 WHERE id = ?2 AND status != 'disconnected'",
        )
        .bind(now_ms())
        .bind(&source_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    state.jobs_notify.notify_one();
    Ok(())
}
