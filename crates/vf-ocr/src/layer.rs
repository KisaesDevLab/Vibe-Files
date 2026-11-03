//! The invisible text layer (P03 step 3): OCR words drawn in text render mode 3 (neither fill
//! nor stroke) over the page image, so the PDF looks the same but its text can be searched,
//! selected and copied in any PDF app.
//!
//! Words are set in Courier (every glyph 600/1000 em wide), stretched horizontally (`Tz`) to
//! the width OCR measured, so selection boxes match the printed words. Box pixels map back to
//! PDF user space through the page's CropBox and `/Rotate` (pdfium renders the rotated page).

use std::path::Path;

use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, StringFormat};

use crate::engine::OcrPage;

/// Resource name of the layer's font.
const FONT: &[u8] = b"VfOcrCourier";
/// Courier advance width, em.
const ADVANCE: f32 = 0.6;

/// OCR words for one page (0-based).
pub struct PageWords<'a> {
    pub page: u32,
    pub ocr: &'a OcrPage,
}

/// Writes `src` plus a text layer on `pages` to `out`.
pub fn add_text_layer(src: &Path, out: &Path, pages: &[PageWords<'_>]) -> Result<(), String> {
    let mut doc = Document::load(src).map_err(|e| format!("cannot read the PDF: {e}"))?;
    if doc.is_encrypted() {
        return Err("the PDF is password protected".into());
    }
    let ids: Vec<ObjectId> = doc.get_pages().values().copied().collect();
    let font = doc.add_object(Dictionary::from_iter(vec![
        ("Type", Object::Name(b"Font".to_vec())),
        ("Subtype", Object::Name(b"Type1".to_vec())),
        ("BaseFont", Object::Name(b"Courier".to_vec())),
        ("Encoding", Object::Name(b"WinAnsiEncoding".to_vec())),
    ]));
    for pw in pages {
        let Some(&page_id) = ids.get(pw.page as usize) else {
            return Err(format!("page {} is not in the PDF", pw.page + 1));
        };
        let geo = Geometry::of(&doc, page_id)?;
        let ops = text_ops(&geo, pw.ocr);
        if ops.is_empty() {
            continue;
        }
        add_font(&mut doc, page_id, font)?;
        wrap_contents(&mut doc, page_id, ops)?;
    }
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes)
        .map_err(|e| format!("cannot write the PDF: {e}"))?;
    std::fs::write(out, bytes).map_err(|e| format!("cannot write {}: {e}", out.display()))
}

/// Where a page's displayed image sits in its user space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    /// CropBox (else MediaBox): x0, y0, x1, y1.
    pub cbox: [f32; 4],
    /// 0, 90, 180 or 270.
    pub rotate: i64,
}

impl Geometry {
    fn of(doc: &Document, page: ObjectId) -> Result<Geometry, String> {
        let rect = |key: &[u8]| -> Option<[f32; 4]> {
            let arr = inherited(doc, page, key)?;
            let arr = doc.dereference(&arr).ok()?.1.as_array().ok()?.clone();
            let v: Vec<f32> = arr
                .iter()
                .filter_map(|o| doc.dereference(o).ok().and_then(|(_, o)| o.as_float().ok()))
                .collect();
            (v.len() == 4).then(|| {
                [
                    v[0].min(v[2]),
                    v[1].min(v[3]),
                    v[0].max(v[2]),
                    v[1].max(v[3]),
                ]
            })
        };
        let cbox = rect(b"CropBox")
            .or_else(|| rect(b"MediaBox"))
            .ok_or("page has no MediaBox")?;
        let rotate = inherited(doc, page, b"Rotate")
            .and_then(|o| doc.dereference(&o).ok()?.1.as_i64().ok())
            .unwrap_or(0)
            .rem_euclid(360);
        Ok(Geometry {
            cbox,
            rotate: rotate - rotate % 90,
        })
    }

    /// Displayed size in points (width, height), after rotation.
    pub fn display(&self) -> (f32, f32) {
        let (w, h) = (self.cbox[2] - self.cbox[0], self.cbox[3] - self.cbox[1]);
        if self.rotate % 180 == 0 {
            (w, h)
        } else {
            (h, w)
        }
    }

    /// Text matrix for a run starting at display point (u, v) (points, origin top left):
    /// the run reads left to right on the displayed page.
    pub fn text_matrix(&self, u: f32, v: f32) -> [f32; 6] {
        let [x0, y0, x1, y1] = self.cbox;
        match self.rotate {
            90 => [0.0, 1.0, -1.0, 0.0, x0 + v, y0 + u],
            180 => [-1.0, 0.0, 0.0, -1.0, x1 - u, y0 + v],
            270 => [0.0, -1.0, 1.0, 0.0, x1 - v, y1 - u],
            _ => [1.0, 0.0, 0.0, 1.0, x0 + u, y1 - v],
        }
    }
}

