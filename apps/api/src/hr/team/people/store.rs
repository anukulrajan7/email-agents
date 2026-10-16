//! Reading and writing people rows and their emails.

use std::collections::HashMap;

use sqlx::{SqliteConnection, SqlitePool};

use super::{Draft, Person};
use crate::platform::db::{new_id, now_ms};

#[derive(sqlx::FromRow)]
pub(super) struct PersonRow {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) employee_type: String,
    pub(super) client: Option<String>,
    pub(super) manager_id: Option<String>,
    pub(super) role: Option<String>,
    pub(super) employee_code: Option<String>,
    pub(super) joining_date: Option<String>,
    pub(super) daily_target: Option<i64>,
    pub(super) weekly_target: Option<i64>,
    pub(super) source_id: Option<String>,
    pub(super) active: bool,
}

pub(super) const COLUMNS: &str = "id, name, employee_type, client, manager_id, role, employee_code, joining_date, daily_target, weekly_target, source_id, active";

async fn load(db: &SqlitePool, workspace_id: &str, only: Option<&str>, include_inactive: bool) -> sqlx::Result<Vec<Person>> {
    let rows: Vec<PersonRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM people WHERE workspace_id = ?1 AND (?2 IS NULL OR id = ?2) AND (active = 1 OR ?3) ORDER BY lower(name), id"
    ))
    .bind(workspace_id)
    .bind(only)
    .bind(include_inactive)
    .fetch_all(db)
    .await?;
    let emails: Vec<(String, String)> = sqlx::query_as(
        "SELECT person_id, email FROM person_emails WHERE workspace_id = ?1 AND (?2 IS NULL OR person_id = ?2) ORDER BY position",
    )
    .bind(workspace_id)
    .bind(only)
    .fetch_all(db)
    .await?;
    let mut by_person: HashMap<String, Vec<String>> = HashMap::new();
    for (p, e) in emails {
        by_person.entry(p).or_default().push(e);
    }
    Ok(rows
        .into_iter()
        .map(|r| Person {
            emails: by_person.remove(&r.id).unwrap_or_default(),
            id: r.id,
            name: r.name,
            employee_type: Some(r.employee_type),
            client: r.client,
            manager_id: r.manager_id,
            role: r.role,
            employee_code: r.employee_code,
            joining_date: r.joining_date,
            daily_target: r.daily_target,
            weekly_target: r.weekly_target,
            source_id: r.source_id,
            active: r.active,
        })
        .collect())
}

pub async fn list(db: &SqlitePool, workspace_id: &str, include_inactive: bool) -> sqlx::Result<Vec<Person>> {
    load(db, workspace_id, None, include_inactive).await
}

pub async fn get(db: &SqlitePool, workspace_id: &str, id: &str) -> sqlx::Result<Option<Person>> {
    Ok(load(db, workspace_id, Some(id), true).await?.pop())
}

async fn write_emails(conn: &mut SqliteConnection, workspace_id: &str, id: &str, emails: &[String]) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM person_emails WHERE person_id = ?1").bind(id).execute(&mut *conn).await?;
    for (i, e) in emails.iter().enumerate() {
        sqlx::query("INSERT INTO person_emails (workspace_id, email, person_id, position) VALUES (?1, ?2, ?3, ?4)")
            .bind(workspace_id)
            .bind(e)
            .bind(id)
            .bind(i as i64)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

pub async fn insert(conn: &mut SqliteConnection, workspace_id: &str, d: &Draft) -> sqlx::Result<String> {
    let id = new_id("per");
    let now = now_ms();
    sqlx::query(
        "INSERT INTO people (id, workspace_id, name, employee_type, client, manager_id, role, joining_date, daily_target,
                             weekly_target, source_id, active, created_at, updated_at, employee_code)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13, ?14)",
    )
    .bind(&id)
    .bind(workspace_id)
    .bind(&d.name)
    .bind(&d.employee_type)
    .bind(&d.client)
    .bind(&d.manager_id)
    .bind(&d.role)
    .bind(&d.joining_date)
    .bind(d.daily_target)
    .bind(d.weekly_target)
    .bind(&d.source_id)
    .bind(d.active)
    .bind(now)
    .bind(&d.employee_code)
    .execute(&mut *conn)
    .await?;
    write_emails(conn, workspace_id, &id, &d.emails).await?;
    Ok(id)
}

pub async fn update(conn: &mut SqliteConnection, workspace_id: &str, id: &str, d: &Draft) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE people SET name = ?1, employee_type = ?2, client = ?3, manager_id = ?4, role = ?5, joining_date = ?6,
                daily_target = ?7, weekly_target = ?8, source_id = ?9, active = ?10, updated_at = ?11, employee_code = ?14
         WHERE id = ?12 AND workspace_id = ?13",
    )
    .bind(&d.name)
    .bind(&d.employee_type)
    .bind(&d.client)
    .bind(&d.manager_id)
    .bind(&d.role)
    .bind(&d.joining_date)
    .bind(d.daily_target)
    .bind(d.weekly_target)
    .bind(&d.source_id)
    .bind(d.active)
    .bind(now_ms())
    .bind(id)
    .bind(workspace_id)
    .bind(&d.employee_code)
    .execute(&mut *conn)
    .await?;
    // An archived person keeps their numbers but releases their addresses.
    if d.active {
        write_emails(conn, workspace_id, id, &d.emails).await?;
    } else {
        sqlx::query("DELETE FROM person_emails WHERE person_id = ?1").bind(id).execute(&mut *conn).await?;
    }
    Ok(())
}
