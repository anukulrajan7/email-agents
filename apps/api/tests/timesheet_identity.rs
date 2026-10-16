//! Slice S8: people nobody registered stay unresolved identities, and nothing merges by itself. A sheet is kept with the
//! name as written; similar names are suggestions, never links; every link is a person's decision, audited and undoable.

use std::sync::Arc;

use axum::http::StatusCode;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use email_ai_api::{
    agents::plan::{self, Wake},
    hr::{staffing::timesheet_records as records, team::clock, team::identity::unresolved},
    jobs::{self, kind, worker},
    platform::{
        db::now_ms,
        flags::{Key, Mode},
    },
};
use serde_json::{Value, json};

use crate::{common::*, staffing_common::Recorder};

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const HR: &str = "hr@client.test";

/// `(month name with year, YYYY-MM)` of last month.
fn last_month() -> (String, String) {
    let month = clock::previous_month(&clock::month_of(now_ms(), 0));
    (format!("{} {}", MONTHS[month[5..7].parse::<usize>().unwrap() - 1], &month[..4]), month)
}

fn sheet(name: &str, code: Option<&str>) -> String {
    let (_, month) = last_month();
    let id = code.map(|c| format!("Employee ID: {c}\n")).unwrap_or_default();
    format!(
        "{id}Employee Name: {name}\nClient: Acme Corp\n\n| Date | Project | Hours |\n| --- | --- | --- |\n\
         | {month}-01 | Portal | 8 |\n| {month}-02 | Portal | 7.5 |\n| {month}-03 | Support | 8 |\n| Total | | 23.5 |\n"
    )
}

async fn env() -> Env {
    let env = Env::new(Opts::packs(&["general", "staffing"])).await;
    env.ok("PUT", "/agent", ADMIN, Some(json!({"permissions": {"send": "auto"}}))).await;
    env
}

impl Env {
    /// A sheet mailed to the shared mailbox and read into records.
    async fn sheet_mail(&self, n: &str, from: &str, name: &str, code: Option<&str>) -> (String, Vec<records::Saved>) {
        let (month_name, _) = last_month();
        let id = self.receive(&self.shared_src, &format!("gm-{n}"), &format!("th-{n}"), from, &format!("{month_name} timesheet"), &sheet(name, code), "timesheet").await;
        let run = records::record_message(&self.state, &self.ws, &id).await.unwrap();
        (id, run.records)
    }

    async fn record_rows(&self) -> Vec<(Option<String>, Option<String>, String)> {
        sqlx::query_as("SELECT person_id, person_name, status FROM timesheet_records WHERE workspace_id = ?1 ORDER BY created_at, rowid").bind(&self.ws).fetch_all(&self.state.db).await.unwrap()
    }

    #[allow(dead_code)]
    async fn users(&self) -> (i64, i64) {
        (self.count("SELECT COUNT(*) FROM users").await, self.count("SELECT COUNT(*) FROM memberships").await)
    }
}

#[tokio::test]
async fn similar_names_are_never_linked_to_the_wrong_person() {
    let env = env().await;
    env.person("Rahul Verma", Some("E500"), "rahul@client.test", None).await;
    env.person("Priya Singh", Some("E501"), "priya.singh@client.test", None).await;
    env.person("Ravi Kumar Singh", None, "ravi.singh@client.test", None).await;
    for (n, name) in ["Rahul Sharma", "Rahul", "Priya S.", "Ravi Kumar"].iter().enumerate() {
        let (_, saved) = env.sheet_mail(&n.to_string(), HR, name, None).await;
        assert_eq!(saved.len(), 1, "{name}");
        assert_eq!(saved[0].person_id, None, "{name} must stay unresolved");
        assert_eq!(saved[0].status, "needs_check", "{name}");
    }
    let rows = env.record_rows().await;
    assert!(rows.iter().all(|r| r.0.is_none()), "{rows:?}");
    assert_eq!(rows.iter().map(|r| r.1.clone().unwrap()).collect::<Vec<_>>(), ["Rahul Sharma", "Rahul", "Priya S.", "Ravi Kumar"], "names exactly as written");
}

#[tokio::test]
async fn two_amit_kumars_are_not_linked_in_one_click() {
    let env = env().await;
    let singh = env.person("Amit Kumar Singh", Some("E1"), "amit.singh@client.test", None).await;
    env.person("Amit Kumar Jha", Some("E2"), "amit.jha@client.test", None).await;
    let (_, a) = env.sheet_mail("1", HR, "Amit Kumar", Some("X-1")).await;
    let (_, b) = env.sheet_mail("2", HR, "Amit Kumar", Some("X-2")).await;
    let (_, c) = env.sheet_mail("3", HR, "Amit Kumar", None).await;
    assert!([&a, &b, &c].iter().all(|r| r[0].person_id.is_none()));
    let base = format!("/staffing/timesheet-records/{}/link", a[0].id);
    let linked = env.ok("POST", &base, ADMIN, Some(json!({"person_id": singh}))).await;
    assert_eq!(linked["person_id"], json!(singh));
    let rows = env.record_rows().await;
    assert_eq!(rows.iter().filter(|r| r.0.is_some()).count(), 1, "only the sheet that was chosen: {rows:?}");
    // Even when the person asks to remember the name, a sheet that names another ID is someone else's.
    let again = format!("/staffing/timesheet-records/{}/link", c[0].id);
    env.ok("POST", &again, ADMIN, Some(json!({"person_id": singh, "remember_name": true}))).await;
    let rows = env.record_rows().await;
    assert_eq!(rows.iter().filter(|r| r.0.is_some()).count(), 2, "the sheet without an ID joined, X-2 did not: {rows:?}");
    assert!(rows[1].0.is_none(), "X-2 is still unresolved");
}

#[tokio::test]
async fn an_alias_owned_by_another_person_is_a_conflict() {
    let env = env().await;
    let ravi = env.person("Ravi Kumar", Some("E1024"), "ravi@client.test", None).await;
    let priya = env.person("Priya Shah", Some("E2000"), "priya@client.test", None).await;
    let (_, saved) = env.sheet_mail("1", HR, "D. Rao", Some("X-77")).await;
    sqlx::query("INSERT INTO timesheet_aliases (workspace_id, kind, value, person_id, created_at) VALUES (?1, 'code', 'x-77', ?2, 1)").bind(&env.ws).bind(&ravi).execute(&env.state.db).await.unwrap();
    let (s, body) = env.req("POST", &format!("/staffing/timesheet-records/{}/link", saved[0].id), ADMIN, Some(json!({"person_id": priya}))).await;
    assert_eq!(s, StatusCode::CONFLICT, "{body}");
    assert!(body.to_string().contains("Ravi Kumar"), "names the owner: {body}");
    let owner: String = sqlx::query_scalar("SELECT person_id FROM timesheet_aliases WHERE value = 'x-77'").fetch_one(&env.state.db).await.unwrap();
    assert_eq!(owner, ravi, "the alias was not re-pointed");
    assert_eq!(env.record_rows().await[0].0, None, "and the sheet stays unresolved");
}

#[tokio::test]
async fn the_owner_can_forget_a_wrong_link_and_the_conflict_says_so() {
    let env = env().await;
    let ravi = env.person("Ravi Kumar", Some("E1024"), "ravi@client.test", None).await;
    let priya = env.person("Priya Shah", Some("E2000"), "priya@client.test", None).await;
    let (_, saved) = env.sheet_mail("1", HR, "D. Rao", Some("X-77")).await;
    sqlx::query("INSERT INTO timesheet_aliases (workspace_id, kind, value, person_id, created_at) VALUES (?1, 'code', 'x-77', ?2, 1)").bind(&env.ws).bind(&ravi).execute(&env.state.db).await.unwrap();
    let link = format!("/staffing/timesheet-records/{}/link", saved[0].id);
    let (s, body) = env.req("POST", &link, ADMIN, Some(json!({"person_id": priya}))).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert!(body.to_string().contains("Ravi Kumar") && body.to_string().contains("forget"), "names the owner and offers the way out: {body}");
    let (s, _) = env.req("POST", "/staffing/identity-links/forget", OPERATOR, Some(json!({"employee_id": "X-77"}))).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "only the owner");
    let (s, _) = env.req("POST", "/staffing/identity-links/forget", ADMIN, Some(json!({"employee_id": "NOPE"}))).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    env.ok("POST", "/staffing/identity-links/forget", ADMIN, Some(json!({"employee_id": " x-77 "}))).await;
    assert_eq!(env.count("SELECT COUNT(*) FROM timesheet_aliases").await, 0);
    let logged: Vec<(String, Option<String>, String)> = sqlx::query_as("SELECT kind, person_id, before_json FROM identity_changes WHERE workspace_id = ?1").bind(&env.ws).fetch_all(&env.state.db).await.unwrap();
    assert_eq!(logged, [("alias".to_string(), Some(ravi.clone()), "{\"kind\":\"code\"}".to_string())], "audited, and the value itself is not copied");
    env.ok("POST", &link, ADMIN, Some(json!({"person_id": priya}))).await;
    let owner: String = sqlx::query_scalar("SELECT person_id FROM timesheet_aliases WHERE value = 'x-77'").fetch_one(&env.state.db).await.unwrap();
    assert_eq!(owner, priya, "linked again, to the right person");
}

