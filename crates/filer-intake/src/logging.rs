//! `data/filer.log`, rotated once a day at startup: yesterday's log becomes
//! `filer.YYYY-MM-DD.log`, and only the newest `KEEP` dated logs are kept.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Local, NaiveDate};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

pub const LOG_FILE: &str = "filer.log";
pub const KEEP: usize = 14;

/// Rotates, then sends `tracing` output to `filer.log` and to the console.
/// Hold the returned guard until exit so buffered lines are flushed.
pub fn init(data_dir: &Path) -> Result<tracing_appender::non_blocking::WorkerGuard> {
    let rotate_result = rotate(data_dir, Local::now().date_naive(), KEEP);

    let (writer, guard) =
        tracing_appender::non_blocking(tracing_appender::rolling::never(data_dir, LOG_FILE));
    let filter = tracing_subscriber::filter::LevelFilter::INFO;
    tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(false),
        )
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .init();

    if let Err(e) = rotate_result {
        tracing::warn!(error = %e, "log rotation failed; continuing with filer.log");
    }

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(%info, "panic");
        default_hook(info);
    }));
    Ok(guard)
}

/// If `filer.log` was last written before `today`, renames it to `filer.<that date>.log`,
/// then deletes the oldest dated logs beyond `keep`.
pub fn rotate(data_dir: &Path, today: NaiveDate, keep: usize) -> Result<()> {
    let current = data_dir.join(LOG_FILE);
    if let Ok(meta) = std::fs::metadata(&current) {
        let written: DateTime<Local> = meta.modified()?.into();
        let date = written.date_naive();
        if date < today {
            let archive = free_archive_name(data_dir, date);
            std::fs::rename(&current, &archive)
                .with_context(|| format!("renaming filer.log to {}", archive.display()))?;
        }
    }

    let mut archives = dated_logs(data_dir)?;
    archives.sort();
    let excess = archives.len().saturating_sub(keep);
    for old in &archives[..excess] {
        std::fs::remove_file(old).with_context(|| format!("deleting {}", old.display()))?;
    }
    Ok(())
}

/// `filer.2026-10-31.log`, or `filer.2026-10-31.2.log` etc. if that name is taken.
fn free_archive_name(data_dir: &Path, date: NaiveDate) -> PathBuf {
    let base = data_dir.join(format!("filer.{date}.log"));
    if !base.exists() {
        return base;
    }
    (2..)
        .map(|n| data_dir.join(format!("filer.{date}.{n}.log")))
        .find(|p| !p.exists())
        .expect("unbounded range")
}

/// Files named `filer.YYYY-MM-DD[.n].log`. Sorting the paths sorts them oldest first.
fn dated_logs(data_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(data_dir)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(middle) = name
            .strip_prefix("filer.")
            .and_then(|s| s.strip_suffix(".log"))
        else {
            continue;
        };
        let date_part = middle.split('.').next().unwrap_or_default();
        if NaiveDate::parse_from_str(date_part, "%Y-%m-%d").is_ok() && path.is_file() {
            out.push(path);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn day(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    fn touch(path: &Path, age_days: u64) {
        std::fs::write(path, "x").unwrap();
        let f = std::fs::File::options().write(true).open(path).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(age_days * 86_400))
            .unwrap();
    }

    #[test]
    fn todays_log_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join(LOG_FILE), 0);
        rotate(dir.path(), Local::now().date_naive(), KEEP).unwrap();
        assert!(dir.path().join(LOG_FILE).exists());
        assert!(dated_logs(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn old_log_is_renamed_to_its_date() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join(LOG_FILE), 2);
        let written = (Local::now() - chrono::Duration::days(2)).date_naive();
        rotate(dir.path(), Local::now().date_naive(), KEEP).unwrap();
        assert!(!dir.path().join(LOG_FILE).exists());
        assert!(dir.path().join(format!("filer.{written}.log")).exists());
    }

    #[test]
    fn taken_archive_name_gets_a_suffix() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join(LOG_FILE), 2);
        let written = (Local::now() - chrono::Duration::days(2)).date_naive();
        std::fs::write(dir.path().join(format!("filer.{written}.log")), "old").unwrap();
        rotate(dir.path(), Local::now().date_naive(), KEEP).unwrap();
        assert!(dir.path().join(format!("filer.{written}.2.log")).exists());
    }

    #[test]
    fn keeps_only_newest_archives() {
        let dir = tempfile::tempdir().unwrap();
        for d in 1..=20 {
            std::fs::write(dir.path().join(format!("filer.2026-10-{d:02}.log")), "x").unwrap();
        }
        std::fs::write(dir.path().join("ops.jsonl"), "keep me").unwrap();
        rotate(dir.path(), day("2026-11-01"), KEEP).unwrap();
        let mut left = dated_logs(dir.path()).unwrap();
        left.sort();
        assert_eq!(left.len(), KEEP);
        assert!(left[0].ends_with("filer.2026-10-07.log"));
        assert!(dir.path().join("ops.jsonl").exists());
    }
}
