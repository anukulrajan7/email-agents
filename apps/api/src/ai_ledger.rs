//! AI call ledger: one `ai_calls` row per model call, a per-workspace daily budget, and the
//! numbers behind `GET /ai-usage`. Recording never fails the caller.

use std::{
    collections::{BTreeMap, HashSet},
    future::Future,
    sync::{Mutex, OnceLock},
};

use axum::{extract::Request, middleware::Next, response::Response};
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::db::{new_id, now_ms};

const DAY_MS: i64 = 86_400_000;
const MAX_ERROR_CHARS: usize = 300;

/// (model, USD per 1M input tokens, USD per 1M output tokens), from OpenRouter on 14 Oct 2026.
const PRICES: &[(&str, f64, f64)] = &[
    ("qwen/qwen3.8-flash", 0.15, 0.47),
    ("moonshotai/kimi-k2.6", 0.65, 3.41),
    ("anthropic/claude-haiku-5.5", 0.10, 0.50),
    ("deepseek/deepseek-v4-flash", 0.091, 0.182),
    ("deepseek/deepseek-v4-pro", 0.40, 0.80),
    ("qwen/qwen3-vl-30b-a3b-instruct", 0.15, 0.60),
    ("qwen/qwen3.8-omni-flash", 0.15, 0.47),
    ("baai/bge-m3", 0.01, 0.0),
];

/// Tasks that run on their own for new mail (not because a person clicked). Drives cost per email.
pub const AUTOMATIC_TASKS: &[&str] =
    &["classify", "analyze", "understand", "learn", "extract", "embed", "judge", "vision", "audio", "agent_plan", "agent_critic"];

/// Tasks a person asks for (running an eval, designing an agent). Never counted as per-email spend.
pub const MANUAL_TASKS: &[&str] = &["eval", "designer"];

fn price(model: &str) -> Option<(f64, f64)> {
    let model = model.to_lowercase();
    PRICES.iter().find(|(m, _, _)| *m == model).map(|(_, i, o)| (*i, *o))
}

fn warn_once(model: &str) {
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let seen = SEEN.get_or_init(Mutex::default);
    if let Ok(mut seen) = seen.lock()
        && seen.insert(model.to_string())
    {
        tracing::warn!(%model, "no price for model: cost recorded from the provider's usage.cost, or 0");
    }
}

/// The provider-reported `usage.cost` (it already includes cache discounts) when present; else the
/// price table; else 0 (logged once). Second value says where the number came from.
pub fn cost_with_source(model: &str, tokens_in: i64, tokens_out: i64, reported: Option<f64>) -> (f64, &'static str) {
    if let Some(cost) = reported.filter(|c| c.is_finite() && *c >= 0.0) {
        return (cost, "provider");
    }
    match price(model) {
        Some((input, output)) => ((tokens_in.max(0) as f64 * input + tokens_out.max(0) as f64 * output) / 1_000_000.0, "table"),
        None => {
            warn_once(model);
            (0.0, "none")
        }
    }
}

pub fn cost_usd(model: &str, tokens_in: i64, tokens_out: i64, reported: Option<f64>) -> f64 {
    cost_with_source(model, tokens_in, tokens_out, reported).0
}

// ---------------------------------------------------------------- context

/// Who a model call is for. The OpenRouter client doesn't know; the worker and an HTTP
/// middleware set it around the work, and `record` picks it up.
#[derive(Debug, Clone, Default)]
pub struct AiContext {
    pub workspace_id: Option<String>,
    pub message_id: Option<String>,
    pub case_id: Option<String>,
    /// Which try of the surrounding job this is (1 = first). Calls outside a job count as 1.
    pub attempt: i64,
}

tokio::task_local! {
    static CONTEXT: AiContext;
}

impl AiContext {
    pub fn for_job(workspace_id: Option<&str>, payload: &Value, attempt: i64) -> Self {
        let field = |k: &str| payload.get(k).and_then(Value::as_str).map(str::to_string);
        Self {
            workspace_id: workspace_id.map(str::to_string),
            message_id: field("message_id"),
            case_id: field("case_id"),
            attempt: attempt.max(1),
        }
    }

