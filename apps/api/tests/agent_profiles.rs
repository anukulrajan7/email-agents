//! The workspace agent and people's profiles: seeding, editing and roles, the agent adapting to
//! the mailbox owner (planner and drafter prompts), jobs gating cases, sending on its own vs
//! asking, suggested automations, and "Test on an email".

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::{
    Json, Router,
    body::Body,
    extract::Path,
    http::{Request, StatusCode},
    routing::{get, post, put},
};
use email_ai_api::{
    agents::planner::{SYSTEM_PROMPT, Scripted, ScriptedPlanner, call as tool, finish},
    ai::{AiError, DraftInput, DraftOutput, Drafter, routing::DEFAULT_REPLY_STYLE},
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
const OPERATOR: &str = "dev:priya@acme.test";
const CLIENT: &str = "ravi@client.com";
const REPLY: &str = "Hello Ravi,\n\nThanks for the update, noted.\n\nPriya";

type Calls = Arc<Mutex<Vec<(String, Value)>>>;

#[derive(Default)]
struct Recorder {
    seen: Mutex<Vec<DraftInput>>,
}

#[async_trait]
impl Drafter for Recorder {
    async fn draft(&self, input: &DraftInput) -> Result<DraftOutput, AiError> {
        self.seen.lock().unwrap().push(input.clone());
        Ok(DraftOutput { body_text: REPLY.into(), model_id: "recorder".into(), unanswered: vec![] })
    }
}

async fn spawn_gmail(calls: Calls) -> String {
    let (c1, c2, c3, c4) = (calls.clone(), calls.clone(), calls.clone(), calls);
    let app = Router::new()
        .route("/proxy/gmail/v1/users/me/profile", get(|| async { Json(json!({"emailAddress": "priya@acme.com"})) }))
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
    priya: String,
    calls: Calls,
    planner: Arc<ScriptedPlanner>,
    recorder: Arc<Recorder>,
    _dir: tempfile::TempDir,
}

impl Env {
    fn base(&self) -> String {
        format!("/api/v1/workspaces/{}", self.ws)
    }

    async fn req(&self, method: &str, path: &str, token: &str, body: Option<Value>) -> (StatusCode, Value) {
        call(&self.app, method, &format!("{}{path}", self.base()), token, body).await
    }

    async fn ok(&self, method: &str, path: &str, token: &str, body: Option<Value>) -> Value {
        let (s, v) = self.req(method, path, token, body).await;
        assert_eq!(s, StatusCode::OK, "{method} {path}: {v}");
        v
    }

    /// Stores an email as if Gmail synced it into Priya's mailbox, then runs the pipeline.
    async fn receive(&self, gmail_id: &str, thread: &str, from: &str, subject: &str, body: &str) -> String {
        let email = NormalizedEmail {
            source_message_id: gmail_id.into(),
            rfc_message_id: Some(format!("<{gmail_id}@mail.example>")),
            sender_address: from.into(),
            recipients: vec!["priya@acme.com".into()],
            subject: subject.into(),
            body_text: body.into(),
            received_at: db::now_ms() - 3_600_000,
            ..Default::default()
        };
        let mut tx = self.state.db.begin().await.unwrap();
        let id = ingest::persist_message(&mut tx, &self.src, &self.ws, &email, Some(thread), json!({})).await.unwrap().unwrap();
        tx.commit().await.unwrap();
        worker::drain(&self.state).await;
        id
    }

    /// We replied to Ravi before, so he is a known contact.
    async fn known_contact(&self) {
        let earlier = self.receive("old1", "t-old", CLIENT, "Earlier question", "Hello, quick question from last month.").await;
        sqlx::query(
            "INSERT INTO drafts (id, workspace_id, message_id, body_text, status, created_at, updated_at)
             VALUES ('drf_old', ?1, ?2, 'Answered.', 'sent', ?3, ?3)",
        )
        .bind(&self.ws)
        .bind(&earlier)
        .bind(db::now_ms())
        .execute(&self.state.db)
        .await
        .unwrap();
    }

    async fn start_inbox(&self, message_id: &str) -> Value {
        let (s, c) = self.req("POST", "/cases", ADMIN, Some(json!({"kind": "inbox", "message_id": message_id}))).await;
        assert_eq!(s, StatusCode::OK, "{c}");
        worker::drain(&self.state).await;
        self.ok("GET", &format!("/cases/{}", c["id"].as_str().unwrap()), ADMIN, None).await
    }

    fn sends(&self) -> Vec<Value> {
        self.calls.lock().unwrap().iter().filter(|(k, _)| k == "send_message" || k == "send_draft").map(|(_, b)| b.clone()).collect()
    }

    async fn audited(&self, action: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE workspace_id = ?1 AND action = ?2")
            .bind(&self.ws)
            .bind(action)
            .fetch_one(&self.state.db)
            .await
            .unwrap()
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
    // The AI send review runs (and passes) as it would in production; automated sends are held without it.
    state.judge = Arc::new(
        email_ai_api::ai::judge::FixedJudge::new()
            .with("promises", email_ai_api::ai::judge::Verdict::YesNo(false), "")
            .with("professional", email_ai_api::ai::judge::Verdict::YesNo(true), "")
            .with("answers_ask", email_ai_api::ai::judge::Verdict::Score(0.9), ""),
    );
    let recorder = Arc::new(Recorder::default());
    state.drafter = recorder.clone();
    let app = router(state.clone());
    let (_, ws) = call(&app, "POST", "/api/v1/workspaces", ADMIN, Some(json!({"name": "Acme", "template": "hr"}))).await;
    let ws = ws["id"].as_str().unwrap().to_string();
    let (_, me) = call(&app, "GET", "/api/v1/me", OPERATOR, None).await;
    let priya = me["user"]["id"].as_str().unwrap().to_string();
    let (s, _) = call(&app, "POST", &format!("/api/v1/workspaces/{ws}/members"), ADMIN, Some(json!({"email": "priya@acme.test", "role": "operator"}))).await;
    assert_eq!(s, StatusCode::OK);
    // Automatic drafts and cases off: tests start cases themselves.
    let settings = json!({"auto_reply": false, "reply_instructions": "", "signature": null});
    call(&app, "PUT", &format!("/api/v1/workspaces/{ws}/reply-settings"), ADMIN, Some(settings)).await;
    sqlx::query("UPDATE agents SET status = 'paused', paused_reason = 'user' WHERE workspace_id = ?1 AND template_key IN ('general.inbox', 'personal.mailbox')")
        .bind(&ws)
        .execute(&state.db)
        .await
        .unwrap();
    let src = "src_priya".to_string();
    sqlx::query(
        "INSERT INTO email_sources (id, workspace_id, kind, status, external_connection_id, provider_config_key, capabilities_json,
                                    created_at, mailbox_address, connected_by_user_id)
         VALUES (?1, ?2, 'nango_gmail', 'connected', 'conn_priya', 'google-mail',
                 '{\"read\":true,\"sync\":true,\"provider_labels\":false,\"provider_drafts\":true,\"send\":true}', ?3, 'priya@acme.com', ?4)",
    )
    .bind(&src)
    .bind(&ws)
    .bind(db::now_ms())
    .bind(&priya)
    .execute(&state.db)
    .await
    .unwrap();
    Env { state, app, ws, src, priya, calls, planner, recorder, _dir: dir }
}

async fn env(planner: ScriptedPlanner) -> Env {
    env_with(planner, |_| {}).await
}

fn profile(send: &str) -> Value {
    json!({
        "display_name": "Priya", "job_title": "Senior Recruiter", "handles": ["candidates", "client_requirements"],
        "handles_note": "Java and React hiring for enterprise clients", "ignore_note": "Payroll questions",
        "audiences": ["clients", "candidates"], "vips": ["client.com"],
        "tone": {"formality": "friendly", "length": "short", "sign_off": "Priya S.", "language": "English"},
        "working_hours": "9:00-18:00", "timezone": "Asia/Kolkata", "never_do": "Never share salary bands",
        "approvals": {"send_email": send},
    })
}

fn reply_script() -> Vec<Scripted> {
    vec![
        tool("draft_email", json!({"message_id": "$message_id"})),
        tool("send_email", json!({"draft_id": "$draft_id"})),
        finish(None, "Replied."),
    ]
}

#[tokio::test]
async fn the_workspace_agent_is_seeded_and_only_admins_change_it() {
    let env = env(ScriptedPlanner::new()).await;
    let settings = json!({"auto_reply": false, "reply_instructions": "Always mention our 1-day turnaround.", "signature": "Acme HR"});
    env.ok("PUT", "/reply-settings", ADMIN, Some(settings)).await;

    let v = env.ok("GET", "/agent", OPERATOR, None).await;
    let agent = &v["agent"];
    assert_eq!((agent["name"].as_str(), agent["avatar"].as_str(), agent["quality"].as_str()), (Some("Meera"), Some("🌼"), Some("balanced")));
    let persona = agent["persona"].as_str().unwrap();
    assert!(persona.starts_with("I'm Meera") && persona.contains(DEFAULT_REPLY_STYLE) && persona.ends_with("Always mention our 1-day turnaround."));
    assert_eq!(agent["signature"], "Acme HR");
    assert_eq!(agent["tasks"], json!(["reply", "requirements"]));
    assert_eq!(agent["permissions"], json!({"draft": true, "send": "ask", "submit_candidate": "ask"}));
    assert_eq!(v["can_edit"], false);
    assert!(v["standard_guidance"].as_str().unwrap().contains(DEFAULT_REPLY_STYLE));
    assert_eq!(env.ok("GET", "/agent", ADMIN, None).await["can_edit"], true);
    assert_eq!(env.audited("agent.created").await, 1, "seeded once");

    assert_eq!(env.req("PUT", "/agent", OPERATOR, Some(json!({"name": "Asha"}))).await.0, StatusCode::FORBIDDEN);
    for bad in [
        json!({"name": " "}),
        json!({"quality": "premium"}),
        json!({"persona": "x".repeat(3001)}),
        json!({"tasks": ["payroll"]}),
        json!({"permissions": {"send": "always"}}),
        json!({"permissions": {"submit_candidate": "auto"}}),
        json!({"avatar": "abc"}),
        json!({"model": "some/model"}),
    ] {
        assert_eq!(env.req("PUT", "/agent", ADMIN, Some(bad.clone())).await.0, StatusCode::BAD_REQUEST, "{bad}");
    }
    let saved = env
        .ok("PUT", "/agent", ADMIN, Some(json!({"name": "Asha", "avatar": "as", "color": "plum", "quality": "best", "permissions": {"send": "auto"}})))
        .await;
    assert_eq!((saved["name"].as_str(), saved["avatar"].as_str(), saved["quality"].as_str()), (Some("Asha"), Some("AS"), Some("best")));
    assert_eq!(saved["permissions"]["send"], "auto");
    assert_eq!(saved["persona"].as_str(), Some(persona), "untouched fields stay");
    assert_eq!(env.audited("agent.updated").await, 1);
    assert!(!saved.to_string().contains("kimi") && !saved.to_string().contains("deepseek"), "no model names");
    assert_eq!(call(&env.app, "GET", &format!("{}/agent", env.base()), "dev:stranger@x.test", None).await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn people_fill_in_their_own_profile_and_admins_can_read_all() {
    let env = env(ScriptedPlanner::new()).await;
    let mine = env.ok("GET", "/me/agent-profile", OPERATOR, None).await;
    assert_eq!(mine["needs_onboarding"], true, "Priya connected a mailbox and hasn't told the agent about herself");
    assert_eq!(mine["mailboxes"], json!(["priya@acme.com"]));
    assert_eq!(mine["agent"]["name"], "Meera");
    assert_eq!(mine["profile"]["approvals"]["send_email"], "always_ask");
    let admin = env.ok("GET", "/me/agent-profile", ADMIN, None).await;
    assert_eq!(admin["needs_onboarding"], false, "no mailbox, no prompt");

    env.ok("POST", "/me/agent-profile/skip", OPERATOR, None).await;
    assert_eq!(env.ok("GET", "/me/agent-profile", OPERATOR, None).await["needs_onboarding"], false, "skipped");

    for bad in [
        json!({"tone": {"formality": "rude"}}),
        json!({"approvals": {"send_email": "never"}}),
        json!({"approvals": {"submit_candidate": "routine_ok"}}),
        json!({"vips": ["not an address"]}),
        json!({"handles": ["crypto"]}),
        json!({"user_id": "someone-else"}),
    ] {
        assert_eq!(env.req("PUT", "/me/agent-profile", OPERATOR, Some(bad.clone())).await.0, StatusCode::BAD_REQUEST, "{bad}");
    }
    let saved = env.ok("PUT", "/me/agent-profile", OPERATOR, Some(profile("routine_ok"))).await;
    assert_eq!((saved["saved"].as_bool(), saved["job_title"].as_str()), (Some(true), Some("Senior Recruiter")));
    assert_eq!(saved["tone"]["sign_off"], "Priya S.");
    assert_eq!(saved["approvals"], json!({"send_email": "routine_ok", "submit_candidate": "always_ask"}));
    assert_eq!(env.audited("user_agent_profile.updated").await, 1);

    assert_eq!(env.req("GET", "/agent/profiles", OPERATOR, None).await.0, StatusCode::FORBIDDEN);
    let all = env.ok("GET", "/agent/profiles", ADMIN, None).await;
    let items = all["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    let priya = items.iter().find(|i| i["user_id"] == env.priya.as_str()).unwrap();
    assert_eq!((priya["profile"]["display_name"].as_str(), priya["mailboxes"][0].as_str()), (Some("Priya"), Some("priya@acme.com")));
    let other = items.iter().find(|i| i["user_id"] != env.priya.as_str()).unwrap();
    assert!(other["profile"].is_null(), "the admin never filled one in");
}

#[tokio::test]
async fn the_agent_works_for_the_mailbox_owner() {
    let env = env(ScriptedPlanner::new().script("inbox", reply_script())).await;
    env.ok("PUT", "/me/agent-profile", OPERATOR, Some(profile("always_ask"))).await;
    let msg = env.receive("m1", "t-1", CLIENT, "Status update", "Hi Priya, the interview went well. Will update you.").await;
    let detail = env.start_inbox(&msg).await;

    assert_eq!(detail["agent"], json!({"name": "Meera", "avatar": "🌼", "color": "ochre"}));
    assert_eq!(detail["working_for"]["email"], "priya@acme.test");
    assert_eq!(detail["working_for"]["name"], "Priya");
    let system = env.planner.systems().first().cloned().unwrap();
    assert!(system.starts_with(SYSTEM_PROMPT), "the safety rules come first");
    assert!(system.contains("the rules win"));
    assert!(system.contains("You are Meera, the team's email agent for the mailbox priya@acme.com."));
    assert!(system.contains("You are working for Priya (priya@acme.test), Senior Recruiter."));
    assert!(system.contains("Never share salary bands") && system.contains("client.com") && system.contains("Payroll questions"));

    let seen = env.recorder.seen.lock().unwrap().last().cloned().expect("the drafter ran");
    assert!(seen.reply_instructions.starts_with("I'm Meera"), "{}", seen.reply_instructions);
    assert!(seen.reply_instructions.contains("Senior Recruiter") && seen.reply_instructions.contains("friendly, short"));
    assert_eq!(seen.signature.as_deref(), Some("Priya S."), "her sign-off");

    // Always ask: the send waits for a person, and the approval names the agent.
    assert_eq!(detail["status"], "waiting_approval");
    assert!(env.sends().is_empty());
    let approvals = env.ok("GET", "/approvals?status=pending", OPERATOR, None).await;
    assert_eq!(approvals["items"][0]["agent"]["name"], "Meera");
    assert_eq!(approvals["items"][0]["working_for"]["name"], "Priya");
    let cases = env.ok("GET", "/cases", OPERATOR, None).await;
    assert_eq!(cases["items"][0]["agent"]["name"], "Meera");
}

#[tokio::test]
async fn jobs_and_permissions_limit_what_the_agent_does() {
    let script = vec![tool("draft_email", json!({"message_id": "$message_id"})), finish(None, "Stopped.")];
    let env = env(ScriptedPlanner::new().script("inbox", script)).await;
    let msg = env.receive("m1", "t-1", CLIENT, "Status update", "Hi, the interview went well.").await;

    env.ok("PUT", "/agent", ADMIN, Some(json!({"tasks": ["requirements"]}))).await;
    let (s, v) = env.req("POST", "/cases", ADMIN, Some(json!({"kind": "inbox", "message_id": msg}))).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert!(v.to_string().contains("Meera is not set up to reply to emails"), "{v}");
    env.ok("PUT", "/agent", ADMIN, Some(json!({"tasks": ["reply"], "enabled": false}))).await;
    assert_eq!(env.req("POST", "/cases", ADMIN, Some(json!({"kind": "inbox", "message_id": msg}))).await.0, StatusCode::CONFLICT);

    // On, but not allowed to write drafts: the step is stopped and nothing is drafted.
    env.ok("PUT", "/agent", ADMIN, Some(json!({"enabled": true, "permissions": {"draft": false}}))).await;
    let detail = env.start_inbox(&msg).await;
    let blocked = detail["steps"].as_array().unwrap().iter().find(|s| s["status"] == "blocked").expect("blocked");
    assert!(blocked["summary"].as_str().unwrap().contains("Writing drafts is switched off for Meera"));
    assert!(env.recorder.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn routine_replies_go_on_their_own_only_when_everyone_allows_it() {
    // Workspace allows routine sends, Priya lets routine replies go, Ravi is a known contact.
    let env = env(ScriptedPlanner::new().script("inbox", reply_script())).await;
    env.ok("PUT", "/agent", ADMIN, Some(json!({"permissions": {"send": "auto"}}))).await;
    env.ok("PUT", "/me/agent-profile", OPERATOR, Some(profile("routine_ok"))).await;
    env.known_contact().await;
    let msg = env.receive("m1", "t-1", CLIENT, "Status update", "Hi Priya, the interview went well. Will update you.").await;
    let detail = env.start_inbox(&msg).await;
    assert_eq!(detail["status"], "done", "{detail}");
    assert_eq!(env.sends().len(), 1, "sent through the mailbox");
    let sent = detail["steps"].as_array().unwrap().iter().find(|s| s["tool"] == "send_email").unwrap();
    assert!(sent["summary"].as_str().unwrap().starts_with("Sent automatically (your profile allows routine replies)"), "{sent}");
    assert_eq!(sent["status"], "ok");
    assert!(env.ok("GET", "/approvals", ADMIN, None).await["items"].as_array().unwrap().is_empty());
    assert_eq!(env.audited("agent.sent_without_approval").await, 1);
}

#[tokio::test]
async fn sends_ask_a_person_otherwise() {
    struct Case {
        what: &'static str,
        workspace_send: &'static str,
        owner: &'static str,
        known: bool,
        cap: i64,
    }
    let cases = [
        Case { what: "workspace asks first", workspace_send: "ask", owner: "routine_ok", known: true, cap: 20 },
        Case { what: "owner always asks", workspace_send: "auto", owner: "always_ask", known: true, cap: 20 },
        Case { what: "first email to this address", workspace_send: "auto", owner: "routine_ok", known: false, cap: 20 },
        Case { what: "daily cap reached", workspace_send: "auto", owner: "routine_ok", known: true, cap: 0 },
    ];
    for c in cases {
        let env = env_with(ScriptedPlanner::new().script("inbox", reply_script()), |cfg| cfg.agent_auto_send_daily_cap = c.cap).await;
        env.ok("PUT", "/agent", ADMIN, Some(json!({"permissions": {"send": c.workspace_send}}))).await;
        env.ok("PUT", "/me/agent-profile", OPERATOR, Some(profile(c.owner))).await;
        if c.known {
            env.known_contact().await;
        }
        let msg = env.receive("m1", "t-1", CLIENT, "Status update", "Hi Priya, the interview went well.").await;
        let detail = env.start_inbox(&msg).await;
        assert_eq!(detail["status"], "waiting_approval", "{}: {detail}", c.what);
        assert!(env.sends().is_empty(), "{}", c.what);
        assert_eq!(env.ok("GET", "/approvals?status=pending", ADMIN, None).await["pending_count"], 1, "{}", c.what);
    }

    // Sending switched off: the send is stopped and nothing waits for approval.
    let env = env(ScriptedPlanner::new().script("inbox", reply_script())).await;
    env.ok("PUT", "/agent", ADMIN, Some(json!({"permissions": {"send": "never"}}))).await;
    let msg = env.receive("m1", "t-1", CLIENT, "Status update", "Hi Priya, the interview went well.").await;
    let detail = env.start_inbox(&msg).await;
    let blocked = detail["steps"].as_array().unwrap().iter().find(|s| s["status"] == "blocked").expect("blocked");
    assert!(blocked["summary"].as_str().unwrap().contains("Sending is switched off"));
    assert_eq!(env.ok("GET", "/approvals?status=pending", ADMIN, None).await["pending_count"], 0);
}

#[tokio::test]
async fn suggested_automations_need_an_admins_approval() {
    let suggestion = json!({
        "name": "Draft replies to status updates",
        "conditions": [{"field": "subject", "op": "contains", "value": "status"}],
        "actions": [{"type": "draft_reply"}],
        "reason": "Status updates come in every week.",
    });
    let script = vec![
        tool("propose_automation", json!({"name": "Broken", "actions": [{"type": "launch_rocket"}]})),
        tool("propose_automation", suggestion.clone()),
        finish(None, "Suggested an automation."),
    ];
    let env = env(ScriptedPlanner::new().script("inbox", script)).await;
    let msg = env.receive("m1", "t-1", CLIENT, "Status update", "Hi Priya, the interview went well.").await;
    let detail = env.start_inbox(&msg).await;
    assert_eq!(detail["status"], "waiting_approval", "{detail}");
    assert_eq!(detail["steps"][0]["status"], "error", "an invalid suggestion never reaches a person");

    let approvals = env.ok("GET", "/approvals?status=pending", OPERATOR, None).await;
    let a = &approvals["items"][0];
    assert_eq!(a["action"], "create_automation");
    assert_eq!(a["summary"], "Suggests an automation: Draft replies to status updates");
    assert_eq!(a["automation"]["name"], "Draft replies to status updates");
    assert_eq!(a["automation"]["actions"], json!([{"type": "draft_reply"}]));
    assert_eq!(a["body"], "Status updates come in every week.");
    let count = || async { env.ok("GET", "/automations", ADMIN, None).await["items"].as_array().unwrap().iter().filter(|x| x["name"] == "Draft replies to status updates").count() };
    assert_eq!(count().await, 0, "never active before approval");

    let id = a["id"].as_str().unwrap();
    assert_eq!(env.req("POST", &format!("/approvals/{id}"), OPERATOR, Some(json!({"decision": "approve"}))).await.0, StatusCode::FORBIDDEN);
    env.ok("POST", &format!("/approvals/{id}"), ADMIN, Some(json!({"decision": "approve"}))).await;
    assert_eq!(count().await, 1);
    let created = env.ok("GET", "/automations", ADMIN, None).await["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == "Draft replies to status updates")
        .cloned()
        .unwrap();
    assert_eq!(created["enabled"], true);
    assert!(env.sends().is_empty());
}

#[tokio::test]
async fn test_on_an_email_previews_without_saving() {
    let env = env(ScriptedPlanner::new()).await;
    env.ok("PUT", "/me/agent-profile", OPERATOR, Some(profile("always_ask"))).await;
    let msg = env.receive("m1", "t-1", CLIENT, "Status update", "Hi Priya, the interview went well.").await;
    let drafts = || async {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM drafts").fetch_one(&env.state.db).await.unwrap()
    };
    let before = drafts().await;

    let v = env.ok("POST", "/agent/preview", OPERATOR, Some(json!({"message_id": msg}))).await;
    assert_eq!((v["body"].as_str(), v["needs_reply"].as_bool()), (Some(REPLY), Some(true)));
    assert_eq!(drafts().await, before, "nothing is stored");
    assert!(env.planner.prompts().is_empty(), "no case");
    let seen = env.recorder.seen.lock().unwrap().last().cloned().unwrap();
    assert!(seen.reply_instructions.contains("Senior Recruiter"));
    assert_eq!(seen.signature.as_deref(), Some("Priya S."));

    // Unsaved changes can be tried: the admin's agent persona, and my own profile.
    let body = json!({"message_id": msg, "agent": {"persona": "Be extremely brief."}, "profile": {"tone": {"sign_off": "P."}}});
    env.ok("POST", "/agent/preview", ADMIN, Some(body.clone())).await;
    let seen = env.recorder.seen.lock().unwrap().last().cloned().unwrap();
    assert!(seen.reply_instructions.starts_with("Be extremely brief."));
    assert_eq!(seen.signature.as_deref(), Some("P."));
    assert!(env.ok("GET", "/agent", ADMIN, None).await["agent"]["persona"].as_str().unwrap().starts_with("I'm Meera"), "not saved");

    assert_eq!(env.req("POST", "/agent/preview", OPERATOR, Some(body)).await.0, StatusCode::FORBIDDEN, "only admins try agent changes");
    assert_eq!(env.req("POST", "/agent/preview", OPERATOR, Some(json!({"message_id": "msg_nope"}))).await.0, StatusCode::NOT_FOUND);
}
