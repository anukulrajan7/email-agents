//! The shape of a person write: the input body, merge rules and every check against the database.

use serde::{Deserialize, Deserializer};
use sqlx::SqliteConnection;

use super::{BILLABLE, MAX_EMAILS, NON_BILLABLE, Person};
use crate::{alerts::valid_email, hr::team::clock};

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

pub(super) fn err<T>(field: &'static str, message: impl Into<String>) -> Result<T, FieldError> {
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
