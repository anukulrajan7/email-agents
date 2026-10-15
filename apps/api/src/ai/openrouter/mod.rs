//! OpenRouter adapters: an efficient classifier and a higher-quality drafter, both with
//! JSON-schema constrained responses and no-retention provider routing.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{
    AiError, Classifier, ClassifyInput, ClassifyOutput, DraftInput, DraftOutput, Drafter, LabelExample, ReplyExample,
    classify_http_error,
    fence::{fence, fence_inline},
    prepare_text, prepare_text_up_to,
};
use crate::{
    platform::domain::{ClassificationSource, Label, Priority},
    understanding::labels,
};

mod drafter;
mod classifier;
mod extractor;
mod analyst;
mod learner;

pub use drafter::OpenRouterDrafter;
pub use drafter::draft_system_prompt;
pub use drafter::draft_system_prompt_with;
pub use drafter::CheckedDraft;
pub use drafter::strip_links;
pub use drafter::truncate_at_sentence;
pub use drafter::validate_draft;
pub use drafter::draft_prompt;
pub use classifier::OpenRouterClassifier;
pub use classifier::ESCALATE_BELOW;
pub use classifier::classify_prompt;
pub use classifier::parse_classification;
pub use classifier::needs_escalation;
pub use extractor::OpenRouterExtractor;
pub use analyst::OpenRouterAnalyst;
pub use learner::OpenRouterLearner;
pub use learner::parse_learning;

#[derive(Clone)]
pub struct OpenRouterClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    /// Where calls are recorded (`ai_calls`); `None` records nothing.
    ledger: Option<sqlx::SqlitePool>,
    /// Ledger task name for calls made through this handle, e.g. "classify".
    task: String,
    /// Version of the prompt text behind `task`, stored on every ledger row.
    prompt_version: String,
}

/// The prompt version recorded for a ledger task. Bump the constant when the prompt text changes.
pub fn default_prompt_version(task: &str) -> String {
    match task {
        "classify" => super::PROMPT_VERSION.to_string(),
        "analyze" => super::ANALYZE_PROMPT_VERSION.to_string(),
        "draft" => super::DRAFT_PROMPT_VERSION.to_string(),
        other => format!("{other}-v1"),
    }
}

/// `timeout|network` for a request that never got an answer.
fn transport_class(e: &reqwest::Error) -> &'static str {
    if e.is_timeout() { "timeout" } else { "network" }
}

/// `http_4xx|http_5xx` for a non-success status.
fn status_class(status: reqwest::StatusCode) -> &'static str {
    if status.is_server_error() { "http_5xx" } else { "http_4xx" }
}

impl OpenRouterClient {
    pub fn new(http: reqwest::Client, base_url: &str, api_key: &str) -> Self {
        Self { http, base_url: base_url.trim_end_matches('/').to_string(), api_key: api_key.into(), ledger: None, task: "other".into(), prompt_version: default_prompt_version("other") }
    }

    pub fn with_ledger(mut self, db: sqlx::SqlitePool) -> Self {
        self.ledger = Some(db);
        self
    }

    /// Same client, recording its calls under `task`.
    pub fn with_task(&self, task: &str) -> Self {
        Self { task: task.into(), prompt_version: default_prompt_version(task), ..self.clone() }
    }

    /// Same client, recording `version` as the prompt version of its calls.
    pub fn with_prompt_version(&self, version: &str) -> Self {
        Self { prompt_version: version.into(), ..self.clone() }
    }

    pub(crate) async fn log_call(&self, model: &str, started: std::time::Instant, body: Option<&Value>, error: Option<&str>) {
        self.log_call_detail(model, started, body, error, None, None).await;
    }

