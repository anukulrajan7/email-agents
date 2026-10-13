//! Nightly online backup: `VACUUM INTO <db dir>/backups/email_ai-YYYYMMDD.db` (a consistent
//! copy taken while the app keeps running), owner-only permissions, newest `KEEP` kept.

use std::{
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::Context;
use sqlx::{SqlitePool, sqlite::SqliteConnectOptions};
use time::Date;

pub const KEEP: usize = 7;
const PREFIX: &str = "email_ai-";

/// `<directory of the database file>/backups`, or `None` for in-memory databases.
pub fn backup_dir(database_url: &str) -> Option<PathBuf> {
    if database_url.contains(":memory:") || database_url.contains("mode=memory") {
        return None;
    }
    let options = SqliteConnectOptions::from_str(database_url).ok()?;
    let file = options.get_filename();
    if file.as_os_str().is_empty() || file.to_str() == Some(":memory:") {
        return None;
    }
    Some(file.parent().unwrap_or(Path::new(".")).join("backups"))
}

pub fn file_name(date: Date) -> String {
    format!("{PREFIX}{:04}{:02}{:02}.db", date.year(), u8::from(date.month()), date.day())
}

/// Writes today's backup unless it already exists, then prunes to the newest `keep`.
/// Returns the new file, or `None` when today's backup was already there.
pub async fn run(db: &SqlitePool, dir: &Path, date: Date, keep: usize) -> anyhow::Result<Option<PathBuf>> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    restrict(dir, 0o700);
    let target = dir.join(file_name(date));
    let created = if target.exists() {
        None
    } else {
        let partial = target.with_extension("db.partial");
        let _ = std::fs::remove_file(&partial);
        let path = partial.to_str().context("backup path is not UTF-8")?;
        sqlx::query("VACUUM INTO ?1").bind(path).execute(db).await.context("VACUUM INTO")?;
        restrict(&partial, 0o600);
        std::fs::rename(&partial, &target).context("finalising backup")?;
        Some(target)
    };
    prune(dir, keep)?;
    Ok(created)
}

/// Deletes all but the newest `keep` backups (names sort by date).
pub fn prune(dir: &Path, keep: usize) -> anyhow::Result<usize> {
    let mut backups: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                n.len() == PREFIX.len() + 11
                    && n.starts_with(PREFIX)
                    && n.ends_with(".db")
                    && n[PREFIX.len()..PREFIX.len() + 8].chars().all(|c| c.is_ascii_digit())
            })
        })
        .collect();
    backups.sort();
    let excess = backups.len().saturating_sub(keep);
    for old in &backups[..excess] {
        std::fs::remove_file(old).with_context(|| format!("removing {}", old.display()))?;
    }
    Ok(excess)
}

#[cfg(unix)]
fn restrict(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)) {
        tracing::warn!(error = %e, path = %path.display(), "could not restrict permissions");
    }
}

#[cfg(not(unix))]
fn restrict(_path: &Path, _mode: u32) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_dirs() {
        let d = Date::from_calendar_date(2026, time::Month::March, 7).unwrap();
        assert_eq!(file_name(d), "email_ai-20260307.db");
        assert_eq!(backup_dir("sqlite://data/email_ai.db"), Some(PathBuf::from("data/backups")));
        assert_eq!(backup_dir("sqlite::memory:"), None);
    }
}
