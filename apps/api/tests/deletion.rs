//! Deleting mail from Post Room: one email, a selection, a thread, and a whole mailbox on disconnect.
//! What is derived from an email goes with it, a deleted email is not imported again, and the
//! permission rules (personal mailboxes, the access wall) hold.

use crate::common;
use crate::staffing_common;

use axum::http::StatusCode;
use common::*;
use email_ai_api::{
    audit::Actor,
    db,
    deletion::{self, Opts as DelOpts},
    domain::NormalizedEmail,
    ingest,
    jobs::{self, kind, worker},
};
use serde_json::{Value, json};

const SENTINEL: &str = "zebraword";

fn mail(gmail_id: &str, from: &str, subject: &str) -> NormalizedEmail {
    NormalizedEmail {
        source_message_id: gmail_id.into(),
        sender_address: from.into(),
        recipients: vec!["team@acme.com".into()],
        subject: subject.into(),
        body_text: format!("{SENTINEL} body"),
        received_at: db::now_ms() - 1_000,
        ..Default::default()
    }
}

/// Stores an email the way every adapter does, without running the pipeline.
async fn put(env: &Env, src: &str, gmail_id: &str, thread: &str, from: &str, subject: &str) -> String {
    put_email(env, src, mail(gmail_id, from, subject), thread).await.unwrap()
}

async fn put_email(env: &Env, src: &str, email: NormalizedEmail, thread: &str) -> Option<String> {
    let mut tx = env.state.db.begin_with("BEGIN IMMEDIATE").await.unwrap();
    let id = ingest::persist_message(&mut tx, src, &env.ws, &email, Some(thread), json!({})).await.unwrap();
    tx.commit().await.unwrap();
    id
}

async fn n(env: &Env, sql: &str, bind: &str) -> i64 {
    sqlx::query_scalar(sql).bind(bind).fetch_one(&env.state.db).await.unwrap()
}

async fn exec(env: &Env, sql: &str, binds: &[&str]) {
    let mut q = sqlx::query(sql);
    for b in binds {
        q = q.bind(*b);
    }
    q.execute(&env.state.db).await.unwrap();
}

