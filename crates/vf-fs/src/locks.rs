//! Check-out / check-in across workstations (PHASES PA-16, owner 2026-11-04 Q-51: lock files on
//! the server, each PC keeping its own catalog). A check-out is `_Vibe/locks/<doc uuid>.lock`.
//! Every PC copies the locks other PCs hold into its catalog's `lock` table ([`sync`]) so Browse
//! shows who; the edits also read the lock files themselves at the moment they change anything
//! ([`refuse_held`]), so a stale copy never lets an edit through. This PC's own check-outs are
//! only the files.
//!
//! Intake: one PC files at a time, by the same kind of file (`_Vibe/intake.lock`) with a
//! heartbeat; a holder silent for [`INTAKE_STALE`] is taken over (audited).
//!
//! Every lock file is written whole before it appears (written beside it, then moved into place
//! without replacing anything: one PC wins, and nobody ever reads a half-written lock), and is
//! taken away by moving it aside and checking it is still the one that was meant (so two PCs
//! taking over at once cannot remove each other's new lock). A lock change whose audit row
//! cannot be written is undone.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use vf_core::audit::AuditEntry;
use vf_core::db::Connection;
use vf_core::tx::{Actor, NoFileOps, Stmt, Transaction};

use crate::cabinet;

pub const LOCKS_DIR: &str = "locks";
pub const INTAKE_LOCK: &str = "intake.lock";
/// An Intake holder that has not written its heartbeat for this long is taken over.
pub const INTAKE_STALE: Duration = Duration::from_secs(45);

/// Who holds a lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Held {
    #[serde(default)]
    pub doc_id: String,
    /// Windows user.
    pub user: String,
    /// Computer name.
    pub machine: String,
    pub device_id: String,
    /// RFC 3339 UTC.
    pub since: String,
}

impl Held {
    pub fn who(&self) -> String {
        format!("{} on {}", self.user, self.machine)
    }
}

fn locks_dir(root: &Path) -> PathBuf {
    root.join(cabinet::VIBE_DIR).join(LOCKS_DIR)
}

fn lock_path(root: &Path, doc_id: &str) -> PathBuf {
    let safe: String = doc_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    locks_dir(root).join(format!("{safe}.lock"))
}

fn machine() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown".into())
}

// ---------------------------------------------------------------------------------------------
// Placing and taking away lock files.

/// Writes `bytes` to `path` whole, only when `path` does not exist. `Ok(false)`: it exists.
fn place(path: &Path, bytes: &[u8]) -> std::io::Result<bool> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".tmp-{}", vf_core::new_id()));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    match filer_intake::commit::move_no_replace(&tmp, path) {
        Ok(()) => Ok(true),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            if path.exists() { Ok(false) } else { Err(e) }
        }
    }
}

/// Moves `path` aside if it still holds `expect`. `Some(aside)`: moved (delete it, or put it
/// back with [`put_back`]); `None`: gone or another lock now.
fn take_aside(path: &Path, expect: &[u8]) -> Option<PathBuf> {
    let aside = path.with_extension(format!("gone-{}", vf_core::new_id()));
    filer_intake::commit::move_no_replace(path, &aside).ok()?;
    if std::fs::read(&aside).ok().as_deref() == Some(expect) {
        return Some(aside);
    }
    // Not the one meant (replaced since it was read): back where it was.
    if filer_intake::commit::move_no_replace(&aside, path).is_err() {
        let _ = std::fs::remove_file(&aside);
    }
    None
}

fn put_back(aside: &Path, path: &Path) {
    let _ = filer_intake::commit::move_no_replace(aside, path);
}

fn audited(
    conn: &mut Connection,
    device_id: &str,
    action: &str,
    doc: Option<&str>,
    detail: serde_json::Value,
    catalog: Option<Stmt>,
) -> Result<(), String> {
    let mut tx = Transaction::new(action).audit(AuditEntry::new(action, doc, detail));
    if let Some(s) = catalog {
        tx = tx.catalog(s);
    }
    tx.commit(conn, &mut NoFileOps, &Actor::device(device_id))
        .map(|_| ())
        .map_err(|e| format!("not recorded in the audit trail: {e}"))
}

