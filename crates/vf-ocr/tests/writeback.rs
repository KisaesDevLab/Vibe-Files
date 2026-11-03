//! The text layer written into the file as one Transaction, and held back while Filer can
//! still undo the batch that filed it.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use image::GrayImage;
use pdf_writer::{Content, Pdf, Rect, Ref};
use vf_core::db::{Connection, Storage};
use vf_ocr::queue::{self, Outcome};
use vf_ocr::writeback::{self, Held, Written};
use vf_ocr::{OcrEngine, OcrError, OcrPage, Word};

struct Fake;

impl OcrEngine for Fake {
    fn name(&self) -> &str {
        "fake"
    }
    fn recognize(&self, page: &GrayImage, _dpi: u32) -> Result<OcrPage, OcrError> {
        let w = |t: &str, x| Word {
            bbox: [x, 400, 300, 50],
            text: t.into(),
            conf: 95.0,
            line: 0,
        };
        Ok(OcrPage {
            width: page.width(),
            height: page.height(),
            words: vec![w("EIN", 300), w("12-3456789", 650)],
        })
    }
}

/// A one-page "scan": a gray box, no text.
fn scan(path: &Path) {
    let mut pdf = Pdf::new();
    let (catalog, tree, page, content) = (Ref::new(1), Ref::new(2), Ref::new(3), Ref::new(4));
    pdf.catalog(catalog).pages(tree);
    pdf.pages(tree).kids([page]).count(1);
    let mut c = Content::new();
    c.set_fill_gray(0.4);
    c.rect(72.0, 600.0, 300.0, 80.0);
    c.fill_nonzero();
    pdf.stream(content, &c.finish());
    let mut p = pdf.page(page);
    p.media_box(Rect::new(0.0, 0.0, 612.0, 792.0));
    p.parent(tree);
    p.contents(content);
    drop(p);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, pdf.finish()).unwrap();
}

struct Fx {
    _t: tempfile::TempDir,
    root: PathBuf,
    conn: Connection,
    rel: String,
    hash: String,
}

const REL: &str = "Acme LLC (1001)/Income Tax/2025/scan.pdf";

fn fx() -> Fx {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("cab");
    std::fs::create_dir_all(root.join("_Vibe")).unwrap();
    let conn = vf_core::db::open(&t.path().join("c.db"), Storage::Local).unwrap();
    scan(&root.join(REL));
    let hash = filer_intake::commit::sha256_file(&root.join(REL)).unwrap();
    conn.execute(
        "INSERT INTO client (id, ext_id, name, folder_rel, device_id, updated_at)
         VALUES ('c1', '1001', 'Acme LLC', 'Acme LLC (1001)', 'dev', 't')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO document (id, client_id, tax_year, rel_path, hash, size, filed_at, device_id, updated_at)
         VALUES ('d1', 'c1', 2025, ?1, ?2, 1, 't', 'dev', 't')",
        (REL, &hash),
    )
    .unwrap();
    Fx {
        _t: t,
        root,
        conn,
        rel: REL.into(),
        hash,
    }
}

fn pdfium() -> &'static pdfium_render::prelude::Pdfium {
    let dir = filer_pdf::pdf::find_pdfium(Path::new("")).expect("pdfium");
    filer_pdf::pdf::pdfium(&dir).unwrap()
}

/// OCR the document so it waits for its text layer.
fn read(f: &Fx) {
    queue::enqueue_missing(&f.conn, queue::INTAKE).unwrap();
    let job = queue::claim(&f.conn).unwrap().unwrap();
    let r = queue::run_job(
        &f.conn,
        &f.root,
        pdfium(),
        Some(&Fake),
        &job,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        r,
        Outcome::Indexed {
            pages: 1,
            ocr_pages: 1
        }
    );
}

