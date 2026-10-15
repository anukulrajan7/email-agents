//! Workspace labels: templates, CRUD guards, reordering, and classification into custom labels.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use email_ai_api::{build_state, platform::config::Config, platform::db, http::{AppState, router}, jobs::worker};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const ADMIN: &str = "dev:admin@acme.test";

async fn setup() -> (AppState, Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let state = build_state(Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()))).await.unwrap();
    db::migrate(&state.db).await.unwrap();
    (state.clone(), router(state), dir)
}

async fn call(app: &Router, method: &str, path: &str, token: &str, body: Option<Value>) -> (StatusCode, Value) {
    let req = Request::builder().method(method).uri(path).header("x-auth-token", token);
    let req = match body {
        Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn keys(v: &Value) -> Vec<String> {
    v["items"].as_array().unwrap().iter().map(|l| l["key"].as_str().unwrap().to_string()).collect()
}

#[tokio::test]
async fn templates_seed_labels_and_custom_labels_drive_classification() {
    let (state, app, _d) = setup().await;
    let (_, templates) = call(&app, "GET", "/api/v1/label-templates", ADMIN, None).await;
    assert_eq!(keys(&templates), ["hr", "general", "blank"]);

    let (_, ws) = call(&app, "POST", "/api/v1/workspaces", ADMIN, Some(json!({"name":"Ops","template":"general"}))).await;
    let ws = ws["id"].as_str().unwrap().to_string();
    let base = format!("/api/v1/workspaces/{ws}");
    let (_, labels) = call(&app, "GET", &format!("{base}/labels"), ADMIN, None).await;
    assert!(keys(&labels).contains(&"sales_lead".to_string()));
    assert!(!keys(&labels).contains(&"timesheet".to_string()));

    // HR workflows belong to the staffing pack: a general workspace has none.
    let (_, wfs) = call(&app, "GET", &format!("{base}/workflows"), ADMIN, None).await;
    assert!(wfs["items"].as_array().unwrap().is_empty());

    // Custom label: created, slugged, validated, conflict on duplicate.
    let (s, l) = call(&app, "POST", &format!("{base}/labels"), ADMIN,
        Some(json!({"name":"Client requirement","description":"A client or recruiter shares a new job requirement or open position.","color":"plum"}))).await;
    assert_eq!((s, l["key"].as_str()), (StatusCode::CREATED, Some("client_requirement")));
    assert_eq!(call(&app, "POST", &format!("{base}/labels"), ADMIN, Some(json!({"name":"Client requirement","description":"duplicate label here"}))).await.0, StatusCode::CONFLICT);
    assert_eq!(call(&app, "POST", &format!("{base}/labels"), ADMIN, Some(json!({"name":"X","description":"short"}))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&app, "POST", &format!("{base}/labels"), "dev:op@acme.test", Some(json!({"name":"Y","description":"long enough text"}))).await.0, StatusCode::FORBIDDEN);

    // The rules classifier matches custom labels by their name.
    let (_, src) = call(&app, "POST", &format!("{base}/sources/forwarding"), ADMIN, None).await;
    let src = src["id"].as_str().unwrap();
    call(&app, "POST", &format!("{base}/sources/{src}/test-email"), ADMIN,
        Some(json!({"subject":"New requirement from Capgemini","body_text":"Client requirement: 3 Java developers, Pune."}))).await;
    worker::drain(&state).await;
    let (_, msgs) = call(&app, "GET", &format!("{base}/messages"), ADMIN, None).await;
    assert_eq!(msgs["items"][0]["category"], "client_requirement");
    let (_, filtered) = call(&app, "GET", &format!("{base}/messages?category=client_requirement"), ADMIN, None).await;
    assert_eq!(filtered["items"].as_array().unwrap().len(), 1);
    assert_eq!(call(&app, "GET", &format!("{base}/messages?category=nope"), ADMIN, None).await.0, StatusCode::BAD_REQUEST);

    // Manual override validates against active labels.
    let id = msgs["items"][0]["id"].as_str().unwrap();
    assert_eq!(call(&app, "PATCH", &format!("{base}/messages/{id}/classification"), ADMIN, Some(json!({"category":"timesheet"}))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&app, "PATCH", &format!("{base}/messages/{id}/classification"), ADMIN, Some(json!({"category":"sales_lead"}))).await.0, StatusCode::OK);

    // message_count reflects classifications.
    let (_, labels) = call(&app, "GET", &format!("{base}/labels"), ADMIN, None).await;
    let lead = labels["items"].as_array().unwrap().iter().find(|l| l["key"] == "sales_lead").unwrap().clone();
    assert_eq!(lead["message_count"], 1);
}

#[tokio::test]
async fn label_guards_and_reordering() {
    let (_state, app, _d) = setup().await;
    let (_, ws) = call(&app, "POST", "/api/v1/workspaces", ADMIN, Some(json!({"name":"HR","template":"hr"}))).await;
    let base = format!("/api/v1/workspaces/{}", ws["id"].as_str().unwrap());
    let (_, labels) = call(&app, "GET", &format!("{base}/labels"), ADMIN, None).await;
    let items = labels["items"].as_array().unwrap().clone();
    let id_of = |k: &str| items.iter().find(|l| l["key"] == k).unwrap()["id"].as_str().unwrap().to_string();

    // `other` can't be archived; a label used by an enabled workflow can't be either.
    assert_eq!(call(&app, "PATCH", &format!("{base}/labels/{}", id_of("other")), ADMIN, Some(json!({"archived":true}))).await.0, StatusCode::CONFLICT);
    assert_eq!(call(&app, "PATCH", &format!("{base}/labels/{}", id_of("timesheet")), ADMIN, Some(json!({"archived":true}))).await.0, StatusCode::CONFLICT);
    let (s, a) = call(&app, "PATCH", &format!("{base}/labels/{}", id_of("approval")), ADMIN, Some(json!({"archived":true}))).await;
    assert_eq!((s, a["archived"].as_bool()), (StatusCode::OK, Some(true)));
    let (_, active) = call(&app, "GET", &format!("{base}/labels"), ADMIN, None).await;
    assert!(!keys(&active).contains(&"approval".to_string()));

    // Move "leave" (position 1) up to position 0: it swaps with "timesheet".
    call(&app, "PATCH", &format!("{base}/labels/{}", id_of("leave")), ADMIN, Some(json!({"position":0}))).await;
    let (_, after) = call(&app, "GET", &format!("{base}/labels"), ADMIN, None).await;
    assert_eq!(&keys(&after)[..2], ["leave", "timesheet"]);
}

#[tokio::test]
async fn workflow_setup_adds_label_and_approve_clears_review() {
    let (state, app, _d) = setup().await;
    let (_, ws) = call(&app, "POST", "/api/v1/workspaces", ADMIN, Some(json!({"name":"Ops","template":"hr"}))).await;
    let base = format!("/api/v1/workspaces/{}", ws["id"].as_str().unwrap());

    // The Timesheet label was archived (after turning its workflow off): the workflow needs its label.
    let (_, wfs) = call(&app, "GET", &format!("{base}/workflows"), ADMIN, None).await;
    let ts_id = wfs["items"].as_array().unwrap().iter().find(|w| w["template_key"] == "timesheet_review").unwrap()["id"]
        .as_str().unwrap().to_string();
    call(&app, "PATCH", &format!("{base}/workflows/{ts_id}"), ADMIN, Some(json!({"enabled":false}))).await;
    let (_, labels) = call(&app, "GET", &format!("{base}/labels"), ADMIN, None).await;
    let ts_label = labels["items"].as_array().unwrap().iter().find(|l| l["key"] == "timesheet").unwrap()["id"].as_str().unwrap().to_string();
    assert_eq!(call(&app, "PATCH", &format!("{base}/labels/{ts_label}"), ADMIN, Some(json!({"archived":true}))).await.0, StatusCode::OK);
    let (_, wfs) = call(&app, "GET", &format!("{base}/workflows"), ADMIN, None).await;
    let ts = wfs["items"].as_array().unwrap().iter().find(|w| w["template_key"] == "timesheet_review").unwrap().clone();
    assert_eq!((ts["available"].as_bool(), ts["enabled"].as_bool()), (Some(false), Some(false)));

    // "Needs label" → one click: label restored from the HR seed, workflow enabled.
    assert_eq!(call(&app, "POST", &format!("{base}/workflows/{ts_id}/setup"), "dev:op@acme.test", None).await.0, StatusCode::FORBIDDEN);
    let (s, wf) = call(&app, "POST", &format!("{base}/workflows/{ts_id}/setup"), ADMIN, None).await;
    assert_eq!((s, wf["available"].as_bool(), wf["enabled"].as_bool()), (StatusCode::OK, Some(true), Some(true)));
    let (_, labels) = call(&app, "GET", &format!("{base}/labels"), ADMIN, None).await;
    assert!(keys(&labels).contains(&"timesheet".to_string()));

    // A timesheet email now runs the workflow, labelled automatically (no review); approving is still allowed.
    let (_, src) = call(&app, "POST", &format!("{base}/sources/forwarding"), ADMIN, None).await;
    call(&app, "POST", &format!("{base}/sources/{}/test-email", src["id"].as_str().unwrap()), ADMIN, None).await;
    worker::drain(&state).await;
    let (_, msgs) = call(&app, "GET", &format!("{base}/messages"), ADMIN, None).await;
    let m = &msgs["items"][0];
    assert_eq!((m["category"].as_str(), m["needs_review"].as_bool()), (Some("timesheet"), Some(false)));
    let id = m["id"].as_str().unwrap();
    let (s, c) = call(&app, "POST", &format!("{base}/messages/{id}/classification/approve"), ADMIN, None).await;
    assert_eq!((s, c["category"].as_str(), c["needs_review"].as_bool(), c["source"].as_str()), (StatusCode::OK, Some("timesheet"), Some(false), Some("fallback")));
    let (_, msgs) = call(&app, "GET", &format!("{base}/messages"), ADMIN, None).await;
    assert_eq!(msgs["items"][0]["needs_review"], false);
    let (_, runs) = call(&app, "GET", &format!("{base}/workflow-runs"), ADMIN, None).await;
    assert_eq!(runs["items"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn new_label_rechecks_recent_other_mail_but_not_manual_choices() {
    let (state, app, _d) = setup().await;
    let (_, ws) = call(&app, "POST", "/api/v1/workspaces", ADMIN, Some(json!({"name":"Ops","template":"general"}))).await;
    let base = format!("/api/v1/workspaces/{}", ws["id"].as_str().unwrap());
    let (_, src) = call(&app, "POST", &format!("{base}/sources/forwarding"), ADMIN, None).await;
    let test_email = format!("{base}/sources/{}/test-email", src["id"].as_str().unwrap());
    for subject in ["Sponsor our tech meetup", "Conference booth for TechConf"] {
        call(&app, "POST", &test_email, ADMIN, Some(json!({"from":"events@meetup.test","subject":subject,"body_text":"We would love to see your team there."}))).await;
    }
    worker::drain(&state).await;
    let (_, msgs) = call(&app, "GET", &format!("{base}/messages"), ADMIN, None).await;
    let items = msgs["items"].as_array().unwrap();
    assert!(items.iter().all(|m| m["category"] == "other"), "no events label yet: {:?}", items.iter().map(|m| (m["subject"].clone(), m["category"].clone())).collect::<Vec<_>>());
    // A person files one of them by hand: that choice must survive.
    let manual = items[0]["id"].as_str().unwrap();
    sqlx::query("UPDATE classifications SET source = 'manual' WHERE message_id = ?1").bind(manual).execute(&state.db).await.unwrap();

    // Two labels added back to back share one re-check: each email is re-classified once.
    for (name, description) in [("Events", "Meetups, conferences and sponsorship requests"), ("Sponsorships", "Requests to sponsor or exhibit")] {
        let (s, _) = call(&app, "POST", &format!("{base}/labels"), ADMIN, Some(json!({"name":name,"description":description}))).await;
        assert_eq!(s, StatusCode::CREATED);
    }
    let queued: Vec<(String, i64)> = sqlx::query_as("SELECT dedupe_key, priority FROM jobs WHERE dedupe_key LIKE 'classify:%:recheck:%'")
        .fetch_all(&state.db).await.unwrap();
    assert_eq!(queued.len(), 1, "only the non-manual 'other' email, once for both labels: {queued:?}");
    assert!(!queued[0].0.contains(manual));
    // Bulk re-checks wait behind new mail.
    assert!(queued[0].1 < email_ai_api::jobs::priority_of("sync_nango_source", "sync:x"));
    assert!(queued[0].1 < email_ai_api::jobs::priority_of("classify_email", "classify:m:taxonomy-v1"));
    worker::drain(&state).await;
}