/// A lock file's holder; one that cannot be read (written by an older build, or damaged) is
/// shown as held by someone unknown, so it can still be broken.
fn read_held(path: &Path) -> Option<(Held, Vec<u8>)> {
    let bytes = std::fs::read(path).ok()?;
    let held = serde_json::from_slice::<Held>(&bytes).unwrap_or_else(|_| Held {
        doc_id: String::new(),
        user: "someone".into(),
        machine: "an unknown PC".into(),
        device_id: String::new(),
        since: std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .map(|t| {
                time::OffsetDateTime::from(t)
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default()
            })
            .unwrap_or_default(),
    });
    Some((held, bytes))
}

// ---------------------------------------------------------------------------------------------
// Check-out.

/// The holder of `doc_id`'s check-out, when it is checked out.
pub fn holder(root: &Path, doc_id: &str) -> Option<Held> {
    read_held(&lock_path(root, doc_id)).map(|(mut h, _)| {
        if h.doc_id.is_empty() {
            h.doc_id = doc_id.into();
        }
        h
    })
}

/// Every check-out in the cabinet.
pub fn all(root: &Path) -> Vec<Held> {
    std::fs::read_dir(locks_dir(root))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "lock"))
        .filter_map(|e| {
            let stem = e.path().file_stem()?.to_string_lossy().into_owned();
            holder(root, &stem)
        })
        .collect()
}

/// The documents this PC has checked out.
pub fn mine(root: &Path, device_id: &str) -> HashSet<String> {
    all(root)
        .into_iter()
        .filter(|h| h.device_id == device_id)
        .map(|h| h.doc_id)
        .collect()
}

/// Checks `doc_id` out to this PC. Refused, naming the holder, when another PC has it; this PC
/// checking out again is fine. Audited `checkout` (undone when that cannot be recorded).
pub fn checkout(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    user: &str,
    doc_id: &str,
) -> Result<(), String> {
    let p = lock_path(root, doc_id);
    let held = Held {
        doc_id: doc_id.into(),
        user: user.into(),
        machine: machine(),
        device_id: device_id.into(),
        since: vf_core::now_rfc3339(),
    };
    let bytes = serde_json::to_vec_pretty(&held).expect("serializes");
    if place(&p, &bytes).map_err(|e| format!("cannot check out: {e}"))? {
        if let Err(e) = audited(conn, device_id, "checkout", Some(doc_id), json!({}), None) {
            if let Some(a) = take_aside(&p, &bytes) {
                let _ = std::fs::remove_file(a);
            }
            return Err(e);
        }
        return Ok(());
    }
    match holder(root, doc_id) {
        Some(h) if h.device_id == device_id => Ok(()),
        Some(h) => Err(format!(
            "checked out by {} since {}",
            h.who(),
            h.since[..h.since.len().min(16)].replace('T', " ")
        )),
        None => Err("someone is checking it out right now; try again".into()),
    }
}

/// What [`remove_audited`] records.
struct Removal<'a> {
    action: &'a str,
    doc: &'a str,
    detail: serde_json::Value,
    catalog: Option<Stmt>,
}

/// Takes a lock file away and audits it; put back when the audit cannot be written.
fn remove_audited(
    conn: &mut Connection,
    device_id: &str,
    path: &Path,
    bytes: &[u8],
    r: Removal<'_>,
) -> Result<(), String> {
    let aside = take_aside(path, bytes).ok_or("it changed meanwhile; look again")?;
    match audited(conn, device_id, r.action, Some(r.doc), r.detail, r.catalog) {
        Ok(()) => {
            let _ = std::fs::remove_file(aside);
            Ok(())
        }
        Err(e) => {
            put_back(&aside, path);
            Err(e)
        }
    }
}

/// Checks `doc_id` back in (this PC's check-out only). Audited `checkin`.
pub fn checkin(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    doc_id: &str,
) -> Result<(), String> {
    let p = lock_path(root, doc_id);
    let Some((h, bytes)) = read_held(&p) else {
        return Ok(());
    };
    if h.device_id != device_id {
        return Err(format!("checked out by {}, not this PC", h.who()));
    }
    remove_audited(
        conn,
        device_id,
        &p,
        &bytes,
        Removal {
            action: "checkin",
            doc: doc_id,
            detail: json!({}),
            catalog: None,
        },
    )
}

