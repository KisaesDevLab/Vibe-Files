//! Soft delete (PHASES P05 step 3). Deleting documents moves their files to
//! `_Vibe/trash/<doc id>/<name>` and tombstones their rows, as one Transaction (undo puts
//! them all back). The Trash view lists what is there, restores it to its old place (or a free
//! name beside it), and empties it: by hand with the firm PIN (Q-29), and on its own after
//! [`AUTO_EMPTY_DAYS`] (Q-15). A document under an active legal hold is never emptied.

use std::path::{Path, PathBuf};

use serde_json::json;
use vf_core::audit::AuditEntry;
use vf_core::db::Connection;
use vf_core::tx::{Actor, FileOp, NoFileOps, Stmt, Transaction};

use crate::cabinet;

/// Transaction label of a delete (Browse › Move to trash).
pub const DELETE_LABEL: &str = "trash";
pub const RESTORE_LABEL: &str = "trash_restore";
/// Emptying cannot be undone (the bytes are gone); the undo stack leaves it out.
pub const PURGE_LABEL: &str = "trash_purge";
/// Trash older than this is emptied on its own (Q-15).
pub const AUTO_EMPTY_DAYS: i64 = 90;

/// A document in the trash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trashed {
    pub doc_id: String,
    /// Where it was (cabinet-relative).
    pub rel_path: String,
    pub deleted_at: String,
    pub reason: Option<String>,
    pub size: i64,
    /// Its file in the trash.
    pub file: PathBuf,
    /// Under an active legal hold: kept until released.
    pub held: bool,
}

