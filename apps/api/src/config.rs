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
    /// Per workspace, per UTC day. At or above it, automatic non-essential AI waits for a person.
    pub ai_daily_budget_usd: f64,
    pub typesafe_api_key: String,
    pub jev_api_url: String,
    pub jev_model_id: String,
    pub openrouter_api_key: String,
    pub openrouter_api_url: String,
    pub openrouter_model_id: String,
    /// Standard drafting tier (routine replies). Empty: the premium model drafts everything.
    pub openrouter_draft_model_id: String,
    /// Premium drafting tier (high priority, client mail, many asks, "Write a better draft").
    pub openrouter_premium_model_id: String,
    pub openrouter_classifier_model_id: String,
    pub openrouter_embedding_model_id: String,
    /// Reads images (OCR, description, fields). Empty disables image reading.
    pub openrouter_vision_model_id: String,
    /// Transcribes audio attachments (`/audio/transcriptions`). Empty disables audio reading.
    pub openrouter_audio_model_id: String,
    /// Agent planner (tool calling). Premium cases use `openrouter_premium_model_id`.
    pub openrouter_planner_model_id: String,
    /// Speech to text for the assistant's voice input. Empty: voice input is off.
    /// Planner turns per agent case before it fails.
    pub agent_max_steps: i64,
    /// AI spend per agent case (USD) before it fails.
    pub agent_budget_usd: f64,
    /// Models behind the agent's quality tiers (Fast / Balanced / Best). Never shown to people.
    pub agent_fast_model_id: String,
    pub agent_balanced_model_id: String,
    pub agent_best_model_id: String,
    /// Replies the agent may send without asking, per mailbox owner per day.
    pub agent_auto_send_daily_cap: i64,
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
    /// Lower-case email domains allowed to sign up. With `signup_allowed_emails` also empty,
    /// sign-up is closed except for invited emails and the very first account.
    pub signup_allowed_domains: Vec<String>,
    /// Lower-case email addresses allowed to sign up (in addition to the allowed domains).
    pub signup_allowed_emails: Vec<String>,
    /// Global, all workspaces, per UTC day: automatic labelling + embedding of new mail. Above it,
    /// labelling falls back to rules and embedding waits until someone opens the email.
    pub ai_global_daily_budget_usd: f64,
    /// Public URL of the web app, used for deep links in alert emails.
    pub app_base_url: String,
    /// Replies are sent this many seconds after "Send" so people can undo (0 = next worker run).
    pub send_undo_seconds: i64,
    /// `production` unless set. `AI_MOCK` and `MAIL_CAPTURE_FILE` are refused in production.
    pub app_env: String,
    /// Browser E2E only: scripted assistant, stub image reader, no model calls at all.
    pub ai_mock: bool,
    /// Browser E2E only: outgoing mail (verification codes, alerts) is appended here as JSON lines.
    pub mail_capture_file: String,
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
        let openrouter_model_id = var("OPENROUTER_MODEL_ID", "");
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
            ai_daily_budget_usd: var("AI_DAILY_BUDGET_USD", "2.0").parse().context("AI_DAILY_BUDGET_USD")?,
            typesafe_api_key: var("TYPESAFE_API_KEY", ""),
            jev_api_url: var("JEV_API_URL", "https://api.typesafe.ai/v1/systemone"),
            jev_model_id: var("JEV_MODEL_ID", "jev-latest"),
            openrouter_api_key: var("OPENROUTER_API_KEY", ""),
            openrouter_api_url: var("OPENROUTER_API_URL", "https://openrouter.ai/api/v1"),
            openrouter_draft_model_id: var("OPENROUTER_DRAFT_MODEL_ID", "deepseek/deepseek-v4-flash"),
            openrouter_premium_model_id: var("OPENROUTER_PREMIUM_MODEL_ID", "deepseek/deepseek-v4-pro"),
            openrouter_model_id,
            openrouter_classifier_model_id: var("OPENROUTER_CLASSIFIER_MODEL_ID", ""),
            openrouter_embedding_model_id: var("OPENROUTER_EMBEDDING_MODEL_ID", "baai/bge-m3"),
            openrouter_vision_model_id: var("OPENROUTER_VISION_MODEL_ID", "qwen/qwen3-vl-30b-a3b-instruct"),
            openrouter_audio_model_id: var("OPENROUTER_AUDIO_MODEL_ID", "qwen/qwen3-asr-1.7b"),
            openrouter_planner_model_id: var("OPENROUTER_PLANNER_MODEL_ID", "deepseek/deepseek-v4-pro"),
            agent_max_steps: var("AGENT_MAX_STEPS", "15").parse().context("AGENT_MAX_STEPS")?,
            agent_budget_usd: var("AGENT_BUDGET_USD", "0.10").parse().context("AGENT_BUDGET_USD")?,
            agent_fast_model_id: var("AGENT_FAST_MODEL_ID", "qwen/qwen3.8-flash"),
            agent_balanced_model_id: var("AGENT_BALANCED_MODEL_ID", "deepseek/deepseek-v4-flash"),
            agent_best_model_id: var("AGENT_BEST_MODEL_ID", "deepseek/deepseek-v4-pro"),
            agent_auto_send_daily_cap: var("AGENT_AUTO_SEND_DAILY_CAP", "20").parse().context("AGENT_AUTO_SEND_DAILY_CAP")?,
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
            signup_allowed_domains: list(&var("SIGNUP_ALLOWED_DOMAINS", ""), true),
            signup_allowed_emails: list(&var("SIGNUP_ALLOWED_EMAILS", ""), false),
            ai_global_daily_budget_usd: var("AI_GLOBAL_DAILY_BUDGET_USD", "10")
                .parse()
                .context("AI_GLOBAL_DAILY_BUDGET_USD")?,
            app_base_url: var("APP_BASE_URL", "https://mail.anukulraj.tech"),
            send_undo_seconds: var("SEND_UNDO_SECONDS", "30").parse::<i64>().context("SEND_UNDO_SECONDS")?.clamp(0, 600),
            app_env: var("APP_ENV", "production").to_lowercase(),
            ai_mock: var("AI_MOCK", "") == "1",
            mail_capture_file: var("MAIL_CAPTURE_FILE", ""),
        };
        check_mock_allowed(&config.app_env, config.ai_mock, &config.mail_capture_file, &config.public_web_url, &config.openrouter_api_key)?;
        if config.auth_mode == AuthMode::Oidc
            && (config.oidc_issuer_url.is_empty() || config.oidc_audience.is_empty())
        {
            bail!("AUTH_MODE=oidc requires OIDC_ISSUER_URL and OIDC_AUDIENCE (use AUTH_MODE=dev locally)");
        }
        if config.auth_mode == AuthMode::Dev {
            check_dev_auth(var("ALLOW_DEV_AUTH", "") == "1", &config.public_web_url, &config.bind_addr)?;
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
            ai_daily_budget_usd: 2.0,
            typesafe_api_key: String::new(),
            jev_api_url: String::new(),
            jev_model_id: "jev-latest".into(),
            openrouter_api_key: String::new(),
            openrouter_api_url: String::new(),
            openrouter_model_id: String::new(),
            openrouter_draft_model_id: String::new(),
            openrouter_premium_model_id: String::new(),
            openrouter_classifier_model_id: String::new(),
            openrouter_embedding_model_id: "baai/bge-m3".into(),
            openrouter_vision_model_id: "qwen/qwen3-vl-30b-a3b-instruct".into(),
            openrouter_audio_model_id: String::new(),
            openrouter_planner_model_id: String::new(),
            agent_max_steps: 15,
            agent_budget_usd: 0.10,
            agent_fast_model_id: String::new(),
            agent_balanced_model_id: String::new(),
            agent_best_model_id: String::new(),
            agent_auto_send_daily_cap: 20,
            email_retention_days: 30,
            job_worker_concurrency: 2,
            allow_test_emails: true,
            max_body_chars: 200_000,
            web_dist_dir: String::new(),
            signup_enabled: true,
            signup_allowed_domains: vec![],
            signup_allowed_emails: vec![],
            ai_global_daily_budget_usd: 10.0,
            smtp_host: String::new(),
            smtp_port: 465,
            smtp_username: String::new(),
            smtp_password: String::new(),
            smtp_from: String::new(),
            app_base_url: "https://mail.anukulraj.tech".into(),
            send_undo_seconds: 0,
            app_env: "test".into(),
            ai_mock: false,
            mail_capture_file: String::new(),
        }
    }
}

