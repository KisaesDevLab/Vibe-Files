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
        .map(|pg| vf_pdf::text::page_text(&pg))
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

/// Step 7: a 400 dpi scan is brought down to 200 dpi and gets smaller; it still renders.
#[test]
fn compress_downsamples_a_high_resolution_scan() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("scan.pdf");
    // 400 dpi on Letter: 3400 x 4400, with some structure.
    let img = image::RgbImage::from_fn(3400, 4400, |x, y| {
        let v = if (x / 60 + y / 37) % 5 == 0 { 40 } else { 235 };
        image::Rgb([v, v, (v as u16 * 9 / 10) as u8])
    });
    vf_ingest::images::frames_to_pdf(
        vec![vf_ingest::images::Frame {
            rgb: img,
            dpi: Some(400.0),
            orientation: 1,
        }],
        false,
        &src,
    )
    .unwrap();
    let out = t.path().join("small.pdf");
    let r = vf_pdf::compress::compress(&src, &out, 200.0, 70).unwrap();
    assert_eq!(r.images, 1);
    assert!(r.after < r.before, "{r:?}");
    let d = lopdf::Document::load(&out).unwrap();
    let w = d
        .objects
        .values()
        .find_map(|o| match o {
            lopdf::Object::Stream(s)
                if s.dict.get(b"Subtype").and_then(|n| n.as_name()).ok() == Some(b"Image") =>
            {
                s.dict.get(b"Width").and_then(|w| w.as_i64()).ok()
            }
            _ => None,
        })
        .unwrap();
    assert!((1600..=2200).contains(&w), "width {w}");
    assert_eq!(
        pdfium()
            .load_pdf_from_file(&out, None)
            .unwrap()
            .pages()
            .len(),
        1
    );
}

/// PHASES P04 done-when (optional outside step): veraPDF accepts the conversions as PDF/A-2b,
/// the fixture set included.
/// Runs when `VERAPDF` names `verapdf.bat` (veraPDF needs Java: set `JAVA_HOME` too);
/// otherwise says so and passes.
#[test]
fn verapdf_accepts_the_conversions_when_available() {
    let Some(cli) = std::env::var_os("VERAPDF") else {
        eprintln!("VERAPDF not set: veraPDF check skipped");
        return;
    };
    let t = tempfile::tempdir().unwrap();
    let mut outs = Vec::new();
    // A text PDF with a stamp (fonts not embedded: rebuilt), a scan, a scan with an OCR layer.
    let text = t.path().join("text.pdf");
    filer_pdf::pdf::write_test_pdf(&text, &["Engagement letter page one", "Page two"]);
    let stamped = t.path().join("stamped.pdf");
    vf_pdf::annot::add(
        &text,
        &stamped,
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
    let scan = t.path().join("scan.pdf");
    vf_fixtures::write_scanned_pdf(pdfium(), &scan, &[vec!["Scanned page".into()]]).unwrap();
    let layered = t.path().join("layered.pdf");
    let ocr = vf_pdf::words::OcrPage {
        width: 1700,
        height: 2200,
        words: vec![vf_pdf::words::Word {
            bbox: [200, 300, 400, 40],
            text: "Scanned".into(),
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
    let mut sources: Vec<std::path::PathBuf> = vec![stamped, scan, layered];
    // And the whole fixture cabinet (text documents and image-only scans).
    let cab = t.path().join("cab");
    let mut fx = vf_fixtures::generate(&cab, 9).unwrap();
    vf_fixtures::add_scans(&cab, &mut fx, pdfium(), 9).unwrap();
    sources.extend(fx.docs.iter().map(|d| cab.join(&d.rel_path)));
    sources.extend(fx.scans.iter().map(|s| cab.join(&s.rel_path)));
    for (i, src) in sources.iter().enumerate() {
        let out = t.path().join(format!("pdfa-{i}.pdf"));
        pdfa::convert(pdfium(), src, &out, "Test", "2026-11-03T10:00:00Z").unwrap();
        outs.push(out);
    }
    assert!(outs.len() > 60);
    let out = std::process::Command::new(cli)
        .args(["--flavour", "2b", "--format", "text"])
        .args(&outs)
        .output()
        .expect("run veraPDF");
    let report = String::from_utf8_lossy(&out.stdout);
    eprintln!("{report}");
    for o in &outs {
        let name = o.file_name().unwrap().to_string_lossy();
        assert!(
            report.lines().any(|l| l.starts_with("PASS")
                && l.split_whitespace()
                    .nth(1)
                    .is_some_and(|p| p.ends_with(name.as_ref()))),
            "veraPDF did not pass {name}:\n{report}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
