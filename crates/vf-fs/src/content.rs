//! New content for documents, as Transactions (P03 write-back, every P04 writer).
//!
//! [`replace`]: a document gets new bytes (text layer, annotations, stamp, flatten, PDF/A,
//! page edits, compression). The old bytes are kept as a version (Q-17) with its sidecar, the
//! new file moves into place, the docmeta line follows the new hash, and the catalog row,
//! version row, `sync_pending` and audit change together.
//!
//! [`add`]: a new document from a prepared file (Save as, a redacted copy, an extracted page
//! range), with its catalog row, docmeta line, tags and audit row.
//!
//! The Filer rule lives here, once: a file in Filer's current undoable batch keeps its bytes
//! (Filer refuses to undo a file whose size changed).

use std::path::{Path, PathBuf};

use serde_json::json;
use vf_core::audit::AuditEntry;
use vf_core::db::{Connection, OptionalExtension};
use vf_core::tx::{FileOp, Stmt, Transaction};

use crate::cabinet;
use crate::docmeta::{self, Meta};

/// Why [`replace`] did not change the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// In Filer's current undoable batch: undo it, or file another batch, first.
    FilerUndo,
    /// Checked out.
    Locked,
    /// The file on disk is not the content the catalog knows (edited outside Vibe Files).
    Changed,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Refused::FilerUndo => {
                "the file is in Intake's last batch; it can be changed once that batch can no longer be undone"
            }
            Refused::Locked => "the document is checked out",
            Refused::Changed => "the file changed since it was read (outside Vibe Files or by another task)",
        })
    }
}

/// A temp file in the cabinet's working folder, for a writer to fill before [`replace`] or
/// [`add`] moves it into place (same volume: a rename).
pub fn tmp_file(root: &Path, ext: &str) -> Result<PathBuf, String> {
    let dir = cabinet::tmp_dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    Ok(dir.join(format!("vf-{}.{ext}", uuid::Uuid::now_v7().simple())))
}

/// What a writer did, for [`replace`].
pub struct Change<'a> {
    /// Transaction and audit action (`annotate`, `stamp`, `ocr_text_layer`, …).
    pub label: &'a str,
    /// Version row `created_by` and sidecar (e.g. [`cabinet::TEXT_LAYER_VERSION_BY`]).
    pub created_by: &'a str,
    /// Why the old bytes were kept (`before annotations`).
    pub reason: &'a str,
    /// Extra audit detail (merged with the hashes).
    pub detail: serde_json::Value,
    /// Further catalog statements in the same Transaction, with their inverses. They may use
    /// `?OLD` / `?NEW` placeholders for the old and new hashes.
    pub extra: Vec<(Stmt, Option<Stmt>)>,
    /// The content the new file was made from (catalog hash when the caller read the file).
    /// When it is no longer the document's content, the replace is refused as
    /// [`Refused::Changed`]: an edit made on an older copy never lands on newer content (QA A11).
    pub based_on: Option<String>,
}

/// What [`replace`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replaced {
    pub tx_id: String,
    pub old_hash: String,
    pub new_hash: String,
}

