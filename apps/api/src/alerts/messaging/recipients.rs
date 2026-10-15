//! Who a message goes to: a person in the workspace, someone in the people directory, a team, or an
//! outside address. Names are only matched exactly, and an address nobody here has written to is
//! `NewExternal`, which always needs a person's OK.

use serde::Serialize;
use sqlx::SqlitePool;

use serde_json::Value;
use crate::replies::send_safety::{bare_address, valid_address};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A member of this workspace.
    Member,
    /// Listed in the people directory, but without an account.
    Directory,
    /// An outside address someone here has written with or to before.
    KnownExternal,
    NewExternal,
}

impl Kind {
    pub fn is_internal(self) -> bool {
        matches!(self, Kind::Member | Kind::Directory)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Member => "member",
            Kind::Directory => "directory",
            Kind::KnownExternal => "known_external",
            Kind::NewExternal => "new_external",
        }
    }

    pub fn parse(s: &str) -> Kind {
        match s {
            "member" => Kind::Member,
            "directory" => Kind::Directory,
            "known_external" => Kind::KnownExternal,
            _ => Kind::NewExternal,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Person {
    /// How to name them to a person: their name, else their address.
    pub label: String,
    pub address: Option<String>,
    pub user_id: Option<String>,
    pub kind: Kind,
}

#[derive(Debug, Clone)]
pub enum Target {
    Person(Person),
    Team { id: String, name: String, desk_agent_id: Option<String>, members: Vec<Person> },
}

impl Target {
    pub fn label(&self) -> String {
        match self {
            Target::Person(p) => p.label.clone(),
            Target::Team { name, .. } => format!("the {name} team"),
        }
    }

    pub fn people(&self) -> Vec<&Person> {
        match self {
            Target::Person(p) => vec![p],
            Target::Team { members, .. } => members.iter().collect(),
        }
    }

    pub fn kinds(&self) -> Vec<Kind> {
        self.people().iter().map(|p| p.kind).collect()
    }
}

/// An address, classified for this workspace.
pub async fn classify(db: &SqlitePool, ws: &str, raw: &str) -> sqlx::Result<Person> {
    let address = bare_address(raw);
    let member: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT u.id, u.name FROM memberships m JOIN users u ON u.id = m.user_id WHERE m.workspace_id = ?1 AND lower(u.email) = ?2",
    )
    .bind(ws)
    .bind(&address)
    .fetch_optional(db)
    .await?;
    if let Some((user_id, name)) = member {
        let label = name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| address.clone());
        return Ok(Person { label, address: Some(address), user_id: Some(user_id), kind: Kind::Member });
    }
    let listed: Option<String> = sqlx::query_scalar(
        "SELECT p.name FROM person_emails e JOIN people p ON p.id = e.person_id
         WHERE e.workspace_id = ?1 AND lower(e.email) = ?2 AND p.active = 1",
    )
    .bind(ws)
    .bind(&address)
    .fetch_optional(db)
    .await?;
    if let Some(name) = listed {
        let kind = if same_domain_as_a_member(db, ws, &address).await? { Kind::Directory } else { Kind::KnownExternal };
        return Ok(Person { label: name, address: Some(address), user_id: None, kind });
    }
    let known: Option<Option<String>> = sqlx::query_scalar("SELECT display_name FROM contacts WHERE workspace_id = ?1 AND lower(email) = ?2")
        .bind(ws)
        .bind(&address)
        .fetch_optional(db)
        .await?;
    let written_to: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM outbound_emails WHERE workspace_id = ?1 AND to_address = ?2 AND status = 'sent')",
    )
    .bind(ws)
    .bind(&address)
    .fetch_one(db)
    .await?;
    if known.is_some() || written_to {
        let label = known.flatten().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| address.clone());
        return Ok(Person { label, address: Some(address), user_id: None, kind: Kind::KnownExternal });
    }
    Ok(Person { label: address.clone(), address: Some(address), user_id: None, kind: Kind::NewExternal })
}

/// A people-directory address counts as inside the workspace only when it is on a domain a member of the workspace uses.
async fn same_domain_as_a_member(db: &SqlitePool, ws: &str, address: &str) -> sqlx::Result<bool> {
    let Some(domain) = address.rsplit_once('@').map(|(_, d)| d.to_lowercase()).filter(|d| !d.is_empty()) else { return Ok(false) };
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM memberships m JOIN users u ON u.id = m.user_id
                        WHERE m.workspace_id = ?1 AND lower(substr(u.email, instr(u.email, '@') + 1)) = ?2)",
    )
    .bind(ws)
    .bind(domain)
    .fetch_one(db)
    .await
}

/// `%`, `_` and `\` in a name are plain characters in a LIKE pattern.
pub fn escape_like(text: &str) -> String {
    text.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

async fn member_by_id(db: &SqlitePool, ws: &str, user_id: &str) -> sqlx::Result<Option<Person>> {
    let row: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT u.email, u.name FROM memberships m JOIN users u ON u.id = m.user_id WHERE m.workspace_id = ?1 AND u.id = ?2")
            .bind(ws)
            .bind(user_id)
            .fetch_optional(db)
            .await?;
    Ok(row.map(|(email, name)| Person {
        label: name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| email.clone()),
        address: Some(email.to_lowercase()),
        user_id: Some(user_id.to_string()),
        kind: Kind::Member,
    }))
}

