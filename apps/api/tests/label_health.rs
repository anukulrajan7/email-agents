//! Label health: correction tracking on human label changes and the per-label health report.

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
const OPERATOR: &str = "dev:op@acme.test";
const DAY_MS: i64 = 86_400_000;

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

async fn send(state: &AppState, app: &Router, base: &str, source: &str, subject: &str) -> String {
    let (s, _) = call(app, "POST", &format!("{base}/sources/{source}/test-email"), ADMIN, Some(json!({
        "from": "Priya Sharma <priya.sharma@example.com>", "subject": subject, "body_text": "Please see the details below."
    }))).await;
    assert!(s.is_success());
    worker::drain(state).await;
    let (_, msgs) = call(app, "GET", &format!("{base}/messages"), ADMIN, None).await;
    msgs["items"].as_array().unwrap().iter().find(|m| m["subject"] == subject).unwrap()["id"].as_str().unwrap().to_string()
}

async fn relabel(app: &Router, base: &str, id: &str, category: &str) {
    let (s, _) = call(app, "PATCH", &format!("{base}/messages/{id}/classification"), OPERATOR, Some(json!({"category": category}))).await;
    assert_eq!(s, StatusCode::OK);
}

fn label<'a>(report: &'a Value, key: &str) -> &'a Value {
    report["labels"].as_array().unwrap().iter().find(|l| l["key"] == key).unwrap()
}

fn kinds(report: &Value) -> Vec<(String, Option<String>)> {
    report["advice"].as_array().unwrap().iter()
        .map(|a| (a["kind"].as_str().unwrap().to_string(), a["label_key"].as_str().map(str::to_string)))
        .collect()
}

#[tokio::test]
async fn corrections_feed_the_health_report() {
    let (state, app, _d) = setup().await;
    let (_, ws) = call(&app, "POST", "/api/v1/workspaces", ADMIN, Some(json!({"name":"Ops","template":"blank"}))).await;
    let ws = ws["id"].as_str().unwrap().to_string();
    let base = format!("/api/v1/workspaces/{ws}");
    call(&app, "GET", "/api/v1/me", OPERATOR, None).await;
    call(&app, "POST", &format!("{base}/members"), ADMIN, Some(json!({"email":"op@acme.test","role":"operator"}))).await;
    for (name, desc) in [("Timesheet", "An employee submits a timesheet or hours worked."), ("Invoices", "Vendor invoices and payment requests.")] {
        let (s, _) = call(&app, "POST", &format!("{base}/labels"), ADMIN, Some(json!({"name": name, "description": desc}))).await;
        assert_eq!(s, StatusCode::CREATED);
    }
    let (_, src) = call(&app, "POST", &format!("{base}/sources/forwarding"), ADMIN, None).await;
    let src = src["id"].as_str().unwrap().to_string();

    // Nothing yet: empty report, no advice.
    let (s, empty) = call(&app, "GET", &format!("{base}/labels/health"), OPERATOR, None).await;
    assert_eq!((s, empty["days"].as_i64(), empty["total"].as_i64()), (StatusCode::OK, Some(30), Some(0)));
    assert!(empty["advice"].as_array().unwrap().is_empty());
    assert_eq!(empty["other_trend"].as_array().unwrap().len(), 30);

    let mut sheets = vec![];
    for i in 0..6 {
        sheets.push(send(&state, &app, &base, &src, &format!("Timesheet week {i}")).await);
    }
    let mut lunch = vec![];
    for i in 0..3 {
        lunch.push(send(&state, &app, &base, &src, &format!("Lunch plans {i}")).await);
    }

    // The AI's own picks are not corrections.
    let (_, before) = call(&app, "GET", &format!("{base}/labels/health"), OPERATOR, None).await;
    assert_eq!((before["total"].as_i64(), before["other_share"].as_f64()), (Some(9), Some(0.33)));
    let ts = label(&before, "timesheet");
    assert_eq!((ts["count"].as_i64(), ts["auto"].as_i64(), ts["corrected_away"].as_i64(), ts["corrected_into"].as_i64()), (Some(6), Some(6), Some(0), Some(0)));

    // People move two timesheets to Other and one lunch mail into Timesheet.
    relabel(&app, &base, &sheets[0], "other").await;
    relabel(&app, &base, &sheets[1], "other").await;
    relabel(&app, &base, &lunch[0], "timesheet").await;

    let (ai, confirmed): (Option<String>, bool) =
        sqlx::query_as("SELECT ai_category, confirmed FROM classifications WHERE message_id = ?1")
            .bind(&sheets[0]).fetch_one(&state.db).await.unwrap();
    assert_eq!((ai.as_deref(), confirmed), (Some("timesheet"), true));

    // A second change keeps what the AI first said.
    relabel(&app, &base, &sheets[0], "invoices").await;
    let ai: Option<String> = sqlx::query_scalar("SELECT ai_category FROM classifications WHERE message_id = ?1")
        .bind(&sheets[0]).fetch_one(&state.db).await.unwrap();
    assert_eq!(ai.as_deref(), Some("timesheet"));

    let (_, report) = call(&app, "GET", &format!("{base}/labels/health?days=30"), OPERATOR, None).await;
    assert_eq!(report["total"], 9);
    let ts = label(&report, "timesheet");
    assert_eq!(
        (ts["count"].as_i64(), ts["auto"].as_i64(), ts["corrected_away"].as_i64(), ts["corrected_into"].as_i64(), ts["correction_rate"].as_f64()),
        (Some(5), Some(4), Some(2), Some(1), Some(0.33)),
    );
    assert_eq!(ts["share"].as_f64(), Some(0.56));
    let trend = ts["trend"].as_array().unwrap();
    assert_eq!(trend.len(), 30);
    assert_eq!(trend.iter().map(|p| p["count"].as_i64().unwrap()).sum::<i64>(), 5);
    assert_eq!(trend[29]["count"], 5);
    let inv = label(&report, "invoices");
    assert_eq!((inv["count"].as_i64(), inv["corrected_into"].as_i64(), inv["corrected_away"].as_i64()), (Some(1), Some(1), Some(0)));
    assert!(report["labels"].as_array().unwrap().iter().all(|l| l["key"] != "other"));
    assert_eq!(report["other_share"].as_f64(), Some(0.33));
    assert_eq!(report["other_trend"][29]["count"], 3);
    assert_eq!(report["auto_share"].as_f64(), Some(0.67));

    // 3 of 9 in Other -> add a label; Timesheet is corrected often -> sharpen it.
    assert_eq!(kinds(&report), [("add_label".to_string(), None), ("label_unreliable".to_string(), Some("timesheet".to_string()))]);
    assert!(report["advice"][0]["text"].as_str().unwrap().contains("33% of mail lands in Other"));
}

