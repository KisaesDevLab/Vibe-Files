//! Opening the catalog (plain SQLite since PA-18, QUESTIONS Q-27) and forward-only migrations
//! by `PRAGMA user_version`.

use std::path::{Path, PathBuf};

pub use rusqlite::types::Value as SqlValue;
pub use rusqlite::{
    Connection, Error as SqlError, OpenFlags, OptionalExtension, Transaction, params,
    params_from_iter,
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
    include_str!("../migrations/007_sync.sql"),
    include_str!("../migrations/008_tx_journal.sql"),
    include_str!("../migrations/009_sync_conflict_kinds.sql"),
    include_str!("../migrations/010_disposition_hash.sql"),
    include_str!("../migrations/011_doc_type_archived.sql"),
    include_str!("../migrations/012_client_partner.sql"),
    include_str!("../migrations/013_file_mtime.sql"),
    include_str!("../migrations/014_fts_doc_index.sql"),
    include_str!("../migrations/015_scale_indexes.sql"),
    include_str!("../migrations/016_journal_owner.sql"),
    include_str!("../migrations/017_hash_optional.sql"),
    include_str!("../migrations/018_no_page_text.sql"),
];

/// The migration that drops the stored page text (PA-65): a catalog that had it is compacted
/// once afterwards, or the file keeps its size.
const DROPS_PAGE_TEXT: u32 = 18;

/// The migration that rebuilds `document` (PA-60): the only one whose references are checked.
const REBUILDS_DOCUMENT: u32 = 17;

/// `local_state` key present while the catalog still waits to be compacted ([`compact_due`]).
const COMPACT_KEY: &str = "compact_due";

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

/// What to say when a row asked for by its id did not come: `missing` when the catalog has no
/// such row; else that the catalog could not be read just now (busy with another connection's
/// write, most often), with what it said. Every such failure used to be "is not in the
/// catalog" (QA 2026-11-17).
pub fn missing_or(e: SqlError, missing: &str) -> String {
    match e {
        SqlError::QueryReturnedNoRows => missing.to_string(),
        e => format!("The catalog could not be read just now ({e}). Try again in a moment."),
    }
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
        // Shorter than a header: not a database.
        Ok(_) => false,
        // Could not be read just now: not judged. Taken for "not plain", a healthy catalog
        // was set aside as an encrypted one and rebuilt (QA 2026-11-17); opening it says
        // what is wrong if something is.
        Err(_) => true,
    }
}

/// Folders whose catalogs are on a network share ([`on_network`]).
static NETWORK: std::sync::RwLock<Vec<std::path::PathBuf>> = std::sync::RwLock::new(Vec::new());

/// Says that catalogs under `dir` are on a network share: [`open`] then opens them as
/// [`Storage::Network`] whatever it is asked (the program opens its catalog in many places,
/// each as local; where the catalog is kept is decided once, by the app's catalog place).
pub fn on_network(dir: &Path) {
    let mut n = NETWORK.write().unwrap_or_else(|e| e.into_inner());
    if !n.iter().any(|d| d == dir) {
        n.push(dir.to_path_buf());
    }
}

/// Whether `path` is under a folder said to be on a network share ([`on_network`]).
pub fn is_on_network(path: &Path) -> bool {
    storage_of(path, Storage::Local) == Storage::Network
}

/// A path as text for comparing: lower case, `/` as `\`, and without the `\\?\` (or
/// `\\?\UNC\`) Windows and SQLite put before a long or a canonical path. The same catalog is
/// named in several ways (as the program built its path, as SQLite reports it).
fn spelled(p: &Path) -> String {
    let s = p.to_string_lossy().replace('/', "\\").to_lowercase();
    match s.strip_prefix(r"\\?\unc\") {
        Some(rest) => format!(r"\\{rest}"),
        None => s.strip_prefix(r"\\?\").unwrap_or(&s).to_string(),
    }
}

/// How the catalog at `path` is opened when `asked` for.
fn storage_of(path: &Path, asked: Storage) -> Storage {
    let n = NETWORK.read().unwrap_or_else(|e| e.into_inner());
    let p = spelled(path);
    let under = |d: &std::path::PathBuf| {
        let d = spelled(d);
        let d = d.trim_end_matches('\\');
        p.strip_prefix(d).is_some_and(|rest| rest.starts_with('\\'))
    };
    if n.iter().any(under) {
        Storage::Network
    } else {
        asked
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
    // Other connections write (the index, a sync): wait for a writer rather than fail. Set
    // before the first read: a catalog that was only busy was refused at once below, as
    // "not a Vibe Files catalog" (owner 2026-11-17, a PC whose index was running).
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))
        .map_err(|e| match e.sqlite_error_code() {
            // Only a file SQLite cannot read as a database is "not a catalog"; a busy or
            // unreadable one says what it is.
            Some(rusqlite::ErrorCode::NotADatabase) => Error::NotACatalog,
            _ => Error::Sql(e),
        })?;
    conn.pragma_update(None, "foreign_keys", true)?;
    let storage = storage_of(path, storage);
    let mode = match storage {
        Storage::Local => "WAL",
        Storage::Network => "DELETE",
    };
    conn.pragma_update(None, "journal_mode", mode)?;

    // Owner 2026-11-13 (Q-102): with a write-ahead log the catalog is flushed to disk at each
    // checkpoint, not at each commit. A power cut can lose the last commits, never corrupt
    // the catalog, and the catalog is rebuilt from the cabinet anyway.
    if storage == Storage::Local {
        conn.pragma_update(None, "synchronous", "NORMAL")?;
    }
    // A large catalog (PA-34): 32 MB of page cache per connection (the default is 2 MB),
    // sorts and temporary indexes in memory.
    conn.pragma_update(None, "cache_size", -32_768)?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    migrate(&conn, path)?;
    compact_due(&conn);
    // Statistics for the query planner, kept up to date cheaply (SQLite's advice for a
    // connection that opens: analyse only what needs it, with a limit).
    let _ = conn.execute_batch("PRAGMA optimize=0x10002;");
    Ok(conn)
}

