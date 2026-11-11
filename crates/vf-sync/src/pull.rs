//! Pull (P11 steps 4, 6, 7): segments whose rev differs from what this workstation applied
//! come down. Each document is compared three ways (bucket, catalog, last exchanged base):
//!
//! - new there → downloaded, SHA-256 checked, moved into place with Filer's primitive, cataloged
//!   under the bucket's id (a file already here at that path with the same content is adopted:
//!   its row takes the bucket's id);
//! - changed there only → new content (the old kept as a version), new path, new metadata;
//! - changed in both places → this workstation's content is kept beside it as
//!   `name (conflict from <this PC>).ext`, the bucket's is canonical, and a Conflicts item is
//!   raised; nothing is overwritten;
//! - deleted there → moved to the trash here, unless changed here (then kept, and raised);
//! - changed here only → left for the next push.
//!
//! A segment with a document that could not be applied yet (it is in Intake's last batch) is
//! applied again on the next pull.

use std::path::{Path, PathBuf};

use serde_json::json;
use vf_core::audit::AuditEntry;
use vf_core::db::{Connection, OptionalExtension};
use vf_core::tx::{Actor, FileOp, NoFileOps, Stmt, Transaction};
use vf_fs::cabinet::{self, VersionSidecar};
use vf_fs::docmeta::{self, Meta};

use crate::catalog::{self, CABINET};
use crate::crypto::sha256;
use crate::layout::{self, CabinetSegment, ClientSegment, DocEntry, Head};
use crate::{Result, Sync, SyncError};

pub const LABEL: &str = "sync_pull";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PullReport {
    pub segments: usize,
    pub new: usize,
    pub updated: usize,
    pub moved: usize,
    pub meta: usize,
    pub deleted: usize,
    pub conflicts: usize,
    /// Documents left for the next pull (in Intake's last batch).
    pub deferred: usize,
    pub head_rev: u64,
}

impl PullReport {
    pub fn changed(&self) -> usize {
        self.new + self.updated + self.moved + self.meta + self.deleted + self.conflicts
    }
}

/// Applies what the bucket has that this workstation has not applied.
pub fn pull(conn: &mut Connection, root: &Path, s: &Sync) -> Result<PullReport> {
    let mut rep = PullReport::default();
    let Some(head) = s.head()? else {
        return Ok(rep);
    };
    // The bucket's manifest never goes back: an older HEAD put back (a deleted object version,
    // a restored bucket) would quietly take back newer documents.
    let seen = catalog::last_rev(conn, &s.device_id)?;
    if head.rev < seen {
        return Err(SyncError::Format(format!(
            "the bucket's manifest is at rev {} but this PC has seen rev {seen}: it went back. Nothing was changed here; check the bucket (B2 file versions, a restore) before syncing again.",
            head.rev
        )));
    }
    rep.head_rev = head.rev;
    apply_head(conn, root, s, &head, &mut rep)?;
    // With documents left for later, this PC has not applied HEAD: the next push pulls first,
    // and never sends a segment it has not applied (`push`).
    if rep.deferred == 0 {
        catalog::set_state(conn, &s.device_id, head.rev, "pull")?;
        crate::push::write_device(s, head.rev)?;
    }
    if rep.segments > 0 {
        audit_pull(conn, s, &rep)?;
        tracing::info!(
            segments = rep.segments,
            changed = rep.changed(),
            conflicts = rep.conflicts,
            "sync pull"
        );
    }
    Ok(rep)
}

pub(crate) fn apply_head(
    conn: &mut Connection,
    root: &Path,
    s: &Sync,
    head: &Head,
    rep: &mut PullReport,
) -> Result<()> {
    if head.cabinet_segment_rev > 0
        && head.cabinet_segment_rev != catalog::segment_rev(conn, CABINET)?
    {
        let key = head.cabinet_key();
        let seg: CabinetSegment = s
            .get_json(&key)?
            .ok_or_else(|| SyncError::Format(format!("{key} is missing")))?;
        seg.check()
            .map_err(|what| SyncError::Format(format!("{key}: refused {what}")))?;
        let merged = apply_files(conn, root, s, &seg.files, &seg.written_by)?;
        let deferred = apply_docs(conn, root, s, None, "", &seg.documents, rep)?;
        if !deferred {
            let fh = serde_json::to_string(&merged).expect("serializes");
            catalog::set_segment(conn, CABINET, seg.segment_rev, None, None, Some(&fh))?;
        }
        rep.segments += 1;
    }
    for (client, rev) in &head.segments {
        apply_client_segment(conn, root, s, head, client, *rev, rep)?;
    }
    Ok(())
}

