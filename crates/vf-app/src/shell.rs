//! Handing files to Windows (P01 step 8): open in the registered app, show in Explorer.

use std::path::Path;

/// What Browse asks the shell to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellAction {
    /// Open the file in the app Windows has registered for its type.
    Open(std::path::PathBuf),
    /// Open its folder in Explorer with the file selected.
    Reveal(std::path::PathBuf),
}

/// Runs `a` against Windows.
pub fn run(a: &ShellAction) -> Result<(), String> {
    match a {
        ShellAction::Open(p) => open(p),
        ShellAction::Reveal(p) => reveal(p),
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn open(p: &Path) -> Result<(), String> {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::PCWSTR;
    if !p.exists() {
        return Err(format!("{} is not there any more.", p.display()));
    }
    let verb = wide("open");
    let file = wide(&p.to_string_lossy());
    // SAFETY: both strings are NUL-terminated UTF-16 buffers that outlive the call.
    let r = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // Values above 32 mean success (ShellExecute's documented contract).
    match r.0 as isize {
        n if n > 32 => Ok(()),
        31 => Err(format!(
            "No app is set to open {} files. Use Open in Explorer and choose one.",
            p.extension()
                .map(|e| e.to_string_lossy().to_uppercase())
                .unwrap_or_else(|| "these".into())
        )),
        n => Err(format!(
            "Windows could not open {} (error {n}).",
            p.display()
        )),
    }
}

fn reveal(p: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    // Explorer parses its own command line: `/select,"<path>"` exactly, unquoted switch.
    std::process::Command::new("explorer.exe")
        .raw_arg(format!("/select,\"{}\"", p.display()))
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Explorer could not be started: {e}"))
}

/// A cabinet-relative path (forward slashes) as a Windows path under `root`.
pub fn full_path(root: &Path, rel: &str) -> std::path::PathBuf {
    root.join(rel.replace('/', "\\"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_path_uses_windows_separators() {
        let p = full_path(
            Path::new(r"\server\share\Clients"),
            "Acme (1001)/Income Tax/2025/w2.pdf",
        );
        assert_eq!(
            p.to_string_lossy(),
            r"\server\share\Clients\Acme (1001)\Income Tax\2025\w2.pdf"
        );
    }

    #[test]
    fn opening_a_missing_file_says_so_without_launching() {
        let tmp = tempfile::tempdir().unwrap();
        let e = run(&ShellAction::Open(tmp.path().join("gone.pdf"))).unwrap_err();
        assert!(e.contains("not there"), "{e}");
    }
}
