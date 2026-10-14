//! Cross-tenant isolation over EVERY `/workspaces/{ws}/...` route. The routes are read from the router's
//! source files (so a new route is covered the day it is added), then each is called by a member of
//! workspace B with workspace A's ids, under A's path and under B's own path. Nothing of A may come back.

mod common;

use std::collections::{BTreeSet, HashMap};

use axum::http::StatusCode;
use common::*;
use regex::Regex;
use serde_json::{Value, json};

const SENTINEL: &str = "a-confidential-sentinel";

/// Routes whose request cannot be built as plain JSON: the extractor rejects the call before the handler's
/// membership check, so the cross-tenant call returns 400. The handler checks membership first thing.
const SHADOW_OK: [&str; 1] = ["POST /workspaces/{ws}/assistant/transcribe"];

/// Id routes the owner may not read with 200 in the fixture. Empty on purpose: add a line with the reason.
const OWNER_GET_EXCEPTIONS: [&str; 0] = [];

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Route {
    method: &'static str,
    path: String,
}

/// Every `(method, path)` the API registers under `/workspaces/{ws}`, read from `src/http/*.rs`.
fn routes() -> Vec<Route> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/http");
    let path_re = Regex::new(r#"\.route\(\s*"(/workspaces/\{ws\}[^"]*)""#).unwrap();
    let method_re = Regex::new(r"(?:^|[\s.(,:])(get|post|put|patch|delete)\(").unwrap();
    let mut out = BTreeSet::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let file = entry.unwrap().path();
        if file.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        for cap in path_re.captures_iter(&text) {
            let whole = cap.get(0).unwrap();
            let after_path = &text[whole.end()..];
            let mut depth = 1;
            let mut end = after_path.len();
            for (i, ch) in after_path.char_indices() {
                match ch {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let chain = &after_path[..end];
            let mut found = false;
            for m in method_re.captures_iter(chain) {
                let method = match &m[1] {
                    "get" => "GET",
                    "post" => "POST",
                    "put" => "PUT",
                    "patch" => "PATCH",
                    _ => "DELETE",
                };
                out.insert(Route { method, path: cap[1].to_string() });
                found = true;
            }
            assert!(found, "no HTTP method found for {}", &cap[1]);
        }
    }
    out.into_iter().collect()
}

/// Which table holds the ids that sit in a path position, keyed by the segment before `{id}`.
fn table_for(segment: &str) -> Option<&'static str> {
    Some(match segment {
        "messages" => "email_messages",
        "drafts" => "drafts",
        "labels" => "labels",
        "sources" => "email_sources",
        "memories" => "memories",
        "workflows" => "workflows",
        "reviews" => "review_items",
        "alerts" => "alerts",
        "jobs" => "jobs",
        "agents" => "agents",
        "eval-runs" => "eval_runs",
        "preferences" => "agent_preferences",
        "requirements" => "requirements",
        "matches" => "matches",
        "automations" => "automations",
        "people" => "people",
        "cases" => "cases",
        "approvals" => "agent_approvals",
        "sessions" => "designer_sessions",
        "submissions" => "submissions",
        "ai-incidents" => "ai_incidents",
        "teams" => "teams",
        "conversations" => "watches",
        "support-tickets" => "support_tickets",
        "in-app" | "sent" => "in_app_messages",
        _ => return None,
    })
}

struct Fixture {
    spec: Value,
    ids: HashMap<&'static str, String>,
    empty: Vec<&'static str>,
}

