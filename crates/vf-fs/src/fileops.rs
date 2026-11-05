//! [`vf_core::tx::FileOps`] backed by the vendored Filer Desktop primitives
//! (`filer_intake::commit::{transfer, copy_verified, sha256_file}`), so every Vibe Files move has
//! Filer's guarantees: never replace an existing file, rename on one volume, copy + SHA-256 verify
//! + delete across volumes. See CLAUDE.md §Document moves and `crates/filer-intake/VENDORED.md`.

use std::path::{Path, PathBuf};

use filer_intake::commit::{self, Method};
use vf_core::tx::{AppliedOp, FileOp, FileOps};

pub struct FilerFileOps {
    /// Folders writes may land in (cabinet root, inboxes). Canonical.
    allowed: Vec<PathBuf>,
    /// Take Filer's copy-verify-delete path even on one volume (tests).
    pub force_copy: bool,
}

impl FilerFileOps {
    /// `allowed`: the cabinet root and the inboxes. Each must exist.
    pub fn new(allowed: &[&Path]) -> std::io::Result<Self> {
        Ok(Self {
            allowed: allowed
                .iter()
                .map(std::fs::canonicalize)
                .collect::<std::io::Result<_>>()?,
            force_copy: false,
        })
    }

    /// Creates the missing folders above `file` (outermost first), refusing anything outside the
    /// allowed roots. Returns the folders it created.
    fn make_parents(&self, file: &Path) -> Result<Vec<PathBuf>, String> {
        let parent = file
            .parent()
            .ok_or_else(|| format!("{} has no folder", file.display()))?;
        self.make_dirs(parent)
    }

    fn make_dirs(&self, dir: &Path) -> Result<Vec<PathBuf>, String> {
        let mut missing = Vec::new();
        let mut cur = dir;
        while !cur.exists() {
            missing.push(cur.to_path_buf());
            cur = cur
                .parent()
                .ok_or_else(|| format!("{} has no existing folder above it", dir.display()))?;
        }
        self.check_inside(cur, dir)?;
        let mut created = Vec::new();
        for d in missing.iter().rev() {
            if let Err(e) = std::fs::create_dir(d) {
                remove_empty(&created);
                return Err(format!("cannot create {}: {e}", d.display()));
            }
            created.push(d.clone());
        }
        Ok(created)
    }

    /// `existing` is the deepest existing folder of `target`; it must be inside an allowed root.
    fn check_inside(&self, existing: &Path, target: &Path) -> Result<(), String> {
        let c = std::fs::canonicalize(existing)
            .map_err(|e| format!("cannot open {}: {e}", existing.display()))?;
        if self.allowed.iter().any(|a| c.starts_with(a)) {
            Ok(())
        } else {
            Err(format!(
                "{} is outside the cabinet and inboxes",
                target.display()
            ))
        }
    }
}

/// Commits `tx` against the cabinet at `root` with Filer's file primitives, as this device.
pub fn commit(
    conn: &mut vf_core::db::Connection,
    root: &Path,
    tx: vf_core::tx::Transaction,
    device_id: &str,
) -> Result<vf_core::tx::Committed, String> {
    let mut fs = FilerFileOps::new(&[root]).map_err(|e| format!("cannot open the cabinet: {e}"))?;
    let done = tx
        .commit(conn, &mut fs, &vf_core::tx::Actor::device(device_id))
        .map_err(|e| e.to_string())?;
    crate::changes::note(done.applied.iter().map(|a| &a.op));
    Ok(done)
}

/// Like [`commit`], with file ops allowed under each of `allowed` (the cabinet and an inbox).
pub fn commit_in(
    conn: &mut vf_core::db::Connection,
    allowed: &[&Path],
    tx: vf_core::tx::Transaction,
    device_id: &str,
) -> Result<vf_core::tx::Committed, String> {
    let mut fs = FilerFileOps::new(allowed).map_err(|e| format!("cannot open the cabinet: {e}"))?;
    let done = tx
        .commit(conn, &mut fs, &vf_core::tx::Actor::device(device_id))
        .map_err(|e| e.to_string())?;
    crate::changes::note(done.applied.iter().map(|a| &a.op));
    Ok(done)
}

