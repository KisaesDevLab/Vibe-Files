//! Open-with (PHASES P01 step 8): open a document in its registered app, show it in Explorer.

use std::path::Path;

/// Opens `path` with the app Windows has registered for it (ShellExecute "open").
#[cfg(windows)]
pub fn open(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let wide = |s: &std::ffi::OsStr| -> Vec<u16> { s.encode_wide().chain(Some(0)).collect() };
    let verb = wide("open".as_ref());
    let file = wide(path.as_os_str());
    // SAFETY: both strings are NUL-terminated UTF-16 that outlive the call; null HWND,
    // parameters and directory are allowed.
    let r = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // Values above 32 mean success (ShellExecute contract).
    if r as usize > 32 {
        Ok(())
    } else {
        Err(format!(
            "Windows could not open {} (no app registered for this file type?)",
            path.display()
        ))
    }
}

/// Opens an Explorer window with `path` selected.
#[cfg(windows)]
pub fn reveal(path: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("explorer.exe")
        .raw_arg(format!("/select,\"{}\"", path.display()))
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("cannot start Explorer: {e}"))
}

#[cfg(not(windows))]
pub fn open(path: &Path) -> Result<(), String> {
    Err(format!("opening {} needs Windows", path.display()))
}

#[cfg(not(windows))]
pub fn reveal(path: &Path) -> Result<(), String> {
    Err(format!("showing {} needs Windows", path.display()))
}

/// Absolute path of a cabinet-relative document path.
pub fn abs(root: &Path, rel: &str) -> std::path::PathBuf {
    rel.split('/').fold(root.to_path_buf(), |p, s| p.join(s))
}
