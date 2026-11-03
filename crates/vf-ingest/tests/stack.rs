//! A scanned stack with cover sheets splits into one PDF per document, named for Filer.

use std::path::{Path, PathBuf};

use pdf_writer::{Content, Name, Pdf, Rect, Ref, Str};
use vf_ingest::pipeline::{self, Context, Origin};
use vf_ingest::sheet::{self, Cover, Labels, Sheet, TaxYear};

const CLIENTS: [(&str, &str); 4] = [
    ("1001", "Acme LLC"),
    ("1042", "Anderson Farms LLC"),
    ("1150", "Delgado, Maria"),
    ("1200", "Baker & Sons"),
];

/// `n` pages of synthetic text (several lines each, so none reads as blank).
fn text_pages(path: &Path, label: &str, n: usize) {
    let mut pdf = Pdf::new();
    let (catalog, tree, font) = (Ref::new(1), Ref::new(2), Ref::new(3));
    let ids: Vec<(Ref, Ref)> = (0..n)
        .map(|i| (Ref::new(4 + 2 * i as i32), Ref::new(5 + 2 * i as i32)))
        .collect();
    pdf.catalog(catalog).pages(tree);
    pdf.pages(tree)
        .kids(ids.iter().map(|(p, _)| *p))
        .count(n as i32);
    for (i, (page, content)) in ids.iter().enumerate() {
        let mut p = pdf.page(*page);
        p.media_box(Rect::new(0.0, 0.0, 612.0, 792.0));
        p.parent(tree);
        p.contents(*content);
        p.resources().fonts().pair(Name(b"F1"), font);
        drop(p);
        let mut c = Content::new();
        c.begin_text();
        c.set_font(Name(b"F1"), 14.0);
        c.next_line(72.0, 720.0);
        for l in 0..30 {
            c.show(Str(format!(
                "{label} page {} line {l}: synthetic test text 0000",
                i + 1
            )
            .as_bytes()));
            c.next_line(0.0, -20.0);
        }
        c.end_text();
        pdf.stream(*content, &c.finish());
    }
    pdf.type1_font(font).base_font(Name(b"Helvetica"));
    std::fs::write(path, pdf.finish()).unwrap();
}

fn blank_page(path: &Path) {
    let mut pdf = Pdf::new();
    pdf.catalog(Ref::new(1)).pages(Ref::new(2));
    pdf.pages(Ref::new(2)).kids([Ref::new(3)]).count(1);
    pdf.page(Ref::new(3))
        .media_box(Rect::new(0.0, 0.0, 612.0, 792.0))
        .parent(Ref::new(2));
    std::fs::write(path, pdf.finish()).unwrap();
}

fn cabinet(t: &Path) -> PathBuf {
    let root = t.join("cabinet");
    std::fs::create_dir_all(vf_fs::clients::master_dir(&root)).unwrap();
    vf_fs::doctypes::seed_if_missing(&root).unwrap();
    let csv = t.join("clients.csv");
    let rows: String = CLIENTS
        .iter()
        .map(|(id, name)| format!("{id},\"{name}\"\n"))
        .collect();
    std::fs::write(&csv, format!("ID,Name\n{rows}")).unwrap();
    let map = filer_core::config::ColumnMap {
        id: Some("ID".into()),
        name: Some("Name".into()),
        extras: vec![],
    };
    vf_fs::clients::import_csv(&root, &csv, &map, &Default::default()).unwrap();
    root
}

fn doc_type(root: &Path, name: &str) -> String {
    vf_fs::doctypes::load(root)
        .unwrap()
        .unwrap()
        .into_iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("seeded doc type {name}"))
        .id
}

