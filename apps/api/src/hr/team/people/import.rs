//! Bulk CSV import of the team directory.

use serde::Serialize;
use sqlx::SqlitePool;

use super::store::{COLUMNS, PersonRow, insert, update};
use super::validate::{Context, Draft, FieldError, Fields, blank_draft, err, merge, validate};
use super::{MAX_CSV_ROWS, Person};

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
