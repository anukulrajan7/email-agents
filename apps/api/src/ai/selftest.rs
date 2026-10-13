//! Production self-test: proves each configured model still answers a tool-calling request
//! built by the same code path the planner and assistant use (`OpenRouterClient::chat_tools`).

use serde_json::{Value, json};

use crate::ai::openrouter::OpenRouterClient;
use crate::config::Config;

const SYSTEM: &str = "You are a connectivity probe. Always answer by calling the provided tool.";
const USER: &str = "Call the ping tool with ok set to true.";

#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub roles: Vec<&'static str>,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub target: Target,
    pub result: Result<(), String>,
}

/// Configured models to probe. Roles sharing one model id are probed once. Unset roles are skipped.
pub fn targets(config: &Config) -> Vec<Target> {
    let roles: [(&'static str, &str); 5] = [
        ("planner", &config.openrouter_planner_model_id),
        ("assistant", &config.openrouter_planner_model_id),
        ("draft", &config.openrouter_draft_model_id),
        ("premium-draft", &config.openrouter_premium_model_id),
        ("classifier", &config.openrouter_classifier_model_id),
    ];
    let mut out: Vec<Target> = Vec::new();
    for (role, model) in roles {
        let model = model.trim();
        if model.is_empty() {
            continue;
        }
        match out.iter_mut().find(|t| t.model == model) {
            Some(t) => t.roles.push(role),
            None => out.push(Target { roles: vec![role], model: model.into() }),
        }
    }
    out
}

fn ping_tool() -> Value {
    json!([{
        "type": "function",
        "function": {
            "name": "ping",
            "description": "Reports that the model can call tools.",
            "parameters": {
                "type": "object",
                "properties": {"ok": {"type": "boolean"}},
                "required": ["ok"]
            }
        }
    }])
}

/// One tool-calling request; fails with a readable reason unless the model returns a `ping` call.
pub async fn probe(client: &OpenRouterClient, model: &str) -> Result<(), String> {
    let reply = client
        .chat_tools(model, SYSTEM, USER.into(), ping_tool(), json!("auto"), 200)
        .await
        .map_err(|e| e.to_string())?;
    let name = reply
        .message
        .pointer("/tool_calls/0/function/name")
        .and_then(Value::as_str);
    match name {
        Some("ping") => Ok(()),
        Some(other) => Err(format!("model called unexpected tool '{other}'")),
        None => Err("model answered without calling the tool (tool calling not working)".into()),
    }
}

/// Probes every target; each gets one retry so a transient blip does not page the owner.
pub async fn run(client: &OpenRouterClient, targets: Vec<Target>) -> Vec<Outcome> {
    let mut outcomes = Vec::with_capacity(targets.len());
    for target in targets {
        let mut result = probe(client, &target.model).await;
        if result.is_err() {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            result = probe(client, &target.model).await;
        }
        outcomes.push(Outcome { target, result });
    }
    outcomes
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, extract::State, routing::post};
    use std::sync::Arc;

    async fn mock(reply: Value) -> String {
        async fn handler(State(reply): State<Arc<Value>>, Json(body): Json<Value>) -> Json<Value> {
            if body["model"] == "bad/model" {
                return Json(json!({"choices": [{"message": {"content": "hello"}}]}));
            }
            Json((*reply).clone())
        }
        let app = Router::new().route("/chat/completions", post(handler)).with_state(Arc::new(reply));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    fn tool_reply(name: &str) -> Value {
        json!({"model": "x", "choices": [{"message": {"tool_calls": [{"function": {"name": name, "arguments": "{\"ok\":true}"}}]}}]})
    }

    #[test]
    fn targets_dedupe_models_and_skip_unset() {
        let mut config = Config::for_tests("sqlite::memory:");
        config.openrouter_planner_model_id = "a/pro".into();
        config.openrouter_premium_model_id = "a/pro".into();
        config.openrouter_draft_model_id = "a/flash".into();
        config.openrouter_classifier_model_id = String::new();
        let t = targets(&config);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0], Target { roles: vec!["planner", "assistant", "premium-draft"], model: "a/pro".into() });
        assert_eq!(t[1].roles, vec!["draft"]);
    }

    #[tokio::test]
    async fn probe_accepts_a_ping_tool_call() {
        let base = mock(tool_reply("ping")).await;
        let client = OpenRouterClient::new(reqwest::Client::new(), &base, "k");
        assert_eq!(probe(&client, "good/model").await, Ok(()));
    }

    #[tokio::test]
    async fn probe_reports_plain_text_and_wrong_tool() {
        let base = mock(tool_reply("other")).await;
        let client = OpenRouterClient::new(reqwest::Client::new(), &base, "k");
        assert!(probe(&client, "good/model").await.unwrap_err().contains("unexpected tool 'other'"));
        assert!(probe(&client, "bad/model").await.unwrap_err().contains("without calling the tool"));
    }

    #[tokio::test]
    async fn probe_reports_http_errors() {
        let client = OpenRouterClient::new(reqwest::Client::new(), "http://127.0.0.1:9", "k");
        assert!(probe(&client, "m").await.is_err());
    }
}
