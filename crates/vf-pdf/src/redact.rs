//! True redaction (P04 step 5): nothing under a box survives in the file.
//!
//! A page with redactions is rebuilt: rendered at 300 dpi (with its annotations), the boxes
//! painted black into the pixels, and the page's content, resources and annotations replaced
//! by that one image. The words outside the boxes go back on as an invisible text layer
//! ([`crate::layer`]) so the page stays searchable. Document info and XMP metadata are
//! removed, unreferenced objects pruned (old content streams and images are gone, not just
//! hidden), and the file is written whole.
//!
//! [`verify`] then re-extracts every page's text and searches every string and stream in the
//! output for the redacted values; the caller refuses the file if any is found.

use std::path::Path;

use lopdf::{Document, Object};
use pdfium_render::prelude::Pdfium;

use crate::layer::{self, PageWords};
use crate::text;
use crate::words::OcrPage;

/// Resolution redacted pages are rebuilt at.
pub const DPI: f32 = crate::raster::DPI;

/// A box to black out, user space of page `page` (0-based).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Redaction {
    pub page: usize,
    pub rect: [f32; 4],
}

/// Writes `src` with `boxes` redacted to `out`. Returns the pages rebuilt.
pub fn apply(
    pdfium: &Pdfium,
    src: &Path,
    out: &Path,
    boxes: &[Redaction],
) -> Result<Vec<usize>, String> {
    let bytes = std::fs::read(src).map_err(|e| format!("cannot read {}: {e}", src.display()))?;
    let mut doc = Document::load_mem(&bytes).map_err(|e| format!("cannot read the PDF: {e}"))?;
    if doc.is_encrypted() {
        return Err("the PDF is password protected".into());
    }
    let geoms = layer::geometries(&doc);
    let page_ids: Vec<_> = doc.get_pages().values().copied().collect();
    let pdf = pdfium
        .load_pdf_from_byte_vec(bytes, None)
        .map_err(|e| format!("cannot open the PDF: {e}"))?;
    let mut pages: Vec<usize> = boxes.iter().map(|b| b.page).collect();
    pages.sort_unstable();
    pages.dedup();

    let mut layers: Vec<(u32, OcrPage)> = Vec::new();
    for &p in &pages {
        let (Some(g), Some(&id)) = (geoms.get(p), page_ids.get(p)) else {
            return Err(format!("page {} is not in the PDF", p + 1));
        };
        let mine: Vec<[f32; 4]> = boxes
            .iter()
            .filter(|b| b.page == p)
            .map(|b| pad(b.rect, 1.0))
            .collect();
        layers.push(crate::raster::rebuild_page(
            &pdf, &mut doc, g, id, p, &mine,
        )?);
    }
    drop(pdf);

    // Metadata goes; unreferenced objects (old content, images, annotations) are dropped.
    doc.trailer.remove(b"Info");
    if let Ok(cat) = doc.catalog_mut() {
        cat.remove(b"Metadata");
    }
    doc.prune_objects();
    let tmp = out.with_extension("redact.tmp");
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes)
        .map_err(|e| format!("cannot write the PDF: {e}"))?;
    std::fs::write(&tmp, bytes).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    let words: Vec<PageWords> = layers
        .iter()
        .map(|(page, ocr)| PageWords { page: *page, ocr })
        .collect();
    let r = layer::add_text_layer(&tmp, out, &words);
    let _ = std::fs::remove_file(&tmp);
    r?;
    Ok(pages)
}

fn pad(r: [f32; 4], d: f32) -> [f32; 4] {
    [r[0] - d, r[1] - d, r[2] + d, r[3] + d]
}

/// Where a redacted value was still found.
pub fn verify(pdfium: &Pdfium, path: &Path, values: &[String]) -> Result<(), String> {
    if values.is_empty() {
        return Ok(());
    }
    let found = |what: &str, text: &str| {
        values
            .iter()
            .find(|v| crate::pii::still_contains(text, v))
            .map(|v| format!("{} still in {what}", crate::pii::mask(v)))
    };
    // Every page's text as a reader would extract it.
    let doc = pdfium
        .load_pdf_from_file(path, None)
        .map_err(|e| format!("cannot open the redacted PDF: {e}"))?;
    for (i, p) in doc.pages().iter().enumerate() {
        let t = p.text().map(|t| t.all()).unwrap_or_default();
        if let Some(e) = found(&format!("the text of page {}", i + 1), &t) {
            return Err(e);
        }
    }
    drop(doc);
    // Every string (annotation contents, bookmarks, form values, info) and every non-image
    // stream in the file.
    let d = Document::load(path).map_err(|e| format!("cannot read the redacted PDF: {e}"))?;
    for (id, obj) in &d.objects {
        let mut strings = Vec::new();
        collect_strings(obj, &mut strings);
        for s in strings {
            if let Some(e) = found(&format!("object {} {}", id.0, id.1), &s) {
                return Err(e);
            }
        }
        if let Object::Stream(st) = obj {
            let image = st
                .dict
                .get(b"Subtype")
                .ok()
                .and_then(|o| o.as_name().ok())
                .is_some_and(|n| n == b"Image");
            if image {
                continue;
            }
            let data = st
                .decompressed_content()
                .unwrap_or_else(|_| st.content.clone());
            let text = String::from_utf8_lossy(&data);
            for v in values {
                if text.contains(v.as_str()) {
                    return Err(format!(
                        "{} still in stream {} {}",
                        crate::pii::mask(v),
                        id.0,
                        id.1
                    ));
                }
            }
        }
    }
    let raw = std::fs::read(path).map_err(|e| e.to_string())?;
    let raw = String::from_utf8_lossy(&raw);
    for v in values {
        if raw.contains(v.as_str()) {
            return Err(format!("{} still in the file", crate::pii::mask(v)));
        }
    }
    Ok(())
}

fn collect_strings(o: &Object, out: &mut Vec<String>) {
    match o {
        Object::String(b, _) => out.push(decode_text(b)),
        Object::Array(a) => a.iter().for_each(|x| collect_strings(x, out)),
        Object::Dictionary(d) => d.iter().for_each(|(_, x)| collect_strings(x, out)),
        Object::Stream(s) => s.dict.iter().for_each(|(_, x)| collect_strings(x, out)),
        _ => {}
    }
}

/// PDF text string: UTF-16BE with BOM, else bytes as Latin-1.
fn decode_text(b: &[u8]) -> String {
    if b.starts_with(&[0xFE, 0xFF]) {
        let u: Vec<u16> = b[2..]
            .chunks(2)
            .map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]))
            .collect();
        String::from_utf16_lossy(&u)
    } else {
        b.iter().map(|&c| c as char).collect()
    }
}

/// Every page's PII matches of `kinds` (pdfium text, including an OCR text layer).
pub fn find(
    pdfium: &Pdfium,
    path: &Path,
    kinds: &[crate::pii::Kind],
) -> Result<Vec<crate::pii::Found>, String> {
    let doc = pdfium
        .load_pdf_from_byte_vec(
            std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?,
            None,
        )
        .map_err(|e| format!("cannot open the PDF: {e}"))?;
    let mut out = Vec::new();
    for (i, p) in doc.pages().iter().enumerate() {
        out.extend(crate::pii::find_on_page(i, &text::chars(&p), kinds));
    }
    Ok(out)
}