/// Brings in one client's segment of `head` only, when it differs from what this computer
/// applied (a push of that client from a computer that does not run the cabinet's sync,
/// PA-41). The cabinet's segment and the other clients' are left for a full pull.
///
/// `same_folder`: the computers that work in this very cabinet folder (several servers on one
/// share). A segment one of them wrote was sent from this folder, after its writer had applied
/// everything before it: its files are here already, edited since perhaps. It is taken as the
/// last exchange without writing a file ([`adopt_client_segment`]), so the folder as it is now
/// goes up. A three-way apply would take an edit made here since as a change on both sides
/// (this catalog's last exchange can be a week old, a seed copy) and leave conflict copies in
/// the shared folder.
pub(crate) fn apply_client(
    conn: &mut Connection,
    root: &Path,
    s: &Sync,
    head: &Head,
    client: &str,
    same_folder: &[String],
    rep: &mut PullReport,
) -> Result<()> {
    if let Some(rev) = head.segments.get(client)
        && *rev != catalog::segment_rev(conn, client)?
    {
        let key = head.client_key(client, *rev);
        let seg: ClientSegment = s
            .get_json(&key)?
            .ok_or_else(|| SyncError::Format(format!("{key} is missing")))?;
        if seg.client_id != client {
            return Err(SyncError::Format(format!("{key}: names another client")));
        }
        seg.check()
            .map_err(|what| SyncError::Format(format!("{key}: refused {what}")))?;
        let here = same_folder
            .iter()
            .any(|c| !seg.written_by.is_empty() && c.eq_ignore_ascii_case(&seg.written_by));
        if here {
            adopt_client_segment(conn, &seg)?;
            rep.segments += 1;
        } else {
            apply_client_segment(conn, root, s, head, client, *rev, rep)?;
        }
    }
    if rep.segments > 0 {
        audit_pull(conn, s, rep)?;
    }
    Ok(())
}

fn apply_client_segment(
    conn: &mut Connection,
    root: &Path,
    s: &Sync,
    head: &Head,
    client: &str,
    rev: u64,
    rep: &mut PullReport,
) -> Result<()> {
    if rev == catalog::segment_rev(conn, client)? {
        return Ok(());
    }
    let key = head.client_key(client, rev);
    let seg: ClientSegment = s
        .get_json(&key)?
        .ok_or_else(|| SyncError::Format(format!("{key} is missing")))?;
    if seg.client_id != client {
        return Err(SyncError::Format(format!("{key}: names another client")));
    }
    seg.check()
        .map_err(|what| SyncError::Format(format!("{key}: refused {what}")))?;
    let folder = ensure_folder(conn, root, s, &seg)?;
    prefetch(conn, s, root, &seg.documents);
    let deferred = apply_docs(conn, root, s, Some(client), &folder, &seg.documents, rep);
    drop_prefetched();
    let deferred = deferred?;
    if !deferred {
        let sh = vf_core::db::OptionalExtension::optional(conn.query_row(
            "SELECT sentinel_hash FROM client WHERE id = ?1",
            [client],
            |r| r.get::<_, Option<String>>(0),
        ))?
        .flatten();
        catalog::set_segment(conn, client, rev, Some(&folder), sh.as_deref(), None)?;
    }
    rep.segments += 1;
    Ok(())
}

/// Takes `seg`, written from this same cabinet folder, as this catalog's last exchange for its
/// client: each document's base becomes the segment's entry, and a document this catalog knows
/// at the same path under another id (indexed here on its own) takes the bucket's id. No file
/// is written or moved; what differs here is a change here, sent by the push that follows.
fn adopt_client_segment(conn: &mut Connection, seg: &ClientSegment) -> Result<()> {
    let client = seg.client_id.as_str();
    let folder = catalog::client_folder(conn, client)?.unwrap_or_else(|| seg.folder_rel.clone());
    for e in &seg.documents {
        let want = join(&folder, &e.rel_path);
        if catalog::doc(conn, &e.doc_id)?.is_none() {
            let other: Option<String> = conn
                .query_row(
                    "SELECT id FROM document WHERE rel_path = ?1 AND deleted_at IS NULL",
                    [&want],
                    |r| r.get(0),
                )
                .optional()?;
            match other {
                // Never sent under its own id: the same file, edited here perhaps.
                Some(id) if catalog::base(conn, &id)?.is_none() => {
                    catalog::rekey(conn, &id, &e.doc_id)?
                }
                // Not here (moved or deleted in this folder since): nothing to take.
                _ => continue,
            }
        }
        catalog::set_base_values(
            conn,
            &e.doc_id,
            Some(client),
            &want,
            &e.hash,
            e.rev,
            e.deleted_at.is_some(),
        )?;
    }
    let sh = sha256(seg.sentinel.as_bytes());
    catalog::set_segment(
        conn,
        client,
        seg.segment_rev,
        Some(&folder),
        Some(&sh),
        None,
    )?;
    Ok(())
}

fn audit_pull(conn: &mut Connection, s: &Sync, rep: &PullReport) -> Result<()> {
    Transaction::new(LABEL)
        .audit(AuditEntry::new(
            LABEL,
            None,
            json!({
                "segments": rep.segments, "new": rep.new, "updated": rep.updated,
                "moved": rep.moved, "meta": rep.meta, "deleted": rep.deleted,
                "conflicts": rep.conflicts, "deferred": rep.deferred, "rev": rep.head_rev,
            }),
        ))
        .commit(conn, &mut NoFileOps, &Actor::device(&s.device_id))?;
    Ok(())
}

/// The admin password file, relative to `_Vibe` (vf_fs::trash::pin_file).
const PIN_FILE: &str = "trash_pin.json";

