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

/// Who holds `data` now, from `in-use.txt`'s first line (empty when unknown).
pub fn holder(data: &Path) -> String {
    std::fs::read_to_string(data.join("in-use.txt"))
        .unwrap_or_default()
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Takes the data folder for this process, or says who has it.
pub fn acquire(data: &Path) -> Result<DataLock, String> {
    acquire_as(
        data,
        vf_core::windows_login().as_deref(),
        &std::env::var("COMPUTERNAME").unwrap_or_default(),
    )
}

fn acquire_as(data: &Path, login: Option<&str>, computer: &str) -> Result<DataLock, String> {
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
            // Second line: the process and its Windows session, so a second start can find this
            // copy (bring its window forward, or close it when it was left in a disconnected
            // session: Citrix, Remote Desktop, owner 2026-11-09).
            let _ = std::fs::write(
                &who,
                format!(
                    "{} on {computer} since {}\nprocess {} session {}\n",
                    login.unwrap_or("someone"),
                    crate::ui::widgets::local_time(&vf_core::now_rfc3339()),
                    std::process::id(),
                    session_of(std::process::id()).map_or("?".into(), |s| s.to_string()),
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

/// Takes the default data folder `data`, or, when another login or another PC holds it (a
/// shared program folder on a server or a network drive), this login's own `personal` folder
/// instead: one person's open copy never stops another from opening Vibe Files. Refused only
/// when this same login on this same PC has it open already (a second window). Returns the
/// folder taken.
pub fn acquire_default(
    data: &Path,
    personal: Option<&Path>,
    login: Option<&str>,
    computer: &str,
) -> Result<(DataLock, PathBuf), String> {
    let first = match acquire_as(data, login, computer) {
        Ok(l) => return Ok((l, data.to_path_buf())),
        Err(e) => e,
    };
    let h = holder(data);
    let me = format!("{} on {computer} ", login.unwrap_or("someone"));
    let mine = login.is_some() && h.to_lowercase().starts_with(&me.to_lowercase());
    match personal {
        Some(p) if !mine && p != data => {
            tracing::info!("data folder held by another session; using this login's own");
            acquire_as(p, login, computer).map(|l| (l, p.to_path_buf()))
        }
        _ if mine => Err(format!(
            "Vibe Files is already open under your login on this computer ({}). Switch to that window, or close it first.",
            data.display()
        )),
        _ => Err(first),
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

/// A copy of Vibe Files running under this login on this computer that holds a data folder.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnCopy {
    pub pid: u32,
    /// Its Windows session (Remote Desktop / Citrix: one per sign-in).
    pub session: Option<u32>,
    /// In this process's session.
    pub same_session: bool,
    /// Since when (`in-use.txt`'s first line).
    pub since: String,
}

/// The copy that holds `data`, when `in-use.txt` names this login on this computer and its
/// process is still a running Vibe Files (not a process that reused the number).
pub fn own_copy(data: &Path, login: Option<&str>, computer: &str) -> Option<OwnCopy> {
    let text = std::fs::read_to_string(data.join("in-use.txt")).ok()?;
    let mut lines = text.lines();
    let first = lines.next()?.trim().to_string();
    let me = format!("{} on {computer} ", login?);
    if !first.to_lowercase().starts_with(&me.to_lowercase()) {
        return None;
    }
    let pid: u32 = lines
        .next()?
        .strip_prefix("process ")?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    if pid == std::process::id() || !is_this_program(pid) {
        return None;
    }
    let session = session_of(pid);
    Some(OwnCopy {
        pid,
        session,
        same_session: session.is_some() && session == session_of(std::process::id()),
        since: first
            .split_once(" since ")
            .map(|(_, t)| t.to_string())
            .unwrap_or_default(),
    })
}

/// The Windows session a process runs in.
pub fn session_of(pid: u32) -> Option<u32> {
    use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    let mut sid = 0u32;
    // SAFETY: `sid` receives the session id.
    unsafe { ProcessIdToSessionId(pid, &mut sid) }
        .ok()
        .map(|()| sid)
}

/// `pid` is running and is the same program file name as this one (`vibe-files.exe`).
fn is_this_program(pid: u32) -> bool {
    use windows::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    let Ok(me) = std::env::current_exe() else {
        return false;
    };
    // SAFETY: plain Win32 calls on a handle closed below; `buf` holds `len` UTF-16 units.
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut code = 0u32;
        let alive = GetExitCodeProcess(h, &mut code).is_ok() && code == STILL_ACTIVE.0 as u32;
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let named = QueryFullProcessImageNameW(
            h,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(h);
        if !alive || !named {
            return false;
        }
        let theirs = PathBuf::from(String::from_utf16_lossy(&buf[..len as usize]));
        theirs.file_name().map(|n| n.to_ascii_lowercase())
            == me.file_name().map(|n| n.to_ascii_lowercase())
    }
}

/// Brings `pid`'s main window forward (restored if minimised). False when it has no window
/// (stuck, or in another session).
pub fn bring_to_front(pid: u32) -> bool {
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GW_OWNER, GetWindow, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
        SW_RESTORE, SetForegroundWindow, ShowWindow,
    };
    struct Find {
        pid: u32,
        found: Option<HWND>,
    }
    unsafe extern "system" fn each(hwnd: HWND, l: LPARAM) -> windows::core::BOOL {
        // SAFETY: `l` is the `Find` passed to EnumWindows below, alive for the call.
        let f = unsafe { &mut *(l.0 as *mut Find) };
        let mut owner = 0u32;
        // SAFETY: plain Win32 calls on a window handle EnumWindows gave.
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut owner));
            if owner == f.pid
                && IsWindowVisible(hwnd).as_bool()
                && GetWindow(hwnd, GW_OWNER).is_err()
            {
                f.found = Some(hwnd);
                return false.into();
            }
        }
        true.into()
    }
    let mut f = Find { pid, found: None };
    // SAFETY: the callback only reads and writes `f`, which outlives the call.
    let _ = unsafe { EnumWindows(Some(each), LPARAM(&mut f as *mut Find as isize)) };
    let Some(hwnd) = f.found else {
        return false;
    };
    // SAFETY: plain Win32 calls on a live window handle.
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let _ = SetForegroundWindow(hwnd);
    }
    true
}

