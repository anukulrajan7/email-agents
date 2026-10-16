use super::*;

/// What "this sheet is that person" did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Linked {
    Done,
    NotFound,
    /// The employee ID or name is already remembered for someone else; nothing was changed.
    Conflict(String),
}

/// A person says "this sheet is that person". The other unmatched sheets that carry the same employee ID join it (the ID
/// is the evidence), and so do the ones with the same name, but only when the person confirmed the name (`remember_name`)
/// and only if they do not name a different ID. A remembered ID or name that belongs to someone else is never re-pointed.
pub async fn link_person(db: &SqlitePool, ws: &str, record_id: &str, person_id: &str, remember_name: bool) -> sqlx::Result<Linked> {
    let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
    let person: Option<String> = sqlx::query_scalar("SELECT name FROM people WHERE id = ?1 AND workspace_id = ?2").bind(person_id).bind(ws).fetch_optional(&mut *tx).await?;
    let rec: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT employee_code, person_name FROM timesheet_records WHERE id = ?1 AND workspace_id = ?2").bind(record_id).bind(ws).fetch_optional(&mut *tx).await?;
    let (Some(_), Some((code, name))) = (person, rec) else { return Ok(Linked::NotFound) };
    let now = now_ms();
    let code = code.map(|c| c.to_lowercase());
    let key = name.as_deref().map(name_key).filter(|k| !k.is_empty());
    if let Some(code) = &code
        && let Some(owner) = remember(&mut tx, ws, "code", code, person_id, now).await?
    {
        return Ok(Linked::Conflict(format!("The employee ID {} already belongs to {owner}.", code.to_uppercase())));
    }
    if remember_name
        && let Some(key) = &key
        && let Some(owner) = remember(&mut tx, ws, "name", key, person_id, now).await?
    {
        return Ok(Linked::Conflict(format!("The name {} already belongs to {owner}.", name.as_deref().unwrap_or(key))));
    }
    let others: Vec<(String, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT id, employee_code, person_name FROM timesheet_records WHERE workspace_id = ?1 AND person_id IS NULL ORDER BY created_at, id").bind(ws).fetch_all(&mut *tx).await?;
    for (id, other_code, other_name) in others {
        let other_code = other_code.map(|c| c.to_lowercase());
        let same_id = code.is_some() && other_code == code;
        let same_name = remember_name && key.is_some() && other_name.as_deref().map(name_key) == key && (other_code.is_none() || other_code == code);
        if id == record_id || same_id || same_name {
            attach_person(&mut tx, ws, &id, person_id, now).await?;
        }
    }
    tx.commit().await?;
    Ok(Linked::Done)
}

/// Remembers that `value` means this person. `Some(name)` when it already means someone else: never re-pointed.
async fn remember(conn: &mut SqliteConnection, ws: &str, kind: &str, value: &str, person_id: &str, now: i64) -> sqlx::Result<Option<String>> {
    let owner: Option<(String, String)> = sqlx::query_as(
        "SELECT a.person_id, COALESCE(p.name, 'another person') FROM timesheet_aliases a LEFT JOIN people p ON p.id = a.person_id
         WHERE a.workspace_id = ?1 AND a.kind = ?2 AND a.value = ?3",
    )
    .bind(ws)
    .bind(kind)
    .bind(value)
    .fetch_optional(&mut *conn)
    .await?;
    match owner {
        Some((owner, name)) if owner != person_id => return Ok(Some(name)),
        Some(_) => return Ok(None),
        None => {}
    }
    sqlx::query("INSERT INTO timesheet_aliases (workspace_id, kind, value, person_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5)")
        .bind(ws)
        .bind(kind)
        .bind(value)
        .bind(person_id)
        .bind(now)
        .execute(conn)
        .await?;
    Ok(None)
}

/// Gives an unmatched record its person: clears "who is this", settles the status and applies the replace rule.
async fn attach_person(conn: &mut SqliteConnection, ws: &str, id: &str, person_id: &str, now: i64) -> sqlx::Result<()> {
    type Own = (String, String, String, Option<i64>, Option<String>, Option<String>);
    let (issues_json, start, end, received, message_id, client): Own = sqlx::query_as(
        "SELECT issues_json, period_start, period_end, email_received_at, message_id, client FROM timesheet_records WHERE id = ?1 AND workspace_id = ?2",
    )
    .bind(id)
    .bind(ws)
    .fetch_one(&mut *conn)
    .await?;
    let mut issues: Vec<Value> = serde_json::from_str(&issues_json).unwrap_or_default();
    issues.retain(|i| i["code"] != "person_unmatched" && i["code"] != "ambiguous_person" && i["code"] != "possible_duplicate");
    let assumed = issues.iter().any(|i| i["code"] == "period_assumed");
    let me = Placed { id, message_id: message_id.as_deref(), client: client.as_deref(), start: &start, end: &end, received: received.unwrap_or(0), assumed };
    let settled = settle(&mut *conn, ws, person_id, &me, now).await?;
    if let Some((a, b)) = &settled.duplicate_of {
        issues.push(json!(duplicate_issue(a, b)));
    }
    let status = if settled.superseded {
        "replaced"
    } else if issues.iter().any(|i| i["severity"] == CHECK) {
        "needs_check"
    } else {
        "recorded"
    };
    sqlx::query(
        "UPDATE timesheet_records SET person_id = ?1, match_method = 'manual', match_confidence = 1, issues_json = ?2, status = ?3,
                replaces_id = COALESCE(?4, replaces_id), updated_at = ?5 WHERE id = ?6 AND workspace_id = ?7",
    )
    .bind(person_id)
    .bind(serde_json::to_string(&issues).unwrap_or_else(|_| "[]".into()))
    .bind(status)
    .bind(settled.replaced.first())
    .bind(now)
    .bind(id)
    .bind(ws)
    .execute(conn)
    .await?;
    Ok(())
}
