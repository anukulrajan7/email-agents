use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::AppState;
use crate::{
    audit::{self, Actor, Entry},
    auth::{AuthUser, require_role},
    db::{iso, new_id, now_ms},
    domain::{LABEL_TEMPLATES, Role},
    error::{AppError, AppResult},
    jobs::matching_jobs::{CANDIDATE_LABEL, REQUIREMENT_LABEL},
};

/// Ensures the two labels matching depends on exist and are active.
pub async fn setup(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    if !crate::packs::has_feature(&state.db, &ws, crate::packs::FEATURE_MATCHING).await? {
        return Err(AppError::Conflict("matching is part of the HR & recruiting pack; turn that pack on first".into()));
    }
    let mut tx = state.db.begin().await?;
    for key in [REQUIREMENT_LABEL, CANDIDATE_LABEL] {
        let seed = LABEL_TEMPLATES
            .iter()
            .flat_map(|t| t.labels.iter())
            .find(|l| l.key == key)
            .ok_or_else(|| AppError::Internal(anyhow::anyhow!("missing seed {key}")))?;
        sqlx::query(
            "INSERT INTO labels (id, workspace_id, key, name, description, color, position, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, (SELECT COALESCE(MAX(position), 0) + 1 FROM labels WHERE workspace_id = ?2), ?7)
             ON CONFLICT(workspace_id, key) DO UPDATE SET archived = 0, archived_by_pack = 0",
        )
        .bind(new_id("lbl"))
        .bind(&ws)
        .bind(seed.key)
        .bind(seed.name)
        .bind(seed.description)
        .bind(seed.color)
        .bind(now_ms())
        .execute(&mut *tx)
        .await?;
    }
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &ws,
            actor: Actor::User(&user.id),
            action: "matching.set_up",
            entity_type: "workspace",
            entity_id: &ws,
            message_id: None,
            metadata: json!({}),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "labels": [REQUIREMENT_LABEL, CANDIDATE_LABEL] })))
}

fn skills(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| json!([]))
}

#[derive(sqlx::FromRow)]
struct ReqListRow {
    id: String,
    message_id: String,
    client: Option<String>,
    role_title: String,
    must_have_skills_json: String,
    nice_to_have_skills_json: String,
    min_experience: Option<f64>,
    max_experience: Option<f64>,
    location: Option<String>,
    positions: Option<i64>,
    status: String,
    source: String,
    created_at: i64,
    match_count: i64,
    top_score: Option<i64>,
}

pub async fn requirements(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let rows: Vec<ReqListRow> = sqlx::query_as(
        "SELECT r.id, r.message_id, r.client, r.role_title, r.must_have_skills_json, r.nice_to_have_skills_json,
                r.min_experience, r.max_experience, r.location, r.positions, r.status, r.source, r.created_at,
                (SELECT COUNT(*) FROM matches x WHERE x.requirement_id = r.id AND x.status != 'dismissed') AS match_count,
                (SELECT MAX(score) FROM matches x WHERE x.requirement_id = r.id AND x.status != 'dismissed') AS top_score
         FROM requirements r WHERE r.workspace_id = ?1 ORDER BY r.status, r.created_at DESC LIMIT 200",
    )
    .bind(&ws)
    .fetch_all(&state.db)
    .await?;
    let items: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            json!({
                "id": r.id, "message_id": r.message_id, "client": r.client, "role_title": r.role_title,
                "must_have_skills": skills(&r.must_have_skills_json), "nice_to_have_skills": skills(&r.nice_to_have_skills_json),
                "min_experience": r.min_experience, "max_experience": r.max_experience, "location": r.location,
                "positions": r.positions, "status": r.status, "source": r.source, "created_at": iso(r.created_at),
                "match_count": r.match_count, "top_score": r.top_score,
            })
        })
        .collect();
    Ok(Json(json!({ "items": items })))
}

#[derive(sqlx::FromRow)]
struct CandListRow {
    id: String,
    message_id: String,
    name: String,
    email: Option<String>,
    skills_json: String,
    total_experience: Option<f64>,
    location: Option<String>,
    notice_days: Option<i64>,
    referred_by: Option<String>,
    summary: Option<String>,
    status: String,
    source: String,
    created_at: i64,
    match_count: i64,
}

pub async fn candidates(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let rows: Vec<CandListRow> = sqlx::query_as(
        "SELECT c.id, c.message_id, c.name, c.email, c.skills_json, c.total_experience, c.location, c.notice_days,
                c.referred_by, c.summary, c.status, c.source, c.created_at,
                (SELECT COUNT(*) FROM matches x WHERE x.candidate_id = c.id AND x.status != 'dismissed') AS match_count
         FROM candidates c WHERE c.workspace_id = ?1 ORDER BY c.created_at DESC LIMIT 200",
    )
    .bind(&ws)
    .fetch_all(&state.db)
    .await?;
    let items: Vec<Value> = rows
        .into_iter()
        .map(|c| {
            json!({
                "id": c.id, "message_id": c.message_id, "name": c.name, "email": c.email, "skills": skills(&c.skills_json),
                "total_experience": c.total_experience, "location": c.location, "notice_days": c.notice_days,
                "referred_by": c.referred_by, "summary": c.summary, "status": c.status, "source": c.source,
                "created_at": iso(c.created_at), "match_count": c.match_count,
            })
        })
        .collect();
    Ok(Json(json!({ "items": items })))
}

