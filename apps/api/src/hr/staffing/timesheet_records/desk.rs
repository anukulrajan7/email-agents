//! The timesheet desk's use of the rules: read the records an email left, run `check_against_rules` on each, and
//! turn the answers into one `Decision`. In act mode the rule issues are written onto the records (a wrong month makes
//! the record `needs_check`); in compare mode nothing is written and only the audit trail learns what would happen.

use serde_json::json;

use super::{
    rules::{self, Checked, PersonRules, RecordView, Rules, RuleIssue, Severity},
    *,
};
use crate::agents::handoff::{Decision, MessageRequest};
use crate::{
    hr::team::settings::Cadence,
    platform::audit::{self, Actor, Entry},
};

/// Codes the rules own on a record. Everything else in `issues_json` was found when the sheet was read.
const RULE_CODES: [&str; 8] = ["missing_data", "wrong_period", "over_max_per_day", "portal_mismatch", "missing_signature", "below_target", "outside_client_domains", "replaces_earlier"];

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
    // A card the admin set aside (outside staff, not a person) is not an unknown person to be added: no receipt, nothing parked.
    let set_aside: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM unresolved_identities WHERE workspace_id = ?1 AND status IN ('external', 'dismissed') AND id IN (SELECT value FROM json_each(?2))",
    )
    .bind(ws)
    .bind(json!(rows.iter().filter_map(|r| r.identity_id.clone()).collect::<Vec<_>>()).to_string())
    .fetch_all(db)
    .await?;
    let (mut all, mut checked) = (vec![], vec![]);
    for row in &rows {
        let (entries, issues) = (row.entries(), reader_issues(row));
        let (portal, person, replaced) = (portal_hours(db, ws, row.person_id.as_deref(), (&row.period_start, &row.period_end), rules.cadence).await?, person_rules(db, ws, row.person_id.as_deref()).await?, replaced_received(db, ws, row).await?);
        let domain = domain_of(row.sender_address.as_deref());
        let written: parse::Identity = serde_json::from_str(&row.identity_json).unwrap_or_default();
        let filled = |v: &Option<String>| v.as_deref().is_some_and(|x| !x.trim().is_empty());
        let (person_has_code, sender_on_roster) = match row.person_id.as_deref() {
            Some(p) => (
                sqlx::query_scalar::<_, Option<String>>("SELECT employee_code FROM people WHERE id = ?1 AND workspace_id = ?2").bind(p).bind(ws).fetch_optional(db).await?.flatten().is_some_and(|c| !c.trim().is_empty()),
                sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM person_emails WHERE workspace_id = ?1 AND person_id = ?2 AND lower(email) = ?3)")
                    .bind(ws)
                    .bind(p)
                    .bind(row.sender_address.as_deref().map(address_of).unwrap_or_default())
                    .fetch_one(db)
                    .await?,
            ),
            None => (false, false),
        };
        let view = RecordView {
            written_name: filled(&written.name),
            written_code: filled(&written.code),
            person_has_code,
            sender_on_roster,
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
        let mut found = rules::check_against_rules(&view, &rules, person.as_ref());
        let missing = rules::missing_fields(&view, &rules);
        if !missing.is_empty() {
            let what = missing.iter().map(|f| f.label().to_lowercase()).collect::<Vec<_>>().join(", ");
            found.push(RuleIssue { code: "missing_data", severity: Severity::Missing, for_sender: String::new(), for_manager: format!("Waiting for the sender to supply: {what}.") });
        }
        checked.push(Checked {
            missing,
            record_id: row.id.clone(),
            person: row.linked_name.clone().or_else(|| row.person_name.clone()).filter(|_| row.person_id.is_some()),
            period: (row.period_start.clone(), row.period_end.clone()),
            total: row.total_hours,
            issues: found.clone(),
            identity: row
                .person_name
                .clone()
                .filter(|n| !n.trim().is_empty())
                .filter(|_| row.person_id.is_none() && row.employee_code.as_deref().is_some_and(|c| !c.trim().is_empty()))
                .filter(|_| !row.identity_id.as_ref().is_some_and(|g| set_aside.contains(g))),
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
    // What Meera would be asked to send to the sender, and at whose level: logged only, nothing is filed or drafted.
    let clean = review.all_matched() && review.rows.iter().all(|r| r.status == "recorded");
    let would_send = match (&review.decision.message, &owner) {
        (Some(m), crate::agents::ownership::Owner::Assigned(jobs)) if m.kind != crate::agents::handoff::MessageKind::Ack || clean => {
            let level = match jobs.first() {
                Some(j) => crate::agents::registry::runtime::level_of(&state.db, &state.config, ws, &j.agent_id, message_id).await?,
                None => 0,
            };
            Some(json!({"kind": m.kind, "to": m.to, "issues": m.issues.len(), "requester_level": level, "goes_out_on_its_own": level >= 2}))
        }
        _ => None,
    };
    let decision = json!(review.decision);
    // Whether a reply would be owed to the sender (and why not), as the duty would decide it. Nothing is written.
    let would_owe_reply = match &owner {
        crate::agents::ownership::Owner::Assigned(_) => {
            let cfg = settings::load(&state.db, ws).await?;
            crate::agents::supervisor::sla::assess(state, ws, message_id, &cfg).await?.map(|a| json!({"excused": a.excuse}))
        }
        _ => None,
    };
    let would_ask = (!review.decision.asks.is_empty()).then(|| json!(review.decision.asks.iter().map(|a| a.key()).collect::<Vec<_>>()));
    let digest = format!("{:x}", fingerprint(&json!({"d": decision, "t": would_tell, "a": would_assign, "s": would_send, "o": would_owe_reply, "k": would_ask}).to_string()));
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
        json!({ "decision": decision, "would_assign": would_assign, "would_tell": would_tell, "would_send": would_send, "would_owe_reply": would_owe_reply, "would_ask": would_ask, "records": review.record_ids(), "digest": digest }),
    )
    .await
}

/// A short stable digest of a string, enough to tell two decisions apart.
fn fingerprint(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// Most receipts one address gets in a day: a receipt is a free answer to anyone who mails a sheet, so it is capped.
pub const RECEIPTS_PER_SENDER_PER_DAY: i64 = 3;
/// Most receipts one workspace sends in an hour, whoever the senders are.
pub const RECEIPTS_PER_WORKSPACE_PER_HOUR: i64 = 20;

/// May this sender get a receipt? Only someone we can tell is real by something a stranger cannot make up: an address on the
/// roster, an address at a company's own domain (never a free-mail one), or an address we have already written to. Mail a sender
/// merely sent before, or a thread they claim to be in, proves nothing (any From: can be forged). And at most
/// `RECEIPTS_PER_SENDER_PER_DAY` to one address and `RECEIPTS_PER_WORKSPACE_PER_HOUR` in all. The request already filed for this
/// email never counts against the caps.
pub(crate) async fn receipt_allowed(state: &AppState, ws: &str, message_id: &str) -> sqlx::Result<bool> {
    let Some(sender): Option<String> = sqlx::query_scalar("SELECT sender_address FROM email_messages WHERE id = ?1 AND workspace_id = ?2").bind(message_id).bind(ws).fetch_optional(&state.db).await? else {
        return Ok(false);
    };
    let address = crate::replies::send_safety::bare_address(&sender).to_lowercase();
    let now = now_ms();
    let day: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM message_requests r JOIN email_messages m ON m.id = r.message_id
         WHERE r.workspace_id = ?1 AND r.job LIKE '%:receipt' AND r.status != 'dropped' AND r.message_id != ?2 AND r.created_at > ?3 AND lower(m.sender_address) = ?4",
    )
    .bind(ws)
    .bind(message_id)
    .bind(now - clock::DAY_MS)
    .bind(&address)
    .fetch_one(&state.db)
    .await?;
    let hour: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM message_requests WHERE workspace_id = ?1 AND job LIKE '%:receipt' AND status != 'dropped' AND message_id != ?2 AND created_at > ?3")
        .bind(ws)
        .bind(message_id)
        .bind(now - 3_600_000)
        .fetch_one(&state.db)
        .await?;
    if day >= RECEIPTS_PER_SENDER_PER_DAY || hour >= RECEIPTS_PER_WORKSPACE_PER_HOUR {
        return Ok(false);
    }
    let on_roster: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM person_emails WHERE workspace_id = ?1 AND lower(email) = ?2)").bind(ws).bind(&address).fetch_one(&state.db).await?;
    if on_roster {
        return Ok(true);
    }
    let mut conn = state.db.acquire().await?;
    let domain = address.rsplit_once('@').map(|(_, d)| d.to_string()).unwrap_or_default();
    let company = crate::hr::team::identity::unresolved::company_domains(&mut conn, ws).await?;
    if crate::hr::team::identity::resolve::is_company_domain(&domain, &company) {
        return Ok(true);
    }
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM email_messages o, json_each(o.recipients_json) j
                       WHERE o.workspace_id = ?1 AND o.direction = 'outbound' AND (lower(j.value) = ?2 OR lower(j.value) LIKE '%<' || ?2 || '>'))",
    )
    .bind(ws)
    .bind(address)
    .fetch_one(&mut *conn)
    .await
}