/// `local_state` key: the month (`YYYY-MM`) of the last [`health_check`].
const HEALTH_KEY: &str = "health_check_month";

/// Whether this month's [`health_check`] has not run yet (Q-68: monthly, at startup).
pub fn health_check_due(conn: &Connection) -> bool {
    let month = &crate::now_rfc3339()[..7];
    let last: Option<String> = conn
        .query_row(
            "SELECT v FROM local_state WHERE k = ?1",
            [HEALTH_KEY],
            |r| r.get(0),
        )
        .ok();
    last.as_deref() != Some(month)
}

/// SQLite's `quick_check` of the catalog (PA-34): the problems it finds, none when the file is
/// sound. A sound check records the month; damage is reported again at every start until
/// the catalog is rebuilt.
pub fn health_check(conn: &Connection) -> Result<Vec<String>> {
    let mut st = conn.prepare("PRAGMA quick_check(20)")?;
    let rows: Vec<String> = st
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    if rows != ["ok"] {
        return Ok(rows);
    }
    conn.execute(
        "INSERT INTO local_state (k, v) VALUES (?1, ?2) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
        [HEALTH_KEY, &crate::now_rfc3339()[..7]],
    )?;
    Ok(vec![])
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
    // A migration that rebuilds a table other tables reference (017) cannot run with foreign
    // keys enforced: dropping the old table fails on its child rows. As SQLite's procedure for
    // it: enforcement off (it cannot change inside a transaction), the references checked
    // before each commit, enforcement on again whatever happened.
    conn.pragma_update(None, "foreign_keys", false)?;
    let run = || -> Result<()> {
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
            let to = i as u32 + 1;
            let tx = begin_write(conn)?;
            // Read again now that the write is this connection's: another login on this
            // computer, started at the same moment after an update, may have made this step
            // while this one waited. Most steps cannot be made twice ("duplicate column
            // name", QA 2026-11-17).
            if user_version(&tx)? >= to {
                continue;
            }
            // Only the migration that rebuilds a referenced table is checked (each check reads
            // every child table: code review 2026-11-13).
            let checked = to == REBUILDS_DOCUMENT;
            let before = if checked { broken_references(&tx)? } else { 0 };
            tx.execute_batch(sql)?;
            // Only what the migration broke stops it: a catalog with an old stray row still
            // opens.
            let after = if checked { broken_references(&tx)? } else { 0 };
            if after > before {
                return Err(Error::Tx(format!(
                    "catalog migration {to} left {} rows pointing at missing rows; the catalog was not changed",
                    after - before
                )));
            }
            if to == DROPS_PAGE_TEXT && current > 0 {
                // The space is given back after the migrations ([`compact_due`]).
                tx.execute(
                    "INSERT OR REPLACE INTO local_state (k, v) VALUES (?1, '1')",
                    [COMPACT_KEY],
                )?;
            }
            tx.pragma_update(None, "user_version", to)?;
            tx.commit()?;
        }
        Ok(())
    };
    let done = run();
    conn.pragma_update(None, "foreign_keys", true)?;
    done
}

