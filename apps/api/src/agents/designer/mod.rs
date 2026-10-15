//! The system agent designer: a person describes a job in plain English, a best-tier model drafts an
//! `AgentSpec`, code validates and repairs it (at most twice), a dry run shows what it would do on
//! recent mail without writing anything, and saving creates a new agent (or a new version of one).
//! The model only proposes. Everything it returns passes `validate` before a person sees it as ready.

pub mod dry_run;
pub mod lenient;
pub mod prompt;
pub mod repair;
pub mod schema;
pub mod session;
pub mod validate;

use std::{collections::VecDeque, sync::Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ai::{AiError, openrouter::OpenRouterClient};

/// Designer calls per workspace per UTC day (one per message a person sends; repairs ride along).
pub const DAILY_LIMIT: i64 = 20;
/// Most a single session may spend on the model.
pub const SESSION_BUDGET_USD: f64 = 0.10;
/// Repair rounds after the first answer.
pub const MAX_REPAIRS: usize = 2;
/// Turns kept per session (a person's message and the designer's answer count one each).
pub const MAX_TURNS: usize = 20;
/// Longest description a person can give.
pub const MAX_GOAL_CHARS: usize = 2_000;
pub const DESIGNER_MAX_TOKENS: u32 = 2_500;

/// One message of a designer conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    /// `user` or `designer`.
    pub role: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct DesignRequest {
    pub system: String,
    pub user: String,
    pub schema: Value,
}

#[derive(Debug, Clone, Default)]
pub struct DesignReply {
    /// The model's JSON answer, unparsed.
    pub json: String,
    pub cost_usd: f64,
}

#[async_trait]
pub trait Designer: Send + Sync {
    /// False when no model is configured: the designer is off.
    fn enabled(&self) -> bool {
        true
    }
    async fn design(&self, req: &DesignRequest) -> Result<DesignReply, AiError>;
    /// One try on the second model, asked when the first model's answers could not be read.
    async fn design_fallback(&self, _req: &DesignRequest) -> Result<DesignReply, AiError> {
        Err(AiError::Permanent("no fallback designer model".into()))
    }
}

pub struct DisabledDesigner;

#[async_trait]
impl Designer for DisabledDesigner {
    fn enabled(&self) -> bool {
        false
    }
    async fn design(&self, _req: &DesignRequest) -> Result<DesignReply, AiError> {
        Err(AiError::Permanent("no designer model is configured".into()))
    }
}

/// The best-tier model, with a second model that steps in when the first fails.
pub struct OpenRouterDesigner {
    client: OpenRouterClient,
    model: String,
    fallback: String,
}

impl OpenRouterDesigner {
    pub fn new(client: OpenRouterClient, model: &str, fallback: &str) -> Self {
        Self { client: client.with_task("designer"), model: model.into(), fallback: fallback.into() }
    }

    async fn ask(&self, model: &str, req: &DesignRequest) -> Result<DesignReply, AiError> {
        let (json, answered_by, usage) =
            self.client.chat_json_usage(model, &req.system, req.user.clone(), "agent_spec", req.schema.clone(), DESIGNER_MAX_TOKENS).await?;
        let cost_usd = crate::ai_ledger::cost_usd(&answered_by, usage.tokens_in, usage.tokens_out, usage.cost_usd);
        Ok(DesignReply { json, cost_usd })
    }
}

#[async_trait]
impl Designer for OpenRouterDesigner {
    async fn design(&self, req: &DesignRequest) -> Result<DesignReply, AiError> {
        match self.ask(&self.model, req).await {
            Err(e) if !self.fallback.is_empty() && self.fallback != self.model => {
                tracing::warn!(error = %e, "designer model failed; trying the fallback");
                self.ask(&self.fallback, req).await
            }
            other => other,
        }
    }

    async fn design_fallback(&self, req: &DesignRequest) -> Result<DesignReply, AiError> {
        if self.fallback.is_empty() || self.fallback == self.model {
            return Err(AiError::Permanent("no fallback designer model".into()));
        }
        self.ask(&self.fallback, req).await
    }
}

/// Replays chosen answers in order and remembers every request. Used by tests and offline runs.
pub struct ScriptedDesigner {
    replies: Mutex<VecDeque<Result<String, AiError>>>,
    requests: Mutex<Vec<DesignRequest>>,
    fallback_requests: Mutex<Vec<DesignRequest>>,
    cost_per_call: f64,
}

impl Default for ScriptedDesigner {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptedDesigner {
    pub fn new() -> Self {
        Self { replies: Mutex::default(), requests: Mutex::default(), fallback_requests: Mutex::default(), cost_per_call: 0.0 }
    }

    pub fn with_cost(mut self, usd: f64) -> Self {
        self.cost_per_call = usd;
        self
    }

    /// The next answer, as the JSON the model would return.
    pub fn reply(self, spec: Value) -> Self {
        self.reply_raw(&spec.to_string())
    }

    pub fn reply_raw(self, text: &str) -> Self {
        self.replies.lock().expect("replies lock").push_back(Ok(text.to_string()));
        self
    }

    pub fn fail(self, error: AiError) -> Self {
        self.replies.lock().expect("replies lock").push_back(Err(error));
        self
    }

    /// Adds an answer after construction (the designer may already be shared).
    pub fn push(&self, spec: Value) {
        self.replies.lock().expect("replies lock").push_back(Ok(spec.to_string()));
    }

    pub fn push_raw(&self, text: &str) {
        self.replies.lock().expect("replies lock").push_back(Ok(text.to_string()));
    }

    pub fn requests(&self) -> Vec<DesignRequest> {
        self.requests.lock().expect("requests lock").clone()
    }

    /// Requests that were sent to the fallback model (they also take the next scripted answer).
    pub fn fallback_calls(&self) -> usize {
        self.fallback_requests.lock().expect("fallback lock").len()
    }

    pub fn calls(&self) -> usize {
        self.requests.lock().expect("requests lock").len()
    }
}

#[async_trait]
impl Designer for ScriptedDesigner {
    async fn design(&self, req: &DesignRequest) -> Result<DesignReply, AiError> {
        self.requests.lock().expect("requests lock").push(req.clone());
        match self.replies.lock().expect("replies lock").pop_front() {
            Some(Ok(json)) => Ok(DesignReply { json, cost_usd: self.cost_per_call }),
            Some(Err(e)) => Err(e),
            None => Err(AiError::Permanent("the script has no more answers".into())),
        }
    }

    async fn design_fallback(&self, req: &DesignRequest) -> Result<DesignReply, AiError> {
        self.fallback_requests.lock().expect("fallback lock").push(req.clone());
        self.design(req).await
    }
}
