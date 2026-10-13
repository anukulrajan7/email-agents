//! AI adapters. Models only *propose*: a category, or draft text. `policy` and the
//! workflow engine decide what actually happens.

pub mod fallback;
pub mod jev;
pub mod openrouter;
pub mod policy;

use async_trait::async_trait;

use crate::domain::{ClassificationSource, Label, Priority};

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
    /// Learned profile line for the sender (trusted, from our own data).
    pub sender_history: Option<String>,
    /// The workspace taxonomy. Output must be one of these keys (else coerced to `other`).
    pub labels: Vec<Label>,
}

#[derive(Debug, Clone)]
pub struct ClassifyOutput {
    /// Label key.
    pub category: String,
    pub priority: Priority,
    pub requires_reply: bool,
    pub confidence: Option<f64>,
    pub source: ClassificationSource,
    pub model_id: Option<String>,
    pub model_version: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DraftInput {
    /// Label key and its human name.
    pub category: String,
    pub category_name: String,
    pub subject: String,
    pub sender: String,
    pub body_text: String,
    /// Staff-written or staff-accepted memories, already rendered one per line.
    pub memory_notes: Vec<String>,
    pub sender_history: Option<String>,
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

/// Turns an email (+ attachment text) into typed staffing records.
#[async_trait]
pub trait Extractor: Send + Sync {
    async fn requirement(&self, subject: &str, text: &str, sender_domain: Option<&str>)
        -> Result<crate::matching::RequirementFields, AiError>;
    async fn candidate(&self, subject: &str, text: &str, sender: &str) -> Result<crate::matching::CandidateFields, AiError>;
}

/// Keyword/regex extraction: used when no model is configured, and in tests.
pub struct RulesExtractor;

#[async_trait]
impl Extractor for RulesExtractor {
    async fn requirement(&self, subject: &str, text: &str, sender_domain: Option<&str>)
        -> Result<crate::matching::RequirementFields, AiError> {
        Ok(crate::matching::rules_requirement(subject, text, sender_domain))
    }
    async fn candidate(&self, subject: &str, text: &str, sender: &str) -> Result<crate::matching::CandidateFields, AiError> {
        Ok(crate::matching::rules_candidate(subject, text, sender))
    }
}

/// A reusable lesson proposed from how an operator changed a draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnedLesson {
    pub kind: String,  // "preference" | "style"
    pub scope: String, // "workspace" | "contact"
    pub content: String,
}

#[async_trait]
pub trait Learner: Send + Sync {
    async fn learn(&self, suggested: &str, approved: &str) -> Result<Vec<LearnedLesson>, AiError>;
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
