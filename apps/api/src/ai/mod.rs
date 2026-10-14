//! AI adapters. Models only *propose*: a category, or draft text. `policy` and the
//! workflow engine decide what actually happens.

pub mod body;
pub mod embeddings;
pub mod fallback;
pub mod fence;
pub mod jev;
pub mod judge;
pub mod label_suggest;
pub mod memory_extractor;
pub mod openrouter;
pub mod policy;
pub mod router;
pub mod routing;
pub mod selftest;
pub mod understand;

use async_trait::async_trait;

use crate::domain::{ClassificationSource, Label, Priority};

pub const PROMPT_VERSION: &str = "classify-v3";
pub const DRAFT_PROMPT_VERSION: &str = "ack-draft-v3";
pub const ANALYZE_PROMPT_VERSION: &str = "analyze-v2";
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
    /// Similar past emails with trustworthy labels (few-shot). Email text is untrusted data.
    pub examples: Vec<LabelExample>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LabelExample {
    pub subject: String,
    pub snippet: String,
    /// Label key the team gave this email.
    pub label: String,
}

/// A reply this team approved or sent to a similar email.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplyExample {
    pub subject: String,
    pub reply: String,
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

/// Which drafting model tier writes the reply. Chosen by `routing::draft_quality`, never shown in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DraftQuality {
    #[default]
    Standard,
    Premium,
}

impl DraftQuality {
    pub fn as_str(self) -> &'static str {
        match self {
            DraftQuality::Standard => "standard",
            DraftQuality::Premium => "premium",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DraftInput {
    /// Label key and its human name.
    pub category: String,
    pub category_name: String,
    /// What the team means by this label (trusted, team-written).
    pub category_description: String,
    pub subject: String,
    pub sender: String,
    pub body_text: String,
    /// Staff-written or staff-accepted memories, already rendered one per line.
    pub memory_notes: Vec<String>,
    pub sender_history: Option<String>,
    /// What the sender asks, from the analysis step. The draft must address each one.
    pub insight: Option<Insight>,
    /// Up to 3 past replies to similar emails (tone and standard answers).
    pub examples: Vec<ReplyExample>,
    pub quality: DraftQuality,
    /// Team-written reply style for the whole workspace (trusted). Empty = none.
    pub reply_instructions: String,
    /// Team-written sign-off; replaces the default one when set.
    pub signature: Option<String>,
    /// Sign-off when `signature` is unset (`packs::default_sign_off`): "HR Operations" for staffing, else the workspace name.
    pub default_sign_off: String,
    /// Short digest of the earlier messages in this conversation (`ai::body::thread_digest`). `body_text` is only
    /// what the sender wrote this time.
    pub thread_digest: Option<String>,
}

/// The AI's reading of an email, produced before any reply is drafted.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Insight {
    pub summary: String,
    pub asks: Vec<String>,
    pub deadline: Option<String>,
    pub needs_reply: bool,
    pub missing_info: Vec<String>,
}

#[async_trait]
pub trait Analyst: Send + Sync {
    async fn analyze(&self, subject: &str, sender: &str, body: &str) -> Result<(Insight, String), AiError>;

    /// Like `analyze`, with a digest of the earlier messages as context (asks answered there are not new).
    async fn analyze_in_thread(&self, subject: &str, sender: &str, body: &str, _thread_digest: Option<&str>) -> Result<(Insight, String), AiError> {
        self.analyze(subject, sender, body).await
    }
}

/// No-AI fallback: questions are sentences ending in '?'.
pub struct RulesAnalyst;

#[async_trait]
impl Analyst for RulesAnalyst {
    async fn analyze(&self, subject: &str, _sender: &str, body: &str) -> Result<(Insight, String), AiError> {
        let asks: Vec<String> = body
            .split(['\n', '.', '!'])
            .map(str::trim)
            .filter(|s| s.ends_with('?') && s.len() > 3)
            .take(5)
            .map(str::to_string)
            .collect();
        let first: String = body.split_whitespace().take(40).collect::<Vec<_>>().join(" ");
        Ok((
            Insight {
                summary: if first.is_empty() { subject.to_string() } else { first },
                // Rules can't tell "no reply needed" safely; only the model may skip a reply.
                needs_reply: true,
                asks,
                deadline: None,
                missing_info: vec![],
            },
            "rules-v1".into(),
        ))
    }
}

#[derive(Debug, Clone)]
pub struct DraftOutput {
    pub body_text: String,
    pub model_id: String,
    /// Asks the draft could not answer; each also appears as `[NEEDS: …]` in the body.
    pub unanswered: Vec<String>,
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
    prepare_text_up_to(input, MAX_MODEL_INPUT_CHARS)
}

/// Like `prepare_text` with a caller-chosen bound.
pub fn prepare_text_up_to(input: &str, max_chars: usize) -> String {
    let truncated = crate::sources::normalize::truncate_chars(input, max_chars);
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
