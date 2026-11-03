//! Vibe Files: document management over a plain client folder tree.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod cli;
mod config;
mod logging;
mod perf;
mod redact;
mod selfcheck;
mod session;
mod shell;
mod soak;
mod ui;

use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;

#[derive(Parser)]
#[command(name = "vibe-files", version, about = "Vibe Files document management")]
struct Args {
    /// Data folder (catalog, config, caches, logs). Defaults to `data` next to the exe; for a
    /// winget install, `%LOCALAPPDATA%\Vibe Files\data` (winget replaces its package folder).
    #[arg(long, global = true)]
    data: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<cli::Command>,
}

fn main() -> anyhow::Result<std::process::ExitCode> {
    let args = Args::parse();
    let data = match args.data {
        Some(d) => d,
        None => default_data(
            &vf_core::exe_dir()
                .context("locating the exe")?
                .join("vibe-files.exe"),
            std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
        ),
    };
    let _log = logging::init(&data);
    if let Some(cmd) = &args.command {
        // The release exe is a windowed program: typed in a console, its output goes there.
        // SAFETY: plain Win32 call; fails harmlessly when there is no parent console.
        #[cfg(not(debug_assertions))]
        unsafe {
            let _ = windows::Win32::System::Console::AttachConsole(
                windows::Win32::System::Console::ATTACH_PARENT_PROCESS,
            );
        }
        let code = cli::run(cmd, &data, &mut std::io::stdout())?;
        return Ok(std::process::ExitCode::from(code as u8));
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Vibe Files")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([1024.0, 700.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "Vibe Files",
        options,
        Box::new(|cc| Ok(Box::new(app::VibeFilesApp::new(&cc.egui_ctx, data)))),
    )
    .map_err(|e| anyhow::anyhow!("ui failed: {e}"))?;
    Ok(std::process::ExitCode::SUCCESS)
}

/// `data` beside the exe (the portable copy, principle 4), unless the exe was installed by
/// winget: its package folder is replaced on upgrade and removed on uninstall, so the data
/// lives in `%LOCALAPPDATA%\Vibe Files\data`. A `data` folder already beside the exe always
/// wins.
fn default_data(exe: &std::path::Path, local_app_data: Option<PathBuf>) -> PathBuf {
    let dir = exe.parent().unwrap_or(std::path::Path::new("."));
    let beside = dir.join("data");
    let winget = dir
        .to_string_lossy()
        .to_lowercase()
        .contains(r"\microsoft\winget\packages\");
    match local_app_data {
        Some(l) if winget && !beside.exists() => l.join("Vibe Files").join("data"),
        _ => beside,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_lives_beside_the_exe_except_under_winget() {
        let t = tempfile::tempdir().unwrap();
        let local = t.path().join("Local");
        let portable = t.path().join("Tools").join("vibe-files.exe");
        assert_eq!(
            default_data(&portable, Some(local.clone())),
            t.path().join("Tools").join("data")
        );
        let pkg = local.join(
            r"Microsoft\WinGet\Packages\Kisaes.VibeFiles_Microsoft.Winget.Source_8wekyb3d8bbwe",
        );
        let exe = pkg.join("vibe-files.exe");
        assert_eq!(
            default_data(&exe, Some(local.clone())),
            local.join("Vibe Files").join("data")
        );
        // A data folder already beside it (moved there by hand) is kept.
        std::fs::create_dir_all(pkg.join("data")).unwrap();
        assert_eq!(default_data(&exe, Some(local)), pkg.join("data"));
    }
}
