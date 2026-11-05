//! Move or copy documents to another folder, another client, or another Vibe Files cabinet
//! (owner 2026-11-05, Q-74). One Transaction, so Undo puts everything back.
//!
//! - Within this cabinet: a move changes the document's path and client; a copy is a new
//!   document with the original's doc type, year, status, source and tags (its own history
//!   from then on).
//! - Into another cabinet: the file goes there with a `docmeta.jsonl` record, so that
//!   cabinet's catalog gives it the same metadata when it finds it; here a move marks the
//!   document moved away.
//!
//! Files move with Filer's engine (never replacing a file: a name taken keeps both, ` (2)`).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use filer_core::template;
use serde_json::json;
use vf_core::audit::AuditEntry;
use vf_core::db::Connection;
use vf_core::tx::{FileOp, Stmt, Transaction};

use crate::docmeta;
use crate::rename::{Outcome, Row};

pub const MOVE_LABEL: &str = "doc_move_to";
pub const COPY_LABEL: &str = "doc_copy";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Move,
    Copy,
}

/// Where documents go: a folder (relative to `root`, `/`-separated) inside a client folder of
/// the cabinet at `root` (this one or another).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dest {
    pub root: PathBuf,
    pub folder: String,
}

/// The destination checked: its client folder (top-level folder) and, in this cabinet, the
/// client's catalog id.
#[derive(Debug, Clone)]
struct Target {
    other_cabinet: bool,
    client_folder: String,
    client_id: Option<String>,
}

fn same_place(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        std::fs::canonicalize(p)
            .unwrap_or_else(|_| p.to_path_buf())
            .to_string_lossy()
            .to_lowercase()
    };
    norm(a) == norm(b)
}

fn target(conn: &Connection, root: &Path, dest: &Dest) -> Result<Target, String> {
    let folder = dest.folder.trim_matches('/');
    let Some(top) = folder.split('/').next().filter(|t| !t.is_empty()) else {
        return Err("Pick a folder inside a client folder.".into());
    };
    if top.eq_ignore_ascii_case(crate::cabinet::VIBE_DIR) {
        return Err("Not into _Vibe.".into());
    }
    crate::cabinet::read(&dest.root)
        .map_err(|_| format!("{} is not a Vibe Files cabinet.", dest.root.display()))?;
    let path = template::checked_join(&dest.root, Path::new(folder)).map_err(|e| e.to_string())?;
    if !path.is_dir() {
        return Err(format!("The folder {folder} is not on disk."));
    }
    let other_cabinet = !same_place(root, &dest.root);
    let client_id = if other_cabinet {
        None
    } else {
        vf_core::db::OptionalExtension::optional(conn.query_row(
            "SELECT id FROM client WHERE folder_rel = ?1 COLLATE NOCASE AND deleted_at IS NULL",
            [top],
            |r| r.get::<_, String>(0),
        ))
        .map_err(|e| e.to_string())?
    };
    let bound = filer_intake::sentinel::sentinel_path(&dest.root.join(top)).is_file();
    if !bound && client_id.is_none() {
        return Err(format!("{top} is not a client folder."));
    }
    Ok(Target {
        other_cabinet,
        client_folder: top.to_string(),
        client_id,
    })
}

/// What moving or copying `ids` into `dest` would do.
pub fn plan(
    conn: &Connection,
    root: &Path,
    ids: &[String],
    dest: &Dest,
    mode: Mode,
) -> Result<Vec<Row>, String> {
    let t = target(conn, root, dest)?;
    let folder = dest.folder.trim_matches('/').to_string();
    let dest_path = dest.root.join(&folder);
    let taken_on_disk: HashSet<String> = std::fs::read_dir(&dest_path)
        .map_err(|e| format!("{folder}: {e}"))?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_lowercase())
        .collect();
    let mut st = conn
        .prepare_cached(
            "SELECT d.rel_path, l.user FROM document d LEFT JOIN lock l ON l.doc_id = d.id
             WHERE d.id = ?1 AND d.deleted_at IS NULL",
        )
        .map_err(|e| e.to_string())?;
    let mut claimed: HashSet<String> = HashSet::new();
    let mut rows = Vec::with_capacity(ids.len());
    for id in ids {
        let (rel, locked_by): (String, Option<String>) = st
            .query_row([id], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| format!("document {id}: {e}"))?;
        let (dir, name) = rel.rsplit_once('/').unwrap_or(("", &rel));
        let outcome = if let Some(u) = locked_by {
            Outcome::Skip(format!("checked out by {u}"))
        } else if !t.other_cabinet && dir.eq_ignore_ascii_case(&folder) && mode == Mode::Move {
            Outcome::Same
        } else {
            let taken = |n: &str| {
                let n = n.to_lowercase();
                taken_on_disk.contains(&n) || claimed.contains(&n)
            };
            let free = crate::rename::free_name(name, taken);
            let new_rel = format!("{folder}/{free}");
            match template::checked_join(&dest.root, Path::new(&new_rel)) {
                Err(e) => Outcome::Skip(e.to_string()),
                Ok(_) => {
                    claimed.insert(free.to_lowercase());
                    if free == name {
                        Outcome::Rename(new_rel)
                    } else {
                        Outcome::KeepBoth(new_rel)
                    }
                }
            }
        };
        rows.push(Row {
            doc_id: id.clone(),
            old_rel: rel,
            outcome,
        });
    }
    Ok(rows)
}