fn blob() -> Vec<u8> {
    let mut v = vec![0f32; 1024];
    v[0] = 1.0;
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

async fn chunk(env: &Env, kind: &str, ref_id: &str, msg: &str, text: &str) -> i64 {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO knowledge_chunks (workspace_id, kind, ref_id, message_id, position, text, embedded, created_at)
         VALUES (?1, ?2, ?3, ?4, 0, ?5, 1, 1) RETURNING id",
    )
    .bind(&env.ws)
    .bind(kind)
    .bind(ref_id)
    .bind(msg)
    .bind(text)
    .fetch_one(&env.state.db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO knowledge_vec (rowid, workspace_id, kind, embedding) VALUES (?1, ?2, ?3, ?4)")
        .bind(id)
        .bind(&env.ws)
        .bind(kind)
        .bind(blob())
        .execute(&env.state.db)
        .await
        .unwrap();
    id
}

struct Seeds {
    draft: String,
    attachment: String,
}

/// Everything an email can leave behind, for `msg`.
async fn seed_derived(env: &Env, msg: &str, tag: &str) -> Seeds {
    let (draft, attachment) = (format!("drf_{tag}"), format!("att_{tag}"));
    exec(env, "INSERT INTO attachments (id, message_id, filename, media_type, size_bytes, text_content, text_status) VALUES (?1, ?2, 'cv.pdf', 'application/pdf', 10, 'zebraword attachment text', 'extracted')", &[&attachment, msg]).await;
    exec(env, "INSERT INTO drafts (id, workspace_id, message_id, body_text, status, created_at, updated_at) VALUES (?1, ?2, ?3, 'zebraword reply', 'suggested', 1, 1)", &[&draft, &env.ws, msg]).await;
    exec(env, "INSERT OR REPLACE INTO classifications (message_id, category, priority, requires_reply, confidence, needs_review, source, taxonomy_version, updated_at) VALUES (?1, 'other', 'normal', 1, 1.0, 0, 'manual', 'v1', 1)", &[msg]).await;
    exec(env, "INSERT OR REPLACE INTO message_insights (message_id, summary, asks_json, needs_reply, missing_json, source, created_at) VALUES (?1, 'zebraword summary', '[]', 1, '[]', 'llm', 1)", &[msg]).await;
    exec(env, "INSERT INTO alerts (id, workspace_id, kind, severity, title, body, message_id, dedupe_key, level_since, created_at, updated_at) VALUES (?1, ?2, 'automation', 'normal', 't', 'b', ?3, ?1, 1, 1, 1)", &[&format!("alr_{tag}"), &env.ws, msg]).await;
    exec(env, "INSERT OR REPLACE INTO message_routes (message_id, workspace_id, agent_id, agent_version, source, shadow, created_at) VALUES (?1, ?2, 'agt_x', 1, 'rule', 0, 1)", &[msg, &env.ws]).await;
    exec(env, "INSERT INTO understanding_shadow (message_id, workspace_id, v1_label, v1_requires_reply, v1_priority, created_at) VALUES (?1, ?2, 'other', 1, 'normal', 1)", &[msg, &env.ws]).await;
    chunk(env, "email", msg, msg, "zebraword email chunk").await;
    chunk(env, "attachment", &attachment, msg, "zebraword attachment chunk").await;
    chunk(env, "reply", &draft, msg, "zebraword reply chunk").await;
    exec(env, "INSERT INTO ai_calls (id, workspace_id, task, model, message_id, created_at) VALUES (?1, ?2, 'classify', 'm', ?3, 1)", &[&format!("aic_{tag}"), &env.ws, msg]).await;
    exec(env, "INSERT INTO feedback_events (id, workspace_id, kind, message_id, draft_id, created_at) VALUES (?1, ?2, 'draft_edited', ?3, ?4, 1)", &[&format!("fbk_{tag}"), &env.ws, msg, &draft]).await;
    let citation = json!([{ "message_id": msg, "subject": "Quarterly zebraword", "attachment_id": attachment, "attachment_name": "cv.pdf" }]);
    exec(env, "INSERT INTO assistant_messages (workspace_id, user_id, role, text, citations_json, created_at) VALUES (?1, ?2, 'assistant', 'It says so.', ?3, 1)", &[&env.ws, &env.admin_id, &citation.to_string()]).await;
    // Learned only from this email / also from another / typed by a person.
    let only = json!([{ "id": msg, "via": "incoming" }]).to_string();
    let both = json!([{ "id": msg, "via": "incoming" }, { "id": "msg_elsewhere", "via": "reply" }]).to_string();
    for (id, source, sources, confidence) in [("mem_only", "learned", &only, "0.5"), ("mem_both", "learned", &both, "0.9"), ("mem_manual", "user", &only, "1.0")] {
        let id = format!("{id}_{tag}");
        exec(env, "INSERT INTO memories (id, workspace_id, scope, kind, content, status, source, created_at, updated_at, origin, confidence, sources_json, evidence_message_id)
                   VALUES (?1, ?2, 'workspace', 'fact', ?3, 'active', ?4, 1, 1, 'auto', CAST(?5 AS REAL), ?6, ?7)", &[&id, &env.ws, &format!("{SENTINEL} fact {id}"), source, confidence, sources, msg]).await;
    }
    let evidence_only = json!([draft]).to_string();
    let evidence_both = json!([draft, "drf_elsewhere"]).to_string();
    for (id, evidence, count) in [("prf_only", &evidence_only, "1"), ("prf_both", &evidence_both, "2")] {
        let id = format!("{id}_{tag}");
        exec(env, "INSERT INTO agent_preferences (id, workspace_id, category, content, status, source, dedupe_key, evidence_count, evidence_json, created_at, updated_at)
                   VALUES (?1, ?2, 'tone', ?1, 'suggested', 'edit', ?1, CAST(?3 AS INTEGER), ?4, 1, 1)", &[&id, &env.ws, count, evidence]).await;
    }
    Seeds { draft, attachment }
}

const FK_TABLES: &str = "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '%\\_fts%' ESCAPE '\\' AND name NOT LIKE 'knowledge_vec%' AND name NOT LIKE 'match_profile_vec%'";

/// Every table that points at the email (by foreign key, or by a message-id column) without a row for it.
/// Audit rows keep ids only and are the one intended exception.
async fn rows_naming(env: &Env, msg: &str) -> Vec<String> {
    use sqlx::Row;
    let tables: Vec<String> = sqlx::query_scalar(FK_TABLES).fetch_all(&env.state.db).await.unwrap();
    let mut left = vec![];
    for t in tables {
        let Ok(cols) = sqlx::query(&format!("PRAGMA table_info({t})")).fetch_all(&env.state.db).await else { continue };
        for c in cols {
            let name: String = c.get("name");
            if !["message_id", "evidence_message_id", "source_message_id", "answer_message_id", "outcome_message_id"].contains(&name.as_str()) {
                continue;
            }
            if t == "email_messages" || t == "audit_events" {
                continue;
            }
            let k: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {t} WHERE {name} = ?1")).bind(msg).fetch_one(&env.state.db).await.unwrap();
            if k > 0 {
                left.push(format!("{t}.{name} x{k}"));
            }
        }
    }
    left
}

#[tokio::test]
async fn deleting_one_email_removes_everything_derived_from_it_and_nothing_else() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let doomed = put(&env, &env.shared_src, "g-doomed", "t-doomed", "only@client.com", "Doomed").await;
    let kept = put(&env, &env.shared_src, "g-kept", "t-kept", CLIENT, "Kept").await;
    let seeds = seed_derived(&env, &doomed, "a").await;
    seed_derived(&env, &kept, "b").await;
    env.set_understanding(&doomed, "other", true, json!([])).await;
    let ws = env.ws.as_str();

    assert_eq!(n(&env, "SELECT COUNT(*) FROM knowledge_vec WHERE workspace_id = ?1", ws).await, 6);
    assert_eq!(n(&env, "SELECT COUNT(*) FROM knowledge_fts WHERE knowledge_fts MATCH 'zebraword'", ws).await, 6);
    assert_eq!(n(&env, "SELECT COUNT(*) FROM contacts WHERE workspace_id = ?1 AND email = 'only@client.com'", ws).await, 1);

    let (s, body) = env.req("DELETE", &format!("/messages/{doomed}"), ADMIN, None).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["deleted"], 1);
    assert_eq!((body["removed"]["attachments"].as_i64(), body["removed"]["drafts"].as_i64(), body["removed"]["chunks"].as_i64()), (Some(1), Some(1), Some(3)));

    // Gone: the email and every table that hangs off it, including both search indexes.
    for (sql, left) in [
        ("SELECT COUNT(*) FROM email_messages WHERE id = ?1", 0),
        ("SELECT COUNT(*) FROM email_threads WHERE provider_thread_id = 't-doomed' AND ?1 != ''", 0),
        ("SELECT COUNT(*) FROM attachments WHERE message_id = ?1", 0),
        ("SELECT COUNT(*) FROM drafts WHERE message_id = ?1", 0),
        ("SELECT COUNT(*) FROM classifications WHERE message_id = ?1", 0),
        ("SELECT COUNT(*) FROM message_insights WHERE message_id = ?1", 0),
        ("SELECT COUNT(*) FROM alerts WHERE message_id = ?1", 0),
        ("SELECT COUNT(*) FROM message_routes WHERE message_id = ?1", 0),
        ("SELECT COUNT(*) FROM understanding_shadow WHERE message_id = ?1", 0),
        ("SELECT COUNT(*) FROM knowledge_chunks WHERE message_id = ?1", 0),
        ("SELECT COUNT(*) FROM feedback_events WHERE message_id = ?1", 0),
        ("SELECT COUNT(*) FROM contacts WHERE email = 'only@client.com' AND ?1 != ''", 0),
    ] {
        assert_eq!(n(&env, sql, &doomed).await, left, "{sql}");
    }
    assert_eq!(n(&env, "SELECT COUNT(*) FROM knowledge_vec WHERE workspace_id = ?1", ws).await, 3, "vectors of the deleted email are gone");
    assert_eq!(n(&env, "SELECT COUNT(*) FROM knowledge_fts WHERE knowledge_fts MATCH 'zebraword'", ws).await, 3, "full-text rows too");
    assert_eq!(rows_naming(&env, &doomed).await, Vec::<String>::new());
    assert_eq!(n(&env, "SELECT COUNT(*) FROM attachments WHERE id = ?1", &seeds.attachment).await, 0);
    assert_eq!(n(&env, "SELECT COUNT(*) FROM drafts WHERE id = ?1", &seeds.draft).await, 0);
    let (s, _) = env.req("GET", &format!("/messages/{doomed}"), ADMIN, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Learning: taught only by this email goes; also confirmed elsewhere (or typed by a person) stays, minus this source.
    assert_eq!(n(&env, "SELECT COUNT(*) FROM memories WHERE id = 'mem_only_a' AND ?1 != ''", ws).await, 0);
    assert_eq!(n(&env, "SELECT COUNT(*) FROM memories WHERE id IN ('mem_both_a', 'mem_manual_a') AND ?1 != ''", ws).await, 2);
    let both: String = sqlx::query_scalar("SELECT sources_json FROM memories WHERE id = 'mem_both_a'").fetch_one(&env.state.db).await.unwrap();
    assert_eq!(serde_json::from_str::<Value>(&both).unwrap(), json!([{ "id": "msg_elsewhere", "via": "reply" }]));
    assert_eq!(n(&env, "SELECT COUNT(*) FROM memories_fts WHERE memories_fts MATCH 'zebraword' AND ?1 != ''", ws).await, 2 + 3, "no full-text row for the deleted memory");
    assert_eq!(n(&env, "SELECT COUNT(*) FROM agent_preferences WHERE id = 'prf_only_a' AND ?1 != ''", ws).await, 0);
    let (count, evidence): (i64, String) = sqlx::query_as("SELECT evidence_count, evidence_json FROM agent_preferences WHERE id = 'prf_both_a'").fetch_one(&env.state.db).await.unwrap();
    assert_eq!((count, evidence.as_str()), (1, "[\"drf_elsewhere\"]"));

    // Spend is history: the ledger row stays, without the email id. Assistant answers keep the sentence, not the email.
    assert_eq!(n(&env, "SELECT COUNT(*) FROM ai_calls WHERE id = 'aic_a' AND message_id IS NULL AND ?1 != ''", ws).await, 1);
    let cited: String = sqlx::query_scalar("SELECT citations_json FROM assistant_messages WHERE workspace_id = ?1 ORDER BY id LIMIT 1").bind(ws).fetch_one(&env.state.db).await.unwrap();
    let cited: Value = serde_json::from_str(&cited).unwrap();
    assert_eq!((cited[0]["subject"].as_str(), cited[0]["deleted"].as_bool(), cited[0].get("attachment_id")), (Some("Deleted email"), Some(true), None));
    assert!(!cited.to_string().contains(SENTINEL));

    // Audit: ids only, and a tombstone for the provider message.
    let meta: String = sqlx::query_scalar("SELECT metadata_json FROM audit_events WHERE action = 'message.deleted' AND message_id = ?1").bind(&doomed).fetch_one(&env.state.db).await.unwrap();
    assert!(!meta.contains(SENTINEL) && !meta.contains("Doomed") && !meta.contains("client.com"), "{meta}");
    assert_eq!(n(&env, "SELECT COUNT(*) FROM deleted_messages WHERE provider_message_id = 'g-doomed' AND ?1 != ''", ws).await, 1);

    // The other email is untouched.
    for (sql, expected) in [
        ("SELECT COUNT(*) FROM email_messages WHERE id = ?1", 1),
        ("SELECT COUNT(*) FROM attachments WHERE message_id = ?1", 1),
        ("SELECT COUNT(*) FROM drafts WHERE message_id = ?1", 1),
        ("SELECT COUNT(*) FROM knowledge_chunks WHERE message_id = ?1", 3),
        ("SELECT COUNT(*) FROM message_insights WHERE message_id = ?1", 1),
        ("SELECT COUNT(*) FROM memories WHERE evidence_message_id = ?1", 3),
    ] {
        assert_eq!(n(&env, sql, &kept).await, expected, "{sql}");
    }
}

#[tokio::test]
async fn a_deleted_email_is_not_imported_again_by_the_next_sync() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let id = put(&env, &env.shared_src, "g-once", "t-once", CLIENT, "Once").await;
    let (s, _) = env.req("DELETE", &format!("/messages/{id}"), ADMIN, None).await;
    assert_eq!(s, StatusCode::OK);

    assert!(put_email(&env, &env.shared_src, mail("g-once", CLIENT, "Once"), "t-once").await.is_none(), "same provider id, same mailbox: skipped");
    assert_eq!(n(&env, "SELECT COUNT(*) FROM email_messages WHERE source_message_id = 'g-once' AND ?1 != ''", &env.ws).await, 0);
    assert!(put_email(&env, &env.op_src, mail("g-once", CLIENT, "Once"), "t-once").await.is_some(), "the tombstone is per mailbox");
    assert!(put_email(&env, &env.shared_src, mail("g-new", CLIENT, "New"), "t-once").await.is_some(), "other mail in the same thread still arrives");
}

#[tokio::test]
async fn deleting_a_thread_or_a_selection_removes_every_email_of_it() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let a1 = put(&env, &env.shared_src, "g1", "t-a", CLIENT, "Thread A first").await;
    let a2 = put(&env, &env.shared_src, "g2", "t-a", "ravi@client.com", "Thread A second").await;
    let b1 = put(&env, &env.shared_src, "g3", "t-b", CLIENT, "Thread B").await;
    let c1 = put(&env, &env.shared_src, "g4", "t-c", CLIENT, "Selected one").await;
    let c2 = put(&env, &env.shared_src, "g5", "t-d", CLIENT, "Selected two").await;
    seed_derived(&env, &a2, "t").await;

    let (s, body) = env.req("DELETE", &format!("/messages/{a1}/thread"), ADMIN, None).await;
    assert_eq!((s, body["deleted"].as_i64()), (StatusCode::OK, Some(2)), "{body}");
    assert_eq!(n(&env, "SELECT COUNT(*) FROM email_messages WHERE id IN (?1) AND ?1 != ''", &a2).await, 0);
    assert_eq!(n(&env, "SELECT COUNT(*) FROM email_threads WHERE provider_thread_id = 't-a' AND ?1 != ''", &env.ws).await, 0, "the empty thread goes too");
    assert_eq!(n(&env, "SELECT COUNT(*) FROM deleted_messages WHERE provider_message_id IN ('g1', 'g2') AND ?1 != ''", &env.ws).await, 2);
    assert_eq!(n(&env, "SELECT COUNT(*) FROM email_messages WHERE id = ?1", &b1).await, 1);

    let (s, body) = env.req("POST", "/messages/delete", ADMIN, Some(json!({ "ids": [c1, c2, c1] }))).await;
    assert_eq!((s, body["deleted"].as_i64()), (StatusCode::OK, Some(2)), "{body}");
    assert_eq!(n(&env, "SELECT COUNT(*) FROM email_messages WHERE workspace_id = ?1", &env.ws).await, 1);
    let (s, _) = env.req("POST", "/messages/delete", ADMIN, Some(json!({ "ids": [] }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn deleting_releases_held_sends_and_cancels_the_open_case_but_keeps_the_case_row() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let msg = put(&env, &env.shared_src, "g-case", "t-case", CLIENT, "Case mail").await;
    let ws = env.ws.clone();
    let lead: String = sqlx::query_scalar("SELECT id FROM agents WHERE workspace_id = ?1 AND kind = 'lead'").bind(&ws).fetch_one(&env.state.db).await.unwrap();
    exec(&env, "INSERT INTO drafts (id, workspace_id, message_id, body_text, status, agent_id, send_at, created_at, updated_at) VALUES ('drf_held', ?1, ?2, 'Hello', 'approved', ?3, ?4, 1, 1)", &[&ws, &msg, &lead, &(db::now_ms() - 1_000).to_string()]).await;
    {
        let mut conn = env.state.db.acquire().await.unwrap();
        jobs::enqueue_at(&mut conn, kind::SEND_DRAFT, "send_draft:drf_held:1", Some(&ws), &json!({ "workspace_id": ws, "draft_id": "drf_held", "send_at": 1 }), 5, 0).await.unwrap();
    }
    exec(&env, "INSERT INTO cases (id, workspace_id, kind, title, goal, status, max_steps, budget_usd, message_id, state_json, agent_id, created_at, updated_at)
                VALUES ('cas_open', ?1, 'inbox', 'Reply to Ravi', 'goal', 'waiting_approval', 10, 1.0, ?2, '{\"read\":\"zebraword\"}', ?3, 1, 1)", &[&ws, &msg, &lead]).await;
    exec(&env, "INSERT INTO cases (id, workspace_id, kind, title, goal, status, max_steps, budget_usd, message_id, created_at, updated_at) VALUES ('cas_done', ?1, 'requirement', 'Old', 'goal', 'done', 10, 1.0, ?2, 1, 1)", &[&ws, &msg]).await;
    exec(&env, "INSERT INTO case_steps (id, case_id, workspace_id, seq, turn, kind, summary, input_json, output_json, status, created_at) VALUES ('stp_1', 'cas_open', ?1, 1, 1, 'tool', 'Read the email', '{\"q\":\"zebraword\"}', '{\"a\":\"zebraword\"}', 'ok', 1)", &[&ws]).await;
    exec(&env, "INSERT INTO agent_approvals (id, workspace_id, case_id, action, summary, body, status, created_at) VALUES ('apr_open', ?1, 'cas_open', 'send_email', 'Send', 'zebraword body', 'pending', 1)", &[&ws]).await;

    let (s, body) = env.req("DELETE", &format!("/messages/{msg}"), ADMIN, None).await;
    assert_eq!((s, body["removed"]["cases_cancelled"].as_i64()), (StatusCode::OK, Some(1)), "{body}");
    worker::drain(&env.state).await;
    assert!(env.sends().is_empty(), "the held reply never went out");
    assert_eq!(n(&env, "SELECT COUNT(*) FROM drafts WHERE id = 'drf_held' AND ?1 != ''", &ws).await, 0);
    assert_eq!(n(&env, "SELECT COUNT(*) FROM jobs WHERE kind = 'send_draft' AND status = 'pending' AND ?1 != ''", &ws).await, 0);

    let (status, note, message_id, state): (String, String, Option<String>, String) =
        sqlx::query_as("SELECT status, status_note, message_id, state_json FROM cases WHERE id = 'cas_open'").fetch_one(&env.state.db).await.unwrap();
    assert_eq!((status.as_str(), message_id, state.as_str()), ("cancelled", None, "{}"));
    assert!(note.contains("deleted"), "{note}");
    let done: (String, Option<String>) = sqlx::query_as("SELECT status, message_id FROM cases WHERE id = 'cas_done'").fetch_one(&env.state.db).await.unwrap();
    assert_eq!(done, ("done".into(), None));
    let (approval, body_left): (String, String) = sqlx::query_as("SELECT status, body FROM agent_approvals WHERE id = 'apr_open'").fetch_one(&env.state.db).await.unwrap();
    assert_eq!((approval.as_str(), body_left.as_str()), ("declined", ""));
    let (input, output): (String, String) = sqlx::query_as("SELECT input_json, output_json FROM case_steps WHERE id = 'stp_1'").fetch_one(&env.state.db).await.unwrap();
    assert_eq!((input.as_str(), output.as_str()), ("null", "null"));
}

#[tokio::test]
async fn a_reply_that_is_being_sent_right_now_blocks_the_delete() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let msg = put(&env, &env.shared_src, "g-busy", "t-busy", CLIENT, "Busy").await;
    exec(&env, "INSERT INTO drafts (id, workspace_id, message_id, body_text, status, send_attempt_id, send_started_at, created_at, updated_at) VALUES ('drf_busy', ?1, ?2, 'Hi', 'approved', 'att', 1, 1, 1)", &[&env.ws, &msg]).await;
    let (s, _) = env.req("DELETE", &format!("/messages/{msg}"), ADMIN, None).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(n(&env, "SELECT COUNT(*) FROM email_messages WHERE id = ?1", &msg).await, 1);
    assert_eq!(n(&env, "SELECT COUNT(*) FROM deleted_messages WHERE ?1 != ''", &msg).await, 0, "a failed delete leaves no tombstone");
}

#[tokio::test]
async fn permissions_follow_the_mailbox_owner_the_role_and_the_workspace() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let caps = "{\"read\":true,\"sync\":true,\"provider_labels\":false,\"provider_drafts\":true,\"send\":true}";
    exec(&env, "INSERT INTO email_sources (id, workspace_id, kind, status, external_connection_id, provider_config_key, capabilities_json, created_at, mailbox_address, connected_by_user_id)
                VALUES ('src_boss', ?1, 'nango_gmail', 'connected', 'conn_boss', 'google-mail', ?2, 1, 'boss@acme.com', ?3)", &[&env.ws, caps, &env.admin_id]).await;
    let boss = put(&env, "src_boss", "g-boss", "t1", CLIENT, "Boss private").await;
    let shared = put(&env, &env.shared_src, "g-shared", "t2", CLIENT, "Shared").await;
    let mine = put(&env, &env.op_src, "g-mine", "t3", CLIENT, "Priya private").await;
    let del = |id: &str| format!("/messages/{id}");

    assert_eq!(env.req("DELETE", &del(&boss), OPERATOR, None).await.0, StatusCode::NOT_FOUND, "another member's personal mailbox is a 404");
    assert_eq!(env.req("POST", "/messages/delete", OPERATOR, Some(json!({ "ids": [shared, boss] }))).await.0, StatusCode::NOT_FOUND);
    assert_eq!(n(&env, "SELECT COUNT(*) FROM email_messages WHERE workspace_id = ?1", &env.ws).await, 3, "a refused selection deletes nothing");
    assert_eq!(env.req("DELETE", &format!("/messages/{boss}/thread"), OPERATOR, None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(env.req("DELETE", &del("msg_nope"), OPERATOR, None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(env.req("DELETE", &del(&shared), OUTSIDER, None).await.0, StatusCode::FORBIDDEN);

    assert_eq!(env.req("DELETE", &del(&shared), OPERATOR, None).await.0, StatusCode::OK, "an operator may delete from a shared mailbox");
    assert_eq!(env.req("DELETE", &del(&mine), OPERATOR, None).await.0, StatusCode::OK, "and from their own");
    assert_eq!(env.req("DELETE", &del(&boss), ADMIN, None).await.0, StatusCode::OK);

    // Disconnecting follows the same ownership.
    let preview = |src: &str| format!("/sources/{src}/purge-preview");
    assert_eq!(env.req("GET", &preview("src_boss"), OPERATOR, None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(env.req("POST", "/sources/src_boss/disconnect", OPERATOR, Some(json!({ "purge": true }))).await.0, StatusCode::NOT_FOUND);
    assert_eq!(env.req("POST", "/sources/src_shared/disconnect", OPERATOR, Some(json!({ "purge": true }))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(env.req("GET", &preview("src_shared"), OPERATOR, None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(n(&env, "SELECT COUNT(*) FROM email_sources WHERE status = 'connected' AND workspace_id = ?1", &env.ws).await, 3);
    assert_eq!(env.req("GET", &preview("src_priya"), OPERATOR, None).await.0, StatusCode::OK);
    let (s, body) = env.req("POST", "/sources/src_priya/disconnect", OPERATOR, Some(json!({ "purge": true }))).await;
    assert_eq!((s, body["status"].as_str(), body["purge"]["status"].as_str()), (StatusCode::OK, Some("disconnected"), Some("running")), "{body}");
}

#[tokio::test]
async fn the_access_wall_applies_to_deleting_too() {
    use staffing_common::*;
    let h = harness().await;
    let ws = h.workspace(&["staffing"]).await;
    let priya = h.member(&ws, "priya@acme.test", "operator").await;
    let (pb, ob) = (h.mailbox(&ws, "priya@acme.test").await, h.mailbox(&ws, "other@acme.test").await);
    h.person(&ws, json!({"name": "Priya", "emails": ["priya@acme.test"], "source_id": pb})).await;
    h.person(&ws, json!({"name": "Other", "emails": ["other@acme.test"], "source_id": ob})).await;
    let settings = json!({"tz_offset_minutes": 0, "manager_after_hours": null, "final_after_hours": null, "final_emails": [], "below_target_hour": 16, "weekly_check_hour": 12, "restrict_mail_to_team": true});
    assert_eq!(h.call("PUT", &format!("{}/team/settings", api(&ws)), OWNER, Some(settings)).await.0, StatusCode::OK);
    let mine = h.ingest(&ws, &pb, email("c@x.test", &["priya@acme.test"], "Mine", "x", WED), "w1").await.unwrap();
    let theirs = h.ingest(&ws, &ob, email("c@x.test", &["other@acme.test"], "Theirs", "x", WED + 1), "w2").await.unwrap();
    let url = |id: &str| format!("{}/messages/{id}", api(&ws));

    assert_eq!(h.call("DELETE", &url(&theirs), &priya, None).await.0, StatusCode::NOT_FOUND, "outside the wall");
    assert_eq!(h.call("DELETE", &format!("{}/thread", url(&theirs)), &priya, None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(h.call("DELETE", &url(&mine), &priya, None).await.0, StatusCode::OK);
    assert_eq!(h.call("DELETE", &url(&theirs), OWNER, None).await.0, StatusCode::OK, "the owner has no wall");
}

async fn mailbox_with_mail(env: &Env, src: &str, count: usize) -> Vec<String> {
    let mut ids = vec![];
    for i in 0..count {
        let sender = if i % 2 == 0 { "solo@client.com".to_string() } else { format!("shared{}@client.com", i % 3) };
        ids.push(put(env, src, &format!("{src}-g{i}"), &format!("{src}-t{}", i / 2), &sender, &format!("Mail {i}")).await);
    }
    ids
}

#[tokio::test]
async fn disconnect_preview_counts_are_exact() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let ids = mailbox_with_mail(&env, &env.op_src, 12).await;
    mailbox_with_mail(&env, &env.shared_src, 4).await;
    for (i, id) in ids.iter().enumerate().take(5) {
        exec(&env, "INSERT INTO attachments (id, message_id, filename) VALUES (?1, ?2, 'a.pdf')", &[&format!("att_p{i}"), id]).await;
        if i < 3 {
            exec(&env, "INSERT INTO attachments (id, message_id, filename) VALUES (?1, ?2, 'b.pdf')", &[&format!("att_q{i}"), id]).await;
        }
    }
    for (i, id) in ids.iter().enumerate().skip(2).take(4) {
        exec(&env, "INSERT INTO drafts (id, workspace_id, message_id, body_text, status, created_at, updated_at) VALUES (?1, ?2, ?3, 'x', 'suggested', 1, 1)", &[&format!("drf_p{i}"), &env.ws, id]).await;
    }
    for id in ids.iter().take(7) {
        exec(&env, "INSERT INTO message_insights (message_id, summary, needs_reply, source, created_at) VALUES (?1, 's', 0, 'llm', 1)", &[id]).await;
    }
    let (s, body) = env.req("GET", "/sources/src_priya/purge-preview", OPERATOR, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body, json!({ "emails": 12, "attachments": 8, "drafts": 4, "summaries": 7, "scheduled_replies": 0, "open_tickets": 0 }));
}

#[tokio::test]
async fn purging_a_mailbox_deletes_only_its_data_resumes_after_a_cut_off_and_is_idempotent() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let ws = env.ws.clone();
    let mine = mailbox_with_mail(&env, &env.op_src, 60).await;
    let shared = mailbox_with_mail(&env, &env.shared_src, 6).await;
    for (i, id) in mine.iter().enumerate().take(9) {
        seed_derived(&env, id, &format!("m{i}")).await;
    }
    let seeds_shared = seed_derived(&env, &shared[0], "s0").await;
    exec(&env, "INSERT INTO memories (id, workspace_id, scope, kind, content, status, source, created_at, updated_at, origin, confidence, sources_json, evidence_message_id) VALUES ('mem_shared_keep', ?1, 'workspace', 'fact', 'kept fact', 'active', 'learned', 1, 1, 'auto', 0.5, ?2, ?3)", &[&ws, &json!([{ "id": shared[0], "via": "incoming" }]).to_string(), &shared[0]]).await;
    let (s, p) = env.req("POST", "/team/people", ADMIN, Some(json!({"name": "Priya P", "emails": ["pp@acme.test"]}))).await;
    assert_eq!(s, StatusCode::CREATED, "{p}");
    exec(&env, "UPDATE people SET source_id = 'src_priya' WHERE id = ?1", &[p["id"].as_str().unwrap()]).await;
    exec(&env, "UPDATE team_settings SET hr_source_id = 'src_priya' WHERE workspace_id = ?1", &[&ws]).await;

    let expected = env.req("GET", "/sources/src_priya/purge-preview", ADMIN, None).await.1;
    assert_eq!(expected["emails"], 60);
    let shared_before = n(&env, "SELECT COUNT(*) FROM email_messages WHERE source_id = 'src_shared' AND ?1 != ''", &ws).await;
    let shared_chunks = n(&env, "SELECT COUNT(*) FROM knowledge_chunks WHERE message_id = ?1", &shared[0]).await;

    let (s, body) = env.req("POST", "/sources/src_priya/disconnect", OPERATOR, Some(json!({ "purge": true }))).await;
    assert_eq!((s, body["purge"]["status"].as_str(), body["purge"]["total"].as_i64(), body["purge"]["done"].as_i64()), (StatusCode::OK, Some("running"), Some(60), Some(0)), "{body}");
    assert!(put_email(&env, &env.op_src, mail("late", CLIENT, "Late"), "tl").await.is_none(), "a sync that is still running cannot add mail to a mailbox being purged");

    // The first job is cut off after one batch: its work is committed, its progress never reported.
    {
        let first: Vec<String> = sqlx::query_scalar("SELECT id FROM email_messages WHERE source_id = 'src_priya' ORDER BY id LIMIT 25").fetch_all(&env.state.db).await.unwrap();
        let mut tx = env.state.db.begin_with("BEGIN IMMEDIATE").await.unwrap();
        let prep = deletion::prepare(&env.state.db, &ws, &first).await.unwrap();
        deletion::delete_batch(&mut tx, &ws, &first, DelOpts { actor: Actor::System, tombstone: false, audit: false }, &prep).await.unwrap();
        tx.commit().await.unwrap();
    }
    assert_eq!(n(&env, "SELECT COUNT(*) FROM email_messages WHERE source_id = 'src_priya' AND ?1 != ''", &ws).await, 35);
    worker::drain(&env.state).await;

    assert_eq!(n(&env, "SELECT COUNT(*) FROM email_messages WHERE source_id = 'src_priya' AND ?1 != ''", &ws).await, 0);
    assert_eq!(n(&env, "SELECT COUNT(*) FROM email_threads WHERE source_id = 'src_priya' AND ?1 != ''", &ws).await, 0);
    assert_eq!(n(&env, "SELECT COUNT(*) FROM knowledge_chunks WHERE message_id IN (SELECT id FROM email_messages WHERE source_id = 'src_priya') AND ?1 != ''", &ws).await, 0);
    assert_eq!(
        n(&env, "SELECT COUNT(*) FROM knowledge_vec WHERE workspace_id = ?1", &ws).await,
        n(&env, "SELECT COUNT(*) FROM knowledge_chunks WHERE workspace_id = ?1", &ws).await,
        "no vector outlives its chunk"
    );
    for id in &mine {
        assert!(rows_naming(&env, id).await.is_empty(), "{id}");
    }
    // The other mailbox is exactly as it was.
    assert_eq!(n(&env, "SELECT COUNT(*) FROM email_messages WHERE source_id = 'src_shared' AND ?1 != ''", &ws).await, shared_before);
    assert_eq!(n(&env, "SELECT COUNT(*) FROM knowledge_chunks WHERE message_id = ?1", &shared[0]).await, shared_chunks);
    assert_eq!(n(&env, "SELECT COUNT(*) FROM attachments WHERE id = ?1", &seeds_shared.attachment).await, 1);
    assert_eq!(n(&env, "SELECT COUNT(*) FROM drafts WHERE id = ?1", &seeds_shared.draft).await, 1);
    assert_eq!(n(&env, "SELECT COUNT(*) FROM memories WHERE id = 'mem_shared_keep' AND ?1 != ''", &ws).await, 1);
    assert_eq!(n(&env, "SELECT COUNT(*) FROM contacts WHERE email = 'solo@client.com' AND ?1 != ''", &ws).await, 1, "a sender still in the other mailbox keeps the card");
    let count: i64 = sqlx::query_scalar("SELECT message_count FROM contacts WHERE workspace_id = ?1 AND email = 'solo@client.com'").bind(&ws).fetch_one(&env.state.db).await.unwrap();
    assert_eq!(count, 3);
    assert_eq!(n(&env, "SELECT COUNT(*) FROM contacts WHERE email = 'late@x.com' AND ?1 != ''", &ws).await, 0);

    // Source-scoped rows are cleaned, the row stays for the record, and the result is audited with exact counts.
    let src: (String, String, i64, i64, Option<String>, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT status, purge_status, purge_total, purge_done, external_connection_id, sync_cursor, mailbox_address FROM email_sources WHERE id = 'src_priya'",
    )
    .fetch_one(&env.state.db)
    .await
    .unwrap();
    assert_eq!(src, ("disconnected".into(), "done".into(), 60, 60, None, None, None));
    assert_eq!(n(&env, "SELECT COUNT(*) FROM people WHERE source_id IS NOT NULL AND workspace_id = ?1", &ws).await, 0, "people are unlinked, not deleted");
    assert_eq!(n(&env, "SELECT COUNT(*) FROM people WHERE workspace_id = ?1", &ws).await, 1);
    assert_eq!(n(&env, "SELECT COUNT(*) FROM team_settings WHERE hr_source_id IS NOT NULL AND workspace_id = ?1", &ws).await, 0);
    let meta: String = sqlx::query_scalar("SELECT metadata_json FROM audit_events WHERE action = 'source.purged' AND entity_id = 'src_priya'").fetch_one(&env.state.db).await.unwrap();
    let meta: Value = serde_json::from_str(&meta).unwrap();
    assert_eq!(meta, json!({ "emails": 60, "attachments": 9, "drafts": 9, "summaries": 9 }));
    assert_eq!(n(&env, "SELECT COUNT(*) FROM audit_events WHERE action = 'message.deleted' AND ?1 != ''", &ws).await, 0, "one source.purged row, not one per email");
    let listed = env.ok("GET", "/sources", ADMIN, None).await;
    let row = listed["items"].as_array().unwrap().iter().find(|s| s["id"] == "src_priya").unwrap();
    assert_eq!(row["purge"], json!({ "status": "done", "total": 60, "done": 60, "error": null }));

    // Idempotent: running the job again, or asking again, changes nothing and writes nothing.
    deletion::purge_source_job(&env.state, &json!({ "source_id": "src_priya", "requested_by": env.op_id })).await.unwrap();
    let (s, _) = env.req("POST", "/sources/src_priya/disconnect", OPERATOR, Some(json!({ "purge": true }))).await;
    assert_eq!(s, StatusCode::OK);
    worker::drain(&env.state).await;
    assert_eq!(n(&env, "SELECT COUNT(*) FROM audit_events WHERE action = 'source.purged' AND ?1 != ''", &ws).await, 1);
    assert_eq!(n(&env, "SELECT COUNT(*) FROM email_messages WHERE source_id = 'src_shared' AND ?1 != ''", &ws).await, shared_before);
}

#[tokio::test]
async fn disconnect_without_the_purge_flag_keeps_the_mail() {
    let env = Env::new(Opts::packs(&["general"])).await;
    mailbox_with_mail(&env, &env.op_src, 3).await;
    let (s, body) = env.req("POST", "/sources/src_priya/disconnect", ADMIN, None).await;
    assert_eq!((s, body["status"].as_str(), body["purge"].is_null()), (StatusCode::OK, Some("disconnected"), true));
    worker::drain(&env.state).await;
    assert_eq!(n(&env, "SELECT COUNT(*) FROM email_messages WHERE source_id = 'src_priya' AND ?1 != ''", &env.ws).await, 3);
}

// ---------------------------------------------------------------- review fixes

async fn start_purge_via_api(env: &Env, src: &str) {
    let (s, body) = env.req("POST", &format!("/sources/{src}/disconnect"), ADMIN, Some(json!({ "purge": true }))).await;
    assert_eq!(s, StatusCode::OK, "{body}");
}

async fn mid_send(env: &Env, msg: &str, id: &str) {
    exec(env, "INSERT INTO drafts (id, workspace_id, message_id, body_text, status, send_attempt_id, send_started_at, created_at, updated_at) VALUES (?1, ?2, ?3, 'Hi', 'approved', 'att', 1, 1, 1)", &[id, &env.ws, msg]).await;
}

#[tokio::test]
async fn a_purge_skips_mid_send_emails_waits_for_them_and_gives_up_without_staying_running() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let ids = mailbox_with_mail(&env, &env.op_src, 6).await;
    mid_send(&env, &ids[2], "drf_busy1").await;
    start_purge_via_api(&env, "src_priya").await;
    worker::drain(&env.state).await;

    assert_eq!(n(&env, "SELECT COUNT(*) FROM email_messages WHERE source_id = 'src_priya' AND ?1 != ''", &env.ws).await, 1, "everything else was deleted");
    let status: String = sqlx::query_scalar("SELECT purge_status FROM email_sources WHERE id = 'src_priya'").fetch_one(&env.state.db).await.unwrap();
    assert_eq!(status, "running");
    let waiting: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE kind = 'purge_source' AND status = 'pending' AND run_at > ?1").bind(db::now_ms()).fetch_one(&env.state.db).await.unwrap();
    assert_eq!(waiting, 1, "a delayed retry is queued, not a busy loop");

    // The send ends; the retry finishes the purge.
    exec(&env, "UPDATE drafts SET send_attempt_id = NULL WHERE id = 'drf_busy1' AND ?1 != ''", &[&env.ws]).await;
    exec(&env, "UPDATE jobs SET run_at = 0 WHERE kind = 'purge_source' AND status = 'pending' AND ?1 != ''", &[&env.ws]).await;
    worker::drain(&env.state).await;
    let status: String = sqlx::query_scalar("SELECT purge_status FROM email_sources WHERE id = 'src_priya'").fetch_one(&env.state.db).await.unwrap();
    assert_eq!(status, "done");
    assert_eq!(n(&env, "SELECT COUNT(*) FROM email_messages WHERE source_id = 'src_priya' AND ?1 != ''", &env.ws).await, 0);
}

#[tokio::test]
async fn a_purge_that_cannot_finish_fails_loudly_and_unblocks_the_mailbox() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let ids = mailbox_with_mail(&env, &env.op_src, 3).await;
    mid_send(&env, &ids[0], "drf_stuck").await;
    start_purge_via_api(&env, "src_priya").await;
    exec(&env, "UPDATE email_sources SET purge_started_at = ?1 WHERE id = 'src_priya'", &[&(db::now_ms() - 31 * 60_000).to_string()]).await;
    worker::drain(&env.state).await;

    let (status, error): (String, Option<String>) = sqlx::query_as("SELECT purge_status, purge_error FROM email_sources WHERE id = 'src_priya'").fetch_one(&env.state.db).await.unwrap();
    assert_eq!(status, "failed");
    assert!(error.unwrap().contains("Disconnect again"));
    assert!(put_email(&env, &env.op_src, mail("after", CLIENT, "After"), "ta").await.is_some(), "new mail is no longer blocked");
    let listed = env.ok("GET", "/sources", ADMIN, None).await;
    let row = listed["items"].as_array().unwrap().iter().find(|s| s["id"] == "src_priya").unwrap();
    assert_eq!(row["purge"]["status"], "failed");

    // Starting again works once the send is over.
    exec(&env, "UPDATE drafts SET send_attempt_id = NULL WHERE id = 'drf_stuck' AND ?1 != ''", &[&env.ws]).await;
    start_purge_via_api(&env, "src_priya").await;
    worker::drain(&env.state).await;
    assert_eq!(n(&env, "SELECT COUNT(*) FROM email_messages WHERE source_id = 'src_priya' AND ?1 != ''", &env.ws).await, 0);
}

#[tokio::test]
async fn finishing_a_purge_waits_for_a_send_in_flight_and_closes_the_approvals_of_deleted_sends() {
    let env = Env::new(Opts::packs(&["general"])).await;
    mailbox_with_mail(&env, &env.op_src, 2).await;
    exec(&env, "INSERT INTO outbound_emails (id, workspace_id, source_id, to_address, subject, body_text, recipient_kind, authored_by, status, send_attempt_id, created_at, updated_at)
                VALUES ('out_1', ?1, 'src_priya', 'x@y.test', 'S', 'secret body', 'external', 'agent', 'scheduled', 'att', 1, 1)", &[&env.ws]).await;
    exec(&env, "INSERT INTO cases (id, workspace_id, kind, title, goal, status, max_steps, budget_usd, created_at, updated_at) VALUES ('cas_o', ?1, 'inbox', 't', 'g', 'waiting_approval', 5, 1.0, 1, 1)", &[&env.ws]).await;
    exec(&env, "INSERT INTO agent_approvals (id, workspace_id, case_id, action, summary, body, input_json, status, created_at) VALUES ('apr_o', ?1, 'cas_o', 'send_message', 's', 'secret body', '{\"outbound_id\":\"out_1\"}', 'pending', 1)", &[&env.ws]).await;
    start_purge_via_api(&env, "src_priya").await;
    worker::drain(&env.state).await;
    assert_eq!(n(&env, "SELECT COUNT(*) FROM outbound_emails WHERE id = 'out_1' AND ?1 != ''", &env.ws).await, 1, "a send in flight is not deleted under it");
    assert_eq!(n(&env, "SELECT COUNT(*) FROM email_sources WHERE purge_status = 'running' AND ?1 != ''", &env.ws).await, 1);

    exec(&env, "UPDATE outbound_emails SET send_attempt_id = NULL WHERE id = 'out_1' AND ?1 != ''", &[&env.ws]).await;
    exec(&env, "UPDATE jobs SET run_at = 0 WHERE kind = 'purge_source' AND status = 'pending' AND ?1 != ''", &[&env.ws]).await;
    worker::drain(&env.state).await;
    assert_eq!(n(&env, "SELECT COUNT(*) FROM outbound_emails WHERE ?1 != ''", &env.ws).await, 0);
    let (status, body, input): (String, String, String) = sqlx::query_as("SELECT status, body, input_json FROM agent_approvals WHERE id = 'apr_o'").fetch_one(&env.state.db).await.unwrap();
    assert_eq!((status.as_str(), body.as_str(), input.as_str()), ("declined", "", "{}"));
}

#[tokio::test]
async fn a_purged_forwarding_address_can_be_set_up_again_with_a_new_alias() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let first = env.ok("POST", "/sources/forwarding", ADMIN, None).await;
    let (id, alias) = (first["id"].as_str().unwrap().to_string(), first["inbound_address"].as_str().unwrap().to_string());
    put(&env, &id, "g-fwd", "t-fwd", CLIENT, "Forwarded").await;
    start_purge_via_api(&env, &id).await;
    worker::drain(&env.state).await;
    let again = env.ok("POST", "/sources/forwarding", ADMIN, None).await;
    assert_eq!((again["id"].as_str(), again["status"].as_str(), again["purge"].is_null()), (Some(id.as_str()), Some("pending"), true));
    let new_alias = again["inbound_address"].as_str().unwrap();
    assert!(!new_alias.is_empty() && new_alias != alias);
}

#[tokio::test]
async fn a_two_thousand_email_purge_never_holds_the_write_lock_long() {
    let env = Env::new(Opts::packs(&["general"])).await;
    {
        let mut tx = env.state.db.begin_with("BEGIN IMMEDIATE").await.unwrap();
        for i in 0..2000 {
            let e = mail(&format!("bulk-{i}"), &format!("s{}@client.com", i % 40), &format!("Bulk {i}"));
            ingest::persist_message(&mut tx, &env.op_src, &env.ws, &e, Some(&format!("bt{}", i / 3)), json!({})).await.unwrap();
        }
        tx.commit().await.unwrap();
    }
    let some: Vec<String> = sqlx::query_scalar("SELECT id FROM email_messages WHERE source_id = 'src_priya' ORDER BY id LIMIT 60").fetch_all(&env.state.db).await.unwrap();
    for (i, id) in some.iter().enumerate() {
        seed_derived(&env, id, &format!("b{i}")).await;
    }
    start_purge_via_api(&env, "src_priya").await;
    let run = deletion::purge_run(&env.state, &env.ws, "src_priya", None, std::time::Duration::from_secs(300), std::time::Duration::ZERO).await.unwrap();
    assert_eq!((run.outcome, run.batches), (deletion::PurgeOutcome::Done, 80));
    // The typical batch is well under 200 ms; the worst one only gets slack for a busy test machine.
    let mut locks = run.locks.clone();
    locks.sort();
    assert!(locks[locks.len() * 9 / 10] < std::time::Duration::from_millis(200), "p90 lock hold {:?}", locks[locks.len() * 9 / 10]);
    assert!(run.max_lock < std::time::Duration::from_millis(1000), "a batch held the write lock for {:?}", run.max_lock);
    assert_eq!(n(&env, "SELECT COUNT(*) FROM email_messages WHERE source_id = 'src_priya' AND ?1 != ''", &env.ws).await, 0);
}

#[tokio::test]
async fn a_bulk_delete_with_one_busy_email_deletes_nothing() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let ids = mailbox_with_mail(&env, &env.shared_src, 30).await;
    mid_send(&env, &ids[29], "drf_last").await;
    let (s, _) = env.req("POST", "/messages/delete", ADMIN, Some(json!({ "ids": ids }))).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(n(&env, "SELECT COUNT(*) FROM email_messages WHERE workspace_id = ?1", &env.ws).await, 30, "the first batch was not deleted either");
}

#[tokio::test]
async fn accepted_preferences_keep_working_when_their_evidence_is_deleted() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let msg = put(&env, &env.shared_src, "g-pref", "t-pref", CLIENT, "Pref").await;
    let seeds = seed_derived(&env, &msg, "p").await;
    let evidence = json!([seeds.draft]).to_string();
    exec(&env, "INSERT INTO agent_preferences (id, workspace_id, category, content, status, source, dedupe_key, evidence_count, evidence_json, created_at, updated_at)
                VALUES ('prf_active', ?1, 'tone', 'accepted', 'active', 'edit', 'k', 1, ?2, 1, 1)", &[&env.ws, &evidence]).await;
    assert_eq!(env.req("DELETE", &format!("/messages/{msg}"), ADMIN, None).await.0, StatusCode::OK);
    let (status, ev): (String, String) = sqlx::query_as("SELECT status, evidence_json FROM agent_preferences WHERE id = 'prf_active'").fetch_one(&env.state.db).await.unwrap();
    assert_eq!((status.as_str(), ev.as_str()), ("active", "[]"));
    assert_eq!(n(&env, "SELECT COUNT(*) FROM agent_preferences WHERE id = 'prf_only_p' AND ?1 != ''", &env.ws).await, 0, "a never-approved suggestion goes");
}

#[tokio::test]
async fn the_confirms_warn_about_scheduled_replies_and_open_tickets() {
    let env = Env::new(Opts::packs(&["general"])).await;
    let a = put(&env, &env.op_src, "g-s1", "t-s1", CLIENT, "Scheduled").await;
    let b = put(&env, &env.op_src, "g-s2", "t-s1", CLIENT, "Same thread").await;
    exec(&env, "INSERT INTO drafts (id, workspace_id, message_id, body_text, status, send_at, send_requested_by, created_at, updated_at) VALUES ('drf_sched', ?1, ?2, 'Hi', 'approved', 99999999999999, ?3, 1, 1)", &[&env.ws, &b, &env.op_id]).await;
    exec(&env, "INSERT INTO watches (id, workspace_id, user_id, kind, title, channel, counterpart, status, source_id, last_update_at, created_at) VALUES ('wch_t', ?1, ?2, 'ticket', 't', 'email', 'x', 'waiting', 'src_priya', 1, 1)", &[&env.ws, &env.op_id]).await;
    exec(&env, "INSERT INTO support_tickets (id, workspace_id, user_id, watch_id, company, support_address, address_source, subject, status, last_update_at, created_at) VALUES ('tkt_t', ?1, ?2, 'wch_t', 'C', 'c@c.test', 'x', 's', 'open', 1, 1)", &[&env.ws, &env.op_id]).await;

    let (s, one) = env.req("POST", "/messages/delete-preview", OPERATOR, Some(json!({ "ids": [a] }))).await;
    assert_eq!((s, one["scheduled_replies"].as_i64()), (StatusCode::OK, Some(0)));
    let (_, thread) = env.req("POST", "/messages/delete-preview", OPERATOR, Some(json!({ "ids": [a], "whole_thread": true }))).await;
    assert_eq!((thread["emails"].as_i64(), thread["scheduled_replies"].as_i64()), (Some(2), Some(1)));
    let (_, src) = env.req("GET", "/sources/src_priya/purge-preview", OPERATOR, None).await;
    assert_eq!((src["scheduled_replies"].as_i64(), src["open_tickets"].as_i64()), (Some(1), Some(1)));
    assert_eq!(env.req("POST", "/messages/delete-preview", OUTSIDER, Some(json!({ "ids": [a] }))).await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn reconnecting_a_mailbox_that_is_still_being_purged_is_refused() {
    let env = Env::new(Opts::packs(&["general"])).await;
    mailbox_with_mail(&env, &env.op_src, 2).await;
    start_purge_via_api(&env, "src_priya").await;
    let integration = env.state.config.nango_gmail_integration.clone();
    let (s, body) = env.req("POST", "/sources/nango-connections", ADMIN, Some(json!({ "connection_id": "conn_src_priya", "provider_config_key": integration }))).await;
    assert_eq!(s, StatusCode::CONFLICT, "{body}");
    assert!(body["error"]["message"].as_str().unwrap().contains("Deleting the old data"));
}

#[tokio::test]
async fn old_tombstones_are_pruned_and_recent_ones_kept() {
    let env = Env::new(Opts::packs(&["general"])).await;
    for (id, age_days) in [("old", 200), ("new", 10)] {
        exec(&env, "INSERT INTO deleted_messages (workspace_id, source_id, provider_message_id, deleted_at) VALUES (?1, 'src_shared', ?2, ?3)", &[&env.ws, id, &(db::now_ms() - age_days * 86_400_000).to_string()]).await;
    }
    assert_eq!(deletion::prune_tombstones(&env.state.db).await.unwrap(), 1);
    assert_eq!(n(&env, "SELECT COUNT(*) FROM deleted_messages WHERE provider_message_id = 'new' AND ?1 != ''", &env.ws).await, 1);
}
