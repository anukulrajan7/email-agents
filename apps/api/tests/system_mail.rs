//! Post Room's own system mail ("[Post Room] 2 emails need attention", "No reply yet: ...") lands in the
//! owner's connected mailbox. It never gets a draft, a case, an automation run or an alert; a manager's
//! "Re: [Post Room] ..." reply is ordinary mail.

use email_ai_api::{
    agents::triggers::{self, Started},
    jobs::{self, kind, worker},
    mail::ingest,
    platform::{db, domain::NormalizedEmail},
};
use serde_json::json;

use crate::common::*;

const FROM: &str = "no-reply@postroom.test";
const SUBJECT: &str = "[Post Room] 2 emails need attention";

async fn env() -> Env {
    let env = Env::new(Opts::packs(&["support"]).config(|c| c.smtp_from = format!("Post Room <{FROM}>"))).await;
    env.ok("PUT", "/reply-settings", ADMIN, Some(json!({"auto_reply": true, "reply_instructions": "", "signature": null}))).await;
    let (status, made) = env
        .req(
            "POST",
            "/automations",
            ADMIN,
            Some(json!({"name": "Draft every email", "enabled": true, "conditions": [], "actions": [{"type": "draft_reply"}]})),
        )
        .await;
    assert_eq!(status, axum::http::StatusCode::CREATED, "{made}");
    env
}

async fn receive_signalled(env: &Env, gmail_id: &str, from: &str, signals: &[&str]) -> String {
    let email = NormalizedEmail {
        source_message_id: gmail_id.into(),
        rfc_message_id: Some(format!("<{gmail_id}@mail.example>")),
        sender_address: from.into(),
        recipients: vec!["team@acme.com".into()],
        subject: SUBJECT.into(),
        body_text: "Two emails have waited longer than 24 hours.".into(),
        received_at: db::now_ms() - 3_600_000,
        bulk_signals: signals.iter().map(|s| (*s).to_string()).collect(),
        ..Default::default()
    };
    let mut tx = env.state.db.begin().await.unwrap();
    let id = ingest::persist_message(&mut tx, &env.shared_src, &env.ws, &email, Some(&format!("t-{gmail_id}")), json!({})).await.unwrap().unwrap();
    tx.commit().await.unwrap();
    worker::drain(&env.state).await;
    env.set_understanding(&id, "general", true, json!([])).await;
    id
}

async fn ask_for_automatic_draft(env: &Env, message_id: &str) {
    let mut conn = env.state.db.acquire().await.unwrap();
    jobs::enqueue(&mut conn, kind::GENERATE_DRAFT, &format!("draft:{message_id}:auto"), Some(&env.ws), &json!({ "message_id": message_id })).await.unwrap();
    drop(conn);
    worker::drain(&env.state).await;
}

async fn run_automations_again(env: &Env, message_id: &str) {
    let mut conn = env.state.db.acquire().await.unwrap();
    jobs::enqueue(&mut conn, kind::RUN_AUTOMATIONS, &format!("automations-again:{message_id}"), Some(&env.ws), &json!({ "message_id": message_id })).await.unwrap();
    drop(conn);
    worker::drain(&env.state).await;
}

async fn nothing_was_done_for(env: &Env, id: &str) {
    ask_for_automatic_draft(env, id).await;
    run_automations_again(env, id).await;
    let started = triggers::start_inbox_case(&env.state, &env.ws, id, None).await.unwrap();
    assert!(matches!(started, Started::NotAllowed(_)), "no automatic case on our own mail: {started:?}");
    let drafts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM drafts WHERE message_id = ?1").bind(id).fetch_one(&env.state.db).await.unwrap();
    let runs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM automation_runs WHERE message_id = ?1").bind(id).fetch_one(&env.state.db).await.unwrap();
    let cases = env.cases_of(id).await.into_iter().filter(|(_, _, shadow)| !shadow).count();
    assert_eq!((drafts, runs, cases), (0, 0, 0), "drafts, automation runs, real cases");
}

