//! A lock file in the cabinet, for a change to a file every PC shares (PA-68, owner 2026-11-14:
//! two people on different session hosts changing the client list, the legal holds or the doc
//! types at once; "wait briefly, then tell").
//!
//! Each change reads the shared file, changes it and writes it back. Two PCs doing that at
//! once lose one of the changes. With the lock held from the read to the write, the second
//! waits for the first (a few seconds at most) and then works on what the first wrote; when it
//! cannot have the lock in time it is told who holds it, and changes nothing.
//!
//! The holder writes the lock file again every [`BEAT`]. A lock not written for [`STALE`] was
//! left by a PC that stopped, and is taken over. Its age is read against the file server's own
//! clock (the time the server gives a file written just now), never this PC's.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::cabinet;

/// How often the holder writes its lock again.
pub const BEAT: Duration = Duration::from_secs(20);
/// A lock not written for this long is nobody's.
pub const STALE: Duration = Duration::from_secs(120);
/// How long a change waits for someone else's (owner: a few seconds, then it is told).
pub const WAIT: Duration = Duration::from_secs(6);

/// A lock held; given back when dropped.
pub struct Held {
    pub(crate) held: Option<(PathBuf, String)>,
    stop: Arc<AtomicBool>,
    beat: Option<std::thread::JoinHandle<()>>,
}

/// Someone else holds the lock: who (Windows login and computer), as the lock file says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Busy {
    pub who: String,
}

impl Busy {
    /// "… is changing {what} right now. Try again in a moment." Nothing was changed.
    pub fn message(&self, what: &str) -> String {
        let who = if self.who.trim().is_empty() {
            "Someone else"
        } else {
            self.who.trim()
        };
        format!("{who} is changing {what} right now. Nothing was changed: try again in a moment.")
    }
}

/// Who this is, for the lock file: the Windows login and the computer.
fn me() -> String {
    let user = vf_core::windows_login().unwrap_or_default();
    let pc = std::env::var("COMPUTERNAME").unwrap_or_default();
    match (user.is_empty(), pc.is_empty()) {
        (false, false) => format!("{user} on {pc}"),
        (false, true) => user,
        _ => pc,
    }
}

/// The lock for the shared cabinet file (or group of files) `name`, waiting up to [`WAIT`].
pub fn take(root: &Path, name: &str) -> Result<Held, Busy> {
    let path = root
        .join(cabinet::VIBE_DIR)
        .join(format!("lock-{name}.lock"));
    take_at(&path, Some(WAIT), BEAT, STALE)
}

/// Runs `change` holding the lock `name`; `what` names the thing in the message when someone
/// else holds it ("the legal holds").
pub fn with<T>(
    root: &Path,
    name: &str,
    what: &str,
    change: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    // Already held by this thread (a change made of several, each under the lock): go on.
    let key = (root.to_path_buf(), name.to_string());
    if MINE.with(|m| m.borrow().contains(&key)) {
        return change();
    }
    let _held = take(root, name).map_err(|b| b.message(what))?;
    MINE.with(|m| m.borrow_mut().insert(key.clone()));
    let file = _held.held.clone();
    if let Some(f) = &file {
        FILES.with(|m| {
            m.borrow_mut()
                .push((f.0.clone(), f.1.clone(), what.to_string()))
        });
    }
    // Taken off the lists however the change ends.
    struct Off((PathBuf, String), Option<(PathBuf, String)>);
    impl Drop for Off {
        fn drop(&mut self) {
            MINE.with(|m| m.borrow_mut().remove(&self.0));
            if let Some(f) = &self.1 {
                FILES.with(|m| m.borrow_mut().retain(|x| x.0 != f.0 || x.1 != f.1));
            }
        }
    }
    let _off = Off(key, file);
    change()
}

