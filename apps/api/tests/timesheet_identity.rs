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
    let theirs = unresolved::attach(&mut conn, &ws2, &identity, HR, &[]).await.unwrap().unwrap();
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
}

async fn world(rohan_level: i64) -> World {
    let mut env = Env::new(Opts::packs(&["general", "staffing"]).config(|c| c.agent_auto_send_delay_secs = 600)).await;
    env.state.alert_mailer = Some(Arc::new(Recorder::default()));
    let hema = env.person("Hema Rao", None, "priya@acme.test", None).await;
    env.person("Ravi Kumar", Some("E1024"), "ravi@client.test", Some(&hema)).await;
    env.ok("PUT", "/agent", ADMIN, Some(json!({"permissions": {"send": "auto"}}))).await;
    env.ok("PUT", "/me/agent-profile", OPERATOR, Some(json!({"display_name": "Priya", "approvals": {"send_email": "routine_ok"}}))).await;
    env.flag(Key::Handoff, Mode::On).await;
    let rohan = env.agent_id(ADMIN, "hrms.timesheets").await;
    sqlx::query("UPDATE agents SET autonomy_level = ?2, autonomy_cap = 3 WHERE id = ?1").bind(&rohan).bind(rohan_level).execute(&env.state.db).await.unwrap();
    World { env }
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