async fn fixture(env: &Env) -> Fixture {
    let msg = env.receive(&env.shared_src, "ga1", "ta1", CLIENT, "A-CONFIDENTIAL-SENTINEL quarterly numbers", "A-CONFIDENTIAL-SENTINEL body text for tenant A only.", "other").await;
    env.req("POST", &format!("/messages/{msg}/draft"), ADMIN, None).await;
    worker_drain(env).await;
    env.req("POST", "/memories", ADMIN, Some(json!({"scope": "workspace", "kind": "fact", "content": "A-CONFIDENTIAL-SENTINEL memory"}))).await;
    let agents = env.ok("GET", "/agents", ADMIN, None).await;
    let lead = agents["categories"].as_array().unwrap().iter().flat_map(|c| c["agents"].as_array().unwrap().clone()).next().unwrap()["id"].as_str().unwrap().to_string();
    env.req("POST", &format!("/agents/{lead}/evals"), ADMIN, Some(json!({"suite": "safety"}))).await;
    env.req("POST", "/designer/sessions", ADMIN, Some(json!({"goal": "A-CONFIDENTIAL-SENTINEL helper"}))).await;
    env.req("POST", "/team/people", ADMIN, Some(json!({"name": "A-CONFIDENTIAL-SENTINEL Person", "emails": ["sentinel@client.test"], "employee_type": "billable"}))).await;
    env.req("POST", "/automations", ADMIN, Some(json!({"name": "A-CONFIDENTIAL-SENTINEL rule", "kind": "auto_reply"}))).await;
    sqlx::query(
        "INSERT OR IGNORE INTO alerts (id, workspace_id, kind, severity, title, body, dedupe_key, level_since, created_at, updated_at)
         VALUES ('alr_a', ?1, 'automation', 'normal', 'A-CONFIDENTIAL-SENTINEL', 'x', 'k', 1, 1, 1)",
    )
    .bind(&env.ws)
    .execute(&env.state.db)
    .await
    .unwrap();
    for (table, id) in [
        ("workflows", "wfl_a"),
        ("review_items", "rvw_a"),
        ("agent_preferences", "prf_a"),
        ("requirements", "req_a"),
        ("matches", "mat_a"),
        ("people", "ppl_a"),
        ("agent_approvals", "apr_a"),
        ("submissions", "sub_a"),
    ] {
        let have: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table} WHERE workspace_id = ?1")).bind(&env.ws).fetch_one(&env.state.db).await.unwrap();
        if have == 0 {
            seed_row(env, table, "id", id, &[]).await;
        }
    }
    let have: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cases WHERE workspace_id = ?1").bind(&env.ws).fetch_one(&env.state.db).await.unwrap();
    if have == 0 {
        seed_row(env, "cases", "id", "cas_a", &[("title", "'A-CONFIDENTIAL-SENTINEL case'")]).await;
    }
    let admin: String = sqlx::query_scalar("SELECT id FROM users WHERE email = 'admin@acme.test'").fetch_one(&env.state.db).await.unwrap();
    let quoted = format!("'{admin}'");
    seed_row(env, "teams", "id", "team_a", &[("name", "'A-CONFIDENTIAL-SENTINEL team'")]).await;
    seed_row(env, "watches", "id", "wch_a", &[("user_id", &quoted), ("kind", "'ask'"), ("channel", "'email'"), ("status", "'waiting'")]).await;
    seed_row(env, "support_tickets", "id", "tkt_a", &[("user_id", &quoted), ("watch_id", "'wch_a'"), ("status", "'open'")]).await;
    seed_row(env, "in_app_messages", "id", "imsg_a", &[("recipient_user_id", &quoted), ("sender_user_id", &quoted), ("kind", "'message'"), ("subject", "'A-CONFIDENTIAL-SENTINEL'")]).await;
    email_ai_api::incidents::record(&env.state.db, email_ai_api::incidents::NewIncident::new(&env.ws, email_ai_api::incidents::Kind::Refusal, json!({"source": "test"}))).await;

    let mut ids = HashMap::new();
    let mut empty = vec![];
    for table in ["email_messages", "drafts", "labels", "email_sources", "memories", "workflows", "review_items", "alerts", "jobs", "agents", "eval_runs", "agent_preferences", "requirements", "matches", "automations", "people", "cases", "agent_approvals", "designer_sessions", "submissions", "ai_incidents", "teams", "watches", "support_tickets", "in_app_messages"] {
        let id: Option<String> = sqlx::query_scalar(&format!("SELECT id FROM {table} WHERE workspace_id = ?1 ORDER BY id LIMIT 1")).bind(&env.ws).fetch_optional(&env.state.db).await.unwrap();
        match id {
            Some(id) => {
                ids.insert(table, id);
            }
            None => empty.push(table),
        }
    }
    let spec = env.ok("GET", &format!("/agents/{lead}/versions/1"), ADMIN, None).await["spec"].clone();
    Fixture { spec, ids, empty }
}