#[tokio::test]
async fn a_partial_name_from_the_persons_own_mailbox_is_not_theirs() {
    let env = env().await;
    env.person("Rahul Verma", Some("E500"), "rahul@client.test", None).await;
    let (_, saved) = env.sheet_mail("1", "rahul@client.test", "Rahul", None).await;
    assert_eq!(saved[0].person_id, None, "\"Rahul\" sent from Rahul Verma's address is still only a suggestion");
    let (_, exact) = env.sheet_mail("2", "rahul@client.test", "Verma, Rahul", None).await;
    assert!(exact[0].person_id.is_some(), "the exact name from his own address is his");
}

#[tokio::test]
async fn sheets_linked_by_a_partial_name_can_be_listed_for_review() {
    let env = env().await;
    let ravi = env.person("Ravi Kumar", Some("E1024"), "ravi@client.test", None).await;
    let (_, saved) = env.sheet_mail("1", HR, "Dinesh Rao", None).await;
    sqlx::query("UPDATE timesheet_records SET person_id = ?1, match_method = 'name', match_confidence = 0.7 WHERE id = ?2").bind(&ravi).bind(&saved[0].id).execute(&env.state.db).await.unwrap();
    let list = env.ok("GET", "/staffing/timesheet-records/weak-links", ADMIN, None).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1, "{list}");
    assert_eq!(list["items"][0]["id"], json!(saved[0].id));
    let (s, _) = env.req("GET", "/staffing/timesheet-records/weak-links", OPERATOR, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

async fn on() -> Env {
    let env = env().await;
    env.flag(Key::Handoff, Mode::On).await;
    env
}

fn identities_path(id: &str, action: &str) -> String {
    format!("/staffing/identities/{id}/{action}")
}

async fn cards(env: &Env) -> Vec<Value> {
    env.ok("GET", "/staffing/identities", ADMIN, None).await["items"].as_array().unwrap().clone()
}

#[tokio::test]
async fn three_sheets_in_one_email_with_one_unregistered_give_three_entries_and_one_unresolved() {
    let env = on().await;
    env.person("Ravi Kumar", Some("E1024"), "ravi@client.test", None).await;
    env.person("Hema Rao", Some("E1025"), "hema@client.test", None).await;
    let users = (env.count("SELECT COUNT(*) FROM users").await, env.count("SELECT COUNT(*) FROM memberships").await);
    let files = [("ravi.xlsx", sheet("Ravi Kumar", Some("E1024"))), ("hema.xlsx", sheet("Hema Rao", Some("E1025"))), ("dinesh.xlsx", sheet("Dinesh  Rao", Some("X-9")))];
    let id = env.receive_files(&env.shared_src, "gm-1", "th-1", HR, "Timesheets", &files, "timesheet").await;
    let run = records::record_message(&env.state, &env.ws, &id).await.unwrap();
    assert_eq!(run.records.len(), 3, "every sheet is kept");
    type Row = (Option<String>, Option<String>, Option<String>, String, Option<String>, String);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT person_id, person_name, employee_code, status, attachment_filename, sender_address FROM timesheet_records WHERE workspace_id = ?1 AND message_id = ?2 ORDER BY attachment_filename",
    )
    .bind(&env.ws)
    .bind(&id)
    .fetch_all(&env.state.db)
    .await
    .unwrap();
    assert_eq!(rows.iter().filter(|r| r.0.is_some()).count(), 2, "the two registered people are matched");
    let dinesh = rows.iter().find(|r| r.4.as_deref() == Some("dinesh.xlsx")).unwrap();
    assert_eq!((dinesh.0.as_deref(), dinesh.1.as_deref(), dinesh.2.as_deref(), dinesh.3.as_str(), dinesh.5.as_str()), (None, Some("Dinesh  Rao"), Some("X-9"), "needs_check", HR), "the name exactly as written, with its source");
    assert_eq!(env.identities().await, [(Some("Dinesh  Rao".to_string()), Some("X-9".to_string()), "open".to_string(), 1)], "one unresolved identity");
    let card = &cards(&env).await[0];
    assert_eq!((card["name"].as_str(), card["employee_code"].as_str(), card["sender"].as_str(), card["record_count"].as_i64()), (Some("Dinesh  Rao"), Some("X-9"), Some(HR), Some(1)));
    assert_eq!(card["records"][0]["file"], "dinesh.xlsx");
    assert_eq!((env.count("SELECT COUNT(*) FROM users").await, env.count("SELECT COUNT(*) FROM memberships").await), users, "no workspace user is made for a sheet's author");
}

#[tokio::test]
async fn similar_names_become_suggestions_on_the_card_and_nothing_is_linked() {
    let env = on().await;
    env.person("Rahul Verma", Some("E500"), "rahul@client.test", None).await;
    env.person("Priya Singh", Some("E501"), "priya.singh@client.test", None).await;
    env.sheet_mail("1", HR, "Rahul Sharma", None).await;
    env.sheet_mail("2", HR, "Priya S.", None).await;
    let cards = cards(&env).await;
    assert_eq!(cards.len(), 2, "{cards:?}");
    let by = |name: &str| cards.iter().find(|c| c["name"] == name).unwrap_or_else(|| panic!("{name}: {cards:?}"));
    assert_eq!(by("Rahul Sharma")["suggestions"][0]["name"], "Rahul Verma");
    assert_eq!(by("Priya S.")["suggestions"][0]["name"], "Priya Singh");
    assert!(by("Priya S.")["suggestions"][0]["reason"].as_str().unwrap().contains("initial"));
    assert!(env.record_rows().await.iter().all(|r| r.0.is_none() && r.2 == "needs_check"), "never auto-linked");
}

#[tokio::test]
async fn the_same_sheet_twice_is_kept_twice_and_flagged_as_a_possible_duplicate() {
    let env = on().await;
    env.sheet_mail("1", HR, "Dinesh Rao", None).await;
    let (_, second) = env.sheet_mail("2", HR, "Dinesh Rao", None).await;
    assert_eq!(env.record_rows().await.len(), 2, "nothing is discarded");
    assert_eq!(env.identities().await, [(Some("Dinesh Rao".to_string()), None, "open".to_string(), 2)], "one card for both");
    assert!(second[0].issues.iter().any(|i| i["code"] == "possible_duplicate"), "{:?}", second[0].issues);
    let card = &cards(&env).await[0];
    assert_eq!(card["records"].as_array().unwrap().iter().filter(|r| r["possible_duplicate"] == true).count(), 1);
}

