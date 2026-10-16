//! The timesheet desk's use of the rules: read the records an email left, run `check_against_rules` on each, and
//! turn the answers into one `Decision`. In act mode the rule issues are written onto the records (a wrong month makes
//! the record `needs_check`); in compare mode nothing is written and only the audit trail learns what would happen.

use serde_json::json;

use super::{
    rules::{self, Checked, Decision, PersonRules, RecordView, Rules, RuleIssue, Severity},
    *,
};
use crate::{
    hr::team::settings::Cadence,
    platform::audit::{self, Actor, Entry},
};

/// Codes the rules own on a record. Everything else in `issues_json` was found when the sheet was read.
const RULE_CODES: [&str; 7] = ["wrong_period", "over_max_per_day", "portal_mismatch", "missing_signature", "below_target", "outside_client_domains", "replaces_earlier"];

pub struct Review {
    pub rows: Vec<RecordRow>,
    pub issues: Vec<Vec<RuleIssue>>,
    pub decision: Decision,
    /// The owner wants signatures but the reader cannot see one yet: the caller records the tool gap.
    pub signature_gap: bool,
}

impl Review {
    pub fn all_matched(&self) -> bool {
        self.rows.iter().all(|r| r.person_id.is_some())
    }

    pub fn record_ids(&self) -> Vec<String> {
        self.rows.iter().map(|r| r.id.clone()).collect()
    }
}

fn reader_issues(row: &RecordRow) -> Vec<Issue> {
    let stored: Vec<Value> = serde_json::from_str(&row.issues_json).unwrap_or_default();
    stored
        .iter()
        .filter_map(|i| {
            let code = i["code"].as_str()?;
            (!RULE_CODES.contains(&code)).then(|| Issue {
                code: code.to_string(),
                message: i["message"].as_str().unwrap_or_default().to_string(),
                severity: if i["severity"] == CHECK { CHECK } else { NOTE },
            })
        })
        .collect()
}

fn domain_of(address: Option<&str>) -> String {
    address.map(address_of).and_then(|a| a.rsplit_once('@').map(|(_, d)| d.to_string())).unwrap_or_default()
}

async fn person_rules(db: &SqlitePool, ws: &str, person_id: Option<&str>) -> sqlx::Result<Option<PersonRules>> {
    let Some(id) = person_id else { return Ok(None) };
    let row: Option<(Option<i64>, Option<i64>)> =
        sqlx::query_as("SELECT daily_target, weekly_target FROM people WHERE id = ?1 AND workspace_id = ?2").bind(id).bind(ws).fetch_optional(db).await?;
    Ok(row.map(|(daily_target, weekly_target)| PersonRules { daily_target, weekly_target }))
}

/// `YYYY-MM` when the period is exactly one whole calendar month: the portal counts hours by month, so only then
/// are the two figures about the same thing.
fn whole_month<'a>(period: (&'a str, &str)) -> Option<&'a str> {
    let month = period.0.get(..7)?;
    let (from, to) = clock::month_bounds(month, 0)?;
    (clock::parse_date(period.0)? == from / clock::DAY_MS && clock::parse_date(period.1)? == to / clock::DAY_MS - 1).then_some(month)
}

/// Hours the portal has for this person, when the owner expects monthly sheets and this one covers a whole month.
async fn portal_hours(db: &SqlitePool, ws: &str, person_id: Option<&str>, period: (&str, &str), cadence: Cadence) -> sqlx::Result<Option<f64>> {
    let (Some(person), Some(month), Cadence::Monthly) = (person_id, whole_month(period), cadence) else { return Ok(None) };
    sqlx::query_scalar("SELECT hours FROM portal_hours WHERE workspace_id = ?1 AND person_id = ?2 AND month = ?3").bind(ws).bind(person).bind(month).fetch_optional(db).await
}

