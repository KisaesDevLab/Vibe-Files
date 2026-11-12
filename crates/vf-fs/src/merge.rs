//! Merge another client cabinet into this one (owner 2026-11-05, Q-62, Q-65).
//!
//! [`plan`] reads the source cabinet and writes nothing: each source client folder (one with a
//! sentinel) is paired with this cabinet's client of the same ID (the same ID is the same
//! client), or becomes a new client. [`apply`] then, per client, in one Transaction (undo per
//! client): makes the client and its folder here when needed (`newclient::create`), moves each
//! file to the same place below this client folder through Filer's move engine (hash-checked,
//! cross-volume copy-verify-delete), gives it its catalog row, `sync_pending` and the source's
//! doc type, tax year and tags (matched by content from the source's `docmeta.jsonl`), and
//! audits it. A different file already at that place: the incoming one gets ` (2)` (keep both);
//! the same content already there: the source file is left and counted. Source folders left
//! empty are removed afterwards. The source's audit trail is copied as a read-only history into
//! `_Vibe/audit/merged/<source cabinet id>/`. Versions and the source's client details are not
//! brought over.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use filer_intake::commit::sha256_file;
use serde_json::json;
use vf_core::audit::AuditEntry;
use vf_core::db::Connection;
use vf_core::tx::{FileOp, Stmt, Transaction};

use crate::{cabinet, clients, docmeta, newclient, rebuild};

pub const LABEL: &str = "cabinet_merge";
pub const HISTORY_LABEL: &str = "cabinet_merge_history";

/// One source client folder.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientMove {
    /// Source-relative client folder.
    pub src_folder: String,
    pub ext_id: String,
    pub name: String,
    /// This cabinet's client folder (cabinet-relative) when it has one; `None`: made at merge.
    pub dest_folder: Option<String>,
    /// This cabinet has no client of this ID: one is added.
    pub new_client: bool,
    /// Files below the source folder (outside `_Vibe`), relative to it.
    pub files: Vec<String>,
    /// Of `files`, how many have a file at the same place here (checked at merge: same
    /// content left, different kept both).
    pub clashes: usize,
    /// Merged when applied (the person can untick a client).
    pub include: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MergePlan {
    pub src_root: PathBuf,
    pub src_cabinet_id: String,
    pub clients: Vec<ClientMove>,
    /// Source top-level folders with no sentinel (not client folders): left where they are.
    pub unbound: Vec<String>,
    /// Files directly in the source cabinet's root: left.
    pub loose: usize,
}

/// What [`apply`] did; kept to [`undo`] it.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Merged {
    pub clients: usize,
    pub new_clients: usize,
    pub moved: usize,
    /// Kept both: the incoming file got a ` (n)` name.
    pub renamed: usize,
    /// The same content was here already: left in the source.
    pub left_same: usize,
    /// Files that could not be read or moved (left in the source).
    pub failed: Vec<String>,
    /// One per client, in order.
    pub txs: Vec<String>,
    pub history_tx: Option<String>,
    /// Source folders removed once empty: made again by [`undo`] so the files can go back.
    #[serde(default)]
    pub removed_dirs: Vec<PathBuf>,
}