    fn from_path(path: &str) -> Self {
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let after = |name: &str| segments.iter().position(|s| *s == name).and_then(|i| segments.get(i + 1)).map(|s| (*s).to_string());
        Self { workspace_id: after("workspaces"), message_id: after("messages"), case_id: None, attempt: 1 }
    }
}

pub fn current_context() -> AiContext {
    CONTEXT.try_with(Clone::clone).unwrap_or_default()
}

pub async fn with_context<F: Future>(ctx: AiContext, fut: F) -> F::Output {
    CONTEXT.scope(ctx, fut).await
}

/// HTTP middleware: model calls made while serving `/workspaces/{ws}/...` belong to that workspace.
/// The handler still authorizes the request; this only labels spend.
pub async fn attach_context(req: Request, next: Next) -> Response {
    let ctx = AiContext::from_path(req.uri().path());
    with_context(ctx, next.run(req)).await
}

// ---------------------------------------------------------------- recording

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Usage {
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub reported_cost: Option<f64>,
    /// Prompt tokens read from the provider's cache (`prompt_tokens_details.cached_tokens`).
    pub cached_tokens: i64,
    pub cache_write_tokens: i64,
    pub reasoning_tokens: i64,
}

/// What else is worth knowing about one call. Everything is optional: recording never needs it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CallMeta {
    /// Upstream provider that answered (`provider` in the response).
    pub provider: Option<String>,
    /// Model that answered, when it differs from the one asked for.
    pub answered_model: Option<String>,
    pub finish_reason: Option<String>,
    pub generation_id: Option<String>,
    pub prompt_version: Option<String>,
    /// `Some(false)` when the answer came back but did not parse or validate.
    pub schema_ok: Option<bool>,
    /// `timeout|network|http_4xx|http_5xx|empty|schema|validation|refusal`.
    pub error_class: Option<String>,
}

/// Reads OpenAI-style `usage` from a provider response body (chat and embeddings).
pub fn usage_from(body: &Value) -> Usage {
    let usage = body.get("usage");
    let int = |k: &str| usage.and_then(|u| u.get(k)).and_then(Value::as_i64).unwrap_or(0);
    let nested = |group: &str, k: &str| {
        usage.and_then(|u| u.get(group)).and_then(|g| g.get(k)).and_then(Value::as_i64).unwrap_or(0)
    };
    Usage {
        tokens_in: int("prompt_tokens"),
        tokens_out: int("completion_tokens"),
        reported_cost: usage.and_then(|u| u.get("cost")).and_then(Value::as_f64),
        cached_tokens: nested("prompt_tokens_details", "cached_tokens"),
        cache_write_tokens: nested("prompt_tokens_details", "cache_write_tokens"),
        reasoning_tokens: nested("completion_tokens_details", "reasoning_tokens"),
    }
}

/// Provider, answering model, finish reason and generation id from a chat response body.
pub fn meta_from(body: &Value, asked_model: &str) -> CallMeta {
    let text = |p: &str| body.pointer(p).and_then(Value::as_str).map(str::to_string);
    CallMeta {
        provider: text("/provider"),
        answered_model: text("/model").filter(|m| !m.eq_ignore_ascii_case(asked_model)),
        finish_reason: text("/choices/0/finish_reason"),
        generation_id: text("/id"),
        ..CallMeta::default()
    }
}

/// Writes one ledger row. Logs and returns on any failure.
pub async fn record(db: &SqlitePool, task: &str, model: &str, usage: Usage, latency_ms: i64, error: Option<&str>) {
    record_full(db, task, model, usage, &CallMeta::default(), latency_ms, error).await;
}