/// Undoes the transaction `tx_id` against the cabinet at `root`, as this device.
pub fn undo(
    conn: &mut vf_core::db::Connection,
    root: &Path,
    tx_id: &str,
    device_id: &str,
) -> Result<(), String> {
    let mut fs = FilerFileOps::new(&[root]).map_err(|e| format!("cannot open the cabinet: {e}"))?;
    // The folders it touches, for the other servers (PA-36).
    let ops: Vec<vf_core::tx::FileOp> = conn
        .query_row(
            "SELECT record FROM undo_record WHERE tx_id = ?1",
            [tx_id],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .and_then(|r| serde_json::from_str::<vf_core::tx::UndoRecord>(&r).ok())
        .map(|r| r.applied.into_iter().map(|a| a.op).collect())
        .unwrap_or_default();
    vf_core::tx::undo(conn, &mut fs, &vf_core::tx::Actor::device(device_id), tx_id)
        .map(|_| ())
        .map_err(|e| e.to_string())?;
    crate::changes::note(&ops);
    Ok(())
}

/// Transactions the undo stack leaves out (P05 step 4): Intake batches (Filer's own Undo),
/// work done in the background (OCR, rebuild), catalog rows that follow files edited elsewhere
/// (client master, doc types), records with nothing to undo, and emptying the trash (the
/// bytes are gone).
pub const NOT_ON_UNDO_STACK: &[&str] = &[
    crate::intake::LABEL,
    "intake batch",
    "rebuild",
    crate::trash::PURGE_LABEL,
    "ocr_text_layer",
    "doc_types_seed",
    "doc_types_edit",
    "retention_seed",
    "retention_edit",
    "seed",
    "duplicate_kept",
    "clients_edit",
    "clients_import",
    "intake_clients",
    "external_modify",
    crate::trash::PIN_LABEL,
    crate::holds::LABEL,
    crate::disposition::DECIDE_LABEL,
    crate::disposition::RUN_LABEL,
    // The folder import has its own Undo (both of its Transactions and a rebuild together).
    crate::import::LABEL,
    crate::import::META_LABEL,
    // So has the cabinet merge (every client's Transaction together).
    crate::merge::LABEL,
    crate::merge::HISTORY_LABEL,
    // B2 sync (P11) applies other workstations' work and records exchanges: not this login's.
    "sync_push",
    "sync_pull",
    "sync_restore",
    // Records only, or done in the background (QA 2026-11-04): Ctrl+Z never lands on them.
    "adopt",
    "print",
    "device_removed",
    "template_edit",
];

/// This Windows login's undo stack: its last 20 undoable transactions, newest first.
pub fn undo_stack(
    conn: &vf_core::db::Connection,
    device_id: &str,
) -> Result<Vec<vf_core::tx::Undoable>, String> {
    vf_core::tx::undo_stack(
        conn,
        &vf_core::tx::Actor::device(device_id),
        NOT_ON_UNDO_STACK,
    )
    .map_err(|e| e.to_string())
}

/// What [`recover`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Recovered {
    /// Transactions found half done.
    pub transactions: usize,
    /// File changes put back.
    pub reversed: usize,
    /// Changes that could not be put back (described; the files stay where they are).
    pub problems: Vec<String>,
}

/// Leftover `~vfd-partial-<id>-<name>` copies of `dst` (a copy cut short).
fn remove_partials(dst: &Path) -> usize {
    let (Some(dir), Some(name)) = (dst.parent(), dst.file_name()) else {
        return 0;
    };
    let suffix = format!("-{}", name.to_string_lossy());
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.starts_with("~vfd-partial-") && n.ends_with(&suffix)
        })
        .filter(|e| std::fs::remove_file(e.path()).is_ok())
        .count()
}

