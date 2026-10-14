use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use serde_json::json;

use super::{
    AppState,
    dto::{Items, JobDto},
};
use crate::{
    audit::{self, Actor, Entry},
    auth::{AuthUser, require_role},
    domain::Role,
    error::{AppError, AppResult},
    jobs,
};

#[derive(Deserialize)]
pub struct ListQuery {
    status: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    Path(ws): Path<String>,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<Items<JobDto>>> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    let status = q.status.unwrap_or_else(|| "dead".into());
    if !matches!(status.as_str(), "pending" | "running" | "succeeded" | "dead") {
        return Err(AppError::BadRequest("unknown job status".into()));
    }
    let rows = jobs::list(&state.db, &ws, &status).await?;
    Ok(Json(Items { items: rows.into_iter().map(Into::into).collect() }))
}

pub async fn retry(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
) -> AppResult<Json<JobDto>> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let job = jobs::retry_dead(&mut tx, &ws, &id).await?.ok_or(AppError::NotFound)?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &ws,
            actor: Actor::User(&user.id),
            action: "job.retried",
            entity_type: "job",
            entity_id: &job.id,
            message_id: job.payload().get("message_id").and_then(|v| v.as_str()),
            metadata: json!({ "kind": job.kind }),
        },
    )
    .await?;
    tx.commit().await?;
    state.jobs_notify.notify_one();
    Ok(Json(job.into()))
}