/// A page attribute, looked up through the page tree (MediaBox, CropBox, Rotate, Resources
/// are inheritable).
fn inherited(doc: &Document, page: ObjectId, key: &[u8]) -> Option<Object> {
    let mut id = page;
    for _ in 0..32 {
        let dict = doc.get_dictionary(id).ok()?;
        if let Ok(v) = dict.get(key) {
            return Some(v.clone());
        }
        id = dict.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

/// Content operations drawing every word invisibly.
pub fn text_ops(geo: &Geometry, ocr: &OcrPage) -> Vec<Operation> {
    if ocr.width == 0 || ocr.height == 0 {
        return vec![];
    }
    let (dw, dh) = geo.display();
    let (su, sv) = (dw / ocr.width as f32, dh / ocr.height as f32);
    let mut ops = vec![
        Operation::new("BT", vec![]),
        Operation::new("Tr", vec![3.into()]),
    ];
    for w in &ocr.words {
        let bytes = win_ansi(&w.text);
        if bytes.is_empty() {
            continue;
        }
        let [l, t, bw, bh] = w.bbox.map(|v| v as f32);
        let size = (bh * sv).max(1.0);
        let width = bw * su;
        let natural = bytes.len() as f32 * ADVANCE * size;
        let scale = if natural > 0.0 {
            100.0 * width / natural
        } else {
            100.0
        };
        // Baseline near the bottom of the box (Courier descends ~0.16 em).
        let m = geo.text_matrix(l * su, (t + bh) * sv - 0.16 * size);
        ops.push(Operation::new(
            "Tf",
            vec![Object::Name(FONT.to_vec()), size.into()],
        ));
        ops.push(Operation::new("Tz", vec![scale.into()]));
        ops.push(Operation::new(
            "Tm",
            m.iter().map(|v| (*v).into()).collect(),
        ));
        ops.push(Operation::new(
            "Tj",
            vec![Object::String(bytes, StringFormat::Literal)],
        ));
    }
    ops.push(Operation::new("ET", vec![]));
    ops
}

/// The word in WinAnsi (Latin-1 range kept, anything else `?`).
fn win_ansi(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| match c as u32 {
            0x20..=0x7e | 0xa0..=0xff => c as u32 as u8,
            0x2018 | 0x2019 => b'\'',
            0x201c | 0x201d => b'"',
            0x2013 | 0x2014 => b'-',
            _ => b'?',
        })
        .collect()
}

/// Adds the layer font to the page's own resources (copying inherited ones down first, so
/// nothing the page used is lost).
fn add_font(doc: &mut Document, page: ObjectId, font: ObjectId) -> Result<(), String> {
    let own = doc
        .get_dictionary(page)
        .map_err(|e| e.to_string())?
        .has(b"Resources");
    if !own {
        let inherited = inherited(doc, page, b"Resources")
            .and_then(|r| doc.dereference(&r).ok().map(|(_, o)| o.clone()))
            .unwrap_or_else(|| Object::Dictionary(Dictionary::new()));
        doc.get_dictionary_mut(page)
            .map_err(|e| e.to_string())?
            .set("Resources", inherited);
    }
    let res = doc
        .get_or_create_resources(page)
        .and_then(Object::as_dict_mut)
        .map_err(|e| format!("page resources: {e}"))?;
    let fonts_ref = match res.get(b"Font") {
        Ok(Object::Reference(id)) => Some(*id),
        _ => None,
    };
    let fonts = match fonts_ref {
        Some(id) => doc.get_dictionary_mut(id).map_err(|e| e.to_string())?,
        None => {
            let res = doc
                .get_or_create_resources(page)
                .and_then(Object::as_dict_mut)
                .map_err(|e| e.to_string())?;
            if !matches!(res.get(b"Font"), Ok(Object::Dictionary(_))) {
                res.set("Font", Dictionary::new());
            }
            res.get_mut(b"Font")
                .and_then(Object::as_dict_mut)
                .map_err(|e| e.to_string())?
        }
    };
    fonts.set(FONT, Object::Reference(font));
    Ok(())
}

/// `q <existing content> Q <layer>`: whatever state the page's own content leaves behind
/// does not move the layer.
fn wrap_contents(doc: &mut Document, page: ObjectId, ops: Vec<Operation>) -> Result<(), String> {
    let existing: Vec<Object> = match doc
        .get_dictionary(page)
        .map_err(|e| e.to_string())?
        .get(b"Contents")
    {
        Ok(Object::Reference(id)) => vec![Object::Reference(*id)],
        Ok(Object::Array(a)) => a.clone(),
        _ => vec![],
    };
    let push = doc.add_object(Stream::new(Dictionary::new(), b"q\n".to_vec()));
    let mut layer = b"Q\n".to_vec();
    layer.extend(
        Content { operations: ops }
            .encode()
            .map_err(|e| format!("text layer: {e}"))?,
    );
    let layer = doc.add_object(Stream::new(Dictionary::new(), layer));
    let mut contents = vec![Object::Reference(push)];
    contents.extend(existing);
    contents.push(Object::Reference(layer));
    doc.get_dictionary_mut(page)
        .map_err(|e| e.to_string())?
        .set("Contents", contents);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_matrix_follows_rotation() {
        let g = |rotate| Geometry {
            cbox: [0.0, 0.0, 612.0, 792.0],
            rotate,
        };
        assert_eq!(g(0).display(), (612.0, 792.0));
        assert_eq!(g(90).display(), (792.0, 612.0));
        // Display top left = user top left on an upright page.
        assert_eq!(g(0).text_matrix(0.0, 0.0)[4..], [0.0, 792.0]);
        // Rotated 90° clockwise: display top left is the user origin.
        assert_eq!(g(90).text_matrix(0.0, 0.0)[4..], [0.0, 0.0]);
        assert_eq!(g(180).text_matrix(0.0, 0.0)[4..], [612.0, 0.0]);
        assert_eq!(g(270).text_matrix(0.0, 0.0)[4..], [612.0, 792.0]);
    }

    #[test]
    fn win_ansi_keeps_latin_and_replaces_the_rest() {
        assert_eq!(win_ansi("Café 12–34 €"), b"Caf\xe9 12-34 ?".to_vec());
    }
}
