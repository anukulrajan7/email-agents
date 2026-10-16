//! Slice S9.1: the headers that tie one mail to another are kept with the message, and a mail we send remembers the Message-ID
//! it went out with, so a reply to it can be recognised. No flag: this only stores what was always in the mail.

use email_ai_api::{
    jobs::worker,
    mail::ingest,
    platform::{audit::Actor, db, domain::NormalizedEmail},
    replies::{
        self,
        live_draft::{self, Mode, NewDraft},
    },
};
use serde_json::json;

use crate::common::*;

async fn store(env: &Env, email: NormalizedEmail) -> String {
    let mut tx = env.state.db.begin().await.unwrap();
    let id = ingest::persist_message(&mut tx, &env.shared_src, &env.ws, &email, Some("th-1"), json!({})).await.unwrap().unwrap();
    tx.commit().await.unwrap();
    id
}

fn mail(gmail_id: &str, subject: &str) -> NormalizedEmail {
    NormalizedEmail {
        source_message_id: gmail_id.into(),
        rfc_message_id: Some(format!("<{gmail_id}@mail.example>")),
        sender_address: "ravi@client.test".into(),
        recipients: vec!["team@acme.com".into()],
        subject: subject.into(),
        body_text: "my id is EMP-1088".into(),
        received_at: db::now_ms() - 3_600_000,
        ..Default::default()
    }
}

type Row = (Option<String>, String, String, Option<String>);

async fn stored(env: &Env, id: &str) -> Row {
    sqlx::query_as("SELECT in_reply_to, references_json, auth_json, auto_kind FROM email_messages WHERE id = ?1").bind(id).fetch_one(&env.state.db).await.unwrap()
}

#[tokio::test]
async fn a_replys_threading_headers_and_the_servers_verdict_on_the_sender_are_stored() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let reply = NormalizedEmail {
        in_reply_to: Some("<snd_1@postroom.mail>".into()),
        references: vec!["<gm-0@mail.example>".into(), "<snd_1@postroom.mail>".into()],
        auth: Some(json!({"dmarc": "pass", "spf": "fail"})),
        ..mail("gm-1", "Re: September timesheet")
    };
    let id = store(&env, reply).await;
    assert_eq!(
        stored(&env, &id).await,
        (Some("<snd_1@postroom.mail>".into()), json!(["<gm-0@mail.example>", "<snd_1@postroom.mail>"]).to_string(), json!({"dmarc": "pass", "spf": "fail"}).to_string(), None)
    );
    let plain = store(&env, mail("gm-2", "September timesheet")).await;
    assert_eq!(stored(&env, &plain).await, (None, "[]".into(), "{}".into(), None), "a mail with no such headers stores nothing in their place");
}

#[tokio::test]
async fn machine_answers_are_marked_so_nobody_replies_to_them() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let ooo = store(&env, mail("gm-1", "Automatic reply: on leave till 20 Oct")).await;
    assert_eq!(stored(&env, &ooo).await.3.as_deref(), Some("ooo"));
    let dsn = store(&env, NormalizedEmail { sender_address: "mailer-daemon@client.test".into(), ..mail("gm-2", "Delivery Status Notification (Failure)") }).await;
    assert_eq!(stored(&env, &dsn).await.3.as_deref(), Some("dsn"));
    let person = store(&env, mail("gm-3", "Re: timesheet")).await;
    assert_eq!(stored(&env, &person).await.3, None);
}

#[tokio::test]
async fn a_sent_reply_remembers_the_message_id_it_went_out_with() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let id = store(&env, mail("gm-1", "September timesheet")).await;
    let mut tx = env.state.db.begin_with("BEGIN IMMEDIATE").await.unwrap();
    let written = live_draft::write(
        &mut tx,
        Mode::KeepExisting,
        &NewDraft { workspace_id: &env.ws, message_id: &id, origin_key: "reply:test", body_text: SAFE_REPLY, model_id: "test", memory_ids_json: "[]".into(), unanswered_json: "[]".into(), audit: json!({}) },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let draft = written.changed().unwrap().to_string();
    let before: Option<String> = sqlx::query_scalar("SELECT rfc_message_id FROM drafts WHERE id = ?1").bind(&draft).fetch_one(&env.state.db).await.unwrap();
    assert_eq!(before, None, "nothing is claimed before it is sent");
    replies::send_draft(&env.state, &env.ws, &draft, Actor::System).await.unwrap();
    worker::drain(&env.state).await;
    assert_eq!(env.sends().len(), 1);
    let (status, rfc, attempt): (String, Option<String>, Option<String>) =
        sqlx::query_as("SELECT status, rfc_message_id, send_attempt_id FROM drafts WHERE id = ?1").bind(&draft).fetch_one(&env.state.db).await.unwrap();
    assert_eq!(status, "sent");
    assert_eq!(rfc, Some(replies::attempt_message_id(&attempt.unwrap())), "the id the mail carries");
}
