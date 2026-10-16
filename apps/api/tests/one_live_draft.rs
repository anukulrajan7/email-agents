//! One live (unsent) reply draft per message: regenerating replaces it in place, a follow-up on the thread
//! retires the earlier machine draft, and a person's edited or approved draft is never overwritten.
//! Candidate-watch follow-ups (`watch:` drafts) are outside the rule.

use email_ai_api::{
    jobs::{self, handlers, kind, worker},
    platform::db,
};
use serde_json::json;

use crate::common::*;

const FIRST: &str = "Hello Ravi,\n\nThanks for the update, noted.\n\nPriya";
const SECOND: &str = "Hello Ravi,\n\nThanks, I have noted your update.\n\nPriya";
const THIRD: &str = "Hello Ravi,\n\nNoted with thanks.\n\nPriya";

async fn env() -> Env {
    Env::new(Opts::packs(&["support"])).await
}

async fn mail(env: &Env, gmail_id: &str, thread: &str) -> String {
    env.receive(&env.shared_src, gmail_id, thread, CLIENT, "Update on the project", "Here is where we are.", "general").await
}

async fn regenerate(env: &Env, message_id: &str, body: &str) {
    env.drafter.set(body);
    let (status, out) = env.req("POST", &format!("/messages/{message_id}/draft"), ADMIN, None).await;
    assert!(status.is_success(), "{status}: {out}");
    worker::drain(&env.state).await;
}

async fn automatic(env: &Env, message_id: &str, body: &str) {
    env.ok("PUT", "/reply-settings", ADMIN, Some(json!({"auto_reply": true, "reply_instructions": "", "signature": null}))).await;
    env.drafter.set(body);
    let mut conn = env.state.db.acquire().await.unwrap();
    jobs::enqueue(&mut conn, kind::GENERATE_DRAFT, &format!("draft:{message_id}:auto"), Some(&env.ws), &json!({ "message_id": message_id })).await.unwrap();
    drop(conn);
    worker::drain(&env.state).await;
}

async fn audits(env: &Env, action: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE workspace_id = ?1 AND action = ?2").bind(&env.ws).bind(action).fetch_one(&env.state.db).await.unwrap()
}

async fn set(env: &Env, sql: &str, id: &str) {
    sqlx::query(sql).bind(id).execute(&env.state.db).await.unwrap();
}

async fn rows(env: &Env, message_id: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM drafts WHERE message_id = ?1").bind(message_id).fetch_one(&env.state.db).await.unwrap()
}

#[tokio::test]
async fn three_regenerates_leave_one_row_with_the_latest_body() {
    let env = env().await;
    let id = mail(&env, "g1", "t1").await;
    regenerate(&env, &id, FIRST).await;
    let first = env.live_drafts(&id).await;
    regenerate(&env, &id, SECOND).await;
    regenerate(&env, &id, THIRD).await;
    let live = env.live_drafts(&id).await;
    assert_eq!(rows(&env, &id).await, 1, "{live:?}");
    assert_eq!(live[0].0, first[0].0, "the same draft, rewritten in place");
    assert_eq!(live[0].1, "suggested");
    assert!(live[0].2.contains("Noted with thanks"), "the third body: {}", live[0].2);
    assert_eq!(audits(&env, "draft.regenerated").await, 2);
    assert_eq!(audits(&env, "draft.generated").await, 1);
}

#[tokio::test]
async fn an_approved_draft_regenerated_goes_back_to_suggested_in_place() {
    let env = env().await;
    let id = mail(&env, "g1", "t1").await;
    regenerate(&env, &id, FIRST).await;
    let before = env.live_drafts(&id).await[0].0.clone();
    set(&env, "UPDATE drafts SET status = 'approved', edit_distance = 0.4 WHERE id = ?1", &before).await;
    regenerate(&env, &id, SECOND).await;
    let live = env.live_drafts(&id).await;
    assert_eq!(live.len(), 1);
    assert_eq!((live[0].0.as_str(), live[0].1.as_str()), (before.as_str(), "suggested"));
    assert!(live[0].2.contains("I have noted your update"), "{}", live[0].2);
    let distance: Option<f64> = sqlx::query_scalar("SELECT edit_distance FROM drafts WHERE id = ?1").bind(&before).fetch_one(&env.state.db).await.unwrap();
    assert_eq!(distance, None);
}