/// Whether every lock this thread holds through [`with`] is still its own, asked just before
/// a shared file is written ([`cabinet::write_replace`]). A computer that stalled for longer
/// than [`STALE`] in the middle of a change (asleep, a share that hung) had its lock taken
/// over, and someone else may have changed the file since: writing now would put this
/// computer's old copy over theirs (research review 2026-11-14). `Err`: what to tell the
/// person; nothing is written.
pub fn still_mine() -> Result<(), String> {
    let held: Vec<(PathBuf, String, String)> = FILES.with(|m| m.borrow().clone());
    for (path, token, what) in held {
        // The holder's own heartbeat rewrites the file: an empty read is looked at again.
        let mut ours = false;
        for _ in 0..5 {
            match std::fs::read_to_string(&path) {
                Ok(t) if t.starts_with(token.as_str()) => {
                    ours = true;
                    break;
                }
                Ok(t) if t.trim().is_empty() => {
                    std::thread::sleep(Duration::from_millis(30));
                }
                _ => break,
            }
        }
        if !ours {
            return Err(format!(
                "This computer stopped for a while in the middle of changing {what}, and someone else went ahead meanwhile. Nothing was saved: make the change again."
            ));
        }
    }
    Ok(())
}

thread_local! {
    /// The locks this thread holds through [`with`].
    static MINE: std::cell::RefCell<std::collections::HashSet<(PathBuf, String)>> =
        Default::default();
    /// Their lock files: (file, this holder's token, what it guards).
    static FILES: std::cell::RefCell<Vec<(PathBuf, String, String)>> = Default::default();
}

/// The age of the file at `path` by the file server's clock: against a file written just now
/// beside it.
fn age_by_server(path: &Path) -> Option<Duration> {
    let was = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
    let mut name = path.file_name()?.to_os_string();
    name.push(format!(".{}.now", uuid::Uuid::now_v7().simple()));
    let probe = path.with_file_name(name);
    std::fs::write(&probe, b"").ok()?;
    let now = std::fs::metadata(&probe).and_then(|m| m.modified());
    let _ = std::fs::remove_file(&probe);
    Some(now.ok()?.duration_since(was).unwrap_or_default())
}

