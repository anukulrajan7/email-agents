use super::*;

#[derive(Debug, Clone)]
pub struct Mailbox {
    pub source_id: String,
    pub connection_id: String,
    pub config_key: String,
    pub address: Option<String>,
}

/// The mailbox a message goes out from: the person's own, or (for a workspace agent) the shared one.
pub async fn mailbox_for(state: &AppState, ws: &str, owner: Option<&str>) -> AppResult<Mailbox> {
    #[derive(sqlx::FromRow)]
    struct SourceRow {
        id: String,
        external_connection_id: Option<String>,
        provider_config_key: Option<String>,
        mailbox_address: Option<String>,
        capabilities_json: String,
    }
    let rows: Vec<SourceRow> = sqlx::query_as(
        "SELECT id, external_connection_id, provider_config_key, mailbox_address, capabilities_json FROM email_sources
         WHERE workspace_id = ?1 AND kind = 'nango_gmail' AND status = 'connected'
           AND ((?2 IS NOT NULL AND connected_by_user_id = ?2) OR (?2 IS NULL AND connected_by_user_id IS NULL))
         ORDER BY created_at, id",
    )
    .bind(ws)
    .bind(owner)
    .fetch_all(&state.db)
    .await?;
    for r in rows {
        let caps: Capabilities = serde_json::from_str(&r.capabilities_json).unwrap_or_else(|_| crate::mail::sources::capabilities("nango_gmail"));
        if let (Some(connection_id), Some(config_key)) = (r.external_connection_id, r.provider_config_key)
            && crate::ai::policy::can_send(&caps)
        {
            return Ok(Mailbox { source_id: r.id, connection_id, config_key, address: r.mailbox_address.filter(|a| !a.is_empty()) });
        }
    }
    Err(AppError::Conflict(if owner.is_some() {
        "Connect your own Gmail mailbox first: I send email from your mailbox.".into()
    } else {
        "No shared mailbox that can send is connected.".into()
    }))
}

/// The mailbox an email was stored against, if it is still connected and can send. Never any other mailbox.
pub async fn mailbox_by_source(state: &AppState, ws: &str, source_id: &str) -> AppResult<Mailbox> {
    type SourceRow = (Option<String>, Option<String>, Option<String>, String);
    let row: Option<SourceRow> = sqlx::query_as(
        "SELECT external_connection_id, provider_config_key, mailbox_address, capabilities_json FROM email_sources
         WHERE id = ?1 AND workspace_id = ?2 AND kind = 'nango_gmail' AND status = 'connected'",
    )
    .bind(source_id)
    .bind(ws)
    .fetch_optional(&state.db)
    .await?;
    if let Some((Some(connection_id), Some(config_key), address, caps)) = row {
        let caps: Capabilities = serde_json::from_str(&caps).unwrap_or_else(|_| crate::mail::sources::capabilities("nango_gmail"));
        if crate::ai::policy::can_send(&caps) {
            return Ok(Mailbox { source_id: source_id.to_string(), connection_id, config_key, address: address.filter(|a| !a.is_empty()) });
        }
    }
    Err(AppError::Conflict("The mailbox this email was written for is not connected any more.".into()))
}