/// The file of trashed document `doc_id` that used to be at `rel_path`.
fn trash_file(root: &Path, doc_id: &str, rel_path: &str) -> PathBuf {
    let name = Path::new(rel_path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    cabinet::trash_dir(root).join(doc_id).join(name)
}

/// Moves documents `ids` to the trash as one Transaction. Returns its id.
pub fn delete(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    ids: &[String],
    reason: &str,
) -> Result<String, String> {
    delete_as(conn, root, device_id, ids, reason, DELETE_LABEL)
}

/// [`delete`] as a Transaction labelled `label` (a delete applied from another workstation by
/// sync is `sync_pull`, off this login's undo stack).
pub fn delete_as(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    ids: &[String],
    reason: &str,
    label: &str,
) -> Result<String, String> {
    let now = vf_core::now_rfc3339();
    let mut tx = Transaction::new(label);
    for id in ids {
        let rel: String = conn
            .query_row(
                "SELECT rel_path FROM document WHERE id = ?1 AND deleted_at IS NULL",
                [id],
                |r| r.get(0),
            )
            .map_err(|_| "a document is not in the catalog any more".to_string())?;
        tx = tx
            .file(FileOp::Move {
                src: root.join(&rel),
                dst: trash_file(root, id, &rel),
            })
            .catalog(Stmt::new(
                "UPDATE document SET deleted_at = ?2, tombstone_reason = ?3, rev = rev + 1,
                 updated_at = ?2, device_id = ?4 WHERE id = ?1",
                vec![
                    id.clone().into(),
                    now.clone().into(),
                    reason.into(),
                    device_id.into(),
                ],
            ))
            .catalog_undo(Stmt::new(
                "UPDATE document SET deleted_at = NULL, tombstone_reason = NULL, rev = rev + 1
                 WHERE id = ?1",
                vec![id.clone().into()],
            ))
            .catalog(Stmt::new(
                "INSERT INTO sync_pending (doc_id, op, since) VALUES (?1, 'delete', ?2)
                 ON CONFLICT(doc_id, op) DO NOTHING",
                vec![id.clone().into(), now.clone().into()],
            ))
            .catalog_undo(Stmt::new(
                "DELETE FROM sync_pending WHERE doc_id = ?1 AND op = 'delete'",
                vec![id.clone().into()],
            ))
            .audit(AuditEntry::new(
                "trash",
                Some(id),
                json!({ "rel_path": rel, "reason": reason }),
            ));
    }
    if ids.is_empty() {
        return Err("nothing selected".into());
    }
    Ok(crate::fileops::commit(conn, root, tx, device_id)?.tx_id)
}

/// Intake's inbox folders, as the cabinet's shared Intake settings name them.
pub fn inboxes(root: &Path) -> Vec<PathBuf> {
    filer_intake::config::Settings::load_or_default(&crate::clients::master_dir(root))
        .0
        .inboxes
}

/// Whether `p` is inside one of `dirs` (any case, as Windows paths are).
fn inside_any(p: &Path, dirs: &[PathBuf]) -> bool {
    let low = |p: &Path| p.to_string_lossy().replace('/', "\\").to_lowercase();
    let p = low(p);
    dirs.iter().any(|d| {
        let d = low(d);
        let d = d.trim_end_matches('\\');
        p.starts_with(&format!("{d}\\"))
    })
}

/// Why an Intake file is in the trash (its row's `tombstone_reason`).
pub const FROM_INTAKE: &str = "removed from Intake";

/// Moves files out of Intake's inbox into the cabinet trash (owner 2026-11-05, Q-61), as one
/// Transaction. Each gets a document row that is in the trash from the start (never live, never
/// synced) whose path is where it was in the inbox, so **Restore** puts it back there for
/// Intake. `inbox_dirs`: the folders the files may be taken from. Returns the Transaction id.
pub fn delete_inbox_files(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    files: &[PathBuf],
    inbox_dirs: &[PathBuf],
) -> Result<String, String> {
    if files.is_empty() {
        return Err("nothing selected".into());
    }
    let now = vf_core::now_rfc3339();
    let mut tx = Transaction::new(DELETE_LABEL);
    for f in files {
        if !inside_any(f, inbox_dirs) {
            return Err(format!("{} is not in an Intake inbox", f.display()));
        }
        if !f.is_file() {
            return Err(format!("{} is not in the inbox any more", f.display()));
        }
        let id = vf_core::new_id();
        let at = f.to_string_lossy().into_owned();
        let size = std::fs::metadata(f).map(|m| m.len() as i64).unwrap_or(0);
        let hash = filer_intake::commit::sha256_file(f)
            .map_err(|e| format!("cannot read {}: {e}", f.display()))?;
        tx = tx
            .file(FileOp::Move {
                src: f.clone(),
                dst: trash_file(root, &id, &at),
            })
            .catalog(Stmt::new(
                "INSERT INTO document (id, client_id, rel_path, hash, size, status, source,
                   filed_by, filed_at, device_id, updated_at, deleted_at, tombstone_reason)
                 VALUES (?1, NULL, ?2, ?3, ?4, 'received', 'client', 'intake', ?5, ?6, ?5, ?5, ?7)",
                vec![
                    id.clone().into(),
                    at.clone().into(),
                    hash.into(),
                    size.into(),
                    now.clone().into(),
                    device_id.into(),
                    FROM_INTAKE.into(),
                ],
            ))
            .catalog_undo(Stmt::new(
                "DELETE FROM document WHERE id = ?1",
                vec![id.clone().into()],
            ))
            .audit(AuditEntry::new(
                "trash",
                Some(&id),
                json!({ "from": "intake", "reason": FROM_INTAKE }),
            ));
    }
    let mut allowed: Vec<&Path> = vec![root];
    allowed.extend(inbox_dirs.iter().map(PathBuf::as_path));
    Ok(crate::fileops::commit_in(conn, &allowed, tx, device_id)?.tx_id)
}

/// Whether a legal hold that is not released covers `doc_id` (the document or its client).
pub fn held(conn: &Connection, doc_id: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM legal_hold h, document d
         WHERE d.id = ?1 AND h.released IS NULL
           AND (h.doc_id = d.id OR (h.client_id IS NOT NULL AND h.client_id = d.client_id))
         LIMIT 1",
        [doc_id],
        |_| Ok(()),
    )
    .is_ok()
}