    /// Writes the ledger row for one call; returns its id so a later parse failure can mark it.
    /// `fallback_cost` prices calls whose response carries no `usage.cost` (speech to text).
    async fn log_call_detail(
        &self,
        model: &str,
        started: std::time::Instant,
        body: Option<&Value>,
        error: Option<&str>,
        error_class: Option<&str>,
        fallback_cost: Option<f64>,
    ) -> Option<String> {
        let db = self.ledger.as_ref()?;
        let mut usage = body.map(crate::ai::ledger::usage_from).unwrap_or_default();
        if usage.reported_cost.is_none() {
            usage.reported_cost = fallback_cost;
        }
        let mut meta = body.map(|b| crate::ai::ledger::meta_from(b, model)).unwrap_or_default();
        meta.prompt_version = Some(self.prompt_version.clone());
        meta.error_class = error_class.map(str::to_string);
        crate::ai::ledger::record_full(db, &self.task, model, usage, &meta, started.elapsed().as_millis() as i64, error).await
    }

    async fn mark_invalid(&self, row_id: Option<&str>, class: &str, error: &str) {
        if let (Some(db), Some(id)) = (&self.ledger, row_id) {
            crate::ai::ledger::mark_invalid(db, id, class, error).await;
        }
    }

    /// One structured chat call. Returns (JSON content string, model that answered). Content that is
    /// not valid JSON is a failure in the ledger, not a success.
    pub(crate) async fn chat_json(
        &self,
        model: &str,
        system: &str,
        user: String,
        schema_name: &str,
        schema: Value,
        max_tokens: u32,
    ) -> Result<(String, String), AiError> {
        self.chat_json_checked(model, system, user, schema_name, schema, max_tokens, |_| Ok(())).await
    }

