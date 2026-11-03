//! OCR workers for the open cabinet (P03) and their footer status.

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
}

impl OcrState {
    /// Starts the workers for `opened`, unless paused on this PC.
    pub fn start(ctx: &egui::Context, opened: &Opened, prefs: &Ocr, device_id: &str) -> OcrState {
        let mut s = OcrState::default();
        if prefs.paused {
            return s;
        }
        let Some(pdfium_dir) = crate::ui::intake::pdfium_dir() else {
            s.error = Some("pdfium.dll not found beside the exe".into());
            return s;
        };
        let ctx = ctx.clone();
        match Queue::start(
            queue::Config {
                catalog: opened.catalog.clone(),
                key: opened.key.clone(),
                root: opened.root.clone(),
                pdfium_dir,
                tesseract: prefs.tesseract.clone(),
                threads: queue::default_threads(),
                device_id: device_id.into(),
            },
            queue::tesseract_factory(prefs.tesseract.clone()),
            move || ctx.request_repaint_after(Duration::from_millis(500)),
        ) {
            Ok(q) => s.queue = Some(q),
            Err(e) => s.error = Some(e),
        }
        s
    }

    /// Job counts, looked up at most every 2 s.
    pub fn counts(&mut self, conn: &Connection) -> &Counts {
        if self.counted.is_none_or(|t| t.elapsed() >= RECOUNT) {
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
            (Some(_), _) => {
                let waiting = c.pending();
                let missing = matches!(engine, Some(Err(_))) && waiting > 0;
                match (waiting, missing) {
                    (0, _) if c.failed > 0 => {
                        format!("Search index up to date · {} could not be read", c.failed)
                    }
                    (0, _) => "Search index up to date".into(),
                    (n, true) => format!("OCR waiting: {n} documents, no OCR engine (Admin › OCR)"),
                    (n, false) => format!("OCR: {n} documents to read"),
                }
            }
        }
    }
}