/// Puts back the file changes of transactions the process died in the middle of (P13 step 2):
/// each one's `tx_journal` row lists its ops and whether their targets existed before. Their
/// catalog changes never committed. An undo that never committed has its reversals applied
/// again. Run at open (with the data folder locked to this copy of the program), before
/// anything else touches the cabinet.
///
/// A journal row means its transaction never committed (the commit deletes the row in the same
/// SQLite transaction); the data folder is held by one copy of the program, so no other one is
/// mid-change. Every step checks what is on disk now: a copy is removed only if it is still the
/// same content, a sentinel only if it still names the same client.
/// Where the running copies of Vibe Files using this catalog hold their `live/<id>.lock`.
static LIVE_DIR: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// This running copy, for as long as it holds its catalog open: a lock file held with no
/// sharing beside the catalog, whose name is this copy's journal owner id. Dropped: removed.
pub struct Live {
    _file: std::fs::File,
    path: PathBuf,
}

impl Drop for Live {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Marks this process as running on the catalog in `catalog_dir` (several sessions of one
/// server share a catalog, PA-34): its journal rows carry its id, and [`recover`] in another
/// session leaves them alone while it runs. `None` when the folder cannot be written (rows
/// then carry no owner and are recovered as before).
pub fn hold_live(catalog_dir: &Path) -> Option<Live> {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = catalog_dir.join("live");
    std::fs::create_dir_all(&dir).ok()?;
    let id = vf_core::new_id();
    let path = dir.join(format!("{id}.lock"));
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(&path)
        .ok()?;
    vf_core::tx::set_journal_owner(Some(id));
    if let Ok(mut d) = LIVE_DIR.lock() {
        *d = Some(dir);
    }
    Some(Live { _file: file, path })
}

/// Whether the copy `owner` is still running (its lock file is held). A lock file nobody holds
/// is left over from a crash and is removed.
fn still_running(owner: &str) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    if vf_core::tx::journal_owner().as_deref() == Some(owner) {
        // This copy's own rows: it is starting, so none of them is in progress.
        return false;
    }
    let Some(dir) = LIVE_DIR.lock().ok().and_then(|d| d.clone()) else {
        return false;
    };
    let path = dir.join(format!("{owner}.lock"));
    if !path.exists() {
        return false;
    }
    match std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&path)
    {
        Ok(f) => {
            drop(f);
            let _ = std::fs::remove_file(&path);
            false
        }
        Err(_) => true,
    }
}

/// Journal rows older than `secs` whose copy is no longer running: changes cut short that
/// recovery has not put back (the startup check).
pub fn interrupted(conn: &vf_core::db::Connection, secs: i64) -> Result<usize, String> {
    let before = vf_core::rfc3339_secs_ago(secs);
    Ok(vf_core::tx::journal(conn)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|e| e.ts < before && !e.owner.as_deref().is_some_and(still_running))
        .count())
}

