//! Searchable-PDF write-back (P03 step 3): a document whose pages were read by OCR gets the
//! invisible text layer ([`crate::layer`]) written into the file itself, so the text travels
//! with the PDF (Explorer, email, any PDF app, rebuild).
//!
//! One Transaction: the original moves to `_Vibe/versions/` (it had no text layer: Q-17
//! keeps it), the new file takes its place, the docmeta line follows the new hash, and the
//! catalog row, version row, `sync_pending`, OCR job and audit row change together.
//!
//! Deferred while the file is part of Filer's current undoable batch: Filer's Undo refuses a
//! filed file whose size changed, and "Undo last batch" must stay available. A newer batch
//! (or the undo itself) releases it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::json;
use vf_core::audit::AuditEntry;
use vf_core::db::{Connection, OptionalExtension, params};
use vf_core::tx::{FileOp, Stmt, Transaction};

use crate::engine::OcrPage;
use crate::layer::{self, PageWords};

pub const LABEL: &str = "ocr_text_layer";

/// Why a document was not written now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Held {
    /// In Filer's current undoable batch.
    FilerUndo,
    /// Checked out.
    Locked,
    /// The file on disk is not the content that was read.
    Changed,
}

/// Paths Filer's "Undo last batch" would check (see `vf_fs::intake::undoable_paths`).
pub fn filer_undoable(root: &Path) -> HashSet<String> {
    vf_fs::intake::undoable_paths(root)
}

/// Documents read by OCR and waiting for their text layer.
/// (document id, cabinet path).
pub fn pending(conn: &Connection) -> vf_core::Result<Vec<(String, String)>> {
    let mut st = conn.prepare(
        "SELECT j.doc_id, d.rel_path FROM ocr_job j
         JOIN document d ON d.id = j.doc_id AND d.deleted_at IS NULL
         WHERE j.state = 'indexed' ORDER BY j.priority, j.queued_at",
    )?;
    let v = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(v)
}

/// What [`write`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Written {
    Done,
    Held(Held),
}

