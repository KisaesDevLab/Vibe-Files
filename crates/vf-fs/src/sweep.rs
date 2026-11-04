//! Integrity sweep (PHASES P05 step 5): re-hash the client folders against the catalog and
//! report the drift.
//!
//! - **Modified** (a file changed outside Vibe Files, e.g. saved from Acrobat through
//!   Explorer) is adopted on its own (QUESTIONS Q-29): the new content becomes current, the old
//!   hash stays as a version row (its bytes were never seen, so none are kept), audit
//!   `external_modify`.
//! - **Moved** (a missing document's content is at another path) follows the move in one
//!   click.
//! - **Missing** documents are marked deleted in one click.
//! - **Unknown** files (in a client folder, not in the catalog) are adopted as documents, or
//!   sent to Intake to be filed.
//!
//! [`scan`] only reads (the app runs it on a background thread with its own connection); the
//! actions are Transactions.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use filer_intake::commit::sha256_file;
use serde_json::json;
use vf_core::audit::AuditEntry;
use vf_core::db::Connection;
use vf_core::tx::{FileOp, Stmt, Transaction};

use crate::rebuild::{self, RebuildReport};

/// `local_state` key: when the last sweep finished.
pub const LAST_SWEEP: &str = "sweep_at";
/// Days between scheduled sweeps.
pub const EVERY_DAYS: i64 = 7;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Modified {
    pub doc_id: String,
    pub rel_path: String,
    pub old_hash: String,
    pub old_size: i64,
    pub new_hash: String,
    pub new_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Missing {
    pub doc_id: String,
    pub rel_path: String,
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unknown {
    pub rel_path: String,
    pub client_id: Option<String>,
    pub hash: String,
    pub size: u64,
}

/// What a sweep found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Drift {
    /// Files checked.
    pub checked: usize,
    pub modified: Vec<Modified>,
    /// A missing document and the unknown file holding its content.
    pub moved: Vec<(Missing, Unknown)>,
    pub missing: Vec<Missing>,
    pub unknown: Vec<Unknown>,
    /// Files that could not be read (open in another program, permissions).
    pub unreadable: Vec<String>,
}

impl Drift {
    pub fn is_clean(&self) -> bool {
        self.modified.is_empty()
            && self.moved.is_empty()
            && self.missing.is_empty()
            && self.unknown.is_empty()
    }
}

/// Re-hashes every file in the bound client folders and compares with the live documents.
pub fn scan(conn: &Connection, root: &Path) -> Result<Drift, String> {
    let mut rep = RebuildReport::default();
    let (_, folders) = rebuild::scan_folders(root, &mut rep).map_err(|e| e.to_string())?;
    let mut live: HashMap<String, (String, String, i64)> = HashMap::new(); // rel -> id, hash, size
    {
        let mut st = conn
            .prepare("SELECT rel_path, id, hash, size FROM document WHERE deleted_at IS NULL")
            .map_err(|e| e.to_string())?;
        let rows = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .map_err(|e| e.to_string())?;
        for r in rows {
            let (p, id, h, s): (String, String, String, i64) = r.map_err(|e| e.to_string())?;
            live.insert(p, (id, h, s));
        }
    }
    let mut d = Drift::default();
    let mut seen: Vec<String> = Vec::new();
    for (folder, client_id) in &folders {
        let mut paths = Vec::new();
        rebuild::files_under(folder, &mut paths).map_err(|e| e.to_string())?;
        for p in paths {
            let rel = rebuild::rel(root, &p);
            let hash = match sha256_file(&p) {
                Ok(h) => h,
                Err(e) => {
                    d.unreadable.push(format!("{rel}: {e}"));
                    seen.push(rel);
                    continue;
                }
            };
            d.checked += 1;
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            match live.get(&rel) {
                Some((_, h, _)) if *h == hash => {}
                Some((id, h, s)) => d.modified.push(Modified {
                    doc_id: id.clone(),
                    rel_path: rel.clone(),
                    old_hash: h.clone(),
                    old_size: *s,
                    new_hash: hash,
                    new_size: size,
                }),
                None => d.unknown.push(Unknown {
                    rel_path: rel.clone(),
                    client_id: client_id.clone(),
                    hash,
                    size,
                }),
            }
            seen.push(rel);
        }
    }
    let seen: std::collections::HashSet<String> = seen.into_iter().collect();
    let mut missing: Vec<Missing> = live
        .into_iter()
        .filter(|(rel, _)| !seen.contains(rel))
        .map(|(rel, (id, hash, _))| Missing {
            doc_id: id,
            rel_path: rel,
            hash,
        })
        .collect();
    missing.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    // A missing document whose content is now an unknown file was moved outside the app.
    let mut by_hash: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, u) in d.unknown.iter().enumerate() {
        by_hash.entry(u.hash.clone()).or_default().push(i);
    }
    let mut taken = vec![false; d.unknown.len()];
    for m in missing {
        let hit = by_hash
            .get_mut(&m.hash)
            .and_then(|v| v.iter().position(|&i| !taken[i]).map(|k| v[k]));
        match hit {
            Some(i) => {
                taken[i] = true;
                d.moved.push((m, d.unknown[i].clone()));
            }
            None => d.missing.push(m),
        }
    }
    d.unknown = d
        .unknown
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !taken[*i])
        .map(|(_, u)| u)
        .collect();
    Ok(d)
}

