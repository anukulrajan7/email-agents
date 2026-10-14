//! Teams: a name and a list of workspace members. Messaging a team messages each member.

use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::{
    audit::{self, Actor, Entry},
    db::{new_id, now_ms},
    error::{AppError, AppResult},
    http::AppState,
};

pub const MAX_TEAMS: i64 = 50;
pub const MAX_MEMBERS: usize = 50;

pub async fn list(db: &SqlitePool, ws: &str) -> sqlx::Result<Vec<Value>> {
    let teams: Vec<(String, String, Option<String>)> = sqlx::query_as("SELECT id, name, desk_agent_id FROM teams WHERE workspace_id = ?1 ORDER BY lower(name)")
        .bind(ws)
        .fetch_all(db)
        .await?;
    let mut out = vec![];
    for (id, name, desk) in teams {
        let members: Vec<(String, Option<String>, String)> = sqlx::query_as(
            "SELECT u.id, u.name, u.email FROM team_members t JOIN users u ON u.id = t.user_id WHERE t.team_id = ?1 AND t.workspace_id = ?2 ORDER BY u.email",
        )
        .bind(&id)
        .bind(ws)
        .fetch_all(db)
        .await?;
        out.push(json!({
            "id": id, "name": name, "desk_agent_id": desk,
            "members": members.into_iter().map(|(user_id, name, email)| json!({ "user_id": user_id, "name": name, "email": email })).collect::<Vec<_>>(),
        }));
    }
    Ok(out)
}

async fn only_members(state: &AppState, ws: &str, user_ids: &[String]) -> AppResult<Vec<String>> {
    if user_ids.len() > MAX_MEMBERS {
        return Err(AppError::BadRequest(format!("a team can have at most {MAX_MEMBERS} members")));
    }
    let mut ids: Vec<String> = vec![];
    for u in user_ids {
        let member: Option<i64> = sqlx::query_scalar("SELECT 1 FROM memberships WHERE workspace_id = ?1 AND user_id = ?2").bind(ws).bind(u).fetch_optional(&state.db).await?;
        if member.is_none() {
            return Err(AppError::BadRequest("a team can only include members of this workspace".into()));
        }
        if !ids.contains(u) {
            ids.push(u.clone());
        }
    }
    Ok(ids)
}

pub async fn save(state: &AppState, ws: &str, actor: &str, team_id: Option<&str>, name: &str, members: &[String], desk_agent_id: Option<&str>) -> AppResult<String> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() || name.chars().count() > 60 {
        return Err(AppError::BadRequest("give the team a name of up to 60 characters".into()));
    }
    let members = only_members(state, ws, members).await?;
    if let Some(agent) = desk_agent_id {
        let known = crate::agent_registry::store::get_in(&state.db, ws, agent).await?.is_some_and(|a| a.scope == crate::agent_registry::store::SCOPE_WORKSPACE);
        if !known {
            return Err(AppError::BadRequest("the desk agent was not found".into()));
        }
    }
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let id = match team_id {
        Some(id) => {
            let updated = sqlx::query("UPDATE teams SET name = ?1, desk_agent_id = ?2 WHERE id = ?3 AND workspace_id = ?4")
                .bind(&name)
                .bind(desk_agent_id)
                .bind(id)
                .bind(ws)
                .execute(&mut *tx)
                .await
                .map_err(name_clash)?
                .rows_affected();
            if updated == 0 {
                return Err(AppError::NotFound);
            }
            id.to_string()
        }
        None => {
            let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM teams WHERE workspace_id = ?1").bind(ws).fetch_one(&mut *tx).await?;
            if total >= MAX_TEAMS {
                return Err(AppError::BadRequest(format!("at most {MAX_TEAMS} teams per workspace")));
            }
            let id = new_id("team");
            sqlx::query("INSERT INTO teams (id, workspace_id, name, desk_agent_id, created_by, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")
                .bind(&id)
                .bind(ws)
                .bind(&name)
                .bind(desk_agent_id)
                .bind(actor)
                .bind(now_ms())
                .execute(&mut *tx)
                .await
                .map_err(name_clash)?;
            id
        }
    };
    sqlx::query("DELETE FROM team_members WHERE team_id = ?1").bind(&id).execute(&mut *tx).await?;
    for u in &members {
        sqlx::query("INSERT INTO team_members (team_id, workspace_id, user_id) VALUES (?1, ?2, ?3)").bind(&id).bind(ws).bind(u).execute(&mut *tx).await?;
    }
    audit::record(
        &mut tx,
        Entry { workspace_id: ws, actor: Actor::User(actor), action: "team.saved", entity_type: "team", entity_id: &id, message_id: None, metadata: json!({ "members": members.len() }) },
    )
    .await?;
    tx.commit().await?;
    Ok(id)
}

fn name_clash(e: sqlx::Error) -> AppError {
    match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => AppError::Conflict("a team with that name already exists".into()),
        other => other.into(),
    }
}

pub async fn delete(state: &AppState, ws: &str, actor: &str, id: &str) -> AppResult<()> {
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let gone = sqlx::query("DELETE FROM teams WHERE id = ?1 AND workspace_id = ?2").bind(id).bind(ws).execute(&mut *tx).await?.rows_affected();
    if gone == 0 {
        return Err(AppError::NotFound);
    }
    audit::record(&mut tx, Entry { workspace_id: ws, actor: Actor::User(actor), action: "team.deleted", entity_type: "team", entity_id: id, message_id: None, metadata: json!({}) }).await?;
    tx.commit().await?;
    Ok(())
}