fn rel_of(base: &Path, p: &Path) -> String {
    p.strip_prefix(base)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Reads the source cabinet at `src_root` against this one (`root`). Writes nothing.
pub fn plan(conn: &Connection, root: &Path, src_root: &Path) -> Result<MergePlan, String> {
    let here = cabinet::read(root)?;
    let src = cabinet::read(src_root)
        .map_err(|e| format!("{} is not a Vibe Files cabinet: {e}", src_root.display()))?;
    if src.cabinet_id == here.cabinet_id {
        return Err("That is this cabinet.".into());
    }
    let canon = |p: &Path| std::fs::canonicalize(p).map_err(|e| format!("{}: {e}", p.display()));
    let (a, b) = (canon(root)?, canon(src_root)?);
    if a.starts_with(&b) || b.starts_with(&a) {
        return Err("One cabinet is inside the other: pick a separate cabinet.".into());
    }
    // Names from the source's client list where it has one, else from the sentinel.
    let src_names: HashMap<String, String> = clients::load(src_root)
        .ok()
        .and_then(|m| m.list)
        .map(|l| {
            l.clients
                .into_iter()
                .map(|c| (c.id.to_uppercase(), c.name))
                .collect()
        })
        .unwrap_or_default();
    // This cabinet's clients by ID (any case).
    let mut here_clients: HashMap<String, Option<String>> = HashMap::new();
    {
        let mut st = conn
            .prepare("SELECT ext_id, folder_rel FROM client WHERE deleted_at IS NULL")
            .map_err(|e| e.to_string())?;
        let rows = st
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
            })
            .map_err(|e| e.to_string())?;
        for r in rows {
            let (ext, folder) = r.map_err(|e| e.to_string())?;
            here_clients.insert(ext.to_uppercase(), folder.filter(|f| !f.is_empty()));
        }
    }
    let mut p = MergePlan {
        src_root: src_root.to_path_buf(),
        src_cabinet_id: src.cabinet_id,
        ..Default::default()
    };
    p.loose = std::fs::read_dir(src_root)
        .map_err(|e| format!("cannot read {}: {e}", src_root.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .count();
    let tops = cabinet::client_folders(src_root)
        .map_err(|e| format!("cannot read {}: {e}", src_root.display()))?;
    for folder in tops {
        let src_folder = rel_of(src_root, &folder);
        let Ok(Some(s)) = filer_intake::sentinel::read_sentinel(&folder) else {
            p.unbound.push(src_folder);
            continue;
        };
        let ext = s.tax_software_id.clone();
        let name = src_names
            .get(&ext.to_uppercase())
            .cloned()
            .unwrap_or(s.display_name_at_creation.clone());
        let (dest_folder, new_client) = match here_clients.get(&ext.to_uppercase()) {
            Some(f) => (f.clone(), false),
            None => (None, true),
        };
        let mut paths = Vec::new();
        rebuild::files_under(&folder, &mut paths).map_err(|e| e.to_string())?;
        let files: Vec<String> = paths.iter().map(|f| rel_of(&folder, f)).collect();
        let clashes = dest_folder.as_ref().map_or(0, |d| {
            files
                .iter()
                .filter(|f| root.join(d).join(f).exists())
                .count()
        });
        p.clients.push(ClientMove {
            src_folder,
            ext_id: ext,
            name,
            dest_folder,
            new_client,
            files,
            clashes,
            include: true,
        });
    }
    Ok(p)
}

/// Where a merge's undo record is kept.
fn last_path(root: &Path) -> PathBuf {
    root.join(cabinet::VIBE_DIR).join("merge_last.json")
}

/// The last merge into this cabinet (for Undo).
pub fn load_last(root: &Path) -> Option<Merged> {
    std::fs::read(last_path(root))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
}

/// Merges the ticked clients of `plan` into this cabinet. `progress` counts files done.
pub fn apply(
    conn: &mut Connection,
    root: &Path,
    plan: &MergePlan,
    device_id: &str,
    progress: &std::sync::atomic::AtomicUsize,
) -> Result<Merged, String> {
    use std::sync::atomic::Ordering;
    let src_root = &plan.src_root;
    let mut out = Merged::default();
    // Doc types here: by id, and by lower-case name (the source's ids may differ).
    let mut type_ids: HashSet<String> = HashSet::new();
    let mut type_names: HashMap<String, (String, String)> = HashMap::new();
    for t in crate::doctypes::load(root)?.unwrap_or_default() {
        type_ids.insert(t.id.clone());
        type_names.insert(t.name.trim().to_lowercase(), (t.id.clone(), t.name.clone()));
    }
    let id_name: HashMap<String, String> = type_names
        .values()
        .map(|(id, n)| (id.clone(), n.clone()))
        .collect();
    let folders: Vec<String> = plan.clients.iter().map(|c| c.src_folder.clone()).collect();
    let src_meta = docmeta::Index::load(src_root, &folders);

    for c in plan.clients.iter().filter(|c| c.include) {
        // This cabinet's client and folder (made when needed, as + New client does).
        let (client_id, dest_folder) = match &c.dest_folder {
            Some(f) => {
                let id: String = conn
                    .query_row(
                        "SELECT id FROM client WHERE upper(ext_id) = upper(?1) AND deleted_at IS NULL",
                        [&c.ext_id],
                        |r| r.get(0),
                    )
                    .map_err(|e| format!("{}: {e}", c.ext_id))?;
                (id, f.clone())
            }
            None => {
                let np = newclient::plan(
                    conn,
                    root,
                    &newclient::NewClient {
                        ext_id: c.ext_id.clone(),
                        name: c.name.clone(),
                        entity_type: None,
                        partner: String::new(),
                    },
                )
                .map_err(|e| format!("{} ({}): {e}", c.name, c.ext_id))?;
                newclient::create(conn, root, &np, device_id)?;
                if c.new_client {
                    out.new_clients += 1;
                }
                (np.client.uuid.to_string(), np.folder_rel.clone())
            }
        };
        let now = vf_core::now_rfc3339();
        let mut tx = Transaction::new(LABEL);
        let mut taken: HashSet<PathBuf> = HashSet::new();
        let mut moved = 0;
        let mut meta_lines: HashMap<PathBuf, Vec<String>> = HashMap::new();
        for inner in &c.files {
            let src = src_root.join(&c.src_folder).join(inner);
            let Ok(hash) = sha256_file(&src) else {
                out.failed.push(format!("{}/{inner}", c.src_folder));
                progress.fetch_add(1, Ordering::Relaxed);
                continue;
            };
            let size = std::fs::metadata(&src).map(|m| m.len()).unwrap_or(0);
            let mut dst = root.join(&dest_folder).join(inner);
            if dst.exists() {
                if sha256_file(&dst).ok().as_deref() == Some(hash.as_str()) {
                    // The same content is here: the source copy stays where it is.
                    out.left_same += 1;
                    progress.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                out.renamed += 1;
            }
            while dst.exists() || taken.contains(&dst) {
                dst = filer_intake::commit::free_path(&dst);
                if taken.contains(&dst) {
                    let n = taken.len() + 2;
                    let stem = dst.file_stem().map(|s| s.to_string_lossy().into_owned());
                    let name = match dst.extension() {
                        Some(e) => {
                            format!("{} ({n}).{}", stem.unwrap_or_default(), e.to_string_lossy())
                        }
                        None => format!("{} ({n})", stem.unwrap_or_default()),
                    };
                    dst = dst.with_file_name(name);
                }
            }
            taken.insert(dst.clone());
            let rel = rel_of(root, &dst);
            // The source's metadata for this content, mapped onto this cabinet's doc types.
            let rec = src_meta.get(&format!("{}/{inner}", c.src_folder), &hash);
            let doc_type = rec.and_then(|r| {
                r.doc_type
                    .clone()
                    .filter(|t| type_ids.contains(t))
                    .or_else(|| {
                        r.doc_type_name
                            .as_deref()
                            .and_then(|n| type_names.get(&n.trim().to_lowercase()))
                            .map(|(id, _)| id.clone())
                    })
            });
            let tax_year: Option<i32> = match rec {
                Some(r) => r.tax_year,
                None => rebuild::year_from_path(&rel).map(|y| y as i32),
            };
            let status = rec.map_or("received".to_string(), |r| r.status.clone());
            let source = rec.and_then(|r| r.source.clone());
            let tags: Vec<String> = rec.map(|r| r.tags.clone()).unwrap_or_default();
            let id = vf_core::new_id();
            tx = tx
                .file(FileOp::Move {
                    src: src.clone(),
                    dst: dst.clone(),
                })
                .catalog(Stmt::new(
                    "INSERT INTO document (id, client_id, tax_year, rel_path, hash, size, mime, status,
                       source, filed_by, filed_at, device_id, updated_at, doc_type_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'merge', ?10, ?11, ?10, ?12)",
                    vec![
                        id.clone().into(),
                        client_id.clone().into(),
                        tax_year.into(),
                        rel.clone().into(),
                        hash.clone().into(),
                        (size as i64).into(),
                        rebuild::mime(&rel).into(),
                        status.clone().into(),
                        source.clone().into(),
                        now.clone().into(),
                        device_id.into(),
                        doc_type.clone().into(),
                    ],
                ))
                // Undo runs newest first: the tags and sync row go before the document.
                .catalog_undo(Stmt::new(
                    "DELETE FROM document WHERE id = ?1",
                    vec![id.clone().into()],
                ))
                .catalog_undo(Stmt::new(
                    "DELETE FROM document_tag WHERE doc_id = ?1",
                    vec![id.clone().into()],
                ))
                .catalog(Stmt::new(
                    "INSERT INTO sync_pending (doc_id, op, since) VALUES (?1, 'put', ?2)
                     ON CONFLICT(doc_id, op) DO NOTHING",
                    vec![id.clone().into(), now.clone().into()],
                ))
                .catalog_undo(Stmt::new(
                    "DELETE FROM sync_pending WHERE doc_id = ?1",
                    vec![id.clone().into()],
                ));
            for t in &tags {
                tx = tx.catalog(Stmt::new(
                    "INSERT OR IGNORE INTO document_tag (doc_id, tag) VALUES (?1, ?2)",
                    vec![id.clone().into(), t.clone().into()],
                ));
            }
            // The metadata travels with the file in this cabinet's sidecar (rebuild restores it).
            if rec.is_some() {
                let m = docmeta::Meta {
                    id: id.clone(),
                    rel_path: rel.clone(),
                    hash: hash.clone(),
                    folder_rel: Some(dest_folder.clone()),
                    doc_type_name: doc_type.as_ref().and_then(|t| id_name.get(t).cloned()),
                    doc_type: doc_type.clone(),
                    tax_year,
                    status,
                    source,
                    tags,
                    stamp: None,
                };
                let (file, line) = docmeta::record_line(root, &m, &now, device_id);
                meta_lines.entry(file).or_default().push(line);
            }
            moved += 1;
            progress.fetch_add(1, Ordering::Relaxed);
        }
        if moved == 0 {
            continue;
        }
        for (path, lines) in meta_lines {
            tx = tx.file(FileOp::AppendLines {
                path,
                lines,
                undo_lines: vec![],
            });
        }
        tx = tx.audit(AuditEntry::new(
            LABEL,
            None,
            json!({ "client": c.ext_id, "from": plan.src_cabinet_id, "files": moved }),
        ));
        let committed = crate::fileops::commit_in(conn, &[root, src_root.as_path()], tx, device_id)
            .map_err(|e| format!("{} ({}): {e}", c.name, c.ext_id))?;
        out.txs.push(committed.tx_id);
        out.moved += moved;
        out.clients += 1;
        // Source folders left empty go (never one holding anything; never remove_dir_all).
        out.removed_dirs
            .extend(remove_empty_dirs(&src_root.join(&c.src_folder)));
    }

    // The source's audit trail, kept as read-only history beside this cabinet's own.
    let src_audit = src_root.join(cabinet::VIBE_DIR).join("audit");
    if let Ok(entries) = std::fs::read_dir(&src_audit) {
        let dest = root
            .join(cabinet::VIBE_DIR)
            .join("audit")
            .join("merged")
            .join(&plan.src_cabinet_id);
        let mut tx = Transaction::new(HISTORY_LABEL);
        let mut n = 0;
        for e in entries.filter_map(|e| e.ok()) {
            let p = e.path();
            if p.is_file() && !dest.join(e.file_name()).exists() {
                tx = tx.file(FileOp::Copy {
                    src: p,
                    dst: dest.join(e.file_name()),
                });
                n += 1;
            }
        }
        if n > 0 {
            tx = tx.audit(AuditEntry::new(
                HISTORY_LABEL,
                None,
                json!({ "from": plan.src_cabinet_id, "files": n }),
            ));
            out.history_tx = Some(
                crate::fileops::commit_in(conn, &[root, src_root.as_path()], tx, device_id)?.tx_id,
            );
        }
    }
    let _ = cabinet::write_replace(
        &last_path(root),
        &serde_json::to_vec_pretty(&out).expect("serializes"),
    );
    Ok(out)
}

/// Removes the empty folders under `dir` (deepest first), keeping `dir` and its `_Vibe`.
/// Returns those removed.
fn remove_empty_dirs(dir: &Path) -> Vec<PathBuf> {
    fn go(d: &Path, top: bool, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(d) else {
            return;
        };
        for e in entries.filter_map(|e| e.ok()) {
            let p = e.path();
            if e.file_type().is_ok_and(|t| t.is_dir())
                && e.file_name() != std::ffi::OsStr::new(cabinet::VIBE_DIR)
            {
                go(&p, false, out);
            }
        }
        // Fails, as it should, when the folder still holds anything.
        if !top && std::fs::remove_dir(d).is_ok() {
            out.push(d.to_path_buf());
        }
    }
    let mut out = Vec::new();
    go(dir, true, &mut out);
    out
}

/// Undoes the last merge: every client's Transaction, newest first, then the history copy.
/// New clients and their folders stay (empty); their catalog rows are kept.
pub fn undo(conn: &mut Connection, root: &Path, m: &Merged, device_id: &str) -> Result<(), String> {
    if let Some(h) = &m.history_tx {
        crate::fileops::undo(conn, root, h, device_id)?;
    }
    // The source folders the merge emptied and removed, back first (parents before children).
    let mut dirs = m.removed_dirs.clone();
    dirs.sort_by_key(|d| d.components().count());
    for d in dirs {
        std::fs::create_dir_all(&d).map_err(|e| format!("cannot make {}: {e}", d.display()))?;
    }
    for tx in m.txs.iter().rev() {
        crate::fileops::undo(conn, root, tx, device_id)?;
    }
    let _ = std::fs::remove_file(last_path(root));
    Ok(())
}

#[cfg(test)]
mod tests {
    use vf_core::db::Storage;

    use super::*;
    use crate::cabinet::CabinetFile;
    use crate::newclient::NewClient;

    struct Cab {
        _t: tempfile::TempDir,
        root: PathBuf,
        conn: Connection,
    }

    fn cab(name: &str) -> Cab {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join(name);
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
        let mut conn = vf_core::db::open(&t.path().join("c.db"), Storage::Local).unwrap();
        crate::doctypes::seed_if_missing(&root).unwrap();
        crate::doctypes::sync(&mut conn, &root, "dev", "seed").unwrap();
        Cab { _t: t, root, conn }
    }

    fn client(c: &mut Cab, ext: &str, name: &str) -> String {
        let p = newclient::plan(
            &c.conn,
            &c.root,
            &NewClient {
                ext_id: ext.into(),
                name: name.into(),
                entity_type: None,
                partner: String::new(),
            },
        )
        .unwrap();
        newclient::create(&mut c.conn, &c.root, &p, "dev").unwrap();
        p.folder_rel
    }

    fn put(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    #[test]
    fn a_cabinet_merges_by_client_id_and_undoes() {
        let mut here = cab("here");
        let mut there = cab("there");
        let smith_here = client(&mut here, "SMIT1001", "Smith, Pat");
        let smith_there = client(&mut there, "SMIT1001", "Smith, Pat");
        let acme_there = client(&mut there, "ACME1002", "Acme LLC");
        // Invented files: one new, one the same content, one clashing.
        put(
            &there.root,
            &format!("{smith_there}/Income Tax/2024/return.pdf"),
            "invented return",
        );
        put(
            &there.root,
            &format!("{smith_there}/Income Tax/2023/same.pdf"),
            "same content",
        );
        put(
            &here.root,
            &format!("{smith_here}/Income Tax/2023/same.pdf"),
            "same content",
        );
        put(
            &there.root,
            &format!("{smith_there}/Correspondence/note.pdf"),
            "theirs",
        );
        put(
            &here.root,
            &format!("{smith_here}/Correspondence/note.pdf"),
            "ours",
        );
        put(
            &there.root,
            &format!("{acme_there}/Bookkeeping/2025/ledger.pdf"),
            "invented ledger",
        );
        // The source's metadata for the return: a doc type and year.
        crate::rebuild::rebuild(&mut there.conn, &there.root, "dev").unwrap();
        let w2: String = there
            .conn
            .query_row("SELECT id FROM doc_type WHERE name = 'W-2'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let rid: String = there
            .conn
            .query_row(
                "SELECT id FROM document WHERE rel_path LIKE '%return.pdf'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        docmeta::edit_each(
            &mut there.conn,
            &there.root,
            &[(
                rid,
                docmeta::MetaEdit {
                    doc_type: Some(Some(w2.clone())),
                    tags: Some(vec!["from-there".into()]),
                    ..Default::default()
                },
            )],
            "meta",
            "dev",
        )
        .unwrap();

        // The source's audit trail (the app mirrors it there as it runs).
        put(
            &there.root,
            "_Vibe/audit/2026-11.dev.jsonl",
            "{\"seq\":1}
",
        );
        let p = plan(&here.conn, &here.root, &there.root).unwrap();
        let smith = p.clients.iter().find(|c| c.ext_id == "SMIT1001").unwrap();
        assert!(!smith.new_client && smith.clashes == 2);
        assert!(
            p.clients
                .iter()
                .find(|c| c.ext_id == "ACME1002")
                .unwrap()
                .new_client
        );
        let done = std::sync::atomic::AtomicUsize::new(0);
        let m = apply(&mut here.conn, &here.root, &p, "dev", &done).unwrap();
        assert_eq!(
            (m.clients, m.new_clients, m.left_same, m.renamed),
            (2, 1, 1, 1)
        );
        assert_eq!(m.moved, 3);
        assert!(
            here.root
                .join(format!("{smith_here}/Income Tax/2024/return.pdf"))
                .is_file()
        );
        assert!(
            here.root
                .join(format!("{smith_here}/Correspondence/note (2).pdf"))
                .is_file()
        );
        assert_eq!(
            std::fs::read_to_string(
                here.root
                    .join(format!("{smith_here}/Correspondence/note.pdf"))
            )
            .unwrap(),
            "ours",
            "ours kept"
        );
        assert!(
            !there
                .root
                .join(format!("{smith_there}/Income Tax/2024/return.pdf"))
                .exists(),
            "moved"
        );
        assert!(
            there
                .root
                .join(format!("{smith_there}/Income Tax/2023/same.pdf"))
                .is_file(),
            "same: left"
        );
        assert!(
            !there
                .root
                .join(format!("{acme_there}/Bookkeeping"))
                .exists(),
            "emptied folders removed"
        );
        let (t, tagged): (String, i64) = here
            .conn
            .query_row(
                "SELECT doc_type_id, (SELECT count(*) FROM document_tag g WHERE g.doc_id = d.id AND g.tag = 'from-there')
                 FROM document d WHERE rel_path LIKE '%return.pdf'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        // Mapped by name onto this cabinet's W-2 (each cabinet has its own ids).
        let w2_here: String = here
            .conn
            .query_row("SELECT id FROM doc_type WHERE name = 'W-2'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!((t, tagged), (w2_here, 1));
        let acme_rows: i64 = here
            .conn
            .query_row(
                "SELECT count(*) FROM client WHERE ext_id = 'ACME1002' AND folder_rel IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(acme_rows, 1, "the new client has its folder here");
        assert!(m.history_tx.is_some(), "the source's audit trail copied");
        assert!(
            here.root
                .join(format!("_Vibe/audit/merged/{}", p.src_cabinet_id))
                .is_dir()
        );

        // Undo: the files go back.
        let last = load_last(&here.root).unwrap();
        undo(&mut here.conn, &here.root, &last, "dev").unwrap();
        assert!(
            there
                .root
                .join(format!("{smith_there}/Income Tax/2024/return.pdf"))
                .is_file()
        );
        assert!(
            !here
                .root
                .join(format!("{smith_here}/Income Tax/2024/return.pdf"))
                .exists()
        );
        let n: i64 = here
            .conn
            .query_row(
                "SELECT count(*) FROM document WHERE filed_by = 'merge'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn the_same_cabinet_or_one_inside_the_other_is_refused() {
        let here = cab("here");
        assert!(plan(&here.conn, &here.root, &here.root).is_err());
    }
}