/// Breaks another PC's check-out (admin, behind the admin password in the app). Audited
/// `lock_broken` with who held it, in the same Transaction as this catalog's lock row.
pub fn break_lock(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    doc_id: &str,
) -> Result<Held, String> {
    let p = lock_path(root, doc_id);
    let (h, bytes) = read_held(&p).ok_or("it is not checked out")?;
    remove_audited(
        conn,
        device_id,
        &p,
        &bytes,
        Removal {
            action: "lock_broken",
            doc: doc_id,
            detail: json!({ "held_by": h.user, "machine": h.machine, "since": h.since }),
            catalog: Some(Stmt::new(
                "DELETE FROM lock WHERE doc_id = ?1",
                vec![doc_id.to_string().into()],
            )),
        },
    )?;
    Ok(h)
}

/// Copies the check-outs other PCs hold into this catalog's `lock` table (only theirs: this
/// PC's own check-outs do not lock it out; only documents this catalog has). Returns whether
/// anything changed.
pub fn sync(conn: &mut Connection, root: &Path, device_id: &str) -> Result<bool, String> {
    let others: Vec<Held> = all(root)
        .into_iter()
        .filter(|h| {
            h.device_id != device_id
                && conn
                    .query_row("SELECT 1 FROM document WHERE id = ?1", [&h.doc_id], |_| {
                        Ok(())
                    })
                    .is_ok()
        })
        .collect();
    let want: HashSet<(String, String)> = others
        .iter()
        .map(|h| (h.doc_id.clone(), h.device_id.clone()))
        .collect();
    let have: HashSet<(String, String)> = {
        let mut st = conn
            .prepare("SELECT doc_id, device_id FROM lock")
            .map_err(|e| e.to_string())?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?
            .flatten()
            .collect()
    };
    if want == have {
        return Ok(false);
    }
    // A local mirror of the files, not a change of the cabinet: no audit row.
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM lock", [])
        .map_err(|e| e.to_string())?;
    for h in &others {
        tx.execute(
            "INSERT OR REPLACE INTO lock (doc_id, user, device_id, since) VALUES (?1, ?2, ?3, ?4)",
            (&h.doc_id, h.who(), &h.device_id, &h.since),
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(true)
}

/// Refuses when one of `ids` is checked out on another PC (the catalog's mirror of the lock
/// files, [`sync`]).
pub fn refuse_locked(conn: &Connection, ids: &[String]) -> Result<(), String> {
    let mut st = conn
        .prepare_cached(
            "SELECT l.user, d.rel_path FROM lock l JOIN document d ON d.id = l.doc_id
             WHERE l.doc_id = ?1",
        )
        .map_err(|e| e.to_string())?;
    for id in ids {
        let held: Option<(String, String)> =
            st.query_row([id], |r| Ok((r.get(0)?, r.get(1)?))).ok();
        if let Some((who, rel)) = held {
            let name = rel.rsplit('/').next().unwrap_or(&rel).to_string();
            return Err(format!("{name} is checked out by {who}"));
        }
    }
    Ok(())
}

/// Refuses when one of `ids` is checked out on another PC, reading the lock files now (the
/// edits call this at the moment they change something, whatever the catalog's copy says).
pub fn refuse_held(
    conn: &Connection,
    root: &Path,
    device_id: &str,
    ids: &[String],
) -> Result<(), String> {
    for id in ids {
        if let Some(h) = holder(root, id).filter(|h| h.device_id != device_id) {
            let name = conn
                .query_row("SELECT rel_path FROM document WHERE id = ?1", [id], |r| {
                    r.get::<_, String>(0)
                })
                .map(|rel| rel.rsplit('/').next().unwrap_or(&rel).to_string())
                .unwrap_or_else(|_| "the document".into());
            return Err(format!("{name} is checked out by {}", h.who()));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Intake: one PC files at a time.

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IntakeHeld {
    #[serde(flatten)]
    held: Held,
    /// RFC 3339 UTC of the holder's last heartbeat.
    beat: String,
}

fn intake_path(root: &Path) -> PathBuf {
    root.join(cabinet::VIBE_DIR).join(INTAKE_LOCK)
}

fn age(rfc3339: &str) -> Option<Duration> {
    let t = time::OffsetDateTime::parse(rfc3339, &time::format_description::well_known::Rfc3339)
        .ok()?;
    let d = time::OffsetDateTime::now_utc() - t;
    Some(Duration::from_secs_f64(d.as_seconds_f64().max(0.0)))
}

/// What taking Intake gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntakeLock {
    /// This PC has Intake (taken now, or already).
    Mine,
    /// Another PC has it.
    Theirs(Held),
}

/// Takes Intake for this PC, or says who has it. A holder whose heartbeat is older than
/// [`INTAKE_STALE`] is taken over (audited `intake_lock_takeover`). A lock file that cannot be
/// read counts as fresh for [`INTAKE_STALE`] after it was written (a PC may be writing it).
pub fn take_intake(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    user: &str,
) -> Result<IntakeLock, String> {
    let p = intake_path(root);
    for _ in 0..3 {
        let now = vf_core::now_rfc3339();
        let me = IntakeHeld {
            held: Held {
                doc_id: String::new(),
                user: user.into(),
                machine: machine(),
                device_id: device_id.into(),
                since: now.clone(),
            },
            beat: now,
        };
        let mine = serde_json::to_vec_pretty(&me).expect("serializes");
        if place(&p, &mine).map_err(|e| format!("cannot take Intake: {e}"))? {
            return Ok(IntakeLock::Mine);
        }
        let Ok(bytes) = std::fs::read(&p) else {
            continue; // let go just now: try again
        };
        let fresh_file = std::fs::metadata(&p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|a| a < INTAKE_STALE);
        match serde_json::from_slice::<IntakeHeld>(&bytes) {
            Ok(c) if c.held.device_id == device_id => return Ok(IntakeLock::Mine),
            Ok(c) if age(&c.beat).is_some_and(|a| a < INTAKE_STALE) => {
                return Ok(IntakeLock::Theirs(c.held));
            }
            Err(_) if fresh_file => {
                return Ok(IntakeLock::Theirs(Held {
                    doc_id: String::new(),
                    user: "someone".into(),
                    machine: "another PC".into(),
                    device_id: String::new(),
                    since: vf_core::now_rfc3339(),
                }));
            }
            stale => {
                // Silent too long (a PC that crashed or lost the network): take over, only if
                // the file is still the one judged stale.
                let from = stale
                    .map(|c| (c.held.who(), c.beat))
                    .unwrap_or_else(|_| ("an unreadable lock".into(), String::new()));
                if let Some(aside) = take_aside(&p, &bytes) {
                    let _ = std::fs::remove_file(aside);
                    audited(
                        conn,
                        device_id,
                        "intake_lock_takeover",
                        None,
                        json!({ "from": from.0, "last_heartbeat": from.1 }),
                        None,
                    )?;
                }
            }
        }
    }
    Err("someone else is taking Intake right now; this tab tries again in a few seconds".into())
}

/// The heartbeat (every 10 s while this PC has Intake, whatever tab shows). `false` when it no
/// longer has it. The lock is replaced only if it is still this PC's.
pub fn intake_heartbeat(root: &Path, device_id: &str) -> bool {
    let p = intake_path(root);
    let Ok(bytes) = std::fs::read(&p) else {
        return false;
    };
    let Ok(mut cur) = serde_json::from_slice::<IntakeHeld>(&bytes) else {
        return false;
    };
    if cur.held.device_id != device_id {
        return false;
    }
    cur.beat = vf_core::now_rfc3339();
    let Some(aside) = take_aside(&p, &bytes) else {
        return false;
    };
    let placed = place(&p, &serde_json::to_vec_pretty(&cur).expect("serializes"));
    let _ = std::fs::remove_file(aside);
    // `false`: another PC took it in the moment it was aside.
    matches!(placed, Ok(true))
}

/// Lets go of Intake (leaving it, closing Vibe Files).
pub fn release_intake(root: &Path, device_id: &str) {
    let p = intake_path(root);
    let Ok(bytes) = std::fs::read(&p) else {
        return;
    };
    let mine =
        serde_json::from_slice::<IntakeHeld>(&bytes).is_ok_and(|c| c.held.device_id == device_id);
    if mine && let Some(aside) = take_aside(&p, &bytes) {
        let _ = std::fs::remove_file(aside);
    }
}

#[cfg(test)]
mod tests {
    use vf_core::db::Storage;

    use super::*;

    fn fx() -> (tempfile::TempDir, PathBuf, Connection, Connection) {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("cab");
        std::fs::create_dir_all(root.join(cabinet::VIBE_DIR)).unwrap();
        let open = |n: &str| {
            let c = vf_core::db::open(&t.path().join(n), Storage::Local).unwrap();
            c.execute(
                "INSERT INTO document (id, rel_path, hash, size, filed_at, device_id, updated_at)
                 VALUES ('d1', 'A/x.pdf', 'h', 1, 't', 'pc-a', 't')",
                [],
            )
            .unwrap();
            c
        };
        let (a, b) = (open("a.db"), open("b.db"));
        (t, root, a, b)
    }

    /// PA-16 done-when: two users cannot both check out the same document; the other PC's
    /// catalog sees the lock; edits read the files themselves; check-in frees it; a broken lock
    /// is audited.
    #[test]
    fn two_pcs_cannot_both_check_out() {
        let (_t, root, mut a, mut b) = fx();
        checkout(&mut a, &root, "pc-a", "anna", "d1").unwrap();
        checkout(&mut a, &root, "pc-a", "anna", "d1").unwrap(); // again: fine
        let e = checkout(&mut b, &root, "pc-b", "ben", "d1").unwrap_err();
        assert!(e.contains("anna"), "{e}");
        // B's edits refuse it at once, before any sync.
        let ids = vec!["d1".to_string()];
        assert!(
            refuse_held(&b, &root, "pc-b", &ids)
                .unwrap_err()
                .contains("anna")
        );
        assert!(refuse_held(&a, &root, "pc-a", &ids).is_ok(), "not A's own");
        assert!(sync(&mut b, &root, "pc-b").unwrap());
        assert!(!sync(&mut b, &root, "pc-b").unwrap(), "in step: no change");
        let n: i64 = b
            .query_row("SELECT count(*) FROM lock WHERE doc_id = 'd1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(n, 1, "B's catalog sees A's check-out");
        sync(&mut a, &root, "pc-a").unwrap();
        let n: i64 = a
            .query_row("SELECT count(*) FROM lock", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
        assert!(
            checkin(&mut b, &root, "pc-b", "d1").is_err(),
            "only A checks in"
        );
        checkin(&mut a, &root, "pc-a", "d1").unwrap();
        assert!(sync(&mut b, &root, "pc-b").unwrap());
        checkout(&mut b, &root, "pc-b", "ben", "d1").unwrap();
        let h = break_lock(&mut a, &root, "pc-a", "d1").unwrap();
        assert_eq!(h.user, "ben");
        let n: i64 = a
            .query_row(
                "SELECT count(*) FROM audit WHERE action = 'lock_broken'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    /// A lock for a document this catalog does not have does not make sync change every time.
    #[test]
    fn locks_on_unknown_documents_are_left_out() {
        let (_t, root, mut a, mut b) = fx();
        checkout(&mut a, &root, "pc-a", "anna", "not-here").unwrap();
        assert!(!sync(&mut b, &root, "pc-b").unwrap());
    }

    /// A damaged (empty) lock file can be seen and broken.
    #[test]
    fn a_damaged_lock_can_be_broken() {
        let (_t, root, mut a, _b) = fx();
        std::fs::create_dir_all(locks_dir(&root)).unwrap();
        std::fs::write(lock_path(&root, "d1"), b"").unwrap();
        let h = holder(&root, "d1").expect("shown as held");
        assert_eq!(h.user, "someone");
        break_lock(&mut a, &root, "pc-a", "d1").unwrap();
        checkout(&mut a, &root, "pc-a", "anna", "d1").unwrap();
    }

    /// Many threads race for one document: exactly one wins.
    #[test]
    fn racing_check_outs_have_one_winner() {
        let (t, root, _a, _b) = fx();
        let wins = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let hs: Vec<_> = (0..8)
            .map(|i| {
                let (root, wins) = (root.clone(), wins.clone());
                let db = t.path().join(format!("r{i}.db"));
                std::thread::spawn(move || {
                    let mut c = vf_core::db::open(&db, Storage::Local).unwrap();
                    if checkout(&mut c, &root, &format!("pc-{i}"), "u", "d1").is_ok() {
                        wins.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    }
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
        assert_eq!(wins.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// One PC has Intake; another waits; a silent holder is taken over (audited).
    #[test]
    fn intake_one_pc_at_a_time() {
        let (_t, root, mut a, mut b) = fx();
        assert_eq!(
            take_intake(&mut a, &root, "pc-a", "anna").unwrap(),
            IntakeLock::Mine
        );
        assert!(matches!(
            take_intake(&mut b, &root, "pc-b", "ben").unwrap(),
            IntakeLock::Theirs(h) if h.user == "anna"
        ));
        assert!(intake_heartbeat(&root, "pc-a"));
        assert!(!intake_heartbeat(&root, "pc-b"));
        // A goes silent: its heartbeat is old.
        let p = intake_path(&root);
        let mut cur: IntakeHeld =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        cur.beat = "2020-01-01T00:00:00Z".into();
        std::fs::write(&p, serde_json::to_vec(&cur).unwrap()).unwrap();
        assert_eq!(
            take_intake(&mut b, &root, "pc-b", "ben").unwrap(),
            IntakeLock::Mine
        );
        let n: i64 = b
            .query_row(
                "SELECT count(*) FROM audit WHERE action = 'intake_lock_takeover'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
        assert!(
            !intake_heartbeat(&root, "pc-a"),
            "A finds it was taken over"
        );
        release_intake(&root, "pc-a"); // not A's any more: nothing happens
        assert!(p.exists());
        release_intake(&root, "pc-b");
        assert!(!p.exists());
    }

    /// A lock file being written (unreadable, just made) is not taken over.
    #[test]
    fn a_fresh_unreadable_intake_lock_is_waited_for() {
        let (_t, root, mut a, _b) = fx();
        std::fs::write(intake_path(&root), b"").unwrap();
        assert!(matches!(
            take_intake(&mut a, &root, "pc-a", "anna").unwrap(),
            IntakeLock::Theirs(_)
        ));
    }

    /// Several PCs taking over a stale Intake at once: one holder afterwards.
    #[test]
    fn racing_takeovers_leave_one_holder() {
        let (t, root, _a, _b) = fx();
        let stale = IntakeHeld {
            held: Held {
                doc_id: String::new(),
                user: "gone".into(),
                machine: "OLD".into(),
                device_id: "pc-old".into(),
                since: "2020-01-01T00:00:00Z".into(),
            },
            beat: "2020-01-01T00:00:00Z".into(),
        };
        std::fs::write(intake_path(&root), serde_json::to_vec(&stale).unwrap()).unwrap();
        let hs: Vec<_> = (0..6)
            .map(|i| {
                let root = root.clone();
                let db = t.path().join(format!("i{i}.db"));
                std::thread::spawn(move || {
                    let mut c = vf_core::db::open(&db, Storage::Local).unwrap();
                    let dev = format!("pc-{i}");
                    match take_intake(&mut c, &root, &dev, "u") {
                        Ok(IntakeLock::Mine) => Some(dev),
                        _ => None,
                    }
                })
            })
            .collect();
        let winners: Vec<String> = hs.into_iter().filter_map(|h| h.join().unwrap()).collect();
        let file: IntakeHeld =
            serde_json::from_slice(&std::fs::read(intake_path(&root)).unwrap()).unwrap();
        assert!(winners.contains(&file.held.device_id), "{winners:?}");
        // Whoever else thought it won finds out at its next heartbeat.
        let still: Vec<&String> = winners
            .iter()
            .filter(|d| intake_heartbeat(&root, d))
            .collect();
        assert_eq!(still.len(), 1, "one holder: {winners:?}");
    }
}
