use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    AppState,
    dto::{Items, Workspace, WorkspaceRow},
};
use crate::{
    audit::{self, Actor, Entry},
    auth::{AuthUser, VERIFIED_USER, require_role},
    config::AuthMode,
    db::{new_id, now_ms},
    domain::{OTHER_SEED, Role},
    error::{AppError, AppResult},
    packs,
};

async fn workspaces_for(state: &AppState, user: &AuthUser) -> AppResult<Vec<Workspace>> {
    let rows: Vec<WorkspaceRow> = sqlx::query_as(
        "SELECT w.id, w.name, m.role, w.created_at,
                (SELECT GROUP_CONCAT(p.pack_key) FROM workspace_packs p WHERE p.workspace_id = w.id) AS packs
         FROM workspaces w
         JOIN memberships m ON m.workspace_id = w.id
         WHERE m.user_id = ?1 ORDER BY w.created_at",
    )
    .bind(&user.id)
    .fetch_all(&state.db)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn me(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Value>> {
    let workspaces = workspaces_for(&state, &user).await?;
    let auth_mode = match state.auth.mode() {
        AuthMode::Dev => "dev",
        AuthMode::Password => "password",
        AuthMode::Oidc => "oidc",
    };
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM users WHERE id = ?1")
        .bind(&user.id)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(json!({
        "user": { "id": user.id, "email": user.email, "name": name },
        "workspaces": workspaces,
        "auth_mode": auth_mode,
    })))
}

pub async fn list(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Items<Workspace>>> {
    Ok(Json(Items { items: workspaces_for(&state, &user).await? }))
}

/// Workspaces one person may administer (each one seeds labels, workflows and automations).
pub const MAX_WORKSPACES_PER_USER: i64 = 5;

#[derive(Deserialize)]
pub struct CreateWorkspace {
    name: String,
    /// Packs to turn on (see `GET /packs`). Default: `["general"]`.
    packs: Option<Vec<String>>,
    /// What the workspace is for, in the person's own words.
    goal: Option<String>,
    /// Legacy starting label set: "hr" (= staffing pack), "general" or "blank" (general pack, only `other`).
    template: Option<String>,
}

const MAX_GOAL_CHARS: usize = 1_000;

/// Validated, deduplicated pack keys in catalogue order.
fn parse_packs(keys: &[String]) -> AppResult<Vec<&'static str>> {
    if let Some(bad) = keys.iter().find(|k| packs::pack(k).is_none()) {
        return Err(AppError::BadRequest(format!("unknown pack \"{bad}\"")));
    }
    let chosen: Vec<&'static str> = packs::PACKS.iter().map(|p| p.key).filter(|k| keys.iter().any(|c| c == k)).collect();
    if chosen.is_empty() {
        return Err(AppError::BadRequest("choose at least one pack".into()));
    }
    Ok(chosen)
}

pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateWorkspace>,
) -> AppResult<(StatusCode, Json<Workspace>)> {
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(AppError::BadRequest("name must be 1-100 characters".into()));
    }
    let goal = body.goal.as_deref().map(str::trim).filter(|g| !g.is_empty());
    if goal.is_some_and(|g| g.chars().count() > MAX_GOAL_CHARS) {
        return Err(AppError::BadRequest(format!("goal can be at most {MAX_GOAL_CHARS} characters")));
    }
    let (chosen, blank) = match (&body.packs, body.template.as_deref()) {
        (Some(keys), _) => (parse_packs(keys)?, false),
        (None, Some(t)) => {
            let key = packs::from_template(t).ok_or_else(|| AppError::BadRequest("template must be hr, general or blank".into()))?;
            (vec![key], t == "blank")
        }
        (None, None) => (vec![packs::GENERAL], false),
    };
    let seeds = if blank { vec![&OTHER_SEED] } else { packs::labels_for(&chosen) };
    let id = new_id("ws");
    let now = now_ms();
    let mut tx = state.db.begin().await?;
    // Write first so SQLite takes the write lock: the cap check below can't race another create.
    sqlx::query("INSERT INTO workspaces (id, name, created_at, goal) VALUES (?1, ?2, ?3, ?4)")
        .bind(&id)
        .bind(name)
        .bind(now)
        .bind(goal)
        .execute(&mut *tx)
        .await?;
    let administered: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM memberships WHERE user_id = ?1 AND role = 'admin'")
        .bind(&user.id)
        .fetch_one(&mut *tx)
        .await?;
    if administered >= MAX_WORKSPACES_PER_USER {
        return Err(AppError::Denied(format!("You can have at most {MAX_WORKSPACES_PER_USER} workspaces")));
    }
    sqlx::query("INSERT INTO memberships (workspace_id, user_id, role) VALUES (?1, ?2, 'admin')")
        .bind(&id)
        .bind(&user.id)
        .execute(&mut *tx)
        .await?;
    packs::seed_labels(&mut tx, &id, &seeds).await?;
    packs::enable(&mut tx, &id, &chosen).await?;
    crate::automations::seed_defaults(&mut tx, &id).await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &id,
            actor: Actor::User(&user.id),
            action: "workspace.created",
            entity_type: "workspace",
            entity_id: &id,
            message_id: None,
            metadata: json!({ "packs": chosen, "template": body.template, "has_goal": goal.is_some() }),
        },
    )
    .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(Workspace {
            id,
            name: name.to_string(),
            role: Role::Admin.as_str().into(),
            created_at: crate::db::iso(now),
            packs: chosen.iter().map(|k| k.to_string()).collect(),
        }),
    ))
}

