use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    AppState,
    dto::{Items, Source, SourceRow},
};
use crate::{
    audit::{self, Actor, Entry},
    auth::{AuthUser, require_role},
    db::{iso, new_id, now_ms},
    domain::Role,
    error::{AppError, AppResult},
    jobs::{self, kind},
    sources::{
        self, nango,
        resend::{ReceivedAttachment, ReceivedEmail},
    },
};

/// Sample emails an admin may inject per workspace per day (`ALLOW_TEST_EMAILS`).
pub const TEST_EMAILS_PER_DAY: i64 = 20;

const SOURCE_COLUMNS: &str =
    "id, kind, status, inbound_alias, capabilities_json, last_synced_at, last_error, created_at, purge_status, purge_total, purge_done, purge_error";

pub async fn list(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Items<Source>>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let rows: Vec<SourceRow> =
        sqlx::query_as(&format!("SELECT {SOURCE_COLUMNS} FROM email_sources WHERE workspace_id = ?1 ORDER BY created_at"))
            .bind(&ws)
            .fetch_all(&state.db)
            .await?;
    Ok(Json(Items { items: rows.into_iter().map(Into::into).collect() }))
}

/// Idempotent: a workspace has at most one forwarding source (enforced by a unique index).
pub async fn create_forwarding(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Source>> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    let id = new_id("src");
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    // A forwarding address whose mail was purged is set up again on the same row (one per workspace), with a new alias.
    let revived = sqlx::query(
        "UPDATE email_sources SET status = 'pending', inbound_alias = ?2, purge_status = NULL, purge_total = NULL, purge_done = NULL,
                purge_counts_json = NULL, purge_started_at = NULL, purge_error = NULL, purged_at = NULL, last_error = NULL
         WHERE workspace_id = ?1 AND kind = 'forwarding' AND purge_status = 'done'",
    )
    .bind(&ws)
    .bind(sources::generate_alias(&state.config.inbound_domain))
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    let created = revived || sqlx::query(
        "INSERT OR IGNORE INTO email_sources (id, workspace_id, kind, status, inbound_alias, capabilities_json, created_at)
         VALUES (?1, ?2, 'forwarding', 'pending', ?3, ?4, ?5)",
    )
    .bind(&id)
    .bind(&ws)
    .bind(sources::generate_alias(&state.config.inbound_domain))
    .bind(serde_json::to_string(&sources::capabilities("forwarding")).expect("serializable"))
    .bind(now_ms())
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if created {
        audit::record(
            &mut tx,
            Entry {
                workspace_id: &ws,
                actor: Actor::User(&user.id),
                action: "source.created",
                entity_type: "email_source",
                entity_id: &id,
                message_id: None,
                metadata: json!({ "kind": "forwarding" }),
            },
        )
        .await?;
    }
    let row: SourceRow = sqlx::query_as(&format!(
        "SELECT {SOURCE_COLUMNS} FROM email_sources WHERE workspace_id = ?1 AND kind = 'forwarding'"
    ))
    .bind(&ws)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(row.into()))
}

#[derive(Deserialize, Default)]
pub struct TestEmail {
    from: Option<String>,
    subject: Option<String>,
    body_text: Option<String>,
}

