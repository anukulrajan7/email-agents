//! AI call ledger, daily budget and `GET /ai-usage`.

use std::sync::Arc;

use axum::{
    Json, Router,
    body::Body,
    http::{Request, StatusCode},
    routing::post,
};
use email_ai_api::{
    ai::{
        Analyst,
        embeddings::{Embedder, OpenRouterEmbedder},
        openrouter::{OpenRouterAnalyst, OpenRouterClient},
    },
    ai::ledger::{self, AiContext},
    build_state,
    platform::config::Config,
    platform::db::{self, new_id, now_ms},
    http::{AppState, router},
    jobs::worker,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const ADMIN: &str = "dev:admin@acme.test";
const OPERATOR: &str = "dev:op@acme.test";
const DAY_MS: i64 = 86_400_000;

struct Env {
    state: AppState,
    app: Router,
    ws: String,
    base: String,
    source: String,
    _dir: tempfile::TempDir,
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

async fn setup_with(tweak: impl FnOnce(&mut Config)) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()));
    tweak(&mut config);
    let state = build_state(config).await.unwrap();
    db::migrate(&state.db).await.unwrap();
    let app = router(state.clone());
    let (_, ws) = call(&app, "POST", "/api/v1/workspaces", ADMIN, Some(json!({"name":"Acme"}))).await;
    let ws = ws["id"].as_str().unwrap().to_string();
    let base = format!("/api/v1/workspaces/{ws}");
    call(&app, "GET", "/api/v1/me", OPERATOR, None).await;
    call(&app, "POST", &format!("{base}/members"), ADMIN, Some(json!({"email":"op@acme.test","role":"operator"}))).await;
    let (_, src) = call(&app, "POST", &format!("{base}/sources/forwarding"), ADMIN, None).await;
    let source = src["id"].as_str().unwrap().to_string();
    Env { state, app, ws, base, source, _dir: dir }
}

impl Env {
    async fn email(&self, subject: &str, body: &str) -> String {
        call(
            &self.app,
            "POST",
            &format!("{}/sources/{}/test-email", self.base, self.source),
            ADMIN,
            Some(json!({"subject": subject, "body_text": body})),
        )
        .await;
        worker::drain(&self.state).await;
        let (_, msgs) = call(&self.app, "GET", &format!("{}/messages", self.base), ADMIN, None).await;
        msgs["items"].as_array().unwrap().iter().find(|m| m["subject"] == subject).unwrap()["id"].as_str().unwrap().to_string()
    }

    async fn insert_call(&self, task: &str, cost: f64, created_at: i64) {
        sqlx::query(
            "INSERT INTO ai_calls (id, workspace_id, task, model, tokens_in, tokens_out, cost_usd, latency_ms, ok, created_at)
             VALUES (?1, ?2, ?3, 'm', 10, 5, ?4, 20, 1, ?5)",
        )
        .bind(new_id("aic"))
        .bind(&self.ws)
        .bind(task)
        .bind(cost)
        .bind(created_at)
        .execute(&self.state.db)
        .await
        .unwrap();
    }

    async fn insights(&self) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM message_insights").fetch_one(&self.state.db).await.unwrap()
    }

    async fn ledger(&self) -> Vec<(String, String, i64, i64, f64, bool, Option<String>, Option<String>)> {
        sqlx::query_as(
            "SELECT task, model, tokens_in, tokens_out, cost_usd, ok, workspace_id, message_id FROM ai_calls ORDER BY created_at, rowid",
        )
        .fetch_all(&self.state.db)
        .await
        .unwrap()
    }
}

/// Minimal OpenRouter stand-in. `fail` makes every call a 500.
async fn fake_openrouter(fail: bool) -> String {
    let chat = move || async move {
        if fail {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({})));
        }
        let content = json!({"summary": "Asks about a payslip", "asks": [], "deadline": null, "needs_reply": true, "missing_info": []});
        (
            StatusCode::OK,
            Json(json!({
                "model": "qwen/qwen3.8-flash",
                "choices": [{"finish_reason": "stop", "message": {"content": content.to_string()}}],
                "usage": {"prompt_tokens": 1_000, "completion_tokens": 200}
            })),
        )
    };
    let embed = move || async move {
        if fail {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({})));
        }
        (
            StatusCode::OK,
            Json(json!({"data": [{"index": 0, "embedding": vec![0.1f32; 1024]}], "usage": {"prompt_tokens": 40, "total_tokens": 40}})),
        )
    };
    let app = Router::new().route("/chat/completions", post(chat)).route("/embeddings", post(embed));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

