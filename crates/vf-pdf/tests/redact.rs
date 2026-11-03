//! Redaction removes content, not just covers it (P04 step 5 done-when).

use std::path::Path;

use pdfium_render::prelude::{PdfRenderConfig, Pdfium};
use vf_pdf::pii::{self, Kind};
use vf_pdf::redact::{self, Redaction};

fn pdfium() -> &'static Pdfium {
    let dir = filer_pdf::pdf::find_pdfium(Path::new("")).expect("pdfium");
    filer_pdf::pdf::pdfium(&dir).unwrap()
}

fn boxes(found: &[pii::Found]) -> Vec<Redaction> {
    found
        .iter()
        .flat_map(|f| {
            f.rects.iter().map(|r| Redaction {
                page: f.page,
                rect: *r,
            })
        })
        .collect()
}

fn text_of(p: &Path) -> String {
    let doc = pdfium().load_pdf_from_file(p, None).unwrap();
    doc.pages()
        .iter()
        .map(|pg| vf_pdf::text::page_text(&pg))
        .collect::<Vec<_>>()
        .join("\n")
}

/// PHASES P04 done-when (first half): auto-find flags every synthetic SSN in the fixture set.
#[test]
fn auto_find_flags_every_fixture_ssn() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("cab");
    let fx = vf_fixtures::generate(&root, 5).unwrap();
    let mut checked = 0;
    for d in &fx.docs {
        let found = redact::find(pdfium(), &root.join(&d.rel_path), &[Kind::Ssn]).unwrap();
        let values: Vec<&str> = found.iter().map(|f| f.value.as_str()).collect();
        match &d.ssn {
            Some(ssn) => {
                assert_eq!(values, [ssn.as_str()], "{}", d.rel_path);
                checked += 1;
            }
            None => assert!(values.is_empty(), "{}: {values:?}", d.rel_path),
        }
    }
    assert!(
        checked >= 30,
        "only {checked} fixture documents carry an SSN"
    );
}

/// Text PDF: after redaction the SSN is nowhere in the file, the rest is still searchable,
/// and the document info is gone.
#[test]
fn redacted_text_pdf_keeps_the_rest_and_loses_the_ssn() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("w2.pdf");
    vf_fixtures::write_pdf(
        &src,
        &[
            "W-2 - tax year 2025".into(),
            "SYNTHETIC TEST DOCUMENT - NOT REAL DATA".into(),
            "Payer EIN: 07-1234567".into(),
            "Recipient SSN: 987-65-4321".into(),
        ],
    )
    .unwrap();
    let mut d = lopdf::Document::load(&src).unwrap();
    let info = d.add_object(lopdf::Dictionary::from_iter(vec![(
        "Subject",
        lopdf::Object::string_literal("SSN 987-65-4321"),
    )]));
    d.trailer.set("Info", lopdf::Object::Reference(info));
    d.save(&src).unwrap();

    let found = redact::find(pdfium(), &src, &[Kind::Ssn]).unwrap();
    assert_eq!(found.len(), 1);
    let out = t.path().join("w2 (redacted).pdf");
    assert_eq!(
        redact::apply(pdfium(), &src, &out, &boxes(&found)).unwrap(),
        [0]
    );
    redact::verify(pdfium(), &out, &["987-65-4321".into()]).unwrap();
    let text = text_of(&out);
    assert!(!pii::still_contains(&text, "987-65-4321"), "{text}");
    assert!(
        text.contains("SYNTHETIC") && text.contains("07-1234567"),
        "{text}"
    );
    assert!(
        lopdf::Document::load(&out)
            .unwrap()
            .trailer
            .get(b"Info")
            .is_err()
    );
    // The original is untouched.
    assert!(text_of(&src).contains("987-65-4321"));
}

