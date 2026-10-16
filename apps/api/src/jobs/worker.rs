use std::{sync::Arc, time::Duration};

use serde_json::json;
use tokio::sync::{Semaphore, watch};
use tracing::Instrument;

use super::{Job, handlers::{self, JobError}, kind};
use crate::{platform::db::now_ms, http::AppState, jobs};

const IDLE_POLL: Duration = Duration::from_millis(1_000);
const MAINTENANCE_EVERY: Duration = Duration::from_secs(30);

/// Single worker loop with bounded concurrency (`JOB_WORKER_CONCURRENCY`).
pub async fn run(state: AppState, mut shutdown: watch::Receiver<bool>) {
    let worker_id = format!("worker-{}", uuid::Uuid::now_v7().simple());
    let concurrency = state.config.job_worker_concurrency.max(1);
    let permits = Arc::new(Semaphore::new(concurrency));
    let mut last_maintenance = tokio::time::Instant::now() - MAINTENANCE_EVERY;
    tracing::info!(%worker_id, concurrency, "job worker started");

    loop {
        if *shutdown.borrow() {
            break;
        }
        if last_maintenance.elapsed() >= MAINTENANCE_EVERY {
            maintenance(&state).await;
            last_maintenance = tokio::time::Instant::now();
        }

        let permit = tokio::select! {
            p = permits.clone().acquire_owned() => p.expect("semaphore never closed"),
            _ = shutdown.changed() => break,
        };

        match jobs::claim_with_lease(&state.db, &worker_id, state.config.job_lease_ms).await {
            Ok(Some(job)) => {
                let state = state.clone();
                let worker_id = worker_id.clone();
                tokio::spawn(async move {
                    process(&state, &worker_id, job).await;
                    drop(permit);
                });
            }
            Ok(None) => {
                drop(permit);
                tokio::select! {
                    _ = state.jobs_notify.notified() => {}
                    _ = tokio::time::sleep(IDLE_POLL) => {}
                    _ = shutdown.changed() => break,
                }
            }
            Err(e) => {
                drop(permit);
                tracing::warn!(error = %e, "job claim failed");
                tokio::time::sleep(IDLE_POLL).await;
            }
        }
    }

    // Let in-flight jobs finish; anything cut off is recovered via lease expiry.
    let _ = tokio::time::timeout(Duration::from_secs(30), permits.acquire_many(concurrency as u32)).await;
    tracing::info!(%worker_id, "job worker stopped");
}

pub async fn process(state: &AppState, worker_id: &str, job: Job) {
    let span = tracing::info_span!("job", job_id = %job.id, kind = %job.kind, attempt = job.attempts);
    process_inner(state, worker_id, job).instrument(span).await
}

async fn process_inner(state: &AppState, worker_id: &str, job: Job) {
    let started = std::time::Instant::now();
    let ctx = crate::ai::ledger::AiContext::for_job(job.workspace_id.as_deref(), &job.payload(), job.attempts);
    let heartbeat = Heartbeat::start(state, worker_id, &job);
    let max_runtime = Duration::from_secs(state.config.job_max_runtime_secs);
    let run = async {
        tokio::time::timeout(max_runtime, handlers::handle(state, &job))
            .await
            .unwrap_or_else(|_| Err(JobError::Retry(format!("the job ran longer than its {} s time limit", max_runtime.as_secs()))))
    };
    let outcome = crate::ai::ledger::with_context(ctx, run).await;
    drop(heartbeat);
    match outcome {
        Ok(()) => {
            if let Err(e) = jobs::complete(&state.db, &job, worker_id).await {
                tracing::warn!(job_id = %job.id, error = %e, "failed to mark job succeeded");
            }
            tracing::info!(job_id = %job.id, kind = %job.kind, ms = started.elapsed().as_millis() as u64, "job succeeded");
            state.jobs_notify.notify_one();
        }
        Err(err) => {
            let permanent = err.is_permanent();
            match jobs::fail(&state.db, &job, worker_id, &err.to_string(), permanent).await {
                Ok(true) => {
                    tracing::error!(job_id = %job.id, kind = %job.kind, error = %err, "job dead-lettered");
                    handlers::on_dead(state, &job, &err.to_string()).await;
                }
                Ok(false) => tracing::warn!(job_id = %job.id, kind = %job.kind, error = %err, "job failed, will retry"),
                Err(e) => tracing::error!(job_id = %job.id, error = %e, "failed to record job failure"),
            }
        }
    }
}

