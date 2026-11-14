//! The cabinet's first index on this PC, made behind the open window (PA-60, owner 2026-11-13:
//! "Nobody waits to index").
//!
//! Opening writes the clients ([`vf_fs::rebuild::index_start`]) and shows the window. One
//! thread, with its own catalog connection, then catalogs the client folders from their
//! listing a few at a time ([`vf_fs::rebuild::index_part`]); the client being looked at in
//! Browse goes first. Until it is done it is the only thing that catalogs files it finds:
//! Browse's own look for new files in a folder, the other servers' changes, the weekly sweep
//! and B2 sync wait for it (each would give a file not listed yet a second id).

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use vf_core::db::Connection;
use vf_fs::rebuild::Staged;

/// Client folders cataloged per part: small enough that a folder asked for is reached within
/// about a second, large enough that the parts' fixed cost does not add up.
const PART: usize = 40;

/// What the window knows of the index running behind it.
#[derive(Default)]
pub struct Shared {
    /// Every top-level folder the index covers.
    folders: HashSet<String>,
    /// Those cataloged so far.
    done: Mutex<HashSet<String>>,
    /// Folders Browse asked for: next, the latest first.
    asked: Mutex<VecDeque<String>>,
    /// Documents cataloged so far.
    docs: AtomicUsize,
    finished: AtomicBool,
    stop: AtomicBool,
    /// What went wrong in a part (the index goes on with the next).
    error: Mutex<Option<String>>,
}

impl Shared {
    pub fn new(staged: &Staged) -> Self {
        Shared {
            folders: staged.folders.iter().cloned().collect(),
            ..Default::default()
        }
    }

    /// Whether `folder`'s documents are in the catalog: cataloged by this index, or not its to
    /// catalog (a folder made after it started).
    pub fn listed(&self, folder: &str) -> bool {
        self.finished()
            || !self.folders.contains(folder)
            || self.done.lock().is_ok_and(|d| d.contains(folder))
    }

    /// `folder` next (the client being looked at).
    pub fn ask(&self, folder: &str) {
        if self.listed(folder) {
            return;
        }
        if let Ok(mut a) = self.asked.lock()
            && a.front().is_none_or(|f| f != folder)
        {
            a.retain(|f| f != folder);
            a.push_front(folder.to_string());
        }
    }

    /// (folders cataloged, folders in all, documents cataloged).
    pub fn progress(&self) -> (usize, usize, usize) {
        (
            self.done.lock().map_or(0, |d| d.len()),
            self.folders.len(),
            self.docs.load(Ordering::Relaxed),
        )
    }

    pub fn finished(&self) -> bool {
        self.finished.load(Ordering::Relaxed)
    }

    pub fn take_error(&self) -> Option<String> {
        self.error.lock().ok().and_then(|mut e| e.take())
    }
}

/// The index running on its thread; dropped (the cabinet closed), it stops after the part it
/// is on, and the next open goes on.
pub struct Indexer {
    pub shared: Arc<Shared>,
}

impl Drop for Indexer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

impl Indexer {
    /// Starts the index on its own thread with its own connection to `catalog`.
    pub fn start(
        ctx: &egui::Context,
        catalog: &Path,
        root: &Path,
        device_id: &str,
        staged: Staged,
    ) -> Self {
        let shared = Arc::new(Shared::new(&staged));
        let (catalog, root, device): (PathBuf, PathBuf, String) =
            (catalog.to_path_buf(), root.to_path_buf(), device_id.into());
        let (ctx, sh) = (ctx.clone(), shared.clone());
        let started = std::thread::Builder::new()
            .name("background-index".into())
            .spawn(move || {
                match vf_core::db::open(&catalog, vf_core::db::Storage::Local) {
                    Ok(mut conn) => run(&mut conn, &root, &device, &staged, &sh, &|| {
                        ctx.request_repaint()
                    }),
                    Err(e) => {
                        if let Ok(mut err) = sh.error.lock() {
                            *err = Some(e.to_string());
                        }
                    }
                }
                ctx.request_repaint();
            });
        if let Err(e) = started
            && let Ok(mut err) = shared.error.lock()
        {
            *err = Some(format!("the index could not start: {e}"));
        }
        Indexer { shared }
    }
}

