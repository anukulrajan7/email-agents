//! Shared harness for the agent registry tests: a workspace with an admin and an operator, a shared
//! mailbox and the operator's own mailbox (a local Gmail mock takes the sends), a scripted planner
//! and a drafter that returns a chosen body.

#![allow(dead_code)]

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use axum::{
    Json, Router,
    body::Body,
    extract::Path,
    http::{Request, StatusCode},
    routing::{get, post, put},
};
use email_ai_api::{
    agents::{
        critic::{Critic, CriticInput, Verdict as CriticVerdict},
        planner::ScriptedPlanner,
    },
    ai::{
        AiError, DraftInput, DraftOutput, Drafter,
        judge::{Answers, FixedJudge, Judge, Question, Verdict},
    },
    build_state,
    platform::config::Config,
    platform::db,
    platform::domain::NormalizedEmail,
    http::{AppState, router},
    mail::ingest,
    jobs::worker,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

pub const ADMIN: &str = "dev:admin@acme.test";
pub const OPERATOR: &str = "dev:priya@acme.test";
pub const OUTSIDER: &str = "dev:outsider@other.test";
pub const CLIENT: &str = "ravi@client.com";
pub const SAFE_REPLY: &str = "Hello Ravi,\n\nThanks for the update, noted.\n\nPriya";

pub type Calls = Arc<Mutex<Vec<(String, Value)>>>;

/// Drafts whatever body the test set, and remembers what it was asked.
pub struct BodyDrafter {
    pub body: Mutex<String>,
    pub seen: Mutex<Vec<DraftInput>>,
}

impl BodyDrafter {
    pub fn set(&self, body: &str) {
        *self.body.lock().unwrap() = body.into();
    }
}

#[async_trait]
impl Drafter for BodyDrafter {
    async fn draft(&self, input: &DraftInput) -> Result<DraftOutput, AiError> {
        self.seen.lock().unwrap().push(input.clone());
        Ok(DraftOutput { body_text: self.body.lock().unwrap().clone(), model_id: "test-drafter".into(), unanswered: vec![] })
    }
}

pub async fn spawn_gmail(calls: Calls) -> String {
    let (c1, c2, c3, c4, c5) = (calls.clone(), calls.clone(), calls.clone(), calls.clone(), calls);
    let threads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let app = Router::new()
        .route("/proxy/gmail/v1/users/me/profile", get(|| async { Json(json!({"emailAddress": "priya@acme.com"})) }))
        .route("/proxy/gmail/v1/users/me/drafts", post(move |Json(b): Json<Value>| async move {
            c1.lock().unwrap().push(("create_draft".into(), b));
            Json(json!({"id": "gdraft1"}))
        }))
        .route("/proxy/gmail/v1/users/me/drafts/{id}", put(move |Path(id): Path<String>, Json(b): Json<Value>| async move {
            c2.lock().unwrap().push((format!("update_draft:{id}"), b));
            Json(json!({"id": id}))
        }).delete(move |Path(id): Path<String>| async move {
            c5.lock().unwrap().push((format!("delete_draft:{id}"), Value::Null));
            Json(json!({}))
        }))
        .route("/proxy/gmail/v1/users/me/drafts/send", post(move |Json(b): Json<Value>| async move {
            c3.lock().unwrap().push(("send_draft".into(), b));
            Json(json!({"id": "gmsg-sent"}))
        }))
        .route("/proxy/gmail/v1/users/me/messages/send", post(move |Json(b): Json<Value>| async move {
            c4.lock().unwrap().push(("send_message".into(), b));
            let n = threads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Json(json!({"id": format!("gmsg-direct-{n}"), "threadId": format!("gthread-{n}")}))
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

pub async fn call(app: &Router, method: &str, path: &str, token: &str, body: Option<Value>) -> (StatusCode, Value) {
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

/// One scripted result of a flaky model call.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    Ok,
    /// Retryable, like an HTTP 429.
    E429,
    /// Retryable, like an HTTP 503.
    E503,
    /// Not retryable, like an HTTP 404.
    E404,
}

/// A model that answers from a script, one result per call; the last result repeats. Records the model each call asked for
/// (`None` = the usual one, `Some(id)` = the fallback).
pub struct Flaky {
    script: Mutex<(VecDeque<Hit>, Hit)>,
    models: Mutex<Vec<Option<String>>>,
}

impl Flaky {
    pub fn new(script: &[Hit]) -> Arc<Flaky> {
        Arc::new(Flaky { script: Mutex::new((script.iter().copied().collect(), *script.last().expect("a script"))), models: Mutex::default() })
    }

    /// Replaces the script (the call record is kept).
    pub fn set(&self, script: &[Hit]) {
        *self.script.lock().unwrap() = (script.iter().copied().collect(), *script.last().expect("a script"));
    }

    pub fn calls(&self) -> usize {
        self.models.lock().unwrap().len()
    }

    pub fn models(&self) -> Vec<Option<String>> {
        self.models.lock().unwrap().clone()
    }

    fn hit(&self, model: Option<&str>) -> Result<(), AiError> {
        self.models.lock().unwrap().push(model.map(str::to_string));
        let mut script = self.script.lock().unwrap();
        let hit = script.0.pop_front().unwrap_or(script.1);
        match hit {
            Hit::Ok => Ok(()),
            Hit::E429 => Err(AiError::Retryable("OpenRouter returned 429 Too Many Requests".into())),
            Hit::E503 => Err(AiError::Retryable("OpenRouter returned 503 Service Unavailable".into())),
            Hit::E404 => Err(AiError::Permanent("OpenRouter returned 404 Not Found".into())),
        }
    }
}

/// The agent critic, failing and recovering on a script. Set it as `state.critic`.
pub struct FlakyCritic(pub Arc<Flaky>);

#[async_trait]
impl Critic for FlakyCritic {
    async fn review(&self, input: &CriticInput) -> Result<CriticVerdict, AiError> {
        self.review_with(input, None).await
    }

    async fn review_with(&self, _: &CriticInput, model: Option<&str>) -> Result<CriticVerdict, AiError> {
        self.0.hit(model)?;
        Ok(CriticVerdict::pass())
    }
}

/// The send-review judge, failing and recovering on a script, then answering like the default reviewing judge. Set it as `state.judge`.
pub struct FlakyJudge {
    pub flaky: Arc<Flaky>,
    pub inner: Arc<FixedJudge>,
}

#[async_trait]
impl Judge for FlakyJudge {
    async fn judge(&self, context: &str, questions: &[Question]) -> Result<Answers, AiError> {
        self.judge_with(context, questions, None).await
    }

    async fn judge_with(&self, context: &str, questions: &[Question], model: Option<&str>) -> Result<Answers, AiError> {
        self.flaky.hit(model)?;
        self.inner.judge(context, questions).await
    }
}

pub struct Opts {
    pub packs: Vec<&'static str>,
    pub planner: ScriptedPlanner,
    pub designer: Arc<email_ai_api::agents::designer::ScriptedDesigner>,
    pub tweak: Box<dyn FnOnce(&mut Config)>,
}

impl Opts {
    pub fn packs(packs: &[&'static str]) -> Self {
        Self { packs: packs.to_vec(), planner: ScriptedPlanner::new(), designer: Arc::default(), tweak: Box::new(|_| {}) }
    }

    pub fn planner(mut self, planner: ScriptedPlanner) -> Self {
        self.planner = planner;
        self
    }

    pub fn designer(mut self, designer: email_ai_api::agents::designer::ScriptedDesigner) -> Self {
        self.designer = Arc::new(designer);
        self
    }

    pub fn config(mut self, tweak: impl FnOnce(&mut Config) + 'static) -> Self {
        self.tweak = Box::new(tweak);
        self
    }

    /// Config-level flag defaults (workspaces without their own row get these).
    pub fn flags(self, defaults: &[(email_ai_api::platform::flags::Key, email_ai_api::platform::flags::Mode)]) -> Self {
        let defaults = defaults.to_vec();
        self.config(move |c| c.flag_defaults = defaults)
    }
}

pub struct Env {
    pub state: AppState,
    pub app: Router,
    pub ws: String,
    /// A mailbox nobody owns (a forwarding address).
    pub shared_src: String,
    /// The operator's own mailbox.
    pub op_src: String,
    pub admin_id: String,
    pub op_id: String,
    pub calls: Calls,
    pub planner: Arc<ScriptedPlanner>,
    pub designer: Arc<email_ai_api::agents::designer::ScriptedDesigner>,
    pub drafter: Arc<BodyDrafter>,
    pub judge: Arc<FixedJudge>,
    pub dir: tempfile::TempDir,
}

impl Env {
    pub async fn new(opts: Opts) -> Env {
        let dir = tempfile::tempdir().unwrap();
        let calls: Calls = Arc::default();
        let mut config = Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()));
        config.nango_api_url = spawn_gmail(calls.clone()).await;
        config.nango_api_key = "k".into();
        (opts.tweak)(&mut config);
        let mut state = build_state(config).await.unwrap();
        db::migrate(&state.db).await.unwrap();
        let planner = Arc::new(opts.planner);
        state.planner = planner.clone();
        let designer = opts.designer;
        state.designer = designer.clone();
        let judge = Arc::new(
            FixedJudge::new()
                .with("promises", Verdict::YesNo(false), "")
                .with("professional", Verdict::YesNo(true), "")
                .with("answers_ask", Verdict::Score(0.9), ""),
        );
        state.judge = judge.clone();
        let drafter = Arc::new(BodyDrafter { body: Mutex::new(SAFE_REPLY.into()), seen: Mutex::default() });
        state.drafter = drafter.clone();
        let app = router(state.clone());
        let (s, ws) = call(&app, "POST", "/api/v1/workspaces", ADMIN, Some(json!({"name": "Acme", "packs": opts.packs}))).await;
        assert_eq!(s, StatusCode::CREATED, "{ws}");
        let ws = ws["id"].as_str().unwrap().to_string();
        let (_, me) = call(&app, "GET", "/api/v1/me", ADMIN, None).await;
        let admin_id = me["user"]["id"].as_str().unwrap().to_string();
        let (_, me) = call(&app, "GET", "/api/v1/me", OPERATOR, None).await;
        let op_id = me["user"]["id"].as_str().unwrap().to_string();
        let (s, _) = call(&app, "POST", &format!("/api/v1/workspaces/{ws}/members"), ADMIN, Some(json!({"email": "priya@acme.test", "role": "operator"}))).await;
        assert_eq!(s, StatusCode::OK);
        let settings = json!({"auto_reply": false, "reply_instructions": "", "signature": null});
        call(&app, "PUT", &format!("/api/v1/workspaces/{ws}/reply-settings"), ADMIN, Some(settings)).await;
        let caps = "{\"read\":true,\"sync\":true,\"provider_labels\":false,\"provider_drafts\":true,\"send\":true}";
        for (src, mailbox, owner) in [("src_shared", "team@acme.com", None), ("src_priya", "priya@acme.com", Some(op_id.clone()))] {
            sqlx::query(
                "INSERT INTO email_sources (id, workspace_id, kind, status, external_connection_id, provider_config_key, capabilities_json,
                                            created_at, mailbox_address, connected_by_user_id)
                 VALUES (?1, ?2, 'nango_gmail', 'connected', ?3, 'google-mail', ?4, ?5, ?6, ?7)",
            )
            .bind(src)
            .bind(&ws)
            .bind(format!("conn_{src}"))
            .bind(caps)
            .bind(db::now_ms())
            .bind(mailbox)
            .bind(owner)
            .execute(&state.db)
            .await
            .unwrap();
        }
        Env { state, app, ws, shared_src: "src_shared".into(), op_src: "src_priya".into(), admin_id, op_id, calls, planner, designer, drafter, judge, dir }
    }

    /// Sets a flag for the test workspace as its admin.
    pub async fn flag(&self, key: email_ai_api::platform::flags::Key, mode: email_ai_api::platform::flags::Mode) {
        email_ai_api::platform::flags::set(&self.state.db, &self.state.config, &self.ws, &self.admin_id, key, mode).await.unwrap();
    }

    pub fn base(&self) -> String {
        format!("/api/v1/workspaces/{}", self.ws)
    }

    pub async fn req(&self, method: &str, path: &str, token: &str, body: Option<Value>) -> (StatusCode, Value) {
        call(&self.app, method, &format!("{}{path}", self.base()), token, body).await
    }

    pub async fn ok(&self, method: &str, path: &str, token: &str, body: Option<Value>) -> Value {
        let (s, v) = self.req(method, path, token, body).await;
        assert_eq!(s, StatusCode::OK, "{method} {path}: {v}");
        v
    }

    /// Every agent the token's user may see, flattened.
    pub async fn agents(&self, token: &str) -> Vec<Value> {
        let v = self.ok("GET", "/agents", token, None).await;
        v["categories"].as_array().unwrap().iter().flat_map(|c| c["agents"].as_array().unwrap().clone()).collect()
    }

    pub async fn agent_id(&self, token: &str, template_key: &str) -> String {
        self.agents(token).await.iter().find(|a| a["template_key"] == template_key).unwrap_or_else(|| panic!("no agent {template_key}"))["id"].as_str().unwrap().to_string()
    }

    /// The retired Inbox Assistant, installed as a custom agent: the fixed-workflow agent a few tests exercise.
    pub async fn install_inbox(&self) -> String {
        use email_ai_api::agents::registry::{
            store::{self, NewAgent},
            templates,
            versions::{self, NewVersion},
        };
        let t = templates::template(templates::INBOX_KEY).unwrap();
        let spec = t.spec();
        let mut conn = self.state.db.acquire().await.unwrap();
        let id = store::insert(
            &mut conn,
            NewAgent {
                workspace_id: &self.ws,
                key: "inbox_assistant",
                kind: "custom",
                scope: "workspace",
                owner_user_id: None,
                category: "general",
                template_key: None,
                template_version: None,
                cloned_from: None,
                name: "Inbox Assistant",
                avatar: t.avatar,
                color: t.color,
                description: &spec.description,
                status: "active",
                autonomy_level: 1,
                autonomy_cap: 1,
                created_by: None,
            },
        )
        .await
        .unwrap()
        .expect("installed once");
        versions::create(&mut conn, &self.ws, &id, &spec, NewVersion { source: "backfill", note: None, created_by: None, designer_session_id: None, activate: true })
            .await
            .unwrap();
        id
    }

    pub async fn personal_id(&self, token: &str) -> String {
        self.ok("GET", "/agents", token, None).await["me"]["id"].as_str().unwrap().to_string()
    }

    /// Stores an email as if it had synced into `src`, then runs the pipeline. The label and "needs a reply" are then set by hand.
    #[allow(clippy::too_many_arguments)]
    pub async fn receive(&self, src: &str, gmail_id: &str, thread: &str, from: &str, subject: &str, body: &str, label: &str) -> String {
        let email = NormalizedEmail {
            source_message_id: gmail_id.into(),
            rfc_message_id: Some(format!("<{gmail_id}@mail.example>")),
            sender_address: from.into(),
            recipients: vec!["team@acme.com".into()],
            subject: subject.into(),
            body_text: body.into(),
            received_at: db::now_ms() - 3_600_000,
            ..Default::default()
        };
        let mut tx = self.state.db.begin().await.unwrap();
        let id = ingest::persist_message(&mut tx, src, &self.ws, &email, Some(thread), json!({})).await.unwrap().unwrap();
        tx.commit().await.unwrap();
        worker::drain(&self.state).await;
        self.set_understanding(&id, label, true, json!([])).await;
        id
    }

    pub async fn set_understanding(&self, message_id: &str, label: &str, needs_reply: bool, asks: Value) {
        sqlx::query("UPDATE classifications SET category = ?1, requires_reply = ?2 WHERE message_id = ?3")
            .bind(label)
            .bind(needs_reply)
            .bind(message_id)
            .execute(&self.state.db)
            .await
            .unwrap();
        sqlx::query(
            "INSERT OR REPLACE INTO message_insights (message_id, summary, asks_json, deadline, needs_reply, missing_json, source, model_id, created_at)
             VALUES (?1, 'summary', ?2, NULL, ?3, '[]', 'llm', 'test', ?4)",
        )
        .bind(message_id)
        .bind(asks.to_string())
        .bind(needs_reply)
        .bind(db::now_ms())
        .execute(&self.state.db)
        .await
        .unwrap();
    }

    pub async fn route(&self, message_id: &str) -> email_ai_api::agents::registry::routing::Routed {
        email_ai_api::agents::registry::routing::route_message(&self.state.db, &self.state.config, &self.ws, message_id)
            .await
            .unwrap()
            .expect("a route")
    }

    pub async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar(sql).bind(&self.ws).fetch_one(&self.state.db).await.unwrap()
    }

    pub fn sends(&self) -> Vec<Value> {
        self.calls.lock().unwrap().iter().filter(|(k, _)| k == "send_message" || k == "send_draft").map(|(_, b)| b.clone()).collect()
    }

    /// Starts the routed agent's case the way the automatic trigger would, runs the queue and returns the case id.
    pub async fn start(&self, message_id: &str) -> String {
        let started = email_ai_api::agents::triggers::start_inbox_case(&self.state, &self.ws, message_id, None).await.unwrap();
        let email_ai_api::agents::triggers::Started::Case(id, _) = started else { panic!("case not started: {started:?}") };
        worker::drain(&self.state).await;
        id
    }

    pub async fn case(&self, token: &str, id: &str) -> Value {
        self.ok("GET", &format!("/cases/{id}"), token, None).await
    }

    /// Ids of every case (real and shadow) of one message, straight from the database.
    pub async fn cases_of(&self, message_id: &str) -> Vec<(String, Option<String>, bool)> {
        sqlx::query_as("SELECT id, agent_id, shadow FROM cases WHERE workspace_id = ?1 AND message_id = ?2 ORDER BY shadow, created_at")
            .bind(&self.ws)
            .bind(message_id)
            .fetch_all(&self.state.db)
            .await
            .unwrap()
    }

    /// (job, lower-case agent name) of every real case of one message.
    pub async fn cases_by_job(&self, message_id: &str) -> Vec<(String, String)> {
        sqlx::query_as(
            "SELECT c.job, lower(a.name) FROM cases c LEFT JOIN agents a ON a.id = c.agent_id
             WHERE c.workspace_id = ?1 AND c.message_id = ?2 AND c.shadow = 0 ORDER BY c.created_at, c.id",
        )
        .bind(&self.ws)
        .bind(message_id)
        .fetch_all(&self.state.db)
        .await
        .unwrap()
    }

    /// The message's live drafts: unsent reply drafts (watch follow-ups are outside the rule), as (id, status, body).
    pub async fn live_drafts(&self, message_id: &str) -> Vec<(String, String, String)> {
        sqlx::query_as(
            "SELECT id, status, body_text FROM drafts WHERE message_id = ?1 AND status != 'sent'
               AND (origin_key IS NULL OR substr(origin_key, 1, 6) != 'watch:') ORDER BY created_at, id",
        )
        .bind(message_id)
        .fetch_all(&self.state.db)
        .await
        .unwrap()
    }

    /// Makes every pending job that is due within `ms` due now, then runs the queue: "`ms` later".
    pub async fn run_due_after(&self, ms: i64) {
        sqlx::query("UPDATE jobs SET run_at = run_at - ?1 WHERE status = 'pending'").bind(ms).execute(&self.state.db).await.unwrap();
        worker::drain(&self.state).await;
    }

    /// Replaces the agent critic with one that follows `script`.
    pub fn flaky_critic(&mut self, script: &[Hit]) -> Arc<Flaky> {
        let flaky = Flaky::new(script);
        self.state.critic = Arc::new(FlakyCritic(flaky.clone()));
        flaky
    }

    /// Replaces the send-review judge with one that follows `script`.
    pub fn flaky_judge(&mut self, script: &[Hit]) -> Arc<Flaky> {
        let flaky = Flaky::new(script);
        self.state.judge = Arc::new(FlakyJudge { flaky: flaky.clone(), inner: self.judge.clone() });
        flaky
    }

    pub async fn set_pref(&self, sql: &str, binds: &[&str]) {
        let mut q = sqlx::query(sql);
        for b in binds {
            q = q.bind(*b);
        }
        q.execute(&self.state.db).await.unwrap();
    }
}

/// The offline safety suite for a spec, run once per process however many tests ask for the same spec.
pub async fn safety_suite(spec: &email_ai_api::agents::registry::spec::AgentSpec) -> Arc<email_ai_api::evals::safety::SuiteResult> {
    use std::{collections::HashMap, sync::OnceLock};
    use tokio::sync::OnceCell;
    type Cell = Arc<OnceCell<Arc<email_ai_api::evals::safety::SuiteResult>>>;
    static CACHE: OnceLock<Mutex<HashMap<String, Cell>>> = OnceLock::new();
    let cell: Cell = CACHE.get_or_init(Default::default).lock().unwrap().entry(serde_json::to_string(spec).unwrap()).or_default().clone();
    cell.get_or_init(|| async { Arc::new(email_ai_api::evals::safety::run_for_spec(spec).await) }).await.clone()
}

impl Env {
    /// (lower-case asking agent, kind, status) of every request filed for one message.
    pub async fn requests(&self, message_id: &str) -> Vec<(String, String, String)> {
        sqlx::query_as(
            "SELECT lower(a.name), r.kind, r.status FROM message_requests r LEFT JOIN agents a ON a.id = r.from_agent_id
             WHERE r.workspace_id = ?1 AND r.message_id = ?2 ORDER BY r.created_at, r.rowid",
        )
        .bind(&self.ws)
        .bind(message_id)
        .fetch_all(&self.state.db)
        .await
        .unwrap()
    }
}