    /// Like `chat_json`, and `validate` also judges the content (schema or business rules). When it
    /// returns an error, the ledger row for this call is marked as a failed, invalid answer.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn chat_json_checked(
        &self,
        model: &str,
        system: &str,
        user: String,
        schema_name: &str,
        schema: Value,
        max_tokens: u32,
        validate: impl FnOnce(&str) -> Result<(), AiError>,
    ) -> Result<(String, String), AiError> {
        let (content, answered_by, _) = self.chat_json_checked_usage(model, system, user, schema_name, schema, max_tokens, validate).await?;
        Ok((content, answered_by))
    }

    /// Like `chat_json`, and also returns the token usage (with the provider's cost when it reports one).
    pub(crate) async fn chat_json_usage(
        &self,
        model: &str,
        system: &str,
        user: String,
        schema_name: &str,
        schema: Value,
        max_tokens: u32,
    ) -> Result<(String, String, ChatUsage), AiError> {
        self.chat_json_checked_usage(model, system, user, schema_name, schema, max_tokens, |_| Ok(())).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn chat_json_checked_usage(
        &self,
        model: &str,
        system: &str,
        user: String,
        schema_name: &str,
        schema: Value,
        max_tokens: u32,
        validate: impl FnOnce(&str) -> Result<(), AiError>,
    ) -> Result<(String, String, ChatUsage), AiError> {
        let body = json!({
            "model": model,
            "messages": [{"role": "system", "content": system}, {"role": "user", "content": user}],
            "max_tokens": max_tokens,
            "temperature": 0.2,
            "response_format": {
                "type": "json_schema",
                "json_schema": {"name": schema_name, "strict": true, "schema": schema}
            },
            // Short structured answers: hidden reasoning would eat the token budget and
            // return null content (seen live with Qwen 3.8 Flash).
            "reasoning": {"enabled": false},
            // Only providers that don't retain/train on prompts, and that honour the schema.
            "provider": {"data_collection": "deny", "require_parameters": true},
            "usage": {"include": true}
        });
        let started = std::time::Instant::now();
        let sent = async {
            let res = self
                .http
                .post(format!("{}/chat/completions", self.base_url))
                .bearer_auth(&self.api_key)
                .json(&body)
                .timeout(Duration::from_secs(45))
                .send()
                .await
                .map_err(|e| (transport_class(&e), AiError::Retryable(format!("OpenRouter request failed: {e}"))))?;
            if !res.status().is_success() {
                return Err((status_class(res.status()), classify_http_error(res.status(), "OpenRouter")));
            }
            res.json::<Value>().await.map_err(|e| ("network", AiError::Retryable(format!("OpenRouter decode: {e}"))))
        }
        .await;
        let body = match sent {
            Ok(body) => body,
            Err((class, e)) => {
                self.log_call_detail(model, started, None, Some(&e.to_string()), Some(class), None).await;
                return Err(e);
            }
        };
        let finish = body.pointer("/choices/0/finish_reason").and_then(Value::as_str).unwrap_or("unknown");
        let refused = body.pointer("/choices/0/message/refusal").and_then(Value::as_str).is_some_and(|r| !r.trim().is_empty());
        let content = body
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .filter(|c| !c.trim().is_empty())
            .ok_or_else(|| AiError::Retryable(format!("OpenRouter returned no content (finish_reason={finish})")));
        let content = match content {
            Ok(content) => content,
            Err(e) => {
                let class = if refused { "refusal" } else { "empty" };
                self.log_call_detail(model, started, Some(&body), Some(&e.to_string()), Some(class), None).await;
                return Err(e);
            }
        };
        let row = self.log_call_detail(model, started, Some(&body), None, None, None).await;
        let verdict = serde_json::from_str::<Value>(content.trim())
            .map_err(|_| ("schema", AiError::Retryable(format!("model returned invalid JSON (finish_reason={finish})"))))
            .and_then(|_| validate(content).map_err(|e| ("validation", e)));
        if let Err((class, e)) = verdict {
            self.mark_invalid(row.as_deref(), class, &e.to_string()).await;
            return Err(e);
        }
        let answered_by = body.get("model").and_then(Value::as_str).unwrap_or(model).to_string();
        let usage = ChatUsage {
            tokens_in: body.pointer("/usage/prompt_tokens").and_then(Value::as_i64).unwrap_or(0),
            tokens_out: body.pointer("/usage/completion_tokens").and_then(Value::as_i64).unwrap_or(0),
            cost_usd: body.pointer("/usage/cost").and_then(Value::as_f64),
        };
        Ok((content.to_string(), answered_by, usage))
    }

    /// One JSON POST to an OpenRouter path with the ledger entry written for the outcome.
    pub(crate) async fn post_json(&self, path: &str, model: &str, body: &Value, timeout: Duration) -> Result<Value, AiError> {
        let started = std::time::Instant::now();
        let request = self.http.post(format!("{}{path}", self.base_url)).bearer_auth(&self.api_key).json(body).timeout(timeout);
        self.finish(model, started, request, None).await
    }

    /// One multipart POST (audio transcription) with the ledger entry written for the outcome.
    pub(crate) async fn post_multipart(
        &self,
        path: &str,
        model: &str,
        form: reqwest::multipart::Form,
        timeout: Duration,
        estimated_cost_usd: Option<f64>,
    ) -> Result<Value, AiError> {
        let started = std::time::Instant::now();
        let request = self.http.post(format!("{}{path}", self.base_url)).bearer_auth(&self.api_key).multipart(form).timeout(timeout);
        self.finish(model, started, request, estimated_cost_usd).await
    }

    async fn finish(
        &self,
        model: &str,
        started: std::time::Instant,
        request: reqwest::RequestBuilder,
        fallback_cost: Option<f64>,
    ) -> Result<Value, AiError> {
        let sent = async {
            let res = request
                .send()
                .await
                .map_err(|e| (transport_class(&e), AiError::Retryable(format!("OpenRouter request failed: {e}"))))?;
            if !res.status().is_success() {
                return Err((status_class(res.status()), classify_http_error(res.status(), "OpenRouter")));
            }
            res.json::<Value>().await.map_err(|e| ("network", AiError::Retryable(format!("OpenRouter decode: {e}"))))
        }
        .await;
        match sent {
            Ok(body) => {
                self.log_call_detail(model, started, Some(&body), None, None, fallback_cost).await;
                Ok(body)
            }
            Err((class, e)) => {
                self.log_call_detail(model, started, None, Some(&e.to_string()), Some(class), None).await;
                Err(e)
            }
        }
    }

    /// One tool-calling chat turn (OpenAI-style `tools`). Returns the assistant message as-is
    /// (the caller validates any tool call), the answering model and token usage.
    pub(crate) async fn chat_tools(
        &self,
        model: &str,
        system: &str,
        user: String,
        tools: Value,
        tool_choice: Value,
        max_tokens: u32,
    ) -> Result<ToolReply, AiError> {
        let body = json!({
            "model": model,
            "messages": [{"role": "system", "content": system}, {"role": "user", "content": user}],
            "tools": tools,
            "tool_choice": tool_choice,
            // No `parallel_tool_calls`: with `require_parameters` it leaves DeepSeek/Qwen with no
            // provider (404). Callers read only the first tool call.
            "max_tokens": max_tokens,
            "temperature": 0.1,
            "reasoning": {"enabled": false},
            "provider": {"data_collection": "deny", "require_parameters": true}
        });
        let started = std::time::Instant::now();
        let res = match self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&body)
            .timeout(Duration::from_secs(60))
            .send()
            .await
        {
            Ok(res) => res,
            Err(e) => {
                let msg = format!("OpenRouter request failed: {e}");
                self.log_call_detail(model, started, None, Some(&msg), Some(transport_class(&e)), None).await;
                return Err(AiError::Retryable(msg));
            }
        };
        if !res.status().is_success() {
            let status = res.status();
            self.log_call_detail(model, started, None, Some(&format!("HTTP {status}")), Some(status_class(status)), None).await;
            return Err(classify_http_error(status, "OpenRouter"));
        }
        let body: Value = res.json().await.map_err(|e| AiError::Retryable(format!("OpenRouter decode: {e}")))?;
        self.log_call(model, started, Some(&body), None).await;
        let mut message = body.pointer("/choices/0/message").cloned().unwrap_or(Value::Null);
        if let Some(m) = message.as_object_mut() {
            for key in ["reasoning", "reasoning_content", "reasoning_details"] {
                m.remove(key);
            }
        }
        let usage = ChatUsage {
            tokens_in: body.pointer("/usage/prompt_tokens").and_then(Value::as_i64).unwrap_or(0),
            tokens_out: body.pointer("/usage/completion_tokens").and_then(Value::as_i64).unwrap_or(0),
            cost_usd: body.pointer("/usage/cost").and_then(Value::as_f64),
        };
        let model_id = body.get("model").and_then(Value::as_str).unwrap_or(model).to_string();
        Ok(ToolReply { message, model_id, usage })
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChatUsage {
    pub tokens_in: i64,
    pub tokens_out: i64,
    /// Provider-reported cost, when the provider returns one.
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct ToolReply {
    pub message: Value,
    pub model_id: String,
    pub usage: ChatUsage,
}

fn email_block(input_subject: &str, body: &str) -> String {
    format!("Subject: {}\n{}", fence_inline(input_subject), fence("email", body))
}

/// Retrieved text must not be able to close or open our prompt tags.
fn defang(text: &str) -> String {
    super::fence::defang(&prepare_text(text))
}

pub fn label_examples_block(examples: &[LabelExample]) -> String {
    if examples.is_empty() {
        return String::new();
    }
    let items: String = examples
        .iter()
        .map(|e| format!("<example label=\"{}\">\nSubject: {}\n{}\n</example>\n", defang(&e.label), defang(&e.subject), defang(&e.snippet)))
        .collect();
    format!("<examples>\n{items}</examples>\n")
}

pub fn past_replies_block(examples: &[ReplyExample]) -> String {
    if examples.is_empty() {
        return String::new();
    }
    let items: String = examples
        .iter()
        .map(|e| format!("<reply to=\"{}\">\n{}\n</reply>\n", defang(&e.subject).replace('"', "'"), defang(&e.reply)))
        .collect();
    format!("<past_replies>\n{items}</past_replies>\n")
}

