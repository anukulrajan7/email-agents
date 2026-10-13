//! Agents end to end with a scripted planner: cases start on their own, work through tools,
//! wait for a person's approval, sleep, wake on a reply, and stop at their limits.
//!
//! Sending uses a Gmail source (inserted directly) whose Nango proxy is a local mock, the same
//! way `gmail_backfill.rs` mocks it, so the approved send really goes through `replies::send_draft`.

use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    body::Body,
    extract::Path,
    http::{Request, StatusCode},
    routing::{get, post, put},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use email_ai_api::{
    agents::planner::{Scripted, ScriptedPlanner, call as tool, finish},
    build_state,
    config::Config,
    db,
    domain::NormalizedEmail,
    http::{AppState, router},
    ingest,
    jobs::worker,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const ADMIN: &str = "dev:admin@acme.test";
const OPERATOR: &str = "dev:op@acme.test";
const CLIENT: &str = "hr@capgemini.com";

type Calls = Arc<Mutex<Vec<(String, Value)>>>;

async fn spawn_gmail(calls: Calls) -> String {
    let (c1, c2, c3, c4) = (calls.clone(), calls.clone(), calls.clone(), calls);
    let app = Router::new()
        .route("/proxy/gmail/v1/users/me/profile", get(|| async { Json(json!({"emailAddress": "me@acme.com"})) }))
        .route("/proxy/gmail/v1/users/me/drafts", post(move |Json(b): Json<Value>| async move {
            c1.lock().unwrap().push(("create_draft".into(), b));
            Json(json!({"id": "gdraft1"}))
        }))
        .route("/proxy/gmail/v1/users/me/drafts/{id}", put(move |Path(id): Path<String>, Json(b): Json<Value>| async move {
            c2.lock().unwrap().push((format!("update_draft:{id}"), b));
            Json(json!({"id": id}))
        }))
        .route("/proxy/gmail/v1/users/me/drafts/send", post(move |Json(b): Json<Value>| async move {
            c3.lock().unwrap().push(("send_draft".into(), b));
            Json(json!({"id": "gmsg-sent"}))
        }))
        .route("/proxy/gmail/v1/users/me/messages/send", post(move |Json(b): Json<Value>| async move {
            c4.lock().unwrap().push(("send_message".into(), b));
            Json(json!({"id": "gmsg-direct"}))
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
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

struct Env {
    state: AppState,
    app: Router,
    ws: String,
    src: String,
    calls: Calls,
    planner: Arc<ScriptedPlanner>,
    _dir: tempfile::TempDir,
}

impl Env {
    fn base(&self) -> String {
        format!("/api/v1/workspaces/{}", self.ws)
    }

    async fn get(&self, path: &str) -> Value {
        let (s, v) = call(&self.app, "GET", &format!("{}{path}", self.base()), ADMIN, None).await;
        assert_eq!(s, StatusCode::OK, "{path}: {v}");
        v
    }

    async fn post(&self, path: &str, token: &str, body: Value) -> (StatusCode, Value) {
        call(&self.app, "POST", &format!("{}{path}", self.base()), token, Some(body)).await
    }

    /// Stores an email as if Gmail synced it (thread = Gmail thread id), then runs the pipeline.
    async fn receive(&self, gmail_id: &str, thread: &str, from: &str, subject: &str, body: &str, hours_ago: i64) -> String {
        let email = NormalizedEmail {
            source_message_id: gmail_id.into(),
            rfc_message_id: Some(format!("<{gmail_id}@mail.example>")),
            sender_address: from.into(),
            recipients: vec!["me@acme.com".into()],
            subject: subject.into(),
            body_text: body.into(),
            received_at: db::now_ms() - hours_ago * 3_600_000,
            ..Default::default()
        };
        let mut tx = self.state.db.begin().await.unwrap();
        let id = ingest::persist_message(&mut tx, &self.src, &self.ws, &email, Some(thread), json!({})).await.unwrap().unwrap();
        tx.commit().await.unwrap();
        worker::drain(&self.state).await;
        id
    }

    async fn cases(&self, kind: &str) -> Vec<Value> {
        self.get(&format!("/cases?kind={kind}")).await["items"].as_array().unwrap().clone()
    }

    async fn case(&self, id: &str) -> Value {
        self.get(&format!("/cases/{id}")).await
    }

    fn sends(&self) -> Vec<Value> {
        self.calls.lock().unwrap().iter().filter(|(k, _)| k == "send_message" || k == "send_draft").map(|(_, b)| b.clone()).collect()
    }
}

async fn env_with(planner: ScriptedPlanner, tweak: impl FnOnce(&mut Config)) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let calls: Calls = Arc::default();
    let mut config = Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()));
    config.nango_api_url = spawn_gmail(calls.clone()).await;
    config.nango_api_key = "k".into();
    tweak(&mut config);
    let mut state = build_state(config).await.unwrap();
    db::migrate(&state.db).await.unwrap();
    let planner = Arc::new(planner);
    state.planner = planner.clone();
    let app = router(state.clone());
    let (_, ws) = call(&app, "POST", "/api/v1/workspaces", ADMIN, Some(json!({"name": "Acme", "template": "hr"}))).await;
    let ws = ws["id"].as_str().unwrap().to_string();
    assert_eq!(call(&app, "POST", &format!("/api/v1/workspaces/{ws}/matching/setup"), ADMIN, None).await.0, StatusCode::OK);
    sqlx::query("UPDATE agents SET status = 'paused', paused_reason = 'user' WHERE workspace_id = ?1 AND template_key IN ('general.inbox', 'personal.mailbox')")
        .bind(&ws)
        .execute(&state.db)
        .await
        .unwrap();
    let src = "src_gmail_test".to_string();
    sqlx::query(
        "INSERT INTO email_sources (id, workspace_id, kind, status, external_connection_id, provider_config_key, capabilities_json, created_at, mailbox_address)
         VALUES (?1, ?2, 'nango_gmail', 'connected', 'conn_test', 'google-mail',
                 '{\"read\":true,\"sync\":true,\"provider_labels\":false,\"provider_drafts\":true,\"send\":true}', ?3, 'me@acme.com')",
    )
    .bind(&src)
    .bind(&ws)
    .bind(db::now_ms())
    .execute(&state.db)
    .await
    .unwrap();
    Env { state, app, ws, src, calls, planner, _dir: dir }
}

async fn env(planner: ScriptedPlanner) -> Env {
    env_with(planner, |_| {}).await
}

/// Two referrals (one strong, one weak) and then the client's requirement.
async fn staffing_mail(env: &Env) -> String {
    env.receive("c1", "t-c1", "friend@gmail.com", "Referral - Rahul Verma",
        "Hi, referring my friend.\nName: Rahul Verma\n6 years in Java, SpringBoot, Microservices.\nPune. Notice period 30 days.\nrahul.v@mail.com", 1).await;
    env.receive("c2", "t-c2", "friend@gmail.com", "Referral - Asha Rao", "Name: Asha Rao\n2 years Python, Django. Chennai. Immediate joiner.", 1).await;
    env.receive("r1", "t-req", CLIENT, "Requirement: Java Developer",
        "Need 3 developers with Java, Spring Boot and Microservices. 5-8 yrs. Location Pune.", 1).await
}

fn submission_script() -> Vec<Scripted> {
    vec![
        tool("find_candidates", json!({"requirement_id": "$requirement_id"})),
        tool("recall_memory", json!({"about": CLIENT})),
        tool("draft_email", json!({"message_id": "$message_id", "instructions": "Submit Rahul Verma (match 90) for the Java role.", "quality": "premium"})),
        tool("submit_candidate", json!({"requirement_id": "$requirement_id", "candidate_ids": "$candidate_ids", "draft_id": "$draft_id"})),
    ]
}

fn kinds(detail: &Value) -> Vec<String> {
    detail["steps"].as_array().unwrap().iter().map(|s| format!("{}:{}", s["kind"].as_str().unwrap(), s["status"].as_str().unwrap())).collect()
}

#[tokio::test]
async fn requirement_case_runs_to_an_interview() {
    let mut script = submission_script();
    script.extend([
        tool("schedule_check", json!({"hours": 48, "reason": "Wait for the client's feedback"})),
        tool("read_thread", json!({"message_id": "$new_message_id"})),
        tool("update_case", json!({"status_note": "Client wants to interview Rahul", "outcome": "interview"})),
        finish(Some("interview"), "The client invited Rahul Verma to an interview."),
    ]);
    let env = env(ScriptedPlanner::new().script("requirement", script)).await;
    let req_msg = staffing_mail(&env).await;

    // The requirement record started its case automatically; it now waits for a person.
    let cases = env.cases("requirement").await;
    assert_eq!(cases.len(), 1);
    let case = &cases[0];
    let case_id = case["id"].as_str().unwrap().to_string();
    assert_eq!((case["status"].as_str(), case["steps"].as_i64(), case["max_steps"].as_i64()), (Some("waiting_approval"), Some(4), Some(15)));
    assert_eq!((case["message_id"].as_str(), case["title"].as_str()), (Some(req_msg.as_str()), Some("Java Developer at Capgemini")));
    assert!(case["goal"].as_str().unwrap().starts_with("Shortlist up to 3 candidates for Java Developer at Capgemini"));
    assert!(env.sends().is_empty(), "nothing is sent before approval");

    let approvals = env.get("/approvals?status=pending").await;
    assert_eq!(approvals["pending_count"], 1);
    let a = &approvals["items"][0];
    assert_eq!((a["action"].as_str(), a["to"].as_str(), a["subject"].as_str()), (Some("submit_candidate"), Some(CLIENT), Some("Re: Requirement: Java Developer")));
    assert_eq!(a["summary"], "Send submission of 1 candidate to hr@capgemini.com");
    assert_eq!(a["candidates"][0]["name"], "Rahul Verma");
    assert_eq!(a["candidates"][0]["score"], 90);
    assert_eq!((a["case_id"].as_str(), a["case_title"].as_str(), a["status"].as_str()), (Some(case_id.as_str()), Some("Java Developer at Capgemini"), Some("pending")));
    assert!(a["step_seq"].as_i64().is_some() && !a["body"].as_str().unwrap().is_empty());

    // An operator approves with an edit: the draft is updated, sent in the thread, the match shortlisted.
    let (_, me) = call(&env.app, "GET", "/api/v1/me", OPERATOR, None).await;
    assert!(me["user"]["id"].is_string());
    env.post("/members", ADMIN, json!({"email": "op@acme.test", "role": "operator"})).await;
    let edited = "Hello,\n\nPlease find the profile of Rahul Verma, a strong match for the Java Developer role.\n\nBest regards,\nHR Operations";
    let (s, decided) = env.post(&format!("/approvals/{}", a["id"].as_str().unwrap()), OPERATOR, json!({"decision": "approve", "edited_body": edited})).await;
    assert_eq!(s, StatusCode::OK, "{decided}");
    assert_eq!((decided["status"].as_str(), decided["decided_by"].as_str(), decided["body"].as_str()), (Some("approved"), Some("op@acme.test"), Some(edited)));
    assert!(decided["decided_at"].is_string());
    // Sends are scheduled (undo window; 0 s in tests) and go out when the worker runs.
    worker::drain(&env.state).await;
    let sends = env.sends();
    assert_eq!(sends.len(), 1);
    assert_eq!(sends[0]["threadId"], "t-req", "reply stays in the client's thread");
    let raw = String::from_utf8(URL_SAFE_NO_PAD.decode(sends[0]["raw"].as_str().unwrap()).unwrap()).unwrap();
    assert!(raw.contains("To: hr@capgemini.com") && raw.contains("Rahul Verma, a strong match"), "{raw}");
    let matches = env.get(&format!("/matching/matches?requirement_id={}", case["requirement_id"].as_str().unwrap())).await;
    assert_eq!(matches["items"][0]["status"], "shortlisted");
    assert_eq!(
        call(&env.app, "POST", &format!("{}/approvals/{}", env.base(), a["id"].as_str().unwrap()), ADMIN, Some(json!({"decision": "approve"}))).await.0,
        StatusCode::CONFLICT,
        "a decision is final"
    );

    // The case continued and went to sleep for 48 hours.
    worker::drain(&env.state).await;
    let detail = env.case(&case_id).await;
    assert_eq!(detail["status"], "waiting_event");
    let wake = db::parse_iso_ms(detail["next_wake_at"].as_str().unwrap()).unwrap();
    assert!(wake > db::now_ms() + 47 * 3_600_000);

    // The client replies in the thread: the case wakes early, reads it and records the outcome.
    let reply = env.receive("r2", "t-req", CLIENT, "Re: Requirement: Java Developer", "Thanks, we'd like to interview Rahul on Monday.", 0).await;
    let detail = env.case(&case_id).await;
    assert_eq!((detail["status"].as_str(), detail["outcome"].as_str()), (Some("done"), Some("interview")), "{:?}", kinds(&detail));
    assert_eq!(detail["state"]["new_message_id"], reply.as_str());
    assert_eq!(detail["state"]["submitted_candidate_ids"].as_array().unwrap().len(), 1);
    assert_eq!(
        kinds(&detail),
        ["tool:ok", "tool:ok", "tool:ok", "approval:waiting", "approval:ok", "tool:ok", "tool:ok", "tool:ok", "finish:ok"]
    );
    let steps = detail["steps"].as_array().unwrap();
    assert_eq!(steps[0]["summary"], "Found 1 candidate (best 90)");
    assert_eq!(steps[3]["summary"], "Waiting for approval: Send submission of 1 candidate to hr@capgemini.com");
    assert!(steps[4]["summary"].as_str().unwrap().starts_with("Approved by a person. Submitted 1 candidate to hr@capgemini.com"));
    assert!(steps[6]["summary"].as_str().unwrap().starts_with("Read the conversation"));
    assert_eq!(detail["approvals"].as_array().unwrap().len(), 1);
    assert!(!detail.to_string().contains("scripted"), "no model names in the API");
    assert!(env.cases("inbox").await.iter().all(|c| c["message_id"] != reply.as_str()), "the reply belongs to the requirement case");
    assert_eq!(env.cases("requirement").await.len(), 1, "a reply in the thread doesn't start a second requirement case");

    let audited: Vec<String> = sqlx::query_scalar("SELECT action FROM audit_events WHERE entity_type IN ('case', 'agent_approval') ORDER BY created_at")
        .fetch_all(&env.state.db)
        .await
        .unwrap();
    for action in ["case.started", "agent_approval.requested", "agent_approval.approved", "case.finished"] {
        assert!(audited.iter().any(|a| a == action), "{action} in {audited:?}");
    }
}

#[tokio::test]
async fn declined_submission_is_never_retried() {
    let mut script = submission_script();
    script.extend([
        tool("submit_candidate", json!({"requirement_id": "$requirement_id", "candidate_ids": "$candidate_ids", "draft_id": "$draft_id"})),
        tool("create_task", json!({"title": "Share our rates with the client before submitting"})),
        finish(None, "A person will share rates before any submission."),
    ]);
    let env = env(ScriptedPlanner::new().script("requirement", script)).await;
    staffing_mail(&env).await;
    let approvals = env.get("/approvals?status=pending").await;
    let id = approvals["items"][0]["id"].as_str().unwrap().to_string();

    let (s, declined) = env.post(&format!("/approvals/{id}"), ADMIN, json!({"decision": "decline", "note": "Client wants rates first"})).await;
    assert_eq!((s, declined["status"].as_str()), (StatusCode::OK, Some("declined")));
    worker::drain(&env.state).await;

    let case = &env.cases("requirement").await[0];
    let detail = env.case(case["id"].as_str().unwrap()).await;
    assert_eq!(detail["status"], "done");
    assert!(env.sends().is_empty(), "nothing was sent");
    let steps = detail["steps"].as_array().unwrap();
    let note = steps.iter().find(|s| s["kind"] == "note").unwrap();
    assert!(note["summary"].as_str().unwrap().contains("A person declined") && note["summary"].as_str().unwrap().contains("Client wants rates first"));
    let blocked = steps.iter().find(|s| s["status"] == "blocked").expect("the repeated send was blocked");
    assert_eq!((blocked["kind"].as_str(), blocked["tool"].as_str()), (Some("critic"), Some("submit_candidate")));
    assert!(blocked["summary"].as_str().unwrap().contains("declined"));
    assert_eq!(env.get("/approvals").await["items"].as_array().unwrap().len(), 1, "no second approval");
    assert!(env.planner.prompts().last().unwrap().contains("Client wants rates first"), "the planner saw the decline");
    let matches = env.get(&format!("/matching/matches?requirement_id={}", case["requirement_id"].as_str().unwrap())).await;
    assert_eq!(matches["items"][0]["status"], "suggested");
    let tasks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM review_items WHERE kind = 'agent_task'").fetch_one(&env.state.db).await.unwrap();
    assert_eq!(tasks, 1);
}

async fn requirement_only(env: &Env) -> Value {
    env.receive("r1", "t-req", CLIENT, "Requirement: Java Developer", "Need 3 developers with Java and Spring Boot. Location Pune.", 1).await;
    let cases = env.cases("requirement").await;
    assert_eq!(cases.len(), 1);
    env.case(cases[0]["id"].as_str().unwrap()).await
}

#[tokio::test]
async fn budget_stops_a_case() {
    let searches = (0..10).map(|_| tool("search_mail", json!({"query": "java developer"}))).collect();
    let env = env(ScriptedPlanner::new().script("requirement", searches).with_cost(0.04)).await;
    let detail = requirement_only(&env).await;
    assert_eq!((detail["status"].as_str(), detail["steps"].as_array().map(Vec::len)), (Some("failed"), Some(4)));
    assert!(detail["failure_reason"].as_str().unwrap().contains("budget of $0.10"));
    assert!((detail["spent_usd"].as_f64().unwrap() - 0.12).abs() < 1e-9);
    let costs: f64 = detail["steps"].as_array().unwrap().iter().map(|s| s["cost_usd"].as_f64().unwrap()).sum();
    assert!((costs - 0.12).abs() < 1e-9, "every turn's cost is on its step");
}

#[tokio::test]
async fn step_limit_and_repeated_errors_stop_a_case() {
    let searches = (0..10).map(|_| tool("search_mail", json!({"query": "java"}))).collect();
    let env = env_with(ScriptedPlanner::new().script("requirement", searches), |c| c.agent_max_steps = 2).await;
    let detail = requirement_only(&env).await;
    assert_eq!(detail["status"], "failed");
    assert!(detail["failure_reason"].as_str().unwrap().contains("limit of 2 steps"));
    let summary = &env.cases("requirement").await[0];
    assert_eq!((summary["steps"].as_i64(), summary["max_steps"].as_i64()), (Some(2), Some(2)));

    let bad = vec![
        Scripted::Invalid("not json".into()),
        tool("launch_rocket", json!({})),
        tool("find_candidates", json!({"limit": 3})),
        tool("search_mail", json!({"query": "never reached"})),
    ];
    let env = env_with(ScriptedPlanner::new().script("requirement", bad), |_| {}).await;
    let detail = requirement_only(&env).await;
    assert_eq!(detail["status"], "failed");
    assert!(detail["failure_reason"].as_str().unwrap().contains("3 steps in a row failed"));
    assert_eq!(kinds(&detail), ["note:error", "note:error", "tool:error", "note:error"]);
    assert!(env.planner.prompts()[1].contains("unusable choice"), "the planner sees its own error");
}

#[tokio::test]
async fn one_case_per_requirement_and_message() {
    let env = env(ScriptedPlanner::new()).await;
    let detail = requirement_only(&env).await;
    let case_id = detail["id"].as_str().unwrap();
    let msg = detail["message_id"].as_str().unwrap();
    call(&env.app, "POST", &format!("{}/messages/{msg}/reprocess", env.base()), ADMIN, None).await;
    worker::drain(&env.state).await;
    assert_eq!(env.cases("requirement").await.len(), 1, "re-matching doesn't duplicate the case");
    let (s, again) = env.post("/cases", ADMIN, json!({"kind": "requirement", "requirement_id": detail["requirement_id"]})).await;
    assert_eq!((s, again["id"].as_str()), (StatusCode::OK, Some(case_id)));
    let (s1, first) = env.post("/cases", ADMIN, json!({"kind": "inbox", "message_id": msg})).await;
    let (s2, second) = env.post("/cases", ADMIN, json!({"kind": "inbox", "message_id": msg})).await;
    assert_eq!((s1, s2), (StatusCode::OK, StatusCode::OK));
    assert_eq!(first["id"], second["id"]);
    assert_eq!(first["kind"], "inbox");
}

#[tokio::test]
async fn inbox_agent_starts_only_when_auto_reply_is_on() {
    let script = vec![
        tool("read_email", json!({"message_id": "$message_id"})),
        tool("draft_email", json!({"message_id": "$message_id"})),
        tool("send_email", json!({"draft_id": "$draft_id"})),
        tool("create_task", json!({"title": "Answer the open questions in the draft"})),
        finish(None, "Drafted a reply; a person must fill in the missing details."),
    ];
    let env = env(ScriptedPlanner::new().script("inbox", script)).await;
    let settings = |on: bool| json!({"auto_reply": on, "reply_instructions": "", "signature": null});
    call(&env.app, "PUT", &format!("{}/reply-settings", env.base()), ADMIN, Some(settings(false))).await;
    let off = env.receive("l1", "t-l1", "priya@acme.com", "Leave request", "Can I take annual leave next Friday?", 1).await;
    assert!(env.cases("inbox").await.is_empty(), "auto-reply off: no automatic case");

    call(&env.app, "PUT", &format!("{}/reply-settings", env.base()), ADMIN, Some(settings(true))).await;
    env.receive("old", "t-old", "sam@acme.com", "Leave request", "Can I take annual leave in May?", 100).await;
    assert!(env.cases("inbox").await.is_empty(), "old mail never starts a case");
    let on = env.receive("l2", "t-l2", "ravi@acme.com", "Leave request", "Can I take annual leave on Monday?", 1).await;
    let cases = env.cases("inbox").await;
    assert_eq!(cases.len(), 1, "{cases:?}");
    assert_eq!(cases[0]["message_id"], on.as_str());
    let detail = env.case(cases[0]["id"].as_str().unwrap()).await;
    assert_eq!(detail["status"], "done");
    let blocked = detail["steps"].as_array().unwrap().iter().find(|s| s["status"] == "blocked").expect("send with gaps blocked");
    assert!(blocked["summary"].as_str().unwrap().contains("[NEEDS"));
    assert_eq!(env.get("/approvals").await["items"].as_array().unwrap().len(), 0);
    assert!(env.sends().is_empty());

    // A person can still hand an email to the agent while auto-reply is off.
    call(&env.app, "PUT", &format!("{}/reply-settings", env.base()), ADMIN, Some(settings(false))).await;
    let (s, manual) = env.post("/cases", ADMIN, json!({"kind": "inbox", "message_id": off})).await;
    assert_eq!((s, manual["status"].as_str()), (StatusCode::OK, Some("running")));
    worker::drain(&env.state).await;
    assert_eq!(env.case(manual["id"].as_str().unwrap()).await["status"], "done");
}

#[tokio::test]
async fn roles_and_tenant_isolation() {
    let env = env(ScriptedPlanner::new().script("requirement", submission_script())).await;
    staffing_mail(&env).await;
    let case_id = env.cases("requirement").await[0]["id"].as_str().unwrap().to_string();
    let approval_id = env.get("/approvals?status=pending").await["items"][0]["id"].as_str().unwrap().to_string();
    let base = env.base();
    let app = &env.app;

    call(app, "GET", "/api/v1/me", OPERATOR, None).await;
    env.post("/members", ADMIN, json!({"email": "op@acme.test", "role": "operator"})).await;
    assert_eq!(call(app, "GET", &format!("{base}/cases"), OPERATOR, None).await.0, StatusCode::OK);
    assert_eq!(call(app, "GET", &format!("{base}/cases/{case_id}"), OPERATOR, None).await.0, StatusCode::OK);
    assert_eq!(call(app, "GET", &format!("{base}/approvals?status=pending"), OPERATOR, None).await.0, StatusCode::OK);
    assert_eq!(call(app, "POST", &format!("{base}/cases/{case_id}/cancel"), OPERATOR, None).await.0, StatusCode::FORBIDDEN);
    for path in ["cases", "approvals"] {
        assert_eq!(call(app, "GET", &format!("{base}/{path}"), "dev:out@sider.test", None).await.0, StatusCode::FORBIDDEN);
    }
    assert_eq!(call(app, "GET", &format!("{base}/cases?status=sleeping"), ADMIN, None).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(app, "GET", &format!("{base}/approvals?status=maybe"), ADMIN, None).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(env.post("/cases", ADMIN, json!({"kind": "other"})).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(env.post("/cases", ADMIN, json!({"kind": "requirement", "requirement_id": "req_nope"})).await.0, StatusCode::NOT_FOUND);
    assert_eq!(env.post(&format!("/approvals/{approval_id}"), ADMIN, json!({"decision": "maybe"})).await.0, StatusCode::BAD_REQUEST);

    // Another workspace can't see or decide this one's cases and approvals.
    let (_, ws2) = call(app, "POST", "/api/v1/workspaces", ADMIN, Some(json!({"name": "Other"}))).await;
    let ws2 = ws2["id"].as_str().unwrap();
    assert_eq!(call(app, "GET", &format!("/api/v1/workspaces/{ws2}/cases/{case_id}"), ADMIN, None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        call(app, "POST", &format!("/api/v1/workspaces/{ws2}/approvals/{approval_id}"), ADMIN, Some(json!({"decision": "approve"}))).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(call(app, "POST", &format!("/api/v1/workspaces/{ws2}/cases/{case_id}/cancel"), ADMIN, None).await.0, StatusCode::NOT_FOUND);
    assert!(call(app, "GET", &format!("/api/v1/workspaces/{ws2}/cases"), ADMIN, None).await.1["items"].as_array().unwrap().is_empty());

    // An admin cancels: the pending approval is closed and nothing is sent.
    let (s, cancelled) = call(app, "POST", &format!("{base}/cases/{case_id}/cancel"), ADMIN, None).await;
    assert_eq!((s, cancelled["status"].as_str()), (StatusCode::OK, Some("cancelled")));
    assert_eq!(env.get("/approvals?status=pending").await["pending_count"], 0);
    assert_eq!(call(app, "POST", &format!("{base}/cases/{case_id}/cancel"), ADMIN, None).await.0, StatusCode::CONFLICT);
    assert_eq!(env.post(&format!("/approvals/{approval_id}"), OPERATOR, json!({"decision": "approve"})).await.0, StatusCode::CONFLICT);
    assert!(env.sends().is_empty());
}

#[tokio::test]
async fn agents_are_off_without_a_planner_model() {
    let dir = tempfile::tempdir().unwrap();
    let state = build_state(Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()))).await.unwrap();
    db::migrate(&state.db).await.unwrap();
    let app = router(state.clone());
    let (_, ws) = call(&app, "POST", "/api/v1/workspaces", ADMIN, Some(json!({"name": "Acme", "template": "hr"}))).await;
    let ws = ws["id"].as_str().unwrap();
    let (_, src) = call(&app, "POST", &format!("/api/v1/workspaces/{ws}/sources/forwarding"), ADMIN, None).await;
    call(&app, "POST", &format!("/api/v1/workspaces/{ws}/sources/{}/test-email", src["id"].as_str().unwrap()), ADMIN,
        Some(json!({"from": "Recruiter <hr@capgemini.com>", "subject": "Requirement: Java Developer", "body_text": "Need Java developers in Pune."}))).await;
    worker::drain(&state).await;
    assert!(call(&app, "GET", &format!("/api/v1/workspaces/{ws}/cases"), ADMIN, None).await.1["items"].as_array().unwrap().is_empty());
    let (_, msgs) = call(&app, "GET", &format!("/api/v1/workspaces/{ws}/messages"), ADMIN, None).await;
    let msg = msgs["items"][0]["id"].as_str().unwrap();
    let (s, _) = call(&app, "POST", &format!("/api/v1/workspaces/{ws}/cases"), ADMIN, Some(json!({"kind": "inbox", "message_id": msg}))).await;
    assert_eq!(s, StatusCode::CONFLICT);
}
