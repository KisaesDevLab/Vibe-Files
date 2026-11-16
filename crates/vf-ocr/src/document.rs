//! Reading a document a person asked to have read (owner 2026-11-15, Q-116, PA-77): the pages
//! without text are rendered, read by the engine, and the words written into a copy of the PDF
//! as an invisible text layer ([`vf_pdf::layer`]), so the Viewer's text features (ten-key's
//! pick from page, find, copy) work on a scan. One document at a time, when asked: nothing
//! here reads the cabinet on its own (that queue was removed, PA-65).

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use pdfium_render::prelude::*;

use crate::{OcrEngine, OcrPage};

const DPI: f32 = 300.0;
/// The longest side of the picture given to the engine, pixels.
const MAX_SIDE: f32 = 4200.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pages {
    /// One page (0-based). `even_with_text`: read it although it has text of its own (a stamp
    /// or a header over a scan).
    One { page: u32, even_with_text: bool },
    /// Every page that has no text of its own.
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Outcome {
    /// Pages given a text layer.
    pub read: u32,
    /// Pages left alone: they have text of their own.
    pub with_text: u32,
    /// Pages read on which no words were found.
    pub empty: u32,
    /// Stopped when asked: nothing was written.
    pub cancelled: bool,
}

/// Reads `pages` of `src` and writes `src` plus their text layer to `out`. `out` is written
/// only when some page got words (`Outcome::read > 0`). `progress(done, of)` is called before
/// each page is read; `cancel` is looked at between pages.
pub fn add_text(
    pdfium: &Pdfium,
    engine: &dyn OcrEngine,
    src: &Path,
    out: &Path,
    pages: Pages,
    progress: &dyn Fn(u32, u32),
    cancel: &AtomicBool,
) -> Result<Outcome, String> {
    let mut rep = Outcome::default();
    let mut read: Vec<(u32, OcrPage)> = Vec::new();
    {
        let doc = pdfium
            .load_pdf_from_file(src, None)
            .map_err(|e| format!("cannot read the PDF: {e}"))?;
        let count = doc.pages().len() as u32;
        let (wanted, even_with_text): (Vec<u32>, bool) = match pages {
            Pages::One {
                page,
                even_with_text,
            } => (vec![page], even_with_text),
            Pages::All => ((0..count).collect(), false),
        };
        let of = wanted.len() as u32;
        for (i, n) in wanted.into_iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                rep.cancelled = true;
                return Ok(rep);
            }
            progress(i as u32, of);
            let page = doc
                .pages()
                .get(n as i32)
                .map_err(|_| format!("page {} is not in the PDF", n + 1))?;
            let own = vf_pdf::text::page_text(&page);
            // Any text of its own, a layer written here before included: reading the page
            // again would put every word on it twice.
            if !even_with_text && own.chars().any(char::is_alphanumeric) {
                rep.with_text += 1;
                continue;
            }
            let (w, h) = (page.width().value, page.height().value);
            let scale = (DPI / 72.0).min(MAX_SIDE / w.max(h).max(1.0));
            let img = page
                .render_with_config(&PdfRenderConfig::new().set_target_width((w * scale) as i32))
                .and_then(|b| b.as_image())
                .map_err(|e| format!("page {} could not be drawn: {e}", n + 1))?
                .to_luma8();
            let ocr = engine
                .recognize(&img, (scale * 72.0).round() as u32)
                .map_err(|e| e.to_string())?;
            if ocr.words.is_empty() {
                rep.empty += 1;
            } else {
                rep.read += 1;
                read.push((n, ocr));
            }
        }
    }
    if cancel.load(Ordering::Relaxed) {
        rep.cancelled = true;
        return Ok(rep);
    }
    if !read.is_empty() {
        let layers: Vec<vf_pdf::layer::PageWords<'_>> = read
            .iter()
            .map(|(page, ocr)| vf_pdf::layer::PageWords { page: *page, ocr })
            .collect();
        vf_pdf::layer::add_text_layer(src, out, &layers)?;
    }
    Ok(rep)
}