/// Whether the cabinet's hold file covers a trashed document.
fn held_in_cabinet(conn: &Connection, root: &Path, t: &Trashed) -> bool {
    let (client, hash): (Option<String>, String) = conn
        .query_row(
            "SELECT client_id, hash FROM document WHERE id = ?1",
            [&t.doc_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap_or((None, String::new()));
    crate::holds::covers(root, client.as_deref(), &t.rel_path, &hash)
}

/// What is in the trash, newest first: tombstoned documents whose file is there.
pub fn list(conn: &Connection, root: &Path) -> Result<Vec<Trashed>, String> {
    let mut st = conn
        .prepare(
            "SELECT id, rel_path, deleted_at, tombstone_reason, size FROM document
             WHERE deleted_at IS NOT NULL ORDER BY deleted_at DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = st
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        let (doc_id, rel_path, deleted_at, reason, size) = r.map_err(|e| e.to_string())?;
        let file = trash_file(root, &doc_id, &rel_path);
        if file.is_file() {
            let held = held(conn, &doc_id);
            out.push(Trashed {
                doc_id,
                rel_path,
                deleted_at,
                reason,
                size,
                file,
                held,
            });
        }
    }
    Ok(out)
}

/// Puts trashed documents back where they were (a free name beside it when that place is
/// taken now), as one Transaction. Returns the paths they got.
pub fn restore(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    items: &[Trashed],
) -> Result<Vec<String>, String> {
    if items.is_empty() {
        return Err("nothing selected".into());
    }
    let now = vf_core::now_rfc3339();
    let mut tx = Transaction::new(RESTORE_LABEL);
    let mut placed = Vec::new();
    let mut taken: Vec<PathBuf> = Vec::new();
    let mut allowed_extra: Vec<PathBuf> = vec![];
    for t in items {
        // From Intake: back to its inbox, where Intake files it (its row stays in the trash's
        // history, never live).
        if Path::new(&t.rel_path).is_absolute() {
            // Back where it was when that is still an inbox Intake watches; else into the
            // first inbox (the inbox was changed since). Never a folder made up again.
            let boxes = inboxes(root);
            let was = PathBuf::from(&t.rel_path);
            let mut dst = if inside_any(&was, &boxes) && was.parent().is_some_and(Path::is_dir) {
                was
            } else {
                let name = was.file_name().ok_or("no file name")?;
                boxes
                    .iter()
                    .find(|b| b.is_dir())
                    .ok_or("Intake has no inbox folder to return it to (Intake › Setup).")?
                    .join(name)
            };
            while dst.exists() || taken.contains(&dst) {
                dst = filer_intake::commit::free_path(&dst);
                if taken.contains(&dst) {
                    // free_path looks at the disk only: step past names this batch took.
                    let stem = dst.file_stem().map(|s| s.to_string_lossy().into_owned());
                    let ext = dst.extension().map(|e| e.to_string_lossy().into_owned());
                    let n = taken.len() + 2;
                    let name = match ext {
                        Some(e) => format!("{} ({n}).{e}", stem.unwrap_or_default()),
                        None => format!("{} ({n})", stem.unwrap_or_default()),
                    };
                    dst = dst.with_file_name(name);
                }
            }
            taken.push(dst.clone());
            if let Some(dir) = dst.parent() {
                allowed_extra.push(dir.to_path_buf());
            }
            tx = tx
                .file(FileOp::Move {
                    src: t.file.clone(),
                    dst: dst.clone(),
                })
                .catalog(Stmt::new(
                    "UPDATE document SET tombstone_reason = 'returned to Intake', rev = rev + 1,
                     updated_at = ?2 WHERE id = ?1",
                    vec![t.doc_id.clone().into(), now.clone().into()],
                ))
                .catalog_undo(Stmt::new(
                    "UPDATE document SET tombstone_reason = ?2, rev = rev + 1 WHERE id = ?1",
                    vec![t.doc_id.clone().into(), t.reason.clone().into()],
                ))
                .audit(AuditEntry::new(
                    "trash_restore",
                    Some(&t.doc_id),
                    json!({ "to": "intake" }),
                ));
            placed.push(dst.to_string_lossy().into_owned());
            continue;
        }
        let mut dst = root.join(&t.rel_path);
        // Free on disk, in the catalog, and among the ones restored together.
        let live = |c: &Connection, p: &Path| -> bool {
            let rel = p
                .strip_prefix(root)
                .map(|r| r.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            c.query_row(
                "SELECT 1 FROM document WHERE rel_path = ?1 AND deleted_at IS NULL",
                [rel],
                |_| Ok(()),
            )
            .is_ok()
        };
        if dst.exists() || live(conn, &dst) || taken.contains(&dst) {
            dst = filer_intake::commit::free_path(&dst);
            while live(conn, &dst) || taken.contains(&dst) {
                dst = filer_intake::commit::free_path(&dst);
            }
        }
        taken.push(dst.clone());
        let rel = dst
            .strip_prefix(root)
            .map_err(|_| "outside the cabinet".to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        tx = tx
            .file(FileOp::Move {
                src: t.file.clone(),
                dst: dst.clone(),
            })
            .catalog(Stmt::new(
                "UPDATE document SET deleted_at = NULL, tombstone_reason = NULL, rel_path = ?2,
                 rev = rev + 1, updated_at = ?3, device_id = ?4 WHERE id = ?1",
                vec![
                    t.doc_id.clone().into(),
                    rel.clone().into(),
                    now.clone().into(),
                    device_id.into(),
                ],
            ))
            .catalog_undo(Stmt::new(
                "UPDATE document SET deleted_at = ?2, tombstone_reason = ?3, rel_path = ?4,
                 rev = rev + 1 WHERE id = ?1",
                vec![
                    t.doc_id.clone().into(),
                    t.deleted_at.clone().into(),
                    t.reason.clone().into(),
                    t.rel_path.clone().into(),
                ],
            ))
            .catalog(Stmt::new(
                "DELETE FROM sync_pending WHERE doc_id = ?1 AND op = 'delete'",
                vec![t.doc_id.clone().into()],
            ))
            .catalog(Stmt::new(
                "INSERT INTO sync_pending (doc_id, op, since) VALUES (?1, 'put', ?2)
                 ON CONFLICT(doc_id, op) DO NOTHING",
                vec![t.doc_id.clone().into(), now.clone().into()],
            ))
            .audit(AuditEntry::new(
                "trash_restore",
                Some(&t.doc_id),
                json!({ "rel_path": rel, "was": t.rel_path }),
            ));
        placed.push(rel);
    }
    let mut allowed: Vec<&Path> = vec![root];
    allowed.extend(allowed_extra.iter().map(PathBuf::as_path));
    crate::fileops::commit_in(conn, &allowed, tx, device_id)?;
    Ok(placed)
}

/// Removes trashed documents for good: their files are deleted, the rows stay as tombstones
/// marked `purged`, one audit row each. Held documents are skipped. Returns how many went.
pub fn purge(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    items: &[Trashed],
    why: &str,
) -> Result<usize, String> {
    // Held in this catalog, or in the cabinet's hold file (a hold placed on another PC).
    let go: Vec<&Trashed> = items
        .iter()
        .filter(|t| !held(conn, &t.doc_id) && !held_in_cabinet(conn, root, t))
        .collect();
    if go.is_empty() {
        return Ok(0);
    }
    let mut tx = Transaction::new(PURGE_LABEL);
    for t in &go {
        tx = tx
            .catalog(Stmt::new(
                "UPDATE document SET tombstone_reason = 'purged', rev = rev + 1 WHERE id = ?1",
                vec![t.doc_id.clone().into()],
            ))
            .audit(AuditEntry::new(
                "trash_purge",
                Some(&t.doc_id),
                json!({ "rel_path": t.rel_path, "size": t.size, "why": why }),
            ));
    }
    // Catalog and audit first: a file that then cannot be deleted is reported, and stays
    // listed nowhere (its row says purged) until the next integrity sweep finds it.
    tx.commit(conn, &mut NoFileOps, &Actor::device(device_id))
        .map_err(|e| e.to_string())?;
    let mut failed = Vec::new();
    for t in &go {
        if let Err(e) = std::fs::remove_file(&t.file) {
            failed.push(format!("{}: {e}", t.file.display()));
            continue;
        }
        if let Some(dir) = t.file.parent() {
            let _ = std::fs::remove_dir(dir); // only when empty
        }
    }
    if failed.is_empty() {
        Ok(go.len())
    } else {
        Err(format!("could not delete: {}", failed.join("; ")))
    }
}

/// Empties what has been in the trash longer than [`AUTO_EMPTY_DAYS`] (held documents stay).
pub fn auto_empty(conn: &mut Connection, root: &Path, device_id: &str) -> Result<usize, String> {
    let cutoff = vf_core::rfc3339_days_ago(AUTO_EMPTY_DAYS);
    let old: Vec<Trashed> = list(conn, root)?
        .into_iter()
        .filter(|t| t.deleted_at < cutoff)
        .collect();
    let n = purge(conn, root, device_id, &old, "in the trash over 90 days")?;
    // Files no trashed document owns (old content set aside by a change whose version was
    // already kept) and the temp folder's leftovers go after the same 90 days.
    let cutoff_t = std::time::SystemTime::now()
        - std::time::Duration::from_secs(AUTO_EMPTY_DAYS as u64 * 86_400);
    for dir in [cabinet::trash_dir(root), cabinet::tmp_dir(root)] {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let Ok(m) = e.metadata() else { continue };
            if m.is_file() && m.modified().is_ok_and(|t| t < cutoff_t) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    Ok(n)
}

/// `_Vibe/trash_pin.json`: the admin password's hash (Q-29 "firm PIN", Q-49 admin password:
/// the same secret, so a PIN set before is the admin password). Deleting the file clears it.
pub fn pin_file(root: &Path) -> PathBuf {
    root.join(cabinet::VIBE_DIR).join("trash_pin.json")
}

fn stored_pin(root: &Path) -> Option<String> {
    let t = std::fs::read_to_string(pin_file(root)).ok()?;
    let v: serde_json::Value = serde_json::from_str(&t).ok()?;
    v.get("hash")?.as_str().map(str::to_string)
}

/// Whether a password is set. Fails closed (QA 2026-11-04): a password file that is there but
/// cannot be read right now (a share hiccup, a write in progress, a damaged file) counts as set,
/// so Admin stays locked rather than opening. Only a file that is not there means "no password".
pub fn has_pin(root: &Path) -> bool {
    match std::fs::read_to_string(pin_file(root)) {
        // Removed: the file says so, so sync carries the removal (owner 2026-11-04).
        Ok(t) => !is_removed_marker(&t),
        Err(e) => e.kind() != std::io::ErrorKind::NotFound,
    }
}

/// The password file's content once the password is removed: kept (not deleted) so B2 sync
/// carries the removal to the other places instead of bringing the old password back.
pub const REMOVED_MARKER: &str = "{\"removed\": true}
";

fn is_removed_marker(text: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|v| v.get("removed").and_then(|r| r.as_bool()))
        .unwrap_or(false)
}

/// Whether `pin` is the firm PIN. After three wrong tries in a row it is not checked for a while
/// ([`pin_wait`]) and this is `false`.
pub fn check_pin(root: &Path, pin: &str) -> bool {
    stored_pin(root).is_some_and(|h| vf_security::pin::check(pin, &h).unwrap_or(false))
}

/// Seconds before the PIN is checked again (after wrong tries).
pub fn pin_wait() -> u64 {
    vf_security::pin::wait_secs()
}

/// Transaction label of a PIN change (left off the undo stack).
pub const PIN_LABEL: &str = "firm_pin";

/// Sets the firm PIN; changing one needs the current PIN. Audited (`firm_pin_set`): a PIN set
/// again after its file was deleted is recorded as a reset, so going around the PIN shows.
pub fn set_pin(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    current: Option<&str>,
    new: &str,
) -> Result<(), String> {
    let had = has_pin(root);
    if had && !current.is_some_and(|c| check_pin(root, c)) {
        return Err("the current admin password is not right".into());
    }
    let set_before = conn
        .query_row(
            "SELECT 1 FROM audit WHERE action = 'firm_pin_set' LIMIT 1",
            [],
            |_| Ok(()),
        )
        .is_ok();
    let hash = vf_security::pin::hash(new)?;
    let body = json!({ "hash": hash, "set": vf_core::now_rfc3339() });
    cabinet::write_replace(
        &pin_file(root),
        &serde_json::to_vec_pretty(&body).expect("serializes"),
    )?;
    let how = if had {
        "changed"
    } else if set_before {
        "reset (the PIN file had been removed)"
    } else {
        "first set"
    };
    Transaction::new(PIN_LABEL)
        .audit(AuditEntry::new("firm_pin_set", None, json!({ "how": how })))
        .commit(conn, &mut NoFileOps, &Actor::device(device_id))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Unlocks Admin and the Sync settings with the admin password (Q-49). Both outcomes are
/// audited (`admin_unlock`, `admin_unlock_refused`; no text). `Err` = wait that many seconds
/// (too many wrong tries).
pub fn unlock_admin(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    password: &str,
) -> Result<bool, u64> {
    let Some(h) = stored_pin(root) else {
        // No password, unless the file is there but unreadable: then locked.
        return Ok(!has_pin(root));
    };
    let ok = vf_security::pin::check(password, &h)?;
    let _ = Transaction::new(PIN_LABEL)
        .audit(AuditEntry::new(
            if ok {
                "admin_unlock"
            } else {
                "admin_unlock_refused"
            },
            None,
            json!({}),
        ))
        .commit(conn, &mut NoFileOps, &Actor::device(device_id));
    Ok(ok)
}

/// Removes the admin password (the current one needed). Audited `firm_pin_removed`.
pub fn remove_pin(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    current: &str,
) -> Result<(), String> {
    if !check_pin(root, current) {
        return Err("the current admin password is not right".into());
    }
    cabinet::write_replace(&pin_file(root), REMOVED_MARKER.as_bytes())
        .map_err(|e| e.to_string())?;
    Transaction::new(PIN_LABEL)
        .audit(AuditEntry::new("firm_pin_removed", None, json!({})))
        .commit(conn, &mut NoFileOps, &Actor::device(device_id))
        .map(|_| ())
        .map_err(|e| e.to_string())
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
        for (id, name) in [("d1", "w2.pdf"), ("d2", "k1.pdf"), ("d3", "1099.pdf")] {
            let rel = format!("A (1)/2025/{name}");
            std::fs::write(root.join(&rel), name.as_bytes()).unwrap();
            conn.execute(
                "INSERT INTO document (id, client_id, tax_year, rel_path, hash, size, filed_at, device_id, updated_at)
                 VALUES (?1, 'c1', 2025, ?2, 'h', 6, 't', 'dev', 't')",
                [id, &rel],
            )
            .unwrap();
        }
        (t, root, conn)
    }

    /// Q-61: a file taken out of Intake's inbox is in the trash (never live), Undo puts it back
    /// in the inbox, and Restore returns it there too.
    #[test]
    fn intake_files_go_to_the_trash_and_back_to_the_inbox() {
        let (t, root, mut conn) = fx();
        let inbox = t.path().join("inbox");
        std::fs::create_dir_all(&inbox).unwrap();
        // Intake's settings name the inbox (Restore returns files only to an inbox it has).
        let master = crate::clients::master_dir(&root);
        std::fs::create_dir_all(&master).unwrap();
        let (mut st, _) = filer_intake::config::Settings::load_or_default(&master);
        st.inboxes = vec![inbox.clone()];
        st.save(&master).unwrap();
        let f = inbox.join("scan0001.pdf");
        std::fs::write(&f, b"invented").unwrap();
        // Not from an inbox: refused.
        let elsewhere = t.path().join("elsewhere.pdf");
        std::fs::write(&elsewhere, b"x").unwrap();
        assert!(
            delete_inbox_files(
                &mut conn,
                &root,
                "dev",
                std::slice::from_ref(&elsewhere),
                std::slice::from_ref(&inbox)
            )
            .is_err()
        );
        let tx = delete_inbox_files(
            &mut conn,
            &root,
            "dev",
            std::slice::from_ref(&f),
            std::slice::from_ref(&inbox),
        )
        .unwrap();
        assert!(!f.exists());
        let listed = list(&conn, &root).unwrap();
        let it = listed
            .iter()
            .find(|x| x.rel_path.ends_with("scan0001.pdf"))
            .unwrap();
        assert_eq!(it.reason.as_deref(), Some(FROM_INTAKE));
        let live: i64 = conn
            .query_row(
                "SELECT count(*) FROM document WHERE deleted_at IS NULL AND rel_path LIKE '%scan0001%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(live, 0, "never a live document");
        // Undo: back in the inbox, no row left.
        crate::fileops::undo(&mut conn, &root, &tx, "dev").unwrap();
        assert!(f.is_file());
        assert!(
            list(&conn, &root)
                .unwrap()
                .iter()
                .all(|x| !x.rel_path.ends_with("scan0001.pdf"))
        );
        // Again, then Restore from the trash: back to the inbox.
        delete_inbox_files(
            &mut conn,
            &root,
            "dev",
            std::slice::from_ref(&f),
            std::slice::from_ref(&inbox),
        )
        .unwrap();
        let it = list(&conn, &root)
            .unwrap()
            .into_iter()
            .find(|x| x.rel_path.ends_with("scan0001.pdf"))
            .unwrap();
        let placed = restore(&mut conn, &root, "dev", &[it]).unwrap();
        assert_eq!(placed, [f.to_string_lossy().into_owned()]);
        assert!(f.is_file());
        // Two trashed files of one name restored together: both come back, side by side.
        delete_inbox_files(
            &mut conn,
            &root,
            "dev",
            std::slice::from_ref(&f),
            std::slice::from_ref(&inbox),
        )
        .unwrap();
        std::fs::write(&f, b"invented again").unwrap();
        delete_inbox_files(
            &mut conn,
            &root,
            "dev",
            std::slice::from_ref(&f),
            std::slice::from_ref(&inbox),
        )
        .unwrap();
        let both: Vec<Trashed> = list(&conn, &root)
            .unwrap()
            .into_iter()
            .filter(|x| x.rel_path.ends_with("scan0001.pdf"))
            .collect();
        assert_eq!(both.len(), 2);
        let placed = restore(&mut conn, &root, "dev", &both).unwrap();
        assert_eq!(placed.len(), 2);
        assert_ne!(placed[0], placed[1]);
        assert!(placed.iter().all(|p| Path::new(p).is_file()));
        // The inbox moved since: the file goes to the inbox Intake has now.
        let new_inbox = t.path().join("inbox2");
        std::fs::create_dir_all(&new_inbox).unwrap();
        let g = inbox.join("late.pdf");
        std::fs::write(&g, b"late").unwrap();
        delete_inbox_files(
            &mut conn,
            &root,
            "dev",
            std::slice::from_ref(&g),
            std::slice::from_ref(&inbox),
        )
        .unwrap();
        st.inboxes = vec![new_inbox.clone()];
        st.save(&master).unwrap();
        let it = list(&conn, &root)
            .unwrap()
            .into_iter()
            .find(|x| x.rel_path.ends_with("late.pdf"))
            .unwrap();
        restore(&mut conn, &root, "dev", &[it]).unwrap();
        assert!(new_inbox.join("late.pdf").is_file());
        assert!(
            list(&conn, &root)
                .unwrap()
                .iter()
                .all(|x| !x.rel_path.ends_with("scan0001.pdf"))
        );
    }

    #[test]
    fn delete_restore_and_undo() {
        let (_t, root, mut conn) = fx();
        let ids = vec!["d1".to_string(), "d2".to_string()];
        let tx = delete(&mut conn, &root, "dev", &ids, "deleted").unwrap();
        assert!(!root.join("A (1)/2025/w2.pdf").exists());
        let listed = list(&conn, &root).unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed[0].file.is_file());
        // Undo puts both back.
        crate::fileops::undo(&mut conn, &root, &tx, "dev").unwrap();
        assert!(root.join("A (1)/2025/w2.pdf").is_file());
        assert!(list(&conn, &root).unwrap().is_empty());

        // Delete again; a new file took w2.pdf's place: restore picks a free name.
        delete(&mut conn, &root, "dev", &ids, "deleted").unwrap();
        std::fs::write(root.join("A (1)/2025/w2.pdf"), b"other").unwrap();
        let listed = list(&conn, &root).unwrap();
        let mut placed = restore(&mut conn, &root, "dev", &listed).unwrap();
        placed.sort();
        assert_eq!(placed, ["A (1)/2025/k1.pdf", "A (1)/2025/w2 (2).pdf"]);
        assert_eq!(
            std::fs::read(root.join("A (1)/2025/w2 (2).pdf")).unwrap(),
            b"w2.pdf"
        );
        let live: i64 = conn
            .query_row(
                "SELECT count(*) FROM document WHERE deleted_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(live, 3);
    }

    #[test]
    fn purge_skips_held_documents_and_auto_empty_waits_90_days() {
        let (_t, root, mut conn) = fx();
        let ids: Vec<String> = ["d1", "d2", "d3"].map(String::from).to_vec();
        delete(&mut conn, &root, "dev", &ids, "deleted").unwrap();
        conn.execute(
            "INSERT INTO legal_hold (id, doc_id, reason, since, device_id, updated_at)
             VALUES ('h1', 'd3', 'IRS exam', 't', 'dev', 't')",
            [],
        )
        .unwrap();
        // Nothing is old enough yet.
        assert_eq!(auto_empty(&mut conn, &root, "dev").unwrap(), 0);
        // d1 has been in the trash for 91 days.
        conn.execute(
            "UPDATE document SET deleted_at = ?1 WHERE id IN ('d1', 'd3')",
            [vf_core::rfc3339_days_ago(91)],
        )
        .unwrap();
        assert_eq!(
            auto_empty(&mut conn, &root, "dev").unwrap(),
            1,
            "d3 is held"
        );
        let left: Vec<String> = list(&conn, &root)
            .unwrap()
            .into_iter()
            .map(|t| t.doc_id)
            .collect();
        assert_eq!(left.len(), 2);
        assert!(left.contains(&"d3".to_string()));
        let reason: String = conn
            .query_row(
                "SELECT tombstone_reason FROM document WHERE id = 'd1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reason, "purged");
        let purged: i64 = conn
            .query_row(
                "SELECT count(*) FROM audit WHERE action = 'trash_purge'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(purged, 1);
        assert!(!cabinet::trash_dir(&root).join("d1").exists());
    }

    #[test]
    fn firm_pin_is_set_checked_changed_and_audited() {
        let (_t, root, mut conn) = fx();
        std::fs::create_dir_all(root.join(cabinet::VIBE_DIR)).unwrap();
        assert!(!has_pin(&root));
        set_pin(&mut conn, &root, "dev", None, "482193").unwrap();
        assert!(has_pin(&root) && check_pin(&root, "482193") && !check_pin(&root, "000000"));
        let stored = std::fs::read_to_string(pin_file(&root)).unwrap();
        assert!(!stored.contains("482193"), "only a hash is stored");
        assert!(set_pin(&mut conn, &root, "dev", None, "111111").is_err());
        assert!(set_pin(&mut conn, &root, "dev", Some("000000"), "111111").is_err());
        set_pin(&mut conn, &root, "dev", Some("482193"), "111111").unwrap();
        assert!(check_pin(&root, "111111"));
        // Someone deletes the PIN file and sets their own: recorded as a reset.
        std::fs::remove_file(pin_file(&root)).unwrap();
        set_pin(&mut conn, &root, "dev", None, "222222").unwrap();
        let hows: Vec<String> = conn
            .prepare("SELECT json_extract(detail, '$.how') FROM audit WHERE action = 'firm_pin_set' ORDER BY seq")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            hows,
            [
                "first set",
                "changed",
                "reset (the PIN file had been removed)"
            ]
        );
        let dump = format!(
            "{:?}",
            conn.query_row("SELECT group_concat(detail) FROM audit", [], |r| r
                .get::<_, String>(0))
                .unwrap()
        );
        assert!(
            !dump.contains("2222"),
            "the PIN is never in the audit trail"
        );
    }

    /// Removing the password leaves the removed marker (so sync carries it), and a new one can
    /// be set after (owner 2026-11-04).
    #[test]
    fn removed_password_is_a_marker() {
        let (_t, root, mut conn) = fx();
        std::fs::create_dir_all(root.join(cabinet::VIBE_DIR)).unwrap();
        set_pin(&mut conn, &root, "dev", None, "482193").unwrap();
        remove_pin(&mut conn, &root, "dev", "482193").unwrap();
        assert!(pin_file(&root).is_file(), "kept as the removed marker");
        assert!(!has_pin(&root));
        assert_eq!(unlock_admin(&mut conn, &root, "dev", "x"), Ok(true));
        set_pin(&mut conn, &root, "dev", None, "555555").unwrap();
        assert!(has_pin(&root) && check_pin(&root, "555555"));
    }

    /// A password file that is there but unreadable keeps Admin locked (QA 2026-11-04).
    #[test]
    fn a_damaged_password_file_fails_closed() {
        let (_t, root, mut conn) = fx();
        std::fs::create_dir_all(root.join(cabinet::VIBE_DIR)).unwrap();
        std::fs::write(pin_file(&root), "{ half written").unwrap();
        assert!(has_pin(&root));
        assert!(!check_pin(&root, "anything"));
        assert_eq!(unlock_admin(&mut conn, &root, "dev", "anything"), Ok(false));
        assert!(set_pin(&mut conn, &root, "dev", None, "123456").is_err());
    }
}
