//! `GET /workspaces/{ws}/search?q=&limit=`: keyword + meaning search over this workspace's
//! emails and attachment text, one result per (kind, message).

use std::collections::{HashMap, HashSet};

use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::{Deserialize, Serialize};

use super::{AppState, dto::Items};
use crate::{
    auth::{AuthUser, require_role},
    domain::Role,
    error::AppResult,
    knowledge::{self, Scope},
    sources::normalize::truncate_chars,
};

const MAX_LIMIT: usize = 50;
const MAX_QUERY_CHARS: usize = 500;
const SNIPPET_CHARS: usize = 200;

#[derive(Deserialize)]
pub struct SearchQuery {
    q: Option<String>,
    limit: Option<usize>,
}

#[derive(Serialize)]
pub struct SearchItem {
    pub kind: String,
    pub message_id: String,
    pub subject: String,
    pub snippet: String,
    pub score: f64,
}

pub async fn search(
    State(state): State<AppState>,
    user: AuthUser,
    Path(ws): Path<String>,
    Query(params): Query<SearchQuery>,
) -> AppResult<Json<Items<SearchItem>>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let q = truncate_chars(params.q.unwrap_or_default().trim(), MAX_QUERY_CHARS);
    if q.is_empty() {
        return Ok(Json(Items { items: vec![] }));
    }
    let limit = params.limit.unwrap_or(10).clamp(1, MAX_LIMIT);
    let hits =
        knowledge::hybrid_search(&state.db, state.embedder.as_ref(), &ws, &q, Scope::Documents, (limit * 4).max(20)).await?;

    let mut seen = HashSet::new();
    let best: Vec<_> = hits
        .into_iter()
        .filter_map(|h| h.message_id.clone().map(|m| (m, h)))
        .filter(|(m, h)| seen.insert((h.kind.clone(), m.clone())))
        .take(limit)
        .collect();
    if best.is_empty() {
        return Ok(Json(Items { items: vec![] }));
    }
    let ids: Vec<&String> = best.iter().map(|(m, _)| m).collect::<HashSet<_>>().into_iter().collect();
    let sql = format!(
        "SELECT id, subject FROM email_messages WHERE workspace_id = ? AND id IN ({})",
        vec!["?"; ids.len()].join(",")
    );
    let mut query = sqlx::query_as::<_, (String, String)>(&sql).bind(&ws);
    for id in &ids {
        query = query.bind(*id);
    }
    let subjects: HashMap<String, String> = query.fetch_all(&state.db).await?.into_iter().collect();

    let items = best
        .into_iter()
        .filter_map(|(message_id, h)| {
            let subject = subjects.get(&message_id)?.clone();
            Some(SearchItem {
                snippet: knowledge::snippet(knowledge::chunk_body(&h.text), &q, SNIPPET_CHARS),
                kind: h.kind,
                message_id,
                subject,
                score: h.score,
            })
        })
        .collect();
    Ok(Json(Items { items }))
}