async fn replaced_received(db: &SqlitePool, ws: &str, row: &RecordRow) -> sqlx::Result<Option<i64>> {
    let Some(id) = &row.replaces_id else { return Ok(None) };
    Ok(sqlx::query_scalar::<_, Option<i64>>("SELECT email_received_at FROM timesheet_records WHERE id = ?1 AND workspace_id = ?2").bind(id).bind(ws).fetch_optional(db).await?.flatten())
}

/// The decision for the records one email left. Reads only.
pub async fn review(db: &SqlitePool, ws: &str, message_id: &str) -> sqlx::Result<Review> {
    let rows: Vec<RecordRow> = sqlx::query_as(&format!("{SELECT} WHERE r.workspace_id = ?1 AND r.message_id = ?2 AND r.status != 'replaced' ORDER BY r.created_at, r.rowid"))
        .bind(ws)
        .bind(message_id)
        .fetch_all(db)
        .await?;
    let rules = Rules::from_settings(&settings::load(db, ws).await?);
    let (mut all, mut checked) = (vec![], vec![]);
    for row in &rows {
        let (entries, issues) = (row.entries(), reader_issues(row));
        let (portal, person, replaced) = (portal_hours(db, ws, row.person_id.as_deref(), (&row.period_start, &row.period_end), rules.cadence).await?, person_rules(db, ws, row.person_id.as_deref()).await?, replaced_received(db, ws, row).await?);
        let domain = domain_of(row.sender_address.as_deref());
        let view = RecordView {
            period: (&row.period_start, &row.period_end),
            total: row.total_hours,
            entries: &entries,
            issues: &issues,
            matched: row.person_id.is_some(),
            received_at: row.email_received_at.unwrap_or_else(now_ms),
            sender_domain: &domain,
            portal_hours: portal,
            signed: None,
            replaces_received_at: replaced,
        };
        let found = rules::check_against_rules(&view, &rules, person.as_ref());
        checked.push(Checked {
            record_id: row.id.clone(),
            person: row.linked_name.clone().or_else(|| row.person_name.clone()).filter(|_| row.person_id.is_some()),
            period: (row.period_start.clone(), row.period_end.clone()),
            total: row.total_hours,
            issues: found.clone(),
        });
        all.push(found);
    }
    Ok(Review { signature_gap: rules.require_signature && !rows.is_empty(), decision: rules::decision(&checked), issues: all, rows })
}

/// Writes the rule issues onto each record and sets its status from all its issues (the rows in `review` follow). Returns the statuses.
pub async fn apply(db: &SqlitePool, ws: &str, review: &mut Review) -> sqlx::Result<Vec<String>> {
    let mut statuses = vec![];
    for (row, found) in review.rows.iter_mut().zip(&review.issues) {
        let mut issues: Vec<Value> = serde_json::from_str(&row.issues_json).unwrap_or_default();
        issues.retain(|i| !i["code"].as_str().is_some_and(|c| RULE_CODES.contains(&c)));
        for i in found.iter().filter(|i| RULE_CODES.contains(&i.code)) {
            let severity = if matches!(i.severity, Severity::Note) { NOTE } else { CHECK };
            issues.push(json!({"code": i.code, "message": i.for_manager, "severity": severity}));
        }
        let status = if issues.iter().any(|i| i["severity"] == CHECK) { "needs_check" } else { "recorded" };
        sqlx::query("UPDATE timesheet_records SET issues_json = ?1, status = ?2, updated_at = ?3 WHERE id = ?4 AND workspace_id = ?5 AND status != 'replaced'")
            .bind(Value::Array(issues).to_string())
            .bind(status)
            .bind(now_ms())
            .bind(&row.id)
            .bind(ws)
            .execute(db)
            .await?;
        row.status = status.to_string();
        statuses.push(status.to_string());
    }
    Ok(statuses)
}

