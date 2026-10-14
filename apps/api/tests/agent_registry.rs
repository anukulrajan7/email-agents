//! The agent registry: rosters seeded from packs (including the HRMS desks), one personal agent per
//! member and its privacy, versions and rollback, pausing, and tenant isolation.

mod common;

use axum::http::StatusCode;
use common::*;
use email_ai_api::{
    agent_registry::{
        seed,
        store,
        templates::{self, AgentTemplate},
    },
    db,
};
use serde_json::{Value, json};

fn names(agents: &[Value], category: &str) -> Vec<String> {
    agents.iter().filter(|a| a["category"] == category).map(|a| a["name"].as_str().unwrap().to_string()).collect()
}

#[tokio::test]
async fn a_support_workspace_shows_general_and_support_agents() {
    let env = Env::new(Opts::packs(&["general", "support"])).await;
    let agents = env.agents(ADMIN).await;
    assert_eq!(names(&agents, "general"), ["Meera", "Inbox Assistant"]);
    assert_eq!(names(&agents, "support"), ["Ticket Resolver", "Escalations"]);
    assert!(names(&agents, "hrms").is_empty() && names(&agents, "sales").is_empty());
    let lead = &agents[0];
    assert_eq!((lead["kind"].as_str(), lead["status"].as_str(), lead["autonomy"]["earned"].as_i64()), (Some("lead"), Some("active"), Some(1)));
    let resolver = agents.iter().find(|a| a["template_key"] == "support.resolver").unwrap();
    assert_eq!((resolver["status"].as_str(), resolver["autonomy"]["earned"].as_i64(), resolver["autonomy"]["effective"].as_i64()), (Some("active"), Some(0), Some(0)), "desks start observing");
    assert_eq!(resolver["active_version"], 1);
    let v = env.ok("GET", "/agents", ADMIN, None).await;
    let order: Vec<&str> = v["categories"].as_array().unwrap().iter().map(|c| c["key"].as_str().unwrap()).collect();
    assert_eq!(order, ["general", "support", "personal"]);
}

#[tokio::test]
async fn a_staffing_workspace_adds_the_five_hrms_desks() {
    let env = Env::new(Opts::packs(&["general", "staffing"])).await;
    let agents = env.agents(ADMIN).await;
    assert_eq!(names(&agents, "hrms"), ["Leave", "Timesheets", "Onboarding", "Candidates & Requirements", "Payroll queries"]);
    let desk = agents.iter().find(|a| a["template_key"] == "hrms.requirements").unwrap();
    assert_eq!(desk["autonomy"]["earned"], 1, "today's requirement agent keeps working for real");
    let leave = agents.iter().find(|a| a["template_key"] == "hrms.leave").unwrap();
    assert_eq!(leave["autonomy"]["cap"], 1);
}

#[tokio::test]
async fn seeding_is_idempotent_and_packs_pause_their_agents_without_deleting_them() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let before = env.count("SELECT COUNT(*) FROM agents WHERE workspace_id = ?1").await;
    let mut conn = env.state.db.acquire().await.unwrap();
    seed::seed_workspace(&mut conn, &env.ws).await.unwrap();
    seed::seed_workspace(&mut conn, &env.ws).await.unwrap();
    drop(conn);
    assert_eq!(env.count("SELECT COUNT(*) FROM agents WHERE workspace_id = ?1").await, before);
    assert_eq!(env.count("SELECT COUNT(*) FROM agent_versions v JOIN agents a ON a.id = v.agent_id WHERE a.workspace_id = ?1").await, before, "one version each");

    env.ok("PUT", "/packs", ADMIN, Some(json!({"packs": ["general", "support"]}))).await;
    let resolver = env.agent_id(ADMIN, "support.resolver").await;
    assert_eq!(env.ok("GET", &format!("/agents/{resolver}"), ADMIN, None).await["status"], "active");

    env.ok("PUT", "/packs", ADMIN, Some(json!({"packs": ["general"]}))).await;
    let paused = env.ok("GET", &format!("/agents/{resolver}"), ADMIN, None).await;
    assert_eq!((paused["status"].as_str(), paused["paused_reason"].as_str()), (Some("paused"), Some("pack_off")));
    assert_eq!(env.req("PATCH", &format!("/agents/{resolver}"), ADMIN, Some(json!({"status": "active"}))).await.0, StatusCode::CONFLICT, "its pack is off");
    assert_eq!(env.agents(ADMIN).await.iter().filter(|a| a["category"] == "support").count(), 2, "still listed, nothing deleted");

    env.ok("PUT", "/packs", ADMIN, Some(json!({"packs": ["general", "support"]}))).await;
    let back = env.ok("GET", &format!("/agents/{resolver}"), ADMIN, None).await;
    assert_eq!((back["status"].as_str(), back["paused_reason"].clone()), (Some("active"), Value::Null));
    assert_eq!(env.count("SELECT COUNT(*) FROM agents WHERE workspace_id = ?1 AND template_key = 'support.resolver'").await, 1, "restored, not duplicated");
}

