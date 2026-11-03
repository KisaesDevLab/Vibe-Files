//! Photos and image scans in the Scans folder become PDFs before Filer sees them.

use std::path::{Path, PathBuf};

use image::RgbImage;
use vf_ingest::images;
use vf_ingest::pipeline::{self, Context, Origin};

struct Fx {
    _t: tempfile::TempDir,
    root: PathBuf,
    inbox: PathBuf,
    staging: PathBuf,
}

fn fx() -> Fx {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("cabinet");
    std::fs::create_dir_all(vf_fs::clients::master_dir(&root)).unwrap();
    let (inbox, staging) = (t.path().join("inbox"), t.path().join("scans"));
    std::fs::create_dir_all(&inbox).unwrap();
    std::fs::create_dir_all(&staging).unwrap();
    Fx {
        _t: t,
        root,
        inbox,
        staging,
    }
}

fn photo(w: u32, h: u32) -> RgbImage {
    RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([
            (x % 256) as u8,
            (y % 256) as u8,
            if y < h / 2 { 200 } else { 30 },
        ])
    })
}

fn process(f: &Fx, staged: &Path) -> pipeline::Processed {
    let dir = filer_pdf::pdf::find_pdfium(Path::new("")).expect("pdfium");
    let ctx = Context {
        root: &f.root,
        inbox: &f.inbox,
        device_id: "dev",
        pdfium: Some(filer_pdf::pdf::pdfium(&dir).unwrap()),
        ocr: None,
    };
    pipeline::process(&ctx, staged, Origin::Scans).unwrap()
}

/// Page sizes (points) of a PDF.
fn page_sizes(p: &Path) -> Vec<(f32, f32)> {
    let doc = lopdf::Document::load(p).unwrap();
    doc.get_pages()
        .values()
        .map(|id| {
            let mb = doc
                .get_dictionary(*id)
                .unwrap()
                .get(b"MediaBox")
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .map(|o| o.as_float().unwrap())
                .collect::<Vec<_>>();
            (mb[2], mb[3])
        })
        .collect()
}

/// PHASES P02 done-when: a HEIC phone photo becomes a 1-page PDF named so Filer routes it.
#[test]
fn heic_photo_becomes_a_one_page_pdf() {
    let f = fx();
    let heic = f.staging.join("Acme LLC_1001_2025 W-2.heic");
    if let Err(e) = images::write_with_wic(&heic, "heif", &[photo(1200, 900)], None) {
        // The HEIF encoder comes with the Store extension; this PC must have it to decode too.
        panic!("this PC cannot write HEIC (HEIF Image Extensions missing?): {e}");
    }
    let done = process(&f, &heic);
    assert_eq!(done.delivered.len(), 1);
    let pdf = &done.delivered[0].path;
    assert_eq!(pdf, &f.inbox.join("Acme LLC_1001_2025 W-2.pdf"));
    assert_eq!(page_sizes(pdf), vec![(792.0, 612.0)], "landscape Letter");
    assert_eq!(done.delivered[0].note, "1 page");
    let re = filer_core::parse::filename_regex(filer_core::parse::DEFAULT_ID_PATTERN).unwrap();
    let parsed = filer_core::parse::parse_filename("Acme LLC_1001_2025 W-2.pdf", &re, 2026);
    assert_eq!(parsed.id.as_deref(), Some("1001"));
    assert!(done.kept.is_file() && !heic.exists());
}

#[test]
fn jpeg_is_turned_upright_from_exif() {
    let f = fx();
    let jpg = f.staging.join("receipt.jpg");
    // Stored landscape, tagged "rotate 90° clockwise to view": the page is portrait.
    images::write_with_wic(&jpg, "jpeg", &[photo(400, 300)], Some(6)).unwrap();
    let frames = images::decode(&jpg).unwrap();
    assert_eq!(frames[0].orientation, 6);
    assert_eq!(frames[0].rgb.dimensions(), (300, 400));
    let done = process(&f, &jpg);
    assert_eq!(page_sizes(&done.delivered[0].path), vec![(612.0, 792.0)]);
    assert_eq!(done.delivered[0].note, "1 page · EXIF rotate");
}

#[test]
fn multi_page_tiff_gives_a_page_per_frame() {
    let f = fx();
    let tif = f.staging.join("fax.tif");
    images::write_with_wic(
        &tif,
        "tiff",
        &[photo(850, 1100), photo(850, 1100), photo(850, 1100)],
        None,
    )
    .unwrap();
    let done = process(&f, &tif);
    assert_eq!(done.delivered[0].path, f.inbox.join("fax.pdf"));
    assert_eq!(page_sizes(&done.delivered[0].path).len(), 3);
    assert!(done.delivered[0].note.starts_with("3 pages"));
}

#[test]
fn unreadable_image_is_an_error_and_nothing_is_delivered() {
    let f = fx();
    let bad = f.staging.join("broken.heic");
    std::fs::write(&bad, b"not an image").unwrap();
    let dir = filer_pdf::pdf::find_pdfium(Path::new("")).expect("pdfium");
    let ctx = Context {
        root: &f.root,
        inbox: &f.inbox,
        device_id: "dev",
        pdfium: Some(filer_pdf::pdf::pdfium(&dir).unwrap()),
        ocr: None,
    };
    let e = pipeline::process(&ctx, &bad, Origin::Scans).unwrap_err();
    assert!(e.contains("broken.heic"), "{e}");
    assert!(bad.exists(), "left for the worker to set aside");
    assert_eq!(
        std::fs::read_dir(&f.inbox).unwrap().count(),
        0,
        "no temp files left"
    );
}
