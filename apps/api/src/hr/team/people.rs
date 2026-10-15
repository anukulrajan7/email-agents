//! The team directory: people, their emails, manager chain, joining date and targets.
//! Billable / non-billable and the client are HR & recruiting fields (ignored and rejected elsewhere).

use std::collections::HashMap;

use serde::{Deserialize, Deserializer, Serialize};
use sqlx::{SqliteConnection, SqlitePool};

use crate::{
    alerts::valid_email,
    db::{new_id, now_ms},
    team::clock,
};

pub const BILLABLE: &str = "billable";
pub const NON_BILLABLE: &str = "non_billable";
pub const MAX_CSV_ROWS: usize = 2_000;
const MAX_EMAILS: usize = 5;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Person {
    pub id: String,
    pub name: String,
    pub emails: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub employee_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    pub manager_id: Option<String>,
    pub role: Option<String>,
    pub employee_code: Option<String>,
    pub joining_date: Option<String>,
    pub daily_target: Option<i64>,
    pub weekly_target: Option<i64>,
    pub source_id: Option<String>,
    pub active: bool,
}

impl Person {
    /// Drops the HR & recruiting fields for workspaces without that pack.
    pub fn without_staffing(mut self) -> Self {
        self.employee_type = None;
        self.client = None;
        self
    }

    pub fn is_billable(&self) -> bool {
        self.employee_type.as_deref() == Some(BILLABLE)
    }

    pub fn primary_email(&self) -> Option<&str> {
        self.emails.first().map(String::as_str)
    }
}

#[derive(sqlx::FromRow)]
struct PersonRow {
    id: String,
    name: String,
    employee_type: String,
    client: Option<String>,
    manager_id: Option<String>,
    role: Option<String>,
    employee_code: Option<String>,
    joining_date: Option<String>,
    daily_target: Option<i64>,
    weekly_target: Option<i64>,
    source_id: Option<String>,
    active: bool,
}

const COLUMNS: &str = "id, name, employee_type, client, manager_id, role, employee_code, joining_date, daily_target, weekly_target, source_id, active";

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

// ---------------------------------------------------------------- input

fn nullable<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Some(Option::deserialize(d)?))
}

/// Body of create and update. For update, a missing field is left alone and `null` clears it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fields {
    pub name: Option<String>,
    pub emails: Option<Vec<String>>,
    pub employee_type: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub client: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    pub manager_id: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    pub role: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    pub employee_code: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    pub joining_date: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    pub daily_target: Option<Option<i64>>,
    #[serde(default, deserialize_with = "nullable")]
    pub weekly_target: Option<Option<i64>>,
    #[serde(default, deserialize_with = "nullable")]
    pub source_id: Option<Option<String>>,
    pub active: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldError {
    pub field: &'static str,
    pub message: String,
}

fn err<T>(field: &'static str, message: impl Into<String>) -> Result<T, FieldError> {
    Err(FieldError { field, message: message.into() })
}

pub struct Context {
    pub staffing: bool,
    pub hr_source_id: Option<String>,
    /// The HR mailbox exists and is not disconnected.
    pub hr_connected: bool,
}

/// Fully resolved values, ready to validate and write.
#[derive(Debug, Clone)]
pub struct Draft {
    pub name: String,
    pub emails: Vec<String>,
    pub employee_type: String,
    pub client: Option<String>,
    pub manager_id: Option<String>,
    pub role: Option<String>,
    pub employee_code: Option<String>,
    pub joining_date: Option<String>,
    pub daily_target: Option<i64>,
    pub weekly_target: Option<i64>,
    pub source_id: Option<String>,
    pub active: bool,
}

impl Draft {
    fn blank() -> Self {
        Self {
            name: String::new(),
            emails: vec![],
            employee_type: NON_BILLABLE.into(),
            client: None,
            manager_id: None,
            role: None,
            employee_code: None,
            joining_date: None,
            daily_target: None,
            weekly_target: None,
            source_id: None,
            active: true,
        }
    }

    pub fn from_person(p: &Person) -> Self {
        Self {
            name: p.name.clone(),
            emails: p.emails.clone(),
            employee_type: p.employee_type.clone().unwrap_or_else(|| NON_BILLABLE.into()),
            client: p.client.clone(),
            manager_id: p.manager_id.clone(),
            role: p.role.clone(),
            employee_code: p.employee_code.clone(),
            joining_date: p.joining_date.clone(),
            daily_target: p.daily_target,
            weekly_target: p.weekly_target,
            source_id: p.source_id.clone(),
            active: p.active,
        }
    }
}

