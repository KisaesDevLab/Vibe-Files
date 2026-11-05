//! The catalog side of Filer Desktop's Intake commits (P02 step 1).
//!
//! The mounted Filer app files documents itself (`filer_intake::commit::run_commit`: Filer's
//! moves, collisions, copies, sentinels, `ops.jsonl`, `last_batch.json`). Vibe Files then records
//! each finished batch in the catalog as **one** Transaction, as CLAUDE.md §Document moves
//! describes: a `document` row (and a `sync_pending` row) per filed file, audit rows from the
//! batch's `ops.jsonl` records, and an undo record whose catalog inverse is run when Filer's own
//! Undo takes the batch back. Files first, catalog second: a crash in between is caught up when
//! the cabinet next opens ([`reconcile`]), and `rebuild` reconciles anything else.

use std::collections::HashMap;
use std::path::Path;

use filer_intake::commit::{LastBatch, load_last_batch};
use filer_intake::oplog::{self, OpResult};
use serde_json::json;
use vf_core::audit::AuditEntry;
use vf_core::db::Connection;
use vf_core::tx::{Actor, FileOp, NoFileOps, Stmt, Transaction};

use crate::hints::Hint;

use crate::rebuild::{mime, rel, year_from_path};

pub const LABEL: &str = "intake_batch";

/// Each PC's Intake works in its own folder, `_Vibe/intake/<device>` (owner 2026-11-04, Q-53:
/// several people file at once): its last batch, op log, undo zips, working PDFs and running
/// marker are its own; the files every PC shares ([`SHARED`]) are copied in and back
/// ([`Shared`]). Batches of every PC are recorded in each catalog ([`reconcile`]).
pub const PC_DIR: &str = "intake";

/// Filer's files the PCs share: settings, rules, client list, relationships, folder template,
/// folder index.
pub const SHARED: [&str; 6] = [
    "config.json",
    "rules.json",
    "clients.json",
    "relationships.json",
    "template.json",
    "folders.json",
];

/// This PC's Intake folder.
pub fn pc_dir(root: &Path, device_id: &str) -> std::path::PathBuf {
    let safe: String = device_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    crate::clients::master_dir(root).join(PC_DIR).join(safe)
}

/// Every folder a batch can be in: the cabinet's `_Vibe` (batches filed before Q-53) and each
/// PC's Intake folder.
pub fn batch_dirs(root: &Path) -> Vec<std::path::PathBuf> {
    let master = crate::clients::master_dir(root);
    let mut v = vec![master.clone()];
    if let Ok(rd) = std::fs::read_dir(master.join(PC_DIR)) {
        let mut pcs: Vec<_> = rd
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
            .collect();
        pcs.sort();
        v.extend(pcs);
    }
    v
}

/// Whether any PC's Filer is filing a batch right now (its last batch saved as in progress
/// within the hour; an older one is a stopped run Filer's recovery deals with). Files appear
/// in client folders before Filer lists them, so nothing is adopted meanwhile (QA 2026-11-04).
pub fn filing_now(root: &Path) -> bool {
    batch_dirs(root).iter().any(|d| {
        load_last_batch(d).is_some_and(|lb| {
            lb.in_progress
                && !lb.undone
                && std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .is_ok_and(|n| (n.as_secs() as i64) - lb.committed_at.timestamp() < 3600)
        })
    })
}

/// The newest change to any PC's last batch (Intake looks again when it moves).
pub fn latest_change(root: &Path) -> Option<std::time::SystemTime> {
    batch_dirs(root)
        .iter()
        .filter_map(|d| {
            std::fs::metadata(filer_intake::commit::last_batch_path(d))
                .and_then(|m| m.modified())
                .ok()
        })
        .max()
}

fn mtime(p: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Keeps a PC's copies of the [`SHARED`] files and the cabinet's in step.
#[derive(Debug, Default)]
pub struct Shared {
    /// Per file: (cabinet copy's time, PC copy's time) as last brought in step.
    seen: HashMap<&'static str, (Option<std::time::SystemTime>, Option<std::time::SystemTime>)>,
}

/// What [`Shared::poll`] copied.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SharedChanges {
    /// Changed by this PC's Filer, copied to the cabinet.
    pub to_cabinet: Vec<&'static str>,
    /// Changed elsewhere (another PC, Vibe's own screens), copied in: Filer, loaded earlier,
    /// should reload to use them.
    pub from_cabinet: Vec<&'static str>,
}

fn copy_over(from: &Path, to: &Path) -> Result<(), String> {
    let bytes = std::fs::read(from).map_err(|e| format!("{}: {e}", from.display()))?;
    crate::cabinet::write_replace(to, &bytes).map_err(|e| format!("{}: {e}", to.display()))
}

/// Filer's `config.json` carries the look (theme, zoom) under `display`: that part is each PC's
/// own and never travels (QA 2026-11-04: a text-size change on one PC made every other PC's
/// Intake ask for a reload). `from`'s settings with `keep`'s `display`; `None` when either is not
/// JSON (then the file is copied as is).
fn config_keeping_display(from: &Path, keep: &Path) -> Option<serde_json::Value> {
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(from).ok()?).ok()?;
    let k: Option<serde_json::Value> = std::fs::read(keep)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
    let o = v.as_object_mut()?;
    match k.as_ref().and_then(|k| k.get("display")) {
        Some(d) => {
            o.insert("display".into(), d.clone());
        }
        None => {
            o.remove("display");
        }
    }
    Some(v)
}

/// Copies shared file `name` from `from` over `to`; `false` when nothing but the look differs
/// (`config.json`), so nothing was written.
fn bring(name: &str, from: &Path, to: &Path) -> Result<bool, String> {
    if name == CONFIG
        && let Some(v) = config_keeping_display(from, to)
    {
        let now: Option<serde_json::Value> = std::fs::read(to)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        if now.as_ref() == Some(&v) {
            return Ok(false);
        }
        let bytes = serde_json::to_vec_pretty(&v).map_err(|e| e.to_string())?;
        crate::cabinet::write_replace(to, &bytes).map_err(|e| format!("{}: {e}", to.display()))?;
        return Ok(true);
    }
    // The same bytes are not written again (another PC would see a change that is none).
    if let (Ok(a), Ok(b)) = (std::fs::read(from), std::fs::read(to))
        && a == b
    {
        return Ok(false);
    }
    copy_over(from, to)?;
    Ok(true)
}

const CONFIG: &str = "config.json";