/// Injects a sample email through the same pipeline a real forwarded email takes.
/// Disabled unless `ALLOW_TEST_EMAILS=true`.
pub async fn test_email(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, source_id)): Path<(String, String)>,
    body: Option<Json<TestEmail>>,
) -> AppResult<(StatusCode, Json<Value>)> {
    if !state.config.allow_test_emails {
        return Err(AppError::NotFound);
    }
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    let alias: Option<String> = sqlx::query_scalar(
        "SELECT inbound_alias FROM email_sources WHERE id = ?1 AND workspace_id = ?2 AND kind = 'forwarding'",
    )
    .bind(&source_id)
    .bind(&ws)
    .fetch_optional(&state.db)
    .await?
    .flatten();
    let alias = alias.ok_or(AppError::NotFound)?;
    if !crate::security::take(&state.db, "test_email", &ws, TEST_EMAILS_PER_DAY, 86_400_000).await? {
        return Err(AppError::RateLimited);
    }
    let body = body.map(|Json(b)| b).unwrap_or_default();
    // The sample attachment only accompanies the default timesheet sample.
    let attachments = if body.subject.is_none() {
        vec![ReceivedAttachment {
            filename: Some("timesheet-september.pdf".into()),
            content_type: Some("application/pdf".into()),
            size: Some(48_213),
        }]
    } else {
        vec![]
    };
    let email = ReceivedEmail {
        id: new_id("test"),
        from: body.from.unwrap_or_else(|| "Priya Sharma <priya.sharma@example.com>".into()),
        to: vec![alias],
        subject: Some(body.subject.unwrap_or_else(|| "September timesheet".into())),
        text: Some(body.body_text.unwrap_or_else(|| {
            "Hi team,\n\nPlease find attached my timesheet for September (168 hours). \
             Let me know if anything needs correcting.\n\nThanks,\nPriya"
                .into()
        })),
        created_at: Some(iso(now_ms())),
        attachments,
        ..Default::default()
    };
    let event_id = new_id("evt");
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query(
        "INSERT INTO inbound_events (id, provider, provider_event_id, payload_json, created_at) VALUES (?1, 'test', ?2, ?3, ?4)",
    )
    .bind(&event_id)
    .bind(&email.id)
    .bind(serde_json::to_string(&email).expect("serializable"))
    .bind(now_ms())
    .execute(&mut *tx)
    .await?;
    jobs::enqueue(
        &mut tx,
        kind::FETCH_FORWARDED_EMAIL,
        &format!("fetch:test:{}", email.id),
        None,
        &json!({ "event_id": event_id, "provider": "test" }),
    )
    .await?;
    tx.commit().await?;
    state.jobs_notify.notify_one();
    Ok((StatusCode::ACCEPTED, Json(json!({ "status": "queued" }))))
}

#[derive(Deserialize)]
pub struct NangoSessionBody {
    provider: String,
}

/// Short-lived Nango Connect session for the consent popup. Gmail only for now;
/// Microsoft 365 stays "not yet supported" until its POC passes.
pub async fn nango_session(
    State(state): State<AppState>,
    user: AuthUser,
    Path(ws): Path<String>,
    Json(body): Json<NangoSessionBody>,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    match body.provider.as_str() {
        "gmail" => {}
        "microsoft" => return Err(AppError::NotImplemented("Microsoft 365 is not yet supported".into())),
        _ => return Err(AppError::BadRequest("provider must be gmail or microsoft".into())),
    }
    let integration = &state.config.nango_gmail_integration;
    let session = state.nango.create_connect_session(&ws, &user.id, &user.email, integration).await?;
    Ok(Json(json!({
        "token": session.token,
        "expires_at": session.expires_at,
        "provider_config_key": integration,
    })))
}

#[derive(Deserialize)]
pub struct NangoConnectionBody {
    connection_id: String,
    provider_config_key: String,
}

/// Called by the browser after Nango Connect succeeds. The IDs are treated as claims:
/// the connection is fetched from Nango and its tags must match this workspace + user.
pub async fn nango_connection(
    State(state): State<AppState>,
    user: AuthUser,
    Path(ws): Path<String>,
    Json(body): Json<NangoConnectionBody>,
) -> AppResult<Json<Source>> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    let source_id = nango::register_connection(
        &state,
        Some(&ws),
        Some(&user.id),
        body.connection_id.trim(),
        body.provider_config_key.trim(),
    )
    .await?;
    let row: SourceRow = sqlx::query_as(&format!("SELECT {SOURCE_COLUMNS} FROM email_sources WHERE id = ?1 AND workspace_id = ?2"))
        .bind(&source_id)
        .bind(&ws)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(row.into()))
}