/// Writes the bucket's master files where this workstation has not changed them since the last
/// exchange, then brings the catalog in line. Returns the hashes now exchanged.
fn apply_files(
    conn: &mut Connection,
    root: &Path,
    s: &Sync,
    files: &std::collections::BTreeMap<String, String>,
    from_device: &str,
) -> Result<std::collections::BTreeMap<String, String>> {
    let base = catalog::files_base(conn)?;
    let vibe = root.join(cabinet::VIBE_DIR);
    let mut merged = base.clone();
    let mut wrote = 0;
    let mut files_conflicts: Vec<String> = vec![];
    for (rel, text) in files {
        let p = vibe.join(rel);
        let local = std::fs::read_to_string(&p).ok();
        let remote_h = sha256(text.as_bytes());
        if local.as_deref() == Some(text.as_str()) {
            merged.insert(rel.clone(), remote_h);
            continue;
        }
        let local_h = local.as_ref().map(|t| sha256(t.as_bytes()));
        // The admin password file deleted here after it was exchanged (the "forgotten password"
        // reset): a removal, not a file to restore. The removed marker goes up instead
        // (owner 2026-11-04).
        if local.is_none() && rel == PIN_FILE && base.get(rel) == Some(&remote_h) {
            cabinet::write_replace(&p, vf_fs::trash::REMOVED_MARKER.as_bytes())?;
            continue;
        }
        if local.is_none() || local_h.as_ref() == base.get(rel) {
            if let Some(d) = p.parent() {
                std::fs::create_dir_all(d).map_err(|e| SyncError::Local(e.to_string()))?;
            }
            cabinet::write_replace(&p, text.as_bytes())?;
            merged.insert(rel.clone(), remote_h);
            wrote += 1;
            continue;
        }
        // Changed on both PCs since the last exchange.
        let mine = local.unwrap_or_default();
        if rel == vf_fs::holds::FILE
            && let Some(m) = vf_fs::holds::merge_text(&mine, text)
        {
            // Legal holds are never lost: both sides' holds, a release on either side kept.
            cabinet::write_replace(&p, m.as_bytes())?;
            merged.insert(rel.clone(), remote_h);
            wrote += 1;
        } else {
            // This PC's copy stays (and goes up with the next push); the other one is kept
            // beside it for a person to compare, and the pull reports it.
            let other = vibe.join("conflicts").join(format!(
                "{}.from-{}",
                rel.replace('/', "_"),
                conflict_device(if from_device.is_empty() {
                    "another PC"
                } else {
                    from_device
                })
            ));
            if let Some(d) = other.parent() {
                std::fs::create_dir_all(d).map_err(|e| SyncError::Local(e.to_string()))?;
            }
            cabinet::write_replace(&other, text.as_bytes())?;
            merged.insert(rel.clone(), remote_h);
            files_conflicts.push(rel.clone());
        }
    }
    if !files_conflicts.is_empty() {
        tracing::warn!(
            files = files_conflicts.len(),
            "cabinet settings changed on two workstations: this PC's kept, the other's saved in _Vibe/conflicts"
        );
    }
    if wrote > 0 {
        let d = &s.device_id;
        vf_fs::retention::sync(conn, root, d, LABEL)?;
        vf_fs::doctypes::sync(conn, root, d, LABEL)?;
        vf_fs::holds::sync(conn, root, d)?;
        vf_fs::rebuild::sync_clients(conn, root, d, LABEL).map_err(|e| e.to_string())?;
    }
    Ok(merged)
}

