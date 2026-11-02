//! PID file in the data folder. A warning only: a second instance is allowed to continue.

use std::path::Path;

/// Returns the PID recorded in `path` if that process is still running and is not us.
pub fn check_other_instance(path: &Path) -> Option<u32> {
    let pid: u32 = std::fs::read_to_string(path).ok()?.trim().parse().ok()?;
    (pid != std::process::id() && process_alive(pid)).then_some(pid)
}

pub fn write(path: &Path) -> std::io::Result<()> {
    std::fs::write(path, std::process::id().to_string())
}

/// Removes the PID file only if it still holds our PID.
pub fn remove(path: &Path) {
    let ours =
        std::fs::read_to_string(path).is_ok_and(|s| s.trim() == std::process::id().to_string());
    if ours {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: plain Win32 calls; the handle is closed before returning.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code) != 0;
        CloseHandle(handle);
        ok && code == STILL_ACTIVE as u32
    }
}

#[cfg(not(windows))]
fn process_alive(_pid: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_pid_is_not_another_instance() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("filer.pid");
        write(&p).unwrap();
        assert_eq!(check_other_instance(&p), None);
        remove(&p);
        assert!(!p.exists());
    }

    #[test]
    fn stale_or_garbage_pid_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("filer.pid");
        std::fs::write(&p, "not a number").unwrap();
        assert_eq!(check_other_instance(&p), None);
        std::fs::write(&p, u32::MAX.to_string()).unwrap();
        assert_eq!(check_other_instance(&p), None);
    }

    #[test]
    fn remove_leaves_another_instances_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("filer.pid");
        std::fs::write(&p, "4").unwrap();
        remove(&p);
        assert!(p.exists());
    }
}
