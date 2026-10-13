//! Speech to text through OpenRouter's OpenAI-compatible `/audio/transcriptions` (multipart).

use std::{sync::Mutex, time::Duration};

use async_trait::async_trait;
use serde_json::Value;

use crate::{
    ai::{AiError, classify_http_error},
    ai_ledger,
};

pub const MAX_AUDIO_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_AUDIO_SECONDS: u64 = 60;
/// Fallback when the provider reports no cost (USD per hour of audio).
const EST_USD_PER_HOUR: f64 = 0.03;
const TASK: &str = "assistant_stt";

#[derive(Debug, Clone)]
pub struct AudioClip {
    pub bytes: Vec<u8>,
    pub content_type: String,
    pub seconds: Option<f64>,
}

impl AudioClip {
    pub fn file_name(&self) -> &'static str {
        let ct = self.content_type.to_ascii_lowercase();
        if ct.contains("mp4") || ct.contains("m4a") || ct.contains("aac") {
            "audio.m4a"
        } else if ct.contains("ogg") {
            "audio.ogg"
        } else if ct.contains("wav") {
            "audio.wav"
        } else if ct.contains("mpeg") || ct.contains("mp3") {
            "audio.mp3"
        } else {
            "audio.webm"
        }
    }
}

#[async_trait]
pub trait Transcriber: Send + Sync {
    /// False when no speech model is configured.
    fn enabled(&self) -> bool {
        true
    }
    async fn transcribe(&self, clip: &AudioClip) -> Result<String, AiError>;
}

pub struct OpenRouterTranscriber {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    ledger: Option<sqlx::SqlitePool>,
}

impl OpenRouterTranscriber {
    pub fn new(http: reqwest::Client, base_url: &str, api_key: &str, model: &str) -> Self {
        Self { http, base_url: base_url.trim_end_matches('/').to_string(), api_key: api_key.into(), model: model.into(), ledger: None }
    }

    pub fn with_ledger(mut self, db: sqlx::SqlitePool) -> Self {
        self.ledger = Some(db);
        self
    }

    async fn send(&self, clip: &AudioClip) -> Result<Value, AiError> {
        let part = reqwest::multipart::Part::bytes(clip.bytes.clone())
            .file_name(clip.file_name())
            .mime_str(&clip.content_type)
            .map_err(|_| AiError::Permanent("unsupported audio type".into()))?;
        let form = reqwest::multipart::Form::new()
            .text("model", self.model.clone())
            .text("response_format", "json")
            .part("file", part);
        let res = self
            .http
            .post(format!("{}/audio/transcriptions", self.base_url))
            .bearer_auth(&self.api_key)
            .multipart(form)
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .map_err(|e| AiError::Retryable(format!("transcription request failed: {e}")))?;
        if !res.status().is_success() {
            return Err(classify_http_error(res.status(), "OpenRouter transcription"));
        }
        res.json::<Value>().await.map_err(|e| AiError::Retryable(format!("transcription decode: {e}")))
    }
}

#[async_trait]
impl Transcriber for OpenRouterTranscriber {
    async fn transcribe(&self, clip: &AudioClip) -> Result<String, AiError> {
        let started = std::time::Instant::now();
        let sent = self.send(clip).await;
        if let Some(db) = &self.ledger {
            let estimate = clip.seconds.unwrap_or(MAX_AUDIO_SECONDS as f64 / 2.0) / 3600.0 * EST_USD_PER_HOUR;
            let reported = sent.as_ref().ok().and_then(|b| b.pointer("/usage/cost").and_then(Value::as_f64)).unwrap_or(estimate);
            let usage = ai_ledger::Usage { reported_cost: Some(reported), ..Default::default() };
            let error = sent.as_ref().err().map(ToString::to_string);
            ai_ledger::record(db, TASK, &self.model, usage, started.elapsed().as_millis() as i64, error.as_deref()).await;
        }
        let body = sent?;
        let text = body.get("text").and_then(Value::as_str).ok_or_else(|| AiError::Retryable("transcription had no text".into()))?;
        Ok(text.split_whitespace().collect::<Vec<_>>().join(" "))
    }
}

/// Used when no speech model is configured.
pub struct UnavailableTranscriber;

#[async_trait]
impl Transcriber for UnavailableTranscriber {
    fn enabled(&self) -> bool {
        false
    }
    async fn transcribe(&self, _clip: &AudioClip) -> Result<String, AiError> {
        Err(AiError::Permanent("speech to text is not configured".into()))
    }
}

/// What a test double was given.
#[derive(Debug, Clone, PartialEq)]
pub struct SeenClip {
    pub content_type: String,
    pub len: usize,
    pub seconds: Option<f64>,
}

/// Test double: returns fixed text and remembers the clips it received.
pub struct ScriptedTranscriber {
    text: String,
    clips: Mutex<Vec<SeenClip>>,
}

impl ScriptedTranscriber {
    pub fn new(text: &str) -> Self {
        Self { text: text.into(), clips: Mutex::default() }
    }

    pub fn clips(&self) -> Vec<SeenClip> {
        self.clips.lock().expect("clips lock").clone()
    }
}

#[async_trait]
impl Transcriber for ScriptedTranscriber {
    async fn transcribe(&self, clip: &AudioClip) -> Result<String, AiError> {
        self.clips.lock().expect("clips lock").push(SeenClip {
            content_type: clip.content_type.clone(),
            len: clip.bytes.len(),
            seconds: clip.seconds,
        });
        Ok(self.text.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_follow_the_content_type() {
        let clip = |ct: &str| AudioClip { bytes: vec![], content_type: ct.into(), seconds: None };
        assert_eq!(clip("audio/webm;codecs=opus").file_name(), "audio.webm");
        assert_eq!(clip("audio/mp4").file_name(), "audio.m4a");
        assert_eq!(clip("audio/ogg").file_name(), "audio.ogg");
        assert_eq!(clip("audio/mpeg").file_name(), "audio.mp3");
    }
}
