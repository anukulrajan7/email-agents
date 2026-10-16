//! Workspace flags: off / compare / on per workspace, set by admins only, every change audited.

use axum::http::StatusCode;
use email_ai_api::platform::flags::{self, Key, Mode};
use serde_json::{Value, json};

use crate::common::*;

fn item<'a>(list: &'a Value, key: &str) -> &'a Value {
    list["items"].as_array().unwrap().iter().find(|f| f["key"] == key).unwrap_or_else(|| panic!("no flag {key}: {list}"))
}

#[tokio::test]
async fn a_flag_defaults_from_config_and_a_workspace_row_overrides_it() {
    let env = Env::new(Opts::packs(&["general"]).flags(&[(Key::Supervisor, Mode::Compare)])).await;
    let db = &env.state.db;
    let cfg = &env.state.config;

    assert_eq!(flags::mode(db, cfg, &env.ws, Key::Handoff).await.unwrap(), Mode::Off, "no default and no row is off");
    assert_eq!(flags::mode(db, cfg, &env.ws, Key::Supervisor).await.unwrap(), Mode::Compare, "the config default");

    env.flag(Key::Supervisor, Mode::On).await;
    assert_eq!(flags::mode(db, cfg, &env.ws, Key::Supervisor).await.unwrap(), Mode::On, "the workspace row wins");
    assert_eq!(flags::mode(db, cfg, &env.ws, Key::Handoff).await.unwrap(), Mode::Off);

    let list = env.ok("GET", "/flags", ADMIN, None).await;
    assert_eq!(item(&list, "supervisor")["mode"], "on");
    assert_eq!(item(&list, "supervisor")["default"], "compare");
    assert_eq!(item(&list, "handoff")["mode"], "off");
}

#[tokio::test]
async fn every_registered_flag_is_listed_and_off_with_no_config_defaults() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let list = env.ok("GET", "/flags", ADMIN, None).await;
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), Key::ALL.len());
    for f in items {
        assert_eq!(f["mode"], "off", "{f}");
        assert!(!f["description"].as_str().unwrap().is_empty(), "{f}");
    }
}

#[tokio::test]
async fn only_admins_set_flags_and_each_change_is_audited() {
    let env = Env::new(Opts::packs(&["general"])).await;

    let (s, _) = env.req("PUT", "/flags/handoff", OPERATOR, Some(json!({"mode": "on"}))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = env.req("GET", "/flags", OPERATOR, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(env.count("SELECT COUNT(*) FROM workspace_flags WHERE workspace_id = ?1").await, 0);

    let set = env.ok("PUT", "/flags/handoff", ADMIN, Some(json!({"mode": "compare"}))).await;
    assert_eq!(set["mode"], "compare");
    assert_eq!(item(&env.ok("GET", "/flags", ADMIN, None).await, "handoff")["mode"], "compare");
    env.ok("PUT", "/flags/handoff", ADMIN, Some(json!({"mode": "on"}))).await;

    let rows: Vec<(String, String, i64, String)> = sqlx::query_as(
        "SELECT actor_id, metadata_json, created_at, entity_id FROM audit_events WHERE workspace_id = ?1 AND action = 'flag.changed' ORDER BY created_at, rowid",
    )
    .bind(&env.ws)
    .fetch_all(&env.state.db)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    let first: Value = serde_json::from_str(&rows[0].1).unwrap();
    let second: Value = serde_json::from_str(&rows[1].1).unwrap();
    assert_eq!((rows[0].0.as_str(), rows[0].3.as_str()), (env.admin_id.as_str(), "handoff"));
    assert!(rows[0].2 > 0);
    assert_eq!((first["key"].as_str(), first["from"].as_str(), first["to"].as_str()), (Some("handoff"), Some("off"), Some("compare")));
    assert_eq!((second["from"].as_str(), second["to"].as_str()), (Some("compare"), Some("on")));

    let by: Option<String> = sqlx::query_scalar("SELECT updated_by FROM workspace_flags WHERE workspace_id = ?1 AND key = 'handoff'").bind(&env.ws).fetch_one(&env.state.db).await.unwrap();
    assert_eq!(by.as_deref(), Some(env.admin_id.as_str()));
}

#[tokio::test]
async fn setting_a_flag_to_the_mode_it_already_has_writes_no_audit_row() {
    let env = Env::new(Opts::packs(&["general"])).await;
    env.ok("PUT", "/flags/relations", ADMIN, Some(json!({"mode": "off"}))).await;
    assert_eq!(env.count("SELECT COUNT(*) FROM audit_events WHERE workspace_id = ?1 AND action = 'flag.changed'").await, 0);
}

#[tokio::test]
async fn an_unknown_key_or_mode_is_rejected() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let (s, body) = env.req("PUT", "/flags/not_a_flag", ADMIN, Some(json!({"mode": "on"}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    let (s, _) = env.req("PUT", "/flags/handoff", ADMIN, Some(json!({"mode": "shadow"}))).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(env.count("SELECT COUNT(*) FROM workspace_flags WHERE workspace_id = ?1").await, 0);
    assert_eq!(env.count("SELECT COUNT(*) FROM audit_events WHERE workspace_id = ?1 AND action = 'flag.changed'").await, 0);
}

#[tokio::test]
async fn another_workspace_can_neither_read_nor_set_these_flags() {
    let env = Env::new(Opts::packs(&["general"])).await;
    env.ok("PUT", "/flags/handoff", ADMIN, Some(json!({"mode": "on"}))).await;

    let (s, b) = call(&env.app, "POST", "/api/v1/workspaces", OUTSIDER, Some(json!({"name": "Other Co", "packs": ["general"]}))).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let other = b["id"].as_str().unwrap().to_string();

    let (s, _) = call(&env.app, "GET", &format!("/api/v1/workspaces/{}/flags", env.ws), OUTSIDER, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(&env.app, "PUT", &format!("/api/v1/workspaces/{}/flags/handoff", env.ws), OUTSIDER, Some(json!({"mode": "off"}))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(flags::mode(&env.state.db, &env.state.config, &env.ws, Key::Handoff).await.unwrap(), Mode::On);

    let (s, mine) = call(&env.app, "GET", &format!("/api/v1/workspaces/{other}/flags"), OUTSIDER, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(item(&mine, "handoff")["mode"], "off", "the other workspace has its own flags");
    assert_eq!(flags::mode(&env.state.db, &env.state.config, &other, Key::Handoff).await.unwrap(), Mode::Off);
}
