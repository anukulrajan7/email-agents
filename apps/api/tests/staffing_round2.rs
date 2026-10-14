//! Round 2: suppressed mail stays out of every read path, release re-judges each message and finishes the skipped
//! steps, the mail wall holds on drafts / reviews / alerts / runs / contacts, owner checks run inside the transaction.

mod staffing_common;

use axum::http::StatusCode;
use email_ai_api::alerts;
use serde_json::{Value, json};
use staffing_common::*;

const T: i64 = WED;

fn settings(extra: Value) -> Value {
    let mut s = json!({"tz_offset_minutes": 0, "manager_after_hours": null, "final_after_hours": null, "final_emails": [],
                       "below_target_hour": 16, "weekly_check_hour": 12});
    for (k, v) in extra.as_object().unwrap() {
        s[k] = v.clone();
    }
    s
}

async fn count(h: &H, sql: &str, bind: &str) -> i64 {
    sqlx::query_scalar(sql).bind(bind).fetch_one(&h.state.db).await.unwrap()
}

async fn put_settings(h: &H, ws: &str, extra: Value) {
    let (s, b) = h.call("PUT", &format!("{}/team/settings", api(ws)), OWNER, Some(settings(extra))).await;
    assert_eq!(s, StatusCode::OK, "{b}");
}

async fn draft(h: &H, ws: &str, message_id: &str, id: &str) {
    sqlx::query("INSERT INTO drafts (id, workspace_id, message_id, body_text, status, created_at, updated_at) VALUES (?1, ?2, ?3, 'Hello', 'edited', 1, 1)")
        .bind(id)
        .bind(ws)
        .bind(message_id)
        .execute(&h.state.db)
        .await
        .unwrap();
}