impl Shared {
    /// Before Filer loads from `pc`: the cabinet's shared files copied in.
    pub fn prepare(&mut self, root: &Path, pc: &Path) -> Result<(), String> {
        std::fs::create_dir_all(pc).map_err(|e| format!("{}: {e}", pc.display()))?;
        let master = crate::clients::master_dir(root);
        self.seen.clear();
        for f in SHARED {
            let (m, p) = (master.join(f), pc.join(f));
            // A save on this PC the 2 s poll had not sent yet (a reload or exit right after
            // it) goes to the cabinet first, not lost under the cabinet's copy (QA 2026-11-04).
            let pc_newer = match (mtime(&p), mtime(&m)) {
                (Some(pt), Some(mt)) => pt > mt,
                (Some(_), None) => true,
                _ => false,
            };
            if pc_newer {
                bring(f, &p, &m)?;
            } else if m.is_file() {
                bring(f, &m, &p)?;
            }
            self.seen.insert(f, (mtime(&m), mtime(&p)));
        }
        Ok(())
    }

    /// Brings the copies in step: a file this PC's Filer saved goes to the cabinet; one changed
    /// in the cabinet comes in. When both changed, this PC's save wins (the last save does).
    pub fn poll(&mut self, root: &Path, pc: &Path) -> Result<SharedChanges, String> {
        let master = crate::clients::master_dir(root);
        let mut out = SharedChanges::default();
        for f in SHARED {
            let (m, p) = (master.join(f), pc.join(f));
            let (mt, pt) = (mtime(&m), mtime(&p));
            let (sm, sp) = self.seen.get(f).copied().unwrap_or((None, None));
            if pt.is_some() && pt != sp {
                if bring(f, &p, &m)? {
                    out.to_cabinet.push(f);
                }
            } else if mt.is_some() && mt != sm {
                if bring(f, &m, &p)? {
                    out.from_cabinet.push(f);
                }
            } else {
                continue;
            }
            self.seen.insert(f, (mtime(&m), mtime(&p)));
        }
        Ok(out)
    }
}

/// What [`reconcile`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Reconciled {
    /// A batch was recorded: (Filer batch id, documents added, documents updated).
    pub recorded: Option<(String, usize, usize)>,
    /// A batch Filer undid had its catalog side undone.
    pub undone: Option<String>,
}

impl Reconciled {
    pub fn changed(&self) -> bool {
        self.recorded.is_some() || self.undone.is_some()
    }
}

/// The undo record of a recorded Filer batch: (tx id, already undone).
fn recorded(conn: &Connection, batch: &str) -> Option<(String, bool)> {
    conn.query_row(
        "SELECT tx_id, undone FROM undo_record
         WHERE label = ?1 AND json_extract(record, '$.extra.filer_batch') = ?2",
        [LABEL, batch],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .ok()
}

/// Brings the catalog in line with Filer's last batch: records a finished batch not recorded
/// yet, or undoes the catalog side of a batch Filer undid. Cheap when there is nothing to do.
/// Cabinet-relative paths (forward slashes, lower case) of the files Filer's "Undo last batch"
/// would check: a file there must keep its bytes (Filer refuses to undo a changed size). Empty
/// when the last batch was undone; a batch still being written counts.
pub fn undoable_paths(root: &Path) -> std::collections::HashSet<String> {
    // The cabinet-level `_Vibe/last_batch.json` predates the per-PC folders (Q-53): no Filer
    // loads it, so it can never be undone and its files are not held (QA 2026-11-04).
    let master = crate::clients::master_dir(root);
    batch_dirs(root)
        .iter()
        .filter(|d| **d != master)
        .flat_map(|d| undoable_in(root, d))
        .collect()
}

/// [`undoable_paths`] of one Intake folder's last batch.
fn undoable_in(root: &Path, dir: &Path) -> std::collections::HashSet<String> {
    let Some(lb) = load_last_batch(dir) else {
        return Default::default();
    };
    if lb.undone {
        return Default::default();
    }
    let mut paths: Vec<&Path> = Vec::new();
    for it in &lb.items {
        paths.push(&it.dst);
        paths.extend(it.copies.iter().map(|c| c.dst.as_path()));
    }
    paths.extend(lb.zip_entries.iter().map(|c| c.dst.as_path()));
    paths
        .into_iter()
        .filter_map(|p| p.strip_prefix(root).ok())
        .map(|r| r.to_string_lossy().replace('\\', "/").to_lowercase())
        .collect()
}

/// Cabinet-relative paths (lower case) of the files in Filer's last batch while that batch is
/// not in the catalog yet (still being filed, or filed and waiting for [`reconcile`]). Files
/// found in client folders at these paths are Intake's to record, not new files (PA-23).
pub fn unrecorded_paths(conn: &Connection, root: &Path) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for d in batch_dirs(root) {
        let Some(lb) = load_last_batch(&d) else {
            continue;
        };
        if lb.undone || (!lb.in_progress && recorded(conn, &lb.batch.to_string()).is_some()) {
            continue;
        }
        out.extend(undoable_in(root, &d));
    }
    out
}

pub fn reconcile(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
) -> Result<Reconciled, String> {
    // One reconcile at a time in this process: two threads (a tab reopened while a big batch is
    // still being hashed) would both see the batch unrecorded and record it twice (QA2 U9).
    // Other processes cannot share the catalog (the data folder's in-use lock).
    static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = ONE.lock().unwrap_or_else(|p| p.into_inner());
    // Every PC's last batch (Q-53), and the cabinet's own from before.
    let mut out = Reconciled::default();
    for dir in batch_dirs(root) {
        let r = reconcile_dir(conn, root, &dir, device_id)?;
        if r.recorded.is_some() {
            out.recorded = r.recorded;
        }
        if r.undone.is_some() {
            out.undone = r.undone;
        }
    }
    Ok(out)
}