pub async fn sync(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, source_id)): Path<(String, String)>,
) -> AppResult<(StatusCode, Json<Value>)> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    let (kind, status): (String, String) =
        sqlx::query_as("SELECT kind, status FROM email_sources WHERE id = ?1 AND workspace_id = ?2")
            .bind(&source_id)
            .bind(&ws)
            .fetch_optional(&state.db)
            .await?
            .ok_or(AppError::NotFound)?;
    if kind == "forwarding" {
        return Err(AppError::Conflict("forwarding sources receive pushes and cannot be synced".into()));
    }
    if status == "disconnected" {
        return Err(AppError::Conflict("source is disconnected; reconnect it first".into()));
    }
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    // Unique per click: a second "Sync now" must never be silently swallowed.
    nango::enqueue_sync(&mut tx, &ws, &source_id, &format!("manual:{}", new_id("req"))).await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &ws,
            actor: Actor::User(&user.id),
            action: "source.sync_requested",
            entity_type: "email_source",
            entity_id: &source_id,
            message_id: None,
            metadata: json!({}),
        },
    )
    .await?;
    tx.commit().await?;
    state.jobs_notify.notify_one();
    Ok((StatusCode::ACCEPTED, Json(json!({ "status": "queued" }))))
}

#[derive(Deserialize, Default)]
pub struct DisconnectBody {
    /// Also delete everything that came from this mailbox, in the background. The web app sends it after the confirm dialog.
    #[serde(default)]
    purge: bool,
}

/// Exact numbers for the "this will be permanently deleted" confirm dialog.
pub async fn purge_preview(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, source_id)): Path<(String, String)>,
) -> AppResult<Json<Value>> {
    crate::deletion::authorize_source(&state.db, &ws, &user, &source_id).await?;
    let c = crate::deletion::source_counts(&state.db, &ws, &source_id).await?;
    let (scheduled_replies, open_tickets) = crate::deletion::source_loose_ends(&state.db, &ws, &source_id).await?;
    Ok(Json(json!({
        "emails": c.emails, "attachments": c.attachments, "drafts": c.drafts, "summaries": c.summaries,
        "scheduled_replies": scheduled_replies, "open_tickets": open_tickets,
    })))
}

/// Stops ingestion and revokes Nango access best-effort. With `purge`, a background job then deletes everything
/// from the mailbox (see `deletion`); without it, existing messages stay. The mailbox's owner or an admin only.
pub async fn disconnect(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, source_id)): Path<(String, String)>,
    body: Option<Json<DisconnectBody>>,
) -> AppResult<Json<Source>> {
    crate::deletion::authorize_source(&state.db, &ws, &user, &source_id).await?;
    let purge = body.is_some_and(|Json(b)| b.purge);
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let kind_of: String = sqlx::query_scalar(
        "UPDATE email_sources SET status = 'disconnected' WHERE id = ?1 AND workspace_id = ?2 RETURNING kind",
    )
    .bind(&source_id)
    .bind(&ws)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    // Cancel queued syncs for this source.
    sqlx::query("UPDATE jobs SET status = 'dead', last_error = 'source disconnected' WHERE status = 'pending' AND kind = ?1 AND json_extract(payload_json, '$.source_id') = ?2")
        .bind(kind::SYNC_NANGO_SOURCE)
        .bind(&source_id)
        .execute(&mut *tx)
        .await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &ws,
            actor: Actor::User(&user.id),
            action: "source.disconnected",
            entity_type: "email_source",
            entity_id: &source_id,
            message_id: None,
            metadata: json!({ "kind": kind_of, "purge": purge }),
        },
    )
    .await?;
    let conn: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT external_connection_id, provider_config_key FROM email_sources WHERE id = ?1 AND workspace_id = ?2")
            .bind(&source_id)
            .bind(&ws)
            .fetch_optional(&mut *tx)
            .await?;
    if purge {
        crate::deletion::start_purge(&mut tx, &ws, &source_id, &user.id).await?;
    }
    let row: SourceRow =
        sqlx::query_as(&format!("SELECT {SOURCE_COLUMNS} FROM email_sources WHERE id = ?1 AND workspace_id = ?2"))
            .bind(&source_id)
            .bind(&ws)
            .fetch_one(&mut *tx)
            .await?;
    tx.commit().await?;
    if purge {
        state.jobs_notify.notify_one();
    }
    if let Some((Some(cid), Some(pck))) = conn
        && let Err(e) = state.nango.delete_connection(&cid, &pck).await
    {
        tracing::warn!(%source_id, error = %e, "failed to revoke Nango connection");
    }
    Ok(Json(row.into()))
}
