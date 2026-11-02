//! Background worker for the folders Vibe Files prepares before Intake (PHASES P02): scans
//! (cover sheets) now; phone photos and print capture join in steps 4–5. Runs on its own thread
//! with its own catalog connection and reports what it did.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use vf_core::db::{self, CatalogKey, Storage};
use vf_fs::intake::Hints;

/// Where a scanner or a "scan to folder" copier should save stacks.
pub fn scans_dir(data: &Path) -> PathBuf {
    data.join("Scans")
}

/// The Filer inbox processed files go to: Filer's first inbox (Setup), else the default.
pub fn inbox(data: &Path) -> PathBuf {
    let dir = crate::ui::intake::filer_dir(data);
    filer_core::config::Settings::load_or_default(&dir)
        .0
        .inboxes
        .first()
        .cloned()
        .unwrap_or_else(|| crate::ui::intake::default_inbox(data))
}

pub struct Ingest {
    stop: Arc<AtomicBool>,
    pub rx: Receiver<String>,
}

impl Drop for Ingest {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_default()
}

impl Ingest {
    pub fn start(
        ctx: &egui::Context,
        catalog: PathBuf,
        key: CatalogKey,
        data: PathBuf,
        hints: Hints,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let (s, ctx) = (stop.clone(), ctx.clone());
        std::thread::spawn(move || run(&s, &ctx, &tx, &catalog, &key, &data, &hints));
        Self { stop, rx }
    }
}

fn run(
    stop: &AtomicBool,
    ctx: &egui::Context,
    tx: &Sender<String>,
    catalog: &Path,
    key: &CatalogKey,
    data: &Path,
    hints: &Hints,
) {
    let say = |m: String| {
        let _ = tx.send(m);
        ctx.request_repaint();
    };
    let conn = match db::open(catalog, key, Storage::Local) {
        Ok(c) => c,
        Err(e) => return say(format!("Scan processing is off: {e}")),
    };
    let pdfium =
        filer_pdf::pdf::find_pdfium(&exe_dir()).and_then(|d| filer_pdf::pdf::pdfium(&d).ok());
    let scans = scans_dir(data);
    let _ = std::fs::create_dir_all(&scans);
    while !stop.load(Ordering::Relaxed) {
        if let Some(pdfium) = pdfium {
            let pass = vf_fs::incoming::process_scans(&conn, pdfium, &scans, &inbox(data), hints);
            if pass.routed + pass.queued > 0 {
                say(format!(
                    "Scans: {} documents split from cover sheets, {} queued as scanned.",
                    pass.routed, pass.queued
                ));
            }
            for e in pass.errors {
                say(format!("Scan could not be processed: {e}"));
            }
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}
