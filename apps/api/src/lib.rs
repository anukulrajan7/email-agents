pub mod agent_profiles;
pub mod agent_registry;
pub mod agents;
pub mod ai;
pub mod ai_health;
pub mod ai_ledger;
pub mod alerts;
pub mod assistant;
pub mod audit;
pub mod automations;
pub mod attachments;
pub mod auth;
pub mod backup;
pub mod bulk;
pub mod candidate_watch;
pub mod config;
pub mod db;
pub mod designer;
pub mod domain;
pub mod error;
pub mod gap_resolver;
pub mod evals;
pub mod format;
pub mod http;
pub mod incidents;
pub mod ingest;
pub mod jobs;
pub mod knowledge;
pub mod learning;
pub mod labels;
pub mod mailer;
pub mod matching;
pub mod messaging;
pub mod metrics;
pub mod owner_said;
pub mod packs;
pub mod replies;
pub mod send_safety;
pub mod security;
pub mod signoff;
pub mod reply_settings;
pub mod repair;
pub mod memory;
pub mod memory_learning;
pub mod sessions;
pub mod skills;
pub mod sources;
pub mod staffing;
pub mod team;
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

    // `AI_MOCK=1` (E2E, never in production): no provider is contacted whatever keys are set.
    let (openrouter_key, typesafe_key) = if config.ai_mock {
        tracing::warn!("AI_MOCK=1: all AI providers are replaced by offline fakes");
        (String::new(), String::new())
    } else {
        (config.openrouter_api_key.clone(), config.typesafe_api_key.clone())
    };
    let config = Config { openrouter_api_key: openrouter_key, typesafe_api_key: typesafe_key, ..config };

    let openrouter = (!config.openrouter_api_key.is_empty())
        .then(|| OpenRouterClient::new(http.clone(), &config.openrouter_api_url, &config.openrouter_api_key).with_ledger(db.clone()));

    // Jev if configured, else an efficient OpenRouter model, else keyword rules.
    let classifier: Arc<dyn Classifier> = match (&openrouter, config.typesafe_api_key.is_empty()) {
        (_, false) => {
            Arc::new(JevClassifier::new(http.clone(), &config.jev_api_url, &config.typesafe_api_key, &config.jev_model_id))
        }
        (Some(client), true) if !config.openrouter_classifier_model_id.is_empty() => {
            tracing::info!(model = %config.openrouter_classifier_model_id, "classifying with OpenRouter");
            Arc::new(
                OpenRouterClassifier::new(client.clone(), &config.openrouter_classifier_model_id)
                    .with_escalation(&config.openrouter_escalation_model_id),
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

    // Verification emails are on only when SMTP is fully configured (or E2E mail capture is on).
    let mailer: Option<Arc<dyn mailer::Mailer>> = if !config.mail_capture_file.is_empty() {
        tracing::warn!(file = %config.mail_capture_file, "MAIL_CAPTURE_FILE: outgoing mail is written to a file, not sent");
        Some(Arc::new(mailer::FileMailer::new(&config.mail_capture_file)))
    } else if config.smtp_host.is_empty() || config.smtp_username.is_empty() {
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

    // Email understanding v2: used only when UNDERSTAND_V2 is shadow or on; without a model it is the rules reader.
    let understand_model = [&config.openrouter_understand_model_id, &config.openrouter_classifier_model_id, &config.openrouter_model_id]
        .into_iter()
        .find(|m| !m.is_empty())
        .cloned()
        .unwrap_or_default();
    let understander: Arc<dyn ai::understand::Understander> = match &openrouter {
        Some(client) if !understand_model.is_empty() => Arc::new(ai::understand::OpenRouterUnderstander::new(client.clone(), &understand_model)),
        _ => Arc::new(ai::understand::RulesUnderstander),
    };

    let embedder: Arc<dyn ai::embeddings::Embedder> =
        if openrouter.is_some() && !config.openrouter_embedding_model_id.is_empty() {
            Arc::new(
                ai::embeddings::OpenRouterEmbedder::new(
                    http.clone(),
                    &config.openrouter_api_url,
                    &config.openrouter_api_key,
                    &config.openrouter_embedding_model_id,
                )
                .with_ledger(db.clone()),
            )
        } else {
            tracing::warn!("no embedding model configured: using offline hash embeddings");
            Arc::new(ai::embeddings::HashEmbedder)
        };

    // Label suggestions read many emails at once and feed the taxonomy: their own model variable.
    let suggest_model = [&config.openrouter_suggest_model_id, &config.openrouter_classifier_model_id].into_iter().find(|m| !m.is_empty());
    let label_suggester: Arc<dyn ai::label_suggest::LabelSuggester> = match (&openrouter, suggest_model) {
        (Some(client), Some(model)) => Arc::new(ai::label_suggest::OpenRouterLabelSuggester::new(client.clone(), model)),
        _ => Arc::new(ai::label_suggest::RulesLabelSuggester),
    };

    // Memory learning: the efficient model for incoming mail, OPENROUTER_LEARN_MODEL_ID for the team's replies.
    let memory_extractor: Arc<dyn ai::memory_extractor::MemoryExtractor> = match &openrouter {
        Some(client) if !(config.openrouter_classifier_model_id.is_empty() && config.openrouter_learn_model_id.is_empty()) => {
            let incoming = if config.openrouter_classifier_model_id.is_empty() { &config.openrouter_learn_model_id } else { &config.openrouter_classifier_model_id };
            let reply = if config.openrouter_learn_model_id.is_empty() { incoming } else { &config.openrouter_learn_model_id };
            Arc::new(ai::memory_extractor::OpenRouterMemoryExtractor::new(client.clone(), incoming, reply))
        }
        _ => Arc::new(ai::memory_extractor::NoopMemoryExtractor),
    };

    // Agents: a tool-calling planner (premium tier for high-priority requirement cases) and a cheap critic.
    let (planner, critic): (Arc<dyn agents::planner::Planner>, Arc<dyn agents::critic::Critic>) = match &openrouter {
        Some(client) if !config.openrouter_planner_model_id.is_empty() => {
            let critic_model = [&config.openrouter_classifier_model_id, &config.openrouter_model_id, &config.openrouter_planner_model_id]
                .into_iter()
                .find(|m| !m.is_empty())
                .cloned()
                .unwrap_or_default();
            tracing::info!(planner = %config.openrouter_planner_model_id, "agents enabled");
            (
                Arc::new(agents::planner::OpenRouterPlanner::new(
                    client.with_task("agent_plan"),
                    &config.openrouter_planner_model_id,
                    &config.openrouter_premium_model_id,
                )),
                Arc::new(agents::critic::OpenRouterCritic::new(client.with_task("agent_critic"), &critic_model)),
            )
        }
        _ => {
            tracing::warn!("no planner model configured: agents are off");
            (Arc::new(agents::planner::DisabledPlanner), Arc::new(agents::critic::PermissiveCritic))
        }
    };

    // The command assistant: the planner model with its own tool set, plus speech to text.
    let assistant_planner: Arc<dyn assistant::CommandPlanner> = match &openrouter {
        Some(client) if !config.openrouter_planner_model_id.is_empty() => Arc::new(assistant::OpenRouterCommandPlanner::new(
            client.with_task("assistant_plan"),
            &config.openrouter_planner_model_id,
        )),
        _ => Arc::new(assistant::DisabledCommandPlanner),
    };
    let transcriber: Arc<dyn assistant::transcribe::Transcriber> =
        if openrouter.is_some() && !config.openrouter_audio_model_id.is_empty() {
            Arc::new(
                assistant::transcribe::OpenRouterTranscriber::new(
                    http.clone(),
                    &config.openrouter_api_url,
                    &config.openrouter_api_key,
                    &config.openrouter_audio_model_id,
                )
                .with_ledger(db.clone()),
            )
        } else {
            tracing::warn!("no speech model configured: voice input is off");
            Arc::new(assistant::transcribe::UnavailableTranscriber)
        };

    // The designer drafts agents from plain English with the best-tier model; without one it is off.
    let designer: Arc<dyn designer::Designer> = match &openrouter {
        Some(client) if !config.designer_model_id.is_empty() => {
            Arc::new(designer::OpenRouterDesigner::new(client.clone(), &config.designer_model_id, &config.designer_fallback_model_id))
        }
        _ => Arc::new(designer::DisabledDesigner),
    };
    // The workflow `extract` step reads named details with the efficient model; without one, plain rules read them.
    let field_extractor: Arc<dyn ai::field_extract::FieldExtractor> = match &openrouter {
        Some(client) if !analyst_model.is_empty() => Arc::new(ai::field_extract::OpenRouterFieldExtractor::new(client.clone(), analyst_model)),
        _ => Arc::new(ai::field_extract::RulesFieldExtractor),
    };

    // Typed decisions: Jev if configured, else the efficient OpenRouter model, else none (callers fail open).
    let judge: Arc<dyn ai::judge::Judge> = match &openrouter {
        _ if !config.typesafe_api_key.is_empty() => Arc::new(ai::judge::JevJudge::new(
            http.clone(),
            &config.jev_api_url,
            &config.typesafe_api_key,
            &config.jev_model_id,
        )),
        Some(client) if !analyst_model.is_empty() => Arc::new(ai::judge::OpenRouterJudge::new(client.with_task("judge"), analyst_model)),
        _ => {
            tracing::warn!("no judge model configured: AI review of sends and alert gating are off");
            Arc::new(ai::judge::UnavailableJudge)
        }
    };

    // Images and audio need OpenRouter; without it (or without a model id) they are skipped, not failed.
    let vision: Option<Arc<dyn attachments::vision::VisionReader>> = match &openrouter {
        Some(client) if !config.openrouter_vision_model_id.is_empty() => {
            Some(Arc::new(attachments::vision::OpenRouterVision::new(client.clone(), &config.openrouter_vision_model_id)))
        }
        _ => None,
    };
    let audio: Option<Arc<dyn attachments::audio::AudioReader>> = match &openrouter {
        Some(client) if !config.openrouter_audio_model_id.is_empty() => {
            Some(Arc::new(attachments::audio::OpenRouterAudio::new(client.clone(), &config.openrouter_audio_model_id, analyst_model)))
        }
        _ => None,
    };

    let (mut assistant_planner, mut vision, mut judge) = (assistant_planner, vision, judge);
    if config.ai_mock {
        assistant_planner = Arc::new(ai::mock::MockCommandPlanner);
        vision = Some(Arc::new(attachments::vision::StubVision::new(ai::mock::mock_image_reading())));
        judge = Arc::new(ai::mock::mock_judge());
    }

    Ok(AppState {
        designer,
        field_extractor,
        assistant_planner,
        transcriber,
        vision,
        audio,
        planner,
        critic,
        judge,
        memory_extractor,
        alert_mailer: mailer.clone(),
        analyst,
        understander,
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
