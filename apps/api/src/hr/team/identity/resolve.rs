//! Who a written name might be, and what an admin may do about it. The rules are pure and strict: a name is only ever
//! a *suggestion* for another person's name, never a link. The actions (link, add as a person, mark external, not a
//! person, undo) each write one `identity_changes` row with what they touched.

use serde::Serialize;
use serde_json::{Value, json};
use sqlx::SqliteConnection;

use super::{
    IdentityError, changes,
    changes::NewChange,
    unresolved::{self, Group},
};
use crate::{
    hr::{
        staffing::{
            timesheet_parse::Candidate,
            timesheet_records::{self as records, Linked},
        },
        team::people::{self, Context, Fields},
    },
    http::AppState,
    jobs::{self, kind},
    platform::{
        audit::{self, Actor, Entry},
        db::now_ms,
    },
};

/// Providers that give mail addresses to anyone: the second-level name before the country part ("yahoo" in yahoo.co.in).
const FREE_MAIL_NAMES: [&str; 25] = [
    "gmail", "googlemail", "yahoo", "ymail", "outlook", "hotmail", "live", "msn", "icloud", "me", "mac", "proton", "protonmail", "pm", "aol", "zoho", "zohomail", "gmx", "mail", "rediffmail", "yandex",
    "fastmail", "tutanota", "hey", "inbox",
];

/// Is this a domain anyone can get an address on? Such a domain says nothing about who a sender is, so it is never a company's
/// own, whoever on the roster uses it.
pub fn is_free_mail(domain: &str) -> bool {
    let labels: Vec<&str> = domain.trim().trim_end_matches('.').split('.').collect();
    let n = labels.len();
    if n < 2 {
        return false;
    }
    let second_level = if n >= 3 && matches!(labels[n - 2], "co" | "com" | "org" | "net" | "ac" | "gov" | "edu") { labels[n - 3] } else { labels[n - 2] };
    FREE_MAIL_NAMES.iter().any(|f| f.eq_ignore_ascii_case(second_level))
}

/// Is `domain` a company's own: on the list of company domains (the roster's and the owner's client domains) and not a free-mail
/// one? The one rule for grouping sheets and for deciding who gets a receipt.
pub fn is_company_domain(domain: &str, company_domains: &[String]) -> bool {
    !domain.is_empty() && !is_free_mail(domain) && company_domains.iter().any(|d| d.eq_ignore_ascii_case(domain))
}

/// Where a sheet came from, as a grouping hint: the domain when it is a company's own, else the whole address. A domain many
/// unrelated people share (gmail, an agency) says nothing about who.
pub fn sender_hint(sender: &str, company_domains: &[String]) -> (String, String) {
    let address = sender.trim().trim_matches(['<', '>']).to_lowercase();
    let domain = address.rsplit_once('@').map(|(_, d)| d.to_string()).unwrap_or_default();
    let hint = if is_company_domain(&domain, company_domains) { domain.clone() } else { address.clone() };
    (domain, hint)
}

/// The group a sheet's identity belongs to: its employee ID, else its normalised name together with where it came from.
/// Two sheets with the same name from different places, or with different IDs, are never one group. `None` when the sheet
/// names nobody.
pub fn group_key(code: Option<&str>, name: Option<&str>, sender: &str, company_domains: &[String]) -> Option<String> {
    if let Some(code) = code.map(str::trim).filter(|c| !c.is_empty()) {
        return Some(format!("code:{}", code.to_lowercase()));
    }
    let key = crate::hr::staffing::name_key(name?);
    (!key.is_empty()).then(|| format!("name:{key}@{}", sender_hint(sender, company_domains).1))
}

/// One person the name might be. Evidence only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Suggestion {
    pub person_id: String,
    pub name: String,
    pub reason: String,
    /// Only part of a name in common (a first or last name, or one name inside the other): worth a second look before linking.
    #[serde(default)]
    pub weak: bool,
}

fn words(name: &str) -> Vec<String> {
    name.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).map(str::to_lowercase).collect()
}

fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (diagonal + usize::from(ca != cb)).min(row[j] + 1).min(above + 1);
            diagonal = above;
        }
    }
    row[b.len()]
}

/// Why `written` might be `candidate`, and how strong that is (higher sorts first).
fn why(written: &[String], candidate: &[String]) -> Option<(u8, String)> {
    let full = |w: &String| w.chars().count() >= 2;
    let (w_full, c_full): (Vec<&String>, Vec<&String>) = (written.iter().filter(|w| full(w)).collect(), candidate.iter().filter(|w| full(w)).collect());
    if w_full.is_empty() || c_full.is_empty() {
        return None;
    }
    let (mut a, mut b) = (w_full.clone(), c_full.clone());
    a.sort();
    b.sort();
    if a == b {
        return Some((9, "The same name".into()));
    }
    let initials: Vec<&String> = written.iter().filter(|w| w.chars().count() == 1).collect();
    let spare: Vec<&&String> = c_full.iter().filter(|c| !w_full.contains(c)).collect();
    if !initials.is_empty() && w_full.iter().all(|w| c_full.contains(w)) && initials.iter().all(|i| spare.iter().any(|c| c.starts_with(i.as_str()))) {
        return Some((8, "The initial could stand for the rest of this name".into()));
    }
    if a.len() == b.len() && a.iter().zip(&b).all(|(x, y)| x == y || (x.chars().count() >= 4 && edit_distance(x, y) <= 1)) {
        return Some((8, "Spelled almost the same".into()));
    }
    if w_full.iter().all(|w| c_full.contains(w)) || c_full.iter().all(|c| w_full.contains(c)) {
        return Some((7, "One name is part of the other".into()));
    }
    let (wf, cf) = (written.first()?, candidate.first()?);
    if wf == cf && wf.chars().count() >= 3 {
        return Some((3, "Same first name".into()));
    }
    let (wl, cl) = (written.last()?, candidate.last()?);
    (wl == cl && wl.chars().count() >= 3 && written.len() > 1 && candidate.len() > 1).then(|| (2, "Same last name".into()))
}

/// The roster people a written name might be, best first, at most five. Nothing here links anything.
pub fn suggest(written: Option<&str>, roster: &[Candidate]) -> Vec<Suggestion> {
    let Some(written) = written.map(words).filter(|w| !w.is_empty()) else { return vec![] };
    let mut found: Vec<(u8, Suggestion)> = roster
        .iter()
        .filter_map(|p| {
            let (strength, reason) = why(&written, &words(&p.name))?;
            Some((strength, Suggestion { person_id: p.id.clone(), name: p.name.clone(), reason, weak: strength < 8 }))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.to_lowercase().cmp(&b.1.name.to_lowercase())));
    found.into_iter().take(5).map(|(_, s)| s).collect()
}

// ---------------------------------------------------------------- the admin's actions

/// The note of a parked request dropped because its person was set aside; undoing that brings it back.
const SET_ASIDE_NOTE: &str = "the person was set aside";

/// What a link or an add did.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub person_id: String,
    pub record_ids: Vec<String>,
    pub change_id: String,
}

/// The details an admin may give when adding the person a card stands for. Blank fields come from the sheet.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewPerson {
    pub name: Option<String>,
    pub email: Option<String>,
    pub employee_code: Option<String>,
}

fn refusal(l: Linked) -> IdentityError {
    match l {
        Linked::NotFound => IdentityError::NotFound,
        Linked::Conflict(why) => IdentityError::Conflict(why),
        Linked::Done => IdentityError::Invalid("nothing to link".into()),
    }
}

async fn open_group(conn: &mut SqliteConnection, ws: &str, id: &str) -> Result<Group, IdentityError> {
    let group = unresolved::get(conn, ws, id).await?.ok_or(IdentityError::NotFound)?;
    if group.status != "open" {
        return Err(IdentityError::Conflict("This name has already been dealt with. Reload the page.".into()));
    }
    Ok(group)
}

