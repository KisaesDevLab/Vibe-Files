//! Catalog schema, document model, Transaction, doc types, retention, audit, locks.
//! No UI or network dependency (CLAUDE.md §Workspace).

pub mod audit;
pub mod browse;
pub mod db;
pub mod doctype;
pub mod meta;
pub mod tx;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("catalog: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("the catalog key is wrong, or this is not a Vibe Files catalog")]
    BadKey,
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
pub fn now_rfc3339() -> String {
    let now = time::OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339)
        .expect("RFC 3339 formats")
}

/// New catalog id (uuid v7, time-ordered).
pub fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}
