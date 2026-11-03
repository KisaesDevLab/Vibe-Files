//! A page rebuilt as one image (redaction, PDF/A for pages whose fonts are not embedded):
//! rendered at 300 dpi with its annotations, optional boxes burnt in black, and the words left
//! outside the boxes returned for the invisible text layer.

use image::{Rgb, RgbImage};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};
use pdfium_render::prelude::{PdfDocument, PdfRenderConfig};

use crate::layer::Geometry;
use crate::text;
use crate::words::{OcrPage, Word};

/// Resolution pages are rebuilt at.
pub const DPI: f32 = 300.0;

/// Rebuilds page `p` (lopdf object `id`, geometry `g`) of `doc` as an image rendered from
/// `pdf` (the same document, as pdfium loaded it), with `black` boxes (user space) burnt in.
/// Returns the page's words outside the boxes, for [`crate::layer::add_text_layer`].
pub fn rebuild_page(
    pdf: &PdfDocument<'_>,
    doc: &mut Document,
    g: &Geometry,
    id: ObjectId,
    p: usize,
    black: &[[f32; 4]],
) -> Result<(u32, OcrPage), String> {
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
    for r in black {
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
        .filter(|wd| !black.iter().any(|r| text::overlaps(*r, wd.rect)))
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
    replace_page(doc, id, &img, dw, dh)?;
    Ok((
        p as u32,
        OcrPage {
            width: iw,
            height: ih,
            words: survivors,
        },
    ))
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