#[tokio::test]
async fn mail_from_our_smtp_address_without_our_header_is_ordinary() {
    let env = env().await;
    let id = env.receive(&env.shared_src, "ord1", "t-ord1", &FROM.to_uppercase(), "Test-pack mail", "Please confirm the schedule.", "general").await;
    assert!(!email_ai_api::mail::system_mail::is_own_message(&env.state.db, &env.ws, &id).await.unwrap());
    let runs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM automation_runs r JOIN automations a ON a.id = r.automation_id WHERE r.message_id = ?1 AND a.name = 'Draft every email'",
    )
    .bind(&id)
    .fetch_one(&env.state.db)
    .await
    .unwrap();
    assert_eq!(runs, 1, "the automation ran");
    let started = triggers::start_inbox_case(&env.state, &env.ws, &id, None).await.unwrap();
    assert!(matches!(started, Started::Case(..)), "a case starts like for any mail: {started:?}");
}

#[tokio::test]
async fn mail_carrying_our_header_signal_is_left_alone() {
    let env = env().await;
    let id = receive_signalled(&env, "own2", "someone@forwarding.test", &["postroom", "auto_submitted"]).await;
    nothing_was_done_for(&env, &id).await;
}

#[tokio::test]
async fn our_header_never_relabels_system_mail_as_a_newsletter() {
    let env = env().await;
    let id = receive_signalled(&env, "own3", "no-reply@news.example", &["postroom", "auto_submitted"]).await;
    assert!(email_ai_api::mail::bulk::should_file(&env.state.db, &env.ws, &id).await.unwrap().is_none());
}

#[tokio::test]
async fn a_managers_reply_to_a_post_room_mail_is_ordinary_mail() {
    let env = env().await;
    let id = env.receive(&env.shared_src, "mgr1", "t-mgr1", "manager@client.test", &format!("Re: {SUBJECT}"), "Please send me the numbers.", "general").await;
    assert!(!email_ai_api::mail::system_mail::is_own_message(&env.state.db, &env.ws, &id).await.unwrap());
    run_automations_again(&env, &id).await;
    let runs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM automation_runs r JOIN automations a ON a.id = r.automation_id WHERE r.message_id = ?1 AND a.name = 'Draft every email'",
    )
    .bind(&id)
    .fetch_one(&env.state.db)
    .await
    .unwrap();
    assert_eq!(runs, 1, "the automation ran for a real sender");
    let started = triggers::start_inbox_case(&env.state, &env.ws, &id, None).await.unwrap();
    assert!(matches!(started, Started::Case(..)), "a case starts for a real sender: {started:?}");
}

#[tokio::test]
async fn a_person_can_still_ask_for_a_draft_on_it() {
    let env = env().await;
    let id = receive_signalled(&env, "own4", FROM, &["postroom"]).await;
    let mut conn = env.state.db.acquire().await.unwrap();
    jobs::enqueue(&mut conn, kind::GENERATE_DRAFT, &format!("draft:{id}:manual:req1"), Some(&env.ws), &json!({ "message_id": id })).await.unwrap();
    drop(conn);
    worker::drain(&env.state).await;
    let drafts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM drafts WHERE message_id = ?1").bind(&id).fetch_one(&env.state.db).await.unwrap();
    assert_eq!(drafts, 1, "a manual request is always allowed");
}

#[tokio::test]
async fn nobody_is_told_that_our_own_mail_is_unanswered() {
    let env = env().await;
    env.ok(
        "PUT",
        "/alert-settings",
        ADMIN,
        Some(json!({"enabled": true, "recipients": ["admin@acme.test"], "escalate_to": [], "unanswered_hours": 4, "escalate_after_hours": 24, "daily_digest": false, "digest_hour": 9})),
    )
    .await;
    let own = receive_signalled(&env, "own5", FROM, &["postroom"]).await;
    let real = env.receive(&env.shared_src, "mgr2", "t-mgr2", "manager@client.test", "Where is the report?", "Need it today.", "general").await;
    sqlx::query("UPDATE classifications SET priority = 'high'").execute(&env.state.db).await.unwrap();
    sqlx::query("UPDATE email_messages SET received_at = ?1").bind(db::now_ms() - 10 * 3_600_000).execute(&env.state.db).await.unwrap();
    email_ai_api::alerts::evaluate_workspace(&env.state, &env.ws, db::now_ms()).await.unwrap();
    let alerted: Vec<String> =
        sqlx::query_scalar("SELECT message_id FROM alerts WHERE workspace_id = ?1 AND kind = 'important_unanswered'").bind(&env.ws).fetch_all(&env.state.db).await.unwrap();
    assert_eq!(alerted, [real], "only the real sender's mail is reported");
    assert!(!alerted.contains(&own));
}
