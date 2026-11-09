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
mod server;
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
    let chosen = args.data.is_some();
    let personal = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|l| l.join("Vibe Files").join("data"));
    let exe_dir = vf_core::exe_dir().context("locating the exe")?;
    let mut data = match (args.data, &personal) {
        (Some(d), _) => d,
        // Run from a network path (a server, or an office PC run from the shared program
        // folder): never a data folder on the share (Q-70: SQLite there can be damaged).
        (None, Some(p)) if server::server_mode() || server::is_network(&exe_dir) => {
            server::personal_data(
                &exe_dir.join("data"),
                p,
                vf_core::windows_login().as_deref(),
            )
        }
        (None, _) => datalock::default_data(
            &exe_dir.join("vibe-files.exe"),
            personal.clone(),
            vf_core::windows_login().as_deref(),
        ),
    };
    // The window: one copy per data folder. The default folder held by another login or PC
    // (a shared program folder) means this login's own folder, not a refusal (owner
    // 2026-11-05); taken before logging starts so the log goes to the folder used.
    let mut window_lock = None;
    if args.command.is_none() {
        let login = vf_core::windows_login();
        let computer = std::env::var("COMPUTERNAME").unwrap_or_default();
        let take = || {
            if chosen {
                datalock::acquire(&data, true).map(|l| (l, data.clone()))
            } else {
                datalock::acquire_default(&data, personal.as_deref(), login.as_deref(), &computer)
            }
        };
        let mut taken = take();
        // Held by this login's own copy (owner 2026-11-09, Citrix / Remote Desktop): its window
        // in this session comes forward; a copy left in another (disconnected) session, or one
        // with no window, can be closed so this one opens.
        if taken.is_err()
            && let Some(other) =
                datalock::own_copy(&data, login.as_deref(), &computer).or_else(|| {
                    personal
                        .as_deref()
                        .filter(|_| !chosen)
                        .and_then(|p| datalock::own_copy(p, login.as_deref(), &computer))
                })
        {
            if !other.window {
                // A command (sync, backup, a Task Scheduler run) is never closed from here: it
                // may be writing the catalog (QA).
                rfd::MessageDialog::new()
                    .set_title("Vibe Files")
                    .set_description(format!(
                        "A Vibe Files task (such as a sync or backup) started at {} is using your data folder. Wait for it to finish, then open Vibe Files again.",
                        other.since
                    ))
                    .set_level(rfd::MessageLevel::Info)
                    .show();
                return Ok(std::process::ExitCode::from(7));
            }
            // In this session: its window comes forward. A copy still starting (a double
            // double-click) has no window yet: it is given 10 seconds (QA).
            if other.same_session {
                let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
                loop {
                    if datalock::bring_to_front(other.pid) {
                        return Ok(std::process::ExitCode::SUCCESS);
                    }
                    if std::time::Instant::now() >= until {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
            }
            let where_ = if other.same_session {
                "in this session, but its window cannot be found (it may have stopped responding)"
                    .to_string()
            } else {
                format!(
                    "in another of your Windows sessions on this computer{}, probably one that was disconnected instead of signed out",
                    other
                        .session
                        .map(|s| format!(" (session {s})"))
                        .unwrap_or_default()
                )
            };
            let answer = rfd::MessageDialog::new()
                .set_title("Vibe Files")
                .set_description(format!(
                    "Vibe Files is still running under your login {where_}, since {}.\n\nClose that copy and open Vibe Files here? Work not saved in that copy is lost.",
                    other.since
                ))
                .set_level(rfd::MessageLevel::Warning)
                .set_buttons(rfd::MessageButtons::YesNo)
                .show();
            if answer != rfd::MessageDialogResult::Yes {
                return Ok(std::process::ExitCode::from(7));
            }
            taken = datalock::close_copy(other.pid).and_then(|()| {
                // Windows (or a virus scanner) may take a moment to let go of the lock file.
                let mut r = take();
                for _ in 0..4 {
                    if r.is_ok() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    r = take();
                }
                r
            });
        }
        match taken {
            Ok((l, at)) => {
                data = at;
                window_lock = Some(l);
            }
            Err(e) => {
                rfd::MessageDialog::new()
                    .set_title("Vibe Files")
                    .set_description(&e)
                    .set_level(rfd::MessageLevel::Warning)
                    .show();
                return Ok(std::process::ExitCode::from(7));
            }
        }
    }
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
            match datalock::acquire(&data, false) {
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
    // Held for as long as the window is open.
    let _lock = window_lock;
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