/// Inserts one row with dummy values for every required column, for tables the API cannot fill in a test.
async fn seed_row(env: &Env, table: &str, id_col: &str, id: &str, set: &[(&str, &str)]) {
    use sqlx::Row;
    let cols = sqlx::query(&format!("PRAGMA table_info({table})")).fetch_all(&env.state.db).await.unwrap();
    let sql: String = sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE name = ?1").bind(table).fetch_one(&env.state.db).await.unwrap();
    let mut names = vec![];
    let mut values = vec![];
    for c in &cols {
        let name: String = c.get("name");
        let ty: String = c.get::<String, _>("type").to_uppercase();
        let required = c.get::<i64, _>("notnull") == 1 && c.get::<Option<String>, _>("dflt_value").is_none();
        let given = set.iter().find(|(k, _)| *k == name).map(|(_, v)| (*v).to_string());
        let value = if name == id_col {
            Some(format!("'{id}'"))
        } else if name == "workspace_id" {
            Some(format!("'{}'", env.ws))
        } else if let Some(v) = given {
            Some(v)
        } else if required {
            let check = Regex::new(&format!(r"{name}\s+(?:IN|in)\s*\(\s*'([^']+)'")).unwrap();
            Some(match check.captures(&sql) {
                Some(c) => format!("'{}'", &c[1]),
                None if ty.contains("INT") || ty.contains("REAL") => "1".into(),
                None => "'x'".into(),
            })
        } else {
            None
        };
        if let Some(v) = value {
            names.push(name);
            values.push(v);
        }
    }
    let mut conn = env.state.db.acquire().await.unwrap();
    sqlx::query("PRAGMA foreign_keys = OFF").execute(&mut *conn).await.unwrap();
    let r = sqlx::query(&format!("INSERT OR IGNORE INTO {table} ({}) VALUES ({})", names.join(","), values.join(","))).execute(&mut *conn).await;
    sqlx::query("PRAGMA foreign_keys = ON").execute(&mut *conn).await.unwrap();
    r.unwrap_or_else(|e| panic!("seeding {table}: {e}"));
}

async fn worker_drain(env: &Env) {
    email_ai_api::jobs::worker::drain(&env.state).await;
}

fn fill(path: &str, ws: &str, fx: &Fixture) -> (String, bool) {
    let mut out = String::new();
    let mut has_id = false;
    let segments: Vec<&str> = path.split('/').collect();
    for (i, seg) in segments.iter().enumerate() {
        if i > 0 {
            out.push('/');
        }
        match *seg {
            "{ws}" => out.push_str(ws),
            "{version}" => {
                has_id = true;
                out.push('1');
            }
            "{email}" => {
                has_id = true;
                out.push_str("ravi@client.com");
            }
            "{key}" => {
                has_id = true;
                out.push_str("out_of_office");
            }
            s if s.starts_with('{') => {
                has_id = true;
                let table = table_for(segments[i - 1]).unwrap_or_else(|| panic!("add the resource behind `{}` to table_for() ({path})", segments[i - 1]));
                out.push_str(fx.ids.get(table).map_or("missing_id", String::as_str));
            }
            s => out.push_str(s),
        }
    }
    (out, has_id)
}