pub fn recover(conn: &vf_core::db::Connection, root: &Path) -> Result<Recovered, String> {
    let mut out = Recovered::default();
    let same = |a: &Path, b: &Path| -> bool {
        matches!((commit::sha256_file(a), commit::sha256_file(b)), (Ok(x), Ok(y)) if x == y)
    };
    for e in vf_core::tx::journal(conn).map_err(|e| e.to_string())? {
        // Another session's change in progress on a shared catalog: not interrupted.
        if e.owner.as_deref().is_some_and(still_running) {
            continue;
        }
        out.transactions += 1;
        if e.ops.iter().any(|j| j.undo) {
            // An undo cut short: its reversals that landed are applied again, oldest first.
            let mut fs = FilerFileOps::new(&[root]).map_err(|e| e.to_string())?;
            for j in &e.ops {
                // A copy-back cut short leaves a partial beside either end.
                if let FileOp::Move { src, dst } | FileOp::Copy { src, dst } = &j.op {
                    remove_partials(src);
                    remove_partials(dst);
                }
                // Undoing a move into the version store copied the bytes back and kept the
                // stored ones: applying it again removes the copy (when it is that content).
                if let FileOp::Move { src, dst } = &j.op
                    && in_versions(dst)
                    && src.is_file()
                    && dst.is_file()
                {
                    if same(src, dst) {
                        match std::fs::remove_file(src) {
                            Ok(()) => out.reversed += 1,
                            Err(err) => out
                                .problems
                                .push(format!("cannot remove {}: {err}", src.display())),
                        }
                    }
                    continue;
                }
                let again = match &j.op {
                    FileOp::Move { src, dst } => src.exists() && !dst.exists(),
                    FileOp::Copy { src, dst } => src.exists() && !dst.exists(),
                    FileOp::AppendLines {
                        path, undo_lines, ..
                    } => {
                        let tail: String = undo_lines.iter().map(|l| format!("{l}\n")).collect();
                        !tail.is_empty()
                            && std::fs::read_to_string(path).is_ok_and(|t| t.ends_with(&tail))
                    }
                    FileOp::WriteSentinel { folder, .. } => {
                        !filer_intake::sentinel::sentinel_path(folder).exists()
                    }
                    FileOp::Mkdir { .. } => false,
                    // The undo put the folder back: take it away again.
                    FileOp::Rmdir { path } => path.is_dir(),
                };
                if again {
                    match fs.apply(&j.op) {
                        Ok(_) => out.reversed += 1,
                        Err(err) => out.problems.push(err),
                    }
                }
            }
            vf_core::tx::clear_journal(conn, &e.tx_id).map_err(|e| e.to_string())?;
            continue;
        }
        for j in e.ops.iter().rev() {
            let r: Result<bool, String> = match &j.op {
                FileOp::Move { src, dst } => {
                    remove_partials(dst);
                    // By what is on disk now, not `existed`: a swap moves the old file out and
                    // the new one into the same path (which existed when it was planned).
                    if dst.exists() && !src.exists() {
                        commit::transfer(dst, src, false).map(|_| true)
                    } else if dst.is_file() && src.is_file() && same(src, dst) {
                        // A copy across volumes finished but the source was not removed yet.
                        std::fs::remove_file(dst)
                            .map(|()| true)
                            .map_err(|err| format!("cannot remove {}: {err}", dst.display()))
                    } else {
                        Ok(false)
                    }
                }
                FileOp::Copy { src, dst } => {
                    remove_partials(dst);
                    if !j.existed && dst.is_file() && same(src, dst) {
                        std::fs::remove_file(dst)
                            .map(|()| true)
                            .map_err(|err| format!("cannot remove {}: {err}", dst.display()))
                    } else {
                        Ok(false)
                    }
                }
                FileOp::Mkdir { path } => Ok(!j.existed && std::fs::remove_dir(path).is_ok()),
                FileOp::Rmdir { path } => {
                    if j.existed && !path.exists() {
                        std::fs::create_dir(path)
                            .map(|()| true)
                            .map_err(|err| format!("cannot create {}: {err}", path.display()))
                    } else {
                        Ok(false)
                    }
                }
                FileOp::AppendLines {
                    path,
                    lines,
                    undo_lines,
                } => {
                    let mut tail = String::new();
                    for l in lines {
                        tail.push_str(l);
                        tail.push('\n');
                    }
                    let landed = std::fs::read_to_string(path)
                        .map(|t| !tail.is_empty() && t.ends_with(&tail))
                        .unwrap_or(false);
                    if landed {
                        append(path, undo_lines).map(|()| true)
                    } else {
                        Ok(false)
                    }
                }
                FileOp::WriteSentinel {
                    folder, client_id, ..
                } => {
                    let p = filer_intake::sentinel::sentinel_path(folder);
                    let ours = filer_intake::sentinel::read_sentinel(folder)
                        .ok()
                        .flatten()
                        .is_some_and(|s| s.client_id.to_string() == *client_id);
                    Ok(!j.existed && ours && std::fs::remove_file(&p).is_ok())
                }
            };
            match r {
                Ok(true) => out.reversed += 1,
                Ok(false) => {}
                Err(err) => out.problems.push(err),
            }
        }
        vf_core::tx::clear_journal(conn, &e.tx_id).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// Removes folders this op created, innermost first, only while they are empty.
fn remove_empty(created: &[PathBuf]) {
    for d in created.iter().rev() {
        if std::fs::remove_dir(d).is_err() {
            break;
        }
    }
}

/// Appends one line per item, flushed to disk.
fn append(path: &Path, lines: &[String]) -> Result<(), String> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let mut buf = String::new();
    // A last line cut short (a crash, a share dropping) would merge with the first new one.
    if std::fs::metadata(path).is_ok_and(|m| m.len() > 0) && !ends_with_newline(path) {
        buf.push('\n');
    }
    for l in lines {
        buf.push_str(l);
        buf.push('\n');
    }
    f.write_all(buf.as_bytes())
        .and_then(|()| f.sync_all())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Whether `p` is in a cabinet's version store (`_Vibe/versions/`).
fn in_versions(p: &Path) -> bool {
    let s = p.to_string_lossy().replace('\\', "/");
    s.contains(&format!(
        "/{}/{}/",
        crate::cabinet::VIBE_DIR,
        crate::cabinet::VERSIONS_DIR
    ))
}

fn ends_with_newline(path: &Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return true;
    };
    let mut b = [0u8; 1];
    f.seek(SeekFrom::End(-1)).is_ok() && f.read_exact(&mut b).is_ok() && b[0] == b'\n'
}

fn method_name(m: Method) -> &'static str {
    match m {
        Method::Move => "move",
        Method::CopyVerifyDelete => "copy_verify_delete",
    }
}