/// Moves or copies the rows that go, in one Transaction. Returns (transaction id, documents).
pub fn commit(
    conn: &mut Connection,
    root: &Path,
    rows: &[Row],
    dest: &Dest,
    mode: Mode,
    device_id: &str,
) -> Result<(String, usize), String> {
    let t = target(conn, root, dest)?;
    let go: Vec<&Row> = rows.iter().filter(|r| r.renames()).collect();
    if go.is_empty() {
        return Err(match mode {
            Mode::Move => "Nothing to move: the documents are in that folder already.".into(),
            Mode::Copy => "Nothing to copy.".into(),
        });
    }
    let label = match mode {
        Mode::Move => MOVE_LABEL,
        Mode::Copy => COPY_LABEL,
    };
    let now = vf_core::now_rfc3339();
    let mut tx = Transaction::new(label);
    // docmeta lines by file: (lines, undo lines).
    let mut meta_lines: BTreeMap<PathBuf, (Vec<String>, Vec<String>)> = BTreeMap::new();
    let mut new_ids: Vec<String> = vec![];
    for r in &go {
        let new_rel = r.new_rel().expect("filtered to rows that go").to_owned();
        let m = docmeta::load(conn, &r.doc_id)?;
        let src = root.join(&r.old_rel);
        let dst = dest.root.join(&new_rel);
        tx = tx.file(match mode {
            Mode::Move => FileOp::Move { src, dst },
            Mode::Copy => FileOp::Copy { src, dst },
        });
        // The metadata travels in the destination folder's docmeta.jsonl.
        let there = docmeta::Meta {
            rel_path: new_rel.clone(),
            folder_rel: Some(t.client_folder.clone()),
            ..m.clone()
        };
        let (file, line) = docmeta::record_line(&dest.root, &there, &now, device_id);
        let e = meta_lines.entry(file).or_default();
        e.0.push(line);
        match (mode, t.other_cabinet) {
            (Mode::Move, false) => {
                tx = tx
                    .catalog(Stmt::new(
                        "UPDATE document SET rel_path = ?2, client_id = ?3, device_id = ?4,
                           updated_at = ?5, rev = rev + 1
                         WHERE id = ?1 AND deleted_at IS NULL",
                        vec![
                            r.doc_id.clone().into(),
                            new_rel.clone().into(),
                            t.client_id.clone().into(),
                            device_id.into(),
                            now.clone().into(),
                        ],
                    ))
                    .catalog_undo(Stmt::new(
                        "UPDATE document SET rel_path = ?2, device_id = ?3, updated_at = ?4,
                           rev = rev + 1
                         WHERE id = ?1",
                        vec![
                            r.doc_id.clone().into(),
                            r.old_rel.clone().into(),
                            device_id.into(),
                            now.clone().into(),
                        ],
                    ));
                // The old client back on undo.
                let old_client: Option<String> = conn
                    .query_row(
                        "SELECT client_id FROM document WHERE id = ?1",
                        [&r.doc_id],
                        |x| x.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                tx = tx.catalog_undo(Stmt::new(
                    "UPDATE document SET client_id = ?2 WHERE id = ?1",
                    vec![r.doc_id.clone().into(), old_client.into()],
                ));
            }
            (Mode::Copy, false) => {
                let id = vf_core::new_id();
                tx = tx
                    .catalog(Stmt::new(
                        "INSERT INTO document (id, client_id, doc_type_id, tax_year, rel_path, hash,
                           size, mime, status, source, filed_by, filed_at, device_id, updated_at)
                         SELECT ?2, ?3, doc_type_id, tax_year, ?4, hash, size, mime, status,
                           source, 'copy', ?5, ?6, ?5
                         FROM document WHERE id = ?1",
                        vec![
                            r.doc_id.clone().into(),
                            id.clone().into(),
                            t.client_id.clone().into(),
                            new_rel.clone().into(),
                            now.clone().into(),
                            device_id.into(),
                        ],
                    ))
                    .catalog(Stmt::new(
                        "INSERT OR IGNORE INTO document_tag (doc_id, tag)
                         SELECT ?2, tag FROM document_tag WHERE doc_id = ?1",
                        vec![r.doc_id.clone().into(), id.clone().into()],
                    ))
                    .catalog(Stmt::new(
                        "INSERT INTO sync_pending (doc_id, op, since) VALUES (?1, 'put', ?2)
                         ON CONFLICT(doc_id, op) DO NOTHING",
                        vec![id.clone().into(), now.clone().into()],
                    ))
                    .catalog_undo(Stmt::new(
                        "UPDATE document SET deleted_at = ?2, tombstone_reason = 'undone',
                           rev = rev + 1 WHERE id = ?1",
                        vec![id.clone().into(), now.clone().into()],
                    ));
                new_ids.push(id);
            }
            (Mode::Move, true) => {
                tx = tx
                    .catalog(Stmt::new(
                        "UPDATE document SET deleted_at = ?2,
                           tombstone_reason = 'moved to another cabinet', device_id = ?3,
                           updated_at = ?2, rev = rev + 1
                         WHERE id = ?1 AND deleted_at IS NULL",
                        vec![
                            r.doc_id.clone().into(),
                            now.clone().into(),
                            device_id.into(),
                        ],
                    ))
                    .catalog_undo(Stmt::new(
                        "UPDATE document SET deleted_at = NULL, tombstone_reason = NULL,
                           rev = rev + 1 WHERE id = ?1",
                        vec![r.doc_id.clone().into()],
                    ))
                    .catalog(Stmt::new(
                        "INSERT INTO sync_pending (doc_id, op, since) VALUES (?1, 'delete', ?2)
                         ON CONFLICT(doc_id, op) DO NOTHING",
                        vec![r.doc_id.clone().into(), now.clone().into()],
                    ))
                    .catalog_undo(Stmt::new(
                        "DELETE FROM sync_pending WHERE doc_id = ?1 AND op = 'delete'",
                        vec![r.doc_id.clone().into()],
                    ));
            }
            (Mode::Copy, true) => {}
        }
    }
    for (path, (lines, undo_lines)) in meta_lines {
        tx = tx.file(FileOp::AppendLines {
            path,
            lines,
            undo_lines,
        });
    }
    tx = tx.audit(AuditEntry::new(
        label,
        if go.len() == 1 {
            Some(go[0].doc_id.as_str())
        } else {
            None
        },
        json!({
            "documents": go.len(),
            "to_cabinet": if t.other_cabinet { Some(dest.root.display().to_string()) } else { None },
            "to_folder": dest.folder,
            "copies": new_ids,
        }),
    ));
    let allowed: Vec<&Path> = if t.other_cabinet {
        vec![root, dest.root.as_path()]
    } else {
        vec![root]
    };
    let c = crate::fileops::commit_in(conn, &allowed, tx, device_id)?;
    Ok((c.tx_id, go.len()))
}

