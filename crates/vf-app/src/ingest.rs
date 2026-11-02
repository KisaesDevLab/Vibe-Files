//! Background worker for the folders Vibe Files prepares before Intake (PHASES P02): scans
//! (cover sheets), phone photos (image → PDF), and the print-capture folder (parser profiles). Runs on its own thread
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

/// Where phone photos (JPG / PNG / TIFF / HEIC) can be dropped; images dropped in the inbox are
/// converted too.
pub fn photos_dir(data: &Path) -> PathBuf {
    data.join("Photos")
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
    let pdfium = filer_pdf::pdf::find_pdfium(&crate::shell::exe_dir())
        .and_then(|d| filer_pdf::pdf::pdfium(&d).ok());
    let scans = scans_dir(data);
    let photos = photos_dir(data);
    let parsers = vf_fs::capture::parsers_dir(data);
    if let Err(e) = vf_fs::capture::seed_profiles(&parsers) {
        say(format!("Parser profiles: {e}"));
    }
    let _ = std::fs::create_dir_all(&scans);
    let _ = std::fs::create_dir_all(&photos);
    while !stop.load(Ordering::Relaxed) {
        let inbox = inbox(data);
        let pass = vf_fs::incoming::process_photos(&[photos.clone(), inbox.clone()], &inbox);
        if pass.queued > 0 {
            say(format!(
                "Photos: {} converted to PDF and queued.",
                pass.queued
            ));
        }
        for e in pass.errors {
            say(format!("Photo could not be converted: {e}"));
        }
        let capture = crate::config::Config::load(data)
            .and_then(|c| c.print_capture)
            .unwrap_or_else(|| data.join("Print capture"));
        let _ = std::fs::create_dir_all(&capture);
        let pass = vf_fs::capture::process_capture(&conn, &capture, &inbox, &parsers, hints);
        if pass.routed + pass.queued > 0 {
            say(format!(
                "Print capture: {} recognized by a parser profile, {} queued as printed.",
                pass.routed, pass.queued
            ));
        }
        for e in pass.errors {
            say(format!("Print capture: {e}"));
        }
        if let Some(pdfium) = pdfium {
            let pass = vf_fs::incoming::process_scans(&conn, pdfium, &scans, &inbox, hints);
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