fn size(p: &Path) -> Option<u64> {
    std::fs::metadata(p).ok().map(|m| m.len())
}

impl FileOps for FilerFileOps {
    fn apply(&mut self, op: &FileOp) -> Result<AppliedOp, String> {
        match op {
            FileOp::Move { src, dst } | FileOp::Copy { src, dst } => {
                if dst.exists() {
                    return Err(format!("{} already exists", dst.display()));
                }
                let created = self.make_parents(dst)?;
                let res = match op {
                    FileOp::Move { .. } => commit::transfer(src, dst, self.force_copy)
                        .map(|(m, bytes, sha)| (method_name(m), bytes, sha)),
                    _ => commit::copy_verified(src, dst).map(|(b, sha)| ("copy", b, Some(sha))),
                };
                match res {
                    Ok((method, bytes, sha256)) => Ok(AppliedOp {
                        op: op.clone(),
                        bytes,
                        sha256,
                        method: method.into(),
                        created_dirs: created,
                    }),
                    Err(e) => {
                        remove_empty(&created);
                        Err(e)
                    }
                }
            }
            FileOp::Mkdir { path } => Ok(AppliedOp {
                op: op.clone(),
                bytes: 0,
                sha256: None,
                method: "mkdir".into(),
                created_dirs: self.make_dirs(path)?,
            }),
            FileOp::Rmdir { path } => {
                self.check_inside(path, path)?;
                // remove_dir refuses a folder that holds anything (never remove_dir_all).
                std::fs::remove_dir(path)
                    .map_err(|e| format!("cannot remove the folder {}: {e}", path.display()))?;
                Ok(AppliedOp {
                    op: op.clone(),
                    bytes: 0,
                    sha256: None,
                    method: "rmdir".into(),
                    created_dirs: vec![],
                })
            }
            FileOp::AppendLines { path, lines, .. } => {
                let created = self.make_parents(path)?;
                if let Err(e) = append(path, lines) {
                    remove_empty(&created);
                    return Err(e);
                }
                Ok(AppliedOp {
                    op: op.clone(),
                    bytes: lines.iter().map(|l| l.len() as u64 + 1).sum(),
                    sha256: None,
                    method: "append".into(),
                    // Never removed on undo: undo appends, the file stays.
                    created_dirs: vec![],
                })
            }
            FileOp::WriteSentinel {
                root,
                folder,
                client_id,
                ext_id,
                name,
                firm_id,
            } => {
                self.check_inside(folder, folder)?;
                let parse = |s: &str, what: &str| {
                    uuid::Uuid::parse_str(s).map_err(|e| format!("{what} {s} is not a uuid: {e}"))
                };
                let client = filer_core::clients::Client {
                    id: ext_id.clone(),
                    uuid: parse(client_id, "client id")?,
                    name: name.clone(),
                    extra: Default::default(),
                };
                let vibe = folder.join(crate::cabinet::VIBE_DIR);
                let created_dirs = if vibe.exists() { vec![] } else { vec![vibe] };
                filer_intake::sentinel::write_sentinel(
                    root,
                    folder,
                    &client,
                    parse(firm_id, "firm id")?,
                )?;
                let p = filer_intake::sentinel::sentinel_path(folder);
                Ok(AppliedOp {
                    op: op.clone(),
                    bytes: size(&p).unwrap_or(0),
                    sha256: Some(commit::sha256_file(&p).map_err(|e| e.to_string())?),
                    method: "sentinel".into(),
                    created_dirs,
                })
            }
        }
    }

