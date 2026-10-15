//! Text embeddings for the knowledge store: `baai/bge-m3` (1024-d) through OpenRouter's
//! OpenAI-compatible endpoint, plus a deterministic offline double for tests and local dev.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{AiError, classify_http_error};

/// Must match `knowledge_vec.embedding FLOAT[1024]`.
pub const EMBEDDING_DIMS: usize = 1024;
/// Inputs per provider request.
pub const MAX_EMBED_BATCH: usize = 32;

#[async_trait]
pub trait Embedder: Send + Sync {
    /// One L2-normalised `EMBEDDING_DIMS` vector per input, in input order.
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AiError>;
}

pub fn normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
}

/// Little-endian f32 blob, the format vec0 accepts for `embedding MATCH ?`.
pub fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

// ---------------------------------------------------------------- OpenRouter

pub struct OpenRouterEmbedder {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    ledger: Option<sqlx::SqlitePool>,
}

impl OpenRouterEmbedder {
    pub fn new(http: reqwest::Client, base_url: &str, api_key: &str, model: &str) -> Self {
        Self { http, base_url: base_url.trim_end_matches('/').to_string(), api_key: api_key.into(), model: model.into(), ledger: None }
    }

    pub fn with_ledger(mut self, db: sqlx::SqlitePool) -> Self {
        self.ledger = Some(db);
        self
    }

    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AiError> {
        let body = json!({
            "model": self.model,
            "input": texts,
            "encoding_format": "float",
            "provider": {"data_collection": "deny"}
        });
        let started = std::time::Instant::now();
        let sent = async {
            let res = self
                .http
                .post(format!("{}/embeddings", self.base_url))
                .bearer_auth(&self.api_key)
                .json(&body)
                .timeout(Duration::from_secs(30))
                .send()
                .await
                .map_err(|e| AiError::Retryable(format!("embeddings request failed: {e}")))?;
            if !res.status().is_success() {
                return Err(classify_http_error(res.status(), "OpenRouter embeddings"));
            }
            res.json::<Value>().await.map_err(|e| AiError::Retryable(format!("embeddings decode: {e}")))
        }
        .await;
        let parsed = sent.as_ref().map_err(|e| AiError::Retryable(e.to_string())).and_then(|b| parse_embeddings(b, texts.len()));
        if let Some(db) = &self.ledger {
            let error = parsed.as_ref().err().map(ToString::to_string);
            crate::ai::ledger::log_call(db, "embed", &self.model, started, sent.as_ref().ok(), error.as_deref()).await;
        }
        match sent {
            Ok(_) => parsed,
            Err(e) => Err(e),
        }
    }
}

/// Validates the provider response: one vector per input, right size, re-ordered by `index`.
pub fn parse_embeddings(body: &Value, expected: usize) -> Result<Vec<Vec<f32>>, AiError> {
    let data = body
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| AiError::Retryable("embeddings response had no data".into()))?;
    if data.len() != expected {
        return Err(AiError::Retryable(format!("expected {expected} embeddings, got {}", data.len())));
    }
    let mut out: Vec<Option<Vec<f32>>> = vec![None; expected];
    for (pos, item) in data.iter().enumerate() {
        let index = item.get("index").and_then(Value::as_u64).map(|i| i as usize).unwrap_or(pos);
        let mut v: Vec<f32> = item
            .get("embedding")
            .and_then(Value::as_array)
            .ok_or_else(|| AiError::Retryable("embedding missing".into()))?
            .iter()
            .map(|x| x.as_f64().unwrap_or(0.0) as f32)
            .collect();
        if v.len() != EMBEDDING_DIMS {
            return Err(AiError::Permanent(format!("embedding has {} dims, expected {EMBEDDING_DIMS}", v.len())));
        }
        normalize(&mut v);
        let slot = out.get_mut(index).ok_or_else(|| AiError::Retryable("embedding index out of range".into()))?;
        *slot = Some(v);
    }
    out.into_iter().map(|v| v.ok_or_else(|| AiError::Retryable("embedding missing for an input".into()))).collect()
}

#[async_trait]
impl Embedder for OpenRouterEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AiError> {
        let mut out = Vec::with_capacity(texts.len());
        for batch in texts.chunks(MAX_EMBED_BATCH) {
            out.extend(self.embed_batch(batch).await?);
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------- test double

/// Deterministic, offline: hashed bag of words plus character trigrams (so near-spellings
/// land close together), signed feature hashing into 1024 dims, L2-normalised.
pub struct HashEmbedder;

fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3))
}

pub fn hash_embedding(text: &str) -> Vec<f32> {
    let mut v = vec![0f32; EMBEDDING_DIMS];
    let mut add = |feature: &str, weight: f32| {
        let h = fnv1a(feature);
        let sign = if h >> 63 == 1 { -1.0 } else { 1.0 };
        v[(h % EMBEDDING_DIMS as u64) as usize] += sign * weight;
    };
    for word in text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()) {
        let word = word.to_lowercase();
        add(&format!("w:{word}"), 1.0);
        let chars: Vec<char> = word.chars().collect();
        for tri in chars.windows(3) {
            add(&format!("t:{}", tri.iter().collect::<String>()), 0.5);
        }
    }
    if v.iter().all(|x| *x == 0.0) {
        v[0] = 1.0;
    }
    normalize(&mut v);
    v
}

#[async_trait]
impl Embedder for HashEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AiError> {
        Ok(texts.iter().map(|t| hash_embedding(t)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| x * y).sum()
    }

    #[test]
    fn hash_embedding_is_deterministic_normalised_and_meaningful() {
        let a = hash_embedding("September timesheet attached");
        assert_eq!(a.len(), EMBEDDING_DIMS);
        assert_eq!(a, hash_embedding("September timesheet attached"));
        assert!((cosine(&a, &a) - 1.0).abs() < 1e-5);
        let near = hash_embedding("my timeshet for september");
        let far = hash_embedding("office party cake in the kitchen");
        assert!(cosine(&a, &near) > cosine(&a, &far) + 0.2);
        assert!((cosine(&hash_embedding("!!!"), &hash_embedding("")) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn parses_and_orders_provider_embeddings() {
        let vec_of = |x: f64| json!(vec![x; EMBEDDING_DIMS]);
        let body = json!({"data": [{"index": 1, "embedding": vec_of(2.0)}, {"index": 0, "embedding": vec_of(-1.0)}]});
        let out = parse_embeddings(&body, 2).unwrap();
        assert!(out[0][0] < 0.0 && out[1][0] > 0.0);
        assert!((cosine(&out[1], &out[1]) - 1.0).abs() < 1e-4);
        assert!(parse_embeddings(&body, 3).is_err());
        let short = json!({"data": [{"index": 0, "embedding": [1.0, 2.0]}]});
        assert!(matches!(parse_embeddings(&short, 1), Err(AiError::Permanent(_))));
    }

    #[test]
    fn blob_is_little_endian_f32() {
        assert_eq!(to_blob(&[1.0, -2.0]), [1f32.to_le_bytes(), (-2f32).to_le_bytes()].concat());
    }
}