/// The document's file (cabinet path, hash, size) if it may be changed now.
pub fn check(
    conn: &Connection,
    root: &Path,
    doc_id: &str,
) -> Result<Result<(String, String, i64), Refused>, String> {
    let (rel, hash, size): (String, String, i64) = conn
        .query_row(
            "SELECT rel_path, hash, size FROM document WHERE id = ?1 AND deleted_at IS NULL",
            [doc_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|_| "the document is not in the catalog".to_string())?;
    if crate::intake::undoable_paths(root).contains(&rel.to_lowercase()) {
        return Ok(Err(Refused::FilerUndo));
    }
    let locked = conn
        .query_row("SELECT 1 FROM lock WHERE doc_id = ?1", [doc_id], |_| Ok(()))
        .optional()
        .map_err(|e| e.to_string())?
        .is_some();
    if locked {
        return Ok(Err(Refused::Locked));
    }
    let on_disk = filer_intake::commit::sha256_file(&root.join(&rel)).map_err(|e| e.to_string())?;
    if on_disk != hash {
        return Ok(Err(Refused::Changed));
    }
    Ok(Ok((rel, hash, size)))
}

/// Gives `doc_id` the content of `new_file` (a file in [`cabinet::tmp_dir`], consumed on
/// success and removed on failure).
pub fn replace(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    doc_id: &str,
    new_file: &Path,
    change: Change<'_>,
) -> Result<Result<Replaced, Refused>, String> {
    replace_also(conn, root, device_id, doc_id, new_file, change, vec![])
}

/// Further audit rows on a document, `(action, detail)`, written in the same Transaction as
/// its content change (a pasted tape's `tape_paste` rows).
pub type AlsoAudit = Vec<(String, serde_json::Value)>;

/// [`replace`] with further audit rows on the document in the same Transaction.
pub fn replace_also(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    doc_id: &str,
    new_file: &Path,
    change: Change<'_>,
    also: AlsoAudit,
) -> Result<Result<Replaced, Refused>, String> {
    let r = replace_inner(conn, root, device_id, doc_id, new_file, change, also);
    if !matches!(r, Ok(Ok(_))) {
        let _ = std::fs::remove_file(new_file);
    }
    r
}

fn replace_inner(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    doc_id: &str,
    new_file: &Path,
    change: Change<'_>,
    also: AlsoAudit,
) -> Result<Result<Replaced, Refused>, String> {
    let (rel, old_hash, old_size) = match check(conn, root, doc_id)? {
        Ok(v) => v,
        Err(why) => return Ok(Err(why)),
    };
    if change.based_on.as_ref().is_some_and(|b| *b != old_hash) {
        return Ok(Err(Refused::Changed));
    }
    let path = root.join(&rel);
    let new_hash = filer_intake::commit::sha256_file(new_file).map_err(|e| e.to_string())?;
    let new_size = std::fs::metadata(new_file)
        .map(|m| m.len())
        .map_err(|e| e.to_string())? as i64;

    // The old bytes are kept as a version; if that content is already kept, they go to trash.
    let version = cabinet::version_path(root, &old_hash);
    let kept_at = if version.exists() {
        cabinet::trash_dir(root).join(format!(
            "{}-{}-{}",
            change.label,
            uuid::Uuid::now_v7().simple(),
            path.file_name().unwrap_or_default().to_string_lossy()
        ))
    } else {
        version.clone()
    };
    let mut meta = docmeta::load(conn, doc_id)?;
    meta.hash = new_hash.clone();
    let now = vf_core::now_rfc3339();
    let (meta_file, meta_line) = docmeta::record_line(root, &meta, &now, device_id);

    let mut tx = Transaction::new(change.label)
        .file(FileOp::Move {
            src: path.clone(),
            dst: kept_at.clone(),
        })
        .file(FileOp::Move {
            src: new_file.to_path_buf(),
            dst: path.clone(),
        })
        .file(FileOp::AppendLines {
            path: meta_file,
            lines: vec![meta_line],
            undo_lines: vec![],
        })
        .catalog(Stmt::new(
            "UPDATE document SET hash = ?2, size = ?3, rev = rev + 1, updated_at = ?4,
             device_id = ?5 WHERE id = ?1",
            vec![
                doc_id.into(),
                new_hash.clone().into(),
                new_size.into(),
                now.clone().into(),
                device_id.into(),
            ],
        ))
        .catalog_undo(Stmt::new(
            "UPDATE document SET hash = ?2, size = ?3, rev = rev + 1 WHERE id = ?1",
            vec![doc_id.into(), old_hash.clone().into(), old_size.into()],
        ))
        .catalog(Stmt::new(
            "INSERT OR IGNORE INTO version (doc_id, hash, size, created, created_by, device_id, reason)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            vec![
                doc_id.into(),
                old_hash.clone().into(),
                old_size.into(),
                now.clone().into(),
                change.created_by.into(),
                device_id.into(),
                change.reason.into(),
            ],
        ))
        .catalog_undo(Stmt::new(
            "DELETE FROM version WHERE doc_id = ?1 AND hash = ?2 AND created = ?3",
            vec![doc_id.into(), old_hash.clone().into(), now.clone().into()],
        ))
        .catalog(Stmt::new(
            "INSERT INTO sync_pending (doc_id, op, since) VALUES (?1, 'put', ?2)
             ON CONFLICT(doc_id, op) DO NOTHING",
            vec![doc_id.into(), now.clone().into()],
        ));
    let sub = |s: Stmt| {
        let params = s
            .params
            .into_iter()
            .map(|p| match p.as_str() {
                Some("?OLD") => old_hash.clone().into(),
                Some("?NEW") => new_hash.clone().into(),
                _ => p,
            })
            .collect();
        Stmt::new(&s.sql, params)
    };
    for (s, undo) in change.extra {
        tx = tx.catalog(sub(s));
        if let Some(u) = undo {
            tx = tx.catalog_undo(sub(u));
        }
    }
    let mut detail = json!({ "old_hash": old_hash, "new_hash": new_hash });
    if let (Some(d), serde_json::Value::Object(extra)) = (detail.as_object_mut(), change.detail) {
        d.extend(extra);
    }
    tx = tx.audit(AuditEntry::new(change.label, Some(doc_id), detail));
    for (action, detail) in also {
        tx = tx.audit(AuditEntry::new(&action, Some(doc_id), detail));
    }
    let c = crate::fileops::commit(conn, root, tx, device_id)?;
    // Every version row has its sidecar entry (rebuild restores rows from them), also when
    // the content was already kept.
    {
        let _ = cabinet::add_version_sidecar(
            root,
            &old_hash,
            cabinet::VersionSidecar {
                rel_path: rel.replace('\\', "/"),
                created: now,
                created_by: Some(change.created_by.into()),
                device_id: device_id.into(),
                reason: Some(change.reason.into()),
                next: Some(new_hash.clone()),
            },
        );
    }
    Ok(Ok(Replaced {
        tx_id: c.tx_id,
        old_hash,
        new_hash,
    }))
}