#[tokio::test]
async fn openrouter_calls_are_recorded_with_task_tokens_cost_and_context() {
    let env = setup_with(|_| {}).await;
    let url = fake_openrouter(false).await;
    let client = OpenRouterClient::new(reqwest::Client::new(), &url, "k").with_ledger(env.state.db.clone());

    let analyst = OpenRouterAnalyst::new(client, "qwen/qwen3.8-flash");
    let ctx = AiContext { workspace_id: Some(env.ws.clone()), ..Default::default() };
    ledger::with_context(ctx, analyst.analyze("Payslip", "a@b.co", "Where is my payslip?")).await.unwrap();
    // No context (e.g. a call outside a job or request): recorded with NULL workspace.
    analyst.analyze("Payslip", "a@b.co", "Where is my payslip?").await.unwrap();

    let embedder = OpenRouterEmbedder::new(reqwest::Client::new(), &url, "k", "baai/bge-m3").with_ledger(env.state.db.clone());
    embedder.embed(&["hello".to_string()]).await.unwrap();

    let rows = env.ledger().await;
    assert_eq!(rows.len(), 3);
    let (task, model, tin, tout, cost, ok, ws, _) = &rows[0];
    assert_eq!((task.as_str(), model.as_str(), *tin, *tout, *ok), ("analyze", "qwen/qwen3.8-flash", 1_000, 200, true));
    assert!((cost - (1_000.0 * 0.15 + 200.0 * 0.47) / 1e6).abs() < 1e-12);
    assert_eq!(ws.as_deref(), Some(env.ws.as_str()));
    assert_eq!(rows[1].6, None);
    let (task, model, tin, tout, cost, ..) = &rows[2];
    assert_eq!((task.as_str(), model.as_str(), *tin, *tout), ("embed", "baai/bge-m3", 40, 0));
    assert!((cost - 40.0 * 0.01 / 1e6).abs() < 1e-15);
}