/// A body that deserializes, so the request reaches the handler's own membership and ownership checks.
fn body_for(method: &str, template: &str, fx: &Fixture) -> Option<Value> {
    if !matches!(method, "POST" | "PUT" | "PATCH") {
        return None;
    }
    let tail = template.strip_prefix("/workspaces/{ws}").unwrap_or(template);
    let id = |t: &str| fx.ids.get(t).cloned().unwrap_or_default();
    Some(match (method, tail) {
        (_, "/matching/matches/{id}") => json!({"status": "approved"}),
        (_, "/matching/requirements/{id}") => json!({"status": "closed"}),
        (_, "/reviews/{id}") => json!({"status": "done"}),
        (_, "/workflows/{id}") => json!({"enabled": true}),
        (_, "/messages/{id}/classification") => json!({"category": "other"}),
        (_, "/messages/delete") => json!({"ids": ["msg_not_in_this_fixture"]}),
        (_, "/cases") => json!({"kind": "inbox", "message_id": id("email_messages")}),
        (_, "/approvals/{id}") => json!({"decision": "approve"}),
        (_, "/agents/pause-all") => json!({"paused": true}),
        (_, "/messages/{id}/route") => json!({"agent_id": id("agents")}),
        (_, "/assistant/command") => json!({"text": "hello"}),
        (_, "/assistant/confirm") => json!({"plan_item_id": "x"}),
        (_, "/designer/sessions") => json!({"goal": "a helper"}),
        (_, "/designer/sessions/{id}/messages") => json!({"text": "hi"}),
        (_, "/designer/sessions/{id}/spec") | (_, "/agents/{id}/spec") | (_, "/agents") => json!({"spec": fx.spec}),
        (_, "/agents/{id}/autonomy") => json!({"level": 1}),
        (_, "/agents/{id}/evals") => json!({"suite": "safety"}),
        (_, "/drafts/{id}/send") => json!({"confirm": true}),
        (_, "/labels") => json!({"name": "Zed", "description": "d"}),
        (_, "/members") => json!({"email": "zed@x.test", "role": "operator"}),
        (_, "/memories") => json!({"scope": "workspace", "kind": "fact", "content": "x"}),
        (_, "/preferences") => json!({"category": "tone", "content": "x"}),
        (_, "/sources/nango-connections") => json!({"connection_id": "c", "provider_config_key": "google-mail"}),
        (_, "/sources/nango-session") => json!({"provider": "google-mail"}),
        (_, "/staffing/portal/hours") | (_, "/staffing/portal/submissions") | (_, "/team/people/import") => json!({"csv": "a", "dry_run": true}),
        (_, "/team/ownership") => json!({"email": "nobody@x.test"}),
        (_, "/packs") => json!({"packs": ["general"]}),
        (_, "/alert-settings") => json!({"enabled": true, "recipients": [], "escalate_to": [], "unanswered_hours": 4, "escalate_after_hours": 24, "daily_digest": false, "digest_hour": 9}),
        (_, "/automations") => json!({"name": "Rule", "kind": "auto_reply"}),
        (_, "/in-app/{id}/reply") => json!({"body": "hi"}),
        (_, "/support-tickets/{id}/status") => json!({"status": "open"}),
        (_, "/teams") | (_, "/teams/{id}") => json!({"name": "Zed team", "member_ids": []}),
        (_, "/reply-settings") => json!({"auto_reply": false}),
        (_, "/team/settings") => json!({"tz_offset_minutes": 0, "below_target_hour": 9, "weekly_check_hour": 9}),
        (_, "/agent/preview") => json!({"message_id": id("email_messages")}),
        _ => json!({}),
    })
}

#[tokio::test]
async fn the_route_list_is_read_from_the_router_and_is_not_empty() {
    let all = routes();
    assert!(all.len() >= 120, "found only {} routes; did the router move?", all.len());
    for must in [("GET", "/workspaces/{ws}/messages/{id}"), ("POST", "/workspaces/{ws}/drafts/{id}/send"), ("POST", "/workspaces/{ws}/ai-incidents/{id}/resolve"), ("GET", "/workspaces/{ws}/cases/{id}")] {
        assert!(all.iter().any(|r| r.method == must.0 && r.path == must.1), "{must:?} missing");
    }
}

