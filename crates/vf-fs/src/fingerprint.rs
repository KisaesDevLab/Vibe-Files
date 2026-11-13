//! A document's SHA-256, computed when it is first needed (PA-60, Q-96).
//!
//! A document is cataloged from its folder's listing: path, size and date. Its content is read
//! only when something needs the fingerprint (a duplicate check, a version, B2 sync, a
//! destruction approval, text recognition). The result is written to the catalog and to the
//! client folder's `docmeta.jsonl`, so no other PC reads the file for it again.

use std::path::Path;

use filer_intake::commit::sha256_file;
use serde_json::json;
use vf_core::audit::AuditEntry;
use vf_core::db::Connection;
use vf_core::tx::{FileOp, Stmt, Transaction};

use crate::docmeta;

/// Label of the Transaction that records a fingerprint (not on the undo list).
pub const LABEL: &str = "fingerprint";

/// How [`ensure`] got a document's fingerprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The catalog already had it.
    Catalog,
    /// Another PC had read the file and noted it in `docmeta.jsonl`; the file was not read.
    Record,
    /// The file was read here.
    Read,
}

/// The SHA-256 of live document `doc_id`, reading its file only when neither the catalog nor
/// the client folder's `docmeta.jsonl` has it for the file as it is now (same size and time).
pub fn ensure(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    doc_id: &str,
) -> Result<String, String> {
    ensure_how(conn, root, device_id, doc_id).map(|(h, _)| h)
}

/// [`ensure`], also saying where the fingerprint came from.
pub fn ensure_how(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    doc_id: &str,
) -> Result<(String, Source), String> {
    let mut meta = docmeta::load(conn, doc_id)?;
    if let Some(h) = &meta.hash {
        return Ok((h.clone(), Source::Catalog));
    }
    let path = root.join(&meta.rel_path);
    let Some((size, mtime, _)) = crate::cloud::stat(&path) else {
        return Err(format!("{} cannot be reached.", meta.rel_path));
    };
    // What another PC noted for this very file (same size and time): not read again.
    let folders: Vec<String> = meta.folder_rel.iter().cloned().collect();
    let noted = docmeta::Index::load(root, &folders)
        .latest_at(&meta.rel_path)
        .filter(|r| r.describes(size, mtime))
        .map(|r| r.hash.clone());
    let (hash, how) = match noted {
        Some(h) => (h, Source::Record),
        None => {
            let h = sha256_file(&path).map_err(|e| format!("{}: {e}", meta.rel_path))?;
            // Changed while it was read: its fingerprint is not this size and time's.
            if crate::cloud::stat(&path).map(|s| (s.0, s.1)) != Some((size, mtime)) {
                return Err(format!(
                    "{} changed while it was read; try again.",
                    meta.rel_path
                ));
            }
            (h, Source::Read)
        }
    };
    meta.hash = Some(hash.clone());
    meta.stamp = Some((size, mtime));
    let now = vf_core::now_rfc3339();
    let mut tx = Transaction::new(LABEL).catalog(Stmt::new(
        // Only while it is still unknown: another session may have filled it in meanwhile.
        "UPDATE document SET hash = ?2, size = ?3, file_mtime = ?4
         WHERE id = ?1 AND hash IS NULL AND deleted_at IS NULL",
        vec![
            doc_id.into(),
            hash.clone().into(),
            (size as i64).into(),
            mtime.into(),
        ],
    ));
    if how == Source::Read {
        // The record first (the Transaction applies file changes before the catalog's): a PC
        // that sees the catalog row without it would read the file again.
        let (file, line) = docmeta::record_line(root, &meta, &now, device_id);
        tx = tx.file(FileOp::AppendLines {
            path: file,
            lines: vec![line],
            undo_lines: vec![],
        });
    }
    tx = tx.audit(AuditEntry::new(
        LABEL,
        Some(doc_id),
        json!({ "from": match how { Source::Read => "file", _ => "record" } }),
    ));
    crate::fileops::commit(conn, root, tx, device_id)?;
    Ok((hash, how))
}

/// [`ensure`] for each of `doc_ids`, in order; a document whose file cannot be read is `Err`
/// and does not stop the others.
pub fn ensure_many(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    doc_ids: &[String],
) -> Vec<Result<String, String>> {
    doc_ids
        .iter()
        .map(|id| ensure(conn, root, device_id, id))
        .collect()
}

