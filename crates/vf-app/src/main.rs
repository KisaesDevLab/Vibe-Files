//! Vibe Files: document management over a plain client folder tree.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod cli;
mod config;
mod logging;
mod redact;
mod session;
mod shell;
mod ui;

use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;

#[derive(Parser)]
#[command(name = "vibe-files", version, about = "Vibe Files document management")]
struct Args {
    /// Data folder (catalog, config, caches, logs). Defaults to `data` next to the exe.
    #[arg(long, global = true)]
    data: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<cli::Command>,
}

fn main() -> anyhow::Result<std::process::ExitCode> {
    let args = Args::parse();
    let data = match args.data {
        Some(d) => d,
        None => std::env::current_exe()
            .context("locating the exe")?
            .parent()
            .map(|p| p.join("data"))
            .unwrap_or_else(|| PathBuf::from("data")),
    };
    let _log = logging::init(&data);
    if let Some(cmd) = &args.command {
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