fn reconcile_dir(
    conn: &mut Connection,
    root: &Path,
    dir: &Path,
    device_id: &str,
) -> Result<Reconciled, String> {
    let lb = load_last_batch(dir);
    // Batches before the last one that this catalog never saw (this PC was closed while
    // another PC filed several): read back from that PC's op log (owner 2026-11-04). Only in
    // the per-PC folders; the cabinet-level log predates them.
    let mut out = Reconciled::default();
    if *dir != crate::clients::master_dir(root) {
        let last = lb.as_ref().map(|l| l.batch);
        for (batch, items, backups) in missed_batches(conn, dir, last) {
            let (added, updated) = record(conn, root, dir, batch, &items, &backups, device_id)?;
            out.recorded = Some((batch.to_string(), added, updated));
        }
    }
    let Some(lb) = lb else {
        return Ok(out);
    };
    if lb.in_progress {
        return Ok(out); // still filing (or crashed mid-batch: Filer recovers)
    }
    let batch = lb.batch.to_string();
    match (recorded(conn, &batch), lb.undone) {
        (None, false) => {
            let backups: Vec<(std::path::PathBuf, std::path::PathBuf)> = lb
                .backups
                .iter()
                .map(|b| (b.original.clone(), b.kept.clone()))
                .collect();
            let (added, updated) = record(
                conn,
                root,
                dir,
                lb.batch,
                &from_last_batch(&lb),
                &backups,
                device_id,
            )?;
            Ok(Reconciled {
                recorded: Some((batch, added, updated)),
                ..Default::default()
            })
        }
        (Some((tx, false)), true) => {
            // Filer already moved the files back: run only the catalog inverse.
            vf_core::tx::undo(conn, &mut NoFileOps, &Actor::device(device_id), &tx)
                .map_err(|e| e.to_string())?;
            // The client folders it emptied again, for the other servers (PA-36).
            let filed = from_last_batch(&lb);
            crate::changes::note_paths(filed.iter().map(|f| f.dst.as_path()));
            Ok(Reconciled {
                undone: Some(batch),
                ..Default::default()
            })
        }
        // Recorded and in step, or undone before it was ever recorded: nothing to do.
        _ => Ok(out),
    }
}

/// A file a batch put in the cabinet: where, for which client (CSV ID), its size, and its
/// SHA-256 when the batch logged one.
pub(crate) struct FiledFile {
    /// A batch item (not a copy to a related client).
    primary: bool,
    dst: std::path::PathBuf,
    client_id: String,
    bytes: u64,
    sha256: Option<String>,
}

fn from_last_batch(lb: &LastBatch) -> Vec<FiledFile> {
    let mut out = Vec::new();
    for it in &lb.items {
        out.push(FiledFile {
            primary: true,
            dst: it.dst.clone(),
            client_id: it.client_id.clone(),
            bytes: it.bytes,
            sha256: None,
        });
        for c in &it.copies {
            out.push(FiledFile {
                primary: false,
                dst: c.dst.clone(),
                client_id: c.client_id.clone(),
                bytes: c.bytes,
                sha256: Some(c.sha256.clone()),
            });
        }
    }
    out
}

/// The batches in `dir`'s op log, oldest first, that are not recorded in this catalog, not
/// undone and not `last` (the last batch is read from `last_batch.json`), each with the files
/// it filed (its successful moves and copies).
fn missed_batches(conn: &Connection, dir: &Path, last: Option<uuid::Uuid>) -> Vec<MissedBatch> {
    use filer_intake::oplog::{Op, OpResult};
    let Ok(log) = oplog::read(&oplog::path(dir)) else {
        return vec![];
    };
    let known: std::collections::HashSet<String> = conn
        .prepare(
            "SELECT json_extract(record, '$.extra.filer_batch') FROM undo_record WHERE label = ?1",
        )
        .and_then(|mut st| {
            st.query_map([LABEL], |r| r.get::<_, Option<String>>(0))?
                .collect::<Result<Vec<_>, _>>()
        })
        .map(|v| v.into_iter().flatten().collect())
        .unwrap_or_default();
    let mut order: Vec<uuid::Uuid> = Vec::new();
    let mut undone = std::collections::HashSet::new();
    for r in &log.records {
        if r.op == Op::Undo {
            undone.insert(r.batch);
        }
        if !order.contains(&r.batch) {
            order.push(r.batch);
        }
    }
    order
        .into_iter()
        .filter(|b| Some(*b) != last && !undone.contains(b) && !known.contains(&b.to_string()))
        .map(|b| {
            let files: Vec<FiledFile> = log
                .records
                .iter()
                .filter(|r| r.batch == b && r.result == OpResult::Ok)
                .filter(|r| matches!(r.op, Op::Move | Op::CopyVerifyDelete | Op::Copy))
                .filter_map(|r| {
                    Some(FiledFile {
                        primary: r.op != Op::Copy,
                        dst: r.dst.clone()?,
                        client_id: r.client_id.clone()?,
                        bytes: r.bytes.unwrap_or(0),
                        sha256: r.sha256.clone(),
                    })
                })
                .collect();
            // Files overwritten by the batch, kept by Filer: (where it was, the kept copy).
            let backups: Vec<(std::path::PathBuf, std::path::PathBuf)> = log
                .records
                .iter()
                .filter(|r| r.batch == b && r.op == Op::Backup && r.result == OpResult::Ok)
                .filter_map(|r| Some((r.src.clone()?, r.dst.clone()?)))
                .collect();
            (b, files, backups)
        })
        .filter(|(_, f, _)| !f.is_empty())
        .collect()
}

/// A batch read back from an op log: its id, the files it filed, the overwritten files kept.
type MissedBatch = (
    uuid::Uuid,
    Vec<FiledFile>,
    Vec<(std::path::PathBuf, std::path::PathBuf)>,
);