/// The pack catalogue.
pub async fn pack_catalogue(_user: AuthUser) -> Json<Value> {
    Json(json!({ "items": packs::PACKS.iter().map(packs::catalogue_json).collect::<Vec<_>>() }))
}

async fn packs_view(state: &AppState, ws: &str) -> AppResult<Json<Value>> {
    let enabled = packs::enabled_keys(&state.db, ws).await?;
    let items: Vec<Value> = packs::PACKS
        .iter()
        .map(|p| {
            let mut v = packs::catalogue_json(p);
            v["enabled"] = json!(enabled.contains(&p.key));
            v
        })
        .collect();
    Ok(Json(json!({ "enabled": enabled, "items": items })))
}

pub async fn get_packs(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    packs_view(&state, &ws).await
}

#[derive(Deserialize)]
pub struct PutPacks {
    packs: Vec<String>,
}

/// Sets the enabled packs. Enabling seeds the pack's labels (and staffing's workflows) without
/// touching existing or archived labels; disabling keeps all data and only gates features.
pub async fn put_packs(
    State(state): State<AppState>,
    user: AuthUser,
    Path(ws): Path<String>,
    Json(body): Json<PutPacks>,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    let chosen = parse_packs(&body.packs)?;
    // Read inside the transaction: a concurrent save makes this one fail instead of diffing a stale list.
    let mut tx = state.db.begin().await?;
    let before = packs::enabled_keys(&mut *tx, &ws).await?;
    let added: Vec<&str> = chosen.iter().copied().filter(|k| !before.contains(k)).collect();
    let removed: Vec<&str> = before.iter().copied().filter(|k| !chosen.contains(k)).collect();
    let seeds = packs::labels_for(&added);
    // Record the implicit default first (a workspace without rows counts as general).
    packs::enable(&mut tx, &ws, &before).await?;
    if !added.is_empty() {
        let active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM labels WHERE workspace_id = ?1 AND archived = 0")
            .bind(&ws)
            .fetch_one(&mut *tx)
            .await?;
        let new = packs::missing_labels(&mut tx, &ws, &seeds).await? as i64;
        if active + new > crate::labels::MAX_ACTIVE_LABELS {
            return Err(AppError::Conflict(format!(
                "these packs add {new} labels, but a workspace can have at most {} active labels",
                crate::labels::MAX_ACTIVE_LABELS
            )));
        }
        packs::seed_labels(&mut tx, &ws, &seeds).await?;
    }
    packs::enable(&mut tx, &ws, &added).await?;
    packs::disable(&mut tx, &ws, &removed).await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &ws,
            actor: Actor::User(&user.id),
            action: "packs.updated",
            entity_type: "workspace",
            entity_id: &ws,
            message_id: None,
            metadata: json!({ "enabled": added, "disabled": removed }),
        },
    )
    .await?;
    tx.commit().await?;
    packs_view(&state, &ws).await
}

