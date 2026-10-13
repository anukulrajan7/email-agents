use email_ai_api::ai::openrouter::OpenRouterClient;
use email_ai_api::ai::selftest;
use email_ai_api::config::Config;

/// Production smoke: one tool-calling request per configured model. Exit 1 lists each failing model.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    let config = Config::from_env()?;
    if config.openrouter_api_key.is_empty() {
        eprintln!("FAIL OPENROUTER_API_KEY is not set");
        std::process::exit(1);
    }
    let targets = selftest::targets(&config);
    if targets.is_empty() {
        eprintln!("FAIL no OpenRouter models are configured");
        std::process::exit(1);
    }
    let client = OpenRouterClient::new(reqwest::Client::builder().user_agent("email-ai-api/0.1").build()?, &config.openrouter_api_url, &config.openrouter_api_key);
    let mut failed = 0;
    for outcome in selftest::run(&client, targets).await {
        let label = format!("{} ({})", outcome.target.roles.join("+"), outcome.target.model);
        match outcome.result {
            Ok(()) => println!("ok   {label}"),
            Err(reason) => {
                failed += 1;
                eprintln!("FAIL {label}: {reason}");
            }
        }
    }
    if failed > 0 {
        eprintln!("{failed} model(s) failed the tool-calling probe");
        std::process::exit(1);
    }
    Ok(())
}