#[tokio::test]
async fn the_migration_backfills_lead_personal_agents_and_the_hrms_roster() {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::connect(&format!("sqlite://{}", dir.path().join("t.db").display())).await.unwrap();
    let mut before = sqlx::migrate!("./migrations");
    before.migrations = before.migrations.iter().filter(|m| m.version < 33).cloned().collect::<Vec<_>>().into();
    before.run(&pool).await.unwrap();
    for (ws, send) in [("ws_auto", "auto"), ("ws_ask", "ask")] {
        sqlx::query("INSERT INTO workspaces (id, name, created_at) VALUES (?1, 'Old', 1)").bind(ws).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO workspace_agents (workspace_id, name, avatar, color, permissions_json, created_at, updated_at) VALUES (?1, 'Asha', 'AS', 'plum', ?2, 1, 1)")
            .bind(ws)
            .bind(json!({"draft": true, "send": send}).to_string())
            .execute(&pool)
            .await
            .unwrap();
        for pack in ["general", "staffing"] {
            sqlx::query("INSERT INTO workspace_packs (workspace_id, pack_key, enabled_at) VALUES (?1, ?2, 1)").bind(ws).bind(pack).execute(&pool).await.unwrap();
        }
        for u in ["u1", "u2"] {
            sqlx::query("INSERT OR IGNORE INTO users (id, external_auth_subject, email, created_at) VALUES (?1, ?1, ?1 || '@x.test', 1)").bind(u).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO memberships (workspace_id, user_id, role) VALUES (?1, ?2, 'admin')").bind(ws).bind(u).execute(&pool).await.unwrap();
        }
        sqlx::query("INSERT INTO cases (id, workspace_id, kind, title, goal, max_steps, budget_usd, created_at, updated_at) VALUES (?1, ?2, 'inbox', 't', 'g', 15, 0.1, 1, 1)")
            .bind(format!("case_{ws}"))
            .bind(ws)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO automations (id, workspace_id, name, created_at, updated_at) VALUES (?1, ?2, 'Auto', 1, 1)")
            .bind(format!("auto_{ws}"))
            .bind(ws)
            .execute(&pool)
            .await
            .unwrap();
    }
    db::migrate(&pool).await.unwrap();

    for (ws, level) in [("ws_auto", 2), ("ws_ask", 1)] {
        let lead = store::lead(&pool, ws).await.unwrap().expect("a lead");
        assert_eq!((lead.name.as_str(), lead.avatar.as_str(), lead.autonomy_level, lead.autonomy_cap), ("Asha", "AS", level, level), "{ws}: keeps its settings");
        assert_eq!(lead.active_version, Some(1));
        let personal: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agents WHERE workspace_id = ?1 AND scope = 'personal'").bind(ws).fetch_one(&pool).await.unwrap();
        assert_eq!(personal, 2, "one per member");
        let hrms: Vec<String> = sqlx::query_scalar("SELECT name FROM agents WHERE workspace_id = ?1 AND category = 'hrms' ORDER BY created_at, id").bind(ws).fetch_all(&pool).await.unwrap();
        assert_eq!(hrms.len(), 5, "existing workspaces are on the staffing pack: {hrms:?}");
        let case_agent: Option<String> = sqlx::query_scalar("SELECT agent_id FROM cases WHERE workspace_id = ?1").bind(ws).fetch_one(&pool).await.unwrap();
        assert_eq!(case_agent.as_deref(), Some(lead.id.as_str()));
        let (agent, version): (Option<String>, i64) = sqlx::query_as("SELECT agent_id, version FROM automations WHERE workspace_id = ?1").bind(ws).fetch_one(&pool).await.unwrap();
        assert_eq!((agent.as_deref(), version), (Some(lead.id.as_str()), 1));
        let versions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM automation_versions v JOIN automations a ON a.id = v.automation_id WHERE a.workspace_id = ?1").bind(ws).fetch_one(&pool).await.unwrap();
        assert_eq!(versions, 1);
        let missing: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agents WHERE workspace_id = ?1 AND active_version IS NULL").bind(ws).fetch_one(&pool).await.unwrap();
        assert_eq!(missing, 0, "every agent has a version");
    }
    db::migrate(&pool).await.unwrap();
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agents").fetch_one(&pool).await.unwrap();
    assert_eq!(total, 2 * (1 + 2 + 1 + 5), "a second run adds nothing (lead, 2 personal, inbox assistant, 5 HRMS per workspace)");
}

