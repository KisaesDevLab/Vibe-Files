//! Opening the catalog (plain SQLite since PA-18, QUESTIONS Q-27) and forward-only migrations
//! by `PRAGMA user_version`.

use std::path::{Path, PathBuf};

use rusqlite::OpenFlags;
pub use rusqlite::types::Value as SqlValue;
pub use rusqlite::{
    Connection, Error as SqlError, OptionalExtension, Transaction, params, params_from_iter,
};

use crate::{Error, Result};

/// Migrations in order; index + 1 is the `user_version` after applying it.
const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/001_initial.sql"),
    include_str!("../migrations/002_ocr_search.sql"),
    include_str!("../migrations/003_lookup_indexes.sql"),
    include_str!("../migrations/004_local_state.sql"),
    include_str!("../migrations/005_disposition.sql"),
    include_str!("../migrations/006_extract.sql"),
];

/// Schema version this build writes.
pub const SCHEMA_VERSION: u32 = MIGRATIONS.len() as u32;

/// Where the catalog lives. WAL only on local disk (CLAUDE.md §Stack); the LAN tier (P09) opens
/// with `Storage::Network`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Storage {
    Local,
    Network,
}

/// Starts a transaction that takes the write lock at once. Use it for every multi-statement
/// write through a shared `&Connection`: OCR workers hold their own connections (P03), and a
/// deferred transaction that reads before it writes cannot wait for them (SQLITE_BUSY_SNAPSHOT
/// fails at once instead of honouring the busy timeout).
pub fn begin_write(conn: &Connection) -> Result<Transaction<'_>> {
    Ok(Transaction::new_unchecked(
        conn,
        rusqlite::TransactionBehavior::Immediate,
    )?)
}

/// The first 16 bytes of every SQLite database file.
const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";

/// Whether `path` is missing, empty, or a plain SQLite file. An encrypted catalog from before
/// PA-18 (SQLCipher) starts with random salt instead.
pub fn is_plain_or_missing(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else {
        return true;
    };
    let mut head = [0u8; 16];
    match f.read(&mut head) {
        Ok(0) => true,
        Ok(16) => &head == SQLITE_HEADER,
        _ => false,
    }
}

/// Opens (creating if needed) the catalog at `path` and migrates it to [`SCHEMA_VERSION`].
/// Before migrating a non-empty catalog, a backup is written beside it.
pub fn open(path: &Path, storage: Storage) -> Result<Connection> {
    if !is_plain_or_missing(path) {
        return Err(Error::NotACatalog);
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    )?;
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))
        .map_err(|_| Error::NotACatalog)?;
    // OCR workers hold their own connections (P03); wait for a writer rather than fail.
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    conn.pragma_update(None, "foreign_keys", true)?;
    let mode = match storage {
        Storage::Local => "WAL",
        Storage::Network => "DELETE",
    };
    conn.pragma_update(None, "journal_mode", mode)?;
    migrate(&conn, path)?;
    Ok(conn)
}

pub fn user_version(conn: &Connection) -> Result<u32> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

fn migrate(conn: &Connection, path: &Path) -> Result<()> {
    let current = user_version(conn)?;
    if current > SCHEMA_VERSION {
        return Err(Error::NewerSchema {
            found: current,
            supported: SCHEMA_VERSION,
        });
    }
    if current == SCHEMA_VERSION {
        return Ok(());
    }
    if current > 0 {
        backup(conn, path, current)?;
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let to = i as u32 + 1;
        let tx = begin_write(conn)?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", to)?;
        tx.commit()?;
    }
    Ok(())
}