/// The client's folder here (cabinet-relative), made from the segment when it is new here:
/// folder, sentinel (its exact text), template folders.
fn ensure_folder(
    conn: &mut Connection,
    root: &Path,
    s: &Sync,
    seg: &ClientSegment,
) -> Result<String> {
    if let Some(f) = catalog::client_folder(conn, &seg.client_id)? {
        let base_folder: Option<String> = conn
            .query_row(
                "SELECT folder_rel FROM sync_segment WHERE client_id = ?1",
                [&seg.client_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        if f != seg.folder_rel
            && base_folder.as_deref() == Some(f.as_str())
            && !root.join(&seg.folder_rel).exists()
        {
            rename_folder(conn, root, s, &seg.client_id, &f, &seg.folder_rel)?;
            return Ok(seg.folder_rel.clone());
        }
        ensure_dirs(root, &f, &seg.empty_dirs);
        return Ok(f);
    }
    let mut folder = seg.folder_rel.clone();
    let mut dir = root.join(&folder);
    if let Ok(Some(sen)) = filer_intake::sentinel::read_sentinel(&dir)
        && sen.client_id.to_string() != seg.client_id
    {
        // The name is another client's here: this one goes beside it.
        dir = filer_intake::commit::free_path(&dir);
        folder = vf_fs::rebuild::rel(root, &dir);
    }
    let sentinel = filer_intake::sentinel::sentinel_path(&dir);
    if !sentinel.exists() {
        let tmp = vf_fs::content::tmp_file(root, "json")?;
        std::fs::write(&tmp, seg.sentinel.as_bytes())
            .map_err(|e| SyncError::Local(e.to_string()))?;
        let tx = Transaction::new(LABEL)
            .file(FileOp::Move {
                src: tmp.clone(),
                dst: sentinel,
            })
            .audit(AuditEntry::new(LABEL, None, json!({ "client_folder": 1 })));
        if let Err(e) = vf_fs::fileops::commit(conn, root, tx, &s.device_id) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
    }
    ensure_dirs(root, &folder, &seg.empty_dirs);
    vf_fs::rebuild::sync_clients(conn, root, &s.device_id, LABEL).map_err(|e| e.to_string())?;
    Ok(folder)
}

fn ensure_dirs(root: &Path, folder: &str, dirs: &[String]) {
    for d in dirs {
        let _ = std::fs::create_dir_all(root.join(folder).join(d));
    }
}

/// A client folder renamed elsewhere (and not here): the folder and its documents' paths follow.
fn rename_folder(
    conn: &mut Connection,
    root: &Path,
    s: &Sync,
    client: &str,
    from: &str,
    to: &str,
) -> Result<()> {
    let tx = Transaction::new(LABEL)
        .file(FileOp::Move {
            src: root.join(from),
            dst: root.join(to),
        })
        .catalog(Stmt::new(
            "UPDATE document SET rel_path = ?2 || substr(rel_path, length(?1) + 1),
               rev = rev + 1, updated_at = ?4
             WHERE client_id = ?3 AND (rel_path LIKE ?1 || '/%')",
            vec![
                from.into(),
                to.into(),
                client.into(),
                vf_core::now_rfc3339().into(),
            ],
        ))
        .audit(AuditEntry::new(
            LABEL,
            None,
            json!({ "client_folder_renamed": 1 }),
        ));
    vf_fs::fileops::commit(conn, root, tx, &s.device_id)?;
    vf_fs::rebuild::sync_clients(conn, root, &s.device_id, LABEL).map_err(|e| e.to_string())?;
    // The documents now sit where the bucket has them: their base follows.
    conn.execute(
        "UPDATE sync_base SET rel_path = ?2 || substr(rel_path, length(?1) + 1)
         WHERE client_id = ?3 AND (rel_path LIKE ?1 || '/%')",
        [from, to, client],
    )?;
    // The rename added 1 to each document's rev: the base follows only where nothing else was
    // pending (an edit made here and not sent yet stays pending).
    conn.execute(
        "UPDATE sync_base SET rev = rev + 1
         WHERE client_id = ?1
           AND rev + 1 = (SELECT rev FROM document WHERE id = sync_base.doc_id)",
        [client],
    )?;
    Ok(())
}

/// Downloads `object` (content `hash`) into the cabinet's temp folder, checked.
/// Downloads fetched ahead of a segment's documents, by content hash (PA-34: one at a time,
/// a pull of thousands of new files waited on each in turn). Temporary files in `_Vibe/tmp`.
static PREFETCHED: std::sync::Mutex<Option<std::collections::HashMap<String, PathBuf>>> =
    std::sync::Mutex::new(None);
/// Downloads at once.
const FETCHERS: usize = 8;
/// Entries at or above this size are not fetched ahead (each is held in memory whole).
const PREFETCH_BELOW: i64 = 32 * 1024 * 1024;

/// Fetches, several at once, the contents of `entries` this catalog has no copy of, for
/// [`fetch`] to take. A failure here is not an error: [`fetch`] then downloads as before.
fn prefetch(conn: &Connection, s: &Sync, root: &Path, entries: &[DocEntry]) {
    let mut want: Vec<(String, String)> = vec![];
    let mut seen = std::collections::HashSet::new();
    let mut have = match conn.prepare_cached(
        "SELECT EXISTS (SELECT 1 FROM document WHERE hash = ?1 AND deleted_at IS NULL)",
    ) {
        Ok(st) => st,
        Err(_) => return,
    };
    for e in entries {
        if e.deleted_at.is_some() || e.size >= PREFETCH_BELOW || !seen.insert(e.hash.clone()) {
            continue;
        }
        let here: bool = have.query_row([&e.hash], |r| r.get(0)).unwrap_or(true);
        if !here {
            want.push((e.object.clone(), e.hash.clone()));
        }
    }
    if want.len() < 2 {
        return;
    }
    let queue = std::sync::Mutex::new(want);
    let got = std::sync::Mutex::new(std::collections::HashMap::new());
    std::thread::scope(|sc| {
        for _ in 0..FETCHERS {
            sc.spawn(|| {
                while let Some((object, hash)) = queue.lock().ok().and_then(|mut q| q.pop()) {
                    match download(s, root, &object, &hash) {
                        Ok(p) => {
                            if let Ok(mut g) = got.lock() {
                                g.insert(ahead_key(root, &hash), p);
                            }
                        }
                        // The bucket is unreachable: the rest is left to fetch.
                        Err(SyncError::Remote(_)) => break,
                        Err(_) => {}
                    }
                }
            });
        }
    });
    if let (Ok(mut p), Ok(got)) = (PREFETCHED.lock(), got.into_inner()) {
        p.get_or_insert_with(Default::default).extend(got);
    }
}

/// [`PREFETCHED`]'s key: the cabinet and the content.
fn ahead_key(root: &Path, hash: &str) -> String {
    format!("{}|{hash}", root.display())
}

/// Deletes downloads [`prefetch`] made that nothing took.
fn drop_prefetched() {
    if let Ok(mut p) = PREFETCHED.lock()
        && let Some(m) = p.take()
    {
        for path in m.into_values() {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn fetch(s: &Sync, root: &Path, object: &str, hash: &str) -> Result<PathBuf> {
    let ahead = PREFETCHED
        .lock()
        .ok()
        .and_then(|mut p| p.as_mut().and_then(|m| m.remove(&ahead_key(root, hash))));
    if let Some(p) = ahead
        && p.is_file()
    {
        return Ok(p);
    }
    download(s, root, object, hash)
}

fn download(s: &Sync, root: &Path, object: &str, hash: &str) -> Result<PathBuf> {
    let key = layout::object_key(object);
    let body = s
        .remote
        .get(&key)?
        .ok_or_else(|| SyncError::Format(format!("{key} is missing")))?;
    let plain = s.keys.open(&body, &key)?;
    if sha256(&plain) != hash {
        return Err(SyncError::Format(format!(
            "{key} does not match its SHA-256"
        )));
    }
    let tmp = vf_fs::content::tmp_file(root, "part")?;
    std::fs::write(&tmp, &plain).map_err(|e| SyncError::Local(e.to_string()))?;
    Ok(tmp)
}

fn join(folder: &str, rel: &str) -> String {
    if folder.is_empty() {
        rel.to_string()
    } else {
        format!("{folder}/{rel}")
    }
}

fn type_name(conn: &Connection, id: &Option<String>) -> Option<String> {
    id.as_ref().and_then(|id| {
        conn.query_row("SELECT name FROM doc_type WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .ok()
    })
}

fn meta_of(conn: &Connection, e: &DocEntry, rel: &str, folder: &str) -> Meta {
    Meta {
        id: e.doc_id.clone(),
        rel_path: rel.into(),
        hash: e.hash.clone(),
        folder_rel: (!folder.is_empty()).then(|| folder.to_string()),
        doc_type: e.doc_type_id.clone(),
        doc_type_name: type_name(conn, &e.doc_type_id),
        tax_year: e.tax_year,
        status: e.status.clone(),
        source: e.source.clone(),
        tags: e.tags.clone(),
        stamp: None,
    }
}

/// The document's doc type exists here (it came with the cabinet segment); else none.
fn known_type(conn: &Connection, id: &Option<String>) -> Option<String> {
    id.clone().filter(|t| {
        conn.query_row("SELECT 1 FROM doc_type WHERE id = ?1", [t], |_| Ok(()))
            .is_ok()
    })
}

/// Versions of `e` this workstation lacks: content into `_Vibe/versions/`, row, sidecar.
fn ensure_versions(
    conn: &mut Connection,
    root: &Path,
    s: &Sync,
    e: &DocEntry,
    rel: &str,
) -> Result<()> {
    for v in &e.versions {
        let have: bool = conn
            .query_row(
                "SELECT 1 FROM version WHERE doc_id = ?1 AND hash = ?2",
                [&e.doc_id, &v.hash],
                |_| Ok(()),
            )
            .is_ok();
        if have {
            continue;
        }
        let path = cabinet::version_path(root, &v.hash);
        let mut tx = Transaction::new(LABEL);
        let mut tmp = None;
        if !path.exists() {
            let t = fetch(s, root, &v.object, &v.hash)?;
            tx = tx.file(FileOp::Move {
                src: t.clone(),
                dst: path,
            });
            tmp = Some(t);
        }
        tx = tx
            .catalog(Stmt::new(
                "INSERT OR IGNORE INTO version (doc_id, hash, size, created, created_by, device_id, reason)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                vec![
                    e.doc_id.clone().into(),
                    v.hash.clone().into(),
                    v.size.into(),
                    v.created.clone().into(),
                    v.created_by.clone().into(),
                    v.device_id.clone().into(),
                    v.reason.clone().into(),
                ],
            ))
            .audit(AuditEntry::new(LABEL, Some(&e.doc_id), json!({ "version": 1 })));
        if let Err(err) = vf_fs::fileops::commit(conn, root, tx, &s.device_id) {
            if let Some(t) = tmp {
                let _ = std::fs::remove_file(t);
            }
            return Err(err.into());
        }
        let _ = cabinet::add_version_sidecar(
            root,
            &v.hash,
            VersionSidecar {
                rel_path: rel.into(),
                created: v.created.clone(),
                created_by: v.created_by.clone(),
                device_id: v.device_id.clone(),
                reason: v.reason.clone(),
                // Pulled versions do not say what replaced them: rebuild goes by the path.
                next: None,
            },
        );
    }
    Ok(())
}

/// Sets the document's metadata to the bucket's, with its docmeta line.
fn set_meta(
    conn: &mut Connection,
    root: &Path,
    s: &Sync,
    e: &DocEntry,
    rel: &str,
    folder: &str,
) -> Result<()> {
    let m = meta_of(conn, e, rel, folder);
    let (file, line) = docmeta::record_line(root, &m, &vf_core::now_rfc3339(), &s.device_id);
    let mut tx = Transaction::new(LABEL)
        .catalog(Stmt::new(
            "UPDATE document SET doc_type_id = ?2, tax_year = ?3, status = ?4, source = ?5,
               rev = rev + 1, updated_at = ?6 WHERE id = ?1",
            vec![
                e.doc_id.clone().into(),
                known_type(conn, &e.doc_type_id).into(),
                e.tax_year.into(),
                e.status.clone().into(),
                e.source.clone().into(),
                vf_core::now_rfc3339().into(),
            ],
        ))
        .catalog(Stmt::new(
            "DELETE FROM document_tag WHERE doc_id = ?1",
            vec![e.doc_id.clone().into()],
        ));
    for t in &e.tags {
        tx = tx.catalog(Stmt::new(
            "INSERT INTO document_tag (doc_id, tag) VALUES (?1, ?2)",
            vec![e.doc_id.clone().into(), t.clone().into()],
        ));
    }
    tx = tx
        .file(FileOp::AppendLines {
            path: file,
            lines: vec![line],
            undo_lines: vec![],
        })
        .audit(AuditEntry::new(
            LABEL,
            Some(&e.doc_id),
            json!({ "meta": 1 }),
        ));
    vf_fs::fileops::commit(conn, root, tx, &s.device_id)?;
    Ok(())
}

fn meta_differs(conn: &Connection, e: &DocEntry) -> Result<bool> {
    let m = docmeta::load(conn, &e.doc_id)?;
    let mut tags = e.tags.clone();
    tags.sort();
    Ok(m.doc_type != known_type(conn, &e.doc_type_id)
        || m.tax_year != e.tax_year
        || m.status != e.status
        || m.source != e.source
        || m.tags != tags)
}

/// A new document from the bucket, under its id.
fn insert_new(
    conn: &mut Connection,
    root: &Path,
    s: &Sync,
    client: Option<&str>,
    folder: &str,
    e: &DocEntry,
) -> Result<String> {
    // Already in the folder with this content, not yet in this catalog: another computer
    // working in the same folder filed it and sent it (PA-41). Taken as it is: a download
    // beside it would be a second copy, ` (2)`.
    let here = root.join(join(folder, &e.rel_path));
    let there =
        here.is_file() && filer_intake::commit::sha256_file(&here).is_ok_and(|h| h == e.hash);
    let (tmp, dst) = if there {
        (None, here)
    } else {
        let tmp = fetch(s, root, &e.object, &e.hash)?;
        (Some(tmp), filer_intake::commit::free_path(&here))
    };
    let rel = vf_fs::rebuild::rel(root, &dst);
    let m = meta_of(conn, e, &rel, folder);
    let (file, line) = docmeta::record_line(root, &m, &vf_core::now_rfc3339(), &s.device_id);
    let now = vf_core::now_rfc3339();
    let mut tx = Transaction::new(LABEL);
    if let Some(src) = &tmp {
        tx = tx.file(FileOp::Move {
            src: src.clone(),
            dst,
        });
    }
    let mut tx = tx.catalog(Stmt::new(
        "INSERT INTO document (id, client_id, doc_type_id, tax_year, rel_path, hash, size, mime,
               source, status, filed_by, filed_at, device_id, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        vec![
            e.doc_id.clone().into(),
            client.map(str::to_owned).into(),
            known_type(conn, &e.doc_type_id).into(),
            e.tax_year.into(),
            rel.clone().into(),
            e.hash.clone().into(),
            e.size.into(),
            e.mime.clone().into(),
            e.source.clone().into(),
            e.status.clone().into(),
            e.filed_by.clone().into(),
            e.filed_at.clone().into(),
            e.device_id.clone().into(),
            now.into(),
        ],
    ));
    for t in &e.tags {
        tx = tx.catalog(Stmt::new(
            "INSERT INTO document_tag (doc_id, tag) VALUES (?1, ?2)",
            vec![e.doc_id.clone().into(), t.clone().into()],
        ));
    }
    tx = tx
        .file(FileOp::AppendLines {
            path: file,
            lines: vec![line],
            undo_lines: vec![],
        })
        .audit(AuditEntry::new(LABEL, Some(&e.doc_id), json!({ "new": 1 })));
    if let Err(err) = vf_fs::fileops::commit(conn, root, tx, &s.device_id) {
        if let Some(t) = &tmp {
            let _ = std::fs::remove_file(t);
        }
        return Err(err.into());
    }
    Ok(rel)
}

/// A device name safe in a file name.
pub(crate) fn conflict_device(device: &str) -> String {
    device
        .chars()
        .map(|c| if r#"<>:"/\|?*"#.contains(c) { '_' } else { c })
        .collect()
}

/// `name (conflict from <device>).ext` beside `rel`.
pub fn conflict_name(rel: &str, device: &str) -> String {
    let (dir, file) = rel.rsplit_once('/').map_or(("", rel), |(d, f)| (d, f));
    let (stem, ext) = file
        .rsplit_once('.')
        .map_or((file, None), |(s, e)| (s, Some(e)));
    let device: String = device
        .chars()
        .map(|c| if r#"<>:"/\|?*"#.contains(c) { '_' } else { c })
        .collect();
    let name = match ext {
        Some(e) => format!("{stem} (conflict from {device}).{e}"),
        None => format!("{stem} (conflict from {device})"),
    };
    if dir.is_empty() {
        name
    } else {
        format!("{dir}/{name}")
    }
}

/// What became of one document.
enum One {
    Done,
    /// Not applied now (in Intake's last batch, refused, a file in use, an object not in the
    /// bucket): the segment is applied again on the next pull.
    Deferred,
}

/// Returns whether anything was deferred. One document that cannot be applied does not stop
/// the others; only bucket errors stop the pull.
fn apply_docs(
    conn: &mut Connection,
    root: &Path,
    s: &Sync,
    client: Option<&str>,
    folder: &str,
    entries: &[DocEntry],
    rep: &mut PullReport,
) -> Result<bool> {
    let mut deferred = false;
    for e in entries {
        match apply_one(conn, root, s, client, folder, e, rep) {
            Ok(One::Done) => {}
            Ok(One::Deferred) => {
                deferred = true;
                rep.deferred += 1;
            }
            Err(err @ SyncError::Remote(_)) => return Err(err),
            Err(err) => {
                let kind = match &err {
                    SyncError::Format(_) => "bucket object",
                    SyncError::Crypto(_) => "decrypt",
                    _ => "local",
                };
                tracing::warn!(kind, "sync pull: a document is left for the next pull");
                deferred = true;
                rep.deferred += 1;
            }
        }
    }
    Ok(deferred)
}

fn apply_one(
    conn: &mut Connection,
    root: &Path,
    s: &Sync,
    client: Option<&str>,
    folder: &str,
    e: &DocEntry,
    rep: &mut PullReport,
) -> Result<One> {
    let want = join(folder, &e.rel_path);
    let local = catalog::doc(conn, &e.doc_id)?;
    let base = catalog::base(conn, &e.doc_id)?;
    let disposed = e.tombstone_reason.as_deref() == Some("disposed");
    if disposed {
        // The bucket object goes once every PC has seen this: never trust it is there again.
        for h in std::iter::once(&e.hash).chain(e.versions.iter().map(|v| &v.hash)) {
            conn.execute("DELETE FROM sync_object WHERE hash = ?1", [h])?;
        }
    }
    let Some(l) = local else {
        if e.deleted_at.is_some() {
            return Ok(One::Done);
        }
        // Already here under another id (a shared cabinet folder, or a restore)?
        let other: Option<(String, String)> = conn
            .query_row(
                "SELECT id, hash FROM document WHERE rel_path = ?1 AND deleted_at IS NULL",
                [&want],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match other {
            Some((id, h)) if h == e.hash && catalog::base(conn, &id)?.is_none() => {
                catalog::rekey(conn, &id, &e.doc_id)?;
                if meta_differs(conn, e)? {
                    set_meta(conn, root, s, e, &want, folder)?;
                }
            }
            _ => {
                insert_new(conn, root, s, client, folder, e)?;
                rep.new += 1;
            }
        }
        ensure_versions(conn, root, s, e, &want)?;
        catalog::set_base(conn, &e.doc_id)?;
        return Ok(One::Done);
    };
    let content_here = base.as_ref().is_none_or(|b| b.hash != l.hash);
    let changed_here = base.as_ref().is_none_or(|b| {
        b.hash != l.hash || b.rel_path != l.rel_path || b.rev != l.rev || b.deleted != l.deleted
    });
    // Something other than the content changed here (metadata, a move) and is not sent yet.
    let other_here = base.as_ref().is_none_or(|b| b.rev != l.rev);
    if e.deleted_at.is_some() {
        if l.deleted {
            catalog::set_base(conn, &e.doc_id)?;
        } else if !content_here {
            if disposed {
                // Destroyed under the retention schedule elsewhere: destroyed here too (unless
                // a hold here covers it: then it stays, and it is raised).
                if vf_fs::disposition::destroy_pulled(conn, root, &s.device_id, &e.doc_id)? {
                    catalog::set_base(conn, &e.doc_id)?;
                    rep.deleted += 1;
                } else {
                    raise(conn, &e.doc_id, "deleted_there", &e.device_id, rep)?;
                }
            } else {
                vf_fs::trash::delete_as(
                    conn,
                    root,
                    &s.device_id,
                    std::slice::from_ref(&e.doc_id),
                    "deleted on another workstation",
                    LABEL,
                )?;
                catalog::set_base(conn, &e.doc_id)?;
                rep.deleted += 1;
            }
        } else {
            raise(conn, &e.doc_id, "deleted_there", &e.device_id, rep)?;
        }
        return Ok(One::Done);
    }
    if l.deleted {
        // Deleted here: the next push sends the tombstone. Changed there meanwhile: raised, so
        // the change is not lost unseen (the trash still has this PC's copy).
        if base.as_ref().is_some_and(|b| b.hash != e.hash) {
            raise(conn, &e.doc_id, "deleted_here", &e.device_id, rep)?;
        }
        return Ok(One::Done);
    }
    let mut took_remote = !changed_here;
    if e.hash != l.hash {
        // Nothing is touched while the file cannot be changed (Intake's last batch, a check-out,
        // edited outside Vibe Files): it waits for the next pull.
        if vf_fs::content::check(conn, root, &e.doc_id)?.is_err() {
            return Ok(One::Deferred);
        }
        if !content_here {
            if !replace_with(conn, root, s, e)? {
                return Ok(One::Deferred);
            }
            rep.updated += 1;
            // A metadata edit made here stays (and goes up with the next push).
            took_remote = !other_here;
        } else {
            // Both changed: keep this workstation's bytes beside, the bucket's take the path.
            let copy_rel = conflict_name(&l.rel_path, &s.device_name);
            let tmp = vf_fs::content::tmp_file(root, "part")?;
            std::fs::copy(root.join(&l.rel_path), &tmp)
                .map_err(|e| SyncError::Local(e.to_string()))?;
            let m = docmeta::load(conn, &l.id)?;
            let added = vf_fs::content::add(
                conn,
                root,
                &s.device_id,
                &tmp,
                vf_fs::content::NewDoc {
                    rel_path: copy_rel,
                    client_id: l.client_id.clone(),
                    doc_type_id: m.doc_type.clone(),
                    tax_year: m.tax_year,
                    source: m.source.clone(),
                    tags: m.tags.clone(),
                    label: LABEL.into(),
                    detail: json!({ "conflict_copy": 1 }),
                },
            )?;
            if !replace_with(conn, root, s, e)? {
                return Ok(One::Deferred);
            }
            record_conflict(
                conn,
                &e.doc_id,
                Some(&added.doc_id),
                "both_changed",
                &e.device_id,
            )?;
            rep.conflicts += 1;
            took_remote = true;
        }
    }
    // Moved or renamed there (and not here).
    let now = catalog::doc(conn, &e.doc_id)?.ok_or_else(|| SyncError::Local("gone".into()))?;
    if now.rel_path != want && base.as_ref().is_some_and(|b| b.rel_path == l.rel_path) {
        let dst = filer_intake::commit::free_path(&root.join(&want));
        let rel = vf_fs::rebuild::rel(root, &dst);
        let tx = Transaction::new(LABEL)
            .file(FileOp::Move {
                src: root.join(&now.rel_path),
                dst,
            })
            .catalog(Stmt::new(
                "UPDATE document SET rel_path = ?2, client_id = ?3, rev = rev + 1, updated_at = ?4
                 WHERE id = ?1",
                vec![
                    e.doc_id.clone().into(),
                    rel.into(),
                    client.map(str::to_owned).into(),
                    vf_core::now_rfc3339().into(),
                ],
            ))
            .audit(AuditEntry::new(
                LABEL,
                Some(&e.doc_id),
                json!({ "moved": 1 }),
            ));
        vf_fs::fileops::commit(conn, root, tx, &s.device_id)?;
        rep.moved += 1;
    }
    if (took_remote || !other_here) && meta_differs(conn, e)? {
        let rel = catalog::doc(conn, &e.doc_id)?
            .ok_or_else(|| SyncError::Local("gone".into()))?
            .rel_path;
        set_meta(conn, root, s, e, &rel, folder)?;
        rep.meta += 1;
    }
    let rel = catalog::doc(conn, &e.doc_id)?
        .ok_or_else(|| SyncError::Local("gone".into()))?
        .rel_path;
    ensure_versions(conn, root, s, e, &rel)?;
    if took_remote || !changed_here {
        catalog::set_base(conn, &e.doc_id)?;
    }
    Ok(One::Done)
}

/// Raises a Conflicts item once per document and kind (an unresolved one is not repeated).
fn raise(
    conn: &Connection,
    doc: &str,
    kind: &str,
    from_device: &str,
    rep: &mut PullReport,
) -> Result<()> {
    let open: bool = conn
        .query_row(
            "SELECT 1 FROM sync_conflict WHERE doc_id = ?1 AND kind = ?2 AND resolved IS NULL",
            [doc, kind],
            |_| Ok(()),
        )
        .is_ok();
    if !open {
        record_conflict(conn, doc, None, kind, from_device)?;
        rep.conflicts += 1;
    }
    Ok(())
}

/// The bucket's content for `e` replaces this workstation's (kept as a version). `false`: the
/// file is in Intake's last batch and must wait.
fn replace_with(conn: &mut Connection, root: &Path, s: &Sync, e: &DocEntry) -> Result<bool> {
    let tmp = fetch(s, root, &e.object, &e.hash)?;
    match vf_fs::content::replace(
        conn,
        root,
        &s.device_id,
        &e.doc_id,
        &tmp,
        vf_fs::content::Change {
            label: LABEL,
            created_by: "sync",
            reason: "replaced by a newer version from another workstation",
            detail: json!({}),
            extra: vec![],
            based_on: None,
        },
    )? {
        Ok(_) => Ok(true),
        Err(vf_fs::content::Refused::FilerUndo) => Ok(false),
        Err(why) => Err(SyncError::Local(why.to_string())),
    }
}

fn record_conflict(
    conn: &Connection,
    doc: &str,
    copy: Option<&str>,
    kind: &str,
    from_device: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO sync_conflict (id, doc_id, copy_doc_id, kind, from_device, at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        vf_core::db::params![
            vf_core::new_id(),
            doc,
            copy,
            kind,
            from_device,
            vf_core::now_rfc3339()
        ],
    )?;
    Ok(())
}
