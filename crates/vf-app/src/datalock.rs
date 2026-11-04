//! One copy of the program per data folder (QA C1/A10; owner 2026-11-03: several people work
//! on the server in their own sessions). The catalog, the settings and the crash journal in a
//! data folder belong to one running copy: `in-use.lock` is held open with no sharing for as
//! long as it runs, so a second copy (another window, another session, a command-line tool
//! that changes things) is refused and told who has it (`in-use.txt`).

use std::fs::File;
use std::path::{Path, PathBuf};

/// Held for the life of the process (dropping it releases the folder).
pub struct DataLock {
    _file: File,
    who: PathBuf,
}

impl Drop for DataLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.who);
    }
}

/// Who holds `data` now, from `in-use.txt` (empty when unknown).
pub fn holder(data: &Path) -> String {
    std::fs::read_to_string(data.join("in-use.txt"))
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Takes the data folder for this process, or says who has it.
pub fn acquire(data: &Path) -> Result<DataLock, String> {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::create_dir_all(data).map_err(|e| format!("cannot create {}: {e}", data.display()))?;
    let lock = data.join("in-use.lock");
    match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(&lock)
    {
        Ok(file) => {
            let who = data.join("in-use.txt");
            let _ = std::fs::write(
                &who,
                format!(
                    "{} on {} since {}",
                    vf_core::windows_login().unwrap_or_else(|| "someone".into()),
                    std::env::var("COMPUTERNAME").unwrap_or_default(),
                    crate::ui::widgets::local_time(&vf_core::now_rfc3339())
                ),
            );
            Ok(DataLock { _file: file, who })
        }
        Err(_) => {
            let h = holder(data);
            Err(format!(
                "Vibe Files is already open with this data folder{} ({}). Close it there first, or use another data folder (--data).",
                if h.is_empty() {
                    String::new()
                } else {
                    format!(": {h}")
                },
                data.display()
            ))
        }
    }
}

/// The data folder of a portable copy (`data` beside the exe) belongs to the Windows login that
/// first used it; another login (another person in a session on the same server) gets its own
/// in `%LOCALAPPDATA%\Vibe Files\data`. A winget install always uses `%LOCALAPPDATA%` (winget
/// replaces its package folder). An existing `data` folder with no owner yet is claimed.
pub fn default_data(exe: &Path, local_app_data: Option<PathBuf>, login: Option<&str>) -> PathBuf {
    let dir = exe.parent().unwrap_or(Path::new("."));
    let beside = dir.join("data");
    let winget = dir
        .to_string_lossy()
        .to_lowercase()
        .contains(r"\microsoft\winget\packages\");
    let personal = local_app_data.map(|l| l.join("Vibe Files").join("data"));
    if winget
        && !beside.exists()
        && let Some(p) = personal
    {
        return p;
    }
    let Some(me) = login else {
        return beside;
    };
    let owner_file = beside.join("owner.txt");
    match std::fs::read_to_string(&owner_file) {
        Ok(owner) if owner.trim().eq_ignore_ascii_case(me) => beside,
        Ok(_) => personal.unwrap_or(beside),
        Err(_) => {
            // No owner yet: this login takes the folder beside the exe if it can write there.
            if std::fs::create_dir_all(&beside).is_ok() && std::fs::write(&owner_file, me).is_ok() {
                beside
            } else {
                personal.unwrap_or(beside)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_folder_per_login_and_one_copy_at_a_time() {
        let t = tempfile::tempdir().unwrap();
        let local_a = t.path().join("A");
        let local_b = t.path().join("B");
        let exe = t.path().join("Tools").join("vibe-files.exe");
        // The first login claims the folder beside the exe; another gets its own.
        let a = default_data(&exe, Some(local_a.clone()), Some(r"KISAES\kim"));
        assert_eq!(a, t.path().join("Tools").join("data"));
        assert_eq!(default_data(&exe, Some(local_a), Some(r"kisaes\KIM")), a);
        let b = default_data(&exe, Some(local_b.clone()), Some(r"KISAES\pat"));
        assert_eq!(b, local_b.join("Vibe Files").join("data"));
        // winget: always the login's own folder.
        let pkg = t
            .path()
            .join(r"Microsoft\WinGet\Packages\Kisaes.VibeFiles_x");
        assert_eq!(
            default_data(
                &pkg.join("vibe-files.exe"),
                Some(t.path().join("C")),
                Some("x")
            ),
            t.path().join("C").join("Vibe Files").join("data")
        );
        // One copy per data folder.
        let first = acquire(&a).unwrap();
        let second = acquire(&a);
        assert!(second.is_err());
        assert!(second.err().unwrap().contains("already open"));
        drop(first);
        assert!(acquire(&a).is_ok());
    }
}
