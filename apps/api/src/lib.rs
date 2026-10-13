pub mod ai;
pub mod alerts;
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
pub mod knowledge;
pub mod labels;
pub mod mailer;
pub mod matching;
pub mod replies;
pub mod reply_settings;
pub mod memory;
pub mod memory_learning;
pub mod sessions;
pub mod skills;
pub mod sources;
pub mod text_clean;
pub mod threads;

use std::{sync::Arc, time::Duration};

use tokio::sync::Notify;

use crate::{
    ai::{
        Classifier, Drafter, fallback,
        router::RouterDrafter,
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
    // Two drafting tiers; with only one model configured, both tiers use it.
    let drafter: Arc<dyn Drafter> = {
        let standard = [&config.openrouter_draft_model_id, &config.openrouter_premium_model_id]
            .into_iter()
            .find(|m| !m.is_empty());
        let premium = [&config.openrouter_premium_model_id, &config.openrouter_draft_model_id].into_iter().find(|m| !m.is_empty());
        match (&openrouter, standard, premium) {
            (Some(client), Some(standard), Some(premium)) => {
                tracing::info!(standard = %standard, premium = %premium, "drafting with OpenRouter");
                Arc::new(RouterDrafter::new(
                    Arc::new(OpenRouterDrafter::new(client.clone(), standard)),
                    Arc::new(OpenRouterDrafter::new(client.clone(), premium)),
                ))
            }
            _ => {
                tracing::warn!("OPENROUTER_API_KEY or a drafting model is not set: using template drafts");
                Arc::new(fallback::TemplateDrafter)
            }
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

    // Understanding must feel instant (it's the first thing people see): use the fast model,
    // falling back to the drafting model when no fast model is configured.
    let analyst_model = if config.openrouter_classifier_model_id.is_empty() { &config.openrouter_model_id } else { &config.openrouter_classifier_model_id };
    let analyst: Arc<dyn ai::Analyst> = match &openrouter {
        Some(client) if !analyst_model.is_empty() => Arc::new(OpenRouterAnalyst::new(client.clone(), analyst_model)),
        _ => Arc::new(ai::RulesAnalyst),
    };

    let embedder: Arc<dyn ai::embeddings::Embedder> =
        if openrouter.is_some() && !config.openrouter_embedding_model_id.is_empty() {
            Arc::new(ai::embeddings::OpenRouterEmbedder::new(
                http.clone(),
                &config.openrouter_api_url,
                &config.openrouter_api_key,
                &config.openrouter_embedding_model_id,
            ))
        } else {
            tracing::warn!("no embedding model configured: using offline hash embeddings");
            Arc::new(ai::embeddings::HashEmbedder)
        };

    // Label suggestions read many emails at once and feed the taxonomy: use the drafting model.
    let label_suggester: Arc<dyn ai::label_suggest::LabelSuggester> = match &openrouter {
        Some(client) if !config.openrouter_model_id.is_empty() => {
            Arc::new(ai::label_suggest::OpenRouterLabelSuggester::new(client.clone(), &config.openrouter_model_id))
        }
        _ => Arc::new(ai::label_suggest::RulesLabelSuggester),
    };

    // Memory learning: efficient model for incoming mail, drafting model for the team's replies.
    let memory_extractor: Arc<dyn ai::memory_extractor::MemoryExtractor> = match &openrouter {
        Some(client) if !(config.openrouter_classifier_model_id.is_empty() && config.openrouter_model_id.is_empty()) => {
            let incoming = if config.openrouter_classifier_model_id.is_empty() { &config.openrouter_model_id } else { &config.openrouter_classifier_model_id };
            let reply = if config.openrouter_model_id.is_empty() { incoming } else { &config.openrouter_model_id };
            Arc::new(ai::memory_extractor::OpenRouterMemoryExtractor::new(client.clone(), incoming, reply))
        }
        _ => Arc::new(ai::memory_extractor::NoopMemoryExtractor),
    };

    Ok(AppState {
        memory_extractor,
        alert_mailer: mailer.clone(),
        analyst,
        embedder,
        label_suggester,
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
