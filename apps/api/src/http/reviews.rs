use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use serde_json::json;

use super::{
    AppState,
    dto::{Items, ReviewItem, ReviewRow},
};
use crate::{
    audit::{self, Actor, Entry},
    auth::{AuthUser, require_role},
    domain::Role,
    error::{AppError, AppResult},
};

const REVIEW_SELECT: &str = "SELECT r.id, r.kind, r.title, r.status, r.message_id, m.subject AS message_subject,
        r.assigned_user_id, r.created_at
 FROM review_items r JOIN email_messages m ON m.id = r.message_id";

#[derive(Deserialize)]
pub struct ListQuery {
    status: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    Path(ws): Path<String>,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<Items<ReviewItem>>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let status = q.status.filter(|s| s == "open" || s == "closed");
    let rows: Vec<ReviewRow> = sqlx::query_as(&format!(
        "{REVIEW_SELECT} WHERE r.workspace_id = ?1 AND (?2 IS NULL OR r.status = ?2) ORDER BY r.created_at DESC LIMIT 200"
    ))
    .bind(&ws)
    .bind(status)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(Items { items: rows.into_iter().map(Into::into).collect() }))
}

#[derive(Deserialize)]
pub struct Patch {
    status: String,
}

pub async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
    Json(body): Json<Patch>,
) -> AppResult<Json<ReviewItem>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    if body.status != "open" && body.status != "closed" {
        return Err(AppError::BadRequest("status must be open or closed".into()));
    }
    let mut tx = state.db.begin().await?;
    let message_id: String =
        sqlx::query_scalar("UPDATE review_items SET status = ?1 WHERE id = ?2 AND workspace_id = ?3 RETURNING message_id")
            .bind(&body.status)
            .bind(&id)
            .bind(&ws)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(AppError::NotFound)?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &ws,
            actor: Actor::User(&user.id),
            action: if body.status == "closed" { "review.closed" } else { "review.reopened" },
            entity_type: "review_item",
            entity_id: &id,
            message_id: Some(&message_id),
            metadata: json!({}),
        },
    )
    .await?;
    let row: ReviewRow = sqlx::query_as(&format!("{REVIEW_SELECT} WHERE r.id = ?1"))
        .bind(&id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(row.into()))
}
