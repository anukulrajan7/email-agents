pub mod ai;
pub mod audit;
pub mod auth;
pub mod config;
pub mod db;
pub mod domain;
pub mod error;
pub mod http;
pub mod ingest;
pub mod jobs;
pub mod sources;

use std::{sync::Arc, time::Duration};

use tokio::sync::Notify;

use crate::{
    ai::{Classifier, Drafter, fallback, jev::JevClassifier, openrouter::OpenRouterDrafter},
    auth::Authenticator,
    config::Config,
    http::AppState,
    sources::{nango::NangoClient, resend::ResendClient},
};

/// Wires adapters from config. Missing model keys fall back to the deterministic
/// rules classifier / template drafter, which are labelled as such everywhere.
pub async fn build_state(config: Config) -> anyhow::Result<AppState> {
    let db = db::connect(&config.database_url).await?;
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(60))
        .user_agent("email-ai-api/0.1")
        .build()?;

    let classifier: Arc<dyn Classifier> = if config.typesafe_api_key.is_empty() {
        tracing::warn!("TYPESAFE_API_KEY not set: using rules-based fallback classifier");
        Arc::new(fallback::RulesClassifier)
    } else {
        Arc::new(JevClassifier::new(http.clone(), &config.jev_api_url, &config.typesafe_api_key, &config.jev_model_id))
    };
    let drafter: Arc<dyn Drafter> = if config.openrouter_api_key.is_empty() || config.openrouter_model_id.is_empty() {
        tracing::warn!("OPENROUTER_API_KEY/OPENROUTER_MODEL_ID not set: using template drafts");
        Arc::new(fallback::TemplateDrafter)
    } else {
        Arc::new(OpenRouterDrafter::new(
            http.clone(),
            &config.openrouter_api_url,
            &config.openrouter_api_key,
            &config.openrouter_model_id,
        ))
    };

    Ok(AppState {
        auth: Arc::new(Authenticator::new(&config, http.clone())),
        nango: Arc::new(NangoClient::new(http.clone(), &config.nango_api_url, &config.nango_api_key)),
        resend: Arc::new(ResendClient::new(http, &config.resend_api_url, &config.resend_api_key)),
        classifier,
        drafter,
        jobs_notify: Arc::new(Notify::new()),
        config: Arc::new(config),
        db,
    })
}