/// Compare mode: what the desk would have decided and done, written to the audit trail once per distinct outcome.
pub async fn compare(state: &AppState, ws: &str, message_id: &str) -> sqlx::Result<()> {
    let review = review(&state.db, ws, message_id).await?;
    if review.rows.is_empty() {
        return Ok(());
    }
    let owner = crate::agents::ownership::decide(state, ws, message_id).await?;
    let would_tell = match (&review.decision.manager_summary, &owner) {
        (Some(_), crate::agents::ownership::Owner::Assigned(_)) => super::summary::recipients(&state.db, ws, &review.rows).await?.len(),
        _ => 0,
    };
    let would_assign = match &owner {
        crate::agents::ownership::Owner::Assigned(jobs) => jobs.first().map(|j| json!({"job": j.key, "agent": j.agent_name})),
        _ => None,
    };
    let decision = json!(review.decision);
    let digest = format!("{:x}", fingerprint(&json!({"d": decision, "t": would_tell, "a": would_assign}).to_string()));
    let seen: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM audit_events WHERE workspace_id = ?1 AND action = 'ownership.compare' AND entity_id = ?2 AND json_extract(metadata_json, '$.digest') = ?3)",
    )
    .bind(ws)
    .bind(message_id)
    .bind(&digest)
    .fetch_one(&state.db)
    .await?;
    if seen {
        return Ok(());
    }
    let mut conn = state.db.acquire().await?;
    crate::agents::ownership::audit_owner(
        &mut conn,
        ws,
        message_id,
        "ownership.compare",
        &owner,
        json!({ "decision": decision, "would_assign": would_assign, "would_tell": would_tell, "records": review.record_ids(), "digest": digest }),
    )
    .await
}

/// A short stable digest of a string, enough to tell two decisions apart.
fn fingerprint(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

pub async fn audit_decision(conn: &mut SqliteConnection, ws: &str, message_id: &str, case_id: &str, review: &Review) -> sqlx::Result<()> {
    audit::record(
        conn,
        Entry {
            workspace_id: ws,
            actor: Actor::System,
            action: "handoff.decided",
            entity_type: "case",
            entity_id: case_id,
            message_id: Some(message_id),
            metadata: json!({ "decision": review.decision, "records": review.record_ids() }),
        },
    )
    .await
}

/// A manager (someone above the person in the directory) or an admin approves a sheet. Nobody approves their own, not
/// even an admin, and a replaced sheet cannot be approved. Approving twice keeps the first approval.
pub async fn approve(db: &SqlitePool, ws: &str, id: &str, a: &Access, user_id: &str) -> crate::platform::error::AppResult<RecordRow> {
    use crate::{hr::team::access::Level, platform::error::AppError};
    let row = get(db, ws, id).await?.ok_or(AppError::NotFound)?;
    let own = row.person_id.is_some() && row.person_id == a.person_id;
    let allowed = !own
        && match (&row.person_id, a.level) {
            (_, Level::SuperAdmin | Level::Admin) => true,
            (Some(p), Level::Operator) => a.can_see(p),
            (None, Level::Operator) => false,
        };
    if !allowed {
        return Err(AppError::Denied("Only this person's manager or an admin can approve a timesheet, and nobody approves their own".into()));
    }
    if row.status == "replaced" {
        return Err(AppError::Conflict("A newer timesheet replaced this one; approve the newer one".into()));
    }
    let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
    let changed = sqlx::query("UPDATE timesheet_records SET approved_by = ?1, approved_at = ?2 WHERE id = ?3 AND workspace_id = ?4 AND approved_at IS NULL")
        .bind(user_id)
        .bind(now_ms())
        .bind(id)
        .bind(ws)
        .execute(&mut *tx)
        .await?
        .rows_affected()
        == 1;
    if changed {
        audit::record(&mut tx, Entry { workspace_id: ws, actor: Actor::User(user_id), action: "timesheet.approved", entity_type: "timesheet_record", entity_id: id, message_id: row.message_id.as_deref(), metadata: json!({}) }).await?;
    }
    tx.commit().await?;
    get(db, ws, id).await?.ok_or(AppError::NotFound)
}

/// The owner wants signed sheets and the reader cannot see a signature yet: counted once per email, however many times
/// the desk looks at it.
pub async fn record_signature_gap(conn: &mut SqliteConnection, ws: &str, agent_id: Option<&str>, message_id: &str) -> sqlx::Result<()> {
    let seen: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tool_gaps WHERE workspace_id = ?1 AND tool = 'read_signature' AND message_id = ?2)")
        .bind(ws)
        .bind(message_id)
        .fetch_one(&mut *conn)
        .await?;
    if seen {
        return Ok(());
    }
    let why = "The owner asks for signed timesheets, but a signature cannot be read from a sheet yet.";
    crate::agents::gaps::record(conn, ws, crate::agents::gaps::NewGap { tool: "read_signature", why, agent_id, message_id: Some(message_id), source: "rule" }).await
}