#[derive(Deserialize)]
pub struct MatchQuery {
    requirement_id: Option<String>,
    candidate_id: Option<String>,
    status: Option<String>,
}

#[derive(sqlx::FromRow)]
struct MatchRow {
    id: String,
    score: i64,
    status: String,
    reasons_json: String,
    created_at: i64,
    updated_at: i64,
    requirement_id: String,
    role_title: String,
    client: Option<String>,
    req_location: Option<String>,
    candidate_id: String,
    name: String,
    total_experience: Option<f64>,
    cand_location: Option<String>,
    notice_days: Option<i64>,
}

const MATCH_SELECT: &str = "SELECT m.id, m.score, m.status, m.reasons_json, m.created_at, m.updated_at,
        r.id AS requirement_id, r.role_title, r.client, r.location AS req_location,
        c.id AS candidate_id, c.name, c.total_experience, c.location AS cand_location, c.notice_days
 FROM matches m JOIN requirements r ON r.id = m.requirement_id JOIN candidates c ON c.id = m.candidate_id";

fn match_json(m: MatchRow) -> Value {
    json!({
        "id": m.id, "score": m.score, "status": m.status,
        "reasons": serde_json::from_str::<Value>(&m.reasons_json).unwrap_or_else(|_| json!([])),
        "requirement": { "id": m.requirement_id, "role_title": m.role_title, "client": m.client, "location": m.req_location },
        "candidate": { "id": m.candidate_id, "name": m.name, "total_experience": m.total_experience,
                       "location": m.cand_location, "notice_days": m.notice_days },
        "created_at": iso(m.created_at), "updated_at": iso(m.updated_at),
    })
}

pub async fn matches(
    State(state): State<AppState>,
    user: AuthUser,
    Path(ws): Path<String>,
    Query(q): Query<MatchQuery>,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let status = q.status.filter(|s| matches!(s.as_str(), "suggested" | "shortlisted" | "dismissed"));
    let rows: Vec<MatchRow> = sqlx::query_as(&format!(
        "{MATCH_SELECT} WHERE m.workspace_id = ?1 AND (?2 IS NULL OR m.requirement_id = ?2) AND (?3 IS NULL OR m.candidate_id = ?3)
           AND (?4 IS NULL OR m.status = ?4) AND (?4 IS NOT NULL OR m.status != 'dismissed')
         ORDER BY m.score DESC, m.updated_at DESC LIMIT 200"
    ))
    .bind(&ws)
    .bind(q.requirement_id.filter(|s| !s.is_empty()))
    .bind(q.candidate_id.filter(|s| !s.is_empty()))
    .bind(status)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": rows.into_iter().map(match_json).collect::<Vec<_>>() })))
}

#[derive(Deserialize)]
pub struct MatchPatch {
    status: String,
}

pub async fn decide(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
    Json(body): Json<MatchPatch>,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    if !matches!(body.status.as_str(), "suggested" | "shortlisted" | "dismissed") {
        return Err(AppError::BadRequest("status must be suggested, shortlisted or dismissed".into()));
    }
    let mut tx = state.db.begin().await?;
    let updated = sqlx::query("UPDATE matches SET status = ?1, decided_by = ?2, updated_at = ?3 WHERE id = ?4 AND workspace_id = ?5")
        .bind(&body.status)
        .bind(&user.id)
        .bind(now_ms())
        .bind(&id)
        .bind(&ws)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if updated == 0 {
        return Err(AppError::NotFound);
    }
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &ws,
            actor: Actor::User(&user.id),
            action: "match.decided",
            entity_type: "match",
            entity_id: &id,
            message_id: None,
            metadata: json!({ "status": body.status }),
        },
    )
    .await?;
    let row: MatchRow = sqlx::query_as(&format!("{MATCH_SELECT} WHERE m.id = ?1")).bind(&id).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(match_json(row)))
}

#[derive(Deserialize)]
pub struct RequirementPatch {
    status: String,
}

pub async fn update_requirement(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
    Json(body): Json<RequirementPatch>,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    if !matches!(body.status.as_str(), "open" | "closed") {
        return Err(AppError::BadRequest("status must be open or closed".into()));
    }
    let n = sqlx::query("UPDATE requirements SET status = ?1 WHERE id = ?2 AND workspace_id = ?3")
        .bind(&body.status)
        .bind(&id)
        .bind(&ws)
        .execute(&state.db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(json!({ "id": id, "status": body.status })))
}
