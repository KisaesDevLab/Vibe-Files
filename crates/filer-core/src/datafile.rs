//! Reading and writing the JSON files in `data/`: written to a temp file next to the target, then
//! renamed over it, so a crash never leaves a half-written file.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Writes `bytes` to `path` through a temp file beside it and a rename.
///
/// Vibe Files (PA-94): the temp file has a name of its own (two sessions saving the same
/// file at once shared `<path>.tmp`), and a rename Windows refuses because the file is open
/// just then (another session reading it, a virus scanner; "Access is denied" on a share)
/// is tried again for a moment (under half a second). A commit's `last_batch.json` was left unwritten by it.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!("{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    std::fs::write(&tmp, bytes)?;
    let mut tries = 0;
    loop {
        match std::fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            // In use just now: a few tries, under half a second in all.
            Err(e)
                if tries < 5
                    && (e.kind() == std::io::ErrorKind::PermissionDenied
                        || matches!(e.raw_os_error(), Some(32 | 33))) =>
            {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(30 * tries));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e.into());
            }
        }
    }
}

/// Pretty-printed JSON, written atomically.
pub fn save_json<T: Serialize + ?Sized>(path: &Path, value: &T) -> anyhow::Result<()> {
    write_atomic(path, &serde_json::to_vec_pretty(value)?)
}

/// `None` when the file doesn't exist.
pub fn load_json<T: for<'de> Deserialize<'de>>(path: &Path) -> anyhow::Result<Option<T>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(serde_json::from_str(&text)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vibe Files (PA-94): a file another program holds open for a moment is still saved,
    /// and no temp file is left beside it.
    #[cfg(windows)]
    #[test]
    fn a_file_in_use_for_a_moment_is_still_saved() {
        use std::os::windows::fs::OpenOptionsExt;
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("last_batch.json");
        write_atomic(&path, b"one").unwrap();
        // Open for reading without letting anyone replace it, as a scanner or a reader
        // on a share holds it.
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            drop(held);
        });
        write_atomic(&path, b"two").unwrap();
        release.join().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        assert_eq!(
            std::fs::read_dir(t.path()).unwrap().count(),
            1,
            "no temp file left"
        );
        // Held for good: the error is said, and the temp file is cleared away.
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .unwrap();
        assert!(write_atomic(&path, b"three").is_err());
        drop(held);
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        assert_eq!(std::fs::read_dir(t.path()).unwrap().count(), 1);
    }

    /// Two savers at once do not share a temp file.
    #[test]
    fn two_savers_at_once_each_write_a_whole_file() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("config.json");
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for _ in 0..20 {
                        // (A rename may lose to another's for good on a slow disk: that is
                        // an error said, never a torn file.)
                        let _ = write_atomic(&path, format!("{i}").repeat(4000).as_bytes());
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.len(), 4000);
        assert!(
            text.chars().all(|c| c == text.chars().next().unwrap()),
            "one saver's text, whole"
        );
    }
}
