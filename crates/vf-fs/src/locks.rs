//! Check-out / check-in across workstations (PHASES PA-16, owner 2026-11-04 Q-51: lock files on
//! the server, each PC keeping its own catalog). A check-out is `_Vibe/locks/<doc uuid>.lock`,
//! made with an atomic create, so two PCs can never both hold it. Every PC copies the locks other
//! PCs hold into its catalog's `lock` table ([`sync`]); the edits that refuse a locked document
//! (versions, annotations, renames, moves, metadata, trash) therefore refuse documents someone
//! else has checked out, and Browse shows who. This PC's own check-outs are only the files.
//!
//! Intake: one PC files at a time, by the same kind of file (`_Vibe/intake.lock`) with a
//! heartbeat; a holder silent for [`INTAKE_STALE`] is taken over (audited).

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use vf_core::audit::AuditEntry;
use vf_core::db::Connection;
use vf_core::tx::{Actor, NoFileOps, Transaction};

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

/// The holder of `doc_id`'s check-out, when it is checked out.
pub fn holder(root: &Path, doc_id: &str) -> Option<Held> {
    let t = std::fs::read_to_string(lock_path(root, doc_id)).ok()?;
    serde_json::from_str(&t).ok()
}

/// Every check-out in the cabinet.
pub fn all(root: &Path) -> Vec<Held> {
    std::fs::read_dir(locks_dir(root))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "lock"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|t| serde_json::from_str(&t).ok())
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

fn audit(
    conn: &mut Connection,
    device_id: &str,
    action: &str,
    doc: &str,
    detail: serde_json::Value,
) {
    let _ = Transaction::new(action)
        .audit(AuditEntry::new(action, Some(doc), detail))
        .commit(conn, &mut NoFileOps, &Actor::device(device_id));
}

/// Checks `doc_id` out to this PC. Refused, naming the holder, when another PC has it; this PC
/// checking out again is fine. Audited `checkout`.
pub fn checkout(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    user: &str,
    doc_id: &str,
) -> Result<(), String> {
    let p = lock_path(root, doc_id);
    std::fs::create_dir_all(locks_dir(root)).map_err(|e| e.to_string())?;
    let held = Held {
        doc_id: doc_id.into(),
        user: user.into(),
        machine: machine(),
        device_id: device_id.into(),
        since: vf_core::now_rfc3339(),
    };
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&p)
    {
        Ok(mut f) => {
            f.write_all(&serde_json::to_vec_pretty(&held).expect("serializes"))
                .and_then(|()| f.sync_all())
                .map_err(|e| e.to_string())?;
            audit(conn, device_id, "checkout", doc_id, json!({}));
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => match holder(root, doc_id) {
            Some(h) if h.device_id == device_id => Ok(()),
            Some(h) => Err(format!(
                "checked out by {} since {}",
                h.who(),
                h.since[..h.since.len().min(16)].replace('T', " ")
            )),
            None => Err("someone is checking it out right now; try again".into()),
        },
        Err(e) => Err(format!("cannot check out: {e}")),
    }
}

/// Checks `doc_id` back in (this PC's check-out only). Audited `checkin`.
pub fn checkin(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    doc_id: &str,
) -> Result<(), String> {
    match holder(root, doc_id) {
        None => Ok(()),
        Some(h) if h.device_id != device_id => {
            Err(format!("checked out by {}, not this PC", h.who()))
        }
        Some(_) => {
            std::fs::remove_file(lock_path(root, doc_id)).map_err(|e| e.to_string())?;
            audit(conn, device_id, "checkin", doc_id, json!({}));
            Ok(())
        }
    }
}

/// Breaks another PC's check-out (admin, behind the admin password in the app). Audited
/// `lock_broken` with who held it.
pub fn break_lock(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    doc_id: &str,
) -> Result<Held, String> {
    let h = holder(root, doc_id).ok_or("it is not checked out")?;
    std::fs::remove_file(lock_path(root, doc_id)).map_err(|e| e.to_string())?;
    audit(
        conn,
        device_id,
        "lock_broken",
        doc_id,
        json!({ "held_by": h.user, "machine": h.machine, "since": h.since }),
    );
    let _ = conn.execute("DELETE FROM lock WHERE doc_id = ?1", [doc_id]);
    Ok(h)
}

