//! Catalog schema, document model, Transaction, doc types, retention, audit, locks.
//! No UI or network dependency (CLAUDE.md §Workspace).

pub mod audit;
pub mod browse;
pub mod db;
pub mod retention;
pub mod tape;
pub mod tx;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("catalog: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("this is not a Vibe Files catalog (an encrypted catalog from before PA-18?)")]
    NotACatalog,
    #[error("catalog schema v{found} is newer than this build supports (v{supported})")]
    NewerSchema { found: u32, supported: u32 },
    #[error("{0}")]
    Tx(String),
    #[error("undo is blocked: {}", .0.join("; "))]
    UndoBlocked(Vec<String>),
    #[error("file change failed: {error}")]
    FileOp {
        error: String,
        /// File changes that could not be reversed (empty when everything was put back).
        not_reversed: Vec<String>,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

/// Current UTC time, RFC 3339 with microseconds (storage format for every timestamp).
/// The Windows login of this process, `DOMAIN\user` (or `user` when there is no domain
/// variable). Recorded in audit rows and undo records (QUESTIONS Q-29); no sign-in in v1.
pub fn windows_login() -> Option<String> {
    let user = std::env::var("USERNAME").ok().filter(|u| !u.is_empty())?;
    Some(match std::env::var("USERDOMAIN") {
        Ok(d) if !d.is_empty() => format!("{d}\\{user}"),
        _ => user,
    })
}

/// A time as stored: RFC 3339 UTC with microseconds and `Z`, fixed width, so the text sorts
/// in time order (QA C9).
pub fn rfc3339(t: time::OffsetDateTime) -> String {
    static FMT: std::sync::OnceLock<Vec<time::format_description::BorrowedFormatItem<'static>>> =
        std::sync::OnceLock::new();
    let fmt = FMT.get_or_init(|| {
        time::format_description::parse_borrowed::<1>(
            "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:6]Z",
        )
        .expect("valid format")
    });
    t.to_offset(time::UtcOffset::UTC)
        .format(fmt)
        .expect("formats")
}

/// The time `days` days ago (for "older than" comparisons with stored times).
pub fn rfc3339_days_ago(days: i64) -> String {
    rfc3339(time::OffsetDateTime::now_utc() - time::Duration::days(days))
}

/// The time `secs` seconds ago.
pub fn rfc3339_secs_ago(secs: i64) -> String {
    rfc3339(time::OffsetDateTime::now_utc() - time::Duration::seconds(secs))
}

pub fn now_rfc3339() -> String {
    rfc3339(time::OffsetDateTime::now_utc())
}

/// New catalog id (uuid v7, time-ordered).
pub fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// The folder the running program's file is really in: through a symlink (a winget portable
/// install is started from `WinGet\Links`), without the `\?\` prefix. `pdfium.dll`,
/// `TWAINDSM.dll` and `tesseract\` are looked for here.
pub fn exe_dir() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let real = std::fs::canonicalize(&exe).unwrap_or(exe);
    plain_path(&real).parent().map(std::path::Path::to_path_buf)
}

/// `p` without Windows' `\\?\` prefix: `\\?\C:\x` → `C:\x`, and a network path
/// `\\?\UNC\server\share\x` → `\\server\share\x` (two backslashes: with one, a program run from
/// a mapped network drive looked for its DLLs and data folder in the wrong place, owner
/// 2026-11-05).
pub fn plain_path(p: &std::path::Path) -> std::path::PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\UNC\") {
        Some(rest) => std::path::PathBuf::from(format!(r"\\{rest}")),
        None => std::path::PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s)),
    }
}

#[cfg(test)]
mod path_tests {
    use super::plain_path;
    use std::path::Path;

    #[test]
    fn network_paths_keep_both_backslashes() {
        assert_eq!(
            plain_path(Path::new(r"\\?\UNC\xc.lan\APPS$\Vibe Files\vibe-files.exe")),
            Path::new(r"\\xc.lan\APPS$\Vibe Files\vibe-files.exe")
        );
        assert_eq!(
            plain_path(Path::new(r"\\?\C:\Vibe Files\vibe-files.exe")),
            Path::new(r"C:\Vibe Files\vibe-files.exe")
        );
        assert_eq!(plain_path(Path::new(r"T:\x.exe")), Path::new(r"T:\x.exe"));
    }
}
