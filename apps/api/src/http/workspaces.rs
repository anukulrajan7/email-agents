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
    auth::{AuthUser, require_role},
    config::AuthMode,
    db::{new_id, now_ms},
    domain::{Role, WORKFLOW_TEMPLATES},
    error::{AppError, AppResult},
};

async fn workspaces_for(state: &AppState, user: &AuthUser) -> AppResult<Vec<Workspace>> {
    let rows: Vec<WorkspaceRow> = sqlx::query_as(
        "SELECT w.id, w.name, m.role, w.created_at FROM workspaces w
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

#[derive(Deserialize)]
pub struct CreateWorkspace {
    name: String,
    /// Starting label set: "hr" (default), "general" or "blank".
    template: Option<String>,
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
    let template = crate::domain::label_template(body.template.as_deref().unwrap_or("hr"))
        .ok_or_else(|| AppError::BadRequest("template must be hr, general or blank".into()))?;
    let id = new_id("ws");
    let now = now_ms();
    let mut tx = state.db.begin().await?;
    sqlx::query("INSERT INTO workspaces (id, name, created_at) VALUES (?1, ?2, ?3)")
        .bind(&id)
        .bind(name)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO memberships (workspace_id, user_id, role) VALUES (?1, ?2, 'admin')")
        .bind(&id)
        .bind(&user.id)
        .execute(&mut *tx)
        .await?;
    crate::labels::seed(&mut tx, &id, template).await?;
    for t in WORKFLOW_TEMPLATES {
        // Enable a workflow only when its label exists in the chosen template.
        let has_label = template.labels.iter().any(|l| l.key == t.label_key);
        sqlx::query("INSERT INTO workflows (id, workspace_id, template_key, enabled, created_at) VALUES (?1, ?2, ?3, ?4, ?5)")
            .bind(new_id("wf"))
            .bind(&id)
            .bind(t.key)
            .bind(t.available && has_label)
            .bind(now)
            .execute(&mut *tx)
            .await?;
    }
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &id,
            actor: Actor::User(&user.id),
            action: "workspace.created",
            entity_type: "workspace",
            entity_id: &id,
            message_id: None,
            metadata: json!({ "template": template.key }),
        },
    )
    .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(Workspace { id, name: name.to_string(), role: Role::Admin.as_str().into(), created_at: crate::db::iso(now) }),
    ))
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
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Admin).await?;
    let role = Role::parse(&body.role).ok_or_else(|| AppError::BadRequest("role must be admin or operator".into()))?;
    let email = body.email.trim().to_lowercase();
    if email.is_empty() {
        return Err(AppError::BadRequest("email is required".into()));
    }
    let member_id: String = sqlx::query_scalar("SELECT id FROM users WHERE email = ?1 ORDER BY created_at LIMIT 1")
        .bind(&email)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::BadRequest("user must sign in once before being added".into()))?;
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
    Ok(Json(json!({ "user_id": member_id, "email": email, "role": role.as_str() })))
}