#[derive(Deserialize)]
pub struct AddMember {
    email: String,
    role: String,
}

pub async fn add_member(
    State(state): State<AppState>,
    user: AuthUser,
    Path(ws): Path<String>,
    Json(body): Json<AddMember>,
) -> AppResult<(StatusCode, Json<Value>)> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    let role = Role::parse(&body.role).ok_or_else(|| AppError::BadRequest("role must be admin or operator".into()))?;
    let email = body.email.trim().to_lowercase();
    if email.is_empty() {
        return Err(AppError::BadRequest("email is required".into()));
    }
    if email.len() > 254 || !email.split_once('@').is_some_and(|(l, d)| !l.is_empty() && d.contains('.')) {
        return Err(AppError::BadRequest("enter a valid email address".into()));
    }
    // Only an account that proved it owns the address joins directly. Anyone else (no account,
    // or an unverified sign-up that might be a squatter) gets a pending invitation that turns
    // into a membership when the real owner signs up and verifies.
    let member_id: Option<String> = sqlx::query_scalar(&format!(
        "SELECT u.id FROM users u WHERE u.email = ?1 AND {VERIFIED_USER} ORDER BY u.created_at LIMIT 1"
    ))
    .bind(&email)
    .fetch_optional(&state.db)
    .await?;
    let Some(member_id) = member_id else {
        let mut tx = state.db.begin().await?;
        sqlx::query(
            "INSERT INTO invitations (workspace_id, email, role, invited_by, created_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(workspace_id, email) DO UPDATE SET role = excluded.role, invited_by = excluded.invited_by",
        )
        .bind(&ws)
        .bind(&email)
        .bind(role.as_str())
        .bind(&user.id)
        .bind(now_ms())
        .execute(&mut *tx)
        .await?;
        audit::record(
            &mut tx,
            Entry {
                workspace_id: &ws,
                actor: Actor::User(&user.id),
                action: "member.invited",
                entity_type: "invitation",
                entity_id: &email,
                message_id: None,
                metadata: json!({ "role": role.as_str() }),
            },
        )
        .await?;
        tx.commit().await?;
        return Ok((StatusCode::ACCEPTED, Json(json!({ "invited": true, "email": email, "role": role.as_str() }))));
    };
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "INSERT INTO memberships (workspace_id, user_id, role) VALUES (?1, ?2, ?3)
         ON CONFLICT(workspace_id, user_id) DO UPDATE SET role = excluded.role",
    )
    .bind(&ws)
    .bind(&member_id)
    .bind(role.as_str())
    .execute(&mut *tx)
    .await?;
    let admins: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM memberships WHERE workspace_id = ?1 AND role = 'admin'")
        .bind(&ws)
        .fetch_one(&mut *tx)
        .await?;
    if admins == 0 {
        tx.rollback().await?;
        return Err(AppError::Conflict("a workspace must keep at least one admin".into()));
    }
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &ws,
            actor: Actor::User(&user.id),
            action: "member.upserted",
            entity_type: "user",
            entity_id: &member_id,
            message_id: None,
            metadata: json!({ "role": role.as_str() }),
        },
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::OK, Json(json!({ "user_id": member_id, "email": email, "role": role.as_str() }))))
}