/// Writes the text layer into `doc_id`'s file. `undoable`: from [`filer_undoable`].
pub fn write(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    doc_id: &str,
    undoable: &HashSet<String>,
) -> Result<Written, String> {
    let row: Option<(String, String, i64)> = conn
        .query_row(
            "SELECT d.rel_path, j.hash, d.size FROM ocr_job j JOIN document d ON d.id = j.doc_id
             WHERE j.doc_id = ?1 AND j.state = 'indexed' AND d.deleted_at IS NULL
               AND d.hash = j.hash",
            [doc_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((rel_path, old_hash, old_size)) = row else {
        return Err("not waiting for a text layer".into());
    };
    if undoable.contains(&rel_path.to_lowercase()) {
        return Ok(Written::Held(Held::FilerUndo));
    }
    let locked: bool = conn
        .query_row("SELECT 1 FROM lock WHERE doc_id = ?1", [doc_id], |_| {
            Ok(true)
        })
        .optional()
        .map_err(|e| e.to_string())?
        .is_some();
    if locked {
        return Ok(Written::Held(Held::Locked));
    }
    let path = root.join(&rel_path);
    let on_disk = filer_intake::commit::sha256_file(&path).map_err(|e| e.to_string())?;
    if on_disk != old_hash {
        // Edited outside Vibe Files: nothing to layer until the file is adopted again (P05's
        // integrity sweep updates the row, and the new content is queued then).
        finish(
            conn,
            doc_id,
            "file changed outside Vibe Files; text layer not written",
        );
        return Ok(Written::Held(Held::Changed));
    }

    // The OCR'd pages and their words.
    let ocr: Vec<(u32, OcrPage)> = {
        let mut st = conn
            .prepare(
                "SELECT page, words FROM page_text WHERE hash = ?1 AND source = 'ocr'
                 AND words IS NOT NULL ORDER BY page",
            )
            .map_err(|e| e.to_string())?;
        st.query_map([&old_hash], |r| {
            Ok((r.get::<_, u32>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .filter_map(|(p, w)| serde_json::from_str(&w).ok().map(|o| (p, o)))
        .collect()
    };
    let words: Vec<PageWords> = ocr
        .iter()
        .map(|(page, ocr)| PageWords { page: *page, ocr })
        .collect();

    // The new file, next to the cabinet's working files.
    let tmp_dir = vf_fs::cabinet::tmp_dir(root);
    std::fs::create_dir_all(&tmp_dir)
        .map_err(|e| format!("cannot create {}: {e}", tmp_dir.display()))?;
    let tmp = tmp_dir.join(format!("vf-ocr-{}.pdf", uuid::Uuid::now_v7().simple()));
    if let Err(e) = layer::add_text_layer(&path, &tmp, &words) {
        let _ = std::fs::remove_file(&tmp);
        // The same PDF fails the same way next time (password, damaged): stop retrying. The
        // document stays searchable from the catalog.
        finish(conn, doc_id, &format!("text layer not written: {e}"));
        return Err(e);
    }
    let new_hash = filer_intake::commit::sha256_file(&tmp).map_err(|e| e.to_string())?;
    let new_size = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0) as i64;

    // The original is kept as a version; if those bytes are already kept, it goes to trash.
    let version = vf_fs::cabinet::version_path(root, &old_hash);
    let kept_at: PathBuf = if version.exists() {
        vf_fs::cabinet::trash_dir(root).join(format!(
            "ocr-{}-{}",
            uuid::Uuid::now_v7().simple(),
            file_name(&path)
        ))
    } else {
        version.clone()
    };

    let mut meta = vf_fs::docmeta::load(conn, doc_id)?;
    meta.hash = new_hash.clone();
    let now = vf_core::now_rfc3339();
    let (meta_file, meta_line) = vf_fs::docmeta::record_line(root, &meta, &now, device_id);

    let tx = Transaction::new(LABEL)
        .file(FileOp::Move {
            src: path.clone(),
            dst: kept_at.clone(),
        })
        .file(FileOp::Move {
            src: tmp.clone(),
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
             VALUES (?1, ?2, ?3, ?4, ?6, ?5, 'before OCR text layer')",
            vec![
                doc_id.into(),
                old_hash.clone().into(),
                old_size.into(),
                now.clone().into(),
                device_id.into(),
                vf_fs::cabinet::TEXT_LAYER_VERSION_BY.into(),
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
        ))
        .catalog(Stmt::new(
            "INSERT OR IGNORE INTO page_text (hash, page, source, text, words, dpi)
             SELECT ?2, page, source, text, words, dpi FROM page_text WHERE hash = ?1",
            vec![old_hash.clone().into(), new_hash.clone().into()],
        ))
        .catalog(Stmt::new(
            "UPDATE ocr_job SET hash = ?2, state = 'done', error = NULL, updated_at = ?3
             WHERE doc_id = ?1",
            vec![doc_id.into(), new_hash.clone().into(), now.clone().into()],
        ))
        .catalog_undo(Stmt::new(
            "UPDATE ocr_job SET hash = ?2, state = 'indexed' WHERE doc_id = ?1",
            vec![doc_id.into(), old_hash.clone().into()],
        ))
        .audit(AuditEntry::new(
            LABEL,
            Some(doc_id),
            json!({
                "pages": words.len(),
                "old_hash": old_hash,
                "new_hash": new_hash,
            }),
        ));
    if let Err(e) = vf_fs::fileops::commit(conn, root, tx, device_id) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if kept_at == version {
        let _ = vf_fs::cabinet::add_version_sidecar(
            root,
            &old_hash,
            vf_fs::cabinet::VersionSidecar {
                rel_path: rel_path.replace('\\', "/"),
                created: now,
                created_by: Some(vf_fs::cabinet::TEXT_LAYER_VERSION_BY.into()),
                device_id: device_id.into(),
                reason: Some("before OCR text layer".into()),
            },
        );
    }
    Ok(Written::Done)
}

/// Writes every waiting document that can be written now. Returns how many were written and
/// the errors (by path).
pub fn write_ready(conn: &mut Connection, root: &Path, device_id: &str) -> (usize, Vec<String>) {
    let undoable = filer_undoable(root);
    let mut written = 0;
    let mut errors = Vec::new();
    for (doc_id, rel) in pending(conn).unwrap_or_default() {
        match write(conn, root, device_id, &doc_id, &undoable) {
            Ok(Written::Done) => written += 1,
            Ok(Written::Held(_)) => {}
            Err(e) => {
                let _ = conn.execute(
                    "UPDATE ocr_job SET error = ?2 WHERE doc_id = ?1",
                    params![doc_id, format!("text layer: {e}")],
                );
                errors.push(format!("{rel}: {e}"));
            }
        }
    }
    (written, errors)
}

/// Leaves the job `done` with `why`: its text is indexed, its file will not get a layer.
fn finish(conn: &Connection, doc_id: &str, why: &str) {
    let _ = conn.execute(
        "UPDATE ocr_job SET state = 'done', error = ?2, updated_at = ?3 WHERE doc_id = ?1",
        params![doc_id, why, vf_core::now_rfc3339()],
    );
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}