#[tokio::test]
async fn a_manual_match_links_every_waiting_sheet_and_undo_puts_them_back_for_a_check() {
    let env = on().await;
    let dinesh = env.person("Dinesh Rao Menon", None, "dinesh@client.test", None).await;
    env.sheet_mail("1", HR, "Dinesh Rao", None).await;
    env.sheet_mail("2", HR, "Dinesh Rao", None).await;
    let card = cards(&env).await[0]["id"].as_str().unwrap().to_string();
    let linked = env.ok("POST", &identities_path(&card, "link"), ADMIN, Some(json!({"person_id": dinesh}))).await;
    assert_eq!(linked["record_ids"].as_array().unwrap().len(), 2);
    // The acknowledgement the link released has gone out already (a reply that went is not recalled by an undo).
    sqlx::query("UPDATE message_requests SET status = 'sent' WHERE workspace_id = ?1").bind(&env.ws).execute(&env.state.db).await.unwrap();
    assert!(env.record_rows().await.iter().all(|r| r.0.as_deref() == Some(dinesh.as_str())), "both sheets joined");
    assert_eq!(env.identities().await[0].2, "linked");
    let changes: Vec<(String, Option<String>, Option<String>)> = sqlx::query_as("SELECT kind, actor_class, undone_by FROM identity_changes WHERE workspace_id = ?1 ORDER BY created_at, rowid").bind(&env.ws).fetch_all(&env.state.db).await.unwrap();
    assert_eq!(changes, [("link".to_string(), Some("user".to_string()), None)], "every link is a logged change");
    let change = linked["change_id"].as_str().unwrap();
    let recent = env.ok("GET", "/staffing/identities", ADMIN, None).await["recent"].clone();
    assert_eq!((recent[0]["kind"].as_str(), recent[0]["can_undo"].as_bool()), (Some("link"), Some(true)));

    env.ok("POST", &format!("/staffing/identity-changes/{change}/undo"), ADMIN, None).await;
    let rows = env.record_rows().await;
    assert!(rows.iter().all(|r| r.0.is_none() && r.2 == "needs_check"), "back to who-is-this and marked needs_check: {rows:?}");
    assert_eq!(env.identities().await, [(Some("Dinesh Rao".to_string()), None, "open".to_string(), 2)]);
    let kinds: Vec<(String, Option<String>)> = sqlx::query_as("SELECT kind, undone_by FROM identity_changes WHERE workspace_id = ?1 ORDER BY created_at, rowid").bind(&env.ws).fetch_all(&env.state.db).await.unwrap();
    assert_eq!(kinds.iter().map(|k| k.0.as_str()).collect::<Vec<_>>(), ["link", "unlink"]);
    assert!(kinds[0].1.is_some(), "the link points at the change that undid it");
    let (s, again) = env.req("POST", &format!("/staffing/identity-changes/{change}/undo"), ADMIN, None).await;
    assert_eq!(s, StatusCode::CONFLICT, "{again}");
}