/// The client folders of the cabinet at `root` (bound by a sentinel), by folder name: for
/// picking a destination in another cabinet.
pub fn client_folders(root: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(root) else {
        return vec![];
    };
    let mut out: Vec<String> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.eq_ignore_ascii_case(crate::cabinet::VIBE_DIR))
        .filter(|n| filer_intake::sentinel::sentinel_path(&root.join(n)).is_file())
        .collect();
    out.sort_by_key(|n| n.to_lowercase());
    out
}

/// Every folder inside `client_folder` of the cabinet at `root` (relative to the root,
/// `/`-separated, `_Vibe` left out), the client folder itself first: the destinations offered.
pub fn folders_in(root: &Path, client_folder: &str) -> Vec<String> {
    fn walk(dir: &Path, rel: &str, out: &mut Vec<String>, depth: usize) {
        if depth > 8 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut names: Vec<String> = rd
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.eq_ignore_ascii_case(crate::cabinet::VIBE_DIR))
            .collect();
        names.sort_by_key(|n| n.to_lowercase());
        for n in names {
            let r = format!("{rel}/{n}");
            out.push(r.clone());
            walk(&dir.join(&n), &r, out, depth + 1);
        }
    }
    let mut out = vec![client_folder.to_string()];
    walk(&root.join(client_folder), client_folder, &mut out, 0);
    out
}

#[cfg(test)]
mod tests;
