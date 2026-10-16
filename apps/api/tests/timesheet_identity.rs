//! Slice S8: people nobody registered stay unresolved identities, and nothing merges by itself. A sheet is kept with the
//! name as written; similar names are suggestions, never links; every link is a person's decision, audited and undoable.

use axum::http::StatusCode;
use email_ai_api::{
    hr::{staffing::timesheet_records as records, team::clock},
    platform::db::now_ms,
};
use serde_json::json;

use crate::common::*;

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
    async fn person(&self, name: &str, code: Option<&str>, email: &str) -> String {
        let (s, p) = self.req("POST", "/team/people", ADMIN, Some(json!({"name": name, "emails": [email], "employee_code": code}))).await;
        assert_eq!(s, StatusCode::CREATED, "{p}");
        p["id"].as_str().unwrap().to_string()
    }

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
    env.person("Rahul Verma", Some("E500"), "rahul@client.test").await;
    env.person("Priya Singh", Some("E501"), "priya.singh@client.test").await;
    env.person("Ravi Kumar Singh", None, "ravi.singh@client.test").await;
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
    let singh = env.person("Amit Kumar Singh", Some("E1"), "amit.singh@client.test").await;
    env.person("Amit Kumar Jha", Some("E2"), "amit.jha@client.test").await;
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
    let ravi = env.person("Ravi Kumar", Some("E1024"), "ravi@client.test").await;
    let priya = env.person("Priya Shah", Some("E2000"), "priya@client.test").await;
    let (_, saved) = env.sheet_mail("1", HR, "D. Rao", Some("X-77")).await;
    sqlx::query("INSERT INTO timesheet_aliases (workspace_id, kind, value, person_id, created_at) VALUES (?1, 'code', 'x-77', ?2, 1)").bind(&env.ws).bind(&ravi).execute(&env.state.db).await.unwrap();
    let (s, body) = env.req("POST", &format!("/staffing/timesheet-records/{}/link", saved[0].id), ADMIN, Some(json!({"person_id": priya}))).await;
    assert_eq!(s, StatusCode::CONFLICT, "{body}");
    assert!(body.to_string().contains("Ravi Kumar"), "names the owner: {body}");
    let owner: String = sqlx::query_scalar("SELECT person_id FROM timesheet_aliases WHERE value = 'x-77'").fetch_one(&env.state.db).await.unwrap();
    assert_eq!(owner, ravi, "the alias was not re-pointed");
    assert_eq!(env.record_rows().await[0].0, None, "and the sheet stays unresolved");
}
