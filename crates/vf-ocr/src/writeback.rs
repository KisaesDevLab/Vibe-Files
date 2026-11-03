//! Searchable-PDF write-back (P03 step 3): a document whose pages were read by OCR gets the
//! invisible text layer ([`crate::layer`]) written into the file itself, so the text travels
//! with the PDF (Explorer, email, any PDF app, rebuild).
//!
//! One `vf_fs::content::replace` Transaction: the original is kept as a version (it had no text
//! layer: Q-17), the new file takes its place, and the OCR job and page text follow the new hash.
//!
//! Deferred while the file is part of Filer's current undoable batch: Filer's Undo refuses a
//! filed file whose size changed, and "Undo last batch" must stay available. A newer batch
//! (or the undo itself) releases it.

use std::collections::HashSet;
use std::path::Path;

use serde_json::json;
use vf_core::db::{Connection, OptionalExtension, params};
use vf_core::tx::Stmt;
use vf_fs::content::{self, Refused};

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

/// Writes the text layer into `doc_id`'s file, as one `vf_fs::content::replace` Transaction.
/// `undoable` is kept for callers that look it up once per pass; `replace` checks Filer's
/// batch itself.
pub fn write(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    doc_id: &str,
    undoable: &HashSet<String>,
) -> Result<Written, String> {
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT d.rel_path, j.hash FROM ocr_job j JOIN document d ON d.id = j.doc_id
             WHERE j.doc_id = ?1 AND j.state = 'indexed' AND d.deleted_at IS NULL
               AND d.hash = j.hash",
            [doc_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((rel_path, old_hash)) = row else {
        return Err("not waiting for a text layer".into());
    };
    // Cheap early answer for a whole pass; `replace` checks again.
    if undoable.contains(&rel_path.to_lowercase()) {
        return Ok(Written::Held(Held::FilerUndo));
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

    let tmp = content::tmp_file(root, "pdf")?;
    if let Err(e) = layer::add_text_layer(&root.join(&rel_path), &tmp, &words) {
        let _ = std::fs::remove_file(&tmp);
        // The same PDF fails the same way next time (password, damaged): stop retrying. The
        // document stays searchable from the catalog.
        finish(conn, doc_id, &format!("text layer not written: {e}"));
        return Err(e);
    }
    let now = vf_core::now_rfc3339();
    let change = content::Change {
        label: LABEL,
        created_by: vf_fs::cabinet::TEXT_LAYER_VERSION_BY,
        reason: "before OCR text layer",
        detail: json!({ "pages": words.len() }),
        extra: vec![
            // The text read for the old content is the new content's text too.
            (
                Stmt::new(
                    "INSERT OR IGNORE INTO page_text (hash, page, source, text, words, dpi)
                     SELECT ?2, page, source, text, words, dpi FROM page_text WHERE hash = ?1",
                    vec!["?OLD".into(), "?NEW".into()],
                ),
                None,
            ),
            (
                Stmt::new(
                    "UPDATE ocr_job SET hash = ?2, state = 'done', error = NULL, updated_at = ?3
                     WHERE doc_id = ?1",
                    vec![doc_id.into(), "?NEW".into(), now.into()],
                ),
                Some(Stmt::new(
                    "UPDATE ocr_job SET hash = ?2, state = 'indexed' WHERE doc_id = ?1",
                    vec![doc_id.into(), "?OLD".into()],
                )),
            ),
        ],
    };
    match content::replace(conn, root, device_id, doc_id, &tmp, change)? {
        Ok(_) => Ok(Written::Done),
        Err(Refused::FilerUndo) => Ok(Written::Held(Held::FilerUndo)),
        Err(Refused::Locked) => Ok(Written::Held(Held::Locked)),
        Err(Refused::Changed) => {
            // Edited outside Vibe Files: nothing to layer until the file is adopted again (P05's
            // integrity sweep updates the row, and the new content is queued then).
            finish(
                conn,
                doc_id,
                "file changed outside Vibe Files; text layer not written",
            );
            Ok(Written::Held(Held::Changed))
        }
    }
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
