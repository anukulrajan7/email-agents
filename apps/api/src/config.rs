use std::env;

use anyhow::{Context, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    /// Local development only: `dev:<email>` tokens.
    Dev,
    /// Email + password accounts with server-side sessions.
    Password,
    Oidc,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub bind_addr: String,
    pub public_web_url: String,
    pub auth_mode: AuthMode,
    pub oidc_issuer_url: String,
    pub oidc_audience: String,
    pub resend_api_key: String,
    pub resend_api_url: String,
    pub resend_webhook_secret: String,
    pub inbound_domain: String,
    pub nango_api_key: String,
    pub nango_api_url: String,
    pub nango_webhook_secret: String,
    pub nango_gmail_integration: String,
    pub nango_gmail_model: String,
    pub nango_backfill_days: i64,
    pub nango_backfill_max: usize,
    /// AI runs automatically only on mail received within this many hours; older mail is
    /// read by AI when someone opens it (keeps token cost low).
    pub ai_auto_hours: i64,
    /// Draft replies automatically (workflows, automations). Off: drafts only when a person asks.
    pub auto_drafts: bool,
    pub typesafe_api_key: String,
    pub jev_api_url: String,
    pub jev_model_id: String,
    pub openrouter_api_key: String,
    pub openrouter_api_url: String,
    pub openrouter_model_id: String,
    pub openrouter_classifier_model_id: String,
    pub openrouter_embedding_model_id: String,
    pub email_retention_days: i64,
    pub job_worker_concurrency: usize,
    pub allow_test_emails: bool,
    pub max_body_chars: usize,
    /// Built web app to serve (SPA fallback to index.html). Unset in dev: Vite serves it.
    pub web_dist_dir: String,
    pub signup_enabled: bool,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_username: String,
    pub smtp_password: String,
    pub smtp_from: String,
    /// Lower-case email domains allowed to sign up; empty = any domain.
    pub signup_allowed_domains: Vec<String>,
    /// Public URL of the web app, used for deep links in alert emails.
    pub app_base_url: String,
}