#[tokio::test]
async fn a_member_of_b_gets_nothing_of_a_on_any_workspace_route() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let fx = fixture(&env).await;
    let (status, b) = call(&env.app, "POST", "/api/v1/workspaces", OUTSIDER, Some(json!({"name": "Other Co", "packs": ["general"]}))).await;
    assert_eq!(status, StatusCode::CREATED, "{b}");
    let ws_b = b["id"].as_str().unwrap().to_string();

    let a_ids: Vec<&String> = fx.ids.values().collect();
    let all = routes();
    let (mut under_a, mut under_b, mut shadowed) = (0, 0, 0);
    let mut failures = vec![];
    for r in &all {
        let method = r.method;
        let (path_a, has_id) = fill(&r.path, &env.ws, &fx);
        let (path_b, _) = fill(&r.path, &ws_b, &fx);
        if path_a.contains("missing_id") {
            failures.push(format!("{method} {} has no fixture id", r.path));
        }

        let (s, body) = call(&env.app, method, &format!("/api/v1{path_a}"), OUTSIDER, body_for(method, &r.path, &fx)).await;
        match s {
            StatusCode::FORBIDDEN | StatusCode::NOT_FOUND => under_a += 1,
            StatusCode::BAD_REQUEST | StatusCode::UNSUPPORTED_MEDIA_TYPE | StatusCode::UNPROCESSABLE_ENTITY => {
                shadowed += 1;
                if !SHADOW_OK.contains(&format!("{method} {}", r.path).as_str()) {
                    failures.push(format!("{method} {path_a} as B -> {s}: the body is rejected before the handler runs; fix body_for(): {body}"));
                }
            }
            other => failures.push(format!("{method} {path_a} as B -> {other}: {body}")),
        }
        if body.to_string().to_lowercase().contains(SENTINEL) {
            failures.push(format!("{method} {path_a} as B leaked A's data: {body}"));
        }

        let (s, body) = call(&env.app, method, &format!("/api/v1{path_b}"), OUTSIDER, body_for(method, &r.path, &fx)).await;
        under_b += 1;
        if s.is_server_error() && s != StatusCode::SERVICE_UNAVAILABLE {
            failures.push(format!("{method} {path_b} as B -> {s}: {body}"));
        }
        if has_id && s.is_success() {
            failures.push(format!("{method} {path_b}: A's id answered under B's path -> {s}: {body}"));
        }
        let text = body.to_string();
        if text.to_lowercase().contains(SENTINEL) || (s.is_success() && a_ids.iter().any(|id| text.contains(id.as_str()))) {
            failures.push(format!("{method} {path_b}: A's data under B's path: {body}"));
        }
    }
    assert!(fx.empty.is_empty(), "no fixture row for {:?}; an id route would be tested with a made-up id", fx.empty);
    assert!(!failures.iter().any(|f| f.contains("missing_id")), "missing ids: {failures:?}");
    println!("routes {} | A-path refused {under_a} | A-path stopped by body validation {shadowed} | B-path checked {under_b} | fixtures missing {:?}", all.len(), fx.empty);
    assert!(shadowed <= SHADOW_OK.len(), "{shadowed} routes were stopped by body validation");
    assert!(failures.is_empty(), "tenant isolation failures:\n{}", failures.join("\n"));
}

#[tokio::test]
async fn the_owner_reads_every_id_route_of_their_own_workspace() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let fx = fixture(&env).await;
    let mut bad = vec![];
    let mut with_ids = 0;
    for r in routes().iter().filter(|r| r.method == "GET") {
        let (path, has_id) = fill(&r.path, &env.ws, &fx);
        if !has_id || OWNER_GET_EXCEPTIONS.contains(&r.path.as_str()) {
            continue;
        }
        with_ids += 1;
        let (s, body) = call(&env.app, "GET", &format!("/api/v1{path}"), ADMIN, None).await;
        if s != StatusCode::OK {
            bad.push(format!("GET {} -> {s}: {body}", r.path));
        }
    }
    println!("owner read {with_ids} id routes");
    assert!(with_ids >= 15, "only {with_ids} id routes");
    assert!(bad.is_empty(), "the owner cannot read their own fixtures, so the cross-tenant result proves nothing:\n{}", bad.join("\n"));
}

/// The owner's own calls to every mutating route must get past role and body checks (201/200/204, or a
/// state answer such as 409). If these were refused for the wrong reason, B's 403/404 would prove nothing.
#[tokio::test]
async fn the_owner_reaches_the_handler_of_every_mutating_route() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let fx = fixture(&env).await;
    let mut all: Vec<Route> = routes().into_iter().filter(|r| r.method != "GET").collect();
    all.sort_by_key(|r| r.method == "DELETE");
    let mut bad = vec![];
    for r in &all {
        let (path, _) = fill(&r.path, &env.ws, &fx);
        let (s, body) = call(&env.app, r.method, &format!("/api/v1{path}"), ADMIN, body_for(r.method, &r.path, &fx)).await;
        let code = body["error"]["code"].as_str().unwrap_or_default().to_string();
        let shape_refused = matches!(s.as_u16(), 401 | 403 | 415 | 422) || s.is_server_error() && s != StatusCode::SERVICE_UNAVAILABLE;
        let unparsed = s == StatusCode::BAD_REQUEST && (code == "invalid_json" || body.to_string().contains("missing field"));
        if (shape_refused || unparsed) && !SHADOW_OK.contains(&format!("{} {}", r.method, r.path).as_str()) {
            bad.push(format!("{} {} -> {s}: {body}", r.method, r.path));
        }
    }
    assert!(bad.is_empty(), "owner calls that never reached a handler:\n{}", bad.join("\n"));
}
