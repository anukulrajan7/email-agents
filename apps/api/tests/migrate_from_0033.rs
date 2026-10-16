//! A database at migration 0033 (before agents, learning, metrics, designer ...) upgrades cleanly, keeps its
//! rows, and the startup backfill gives every workspace its lead, desks and personal agents.

use email_ai_api::{agents::registry, platform::db};
use sqlx::{SqlitePool, migrate::Migrator};

async fn migrator_up_to(version: i64) -> Migrator {
    let mut m = sqlx::migrate!("./migrations");
    m.migrations = m.migrations.iter().filter(|x| x.version <= version).cloned().collect::<Vec<_>>().into();
    m
}

async fn exec(pool: &SqlitePool, sql: &str) {
    sqlx::query(sql).execute(pool).await.unwrap_or_else(|e| panic!("{sql}: {e}"));
}

async fn count(pool: &SqlitePool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

#[tokio::test]
async fn a_database_at_0033_migrates_and_backfills() {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::connect(&format!("sqlite://{}", dir.path().join("old.db").display())).await.unwrap();
    migrator_up_to(33).await.run(&pool).await.unwrap();
    assert_eq!(count(&pool, "SELECT MAX(version) FROM _sqlx_migrations").await, 33);

    exec(&pool, "INSERT INTO users (id, external_auth_subject, email, created_at) VALUES ('u1','s1','admin@acme.test',1), ('u2','s2','op@acme.test',1)").await;
    exec(&pool, "INSERT INTO workspaces (id, name, created_at) VALUES ('w1','Acme',1), ('w2','Plain',1)").await;
    exec(&pool, "INSERT INTO memberships (workspace_id, user_id, role) VALUES ('w1','u1','admin'), ('w1','u2','operator'), ('w2','u1','admin')").await;
    exec(&pool, "INSERT INTO workspace_packs (workspace_id, pack_key, enabled_at) VALUES ('w1','general',1), ('w1','staffing',1)").await;
    exec(&pool, "INSERT INTO workspace_agents (workspace_id, name, avatar, color, permissions_json, created_at, updated_at) VALUES ('w1','Meera','🌸','plum','{\"draft\":true,\"send\":\"auto\"}',1,1)").await;
    exec(&pool, "INSERT INTO labels (id, workspace_id, key, name, description, created_at) VALUES ('l1','w1','other','Other','Anything else',1), ('l2','w1','timesheet','Timesheet','Hours',1), ('l3','w2','other','Other','Anything else',1)").await;
    exec(&pool, "INSERT INTO email_sources (id, workspace_id, kind, status, inbound_alias, created_at, connected_by_user_id) VALUES ('s1','w1','forwarding','connected','a1',1,'u2')").await;
    exec(&pool, "INSERT INTO email_threads (id, workspace_id, source_id, subject, updated_at) VALUES ('t1','w1','s1','Hours',1)").await;
    exec(&pool, "INSERT INTO email_messages (id, workspace_id, source_id, thread_id, source_message_id, sender_address, subject, body_text, received_at, created_at) VALUES ('m1','w1','s1','t1','g1','ravi@client.com','Hours','Please find my hours',1,1)").await;
    exec(&pool, "INSERT INTO classifications (message_id, category, priority, requires_reply, source, taxonomy_version, updated_at) VALUES ('m1','timesheet','normal',1,'llm','v1',1)").await;
    exec(&pool, "INSERT INTO drafts (id, workspace_id, message_id, body_text, status, created_at, updated_at) VALUES ('d1','w1','m1','Thanks','suggested',1,1)").await;
    exec(&pool, "INSERT INTO cases (id, workspace_id, kind, title, goal, max_steps, budget_usd, message_id, thread_id, created_at, updated_at) VALUES ('c1','w1','inbox','Hours','Reply',15,0.1,'m1','t1',1,1)").await;
    exec(&pool, "INSERT INTO automations (id, workspace_id, name, created_at, updated_at) VALUES ('a1','w1','Ack',1,1)").await;
    exec(&pool, "INSERT INTO user_agent_profiles (workspace_id, user_id, approvals_json, created_at, updated_at) VALUES ('w1','u2','{\"send_email\":\"routine_ok\"}',1,1)").await;

    db::migrate(&pool).await.unwrap();
    db::migrate(&pool).await.unwrap();

    let versions: Vec<i64> = sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version").fetch_all(&pool).await.unwrap();
    assert!(versions.windows(2).all(|w| w[0] < w[1]), "{versions:?}");
    assert!(versions.contains(&51) && versions.last().copied() >= Some(54), "every migration up to 0054 applied: {versions:?}");

    for (table, expected) in [("labels", 3), ("email_messages", 1), ("classifications", 1), ("drafts", 1), ("cases", 1), ("automations", 1), ("workspace_packs", 2)] {
        assert_eq!(count(&pool, &format!("SELECT COUNT(*) FROM {table}")).await, expected, "{table}");
    }
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM workspace_agents").await, 1);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM agents WHERE kind = 'lead'").await, 2, "one lead per workspace");
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM agents WHERE scope = 'personal'").await, 3, "one personal agent per member");
    assert!(count(&pool, "SELECT COUNT(*) FROM agents WHERE workspace_id = 'w1' AND scope = 'workspace' AND kind != 'lead'").await > 1, "pack desks");
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM agents WHERE active_version IS NULL").await, 0, "every agent has a version");
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM agents WHERE name = 'My assistant'").await, 0);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM agents WHERE scope = 'personal' AND name = 'Meera'").await, 3, "every personal agent is Meera");
    registry::seed::backfill_all(&pool).await.unwrap();
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM agents WHERE scope = 'personal'").await, 3, "the backfill is idempotent");
}