/// A new document's metadata, for [`add`].
#[derive(Debug, Clone, Default)]
pub struct NewDoc {
    /// Cabinet-relative path wanted; ` (2)` etc. is added when it is taken.
    pub rel_path: String,
    pub client_id: Option<String>,
    pub doc_type_id: Option<String>,
    pub tax_year: Option<i32>,
    pub source: Option<String>,
    pub tags: Vec<String>,
    /// Transaction and audit action (`save_as`, `redact`, `extract_pages`).
    pub label: String,
    pub detail: serde_json::Value,
}

/// What [`add`] made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Added {
    pub doc_id: String,
    pub rel_path: String,
    pub tx_id: String,
}

/// Files `file` (a file in [`cabinet::tmp_dir`], consumed on success, removed on failure) as a
/// new document.
pub fn add(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    file: &Path,
    d: NewDoc,
) -> Result<Added, String> {
    add_also(conn, root, device_id, file, d, vec![])
}

/// [`add`] with further audit rows on the new document in the same Transaction.
pub fn add_also(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    file: &Path,
    d: NewDoc,
    also: AlsoAudit,
) -> Result<Added, String> {
    let r = add_inner(conn, root, device_id, file, d, also);
    if r.is_err() {
        let _ = std::fs::remove_file(file);
    }
    r
}