fn list(raw: &str, domains: bool) -> Vec<String> {
    raw.split(',')
        .map(|d| if domains { d.trim().trim_start_matches('@') } else { d.trim() }.to_lowercase())
        .filter(|d| !d.is_empty())
        .collect()
}

/// `AUTH_MODE=dev` accepts any email as a login: only with `ALLOW_DEV_AUTH=1`, a non-https
/// public URL and a loopback bind address.
pub fn check_dev_auth(allow_dev_auth: bool, public_web_url: &str, bind_addr: &str) -> anyhow::Result<()> {
    if !allow_dev_auth {
        bail!("AUTH_MODE=dev accepts any email as a login; set ALLOW_DEV_AUTH=1 to use it locally (use AUTH_MODE=password otherwise)");
    }
    if public_web_url.trim().to_lowercase().starts_with("https://") {
        bail!("AUTH_MODE=dev is refused when PUBLIC_WEB_URL is https (that looks like production); use AUTH_MODE=password");
    }
    let loopback = bind_addr.parse::<std::net::SocketAddr>().map(|a| a.ip().is_loopback()).unwrap_or(false);
    if !loopback {
        bail!("AUTH_MODE=dev accepts any email as a login; it may only bind to a loopback address");
    }
    Ok(())
}

/// Environments where the E2E-only switches may be on. Anything else (including unset = production) refuses.
const MOCK_ENVS: [&str; 5] = ["development", "dev", "local", "test", "e2e"];

