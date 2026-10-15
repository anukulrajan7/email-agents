use super::*;

pub(super) struct CannedDrafter(pub(super) Option<String>);

#[async_trait]
impl Drafter for CannedDrafter {
    async fn draft(&self, input: &DraftInput) -> Result<DraftOutput, AiError> {
        match &self.0 {
            Some(body) => Ok(DraftOutput {
                body_text: body.clone(),
                model_id: "canned".into(),
                unanswered: if body.contains("[NEEDS:") { vec!["open question".into()] } else { vec![] },
            }),
            None => TemplateDrafter.draft(input).await,
        }
    }
}

pub(super) type Calls = Arc<Mutex<Vec<(String, Value)>>>;

pub(super) async fn spawn_gmail(calls: Calls) -> String {
    let (c1, c2, c3, c4) = (calls.clone(), calls.clone(), calls.clone(), calls);
    let app = Router::new()
        .route("/proxy/gmail/v1/users/me/profile", get(|| async { Json(json!({"emailAddress": MAILBOX})) }))
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

pub(super) async fn call(app: &Router, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
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

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| default.to_string())
}

/// The models of a running server's config, for a live run in an isolated workspace.
pub fn live_config_from(server: &Config, db_url: &str) -> Option<Config> {
    if server.openrouter_api_key.trim().is_empty() {
        return None;
    }
    let mut c = Config::for_tests(db_url);
    c.openrouter_api_key = server.openrouter_api_key.clone();
    c.openrouter_api_url = server.openrouter_api_url.clone();
    c.openrouter_draft_model_id = server.openrouter_draft_model_id.clone();
    c.openrouter_premium_model_id = server.openrouter_premium_model_id.clone();
    c.openrouter_model_id = server.openrouter_model_id.clone();
    c.openrouter_classifier_model_id = server.openrouter_classifier_model_id.clone();
    c.openrouter_planner_model_id = server.openrouter_planner_model_id.clone();
    c.agent_fast_model_id = server.agent_fast_model_id.clone();
    c.agent_balanced_model_id = server.agent_balanced_model_id.clone();
    c.agent_best_model_id = server.agent_best_model_id.clone();
    Some(c)
}

/// The real planner, drafter, critic and judge, from the usual OPENROUTER_* variables.
pub fn live_config(db_url: &str) -> Option<Config> {
    let key = std::env::var("OPENROUTER_API_KEY").ok().filter(|k| !k.trim().is_empty())?;
    let mut c = Config::for_tests(db_url);
    c.openrouter_api_key = key;
    c.openrouter_api_url = env_or("OPENROUTER_API_URL", "https://openrouter.ai/api/v1");
    c.openrouter_draft_model_id = env_or("OPENROUTER_DRAFT_MODEL_ID", "deepseek/deepseek-v4-flash");
    c.openrouter_premium_model_id = env_or("OPENROUTER_PREMIUM_MODEL_ID", "deepseek/deepseek-v4-pro");
    c.openrouter_model_id = env_or("OPENROUTER_MODEL_ID", "");
    c.openrouter_classifier_model_id = env_or("OPENROUTER_CLASSIFIER_MODEL_ID", "");
    c.openrouter_planner_model_id = env_or("OPENROUTER_PLANNER_MODEL_ID", "deepseek/deepseek-v4-pro");
    c.agent_fast_model_id = env_or("AGENT_FAST_MODEL_ID", "qwen/qwen3.8-flash");
    c.agent_balanced_model_id = env_or("AGENT_BALANCED_MODEL_ID", "deepseek/deepseek-v4-flash");
    c.agent_best_model_id = env_or("AGENT_BEST_MODEL_ID", "deepseek/deepseek-v4-pro");
    Some(c)
}