/// Like `record`, with the extra detail. Returns the new row id.
pub async fn record_full(
    db: &SqlitePool,
    task: &str,
    model: &str,
    usage: Usage,
    meta: &CallMeta,
    latency_ms: i64,
    error: Option<&str>,
) -> Option<String> {
    let ctx = current_context();
    let (cost, cost_source) = cost_with_source(model, usage.tokens_in, usage.tokens_out, usage.reported_cost);
    let error = error.map(|e| crate::sources::normalize::truncate_chars(e, MAX_ERROR_CHARS));
    let id = new_id("aic");
    let result = sqlx::query(
        "INSERT INTO ai_calls (id, workspace_id, task, model, tokens_in, tokens_out, cost_usd, latency_ms, ok, error, message_id, case_id,
                               created_at, cached_tokens, cache_write_tokens, reasoning_tokens, provider, answered_model, finish_reason,
                               generation_id, prompt_version, schema_ok, attempt, error_class, cost_source)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)",
    )
    .bind(&id)
    .bind(ctx.workspace_id)
    .bind(task)
    .bind(model)
    .bind(usage.tokens_in)
    .bind(usage.tokens_out)
    .bind(cost)
    .bind(latency_ms)
    .bind(error.is_none())
    .bind(error)
    .bind(ctx.message_id)
    .bind(ctx.case_id)
    .bind(now_ms())
    .bind(usage.cached_tokens)
    .bind(usage.cache_write_tokens)
    .bind(usage.reasoning_tokens)
    .bind(&meta.provider)
    .bind(&meta.answered_model)
    .bind(&meta.finish_reason)
    .bind(&meta.generation_id)
    .bind(&meta.prompt_version)
    .bind(meta.schema_ok)
    .bind(ctx.attempt.max(1))
    .bind(&meta.error_class)
    .bind(cost_source)
    .execute(db)
    .await;
    match result {
        Ok(_) => Some(id),
        Err(e) => {
            tracing::warn!(error = %e, %task, "failed to record AI call");
            None
        }
    }
}

/// Records a finished provider call: tokens and detail come from the body when there is one.
pub async fn log_call(db: &SqlitePool, task: &str, model: &str, started: std::time::Instant, body: Option<&Value>, error: Option<&str>) {
    let usage = body.map(usage_from).unwrap_or_default();
    let meta = body.map(|b| meta_from(b, model)).unwrap_or_default();
    record_full(db, task, model, usage, &meta, started.elapsed().as_millis() as i64, error).await;
}

/// A call that got a well-formed HTTP answer whose content failed parsing or validation: the row
/// becomes a failure (`ok = 0`, `schema_ok = 0`) instead of looking like a success.
pub async fn mark_invalid(db: &SqlitePool, row_id: &str, error_class: &str, error: &str) {
    let error = crate::sources::normalize::truncate_chars(error, MAX_ERROR_CHARS);
    let result = sqlx::query("UPDATE ai_calls SET ok = 0, schema_ok = 0, error = ?2, error_class = ?3 WHERE id = ?1")
        .bind(row_id)
        .bind(error)
        .bind(error_class)
        .execute(db)
        .await;
    if let Err(e) = result {
        tracing::warn!(error = %e, "failed to mark AI call invalid");
        return;
    }
    record_call_incident(db, row_id, error_class).await;
}

/// A call that came back unusable is an incident (`refusal` or `schema_failure`), attributed to its case's agent.
async fn record_call_incident(db: &SqlitePool, row_id: &str, error_class: &str) {
    let row: Result<Option<(Option<String>, String, Option<String>, Option<String>)>, _> =
        sqlx::query_as("SELECT workspace_id, task, case_id, message_id FROM ai_calls WHERE id = ?1").bind(row_id).fetch_optional(db).await;
    let Ok(Some((Some(ws), task, case_id, message_id))) = row else { return };
    let agent_id: Option<String> = match &case_id {
        Some(c) => sqlx::query_scalar("SELECT agent_id FROM cases WHERE id = ?1 AND workspace_id = ?2")
            .bind(c)
            .bind(&ws)
            .fetch_optional(db)
            .await
            .ok()
            .flatten()
            .flatten(),
        None => None,
    };
    let kind = if error_class == "refusal" { crate::incidents::Kind::Refusal } else { crate::incidents::Kind::SchemaFailure };
    crate::incidents::record(
        db,
        crate::incidents::NewIncident::new(&ws, kind, json!({"task": task, "error_class": error_class}))
            .agent(agent_id.as_deref())
            .case(case_id.as_deref())
            .message(message_id.as_deref())
            .dedupe(format!("call:{row_id}")),
    )
    .await;
}