/// Production has messages with up to four unsent drafts. The one-live-draft migration keeps the right one per message
/// (a scheduled or sending send first, then a person's edited or approved draft, then the newest), never touches sent
/// drafts or candidate-watch follow-ups, and then the unique index holds.
#[tokio::test]
async fn the_one_live_draft_migration_keeps_the_right_draft_per_message() {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::connect(&format!("sqlite://{}", dir.path().join("old.db").display())).await.unwrap();
    migrator_up_to(57).await.run(&pool).await.unwrap();

    exec(&pool, "INSERT INTO users (id, external_auth_subject, email, created_at) VALUES ('u1','s1','admin@acme.test',1)").await;
    exec(&pool, "INSERT INTO workspaces (id, name, created_at) VALUES ('w1','Acme',1)").await;
    exec(&pool, "INSERT INTO email_sources (id, workspace_id, kind, status, inbound_alias, created_at, connected_by_user_id) VALUES ('s1','w1','forwarding','connected','a1',1,'u1')").await;
    exec(&pool, "INSERT INTO email_threads (id, workspace_id, source_id, subject, updated_at) VALUES ('t1','w1','s1','Hours',1)").await;
    for m in 1..=7 {
        exec(&pool, &format!("INSERT INTO email_messages (id, workspace_id, source_id, thread_id, source_message_id, sender_address, subject, body_text, received_at, created_at) VALUES ('m{m}','w1','s1','t1','g{m}','ravi@client.com','Hours','Hi',1,1)")).await;
    }
    // (id, message, status, origin_key, send_at, send_attempt_id, created_at)
    let drafts: [(&str, &str, &str, &str, &str, &str, i64); 19] = [
        ("a1", "m1", "suggested", "NULL", "NULL", "NULL", 1),
        ("a2", "m1", "suggested", "NULL", "NULL", "NULL", 3),
        ("a3", "m1", "edited", "NULL", "NULL", "NULL", 2),
        ("b1", "m2", "approved", "NULL", "NULL", "NULL", 2),
        ("b2", "m2", "suggested", "NULL", "NULL", "NULL", 5),
        ("b3", "m2", "approved", "NULL", "99999999999999", "NULL", 1),
        ("c1", "m3", "sent", "NULL", "NULL", "NULL", 1),
        ("c2", "m3", "suggested", "NULL", "NULL", "NULL", 2),
        ("c3", "m3", "suggested", "NULL", "NULL", "NULL", 3),
        ("d1", "m4", "suggested", "'watch:w1:r1'", "NULL", "NULL", 1),
        ("d2", "m4", "suggested", "'watch:w1:r2'", "NULL", "NULL", 1),
        ("d3", "m4", "suggested", "'draft:m4:auto'", "NULL", "NULL", 1),
        ("d4", "m4", "suggested", "'agent:c1:1'", "NULL", "NULL", 2),
        ("e1", "m5", "edited", "NULL", "NULL", "NULL", 1),
        ("e2", "m5", "approved", "NULL", "NULL", "NULL", 2),
        ("e3", "m5", "suggested", "NULL", "NULL", "NULL", 3),
        ("f1", "m6", "approved", "NULL", "NULL", "'attempt-1'", 1),
        ("f2", "m6", "suggested", "NULL", "NULL", "NULL", 9),
        ("g1", "m7", "suggested", "NULL", "NULL", "NULL", 1),
    ];
    for (id, msg, status, origin, send_at, attempt, at) in drafts {
        exec(&pool, &format!("INSERT INTO drafts (id, workspace_id, message_id, body_text, status, origin_key, send_at, send_attempt_id, created_at, updated_at) VALUES ('{id}','w1','{msg}','body','{status}',{origin},{send_at},{attempt},{at},{at})")).await;
    }
    exec(&pool, "INSERT INTO draft_owner_words (workspace_id, draft_id, user_id, text, created_at) VALUES ('w1','a2','u1','Say Friday',1)").await;

    db::migrate(&pool).await.unwrap();

    let kept: Vec<String> = sqlx::query_scalar("SELECT id FROM drafts ORDER BY id").fetch_all(&pool).await.unwrap();
    assert_eq!(
        kept,
        ["a3", "b3", "c1", "c3", "d1", "d2", "d4", "e2", "f1", "g1"],
        "edited beats a newer suggestion; a scheduled send beats everything; sent and watch drafts stay; the newest wins a tie of equals"
    );
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM draft_owner_words").await, 0, "a removed draft takes its owner words with it");

    let live_per_message = "SELECT COALESCE(MAX(n), 0) FROM (SELECT COUNT(*) AS n FROM drafts WHERE status != 'sent' AND (origin_key IS NULL OR substr(origin_key,1,6) != 'watch:') GROUP BY message_id)";
    assert_eq!(count(&pool, live_per_message).await, 1);
    let second_live = sqlx::query("INSERT INTO drafts (id, workspace_id, message_id, body_text, status, created_at, updated_at) VALUES ('x1','w1','m1','again','suggested',9,9)").execute(&pool).await;
    assert!(second_live.is_err(), "the unique index refuses a second live draft");
    exec(&pool, "INSERT INTO drafts (id, workspace_id, message_id, body_text, status, origin_key, created_at, updated_at) VALUES ('x2','w1','m1','follow up','suggested','watch:w1:r9',9,9)").await;
    exec(&pool, "INSERT INTO drafts (id, workspace_id, message_id, body_text, status, created_at, updated_at) VALUES ('x3','w1','m3','sent already','sent',9,9)").await;
}