/// The whole index, here and now (tests; and what the thread runs). `tick` after each part.
pub fn run(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    staged: &Staged,
    shared: &Shared,
    tick: &dyn Fn(),
) {
    let started = std::time::Instant::now();
    let mut rest: VecDeque<String> = staged.folders.iter().cloned().collect();
    loop {
        if shared.stop.load(Ordering::Relaxed) {
            return; // left unfinished: the next open goes on
        }
        let asked = shared.asked.lock().ok().and_then(|mut a| a.pop_front());
        let part: Vec<String> = match asked {
            Some(f) if !shared.listed(&f) => vec![f],
            Some(_) => continue,
            None => {
                let mut part = Vec::with_capacity(PART);
                while part.len() < PART {
                    let Some(f) = rest.pop_front() else {
                        break;
                    };
                    if !shared.listed(&f) {
                        part.push(f);
                    }
                }
                part
            }
        };
        if part.is_empty() {
            break;
        }
        match vf_fs::rebuild::index_part(conn, root, device_id, staged, &part) {
            Ok(rep) => {
                shared.docs.fetch_add(rep.docs_new, Ordering::Relaxed);
            }
            Err(e) => {
                tracing::warn!(
                    kind = crate::logging::kind(&e.to_string()),
                    "a part of the index failed"
                );
                if let Ok(mut err) = shared.error.lock() {
                    *err = Some(e.to_string());
                }
            }
        }
        // Done either way: a folder that cannot be cataloged now is not tried in a loop (the
        // client's folder is read again when it is opened in Browse).
        if let Ok(mut d) = shared.done.lock() {
            d.extend(part);
        }
        tick();
    }
    if let Err(e) = vf_fs::rebuild::index_finish(conn, root, device_id)
        && let Ok(mut err) = shared.error.lock()
    {
        *err = Some(e.to_string());
    }
    let (_, folders, docs) = shared.progress();
    tracing::info!(
        folders,
        docs,
        secs = started.elapsed().as_secs(),
        "index made behind the window"
    );
    shared.finished.store(true, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The folder asked for is cataloged first; the rest follow; nothing twice.
    #[test]
    fn the_folder_asked_for_goes_first() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("cab");
        let req = crate::session::CreateRequest {
            root: &root,
            name: "Test",
        };
        drop(crate::session::create(&tmp.path().join("pc1"), "dev-1", &req).unwrap());
        for (client, n) in [("Alpha", 2), ("Beta", 3), ("Gamma", 1)] {
            for i in 0..n {
                let f = root.join(format!("{client}/2025/doc{i}.pdf"));
                std::fs::create_dir_all(f.parent().unwrap()).unwrap();
                std::fs::write(&f, format!("{client} {i}")).unwrap();
            }
        }
        let data = tmp.path().join("pc2");
        std::fs::create_dir_all(&data).unwrap();
        let mut conn =
            vf_core::db::open(&data.join("catalog.db"), vf_core::db::Storage::Local).unwrap();
        let staged = vf_fs::rebuild::index_start(&mut conn, &root, "dev-2").unwrap();
        let shared = Shared::new(&staged);
        assert!(!shared.listed("Beta"));
        assert!(shared.listed("Made later"), "not this index's to catalog");
        shared.ask("Beta");
        shared.ask("Gamma");
        shared.ask("Beta"); // the latest asked for is first
        // After the first part only Beta is there.
        let seen = std::cell::RefCell::new(Vec::new());
        let count = |conn: &Connection| -> i64 {
            conn.query_row(
                "SELECT count(*) FROM document WHERE deleted_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        let check =
            vf_core::db::open(&data.join("catalog.db"), vf_core::db::Storage::Local).unwrap();
        run(&mut conn, &root, "dev-2", &staged, &shared, &|| {
            seen.borrow_mut().push(count(&check));
        });
        assert_eq!(
            seen.borrow()[..2],
            [3, 4],
            "Beta, then Gamma, then the rest"
        );
        assert_eq!(count(&conn), 6);
        assert!(shared.finished());
        assert!(!vf_fs::rebuild::unfinished(&conn));
        let (done, all, docs) = shared.progress();
        assert_eq!((done, all, docs), (3, 3, 6));
        assert!(shared.take_error().is_none());
    }
}
