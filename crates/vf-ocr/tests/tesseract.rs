//! The real Tesseract on a scanned page (an image-only PDF), and the text layer written back
//! where the words are printed, upright and on a page with `/Rotate 90`.
//!
//! Needs `tesseract.exe` (Q-25). Set `VF_SKIP_TESSERACT=1` on a machine without it.

use std::path::Path;

use pdf_writer::{Content, Name, Pdf, Rect, Ref, Str};
use pdfium_render::prelude::{PdfRect, PdfRenderConfig, Pdfium};
use vf_ocr::layer::{self, PageWords};
use vf_ocr::{OcrEngine, Tesseract};

fn pdfium() -> &'static Pdfium {
    let dir = filer_pdf::pdf::find_pdfium(Path::new("")).expect("pdfium");
    filer_pdf::pdf::pdfium(&dir).unwrap()
}

fn engine() -> Option<Tesseract> {
    match Tesseract::locate(None) {
        Ok(t) => Some(t),
        Err(_) if std::env::var_os("VF_SKIP_TESSERACT").is_some() => None,
        Err(e) => panic!("{e} (set VF_SKIP_TESSERACT=1 to skip)"),
    }
}

/// A Letter page printing `EIN 12-3456789` at (72, 600), 24 pt, and a name below it.
fn printed(path: &Path) {
    let mut pdf = Pdf::new();
    let (catalog, tree, page, content, font) = (
        Ref::new(1),
        Ref::new(2),
        Ref::new(3),
        Ref::new(4),
        Ref::new(5),
    );
    pdf.catalog(catalog).pages(tree);
    pdf.pages(tree).kids([page]).count(1);
    pdf.type1_font(font).base_font(Name(b"Helvetica"));
    let mut c = Content::new();
    c.begin_text();
    c.set_font(Name(b"F1"), 24.0);
    c.next_line(72.0, 600.0);
    c.show(Str(b"EIN 12-3456789"));
    c.next_line(0.0, -60.0);
    c.show(Str(b"Anderson Farms LLC"));
    c.end_text();
    pdf.stream(content, &c.finish());
    let mut p = pdf.page(page);
    p.media_box(Rect::new(0.0, 0.0, 612.0, 792.0));
    p.parent(tree);
    p.contents(content);
    p.resources().fonts().pair(Name(b"F1"), font);
    drop(p);
    std::fs::write(path, pdf.finish()).unwrap();
}

/// The printed page "scanned": rendered at 300 dpi and saved as an image-only PDF. With
/// `rotate` 90 the image is stored turned a quarter left and `/Rotate 90` shows it upright (as
/// scanners write a page fed sideways).
fn scanned(dir: &Path, rotate: i64) -> std::path::PathBuf {
    let src = dir.join("printed.pdf");
    printed(&src);
    let doc = pdfium().load_pdf_from_file(&src, None).unwrap();
    let img = doc
        .pages()
        .first()
        .unwrap()
        .render_with_config(&PdfRenderConfig::new().set_target_width(2550))
        .unwrap()
        .as_image()
        .unwrap()
        .to_rgb8();
    drop(doc);
    let img = if rotate == 90 {
        image::imageops::rotate270(&img)
    } else {
        img
    };
    let out = dir.join(format!("scan{rotate}.pdf"));
    vf_ingest::images::frames_to_pdf(
        vec![vf_ingest::images::Frame {
            rgb: img,
            dpi: Some(300.0),
            orientation: 1,
        }],
        false,
        &out,
    )
    .unwrap();
    if rotate != 0 {
        let mut d = lopdf::Document::load(&out).unwrap();
        let id = *d.get_pages().values().next().unwrap();
        d.get_dictionary_mut(id).unwrap().set("Rotate", rotate);
        d.save(&out).unwrap();
    }
    out
}

/// Renders page 0 the way the queue does (300 dpi, gray) and reads it.
fn read(engine: &Tesseract, pdf: &Path) -> vf_ocr::OcrPage {
    let doc = pdfium().load_pdf_from_file(pdf, None).unwrap();
    let page = doc.pages().first().unwrap();
    assert!(page.text().unwrap().all().trim().is_empty(), "image only");
    let w = page.width().value * 300.0 / 72.0;
    let img = page
        .render_with_config(&PdfRenderConfig::new().set_target_width(w as i32))
        .unwrap()
        .as_image()
        .unwrap()
        .to_luma8();
    engine.recognize(&img, 300).unwrap()
}

fn check(rotate: i64) {
    let Some(engine) = engine() else { return };
    let t = tempfile::tempdir().unwrap();
    let scan = scanned(t.path(), rotate);
    let ocr = read(&engine, &scan);
    let text = ocr.text();
    assert!(text.contains("12-3456789"), "{text}");
    assert!(text.contains("Anderson Farms"), "{text}");

    let out = t.path().join("searchable.pdf");
    layer::add_text_layer(&scan, &out, &[PageWords { page: 0, ocr: &ocr }]).unwrap();
    let doc = pdfium().load_pdf_from_file(&out, None).unwrap();
    let page = doc.pages().first().unwrap();
    let all = page.text().unwrap().all();
    assert!(all.contains("12-3456789"), "{all}");
    // Where it is printed: on the displayed page x 72–260 pt from the left, 172–192 pt from
    // the top. In user space that is y 600–620 upright; turned, x 172–192 and y 72–260.
    let rect = if rotate == 90 {
        PdfRect::new_from_values(60.0, 160.0, 330.0, 205.0)
    } else {
        PdfRect::new_from_values(585.0, 60.0, 640.0, 330.0)
    };
    let at = page.text().unwrap().inside_rect(rect);
    assert!(at.contains("12-3456789"), "rotate {rotate}: found {at:?}");
    assert!(!at.contains("Anderson"), "rotate {rotate}: {at:?}");
    // Looks the same: the layer draws nothing.
    let before = render(&scan);
    let after = render(&out);
    assert_eq!(before, after, "the text layer is invisible");
}

fn render(pdf: &Path) -> Vec<u8> {
    let doc = pdfium().load_pdf_from_file(pdf, None).unwrap();
    doc.pages()
        .first()
        .unwrap()
        .render_with_config(&PdfRenderConfig::new().set_target_width(600))
        .unwrap()
        .as_image()
        .unwrap()
        .to_luma8()
        .into_raw()
}

#[test]
fn scanned_page_gets_an_invisible_layer_where_the_words_are() {
    check(0);
}

#[test]
fn rotated_scan_maps_words_back_through_rotate() {
    check(90);
}
