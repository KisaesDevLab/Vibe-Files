//! PHASES P08: synthetic forms rendered to PDFs go through the intake pipeline and reach
//! Filer's inbox named for their client; the scanned one is read by real Tesseract. And no
//! full TIN ends up in anything the pipeline writes.

use std::path::{Path, PathBuf};

use vf_ingest::pipeline::{self, Context, Origin};

fn pdfium() -> &'static pdfium_render::prelude::Pdfium {
    let dir = filer_pdf::pdf::find_pdfium(Path::new("")).expect("pdfium");
    filer_pdf::pdf::pdfium(&dir).unwrap()
}

/// A cabinet with the synthetic clients (TIN column kept as last 4) and an inbox.
fn cabinet(t: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let root = t.join("cabinet");
    std::fs::create_dir_all(root.join("_Vibe")).unwrap();
    vf_fs::doctypes::seed_if_missing(&root).unwrap();
    let csv = t.join("clients.csv");
    let mut text = String::from("ID,Name,TIN\n");
    for (ext, name, tin) in vf_extract::synthetic::CLIENTS {
        text.push_str(&format!("{ext},\"{name}\",{tin}\n"));
    }
    std::fs::write(&csv, text).unwrap();
    let map = filer_core::config::ColumnMap {
        id: Some("ID".into()),
        name: Some("Name".into()),
        extras: vec![],
    };
    vf_fs::clients::import_csv_tin(&root, &csv, &map, &Default::default(), Some("TIN")).unwrap();
    let (inbox, staging) = (t.join("inbox"), t.join("scans"));
    std::fs::create_dir_all(&inbox).unwrap();
    std::fs::create_dir_all(&staging).unwrap();
    (root, inbox, staging)
}

fn lines(text: &str) -> Vec<String> {
    text.lines().map(str::to_string).collect()
}

fn delivered_name(p: &pipeline::Processed) -> String {
    p.delivered[0]
        .path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

/// Every file under `dir`, as text.
fn all_text(dir: &Path, out: &mut String) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            all_text(&p, out);
        } else if p.extension().is_some_and(|x| x != "pdf") {
            out.push_str(&std::fs::read_to_string(&p).unwrap_or_default());
        }
    }
}

#[test]
fn a_w2_with_a_text_layer_files_itself_and_no_full_tin_is_written() {
    let t = tempfile::tempdir().unwrap();
    let (root, inbox, staging) = cabinet(t.path());
    let cases = vf_extract::synthetic::cases();
    let w2 = cases
        .iter()
        .find(|c| c.name.starts_with("w2 2025 REYE5521"))
        .unwrap();
    let scan = staging.join("scan0001.pdf");
    vf_fixtures::write_pdf(&scan, &lines(&w2.text)).unwrap();
    let ctx = Context {
        root: &root,
        inbox: &inbox,
        device_id: "dev",
        pdfium: Some(pdfium()),
        ocr: None,
    };
    let done = pipeline::process(&ctx, &scan, Origin::Scans).unwrap();
    assert_eq!(
        delivered_name(&done),
        "Reyes, Dana & Lee_REYE5521_2025 W-2 Anderson Farms LLC.pdf"
    );
    assert!(
        done.delivered[0].note.contains("recognized W-2 2025"),
        "{}",
        done.delivered[0].note
    );
    let hints = vf_fs::hints::load(&root);
    let h = &hints[&done.delivered[0].sha256];
    assert_eq!(h.tax_year, Some(2025));
    let x = h.extract.as_ref().unwrap();
    assert_eq!(x.recipient_tin4.as_deref(), Some("4321"));

    // A K-1 too: entity primary, partner named in the note.
    let k1 = cases
        .iter()
        .find(|c| c.name.starts_with("k1-1065 2025 ORTI3300"))
        .unwrap();
    let scan = staging.join("scan0002.pdf");
    vf_fixtures::write_pdf(&scan, &lines(&k1.text)).unwrap();
    let done = pipeline::process(&ctx, &scan, Origin::Scans).unwrap();
    assert!(
        delivered_name(&done).contains("_CEDA7788_"),
        "{}",
        delivered_name(&done)
    );
    assert!(done.delivered[0].note.contains("also for Ortiz, Sam"));

    // PHASES P08 done-when 3: no full TIN anywhere the pipeline or the import wrote.
    let mut text = String::new();
    all_text(&root, &mut text);
    all_text(&inbox, &mut text);
    for full in vf_extract::synthetic::full_tins() {
        assert!(!text.contains(&full), "a full TIN {full} was written");
    }
}

/// A scanned 1099-INT (image only) is read by Tesseract when it is installed.
#[test]
fn a_scanned_1099_is_read_by_tesseract() {
    let Ok(tesseract) = vf_ocr::Tesseract::locate(None) else {
        eprintln!("Tesseract not installed: OCR recognition skipped");
        return;
    };
    let t = tempfile::tempdir().unwrap();
    let (root, inbox, staging) = cabinet(t.path());
    let text = "Form 1099-INT Interest Income\nFor calendar year 2025\nPAYER'S name\nNorthwind Bank NA\nRECIPIENT'S TIN\nXXX-XX-8765\nRECIPIENT'S name\nSam Ortiz\n1 Interest income 412.08";
    let scan = staging.join("scan0003.pdf");
    vf_fixtures::write_scanned_pdf(pdfium(), &scan, &[lines(text)]).unwrap();
    let ctx = Context {
        root: &root,
        inbox: &inbox,
        device_id: "dev",
        pdfium: Some(pdfium()),
        ocr: Some(&tesseract),
    };
    let done = pipeline::process(&ctx, &scan, Origin::Scans).unwrap();
    assert!(
        delivered_name(&done).starts_with("Ortiz, Sam_ORTI3300_2025 1099-INT"),
        "{} / {}",
        delivered_name(&done),
        done.delivered[0].note
    );
}