    fn check_reverse(&self, a: &AppliedOp, vacated: &[PathBuf]) -> Result<(), String> {
        match &a.op {
            FileOp::Move { src, dst } => {
                // A moved folder (the folder import's rename) has no size to compare: its
                // files are checked by the operations that touched them.
                if !dst.is_dir() && size(dst) != Some(a.bytes) {
                    return Err(format!(
                        "{} has changed or is gone since it was filed",
                        dst.display()
                    ));
                }
                if src.exists() && !vacated.contains(src) {
                    return Err(format!("{} exists again", src.display()));
                }
                Ok(())
            }
            FileOp::Copy { dst, .. } => match (&a.sha256, commit::sha256_file(dst)) {
                (Some(want), Ok(have)) if *want == have => Ok(()),
                _ => Err(format!(
                    "{} was changed after it was copied; it is left as it is",
                    dst.display()
                )),
            },
            FileOp::Mkdir { .. } => Ok(()),
            FileOp::Rmdir { path } => {
                if path.exists() {
                    Err(format!("{} exists again", path.display()))
                } else {
                    Ok(())
                }
            }
            FileOp::AppendLines { path, .. } => {
                if path.is_file() {
                    Ok(())
                } else {
                    Err(format!("{} is gone", path.display()))
                }
            }
            FileOp::WriteSentinel { folder, .. } => {
                let p = filer_intake::sentinel::sentinel_path(folder);
                match (&a.sha256, commit::sha256_file(&p)) {
                    (Some(want), Ok(have)) if *want == have => Ok(()),
                    _ => Err(format!(
                        "{} was changed or removed; it is left as it is",
                        p.display()
                    )),
                }
            }
        }
    }