/// A chat endpoint that answers `content` with provider detail: cache reads, a billed cost, the provider.
async fn fake_detailed_openrouter(content: &'static str) -> String {
    let chat = move || async move {
        Json(json!({
            "id": "gen-abc", "model": "deepseek/deepseek-v4-flash", "provider": "DeepSeek",
            "choices": [{"finish_reason": "stop", "message": {"content": content}}],
            "usage": {"prompt_tokens": 1_000, "completion_tokens": 200, "cost": 0.00002,
                      "prompt_tokens_details": {"cached_tokens": 900, "cache_write_tokens": 0},
                      "completion_tokens_details": {"reasoning_tokens": 0}}
        }))
    };
    let app = Router::new().route("/chat/completions", post(chat));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

type DetailRow = (f64, String, i64, Option<String>, Option<String>, Option<String>, Option<String>, Option<bool>, i64, bool, Option<String>);

async fn detail_rows(env: &Env) -> Vec<DetailRow> {
    sqlx::query_as(
        "SELECT cost_usd, cost_source, cached_tokens, provider, finish_reason, generation_id, prompt_version, schema_ok, attempt, ok, error_class
         FROM ai_calls ORDER BY rowid",
    )
    .fetch_all(&env.state.db)
    .await
    .unwrap()
}

#[tokio::test]
async fn ledger_records_cache_provider_finish_reason_prompt_version_and_provider_cost() {
    let env = setup_with(|_| {}).await;
    let url = fake_detailed_openrouter(r#"{"summary":"s","asks":[],"deadline":null,"needs_reply":false,"missing_info":[]}"#).await;
    let client = OpenRouterClient::new(reqwest::Client::new(), &url, "k").with_ledger(env.state.db.clone());
    OpenRouterAnalyst::new(client, "deepseek/deepseek-v4-flash").analyze("s", "a@b.co", "b").await.unwrap();

    let rows = detail_rows(&env).await;
    assert_eq!(rows.len(), 1);
    let (cost, source, cached, provider, finish, gen_id, version, schema_ok, attempt, ok, class) = rows[0].clone();
    assert_eq!((cost, source.as_str()), (0.00002, "provider"), "the provider's own cost wins over the price table");
    assert_eq!((cached, provider.as_deref(), finish.as_deref(), gen_id.as_deref()), (900, Some("DeepSeek"), Some("stop"), Some("gen-abc")));
    assert_eq!(version.as_deref(), Some("analyze-v3"));
    assert_eq!((schema_ok, attempt, ok, class), (None, 1, true, None));
}

#[tokio::test]
async fn unparseable_answers_are_logged_as_failures_not_successes() {
    let env = setup_with(|_| {}).await;
    let client = |url: &str| OpenRouterClient::new(reqwest::Client::new(), url, "k").with_ledger(env.state.db.clone());

    let not_json = fake_detailed_openrouter("sorry, here is some prose").await;
    assert!(OpenRouterAnalyst::new(client(&not_json), "m").analyze("s", "a@b.co", "b").await.is_err());
    let wrong_shape = fake_detailed_openrouter(r#"{"summary": 7}"#).await;
    assert!(OpenRouterAnalyst::new(client(&wrong_shape), "m").analyze("s", "a@b.co", "b").await.is_err());

    let rows = detail_rows(&env).await;
    assert_eq!(rows.len(), 2);
    for r in &rows {
        assert!(!r.9, "ok must be false");
        assert_eq!(r.7, Some(false), "schema_ok must be false");
    }
    assert_eq!(rows[0].10.as_deref(), Some("schema"));
    assert_eq!(rows[1].10.as_deref(), Some("validation"));
}

#[tokio::test]
async fn understanding_calls_are_verified_and_recorded_under_their_own_task_and_prompt_version() {
    use email_ai_api::{
        ai::understand::{OpenRouterUnderstander, UnderstandInput, Understander},
        platform::domain::Label,
    };
    let env = setup_with(|_| {}).await;
    let url = fake_detailed_openrouter(
        r#"{"label":"leave","label_confidence":0.9,"priority":"normal","needs_reply":true,"intent":"request","sentiment":"calm","summary":"s","asks":[{"text":"Approve","kind":"request","quote":"approve my leave"},{"text":"Fake","kind":"request","quote":"never written"}],"commitments":[],"deadlines":[],"entities":[],"missing_info":[],"facts_to_remember":[],"alert_signals":[],"injection_signals":[],"language":"en"}"#,
    )
    .await;
    let client = OpenRouterClient::new(reqwest::Client::new(), &url, "k").with_ledger(env.state.db.clone());
    let input = UnderstandInput {
        subject: "Leave".into(),
        sender: "a@b.co".into(),
        received_at_ms: now_ms(),
        new_part: "Please approve my leave.".into(),
        attachments_block: String::new(),
        attachment_names: vec![],
        labels: vec![Label { key: "leave".into(), name: "Leave".into(), description: "d".into() }],
        examples: vec![],
        sender_history: None,
        thread_digest: None,
    };
    let (u, _) = OpenRouterUnderstander::new(client, "deepseek/deepseek-v4-flash").understand(&input).await.unwrap();
    assert_eq!((u.asks.len(), u.dropped_unverified), (1, 1));
    let (task, version): (String, Option<String>) = sqlx::query_as("SELECT task, prompt_version FROM ai_calls").fetch_one(&env.state.db).await.unwrap();
    assert_eq!((task.as_str(), version.as_deref()), ("understand", Some("understand-v2")));
}

#[tokio::test]
async fn the_stronger_model_is_asked_only_below_half_confidence() {
    use axum::extract::State;
    use email_ai_api::{
        ai::{Classifier, ClassifyInput, openrouter::OpenRouterClassifier},
        platform::domain::Label,
    };
    use std::sync::Mutex;

    async fn chat(State(seen): State<Arc<(Mutex<Vec<String>>, f64)>>, Json(req): Json<Value>) -> Json<Value> {
        seen.0.lock().unwrap().push(req["model"].as_str().unwrap_or_default().to_string());
        let confidence = if req["model"] == "strong/model" { 0.95 } else { seen.1 };
        let content = json!({"category": "other", "priority": "normal", "requires_reply": true, "confidence": confidence});
        Json(json!({"model": req["model"], "choices": [{"finish_reason": "stop", "message": {"content": content.to_string()}}], "usage": {}}))
    }
    let input = ClassifyInput {
        subject: "s".into(),
        sender: "a@b.co".into(),
        body_text: "b".into(),
        attachment_names: vec![],
        sender_history: None,
        labels: vec![Label { key: "other".into(), name: "Other".into(), description: "d".into() }],
        examples: vec![],
    };
    for (fast_confidence, escalated) in [(0.6, false), (0.74, false), (0.49, true), (0.1, true)] {
        let seen = Arc::new((Mutex::new(Vec::new()), fast_confidence));
        let app = Router::new().route("/chat/completions", post(chat)).with_state(seen.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = OpenRouterClient::new(reqwest::Client::new(), &url, "k");
        let classifier = OpenRouterClassifier::new(client, "fast/model").with_escalation("strong/model");
        classifier.classify(&input).await.unwrap();
        let models = seen.0.lock().unwrap().clone();
        assert_eq!(models.len(), if escalated { 2 } else { 1 }, "confidence {fast_confidence}: {models:?}");
    }
}

#[tokio::test]
async fn job_attempt_is_recorded_on_the_ledger_row() {
    let env = setup_with(|_| {}).await;
    let url = fake_detailed_openrouter(r#"{"summary":"s","asks":[],"deadline":null,"needs_reply":false,"missing_info":[]}"#).await;
    let client = OpenRouterClient::new(reqwest::Client::new(), &url, "k").with_ledger(env.state.db.clone());
    let ctx = AiContext::for_job(Some(&env.ws), &json!({}), 3);
    ledger::with_context(ctx, OpenRouterAnalyst::new(client, "m").analyze("s", "a@b.co", "b")).await.unwrap();
    assert_eq!(detail_rows(&env).await[0].8, 3);
}

#[tokio::test]
async fn failed_calls_are_recorded_as_errors() {
    let env = setup_with(|_| {}).await;
    let url = fake_openrouter(true).await;
    let client = OpenRouterClient::new(reqwest::Client::new(), &url, "k").with_ledger(env.state.db.clone());
    assert!(OpenRouterAnalyst::new(client, "qwen/qwen3.8-flash").analyze("s", "a@b.co", "b").await.is_err());
    let embedder = OpenRouterEmbedder::new(reqwest::Client::new(), &url, "k", "baai/bge-m3").with_ledger(env.state.db.clone());
    assert!(embedder.embed(&["x".to_string()]).await.is_err());

    let rows = env.ledger().await;
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| !r.5 && r.4 == 0.0));
    let errors: Vec<Option<String>> =
        sqlx::query_scalar("SELECT error FROM ai_calls ORDER BY rowid").fetch_all(&env.state.db).await.unwrap();
    assert!(errors.iter().all(|e| e.as_deref().is_some_and(|e| e.contains("500"))));
    let classes: Vec<Option<String>> =
        sqlx::query_scalar("SELECT error_class FROM ai_calls WHERE task = 'analyze'").fetch_all(&env.state.db).await.unwrap();
    assert_eq!(classes, [Some("http_5xx".to_string())]);
}

#[tokio::test]
async fn worker_attaches_workspace_and_message_to_job_calls() {
    let env = setup_with(|_| {}).await;
    let url = fake_openrouter(false).await;
    let mut state = env.state.clone();
    let client = OpenRouterClient::new(reqwest::Client::new(), &url, "k").with_ledger(state.db.clone());
    state.analyst = Arc::new(OpenRouterAnalyst::new(client, "qwen/qwen3.8-flash"));
    let env = Env { state, ..env };

    let id = env.email("Where is my payslip?", "Hi, where can I find my payslip for last month?").await;
    let rows = env.ledger().await;
    assert!(!rows.is_empty(), "the analyze job made a model call");
    assert!(rows.iter().all(|r| r.0 == "analyze" && r.6.as_deref() == Some(env.ws.as_str()) && r.7.as_deref() == Some(id.as_str())));
}

#[tokio::test]
async fn usage_report_matches_the_contract_and_is_admin_only() {
    let env = setup_with(|c| c.ai_daily_budget_usd = 1.5).await;
    env.email("Where is my payslip?", "Hi, where can I find my payslip for last month?").await;
    env.email("Can you send the offer letter?", "Hi, can you send the offer letter today?").await;

    let now = now_ms();
    env.insert_call("classify", 0.01, now).await;
    env.insert_call("classify", 0.01, now).await;
    env.insert_call("draft", 0.03, now).await;
    env.insert_call("analyze", 0.02, now - 2 * DAY_MS).await;
    env.insert_call("agent_plan", 0.004, now - 2 * DAY_MS).await;
    env.insert_call("agent_critic", 0.001, now - 2 * DAY_MS).await;
    env.insert_call("embed", 0.001, now - 2 * DAY_MS).await;
    env.insert_call("classify", 9.0, now - 40 * DAY_MS).await;
    sqlx::query("INSERT INTO ai_calls (id, task, model, cost_usd, created_at) VALUES ('aic_x', 'draft', 'm', 7.0, ?1)")
        .bind(now)
        .execute(&env.state.db)
        .await
        .unwrap();

    let path = format!("{}/ai-usage", env.base);
    assert_eq!(call(&env.app, "GET", &path, OPERATOR, None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&env.app, "GET", &path, "dev:stranger@x.test", None).await.0, StatusCode::FORBIDDEN);
    let (s, v) = call(&env.app, "GET", &format!("{path}?days=30"), ADMIN, None).await;
    assert_eq!(s, StatusCode::OK);

    assert_eq!(v["days"], 30);
    assert_eq!(v["budget_usd_per_day"], 1.5);
    assert!((v["total_usd"].as_f64().unwrap() - 0.076).abs() < 1e-9, "old and unattributed rows are excluded: {v}");
    assert!((v["today_usd"].as_f64().unwrap() - 0.05).abs() < 1e-9);

    let by_day = v["by_day"].as_array().unwrap();
    assert_eq!(by_day.len(), 30);
    let today = time::OffsetDateTime::now_utc().date().to_string();
    assert_eq!(by_day[29]["date"], today.as_str());
    assert_eq!(by_day[29]["calls"], 3);
    assert_eq!(by_day[27]["calls"], 4);
    assert_eq!(by_day[0]["calls"], 0);

    let by_task: Vec<(String, f64, i64)> = v["by_task"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["task"].as_str().unwrap().to_string(), t["usd"].as_f64().unwrap(), t["calls"].as_i64().unwrap()))
        .collect();
    let names: Vec<&str> = by_task.iter().map(|t| t.0.as_str()).collect();
    assert_eq!(names, ["Drafts", "Labelling", "Summaries", "Agents", "Search"]);
    assert_eq!(by_task[3].2, 2, "planner and critic calls merge into Agents");

    // Automatic spend (classify .02 + analyze .02 + agents .005 + embed .001) over 2 new emails.
    assert!((v["per_email_usd"].as_f64().unwrap() - 0.0230).abs() < 1e-9, "{v}");

    let (_, short) = call(&env.app, "GET", &format!("{path}?days=1"), ADMIN, None).await;
    assert_eq!(short["by_day"].as_array().unwrap().len(), 1);
    assert!((short["total_usd"].as_f64().unwrap() - 0.05).abs() < 1e-9);
}

#[tokio::test]
async fn over_budget_skips_automatic_ai_but_people_can_still_ask() {
    let env = setup_with(|c| {
        c.ai_daily_budget_usd = 0.01;
        c.auto_drafts = false;
    })
    .await;
    assert!(ledger::within_budget(&env.state.db, 0.01, &env.ws).await);
    env.insert_call("classify", 0.05, now_ms()).await;
    assert!(!ledger::within_budget(&env.state.db, 0.01, &env.ws).await);

    // Automatic stages are skipped; classification still runs.
    let id = env.email("Where is my payslip?", "Hi, where can I find my payslip for last month?").await;
    assert_eq!(env.insights().await, 0, "analysis waits while the budget is spent");
    let (_, detail) = call(&env.app, "GET", &format!("{}/messages/{id}", env.base), ADMIN, None).await;
    assert!(detail["classification"]["category"].is_string(), "classification still runs: {detail}");

    // Opening the email (person-initiated) runs analysis anyway.
    let (s, v) = call(&env.app, "POST", &format!("{}/messages/{id}/insight", env.base), OPERATOR, None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(v["insight"]["summary"].is_string());
    assert_eq!(env.insights().await, 1);

    // Generate draft (person-initiated) always runs.
    let (s, _) = call(&env.app, "POST", &format!("{}/messages/{id}/draft", env.base), OPERATOR, None).await;
    assert!(s.is_success());
    worker::drain(&env.state).await;
    let (_, detail) = call(&env.app, "GET", &format!("{}/messages/{id}", env.base), ADMIN, None).await;
    assert!(detail["draft"]["id"].is_string());

    // A workspace under its budget analyses new mail automatically.
    let fresh = setup_with(|c| {
        c.ai_daily_budget_usd = 5.0;
        c.auto_drafts = false;
    })
    .await;
    fresh.email("Where is my payslip?", "Hi, where can I find my payslip for last month?").await;
    assert_eq!(fresh.insights().await, 1);
}

#[tokio::test]
async fn budget_is_per_workspace_and_per_day() {
    let env = setup_with(|_| {}).await;
    env.insert_call("draft", 1.9, now_ms()).await;
    assert!(ledger::within_budget(&env.state.db, 2.0, &env.ws).await);
    env.insert_call("draft", 0.2, now_ms()).await;
    assert!(!ledger::within_budget(&env.state.db, 2.0, &env.ws).await);

    env.insert_call("draft", 50.0, now_ms() - 2 * DAY_MS).await;
    let (_, other) = call(&env.app, "POST", "/api/v1/workspaces", "dev:other@x.test", Some(json!({"name":"Other"}))).await;
    assert!(ledger::within_budget(&env.state.db, 2.0, other["id"].as_str().unwrap()).await);
}
