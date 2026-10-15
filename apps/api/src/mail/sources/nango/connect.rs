use super::*;

/// Verifies a connection against Nango (never trusting browser-supplied IDs), then
/// stores it as a source and schedules the initial backfill. `expected_*` are set when a
/// signed-in admin reports the connection; the webhook path relies on tags alone.
pub async fn register_connection(
    state: &AppState,
    expected_workspace: Option<&str>,
    expected_user: Option<&str>,
    connection_id: &str,
    provider_config_key: &str,
) -> Result<String, AppError> {
    if provider_config_key != state.config.nango_gmail_integration {
        return Err(AppError::BadRequest("unsupported integration".into()));
    }
    let still_purging: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM email_sources WHERE kind = ?1 AND external_connection_id = ?2 AND purge_status = 'running'",
    )
    .bind(GMAIL_KIND)
    .bind(connection_id)
    .fetch_optional(&state.db)
    .await?;
    if still_purging.is_some() {
        return Err(AppError::Conflict("Deleting the old data from this mailbox. Try again in a minute.".into()));
    }
    let conn = state.nango.get_connection(connection_id, provider_config_key).await?;
    let workspace_id = conn.tags.get("organization_id").cloned().ok_or(AppError::Forbidden)?;
    let owner = conn.tags.get("end_user_id").cloned().ok_or(AppError::Forbidden)?;
    if expected_workspace.is_some_and(|w| w != workspace_id) || expected_user.is_some_and(|u| u != owner) {
        return Err(AppError::Forbidden);
    }
    if conn.connection_id != connection_id || conn.provider_config_key != provider_config_key {
        return Err(AppError::Forbidden);
    }
    // The person who consented must (still) be an admin of the tagged workspace.
    let role: Option<String> = sqlx::query_scalar("SELECT role FROM memberships WHERE workspace_id = ?1 AND user_id = ?2")
        .bind(&workspace_id)
        .bind(&owner)
        .fetch_optional(&state.db)
        .await?;
    if role.as_deref() != Some("admin") {
        return Err(AppError::Forbidden);
    }

    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let purging: Option<String> = sqlx::query_scalar(
        "SELECT purge_status FROM email_sources WHERE kind = ?1 AND external_connection_id = ?2 AND purge_status = 'running'",
    )
    .bind(GMAIL_KIND)
    .bind(connection_id)
    .fetch_optional(&mut *tx)
    .await?;
    if purging.is_some() {
        return Err(AppError::Conflict("Deleting the old data from this mailbox. Try again in a minute.".into()));
    }
    sqlx::query(
        "INSERT INTO email_sources
           (id, workspace_id, kind, status, external_connection_id, provider_config_key, capabilities_json,
            connected_by_user_id, created_at)
         VALUES (?1, ?2, ?3, 'connected', ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(kind, external_connection_id) DO UPDATE SET status = 'connected', last_error = NULL
         WHERE email_sources.workspace_id = excluded.workspace_id",
    )
    .bind(new_id("src"))
    .bind(&workspace_id)
    .bind(GMAIL_KIND)
    .bind(connection_id)
    .bind(provider_config_key)
    .bind(serde_json::to_string(&sources::capabilities(GMAIL_KIND)).expect("serializable"))
    .bind(&owner)
    .bind(now_ms())
    .execute(&mut *tx)
    .await?;
    let (source_id, owner_ws): (String, String) =
        sqlx::query_as("SELECT id, workspace_id FROM email_sources WHERE kind = ?1 AND external_connection_id = ?2")
            .bind(GMAIL_KIND)
            .bind(connection_id)
            .fetch_one(&mut *tx)
            .await?;
    if owner_ws != workspace_id {
        return Err(AppError::Forbidden);
    }
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &workspace_id,
            actor: Actor::User(&owner),
            action: "source.connected",
            entity_type: "email_source",
            entity_id: &source_id,
            message_id: None,
            metadata: json!({ "kind": GMAIL_KIND }),
        },
    )
    .await?;
    enqueue_sync(&mut tx, &workspace_id, &source_id, "connected").await?;
    tx.commit().await?;
    state.jobs_notify.notify_one();
    Ok(source_id)
}

pub async fn enqueue_sync(
    conn: &mut sqlx::SqliteConnection,
    workspace_id: &str,
    source_id: &str,
    reason: &str,
) -> sqlx::Result<bool> {
    // One pending sync per source per reason-bucket; a running sync picks up everything anyway.
    let bucket = now_ms() / 60_000;
    jobs::enqueue_with_attempts(
        conn,
        kind::SYNC_NANGO_SOURCE,
        &format!("sync:{source_id}:{reason}:{bucket}"),
        Some(workspace_id),
        &json!({ "source_id": source_id }),
        jobs::PROVIDER_MAX_ATTEMPTS,
    )
    .await
}


/// Background registration from an `auth` webhook (no signed-in user available).
pub async fn register_from_webhook(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let connection_id = payload.get("connection_id").and_then(Value::as_str).unwrap_or_default();
    let pck = payload.get("provider_config_key").and_then(Value::as_str).unwrap_or_default();
    match register_connection(state, None, None, connection_id, pck).await {
        Ok(_) => Ok(()),
        Err(AppError::Unavailable | AppError::Internal(_)) => Err(JobError::Retry("registration failed".into())),
        Err(e) => {
            tracing::warn!(error = %e, "nango connection from webhook rejected");
            Ok(())
        }
    }
}