#[tokio::test]
async fn window_roles_and_unused_labels() {
    let (state, app, _d) = setup().await;
    let (_, ws) = call(&app, "POST", "/api/v1/workspaces", ADMIN, Some(json!({"name":"Ops","template":"blank"}))).await;
    let ws = ws["id"].as_str().unwrap().to_string();
    let base = format!("/api/v1/workspaces/{ws}");
    call(&app, "GET", "/api/v1/me", OPERATOR, None).await;
    call(&app, "POST", &format!("{base}/members"), ADMIN, Some(json!({"email":"op@acme.test","role":"operator"}))).await;
    call(&app, "POST", &format!("{base}/labels"), ADMIN, Some(json!({"name":"Timesheet","description":"An employee submits a timesheet."}))).await;
    call(&app, "POST", &format!("{base}/labels"), ADMIN, Some(json!({"name":"Invoices","description":"Vendor invoices and payment requests."}))).await;
    let (_, src) = call(&app, "POST", &format!("{base}/sources/forwarding"), ADMIN, None).await;
    let src = src["id"].as_str().unwrap().to_string();

    let recent = send(&state, &app, &base, &src, "Timesheet this week").await;
    let old = send(&state, &app, &base, &src, "Timesheet last quarter").await;
    sqlx::query("UPDATE email_messages SET received_at = received_at - ?1 WHERE id = ?2")
        .bind(100 * DAY_MS).bind(&old).execute(&state.db).await.unwrap();
    // Outbound mail never counts.
    sqlx::query("UPDATE email_messages SET direction = 'outbound' WHERE id = ?1")
        .bind(&recent).execute(&state.db).await.unwrap();
    let kept = send(&state, &app, &base, &src, "Timesheet kept").await;
    assert_ne!(kept, recent);

    let (_, thirty) = call(&app, "GET", &format!("{base}/labels/health"), OPERATOR, None).await;
    assert_eq!(thirty["total"], 1);
    let (_, all) = call(&app, "GET", &format!("{base}/labels/health?days=180"), OPERATOR, None).await;
    assert_eq!((all["total"].as_i64(), all["other_trend"].as_array().unwrap().len()), (Some(2), 180));

    // Invoices got nothing in the window.
    assert_eq!(kinds(&thirty), [("label_unused".to_string(), Some("invoices".to_string()))]);

    for bad in ["days=0", "days=181", "days=abc"] {
        assert_eq!(call(&app, "GET", &format!("{base}/labels/health?{bad}"), OPERATOR, None).await.0, StatusCode::BAD_REQUEST, "{bad}");
    }
    call(&app, "GET", "/api/v1/me", "dev:stranger@acme.test", None).await;
    assert_eq!(call(&app, "GET", &format!("{base}/labels/health"), "dev:stranger@acme.test", None).await.0, StatusCode::FORBIDDEN);
}
