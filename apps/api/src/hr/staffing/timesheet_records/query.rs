use super::*;

pub(super) const SELECT: &str = "SELECT r.id, r.person_id, p.name AS linked_name, r.employee_code, r.person_name, r.sender_address, r.identity_json, r.match_method,
        r.match_confidence, r.message_id, r.source, r.email_subject, r.email_received_at, r.attachment_filename, r.attachment_kind,
        r.period_start, r.period_end, r.total_hours, r.entries_json, r.client, r.status, r.replaces_id, r.issues_json, r.created_at, r.updated_at
    FROM timesheet_records r LEFT JOIN people p ON p.id = r.person_id";

impl RecordRow {
    pub(super) fn saved(self, created: bool) -> Saved {
        Saved {
            id: self.id,
            status: self.status,
            person_id: self.person_id,
            person_name: self.linked_name,
            period_start: self.period_start,
            period_end: self.period_end,
            total_hours: self.total_hours,
            issues: serde_json::from_str(&self.issues_json).unwrap_or_default(),
            created,
        }
    }

    pub(super) fn entries(&self) -> Vec<Entry> {
        serde_json::from_str(&self.entries_json).unwrap_or_default()
    }

    pub fn json(&self, detail: bool) -> Value {
        let issues: Value = serde_json::from_str(&self.issues_json).unwrap_or_else(|_| json!([]));
        let source_label = if self.source == "pasted" { "Pasted".to_string() } else { self.attachment_kind.clone().unwrap_or_else(|| "File".into()) };
        let mut v = json!({
            "id": self.id,
            "person_id": self.person_id,
            "person_name": self.linked_name.clone().or_else(|| self.person_name.clone()),
            "name_as_written": self.person_name,
            "employee_code": self.employee_code,
            "client": self.client,
            "period_start": self.period_start,
            "period_end": self.period_end,
            "total_hours": self.total_hours,
            "status": self.status,
            "source": self.source,
            "source_label": source_label,
            "attachment_filename": self.attachment_filename,
            "match_method": self.match_method,
            "match_confidence": self.match_confidence,
            "needs_person": self.person_id.is_none(),
            "issues": issues,
            "replaces_id": self.replaces_id,
            "email": {
                "message_id": self.message_id,
                "available": self.message_id.is_some(),
                "subject": self.email_subject,
                "from": self.sender_address,
                "received_at": self.email_received_at.map(iso),
            },
            "created_at": iso(self.created_at),
            "updated_at": iso(self.updated_at),
        });
        if detail {
            v["entries"] = json!(self.entries());
            v["identity"] = serde_json::from_str(&self.identity_json).unwrap_or_else(|_| json!({}));
        }
        v
    }
}

#[derive(Debug, Default)]
pub struct Filter {
    pub person_id: Option<String>,
    pub month: Option<String>,
    pub status: Option<String>,
    pub q: Option<String>,
}

pub(super) fn visible(a: &Access, r: &RecordRow) -> bool {
    match &r.person_id {
        Some(p) => a.can_see(p),
        None => a.visible().is_none(),
    }
}

pub async fn list(db: &SqlitePool, ws: &str, f: &Filter, a: &Access) -> sqlx::Result<Vec<Value>> {
    let bounds = f.month.as_deref().and_then(parse::month_dates);
    let like = f.q.as_deref().map(str::trim).filter(|q| !q.is_empty()).map(|q| format!("%{}%", q.replace(['%', '_'], "")));
    let rows: Vec<RecordRow> = sqlx::query_as(&format!(
        "{SELECT} WHERE r.workspace_id = ?1 AND (?2 IS NULL OR r.person_id = ?2) AND (?3 IS NULL OR r.status = ?3)
           AND (?4 IS NULL OR (r.period_start <= ?5 AND r.period_end >= ?4))
           AND (?6 IS NULL OR r.person_name LIKE ?6 OR p.name LIKE ?6 OR r.employee_code LIKE ?6 OR r.client LIKE ?6
                OR r.email_subject LIKE ?6 OR r.attachment_filename LIKE ?6 OR r.sender_address LIKE ?6)
           AND (?7 IS NULL OR r.person_id IN (SELECT value FROM json_each(?7)))
         ORDER BY r.period_start DESC, r.created_at DESC LIMIT 500"
    ))
    .bind(ws)
    .bind(&f.person_id)
    .bind(&f.status)
    .bind(bounds.as_ref().map(|b| b.0.clone()))
    .bind(bounds.as_ref().map(|b| b.1.clone()))
    .bind(like)
    .bind(a.visible().map(|v| json!(v).to_string()))
    .fetch_all(db)
    .await?;
    Ok(rows.iter().filter(|r| visible(a, r)).map(|r| r.json(false)).collect())
}

pub async fn get(db: &SqlitePool, ws: &str, id: &str) -> sqlx::Result<Option<RecordRow>> {
    sqlx::query_as(&format!("{SELECT} WHERE r.workspace_id = ?1 AND r.id = ?2")).bind(ws).bind(id).fetch_optional(db).await
}

pub fn can_see_record(a: &Access, r: &RecordRow) -> bool {
    visible(a, r)
}

/// The records read from one email, for the reader's chip and the message detail.
pub async fn for_message(db: &SqlitePool, ws: &str, message_id: &str) -> sqlx::Result<Vec<Value>> {
    let rows: Vec<RecordRow> =
        sqlx::query_as(&format!("{SELECT} WHERE r.workspace_id = ?1 AND r.message_id = ?2 AND r.status != 'replaced' ORDER BY r.created_at")).bind(ws).bind(message_id).fetch_all(db).await?;
    Ok(rows.iter().map(|r| r.json(false)).collect())
}

/// Is a person needed? `None`: nothing recorded for this email.
pub async fn attention(db: &SqlitePool, ws: &str, message_id: &str) -> sqlx::Result<Option<bool>> {
    let rows: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT status, person_id FROM timesheet_records WHERE workspace_id = ?1 AND message_id = ?2 AND status != 'replaced'").bind(ws).bind(message_id).fetch_all(db).await?;
    if rows.is_empty() {
        return Ok(None);
    }
    Ok(Some(rows.iter().any(|(status, person)| status == "needs_check" || person.is_none())))
}

/// Mail the timesheet desk has taken: it is labelled `timesheet` now and a record with real hours was read from it.
/// No automatic reply is drafted for it; a person can still ask for a draft by hand. Relabelled mail, and a record
/// with nothing in it (no hours found) get the normal replies again. A timesheet file that could not be read is
/// still the desk's: a person is asked to look at it.
pub async fn suppresses_auto_reply(db: &SqlitePool, ws: &str, message_id: &str) -> sqlx::Result<bool> {
    let labelled: Option<String> = sqlx::query_scalar("SELECT category FROM classifications WHERE message_id = ?1").bind(message_id).fetch_optional(db).await?;
    if labelled.as_deref() != Some(LABEL) {
        return Ok(false);
    }
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM timesheet_records WHERE workspace_id = ?1 AND message_id = ?2 AND (total_hours IS NOT NULL OR entries_json != '[]' OR issues_json LIKE '%\"unreadable\"%'))",
    )
    .bind(ws)
    .bind(message_id)
    .fetch_one(db)
    .await
}
