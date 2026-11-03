//! Exact duplicates (P03 step 7, QUESTIONS Q-27): a document whose content (SHA-256) is
//! already in the cabinet. Found right after Filer files a batch; the user removes the new
//! copy, keeps both, or replaces the old one. Removing moves the file to `_Vibe/trash/` and
//! tombstones its row in one Transaction (undoable; P05 adds the Trash view).

use std::path::Path;

use serde_json::json;
use vf_core::audit::AuditEntry;
use vf_core::db::Connection;
use vf_core::tx::{FileOp, Stmt, Transaction};

pub const TRASH_LABEL: &str = "trash_duplicate";

/// A new document and an older one with the same content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dup {
    pub new_id: String,
    pub new_rel: String,
    pub old_id: String,
    pub old_rel: String,
}

/// Live documents holding content `hash`, oldest first: (id, cabinet path, filed at). A
/// document whose file OCR gave a text layer still answers to its original scan (that content
/// is kept as a version, P03 step 3). Both branches use an index on `hash`.
fn holders(conn: &Connection, hash: &str) -> Result<Vec<(String, String, String)>, String> {
    let mut st = conn
        .prepare_cached(
            "SELECT id, rel_path, filed_at FROM document WHERE hash = ?1 AND deleted_at IS NULL
             UNION
             SELECT d.id, d.rel_path, d.filed_at FROM version v JOIN document d ON d.id = v.doc_id
              WHERE v.hash = ?1 AND v.created_by = ?2 AND d.deleted_at IS NULL
             ORDER BY 3, 1",
        )
        .map_err(|e| e.to_string())?;
    let rows = st
        .query_map([hash, crate::cabinet::TEXT_LAYER_VERSION_BY], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
}

/// Live documents filed at or after `since` (RFC 3339) whose content an older live document
/// already has (the oldest such document is named).
pub fn filed_since(conn: &Connection, since: &str) -> Result<Vec<Dup>, String> {
    let mut st = conn
        .prepare(
            "SELECT id, rel_path, hash, filed_at FROM document
             WHERE deleted_at IS NULL AND filed_at >= ?1 ORDER BY rel_path",
        )
        .map_err(|e| e.to_string())?;
    let new: Vec<(String, String, String, String)> = st
        .query_map([since], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for (new_id, new_rel, hash, filed) in new {
        let older = holders(conn, &hash)?.into_iter().find(|(id, _, at)| {
            *id != new_id && (at.as_str(), id.as_str()) < (filed.as_str(), new_id.as_str())
        });
        if let Some((old_id, old_rel, _)) = older {
            out.push(Dup {
                new_id,
                new_rel,
                old_id,
                old_rel,
            });
        }
    }
    Ok(out)
}

/// The live document holding content `hash`, if any (cabinet-relative path; see [`holders`]).
pub fn existing(conn: &Connection, hash: &str) -> Option<String> {
    holders(conn, hash).ok()?.into_iter().next().map(|h| h.1)
}

/// Moves document `doc_id`'s file to `_Vibe/trash/<id>/<name>` and tombstones it, as one
/// Transaction. Returns the transaction id (undo puts it back).
pub fn trash(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    doc_id: &str,
    reason: &str,
) -> Result<String, String> {
    let rel: String = conn
        .query_row(
            "SELECT rel_path FROM document WHERE id = ?1 AND deleted_at IS NULL",
            [doc_id],
            |r| r.get(0),
        )
        .map_err(|_| "the document is not in the catalog any more".to_string())?;
    let src = root.join(&rel);
    let name = src
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let dst = crate::cabinet::trash_dir(root).join(doc_id).join(name);
    let now = vf_core::now_rfc3339();
    let tx = Transaction::new(TRASH_LABEL)
        .file(FileOp::Move { src, dst })
        .catalog(Stmt::new(
            "UPDATE document SET deleted_at = ?2, tombstone_reason = ?3, rev = rev + 1,
             updated_at = ?2, device_id = ?4 WHERE id = ?1",
            vec![
                doc_id.into(),
                now.clone().into(),
                reason.into(),
                device_id.into(),
            ],
        ))
        .catalog_undo(Stmt::new(
            "UPDATE document SET deleted_at = NULL, tombstone_reason = NULL, rev = rev + 1
             WHERE id = ?1",
            vec![doc_id.into()],
        ))
        .catalog(Stmt::new(
            "INSERT INTO sync_pending (doc_id, op, since) VALUES (?1, 'delete', ?2)
             ON CONFLICT(doc_id, op) DO NOTHING",
            vec![doc_id.into(), now.clone().into()],
        ))
        .catalog_undo(Stmt::new(
            "DELETE FROM sync_pending WHERE doc_id = ?1 AND op = 'delete'",
            vec![doc_id.into()],
        ))
        .audit(AuditEntry::new(
            "trash",
            Some(doc_id),
            json!({ "rel_path": rel, "reason": reason }),
        ));
    let c = crate::fileops::commit(conn, root, tx, device_id)?;
    Ok(c.tx_id)
}

/// What the user chose for a duplicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    RemoveNew,
    KeepBoth,
    ReplaceOld,
}

/// Applies `choice` to `d`. Returns the trash transaction, if any.
pub fn resolve(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    d: &Dup,
    choice: Choice,
) -> Result<Option<String>, String> {
    match choice {
        Choice::KeepBoth => {
            let tx = Transaction::new("duplicate_kept").audit(AuditEntry::new(
                "duplicate_kept",
                Some(&d.new_id),
                json!({ "same_as": d.old_id }),
            ));
            crate::fileops::commit(conn, root, tx, device_id)?;
            Ok(None)
        }
        Choice::RemoveNew => trash(conn, root, device_id, &d.new_id, "duplicate").map(Some),
        Choice::ReplaceOld => {
            trash(conn, root, device_id, &d.old_id, "replaced by duplicate").map(Some)
        }
    }
}

#[cfg(test)]
mod tests {
    use vf_core::db::Storage;

    use super::*;

    fn fx() -> (tempfile::TempDir, std::path::PathBuf, Connection) {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("cab");
        std::fs::create_dir_all(root.join("A (1)/2025")).unwrap();
        let conn = vf_core::db::open(&t.path().join("c.db"), Storage::Local).unwrap();
        for (id, rel, hash, filed) in [
            ("old", "A (1)/2025/w2.pdf", "h1", "2026-01-01T00:00:00Z"),
            ("other", "A (1)/2025/k1.pdf", "h2", "2026-01-01T00:00:00Z"),
            ("new", "A (1)/2025/w2 (2).pdf", "h1", "2026-11-03T10:00:00Z"),
        ] {
            std::fs::write(root.join(rel), id).unwrap();
            conn.execute(
                "INSERT INTO document (id, rel_path, hash, size, filed_at, device_id, updated_at)
                 VALUES (?1, ?2, ?3, 1, ?4, 'dev', 't')",
                (id, rel, hash, filed),
            )
            .unwrap();
        }
        (t, root, conn)
    }

    #[test]
    fn new_copies_of_known_content_are_found() {
        let (_t, _root, conn) = fx();
        let d = filed_since(&conn, "2026-11-01T00:00:00Z").unwrap();
        assert_eq!(
            d,
            vec![Dup {
                new_id: "new".into(),
                new_rel: "A (1)/2025/w2 (2).pdf".into(),
                old_id: "old".into(),
                old_rel: "A (1)/2025/w2.pdf".into(),
            }]
        );
        assert!(
            filed_since(&conn, "2026-11-04T00:00:00Z")
                .unwrap()
                .is_empty()
        );
        assert_eq!(existing(&conn, "h1").as_deref(), Some("A (1)/2025/w2.pdf"));
        assert_eq!(existing(&conn, "nope"), None);
        // OCR wrote a text layer into "other": its original scan still matches it.
        conn.execute(
            "INSERT INTO version (doc_id, hash, size, created, created_by, device_id)
             VALUES ('other', 'scan-original', 1, 't', 'ocr', 'dev')",
            [],
        )
        .unwrap();
        assert_eq!(
            existing(&conn, "scan-original").as_deref(),
            Some("A (1)/2025/k1.pdf")
        );
    }

    #[test]
    fn remove_new_trashes_it_and_undo_restores() {
        let (_t, root, mut conn) = fx();
        let d = filed_since(&conn, "2026-11-01T00:00:00Z")
            .unwrap()
            .remove(0);
        let tx = resolve(&mut conn, &root, "dev", &d, Choice::RemoveNew)
            .unwrap()
            .unwrap();
        assert!(!root.join(&d.new_rel).exists());
        assert!(root.join("_Vibe/trash/new/w2 (2).pdf").is_file());
        let (deleted, reason): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT deleted_at, tombstone_reason FROM document WHERE id = 'new'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(deleted.is_some());
        assert_eq!(reason.as_deref(), Some("duplicate"));
        assert!(
            filed_since(&conn, "2026-11-01T00:00:00Z")
                .unwrap()
                .is_empty()
        );

        crate::fileops::undo(&mut conn, &root, &tx, "dev").unwrap();
        assert!(root.join(&d.new_rel).is_file());
        let deleted: Option<String> = conn
            .query_row(
                "SELECT deleted_at FROM document WHERE id = 'new'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(deleted.is_none());
    }

    #[test]
    fn replace_old_and_keep_both() {
        let (_t, root, mut conn) = fx();
        let d = filed_since(&conn, "2026-11-01T00:00:00Z")
            .unwrap()
            .remove(0);
        assert_eq!(
            resolve(&mut conn, &root, "dev", &d, Choice::KeepBoth).unwrap(),
            None
        );
        assert!(root.join(&d.old_rel).is_file() && root.join(&d.new_rel).is_file());
        resolve(&mut conn, &root, "dev", &d, Choice::ReplaceOld).unwrap();
        assert!(!root.join(&d.old_rel).exists());
        assert!(root.join(&d.new_rel).is_file());
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM audit WHERE action IN ('trash', 'duplicate_kept')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 2);
    }
}
