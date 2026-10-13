pub mod ai;
pub mod audit;
pub mod automations;
pub mod auth;
pub mod config;
pub mod db;
pub mod domain;
pub mod error;
pub mod http;
pub mod ingest;
pub mod jobs;
pub mod labels;
pub mod mailer;
pub mod matching;
pub mod replies;
pub mod memory;
pub mod sessions;
pub mod sources;

use std::{sync::Arc, time::Duration};

use tokio::sync::Notify;

use crate::{
    ai::{
        Classifier, Drafter, fallback,
        jev::JevClassifier,
        openrouter::{OpenRouterAnalyst, OpenRouterClassifier, OpenRouterClient, OpenRouterDrafter, OpenRouterExtractor, OpenRouterLearner},
    },
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

    let openrouter = (!config.openrouter_api_key.is_empty())
        .then(|| OpenRouterClient::new(http.clone(), &config.openrouter_api_url, &config.openrouter_api_key));

    // Jev if configured, else an efficient OpenRouter model, else keyword rules.
    let classifier: Arc<dyn Classifier> = match (&openrouter, config.typesafe_api_key.is_empty()) {
        (_, false) => {
            Arc::new(JevClassifier::new(http.clone(), &config.jev_api_url, &config.typesafe_api_key, &config.jev_model_id))
        }
        (Some(client), true) if !config.openrouter_classifier_model_id.is_empty() => {
            tracing::info!(model = %config.openrouter_classifier_model_id, "classifying with OpenRouter");
            Arc::new(
                OpenRouterClassifier::new(client.clone(), &config.openrouter_classifier_model_id)
                    .with_escalation(&config.openrouter_model_id),
            )
        }
        _ => {
            tracing::warn!("no classifier model configured: using rules-based fallback classifier");
            Arc::new(fallback::RulesClassifier)
        }
    };
    let drafter: Arc<dyn Drafter> = match &openrouter {
        Some(client) if !config.openrouter_model_id.is_empty() => {
            tracing::info!(model = %config.openrouter_model_id, "drafting with OpenRouter");
            Arc::new(OpenRouterDrafter::new(client.clone(), &config.openrouter_model_id))
        }
        _ => {
            tracing::warn!("OPENROUTER_API_KEY/OPENROUTER_MODEL_ID not set: using template drafts");
            Arc::new(fallback::TemplateDrafter)
        }
    };

    // Learning uses the efficient model; it only ever produces suggestions.
    let learner: Option<Arc<dyn ai::Learner>> = match &openrouter {
        Some(client) if !config.openrouter_classifier_model_id.is_empty() => {
            Some(Arc::new(OpenRouterLearner::new(client.clone(), &config.openrouter_classifier_model_id)))
        }
        _ => None,
    };

    // Verification emails are on only when SMTP is fully configured.
    let mailer: Option<Arc<dyn mailer::Mailer>> = if config.smtp_host.is_empty() || config.smtp_username.is_empty() {
        tracing::warn!("SMTP not configured: email verification disabled");
        None
    } else {
        let from = if config.smtp_from.is_empty() { format!("Post Room <{}>", config.smtp_username) } else { config.smtp_from.clone() };
        Some(Arc::new(mailer::SmtpMailer::new(
            &config.smtp_host,
            config.smtp_port,
            &config.smtp_username,
            &config.smtp_password,
            &from,
        )?))
    };

    let extractor: Arc<dyn ai::Extractor> = match &openrouter {
        Some(client) if !config.openrouter_classifier_model_id.is_empty() => {
            Arc::new(OpenRouterExtractor::new(client.clone(), &config.openrouter_classifier_model_id))
        }
        _ => Arc::new(ai::RulesExtractor),
    };

    // Understanding uses the stronger model: misreading an email is costlier than tokens.
    let analyst: Arc<dyn ai::Analyst> = match &openrouter {
        Some(client) if !config.openrouter_model_id.is_empty() => Arc::new(OpenRouterAnalyst::new(client.clone(), &config.openrouter_model_id)),
        _ => Arc::new(ai::RulesAnalyst),
    };

    Ok(AppState {
        analyst,
        extractor,
        mailer,
        learner,
        login_limiter: Arc::new(sessions::LoginLimiter::default()),
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
