use super::*;

pub(super) async fn backup_database(state: &AppState) -> Result<(), JobError> {
    let Some(dir) = crate::platform::backup::backup_dir(&state.config.database_url) else { return Ok(()) };
    let today = time::OffsetDateTime::now_utc().date();
    match crate::platform::backup::run(&state.db, &dir, today, crate::platform::backup::KEEP).await {
        Ok(Some(path)) => tracing::info!(path = %path.display(), "database backup written"),
        Ok(None) => {}
        Err(e) => return Err(JobError::Retry(format!("database backup failed: {e:#}"))),
    }
    Ok(())
}

pub(super) async fn purge_expired_events(state: &AppState) -> Result<(), JobError> {
    let cutoff = now_ms() - state.config.email_retention_days * 86_400_000;
    let purged = sqlx::query("UPDATE inbound_events SET payload_json = '{}' WHERE created_at < ?1 AND payload_json != '{}'")
        .bind(cutoff)
        .execute(&state.db)
        .await?
        .rows_affected();
    let jobs = sqlx::query("DELETE FROM jobs WHERE status = 'succeeded' AND updated_at < ?1")
        .bind(cutoff)
        .execute(&state.db)
        .await?
        .rows_affected();
    sqlx::query("DELETE FROM judge_cache WHERE created_at < ?1").bind(cutoff).execute(&state.db).await?;
    crate::mail::deletion::prune_tombstones(&state.db).await?;
    sqlx::query("DELETE FROM email_outbox WHERE created_at < ?1").bind(cutoff).execute(&state.db).await?;
    let rate_events = crate::platform::security::purge_rate_events(&state.db).await?;
    tracing::info!(purged_payloads = purged, deleted_jobs = jobs, rate_events, "retention purge");
    Ok(())
}