#[tokio::test]
async fn a_draft_with_a_send_scheduled_is_never_rewritten() {
    let env = env().await;
    let id = mail(&env, "g1", "t1").await;
    regenerate(&env, &id, FIRST).await;
    let draft = env.live_drafts(&id).await[0].clone();
    sqlx::query("UPDATE drafts SET status = 'approved', send_at = ?2 WHERE id = ?1").bind(&draft.0).bind(db::now_ms() + 600_000).execute(&env.state.db).await.unwrap();
    regenerate(&env, &id, SECOND).await;
    let live = env.live_drafts(&id).await;
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].2, draft.2, "the body of a scheduled reply stays as the person approved it");
    assert_eq!(audits(&env, "draft.regenerated").await, 0);
}

#[tokio::test]
async fn after_a_reply_is_sent_a_manual_draft_is_a_new_live_draft() {
    let env = env().await;
    let id = mail(&env, "g1", "t1").await;
    regenerate(&env, &id, FIRST).await;
    let sent = env.live_drafts(&id).await[0].0.clone();
    set(&env, "UPDATE drafts SET status = 'sent' WHERE id = ?1", &sent).await;
    regenerate(&env, &id, SECOND).await;
    assert_eq!(rows(&env, &id).await, 2);
    let live = env.live_drafts(&id).await;
    assert_eq!(live.len(), 1);
    assert_ne!(live[0].0, sent);
    regenerate(&env, &id, THIRD).await;
    assert_eq!(rows(&env, &id).await, 2, "1 sent + 1 live");
}

#[tokio::test]
async fn an_automatic_draft_is_not_written_for_an_email_that_was_already_answered() {
    let env = env().await;
    let id = mail(&env, "g1", "t1").await;
    regenerate(&env, &id, FIRST).await;
    let sent = env.live_drafts(&id).await[0].0.clone();
    set(&env, "UPDATE drafts SET status = 'sent' WHERE id = ?1", &sent).await;
    automatic(&env, &id, SECOND).await;
    assert_eq!(rows(&env, &id).await, 1);
}

#[tokio::test]
async fn a_follow_up_on_the_thread_retires_the_earlier_machine_draft() {
    let env = env().await;
    let first = mail(&env, "g1", "t1").await;
    let second = mail(&env, "g2", "t1").await;
    sqlx::query("UPDATE email_messages SET received_at = ?2 WHERE id = ?1").bind(&second).bind(db::now_ms()).execute(&env.state.db).await.unwrap();

    automatic(&env, &first, FIRST).await;
    assert_eq!(env.live_drafts(&first).await.len(), 1);
    sqlx::query("UPDATE drafts SET provider_draft_id = 'gdraft9' WHERE message_id = ?1").bind(&first).execute(&env.state.db).await.unwrap();

    automatic(&env, &second, SECOND).await;
    assert_eq!(rows(&env, &first).await, 0, "the earlier machine draft is gone");
    assert_eq!(env.live_drafts(&second).await.len(), 1);
    assert_eq!(audits(&env, "draft.retired").await, 1);
    assert!(env.calls.lock().unwrap().iter().any(|(k, _)| k == "delete_draft:gdraft9"), "its Gmail draft is deleted too");
}

#[tokio::test]
async fn a_person_edited_draft_is_never_replaced_or_retired() {
    let env = env().await;
    let first = mail(&env, "g1", "t1").await;
    let second = mail(&env, "g2", "t1").await;
    sqlx::query("UPDATE email_messages SET received_at = ?2 WHERE id = ?1").bind(&second).bind(db::now_ms()).execute(&env.state.db).await.unwrap();
    regenerate(&env, &first, FIRST).await;
    let mine = env.live_drafts(&first).await[0].0.clone();
    sqlx::query("UPDATE drafts SET status = 'edited', body_text = 'My own words.' WHERE id = ?1").bind(&mine).execute(&env.state.db).await.unwrap();

    automatic(&env, &second, SECOND).await;
    assert_eq!(env.live_drafts(&second).await.len(), 1, "the follow-up still gets its own draft");
    assert_eq!(env.live_drafts(&first).await, vec![(mine.clone(), "edited".to_string(), "My own words.".to_string())], "not retired");

    automatic(&env, &first, THIRD).await;
    let kept = handlers::create_draft_for_message(&env.state, &first, None).await.unwrap();
    assert_eq!(kept.as_deref(), Some(mine.as_str()), "an automation uses the person's draft");
    let agent = handlers::create_agent_draft(&env.state, &first, Some("Say Friday"), None, "agent:c1:1", None, None).await.unwrap();
    assert_eq!(agent, Some((mine.clone(), "My own words.".to_string())), "an agent never overwrites it");
    assert_eq!(env.live_drafts(&first).await[0].2, "My own words.");
}

