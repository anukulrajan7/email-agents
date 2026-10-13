use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::AppState;
use crate::{
    alerts::{self, ALERT_SELECT, AlertLine, AlertRow, AlertSettings},
    audit::{self, Actor, Entry},
    auth::{AuthUser, require_role},
    db::now_ms,
    domain::Role,
    error::{AppError, AppResult},
};

#[derive(Deserialize)]
pub struct ListQuery {
    status: Option<String>,
}

/// `status=open` (default) includes snoozed alerts; `open_count` counts only `open` ones.
pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    Path(ws): Path<String>,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let filter = match q.status.as_deref().unwrap_or("open") {
        "open" => "a.status IN ('open', 'snoozed')",
        "resolved" => "a.status = 'resolved'",
        "all" => "1 = 1",
        _ => return Err(AppError::BadRequest("status must be open, resolved or all".into())),
    };
    let rows: Vec<AlertRow> = sqlx::query_as(&format!(
        "{ALERT_SELECT} WHERE a.workspace_id = ?1 AND {filter}
         ORDER BY a.status = 'open' DESC, a.severity = 'high' DESC, a.created_at DESC LIMIT 200"
    ))
    .bind(&ws)
    .fetch_all(&state.db)
    .await?;
    let open_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM alerts WHERE workspace_id = ?1 AND status = 'open'")
        .bind(&ws)
        .fetch_one(&state.db)
        .await?;
    let items: Vec<Value> = rows.into_iter().map(alerts::alert_json).collect();
    Ok(Json(json!({ "items": items, "open_count": open_count })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    status: Option<String>,
    snooze_hours: Option<i64>,
}

/// `{status:'resolved'}` resolves; `{status:'open'}` reopens; `snooze_hours` (1-168) snoozes an unresolved alert.
pub async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
    Json(body): Json<Patch>,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let now = now_ms();
    let (sql, action) = match (body.status.as_deref(), body.snooze_hours) {
        (Some("resolved"), None) => (
            "UPDATE alerts SET status = 'resolved', resolved_at = ?1, resolved_by = ?2, snoozed_until = NULL, updated_at = ?1
             WHERE id = ?3 AND workspace_id = ?4 AND ?5 IS NULL",
            "alert.resolved",
        ),
        (Some("open") | None, Some(h)) if (1..=168).contains(&h) => (
            "UPDATE alerts SET status = 'snoozed', snoozed_until = ?1 + ?5 * 3600000, resolved_at = NULL, resolved_by = NULL,
                    updated_at = ?1
             WHERE id = ?3 AND workspace_id = ?4 AND ?2 IS NOT NULL",
            "alert.snoozed",
        ),
        (Some("open"), None) => (
            "UPDATE alerts SET status = 'open', snoozed_until = NULL, resolved_at = NULL, resolved_by = NULL, level_since = ?1,
                    updated_at = ?1
             WHERE id = ?3 AND workspace_id = ?4 AND ?2 IS NOT NULL AND ?5 IS NULL",
            "alert.reopened",
        ),
        (_, Some(_)) => return Err(AppError::BadRequest("snooze_hours must be 1-168 (with status open or omitted)".into())),
        _ => return Err(AppError::BadRequest("status must be resolved or open".into())),
    };
    let mut tx = state.db.begin().await?;
    let updated = sqlx::query(sql)
        .bind(now)
        .bind(&user.id)
        .bind(&id)
        .bind(&ws)
        .bind(body.snooze_hours)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if updated == 0 {
        return Err(AppError::NotFound);
    }
    let row: AlertRow = sqlx::query_as(&format!("{ALERT_SELECT} WHERE a.id = ?1")).bind(&id).fetch_one(&mut *tx).await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &ws,
            actor: Actor::User(&user.id),
            action,
            entity_type: "alert",
            entity_id: &id,
            message_id: row.message_id.as_deref(),
            metadata: json!({ "snooze_hours": body.snooze_hours }),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Json(alerts::alert_json(row)))
}

pub async fn get_settings(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<AlertSettings>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    Ok(Json(alerts::load_settings(&state.db, &ws).await?))
}

pub async fn put_settings(
    State(state): State<AppState>,
    user: AuthUser,
    Path(ws): Path<String>,
    Json(body): Json<AlertSettings>,
) -> AppResult<Json<AlertSettings>> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    let settings = body.validated().map_err(AppError::BadRequest)?;
    alerts::save_settings(&state.db, &ws, &settings).await?;
    let mut conn = state.db.acquire().await?;
    audit::record(
        &mut conn,
        Entry {
            workspace_id: &ws,
            actor: Actor::User(&user.id),
            action: "alert_settings.updated",
            entity_type: "workspace",
            entity_id: &ws,
            message_id: None,
            metadata: json!({ "enabled": settings.enabled, "recipients": settings.recipients.len(),
                              "escalate_to": settings.escalate_to.len() }),
        },
    )
    .await?;
    Ok(Json(settings))
}

/// Sends a sample alert email to the current recipients.
pub async fn test(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    let mailer = state.alert_mailer.clone().ok_or_else(|| AppError::Conflict("email sending isn't configured on this server".into()))?;
    let settings = alerts::load_settings(&state.db, &ws).await?;
    if settings.recipients.is_empty() {
        return Err(AppError::BadRequest("add at least one recipient first".into()));
    }
    let base = state.config.app_base_url.trim_end_matches('/').to_string();
    let name: String = sqlx::query_scalar("SELECT name FROM workspaces WHERE id = ?1").bind(&ws).fetch_one(&state.db).await?;
    let line = AlertLine {
        title: "Test alert: this is what an alert looks like".into(),
        body: "Nothing needs your attention. Alerts for unanswered or pending emails will arrive like this.".into(),
        link: alerts::deep_link(&base, &ws, None),
        level: 1,
    };
    let (subject, text, html) = alerts::render_email(&name, &base, &ws, &[line], None);
    let mut sent_to = vec![];
    for to in &settings.recipients {
        match mailer.send(to, &subject, &text, &html).await {
            Ok(()) => sent_to.push(to.clone()),
            Err(e) => tracing::warn!(error = %e, "test alert email failed"),
        }
    }
    if sent_to.is_empty() {
        return Err(AppError::Unavailable);
    }
    Ok(Json(json!({ "sent_to": sent_to })))
}
