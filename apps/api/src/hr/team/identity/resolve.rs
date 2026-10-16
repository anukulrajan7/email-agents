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
    agents::plan::{self, Wake},
    hr::{
        staffing::{
            timesheet_parse::Candidate,
            timesheet_records::{self as records, Linked},
        },
        team::people::{self, Context, Fields},
    },
    http::AppState,
    platform::{
        audit::{self, Actor, Entry},
        db::now_ms,
    },
};

/// Mail domains many unrelated people share: grouping by them would put two different people together.
const FREE_MAIL: [&str; 9] = ["gmail.com", "googlemail.com", "yahoo.com", "yahoo.in", "outlook.com", "hotmail.com", "icloud.com", "proton.me", "rediffmail.com"];

/// Where a sheet came from, as a grouping hint: the domain, or the whole address for a free mail domain.
pub fn sender_hint(sender: &str) -> (String, String) {
    let address = sender.trim().trim_matches(['<', '>']).to_lowercase();
    let domain = address.rsplit_once('@').map(|(_, d)| d.to_string()).unwrap_or_default();
    let hint = if FREE_MAIL.contains(&domain.as_str()) || domain.is_empty() { address.clone() } else { domain.clone() };
    (domain, hint)
}

/// The group a sheet's identity belongs to: its employee ID, else its normalised name together with where it came from.
/// Two sheets with the same name from different places, or with different IDs, are never one group. `None` when the sheet
/// names nobody.
pub fn group_key(code: Option<&str>, name: Option<&str>, sender: &str) -> Option<String> {
    if let Some(code) = code.map(str::trim).filter(|c| !c.is_empty()) {
        return Some(format!("code:{}", code.to_lowercase()));
    }
    let key = crate::hr::staffing::name_key(name?);
    (!key.is_empty()).then(|| format!("name:{key}@{}", sender_hint(sender).1))
}

/// One person the name might be. Evidence only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Suggestion {
    pub person_id: String,
    pub name: String,
    pub reason: String,
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
            Some((strength, Suggestion { person_id: p.id.clone(), name: p.name.clone(), reason }))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.to_lowercase().cmp(&b.1.name.to_lowercase())));
    found.into_iter().take(5).map(|(_, s)| s).collect()
}

// ---------------------------------------------------------------- the admin's actions

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
    Ok((change, plan.record_ids.clone()))
}

async fn woke(state: &AppState, ws: &str, record_ids: &[String]) -> Result<(), IdentityError> {
    plan::poke(state, ws, &Wake::PersonLinked { record_ids: record_ids.to_vec() }).await?;
    Ok(())
}

/// "This card is that person": every sheet waiting under the card joins them.
pub async fn link_identity(state: &AppState, ws: &str, actor: Actor<'_>, identity_id: &str, person_id: &str, remember_name: bool) -> Result<Resolved, IdentityError> {
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    open_group(&mut tx, ws, identity_id).await?;
    let plan = records::plan_identity(&mut tx, ws, identity_id, person_id, remember_name).await?.map_err(refusal)?;
    let (change_id, record_ids) = link_in(&mut tx, ws, actor, person_id, Some(identity_id), &plan, remember_name).await?;
    tx.commit().await?;
    woke(state, ws, &record_ids).await?;
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
    woke(state, ws, &record_ids).await?;
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
            after: json!({"name": draft.name, "employee_code": draft.employee_code}),
            actor,
            evidence: json!({"name_as_written": group.name_as_written, "employee_code": group.employee_code}),
        },
    )
    .await?;
    audit_change(&mut tx, ws, actor, "identity.person_created", &created, json!({"person_id": person_id})).await?;
    let plan = records::plan_identity(&mut tx, ws, identity_id, &person_id, false).await?.map_err(refusal)?;
    let (change_id, record_ids) = link_in(&mut tx, ws, actor, &person_id, Some(identity_id), &plan, false).await?;
    tx.commit().await?;
    woke(state, ws, &record_ids).await?;
    Ok(Resolved { person_id, record_ids, change_id })
}

async fn set_group(state: &AppState, ws: &str, actor: Actor<'_>, identity_id: &str, kind: &str, status: &str) -> Result<String, IdentityError> {
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let group = open_group(&mut tx, ws, identity_id).await?;
    sqlx::query("UPDATE unresolved_identities SET status = ?1, updated_at = ?2 WHERE id = ?3 AND workspace_id = ?4").bind(status).bind(now_ms()).bind(identity_id).bind(ws).execute(&mut *tx).await?;
    let change = changes::record(
        &mut tx,
        NewChange { ws, kind, person_id: None, identity_id: Some(identity_id), record_ids: &[], before: json!({"status": "open"}), after: json!({"status": status}), actor, evidence: json!({"name_as_written": group.name_as_written}) },
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
                 WHERE t.workspace_id = ?1 AND t.id IN (SELECT value FROM json_each(?2)) AND r.status IN ('pending', 'composed') AND r.kind = 'ack'",
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
        _ => {
            let identity_id = change.identity_id.clone().ok_or(IdentityError::NotFound)?;
            sqlx::query("UPDATE unresolved_identities SET status = 'open', updated_at = ?1 WHERE id = ?2 AND workspace_id = ?3 AND status IN ('external', 'dismissed')").bind(now_ms()).bind(&identity_id).bind(ws).execute(&mut *tx).await?;
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
    fn groups_split_by_id_and_by_where_the_sheet_came_from() {
        assert_eq!(group_key(Some("X-77"), Some("D. Rao"), "hr@a.test"), Some("code:x-77".into()));
        assert_eq!(group_key(Some(" x-77 "), None, "hr@b.test"), Some("code:x-77".into()), "the ID alone is the key");
        let a = group_key(None, Some("Amit Kumar"), "hr@a.test");
        assert_eq!(a, group_key(None, Some("Kumar, Amit"), "other@a.test"), "same name, same company");
        assert_ne!(a, group_key(None, Some("Amit Kumar"), "hr@b.test"), "another company is another Amit");
        assert_ne!(group_key(None, Some("Amit Kumar"), "x@gmail.com"), group_key(None, Some("Amit Kumar"), "y@gmail.com"), "free mail says nothing about who");
        assert_eq!(group_key(None, None, "hr@a.test"), None);
        assert_eq!(group_key(None, Some("S."), "hr@a.test"), None, "an initial alone is not a name");
    }
}