/// Files what the desk wants said to the sender with Meera. A "recorded" acknowledgement is only filed when every sheet is
/// recorded and matched to a person, the same rule the manager summary follows. A receipt for a sheet whose person is not on
/// the roster is its own request (`desk:timesheet:receipt`), and the acknowledgement that waits for the link is parked.
pub async fn hand_off(state: &AppState, ws: &str, asker: &crate::agents::handoff::Asker<'_>, message_id: &str, decision: &Decision, clean: bool) -> sqlx::Result<Option<crate::agents::handoff::Filed>> {
    use crate::agents::handoff::{self, Asker, MessageKind};
    let mut filed = None;
    let wanted = decision.message.as_ref().filter(|m| m.kind != MessageKind::Ack || clean);
    // Details already asked for (or an owner's answer awaited) are not asked again, and a loop that ended without an answer never restarts.
    let ask_blocked = wanted.is_some_and(MessageRequest::is_ask) && !crate::hr::team::info_requests::may_ask(&state.db, ws, message_id).await?;
    let receipt_job = format!("{}:receipt", asker.job);
    if wanted.is_none() || ask_blocked {
        // What the desk asked before no longer stands (the sheet changed, or its person is unknown again): withdraw it.
        if !ask_blocked {
            handoff::store::withdraw(state, ws, asker, message_id).await?;
            if crate::hr::team::info_requests::latest_for(&state.db, ws, message_id).await?.is_some_and(|r| matches!(r.state.as_str(), "asking" | "waiting")) {
                handoff::store::withdraw(state, ws, &Asker { job: &receipt_job, ..*asker }, message_id).await?;
            }
            crate::hr::team::info_requests::settle_complete(state, ws, message_id).await?;
        }
    }
    if let Some(message) = wanted.filter(|_| !ask_blocked) {
        let is_ask = message.is_ask();
        let receipt = message.kind == MessageKind::Holding || is_ask;
        let by = if receipt { Asker { job: &receipt_job, ..*asker } } else { *asker };
        if receipt && !receipt_allowed(state, ws, message_id).await? {
            let mut conn = state.db.acquire().await?;
            let action = if is_ask { "handoff.ask_skipped" } else { "handoff.receipt_skipped" };
            audit::record(&mut conn, Entry { workspace_id: ws, actor: Actor::System, action, entity_type: "message", entity_id: message_id, message_id: Some(message_id), metadata: json!({}) }).await?;
        } else {
            let result = handoff::hand_to_meera(state, ws, &by, message_id, message).await?;
            if is_ask && !matches!(result, handoff::Filed::Dropped(_) | handoff::Filed::Late) {
                crate::hr::team::info_requests::upsert_asks(state, ws, message_id, asker.agent_id, &decision.asks).await?;
            }
            filed = Some(result);
        }
    }
    if let Some(parked) = &decision.parked {
        handoff::park(state, ws, asker, message_id, parked).await?;
    }
    Ok(filed)
}