pub async fn team_members(db: &SqlitePool, ws: &str, team_id: &str) -> sqlx::Result<Vec<Person>> {
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT t.user_id FROM team_members t JOIN memberships m ON m.workspace_id = t.workspace_id AND m.user_id = t.user_id
         WHERE t.team_id = ?1 AND t.workspace_id = ?2 ORDER BY t.user_id",
    )
    .bind(team_id)
    .bind(ws)
    .fetch_all(db)
    .await?;
    let mut people = vec![];
    for id in ids {
        if let Some(p) = member_by_id(db, ws, &id).await? {
            people.push(p);
        }
    }
    Ok(people)
}

fn strip_team(text: &str) -> &str {
    let t = text.trim();
    let t = t.strip_prefix("the ").or_else(|| t.strip_prefix("The ")).unwrap_or(t);
    t.strip_suffix(" team").or_else(|| t.strip_suffix(" Team")).unwrap_or(t).trim()
}

/// A team, a member, a person from the directory, a known contact or an address, by what the person said.
pub async fn resolve(db: &SqlitePool, ws: &str, text: &str) -> Result<Target, String> {
    let db_err = |e: sqlx::Error| {
        tracing::warn!(error = %e, "could not look up the recipient");
        "I could not look that person up right now.".to_string()
    };
    let text = text.trim();
    if text.is_empty() {
        return Err("who should it go to?".into());
    }
    if text.contains('@') {
        let address = bare_address(text);
        if !valid_address(&address) {
            return Err(format!("\"{text}\" is not a valid email address."));
        }
        return classify(db, ws, &address).await.map(Target::Person).map_err(db_err);
    }
    let name = strip_team(text).to_lowercase();
    let team: Option<(String, String, Option<String>)> =
        sqlx::query_as("SELECT id, name, desk_agent_id FROM teams WHERE workspace_id = ?1 AND (lower(name) = ?2 OR lower(name) = ?3)")
            .bind(ws)
            .bind(text.to_lowercase())
            .bind(&name)
            .fetch_optional(db)
            .await
            .map_err(db_err)?;
    if let Some((id, name, desk_agent_id)) = team {
        let members = team_members(db, ws, &id).await.map_err(db_err)?;
        if members.is_empty() {
            return Err(format!("The {name} team has no members yet."));
        }
        return Ok(Target::Team { id, name, desk_agent_id, members });
    }
    let lowered = text.to_lowercase();
    let members: Vec<String> = sqlx::query_scalar(
        "SELECT u.id FROM memberships m JOIN users u ON u.id = m.user_id
         WHERE m.workspace_id = ?1 AND (lower(u.name) = ?2 OR lower(u.email) LIKE ?3 || '@%' ESCAPE '\\')",
    )
    .bind(ws)
    .bind(&lowered)
    .bind(escape_like(&lowered))
    .fetch_all(db)
    .await
    .map_err(db_err)?;
    match members.as_slice() {
        [one] => {
            if let Some(p) = member_by_id(db, ws, one).await.map_err(db_err)? {
                return Ok(Target::Person(p));
            }
        }
        [] => {}
        _ => return Err(format!("More than one person is called {text}. Use their email address.")),
    }
    let listed: Vec<String> = sqlx::query_scalar(
        "SELECT e.email FROM people p JOIN person_emails e ON e.person_id = p.id
         WHERE p.workspace_id = ?1 AND p.active = 1 AND lower(p.name) = ?2 AND e.position = (SELECT MIN(position) FROM person_emails WHERE person_id = p.id)",
    )
    .bind(ws)
    .bind(&lowered)
    .fetch_all(db)
    .await
    .map_err(db_err)?;
    match listed.as_slice() {
        [one] => return classify(db, ws, one).await.map(Target::Person).map_err(db_err),
        [] => {}
        _ => return Err(format!("More than one person is called {text}. Use their email address.")),
    }
    let contacts: Vec<String> = sqlx::query_scalar("SELECT email FROM contacts WHERE workspace_id = ?1 AND lower(display_name) = ?2")
        .bind(ws)
        .bind(&lowered)
        .fetch_all(db)
        .await
        .map_err(db_err)?;
    match contacts.as_slice() {
        [one] => classify(db, ws, one).await.map(Target::Person).map_err(db_err),
        [] => Err(format!("I could not find anyone called {text}. Give me their email address.")),
        _ => Err(format!("More than one contact is called {text}. Use their email address.")),
    }
}

/// The exact recipients a card was prepared for: an address, or a member's id, for each person.
pub fn pin(target: &Target) -> Value {
    let mut keys: Vec<String> = target.people().iter().map(|p| pin_key(p)).collect();
    keys.sort();
    serde_json::json!(keys)
}

fn pin_key(p: &Person) -> String {
    p.user_id.as_deref().map(|u| format!("user:{u}")).or_else(|| p.address.as_deref().map(|a| format!("address:{a}"))).unwrap_or_default()
}

/// Does this target still go to exactly the people the card named?
pub fn matches_pin(target: &Target, pinned: Option<&Value>) -> bool {
    pinned.is_some_and(|p| *p == pin(target))
}

/// Plain words for where a recipient's address came from.
pub fn source_words(typed: &str, p: &Person) -> &'static str {
    if typed.contains('@') {
        return "the address you gave me";
    }
    match p.kind {
        Kind::Member => "a member of this workspace",
        Kind::Directory => "your people directory",
        Kind::KnownExternal => "someone you have written with before",
        Kind::NewExternal => "an address nobody here has written to before",
    }
}

pub async fn member(db: &SqlitePool, ws: &str, user_id: &str) -> sqlx::Result<Option<Person>> {
    member_by_id(db, ws, user_id).await
}
