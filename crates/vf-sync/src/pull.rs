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
    rep.head_rev = head.rev;
    apply_head(conn, root, s, &head, &mut rep)?;
    catalog::set_state(conn, &s.device_id, head.rev, "pull")?;
    crate::push::write_device(s, head.rev)?;
    if rep.segments > 0 {
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
        let key = layout::cabinet_segment_key(head.cabinet_segment_rev);
        let seg: CabinetSegment = s
            .get_json(&key)?
            .ok_or_else(|| SyncError::Format(format!("{key} is missing")))?;
        let merged = apply_files(conn, root, s, &seg.files)?;
        let deferred = apply_docs(conn, root, s, None, "", &seg.documents, rep)?;
        if !deferred {
            let fh = serde_json::to_string(&merged).expect("serializes");
            catalog::set_segment(conn, CABINET, seg.segment_rev, None, None, Some(&fh))?;
        }
        rep.segments += 1;
    }
    for (client, rev) in &head.segments {
        if *rev == catalog::segment_rev(conn, client)? {
            continue;
        }
        let key = layout::segment_key(client, *rev);
        let seg: ClientSegment = s
            .get_json(&key)?
            .ok_or_else(|| SyncError::Format(format!("{key} is missing")))?;
        let folder = ensure_folder(conn, root, s, &seg)?;
        let deferred = apply_docs(conn, root, s, Some(client), &folder, &seg.documents, rep)?;
        if !deferred {
            let sh = vf_core::db::OptionalExtension::optional(conn.query_row(
                "SELECT sentinel_hash FROM client WHERE id = ?1",
                [client],
                |r| r.get::<_, Option<String>>(0),
            ))?
            .flatten();
            catalog::set_segment(conn, client, *rev, Some(&folder), sh.as_deref(), None)?;
        }
        rep.segments += 1;
    }
    Ok(())
}

/// Writes the bucket's master files where this workstation has not changed them since the last
/// exchange, then brings the catalog in line. Returns the hashes now exchanged.
fn apply_files(
    conn: &mut Connection,
    root: &Path,
    s: &Sync,
    files: &std::collections::BTreeMap<String, String>,
) -> Result<std::collections::BTreeMap<String, String>> {
    let base = catalog::files_base(conn)?;
    let vibe = root.join(cabinet::VIBE_DIR);
    let mut merged = base.clone();
    let mut wrote = 0;
    for (rel, text) in files {
        let p = vibe.join(rel);
        let local = std::fs::read_to_string(&p).ok();
        let remote_h = sha256(text.as_bytes());
        if local.as_deref() == Some(text.as_str()) {
            merged.insert(rel.clone(), remote_h);
            continue;
        }
        let local_h = local.as_ref().map(|t| sha256(t.as_bytes()));
        if local.is_none() || local_h.as_ref() == base.get(rel) {
            if let Some(d) = p.parent() {
                std::fs::create_dir_all(d).map_err(|e| SyncError::Local(e.to_string()))?;
            }
            cabinet::write_replace(&p, text.as_bytes())?;
            merged.insert(rel.clone(), remote_h);
            wrote += 1;
        }
        // Changed here as well: this workstation's copy stays and goes up on the next push.
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
    conn.execute(
        "UPDATE sync_base SET rev = (SELECT rev FROM document WHERE id = sync_base.doc_id)
         WHERE client_id = ?1",
        [client],
    )?;
    Ok(())
}

/// Downloads `object` (content `hash`) into the cabinet's temp folder, checked.
fn fetch(s: &Sync, root: &Path, object: &str, hash: &str) -> Result<PathBuf> {
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
    let tmp = fetch(s, root, &e.object, &e.hash)?;
    let dst = filer_intake::commit::free_path(&root.join(join(folder, &e.rel_path)));
    let rel = vf_fs::rebuild::rel(root, &dst);
    let m = meta_of(conn, e, &rel, folder);
    let (file, line) = docmeta::record_line(root, &m, &vf_core::now_rfc3339(), &s.device_id);
    let now = vf_core::now_rfc3339();
    let mut tx = Transaction::new(LABEL)
        .file(FileOp::Move {
            src: tmp.clone(),
            dst,
        })
        .catalog(Stmt::new(
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
        let _ = std::fs::remove_file(&tmp);
        return Err(err.into());
    }
    Ok(rel)
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

/// Returns whether anything was deferred.
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
        let want = join(folder, &e.rel_path);
        let local = catalog::doc(conn, &e.doc_id)?;
        let base = catalog::base(conn, &e.doc_id)?;
        let Some(l) = local else {
            if e.deleted_at.is_some() {
                continue;
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
            continue;
        };
        let content_here = base.as_ref().is_none_or(|b| b.hash != l.hash);
        let changed_here = base.as_ref().is_none_or(|b| {
            b.hash != l.hash || b.rel_path != l.rel_path || b.rev != l.rev || b.deleted != l.deleted
        });
        if e.deleted_at.is_some() {
            if l.deleted {
                catalog::set_base(conn, &e.doc_id)?;
            } else if !content_here {
                vf_fs::trash::delete(
                    conn,
                    root,
                    &s.device_id,
                    std::slice::from_ref(&e.doc_id),
                    "deleted on another workstation",
                )?;
                catalog::set_base(conn, &e.doc_id)?;
                rep.deleted += 1;
            } else {
                record_conflict(conn, &e.doc_id, None, "deleted_there", &e.device_id)?;
                rep.conflicts += 1;
            }
            continue;
        }
        if l.deleted {
            continue; // deleted here: the next push sends the tombstone
        }
        let mut took_remote = !changed_here;
        if e.hash != l.hash {
            if !content_here {
                match replace_with(conn, root, s, e)? {
                    true => {
                        rep.updated += 1;
                        took_remote = true;
                    }
                    false => {
                        deferred = true;
                        rep.deferred += 1;
                        continue;
                    }
                }
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
                    deferred = true;
                    rep.deferred += 1;
                    continue;
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
        let now = catalog::doc(conn, &e.doc_id)?.expect("still here");
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
                .audit(AuditEntry::new(LABEL, Some(&e.doc_id), json!({ "moved": 1 })));
            vf_fs::fileops::commit(conn, root, tx, &s.device_id)?;
            rep.moved += 1;
        }
        let local_meta_changed = base.as_ref().is_none_or(|b| b.rev != l.rev);
        if (took_remote || !local_meta_changed) && meta_differs(conn, e)? {
            let rel = catalog::doc(conn, &e.doc_id)?.expect("still here").rel_path;
            set_meta(conn, root, s, e, &rel, folder)?;
            rep.meta += 1;
        }
        let rel = catalog::doc(conn, &e.doc_id)?.expect("still here").rel_path;
        ensure_versions(conn, root, s, e, &rel)?;
        if took_remote || !changed_here {
            catalog::set_base(conn, &e.doc_id)?;
        }
    }
    Ok(deferred)
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
