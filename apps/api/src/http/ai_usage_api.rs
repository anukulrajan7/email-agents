use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;

use super::AppState;
use crate::{
    ai_ledger::{self, AiUsage},
    auth::{AuthUser, require_role},
    domain::Role,
    error::AppResult,
};

#[derive(Deserialize)]
pub struct UsageQuery {
    days: Option<i64>,
}

pub async fn get(
    State(state): State<AppState>,
    user: AuthUser,
    Path(ws): Path<String>,
    Query(q): Query<UsageQuery>,
) -> AppResult<Json<AiUsage>> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    let usage = ai_ledger::usage(&state.db, &ws, q.days.unwrap_or(30), state.config.ai_daily_budget_usd).await?;
    Ok(Json(usage))
}