/// Ends `pid` (the person's own leftover copy) and waits up to 15 s for it to go.
pub fn close_copy(pid: u32) -> Result<(), String> {
    use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
    };
    // SAFETY: plain Win32 calls on a handle closed below.
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, pid)
            .map_err(|e| format!("cannot reach the other copy: {e}"))?;
        let r = TerminateProcess(h, 1).map_err(|e| format!("cannot close the other copy: {e}"));
        let gone = WaitForSingleObject(h, 15_000) == WAIT_OBJECT_0;
        let _ = CloseHandle(h);
        r?;
        if !gone {
            return Err("the other copy did not close in time".into());
        }
    }
    tracing::warn!(pid, "closed this login's other copy of Vibe Files");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Owner 2026-11-09 (Citrix): the copy holding the folder is found by its process and
    /// session; another login's or another computer's copy, or a process that is gone, is not.
    #[test]
    fn the_copy_holding_the_folder_is_found_by_process_and_session() {
        let t = tempfile::tempdir().unwrap();
        let data = t.path().join("data");
        let lock = acquire_as(&data, Some(r"XC\kim"), "SERVER-1").unwrap();
        assert!(holder(&data).starts_with(r"XC\kim on SERVER-1 since "));
        // This process holds it: not "another copy".
        assert_eq!(own_copy(&data, Some(r"XC\kim"), "SERVER-1"), None);
        // As if another copy of this program held it: a child running this test exe, kept
        // alive by its stdin until the checks are done.
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "datalock::tests::wait_for_stdin", "--nocapture"])
            .env("VF_WAIT_FOR_STDIN", "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        let first = holder(&data);
        std::fs::write(
            data.join("in-use.txt"),
            format!("{first}\nprocess {pid} session 0\n"),
        )
        .unwrap();
        let c = own_copy(&data, Some(r"xc\KIM"), "SERVER-1").expect("found");
        assert_eq!(c.pid, pid);
        assert!(c.same_session);
        assert!(!c.since.is_empty());
        assert_eq!(own_copy(&data, Some(r"XC\pat"), "SERVER-1"), None);
        assert_eq!(own_copy(&data, Some(r"XC\kim"), "SERVER-2"), None);
        close_copy(pid).unwrap();
        let _ = child.wait();
        assert_eq!(own_copy(&data, Some(r"XC\kim"), "SERVER-1"), None, "gone");
        drop(lock);
    }

    /// Helper for the test above: in the child, waits until its stdin closes.
    #[test]
    fn wait_for_stdin() {
        if std::env::var_os("VF_WAIT_FOR_STDIN").is_some() {
            use std::io::Read;
            let _ = std::io::stdin().read_to_end(&mut Vec::new());
        }
    }

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

    /// Owner 2026-11-05: someone else's open copy never stops a person opening Vibe Files.
    #[test]
    fn another_login_or_pc_gets_its_own_folder_instead_of_a_refusal() {
        let t = tempfile::tempdir().unwrap();
        let shared = t.path().join("APPS").join("data");
        let kim = t.path().join("kim");
        let pat = t.path().join("pat");
        // Kim opens first, on SERVER-1: the folder beside the exe.
        let (k, at) = acquire_default(&shared, Some(&kim), Some(r"XC\kim"), "SERVER-1").unwrap();
        assert_eq!(at, shared);
        // Pat, while Kim has it: Pat's own folder, no refusal.
        let (p, at) = acquire_default(&shared, Some(&pat), Some(r"XC\pat"), "SERVER-1").unwrap();
        assert_eq!(at, pat);
        // Kim on another session host: Kim's own folder there.
        let other = t.path().join("kim-on-2");
        let (k2, at) = acquire_default(&shared, Some(&other), Some(r"XC\kim"), "SERVER-2").unwrap();
        assert_eq!(at, other);
        // Kim again on SERVER-1: a second window, refused, and told so.
        let e = acquire_default(&shared, Some(&kim), Some(r"xc\KIM"), "SERVER-1")
            .err()
            .unwrap();
        assert!(e.contains("already open under your login"), "{e}");
        drop((k, p, k2));
    }
}