/// Gives back the space the dropped page text took (PA-65), once: `VACUUM` cannot run inside a
/// transaction, and it can take a minute or more on a large catalog. Its working copy goes to
/// a file, not to memory (the catalog can be gigabytes: code review 2026-11-13). When it
/// cannot run now (another session has the catalog open), the note stays and the next open
/// tries again.
fn compact_due(conn: &Connection) {
    let due = conn
        .query_row(
            "SELECT 1 FROM local_state WHERE k = ?1",
            [COMPACT_KEY],
            |_| Ok(()),
        )
        .is_ok();
    if !due {
        return;
    }
    // Tried without waiting, and not again in this run once it could not be: every connection the program
    // opens (each worker opens its own) used to try, and while another login's VACUUM ran
    // each waited ten seconds for nothing (QA 2026-11-17).
    static GAVE_UP: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let file = conn.path().unwrap_or_default().to_lowercase();
    if GAVE_UP
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&file)
    {
        return;
    }
    let _ = conn.busy_timeout(std::time::Duration::ZERO);
    let _ = conn.pragma_update(None, "temp_store", "FILE");
    let done = conn.execute_batch("VACUUM").is_ok();
    let _ = conn.pragma_update(None, "temp_store", "MEMORY");
    let _ = conn.busy_timeout(std::time::Duration::from_secs(10));
    if !done {
        // Busy now (another login has the catalog): the next run of the program tries.
        GAVE_UP.lock().unwrap_or_else(|e| e.into_inner()).push(file);
    }
    if done {
        let _ = conn.execute("DELETE FROM local_state WHERE k = ?1", [COMPACT_KEY]);
    }
}

/// Rows whose foreign key points at no row (`PRAGMA foreign_key_check`).
fn broken_references(conn: &Connection) -> Result<usize> {
    let mut st = conn.prepare("PRAGMA foreign_key_check")?;
    let mut rows = st.query([])?;
    let mut n = 0;
    while rows.next()?.is_some() {
        n += 1;
    }
    Ok(n)
}