async fn run_review_alert(h: &H, ws: &str, message_id: &str, tag: &str) {
    sqlx::query("INSERT OR IGNORE INTO workflows (id, workspace_id, template_key, enabled, config_json, created_at) VALUES ('wf1', ?1, 'tpl', 1, '{}', 1)")
        .bind(ws)
        .execute(&h.state.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO workflow_runs (id, workspace_id, workflow_id, message_id, status, created_at) VALUES (?1, ?2, 'wf1', ?3, 'completed', 1)")
        .bind(format!("run_{tag}"))
        .bind(ws)
        .bind(message_id)
        .execute(&h.state.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO review_items (id, workspace_id, message_id, workflow_run_id, kind, title, status, created_at) VALUES (?1, ?2, ?3, ?4, 'k', ?5, 'open', 1)")
        .bind(format!("rev_{tag}"))
        .bind(ws)
        .bind(message_id)
        .bind(format!("run_{tag}"))
        .bind(format!("review {tag}"))
        .execute(&h.state.db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO alerts (id, workspace_id, kind, severity, title, body, message_id, dedupe_key, status, level_since, created_at, updated_at)
         VALUES (?1, ?2, 'important_unanswered', 'high', ?3, 'b', ?4, ?1, 'open', 1, 1, 1)",
    )
    .bind(format!("alt_{tag}"))
    .bind(ws)
    .bind(format!("alert {tag}"))
    .bind(message_id)
    .execute(&h.state.db)
    .await
    .unwrap();
}

fn titles(v: &Value) -> Vec<String> {
    v["items"].as_array().unwrap().iter().map(|i| i["title"].as_str().unwrap().to_string()).collect()
}

#[tokio::test]
async fn suppressed_mail_is_invisible_to_every_read_path_and_never_counts_as_a_reply() {
    let h = harness().await;
    let ws = h.workspace(&["staffing"]).await;
    let hr = h.mailbox(&ws, "hr@acme.test").await;
    let bx = h.mailbox(&ws, "priya@acme.test").await;
    put_settings(&h, &ws, json!({"hr_source_id": hr})).await;
    h.person(&ws, json!({"name": "Ravi Kumar", "emails": ["ravi@client.test"], "employee_type": "billable", "client": "TCS"})).await;

    // A kept inbound mail, then a reply that only a billable person sent outside the HR mailbox.
    let kept = h.ingest(&ws, &bx, email("cust@shop.test", &["priya@acme.test"], "Kept question", "q", T), "th1").await.unwrap();
    let hidden_reply = h.ingest(&ws, &bx, sent(email("ravi@client.test", &["cust@shop.test"], "Re: Kept question", "private", T + 10)), "th1").await.unwrap();
    assert_eq!(count(&h, "SELECT suppressed FROM email_messages WHERE id = ?1", &hidden_reply).await, 1);
    let kept_thread: String = sqlx::query_scalar("SELECT thread_id FROM email_messages WHERE id = ?1").bind(&kept).fetch_one(&h.state.db).await.unwrap();
    let hidden_thread: String = sqlx::query_scalar("SELECT thread_id FROM email_messages WHERE id = ?1").bind(&hidden_reply).fetch_one(&h.state.db).await.unwrap();
    assert_eq!(kept_thread, hidden_thread);

    let (_, list) = h.get(OWNER, &format!("{}/messages?direction=all", api(&ws))).await;
    let row = list["items"].as_array().unwrap().iter().find(|m| m["id"] == kept.as_str()).unwrap();
    assert_eq!(row["replied"], false, "a hidden reply is not a reply");
    assert_eq!(h.get(OWNER, &format!("{}/messages?direction=all&reply=replied", api(&ws))).await.1["items"].as_array().unwrap().len(), 0);

    // Every route that acts on the hidden message answers 404.
    let base = api(&ws);
    for (m, p, b) in [
        ("GET", format!("messages/{hidden_reply}"), None),
        ("POST", format!("messages/{hidden_reply}/reprocess"), None),
        ("POST", format!("messages/{hidden_reply}/draft"), None),
        ("POST", format!("messages/{hidden_reply}/insight"), None),
        ("PATCH", format!("messages/{hidden_reply}/classification"), Some(json!({"category": "question"}))),
        ("POST", format!("messages/{hidden_reply}/classification/approve"), None),
    ] {
        assert_eq!(h.call(m, &format!("{base}/{p}"), OWNER, b).await.0, StatusCode::NOT_FOUND, "{m} {p}");
    }
    // The kept message is still reachable.
    assert_eq!(h.get(OWNER, &format!("{base}/messages/{kept}")).await.0, StatusCode::OK);

    // Reviews, alerts, workflow runs and a contact's recent mail leave it out.
    run_review_alert(&h, &ws, &kept, "kept").await;
    run_review_alert(&h, &ws, &hidden_reply, "hidden").await;
    assert_eq!(titles(&h.get(OWNER, &format!("{base}/reviews")).await.1), vec!["review kept"]);
    let (_, al) = h.get(OWNER, &format!("{base}/alerts")).await;
    assert_eq!(titles(&al), vec!["alert kept"]);
    assert_eq!(al["open_count"], 1);
    let runs = h.get(OWNER, &format!("{base}/workflow-runs")).await.1;
    assert_eq!(runs["items"].as_array().unwrap().len(), 1);
    h.ingest(&ws, &bx, email("ravi@client.test", &["priya@acme.test"], "Direct hidden", "x", T + 20), "th2").await.unwrap();
    h.ingest(&ws, &hr, email("ravi@client.test", &["hr@acme.test"], "Timesheet kept", "Total hours: 100", T + 30), "th3").await.unwrap();
    let (s, c) = h.get(OWNER, &format!("{base}/contacts/ravi@client.test")).await;
    assert_eq!(s, StatusCode::OK, "{c}");
    let subjects: Vec<&str> = c["recent_messages"].as_array().unwrap().iter().map(|m| m["subject"].as_str().unwrap()).collect();
    assert_eq!(subjects, vec!["Timesheet kept"]);

    // The thread-level reply check and the alert engine ignore it too.
    assert!(!email_ai_api::threads::thread_has_reply_after(&h.state.db, &ws, &kept_thread, T).await.unwrap());
    let _ = alerts::evaluate_workspace(&h.state, &ws, T + 100 * HOUR).await.unwrap();
}

#[tokio::test]
async fn suppressed_attachments_are_stored_as_metadata_and_release_finishes_every_skipped_step() {
    let h = harness().await;
    let ws = h.workspace(&["staffing"]).await;
    let hr = h.mailbox(&ws, "hr@acme.test").await;
    let bx = h.mailbox(&ws, "priya@acme.test").await;
    put_settings(&h, &ws, json!({"hr_source_id": hr})).await;
    let ravi = h.person(&ws, json!({"name": "Ravi Kumar", "emails": ["ravi@client.test"], "employee_type": "billable", "client": "TCS"})).await;

    let id = h.ingest(&ws, &bx, with_files(email("ravi@client.test", &["priya@acme.test"], "Offer letter", "see attached", T), &["offer.pdf"]), "th1").await.unwrap();
    assert_eq!(count(&h, "SELECT COUNT(*) FROM attachments WHERE message_id = ?1", &id).await, 1);
    assert_eq!(count(&h, "SELECT COUNT(*) FROM jobs WHERE payload_json LIKE '%' || ?1 || '%'", &id).await, 0);
    assert_eq!(count(&h, "SELECT COUNT(*) FROM contacts WHERE email = ?1", "ravi@client.test").await, 0);

    // Ravi stops being billable: the mail is released with everything it skipped.
    let (s, b) = h.call("PATCH", &format!("{}/team/people/{}", api(&ws), ravi["id"].as_str().unwrap()), OWNER, Some(json!({"employee_type": "non_billable"}))).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(count(&h, "SELECT suppressed FROM email_messages WHERE id = ?1", &id).await, 0);
    assert_eq!(count(&h, "SELECT COUNT(*) FROM contacts WHERE email = ?1", "ravi@client.test").await, 1, "contact recorded");
    assert_eq!(count(&h, "SELECT COUNT(*) FROM jobs WHERE kind = 'classify_email' AND payload_json LIKE '%' || ?1 || '%'", &id).await, 1);
    assert_eq!(count(&h, "SELECT COUNT(*) FROM jobs WHERE kind = 'extract_attachments' AND payload_json LIKE '%' || ?1 || '%'", &id).await, 1);
}

#[tokio::test]
async fn release_judges_each_message_again_so_moving_the_hr_mailbox_keeps_other_mailboxes_suppressed() {
    let h = harness().await;
    let ws = h.workspace(&["staffing"]).await;
    let (a, b, c) = (h.mailbox(&ws, "hr-a@acme.test").await, h.mailbox(&ws, "hr-b@acme.test").await, h.mailbox(&ws, "priya@acme.test").await);
    put_settings(&h, &ws, json!({"hr_source_id": a})).await;
    h.person(&ws, json!({"name": "Ravi Kumar", "emails": ["ravi@client.test"], "employee_type": "billable", "client": "TCS"})).await;

    let in_b = h.ingest(&ws, &b, email("ravi@client.test", &["hr-b@acme.test"], "Via B", "x", T), "t1").await.unwrap();
    let in_c = h.ingest(&ws, &c, email("ravi@client.test", &["priya@acme.test"], "Via C", "x", T + 1), "t2").await.unwrap();
    let in_a = h.ingest(&ws, &a, email("ravi@client.test", &["hr-a@acme.test"], "Via A", "x", T + 2), "t3").await.unwrap();
    let s = |id: String| {
        let h = &h;
        async move { count(h, "SELECT suppressed FROM email_messages WHERE id = ?1", &id).await }
    };
    assert_eq!((s(in_b.clone()).await, s(in_c.clone()).await, s(in_a).await), (1, 1, 0));

    put_settings(&h, &ws, json!({"hr_source_id": b})).await;
    assert_eq!((s(in_b).await, s(in_c).await), (0, 1), "HR moved A to B: B's mail is now the HR mailbox, C's stays hidden");
}

#[tokio::test]
async fn the_wall_holds_on_drafts_reviews_alerts_runs_and_contacts() {
    let h = harness().await;
    let ws = h.workspace(&["staffing"]).await;
    let priya_t = h.member(&ws, "priya@acme.test", "operator").await;
    let (pb, ob) = (h.mailbox(&ws, "priya@acme.test").await, h.mailbox(&ws, "other@acme.test").await);
    h.person(&ws, json!({"name": "Priya", "emails": ["priya@acme.test"], "source_id": pb})).await;
    h.person(&ws, json!({"name": "Other", "emails": ["other@acme.test"], "source_id": ob})).await;
    put_settings(&h, &ws, json!({"restrict_mail_to_team": true})).await;

    let mine = h.ingest(&ws, &pb, email("c@x.test", &["priya@acme.test"], "Mine", "x", T), "w1").await.unwrap();
    let theirs = h.ingest(&ws, &ob, email("c@x.test", &["other@acme.test"], "Theirs", "x", T + 1), "w2").await.unwrap();
    draft(&h, &ws, &mine, "drf_mine").await;
    draft(&h, &ws, &theirs, "drf_theirs").await;
    run_review_alert(&h, &ws, &mine, "mine").await;
    run_review_alert(&h, &ws, &theirs, "theirs").await;
    let base = api(&ws);

    for (m, p, b) in [
        ("PATCH", "drafts/drf_theirs", Some(json!({"body_text": "x"}))),
        ("POST", "drafts/drf_theirs/provider-draft", None),
        ("POST", "drafts/drf_theirs/send", Some(json!({"confirm": true}))),
        ("POST", "drafts/drf_theirs/check", None),
        ("POST", "drafts/drf_theirs/undo-send", None),
    ] {
        assert_eq!(h.call(m, &format!("{base}/{p}"), &priya_t, b).await.0, StatusCode::NOT_FOUND, "{m} {p}");
    }
    assert_eq!(h.call("PATCH", &format!("{base}/drafts/drf_mine"), &priya_t, Some(json!({"body_text": "ok"}))).await.0, StatusCode::OK);

    assert_eq!(titles(&h.get(&priya_t, &format!("{base}/reviews")).await.1), vec!["review mine"]);
    assert_eq!(titles(&h.get(OWNER, &format!("{base}/reviews")).await.1).len(), 2);
    let (_, al) = h.get(&priya_t, &format!("{base}/alerts")).await;
    assert_eq!((titles(&al), al["open_count"].as_i64()), (vec!["alert mine".to_string()], Some(1)));
    assert_eq!(h.get(&priya_t, &format!("{base}/workflow-runs")).await.1["items"].as_array().unwrap().len(), 1);

    h.ingest(&ws, &ob, email("shared@x.test", &["other@acme.test"], "Hidden by wall", "x", T + 5), "w3").await.unwrap();
    h.ingest(&ws, &pb, email("shared@x.test", &["priya@acme.test"], "Visible", "x", T + 6), "w4").await.unwrap();
    let (s, c) = h.get(&priya_t, &format!("{base}/contacts/shared@x.test")).await;
    assert_eq!(s, StatusCode::OK, "{c}");
    let subjects: Vec<&str> = c["recent_messages"].as_array().unwrap().iter().map(|m| m["subject"].as_str().unwrap()).collect();
    assert_eq!(subjects, vec!["Visible"]);
}

#[tokio::test]
async fn owner_demotion_and_transfer_checks_agree_with_the_committed_state() {
    let h = harness().await;
    let ws = h.workspace(&["general"]).await;
    let second = h.member(&ws, "second@acme.test", "admin").await;
    let team = format!("{}/team", api(&ws));
    let members = format!("{}/members", api(&ws));
    assert_eq!(h.call("POST", &members, &second, Some(json!({"email": "owner@acme.test", "role": "operator"}))).await.0, StatusCode::CONFLICT);
    assert_eq!(h.call("POST", &format!("{team}/ownership"), OWNER, Some(json!({"email": "second@acme.test"}))).await.0, StatusCode::OK);
    // The new owner is protected, the old one can now be demoted.
    assert_eq!(h.call("POST", &members, OWNER, Some(json!({"email": "second@acme.test", "role": "operator"}))).await.0, StatusCode::CONFLICT);
    assert_eq!(h.call("POST", &members, &second, Some(json!({"email": "owner@acme.test", "role": "operator"}))).await.0, StatusCode::OK);
    // A target who is no longer an admin is refused.
    assert_eq!(h.call("POST", &format!("{team}/ownership"), &second, Some(json!({"email": "owner@acme.test"}))).await.0, StatusCode::BAD_REQUEST);
}