/// Makes each modified file's content current (Q-29): version row for the old hash (content
/// not kept), new hash on the document, docmeta line, `sync_pending`, audit `external_modify`.
/// One Transaction; undo puts the catalog back (the file itself stays as edited).
///
/// The sweep's findings can be minutes old: only items whose catalog row still has the old
/// content at the same path, and whose file still has the new content, are adopted (QA A8).
/// Returns the paths adopted.
pub fn adopt_modified(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    items: &[Modified],
) -> Result<Vec<String>, String> {
    let items: Vec<&Modified> = items
        .iter()
        .filter(|m| {
            let row: Option<(String, String)> = conn
                .query_row(
                    "SELECT hash, rel_path FROM document WHERE id = ?1 AND deleted_at IS NULL",
                    [&m.doc_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .ok();
            row.is_some_and(|(h, p)| h == m.old_hash && p == m.rel_path)
                && sha256_file(&root.join(&m.rel_path)).is_ok_and(|h| h == m.new_hash)
        })
        .collect();
    if items.is_empty() {
        return Ok(vec![]);
    }
    let now = vf_core::now_rfc3339();
    let mut tx = Transaction::new("external_modify");
    for m in &items {
        let mut meta = crate::docmeta::load(conn, &m.doc_id)?;
        meta.hash = m.new_hash.clone();
        let (file, line) = crate::docmeta::record_line(root, &meta, &now, device_id);
        tx = tx
            .file(FileOp::AppendLines {
                path: file,
                lines: vec![line],
                undo_lines: vec![],
            })
            .catalog(Stmt::new(
                "INSERT OR IGNORE INTO version (doc_id, hash, size, created, created_by, device_id, reason)
                 VALUES (?1, ?2, ?3, ?4, 'external_modify', ?5, 'changed outside Vibe Files')",
                vec![
                    m.doc_id.clone().into(),
                    m.old_hash.clone().into(),
                    m.old_size.into(),
                    now.clone().into(),
                    device_id.into(),
                ],
            ))
            .catalog_undo(Stmt::new(
                "DELETE FROM version WHERE doc_id = ?1 AND hash = ?2 AND created = ?3",
                vec![
                    m.doc_id.clone().into(),
                    m.old_hash.clone().into(),
                    now.clone().into(),
                ],
            ))
            .catalog(Stmt::new(
                "UPDATE document SET hash = ?2, size = ?3, rev = rev + 1, updated_at = ?4,
                 device_id = ?5 WHERE id = ?1",
                vec![
                    m.doc_id.clone().into(),
                    m.new_hash.clone().into(),
                    (m.new_size as i64).into(),
                    now.clone().into(),
                    device_id.into(),
                ],
            ))
            .catalog_undo(Stmt::new(
                "UPDATE document SET hash = ?2, size = ?3, rev = rev + 1 WHERE id = ?1",
                vec![
                    m.doc_id.clone().into(),
                    m.old_hash.clone().into(),
                    m.old_size.into(),
                ],
            ))
            .catalog(Stmt::new(
                "INSERT INTO sync_pending (doc_id, op, since) VALUES (?1, 'put', ?2)
                 ON CONFLICT(doc_id, op) DO NOTHING",
                vec![m.doc_id.clone().into(), now.clone().into()],
            ))
            .audit(AuditEntry::new(
                "external_modify",
                Some(&m.doc_id),
                json!({ "rel_path": m.rel_path, "old_hash": m.old_hash, "new_hash": m.new_hash }),
            ));
    }
    crate::fileops::commit(conn, root, tx, device_id)?;
    Ok(items.iter().map(|m| m.rel_path.clone()).collect())
}

/// Follows moves made outside the app: each document's path becomes where its content is now.
pub fn follow_moves(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    items: &[(Missing, Unknown)],
) -> Result<usize, String> {
    if items.is_empty() {
        return Ok(0);
    }
    let now = vf_core::now_rfc3339();
    let mut tx = Transaction::new("external_move");
    for (m, u) in items {
        let old_client: Option<String> = conn
            .query_row(
                "SELECT client_id FROM document WHERE id = ?1",
                [&m.doc_id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        tx = tx
            .catalog(Stmt::new(
                "UPDATE document SET rel_path = ?2, client_id = ?3, rev = rev + 1, updated_at = ?4,
                 device_id = ?5 WHERE id = ?1",
                vec![
                    m.doc_id.clone().into(),
                    u.rel_path.clone().into(),
                    u.client_id.clone().into(),
                    now.clone().into(),
                    device_id.into(),
                ],
            ))
            .catalog_undo(Stmt::new(
                "UPDATE document SET rel_path = ?2, client_id = ?3, rev = rev + 1 WHERE id = ?1",
                vec![
                    m.doc_id.clone().into(),
                    m.rel_path.clone().into(),
                    old_client.into(),
                ],
            ))
            .catalog(Stmt::new(
                "INSERT INTO sync_pending (doc_id, op, since) VALUES (?1, 'meta', ?2)
                 ON CONFLICT(doc_id, op) DO NOTHING",
                vec![m.doc_id.clone().into(), now.clone().into()],
            ))
            .audit(AuditEntry::new(
                "external_move",
                Some(&m.doc_id),
                json!({ "from": m.rel_path, "to": u.rel_path }),
            ));
    }
    crate::fileops::commit(conn, root, tx, device_id)?;
    Ok(items.len())
}

/// Marks missing documents deleted (tombstone `missing`).
pub fn mark_missing(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    items: &[Missing],
) -> Result<usize, String> {
    if items.is_empty() {
        return Ok(0);
    }
    let now = vf_core::now_rfc3339();
    let mut tx = Transaction::new("missing");
    for m in items {
        tx = tx
            .catalog(Stmt::new(
                "UPDATE document SET deleted_at = ?2, tombstone_reason = 'missing', rev = rev + 1,
                 updated_at = ?2, device_id = ?3 WHERE id = ?1 AND deleted_at IS NULL",
                vec![
                    m.doc_id.clone().into(),
                    now.clone().into(),
                    device_id.into(),
                ],
            ))
            .catalog_undo(Stmt::new(
                "UPDATE document SET deleted_at = NULL, tombstone_reason = NULL, rev = rev + 1
                 WHERE id = ?1",
                vec![m.doc_id.clone().into()],
            ))
            .catalog(Stmt::new(
                "INSERT INTO sync_pending (doc_id, op, since) VALUES (?1, 'delete', ?2)
                 ON CONFLICT(doc_id, op) DO NOTHING",
                vec![m.doc_id.clone().into(), now.clone().into()],
            ))
            .catalog_undo(Stmt::new(
                "DELETE FROM sync_pending WHERE doc_id = ?1 AND op = 'delete'",
                vec![m.doc_id.clone().into()],
            ))
            .audit(AuditEntry::new(
                "missing",
                Some(&m.doc_id),
                json!({ "rel_path": m.rel_path }),
            ));
    }
    crate::fileops::commit(conn, root, tx, device_id)?;
    Ok(items.len())
}

/// Adds unknown files as documents where they are (tax year from the path; doc type to set).
pub fn adopt_unknown(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    items: &[Unknown],
) -> Result<usize, String> {
    if items.is_empty() {
        return Ok(0);
    }
    let now = vf_core::now_rfc3339();
    let mut tx = Transaction::new("adopt");
    for u in items {
        let id = vf_core::new_id();
        let year = rebuild::year_from_path(&u.rel_path);
        tx = tx
            .catalog(Stmt::new(
                "INSERT INTO document (id, client_id, tax_year, rel_path, hash, size, mime, status,
                   filed_by, filed_at, device_id, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'received', 'sweep', ?8, ?9, ?8)",
                vec![
                    id.clone().into(),
                    u.client_id.clone().into(),
                    year.into(),
                    u.rel_path.clone().into(),
                    u.hash.clone().into(),
                    (u.size as i64).into(),
                    rebuild::mime(&u.rel_path).into(),
                    now.clone().into(),
                    device_id.into(),
                ],
            ))
            .catalog_undo(Stmt::new(
                "UPDATE document SET deleted_at = ?2, tombstone_reason = 'undone', rev = rev + 1
                 WHERE id = ?1",
                vec![id.clone().into(), now.clone().into()],
            ))
            .catalog(Stmt::new(
                "INSERT INTO sync_pending (doc_id, op, since) VALUES (?1, 'put', ?2)
                 ON CONFLICT(doc_id, op) DO NOTHING",
                vec![id.clone().into(), now.clone().into()],
            ))
            .audit(AuditEntry::new(
                "adopt",
                Some(&id),
                json!({ "rel_path": u.rel_path, "hash": u.hash }),
            ));
    }
    crate::fileops::commit(conn, root, tx, device_id)?;
    Ok(items.len())
}

/// Files in the client folders that are not in the catalog: a file copied in through Explorer.
/// Browse looks for them on its folder-listing thread and adds them on its own (PA-23, owner
/// 2026-11-04), so they show without waiting for the weekly sweep. Only reads.
///
/// `clients`: (client folder, client id). `known`: the live documents' paths, lower case.
/// Left out: Office and Explorer working files (`~$…`, `.…`, `desktop.ini`, `Thumbs.db`), and a
/// file with the content of a live document (a move or a copy: the sweep's "moved" and the
/// duplicate finder handle those).
pub fn find_new(
    root: &Path,
    clients: &[(String, String)],
    known: &std::collections::HashSet<String>,
    known_hashes: &std::collections::HashSet<String>,
) -> Vec<Unknown> {
    let mut out = Vec::new();
    for (folder, client_id) in clients {
        let mut paths = Vec::new();
        if rebuild::files_under(&root.join(folder), &mut paths).is_err() {
            continue;
        }
        for p in paths {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if name.starts_with('~')
                || name.starts_with('.')
                || name == "desktop.ini"
                || name == "thumbs.db"
            {
                continue;
            }
            let rel = rebuild::rel(root, &p);
            if known.contains(&rel.to_lowercase()) {
                continue;
            }
            // A file still being copied in cannot be opened for reading: next time.
            let Ok(hash) = sha256_file(&p) else { continue };
            if known_hashes.contains(&hash) {
                continue;
            }
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            out.push(Unknown {
                rel_path: rel,
                client_id: Some(client_id.clone()),
                hash,
                size,
            });
        }
    }
    out
}

/// Adds what [`find_new`] found, after checking again against the catalog now (Intake may have
/// recorded a file meanwhile, or still be filing it) and the disk (same content still there).
/// Returns the paths added.
pub fn adopt_new(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    found: &[Unknown],
) -> Result<Vec<String>, String> {
    // A batch being filed: its files are Intake's; looked at again next time.
    if found.is_empty() || crate::intake::filing_now(root) {
        return Ok(vec![]);
    }
    let pending = crate::intake::unrecorded_paths(conn, root);
    let mut go = Vec::new();
    {
        let mut st = conn
            .prepare_cached(
                "SELECT 1 FROM document WHERE deleted_at IS NULL
                   AND (rel_path = ?1 COLLATE NOCASE OR hash = ?2)",
            )
            .map_err(|e| e.to_string())?;
        for u in found {
            if pending.contains(&u.rel_path.to_lowercase())
                || st
                    .exists((&u.rel_path, &u.hash))
                    .map_err(|e| e.to_string())?
                || sha256_file(&root.join(&u.rel_path)).ok().as_ref() != Some(&u.hash)
            {
                continue;
            }
            go.push(u.clone());
        }
    }
    adopt_unknown(conn, root, device_id, &go)?;
    Ok(go.into_iter().map(|u| u.rel_path).collect())
}

/// Moves unknown files into Filer's `inbox`, to be filed by Intake like any new file.
pub fn refile_unknown(
    conn: &mut Connection,
    root: &Path,
    inbox: &Path,
    device_id: &str,
    items: &[Unknown],
) -> Result<Vec<PathBuf>, String> {
    if items.is_empty() {
        return Ok(vec![]);
    }
    let mut tx = Transaction::new("refile");
    let mut dests = Vec::new();
    for u in items {
        let src = root.join(&u.rel_path);
        let mut dst = inbox.join(src.file_name().ok_or("no file name")?);
        while dst.exists() || dests.contains(&dst) {
            dst = filer_intake::commit::free_path(&dst);
        }
        dests.push(dst.clone());
        tx = tx
            .file(FileOp::Move {
                src,
                dst: dst.clone(),
            })
            .audit(AuditEntry::new(
                "refile",
                None,
                json!({ "rel_path": u.rel_path, "inbox": dst.display().to_string() }),
            ));
    }
    crate::fileops::commit_in(conn, &[root, inbox], tx, device_id)?;
    Ok(dests)
}

/// When the last sweep finished (RFC 3339), if one ever did.
pub fn last_sweep(conn: &Connection) -> Option<String> {
    conn.query_row(
        "SELECT v FROM local_state WHERE k = ?1",
        [LAST_SWEEP],
        |r| r.get(0),
    )
    .ok()
}

pub fn set_last_sweep(conn: &Connection, ts: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO local_state (k, v) VALUES (?1, ?2) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
        [LAST_SWEEP, ts],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// Whether the scheduled sweep is due (never ran, or over [`EVERY_DAYS`] ago).
pub fn due(conn: &Connection) -> bool {
    last_sweep(conn).is_none_or(|t| t < vf_core::rfc3339_days_ago(EVERY_DAYS))
}

#[cfg(test)]
mod tests {
    use vf_core::db::Storage;

    use super::*;

    fn fx() -> (tempfile::TempDir, PathBuf, Connection) {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("cab");
        vf_fs_fixture(&root);
        let mut conn = vf_core::db::open(&t.path().join("c.db"), Storage::Local).unwrap();
        crate::rebuild::rebuild(&mut conn, &root, "dev").unwrap();
        (t, root, conn)
    }

    /// A cabinet with one bound client folder and three files.
    fn vf_fs_fixture(root: &Path) {
        crate::cabinet::create(
            root,
            &crate::cabinet::CabinetFile {
                cabinet_id: vf_core::new_id(),
                firm_id: uuid::Uuid::new_v4().to_string(),
                schema_version: vf_core::db::SCHEMA_VERSION,
                layout_version: 1,
                created: vf_core::now_rfc3339(),
                name: None,
            },
        )
        .unwrap();
        let firm = uuid::Uuid::parse_str(&crate::cabinet::read(root).unwrap().firm_id).unwrap();
        let folder = root.join("Anderson Farms LLC (ANDE1042)");
        std::fs::create_dir_all(folder.join("Income Tax/2025")).unwrap();
        filer_intake::sentinel::write_sentinel(
            root,
            &folder,
            &filer_core::clients::Client {
                id: "ANDE1042".into(),
                uuid: uuid::Uuid::new_v4(),
                name: "Anderson Farms LLC".into(),
                extra: Default::default(),
            },
            firm,
        )
        .unwrap();
        for (name, body) in [("w2.pdf", "w2"), ("k1.pdf", "k1"), ("1099.pdf", "1099")] {
            std::fs::write(folder.join("Income Tax/2025").join(name), body).unwrap();
        }
    }

    fn count(c: &Connection, sql: &str) -> i64 {
        c.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    /// PHASES P05 done-when: a file edited in Explorer is detected by the sweep and adopted as
    /// a new version with audit `external_modify`.
    #[test]
    fn edited_file_is_found_and_adopted_as_a_new_version() {
        let (_t, root, mut conn) = fx();
        assert!(scan(&conn, &root).unwrap().is_clean());
        let w2 = root.join("Anderson Farms LLC (ANDE1042)/Income Tax/2025/w2.pdf");
        std::fs::write(&w2, "w2 signed in Acrobat").unwrap();
        let d = scan(&conn, &root).unwrap();
        assert_eq!(d.checked, 3);
        assert_eq!(d.modified.len(), 1);
        let m = &d.modified[0];
        assert!(m.rel_path.ends_with("w2.pdf"));
        assert_eq!(
            adopt_modified(&mut conn, &root, "dev", &d.modified)
                .unwrap()
                .len(),
            1
        );
        // A second look at the same (now stale) findings adopts nothing.
        assert!(
            adopt_modified(&mut conn, &root, "dev", &d.modified)
                .unwrap()
                .is_empty()
        );
        assert!(scan(&conn, &root).unwrap().is_clean());
        let new_hash = sha256_file(&w2).unwrap();
        let (hash, versions): (String, i64) = conn
            .query_row(
                "SELECT hash, (SELECT count(*) FROM version v WHERE v.doc_id = d.id
                   AND v.hash = ?2 AND v.created_by = 'external_modify')
                 FROM document d WHERE d.id = ?1",
                [&m.doc_id, &m.old_hash],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(hash, new_hash);
        assert_eq!(versions, 1);
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM audit WHERE action = 'external_modify'"
            ),
            1
        );
        let v = crate::versions::list(&conn, &root, &m.doc_id).unwrap();
        assert_eq!(v.len(), 1);
        assert!(
            !v[0].kept,
            "the content before the outside edit was never seen"
        );
    }

    /// A file copied into a client folder through Explorer is found and added (PA-23); working
    /// files, moved copies and already cataloged paths are not.
    #[test]
    fn new_files_in_client_folders_are_found_and_added() {
        let (_t, root, mut conn) = fx();
        let client = "Anderson Farms LLC (ANDE1042)";
        let fin = root.join(client).join("Financials");
        std::fs::create_dir_all(&fin).unwrap();
        std::fs::write(fin.join("balance sheet.xlsx"), "invented balance sheet").unwrap();
        std::fs::write(fin.join("~$balance sheet.xlsx"), "office lock").unwrap();
        std::fs::write(fin.join("desktop.ini"), "x").unwrap();
        // A copy of a cataloged document's content: left for the sweep.
        let k1 = std::fs::read(root.join(client).join("Income Tax/2025/k1.pdf")).unwrap();
        std::fs::write(fin.join("k1 copy.pdf"), k1).unwrap();
        let (known, hashes): (
            std::collections::HashSet<String>,
            std::collections::HashSet<String>,
        ) = {
            let mut st = conn
                .prepare("SELECT rel_path, hash FROM document WHERE deleted_at IS NULL")
                .unwrap();
            let rows: Vec<(String, String)> = st
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            (
                rows.iter().map(|r| r.0.to_lowercase()).collect(),
                rows.into_iter().map(|r| r.1).collect(),
            )
        };
        let client_id: String = conn
            .query_row("SELECT id FROM client", [], |r| r.get(0))
            .unwrap();
        let found = find_new(&root, &[(client.into(), client_id)], &known, &hashes);
        let names: Vec<&str> = found.iter().map(|u| u.rel_path.as_str()).collect();
        assert_eq!(names, [format!("{client}/Financials/balance sheet.xlsx")]);
        let added = adopt_new(&mut conn, &root, "dev", &found).unwrap();
        assert_eq!(added.len(), 1);
        // Found again before the catalog was re-read: not added twice.
        assert!(
            adopt_new(&mut conn, &root, "dev", &found)
                .unwrap()
                .is_empty()
        );
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM document WHERE rel_path LIKE '%balance sheet.xlsx'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn moves_missing_and_unknown_files() {
        let (t, root, mut conn) = fx();
        let year = root.join("Anderson Farms LLC (ANDE1042)/Income Tax/2025");
        let other = root.join("Anderson Farms LLC (ANDE1042)/Correspondence");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::rename(year.join("k1.pdf"), other.join("k1.pdf")).unwrap();
        std::fs::remove_file(year.join("1099.pdf")).unwrap();
        std::fs::write(year.join("notice.pdf"), "IRS notice").unwrap();
        std::fs::write(year.join("scan.pdf"), "a scan").unwrap();
        let d = scan(&conn, &root).unwrap();
        assert_eq!(d.moved.len(), 1);
        assert!(d.moved[0].1.rel_path.ends_with("Correspondence/k1.pdf"));
        assert_eq!(d.missing.len(), 1);
        assert!(d.missing[0].rel_path.ends_with("1099.pdf"));
        let mut unknown: Vec<&str> = d.unknown.iter().map(|u| u.rel_path.as_str()).collect();
        unknown.sort();
        assert!(unknown[0].ends_with("notice.pdf") && unknown[1].ends_with("scan.pdf"));

        follow_moves(&mut conn, &root, "dev", &d.moved).unwrap();
        mark_missing(&mut conn, &root, "dev", &d.missing).unwrap();
        // Undoing it takes the queued sync delete back too.
        let undo_id: String = conn
            .query_row(
                "SELECT tx_id FROM undo_record WHERE label = 'missing'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        crate::fileops::undo(&mut conn, &root, &undo_id, "dev").unwrap();
        let pending: i64 = conn
            .query_row(
                "SELECT count(*) FROM sync_pending WHERE op = 'delete'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pending, 0);
        mark_missing(&mut conn, &root, "dev", &d.missing).unwrap();
        let notice: Vec<Unknown> = d
            .unknown
            .iter()
            .filter(|u| u.rel_path.ends_with("notice.pdf"))
            .cloned()
            .collect();
        adopt_unknown(&mut conn, &root, "dev", &notice).unwrap();
        let inbox = t.path().join("inbox");
        std::fs::create_dir_all(&inbox).unwrap();
        let scan_file: Vec<Unknown> = d
            .unknown
            .iter()
            .filter(|u| u.rel_path.ends_with("scan.pdf"))
            .cloned()
            .collect();
        let moved = refile_unknown(&mut conn, &root, &inbox, "dev", &scan_file).unwrap();
        assert_eq!(moved, [inbox.join("scan.pdf")]);
        assert!(scan(&conn, &root).unwrap().is_clean());
        let (year_of_notice, client): (Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT tax_year, client_id FROM document WHERE rel_path LIKE '%notice.pdf'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(year_of_notice, Some(2025));
        assert!(client.is_some());

        assert!(due(&conn));
        set_last_sweep(&conn, &vf_core::now_rfc3339()).unwrap();
        assert!(!due(&conn));
    }
}