/// Records one finished, not-undone batch. Returns (documents added, documents updated).
fn record(
    conn: &mut Connection,
    root: &Path,
    dir: &Path,
    batch: uuid::Uuid,
    files: &[FiledFile],
    backups: &[(std::path::PathBuf, std::path::PathBuf)],
    device_id: &str,
) -> Result<(usize, usize), String> {
    // A batch can create client folders and sentinels: catalog clients first.
    crate::rebuild::sync_clients(conn, root, device_id, "intake_clients")
        .map_err(|e| e.to_string())?;
    // CSV ID → (client id, client folder).
    let clients: HashMap<String, (String, Option<String>)> = conn
        .prepare("SELECT ext_id, id, folder_rel FROM client WHERE deleted_at IS NULL")
        .and_then(|mut st| {
            st.query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?
                .collect()
        })
        .map_err(|e| e.to_string())?;
    let doc_types: HashMap<String, String> = conn
        .prepare("SELECT id, name FROM doc_type WHERE deleted_at IS NULL")
        .and_then(|mut st| st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
        .map_err(|e| e.to_string())?;
    let hints = crate::hints::load(root);
    // Payer → client corrections to learn once the batch is recorded (P08 step 4).
    let mut learned: Vec<(String, String)> = Vec::new();

    // Every file the batch put in the cabinet: each item's destination and its copies.
    struct Filed<'a> {
        rel: String,
        client: Option<&'a (String, Option<String>)>,
        bytes: u64,
        sha256: Option<String>,
    }
    let filed: Vec<Filed> = files
        .iter()
        .map(|f| Filed {
            rel: rel(root, &f.dst),
            client: clients.get(&f.client_id),
            bytes: f.bytes,
            sha256: f.sha256.clone(),
        })
        .collect();

    let now = vf_core::now_rfc3339();
    let mut tx = Transaction::new(LABEL).extra(json!({ "filer_batch": batch.to_string() }));
    let (mut added, mut updated) = (0, 0);
    // What the client folders' docmeta.jsonl already say about these files: a batch another
    // server recorded first (several servers record every PC's batches) keeps that server's id
    // and record (QA).
    let folders: Vec<String> = filed
        .iter()
        .filter_map(|f| f.client.and_then(|c| c.1.clone()))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let recorded = crate::docmeta::Index::load(root, &folders);
    for f in &filed {
        let path = root.join(&f.rel);
        if !path.is_file() {
            continue; // a later batch or a person moved it; rebuild reconciles
        }
        // A rename on one volume carries no hash; hash what is in the cabinet now.
        let hash = match &f.sha256 {
            Some(h) => h.clone(),
            None => filer_intake::commit::sha256_file(&path)
                .map_err(|e| format!("cannot hash {}: {e}", path.display()))?,
        };
        let existing: Option<(String, String, i64)> = conn
            .query_row(
                "SELECT id, hash, size FROM document WHERE rel_path = ?1 AND deleted_at IS NULL",
                [&f.rel],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok();
        match existing {
            // Already in the catalog with this content (picked up from the folder before the
            // batch was recorded): nothing to change.
            Some((_, ref old_hash, _)) if *old_hash == hash => {}
            // Overwrite: the cataloged document now has new content; the old content is a
            // version (Filer kept the bytes in `_Vibe/overwritten/<batch>/`, Q-17).
            Some((id, old_hash, old_size)) => {
                tx = tx
                    .catalog(Stmt::new(
                        "INSERT OR IGNORE INTO version (doc_id, hash, size, created, created_by, device_id, reason)
                         VALUES (?1, ?2, ?3, ?4, 'intake', ?5, 'overwritten by intake')",
                        vec![
                            id.clone().into(),
                            old_hash.clone().into(),
                            old_size.into(),
                            now.clone().into(),
                            device_id.into(),
                        ],
                    ))
                    .catalog(Stmt::new(
                        "UPDATE document SET hash = ?2, size = ?3, device_id = ?4, updated_at = ?5, rev = rev + 1
                         WHERE id = ?1",
                        vec![
                            id.clone().into(),
                            hash.clone().into(),
                            (f.bytes as i64).into(),
                            device_id.into(),
                            now.clone().into(),
                        ],
                    ))
                    .catalog_undo(Stmt::new(
                        "UPDATE document SET hash = ?2, size = ?3, device_id = ?4, updated_at = ?5, rev = rev + 1
                         WHERE id = ?1",
                        vec![
                            id.clone().into(),
                            old_hash.into(),
                            old_size.into(),
                            device_id.into(),
                            now.clone().into(),
                        ],
                    ));
                tx = tx.catalog(sync_put(&id, &now));
                updated += 1;
            }
            None => {
                let prior = recorded.get_at(&f.rel, &hash);
                // Its recorded id when no row here has it (else a new one).
                let id = prior
                    .and_then(|r| r.id.clone())
                    .filter(|rid| {
                        conn.query_row("SELECT 1 FROM document WHERE id = ?1", [rid], |_| Ok(()))
                            .is_err()
                    })
                    .unwrap_or_else(vf_core::new_id);
                // What a Vibe intake processor knew about the file (barcode cover sheet, print
                // capture parser): doc type, tax year, source. Kept in docmeta.jsonl too, so
                // rebuild restores it (Q-22).
                let hint = hints.get(&hash);
                let doc_type = hint
                    .and_then(|h| h.doc_type.clone())
                    .filter(|t| doc_types.contains_key(t));
                let tax_year = match hint {
                    Some(h) if h.permanent => None,
                    // Read from the form: the year the person filed it under (its folder)
                    // wins; the reading keeps its own (Q-62).
                    Some(h) if h.origin == "recognized" && year_from_path(&f.rel).is_some() => {
                        year_from_path(&f.rel)
                    }
                    Some(Hint {
                        tax_year: Some(y), ..
                    }) => Some(i64::from(*y)),
                    _ => year_from_path(&f.rel),
                };
                let source = hint
                    .and_then(|h| h.source.clone())
                    .unwrap_or_else(|| "client".into());
                // What a form recognizer read (P08): kept with the document, and a correction
                // (filed to another client than proposed) teaches the payer's client.
                let extract = hint.and_then(|h| h.extract.clone());
                if let Some(x) = &extract {
                    let filed = f.client.map(|c| c.0.clone());
                    if let (Some(payer), Some(filed)) = (&x.payer_name, &filed)
                        && x.proposed_client.as_ref() != Some(filed)
                    {
                        learned.push((payer.clone(), filed.clone()));
                        tx = tx.audit(AuditEntry::new(
                            "extract_corrected",
                            Some(&id),
                            json!({ "form": x.form, "proposed": x.proposed_client, "filed": filed }),
                        ));
                    }
                }
                // Read from the form, not told (Q-61): flagged for a person to check, and kept
                // out of disposition until someone sets its doc type or year.
                let guessed: Vec<String> = if extract.is_some() {
                    vec![crate::docmeta::GUESSED_TAG.into()]
                } else {
                    vec![]
                };
                // Every filed document gets its docmeta record (its id included), so the other
                // servers and a rebuilt catalog know it by the same id (owner 2026-11-05); not
                // a second one when another server recorded this batch first.
                if prior.is_none() {
                    let meta = crate::docmeta::Meta {
                        id: id.clone(),
                        rel_path: f.rel.clone(),
                        hash: hash.clone(),
                        folder_rel: f.client.and_then(|c| c.1.clone()),
                        doc_type_name: doc_type.as_ref().and_then(|t| doc_types.get(t).cloned()),
                        doc_type: doc_type.clone(),
                        tax_year: tax_year.map(|y| y as i32),
                        status: "received".into(),
                        source: Some(source.clone()),
                        tags: guessed.clone(),
                    };
                    let (path, line) = crate::docmeta::record_line(root, &meta, &now, device_id);
                    tx = tx.file(FileOp::AppendLines {
                        path,
                        lines: vec![line],
                        undo_lines: vec![],
                    });
                }
                tx = tx
                    .catalog(Stmt::new(
                        "INSERT INTO document (id, client_id, tax_year, rel_path, hash, size, mime, status, source,
                                               filed_by, filed_at, device_id, updated_at, doc_type_id)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'received', ?10, 'intake', ?8, ?9, ?8, ?11)",
                        vec![
                            id.clone().into(),
                            f.client.map(|c| c.0.clone()).into(),
                            tax_year.into(),
                            f.rel.clone().into(),
                            hash.into(),
                            (f.bytes as i64).into(),
                            mime(&f.rel).into(),
                            now.clone().into(),
                            device_id.into(),
                            source.into(),
                            doc_type.into(),
                        ],
                    ))
                    .catalog(sync_put(&id, &now));
                for t in &guessed {
                    tx = tx.catalog(Stmt::new(
                        "INSERT OR IGNORE INTO document_tag (doc_id, tag) VALUES (?1, ?2)",
                        vec![id.clone().into(), t.clone().into()],
                    ));
                }
                if let Some(x) = extract {
                    tx = tx
                        .catalog(Stmt::new(
                            "INSERT OR REPLACE INTO document_extract (doc_id, hash, form, tax_year,
                               payer_name, payer_tin4, recipient_name, recipient_tin4, confidence,
                               proposed_client, read_at)
                             VALUES (?1, (SELECT hash FROM document WHERE id = ?1), ?2, ?3, ?4, ?5,
                               ?6, ?7, ?8, ?9, ?10)",
                            vec![
                                id.clone().into(),
                                x.form.into(),
                                tax_year.into(),
                                x.payer_name.into(),
                                x.payer_tin4.into(),
                                x.recipient_name.into(),
                                x.recipient_tin4.into(),
                                (f64::from(x.confidence) / 100.0).into(),
                                x.proposed_client.into(),
                                now.clone().into(),
                            ],
                        ))
                        .catalog_undo(Stmt::new(
                            "DELETE FROM document_extract WHERE doc_id = ?1",
                            vec![id.clone().into()],
                        ));
                }
                tx = tx.catalog_undo(Stmt::new(
                    "UPDATE document SET deleted_at = ?2, tombstone_reason = 'intake undone',
                           device_id = ?3, updated_at = ?2, rev = rev + 1
                         WHERE id = ?1",
                    vec![id.clone().into(), now.clone().into(), device_id.into()],
                ));
                added += 1;
            }
        }
    }

    // Filer's op log for this batch becomes audit rows (one per record).
    let ops = oplog::read(&oplog::path(dir))
        .map(|r| r.records)
        .unwrap_or_default();
    let mut audited = 0;
    for op in ops.iter().filter(|o| o.batch == batch) {
        let action = serde_json::to_value(op.op)
            .ok()
            .and_then(|v| v.as_str().map(|s| format!("intake_{s}")))
            .unwrap_or_else(|| "intake_op".into());
        let path_rel = |p: &Option<std::path::PathBuf>| p.as_ref().map(|p| rel(root, p));
        tx = tx.audit(AuditEntry::new(
            &action,
            None,
            json!({
                "filer_batch": batch.to_string(),
                "client": op.client_id,
                "src": op.src.as_ref().map(|p| p.display().to_string()),
                "dst": path_rel(&op.dst),
                "bytes": op.bytes,
                "sha256": op.sha256,
                "ok": op.result == OpResult::Ok,
                "error": op.error,
            }),
        ));
        audited += 1;
    }
    let moved = files.iter().filter(|f| f.primary).count();
    tx = tx.audit(AuditEntry::new(
        LABEL,
        None,
        json!({
            "filer_batch": batch.to_string(),
            "files": moved,
            "documents_added": added,
            "documents_updated": updated,
            "op_records": audited,
        }),
    ));
    // The only file operations are docmeta lines for hinted documents (Filer moved the files).
    crate::fileops::commit(conn, root, tx, device_id)?;
    // Filer's moves, for the other servers (PA-36).
    crate::changes::note_paths(files.iter().map(|f| f.dst.as_path()));
    for (payer, client) in learned {
        let _ = crate::aliases::learn(root, &payer, &client);
    }

    // Q-17: keep overwritten content as versions in `_Vibe/versions/` (content-addressed, with a
    // sidecar so rebuild restores the version rows). Filer's own copy stays for its undo.
    for (original, kept) in backups {
        let _ = keep_overwritten(root, original, kept, device_id, &now);
    }
    Ok((added, updated))
}

