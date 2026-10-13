//! Who may see what. Three levels: the super admin (the workspace owner: HRMS settings, directory,
//! targets, imports, every person's numbers), other admins and operators. Everyone except the super
//! admin sees only themselves and the people who report to them through the manager chain.
//!
//! The super admin is a row in `workspace_owners` (always an admin member), not a new `memberships.role`
//! value: widening that CHECK would rebuild a table other tables reference.

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use sqlx::{SqliteConnection, SqlitePool};

use crate::{
    auth::{AuthUser, require_role},
    db::now_ms,
    domain::Role,
    error::{AppError, AppResult},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    SuperAdmin,
    Admin,
    Operator,
}

#[derive(Debug, Clone)]
pub struct Access {
    pub level: Level,
    pub user_id: String,
    /// The directory person whose email is the signed-in user's.
    pub person_id: Option<String>,
    /// `None` = everyone (super admin).
    visible: Option<HashSet<String>>,
}

impl Access {
    pub fn is_owner(&self) -> bool {
        self.level == Level::SuperAdmin
    }

    pub fn can_see(&self, person_id: &str) -> bool {
        self.visible.as_ref().is_none_or(|v| v.contains(person_id))
    }

    pub fn visible(&self) -> Option<&HashSet<String>> {
        self.visible.as_ref()
    }

    /// Reports below this person (excluding themselves); drives the "manager" view.
    pub fn report_count(&self) -> usize {
        match (&self.visible, &self.person_id) {
            (Some(v), Some(_)) => v.len().saturating_sub(1),
            _ => 0,
        }
    }

    pub fn require_owner(&self) -> AppResult<()> {
        if self.is_owner() { Ok(()) } else { Err(AppError::Denied("Only the workspace's super admin can do this".into())) }
    }

    pub fn require_visible(&self, person_id: &str) -> AppResult<()> {
        if self.can_see(person_id) { Ok(()) } else { Err(AppError::NotFound) }
    }
}

/// The super admin: the stored owner, else the first admin (workspaces created before owners existed).
pub async fn owner_user_id(db: &SqlitePool, workspace_id: &str) -> sqlx::Result<Option<String>> {
    if let Some(id) = sqlx::query_scalar("SELECT user_id FROM workspace_owners WHERE workspace_id = ?1")
        .bind(workspace_id)
        .fetch_optional(db)
        .await?
    {
        return Ok(Some(id));
    }
    sqlx::query_scalar("SELECT user_id FROM memberships WHERE workspace_id = ?1 AND role = 'admin' ORDER BY rowid LIMIT 1")
        .bind(workspace_id)
        .fetch_optional(db)
        .await
}

/// Records the creator as the super admin (called inside workspace creation).
pub async fn seed_owner(conn: &mut SqliteConnection, workspace_id: &str, user_id: &str) -> sqlx::Result<()> {
    sqlx::query("INSERT OR IGNORE INTO workspace_owners (workspace_id, user_id, created_at) VALUES (?1, ?2, ?3)")
        .bind(workspace_id)
        .bind(user_id)
        .bind(now_ms())
        .execute(conn)
        .await?;
    Ok(())
}

/// `person_id` plus everyone below them in the manager chain.
pub async fn subtree(db: &SqlitePool, workspace_id: &str, person_id: &str) -> sqlx::Result<HashSet<String>> {
    let rows: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT id, manager_id FROM people WHERE workspace_id = ?1 AND active = 1").bind(workspace_id).fetch_all(db).await?;
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    for (id, manager) in &rows {
        if let Some(m) = manager {
            children.entry(m.as_str()).or_default().push(id.as_str());
        }
    }
    let mut out = HashSet::from([person_id.to_string()]);
    let mut stack = vec![person_id];
    while let Some(p) = stack.pop() {
        for c in children.get(p).into_iter().flatten() {
            if out.insert((*c).to_string()) {
                stack.push(c);
            }
        }
    }
    Ok(out)
}

pub async fn person_for_email(db: &SqlitePool, workspace_id: &str, email: &str) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar(
        "SELECT e.person_id FROM person_emails e JOIN people p ON p.id = e.person_id
         WHERE e.workspace_id = ?1 AND e.email = lower(?2) AND p.active = 1",
    )
    .bind(workspace_id)
    .bind(email)
    .fetch_optional(db)
    .await
}

/// Membership is required; the level and visible people follow from the owner row and the directory.
pub async fn resolve(db: &SqlitePool, workspace_id: &str, user: &AuthUser) -> AppResult<Access> {
    let role = require_role(db, workspace_id, user, Role::Operator).await?;
    let person_id = person_for_email(db, workspace_id, &user.email).await?;
    let owner = owner_user_id(db, workspace_id).await?.is_some_and(|o| o == user.id);
    let (level, visible) = if owner {
        (Level::SuperAdmin, None)
    } else {
        let visible = match &person_id {
            Some(p) => subtree(db, workspace_id, p).await?,
            None => HashSet::new(),
        };
        (if role == Role::Admin { Level::Admin } else { Level::Operator }, Some(visible))
    };
    Ok(Access { level, user_id: user.id.clone(), person_id, visible })
}