/// A person was linked to sheets of this email: the desk looks at the email again. What was parked for the link is now
/// decided on the facts as they are (clean: the acknowledgement; a rule that needed the person: a correction or a question),
/// the manager is told once, and the "check this timesheet" item closes when nothing is left to check. Looking again at an
/// email whose reply already went out files nothing new.
pub async fn resume(state: &AppState, ws: &str, message_id: &str) -> sqlx::Result<()> {
    let mut review = review(&state.db, ws, message_id).await?;
    if review.rows.is_empty() {
        return Ok(());
    }
    let parked_by: Option<String> = sqlx::query_scalar("SELECT from_agent_id FROM message_requests WHERE workspace_id = ?1 AND message_id = ?2 AND job = ?3 AND (status = 'waiting' OR (status = 'pending' AND draft_id IS NULL AND digest = '')) LIMIT 1")
        .bind(ws)
        .bind(message_id)
        .bind(crate::agents::ownership::DESK_JOB)
        .fetch_optional(&state.db)
        .await?;
    let statuses = apply(&state.db, ws, &mut review).await?;
    // Only an email the desk itself parked an acknowledgement for is looked at again (still waiting, or released but not yet written:
    // a retry after a failure between the two finds it). Sheets recorded before the desk acted on
    // them (the flag was off, or an old card) get their person and nothing more: no mail, no summary, no surprise.
    let Some(agent) = parked_by else { return Ok(()) };
    let mut conn = state.db.acquire().await?;
    audit::record(
        &mut conn,
        Entry { workspace_id: ws, actor: Actor::System, action: "handoff.resumed", entity_type: "message", entity_id: message_id, message_id: Some(message_id), metadata: json!({ "decision": review.decision, "records": review.record_ids() }) },
    )
    .await?;
    drop(conn);
    summary::summarize(state, ws, Some(&agent), &review.rows, &review.decision).await?;
    let clean = review.all_matched() && statuses.iter().all(|s| s == "recorded");
    if clean {
        sqlx::query(
            "UPDATE review_items SET status = 'closed' WHERE workspace_id = ?1 AND message_id = ?2 AND status = 'open' AND ((kind = 'agent_task' AND title = 'Check: Timesheet') OR kind = 'check_timesheet')",
        )
        .bind(ws)
        .bind(message_id)
        .execute(&state.db)
        .await?;
    }
    let asker = crate::agents::handoff::Asker { agent_id: &agent, case_id: None, job: crate::agents::ownership::DESK_JOB };
    // The parked acknowledgement is released only when nothing is parked any more: a sheet in the same email whose person is still
    // unknown keeps it waiting (the receipt is not an answer).
    let ready = review.decision.parked.is_none() && review.decision.message.as_ref().is_some_and(|m| m.kind != crate::agents::handoff::MessageKind::Ack || clean);
    if ready {
        crate::agents::handoff::unpark(state, ws, message_id, crate::agents::ownership::DESK_JOB).await?;
    }
    hand_off(state, ws, &asker, message_id, &review.decision, clean).await?;
    Ok(())
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
    crate::agents::gaps::record(conn, ws, crate::agents::gaps::NewGap { tool: "read_signature", why, agent_id, message_id: Some(message_id), source: "rule" }).await?;
    Ok(())
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
    drop(conn);
    let agent = jobs.first().map(|j| j.agent_id.as_str());
    let told = summary::summarize(state, ws, agent, &review.rows, &review.decision).await?;
    let clean = review.all_matched() && statuses.iter().all(|s| s == "recorded");
    // As in the desk case: when the manager was asked to decide a portal question, the summary is the ask, not a second item.
    let manager_decides = told > 0 && review.decision.message.as_ref().is_some_and(|m| m.kind == crate::agents::handoff::MessageKind::Question);
    if !clean && !manager_decides {
        // One review item per sheet: the desk case's task, this one and the old workflow's item count as the same ask.
        sqlx::query(
            "INSERT INTO review_items (id, workspace_id, message_id, workflow_run_id, kind, title, status, created_at)
             SELECT ?1, ?2, ?3, NULL, 'agent_task', 'Check: Timesheet', 'open', ?4
             WHERE NOT EXISTS (SELECT 1 FROM review_items WHERE message_id = ?3 AND status = 'open'
                               AND ((kind = 'agent_task' AND title = 'Check: Timesheet') OR kind = 'check_timesheet'))",
        )
        .bind(new_id("rev"))
        .bind(ws)
        .bind(message_id)
        .bind(now_ms())
        .execute(&state.db)
        .await?;
    }
    if let Some(agent_id) = agent {
        let asker = crate::agents::handoff::Asker { agent_id, case_id: None, job: crate::agents::ownership::DESK_JOB };
        hand_off(state, ws, &asker, message_id, &review.decision, clean).await?;
    }
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
