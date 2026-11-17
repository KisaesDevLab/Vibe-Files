//! The app's log (CLAUDE.md conventions): `tracing`, a daily rolling file in `<data>/logs/`
//! (`vibe-files.<date>.log`, the newest 30 kept). No PII: what is logged here names actions,
//! counts and error kinds, never client names, document paths or file contents; the details
//! stay on screen and in the audit trail.

use std::path::Path;

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Dated log files kept.
pub const KEEP: usize = 30;

/// Where this run logs, once [`init`] has started it.
static DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// The folder this run logs to (in the login's own data folder), when it logs.
pub fn dir() -> Option<&'static Path> {
    DIR.get().map(std::path::PathBuf::as_path)
}

/// Makes `dir` this run's log folder (a test of the page that shows it; the first call wins).
#[cfg(test)]
pub fn set_dir_for_test(dir: &Path) -> &'static Path {
    DIR.get_or_init(|| dir.to_path_buf())
}

/// The days there is a log for in `dir`, newest first (`2026-11-17`).
pub fn days(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.strip_prefix("vibe-files.")?
                .strip_suffix(".log")
                .map(str::to_string)
        })
        .collect();
    out.sort();
    out.reverse();
    out
}

/// The most lines of a day's log shown (the newest).
pub const SHOWN: usize = 2_000;

/// The log of `day` in `dir`: its last [`SHOWN`] lines (Admin › Devices shows them, for a
/// person who cannot open the folder; owner 2026-11-17). Nothing in it is client data.
pub fn read_day(dir: &Path, day: &str) -> Result<String, String> {
    // A day as [`days`] gives it, never a path.
    if !day.chars().all(|c| c.is_ascii_digit() || c == '-') {
        return Err("not a day".into());
    }
    let file = dir.join(format!("vibe-files.{day}.log"));
    let text = std::fs::read(&file).map_err(|e| format!("the log of {day} cannot be read: {e}"))?;
    let text = String::from_utf8_lossy(&text);
    let lines: Vec<&str> = text.lines().collect();
    let from = lines.len().saturating_sub(SHOWN);
    Ok(lines[from..].join("\n"))
}

/// Starts logging to `<data>/logs/`. Hold the guard until exit so buffered lines are written.
/// `None` when the folder cannot be used (the app runs without a log then).
pub fn init(data: &Path) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let dir = data.join("logs");
    std::fs::create_dir_all(&dir).ok()?;
    let _ = DIR.set(dir.clone());
    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("vibe-files")
        .filename_suffix("log")
        .max_log_files(KEEP)
        .build(&dir)
        .ok()?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    // The vendored Filer crates log file paths and client folder names (INFO and WARN): only
    // their errors reach this log, so it stays free of client data (CLAUDE.md conventions).
    let filter = tracing_subscriber::filter::Targets::new()
        .with_default(tracing_subscriber::filter::LevelFilter::INFO)
        .with_target(
            "filer_intake",
            tracing_subscriber::filter::LevelFilter::ERROR,
        )
        .with_target("filer_core", tracing_subscriber::filter::LevelFilter::ERROR)
        .with_target("filer_pdf", tracing_subscriber::filter::LevelFilter::ERROR);
    tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(false),
        )
        .try_init()
        .ok()?;
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Location only: a panic message can quote data.
        let at = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        tracing::error!(at, "panic");
        default_hook(info);
    }));
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "started");
    Some(guard)
}

/// An error's kind as a short, data-free label (the text itself can hold a path or a name).
pub fn kind(e: &str) -> &'static str {
    let e = e.to_ascii_lowercase();
    if e.contains("denied") || e.contains("permission") {
        "access denied"
    } else if e.contains("not found") || e.contains("cannot find") {
        "not found"
    } else if e.contains("used by another process") || e.contains("in use") {
        "file in use"
    } else if e.contains("locked") || e.contains("busy") {
        "catalog busy"
    } else if e.contains("disk") || e.contains("space") {
        "disk"
    } else {
        "other"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_days_log_is_listed_and_read() {
        let tmp = tempfile::tempdir().unwrap();
        for (day, text) in [("2026-11-16", "old\n"), ("2026-11-17", "a\nb\n")] {
            std::fs::write(tmp.path().join(format!("vibe-files.{day}.log")), text).unwrap();
        }
        std::fs::write(tmp.path().join("other.txt"), "x").unwrap();
        assert_eq!(days(tmp.path()), ["2026-11-17", "2026-11-16"]);
        assert_eq!(read_day(tmp.path(), "2026-11-17").unwrap(), "a\nb");
        assert!(read_day(tmp.path(), "..\\x").is_err());
        let long: String = (0..SHOWN + 5).map(|i| format!("line {i}\n")).collect();
        std::fs::write(tmp.path().join("vibe-files.2026-11-18.log"), long).unwrap();
        let shown = read_day(tmp.path(), "2026-11-18").unwrap();
        assert_eq!(shown.lines().count(), SHOWN);
        assert!(shown.starts_with("line 5\n"));
    }

    #[test]
    fn error_kinds_carry_no_text() {
        assert_eq!(
            kind(r"cannot read C:\Clients\Anderson Farms LLC\w2.pdf: Access is denied."),
            "access denied"
        );
        assert_eq!(kind("database is locked"), "catalog busy");
        assert_eq!(kind("Anderson Farms LLC: weird"), "other");
    }
}