#[tokio::test]
async fn add_as_a_person_links_every_sheet_and_makes_no_user() {
    let env = on().await;
    let users = (env.count("SELECT COUNT(*) FROM users").await, env.count("SELECT COUNT(*) FROM memberships").await);
    env.sheet_mail("1", HR, "Dinesh Rao", Some("X-9")).await;
    env.sheet_mail("2", HR, "Dinesh Rao", Some("X-9")).await;
    let card = cards(&env).await[0]["id"].as_str().unwrap().to_string();
    let (s, nothing) = env.req("POST", &identities_path(&card, "add-person"), ADMIN, Some(json!({}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "an email is needed, and the sheet gave none: {nothing}");
    let added = env.ok("POST", &identities_path(&card, "add-person"), ADMIN, Some(json!({"email": "dinesh@client.test"}))).await;
    let person = added["person_id"].as_str().unwrap();
    let (name, code): (String, Option<String>) = sqlx::query_as("SELECT name, employee_code FROM people WHERE id = ?1").bind(person).fetch_one(&env.state.db).await.unwrap();
    assert_eq!((name.as_str(), code.as_deref()), ("Dinesh Rao", Some("X-9")), "the name and ID as written");
    assert!(env.record_rows().await.iter().all(|r| r.0.as_deref() == Some(person)));
    let kinds: Vec<String> = sqlx::query_scalar("SELECT kind FROM identity_changes WHERE workspace_id = ?1 ORDER BY created_at, rowid").bind(&env.ws).fetch_all(&env.state.db).await.unwrap();
    assert_eq!(kinds, ["create_person", "link"]);
    assert_eq!((env.count("SELECT COUNT(*) FROM users").await, env.count("SELECT COUNT(*) FROM memberships").await), users, "a person on the roster is not a user");
    let next = env.sheet_mail("3", HR, "Dinesh Rao", Some("X-9")).await.1;
    assert_eq!(next[0].person_id.as_deref(), Some(person), "the next sheet matches by its ID");
}

#[tokio::test]
async fn an_external_or_not_a_person_card_stops_asking_and_can_be_reopened() {
    let env = on().await;
    env.sheet_mail("1", HR, "Client Staff", None).await;
    env.sheet_mail("2", HR, "Sample Person", None).await;
    let list = cards(&env).await;
    let id = |name: &str| list.iter().find(|c| c["name"] == name).unwrap_or_else(|| panic!("{name}: {list:?}"))["id"].as_str().unwrap().to_string();
    let external = env.ok("POST", &identities_path(&id("Client Staff"), "external"), ADMIN, None).await;
    env.ok("POST", &identities_path(&id("Sample Person"), "dismiss"), ADMIN, None).await;
    assert!(cards(&env).await.is_empty());
    assert_eq!(env.identities().await.iter().map(|i| i.2.as_str()).collect::<Vec<_>>(), ["external", "dismissed"]);
    env.ok("POST", &format!("/staffing/identity-changes/{}/undo", external["change_id"].as_str().unwrap()), ADMIN, None).await;
    assert_eq!(cards(&env).await.len(), 1);
    assert_eq!(env.record_rows().await.len(), 2, "no sheet was ever discarded");
}

#[tokio::test]
async fn two_workspaces_never_share_identities() {
    let env = on().await;
    env.sheet_mail("1", HR, "Dinesh Rao", Some("X-9")).await;
    let (s, ws2) = call(&env.app, "POST", "/api/v1/workspaces", ADMIN, Some(json!({"name": "Other", "packs": ["staffing"]}))).await;
    assert_eq!(s, StatusCode::CREATED, "{ws2}");
    let ws2 = ws2["id"].as_str().unwrap().to_string();
    let mut conn = env.state.db.acquire().await.unwrap();
    let identity = email_ai_api::hr::staffing::timesheet_parse::Identity { name: Some("Dinesh Rao".into()), code: Some("X-9".into()), ..Default::default() };
    let theirs = unresolved::attach(&mut conn, &ws2, &identity, HR, &[], &[]).await.unwrap().unwrap();
    drop(conn);
    let mine = cards(&env).await[0]["id"].as_str().unwrap().to_string();
    assert_ne!(theirs, mine, "the same name and ID in another workspace is another identity");
    let person = env.person("Dinesh Rao", Some("X-9"), "dinesh@client.test", None).await;
    let (s, body) = env.req("POST", &identities_path(&theirs, "link"), ADMIN, Some(json!({"person_id": person}))).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "another workspace's card is not here: {body}");
    assert_eq!(cards(&env).await.len(), 1);
    let waiting: i64 = sqlx::query_scalar("SELECT status = 'open' FROM unresolved_identities WHERE id = ?1").bind(&theirs).fetch_one(&env.state.db).await.unwrap();
    assert_eq!(waiting, 1, "untouched");
}

#[tokio::test]
async fn with_the_flag_off_nothing_is_grouped_and_the_old_link_still_works() {
    let env = env().await;
    let ravi = env.person("Ravi Kumar", Some("E1024"), "ravi@client.test", None).await;
    let (_, saved) = env.sheet_mail("1", HR, "D. Rao", Some("X-77")).await;
    assert!(env.identities().await.is_empty(), "no card without the flag");
    let off = env.ok("GET", "/staffing/identities", ADMIN, None).await;
    assert_eq!((off["enabled"].as_bool(), off["items"].as_array().unwrap().len()), (Some(false), 0));
    let (s, _) = env.req("POST", &identities_path("x", "dismiss"), ADMIN, None).await;
    assert_eq!(s, StatusCode::CONFLICT);
    env.ok("POST", &format!("/staffing/timesheet-records/{}/link", saved[0].id), ADMIN, Some(json!({"person_id": ravi}))).await;
    assert_eq!(env.record_rows().await[0].0.as_deref(), Some(ravi.as_str()));
    assert_eq!(env.count("SELECT COUNT(*) FROM identity_changes").await, 0, "the old path writes no change");
    let (s, _) = env.req("GET", "/staffing/identities", OPERATOR, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "only the admin sees names nobody registered");
}

#[tokio::test]
async fn pending_counts_only_roster_people_and_lists_unresolved_apart() {
    let env = on().await;
    env.person("Ravi Kumar", Some("E1024"), "ravi@client.test", None).await;
    env.sheet_mail("1", HR, "Dinesh Rao", Some("X-9")).await;
    env.sheet_mail("2", HR, "Dinesh Rao", Some("X-9")).await;
    let (_, month) = last_month();
    let grid = env.ok("GET", &format!("/staffing/timesheet-grid?month={month}"), ADMIN, None).await;
    let rows = grid["rows"].as_array().unwrap();
    let ravi = rows.iter().find(|r| r["name"] == "Ravi Kumar").unwrap();
    assert_eq!((ravi["status"].as_str(), ravi["unresolved"].as_bool()), (Some("missing"), Some(false)), "Ravi has no sheet, however many unknown names sent one");
    assert_eq!(grid["summary"]["recorded"], 0);
    assert_eq!(grid["summary"]["needs_check"], 0, "an unresolved sheet is not counted for anyone");
    assert_eq!(grid["summary"]["unresolved"], 1, "one card, not one per sheet");
    assert_eq!(grid["unresolved"][0]["name"], "Dinesh Rao");
    assert_eq!(grid["unresolved"][0]["record_ids"].as_array().unwrap().len(), 2);
    assert_eq!(rows.iter().filter(|r| r["unresolved"] == true).count(), 1);
    assert_eq!(grid["roster_empty"], false);
}

// ---------------------------------------------------------------- the sender hears now, the acknowledgement after the link

struct World {
    env: Env,
    mail: Arc<Recorder>,
}

async fn world(rohan_level: i64) -> World {
    world_with(rohan_level, true).await
}

async fn world_with(rohan_level: i64, flag_on: bool) -> World {
    let mut env = Env::new(Opts::packs(&["general", "staffing"]).config(|c| c.agent_auto_send_delay_secs = 600)).await;
    let mail = Arc::new(Recorder::default());
    env.state.alert_mailer = Some(mail.clone());
    let hema = env.person("Hema Rao", None, "priya@acme.test", None).await;
    env.person("Ravi Kumar", Some("E1024"), "ravi@client.test", Some(&hema)).await;
    env.ok("PUT", "/agent", ADMIN, Some(json!({"permissions": {"send": "auto"}}))).await;
    env.ok("PUT", "/me/agent-profile", OPERATOR, Some(json!({"display_name": "Priya", "approvals": {"send_email": "routine_ok"}}))).await;
    if flag_on {
        env.flag(Key::Handoff, Mode::On).await;
    }
    let rohan = env.agent_id(ADMIN, "hrms.timesheets").await;
    sqlx::query("UPDATE agents SET autonomy_level = ?2, autonomy_cap = 3 WHERE id = ?1").bind(&rohan).bind(rohan_level).execute(&env.state.db).await.unwrap();
    World { env, mail }
}

impl World {
    async fn receive(&self, gmail_id: &str, name: &str, code: Option<&str>, from: &str) -> String {
        let (month_name, _) = last_month();
        let id = self.env.receive(&self.env.op_src, gmail_id, &format!("th-{gmail_id}"), from, &format!("{month_name} timesheet"), &sheet(name, code), "timesheet").await;
        self.env.set_understanding(&id, "timesheet", false, json!([])).await;
        worker::drain(&self.env.state).await;
        id
    }

    /// Makes the held send due and runs it.
    async fn release_hold(&self, message_id: &str) {
        let (draft, send_at): (String, i64) = sqlx::query_as("SELECT id, send_at FROM drafts WHERE workspace_id = ?1 AND message_id = ?2 AND send_at IS NOT NULL AND status != 'sent'")
            .bind(&self.env.ws)
            .bind(message_id)
            .fetch_one(&self.env.state.db)
            .await
            .unwrap();
        assert!(send_at > now_ms() + 500_000, "held about ten minutes");
        let due = now_ms();
        sqlx::query("UPDATE drafts SET send_at = ?1 WHERE id = ?2").bind(due).bind(&draft).execute(&self.env.state.db).await.unwrap();
        let mut conn = self.env.state.db.acquire().await.unwrap();
        jobs::enqueue_at(&mut conn, kind::SEND_DRAFT, &format!("send_draft:{draft}:{due}"), Some(&self.env.ws), &json!({"workspace_id": self.env.ws, "draft_id": draft, "send_at": due}), 3, due).await.unwrap();
        drop(conn);
        worker::drain(&self.env.state).await;
    }

    /// Ten minutes pass: a mail we sent on the thread no longer counts as "someone just replied".
    async fn age_replies(&self, message_id: &str) {
        sqlx::query("UPDATE email_messages SET received_at = received_at - 3600000 WHERE workspace_id = ?1 AND direction = 'outbound' AND thread_id = (SELECT thread_id FROM email_messages WHERE id = ?2)")
            .bind(&self.env.ws)
            .bind(message_id)
            .execute(&self.env.state.db)
            .await
            .unwrap();
        sqlx::query("UPDATE drafts SET updated_at = updated_at - 3600000 WHERE workspace_id = ?1 AND status = 'sent' AND message_id = ?2").bind(&self.env.ws).bind(message_id).execute(&self.env.state.db).await.unwrap();
    }

    /// The text of every mail that reached Gmail, oldest first.
    fn sent_texts(&self) -> Vec<String> {
        self.env
            .sends()
            .iter()
            .filter_map(|s| s.get("raw").and_then(Value::as_str).map(|raw| String::from_utf8(URL_SAFE_NO_PAD.decode(raw).unwrap()).unwrap()))
            .collect()
    }
}

#[tokio::test]
async fn a_complete_unknown_sheet_gets_a_receipt_now_and_the_ack_once_after_the_person_is_registered() {
    let w = world(3).await;
    let dinesh_mail = "dinesh@client.test";
    let id = w.receive("gm-1", "Dinesh Rao", Some("X-9"), dinesh_mail).await;

    assert_eq!(w.env.requests(&id).await, [("rohan".to_string(), "holding".to_string(), "composed".to_string()), ("rohan".to_string(), "ack".to_string(), "waiting".to_string())], "a receipt now, the ack parked");
    let drafts = w.env.live_drafts(&id).await;
    assert_eq!(drafts.len(), 1, "one live draft: the receipt");
    assert!(drafts[0].2.contains("received it and are matching it to your record") && !drafts[0].2.contains("recorded") && !drafts[0].2.chars().any(|c| c.is_ascii_digit()), "no hours, no claim: {}", drafts[0].2);
    w.release_hold(&id).await;
    assert_eq!(w.sent_texts().len(), 1, "the receipt went");
    assert_eq!(w.env.requests(&id).await[0].2, "sent");
    w.age_replies(&id).await;

    // They are registered later; one click on the card links the waiting sheet.
    let dinesh = w.env.person("Dinesh Rao", Some("X-9"), dinesh_mail, None).await;
    let card = cards(&w.env).await[0].clone();
    assert_eq!(card["suggestions"][0]["name"], "Dinesh Rao", "the new person is suggested");
    w.env.ok("POST", &identities_path(card["id"].as_str().unwrap(), "link"), ADMIN, Some(json!({"person_id": dinesh}))).await;
    worker::drain(&w.env.state).await;
    assert_eq!(w.env.requests(&id).await, [("rohan".to_string(), "holding".to_string(), "sent".to_string()), ("rohan".to_string(), "ack".to_string(), "composed".to_string())]);
    let drafts = w.env.live_drafts(&id).await;
    assert_eq!(drafts.len(), 1, "the ack is the one live draft now");
    let (month_name, _) = last_month();
    assert!(drafts[0].2.contains(&format!("your timesheet for {month_name} (23.5 h) and recorded it")), "{}", drafts[0].2);
    assert!(w.env.cases_by_job(&id).await.contains(&("reply#2".to_string(), "meera".to_string())), "a follow-up case writes the second mail: {:?}", w.env.cases_by_job(&id).await);
    w.release_hold(&id).await;
    assert_eq!(w.sent_texts().len(), 2, "the receipt, then the ack");

    // Looking again, or linking nothing new, sends nothing more.
    plan::poke(&w.env.state, &w.env.ws, &Wake::PersonLinked { record_ids: vec![] }).await.unwrap();
    let rec: String = sqlx::query_scalar("SELECT id FROM timesheet_records WHERE message_id = ?1").bind(&id).fetch_one(&w.env.state.db).await.unwrap();
    plan::poke(&w.env.state, &w.env.ws, &Wake::PersonLinked { record_ids: vec![rec] }).await.unwrap();
    worker::drain(&w.env.state).await;
    assert_eq!(w.sent_texts().len(), 2, "the parked ack was released once");
    assert!(w.env.live_drafts(&id).await.is_empty());
}

#[tokio::test]
async fn a_correction_for_an_unknown_person_goes_now_and_a_sheet_with_a_missing_field_is_not_acknowledged() {
    let w = world(3).await;
    let wrong_month = {
        let month = clock::previous_month(&clock::previous_month(&clock::month_of(now_ms(), 0)));
        sheet("Dinesh Rao", Some("X-9")).replace(&last_month().1, &month)
    };
    let (month_name, _) = last_month();
    let id = w.env.receive(&w.env.op_src, "gm-1", "th-1", "dinesh@client.test", &format!("{month_name} timesheet"), &wrong_month, "timesheet").await;
    w.env.set_understanding(&id, "timesheet", false, json!([])).await;
    worker::drain(&w.env.state).await;
    let requests = w.env.requests(&id).await;
    assert_eq!(requests, [("rohan".to_string(), "correction".to_string(), "composed".to_string())], "the correction needs no identity, so it goes now: {requests:?}");

    let name_only = w.receive("gm-2", "Mohan Das", None, "mohan@client.test").await;
    assert!(w.env.requests(&name_only).await.is_empty(), "no receipt, no parked ack: asking for the missing field is the next slice's");
    assert!(w.env.live_drafts(&name_only).await.is_empty());
}

#[tokio::test]
async fn undo_is_refused_while_the_ack_it_released_waits_to_go_out() {
    let w = world(1).await;
    let id = w.receive("gm-1", "Dinesh Rao", Some("X-9"), "dinesh@client.test").await;
    let dinesh = w.env.person("Dinesh Rao", Some("X-9"), "dinesh@client.test", None).await;
    let card = cards(&w.env).await[0]["id"].as_str().unwrap().to_string();
    let linked = w.env.ok("POST", &identities_path(&card, "link"), ADMIN, Some(json!({"person_id": dinesh}))).await;
    worker::drain(&w.env.state).await;
    assert!(w.env.requests(&id).await.iter().any(|r| r.1 == "ack" && r.2 == "composed"), "{:?}", w.env.requests(&id).await);
    let (s, body) = w.env.req("POST", &format!("/staffing/identity-changes/{}/undo", linked["change_id"].as_str().unwrap()), ADMIN, None).await;
    assert_eq!(s, StatusCode::CONFLICT, "{body}");
    assert!(body.to_string().contains("waiting to go out"), "{body}");
    assert!(w.env.record_rows().await.iter().all(|r| r.0.is_some()), "nothing was undone");
}

#[tokio::test]
async fn an_ack_right_after_the_receipt_waits_for_a_person_instead_of_a_second_mail_within_ten_minutes() {
    let w = world(3).await;
    let id = w.receive("gm-1", "Dinesh Rao", Some("X-9"), "dinesh@client.test").await;
    w.release_hold(&id).await;
    assert_eq!(w.sent_texts().len(), 1);
    let dinesh = w.env.person("Dinesh Rao", Some("X-9"), "dinesh@client.test", None).await;
    let card = cards(&w.env).await[0]["id"].as_str().unwrap().to_string();
    w.env.ok("POST", &identities_path(&card, "link"), ADMIN, Some(json!({"person_id": dinesh}))).await;
    worker::drain(&w.env.state).await;
    let pending = w.env.ok("GET", "/approvals?status=pending", OPERATOR, None).await["items"].as_array().unwrap().clone();
    assert_eq!(pending.len(), 1, "the existing duplicate guard asks a person: {pending:?}");
    assert!(pending[0]["body"].as_str().unwrap().contains("recorded it"));
    assert_eq!(w.sent_texts().len(), 1, "nothing more went on its own");
}

#[tokio::test]
async fn forgetting_a_sender_erases_the_unknown_names_they_brought() {
    let env = on().await;
    env.sheet_mail("1", "dinesh@client.test", "Dinesh Rao", Some("X-9")).await;
    env.sheet_mail("2", "hema@client.test", "Mohan Das", Some("X-10")).await;
    assert_eq!(env.identities().await.len(), 2);
    let mut conn = env.state.db.acquire().await.unwrap();
    assert_eq!(records::erase_sender(&mut conn, &env.ws, "Dinesh <dinesh@client.test>").await.unwrap(), 1);
    drop(conn);
    assert_eq!(env.identities().await, [(Some("Mohan Das".to_string()), Some("X-10".to_string()), "open".to_string(), 1)], "their card is gone, the other sender's stays");
}

// ---------------------------------------------------------------- review fixes

impl World {
    async fn receive_two(&self, gmail_id: &str, from: &str) -> String {
        let (month_name, _) = last_month();
        let files = [("dinesh.xlsx", sheet("Dinesh Rao", Some("X-9"))), ("mohan.xlsx", sheet("Mohan Das", Some("X-10")))];
        let id = self.env.receive_files(&self.env.op_src, gmail_id, &format!("th-{gmail_id}"), from, &format!("{month_name} timesheets"), &files, "timesheet").await;
        self.env.set_understanding(&id, "timesheet", false, json!([])).await;
        worker::drain(&self.env.state).await;
        id
    }

    async fn card(&self, name: &str) -> String {
        cards(&self.env).await.iter().find(|c| c["name"] == name).unwrap_or_else(|| panic!("no card for {name}"))["id"].as_str().unwrap().to_string()
    }
}

#[tokio::test]
async fn linking_one_of_two_unknown_people_in_a_mail_keeps_the_ack_waiting_for_the_other() {
    let w = world(3).await;
    let id = w.receive_two("gm-1", "dinesh@client.test").await;
    assert_eq!(w.env.requests(&id).await, [("rohan".to_string(), "holding".to_string(), "composed".to_string()), ("rohan".to_string(), "ack".to_string(), "waiting".to_string())]);
    w.release_hold(&id).await;
    w.age_replies(&id).await;
    let dinesh = w.env.person("Dinesh Rao", Some("X-9"), "dinesh@client.test", None).await;
    w.env.ok("POST", &identities_path(&w.card("Dinesh Rao").await, "link"), ADMIN, Some(json!({"person_id": dinesh}))).await;
    worker::drain(&w.env.state).await;
    assert_eq!(w.env.requests(&id).await[1].2, "waiting", "Mohan is still unknown, so nothing is confirmed");
    assert!(w.env.live_drafts(&id).await.is_empty(), "no acknowledgement naming both");
    assert_eq!(w.sent_texts().len(), 1, "only the receipt went");

    let mohan = w.env.person("Mohan Das", Some("X-10"), "mohan@client.test", None).await;
    w.env.ok("POST", &identities_path(&w.card("Mohan Das").await, "link"), ADMIN, Some(json!({"person_id": mohan}))).await;
    worker::drain(&w.env.state).await;
    let drafts = w.env.live_drafts(&id).await;
    assert_eq!(drafts.len(), 1);
    assert!(drafts[0].2.contains("Dinesh Rao") && drafts[0].2.contains("Mohan Das"), "{}", drafts[0].2);
    w.release_hold(&id).await;
    assert_eq!(w.sent_texts().len(), 2, "one acknowledgement, after the last person was linked");
}

#[tokio::test]
async fn linking_a_card_the_desk_never_parked_sends_nothing_and_tells_nobody() {
    let w = world_with(3, false).await;
    let id = w.receive("gm-1", "Dinesh Rao", Some("X-9"), "dinesh@client.test").await;
    assert!(w.env.requests(&id).await.is_empty(), "the flag was off: the desk did nothing");
    w.env.flag(Key::Handoff, Mode::On).await;
    assert_eq!(w.env.count("SELECT COUNT(*) FROM unresolved_identities").await, 0, "turning the flag on writes no card by itself");
    worker::drain(&w.env.state).await;
    assert_eq!(w.env.identities().await.len(), 1, "the refresh job grouped the old sheet");
    let dinesh = w.env.person("Dinesh Rao", Some("X-9"), "dinesh@client.test", None).await;
    w.mail.take();
    w.env.ok("POST", &identities_path(&w.card("Dinesh Rao").await, "link"), ADMIN, Some(json!({"person_id": dinesh}))).await;
    worker::drain(&w.env.state).await;
    assert!(w.env.record_rows().await.iter().all(|r| r.0.as_deref() == Some(dinesh.as_str())), "the sheet got its person");
    assert!(w.env.requests(&id).await.is_empty(), "no request");
    assert!(w.env.live_drafts(&id).await.is_empty(), "no draft");
    assert!(w.sent_texts().is_empty());
    assert!(w.mail.take().is_empty(), "no manager summary either");
}

#[tokio::test]
async fn listing_the_cards_writes_nothing_and_a_refresh_next_to_a_link_never_reopens_it() {
    let w = world_with(3, false).await;
    w.receive("gm-1", "Dinesh Rao", Some("X-9"), "dinesh@client.test").await;
    w.env.flag(Key::Handoff, Mode::On).await;
    let before: (i64, i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM unresolved_identities), (SELECT COALESCE(SUM(updated_at), 0) FROM timesheet_records)").fetch_one(&w.env.state.db).await.unwrap();
    for _ in 0..2 {
        assert!(cards(&w.env).await.is_empty(), "nothing is grouped by a read");
    }
    let after: (i64, i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM unresolved_identities), (SELECT COALESCE(SUM(updated_at), 0) FROM timesheet_records)").fetch_one(&w.env.state.db).await.unwrap();
    assert_eq!(before, after, "GET /identities is read-only");
    let grouped = w.env.ok("POST", "/staffing/identities/refresh", ADMIN, None).await;
    assert_eq!(grouped["grouped"], 1);
    let stamp: i64 = sqlx::query_scalar("SELECT updated_at FROM unresolved_identities").fetch_one(&w.env.state.db).await.unwrap();
    assert_eq!(cards(&w.env).await.len(), 1);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT updated_at FROM unresolved_identities").fetch_one(&w.env.state.db).await.unwrap(), stamp, "and listing does not touch the card");
    let dinesh = w.env.person("Dinesh Rao", Some("X-9"), "dinesh@client.test", None).await;
    w.env.ok("POST", &identities_path(&w.card("Dinesh Rao").await, "link"), ADMIN, Some(json!({"person_id": dinesh}))).await;
    assert_eq!(w.env.ok("POST", "/staffing/identities/refresh", ADMIN, None).await["grouped"], 0);
    assert_eq!(w.env.identities().await[0].2, "linked", "a refresh after the link leaves it linked");
    assert!(w.env.record_rows().await.iter().all(|r| r.0.is_some()));
}

#[tokio::test]
async fn the_link_writes_a_wake_up_job_in_its_own_transaction_and_the_worker_does_the_rest() {
    let w = world(3).await;
    let id = w.receive("gm-1", "Dinesh Rao", Some("X-9"), "dinesh@client.test").await;
    let dinesh = w.env.person("Dinesh Rao", Some("X-9"), "dinesh@client.test", None).await;
    w.env.ok("POST", &identities_path(&w.card("Dinesh Rao").await, "link"), ADMIN, Some(json!({"person_id": dinesh}))).await;
    let jobs: Vec<(String, String)> = sqlx::query_as("SELECT kind, status FROM jobs WHERE kind = 'person_linked'").fetch_all(&w.env.state.db).await.unwrap();
    assert_eq!(jobs, [("person_linked".to_string(), "pending".to_string())], "queued by the link itself");
    assert_eq!(w.env.requests(&id).await[1].2, "waiting", "the request did no desk work");
    worker::drain(&w.env.state).await;
    assert_eq!(w.env.requests(&id).await[1].2, "pending", "the worker released it; the receipt is still held, so the ack follows it");
    assert_eq!(w.env.count("SELECT COUNT(*) FROM audit_events WHERE action = 'handoff.released'").await, 1);
}

#[tokio::test]
async fn a_parked_ack_older_than_manager_after_hours_reminds_the_final_contacts_once() {
    let w = world(3).await;
    w.receive("gm-1", "Dinesh Rao", Some("X-9"), "dinesh@client.test").await;
    let mut settings = w.env.ok("GET", "/team/settings", ADMIN, None).await;
    settings.as_object_mut().unwrap().remove("sources");
    settings["manager_after_hours"] = json!(4);
    settings["final_emails"] = json!(["boss@acme.test"]);
    w.env.ok("PUT", "/team/settings", ADMIN, Some(settings)).await;
    w.mail.take();
    assert_eq!(plan::remind_waiting(&w.env.state, &w.env.ws).await.unwrap(), 0, "not old enough yet");
    sqlx::query("UPDATE message_requests SET created_at = created_at - 5 * 3600000 WHERE status = 'waiting'").execute(&w.env.state.db).await.unwrap();
    assert_eq!(plan::remind_waiting(&w.env.state, &w.env.ws).await.unwrap(), 1);
    let sent = w.mail.take();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].0, "boss@acme.test");
    assert!(!sent[0].2.contains("Dinesh"), "no name in the note: {}", sent[0].2);
    assert!(sent[0].2.contains("has not had a confirmation") && !sent[0].2.contains("receipt"), "claims nothing about a receipt: {}", sent[0].2);
    assert_eq!(plan::remind_waiting(&w.env.state, &w.env.ws).await.unwrap(), 0, "once");
}