/// `AI_MOCK=1` and `MAIL_CAPTURE_FILE` replace real providers with fakes, so they only start in a
/// non-production `APP_ENV` and never behind an https public URL.
pub fn check_mock_allowed(
    app_env: &str,
    ai_mock: bool,
    mail_capture_file: &str,
    public_web_url: &str,
    openrouter_api_key: &str,
) -> anyhow::Result<()> {
    if !ai_mock && mail_capture_file.is_empty() {
        return Ok(());
    }
    if !MOCK_ENVS.contains(&app_env) {
        bail!("AI_MOCK / MAIL_CAPTURE_FILE are test switches; they need APP_ENV to be one of {MOCK_ENVS:?} (got '{app_env}')");
    }
    if public_web_url.trim().to_lowercase().starts_with("https://") {
        bail!("AI_MOCK / MAIL_CAPTURE_FILE are refused when PUBLIC_WEB_URL is https (that looks like production)");
    }
    if ai_mock && !openrouter_api_key.trim().is_empty() {
        bail!("AI_MOCK is refused when OPENROUTER_API_KEY is set (a real deployment's key must never sit next to the fakes)");
    }
    Ok(())
}

/// Svix-format secret for tests, built at runtime so no secret-shaped literal lives in
/// the source (secret scanners flag `whsec_` strings). Not a credential.
pub fn test_webhook_secret() -> String {
    use base64::{Engine, engine::general_purpose::STANDARD};
    format!("{}_{}", "whsec", STANDARD.encode("unit-test-only-not-a-credential"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_auth_is_hard_opt_in() {
        assert!(check_dev_auth(true, "http://localhost:5173", "127.0.0.1:8080").is_ok());
        assert!(check_dev_auth(false, "http://localhost:5173", "127.0.0.1:8080").is_err());
        assert!(check_dev_auth(true, "https://mail.example.com", "127.0.0.1:8080").is_err());
        assert!(check_dev_auth(true, "HTTPS://mail.example.com", "127.0.0.1:8080").is_err());
        assert!(check_dev_auth(true, "http://localhost:5173", "0.0.0.0:8080").is_err());
    }

    #[test]
    fn mock_switches_are_refused_in_production() {
        let dev = "http://localhost:5173";
        assert!(check_mock_allowed("production", false, "", "https://mail.example.com", "sk-live").is_ok());
        assert!(check_mock_allowed("production", true, "", dev, "").is_err());
        assert!(check_mock_allowed("", true, "", dev, "").is_err());
        assert!(check_mock_allowed("Production", true, "", dev, "").is_err());
        assert!(check_mock_allowed("production", false, "/tmp/mail.jsonl", dev, "").is_err());
        assert!(check_mock_allowed("e2e", true, "/tmp/mail.jsonl", dev, "").is_ok());
        assert!(check_mock_allowed("e2e", true, "", "https://mail.example.com", "").is_err());
        assert!(check_mock_allowed("e2e", true, "", dev, "sk-live").is_err());
        assert!(check_mock_allowed("e2e", true, "", dev, "  ").is_ok());
    }

    #[test]
    fn allow_lists_are_normalised() {
        assert_eq!(list(" @Acme.com, ,beta.io ", true), ["acme.com", "beta.io"]);
        assert_eq!(list("Priya@Acme.com,", false), ["priya@acme.com"]);
        assert!(list("", false).is_empty());
    }
}