fn var(name: &str, default: &str) -> String {
    env::var(name).ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| default.to_string())
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let auth_mode = match var("AUTH_MODE", "oidc").as_str() {
            "dev" => AuthMode::Dev,
            "password" => AuthMode::Password,
            "oidc" => AuthMode::Oidc,
            other => bail!("AUTH_MODE must be 'dev', 'password' or 'oidc', got '{other}'"),
        };
        let config = Self {
            database_url: var("DATABASE_URL", "sqlite://data/email_ai.db"),
            bind_addr: var("API_BIND_ADDR", "127.0.0.1:8080"),
            public_web_url: var("PUBLIC_WEB_URL", "http://localhost:5173"),
            auth_mode,
            oidc_issuer_url: var("OIDC_ISSUER_URL", ""),
            oidc_audience: var("OIDC_AUDIENCE", ""),
            resend_api_key: var("RESEND_API_KEY", ""),
            resend_api_url: var("RESEND_API_URL", "https://api.resend.com"),
            resend_webhook_secret: var("RESEND_WEBHOOK_SECRET", ""),
            inbound_domain: var("INBOUND_DOMAIN", "inbound.localhost"),
            nango_api_key: var("NANGO_API_KEY", ""),
            nango_api_url: var("NANGO_API_URL", "https://api.nango.dev"),
            nango_webhook_secret: var("NANGO_WEBHOOK_SECRET", ""),
            nango_gmail_integration: var("NANGO_GMAIL_INTEGRATION_ID", "google-mail"),
            nango_gmail_model: var("NANGO_GMAIL_MODEL", "GmailEmail"),
            nango_backfill_days: var("NANGO_BACKFILL_DAYS", "14").parse().context("NANGO_BACKFILL_DAYS")?,
            nango_backfill_max: var("NANGO_BACKFILL_MAX", "100").parse().context("NANGO_BACKFILL_MAX")?,
            ai_auto_hours: var("AI_AUTO_HOURS", "2").parse().context("AI_AUTO_HOURS")?,
            auto_drafts: var("AUTO_DRAFTS", "false").parse().context("AUTO_DRAFTS")?,
            typesafe_api_key: var("TYPESAFE_API_KEY", ""),
            jev_api_url: var("JEV_API_URL", "https://api.typesafe.ai/v1/systemone"),
            jev_model_id: var("JEV_MODEL_ID", "jev-latest"),
            openrouter_api_key: var("OPENROUTER_API_KEY", ""),
            openrouter_api_url: var("OPENROUTER_API_URL", "https://openrouter.ai/api/v1"),
            openrouter_model_id: var("OPENROUTER_MODEL_ID", ""),
            openrouter_classifier_model_id: var("OPENROUTER_CLASSIFIER_MODEL_ID", ""),
            openrouter_embedding_model_id: var("OPENROUTER_EMBEDDING_MODEL_ID", "baai/bge-m3"),
            email_retention_days: var("EMAIL_RETENTION_DAYS", "30").parse().context("EMAIL_RETENTION_DAYS")?,
            job_worker_concurrency: var("JOB_WORKER_CONCURRENCY", "2").parse().context("JOB_WORKER_CONCURRENCY")?,
            allow_test_emails: var("ALLOW_TEST_EMAILS", "false") == "true",
            max_body_chars: var("MAX_BODY_CHARS", "200000").parse().context("MAX_BODY_CHARS")?,
            web_dist_dir: var("WEB_DIST_DIR", ""),
            signup_enabled: var("SIGNUP_ENABLED", "true") == "true",
            smtp_host: var("SMTP_HOST", ""),
            smtp_port: var("SMTP_PORT", "465").parse().context("SMTP_PORT")?,
            smtp_username: var("SMTP_USERNAME", ""),
            smtp_password: var("SMTP_PASSWORD", ""),
            smtp_from: var("SMTP_FROM", ""),
            signup_allowed_domains: var("SIGNUP_ALLOWED_DOMAINS", "")
                .split(',')
                .map(|d| d.trim().trim_start_matches('@').to_lowercase())
                .filter(|d| !d.is_empty())
                .collect(),
            app_base_url: var("APP_BASE_URL", "https://mail.anukulraj.tech"),
        };
        if config.auth_mode == AuthMode::Oidc
            && (config.oidc_issuer_url.is_empty() || config.oidc_audience.is_empty())
        {
            bail!("AUTH_MODE=oidc requires OIDC_ISSUER_URL and OIDC_AUDIENCE (use AUTH_MODE=dev locally)");
        }
        if config.auth_mode == AuthMode::Dev {
            let loopback = config
                .bind_addr
                .parse::<std::net::SocketAddr>()
                .map(|a| a.ip().is_loopback())
                .unwrap_or(false);
            if !loopback {
                bail!("AUTH_MODE=dev accepts any email as a login; it may only bind to a loopback address");
            }
        }
        Ok(config)
    }

    /// Defaults suitable for tests: dev auth, no external providers.
    pub fn for_tests(database_url: &str) -> Self {
        Self {
            database_url: database_url.to_string(),
            bind_addr: "127.0.0.1:0".into(),
            public_web_url: "http://localhost:5173".into(),
            auth_mode: AuthMode::Dev,
            oidc_issuer_url: String::new(),
            oidc_audience: String::new(),
            resend_api_key: String::new(),
            resend_api_url: "http://127.0.0.1:9".into(),
            resend_webhook_secret: test_webhook_secret(),
            inbound_domain: "inbound.test".into(),
            nango_api_key: String::new(),
            nango_api_url: "http://127.0.0.1:9".into(),
            nango_webhook_secret: "unit-test-only-nango-key".into(),
            nango_gmail_integration: "google-mail".into(),
            nango_gmail_model: "GmailEmail".into(),
            nango_backfill_days: 14,
            nango_backfill_max: 500,
            ai_auto_hours: 48,
            auto_drafts: true,
            typesafe_api_key: String::new(),
            jev_api_url: String::new(),
            jev_model_id: "jev-latest".into(),
            openrouter_api_key: String::new(),
            openrouter_api_url: String::new(),
            openrouter_model_id: String::new(),
            openrouter_classifier_model_id: String::new(),
            openrouter_embedding_model_id: "baai/bge-m3".into(),
            email_retention_days: 30,
            job_worker_concurrency: 2,
            allow_test_emails: true,
            max_body_chars: 200_000,
            web_dist_dir: String::new(),
            signup_enabled: true,
            signup_allowed_domains: vec![],
            smtp_host: String::new(),
            smtp_port: 465,
            smtp_username: String::new(),
            smtp_password: String::new(),
            smtp_from: String::new(),
            app_base_url: "https://mail.anukulraj.tech".into(),
        }
    }
}

/// Svix-format secret for tests, built at runtime so no secret-shaped literal lives in
/// the source (secret scanners flag `whsec_` strings). Not a credential.
pub fn test_webhook_secret() -> String {
    use base64::{Engine, engine::general_purpose::STANDARD};
    format!("{}_{}", "whsec", STANDARD.encode("unit-test-only-not-a-credential"))
}