fn add_inner(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    file: &Path,
    d: NewDoc,
    also: AlsoAudit,
) -> Result<Added, String> {
    let rel = free_rel(root, &d.rel_path);
    let hash = filer_intake::commit::sha256_file(file).map_err(|e| e.to_string())?;
    let size = std::fs::metadata(file).map_err(|e| e.to_string())?.len() as i64;
    let id = vf_core::new_id();
    let now = vf_core::now_rfc3339();
    let (folder_rel, doc_type_name): (Option<String>, Option<String>) = (
        d.client_id.as_ref().and_then(|c| {
            conn.query_row("SELECT folder_rel FROM client WHERE id = ?1", [c], |r| {
                r.get(0)
            })
            .ok()
            .flatten()
        }),
        d.doc_type_id.as_ref().and_then(|t| {
            conn.query_row("SELECT name FROM doc_type WHERE id = ?1", [t], |r| r.get(0))
                .ok()
        }),
    );
    let meta = Meta {
        id: id.clone(),
        rel_path: rel.clone(),
        hash: hash.clone(),
        folder_rel,
        doc_type: d.doc_type_id.clone(),
        doc_type_name,
        tax_year: d.tax_year,
        status: "received".into(),
        source: d.source.clone(),
        tags: d.tags.clone(),
    };
    let (meta_file, meta_line) = docmeta::record_line(root, &meta, &now, device_id);
    let mime = rel
        .rsplit('.')
        .next()
        .filter(|e| e.eq_ignore_ascii_case("pdf"))
        .map(|_| "application/pdf");
    let mut tx = Transaction::new(&d.label)
        .file(FileOp::Move {
            src: file.to_path_buf(),
            dst: root.join(&rel),
        })
        .file(FileOp::AppendLines {
            path: meta_file,
            lines: vec![meta_line],
            undo_lines: vec![],
        })
        .catalog(Stmt::new(
            "INSERT INTO document (id, client_id, doc_type_id, tax_year, rel_path, hash, size, mime,
                                   source, status, filed_by, filed_at, device_id, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'received', ?10, ?11, ?12, ?11)",
            vec![
                id.clone().into(),
                d.client_id.clone().into(),
                d.doc_type_id.clone().into(),
                d.tax_year.into(),
                rel.clone().into(),
                hash.clone().into(),
                size.into(),
                mime.into(),
                d.source.clone().into(),
                d.label.clone().into(),
                now.clone().into(),
                device_id.into(),
            ],
        ))
        .catalog_undo(Stmt::new(
            "UPDATE document SET deleted_at = ?2, tombstone_reason = 'undone' WHERE id = ?1",
            vec![id.clone().into(), now.clone().into()],
        ))
        .catalog(Stmt::new(
            "INSERT INTO sync_pending (doc_id, op, since) VALUES (?1, 'put', ?2)
             ON CONFLICT(doc_id, op) DO NOTHING",
            vec![id.clone().into(), now.clone().into()],
        ));
    for t in &d.tags {
        tx = tx.catalog(Stmt::new(
            "INSERT OR IGNORE INTO document_tag (doc_id, tag) VALUES (?1, ?2)",
            vec![id.clone().into(), t.clone().into()],
        ));
    }
    let mut detail = json!({ "rel_path": rel, "hash": hash });
    if let (Some(o), serde_json::Value::Object(extra)) = (detail.as_object_mut(), d.detail) {
        o.extend(extra);
    }
    tx = tx.audit(AuditEntry::new(&d.label, Some(&id), detail));
    for (action, detail) in also {
        tx = tx.audit(AuditEntry::new(&action, Some(&id), detail));
    }
    let c = crate::fileops::commit(conn, root, tx, device_id)?;
    Ok(Added {
        doc_id: id,
        rel_path: rel,
        tx_id: c.tx_id,
    })
}

/// `rel`, or `name (2).ext`, `name (3).ext`, … when a file is already there.
fn free_rel(root: &Path, rel: &str) -> String {
    if !root.join(rel).exists() {
        return rel.to_string();
    }
    let (dir, name) = rel.rsplit_once('/').unwrap_or(("", rel));
    let (stem, ext) = name
        .rsplit_once('.')
        .map(|(s, e)| (s.to_string(), format!(".{e}")))
        .unwrap_or((name.to_string(), String::new()));
    (2..)
        .map(|n| {
            let f = format!("{stem} ({n}){ext}");
            if dir.is_empty() {
                f
            } else {
                format!("{dir}/{f}")
            }
        })
        .find(|r| !root.join(r).exists())
        .expect("a free name")
}

#[cfg(test)]
mod tests {
    use vf_core::db::Storage;

    use super::*;

    fn fx() -> (tempfile::TempDir, PathBuf, Connection) {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("cab");
        std::fs::create_dir_all(root.join("A (1)/2025")).unwrap();
        let conn = vf_core::db::open(&t.path().join("c.db"), Storage::Local).unwrap();
        conn.execute(
            "INSERT INTO client (id, ext_id, name, folder_rel, device_id, updated_at)
             VALUES ('c1', '1', 'A', 'A (1)', 'dev', 't')",
            [],
        )
        .unwrap();
        std::fs::write(root.join("A (1)/2025/doc.pdf"), b"old bytes").unwrap();
        let hash = filer_intake::commit::sha256_file(&root.join("A (1)/2025/doc.pdf")).unwrap();
        conn.execute(
            "INSERT INTO document (id, client_id, tax_year, rel_path, hash, size, filed_at, device_id, updated_at)
             VALUES ('d1', 'c1', 2025, 'A (1)/2025/doc.pdf', ?1, 9, 't', 'dev', 't')",
            [&hash],
        )
        .unwrap();
        (t, root, conn)
    }

