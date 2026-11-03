//! The app's log (CLAUDE.md conventions): `tracing`, a daily rolling file in `<data>/logs/`
//! (`vibe-files.<date>.log`, the newest 30 kept). No PII: what is logged here names actions,
//! counts and error kinds, never client names, document paths or file contents; the details
//! stay on screen and in the audit trail.

use std::path::Path;

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Dated log files kept.
pub const KEEP: usize = 30;

/// Starts logging to `<data>/logs/`. Hold the guard until exit so buffered lines are written.
/// `None` when the folder cannot be used (the app runs without a log then).
pub fn init(data: &Path) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let dir = data.join("logs");
    std::fs::create_dir_all(&dir).ok()?;
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
    fn error_kinds_carry_no_text() {
        assert_eq!(
            kind(r"cannot read C:\Clients\Anderson Farms LLC\w2.pdf: Access is denied."),
            "access denied"
        );
        assert_eq!(kind("database is locked"), "catalog busy");
        assert_eq!(kind("Anderson Farms LLC: weird"), "other");
    }
}