/// Copies the check-outs other PCs hold into this catalog's `lock` table (only theirs: this
/// PC's own check-outs do not lock it out). Returns whether anything changed.
pub fn sync(conn: &mut Connection, root: &Path, device_id: &str) -> Result<bool, String> {
    let others: Vec<Held> = all(root)
        .into_iter()
        .filter(|h| h.device_id != device_id)
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
            "INSERT OR REPLACE INTO lock (doc_id, user, device_id, since)
             SELECT ?1, ?2, ?3, ?4 WHERE EXISTS (SELECT 1 FROM document WHERE id = ?1)",
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
/// [`INTAKE_STALE`] is taken over (audited `intake_lock_takeover`).
pub fn take_intake(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    user: &str,
) -> Result<IntakeLock, String> {
    let p = intake_path(root);
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
    for _ in 0..2 {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&p)
        {
            Ok(mut f) => {
                f.write_all(&serde_json::to_vec_pretty(&me).expect("serializes"))
                    .map_err(|e| e.to_string())?;
                return Ok(IntakeLock::Mine);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let cur: Option<IntakeHeld> = std::fs::read_to_string(&p)
                    .ok()
                    .and_then(|t| serde_json::from_str(&t).ok());
                match cur {
                    Some(c) if c.held.device_id == device_id => return Ok(IntakeLock::Mine),
                    Some(c) if age(&c.beat).is_some_and(|a| a < INTAKE_STALE) => {
                        return Ok(IntakeLock::Theirs(c.held));
                    }
                    stale => {
                        // Silent too long (a PC that crashed or lost the network): take over.
                        let _ = std::fs::remove_file(&p);
                        if let Some(c) = stale {
                            let _ = Transaction::new("intake_lock_takeover")
                                .audit(AuditEntry::new(
                                    "intake_lock_takeover",
                                    None,
                                    json!({ "from": c.held.who(), "last_heartbeat": c.beat }),
                                ))
                                .commit(conn, &mut NoFileOps, &Actor::device(device_id));
                        }
                    }
                }
            }
            Err(e) => return Err(format!("cannot take Intake: {e}")),
        }
    }
    Err("someone else is taking Intake right now; try again".into())
}

/// The heartbeat (every 10 s while this PC has Intake). `false` when it no longer has it.
pub fn intake_heartbeat(root: &Path, device_id: &str) -> bool {
    let p = intake_path(root);
    let Some(mut cur) = std::fs::read_to_string(&p)
        .ok()
        .and_then(|t| serde_json::from_str::<IntakeHeld>(&t).ok())
    else {
        return false;
    };
    if cur.held.device_id != device_id {
        return false;
    }
    cur.beat = vf_core::now_rfc3339();
    cabinet::write_replace(&p, &serde_json::to_vec_pretty(&cur).expect("serializes")).is_ok()
}

/// Lets go of Intake (leaving it, closing Vibe Files).
pub fn release_intake(root: &Path, device_id: &str) {
    let p = intake_path(root);
    let mine = std::fs::read_to_string(&p)
        .ok()
        .and_then(|t| serde_json::from_str::<IntakeHeld>(&t).ok())
        .is_some_and(|c| c.held.device_id == device_id);
    if mine {
        let _ = std::fs::remove_file(p);
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
    /// catalog sees the lock (so its edits refuse it); check-in frees it; a broken lock is
    /// audited.
    #[test]
    fn two_pcs_cannot_both_check_out() {
        let (_t, root, mut a, mut b) = fx();
        checkout(&mut a, &root, "pc-a", "anna", "d1").unwrap();
        checkout(&mut a, &root, "pc-a", "anna", "d1").unwrap(); // again: fine
        let e = checkout(&mut b, &root, "pc-b", "ben", "d1").unwrap_err();
        assert!(e.contains("anna"), "{e}");
        assert!(sync(&mut b, &root, "pc-b").unwrap());
        let n: i64 = b
            .query_row("SELECT count(*) FROM lock WHERE doc_id = 'd1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(n, 1, "B's catalog sees A's check-out");
        // A's own check-out does not lock A out.
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
        release_intake(&root, "pc-a"); // not A's any more: nothing happens
        assert!(p.exists());
        release_intake(&root, "pc-b");
        assert!(!p.exists());
    }
}