    fn change() -> Change<'static> {
        Change {
            label: "annotate",
            created_by: "annotate",
            reason: "before annotations",
            detail: json!({ "annotations": 2 }),
            extra: vec![],
            based_on: None,
        }
    }

    /// QA A11: an edit made from older content is refused, and nothing changes.
    #[test]
    fn replace_based_on_older_content_is_refused() {
        let (_t, root, mut conn) = fx();
        let tmp = tmp_file(&root, "pdf").unwrap();
        std::fs::write(&tmp, b"new bytes").unwrap();
        let mut c = change();
        c.based_on = Some("0".repeat(64));
        let r = replace(&mut conn, &root, "dev", "d1", &tmp, c).unwrap();
        assert_eq!(r, Err(Refused::Changed));
        assert_eq!(
            std::fs::read(root.join("A (1)/2025/doc.pdf")).unwrap(),
            b"old bytes"
        );
        assert!(!tmp.exists());
    }

    #[test]
    fn replace_keeps_the_old_bytes_as_a_version_and_undo_restores() {
        let (_t, root, mut conn) = fx();
        let tmp = tmp_file(&root, "pdf").unwrap();
        std::fs::write(&tmp, b"new bytes").unwrap();
        let r = replace(&mut conn, &root, "dev", "d1", &tmp, change())
            .unwrap()
            .unwrap();
        let path = root.join("A (1)/2025/doc.pdf");
        assert_eq!(std::fs::read(&path).unwrap(), b"new bytes");
        assert_eq!(
            std::fs::read(cabinet::version_path(&root, &r.old_hash)).unwrap(),
            b"old bytes"
        );
        assert!(
            cabinet::version_path(&root, &r.old_hash)
                .with_extension("json")
                .is_file()
        );
        let (hash, by): (String, String) = conn
            .query_row(
                "SELECT d.hash, v.created_by FROM document d JOIN version v ON v.doc_id = d.id
                 WHERE d.id = 'd1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (hash.as_str(), by.as_str()),
            (r.new_hash.as_str(), "annotate")
        );
        let detail: String = conn
            .query_row(
                "SELECT detail FROM audit WHERE action = 'annotate'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(detail.contains("\"annotations\":2"), "{detail}");
        assert!(!tmp.exists());

        crate::fileops::undo(&mut conn, &root, &r.tx_id, "dev").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"old bytes");
    }

    #[test]
    fn replace_refuses_locked_and_changed_files() {
        let (_t, root, mut conn) = fx();
        conn.execute(
            "INSERT INTO lock (doc_id, user, device_id, since) VALUES ('d1', 'u', 'dev', 't')",
            [],
        )
        .unwrap();
        let tmp = tmp_file(&root, "pdf").unwrap();
        std::fs::write(&tmp, b"x").unwrap();
        assert_eq!(
            replace(&mut conn, &root, "dev", "d1", &tmp, change()).unwrap(),
            Err(Refused::Locked)
        );
        assert!(!tmp.exists(), "temp removed");
        conn.execute("DELETE FROM lock", []).unwrap();
        std::fs::write(root.join("A (1)/2025/doc.pdf"), b"edited in Explorer").unwrap();
        let tmp = tmp_file(&root, "pdf").unwrap();
        std::fs::write(&tmp, b"x").unwrap();
        assert_eq!(
            replace(&mut conn, &root, "dev", "d1", &tmp, change()).unwrap(),
            Err(Refused::Changed)
        );
    }

    #[test]
    fn add_files_a_new_document_beside_an_existing_name() {
        let (_t, root, mut conn) = fx();
        let tmp = tmp_file(&root, "pdf").unwrap();
        std::fs::write(&tmp, b"copy").unwrap();
        let a = add(
            &mut conn,
            &root,
            "dev",
            &tmp,
            NewDoc {
                rel_path: "A (1)/2025/doc.pdf".into(),
                client_id: Some("c1".into()),
                tax_year: Some(2025),
                tags: vec!["copy-of:d1".into()],
                label: "save_as".into(),
                ..NewDoc::default()
            },
        )
        .unwrap();
        assert_eq!(a.rel_path, "A (1)/2025/doc (2).pdf");
        assert_eq!(std::fs::read(root.join(&a.rel_path)).unwrap(), b"copy");
        let (client, tag): (String, String) = conn
            .query_row(
                "SELECT d.client_id, t.tag FROM document d JOIN document_tag t ON t.doc_id = d.id
                 WHERE d.id = ?1",
                [&a.doc_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((client.as_str(), tag.as_str()), ("c1", "copy-of:d1"));
        let line = std::fs::read_to_string(root.join("A (1)/_Vibe/docmeta.jsonl")).unwrap();
        assert!(
            line.contains("doc (2).pdf") && line.contains("copy-of:d1"),
            "{line}"
        );
    }
}