/// A recent sheet whose records appeared after the desk's case had finished or never started (the files took longer than
/// the case waited, or the case could not start): check it, tell the manager once, and ask a person when it needs a look.
/// Does nothing when a live case will do it, or when a case already checked records of this email.
pub async fn catch_up(state: &AppState, ws: &str, message_id: &str) -> sqlx::Result<()> {
    let handled: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM cases WHERE workspace_id = ?1 AND message_id = ?2 AND job = ?3 AND shadow = 0 AND json_extract(state_json, '$.desk') = 1
           AND (status IN ('running', 'waiting_event', 'waiting_approval') OR json_array_length(COALESCE(json_extract(state_json, '$.record_ids'), '[]')) > 0))",
    )
    .bind(ws)
    .bind(message_id)
    .bind(crate::agents::ownership::DESK_JOB)
    .fetch_one(&state.db)
    .await?;
    if handled {
        return Ok(());
    }
    let owner = crate::agents::ownership::decide(state, ws, message_id).await?;
    let crate::agents::ownership::Owner::Assigned(jobs) = &owner else { return Ok(()) };
    let mut review = review(&state.db, ws, message_id).await?;
    if review.rows.is_empty() {
        return Ok(());
    }
    let statuses = apply(&state.db, ws, &mut review).await?;
    let mut conn = state.db.acquire().await?;
    if review.signature_gap {
        record_signature_gap(&mut conn, ws, jobs.first().map(|j| j.agent_id.as_str()), message_id).await?;
    }
    audit::record(
        &mut conn,
        Entry { workspace_id: ws, actor: Actor::System, action: "handoff.caught_up", entity_type: "message", entity_id: message_id, message_id: Some(message_id), metadata: json!({ "decision": review.decision, "records": review.record_ids() }) },
    )
    .await?;
    if statuses.iter().any(|s| s != "recorded") || !review.all_matched() {
        sqlx::query(
            "INSERT INTO review_items (id, workspace_id, message_id, workflow_run_id, kind, title, status, created_at)
             SELECT ?1, ?2, ?3, NULL, 'agent_task', 'Check: Timesheet', 'open', ?4
             WHERE NOT EXISTS (SELECT 1 FROM review_items WHERE message_id = ?3 AND kind = 'agent_task' AND title = 'Check: Timesheet' AND status = 'open')",
        )
        .bind(new_id("rev"))
        .bind(ws)
        .bind(message_id)
        .bind(now_ms())
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    summary::summarize(state, ws, jobs.first().map(|j| j.agent_id.as_str()), &review.rows, &review.decision).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_whole_calendar_month_is_compared_with_the_portal() {
        assert_eq!(whole_month(("2026-09-01", "2026-09-30")), Some("2026-09"));
        assert_eq!(whole_month(("2026-02-01", "2026-02-28")), Some("2026-02"));
        assert_eq!(whole_month(("2026-09-01", "2026-09-07")), None, "one week of September");
        assert_eq!(whole_month(("2026-09-02", "2026-09-30")), None);
        assert_eq!(whole_month(("2026-08-25", "2026-09-05")), None, "a period across two months");
    }
}