#[tokio::test]
async fn system_agents_cannot_be_deleted_but_custom_ones_can() {
    let env = Env::new(Opts::packs(&["general", "support"])).await;
    let lead = env.agent_id(ADMIN, "general.lead").await;
    let resolver = env.agent_id(ADMIN, "support.resolver").await;
    for id in [&lead, &resolver] {
        assert_eq!(env.req("DELETE", &format!("/agents/{id}"), ADMIN, None).await.0, StatusCode::CONFLICT);
    }
    assert_eq!(env.req("PATCH", &format!("/agents/{lead}"), ADMIN, Some(json!({"status": "paused"}))).await.0, StatusCode::CONFLICT, "the lead is only paused by pause all");
    assert_eq!(env.req("PATCH", &format!("/agents/{resolver}"), ADMIN, Some(json!({"status": "archived"}))).await.0, StatusCode::CONFLICT);
    let paused = env.ok("PATCH", &format!("/agents/{resolver}"), ADMIN, Some(json!({"status": "paused"}))).await;
    assert_eq!((paused["status"].as_str(), paused["paused_reason"].as_str()), (Some("paused"), Some("user")));

    let (s, copy) = env.req("POST", &format!("/agents/{resolver}/clone"), ADMIN, Some(json!({"name": "Refund Desk"}))).await;
    assert_eq!(s, StatusCode::CREATED, "{copy}");
    assert_eq!((copy["kind"].as_str(), copy["status"].as_str(), copy["name"].as_str(), copy["key"].as_str()), (Some("custom"), Some("draft"), Some("Refund Desk"), Some("ticket_resolver_copy")));
    assert_eq!(copy["autonomy"]["earned"], 0);
    assert_eq!(copy["spec"]["permissions"]["max_autonomy"], 2, "a custom agent never exceeds level 2");
    let copy_id = copy["id"].as_str().unwrap();
    assert_eq!(env.req("PATCH", &format!("/agents/{copy_id}"), OPERATOR, Some(json!({"status": "active"}))).await.0, StatusCode::FORBIDDEN, "workspace agents need an admin");
    env.ok("PATCH", &format!("/agents/{copy_id}"), ADMIN, Some(json!({"status": "active", "color": "plum"}))).await;
    let (s, second) = env.req("POST", &format!("/agents/{resolver}/clone"), ADMIN, None).await;
    assert_eq!((s, second["key"].as_str()), (StatusCode::CREATED, Some("ticket_resolver_copy2")));
    assert_eq!(env.ok("DELETE", &format!("/agents/{copy_id}"), ADMIN, None).await["deleted"], true);
    assert_eq!(env.req("GET", &format!("/agents/{copy_id}"), ADMIN, None).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn new_agents_are_validated_against_the_workspace() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let spec = |mutate: &dyn Fn(&mut Value)| {
        let mut s = json!({
            "name": "Refund Helper", "key": "refund_helper", "category": "custom", "description": "d", "goal": "Handle refund questions",
            "mode": "workflow",
            "triggers": [{"type": "email_received", "match": {"labels": ["billing"], "from_domains": [], "from_addresses": [], "subject_keywords": [], "needs_reply": null, "priority": null, "attachment_types": []}}],
            "steps": [{"type": "draft_reply", "quality": "balanced"}],
            "tools": ["read_email", "draft_email"], "scopes": ["read_mail", "draft"],
            "permissions": {"draft": true, "send": "ask", "max_autonomy": 1, "daily_send_cap": 5},
            "memory": {"scope": "own+shared", "learn_from": ["edits"]}, "drafting": {"rules": [], "never_do": [], "quality_tier": "balanced"},
            "budget": {"max_steps": 10, "usd_per_case": 0.05}, "questions_for_user": [], "assumptions": []
        });
        mutate(&mut s);
        json!({"spec": s})
    };
    let (s, v) = env.req("POST", "/agents", ADMIN, Some(spec(&|_| {}))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!((v["status"].as_str(), v["autonomy"]["earned"].as_i64(), v["active_version"].as_i64()), (Some("draft"), Some(0), Some(1)));
    assert_eq!(env.req("POST", "/agents", ADMIN, Some(spec(&|_| {}))).await.0, StatusCode::CONFLICT, "the key is taken");
    assert_eq!(env.req("POST", "/agents", OPERATOR, Some(spec(&|_| {}))).await.0, StatusCode::FORBIDDEN);
    for (what, mutate) in [
        ("unknown label", Box::new(|s: &mut Value| s["triggers"][0]["match"]["labels"] = json!(["nope"])) as Box<dyn Fn(&mut Value)>),
        ("unknown tool", Box::new(|s| s["tools"] = json!(["read_email", "rm_rf"]))),
        ("tool outside its scopes", Box::new(|s| s["tools"] = json!(["read_email", "draft_email", "send_email"]))),
        ("matching without the pack", Box::new(|s| { s["tools"] = json!(["find_candidates"]); s["scopes"] = json!(["read_mail"]); })),
        ("autonomy above 2", Box::new(|s| s["permissions"]["max_autonomy"] = json!(3))),
        ("category of an off pack", Box::new(|s| s["category"] = json!("hrms"))),
        ("bad key", Box::new(|s| s["key"] = json!("Bad Key"))),
        ("unknown field", Box::new(|s| s["secret_power"] = json!(true))),
    ] {
        let mut body = spec(&|s| s["key"] = json!("another_key"));
        mutate(&mut body["spec"]);
        let (s, v) = env.req("POST", "/agents", ADMIN, Some(body)).await;
        assert!(s == StatusCode::BAD_REQUEST || s == StatusCode::UNPROCESSABLE_ENTITY, "{what}: {s} {v}");
    }
}

#[tokio::test]
async fn every_member_has_one_personal_agent_nobody_else_can_see() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let mine_admin = env.personal_id(ADMIN).await;
    let mine_op = env.personal_id(OPERATOR).await;
    assert_ne!(mine_admin, mine_op);
    let admin_list = env.agents(ADMIN).await;
    let op_list = env.agents(OPERATOR).await;
    assert_eq!(admin_list.iter().filter(|a| a["scope"] == "personal").count(), 1, "admins do not see members' personal agents");
    assert_eq!(op_list.iter().filter(|a| a["scope"] == "personal").count(), 1);
    assert!(admin_list.iter().all(|a| a["id"] != mine_op.as_str()) && op_list.iter().all(|a| a["id"] != mine_admin.as_str()));
    let me = env.ok("GET", &format!("/agents/{mine_op}"), OPERATOR, None).await;
    assert_eq!((me["kind"].as_str(), me["scope"].as_str(), me["owner_user_id"].as_str(), me["status"].as_str()), (Some("system"), Some("personal"), Some(env.op_id.as_str()), Some("active")));
    assert_eq!(me["spec"]["memory"]["scope"], "own", "its memory is its own");

    // Everything about it is a 404 to anyone else, an admin included.
    for (method, path, body) in [
        ("GET", format!("/agents/{mine_op}"), None),
        ("PATCH", format!("/agents/{mine_op}"), Some(json!({"status": "paused"}))),
        ("GET", format!("/agents/{mine_op}/versions"), None),
        ("GET", format!("/agents/{mine_op}/versions/1"), None),
        ("POST", format!("/agents/{mine_op}/versions/1/activate"), Some(json!({}))),
        ("GET", format!("/agents/{mine_op}/outgoing"), None),
        ("GET", format!("/agents/{mine_op}/audit"), None),
        ("POST", format!("/agents/{mine_op}/clone"), Some(json!({}))),
        ("DELETE", format!("/agents/{mine_op}"), None),
    ] {
        let (s, v) = env.req(method, &path, ADMIN, body).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{method} {path}: {v}");
    }
    let spec = me["spec"].clone();
    assert_eq!(env.req("PUT", &format!("/agents/{mine_op}/spec"), ADMIN, Some(json!({"spec": spec.clone()}))).await.0, StatusCode::NOT_FOUND);

    // The owner manages it; the cap stays within what the agent can ever do.
    let paused = env.ok("PATCH", &format!("/agents/{mine_op}"), OPERATOR, Some(json!({"status": "paused", "name": "Priya's helper"}))).await;
    assert_eq!((paused["status"].as_str(), paused["name"].as_str()), (Some("paused"), Some("Priya's helper")));
    assert_eq!(env.req("PATCH", &format!("/agents/{mine_op}"), OPERATOR, Some(json!({"autonomy_cap": 3}))).await.0, StatusCode::BAD_REQUEST);
    env.ok("PATCH", &format!("/agents/{mine_op}"), OPERATOR, Some(json!({"autonomy_cap": 2, "status": "active"}))).await;
    let mut tuned = spec;
    tuned["drafting"]["rules"] = json!(["Keep it short"]);
    let saved = env.ok("PUT", &format!("/agents/{mine_op}/spec"), OPERATOR, Some(json!({"spec": tuned}))).await;
    assert_eq!(saved["tuned"], true);

    // A member added later gets one too, and re-seeding repairs a missing one.
    let (s, _) = call(&env.app, "GET", "/api/v1/me", "dev:late@acme.test", None).await;
    assert_eq!(s, StatusCode::OK);
    env.ok("POST", "/members", ADMIN, Some(json!({"email": "late@acme.test", "role": "operator"}))).await;
    assert_eq!(env.count("SELECT COUNT(*) FROM agents WHERE workspace_id = ?1 AND scope = 'personal'").await, 3);
    env.set_pref("DELETE FROM agents WHERE scope = 'personal' AND owner_user_id = ?1", &[&env.op_id]).await;
    seed::ensure_workspace(&env.state.db, &env.ws).await.unwrap();
    assert_eq!(env.count("SELECT COUNT(*) FROM agents WHERE workspace_id = ?1 AND scope = 'personal'").await, 3);
}

#[tokio::test]
async fn a_personal_agents_cases_approvals_and_route_stay_private() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let mine_op = env.personal_id(OPERATOR).await;
    let msg = env.receive(&env.op_src, "m1", "t1", CLIENT, "Status update", "Hi Priya, the interview went well.", "general").await;
    let now = db::now_ms();
    sqlx::query("INSERT INTO cases (id, workspace_id, kind, title, goal, status, max_steps, budget_usd, message_id, created_at, updated_at, agent_id, agent_user_id)
                 VALUES ('case_priv', ?1, 'inbox', 'Private', 'g', 'waiting_approval', 15, 0.1, ?2, ?3, ?3, ?4, ?5)")
        .bind(&env.ws).bind(&msg).bind(now).bind(&mine_op).bind(&env.op_id).execute(&env.state.db).await.unwrap();
    sqlx::query("INSERT INTO agent_approvals (id, workspace_id, case_id, action, summary, status, created_at, agent_id) VALUES ('apr_priv', ?1, 'case_priv', 'send_email', 'Send', 'pending', ?2, ?3)")
        .bind(&env.ws).bind(now).bind(&mine_op).execute(&env.state.db).await.unwrap();
    sqlx::query("UPDATE message_routes SET agent_id = ?1 WHERE message_id = ?2").bind(&mine_op).bind(&msg).execute(&env.state.db).await.unwrap();

    let ids = |v: &Value| v["items"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(ids(&env.ok("GET", "/cases", OPERATOR, None).await), ["case_priv"]);
    assert!(ids(&env.ok("GET", "/cases", ADMIN, None).await).is_empty(), "not even an admin lists it");
    assert!(ids(&env.ok("GET", &format!("/cases?agent_id={mine_op}"), ADMIN, None).await).is_empty());
    assert_eq!(env.req("GET", "/cases/case_priv", ADMIN, None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(env.req("POST", "/cases/case_priv/cancel", ADMIN, None).await.0, StatusCode::NOT_FOUND);
    env.ok("GET", "/cases/case_priv", OPERATOR, None).await;
    assert_eq!(env.ok("GET", "/approvals?status=pending", OPERATOR, None).await["pending_count"], 1);
    let admin_approvals = env.ok("GET", "/approvals?status=pending", ADMIN, None).await;
    assert_eq!((admin_approvals["pending_count"].as_i64(), admin_approvals["items"].as_array().unwrap().len()), (Some(0), 0));
    assert_eq!(env.req("POST", "/approvals/apr_priv", ADMIN, Some(json!({"decision": "decline"}))).await.0, StatusCode::NOT_FOUND);
    let route = env.ok("GET", &format!("/messages/{msg}/route"), ADMIN, None).await;
    assert_eq!(route["route"], Value::Null, "the admin is not told which personal agent has it");
    assert_eq!(env.ok("GET", &format!("/messages/{msg}/route"), OPERATOR, None).await["route"]["agent_id"], mine_op.as_str());
    assert_eq!(env.req("POST", &format!("/messages/{msg}/route"), ADMIN, Some(json!({"agent_id": mine_op}))).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn versions_diff_and_roll_back_with_their_automations() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let lead = env.agent_id(ADMIN, "general.lead").await;
    let detail = env.ok("GET", &format!("/agents/{lead}"), ADMIN, None).await;
    assert_eq!(detail["versions"].as_array().unwrap().len(), 1);
    let v1 = detail["spec"].clone();

    // An automation the lead owns, at version 1.
    sqlx::query("INSERT INTO automations (id, workspace_id, name, enabled, conditions_json, actions_json, created_at, updated_at, agent_id, version)
                 VALUES ('auto_1', ?1, 'Original', 1, '[]', '[]', 1, 1, ?2, 1)").bind(&env.ws).bind(&lead).execute(&env.state.db).await.unwrap();
    sqlx::query("INSERT INTO automation_versions (automation_id, version, definition_json, created_at) VALUES ('auto_1', 1, ?1, 1)")
        .bind(json!({"name": "Original", "enabled": true, "conditions": [], "actions": []}).to_string()).execute(&env.state.db).await.unwrap();

    let mut v2spec = v1.clone();
    v2spec["drafting"]["rules"] = json!(["Keep it short"]);
    let saved = env.ok("PUT", &format!("/agents/{lead}/spec"), ADMIN, Some(json!({"spec": v2spec.clone(), "note": "shorter"}))).await;
    assert_eq!((saved["active_version"].as_i64(), saved["tuned"].as_bool()), (Some(2), Some(true)));
    assert_eq!(saved["versions"][0]["source"], "tuned");
    assert_eq!(saved["versions"][0]["note"], "shorter");

    // The automation changes after v2 was saved; v2 pinned it at version 1.
    sqlx::query("UPDATE automations SET name = 'Edited', version = 2 WHERE id = 'auto_1'").execute(&env.state.db).await.unwrap();
    sqlx::query("INSERT INTO automation_versions (automation_id, version, definition_json, created_at) VALUES ('auto_1', 2, ?1, 2)")
        .bind(json!({"name": "Edited", "enabled": true, "conditions": [], "actions": []}).to_string()).execute(&env.state.db).await.unwrap();
    let mut v3spec = v2spec.clone();
    v3spec["goal"] = json!("A different goal");
    env.ok("PUT", &format!("/agents/{lead}/spec"), ADMIN, Some(json!({"spec": v3spec}))).await;

    let diff = env.ok("GET", &format!("/agents/{lead}/versions/2?diff=prev"), ADMIN, None).await;
    assert_eq!(diff["diff"], json!([{"path": "drafting.rules", "before": [], "after": ["Keep it short"]}]));
    let first = env.ok("GET", &format!("/agents/{lead}/versions/1?diff=prev"), ADMIN, None).await;
    assert_eq!(first["diff"], Value::Null);
    assert_eq!(env.req("GET", &format!("/agents/{lead}/versions/9"), ADMIN, None).await.0, StatusCode::NOT_FOUND);

    let rolled = env.ok("POST", &format!("/agents/{lead}/versions/2/activate"), ADMIN, Some(json!({}))).await;
    assert_eq!(rolled["active_version"], 4, "a rollback is a new version");
    assert_eq!((rolled["versions"][0]["source"].as_str(), rolled["spec"]["drafting"]["rules"].clone()), (Some("rollback"), json!(["Keep it short"])));
    let (name, version): (String, i64) = sqlx::query_as("SELECT name, version FROM automations WHERE id = 'auto_1'").fetch_one(&env.state.db).await.unwrap();
    assert_eq!((name.as_str(), version), ("Original", 1), "the automation is back at the version v2 pinned");
    assert_eq!(env.req("POST", &format!("/agents/{lead}/versions/9/activate"), ADMIN, Some(json!({}))).await.0, StatusCode::NOT_FOUND);
    let mut renamed = v1;
    renamed["key"] = json!("renamed");
    assert_eq!(env.req("PUT", &format!("/agents/{lead}/spec"), ADMIN, Some(json!({"spec": renamed}))).await.0, StatusCode::BAD_REQUEST, "the key never changes");
}

#[tokio::test]
async fn changing_how_an_agent_behaves_caps_it_at_level_one_until_it_passes_the_gate() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let lead = env.agent_id(ADMIN, "general.lead").await;
    env.ok("PATCH", &format!("/agents/{lead}"), ADMIN, Some(json!({"autonomy_cap": 2}))).await;
    let mut conn = env.state.db.acquire().await.unwrap();
    assert!(store::set_autonomy_level(&mut conn, &env.ws, &lead, 2).await.unwrap());
    assert!(!store::set_autonomy_level(&mut conn, &env.ws, &lead, 3).await.unwrap(), "above the cap: refused");
    drop(conn);
    let spec = env.ok("GET", &format!("/agents/{lead}"), ADMIN, None).await["spec"].clone();
    let mut wording = spec.clone();
    wording["description"] = json!("Only words changed");
    let kept = env.ok("PUT", &format!("/agents/{lead}/spec"), ADMIN, Some(json!({"spec": wording}))).await;
    assert_eq!(kept["autonomy"]["earned"], 2, "wording alone does not demote");
    let mut behaviour = spec;
    behaviour["drafting"]["never_do"] = json!(["Offer a discount"]);
    let capped = env.ok("PUT", &format!("/agents/{lead}/spec"), ADMIN, Some(json!({"spec": behaviour}))).await;
    assert_eq!(capped["autonomy"]["earned"], 1);
    let back = env.ok("POST", &format!("/agents/{lead}/versions/1/activate"), ADMIN, Some(json!({}))).await;
    assert_eq!(back["autonomy"]["earned"], 1, "a rollback never raises the level");
}

#[tokio::test]
async fn template_upgrades_follow_untuned_agents_and_offer_tuned_ones_an_update() {
    let env = Env::new(Opts::packs(&["general", "support"])).await;
    let resolver = env.agent_id(ADMIN, "support.resolver").await;
    let escalation = env.agent_id(ADMIN, "support.escalation").await;
    let spec = env.ok("GET", &format!("/agents/{escalation}"), ADMIN, None).await["spec"].clone();
    env.ok("PUT", &format!("/agents/{escalation}/spec"), ADMIN, Some(json!({"spec": spec}))).await;

    let newer: Vec<AgentTemplate> = templates::TEMPLATES
        .iter()
        .map(|t| AgentTemplate {
            key: t.key, category: t.category, pack: t.pack, version: 2, kind: t.kind, scope: t.scope, initial_level: t.initial_level,
            name: t.name, avatar: t.avatar, color: t.color, spec: t.spec,
        })
        .collect();
    let mut conn = env.state.db.acquire().await.unwrap();
    let upgraded = seed::upgrade_templates(&mut conn, &env.ws, &newer).await.unwrap();
    drop(conn);
    assert!(upgraded >= 3, "untuned agents moved to the new template: {upgraded}");
    let resolver_after = env.ok("GET", &format!("/agents/{resolver}"), ADMIN, None).await;
    assert_eq!(resolver_after["versions"][0]["source"], "template");
    assert_eq!(resolver_after["update_available"], false);
    let tuned_after = env.ok("GET", &format!("/agents/{escalation}"), ADMIN, None).await;
    assert_eq!(tuned_after["versions"].as_array().unwrap().len(), 2, "a tuned agent is left alone");
    sqlx::query("UPDATE agents SET template_version = 0 WHERE id = ?1").bind(&escalation).execute(&env.state.db).await.unwrap();
    assert_eq!(env.ok("GET", &format!("/agents/{escalation}"), ADMIN, None).await["update_available"], true, "it offers the update");
}

#[tokio::test]
async fn pause_all_stops_everyone_and_restores_only_what_it_paused() {
    let env = Env::new(Opts::packs(&["general", "support"])).await;
    let resolver = env.agent_id(ADMIN, "support.resolver").await;
    env.ok("PATCH", &format!("/agents/{resolver}"), ADMIN, Some(json!({"status": "paused"}))).await;
    assert_eq!(env.req("POST", "/agents/pause-all", OPERATOR, Some(json!({"paused": true}))).await.0, StatusCode::FORBIDDEN);
    let out = env.ok("POST", "/agents/pause-all", ADMIN, Some(json!({"paused": true}))).await;
    assert!(out["affected"].as_i64().unwrap() >= 4);
    let listed = env.ok("GET", "/agents", ADMIN, None).await;
    assert_eq!(listed["paused_all"], true);
    assert!(env.agents(ADMIN).await.iter().all(|a| a["status"] == "paused"));
    env.ok("POST", "/agents/pause-all", ADMIN, Some(json!({"paused": false}))).await;
    let agents = env.agents(ADMIN).await;
    let by = |k: &str| agents.iter().find(|a| a["template_key"] == k).unwrap()["status"].as_str().unwrap().to_string();
    assert_eq!((by("general.lead").as_str(), by("support.escalation").as_str(), by("support.resolver").as_str()), ("active", "active", "paused"), "the one a person paused stays paused");
    assert_eq!(env.ok("GET", "/agents", ADMIN, None).await["paused_all"], false);
}

#[tokio::test]
async fn the_template_catalogue_says_what_is_installed() {
    let env = Env::new(Opts::packs(&["general", "support"])).await;
    let v = env.ok("GET", "/agent-templates", OPERATOR, None).await;
    let items = v["items"].as_array().unwrap();
    let find = |k: &str| items.iter().find(|t| t["key"] == k).unwrap().clone();
    assert_eq!((find("support.resolver")["installed"].as_bool(), find("support.resolver")["pack_enabled"].as_bool()), (Some(true), Some(true)));
    assert_eq!((find("hrms.leave")["installed"].as_bool(), find("hrms.leave")["pack_enabled"].as_bool()), (Some(false), Some(false)));
    assert_eq!(find("personal.mailbox")["installed"], true, "your own personal agent");
    assert_eq!(find("support.escalation")["spec"]["permissions"]["max_autonomy"], 1);
}

#[tokio::test]
async fn another_workspaces_ids_are_never_found() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let (s, other) = call(&env.app, "POST", "/api/v1/workspaces", OUTSIDER, Some(json!({"name": "Other", "packs": ["general", "support"]}))).await;
    assert_eq!(s, StatusCode::CREATED);
    let other_ws = other["id"].as_str().unwrap();
    let (_, theirs) = call(&env.app, "GET", &format!("/api/v1/workspaces/{other_ws}/agents"), OUTSIDER, None).await;
    let their_agent = theirs["categories"][0]["agents"][0]["id"].as_str().unwrap().to_string();
    // Another workspace's agent under this workspace's path, and this workspace's path for a stranger.
    for path in [format!("/agents/{their_agent}"), format!("/agents/{their_agent}/versions"), format!("/agents/{their_agent}/outgoing")] {
        assert_eq!(env.req("GET", &path, ADMIN, None).await.0, StatusCode::NOT_FOUND, "{path}");
    }
    assert_eq!(env.req("PATCH", &format!("/agents/{their_agent}"), ADMIN, Some(json!({"status": "paused"}))).await.0, StatusCode::NOT_FOUND);
    assert_eq!(env.req("GET", "/agents", OUTSIDER, None).await.0, StatusCode::FORBIDDEN);
    assert!(store::get_in(&env.state.db, &env.ws, &their_agent).await.unwrap().is_none());
    assert!(store::get_in(&env.state.db, other_ws, &their_agent).await.unwrap().is_some());
}