/// `catalog.db.v<from>-<unix secs>.bak`.
fn backup(conn: &Connection, path: &Path, from: u32) -> Result<PathBuf> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    // With this process's id: two logins migrating in the same second each write their own.
    let name = format!(
        "{}.v{from}-{secs}-{}.bak",
        path.file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default(),
        std::process::id()
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

    /// PA-84: a catalog under a folder said to be on a network share is opened without the
    /// write-ahead log, whatever the caller asks; one elsewhere is not affected.
    #[test]
    fn a_catalog_on_a_network_share_is_opened_without_wal() {
        let t = tempfile::tempdir().unwrap();
        let (share, local) = (t.path().join("share"), t.path().join("local"));
        std::fs::create_dir_all(share.join("HOST")).unwrap();
        std::fs::create_dir_all(&local).unwrap();
        on_network(&share);
        let mode = |p: &Path| -> String {
            open(p, Storage::Local)
                .unwrap()
                .query_row("PRAGMA journal_mode", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(mode(&share.join("HOST").join("catalog.db")), "delete");
        assert_eq!(mode(&local.join("catalog.db")), "wal");
        // However the path is spelled (another case, `/`, the `\\?\` of a canonical path).
        let again = share.join("HOST").join("catalog.db");
        let upper = Path::new(&again.to_string_lossy().to_uppercase()).to_path_buf();
        assert!(is_on_network(&upper));
        assert!(is_on_network(Path::new(&format!(
            r"\\?\{}",
            again.display()
        ))));
        assert!(is_on_network(Path::new(
            &again.to_string_lossy().replace('\\', "/")
        )));
        // A folder whose name only starts the same is another folder.
        assert!(!is_on_network(&t.path().join("share2").join("catalog.db")));
    }
    const TABLES: &[&str] = &[
        "audit",
        "cabinet",
        "client",
        "client_relationship",
        "consent_7216",
        "device",
        "disposition",
        "disposition_run",
        "doc_type",
        "document",
        "document_extract",
        "document_tag",
        "legal_hold",
        "local_state",
        "lock",
        "retention_class",
        "saved_search",
        "sync_base",
        "sync_conflict",
        "sync_object",
        "sync_pending",
        "sync_segment",
        "sync_state",
        "tx_journal",
        "undo_record",
        "user",
        "user_client_acl",
        "version",
    ];

    fn tables(conn: &Connection) -> Vec<String> {
        let mut st = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .unwrap();
        st.query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    /// PA-34 (Q-68): the integrity check runs once a month; the scale indexes exist.
    #[test]
    fn health_check_runs_monthly() {
        let t = tempfile::tempdir().unwrap();
        let conn = open(&t.path().join("c.db"), Storage::Local).unwrap();
        assert!(health_check_due(&conn));
        assert!(health_check(&conn).unwrap().is_empty());
        assert!(!health_check_due(&conn));
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'index'
                 AND name IN ('audit_tx', 'document_tag_tag', 'document_deleted')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 3);
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

    /// PA-60: migration 017 lets a document have no fingerprint yet. A version 16 catalog keeps
    /// every document, its child rows and its indexes; foreign keys are enforced again after.
    #[test]
    fn migration_017_keeps_documents_and_lets_the_hash_be_unknown() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("catalog.db");
        {
            let conn = Connection::open(&path).unwrap();
            for sql in &MIGRATIONS[..16] {
                conn.execute_batch(sql).unwrap();
            }
            conn.pragma_update(None, "user_version", 16).unwrap();
            conn.execute_batch(
                "INSERT INTO document (id, rel_path, hash, size, filed_at, device_id, updated_at, file_mtime, tombstone_reason)
                 VALUES ('d1', 'A (1)/a.pdf', 'h1', 10, 't', 'dev', 't', 1700000000, NULL),
                        ('d2', 'A (1)/b.pdf', 'h2', 20, 't', 'dev', 't', NULL, 'why');
                 UPDATE document SET deleted_at = 't' WHERE id = 'd2';
                 INSERT INTO document_tag (doc_id, tag) VALUES ('d1', 'x');
                 INSERT INTO version (doc_id, hash, size, created, device_id)
                 VALUES ('d1', 'h0', 5, 't', 'dev');",
            )
            .unwrap();
        }
        let indexes = |conn: &Connection| -> Vec<String> {
            let mut st = conn
                .prepare(
                    "SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'document'
                     AND name NOT LIKE 'sqlite_%' ORDER BY name",
                )
                .unwrap();
            st.query_map([], |r| r.get(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        let conn = open(&path, Storage::Local).unwrap();
        assert_eq!(user_version(&conn).unwrap(), SCHEMA_VERSION);
        type Row = (String, Option<String>, i64, Option<i64>, Option<String>);
        let rows: Vec<Row> = conn
            .prepare(
                "SELECT id, hash, size, file_mtime, tombstone_reason FROM document ORDER BY id",
            )
            .unwrap()
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(
            rows,
            [
                (
                    "d1".into(),
                    Some("h1".into()),
                    10,
                    Some(1_700_000_000),
                    None
                ),
                ("d2".into(), Some("h2".into()), 20, None, Some("why".into())),
            ]
        );
        assert_eq!(
            indexes(&conn),
            [
                "document_client",
                "document_deleted",
                "document_filed_at",
                "document_filed_by",
                "document_hash",
                "document_rel_path",
                "document_unhashed_size",
                "document_updated_at",
            ]
        );
        assert_eq!(broken_references(&conn).unwrap(), 0);
        assert!(tables(&conn).iter().all(|t| t != "document_new"));
        // PA-65: the page text tables are gone and the catalog was compacted (nothing waits).
        assert!(
            tables(&conn)
                .iter()
                .all(|t| t != "page_text" && t != "ocr_job")
        );
        let waits: i64 = conn
            .query_row(
                "SELECT count(*) FROM local_state WHERE k = 'compact_due'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(waits, 0);
        // A document with no fingerprint yet; the live-path index still refuses a second one.
        conn.execute(
            "INSERT INTO document (id, rel_path, hash, size, filed_at, device_id, updated_at)
             VALUES ('d3', 'A (1)/c.pdf', NULL, 30, 't', 'dev', 't')",
            [],
        )
        .unwrap();
        assert!(
            conn.execute(
                "INSERT INTO document (id, rel_path, hash, size, filed_at, device_id, updated_at)
                 VALUES ('d4', 'A (1)/c.pdf', NULL, 30, 't', 'dev', 't')",
                [],
            )
            .is_err()
        );
        // Foreign keys are enforced again: a tag of no document is refused.
        let on: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(on, 1);
        assert!(
            conn.execute(
                "INSERT INTO document_tag (doc_id, tag) VALUES ('nope', 'x')",
                []
            )
            .is_err()
        );
        // A catalog made new has the same document table.
        let fresh = open(&tmp.path().join("fresh.db"), Storage::Local).unwrap();
        assert_eq!(indexes(&fresh), indexes(&conn));
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
        // Migration 002's page text tables came and, with PA-65, went again.
        assert_eq!(tables(&conn), TABLES);
        let baks = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter(|e| e.as_ref().unwrap().path().extension() == Some("bak".as_ref()))
            .count();
        assert_eq!(baks, 1);
    }

    /// Owner 2026-11-17: a catalog another connection is writing is busy, not "not a Vibe
    /// Files catalog" (the audit check said so on a PC whose index was running).
    #[test]
    fn a_busy_catalog_is_busy_not_foreign() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("catalog.db");
        let writer = open(&path, Storage::Local).unwrap();
        // Held as SQLite holds a catalog on a share while it is written.
        writer
            .execute_batch("PRAGMA locking_mode = EXCLUSIVE; BEGIN EXCLUSIVE;")
            .unwrap();
        let e = open(&path, Storage::Local).unwrap_err();
        assert!(e.is_busy(), "{e}");
        assert!(!matches!(e, Error::NotACatalog));
        writer
            .execute_batch("COMMIT; PRAGMA locking_mode = NORMAL;")
            .unwrap();
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
