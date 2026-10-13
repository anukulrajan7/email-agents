//! AI adapters. Models only *propose*: a category, or draft text. `policy` and the
//! workflow engine decide what actually happens.

pub mod fallback;
pub mod jev;
pub mod openrouter;
pub mod policy;

use async_trait::async_trait;

use crate::domain::{Category, ClassificationSource, Priority};

pub const PROMPT_VERSION: &str = "classify-v1";
pub const DRAFT_PROMPT_VERSION: &str = "ack-draft-v1";
/// Upper bound on email text sent to any model.
pub const MAX_MODEL_INPUT_CHARS: usize = 6_000;

#[derive(Debug, Clone)]
pub struct ClassifyInput {
    pub subject: String,
    pub sender: String,
    pub body_text: String,
    pub attachment_names: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ClassifyOutput {
    pub category: Category,
    pub priority: Priority,
    pub requires_reply: bool,
    pub confidence: Option<f64>,
    pub source: ClassificationSource,
    pub model_id: Option<String>,
    pub model_version: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DraftInput {
    pub category: Category,
    pub subject: String,
    pub sender: String,
    pub body_text: String,
}

#[derive(Debug, Clone)]
pub struct DraftOutput {
    pub body_text: String,
    pub model_id: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AiError {
    #[error("retryable model error: {0}")]
    Retryable(String),
    #[error("permanent model error: {0}")]
    Permanent(String),
}

#[async_trait]
pub trait Classifier: Send + Sync {
    async fn classify(&self, input: &ClassifyInput) -> Result<ClassifyOutput, AiError>;
}

#[async_trait]
pub trait Drafter: Send + Sync {
    async fn draft(&self, input: &DraftInput) -> Result<DraftOutput, AiError>;
}

/// Minimal context for models: bounded, with obvious secrets masked.
pub fn prepare_text(input: &str) -> String {
    let truncated = crate::sources::normalize::truncate_chars(input, MAX_MODEL_INPUT_CHARS);
    truncated
        .lines()
        .map(|line| {
            let lower = line.to_lowercase();
            if ["password", "passcode", "otp", "api key", "apikey", "secret", "ssn", "iban", "account number"]
                .iter()
                .any(|k| lower.contains(k))
            {
                "[redacted line]".to_string()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn classify_http_error(status: reqwest::StatusCode, what: &str) -> AiError {
    let msg = format!("{what} returned {status}");
    if status.as_u16() == 429 || status.is_server_error() || status.as_u16() == 408 {
        AiError::Retryable(msg)
    } else {
        AiError::Permanent(msg)
    }
}
