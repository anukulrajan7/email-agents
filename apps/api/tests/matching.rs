//! Referral ↔ requirement matching, end to end: labelled emails → records → scored
//! matches with reasons → recruiter decision that re-scoring never overrides.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use email_ai_api::{build_state, config::Config, db, http::{AppState, router}, jobs::worker};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const ADMIN: &str = "dev:admin@acme.test";

async fn call(app: &Router, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let req = Request::builder().method(method).uri(path).header("x-auth-token", ADMIN);
    let req = match body {
        Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn setup() -> (AppState, Router, String, String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let state = build_state(Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()))).await.unwrap();
    db::migrate(&state.db).await.unwrap();
    let app = router(state.clone());
    let (_, ws) = call(&app, "POST", "/api/v1/workspaces", Some(json!({"name":"Wokay","template":"hr"}))).await;
    let ws = ws["id"].as_str().unwrap().to_string();
    let (_, src) = call(&app, "POST", &format!("/api/v1/workspaces/{ws}/sources/forwarding"), None).await;
    (state, app, ws, src["id"].as_str().unwrap().to_string(), dir)
}

async fn email(app: &Router, state: &AppState, ws: &str, src: &str, from: &str, subject: &str, body: &str) {
    let (s, _) = call(app, "POST", &format!("/api/v1/workspaces/{ws}/sources/{src}/test-email"),
        Some(json!({"from": from, "subject": subject, "body_text": body}))).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    worker::drain(state).await;
}

#[tokio::test]
async fn referral_and_requirement_emails_become_explained_matches() {
    let (state, app, ws, src, _d) = setup().await;
    let base = format!("/api/v1/workspaces/{ws}/matching");

    // Setup is idempotent and admin-only labels already exist in the HR template.
    assert_eq!(call(&app, "POST", &format!("{base}/setup"), None).await.0, StatusCode::OK);

    email(&app, &state, &ws, &src, "Friend <friend@gmail.com>", "Referral - Rahul Verma",
        "Hi, referring my friend.\nName: Rahul Verma\n6 years in Java, SpringBoot, Microservices.\nPune. Notice period 30 days.\nrahul.v@mail.com").await;
    email(&app, &state, &ws, &src, "Recruiter <hr@capgemini.com>", "Requirement: Java Developer",
        "Need 3 developers with Java, Spring Boot and Microservices. 5-8 yrs. Location Pune.").await;
    email(&app, &state, &ws, &src, "Friend <friend@gmail.com>", "Referral - Asha Rao",
        "Name: Asha Rao\n2 years Python, Django. Chennai. Immediate joiner.").await;

    let (_, reqs) = call(&app, "GET", &format!("{base}/requirements"), None).await;
    let req = &reqs["items"][0];
    assert_eq!(reqs["items"].as_array().unwrap().len(), 1);
    assert_eq!((req["role_title"].as_str(), req["client"].as_str(), req["location"].as_str()), (Some("Java Developer"), Some("capgemini"), Some("Pune")));
    assert_eq!(req["must_have_skills"], json!(["java", "spring boot", "microservices"]));
    assert_eq!((req["min_experience"].as_f64(), req["max_experience"].as_f64(), req["positions"].as_i64()), (Some(5.0), Some(8.0), Some(3)));

    let (_, cands) = call(&app, "GET", &format!("{base}/candidates"), None).await;
    let names: Vec<&str> = cands["items"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"Rahul Verma") && names.contains(&"Asha Rao"));

    // Rahul matches strongly (found when the requirement arrived after him); Asha doesn't qualify.
    let (_, matches) = call(&app, "GET", &format!("{base}/matches?requirement_id={}", req["id"].as_str().unwrap()), None).await;
    let items = matches["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "weak candidate filtered out");
    let m = &items[0];
    assert_eq!((m["candidate"]["name"].as_str(), m["score"].as_i64()), (Some("Rahul Verma"), Some(100)));
    let reasons: Vec<&str> = m["reasons"].as_array().unwrap().iter().map(|r| r["text"].as_str().unwrap()).collect();
    assert!(reasons.iter().any(|r| r.starts_with("Has 3/3 must-have skills")), "{reasons:?}");
    assert!(reasons.contains(&"6 yrs fits 5–8 yrs"), "{reasons:?}");

    // Recruiter shortlists; a later re-score keeps the decision.
    let mid = m["id"].as_str().unwrap();
    let (s, decided) = call(&app, "PATCH", &format!("{base}/matches/{mid}"), Some(json!({"status":"shortlisted"}))).await;
    assert_eq!((s, decided["status"].as_str()), (StatusCode::OK, Some("shortlisted")));
    let (_, msgs) = call(&app, "GET", &format!("/api/v1/workspaces/{ws}/messages"), None).await;
    for msg in msgs["items"].as_array().unwrap() {
        call(&app, "POST", &format!("/api/v1/workspaces/{ws}/messages/{}/reprocess", msg["id"].as_str().unwrap()), None).await;
    }
    worker::drain(&state).await;
    let (_, after) = call(&app, "GET", &format!("{base}/matches"), None).await;
    assert_eq!(after["items"][0]["status"], "shortlisted");
    assert_eq!(call(&app, "GET", &format!("{base}/requirements"), None).await.1["items"].as_array().unwrap().len(), 1, "no duplicate records on re-run");

    // Validation + tenant isolation.
    assert_eq!(call(&app, "PATCH", &format!("{base}/matches/{mid}"), Some(json!({"status":"hired"}))).await.0, StatusCode::BAD_REQUEST);
    let other = Request::builder().uri(format!("{base}/matches")).header("x-auth-token", "dev:out@sider.test").body(Body::empty()).unwrap();
    assert_eq!(app.clone().oneshot(other).await.unwrap().status(), StatusCode::FORBIDDEN);
}
