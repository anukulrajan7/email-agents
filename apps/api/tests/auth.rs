//! Email + password auth: signup, login, sessions, logout, rate limiting, allow-list.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use std::sync::Arc;

use email_ai_api::{
    build_state,
    config::{AuthMode, Config},
    db,
    http::router,
    mailer::MemoryMailer,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

async fn make_app(configure: impl FnOnce(&mut Config)) -> (Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()));
    config.auth_mode = AuthMode::Password;
    config.signup_allowed_domains = vec!["acme.com".into()];
    configure(&mut config);
    let state = build_state(config).await.unwrap();
    db::migrate(&state.db).await.unwrap();
    (router(state), dir)
}

async fn call(app: &Router, method: &str, path: &str, token: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(t) = token {
        req = req.header("x-auth-token", t);
    }
    let req = match body {
        Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

const PW: &str = "correct horse battery";

#[tokio::test]
async fn signup_login_session_logout() {
    let (app, _d) = make_app(|_| {}).await;
    assert_eq!(call(&app, "GET", "/api/v1/auth/config", None, None).await.1, json!({"mode":"password","signup_enabled":true,"email_verification":false}));

    let (s, body) =
        call(&app, "POST", "/api/v1/auth/signup", None, Some(json!({"email":"Priya@Acme.com","password":PW,"name":"Priya"}))).await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(body["user"]["email"], "priya@acme.com");
    let token = body["token"].as_str().unwrap().to_string();

    let (s, me) = call(&app, "GET", "/api/v1/me", Some(&token), None).await;
    assert_eq!((s, me["user"]["name"].as_str(), me["auth_mode"].as_str()), (StatusCode::OK, Some("Priya"), Some("password")));
    // Dev tokens are not accepted in password mode.
    assert_eq!(call(&app, "GET", "/api/v1/me", Some("dev:priya@acme.com"), None).await.0, StatusCode::UNAUTHORIZED);

    // Duplicate, short password, bad email.
    assert_eq!(call(&app, "POST", "/api/v1/auth/signup", None, Some(json!({"email":"priya@acme.com","password":PW}))).await.0, StatusCode::CONFLICT);
    assert_eq!(call(&app, "POST", "/api/v1/auth/signup", None, Some(json!({"email":"x@acme.com","password":"short"}))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&app, "POST", "/api/v1/auth/signup", None, Some(json!({"email":"nope","password":PW}))).await.0, StatusCode::BAD_REQUEST);

    // Wrong password and unknown account look identical.
    let wrong = call(&app, "POST", "/api/v1/auth/login", None, Some(json!({"email":"priya@acme.com","password":"wrong password!"}))).await;
    let unknown = call(&app, "POST", "/api/v1/auth/login", None, Some(json!({"email":"ghost@acme.com","password":PW}))).await;
    assert_eq!(wrong.0, StatusCode::UNAUTHORIZED);
    assert_eq!(wrong.1["error"]["message"], unknown.1["error"]["message"]);

    let (s, login) = call(&app, "POST", "/api/v1/auth/login", None, Some(json!({"email":" PRIYA@acme.com ","password":PW}))).await;
    assert_eq!(s, StatusCode::OK);
    let token2 = login["token"].as_str().unwrap().to_string();
    assert_ne!(token, token2);

    // Logout revokes only that session.
    assert_eq!(call(&app, "POST", "/api/v1/auth/logout", Some(&token2), None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(call(&app, "GET", "/api/v1/me", Some(&token2), None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&app, "GET", "/api/v1/me", Some(&token), None).await.0, StatusCode::OK);
}

#[tokio::test]
async fn login_is_rate_limited_per_email() {
    let (app, _d) = make_app(|_| {}).await;
    call(&app, "POST", "/api/v1/auth/signup", None, Some(json!({"email":"a@acme.com","password":PW}))).await;
    for _ in 0..10 {
        call(&app, "POST", "/api/v1/auth/login", None, Some(json!({"email":"a@acme.com","password":"wrong password!"}))).await;
    }
    let (s, body) = call(&app, "POST", "/api/v1/auth/login", None, Some(json!({"email":"a@acme.com","password":PW}))).await;
    assert_eq!((s, body["error"]["code"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("rate_limited")));
}

#[tokio::test]
async fn signup_respects_allow_list_and_switch() {
    let (app, _d) = make_app(|c| c.signup_allowed_domains = vec!["acme.com".into()]).await;
    assert_eq!(call(&app, "POST", "/api/v1/auth/signup", None, Some(json!({"email":"x@evil.com","password":PW}))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&app, "POST", "/api/v1/auth/signup", None, Some(json!({"email":"x@acme.com","password":PW}))).await.0, StatusCode::CREATED);

    let (app, _d) = make_app(|c| c.signup_enabled = false).await;
    assert_eq!(call(&app, "POST", "/api/v1/auth/signup", None, Some(json!({"email":"x@acme.com","password":PW}))).await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn email_verification_with_otp() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()));
    config.auth_mode = AuthMode::Password;
    let mut state = build_state(config).await.unwrap();
    db::migrate(&state.db).await.unwrap();
    let mailbox = Arc::new(MemoryMailer::default());
    state.mailer = Some(mailbox.clone());
    let app = router(state.clone());
    let last_code = || {
        let sent = mailbox.sent.lock().unwrap();
        let text = &sent.last().unwrap().2;
        text.split_whitespace().find(|w| w.trim_end_matches('.').len() == 6 && w.trim_end_matches('.').chars().all(|c| c.is_ascii_digit()))
            .unwrap().trim_end_matches('.').to_string()
    };

    assert_eq!(call(&app, "GET", "/api/v1/auth/config", None, None).await.1["email_verification"], true);
    let (s, body) = call(&app, "POST", "/api/v1/auth/signup", None, Some(json!({"email":"meera@acme.com","password":PW}))).await;
    assert_eq!((s, body["verification_required"].as_bool()), (StatusCode::ACCEPTED, Some(true)));
    assert!(body.get("token").is_none(), "no session before verification");
    assert_eq!(mailbox.sent.lock().unwrap()[0].0, "meera@acme.com");
    let code = last_code();

    // Login before verifying: right password, but blocked with a dedicated code.
    let (s, err) = call(&app, "POST", "/api/v1/auth/login", None, Some(json!({"email":"meera@acme.com","password":PW}))).await;
    assert_eq!((s, err["error"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("email_not_verified")));
    assert_eq!(mailbox.sent.lock().unwrap().len(), 1, "no second code within 60 s");

    // Wrong code, then the right one signs the user in.
    let wrong = if code == "000000" { "111111" } else { "000000" };
    assert_eq!(call(&app, "POST", "/api/v1/auth/verify", None, Some(json!({"email":"meera@acme.com","code":wrong}))).await.0, StatusCode::BAD_REQUEST);
    let (s, session) = call(&app, "POST", "/api/v1/auth/verify", None, Some(json!({"email":"meera@acme.com","code":code}))).await;
    assert_eq!(s, StatusCode::OK);
    let token = session["token"].as_str().unwrap().to_string();
    assert_eq!(call(&app, "GET", "/api/v1/me", Some(&token), None).await.0, StatusCode::OK);
    // Codes are single use; login now works.
    assert_eq!(call(&app, "POST", "/api/v1/auth/verify", None, Some(json!({"email":"meera@acme.com","code":code}))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&app, "POST", "/api/v1/auth/login", None, Some(json!({"email":"meera@acme.com","password":PW}))).await.0, StatusCode::OK);

    // Resend never reveals whether an account exists.
    assert_eq!(call(&app, "POST", "/api/v1/auth/resend-code", None, Some(json!({"email":"ghost@acme.com"}))).await.0, StatusCode::ACCEPTED);
}

#[tokio::test]
async fn otp_attempts_are_capped() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()));
    config.auth_mode = AuthMode::Password;
    let mut state = build_state(config).await.unwrap();
    db::migrate(&state.db).await.unwrap();
    let mailbox = Arc::new(MemoryMailer::default());
    state.mailer = Some(mailbox.clone());
    let app = router(state);
    call(&app, "POST", "/api/v1/auth/signup", None, Some(json!({"email":"x@acme.com","password":PW}))).await;
    let text = mailbox.sent.lock().unwrap()[0].2.clone();
    let code: String = text.chars().filter(|c| c.is_ascii_digit()).take(6).collect();
    let wrong = if code == "000000" { "111111" } else { "000000" };
    for _ in 0..5 {
        call(&app, "POST", "/api/v1/auth/verify", None, Some(json!({"email":"x@acme.com","code":wrong}))).await;
    }
    // The real code no longer works after 5 wrong attempts.
    let (s, _) = call(&app, "POST", "/api/v1/auth/verify", None, Some(json!({"email":"x@acme.com","code":code}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}