/// Scanned page with an OCR text layer: the scan image itself is replaced (its bytes are not
/// in the output) and the pixels under the box are black.
#[test]
fn scanned_page_loses_its_original_image() {
    let t = tempfile::tempdir().unwrap();
    let printed = t.path().join("printed.pdf");
    let lines = vec![
        "1099-MISC (scanned)".to_string(),
        "Recipient SSN: 987-65-4325".into(),
        "Other income: 1200.00".into(),
    ];
    vf_fixtures::write_pdf(&printed, &lines).unwrap();
    let scan = t.path().join("scan.pdf");
    vf_fixtures::write_scanned_pdf(pdfium(), &scan, std::slice::from_ref(&lines)).unwrap();
    // An OCR-style text layer: the printed words at the same places.
    let words = {
        let doc = pdfium().load_pdf_from_file(&printed, None).unwrap();
        let page = doc.pages().first().unwrap();
        vf_pdf::text::words(&page)
    };
    let (w, h) = (2550u32, 3300u32);
    let ocr = vf_pdf::words::OcrPage {
        width: w,
        height: h,
        words: words
            .iter()
            .enumerate()
            .map(|(i, wd)| {
                let s = w as f32 / 612.0;
                vf_pdf::words::Word {
                    bbox: [
                        (wd.rect[0] * s) as u32,
                        ((792.0 - wd.rect[3]) * s) as u32,
                        ((wd.rect[2] - wd.rect[0]) * s) as u32,
                        ((wd.rect[3] - wd.rect[1]) * s) as u32,
                    ],
                    text: wd.text.clone(),
                    conf: 99.0,
                    line: i as u32,
                }
            })
            .collect(),
    };
    let layered = t.path().join("layered.pdf");
    vf_pdf::layer::add_text_layer(
        &scan,
        &layered,
        &[vf_pdf::layer::PageWords { page: 0, ocr: &ocr }],
    )
    .unwrap();
    let image_bytes: Vec<u8> = {
        let d = lopdf::Document::load(&layered).unwrap();
        d.objects
            .values()
            .find_map(|o| match o {
                lopdf::Object::Stream(s)
                    if s.dict.get(b"Subtype").and_then(|n| n.as_name()).ok() == Some(b"Image") =>
                {
                    Some(s.content.clone())
                }
                _ => None,
            })
            .unwrap()
    };

    let found = redact::find(pdfium(), &layered, &[Kind::Ssn]).unwrap();
    assert_eq!(found.len(), 1, "found through the text layer");
    let out = t.path().join("redacted.pdf");
    redact::apply(pdfium(), &layered, &out, &boxes(&found)).unwrap();
    redact::verify(pdfium(), &out, &["987-65-4325".into()]).unwrap();

    let raw = std::fs::read(&out).unwrap();
    let probe = &image_bytes[image_bytes.len() / 2..image_bytes.len() / 2 + 64];
    assert!(
        !raw.windows(probe.len()).any(|w| w == probe),
        "the original scan's bytes are still in the file"
    );
    assert!(
        text_of(&out).contains("1200.00"),
        "the rest stays searchable"
    );

    // Under the box: black.
    let r = found[0].rects[0];
    let doc = pdfium().load_pdf_from_file(&out, None).unwrap();
    let img = doc
        .pages()
        .first()
        .unwrap()
        .render_with_config(&PdfRenderConfig::new().set_target_width(612))
        .unwrap()
        .as_image()
        .unwrap()
        .to_luma8();
    let (cx, cy) = (
        ((r[0] + r[2]) / 2.0) as u32,
        (792.0 - (r[1] + r[3]) / 2.0) as u32,
    );
    assert!(img.get_pixel(cx, cy).0[0] < 30, "not black under the box");
}

/// An SSN left in a sticky note on another page fails the check: the caller refuses the file.
#[test]
fn verify_refuses_an_ssn_left_in_a_note() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("two.pdf");
    filer_pdf::pdf::write_test_pdf(&src, &["SSN 987-65-4321", "Second page"]);
    let noted = t.path().join("noted.pdf");
    vf_pdf::annot::add(
        &src,
        &noted,
        &[vf_pdf::annot::Annotation {
            page: 1,
            kind: vf_pdf::annot::Kind::Note {
                at: [72.0, 700.0],
                text: "client SSN is 987-65-4321".into(),
            },
            color: vf_pdf::annot::YELLOW,
            author: "KWK".into(),
        }],
        "2026-11-03T10:00:00Z",
    )
    .unwrap();
    let found = redact::find(pdfium(), &noted, &[Kind::Ssn]).unwrap();
    assert_eq!(found.len(), 1, "page text only");
    let out = t.path().join("out.pdf");
    redact::apply(pdfium(), &noted, &out, &boxes(&found)).unwrap();
    let e = redact::verify(pdfium(), &out, &["987-65-4321".into()]).unwrap_err();
    assert!(e.contains("•••-••-4321"), "{e}");
}
