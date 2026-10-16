use super::*;

pub(super) async fn existing_by_source(db: &SqlitePool, ws: &str, source_key: &str) -> sqlx::Result<Option<Saved>> {
    let row: Option<RecordRow> = sqlx::query_as(&format!("{SELECT} WHERE r.workspace_id = ?1 AND r.source_key = ?2")).bind(ws).bind(source_key).fetch_optional(db).await?;
    Ok(row.map(|r| r.saved(false)))
}


pub(super) async fn insert_record(db: &SqlitePool, d: Draft<'_>, person_name: Option<String>) -> sqlx::Result<Saved> {
    let id = new_id("tsr");
    let now = now_ms();
    let identity_json = serde_json::to_string(&d.sheet.identity).unwrap_or_else(|_| "{}".into());
    let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
    if let Some(existing) = existing_by_source_tx(&mut tx, d.ws, &d.unit.source_key).await? {
        tx.commit().await?;
        return Ok(existing);
    }
    let mut issues = d.issues.clone();
    let mut identity_id: Option<String> = None;
    if d.person_id.is_none() && d.group {
        let company = unresolved::company_domains(&mut tx, d.ws).await?;
        identity_id = unresolved::attach(&mut tx, d.ws, &d.sheet.identity, &address_of(&d.msg.sender_address), d.roster, &company).await?;
        if let Some(group) = &identity_id {
            let twin: Option<String> = sqlx::query_scalar(
                "SELECT id FROM timesheet_records WHERE workspace_id = ?1 AND identity_id = ?2 AND person_id IS NULL AND status != 'replaced'
                   AND period_start = ?3 AND period_end = ?4 AND COALESCE(total_hours, -1) = COALESCE(?5, -1) LIMIT 1",
            )
            .bind(d.ws)
            .bind(group)
            .bind(&d.period.0)
            .bind(&d.period.1)
            .bind(d.total)
            .fetch_optional(&mut *tx)
            .await?;
            if twin.is_some() {
                issues.push(note("possible_duplicate", "This looks like the same sheet as one already received for this name."));
            }
        }
    }
    let mut status = d.status;
    let mut replaces: Option<String> = None;
    if let Some(person) = &d.person_id {
        let me = Placed { id: &id, message_id: Some(d.message_id), client: d.client.as_deref(), start: &d.period.0, end: &d.period.1, received: d.msg.received_at, assumed: d.assumed };
        let settled = settle(&mut tx, d.ws, person, &me, now).await?;
        if settled.superseded {
            status = "replaced";
        }
        if let Some((a, b)) = &settled.duplicate_of {
            issues.push(duplicate_issue(a, b));
        }
        replaces = settled.replaced.into_iter().next();
    }
    sqlx::query(
        "INSERT INTO timesheet_records
           (id, workspace_id, person_id, employee_code, person_name, sender_address, identity_json, match_method, match_confidence,
            message_id, attachment_id, source_key, source, email_subject, email_received_at, attachment_filename, attachment_kind,
            period_start, period_end, total_hours, entries_json, client, status, replaces_id, issues_json, created_at, updated_at, identity_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?26, ?27)",
    )
    .bind(&id)
    .bind(d.ws)
    .bind(&d.person_id)
    .bind(&d.sheet.identity.code)
    .bind(&d.sheet.identity.name)
    .bind(address_of(&d.msg.sender_address))
    .bind(identity_json)
    .bind(d.method)
    .bind(d.confidence)
    .bind(d.message_id)
    .bind(&d.unit.attachment_id)
    .bind(&d.unit.source_key)
    .bind(d.unit.source)
    .bind(&d.msg.subject)
    .bind(d.msg.received_at)
    .bind(&d.unit.filename)
    .bind(kind_label(d.unit.kind.as_deref(), d.unit.filename.as_deref()))
    .bind(&d.period.0)
    .bind(&d.period.1)
    .bind(d.total)
    .bind(serde_json::to_string(&d.sheet.entries).unwrap_or_else(|_| "[]".into()))
    .bind(&d.client)
    .bind(status)
    .bind(&replaces)
    .bind(serde_json::to_string(&issues).unwrap_or_else(|_| "[]".into()))
    .bind(now)
    .bind(&identity_id)
    .execute(&mut *tx)
    .await?;
    if let Some(group) = &identity_id {
        unresolved::recount(&mut tx, d.ws, group).await?;
    }
    tx.commit().await?;
    Ok(Saved {
        id,
        status: status.into(),
        person_id: d.person_id,
        person_name,
        period_start: d.period.0,
        period_end: d.period.1,
        total_hours: d.total,
        issues: issues.iter().map(|i| json!(i)).collect(),
        created: true,
    })
}

async fn existing_by_source_tx(conn: &mut SqliteConnection, ws: &str, source_key: &str) -> sqlx::Result<Option<Saved>> {
    let row: Option<RecordRow> = sqlx::query_as(&format!("{SELECT} WHERE r.workspace_id = ?1 AND r.source_key = ?2")).bind(ws).bind(source_key).fetch_optional(&mut *conn).await?;
    Ok(row.map(|r| r.saved(false)))
}

/// A body that only said "see attached" and was recorded empty before the file was read: the file's record replaces it.
pub(super) async fn retire_empty_pasted(db: &SqlitePool, ws: &str, message_id: &str) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE timesheet_records SET status = 'replaced', updated_at = ?3
         WHERE workspace_id = ?1 AND message_id = ?2 AND source = 'pasted' AND total_hours IS NULL AND entries_json = '[]' AND status != 'replaced'",
    )
    .bind(ws)
    .bind(message_id)
    .bind(now_ms())
    .execute(db)
    .await?;
    Ok(())
}