// ---------------------------------------------------------------- budget

fn today_start_ms() -> i64 {
    let now = now_ms();
    now - now.rem_euclid(DAY_MS)
}

/// What this workspace has spent on AI since 00:00 UTC.
pub async fn spend_today(db: &SqlitePool, workspace_id: &str) -> sqlx::Result<f64> {
    sqlx::query_scalar("SELECT COALESCE(SUM(cost_usd), 0.0) FROM ai_calls WHERE workspace_id = ?1 AND created_at >= ?2")
        .bind(workspace_id)
        .bind(today_start_ms())
        .fetch_one(db)
        .await
}

/// True while today's spend is under `daily_budget_usd` (`AI_DAILY_BUDGET_USD`). Automatic,
/// non-essential AI checks this; person-initiated work never does. A lookup error counts as
/// within budget: the ledger must not stop the product.
pub async fn within_budget(db: &SqlitePool, daily_budget_usd: f64, workspace_id: &str) -> bool {
    match spend_today(db, workspace_id).await {
        Ok(spent) => spent < daily_budget_usd,
        Err(e) => {
            tracing::warn!(error = %e, "budget check failed; allowing");
            true
        }
    }
}

/// Inbound work every new email triggers on its own, whoever sent it: counted against one
/// global cap so a mail flood can't run up an unbounded bill.
pub const GLOBAL_CAPPED_TASKS: &[&str] = &["classify", "embed"];

/// Today's (UTC) spend on `GLOBAL_CAPPED_TASKS`, across all workspaces.
pub async fn global_inbound_spend_today(db: &SqlitePool) -> sqlx::Result<f64> {
    sqlx::query_scalar(
        "SELECT COALESCE(SUM(cost_usd), 0.0) FROM ai_calls WHERE created_at >= ?1 AND task IN (SELECT value FROM json_each(?2))",
    )
    .bind(today_start_ms())
    .bind(serde_json::json!(GLOBAL_CAPPED_TASKS).to_string())
    .fetch_one(db)
    .await
}

/// True while today's global inbound spend is under `AI_GLOBAL_DAILY_BUDGET_USD`. A lookup error
/// counts as within budget.
pub async fn within_global_budget(db: &SqlitePool, global_budget_usd: f64) -> bool {
    match global_inbound_spend_today(db).await {
        Ok(spent) => spent < global_budget_usd,
        Err(e) => {
            tracing::warn!(error = %e, "global budget check failed; allowing");
            true
        }
    }
}

// ---------------------------------------------------------------- usage report

#[derive(Debug, Serialize, PartialEq)]
pub struct DayUsage {
    pub date: String,
    pub usd: f64,
    pub calls: i64,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct TaskUsage {
    pub task: String,
    pub usd: f64,
    pub calls: i64,
}

#[derive(Debug, Serialize)]
pub struct AiUsage {
    pub days: i64,
    pub total_usd: f64,
    pub today_usd: f64,
    pub budget_usd_per_day: f64,
    pub by_day: Vec<DayUsage>,
    pub by_task: Vec<TaskUsage>,
    /// Average automatic cost per new inbound email in the window.
    pub per_email_usd: f64,
}

/// Plain name for a task; never a model name.
pub fn task_label(task: &str) -> &'static str {
    match task {
        "classify" => "Labelling",
        "analyze" | "understand" => "Summaries",
        "draft" => "Drafts",
        "learn" => "Memory",
        "extract" => "Matching",
        "embed" => "Search",
        "suggest_labels" => "Label suggestions",
        "judge" => "Checks",
        "vision" | "audio" => "Files",
        t if t.starts_with("agent_") => "Agents",
        "eval" => "Evals",
        "designer" => "Agent design",
        t if t.starts_with("assistant_") => "Assistant",
        _ => "Other",
    }
}

