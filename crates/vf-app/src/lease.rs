//! The sync lease (owner 2026-11-20, PA-99, Q-133).
//!
//! On several servers the whole cabinet syncs with B2 on one server at a time: whichever holds
//! the lease. A server with Vibe Files open and sync unlocked takes it when nobody holds it, or
//! when its holder stopped (the cabinet lock `lock-sync.lock`, written again every
//! [`vf_fs::lock::BEAT`] and taken over after [`vf_fs::lock::STALE`] by the file server's
//! clock). The server chosen in Admin › Devices is the preferred one: it takes the lease the
//! same way, when it is free, and never from a live holder. The portal's timed fetch follows
//! the lease; the weekly sweep and the catalog copy stay on the preferred server
//! ([`crate::server::runs_here`]).
//!
//! Every session of the holding computer counts as holding it (one server's sessions share one
//! catalog, and its sync lock lets one sync at a time). A single PC, and a cabinet in a OneDrive
//! or Dropbox folder (where "made only if not there" is not exclusive), keep
//! [`crate::server::runs_here`].

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const NAME: &str = "sync";

/// Held by this process: the cabinet, the lock, and the watcher's stop flag.
struct Holding {
    root: PathBuf,
    held: Arc<vf_fs::lock::Held>,
    stop: Arc<AtomicBool>,
}

static HOLDING: Mutex<Option<Holding>> = Mutex::new(None);

/// Whether the sync lease decides where the whole cabinet syncs: several servers on one share.
pub fn applies(_root: &Path) -> bool {
    crate::server::server_mode()
}

/// Whether the whole cabinet's B2 sync (and the portal's timed fetch) may run here now.
pub fn sync_here(root: &Path) -> bool {
    if !applies(root) {
        return crate::server::runs_here(root);
    }
    held_here(root)
}

/// This process holds the lease for `root`, or another session of this computer does.
fn held_here(root: &Path) -> bool {
    if let Some(h) = HOLDING.lock().ok().and_then(|g| {
        g.as_ref()
            .filter(|h| h.root == root)
            .map(|h| h.held.still_held())
    }) {
        return h;
    }
    holder_computer(root).is_some_and(|c| c.eq_ignore_ascii_case(&crate::server::computer()))
}

/// The computer that holds the lease now, as its file says.
pub fn holder_computer(root: &Path) -> Option<String> {
    let who = vf_fs::lock::holder(root, NAME)?;
    // "login on COMPUTER", or the computer alone.
    Some(match who.rsplit_once(" on ") {
        Some((_, pc)) => pc.trim().to_string(),
        None => who,
    })
}

/// Where the whole cabinet syncs, for a message: the lease's holder, or "no server just now".
pub fn where_it_syncs(root: &Path) -> String {
    holder_computer(root).unwrap_or_else(|| "no server just now".into())
}

/// Takes the lease for `root` when it is free (or its holder stopped). `true`: held here now
/// (by this process or another session of this computer). A lease taken over while held (this
/// computer stalled past the take-over time) asks a push under way to stop.
pub fn hold(root: &Path, cabinet_id: &str) -> bool {
    if !applies(root) {
        return crate::server::runs_here(root);
    }
    let Ok(mut g) = HOLDING.lock() else {
        return false;
    };
    if let Some(h) = g.as_ref() {
        if h.root == root && h.held.still_held() {
            return true;
        }
        // Lost, or another cabinet: given back.
        if let Some(h) = g.take() {
            h.stop.store(true, Ordering::Relaxed);
        }
    }
    if holder_computer(root).is_some_and(|c| c.eq_ignore_ascii_case(&crate::server::computer())) {
        return true;
    }
    let Ok(held) = vf_fs::lock::try_take(root, NAME) else {
        return false;
    };
    let held = Arc::new(held);
    let stop = Arc::new(AtomicBool::new(false));
    // Watches the lease on its own thread (frames may not come while the window is idle): a
    // lease taken over stops a push under way at its next file.
    let (w_held, w_stop, id) = (held.clone(), stop.clone(), cabinet_id.to_string());
    std::thread::Builder::new()
        .name("sync-lease".into())
        .spawn(move || {
            while !w_stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_secs(5));
                if w_stop.load(Ordering::Relaxed) {
                    return;
                }
                if !w_held.still_held() {
                    tracing::warn!("sync lease taken over: a push under way stops");
                    vf_sync::push::ask_stop(&id);
                    return;
                }
            }
        })
        .ok();
    tracing::info!("sync lease taken: the whole cabinet syncs on this computer");
    *g = Some(Holding {
        root: root.to_path_buf(),
        held,
        stop,
    });
    true
}

/// Gives the lease back (sync locked, the cabinet closed, Vibe Files closing).
pub fn release() {
    if let Some(h) = HOLDING.lock().ok().and_then(|mut g| g.take()) {
        h.stop.store(true, Ordering::Relaxed);
        drop(h.held);
        tracing::info!("sync lease given back");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lease's file names its holder; a computer reads which computer that is.
    #[test]
    fn the_holder_is_read_from_the_lock_file() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        assert_eq!(holder_computer(root), None);
        std::fs::create_dir_all(root.join("_Vibe")).unwrap();
        std::fs::write(
            root.join("_Vibe/lock-sync.lock"),
            "tok 3 pat on THGR18C-XA004",
        )
        .unwrap();
        assert_eq!(holder_computer(root).as_deref(), Some("THGR18C-XA004"));
        assert_eq!(where_it_syncs(root), "THGR18C-XA004");
    }

    /// A held lease is not taken; one given back is; the holder's lease, once taken over,
    /// reads as not held.
    #[test]
    fn a_held_lease_is_not_taken_and_a_free_one_is() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        std::fs::create_dir_all(root.join("_Vibe")).unwrap();
        let first = vf_fs::lock::try_take(root, NAME).unwrap();
        assert!(first.still_held());
        assert!(
            vf_fs::lock::try_take(root, NAME).is_err(),
            "held: not taken"
        );
        drop(first);
        let second = vf_fs::lock::try_take(root, NAME).unwrap();
        // Taken over by another computer (after a stall here): no longer this holder's.
        std::fs::write(root.join("_Vibe/lock-sync.lock"), "other 1 sam on HOST7").unwrap();
        assert!(!second.still_held());
    }
}
