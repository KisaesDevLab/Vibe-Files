//! PDF/A-2b conversion passes this tool's own checks (QUESTIONS Q-28; veraPDF is the
//! optional outside check).

use std::path::Path;

use pdfium_render::prelude::Pdfium;
use vf_pdf::pdfa;

fn pdfium() -> &'static Pdfium {
    let dir = filer_pdf::pdf::find_pdfium(Path::new("")).expect("pdfium");
    filer_pdf::pdf::pdfium(&dir).unwrap()
}

fn text_of(p: &Path) -> String {
    let doc = pdfium().load_pdf_from_file(p, None).unwrap();
    doc.pages()
        .iter()
        .map(|pg| pg.text().unwrap().all())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Fonts not embedded (Helvetica) and an annotation: pages are rebuilt with a text layer, the
/// note is flattened, and the result passes.
#[test]
fn text_pdf_with_a_note_converts_and_stays_searchable() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("src.pdf");
    filer_pdf::pdf::write_test_pdf(
        &src,
        &["Engagement letter page one", "Page two of the letter"],
    );
    let noted = t.path().join("noted.pdf");
    vf_pdf::annot::add(
        &src,
        &noted,
        &[vf_pdf::annot::Annotation {
            page: 0,
            kind: vf_pdf::annot::Kind::Stamp {
                text: "FILED 11/03/2026".into(),
                at: [300.0, 400.0],
                size: 16.0,
            },
            color: vf_pdf::annot::ACCENT,
            author: "KWK".into(),
        }],
        "2026-11-03T10:00:00Z",
    )
    .unwrap();
    let before = pdfa::check(&noted).unwrap();
    assert!(
        before.iter().any(|i| i.contains("font is not embedded")),
        "{before:?}"
    );
    assert!(before.iter().any(|i| i.contains("XMP")), "{before:?}");

    let out = t.path().join("pdfa.pdf");
    let r = pdfa::convert(
        pdfium(),
        &noted,
        &out,
        "Engagement letter",
        "2026-11-03T10:00:00Z",
    )
    .unwrap();
    assert_eq!(r.rasterized, [0, 1]);
    assert_eq!(r.flattened, 1);
    assert_eq!(pdfa::check(&out).unwrap(), Vec::<String>::new());
    let text = text_of(&out);
    assert!(
        text.contains("Engagement letter page one") && text.contains("Page two"),
        "{text}"
    );
    assert!(vf_pdf::annot::read(&out).unwrap().is_empty());
    let raw = std::fs::read(&out).unwrap();
    let raw = String::from_utf8_lossy(&raw);
    assert!(raw.contains("<pdfaid:part>2</pdfaid:part>"));
}

/// A pure scan (images only) needs no rebuilding: same pages, now PDF/A.
#[test]
fn scan_converts_without_rasterizing() {
    let t = tempfile::tempdir().unwrap();
    let scan = t.path().join("scan.pdf");
    vf_fixtures::write_scanned_pdf(
        pdfium(),
        &scan,
        &[
            vec!["Scanned page".into()],
            vec!["Second scanned page".into()],
        ],
    )
    .unwrap();
    let out = t.path().join("pdfa.pdf");
    let r = pdfa::convert(pdfium(), &scan, &out, "Scan", "2026-11-03T10:00:00Z").unwrap();
    assert!(r.rasterized.is_empty());
    assert_eq!(pdfa::check(&out).unwrap(), Vec::<String>::new());
    assert_eq!(
        pdfium()
            .load_pdf_from_file(&out, None)
            .unwrap()
            .pages()
            .len(),
        2
    );
}

/// A scan with the OCR text layer: the layer's font is used only invisibly, so the page is
/// not rebuilt.
#[test]
fn ocr_layer_font_is_exempt() {
    let t = tempfile::tempdir().unwrap();
    let scan = t.path().join("scan.pdf");
    vf_fixtures::write_scanned_pdf(pdfium(), &scan, &[vec!["Payer EIN 07-1234567".into()]])
        .unwrap();
    let layered = t.path().join("layered.pdf");
    let ocr = vf_pdf::words::OcrPage {
        width: 1700,
        height: 2200,
        words: vec![vf_pdf::words::Word {
            bbox: [200, 300, 400, 40],
            text: "07-1234567".into(),
            conf: 95.0,
            line: 0,
        }],
    };
    vf_pdf::layer::add_text_layer(
        &scan,
        &layered,
        &[vf_pdf::layer::PageWords { page: 0, ocr: &ocr }],
    )
    .unwrap();
    let out = t.path().join("pdfa.pdf");
    let r = pdfa::convert(pdfium(), &layered, &out, "Scan", "2026-11-03T10:00:00Z").unwrap();
    assert!(r.rasterized.is_empty(), "{r:?}");
    assert_eq!(pdfa::check(&out).unwrap(), Vec::<String>::new());
    assert!(text_of(&out).contains("07-1234567"));
}
