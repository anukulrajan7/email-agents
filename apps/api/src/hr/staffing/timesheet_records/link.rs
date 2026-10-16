use super::*;

/// What "this sheet is that person" did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Linked {
    Done,
    NotFound,
    /// The employee ID or name is already remembered for someone else; nothing was changed.
    Conflict(String),
}

/// What a link is about to do: the sheets that join the person, and the ID / name links to remember.
pub(crate) struct Plan {
    pub record_ids: Vec<String>,
    pub aliases: Vec<(&'static str, String)>,
}

/// What a link did, enough to put everything back: each sheet as it was, the person's other sheets as they were, and the
/// links that were new.
pub(crate) struct Applied {
    pub before: Vec<Value>,
    pub person_before: Vec<Value>,
    pub new_aliases: Vec<Value>,
}

/// Who a remembered ID or name already belongs to, if it is someone other than `person_id`.
async fn alias_owner(conn: &mut SqliteConnection, ws: &str, kind: &str, value: &str, person_id: &str) -> sqlx::Result<Option<String>> {
    let owner: Option<(String, String)> = sqlx::query_as(
        "SELECT a.person_id, COALESCE(p.name, 'another person') FROM timesheet_aliases a LEFT JOIN people p ON p.id = a.person_id
         WHERE a.workspace_id = ?1 AND a.kind = ?2 AND a.value = ?3",
    )
    .bind(ws)
    .bind(kind)
    .bind(value)
    .fetch_optional(conn)
    .await?;
    Ok(owner.filter(|(owner, _)| owner != person_id).map(|(_, name)| name))
}

/// An employee ID, or a name, to remember for a person: `(kind, value kept, value shown to people)`.
type Wanted = Vec<(&'static str, String, String)>;

fn wanted_aliases(code: Option<&str>, name: Option<&str>, remember_name: bool) -> Wanted {
    let mut out: Wanted = vec![];
    if let Some(code) = code {
        out.push(("code", code.to_lowercase(), code.to_uppercase()));
    }
    if remember_name && let Some(name) = name && let key = name_key(name) && !key.is_empty() {
        out.push(("name", key, name.to_string()));
    }
    out
}

async fn refuse_taken(conn: &mut SqliteConnection, ws: &str, wanted: &Wanted, person_id: &str) -> sqlx::Result<Option<Linked>> {
    for (kind, value, shown) in wanted {
        if let Some(owner) = alias_owner(conn, ws, kind, value, person_id).await? {
            let what = if *kind == "code" { "employee ID" } else { "name" };
            return Ok(Some(Linked::Conflict(format!(
                "The {what} {shown} already belongs to {owner}. If that link is wrong, the workspace owner can forget it (Forget this link), and then you can link it again."
            ))));
        }
    }
    Ok(None)
}

/// A person says "this sheet is that person". The other unmatched sheets that carry the same employee ID join it (the ID
/// is the evidence), and so do the ones with the same name, but only when the person confirmed the name (`remember_name`)
/// and only if they do not name a different ID. A remembered ID or name that belongs to someone else is never re-pointed.
pub(crate) async fn plan_record(conn: &mut SqliteConnection, ws: &str, record_id: &str, person_id: &str, remember_name: bool) -> sqlx::Result<Result<Plan, Linked>> {
    let known: Option<i64> = sqlx::query_scalar("SELECT 1 FROM people WHERE id = ?1 AND workspace_id = ?2 AND active = 1").bind(person_id).bind(ws).fetch_optional(&mut *conn).await?;
    let rec: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT employee_code, person_name FROM timesheet_records WHERE id = ?1 AND workspace_id = ?2").bind(record_id).bind(ws).fetch_optional(&mut *conn).await?;
    let (Some(_), Some((code, name))) = (known, rec) else { return Ok(Err(Linked::NotFound)) };
    let code = code.map(|c| c.to_lowercase());
    let key = name.as_deref().map(name_key).filter(|k| !k.is_empty());
    let wanted = wanted_aliases(code.as_deref(), name.as_deref(), remember_name);
    if let Some(refusal) = refuse_taken(conn, ws, &wanted, person_id).await? {
        return Ok(Err(refusal));
    }
    let others: Vec<(String, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT id, employee_code, person_name FROM timesheet_records WHERE workspace_id = ?1 AND person_id IS NULL AND status != 'replaced' ORDER BY created_at, id").bind(ws).fetch_all(&mut *conn).await?;
    let mut record_ids = vec![];
    for (id, other_code, other_name) in others {
        let other_code = other_code.map(|c| c.to_lowercase());
        let same_id = code.is_some() && other_code == code;
        let same_name = remember_name && key.is_some() && other_name.as_deref().map(name_key) == key && (other_code.is_none() || other_code == code);
        if id == record_id || same_id || same_name {
            record_ids.push(id);
        }
    }
    Ok(Ok(Plan { record_ids, aliases: wanted.into_iter().map(|(kind, value, _)| (kind, value)).collect() }))
}

/// An admin says "this card is that person": every sheet waiting under the card joins the person, and the card's employee ID
/// is remembered (and its name, when the admin asked).
pub(crate) async fn plan_identity(conn: &mut SqliteConnection, ws: &str, identity_id: &str, person_id: &str, remember_name: bool) -> sqlx::Result<Result<Plan, Linked>> {
    let known: Option<i64> = sqlx::query_scalar("SELECT 1 FROM people WHERE id = ?1 AND workspace_id = ?2 AND active = 1").bind(person_id).bind(ws).fetch_optional(&mut *conn).await?;
    let group: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT employee_code, name_as_written FROM unresolved_identities WHERE id = ?1 AND workspace_id = ?2").bind(identity_id).bind(ws).fetch_optional(&mut *conn).await?;
    let (Some(_), Some((code, name))) = (known, group) else { return Ok(Err(Linked::NotFound)) };
    let wanted = wanted_aliases(code.as_deref(), name.as_deref(), remember_name);
    if let Some(refusal) = refuse_taken(conn, ws, &wanted, person_id).await? {
        return Ok(Err(refusal));
    }
    let record_ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM timesheet_records WHERE workspace_id = ?1 AND identity_id = ?2 AND person_id IS NULL AND status != 'replaced' ORDER BY created_at, id",
    )
    .bind(ws)
    .bind(identity_id)
    .fetch_all(conn)
    .await?;
    Ok(Ok(Plan { record_ids, aliases: wanted.into_iter().map(|(kind, value, _)| (kind, value)).collect() }))
}

/// Issue codes a link clears from a sheet; `undo_link` puts back the ones the sheet had.
const CLEARED: [&str; 3] = ["person_unmatched", "ambiguous_person", "possible_duplicate"];

/// Does the plan: remembers the new links and gives each sheet its person. Everything it changed is returned for undo, as
/// ids and statuses only: no name, no issue text and no email is copied into the change log.
pub(crate) async fn apply(conn: &mut SqliteConnection, ws: &str, person_id: &str, plan: &Plan) -> sqlx::Result<Applied> {
    let now = now_ms();
    type Snap = (String, String, String, Option<String>, String);
    let snapshot = |r: Snap| {
        let issues: Vec<Value> = serde_json::from_str(&r.4).unwrap_or_default();
        let cleared: Vec<&str> = CLEARED.iter().copied().filter(|c| issues.iter().any(|i| i["code"] == *c)).collect();
        json!({"id": r.0, "match_method": r.1, "status": r.2, "replaces_id": r.3, "cleared": cleared})
    };
    let ids = json!(plan.record_ids).to_string();
    // The person's other sheets a link may touch: the ones whose period overlaps a sheet that joins.
    let person_before: Vec<Snap> = sqlx::query_as(
        "SELECT o.id, o.match_method, o.status, o.replaces_id, o.issues_json FROM timesheet_records o
         WHERE o.workspace_id = ?1 AND o.person_id = ?2 AND EXISTS (
           SELECT 1 FROM timesheet_records l WHERE l.workspace_id = ?1 AND l.id IN (SELECT value FROM json_each(?3)) AND o.period_start <= l.period_end AND o.period_end >= l.period_start)",
    )
    .bind(ws)
    .bind(person_id)
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await?;
    let mut before = vec![];
    for id in &plan.record_ids {
        let row: Option<Snap> =
            sqlx::query_as("SELECT id, match_method, status, replaces_id, issues_json FROM timesheet_records WHERE id = ?1 AND workspace_id = ?2").bind(id).bind(ws).fetch_optional(&mut *conn).await?;
        before.extend(row.map(snapshot));
    }
    let mut new_aliases = vec![];
    for (kind, value) in &plan.aliases {
        let done = sqlx::query("INSERT OR IGNORE INTO timesheet_aliases (workspace_id, kind, value, person_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5)")
            .bind(ws)
            .bind(kind)
            .bind(value)
            .bind(person_id)
            .bind(now)
            .execute(&mut *conn)
            .await?
            .rows_affected();
        if done == 1 {
            new_aliases.push(json!({"kind": kind, "value": value}));
        }
    }
    for id in &plan.record_ids {
        attach_person(conn, ws, id, person_id, now).await?;
    }
    Ok(Applied { before, person_before: person_before.into_iter().map(snapshot).collect(), new_aliases })
}

/// Puts back what `apply` changed. The sheets go back to "who is this" and are marked `needs_check`: an undo is never a
/// silent edit, a person looks again. A sheet of the person that the link had replaced comes back only while nothing newer
/// stands for the same days. The links it remembered are forgotten (only while they still point at this person).
pub(crate) async fn undo_link(conn: &mut SqliteConnection, ws: &str, person_id: &str, before: &[Value], person_before: &[Value], new_aliases: &[Value]) -> sqlx::Result<Vec<String>> {
    let now = now_ms();
    let linked: Vec<String> = before.iter().filter_map(|s| s["id"].as_str().map(str::to_string)).collect();
    let linked_json = json!(linked).to_string();
    // Sheets this link replaced, read before the linked sheets forget what they replaced.
    let replaced_here: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT replaces_id FROM timesheet_records WHERE workspace_id = ?1 AND id IN (SELECT value FROM json_each(?2)) AND person_id = ?3 AND replaces_id IS NOT NULL",
    )
    .bind(ws)
    .bind(&linked_json)
    .bind(person_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut restored = vec![];
    for snap in before {
        let Some(id) = snap["id"].as_str() else { continue };
        let cleared: Vec<&str> = snap["cleared"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        let (written, code, current): (Option<String>, Option<String>, String) = sqlx::query_as("SELECT person_name, employee_code, issues_json FROM timesheet_records WHERE id = ?1 AND workspace_id = ?2")
            .bind(id)
            .bind(ws)
            .fetch_one(&mut *conn)
            .await?;
        let mut issues: Vec<Value> = serde_json::from_str(&current).unwrap_or_default();
        for code_cleared in cleared {
            if issues.iter().any(|i| i["code"] == code_cleared) {
                continue;
            }
            issues.push(json!(match code_cleared {
                "person_unmatched" => {
                    let who = match (written.as_deref(), code.as_deref()) {
                        (Some(n), Some(c)) => format!("{n}, ID {c}"),
                        (Some(n), None) => n.to_string(),
                        (None, Some(c)) => format!("ID {c}"),
                        (None, None) => "no name or ID in the sheet".to_string(),
                    };
                    check("person_unmatched", format!("Who is this? ({who})"))
                }
                "ambiguous_person" => note("ambiguous_person", "More than one person has this name."),
                _ => note("possible_duplicate", "This looks like the same sheet as one already received for this name."),
            }));
        }
        let moved = sqlx::query(
            "UPDATE timesheet_records SET person_id = NULL, match_method = ?1, match_confidence = NULL, status = 'needs_check', issues_json = ?2, replaces_id = ?3, updated_at = ?4
             WHERE id = ?5 AND workspace_id = ?6 AND person_id = ?7",
        )
        .bind(snap["match_method"].as_str().unwrap_or("none"))
        .bind(Value::Array(issues).to_string())
        .bind(snap["replaces_id"].as_str())
        .bind(now)
        .bind(id)
        .bind(ws)
        .bind(person_id)
        .execute(&mut *conn)
        .await?
        .rows_affected();
        if moved == 1 {
            restored.push(id.to_string());
        }
    }
    for snap in person_before {
        let Some(id) = snap["id"].as_str() else { continue };
        let was = snap["status"].as_str().unwrap_or("recorded");
        let cleared_dup = snap["cleared"].as_array().is_some_and(|a| a.iter().any(|c| c == "possible_duplicate"));
        let mine = replaced_here.iter().any(|r| r == id);
        if mine && was != "replaced" {
            // Only while no other sheet of the person (not one of the linked ones) covers any of those days: a newer sheet that
            // replaced the linked one already stands for this period, and the old sheet stays replaced.
            sqlx::query(
                "UPDATE timesheet_records SET status = ?1, updated_at = ?2 WHERE id = ?3 AND workspace_id = ?4 AND status = 'replaced'
                   AND NOT EXISTS (SELECT 1 FROM timesheet_records n WHERE n.workspace_id = ?4 AND n.person_id = timesheet_records.person_id AND n.id != timesheet_records.id AND n.status != 'replaced'
                                     AND n.id NOT IN (SELECT value FROM json_each(?5)) AND n.period_start <= timesheet_records.period_end AND n.period_end >= timesheet_records.period_start)",
            )
            .bind(was)
            .bind(now)
            .bind(id)
            .bind(ws)
            .bind(&linked_json)
            .execute(&mut *conn)
            .await?;
        }
        if !cleared_dup {
            let now_issues: Option<String> = sqlx::query_scalar("SELECT issues_json FROM timesheet_records WHERE id = ?1 AND workspace_id = ?2").bind(id).bind(ws).fetch_optional(&mut *conn).await?;
            let mut issues: Vec<Value> = serde_json::from_str(&now_issues.unwrap_or_default()).unwrap_or_default();
            let len = issues.len();
            issues.retain(|i| i["code"] != "possible_duplicate");
            if issues.len() != len {
                sqlx::query("UPDATE timesheet_records SET issues_json = ?1 WHERE id = ?2 AND workspace_id = ?3").bind(Value::Array(issues).to_string()).bind(id).bind(ws).execute(&mut *conn).await?;
            }
        }
    }
    for alias in new_aliases {
        sqlx::query("DELETE FROM timesheet_aliases WHERE workspace_id = ?1 AND kind = ?2 AND value = ?3 AND person_id = ?4")
            .bind(ws)
            .bind(alias["kind"].as_str().unwrap_or_default())
            .bind(alias["value"].as_str().unwrap_or_default())
            .bind(person_id)
            .execute(&mut *conn)
            .await?;
    }
    Ok(restored)
}

/// The workspace owner forgets a remembered employee ID or name: the next sheet with it is unmatched until someone links it
/// again. Returns the person it pointed to (for the log), `None` when nothing was remembered.
pub async fn forget_alias(conn: &mut SqliteConnection, ws: &str, kind: &str, raw_value: &str) -> sqlx::Result<Option<(String, Option<String>)>> {
    let value = if kind == "code" { raw_value.trim().to_lowercase() } else { name_key(raw_value) };
    if value.is_empty() {
        return Ok(None);
    }
    let owner: Option<(String, Option<String>)> = sqlx::query_as(
        "DELETE FROM timesheet_aliases WHERE workspace_id = ?1 AND kind = ?2 AND value = ?3
         RETURNING person_id, (SELECT name FROM people p WHERE p.id = timesheet_aliases.person_id)",
    )
    .bind(ws)
    .bind(kind)
    .bind(value)
    .fetch_optional(conn)
    .await?;
    Ok(owner)
}

/// The older, per-sheet way to link (before the "people we don't know yet" cards): used while that feature is off.
pub async fn link_person(db: &SqlitePool, ws: &str, record_id: &str, person_id: &str, remember_name: bool) -> sqlx::Result<Linked> {
    let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
    let plan = match plan_record(&mut tx, ws, record_id, person_id, remember_name).await? {
        Ok(plan) => plan,
        Err(refusal) => return Ok(refusal),
    };
    apply(&mut tx, ws, person_id, &plan).await?;
    tx.commit().await?;
    Ok(Linked::Done)
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