#[tokio::test]
async fn watch_drafts_are_outside_the_rule() {
    let env = env().await;
    let first = mail(&env, "g1", "t1").await;
    let second = mail(&env, "g2", "t1").await;
    sqlx::query("UPDATE email_messages SET received_at = ?2 WHERE id = ?1").bind(&second).bind(db::now_ms()).execute(&env.state.db).await.unwrap();
    for key in ["watch:w1:r1", "watch:w1:r2"] {
        sqlx::query("INSERT INTO drafts (id, workspace_id, message_id, body_text, status, origin_key, created_at, updated_at) VALUES (?1, ?2, ?3, 'A role for you', 'suggested', ?1, 1, 1)")
            .bind(key)
            .bind(&env.ws)
            .bind(&first)
            .execute(&env.state.db)
            .await
            .unwrap();
    }

    automatic(&env, &first, FIRST).await;
    assert_eq!(rows(&env, &first).await, 3, "two watch drafts and the reply draft coexist");
    regenerate(&env, &first, SECOND).await;
    assert_eq!(rows(&env, &first).await, 3, "regenerating replaces only the reply draft");
    assert_eq!(env.live_drafts(&first).await.len(), 1);

    automatic(&env, &second, THIRD).await;
    let left: Vec<String> = sqlx::query_scalar("SELECT id FROM drafts WHERE message_id = ?1 ORDER BY id").bind(&first).fetch_all(&env.state.db).await.unwrap();
    assert_eq!(left, ["watch:w1:r1", "watch:w1:r2"], "a follow-up retires the machine reply draft, not the watch drafts");
}

#[tokio::test]
async fn an_automation_and_the_automatic_draft_share_the_one_live_draft() {
    let env = env().await;
    let id = mail(&env, "g1", "t1").await;
    env.drafter.set(FIRST);
    let made = handlers::create_draft_for_message(&env.state, &id, None).await.unwrap().expect("a draft");
    let again = handlers::create_draft_for_message(&env.state, &id, Some("Mention Friday")).await.unwrap();
    assert_eq!(again.as_deref(), Some(made.as_str()));
    automatic(&env, &id, SECOND).await;
    let live = env.live_drafts(&id).await;
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].0, made);
    assert!(live[0].2.contains("Thanks for the update"), "the automation's draft is kept: {}", live[0].2);
}

#[tokio::test]
async fn an_agent_with_instructions_rewrites_a_suggested_draft_in_place() {
    let env = env().await;
    let id = mail(&env, "g1", "t1").await;
    regenerate(&env, &id, FIRST).await;
    let draft = env.live_drafts(&id).await[0].0.clone();
    env.drafter.set(SECOND);
    let written = handlers::create_agent_draft(&env.state, &id, Some("Say Friday"), None, "agent:c1:abc", None, None).await.unwrap().expect("a draft");
    assert_eq!(written.0, draft);
    assert!(written.1.contains("I have noted your update"), "{}", written.1);
    assert_eq!(rows(&env, &id).await, 1);
}

#[tokio::test]
async fn regenerating_refreshes_the_gmail_draft() {
    let env = env().await;
    let id = mail(&env, "g1", "t1").await;
    regenerate(&env, &id, FIRST).await;
    sqlx::query("UPDATE drafts SET provider_draft_id = 'gdraft7' WHERE message_id = ?1").bind(&id).execute(&env.state.db).await.unwrap();
    regenerate(&env, &id, SECOND).await;
    assert!(env.calls.lock().unwrap().iter().any(|(k, _)| k == "update_draft:gdraft7"), "{:?}", env.calls.lock().unwrap());
}
