//! Opening the SQLCipher catalog and forward-only migrations by `PRAGMA user_version`.

use std::path::{Path, PathBuf};

use rusqlite::OpenFlags;
pub use rusqlite::types::Value as SqlValue;
pub use rusqlite::{Connection, Error as SqlError, OptionalExtension, params, params_from_iter};

use crate::{Error, Result};

/// Migrations in order; index + 1 is the `user_version` after applying it.
const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/001_initial.sql"),
    include_str!("../migrations/002_ocr_search.sql"),
];

/// Schema version this build writes.
pub const SCHEMA_VERSION: u32 = MIGRATIONS.len() as u32;

/// Raw 32-byte catalog key. Wrapping (DPAPI, recovery passphrase) is `vf-security::keys` (P06).
#[derive(Clone)]
pub struct CatalogKey(pub [u8; 32]);

impl std::fmt::Debug for CatalogKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CatalogKey(..)")
    }
}

impl CatalogKey {
    fn pragma(&self) -> String {
        let hex: String = self.0.iter().map(|b| format!("{b:02x}")).collect();
        format!("x'{hex}'")
    }
}

/// Where the catalog lives. WAL only on local disk (CLAUDE.md §Stack); the LAN tier (P09) opens
/// with `Storage::Network`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Storage {
    Local,
    Network,
}

/// Opens (creating if needed) the catalog at `path`, checks the key, and migrates it to
/// [`SCHEMA_VERSION`]. Before migrating a non-empty catalog, a keyed backup is written beside it.
pub fn open(path: &Path, key: &CatalogKey, storage: Storage) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    )?;
    apply_key(&conn, key)?;
    // OCR workers hold their own connections (P03); wait for a writer rather than fail.
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    conn.pragma_update(None, "foreign_keys", true)?;
    let mode = match storage {
        Storage::Local => "WAL",
        Storage::Network => "DELETE",
    };
    conn.pragma_update(None, "journal_mode", mode)?;
    migrate(&conn, path, key)?;
    Ok(conn)
}

fn apply_key(conn: &Connection, key: &CatalogKey) -> Result<()> {
    conn.execute_batch(&format!("PRAGMA key = \"{}\";", key.pragma()))?;
    // Wrong key or not a SQLCipher file: the first read fails.
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))
        .map_err(|_| Error::BadKey)
}

pub fn user_version(conn: &Connection) -> Result<u32> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

fn migrate(conn: &Connection, path: &Path, key: &CatalogKey) -> Result<()> {
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
        backup(conn, path, key, current)?;
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let to = i as u32 + 1;
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", to)?;
        tx.commit()?;
    }
    Ok(())
}

/// `catalog.db.v<from>-<unix secs>.bak`, encrypted with the same key.
fn backup(conn: &Connection, path: &Path, key: &CatalogKey, from: u32) -> Result<PathBuf> {
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
    apply_key(&dst, key)?;
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

    fn key(b: u8) -> CatalogKey {
        CatalogKey([b; 32])
    }

    const TABLES: &[&str] = &[
        "audit",
        "cabinet",
        "client",
        "client_relationship",
        "consent_7216",
        "device",
        "doc_fts",
        "doc_fts_config",
        "doc_fts_content",
        "doc_fts_data",
        "doc_fts_docsize",
        "doc_fts_idx",
        "doc_type",
        "document",
        "document_tag",
        "legal_hold",
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

    /// P00 done-when: migration 001 applies to an empty SQLCipher DB.
    #[test]
    fn migration_001_applies_to_empty_sqlcipher_db() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("catalog.db");
        let conn = open(&path, &key(7), Storage::Local).unwrap();
        assert_eq!(user_version(&conn).unwrap(), SCHEMA_VERSION);
        assert_eq!(tables(&conn), TABLES);
        drop(conn);
        // Encrypted on disk: no SQLite header.
        let head = std::fs::read(&path).unwrap();
        assert!(!head.starts_with(b"SQLite format 3"));
        // Reopen is a no-op migration and makes no backup.
        let conn = open(&path, &key(7), Storage::Local).unwrap();
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
            apply_key(&conn, &key(3)).unwrap();
            conn.execute_batch(MIGRATIONS[0]).unwrap();
            conn.pragma_update(None, "user_version", 1).unwrap();
        }
        let conn = open(&path, &key(3), Storage::Local).unwrap();
        assert_eq!(user_version(&conn).unwrap(), 2);
        assert!(tables(&conn).contains(&"ocr_job".to_string()));
        let baks = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter(|e| e.as_ref().unwrap().path().extension() == Some("bak".as_ref()))
            .count();
        assert_eq!(baks, 1);
    }

    #[test]
    fn wrong_key_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("catalog.db");
        drop(open(&path, &key(1), Storage::Local).unwrap());
        assert!(matches!(
            open(&path, &key(2), Storage::Local),
            Err(Error::BadKey)
        ));
    }

    #[test]
    fn newer_schema_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("catalog.db");
        let conn = open(&path, &key(1), Storage::Local).unwrap();
        conn.pragma_update(None, "user_version", 99).unwrap();
        drop(conn);
        assert!(matches!(
            open(&path, &key(1), Storage::Local),
            Err(Error::NewerSchema { found: 99, .. })
        ));
    }

    #[test]
    fn audit_is_append_only() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = open(&tmp.path().join("c.db"), &key(1), Storage::Local).unwrap();
        conn.execute(
            "INSERT INTO audit (seq, ts, device_id, action, prev_hash, hash) VALUES (1, 't', 'd', 'a', 'p', 'h')",
            [],
        )
        .unwrap();
        assert!(conn.execute("UPDATE audit SET action = 'x'", []).is_err());
        assert!(conn.execute("DELETE FROM audit", []).is_err());
    }
}
