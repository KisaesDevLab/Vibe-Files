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

use image::{Rgb, RgbImage};
use lopdf::{Dictionary, Document, Object, Stream};
use pdfium_render::prelude::{PdfRenderConfig, Pdfium};

use crate::layer::{self, Geometry, PageWords};
use crate::text;
use crate::words::{OcrPage, Word};

/// Resolution redacted pages are rebuilt at.
pub const DPI: f32 = 300.0;

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
        let page = pdf
            .pages()
            .get(p as i32)
            .map_err(|e| format!("page {}: {e}", p + 1))?;
        let (dw, dh) = g.display();
        let s = DPI / 72.0;
        let (w, h) = ((dw * s).round() as u32, (dh * s).round() as u32);
        let mut img = page
            .render_with_config(
                &PdfRenderConfig::new()
                    .set_target_width(w as i32)
                    .set_maximum_height(h as i32)
                    .render_annotations(true),
            )
            .and_then(|b| b.as_image())
            .map_err(|e| format!("cannot render page {}: {e}", p + 1))?
            .to_rgb8();
        let (iw, ih) = img.dimensions();
        let (sx, sy) = (iw as f32 / dw, ih as f32 / dh);
        for r in &mine {
            let [x0, y0, x1, y1] = to_pixels(g, *r, sx, sy, iw, ih);
            for y in y0..y1 {
                for x in x0..x1 {
                    img.put_pixel(x, y, Rgb([0, 0, 0]));
                }
            }
        }
        // The words outside every box stay searchable.
        let survivors: Vec<Word> = text::words(&page)
            .into_iter()
            .filter(|wd| !mine.iter().any(|r| text::overlaps(*r, wd.rect)))
            .enumerate()
            .map(|(i, wd)| {
                let [x0, y0, x1, y1] = to_pixels(g, wd.rect, sx, sy, iw, ih);
                Word {
                    bbox: [x0, y0, x1 - x0, y1 - y0],
                    text: wd.text,
                    conf: 100.0,
                    line: i as u32,
                }
            })
            .collect();
        layers.push((
            p as u32,
            OcrPage {
                width: iw,
                height: ih,
                words: survivors,
            },
        ));
        replace_page(&mut doc, id, &img, dw, dh)?;
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

/// User-space box → pixel box `[x0, y0, x1, y1)` on the page image (display orientation).
fn to_pixels(g: &Geometry, r: [f32; 4], sx: f32, sy: f32, iw: u32, ih: u32) -> [u32; 4] {
    let a = g.to_display(r[0], r[1]);
    let b = g.to_display(r[2], r[3]);
    let (u0, u1) = (a[0].min(b[0]), a[0].max(b[0]));
    let (v0, v1) = (a[1].min(b[1]), a[1].max(b[1]));
    [
        ((u0 * sx).floor().max(0.0) as u32).min(iw),
        ((v0 * sy).floor().max(0.0) as u32).min(ih),
        ((u1 * sx).ceil().max(0.0) as u32).min(iw),
        ((v1 * sy).ceil().max(0.0) as u32).min(ih),
    ]
}

/// Makes page `id` one image, `dw` × `dh` points, upright (no `/Rotate`, no CropBox), with no
/// annotations and no other content.
fn replace_page(
    doc: &mut Document,
    id: lopdf::ObjectId,
    img: &RgbImage,
    dw: f32,
    dh: f32,
) -> Result<(), String> {
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 88)
        .encode_image(img)
        .map_err(|e| format!("cannot encode the page: {e}"))?;
    let xobj = doc.add_object(Stream::new(
        Dictionary::from_iter(vec![
            ("Type", Object::Name(b"XObject".to_vec())),
            ("Subtype", Object::Name(b"Image".to_vec())),
            ("Width", (img.width() as i64).into()),
            ("Height", (img.height() as i64).into()),
            ("ColorSpace", Object::Name(b"DeviceRGB".to_vec())),
            ("BitsPerComponent", 8.into()),
            ("Filter", Object::Name(b"DCTDecode".to_vec())),
        ]),
        jpeg,
    ));
    let content = doc.add_object(Stream::new(
        Dictionary::new(),
        format!("q {dw} 0 0 {dh} 0 0 cm /VfPage Do Q\n").into_bytes(),
    ));
    let page = doc.get_dictionary_mut(id).map_err(|e| e.to_string())?;
    page.set(
        "MediaBox",
        Object::Array(vec![0.into(), 0.into(), dw.into(), dh.into()]),
    );
    page.remove(b"CropBox");
    page.remove(b"TrimBox");
    page.remove(b"BleedBox");
    page.remove(b"ArtBox");
    page.set("Rotate", 0);
    page.remove(b"Annots");
    page.remove(b"Thumb");
    page.set("Contents", Object::Reference(content));
    page.set(
        "Resources",
        Dictionary::from_iter(vec![(
            "XObject",
            Object::Dictionary(Dictionary::from_iter(vec![(
                "VfPage",
                Object::Reference(xobj),
            )])),
        )]),
    );
    Ok(())
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