    fn reverse(&mut self, a: &AppliedOp) -> Result<(), String> {
        self.check_reverse(a, &[])?;
        match &a.op {
            // Content kept as a version may be shared by other documents' versions since (the
            // store is by content): undoing copies it back and leaves the stored one.
            FileOp::Move { src, dst } if in_versions(dst) => {
                commit::copy_verified(dst, src)?;
            }
            FileOp::Move { src, dst } => {
                commit::transfer(dst, src, self.force_copy)?;
            }
            // Only a copy this op wrote whose hash still matches (checked above).
            FileOp::Copy { dst, .. } => std::fs::remove_file(dst)
                .map_err(|e| format!("cannot remove {}: {e}", dst.display()))?,
            FileOp::Mkdir { .. } => {}
            FileOp::Rmdir { path } => std::fs::create_dir(path)
                .map_err(|e| format!("cannot create {}: {e}", path.display()))?,
            FileOp::AppendLines {
                path, undo_lines, ..
            } => append(path, undo_lines)?,
            FileOp::WriteSentinel { folder, .. } => {
                let p = filer_intake::sentinel::sentinel_path(folder);
                std::fs::remove_file(&p)
                    .map_err(|e| format!("cannot remove {}: {e}", p.display()))?
            }
        }
        remove_empty(&a.created_dirs);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use vf_core::audit::AuditEntry;
    use vf_core::db::Storage;
    use vf_core::tx::{self, Actor, Stmt, Transaction};

    use super::*;

    /// Every folder and file under `dir`: relative path → SHA-256 (folders: "dir").
    fn snapshot(dir: &Path) -> BTreeMap<String, String> {
        fn walk(base: &Path, d: &Path, out: &mut BTreeMap<String, String>) {
            for e in std::fs::read_dir(d).unwrap() {
                let p = e.unwrap().path();
                let rel = p
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                if p.is_dir() {
                    out.insert(rel, "dir".into());
                    walk(base, &p, out);
                } else {
                    out.insert(rel, commit::sha256_file(&p).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(dir, dir, &mut out);
        out
    }

    struct World {
        _tmp: tempfile::TempDir,
        cab: PathBuf,
        inbox: PathBuf,
        root: PathBuf,
        conn: vf_core::db::Connection,
        actor: Actor,
    }

    fn world() -> World {
        let tmp = tempfile::tempdir().unwrap();
        let cab = tmp.path().join("cab");
        let (inbox, root) = (cab.join("inbox"), cab.join("root"));
        std::fs::create_dir_all(&inbox).unwrap();
        std::fs::create_dir_all(root.join("Smith, John (1001)")).unwrap();
        std::fs::create_dir_all(root.join("Smith Holdings LLC (1002)")).unwrap();
        for (i, name) in ["w2.pdf", "1099-int.pdf", "k1.pdf"].iter().enumerate() {
            std::fs::write(
                inbox.join(name),
                format!("%PDF-1.4 synthetic {i}").repeat(100 + i),
            )
            .unwrap();
        }
        let conn = vf_core::db::open(&tmp.path().join("catalog.db"), Storage::Local).unwrap();
        World {
            _tmp: tmp,
            cab,
            inbox,
            root,
            conn,
            actor: Actor {
                user: Some("kwk".into()),
                device_id: "dev-1".into(),
            },
        }
    }

    fn batch(w: &World) -> Transaction {
        let client = w.root.join("Smith, John (1001)");
        let year = client.join("Income Tax").join("2025");
        let mut tx = Transaction::new("intake batch");
        for name in ["w2.pdf", "1099-int.pdf", "k1.pdf"] {
            tx = tx.file(FileOp::Move {
                src: w.inbox.join(name),
                dst: year.join(name),
            });
        }
        // K-1 copy to the related entity (Filer's own commit writes copies before the primary
        // move; this test checks reversibility only).
        tx.file(FileOp::Copy {
            src: year.join("k1.pdf"),
            dst: w.root.join("Smith Holdings LLC (1002)").join("K-1").join("k1.pdf"),
        })
        .catalog(Stmt::new(
            "INSERT INTO device (id, name, first_seen, last_seen, mode) VALUES ('dev-1', 'PC', 't', 't', 'local')",
            vec![],
        ))
        .catalog_undo(Stmt::new("DELETE FROM device WHERE id = 'dev-1'", vec![]))
        .audit(AuditEntry::new("file", None, serde_json::json!({"files": 3, "copies": 1})))
    }

    fn round_trip(force_copy: bool) {
        let mut w = world();
        let mut fs = FilerFileOps::new(&[&w.root, &w.inbox]).unwrap();
        fs.force_copy = force_copy;
        let before = snapshot(&w.cab);

        let done = batch(&w).commit(&mut w.conn, &mut fs, &w.actor).unwrap();
        let filed = w.root.join("Smith, John (1001)/Income Tax/2025/w2.pdf");
        assert!(filed.is_file());
        assert!(!w.inbox.join("w2.pdf").exists());
        let want = if force_copy {
            "copy_verify_delete"
        } else {
            "move"
        };
        assert_eq!(done.applied[0].method, want);

        tx::undo(&mut w.conn, &mut fs, &w.actor, &done.tx_id).unwrap();
        assert_eq!(
            snapshot(&w.cab),
            before,
            "undo leaves cabinet + inbox byte-identical"
        );

        batch(&w).commit(&mut w.conn, &mut fs, &w.actor).unwrap();
        assert!(filed.is_file());
        assert!(vf_core::audit::verify(&w.conn).unwrap().ok());
    }

    /// P00 done-when: commit → undo → commit through Transaction, same-volume rename.
    /// PA-34 review: on a catalog shared by several sessions, a change another running copy has
    /// in progress is not recovered; once that copy is gone, it is.
    #[test]
    fn recovery_leaves_running_copies_alone() {
        use std::os::windows::fs::OpenOptionsExt;
        let t = tempfile::tempdir().unwrap();
        let conn = vf_core::db::open(&t.path().join("c.db"), vf_core::db::Storage::Local).unwrap();
        let _me = hold_live(t.path()).unwrap();
        let other = t.path().join("live").join("other-copy.lock");
        let held = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .open(&other)
            .unwrap();
        conn.execute(
            "INSERT INTO tx_journal (tx_id, ts, label, ops, owner)
             VALUES ('t1', 't', 'move', '[]', 'other-copy')",
            [],
        )
        .unwrap();
        let rows = |c: &vf_core::db::Connection| -> i64 {
            c.query_row("SELECT count(*) FROM tx_journal", [], |r| r.get(0))
                .unwrap()
        };
        let r = recover(&conn, t.path()).unwrap();
        assert_eq!(r.transactions, 0, "running: left alone");
        assert_eq!(rows(&conn), 1);
        drop(held);
        let r = recover(&conn, t.path()).unwrap();
        assert_eq!(r.transactions, 1, "gone: recovered");
        assert_eq!(rows(&conn), 0);
        assert!(!other.exists(), "its leftover lock file is removed");
    }

    #[test]
    fn commit_undo_commit_rename() {
        round_trip(false);
    }

    /// P00 done-when: the same through Filer's forced copy + SHA-256 verify + delete path.
    #[test]
    fn commit_undo_commit_copy_verify() {
        round_trip(true);
    }

    #[test]
    fn collision_mid_batch_puts_everything_back() {
        let mut w = world();
        let mut fs = FilerFileOps::new(&[&w.root, &w.inbox]).unwrap();
        let taken = w.root.join("Smith, John (1001)/Income Tax/2025");
        std::fs::create_dir_all(&taken).unwrap();
        std::fs::write(taken.join("k1.pdf"), b"already filed").unwrap();
        let before = snapshot(&w.cab);
        assert!(batch(&w).commit(&mut w.conn, &mut fs, &w.actor).is_err());
        assert_eq!(snapshot(&w.cab), before);
    }

    #[test]
    fn modified_copy_blocks_undo_and_nothing_moves() {
        let mut w = world();
        let mut fs = FilerFileOps::new(&[&w.root, &w.inbox]).unwrap();
        let done = batch(&w).commit(&mut w.conn, &mut fs, &w.actor).unwrap();
        let copy = w.root.join("Smith Holdings LLC (1002)/K-1/k1.pdf");
        std::fs::write(&copy, b"edited by someone").unwrap();
        let after = snapshot(&w.cab);
        let err = tx::undo(&mut w.conn, &mut fs, &w.actor, &done.tx_id).unwrap_err();
        assert!(matches!(err, vf_core::Error::UndoBlocked(_)), "{err}");
        assert_eq!(snapshot(&w.cab), after);
    }

    #[test]
    fn writes_outside_the_cabinet_are_refused() {
        let mut w = world();
        let mut fs = FilerFileOps::new(&[&w.root, &w.inbox]).unwrap();
        let outside = w.cab.parent().unwrap().join("elsewhere").join("w2.pdf");
        let tx = Transaction::new("bad")
            .file(FileOp::Move {
                src: w.inbox.join("w2.pdf"),
                dst: outside.clone(),
            })
            .audit(AuditEntry::new("file", None, serde_json::json!({})));
        assert!(tx.commit(&mut w.conn, &mut fs, &w.actor).is_err());
        assert!(w.inbox.join("w2.pdf").is_file());
        assert!(!outside.parent().unwrap().exists());
    }
}
