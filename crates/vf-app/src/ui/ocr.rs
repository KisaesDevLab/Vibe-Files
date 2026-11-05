//! OCR workers for the open cabinet (P03) and their footer status.

use std::path::Path;
use std::time::{Duration, Instant};

use vf_core::db::Connection;
use vf_ocr::queue::{self, Counts, Queue};

use crate::config::Ocr;
use crate::session::Opened;

const RECOUNT: Duration = Duration::from_secs(2);

#[derive(Default)]
pub struct OcrState {
    pub queue: Option<Queue>,
    /// Why the queue did not start.
    pub error: Option<String>,
    counts: Counts,
    counted: Option<Instant>,
    /// Several servers (PA-36): the server text recognition runs on, when not this one. Here
    /// the queue only indexes pages that carry their own text (the text layers it writes).
    pub elsewhere: Option<String>,
}

/// Starts the OCR workers for a cabinet with this PC's settings (the app and the CLI).
pub fn start_queue(
    catalog: &Path,
    root: &Path,
    prefs: &Ocr,
    device_id: &str,
    threads: usize,
    wake: impl Fn() + Send + Sync + 'static,
) -> Result<Queue, String> {
    let pdfium_dir = crate::ui::intake::pdfium_dir()
        .ok_or_else(|| "pdfium.dll not found beside vibe-files.exe".to_string())?;
    Queue::start(
        queue::Config {
            catalog: catalog.to_path_buf(),
            root: root.to_path_buf(),
            pdfium_dir,
            threads,
            device_id: device_id.into(),
        },
        queue::tesseract_factory(prefs.tesseract.clone()),
        wake,
    )
}

impl OcrState {
    /// Starts the workers for `opened`, unless paused on this PC.
    pub fn start(ctx: &egui::Context, opened: &Opened, prefs: &Ocr, device_id: &str) -> OcrState {
        let mut s = OcrState::default();
        if prefs.paused {
            return s;
        }
        if !crate::server::runs_here(&opened.root) {
            s.elsewhere = Some(
                crate::server::main_server(&opened.root)
                    .map(|m| m.computer)
                    .unwrap_or_else(|| "another server".into()),
            );
        }
        let ctx = ctx.clone();
        let wake = move || ctx.request_repaint_after(Duration::from_millis(500));
        let started = match &s.elsewhere {
            // No engine here: scans wait for the main server's text layer.
            Some(main) => {
                let why = format!("text recognition runs on {main}");
                crate::ui::intake::pdfium_dir()
                    .ok_or_else(|| "pdfium.dll not found beside vibe-files.exe".to_string())
                    .and_then(|pdfium_dir| {
                        Queue::start(
                            queue::Config {
                                catalog: opened.catalog.clone(),
                                root: opened.root.clone(),
                                pdfium_dir,
                                threads: 1,
                                device_id: device_id.into(),
                            },
                            std::sync::Arc::new(move || Err(why.clone())),
                            wake,
                        )
                    })
            }
            None => start_queue(
                &opened.catalog,
                &opened.root,
                prefs,
                device_id,
                queue::default_threads(),
                wake,
            ),
        };
        match started {
            Ok(q) => s.queue = Some(q),
            Err(e) => s.error = Some(e),
        }
        s
    }

    /// Job counts: the workers' own count; looked up here (at most every 2 s) only before
    /// they have one (PA-34: not a count of a long queue on the window's thread).
    pub fn counts(&mut self, conn: &Connection) -> &Counts {
        if let Some(c) = self.queue.as_ref().and_then(|q| q.status().counts) {
            self.counts = c;
            self.counted = Some(Instant::now());
        } else if self.counted.is_none_or(|t| t.elapsed() >= RECOUNT) {
            self.counted = Some(Instant::now());
            if let Ok(c) = Counts::load(conn) {
                self.counts = c;
            }
        }
        &self.counts
    }

    /// Footer text.
    pub fn footer(&mut self, conn: &Connection) -> String {
        let engine = self.queue.as_ref().map(|q| q.status().engine);
        let c = self.counts(conn).clone();
        match (&self.queue, &self.error) {
            (None, Some(e)) => format!("OCR off: {e}"),
            (None, None) => "OCR paused on this PC".into(),
            (Some(_), _) if self.elsewhere.is_some() => {
                let main = self.elsewhere.clone().unwrap_or_default();
                match c.pending() {
                    0 => format!("Search index up to date · text recognition runs on {main}"),
                    n => format!(
                        "Text recognition runs on {main}: {n} scans become searchable here once it has read them"
                    ),
                }
            }
            (Some(_), _) => {
                let waiting = c.pending();
                let missing = matches!(engine, Some(Err(_))) && waiting > 0;
                match (waiting, missing) {
                    (0, _) if c.failed > 0 => {
                        format!("Search index up to date · {} could not be read", c.failed)
                    }
                    (0, _) => "Search index up to date".into(),
                    (n, true) => format!(
                        "OCR waiting: {n} documents, no OCR engine (see Help › Text recognition)"
                    ),
                    (n, false) => format!("OCR: {n} documents to read"),
                }
            }
        }
    }
}