fn round6(v: f64) -> f64 {
    (v * 1_000_000.0).round() / 1_000_000.0
}

pub async fn usage(db: &SqlitePool, workspace_id: &str, days: i64, budget_usd_per_day: f64) -> sqlx::Result<AiUsage> {
    let days = days.clamp(1, 90);
    let today = today_start_ms();
    let since = today - (days - 1) * DAY_MS;

    let day_rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT strftime('%Y-%m-%d', created_at / 1000, 'unixepoch'), SUM(cost_usd), COUNT(*)
         FROM ai_calls WHERE workspace_id = ?1 AND created_at >= ?2 GROUP BY 1",
    )
    .bind(workspace_id)
    .bind(since)
    .fetch_all(db)
    .await?;
    let mut by_date: BTreeMap<String, (f64, i64)> = day_rows.into_iter().map(|(d, usd, n)| (d, (usd, n))).collect();
    let by_day: Vec<DayUsage> = (0..days)
        .map(|i| {
            let secs = (since + i * DAY_MS) / 1000;
            let date = time::OffsetDateTime::from_unix_timestamp(secs).map(|t| t.date().to_string()).unwrap_or_default();
            let (usd, calls) = by_date.remove(&date).unwrap_or_default();
            DayUsage { date, usd: round6(usd), calls }
        })
        .collect();

    let task_rows: Vec<(String, f64, i64)> = sqlx::query_as(
        "SELECT task, SUM(cost_usd), COUNT(*) FROM ai_calls WHERE workspace_id = ?1 AND created_at >= ?2 GROUP BY task",
    )
    .bind(workspace_id)
    .bind(since)
    .fetch_all(db)
    .await?;
    let mut merged: BTreeMap<&'static str, (f64, i64)> = BTreeMap::new();
    let mut automatic = 0.0;
    for (task, usd, calls) in &task_rows {
        let entry = merged.entry(task_label(task)).or_default();
        entry.0 += usd;
        entry.1 += calls;
        if AUTOMATIC_TASKS.contains(&task.as_str()) {
            automatic += usd;
        }
    }
    let mut by_task: Vec<TaskUsage> =
        merged.into_iter().map(|(task, (usd, calls))| TaskUsage { task: task.to_string(), usd: round6(usd), calls }).collect();
    by_task.sort_by(|a, b| b.usd.total_cmp(&a.usd).then_with(|| a.task.cmp(&b.task)));

    let new_emails: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM email_messages WHERE workspace_id = ?1 AND direction = 'inbound' AND received_at >= ?2",
    )
    .bind(workspace_id)
    .bind(since)
    .fetch_one(db)
    .await?;

    Ok(AiUsage {
        days,
        total_usd: round6(task_rows.iter().map(|(_, usd, _)| usd).sum()),
        today_usd: round6(spend_today(db, workspace_id).await?),
        budget_usd_per_day,
        by_day,
        by_task,
        per_email_usd: if new_emails > 0 { round6(automatic / new_emails as f64) } else { 0.0 },
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn known_models_use_the_price_table() {
        let c = cost_usd("qwen/qwen3.8-flash", 1_000_000, 1_000_000, None);
        assert!((c - 0.62).abs() < 1e-9);
        let c = cost_usd("Anthropic/Claude-Haiku-5.5", 2_000, 500, None);
        assert!((c - (2_000.0 * 0.10 + 500.0 * 0.50) / 1e6).abs() < 1e-12);
        assert!((cost_usd("baai/bge-m3", 1_000_000, 0, None) - 0.01).abs() < 1e-12);
        assert!((cost_usd("moonshotai/kimi-k2.6", 1_000_000, 1_000_000, None) - 4.06).abs() < 1e-9);
        assert!((cost_usd("deepseek/deepseek-v4-flash", 1_000_000, 1_000_000, None) - 0.273).abs() < 1e-9);
    }

    #[test]
    fn provider_cost_wins_over_the_table() {
        assert_eq!(cost_with_source("qwen/qwen3.8-flash", 1_000_000, 1_000_000, Some(0.0042)), (0.0042, "provider"));
        assert_eq!(cost_with_source("qwen/qwen3.8-flash", 1_000_000, 0, Some(-1.0)).1, "table");
        assert_eq!(cost_with_source("qwen/qwen3.8-flash", 1_000_000, 0, Some(f64::NAN)).1, "table");
        assert_eq!(cost_with_source("acme/new-model", 10, 10, None), (0.0, "none"));
        assert_eq!(cost_with_source("deepseek/deepseek-v4-flash", 1_000_000, 0, Some(0.0)), (0.0, "provider"));
    }

    #[test]
    fn unknown_model_uses_reported_cost_else_zero() {
        assert_eq!(cost_usd("acme/new-model", 1_000, 1_000, Some(0.0042)), 0.0042);
        assert_eq!(cost_usd("acme/new-model", 1_000, 1_000, None), 0.0);
        assert_eq!(cost_usd("acme/new-model", 1_000, 1_000, Some(-1.0)), 0.0);
    }

    #[test]
    fn reads_usage_from_provider_body() {
        let body = json!({"usage": {"prompt_tokens": 120, "completion_tokens": 30, "cost": 0.0001}});
        assert_eq!(usage_from(&body), Usage { tokens_in: 120, tokens_out: 30, reported_cost: Some(0.0001), ..Usage::default() });
        assert_eq!(usage_from(&json!({})), Usage::default());
    }

    #[test]
    fn reads_cache_reasoning_and_meta_from_the_body() {
        let body = json!({
            "id": "gen-1", "model": "deepseek/deepseek-v4-flash-0901", "provider": "DeepSeek",
            "choices": [{"finish_reason": "length", "message": {"content": "{}"}}],
            "usage": {"prompt_tokens": 1000, "completion_tokens": 50, "cost": 0.00004,
                      "prompt_tokens_details": {"cached_tokens": 800, "cache_write_tokens": 64},
                      "completion_tokens_details": {"reasoning_tokens": 7}}
        });
        let u = usage_from(&body);
        assert_eq!((u.cached_tokens, u.cache_write_tokens, u.reasoning_tokens), (800, 64, 7));
        let m = meta_from(&body, "deepseek/deepseek-v4-flash");
        assert_eq!(m.provider.as_deref(), Some("DeepSeek"));
        assert_eq!(m.answered_model.as_deref(), Some("deepseek/deepseek-v4-flash-0901"));
        assert_eq!((m.finish_reason.as_deref(), m.generation_id.as_deref()), (Some("length"), Some("gen-1")));
        assert_eq!(meta_from(&body, "DeepSeek/DeepSeek-V4-Flash-0901").answered_model, None);
    }

    #[test]
    fn task_labels_are_plain_names() {
        assert_eq!(task_label("classify"), "Labelling");
        assert_eq!(task_label("agent_critic"), "Agents");
        assert_eq!(task_label("embed"), "Search");
        assert_eq!(task_label("eval"), "Evals");
        assert!(MANUAL_TASKS.iter().all(|t| !AUTOMATIC_TASKS.contains(t)));
        assert_eq!(task_label("whatever"), "Other");
    }

    #[test]
    fn context_comes_from_the_request_path() {
        let c = AiContext::from_path("/workspaces/ws_1/messages/msg_9/draft");
        assert_eq!((c.workspace_id.as_deref(), c.message_id.as_deref()), (Some("ws_1"), Some("msg_9")));
        let c = AiContext::from_path("/api/v1/workspaces/ws_2/search");
        assert_eq!((c.workspace_id.as_deref(), c.message_id), (Some("ws_2"), None));
        assert_eq!(AiContext::from_path("/me").workspace_id, None);
        assert_eq!(AiContext::for_job(None, &json!({}), 0).attempt, 1);
        assert_eq!(AiContext::for_job(None, &json!({}), 3).attempt, 3);
    }
}
