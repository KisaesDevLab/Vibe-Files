//! Vibe Files: document management over a plain client folder tree.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod cli;
mod config;
mod datalock;
mod help;
mod logging;
mod perf;
mod print;
mod redact;
mod screens;
mod selfcheck;
mod session;
mod shared;
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
        None => datalock::default_data(
            &vf_core::exe_dir()
                .context("locating the exe")?
                .join("vibe-files.exe"),
            std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
            vf_core::windows_login().as_deref(),
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
        // Commands that change the catalog or the cabinet need the data folder to themselves.
        let _lock = if cmd.changes_things() {
            match datalock::acquire(&data) {
                Ok(l) => Some(l),
                Err(e) => {
                    eprintln!("{e}");
                    return Ok(std::process::ExitCode::from(7));
                }
            }
        } else {
            None
        };
        let code = cli::run(cmd, &data, &mut std::io::stdout())?;
        return Ok(std::process::ExitCode::from(code as u8));
    }
    // One window per data folder: a second copy would share the catalog and the crash journal.
    let _lock = match datalock::acquire(&data) {
        Ok(l) => l,
        Err(e) => {
            rfd::MessageDialog::new()
                .set_title("Vibe Files")
                .set_description(&e)
                .set_level(rfd::MessageLevel::Warning)
                .show();
            return Ok(std::process::ExitCode::from(7));
        }
    };
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
