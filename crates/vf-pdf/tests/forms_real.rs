//! Real fillable forms (PA-27 QA): every PDF in `VF_FORMS_DIR` (blank public forms, e.g. IRS
//! PDFs; never client documents) is read, every field filled with a test value, saved, read
//! back, rendered with pdfium and flattened.
//!
//! `VF_FORMS_DIR=<dir> cargo test -p vf-pdf --test forms_real -- --ignored --nocapture`

use std::path::Path;

use pdfium_render::prelude::Pdfium;
use vf_pdf::forms::{self, FieldKind};

fn pdfium() -> &'static Pdfium {
    let dir = filer_pdf::pdf::find_pdfium(Path::new("")).expect("pdfium");
    filer_pdf::pdf::pdfium(&dir).unwrap()
}

#[test]
#[ignore = "needs VF_FORMS_DIR with blank fillable PDFs"]
fn real_forms_fill_round_trip() {
    let dir = std::env::var("VF_FORMS_DIR").expect("VF_FORMS_DIR");
    let out_dir = tempfile::tempdir().unwrap();
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pdf")))
        .collect();
    files.sort();
    assert!(!files.is_empty());
    for src in files {
        let name = src.file_name().unwrap().to_string_lossy().to_string();
        let fields = forms::read(&src).unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut values = Vec::new();
        let (mut text, mut checks, mut radios, mut choices) = (0, 0, 0, 0);
        for (k, f) in fields.iter().enumerate().filter(|(_, f)| !f.read_only) {
            let v = match &f.kind {
                FieldKind::Text { max_len, comb, .. } => {
                    text += 1;
                    let base = if *comb {
                        "1234567890".to_string()
                    } else {
                        format!("Test {k} (a\\b)")
                    };
                    match max_len {
                        Some(m) => base.chars().take(*m).collect(),
                        None => base,
                    }
                }
                FieldKind::Check { on } => {
                    checks += 1;
                    on.clone()
                }
                FieldKind::Radio { options } => {
                    radios += 1;
                    match options.last() {
                        Some(o) => o.clone(),
                        None => continue,
                    }
                }
                FieldKind::Choice { options, .. } => {
                    choices += 1;
                    match options.last() {
                        Some(o) => o.value.clone(),
                        None => continue,
                    }
                }
            };
            values.push((f.name.clone(), v));
        }
        let out = out_dir.path().join(&name);
        let n = forms::fill(&src, &out, &values).unwrap_or_else(|e| panic!("{name}: {e}"));
        let back = forms::read(&out).unwrap();
        for (fname, v) in &values {
            let got = &back.iter().find(|f| &f.name == fname).unwrap().value;
            assert_eq!(got, v, "{name}: {fname}");
        }
        // pdfium opens and renders every page; Flatten keeps no fields.
        let doc = pdfium().load_pdf_from_file(&out, None).unwrap();
        for p in doc.pages().iter() {
            p.render_with_config(
                &pdfium_render::prelude::PdfRenderConfig::new().set_target_width(400),
            )
            .unwrap_or_else(|e| panic!("{name}: render {e}"));
        }
        // `VF_FORMS_PNG=<dir>`: page 1 of the filled form, to look at.
        if let Ok(png) = std::env::var("VF_FORMS_PNG") {
            let img = doc
                .pages()
                .get(0)
                .unwrap()
                .render_with_config(
                    &pdfium_render::prelude::PdfRenderConfig::new().set_target_width(1200),
                )
                .unwrap()
                .as_image()
                .unwrap();
            img.save(Path::new(&png).join(format!("{name}.png")))
                .unwrap();
        }
        let flat = out_dir.path().join(format!("flat-{name}"));
        vf_pdf::flatten::flatten(&out, &flat).unwrap();
        assert!(forms::read(&flat).unwrap().is_empty(), "{name}: flattened");
        if let Ok(png) = std::env::var("VF_FORMS_PNG") {
            let d = pdfium().load_pdf_from_file(&flat, None).unwrap();
            let img = d
                .pages()
                .get(0)
                .unwrap()
                .render_with_config(
                    &pdfium_render::prelude::PdfRenderConfig::new().set_target_width(1200),
                )
                .unwrap()
                .as_image()
                .unwrap();
            img.save(Path::new(&png).join(format!("flat-{name}.png")))
                .unwrap();
        }
        println!(
            "{name}: {} fields ({text} text, {checks} check, {radios} radio, {choices} choice), {n} filled, {} pages",
            fields.len(),
            doc.pages().len()
        );
    }
}
