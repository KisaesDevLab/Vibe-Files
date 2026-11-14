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
        .join("locks")
        .join(format!("cabinet-{name}.lock"));
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
    let _held = take(root, name).map_err(|b| b.message(what))?;
    change()
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
                    let _ = std::fs::remove_file(path);
                    seen = None;
                    continue;
                }
                if wait.is_some_and(|w| started.elapsed() >= w) {
                    // "token n who": the holder's name is the rest.
                    let who = now.splitn(3, ' ').nth(2).unwrap_or_default().to_string();
                    return Err(Busy { who });
                }
                std::thread::sleep(Duration::from_millis(100).min(beat));
            }
            // A cabinet that cannot be written (read-only share): nothing to guard.
            Err(_) => return Ok(none()),
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

    /// The second change waits for the first; one that cannot have the lock in time is told
    /// who holds it; a lock left by a PC that stopped is taken over.
    #[test]
    fn the_second_waits_then_is_told_who() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("_Vibe/locks/cabinet-holds.lock");
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
    }
}
