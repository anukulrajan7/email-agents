use anyhow::Context;
use email_ai_api::{build_state, config::Config, db, http, jobs};
use tokio::sync::watch;
use tracing_subscriber::{EnvFilter, fmt};

/// Usage: `email-ai-api [serve|api|worker|migrate]`
/// - `serve` (default): API + worker in one process (first pilot).
/// - `api` / `worker`: separate processes on the same host, sharing the SQLite file.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    fmt().with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))).init();

    let mode = std::env::args().nth(1).unwrap_or_else(|| "serve".into());
    let config = Config::from_env()?;
    let bind_addr = config.bind_addr.clone();
    let state = build_state(config).await?;
    db::migrate(&state.db).await?;
    if mode == "migrate" {
        tracing::info!("migrations applied");
        return Ok(());
    }

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let worker = matches!(mode.as_str(), "serve" | "worker")
        .then(|| tokio::spawn(jobs::worker::run(state.clone(), shutdown_rx.clone())));

    if matches!(mode.as_str(), "serve" | "api") {
        let listener = tokio::net::TcpListener::bind(&bind_addr).await.with_context(|| format!("binding {bind_addr}"))?;
        tracing::info!(%bind_addr, "API listening");
        axum::serve(listener, http::router(state.clone()))
            .with_graceful_shutdown(shutdown_signal())
            .await?;
    } else if mode == "worker" {
        shutdown_signal().await;
    } else {
        anyhow::bail!("unknown mode '{mode}' (expected serve, api, worker or migrate)");
    }

    let _ = shutdown_tx.send(true);
    if let Some(worker) = worker {
        let _ = worker.await;
    }
    state.db.close().await;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = terminate => {} }
    tracing::info!("shutdown signal received");
}