/// `catalog.db.v<from>-<unix secs>.bak`.
fn backup(conn: &Connection, path: &Path, from: u32) -> Result<PathBuf> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let name = format!(
        "{}.v{from}-{secs}.bak",
        path.file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default()
    );
    let dst_path = path.with_file_name(name);
    let mut dst = Connection::open(&dst_path)?;
    rusqlite::backup::Backup::new(conn, &mut dst)?.run_to_completion(
        256,
        std::time::Duration::ZERO,
        None,
    )?;
    Ok(dst_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLES: &[&str] = &[
        "audit",
        "cabinet",
        "client",
        "client_relationship",
        "consent_7216",
        "device",
        "disposition",
        "disposition_run",
        "doc_fts",
        "doc_fts_config",
        "doc_fts_content",
        "doc_fts_data",
        "doc_fts_docsize",
        "doc_fts_idx",
        "doc_type",
        "document",
        "document_extract",
        "document_tag",
        "legal_hold",
        "local_state",
        "lock",
        "ocr_job",
        "page_text",
        "retention_class",
        "saved_search",
        "sync_pending",
        "sync_state",
        "undo_record",
        "user",
        "user_client_acl",
        "version",
    ];

    fn tables(conn: &Connection) -> Vec<String> {
        let mut st = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap();
        st.query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    /// P00 done-when (PA-18: a plain SQLite file now): the migrations apply to an empty DB.
    #[test]
    fn migrations_apply_to_an_empty_db() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("catalog.db");
        let conn = open(&path, Storage::Local).unwrap();
        assert_eq!(user_version(&conn).unwrap(), SCHEMA_VERSION);
        assert_eq!(tables(&conn), TABLES);
        drop(conn);
        // PA-18 done-when: a plain SQLite file.
        let head = std::fs::read(&path).unwrap();
        assert!(head.starts_with(b"SQLite format 3\0"));
        // Reopen is a no-op migration and makes no backup.
        let conn = open(&path, Storage::Local).unwrap();
        assert_eq!(user_version(&conn).unwrap(), SCHEMA_VERSION);
        let baks = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter(|e| e.as_ref().unwrap().path().extension() == Some("bak".as_ref()))
            .count();
        assert_eq!(baks, 0);
    }

    /// P03: a P00–P02 catalog (version 1) gains the OCR and search tables, after a backup.
    #[test]
    fn migration_002_upgrades_a_version_1_catalog() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("catalog.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(MIGRATIONS[0]).unwrap();
            conn.pragma_update(None, "user_version", 1).unwrap();
        }
        let conn = open(&path, Storage::Local).unwrap();
        assert_eq!(user_version(&conn).unwrap(), SCHEMA_VERSION);
        assert!(tables(&conn).contains(&"ocr_job".to_string()));
        let baks = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter(|e| e.as_ref().unwrap().path().extension() == Some("bak".as_ref()))
            .count();
        assert_eq!(baks, 1);
    }

    /// An encrypted catalog from before PA-18 (or any other file) is refused, not overwritten.
    #[test]
    fn a_file_that_is_not_sqlite_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("catalog.db");
        std::fs::write(&path, vec![0x5a; 4096]).unwrap();
        assert!(!is_plain_or_missing(&path));
        assert!(matches!(
            open(&path, Storage::Local),
            Err(Error::NotACatalog)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), vec![0x5a; 4096]);
        assert!(is_plain_or_missing(&tmp.path().join("missing.db")));
    }

    #[test]
    fn newer_schema_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("catalog.db");
        let conn = open(&path, Storage::Local).unwrap();
        conn.pragma_update(None, "user_version", 99).unwrap();
        drop(conn);
        assert!(matches!(
            open(&path, Storage::Local),
            Err(Error::NewerSchema { found: 99, .. })
        ));
    }

    #[test]
    fn audit_is_append_only() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = open(&tmp.path().join("c.db"), Storage::Local).unwrap();
        conn.execute(
            "INSERT INTO audit (seq, ts, device_id, action, prev_hash, hash) VALUES (1, 't', 'd', 'a', 'p', 'h')",
            [],
        )
        .unwrap();
        assert!(conn.execute("UPDATE audit SET action = 'x'", []).is_err());
        assert!(conn.execute("DELETE FROM audit", []).is_err());
    }
}