fn sync_put(id: &str, now: &str) -> Stmt {
    Stmt::new(
        "INSERT INTO sync_pending (doc_id, op, since) VALUES (?1, 'put', ?2)
         ON CONFLICT(doc_id, op) DO NOTHING",
        vec![id.into(), now.into()],
    )
}

/// Copies an overwritten file's kept bytes into `_Vibe/versions/<hash>` (once) and adds a
/// sidecar entry for the document path it was overwritten at.
fn keep_overwritten(
    root: &Path,
    original: &Path,
    kept: &Path,
    device_id: &str,
    now: &str,
) -> Result<(), String> {
    let hash = filer_intake::commit::sha256_file(kept).map_err(|e| e.to_string())?;
    let dst = crate::cabinet::version_path(root, &hash);
    if !dst.exists() {
        if let Some(d) = dst.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        filer_intake::commit::copy_verified(kept, &dst)?;
    }
    crate::cabinet::add_version_sidecar(
        root,
        &hash,
        crate::cabinet::VersionSidecar {
            rel_path: rel(root, original),
            created: now.into(),
            created_by: Some("intake".into()),
            device_id: device_id.into(),
            reason: Some("overwritten by intake".into()),
            // What overwrote it: the file at the document's path now.
            next: filer_intake::commit::sha256_file(original).ok(),
        },
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;

    use filer_core::clients::Client;
    use filer_intake::commit::{CommitInput, CommitItem, CommitReport, run_commit};
    use filer_intake::intake::Source;
    use filer_intake::plan::FolderAction;
    use vf_core::db::Storage;

    use super::*;

    /// Q-53: a PC's copies of the shared files go to the cabinet when its Filer saves them, and
    /// come in when the cabinet's change; every PC's batch folder is looked at.
    #[test]
    fn shared_files_go_both_ways() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("cab");
        let master = crate::clients::master_dir(&root);
        std::fs::create_dir_all(&master).unwrap();
        std::fs::write(master.join("rules.json"), "cabinet rules").unwrap();
        let (a, b) = (pc_dir(&root, "pc-a"), pc_dir(&root, "pc-b"));
        let (mut sa, mut sb) = (Shared::default(), Shared::default());
        sa.prepare(&root, &a).unwrap();
        sb.prepare(&root, &b).unwrap();
        assert_eq!(
            std::fs::read_to_string(a.join("rules.json")).unwrap(),
            "cabinet rules"
        );
        assert_eq!(sa.poll(&root, &a).unwrap(), SharedChanges::default());
        // A's Filer saves its rules: they reach the cabinet, then B.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(a.join("rules.json"), "rules from A").unwrap();
        assert_eq!(sa.poll(&root, &a).unwrap().to_cabinet, ["rules.json"]);
        assert_eq!(
            std::fs::read_to_string(master.join("rules.json")).unwrap(),
            "rules from A"
        );
        assert_eq!(sb.poll(&root, &b).unwrap().from_cabinet, ["rules.json"]);
        assert_eq!(
            std::fs::read_to_string(b.join("rules.json")).unwrap(),
            "rules from A"
        );
        assert_eq!(
            sa.poll(&root, &a).unwrap(),
            SharedChanges::default(),
            "A is in step"
        );
        // A's save right before a reload (the poll had not run): prepare sends it, not loses it.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(a.join("rules.json"), "rules saved just before reload").unwrap();
        sa.prepare(&root, &a).unwrap();
        assert_eq!(
            std::fs::read_to_string(master.join("rules.json")).unwrap(),
            "rules saved just before reload"
        );
        let dirs = batch_dirs(&root);
        assert_eq!(dirs, [master.clone(), a, b]);
    }

    /// A PC's look (theme, text size) in Filer's config.json stays on that PC: a save that
    /// changes only it is not sent, and settings coming in keep this PC's look (QA 2026-11-04).
    #[test]
    fn the_look_in_config_stays_on_its_pc() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("cab");
        let master = crate::clients::master_dir(&root);
        std::fs::create_dir_all(&master).unwrap();
        std::fs::write(
            master.join(CONFIG),
            r#"{"root":"X","display":{"zoom":1.0}}"#,
        )
        .unwrap();
        let (a, b) = (pc_dir(&root, "pc-a"), pc_dir(&root, "pc-b"));
        let (mut sa, mut sb) = (Shared::default(), Shared::default());
        sa.prepare(&root, &a).unwrap();
        sb.prepare(&root, &b).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(a.join(CONFIG), r#"{"root":"X","display":{"zoom":1.5}}"#).unwrap();
        assert_eq!(sa.poll(&root, &a).unwrap(), SharedChanges::default());
        assert_eq!(sb.poll(&root, &b).unwrap(), SharedChanges::default());
        // A real settings change travels; B keeps its own look.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(b.join(CONFIG), r#"{"root":"X","display":{"zoom":0.9}}"#).unwrap();
        sb.poll(&root, &b).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(a.join(CONFIG), r#"{"root":"Y","display":{"zoom":1.5}}"#).unwrap();
        assert_eq!(sa.poll(&root, &a).unwrap().to_cabinet, [CONFIG]);
        let m: serde_json::Value =
            serde_json::from_slice(&std::fs::read(master.join(CONFIG)).unwrap()).unwrap();
        assert_eq!(
            (m["root"].as_str(), m["display"]["zoom"].as_f64()),
            (Some("Y"), Some(1.0))
        );
        assert_eq!(sb.poll(&root, &b).unwrap().from_cabinet, [CONFIG]);
        let bb: serde_json::Value =
            serde_json::from_slice(&std::fs::read(b.join(CONFIG)).unwrap()).unwrap();
        assert_eq!(
            (bb["root"].as_str(), bb["display"]["zoom"].as_f64()),
            (Some("Y"), Some(0.9))
        );
    }
    use crate::cabinet::{self, CabinetFile};

    struct Fx {
        _t: tempfile::TempDir,
        root: PathBuf,
        inbox: PathBuf,
        conn: Connection,
        firm: uuid::Uuid,
    }

    fn fx() -> Fx {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("cabinet");
        let inbox = t.path().join("inbox");
        std::fs::create_dir_all(&inbox).unwrap();
        let firm = uuid::Uuid::new_v4();
        cabinet::create(
            &root,
            &CabinetFile {
                cabinet_id: vf_core::new_id(),
                firm_id: firm.to_string(),
                schema_version: vf_core::db::SCHEMA_VERSION,
                layout_version: 1,
                created: vf_core::now_rfc3339(),
                name: None,
            },
        )
        .unwrap();
        let conn = vf_core::db::open(&t.path().join("c.db"), Storage::Local).unwrap();
        Fx {
            _t: t,
            root,
            inbox,
            conn,
            firm,
        }
    }

    fn client(id: &str, name: &str) -> Client {
        Client {
            id: id.into(),
            uuid: uuid::Uuid::new_v4(),
            name: name.into(),
            extra: BTreeMap::new(),
        }
    }

    /// Files `names` from the inbox into the folder for `c` with Filer's own commit.
    fn file(f: &Fx, c: &Client, names: &[&str], overwrite: bool) -> CommitReport {
        file_from(f, c, names, overwrite, &crate::clients::master_dir(&f.root))
    }

    /// [`file`] from the Intake folder `data_dir` (a PC's own, Q-53).
    fn file_from(
        f: &Fx,
        c: &Client,
        names: &[&str],
        overwrite: bool,
        data_dir: &Path,
    ) -> CommitReport {
        let folder = f.root.join(format!("{} ({})", c.name, c.id));
        let action = if folder.exists() {
            FolderAction::Exists(folder.clone())
        } else {
            FolderAction::WillCreate(folder.clone())
        };
        let items = names
            .iter()
            .map(|name| {
                let src = f.inbox.join(name);
                if !src.exists() {
                    std::fs::write(&src, format!("content of {name}")).unwrap();
                }
                CommitItem {
                    src,
                    dest: folder.join("Income Tax/2025").join(name),
                    folder: action.clone(),
                    client: c.clone(),
                    overwrite,
                    source: Source::Inbox(f.inbox.clone()),
                    merged_from: vec![],
                    split_of: None,
                    copies: vec![],
                    kept_both: false,
                    also_dirs: vec![],
                }
            })
            .collect();
        let input = CommitInput {
            batch: uuid::Uuid::new_v4(),
            items,
            skipped: vec![],
            root: f.root.clone(),
            first_inbox: Some(f.inbox.clone()),
            data_dir: data_dir.to_path_buf(),
            firm_id: f.firm,
            template: filer_core::template::default_tree(),
            splits: vec![],
            force_copy: false,
        };
        let rep = run_commit(&input, &AtomicUsize::new(0));
        assert!(rep.warnings.is_empty(), "{:?}", rep.warnings);
        rep
    }

    fn n(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn w2(f: &Fx) -> PathBuf {
        f.root.join("Acme LLC (1001)/Income Tax/2025/w2.pdf")
    }

    /// Q-53: two PCs file at once, each from its own Intake folder: both batches are recorded
    /// (documents and audit rows), and one PC's Undo takes back only its own batch.
    #[test]
    fn two_pcs_filing_at_once_are_both_recorded() {
        let mut f = fx();
        let acme = client("1001", "Acme LLC");
        let (a, b) = (pc_dir(&f.root, "pc-a"), pc_dir(&f.root, "pc-b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let ra = file_from(&f, &acme, &["w2.pdf"], false, &a);
        let rb = file_from(&f, &acme, &["1099-int.pdf"], false, &b);
        reconcile(&mut f.conn, &f.root, "dev").unwrap();
        assert_eq!(
            n(
                &f.conn,
                "SELECT count(*) FROM document WHERE filed_by = 'intake' AND deleted_at IS NULL"
            ),
            2,
            "both batches recorded"
        );
        for batch in [ra.last_batch.batch, rb.last_batch.batch] {
            assert!(recorded(&f.conn, &batch.to_string()).is_some());
        }
        // PC A's Undo: its own batch only.
        filer_intake::undo::run_undo(&a, ra.last_batch, false);
        reconcile(&mut f.conn, &f.root, "dev").unwrap();
        assert_eq!(
            n(
                &f.conn,
                "SELECT count(*) FROM document WHERE filed_by = 'intake' AND deleted_at IS NULL"
            ),
            1
        );
        assert!(
            f.root
                .join("Acme LLC (1001)/Income Tax/2025/1099-int.pdf")
                .is_file()
        );
    }

    /// A PC files several batches while this catalog does not look: every one is recorded
    /// from that PC's op log, with Intake as the filer, not only the last (owner 2026-11-04);
    /// an undone one is not.
    #[test]
    fn every_batch_another_pc_filed_is_recorded() {
        let mut f = fx();
        let acme = client("1001", "Acme LLC");
        let a = pc_dir(&f.root, "pc-a");
        std::fs::create_dir_all(&a).unwrap();
        let r1 = file_from(&f, &acme, &["w2.pdf"], false, &a);
        let r2 = file_from(&f, &acme, &["1099-int.pdf"], false, &a);
        filer_intake::undo::run_undo(&a, r2.last_batch.clone(), false);
        let r3 = file_from(&f, &acme, &["1099-div.pdf"], false, &a);
        reconcile(&mut f.conn, &f.root, "dev").unwrap();
        let filed: Vec<String> = f
            .conn
            .prepare(
                "SELECT rel_path FROM document WHERE filed_by = 'intake' AND deleted_at IS NULL
                 ORDER BY rel_path",
            )
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            filed,
            [
                "Acme LLC (1001)/Income Tax/2025/1099-div.pdf",
                "Acme LLC (1001)/Income Tax/2025/w2.pdf"
            ],
            "the first and the last batch, not the undone one"
        );
        assert!(recorded(&f.conn, &r1.last_batch.batch.to_string()).is_some());
        assert!(recorded(&f.conn, &r2.last_batch.batch.to_string()).is_none());
        assert!(recorded(&f.conn, &r3.last_batch.batch.to_string()).is_some());
        // Again: nothing more.
        let again = reconcile(&mut f.conn, &f.root, "dev").unwrap();
        assert!(!again.changed());
    }

    /// A Filer batch becomes one Transaction: document + sync rows, audit rows from ops.jsonl,
    /// an undo record keyed by the Filer batch; reconcile is idempotent.
    #[test]
    fn filer_batch_is_recorded_once() {
        let mut f = fx();
        let acme = client("1001", "Acme LLC");
        let rep = file(&f, &acme, &["w2.pdf", "1099-int.pdf"], false);
        let r = reconcile(&mut f.conn, &f.root, "dev").unwrap();
        let batch = rep.last_batch.batch.to_string();
        assert_eq!(r.recorded, Some((batch, 2, 0)));

        let acme_id: String = f
            .conn
            .query_row("SELECT id FROM client WHERE ext_id = '1001'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(acme_id, acme.uuid.to_string(), "bound by Filer's sentinel");
        let sql = format!(
            "SELECT count(*) FROM document WHERE client_id = '{acme_id}' AND tax_year = 2025
             AND status = 'received' AND filed_by = 'intake' AND deleted_at IS NULL"
        );
        assert_eq!(n(&f.conn, &sql), 2);
        let hash: String = f
            .conn
            .query_row(
                "SELECT hash FROM document WHERE rel_path LIKE '%/w2.pdf'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hash, filer_intake::commit::sha256_file(&w2(&f)).unwrap());
        assert_eq!(
            n(
                &f.conn,
                "SELECT count(*) FROM sync_pending WHERE op = 'put'"
            ),
            2
        );
        assert!(
            n(
                &f.conn,
                "SELECT count(*) FROM audit WHERE action LIKE 'intake_%'"
            ) >= 3
        );
        assert_eq!(
            n(
                &f.conn,
                "SELECT count(*) FROM undo_record WHERE label = 'intake_batch'"
            ),
            1
        );

        // Again: nothing more.
        assert!(!reconcile(&mut f.conn, &f.root, "dev").unwrap().changed());
        assert_eq!(
            n(
                &f.conn,
                "SELECT count(*) FROM document WHERE deleted_at IS NULL"
            ),
            2
        );
    }

    /// PHASES P03 done-when: exact duplicates are caught on intake. A second Filer batch filing
    /// content the cabinet already holds is found right after it is recorded.
    #[test]
    fn duplicate_content_is_caught_after_filing() {
        let mut f = fx();
        let acme = client("1001", "Acme LLC");
        file(&f, &acme, &["w2.pdf"], false);
        reconcile(&mut f.conn, &f.root, "dev").unwrap();
        // The same W-2 arrives again under another name.
        std::fs::write(f.inbox.join("w2 again.pdf"), "content of w2.pdf").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let before = vf_core::now_rfc3339();
        file(&f, &acme, &["w2 again.pdf", "k1.pdf"], false);
        reconcile(&mut f.conn, &f.root, "dev").unwrap();
        let d = crate::dupes::filed_since(&f.conn, &before).unwrap();
        assert_eq!(d.len(), 1, "{d:?}");
        assert!(d[0].new_rel.ends_with("/w2 again.pdf"));
        assert!(d[0].old_rel.ends_with("/w2.pdf"));
    }

    /// Filer's own Undo moves the files back; reconcile then undoes the catalog side only.
    #[test]
    fn filer_undo_tombstones_the_batch() {
        let mut f = fx();
        let acme = client("1001", "Acme LLC");
        let rep = file(&f, &acme, &["w2.pdf"], false);
        reconcile(&mut f.conn, &f.root, "dev").unwrap();
        let undo = filer_intake::undo::run_undo(
            &crate::clients::master_dir(&f.root),
            rep.last_batch.clone(),
            false,
        );
        assert!(undo.last_batch.undone);
        assert!(f.inbox.join("w2.pdf").exists(), "Filer put the file back");

        let r = reconcile(&mut f.conn, &f.root, "dev").unwrap();
        assert_eq!(r.undone, Some(rep.last_batch.batch.to_string()));
        assert_eq!(
            n(
                &f.conn,
                "SELECT count(*) FROM document WHERE deleted_at IS NULL"
            ),
            0
        );
        assert_eq!(
            n(
                &f.conn,
                "SELECT count(*) FROM document WHERE tombstone_reason = 'intake undone'"
            ),
            1
        );
        assert!(!reconcile(&mut f.conn, &f.root, "dev").unwrap().changed());
    }

    /// A hint from a Vibe intake processor (barcode cover sheet, print capture) sets the doc
    /// type, year and source of the document it names by hash, and is kept in docmeta.jsonl.
    #[test]
    fn hints_set_doc_type_year_and_source() {
        let mut f = fx();
        crate::doctypes::seed_if_missing(&f.root).unwrap();
        crate::doctypes::sync(&mut f.conn, &f.root, "dev", "seed").unwrap();
        let w2_type = crate::doctypes::load(&f.root).unwrap().unwrap()[0].clone();
        assert_eq!(w2_type.name, "W-2");
        std::fs::write(f.inbox.join("w2.pdf"), "content of w2.pdf").unwrap();
        let hash = filer_intake::commit::sha256_file(&f.inbox.join("w2.pdf")).unwrap();
        let mut h = crate::hints::Hint::new(&hash, "w2.pdf", "print_capture:test", "dev");
        h.doc_type = Some(w2_type.id.clone());
        h.tax_year = Some(2024);
        h.source = Some("firm".into());
        crate::hints::append(&f.root, &h).unwrap();

        let acme = client("1001", "Acme LLC");
        file(&f, &acme, &["w2.pdf", "other.pdf"], false);
        reconcile(&mut f.conn, &f.root, "dev").unwrap();
        let row = |name: &str| -> (Option<String>, Option<i64>, String) {
            f.conn
                .query_row(
                    "SELECT doc_type_id, tax_year, source FROM document WHERE rel_path LIKE ?1",
                    [format!("%/{name}")],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap()
        };
        assert_eq!(
            row("w2.pdf"),
            (Some(w2_type.id.clone()), Some(2024), "firm".into())
        );
        assert_eq!(row("other.pdf"), (None, Some(2025), "client".into()));
        let meta =
            std::fs::read_to_string(f.root.join("Acme LLC (1001)/_Vibe/docmeta.jsonl")).unwrap();
        // Every filed document has its record (with its id, shared between servers).
        assert_eq!(meta.lines().count(), 2);
        assert!(meta.contains(&w2_type.id) && meta.contains("\"firm\""));
        assert!(meta.lines().all(|l| l.contains("\"id\":")));
    }

    /// P08 steps 2 and 4: a recognized form's reading is kept with the document; filed to a
    /// different client than proposed, the payer's client is learned for next time.
    #[test]
    fn recognized_forms_keep_their_reading_and_corrections_are_learned() {
        let mut f = fx();
        std::fs::write(f.inbox.join("1099.pdf"), "content of 1099.pdf").unwrap();
        let hash = filer_intake::commit::sha256_file(&f.inbox.join("1099.pdf")).unwrap();
        let mut h = crate::hints::Hint::new(&hash, "1099.pdf", "recognized", "dev");
        // The form said 2024; it is filed under Income Tax/2025: the folder's year wins (Q-62).
        h.tax_year = Some(2024);
        h.extract = Some(crate::hints::Recognized {
            form: "1099-nec".into(),
            payer_name: Some("Summit Brokerage LLC".into()),
            payer_tin4: Some("2222".into()),
            recipient_name: Some("Bell Ranch".into()),
            recipient_tin4: None,
            proposed_client: Some("someone-else".into()),
            confidence: 70,
            copies: vec![],
        });
        crate::hints::append(&f.root, &h).unwrap();
        let bell = client("2002", "Bell Ranch Inc");
        file(&f, &bell, &["1099.pdf"], false);
        reconcile(&mut f.conn, &f.root, "dev").unwrap();
        let (form, payer, conf): (String, String, f64) = f
            .conn
            .query_row(
                "SELECT form, payer_name, confidence FROM document_extract",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (form.as_str(), payer.as_str()),
            ("1099-nec", "Summit Brokerage LLC")
        );
        assert!((conf - 0.7).abs() < 1e-6);
        assert_eq!(
            n(
                &f.conn,
                "SELECT tax_year FROM document WHERE rel_path LIKE '%1099.pdf'"
            ),
            2025
        );
        // Read, not told: flagged for a person to check (Q-61).
        assert_eq!(
            n(
                &f.conn,
                "SELECT count(*) FROM document_tag WHERE tag = 'import:guessed'"
            ),
            1
        );
        let learned = crate::aliases::load(&f.root);
        assert_eq!(learned.len(), 1);
        assert_eq!(learned[0].client_id, bell.uuid.to_string());
        assert_eq!(
            n(
                &f.conn,
                "SELECT count(*) FROM audit WHERE action = 'extract_corrected'"
            ),
            1
        );
    }

    /// Overwrite: the cataloged document gets the new content; the old content becomes a
    /// version in `_Vibe/versions/` with a sidecar (Q-17).
    #[test]
    fn overwrite_keeps_the_old_content_as_a_version() {
        let mut f = fx();
        let acme = client("1001", "Acme LLC");
        file(&f, &acme, &["w2.pdf"], false);
        reconcile(&mut f.conn, &f.root, "dev").unwrap();
        let old = filer_intake::commit::sha256_file(&w2(&f)).unwrap();
        std::fs::write(f.inbox.join("w2.pdf"), "corrected W-2").unwrap();
        file(&f, &acme, &["w2.pdf"], true);
        let r = reconcile(&mut f.conn, &f.root, "dev").unwrap();
        assert_eq!(r.recorded.map(|(_, a, u)| (a, u)), Some((0, 1)));
        assert_eq!(n(&f.conn, "SELECT count(*) FROM version"), 1);
        assert!(cabinet::version_path(&f.root, &old).is_file());
        assert!(
            cabinet::version_path(&f.root, &old)
                .with_extension("json")
                .is_file()
        );
        assert_eq!(
            n(
                &f.conn,
                "SELECT count(*) FROM document WHERE deleted_at IS NULL"
            ),
            1
        );
    }
}