fn blank_to_none(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// "Non-billable", "non billable", "nonbillable" and "billable" in any case.
pub fn parse_employee_type(raw: &str) -> Option<&'static str> {
    match raw.trim().to_lowercase().replace([' ', '-'], "_").as_str() {
        "billable" => Some(BILLABLE),
        "non_billable" | "nonbillable" => Some(NON_BILLABLE),
        _ => None,
    }
}

/// Applies `f` to `base` (a blank draft for create). Pack rules and field shapes are checked here.
pub fn merge(base: Draft, f: Fields, ctx: &Context) -> Result<Draft, FieldError> {
    let mut d = base;
    if let Some(n) = f.name {
        d.name = n.trim().to_string();
    }
    if let Some(list) = f.emails {
        let mut out: Vec<String> = vec![];
        for e in list {
            let e = e.trim().to_lowercase();
            if !valid_email(&e) {
                return err("emails", format!("'{e}' is not a valid email address"));
            }
            if !out.contains(&e) {
                out.push(e);
            }
        }
        d.emails = out;
    }
    if let Some(t) = f.employee_type {
        if !ctx.staffing {
            return err("employee_type", "billable / non-billable is part of the HR & recruiting pack");
        }
        d.employee_type = parse_employee_type(&t).ok_or(FieldError { field: "employee_type", message: "must be billable or non_billable".into() })?.into();
    }
    if let Some(c) = f.client {
        if !ctx.staffing && blank_to_none(c.clone()).is_some() {
            return err("client", "client is part of the HR & recruiting pack");
        }
        d.client = blank_to_none(c);
    }
    if let Some(m) = f.manager_id {
        d.manager_id = blank_to_none(m);
    }
    if let Some(r) = f.role {
        d.role = blank_to_none(r);
    }
    if let Some(c) = f.employee_code {
        d.employee_code = blank_to_none(c);
    }
    if let Some(j) = f.joining_date {
        d.joining_date = blank_to_none(j);
    }
    if let Some(t) = f.daily_target {
        d.daily_target = t;
    }
    if let Some(t) = f.weekly_target {
        d.weekly_target = t;
    }
    if let Some(s) = f.source_id {
        d.source_id = blank_to_none(s);
    }
    if let Some(a) = f.active {
        d.active = a;
    }
    Ok(d)
}

pub fn blank_draft() -> Draft {
    Draft::blank()
}

