//! The new-version safety gate for fixed-workflow agents: scripted-planner expectations do not apply,
//! the safety invariants still do, text-only template upgrades do not count as behaviour changes,
//! and agents demoted by the old gate are restored once at startup.

use crate::common;

use common::*;
use email_ai_api::{
    agent_registry::{
        spec::SendMode,
        store, templates,
        versions::{self, NewVersion},
    },
    evals::{gate, run},
};
use serde_json::json;

fn personal_spec() -> email_ai_api::agent_registry::spec::AgentSpec {
    templates::personal_template().spec()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_personal_workflow_agent_passes_the_safety_suite() {
    let result = safety_suite(&personal_spec()).await;
    let unsafe_cases: Vec<_> = result.outcomes.iter().filter(|o| !o.safe()).map(|o| (&o.id, &o.failures)).collect();
    assert!(result.all_safe(), "{unsafe_cases:?}");
    assert_eq!(result.passed(), result.total());
}

#[tokio::test(flavor = "multi_thread")]
async fn workflow_cases_stay_inside_the_step_and_budget_limits() {
    let result = safety_suite(&personal_spec()).await;
    for id in ["limit_step_cap", "limit_budget_cap"] {
        let o = result.outcomes.iter().find(|o| o.id == id).unwrap();
        assert!(o.failures.iter().all(|f| !f.starts_with("SAFETY")), "{id}: {:?}", o.failures);
        assert!(matches!(o.class.as_str(), "failed" | "approval"), "{id} is stopped or waits for a person, never finished on its own: {}", o.class);
        assert!(o.steps <= 3, "{id}: {} steps", o.steps);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_workflow_that_sends_on_its_own_passes_only_because_the_sensitive_floor_holds() {
    let mut spec = personal_spec();
    spec.permissions.send = SendMode::Earned;
    spec.permissions.max_autonomy = 3;
    let result = safety_suite(&spec).await;
    let unsafe_cases: Vec<_> = result.outcomes.iter().filter(|o| !o.safe()).map(|o| (&o.id, &o.failures)).collect();
    assert!(result.all_safe(), "level 3 acts on its own but money, legal, security, attachments and injected mail still ask: {unsafe_cases:?}");
}

async fn personal_at_level_one(env: &Env) -> (String, String) {
    let id = env.personal_id(OPERATOR).await;
    sqlx::query("UPDATE agents SET autonomy_level = 1, autonomy_cap = 2 WHERE id = ?1").bind(&id).execute(&env.state.db).await.unwrap();
    (env.ws.clone(), id)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_text_only_template_upgrade_keeps_the_level_and_a_behaviour_upgrade_passes_the_gate() {
    let env = Env::new(Opts::packs(&["general"]).config(|c| c.evals_on_version = true)).await;
    let (ws, id) = personal_at_level_one(&env).await;
    let agent = store::get_in(&env.state.db, &ws, &id).await.unwrap().unwrap();
    let (current, _) = versions::active_spec_in(&env.state.db, &agent).await.unwrap();

    let mut text = current.clone();
    text.name = "Someone else".into();
    text.persona = "A new persona.".into();
    text.description = "New words".into();
    let mut conn = env.state.db.acquire().await.unwrap();
    let v = versions::create(&mut conn, &ws, &id, &text, NewVersion { source: "template", note: None, created_by: None, designer_session_id: None, activate: true }).await.unwrap();
    drop(conn);
    let queued: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM eval_runs WHERE agent_id = ?1 AND agent_version = ?2").bind(&id).bind(v).fetch_one(&env.state.db).await.unwrap();
    assert_eq!(queued, 0, "wording alone queues no checks");

    let mut changed = text.clone();
    changed.drafting.rules.push("Keep it short".into());
    let mut conn = env.state.db.acquire().await.unwrap();
    let v = versions::create(&mut conn, &ws, &id, &changed, NewVersion { source: "template", note: None, created_by: None, designer_session_id: None, activate: true }).await.unwrap();
    drop(conn);
    let runs: Vec<String> = sqlx::query_scalar("SELECT id FROM eval_runs WHERE agent_id = ?1 AND agent_version = ?2 AND suite = 'safety'").bind(&id).bind(v).fetch_all(&env.state.db).await.unwrap();
    assert_eq!(runs.len(), 1);
    run::execute(&env.state, &runs[0]).await.unwrap();
    let level: i64 = sqlx::query_scalar("SELECT autonomy_level FROM agents WHERE id = ?1").bind(&id).fetch_one(&env.state.db).await.unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM eval_runs WHERE id = ?1").bind(&runs[0]).fetch_one(&env.state.db).await.unwrap();
    assert_eq!((status.as_str(), level), ("passed", 1), "the personal agent keeps its level");
}

async fn demote_by_version(env: &Env, ws: &str, id: &str, source: &str) -> i64 {
    let agent = store::get_in(&env.state.db, ws, id).await.unwrap().unwrap();
    let (mut spec, _) = versions::active_spec_in(&env.state.db, &agent).await.unwrap();
    spec.persona = format!("{source} persona");
    let mut tx = env.state.db.begin_with("BEGIN IMMEDIATE").await.unwrap();
    let v = versions::create(&mut tx, ws, id, &spec, NewVersion { source, note: None, created_by: None, designer_session_id: None, activate: true }).await.unwrap();
    assert!(gate::demote(&mut tx, ws, id, 0, gate::SAFETY_DEMOTION_REASON, json!({"version": v}), "system").await.unwrap());
    tx.commit().await.unwrap();
    v
}

async fn level_of(env: &Env, id: &str) -> i64 {
    sqlx::query_scalar("SELECT autonomy_level FROM agents WHERE id = ?1").bind(id).fetch_one(&env.state.db).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn startup_restores_an_agent_the_old_gate_demoted_after_a_template_upgrade() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let (ws, id) = personal_at_level_one(&env).await;
    let v = demote_by_version(&env, &ws, &id, "template").await;
    assert_eq!(level_of(&env, &id).await, 0);

    assert_eq!(gate::repair_template_demotions(&env.state.db).await.unwrap(), 1);
    assert_eq!(level_of(&env, &id).await, 1, "back to the level it had");
    let (from, to, reason): (i64, i64, String) =
        sqlx::query_as("SELECT from_level, to_level, reason FROM autonomy_changes WHERE agent_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT 1").bind(&id).fetch_one(&env.state.db).await.unwrap();
    assert_eq!((from, to, reason.as_str()), (0, 1, "Restored after gate fix"));
    let gate_status: String = sqlx::query_scalar("SELECT gate_status FROM agent_versions WHERE agent_id = ?1 AND version = ?2").bind(&id).bind(v).fetch_one(&env.state.db).await.unwrap();
    assert_eq!(gate_status, "passed");

    assert_eq!(gate::repair_template_demotions(&env.state.db).await.unwrap(), 0, "idempotent");
    sqlx::query("UPDATE agents SET autonomy_level = 0 WHERE id = ?1").bind(&id).execute(&env.state.db).await.unwrap();
    assert_eq!(gate::repair_template_demotions(&env.state.db).await.unwrap(), 0, "a later change is never undone");
    assert_eq!(level_of(&env, &id).await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn startup_leaves_an_agent_demoted_after_an_admin_edit_alone() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let (ws, id) = personal_at_level_one(&env).await;
    demote_by_version(&env, &ws, &id, "manual").await;
    assert_eq!(gate::repair_template_demotions(&env.state.db).await.unwrap(), 0);
    assert_eq!(level_of(&env, &id).await, 0);
}
