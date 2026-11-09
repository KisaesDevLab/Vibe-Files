//! `vibe-scan32.exe`: the 32-bit scanning helper (owner 2026-11-09). Built for 32-bit Windows
//! (`i686-pc-windows-msvc`) and shipped beside `vibe-files.exe`, which runs it to list and scan
//! with scanners that remote-scanning set-ups (Citrix Workspace, Remote Desktop scanning
//! utilities) offer to 32-bit programs only. See `vf_ingest::twain::helper`.

// No console window: Vibe Files reads the answer from its output.
#![windows_subsystem = "windows"]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    std::process::exit(vf_ingest::twain::helper::helper_main(&args));
}
