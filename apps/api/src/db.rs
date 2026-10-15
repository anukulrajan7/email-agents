use std::{path::Path, str::FromStr, time::Duration};

use anyhow::Context;
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

/// Opens the pool. Pragmas are applied per connection; WAL is set here,
/// outside any migration transaction.
/// Registers the sqlite-vec extension for every connection opened afterwards
/// (SQLx's bundled SQLite). Idempotent.
pub fn register_vector_extension() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        type Init = unsafe extern "C" fn(
            *mut libsqlite3_sys::sqlite3,
            *mut *mut std::os::raw::c_char,
            *const libsqlite3_sys::sqlite3_api_routines,
        ) -> std::os::raw::c_int;
        let init: Init = std::mem::transmute::<*const (), Init>(sqlite_vec::sqlite3_vec_init as *const ());
        libsqlite3_sys::sqlite3_auto_extension(Some(init));
    });
}

pub async fn connect(database_url: &str) -> anyhow::Result<SqlitePool> {
    register_vector_extension();
    let options = SqliteConnectOptions::from_str(database_url)
        .with_context(|| format!("invalid DATABASE_URL {database_url}"))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(15));

    if let Some(parent) = Path::new(options.get_filename()).parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).context("creating database directory")?;
    }
    create_private(options.get_filename()).context("creating database file")?;

    SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await
        .context("connecting to SQLite")
}

/// A new database file is created owner-only (0600); SQLite gives its -wal/-shm files the
/// same permissions. An existing file is left as it is.
#[cfg(unix)]
fn create_private(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    if path.as_os_str().is_empty() || path.to_str() == Some(":memory:") || path.exists() {
        return Ok(());
    }
    match std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path) {
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => Err(e),
        _ => Ok(()),
    }
}

#[cfg(not(unix))]
fn create_private(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

pub async fn migrate(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::migrate!("./migrations").run(pool).await.context("running migrations")?;
    crate::agent_registry::seed::backfill_all(pool).await.context("seeding agents")?;
    crate::evals::gate::repair_template_demotions(pool).await.context("restoring agents demoted by the old safety gate")?;
    Ok(())
}

/// Tests only: if `database_url` names a file that does not exist yet, copies a database that was
/// migrated and seeded once per process into it, so each test skips the 50-odd migrations.
#[doc(hidden)]
pub fn seed_from_template(database_url: &str) -> anyhow::Result<()> {
    static TEMPLATE: std::sync::OnceLock<Result<std::path::PathBuf, String>> = std::sync::OnceLock::new();
    let Ok(options) = SqliteConnectOptions::from_str(database_url) else { return Ok(()) };
    let target = options.get_filename().to_path_buf();
    if target.as_os_str().is_empty() || target.to_str() == Some(":memory:") || target.exists() {
        return Ok(());
    }
    let template = TEMPLATE
        .get_or_init(|| {
            std::thread::spawn(|| -> anyhow::Result<std::path::PathBuf> {
                let dir = std::env::temp_dir().join("postroom-db-template");
                std::fs::create_dir_all(&dir)?;
                let path = dir.join(format!("template-{}.db", std::process::id()));
                let _ = std::fs::remove_file(&path);
                let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
                rt.block_on(async {
                    let pool = connect(&format!("sqlite://{}", path.display())).await?;
                    migrate(&pool).await?;
                    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)").execute(&pool).await?;
                    pool.close().await;
                    anyhow::Ok(())
                })?;
                for entry in std::fs::read_dir(&dir)?.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let stale = name.starts_with("template-") && !name.starts_with(&format!("template-{}.", std::process::id()));
                    if stale && entry.metadata().and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|e| e.as_secs() > 3600)) {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
                Ok(path)
            })
            .join()
            .map_err(|_| "the template thread panicked".to_string())
            .and_then(|r| r.map_err(|e| format!("{e:#}")))
        })
        .as_ref()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).context("creating database directory")?;
    }
    std::fs::copy(template, &target).context("copying the template database")?;
    Ok(())
}

thread_local! {
    static FROZEN_CLOCK: std::cell::Cell<Option<i64>> = const { std::cell::Cell::new(None) };
}

/// Tests only: pins `now_ms()` on the current thread (a `#[tokio::test]` runs on one thread), so day-boundary
/// logic does not depend on when the test happens to run.
#[doc(hidden)]
pub fn freeze_clock(ms: i64) {
    FROZEN_CLOCK.with(|c| c.set(Some(ms)));
}

pub fn now_ms() -> i64 {
    if let Some(ms) = FROZEN_CLOCK.with(std::cell::Cell::get) {
        return ms;
    }
    (OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64
}

pub fn iso(ms: i64) -> String {
    OffsetDateTime::from_unix_timestamp_nanos(ms as i128 * 1_000_000)
        .ok()
        .and_then(|t| t.format(&Rfc3339).ok())
        .unwrap_or_default()
}

pub fn parse_iso_ms(s: &str) -> Option<i64> {
    OffsetDateTime::parse(s, &Rfc3339)
        .ok()
        .map(|t| (t.unix_timestamp_nanos() / 1_000_000) as i64)
}

/// Time-ordered, prefixed identifier, e.g. `msg_0192...`.
pub fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::now_v7().simple())
}