/// Whether a file can be made beside the lock file `path` (the cabinet can be written).
fn writable(path: &Path) -> bool {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.probe", uuid::Uuid::now_v7().simple()));
    let probe = path.with_file_name(name);
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// The lock file `path`. `wait`: how long to wait for another holder (`None`: for as long as
/// it keeps its lock alive). A cabinet that cannot be written gives a lock that guards nothing.
pub(crate) fn take_at(
    path: &Path,
    wait: Option<Duration>,
    beat: Duration,
    stale: Duration,
) -> Result<Held, Busy> {
    let none = || Held {
        held: None,
        stop: Default::default(),
        beat: None,
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let token = vf_core::new_id();
    let who = me();
    let started = Instant::now();
    // What the lock held by another looked like, and since when it has not changed.
    let mut seen: Option<(String, Instant)> = None;
    let mut checked_age = false;
    // The holder's name, as last read from the lock file.
    let mut holder = String::new();
    // Since when the lock file could be neither made nor found there.
    let mut trouble: Option<Instant> = None;
    loop {
        let made = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path);
        match made {
            Ok(mut f) => {
                use std::io::Write;
                let _ = f.write_all(format!("{token} 0 {who}").as_bytes());
                drop(f);
                let stop = Arc::new(AtomicBool::new(false));
                let (stop2, path2, token2, who2) =
                    (stop.clone(), path.to_path_buf(), token.clone(), who.clone());
                let thread = std::thread::Builder::new()
                    .name("cabinet-lock".into())
                    .spawn(move || {
                        let mut n = 0u64;
                        let mut last = Instant::now();
                        while !stop2.load(Ordering::Relaxed) {
                            // Woken at once when the lock is given back.
                            std::thread::park_timeout(beat.saturating_sub(last.elapsed()));
                            if stop2.load(Ordering::Relaxed) {
                                return;
                            }
                            if last.elapsed() < beat {
                                continue;
                            }
                            last = Instant::now();
                            // Only while it is still this holder's (taken over: left alone).
                            let mine = std::fs::read_to_string(&path2)
                                .is_ok_and(|t| t.starts_with(token2.as_str()));
                            if !mine {
                                return;
                            }
                            n += 1;
                            let _ = std::fs::write(&path2, format!("{token2} {n} {who2}"));
                        }
                    })
                    .ok();
                return Ok(Held {
                    held: Some((path.to_path_buf(), token)),
                    stop,
                    beat: thread,
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let now = std::fs::read_to_string(path).unwrap_or_default();
                // "token n who": the holder's name is the rest. Kept from the last look that
                // read one: the holder writes the file again at every beat, and a look in
                // that instant reads it empty (the person was then told "Someone else").
                if let Some(w) = now.splitn(3, ' ').nth(2).filter(|w| !w.trim().is_empty()) {
                    holder = w.to_string();
                }
                // Left by a PC that stopped: by the server's clock (looked at once, and again
                // whenever the lock is seen to change hands), or seen unchanged for that long.
                let dead = match &seen {
                    Some((was, since)) if *was == now => since.elapsed() > stale,
                    _ => {
                        seen = Some((now.clone(), Instant::now()));
                        checked_age = false;
                        false
                    }
                } || (!checked_age && {
                    checked_age = true;
                    age_by_server(path).is_some_and(|age| age > stale)
                });
                if dead {
                    // Taken away under a name of its own first: of several waiters that
                    // judged the same lock dead, only one's rename finds it (code review
                    // 2026-11-14: read-then-remove let a second waiter remove the first
                    // one's new lock, and both held it).
                    let mut aside = path.as_os_str().to_os_string();
                    aside.push(format!(".{}.old", uuid::Uuid::now_v7().simple()));
                    let aside = PathBuf::from(aside);
                    if std::fs::rename(path, &aside).is_ok() {
                        // The lock that was judged; or not it, but another has been made
                        // since (the one taken away lost its place either way).
                        if std::fs::read_to_string(&aside).is_ok_and(|t| t == now) || path.exists()
                        {
                            let _ = std::fs::remove_file(&aside);
                        } else {
                            // Not the lock that was judged (someone's new one): put back.
                            let _ = std::fs::rename(&aside, path);
                        }
                    }
                    seen = None;
                    continue;
                }
                if wait.is_some_and(|w| started.elapsed() >= w) {
                    return Err(Busy { who: holder });
                }
                std::thread::sleep(Duration::from_millis(100).min(beat));
            }
            // Not "already there": a share error for a moment, or the lock file being
            // deleted by its last holder while a waiter still has it open. Tried again; only
            // a cabinet that really cannot be written (a read-only share) has nothing to
            // guard. Taking any error for that let a change run with no lock beside the
            // real holder (code review 2026-11-14).
            Err(_) => {
                let since = *trouble.get_or_insert_with(Instant::now);
                if since.elapsed() >= Duration::from_secs(3) {
                    if !writable(path) {
                        return Ok(none());
                    }
                    if wait.is_some_and(|w| started.elapsed() >= w) {
                        return Err(Busy { who: holder });
                    }
                }
                std::thread::sleep(Duration::from_millis(100).min(beat));
            }
        }
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.beat.take() {
            t.thread().unpark();
            let _ = t.join();
        }
        // Given back only if it is still this holder's.
        if let Some((p, token)) = self.held.take()
            && std::fs::read_to_string(&p).is_ok_and(|t| t.starts_with(token.as_str()))
        {
            let _ = std::fs::remove_file(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A computer that stalled past the takeover time in the middle of a change does not
    /// write its old copy over what the computer that took the lock over wrote.
    #[test]
    fn a_change_whose_lock_was_taken_over_is_not_written() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let file = root.join("_Vibe/holds.json");
        // Held all the way: written.
        with(&root, "holds", "the legal holds", || {
            cabinet::write_replace(&file, b"first")
        })
        .unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "first");
        // Taken over while this change was under way: not written, and said.
        let lock = root.join("_Vibe/lock-holds.lock");
        let e = with(&root, "holds", "the legal holds", || {
            std::fs::write(&lock, "another-token 1 Pat on HOST7").unwrap();
            cabinet::write_replace(&file, b"stale copy")
        })
        .unwrap_err();
        assert!(e.contains("Nothing was saved"), "{e}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "first");
        // The other holder's lock is left alone, and no temp file is left behind.
        assert!(
            std::fs::read_to_string(&lock)
                .unwrap()
                .starts_with("another-token")
        );
        let left: Vec<String> = std::fs::read_dir(root.join("_Vibe"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(left.is_empty(), "{left:?}");
        // Outside any lock, a write is not asked about locks.
        std::fs::remove_file(&lock).unwrap();
        cabinet::write_replace(&file, b"second").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "second");
    }

    /// The second change waits for the first; one that cannot have the lock in time is told
    /// who holds it; a lock left by a PC that stopped is taken over.
    #[test]
    fn the_second_waits_then_is_told_who() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("_Vibe/lock-holds.lock");
        let (beat, stale) = (Duration::from_millis(60), Duration::from_millis(500));
        let first = take_at(&path, Some(Duration::from_millis(200)), beat, stale).unwrap();
        // Held past the second's wait: told who.
        let busy = take_at(&path, Some(Duration::from_millis(200)), beat, stale)
            .err()
            .unwrap();
        assert_eq!(busy.who, me());
        assert!(busy.message("the legal holds").contains("the legal holds"));
        // Given back within the wait: the second goes on.
        let p2 = path.clone();
        let second = std::thread::spawn(move || {
            take_at(&p2, Some(Duration::from_secs(5)), beat, stale).is_ok()
        });
        std::thread::sleep(Duration::from_millis(150));
        drop(first);
        assert!(second.join().unwrap());
        assert!(!path.exists(), "given back");

        // Left behind long ago (the file's own time, by the server's clock): taken over at
        // once, not after the wait.
        std::fs::write(&path, "gone 0 Pat on HOST7").unwrap();
        let old = std::time::SystemTime::now() - Duration::from_secs(600);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let t0 = Instant::now();
        let third = take_at(&path, Some(Duration::from_secs(5)), beat, stale).unwrap();
        assert!(t0.elapsed() < Duration::from_secs(2));
        assert!(third.held.is_some());
        // A live holder elsewhere is named.
        drop(third);
        std::fs::write(&path, "tok 3 Pat on HOST7").unwrap();
        let busy = take_at(
            &path,
            Some(Duration::from_millis(100)),
            beat,
            Duration::from_secs(60),
        )
        .err()
        .unwrap();
        assert_eq!(busy.who, "Pat on HOST7");
        assert!(
            busy.message("the client list")
                .starts_with("Pat on HOST7 is changing")
        );
    }

    /// Two PCs adding to one shared file at once: with the lock, neither change is lost.
    #[test]
    fn changes_made_at_once_are_both_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let file = root.join("_Vibe/shared.txt");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "").unwrap();
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let (root, file) = (root.clone(), file.clone());
                std::thread::spawn(move || {
                    with(&root, "shared", "the list", || {
                        let mut text = std::fs::read_to_string(&file).unwrap();
                        std::thread::sleep(Duration::from_millis(20));
                        text.push_str(&format!("{i}\n"));
                        std::fs::write(&file, text).map_err(|e| e.to_string())
                    })
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap().unwrap();
        }
        assert_eq!(std::fs::read_to_string(&file).unwrap().lines().count(), 8);
        // A change made of several, each under the same lock, does not wait for itself.
        let t0 = Instant::now();
        with(&root, "shared", "the list", || {
            with(&root, "shared", "the list", || Ok::<_, String>(()))
        })
        .unwrap();
        assert!(t0.elapsed() < Duration::from_secs(2));
        assert!(!root.join("_Vibe/lock-shared.lock").exists());
    }
}