/// Checks everything that needs the database: references, uniqueness, the manager chain and the billable rule.
pub async fn validate(conn: &mut SqliteConnection, workspace_id: &str, id: Option<&str>, d: &Draft, ctx: &Context) -> Result<(), FieldError> {
    let db_err = |_e: sqlx::Error| FieldError { field: "name", message: "database error".into() };
    if d.name.is_empty() || d.name.chars().count() > 100 {
        return err("name", "name must be 1-100 characters");
    }
    if d.emails.is_empty() || d.emails.len() > MAX_EMAILS {
        return err("emails", format!("add 1 to {MAX_EMAILS} email addresses"));
    }
    for e in &d.emails {
        let owner: Option<(String, String)> = sqlx::query_as(
            "SELECT p.id, p.name FROM person_emails e JOIN people p ON p.id = e.person_id WHERE e.workspace_id = ?1 AND e.email = ?2",
        )
        .bind(workspace_id)
        .bind(e)
        .fetch_optional(&mut *conn)
        .await
        .map_err(db_err)?;
        if let Some((other, name)) = owner
            && Some(other.as_str()) != id
        {
            return err("emails", format!("{e} already belongs to {name}"));
        }
    }
    if let Some(code) = &d.employee_code {
        if code.chars().count() > 24 || code.contains(char::is_whitespace) {
            return err("employee_code", "an employee ID is at most 24 characters with no spaces");
        }
        let owner: Option<(String, String)> = sqlx::query_as("SELECT id, name FROM people WHERE workspace_id = ?1 AND lower(employee_code) = lower(?2)")
            .bind(workspace_id)
            .bind(code)
            .fetch_optional(&mut *conn)
            .await
            .map_err(db_err)?;
        if let Some((other, name)) = owner
            && Some(other.as_str()) != id
        {
            return err("employee_code", format!("{code} already belongs to {name}"));
        }
    }
    if d.role.as_ref().is_some_and(|r| r.chars().count() > 60) || d.client.as_ref().is_some_and(|c| c.chars().count() > 100) {
        return err("role", "role is at most 60 characters and client at most 100");
    }
    if let Some(j) = &d.joining_date
        && clock::parse_date(j).is_none()
    {
        return err("joining_date", "use YYYY-MM-DD");
    }
    for (field, t, max) in [("daily_target", d.daily_target, 1_000), ("weekly_target", d.weekly_target, 5_000)] {
        if t.is_some_and(|v| !(0..=max).contains(&v)) {
            return err(field, format!("must be 0-{max}"));
        }
    }
    if d.employee_type == BILLABLE && ctx.staffing && !ctx.hr_connected {
        return err("employee_type", "name a connected HR mailbox in the settings before marking anyone billable: their mail is only read through it");
    }
    if let Some(src) = &d.source_id {
        let ok: Option<i64> = sqlx::query_scalar("SELECT 1 FROM email_sources WHERE id = ?1 AND workspace_id = ?2")
            .bind(src)
            .bind(workspace_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(db_err)?;
        if ok.is_none() {
            return err("source_id", "unknown mailbox");
        }
        if d.employee_type == BILLABLE && ctx.hr_source_id.as_deref() != Some(src.as_str()) {
            return err("source_id", "a billable person's mail arrives only through the HR mailbox, so they cannot have their own mailbox linked");
        }
    }
    if let Some(m) = &d.manager_id {
        let mut cursor = Some(m.clone());
        let mut hops = 0;
        while let Some(c) = cursor {
            if Some(c.as_str()) == id {
                return err("manager_id", "that would make a loop in the manager chain");
            }
            let row: Option<Option<String>> = sqlx::query_scalar("SELECT manager_id FROM people WHERE id = ?1 AND workspace_id = ?2")
                .bind(&c)
                .bind(workspace_id)
                .fetch_optional(&mut *conn)
                .await
                .map_err(db_err)?;
            let Some(next) = row else { return err("manager_id", "unknown manager") };
            hops += 1;
            if hops > 20 {
                return err("manager_id", "the manager chain is too deep");
            }
            cursor = next;
        }
    }
    Ok(())
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

// ---------------------------------------------------------------- CSV import

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RowError {
    pub row: usize,
    pub field: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ImportReport {
    pub dry_run: bool,
    pub total_rows: usize,
    pub created: usize,
    pub updated: usize,
    pub skipped: usize,
    pub errors: Vec<RowError>,
}

fn column(headers: &[String], names: &[&str]) -> Option<usize> {
    headers.iter().position(|h| names.contains(&h.as_str()))
}

fn int_cell(v: &str, field: &'static str) -> Result<Option<i64>, FieldError> {
    if v.is_empty() {
        return Ok(None);
    }
    v.parse::<i64>().map(Some).or_else(|_| err(field, format!("'{v}' is not a whole number")))
}

/// Imports a CSV in one transaction. Rows with problems are skipped and listed; `dry_run` rolls everything back.
/// Columns: name, email (several separated by `;`), employee_type, client, manager_email, role, employee_code, joining_date,
/// daily_target, weekly_target, mailbox (the connected Gmail address). A person is matched by any of their emails.
pub async fn import_csv(db: &SqlitePool, workspace_id: &str, csv_text: &str, dry_run: bool, ctx: &Context) -> Result<ImportReport, String> {
    let mut reader = csv::ReaderBuilder::new().trim(csv::Trim::All).flexible(true).from_reader(csv_text.trim_start_matches('\u{feff}').as_bytes());
    let headers: Vec<String> =
        reader.headers().map_err(|e| format!("could not read the header row: {e}"))?.iter().map(|h| h.trim().to_lowercase().replace(' ', "_")).collect();
    let name_c = column(&headers, &["name", "full_name", "employee"]);
    let email_c = column(&headers, &["email", "emails", "email_id", "email_address"]);
    let (Some(name_c), Some(email_c)) = (name_c, email_c) else {
        return Err("the first row must have at least the columns name and email".into());
    };
    let type_c = column(&headers, &["employee_type", "type"]);
    let client_c = column(&headers, &["client"]);
    let manager_c = column(&headers, &["manager_email", "manager"]);
    let role_c = column(&headers, &["role", "designation"]);
    let code_c = column(&headers, &["employee_code", "employee_id", "emp_id", "emp_code", "employee_no"]);
    let join_c = column(&headers, &["joining_date", "joined", "doj"]);
    let daily_c = column(&headers, &["daily_target"]);
    let weekly_c = column(&headers, &["weekly_target"]);
    let mailbox_c = column(&headers, &["mailbox", "mailbox_email"]);

    let mut report = ImportReport { dry_run, ..Default::default() };
    let mut tx = db.begin_with("BEGIN IMMEDIATE").await.map_err(|e| e.to_string())?;
    // (row number, person id, manager email) resolved after every person exists.
    let mut managers: Vec<(usize, String, String)> = vec![];
    for (i, record) in reader.records().enumerate() {
        let row = i + 2;
        report.total_rows += 1;
        if report.total_rows > MAX_CSV_ROWS {
            return Err(format!("at most {MAX_CSV_ROWS} rows per import"));
        }
        let record = match record {
            Ok(r) => r,
            Err(e) => {
                report.errors.push(RowError { row, field: "row".into(), message: e.to_string() });
                report.skipped += 1;
                continue;
            }
        };
        let cell = |c: Option<usize>| c.and_then(|c| record.get(c)).unwrap_or("").trim().to_string();
        let outcome: Result<(Draft, Option<String>, Option<String>), FieldError> = async {
            let emails: Vec<String> = cell(Some(email_c)).split([';', ',', ' ', '|']).filter(|e| !e.is_empty()).map(str::to_string).collect();
            let mailbox = cell(mailbox_c);
            let manager = cell(manager_c);
            let known = match emails.first() {
                Some(first) => {
                    let first = first.trim().to_lowercase();
                    let id: Option<String> = sqlx::query_scalar("SELECT person_id FROM person_emails WHERE workspace_id = ?1 AND email = ?2")
                        .bind(workspace_id)
                        .bind(&first)
                        .fetch_optional(&mut *tx)
                        .await
                        .map_err(|e| FieldError { field: "emails", message: e.to_string() })?;
                    id
                }
                None => None,
            };
            let base = match &known {
                Some(id) => {
                    let p: Vec<Person> = {
                        // Read inside the transaction so earlier rows of this file are visible.
                        let rows: Vec<PersonRow> = sqlx::query_as(&format!("SELECT {COLUMNS} FROM people WHERE id = ?1"))
                            .bind(id)
                            .fetch_all(&mut *tx)
                            .await
                            .map_err(|e| FieldError { field: "name", message: e.to_string() })?;
                        let emails: Vec<String> = sqlx::query_scalar("SELECT email FROM person_emails WHERE person_id = ?1 ORDER BY position")
                            .bind(id)
                            .fetch_all(&mut *tx)
                            .await
                            .map_err(|e| FieldError { field: "name", message: e.to_string() })?;
                        rows.into_iter()
                            .map(|r| Person {
                                id: r.id,
                                name: r.name,
                                emails: emails.clone(),
                                employee_type: Some(r.employee_type),
                                client: r.client,
                                manager_id: r.manager_id,
                                role: r.role,
                                employee_code: r.employee_code,
                                joining_date: r.joining_date,
                                daily_target: r.daily_target,
                                weekly_target: r.weekly_target,
                                source_id: r.source_id,
                                active: true,
                            })
                            .collect()
                    };
                    Draft::from_person(&p[0])
                }
                None => blank_draft(),
            };
            let mut fields = Fields { name: Some(cell(Some(name_c))), emails: Some(emails), ..Default::default() };
            let t = cell(type_c);
            if !t.is_empty() {
                fields.employee_type = Some(t);
            }
            let c = cell(client_c);
            if !c.is_empty() {
                fields.client = Some(Some(c));
            }
            let r = cell(role_c);
            if !r.is_empty() {
                fields.role = Some(Some(r));
            }
            let code = cell(code_c);
            if !code.is_empty() {
                fields.employee_code = Some(Some(code));
            }
            let j = cell(join_c);
            if !j.is_empty() {
                fields.joining_date = Some(Some(j));
            }
            if let Some(v) = int_cell(&cell(daily_c), "daily_target")? {
                fields.daily_target = Some(Some(v));
            }
            if let Some(v) = int_cell(&cell(weekly_c), "weekly_target")? {
                fields.weekly_target = Some(Some(v));
            }
            if !mailbox.is_empty() {
                let src: Option<String> = sqlx::query_scalar(
                    "SELECT id FROM email_sources WHERE workspace_id = ?1 AND lower(mailbox_address) = lower(?2) LIMIT 1",
                )
                .bind(workspace_id)
                .bind(&mailbox)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|e| FieldError { field: "mailbox", message: e.to_string() })?;
                match src {
                    Some(s) => fields.source_id = Some(Some(s)),
                    None => return err("mailbox", format!("no connected mailbox {mailbox}")),
                }
            }
            let draft = merge(base, fields, ctx)?;
            validate(&mut tx, workspace_id, known.as_deref(), &draft, ctx).await?;
            Ok((draft, known, (!manager.is_empty()).then_some(manager)))
        }
        .await;
        match outcome {
            Ok((draft, known, manager)) => {
                let id = match known {
                    Some(id) => {
                        update(&mut tx, workspace_id, &id, &draft).await.map_err(|e| e.to_string())?;
                        report.updated += 1;
                        id
                    }
                    None => {
                        report.created += 1;
                        insert(&mut tx, workspace_id, &draft).await.map_err(|e| e.to_string())?
                    }
                };
                if let Some(m) = manager {
                    managers.push((row, id, m.to_lowercase()));
                }
            }
            Err(e) => {
                report.errors.push(RowError { row, field: e.field.into(), message: e.message });
                report.skipped += 1;
            }
        }
    }
    for (row, id, manager_email) in managers {
        let manager: Option<String> = sqlx::query_scalar("SELECT person_id FROM person_emails WHERE workspace_id = ?1 AND email = ?2")
            .bind(workspace_id)
            .bind(&manager_email)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        let Some(manager) = manager else {
            report.errors.push(RowError { row, field: "manager_email".into(), message: format!("no one in the file or directory has the email {manager_email}") });
            continue;
        };
        let draft = {
            let rows: Vec<PersonRow> = sqlx::query_as(&format!("SELECT {COLUMNS} FROM people WHERE id = ?1")).bind(&id).fetch_all(&mut *tx).await.map_err(|e| e.to_string())?;
            let emails: Vec<String> = sqlx::query_scalar("SELECT email FROM person_emails WHERE person_id = ?1 ORDER BY position").bind(&id).fetch_all(&mut *tx).await.map_err(|e| e.to_string())?;
            let r = &rows[0];
            Draft {
                name: r.name.clone(),
                emails,
                employee_type: r.employee_type.clone(),
                client: r.client.clone(),
                manager_id: Some(manager),
                role: r.role.clone(),
                employee_code: r.employee_code.clone(),
                joining_date: r.joining_date.clone(),
                daily_target: r.daily_target,
                weekly_target: r.weekly_target,
                source_id: r.source_id.clone(),
                active: true,
            }
        };
        match validate(&mut tx, workspace_id, Some(&id), &draft, ctx).await {
            Ok(()) => update(&mut tx, workspace_id, &id, &draft).await.map_err(|e| e.to_string())?,
            Err(e) => report.errors.push(RowError { row, field: e.field.into(), message: e.message }),
        }
    }
    if dry_run {
        tx.rollback().await.map_err(|e| e.to_string())?;
    } else {
        tx.commit().await.map_err(|e| e.to_string())?;
    }
    report.errors.sort_by_key(|e| e.row);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn employee_types_and_merge_rules() {
        assert_eq!(parse_employee_type(" Non-Billable "), Some(NON_BILLABLE));
        assert_eq!(parse_employee_type("BILLABLE"), Some(BILLABLE));
        assert_eq!(parse_employee_type("contractor"), None);
        let general = Context { staffing: false, hr_source_id: None, hr_connected: false };
        let f = Fields { name: Some(" Asha ".into()), emails: Some(vec!["Asha@Acme.com".into(), "asha@acme.com".into()]), ..Default::default() };
        let d = merge(blank_draft(), f, &general).unwrap();
        assert_eq!((d.name.as_str(), d.emails.clone()), ("Asha", vec!["asha@acme.com".to_string()]));
        let f = Fields { employee_type: Some("billable".into()), ..Default::default() };
        assert_eq!(merge(blank_draft(), f, &general).unwrap_err().field, "employee_type");
        let f = Fields { employee_type: Some("billable".into()), ..Default::default() };
        assert_eq!(merge(blank_draft(), f, &Context { staffing: true, hr_source_id: None, hr_connected: true }).unwrap().employee_type, BILLABLE);
    }
}