/// Renews a running job's lease every quarter of the lease (30 s for the default 120 s) so a long
/// handler is not claimed again while it still runs. Dropping it, even on a panic, stops the renewals.
struct Heartbeat(tokio::task::JoinHandle<()>);

impl Heartbeat {
    fn start(state: &AppState, worker_id: &str, job: &Job) -> Self {
        let (pool, worker_id, job) = (state.db.clone(), worker_id.to_string(), job.clone());
        let lease_ms = state.config.job_lease_ms;
        Self(tokio::spawn(async move {
            let every = Duration::from_millis((lease_ms / 4).max(1) as u64);
            loop {
                tokio::time::sleep(every).await;
                match jobs::extend_lease(&pool, &job, &worker_id, lease_ms).await {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(e) => tracing::warn!(job_id = %job.id, error = %e, "could not renew the job lease"),
                }
            }
        }))
    }
}

impl Drop for Heartbeat {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Test/CLI helper: process due jobs sequentially until none are claimable.
pub async fn drain(state: &AppState) -> usize {
    let mut n = 0;
    while let Ok(Some(job)) = jobs::claim_with_lease(&state.db, "drain", state.config.job_lease_ms).await {
        process(state, "drain", job).await;
        n += 1;
    }
    n
}

async fn maintenance(state: &AppState) {
    match jobs::sweep_exhausted(&state.db).await {
        Ok(dead) => {
            for job in dead {
                tracing::error!(job_id = %job.id, kind = %job.kind, "lease expired with no attempts left");
                handlers::on_dead(state, &job, "lease expired").await;
            }
        }
        Err(e) => tracing::warn!(error = %e, "lease sweep failed"),
    }
    // Connected mailboxes: pull new mail every 5 minutes (Nango's template sync is hourly).
    let five_min_bucket = now_ms() / 300_000;
    let polled = async {
        let sources: Vec<(String, String)> = sqlx::query_as(
            "SELECT id, workspace_id FROM email_sources WHERE kind = 'nango_gmail' AND status != 'disconnected'",
        )
        .fetch_all(&state.db)
        .await?;
        let mut conn = state.db.acquire().await?;
        for (source_id, ws) in sources {
            jobs::enqueue_with_attempts(
                &mut conn,
                kind::SYNC_NANGO_SOURCE,
                &format!("sync:{source_id}:poll:{five_min_bucket}"),
                Some(&ws),
                &json!({ "source_id": source_id }),
                3,
            )
            .await?;
        }
        Ok::<(), sqlx::Error>(())
    }
    .await;
    if let Err(e) = polled {
        tracing::warn!(error = %e, "failed to schedule mailbox polling");
    }
    if let Err(e) = crate::alerts::schedule(state).await {
        tracing::warn!(error = %e, "failed to schedule alert evaluation");
    }
    if let Err(e) = crate::agents::triggers::schedule(state).await {
        tracing::warn!(error = %e, "failed to schedule agent ticks");
    }
    if let Err(e) = jobs::eval_jobs::schedule(state).await {
        tracing::warn!(error = %e, "failed to schedule the metrics rollup");
    }
    if let Err(e) = jobs::match_digest::schedule(state).await {
        tracing::warn!(error = %e, "failed to schedule match digest");
    }
    if let Err(e) = crate::hr::staffing::schedule(state).await {
        tracing::warn!(error = %e, "failed to schedule staffing scan");
    }
    let hour_bucket = now_ms() / 3_600_000;
    let day = now_ms() / 86_400_000;
    let enqueued = async {
        let mut conn = state.db.acquire().await?;
        jobs::enqueue(&mut conn, kind::PURGE_EXPIRED_EVENTS, &format!("purge:{hour_bucket}"), None, &json!({})).await?;
        // Nightly: the first maintenance pass of each UTC day.
        jobs::enqueue(&mut conn, kind::BACKUP_DATABASE, &format!("backup:{day}"), None, &json!({})).await
    }
    .await;
    if let Err(e) = enqueued {
        tracing::warn!(error = %e, "failed to schedule purge");
    }
    if let Err(e) = jobs::message_jobs::schedule_text_clean(state).await {
        tracing::warn!(error = %e, "failed to schedule text clean-up");
    }
}