/// Groups whose sheets all found a person are done; the others keep waiting.
async fn settle_groups(conn: &mut SqliteConnection, ws: &str, person_id: &str, record_ids: &[String]) -> sqlx::Result<()> {
    let groups: Vec<String> = sqlx::query_scalar("SELECT DISTINCT identity_id FROM timesheet_records WHERE workspace_id = ?1 AND id IN (SELECT value FROM json_each(?2)) AND identity_id IS NOT NULL")
        .bind(ws)
        .bind(json!(record_ids).to_string())
        .fetch_all(&mut *conn)
        .await?;
    for group in groups {
        unresolved::recount(conn, ws, &group).await?;
        sqlx::query("UPDATE unresolved_identities SET status = 'linked', person_id = ?1 WHERE workspace_id = ?2 AND id = ?3 AND status = 'open' AND record_count = 0")
            .bind(person_id)
            .bind(ws)
            .bind(&group)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

async fn audit_change(conn: &mut SqliteConnection, ws: &str, actor: Actor<'_>, action: &str, change_id: &str, metadata: Value) -> sqlx::Result<()> {
    audit::record(conn, Entry { workspace_id: ws, actor, action, entity_type: "identity_change", entity_id: change_id, message_id: None, metadata }).await
}

/// Runs a plan for a person inside the caller's transaction: the sheets join, the change is logged, the cards are settled.
async fn link_in(
    tx: &mut SqliteConnection,
    ws: &str,
    actor: Actor<'_>,
    person_id: &str,
    identity_id: Option<&str>,
    plan: &records::LinkPlan,
    remember_name: bool,
) -> Result<(String, Vec<String>), IdentityError> {
    if plan.record_ids.is_empty() {
        return Err(IdentityError::Invalid("Nothing is waiting under this name.".into()));
    }
    let applied = records::apply_plan(tx, ws, person_id, plan).await?;
    let change = changes::record(
        tx,
        NewChange {
            ws,
            kind: "link",
            person_id: Some(person_id),
            identity_id,
            record_ids: &plan.record_ids,
            before: json!({"records": applied.before, "person_records": applied.person_before}),
            after: json!({"new_aliases": applied.new_aliases, "remember_name": remember_name}),
            actor,
            evidence: json!({"by": "an admin's choice"}),
        },
    )
    .await?;
    settle_groups(tx, ws, person_id, &plan.record_ids).await?;
    audit_change(tx, ws, actor, "identity.linked", &change, json!({"person_id": person_id, "records": plan.record_ids.len()})).await?;
    // The desk is woken by a job written in this same transaction: the link and the wake-up stand or fall together, and the
    // worker retries the wake-up, not this request.
    jobs::enqueue(tx, kind::PERSON_LINKED, &format!("person_linked:{change}"), Some(ws), &json!({"workspace_id": ws, "record_ids": plan.record_ids})).await?;
    Ok((change, plan.record_ids.clone()))
}

/// "This card is that person": every sheet waiting under the card joins them.
pub async fn link_identity(state: &AppState, ws: &str, actor: Actor<'_>, identity_id: &str, person_id: &str, remember_name: bool) -> Result<Resolved, IdentityError> {
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    open_group(&mut tx, ws, identity_id).await?;
    let plan = records::plan_identity(&mut tx, ws, identity_id, person_id, remember_name).await?.map_err(refusal)?;
    let (change_id, record_ids) = link_in(&mut tx, ws, actor, person_id, Some(identity_id), &plan, remember_name).await?;
    tx.commit().await?;
    state.jobs_notify.notify_one();
    Ok(Resolved { person_id: person_id.into(), record_ids, change_id })
}

/// "This sheet is that person", the per-sheet way: the same plan as before (same employee ID, or a name that was confirmed),
/// now logged and undoable.
pub async fn link_record(state: &AppState, ws: &str, actor: Actor<'_>, record_id: &str, person_id: &str, remember_name: bool) -> Result<Resolved, IdentityError> {
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let identity_id: Option<String> = sqlx::query_scalar("SELECT identity_id FROM timesheet_records WHERE id = ?1 AND workspace_id = ?2").bind(record_id).bind(ws).fetch_optional(&mut *tx).await?.flatten();
    let plan = records::plan_record(&mut tx, ws, record_id, person_id, remember_name).await?.map_err(refusal)?;
    let (change_id, record_ids) = link_in(&mut tx, ws, actor, person_id, identity_id.as_deref(), &plan, remember_name).await?;
    tx.commit().await?;
    state.jobs_notify.notify_one();
    Ok(Resolved { person_id: person_id.into(), record_ids, change_id })
}

/// "Add them as a person": the card becomes a roster person (name and employee ID as written unless the admin changed them),
/// and every waiting sheet joins them. Nothing here creates a user or a membership: they cannot sign in.
pub async fn add_person(state: &AppState, ws: &str, actor: Actor<'_>, identity_id: &str, input: NewPerson, ctx: &Context) -> Result<Resolved, IdentityError> {
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let group = open_group(&mut tx, ws, identity_id).await?;
    let sheet_email: Option<String> = sqlx::query_scalar(
        "SELECT json_extract(identity_json, '$.email') FROM timesheet_records WHERE workspace_id = ?1 AND identity_id = ?2 AND json_extract(identity_json, '$.email') IS NOT NULL LIMIT 1",
    )
    .bind(ws)
    .bind(identity_id)
    .fetch_optional(&mut *tx)
    .await?
    .flatten();
    let blank = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let name = blank(input.name).or_else(|| group.name_as_written.clone());
    let email = blank(input.email).or(sheet_email);
    let Some(email) = email else { return Err(IdentityError::Invalid("emails: add the email address this person uses".into())) };
    let fields = Fields { name, emails: Some(vec![email]), employee_code: Some(blank(input.employee_code).or_else(|| group.employee_code.clone())), ..Default::default() };
    let draft = people::merge(people::blank_draft(), fields, ctx).map_err(|e| IdentityError::Invalid(format!("{}: {}", e.field, e.message)))?;
    people::validate(&mut tx, ws, None, &draft, ctx).await.map_err(|e| IdentityError::Invalid(format!("{}: {}", e.field, e.message)))?;
    let person_id = people::insert(&mut tx, ws, &draft).await?;
    let created = changes::record(
        &mut tx,
        NewChange {
            ws,
            kind: "create_person",
            person_id: Some(&person_id),
            identity_id: Some(identity_id),
            record_ids: &[],
            before: json!({}),
            after: json!({}),
            actor,
            evidence: json!({}),
        },
    )
    .await?;
    audit_change(&mut tx, ws, actor, "identity.person_created", &created, json!({"person_id": person_id})).await?;
    let plan = records::plan_identity(&mut tx, ws, identity_id, &person_id, false).await?.map_err(refusal)?;
    let (change_id, record_ids) = link_in(&mut tx, ws, actor, &person_id, Some(identity_id), &plan, false).await?;
    tx.commit().await?;
    state.jobs_notify.notify_one();
    Ok(Resolved { person_id, record_ids, change_id })
}

async fn set_group(state: &AppState, ws: &str, actor: Actor<'_>, identity_id: &str, kind: &str, status: &str) -> Result<String, IdentityError> {
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    open_group(&mut tx, ws, identity_id).await?;
    sqlx::query("UPDATE unresolved_identities SET status = ?1, updated_at = ?2 WHERE id = ?3 AND workspace_id = ?4").bind(status).bind(now_ms()).bind(identity_id).bind(ws).execute(&mut *tx).await?;
    // Nobody is waiting for this person any more: what was parked for them is dropped, unless another sheet of the same email
    // still waits for someone (then the acknowledgement stays parked for that one).
    sqlx::query(
        "UPDATE message_requests SET status = 'dropped', status_note = ?3, updated_at = ?4
         WHERE workspace_id = ?1 AND status = 'waiting' AND message_id IN (SELECT message_id FROM timesheet_records WHERE workspace_id = ?1 AND identity_id = ?2 AND message_id IS NOT NULL)
           AND NOT EXISTS (SELECT 1 FROM timesheet_records t WHERE t.workspace_id = ?1 AND t.message_id = message_requests.message_id AND t.person_id IS NULL AND t.status != 'replaced'
                             AND COALESCE((SELECT g.status FROM unresolved_identities g WHERE g.id = t.identity_id), 'open') NOT IN ('external', 'dismissed'))",
    )
    .bind(ws)
    .bind(identity_id)
    .bind(SET_ASIDE_NOTE)
    .bind(now_ms())
    .execute(&mut *tx)
    .await?;
    let change = changes::record(
        &mut tx,
        NewChange { ws, kind, person_id: None, identity_id: Some(identity_id), record_ids: &[], before: json!({"status": "open"}), after: json!({"status": status}), actor, evidence: json!({}) },
    )
    .await?;
    audit_change(&mut tx, ws, actor, &format!("identity.{kind}"), &change, json!({"identity_id": identity_id})).await?;
    tx.commit().await?;
    Ok(change)
}

/// "They work for a client, not for us": the card is kept as external and stops asking.
pub async fn mark_external(state: &AppState, ws: &str, actor: Actor<'_>, identity_id: &str) -> Result<String, IdentityError> {
    set_group(state, ws, actor, identity_id, "mark_external", "external").await
}

/// "That is not a person" (a total row, a template): the card is kept as dismissed and stops asking.
pub async fn dismiss(state: &AppState, ws: &str, actor: Actor<'_>, identity_id: &str) -> Result<String, IdentityError> {
    set_group(state, ws, actor, identity_id, "dismiss", "dismissed").await
}

/// Undoes one change, once. A link puts its sheets back to "who is this" and marks them `needs_check`; a reply that already
/// went out is not recalled, and a reply still waiting to go out blocks the undo until someone decides about it.
pub async fn undo(state: &AppState, ws: &str, actor: Actor<'_>, change_id: &str) -> Result<String, IdentityError> {
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let change = changes::get(&mut tx, ws, change_id).await?.ok_or(IdentityError::NotFound)?;
    if !change.undoable() {
        return Err(IdentityError::Conflict("This change cannot be undone, or it was undone already.".into()));
    }
    let undone = match change.kind.as_str() {
        "link" => {
            let person_id = change.person_id.clone().ok_or(IdentityError::NotFound)?;
            let ids = change.record_ids();
            let live: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM message_requests r JOIN timesheet_records t ON t.message_id = r.message_id
                 WHERE t.workspace_id = ?1 AND t.id IN (SELECT value FROM json_each(?2)) AND r.status IN ('pending', 'composed')",
            )
            .bind(ws)
            .bind(json!(ids).to_string())
            .fetch_one(&mut *tx)
            .await?;
            if live > 0 {
                return Err(IdentityError::Conflict("A reply to this sheet is waiting to go out. Decide about that reply first, then undo.".into()));
            }
            let (before, after) = (change.before(), change.after());
            let rows = |v: &Value, k: &str| v[k].as_array().cloned().unwrap_or_default();
            let restored = records::undo_link(&mut tx, ws, &person_id, &rows(&before, "records"), &rows(&before, "person_records"), &rows(&after, "new_aliases")).await?;
            let groups: Vec<String> = sqlx::query_scalar("SELECT DISTINCT identity_id FROM timesheet_records WHERE workspace_id = ?1 AND id IN (SELECT value FROM json_each(?2)) AND identity_id IS NOT NULL")
                .bind(ws)
                .bind(json!(restored).to_string())
                .fetch_all(&mut *tx)
                .await?;
            for group in groups {
                sqlx::query("UPDATE unresolved_identities SET status = 'open', person_id = NULL WHERE id = ?1 AND workspace_id = ?2 AND status = 'linked'").bind(&group).bind(ws).execute(&mut *tx).await?;
                unresolved::recount(&mut tx, ws, &group).await?;
            }
            changes::record(
                &mut tx,
                NewChange {
                    ws,
                    kind: "unlink",
                    person_id: Some(&person_id),
                    identity_id: change.identity_id.as_deref(),
                    record_ids: &restored,
                    before: json!({"undoes": change.id}),
                    after: json!({"status": "needs_check"}),
                    actor,
                    evidence: json!({"by": "an admin's choice"}),
                },
            )
            .await?
        }
        "unlink" => {
            let person_id = change.before()["relink"]["person_id"].as_str().map(str::to_string).ok_or(IdentityError::NotFound)?;
            let ids = change.record_ids();
            let Some(record_id) = ids.first() else { return Err(IdentityError::NotFound) };
            if !records::relink_sheet(&mut tx, ws, record_id, &person_id).await? {
                return Err(IdentityError::Conflict("That sheet or that person is no longer available to link again.".into()));
            }
            settle_groups(&mut tx, ws, &person_id, &ids).await?;
            changes::record(
                &mut tx,
                NewChange { ws, kind: "link", person_id: Some(&person_id), identity_id: change.identity_id.as_deref(), record_ids: &ids, before: json!({"undoes": change.id}), after: json!({}), actor, evidence: json!({}) },
            )
            .await?
        }
        _ => {
            let identity_id = change.identity_id.clone().ok_or(IdentityError::NotFound)?;
            sqlx::query("UPDATE unresolved_identities SET status = 'open', updated_at = ?1 WHERE id = ?2 AND workspace_id = ?3 AND status IN ('external', 'dismissed')").bind(now_ms()).bind(&identity_id).bind(ws).execute(&mut *tx).await?;
            sqlx::query(
                "UPDATE message_requests SET status = 'waiting', status_note = 'waiting for the person to be linked', updated_at = ?4
                 WHERE workspace_id = ?1 AND status = 'dropped' AND status_note = ?3 AND message_id IN (SELECT message_id FROM timesheet_records WHERE workspace_id = ?1 AND identity_id = ?2 AND message_id IS NOT NULL)",
            )
            .bind(ws)
            .bind(&identity_id)
            .bind(SET_ASIDE_NOTE)
            .bind(now_ms())
            .execute(&mut *tx)
            .await?;
            changes::record(
                &mut tx,
                NewChange { ws, kind: "reopen", person_id: None, identity_id: Some(&identity_id), record_ids: &[], before: json!({"undoes": change.id}), after: json!({"status": "open"}), actor, evidence: json!({}) },
            )
            .await?
        }
    };
    changes::mark_undone(&mut tx, ws, &change.id, &undone).await?;
    audit_change(&mut tx, ws, actor, "identity.undone", &undone, json!({"undoes": change.id, "kind": change.kind})).await?;
    tx.commit().await?;
    Ok(undone)
}

/// The owner says a sheet is not that person's: it goes back to "who is this" and `needs_check`, under its card (or a new one when
/// the feature is on). Logged, and undoable while the sheet is still unmatched.
pub async fn unlink_record(state: &AppState, ws: &str, actor: Actor<'_>, record_id: &str) -> Result<String, IdentityError> {
    let acts = crate::platform::flags::mode(&state.db, &state.config, ws, crate::platform::flags::Key::Handoff).await?.acts();
    let roster = unresolved::roster(&state.db, ws).await?;
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let Some((snapshot, identity_id)) = records::unlink_sheet(&mut tx, ws, record_id).await? else {
        return Err(IdentityError::Conflict("This sheet is not linked to anyone.".into()));
    };
    let mut card = identity_id;
    if card.is_none() && acts {
        let (identity_json, sender): (String, Option<String>) = sqlx::query_as("SELECT identity_json, sender_address FROM timesheet_records WHERE id = ?1 AND workspace_id = ?2").bind(record_id).bind(ws).fetch_one(&mut *tx).await?;
        let identity: crate::hr::staffing::timesheet_parse::Identity = serde_json::from_str(&identity_json).unwrap_or_default();
        let company = unresolved::company_domains(&mut tx, ws).await?;
        card = unresolved::attach(&mut tx, ws, &identity, sender.as_deref().unwrap_or_default(), &roster, &company).await?;
        sqlx::query("UPDATE timesheet_records SET identity_id = ?1 WHERE id = ?2 AND workspace_id = ?3").bind(&card).bind(record_id).bind(ws).execute(&mut *tx).await?;
    }
    if let Some(g) = &card {
        unresolved::recount(&mut tx, ws, g).await?;
        sqlx::query("UPDATE unresolved_identities SET status = 'open', person_id = NULL WHERE id = ?1 AND workspace_id = ?2 AND status = 'linked'").bind(g).bind(ws).execute(&mut *tx).await?;
    }
    let person_id = snapshot["person_id"].as_str().map(str::to_string);
    let change = changes::record(
        &mut tx,
        NewChange {
            ws,
            kind: "unlink",
            person_id: person_id.as_deref(),
            identity_id: card.as_deref(),
            record_ids: &[record_id.to_string()],
            before: json!({"relink": snapshot}),
            after: json!({"status": "needs_check"}),
            actor,
            evidence: json!({"by": "the owner"}),
        },
    )
    .await?;
    audit_change(&mut tx, ws, actor, "identity.sheet_unlinked", &change, json!({"record_id": record_id})).await?;
    tx.commit().await?;
    Ok(change)
}

/// The owner checked a sheet the system had linked by a partial name and says it is right.
pub async fn confirm_record(state: &AppState, ws: &str, actor: Actor<'_>, record_id: &str) -> Result<(), IdentityError> {
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    if !records::confirm_sheet(&mut tx, ws, record_id).await? {
        return Err(IdentityError::Conflict("This sheet is not linked to a person, or the link was already chosen by a person.".into()));
    }
    audit::record(&mut tx, Entry { workspace_id: ws, actor, action: "timesheet.link_confirmed", entity_type: "timesheet_record", entity_id: record_id, message_id: None, metadata: json!({}) }).await?;
    tx.commit().await?;
    Ok(())
}

/// The workspace owner forgets a remembered employee ID (`code`) or name (`name`), so a wrong link can be fixed. Logged as an
/// `alias` change (the kind of link only, never the value: a name is personal data).
pub async fn forget_link(state: &AppState, ws: &str, actor: Actor<'_>, kind: &str, raw_value: &str) -> Result<String, IdentityError> {
    if !matches!(kind, "code" | "name") {
        return Err(IdentityError::Invalid("kind must be code or name".into()));
    }
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let Some((person_id, _)) = records::forget_alias(&mut tx, ws, kind, raw_value).await? else { return Err(IdentityError::NotFound) };
    let change = changes::record(
        &mut tx,
        NewChange { ws, kind: "alias", person_id: Some(&person_id), identity_id: None, record_ids: &[], before: json!({"kind": kind}), after: json!({"forgotten": true}), actor, evidence: json!({}) },
    )
    .await?;
    audit_change(&mut tx, ws, actor, "identity.link_forgotten", &change, json!({"kind": kind, "person_id": person_id})).await?;
    tx.commit().await?;
    Ok(change)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roster() -> Vec<Candidate> {
        let c = |id: &str, name: &str| Candidate { id: id.into(), name: name.into(), code: None, emails: vec![] };
        vec![c("p1", "Rahul Verma"), c("p2", "Priya Singh"), c("p3", "Ravi Kumar Singh"), c("p4", "Anita Desai"), c("p5", "Rahul Sharna")]
    }

    fn names(written: &str) -> Vec<String> {
        suggest(Some(written), &roster()).into_iter().map(|s| s.name).collect()
    }

    #[test]
    fn a_first_name_or_a_part_of_a_name_suggests_but_never_decides() {
        assert_eq!(names("Rahul Sharma"), ["Rahul Sharna", "Rahul Verma"], "a near spelling first, then the same first name");
        assert_eq!(names("Rahul"), ["Rahul Sharna", "Rahul Verma"]);
        assert_eq!(names("Ravi Kumar"), ["Ravi Kumar Singh"]);
    }

    #[test]
    fn only_a_part_of_a_name_in_common_is_a_weak_suggestion() {
        let weak = |written: &str, name: &str| suggest(Some(written), &roster()).into_iter().find(|s| s.name == name).unwrap_or_else(|| panic!("{written} -> {name}")).weak;
        assert!(weak("Ravi Kumar", "Ravi Kumar Singh"), "one name inside the other");
        assert!(weak("Rahul", "Rahul Verma"));
        assert!(weak("Rahul Sharma", "Rahul Verma"), "same first name only");
        assert!(!weak("Rahul Sharma", "Rahul Sharna"), "spelled almost the same");
        assert!(!weak("Priya S.", "Priya Singh"), "an initial that fits");
    }

    #[test]
    fn an_initial_could_be_a_surname() {
        let found = suggest(Some("Priya S."), &roster());
        assert_eq!(found.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Priya Singh"]);
        assert!(found[0].reason.contains("initial"), "{}", found[0].reason);
    }

    #[test]
    fn strangers_get_no_suggestion() {
        assert!(names("Dinesh Rao").is_empty());
        assert!(suggest(None, &roster()).is_empty());
        assert!(suggest(Some("  "), &roster()).is_empty());
    }

    #[test]
    fn free_mail_is_never_a_companys_own_domain() {
        for free in ["gmail.com", "Gmail.com", "yahoo.co.in", "yahoo.com", "outlook.com", "hotmail.co.uk", "live.com", "icloud.com", "me.com", "protonmail.com", "proton.me", "aol.com", "zoho.in", "gmx.de", "rediffmail.com", "yandex.ru", "fastmail.com"] {
            assert!(is_free_mail(free), "{free}");
            assert!(!is_company_domain(free, &[free.to_string()]), "{free} stays free even when the roster lists it");
        }
        for own in ["acme.com", "client.test", "mail.acme.com", "live.acme.com", "agency.co.in"] {
            assert!(!is_free_mail(own), "{own}");
            assert!(is_company_domain(own, &[own.to_string()]));
        }
        assert!(!is_company_domain("acme.com", &[]) && !is_company_domain("", &["".to_string()]));
        let company = vec!["gmail.com".to_string()];
        assert_ne!(group_key(None, Some("Amit Kumar"), "x@gmail.com", &company), group_key(None, Some("Amit Kumar"), "y@gmail.com", &company));
    }

    #[test]
    fn groups_split_by_id_and_by_where_the_sheet_came_from() {
        let company = vec!["a.test".to_string()];
        assert_eq!(group_key(Some("X-77"), Some("D. Rao"), "hr@a.test", &company), Some("code:x-77".into()));
        assert_eq!(group_key(Some(" x-77 "), None, "hr@b.test", &company), Some("code:x-77".into()), "the ID alone is the key");
        let a = group_key(None, Some("Amit Kumar"), "hr@a.test", &company);
        assert_eq!(a, group_key(None, Some("Kumar, Amit"), "other@a.test", &company), "same name, same company");
        assert_ne!(a, group_key(None, Some("Amit Kumar"), "hr@b.test", &company), "another sender is another Amit");
        assert_ne!(group_key(None, Some("Amit Kumar"), "x@gmail.com", &company), group_key(None, Some("Amit Kumar"), "y@gmail.com", &company), "an unknown domain says nothing about who");
        assert_ne!(group_key(None, Some("Amit Kumar"), "x@agency.test", &company), group_key(None, Some("Amit Kumar"), "y@agency.test", &company), "an agency's domain is not a company's");
        assert_eq!(group_key(None, None, "hr@a.test", &company), None);
        assert_eq!(group_key(None, Some("S."), "hr@a.test", &company), None, "an initial alone is not a name");
    }
}