#[test]
fn text_layer_is_written_and_the_original_kept_as_a_version() {
    let mut f = fx();
    read(&f);
    let (written, errors) = writeback::write_ready(&mut f.conn, &f.root, "dev");
    assert_eq!((written, errors), (1, vec![]));

    let path = f.root.join(&f.rel);
    let new_hash = filer_intake::commit::sha256_file(&path).unwrap();
    assert_ne!(new_hash, f.hash);
    let (hash, size, rev): (String, i64, i64) = f
        .conn
        .query_row(
            "SELECT hash, size, rev FROM document WHERE id = 'd1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(hash, new_hash);
    assert_eq!(size as u64, std::fs::metadata(&path).unwrap().len());
    assert_eq!(rev, 2);

    // The text is in the file.
    let doc = pdfium().load_pdf_from_file(&path, None).unwrap();
    let text = doc.pages().first().unwrap().text().unwrap().all();
    assert!(text.contains("EIN 12-3456789"), "{text}");
    drop(doc);

    // The original: kept as a version, with its sidecar and row.
    let kept = vf_fs::cabinet::version_path(&f.root, &f.hash);
    assert_eq!(filer_intake::commit::sha256_file(&kept).unwrap(), f.hash);
    assert!(kept.with_extension("json").is_file());
    let versions: i64 = f
        .conn
        .query_row(
            "SELECT count(*) FROM version WHERE doc_id = 'd1' AND hash = ?1 AND created_by = 'ocr'",
            [&f.hash],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(versions, 1);

    // Job done for the new content: nothing is queued again, search still finds it.
    let (state, job_hash): (String, String) = f
        .conn
        .query_row(
            "SELECT state, hash FROM ocr_job WHERE doc_id = 'd1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        (state.as_str(), job_hash.as_str()),
        ("done", new_hash.as_str())
    );
    assert_eq!(queue::enqueue_missing(&f.conn, queue::INTAKE).unwrap(), 0);
    assert_eq!(vf_index::fts::pages_of(&f.conn, "d1").unwrap(), vec![0]);

    // Rebuild keeps the document's metadata: docmeta has a line for the new hash.
    let meta = std::fs::read_to_string(f.root.join("Acme LLC (1001)/_Vibe/docmeta.jsonl")).unwrap();
    assert!(meta.contains(&new_hash), "{meta}");
    let audited: i64 = f
        .conn
        .query_row(
            "SELECT count(*) FROM audit WHERE action = 'ocr_text_layer' AND doc_id = 'd1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(audited, 1);
    assert!(
        std::fs::read_dir(f.root.join("_Vibe/tmp"))
            .unwrap()
            .next()
            .is_none(),
        "no temp files left"
    );
}

/// "Undo last batch" stays available: a file in Filer's undoable batch keeps its bytes (Filer
/// refuses to undo a file whose size changed) until the batch is undone or replaced.
#[test]
fn files_in_filers_undoable_batch_wait() {
    let mut f = fx();
    read(&f);
    let master = vf_fs::clients::master_dir(&f.root);
    std::fs::create_dir_all(&master).unwrap();
    let batch = |undone: bool| {
        serde_json::json!({
            "batch": "6f1b2c3d-0000-4000-8000-000000000001",
            "committed_at": "2026-11-03T12:00:00Z",
            "in_progress": false,
            "undone": undone,
            "items": [{
                "src": "C:/inbox/scan.pdf",
                "dst": f.root.join(&f.rel),
                "method": "move",
                "bytes": 1,
                "client_id": "1001",
                "return_to": "C:/inbox/scan.pdf",
            }],
            "created_dirs": [],
            "created_files": [],
            "backups": [],
        })
    };
    let lb = filer_intake::commit::last_batch_path(&master);
    std::fs::write(&lb, batch(false).to_string()).unwrap();
    let undoable = writeback::filer_undoable(&f.root);
    assert_eq!(undoable.len(), 1, "{undoable:?}");
    assert_eq!(
        writeback::write(&mut f.conn, &f.root, "dev", "d1", &undoable).unwrap(),
        Written::Held(Held::FilerUndo)
    );
    assert_eq!(
        filer_intake::commit::sha256_file(&f.root.join(&f.rel)).unwrap(),
        f.hash,
        "untouched"
    );
    assert_eq!(writeback::write_ready(&mut f.conn, &f.root, "dev").0, 0);

    // A checked-out document waits too.
    std::fs::write(&lb, batch(true).to_string()).unwrap();
    f.conn
        .execute(
            "INSERT INTO lock (doc_id, user, device_id, since) VALUES ('d1', 'u', 'dev', 't')",
            [],
        )
        .unwrap();
    assert_eq!(writeback::write_ready(&mut f.conn, &f.root, "dev").0, 0);
    f.conn.execute("DELETE FROM lock", []).unwrap();

    // The batch was undone (or replaced): written.
    assert_eq!(writeback::write_ready(&mut f.conn, &f.root, "dev").0, 1);
}