#[tokio::test]
async fn a_stranger_gets_no_receipt_and_nobody_gets_more_than_three_a_day() {
    let w = world(3).await;
    let id = w.receive("gm-1", "Dinesh Rao", Some("X-9"), "dinesh@nowhere-we-know.test").await;
    assert_eq!(w.env.requests(&id).await, [("rohan".to_string(), "ack".to_string(), "waiting".to_string())], "no receipt for someone we have never heard of; the ack is still parked");
    assert_eq!(w.env.count("SELECT COUNT(*) FROM audit_events WHERE action = 'handoff.receipt_skipped'").await, 1);

    let mut receipts = 0;
    for (n, first) in ["Anil", "Bina", "Charu", "Deep"].iter().enumerate() {
        let id = w.receive(&format!("gm-k{n}"), &format!("{first} Kumar"), Some(&format!("X-{n}")), "dinesh@client.test").await;
        receipts += w.env.requests(&id).await.iter().filter(|r| r.1 == "holding").count();
    }
    assert_eq!(receipts, 3, "a sender known by their company's domain gets three receipts a day, not four");
}

#[tokio::test]
async fn a_card_set_aside_gets_no_receipt_and_stays_out_of_the_grid_list() {
    let w = world(3).await;
    w.receive("gm-1", "Dinesh Rao", Some("X-9"), "dinesh@client.test").await;
    w.env.ok("POST", &identities_path(&w.card("Dinesh Rao").await, "external"), ADMIN, None).await;
    let again = w.receive("gm-2", "Dinesh Rao", Some("X-9"), "dinesh@client.test").await;
    assert!(w.env.requests(&again).await.is_empty(), "outside staff is not an unknown person to be added: no receipt, nothing parked");
    let (_, month) = last_month();
    let grid = w.env.ok("GET", &format!("/staffing/timesheet-grid?month={month}"), ADMIN, None).await;
    assert_eq!(grid["summary"]["unresolved"], 0);
    assert!(grid["unresolved"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn forgetting_a_sender_also_scrubs_the_change_log_and_the_log_holds_no_names() {
    let env = on().await;
    let dinesh = env.person("Dinesh Rao", Some("X-9"), "dinesh@client.test", None).await;
    env.sheet_mail("1", "dinesh@client.test", "Mohan Das", Some("X-10")).await;
    env.sheet_mail("2", "hema@client.test", "Hema Rao", Some("X-11")).await;
    let card = cards(&env).await.iter().find(|c| c["name"] == "Mohan Das").unwrap()["id"].as_str().unwrap().to_string();
    env.ok("POST", &identities_path(&card, "link"), ADMIN, Some(json!({"person_id": dinesh}))).await;
    let hema_card = cards(&env).await[0]["id"].as_str().unwrap().to_string();
    env.ok("POST", &identities_path(&hema_card, "external"), ADMIN, None).await;
    let log: Vec<String> = sqlx::query_scalar("SELECT before_json || after_json || evidence_json FROM identity_changes WHERE workspace_id = ?1").bind(&env.ws).fetch_all(&env.state.db).await.unwrap();
    assert_eq!(log.len(), 2);
    assert!(log.iter().all(|l| !l.contains("Mohan") && !l.contains("Hema") && !l.contains("Who is this") && !l.contains("Acme")), "ids and statuses only: {log:?}");
    let mut conn = env.state.db.acquire().await.unwrap();
    records::erase_sender(&mut conn, &env.ws, "dinesh@client.test").await.unwrap();
    drop(conn);
    let left: Vec<(String, Option<String>)> = sqlx::query_as("SELECT kind, identity_id FROM identity_changes WHERE workspace_id = ?1").bind(&env.ws).fetch_all(&env.state.db).await.unwrap();
    assert_eq!(left.iter().map(|l| l.0.as_str()).collect::<Vec<_>>(), ["mark_external"], "the link that touched the erased sheet is gone, the other sender's change stays");
}

#[tokio::test]
async fn undo_never_brings_back_a_sheet_a_newer_sheet_stands_in_for() {
    let env = on().await;
    let ravi = env.person("Ravi Kumar", Some("E1024"), "ravi@client.test", None).await;
    env.sheet_mail("1", "ravi@client.test", "Ravi Kumar", Some("E1024")).await;
    env.sheet_mail("2", HR, "Mohan Das", None).await;
    let card = cards(&env).await[0]["id"].as_str().unwrap().to_string();
    let status_of = |name: &'static str| {
        let db = env.state.db.clone();
        async move { sqlx::query_scalar::<_, String>("SELECT status FROM timesheet_records WHERE person_name = ?1 ORDER BY created_at DESC LIMIT 1").bind(name).fetch_one(&db).await.unwrap() }
    };
    let first = env.ok("POST", &identities_path(&card, "link"), ADMIN, Some(json!({"person_id": ravi}))).await;
    assert_eq!(status_of("Ravi Kumar").await, "replaced", "the linked sheet is the newer version of Ravi's month");
    env.ok("POST", &format!("/staffing/identity-changes/{}/undo", first["change_id"].as_str().unwrap()), ADMIN, None).await;
    assert_eq!(status_of("Ravi Kumar").await, "recorded", "undo with nothing newer brings Ravi's own sheet back");

    let second = env.ok("POST", &identities_path(&card, "link"), ADMIN, Some(json!({"person_id": ravi}))).await;
    env.sheet_mail("3", "ravi@client.test", "Ravi Kumar", Some("E1024")).await;
    env.ok("POST", &format!("/staffing/identity-changes/{}/undo", second["change_id"].as_str().unwrap()), ADMIN, None).await;
    let live: i64 = env.count("SELECT COUNT(*) FROM timesheet_records WHERE person_name = 'Ravi Kumar' AND status != 'replaced'").await;
    assert_eq!(live, 1, "only the newest sheet of Ravi's month stands; the old one is not resurrected beside it");
    let issue: String = sqlx::query_scalar("SELECT issues_json FROM timesheet_records WHERE person_name = 'Mohan Das'").fetch_one(&env.state.db).await.unwrap();
    assert!(issue.contains("Who is this? (Mohan Das)"), "the question is rebuilt from the sheet, not copied into the log: {issue}");
}

#[tokio::test]
async fn an_archived_person_cannot_be_linked_to() {
    let env = on().await;
    let ravi = env.person("Ravi Kumar", Some("E1024"), "ravi@client.test", None).await;
    env.sheet_mail("1", HR, "Mohan Das", None).await;
    let card = cards(&env).await[0]["id"].as_str().unwrap().to_string();
    let (s, _) = env.req("DELETE", &format!("/team/people/{ravi}"), ADMIN, None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, body) = env.req("POST", &identities_path(&card, "link"), ADMIN, Some(json!({"person_id": ravi}))).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{body}");
    assert!(env.record_rows().await.iter().all(|r| r.0.is_none()));
}

#[tokio::test]
async fn withdrawing_an_unsent_reply_also_removes_its_gmail_draft() {
    let w = world(1).await;
    let id = w.receive("gm-1", "Ravi Kumar", Some("E1024"), "ravi@client.test").await;
    let draft = w.env.live_drafts(&id).await;
    assert_eq!(draft.len(), 1, "the ack waits for an approval");
    sqlx::query("UPDATE drafts SET provider_draft_id = 'gd9' WHERE id = ?1").bind(&draft[0].0).execute(&w.env.state.db).await.unwrap();
    let rohan = w.env.agent_id(ADMIN, "hrms.timesheets").await;
    let asker = email_ai_api::agents::handoff::Asker { agent_id: &rohan, case_id: None, job: "desk:timesheet" };
    email_ai_api::agents::handoff::store::withdraw(&w.env.state, &w.env.ws, &asker, &id).await.unwrap();
    assert!(w.env.live_drafts(&id).await.is_empty());
    assert!(w.env.calls.lock().unwrap().iter().any(|(k, _)| k == "delete_draft:gd9"), "{:?}", w.env.calls.lock().unwrap());
}

// ---------------------------------------------------------------- round 3

impl World {
    async fn receive_in(&self, gmail_id: &str, thread: &str, subject: &str, body: &str, from: &str) -> String {
        let id = self.env.receive(&self.env.op_src, gmail_id, thread, from, subject, body, "timesheet").await;
        self.env.set_understanding(&id, "timesheet", false, json!([])).await;
        worker::drain(&self.env.state).await;
        id
    }
}

fn holdings(requests: &[(String, String, String)]) -> usize {
    requests.iter().filter(|r| r.1 == "holding").count()
}

#[tokio::test]
async fn a_roster_person_on_gmail_does_not_make_gmail_a_company() {
    let w = world(3).await;
    w.env.person("Gopal Rao", None, "gopal@gmail.com", None).await;
    let stranger = w.receive("gm-1", "Dinesh Rao", Some("X-9"), "stranger@gmail.com").await;
    assert_eq!(holdings(&w.env.requests(&stranger).await), 0, "no receipt to a stranger on gmail");
    let gopal = w.receive("gm-2", "Mohan Das", Some("X-10"), "gopal@gmail.com").await;
    assert_eq!(holdings(&w.env.requests(&gopal).await), 1, "the roster person's own address is trusted, by its exact address");
    // Two different Amit Kumars on gmail are two cards, not one.
    let env = on().await;
    env.person("Gopal Rao", None, "gopal@gmail.com", None).await;
    env.sheet_mail("1", "amit.one@gmail.com", "Amit Kumar", None).await;
    env.sheet_mail("2", "amit.two@gmail.com", "Amit Kumar", None).await;
    assert_eq!(env.identities().await.len(), 2);
}

#[tokio::test]
async fn an_earlier_mail_or_a_claimed_thread_earns_no_receipt_but_an_address_we_wrote_to_does() {
    let w = world(3).await;
    let from = "stranger@nowhere-we-know.test";
    w.env.receive(&w.env.op_src, "gm-0", "th-x", from, "hello", "hi there", "other").await;
    let id = w.receive_in("gm-1", "th-x", "September timesheet", &sheet("Dinesh Rao", Some("X-9")), from).await;
    assert_eq!(holdings(&w.env.requests(&id).await), 0, "writing first, or into a thread, proves nothing: any From can be forged");

    let email = email_ai_api::platform::domain::NormalizedEmail {
        source_message_id: "out-1".into(),
        sender_address: "priya@acme.com".into(),
        recipients: vec![format!("Stranger <{from}>")],
        subject: "Welcome".into(),
        body_text: "Hello".into(),
        received_at: now_ms() - 7_200_000,
        outbound: true,
        ..Default::default()
    };
    let mut tx = w.env.state.db.begin().await.unwrap();
    email_ai_api::mail::ingest::persist_message(&mut tx, &w.env.op_src, &w.env.ws, &email, Some("th-out"), json!({})).await.unwrap();
    tx.commit().await.unwrap();
    let again = w.receive_in("gm-2", "th-y", "October timesheet", &sheet("Mohan Das", Some("X-10")), from).await;
    assert_eq!(holdings(&w.env.requests(&again).await), 1, "an address we have written to is known");
}

#[tokio::test]
async fn a_workspace_sends_at_most_twenty_receipts_an_hour() {
    let w = world(3).await;
    let mut total = 0;
    for n in 0..22 {
        let id = w.receive(&format!("gm-w{n}"), &format!("Person {}", char::from(b'a' + (n % 26) as u8).to_string().repeat(3)), Some(&format!("W-{n}")), &format!("u{n}@client.test")).await;
        total += holdings(&w.env.requests(&id).await);
    }
    assert_eq!(total as i64, email_ai_api::hr::staffing::timesheet_records::desk::RECEIPTS_PER_WORKSPACE_PER_HOUR);
}

#[tokio::test]
async fn a_failure_between_releasing_and_filing_is_finished_by_the_retry() {
    let w = world(3).await;
    let id = w.receive("gm-1", "Dinesh Rao", Some("X-9"), "dinesh@client.test").await;
    w.release_hold(&id).await;
    w.age_replies(&id).await;
    let dinesh = w.env.person("Dinesh Rao", Some("X-9"), "dinesh@client.test", None).await;
    // Filing the acknowledgement fails once (the release itself, which only blanks the digest, still commits).
    sqlx::query("CREATE TRIGGER fail_filing BEFORE UPDATE ON message_requests WHEN NEW.digest != '' AND NEW.kind = 'ack' AND OLD.digest = '' BEGIN SELECT RAISE(ABORT, 'disk full'); END").execute(&w.env.state.db).await.unwrap();
    w.env.ok("POST", &identities_path(&w.card("Dinesh Rao").await, "link"), ADMIN, Some(json!({"person_id": dinesh}))).await;
    worker::drain(&w.env.state).await;
    let after_failure = w.env.requests(&id).await;
    assert_eq!(after_failure[1], ("rohan".to_string(), "ack".to_string(), "pending".to_string()), "released but not written: {after_failure:?}");
    assert!(w.env.live_drafts(&id).await.is_empty());
    sqlx::query("DROP TRIGGER fail_filing").execute(&w.env.state.db).await.unwrap();
    w.env.run_due_after(3_600_000).await;
    assert_eq!(w.env.live_drafts(&id).await.len(), 1, "the retry found the released request and filed the ack");
    w.release_hold(&id).await;
    assert_eq!(w.sent_texts().len(), 2);
}

#[tokio::test]
async fn setting_a_card_aside_drops_what_was_parked_for_it_and_undo_brings_it_back() {
    let w = world(3).await;
    let id = w.receive("gm-1", "Dinesh Rao", Some("X-9"), "dinesh@client.test").await;
    assert_eq!(w.env.requests(&id).await[1].2, "waiting");
    let set = w.env.ok("POST", &identities_path(&w.card("Dinesh Rao").await, "external"), ADMIN, None).await;
    assert_eq!(w.env.requests(&id).await[1].2, "dropped", "nobody is waiting for outside staff");
    let mut settings = w.env.ok("GET", "/team/settings", ADMIN, None).await;
    settings.as_object_mut().unwrap().remove("sources");
    settings["manager_after_hours"] = json!(1);
    settings["final_emails"] = json!(["boss@acme.test"]);
    w.env.ok("PUT", "/team/settings", ADMIN, Some(settings)).await;
    sqlx::query("UPDATE message_requests SET created_at = created_at - 5 * 3600000").execute(&w.env.state.db).await.unwrap();
    w.mail.take();
    assert_eq!(plan::remind_waiting(&w.env.state, &w.env.ws).await.unwrap(), 0, "no reminder about a set-aside card");
    w.env.ok("POST", &format!("/staffing/identity-changes/{}/undo", set["change_id"].as_str().unwrap()), ADMIN, None).await;
    assert_eq!(w.env.requests(&id).await[1].2, "waiting", "reopened, so the acknowledgement waits again");
    // One email, two unknown people: setting one aside keeps the ack parked for the other.
    let two = w.receive_two("gm-2", "dinesh@client.test").await;
    let c = w.card("Mohan Das").await;
    w.env.ok("POST", &identities_path(&c, "dismiss"), ADMIN, None).await;
    assert_eq!(w.env.requests(&two).await[1].2, "waiting", "Dinesh is still unknown in that email");
}

#[tokio::test]
async fn a_forwarded_nameless_sheet_is_nobodys_even_without_an_original_sender() {
    let env = env().await;
    env.person("Ravi Kumar", Some("E1024"), "ravi@client.test", None).await;
    let nameless = sheet("Ravi Kumar", None).replace("Employee Name: Ravi Kumar\n", "");
    let (month_name, _) = last_month();
    let plain = env.receive(&env.shared_src, "gm-1", "th-1", "ravi@client.test", &format!("{month_name} timesheet"), &nameless, "timesheet").await;
    assert!(records::record_message(&env.state, &env.ws, &plain).await.unwrap().records[0].person_id.is_some(), "from his own mailbox it is his");
    for (n, subject) in ["Fwd: September timesheet", "FW: hours"].iter().enumerate() {
        let id = env.receive(&env.shared_src, &format!("gm-f{n}"), &format!("th-f{n}"), "ravi@client.test", subject, &nameless, "timesheet").await;
        let saved = records::record_message(&env.state, &env.ws, &id).await.unwrap();
        assert_eq!(saved.records[0].person_id, None, "{subject}");
    }
}

#[tokio::test]
async fn the_owner_can_unlink_confirm_and_page_through_sheets_linked_by_a_partial_name() {
    let env = on().await;
    let ravi = env.person("Ravi Kumar", Some("E1024"), "ravi@client.test", None).await;
    let mut ids = vec![];
    for (n, name) in ["Dinesh Rao", "Mohan Das", "Hema Rao"].iter().enumerate() {
        let (_, saved) = env.sheet_mail(&n.to_string(), HR, name, None).await;
        sqlx::query("UPDATE timesheet_records SET person_id = ?1, match_method = 'name', match_confidence = 0.7, status = 'recorded', created_at = created_at + ?3 WHERE id = ?2").bind(&ravi).bind(&saved[0].id).bind(n as i64).execute(&env.state.db).await.unwrap();
        ids.push(saved[0].id.clone());
    }
    let first = env.ok("GET", "/staffing/timesheet-records/weak-links?limit=2", ADMIN, None).await;
    assert_eq!(first["items"].as_array().unwrap().len(), 2);
    let next = first["next"].as_str().expect("a next page").to_string();
    let second = env.ok("GET", &format!("/staffing/timesheet-records/weak-links?limit=2&after={}", next.replace('|', "%7C")), ADMIN, None).await;
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    assert!(second["next"].is_null());
    let seen: Vec<&str> = first["items"].as_array().unwrap().iter().chain(second["items"].as_array().unwrap()).map(|i| i["id"].as_str().unwrap()).collect();
    assert_eq!(seen.len(), 3, "every sheet once");

    let (s, _) = env.req("POST", &format!("/staffing/timesheet-records/{}/unlink", ids[0]), OPERATOR, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = env.req("POST", &format!("/staffing/timesheet-records/{}/confirm", ids[0]), OPERATOR, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let unlinked = env.ok("POST", &format!("/staffing/timesheet-records/{}/unlink", ids[0]), ADMIN, None).await;
    let row: (Option<String>, String, String) = sqlx::query_as("SELECT person_id, status, issues_json FROM timesheet_records WHERE id = ?1").bind(&ids[0]).fetch_one(&env.state.db).await.unwrap();
    assert_eq!((row.0, row.1.as_str()), (None, "needs_check"));
    assert!(row.2.contains("Who is this?"));
    let card: (String, i64) = sqlx::query_as("SELECT g.status, g.record_count FROM unresolved_identities g JOIN timesheet_records r ON r.identity_id = g.id WHERE r.id = ?1").bind(&ids[0]).fetch_one(&env.state.db).await.unwrap();
    assert_eq!(card, ("open".to_string(), 1), "it is a card to sort out again");
    let (s, _) = env.req("POST", &format!("/staffing/timesheet-records/{}/unlink", ids[0]), ADMIN, None).await;
    assert_eq!(s, StatusCode::CONFLICT, "not linked any more");
    env.ok("POST", &format!("/staffing/identity-changes/{}/undo", unlinked["change_id"].as_str().unwrap()), ADMIN, None).await;
    let back: Option<String> = sqlx::query_scalar("SELECT person_id FROM timesheet_records WHERE id = ?1").bind(&ids[0]).fetch_one(&env.state.db).await.unwrap();
    assert_eq!(back.as_deref(), Some(ravi.as_str()), "undo puts it back");

    let confirmed = env.ok("POST", &format!("/staffing/timesheet-records/{}/confirm", ids[1]), ADMIN, None).await;
    assert_eq!((confirmed["match_method"].as_str(), confirmed["match_confidence"].as_f64()), (Some("manual"), Some(1.0)));
    let (s, _) = env.req("POST", &format!("/staffing/timesheet-records/{}/confirm", ids[1]), ADMIN, None).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(env.count("SELECT COUNT(*) FROM audit_events WHERE action = 'timesheet.link_confirmed'").await, 1);
    let left = env.ok("GET", "/staffing/timesheet-records/weak-links", ADMIN, None).await;
    assert_eq!(left["items"].as_array().unwrap().len(), 1, "the one put back by hand and the confirmed one left the list");
}

#[tokio::test]
async fn the_scans_that_group_and_list_have_indexes() {
    let env = env().await;
    let names: Vec<String> = sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'timesheet_records'").fetch_all(&env.state.db).await.unwrap();
    assert!(names.contains(&"idx_timesheet_records_ungrouped".to_string()) && names.contains(&"idx_timesheet_records_weak_link".to_string()), "{names:?}");
}