#[test]
fn forty_page_stack_with_four_cover_sheets_splits_into_four_documents() {
    let Some(dir) = filer_pdf::pdf::find_pdfium(Path::new("")) else {
        panic!("pdfium.dll not found");
    };
    let pdfium = filer_pdf::pdf::pdfium(&dir).unwrap();
    let t = tempfile::tempdir().unwrap();
    let root = cabinet(t.path());
    let (inbox, staging) = (t.path().join("inbox"), t.path().join("scans"));
    std::fs::create_dir_all(&inbox).unwrap();
    std::fs::create_dir_all(&staging).unwrap();

    let types = ["W-2", "1099-INT", "K-1 (1065)", "Return copy — federal"];
    // 4 × (cover + 9 pages) = 40 pages; the first cover's blank back was scanned too.
    let mut parts: Vec<(PathBuf, String)> = Vec::new();
    for (i, ((id, name), ty)) in CLIENTS.iter().zip(types).enumerate() {
        let cover = t.path().join(format!("cover{i}.pdf"));
        let sheet = Sheet(Some(Cover {
            client_id: id.to_string(),
            doc_type: Some(doc_type(&root, ty)),
            tax_year: Some(TaxYear::Year(2025)),
        }));
        sheet::write_pdf(
            &cover,
            &[(
                sheet,
                Labels {
                    client: format!("{name} ({id})"),
                    doc_type: ty.into(),
                    tax_year: "2025".into(),
                },
            )],
        )
        .unwrap();
        parts.push((cover, String::new()));
        if i == 0 {
            let back = t.path().join("back.pdf");
            blank_page(&back);
            parts.push((back, String::new()));
        }
        let body = t.path().join(format!("body{i}.pdf"));
        text_pages(&body, name, 9);
        parts.push((body, String::new()));
    }
    let scan = staging.join("scan0001.pdf");
    assert_eq!(filer_pdf::pdf::merge_pdfs(&parts, &scan).unwrap(), 41);

    let ctx = Context {
        root: &root,
        inbox: &inbox,
        device_id: "dev",
        pdfium: Some(pdfium),
    };
    let done = pipeline::process(&ctx, &scan, Origin::Scans).unwrap();
    let names: Vec<String> = done
        .delivered
        .iter()
        .map(|d| d.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        vec![
            "Acme LLC_1001_2025 W-2.pdf",
            "Anderson Farms LLC_1042_2025 1099-INT.pdf",
            "Delgado, Maria_1150_2025 K-1 (1065).pdf",
            "Baker & Sons_1200_2025 Return copy — federal.pdf",
        ]
    );
    for d in &done.delivered {
        assert_eq!(filer_pdf::pdf::page_count(&d.path).unwrap(), 9, "{d:?}");
        assert!(d.note.ends_with("barcode cover sheet"), "{}", d.note);
    }
    // Filer reads the ID out of the name.
    let re = filer_core::parse::filename_regex(filer_core::parse::DEFAULT_ID_PATTERN).unwrap();
    let p = filer_core::parse::parse_filename(&names[1], &re, 2026);
    assert_eq!(
        (p.id.as_deref(), p.year),
        (Some("1042"), Some(2025)),
        "{p:?}"
    );

    // One hint per document, keyed by the delivered bytes.
    let hints = vf_fs::hints::load(&root);
    assert_eq!(hints.len(), 4);
    let k1 = &done.delivered[2].path;
    let h = &hints[&filer_intake::commit::sha256_file(k1).unwrap()];
    assert_eq!(
        h.doc_type.as_deref(),
        Some(doc_type(&root, "K-1 (1065)").as_str())
    );
    assert_eq!(h.tax_year, Some(2025));
    assert_eq!(h.origin, "barcode");

    // The original is kept, out of the way.
    assert!(!scan.exists());
    assert!(done.kept.starts_with(staging.join(pipeline::PROCESSED_DIR)));
    assert!(done.kept.is_file());
    // Nothing else is left in the inbox (no temporary files).
    assert_eq!(std::fs::read_dir(&inbox).unwrap().count(), 4);
}

#[test]
fn a_pdf_without_sheets_goes_through_unchanged() {
    let Some(dir) = filer_pdf::pdf::find_pdfium(Path::new("")) else {
        panic!("pdfium.dll not found");
    };
    let pdfium = filer_pdf::pdf::pdfium(&dir).unwrap();
    let t = tempfile::tempdir().unwrap();
    let root = cabinet(t.path());
    let (inbox, staging) = (t.path().join("inbox"), t.path().join("scans"));
    std::fs::create_dir_all(&inbox).unwrap();
    std::fs::create_dir_all(&staging).unwrap();
    let scan = staging.join("Acme LLC_1001_2025 W-2.pdf");
    text_pages(&scan, "W-2", 2);
    let bytes = std::fs::read(&scan).unwrap();
    // Taken in the inbox: delivered as " (2)".
    std::fs::write(inbox.join("Acme LLC_1001_2025 W-2.pdf"), b"other").unwrap();
    let ctx = Context {
        root: &root,
        inbox: &inbox,
        device_id: "dev",
        pdfium: Some(pdfium),
    };
    let done = pipeline::process(&ctx, &scan, Origin::Scans).unwrap();
    assert_eq!(done.delivered.len(), 1);
    assert_eq!(
        done.delivered[0].path,
        inbox.join("Acme LLC_1001_2025 W-2 (2).pdf")
    );
    assert_eq!(std::fs::read(&done.delivered[0].path).unwrap(), bytes);
    assert!(vf_fs::hints::load(&root).is_empty());
}