/// Live documents not fingerprinted yet.
pub fn pending(conn: &Connection) -> Result<usize, String> {
    conn.query_row(
        "SELECT count(*) FROM document WHERE hash IS NULL AND deleted_at IS NULL",
        [],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n as usize)
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use vf_core::db::Storage;

    use super::*;
    use crate::cabinet::{self, CabinetFile};

    struct Fx {
        _tmp: tempfile::TempDir,
        root: std::path::PathBuf,
        conn: Connection,
    }

    /// A cabinet with one document cataloged without a fingerprint, in a client folder.
    fn fx() -> Fx {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("cabinet");
        cabinet::create(
            &root,
            &CabinetFile {
                cabinet_id: vf_core::new_id(),
                firm_id: uuid::Uuid::new_v4().to_string(),
                schema_version: vf_core::db::SCHEMA_VERSION,
                layout_version: 1,
                created: vf_core::now_rfc3339(),
                name: None,
            },
        )
        .unwrap();
        let folder = root.join("Smith, Pat (1001)/Income Tax/2025");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("w2.pdf"), b"w2 of 2025").unwrap();
        let (size, mtime, _) = crate::cloud::stat(&folder.join("w2.pdf")).unwrap();
        let conn = vf_core::db::open(&tmp.path().join("catalog.db"), Storage::Local).unwrap();
        conn.execute(
            "INSERT INTO client (id, ext_id, name, folder_rel, device_id, updated_at)
             VALUES ('c1', '1001', 'Smith, Pat', 'Smith, Pat (1001)', 'dev', 't')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO document (id, client_id, rel_path, hash, size, filed_at, device_id, updated_at, file_mtime)
             VALUES ('d1', 'c1', 'Smith, Pat (1001)/Income Tax/2025/w2.pdf', NULL, ?1, 't', 'dev', 't', ?2)",
            (size as i64, mtime),
        )
        .unwrap();
        Fx {
            _tmp: tmp,
            root,
            conn,
        }
    }

    fn records(fx: &Fx) -> Vec<docmeta::Record> {
        std::fs::read_to_string(fx.root.join("Smith, Pat (1001)/_Vibe/docmeta.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    /// PA-60: the first need reads the file and writes the catalog and the record; the next
    /// takes the catalog's; another PC's catalog takes the record's without reading the file.
    #[test]
    fn a_fingerprint_is_read_once_and_shared() {
        let mut fx = fx();
        assert_eq!(pending(&fx.conn).unwrap(), 1);
        let want = sha256_file(&fx.root.join("Smith, Pat (1001)/Income Tax/2025/w2.pdf")).unwrap();
        let (h, how) = ensure_how(&mut fx.conn, &fx.root, "dev", "d1").unwrap();
        assert_eq!((h.as_str(), how), (want.as_str(), Source::Read));
        assert_eq!(pending(&fx.conn).unwrap(), 0);
        let recs = records(&fx);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].hash, want);
        assert_eq!(recs[0].id.as_deref(), Some("d1"));
        assert_eq!(recs[0].path, "Income Tax/2025/w2.pdf");
        assert!(recs[0].size.is_some() && recs[0].mtime.is_some());
        // Again: from the catalog, nothing more written.
        let (h, how) = ensure_how(&mut fx.conn, &fx.root, "dev", "d1").unwrap();
        assert_eq!((h.as_str(), how), (want.as_str(), Source::Catalog));
        assert_eq!(records(&fx).len(), 1);
        // Another PC's catalog (no fingerprint yet): the record's, and no second line.
        fx.conn
            .execute("UPDATE document SET hash = NULL WHERE id = 'd1'", [])
            .unwrap();
        let (h, how) = ensure_how(&mut fx.conn, &fx.root, "pc-2", "d1").unwrap();
        assert_eq!((h.as_str(), how), (want.as_str(), Source::Record));
        assert_eq!(records(&fx).len(), 1);
        assert_eq!(pending(&fx.conn).unwrap(), 0);
        // A PC that had not read the file writes a later record for it (a doc type set
        // there), with no fingerprint: the one recorded before is still found, not read again.
        let mut later = records(&fx)[0].clone();
        later.hash = String::new();
        later.device = "pc-3".into();
        later.status = "final".into();
        let file = fx.root.join("Smith, Pat (1001)/_Vibe/docmeta.jsonl");
        let mut text = std::fs::read_to_string(&file).unwrap();
        text.push_str(&serde_json::to_string(&later).unwrap());
        text.push(char::from(10));
        std::fs::write(&file, text).unwrap();
        fx.conn
            .execute("UPDATE document SET hash = NULL WHERE id = 'd1'", [])
            .unwrap();
        let (h, how) = ensure_how(&mut fx.conn, &fx.root, "pc-4", "d1").unwrap();
        assert_eq!((h.as_str(), how), (want.as_str(), Source::Record));
        assert_eq!(records(&fx).len(), 2);
    }

    /// A record written for the file as it was (another size) is not trusted: the file is read.
    #[test]
    fn a_record_of_other_content_is_not_trusted() {
        let mut fx = fx();
        let stale = serde_json::json!({
            "v": 1, "ts": "t", "device": "pc-a", "id": "d1", "hash": "ab".repeat(32),
            "path": "Income Tax/2025/w2.pdf", "doc_type": null, "doc_type_name": null,
            "tax_year": 2025, "status": "received", "source": null, "tags": [],
            "size": 3, "mtime": 5
        });
        let file = fx.root.join("Smith, Pat (1001)/_Vibe/docmeta.jsonl");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, format!("{stale}\n")).unwrap();
        let (h, how) = ensure_how(&mut fx.conn, &fx.root, "dev", "d1").unwrap();
        assert_eq!(how, Source::Read);
        assert_ne!(h, "ab".repeat(32));
        assert_eq!(records(&fx).len(), 2);
        // A file that is gone is an error, and the document stays unfingerprinted.
        fx.conn
            .execute("UPDATE document SET hash = NULL WHERE id = 'd1'", [])
            .unwrap();
        std::fs::remove_file(fx.root.join("Smith, Pat (1001)/Income Tax/2025/w2.pdf")).unwrap();
        assert!(ensure(&mut fx.conn, &fx.root, "dev", "d1").is_err());
        assert_eq!(pending(&fx.conn).unwrap(), 1);
    }
}
