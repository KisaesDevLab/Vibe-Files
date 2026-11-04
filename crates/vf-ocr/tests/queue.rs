//! The OCR queue against a real catalog and real PDFs, with a stand-in engine.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use image::GrayImage;
use pdf_writer::{Content, Name, Pdf, Rect, Ref};
use vf_core::db::{Connection, Storage};
use vf_ocr::queue::{self, Counts, Job, Outcome};
use vf_ocr::{OcrEngine, OcrError, OcrPage, Word};

/// Reads every page as "EIN 12-3456789 page <n>"; can raise a stop flag after `stop_after`
/// calls (a quit mid-document).
struct Fake {
    calls: AtomicUsize,
    stop_after: Option<(usize, Arc<AtomicBool>)>,
}

impl Fake {
    fn new() -> Fake {
        Fake {
            calls: AtomicUsize::new(0),
            stop_after: None,
        }
    }
}

impl OcrEngine for Fake {
    fn name(&self) -> &str {
        "fake"
    }
    fn recognize(&self, page: &GrayImage, dpi: u32) -> Result<OcrPage, OcrError> {
        assert_eq!(dpi, 300);
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some((after, stop)) = &self.stop_after
            && n >= *after
        {
            stop.store(true, Ordering::SeqCst);
        }
        let w = |t: &str, x| Word {
            bbox: [x, 100, 200, 40],
            text: t.into(),
            conf: 95.0,
            line: 0,
        };
        Ok(OcrPage {
            width: page.width(),
            height: page.height(),
            words: vec![
                w("EIN", 100),
                w("12-3456789", 320),
                w(&format!("call{n}"), 540),
            ],
        })
    }
}

/// `pages` Letter pages; with `text`, each carries real text, otherwise only a gray box (a
/// scan with no text layer).
fn pdf(path: &Path, pages: usize, text: bool) {
    let mut pdf = Pdf::new();
    let (catalog, tree, font) = (Ref::new(1), Ref::new(2), Ref::new(3));
    pdf.catalog(catalog).pages(tree);
    let kids: Vec<Ref> = (0..pages).map(|i| Ref::new(10 + 2 * i as i32)).collect();
    pdf.pages(tree)
        .kids(kids.iter().copied())
        .count(pages as i32);
    pdf.type1_font(font).base_font(Name(b"Helvetica"));
    for (i, id) in kids.iter().enumerate() {
        let content = Ref::new(id.get() + 1);
        let mut c = Content::new();
        if text {
            c.begin_text();
            c.set_font(Name(b"F1"), 14.0);
            c.next_line(72.0, 700.0);
            c.show(pdf_writer::Str(
                format!("Engagement letter for Acme LLC, page {}", i + 1).as_bytes(),
            ));
            c.end_text();
        } else {
            c.set_fill_gray(0.3);
            c.rect(72.0, 600.0, 300.0, 80.0);
            c.fill_nonzero();
        }
        pdf.stream(content, &c.finish());
        let mut p = pdf.page(*id);
        p.media_box(Rect::new(0.0, 0.0, 612.0, 792.0));
        p.parent(tree);
        p.contents(content);
        p.resources().fonts().pair(Name(b"F1"), font);
    }
    std::fs::write(path, pdf.finish()).unwrap();
}

struct Fx {
    _t: tempfile::TempDir,
    root: PathBuf,
    catalog: PathBuf,
    conn: Connection,
}

fn fx() -> Fx {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("cab");
    std::fs::create_dir_all(&root).unwrap();
    let catalog = t.path().join("catalog.db");
    let conn = vf_core::db::open(&catalog, Storage::Local).unwrap();
    Fx {
        _t: t,
        root,
        catalog,
        conn,
    }
}

impl Fx {
    fn add(&self, id: &str, name: &str, pages: usize, text: bool) {
        pdf(&self.root.join(name), pages, text);
        self.conn
            .execute(
                "INSERT INTO document (id, rel_path, hash, size, filed_at, device_id, updated_at)
                 VALUES (?1, ?2, ?3, 1, 't', 'dev', 't')",
                (id, name, format!("hash-{id}")),
            )
            .unwrap();
    }
    fn state(&self, id: &str) -> String {
        self.conn
            .query_row("SELECT state FROM ocr_job WHERE doc_id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap()
    }
    fn hits(&self, q: &str) -> Vec<(String, i64)> {
        let mut st = self
            .conn
            .prepare(
                "SELECT doc_id, page FROM doc_fts WHERE doc_fts MATCH ?1 ORDER BY doc_id, page",
            )
            .unwrap();
        st.query_map([format!("\"{q}\"")], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }
}

fn pdfium() -> &'static pdfium_render::prelude::Pdfium {
    let dir = filer_pdf::pdf::find_pdfium(Path::new("")).expect("pdfium");
    filer_pdf::pdf::pdfium(&dir).unwrap()
}

/// PHASES P03 done-when: OCR survives an app restart mid-backlog without re-doing finished
/// pages.
#[test]
fn restart_mid_document_resumes_without_redoing_pages() {
    let f = fx();
    f.add("d1", "scan.pdf", 3, false);
    assert_eq!(queue::enqueue_missing(&f.conn, queue::BACKLOG).unwrap(), 1);

    // First run: the app quits after the second page.
    let stop = Arc::new(AtomicBool::new(false));
    let first = Fake {
        calls: AtomicUsize::new(0),
        stop_after: Some((2, stop.clone())),
    };
    let job = queue::claim(&f.conn).unwrap().unwrap();
    let r = queue::run_job(&f.conn, &f.root, pdfium(), Some(&first), &job, &stop).unwrap();
    assert_eq!(r, Outcome::Stopped);
    assert_eq!(first.calls.load(Ordering::SeqCst), 2);
    assert_eq!(f.state("d1"), "queued", "a quit is not an attempt");

    // Restart.
    assert_eq!(queue::recover(&f.conn).unwrap(), 0);
    let second = Fake::new();
    let job = queue::claim(&f.conn).unwrap().unwrap();
    let r = queue::run_job(
        &f.conn,
        &f.root,
        pdfium(),
        Some(&second),
        &job,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        r,
        Outcome::Indexed {
            pages: 3,
            ocr_pages: 3
        }
    );
    assert_eq!(
        second.calls.load(Ordering::SeqCst),
        1,
        "only page 3 was left"
    );
    assert_eq!(f.state("d1"), "indexed", "waits for the text layer");
    assert_eq!(
        f.hits("3456789"),
        vec![("d1".into(), 0), ("d1".into(), 1), ("d1".into(), 2)]
    );
}

#[test]
fn pages_with_their_own_text_need_no_engine() {
    let f = fx();
    f.add("d1", "letter.pdf", 2, true);
    queue::enqueue_missing(&f.conn, queue::BACKLOG).unwrap();
    let job = queue::claim(&f.conn).unwrap().unwrap();
    let r = queue::run_job(
        &f.conn,
        &f.root,
        pdfium(),
        None,
        &job,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        r,
        Outcome::Indexed {
            pages: 2,
            ocr_pages: 0
        }
    );
    assert_eq!(f.state("d1"), "done");
    assert_eq!(
        f.hits("Engagement"),
        vec![("d1".into(), 0), ("d1".into(), 1)]
    );
    assert_eq!(f.hits("page 2"), vec![("d1".into(), 1)]);
}

#[test]
fn without_an_engine_scans_wait_and_text_pdfs_still_index() {
    let f = fx();
    f.add("a", "scan.pdf", 1, false);
    f.add("b", "letter.pdf", 1, true);
    queue::enqueue_missing(&f.conn, queue::BACKLOG).unwrap();
    let mut outcomes = Vec::new();
    for _ in 0..2 {
        let job: Job = queue::claim(&f.conn).unwrap().unwrap();
        outcomes.push((
            job.doc_id.clone(),
            queue::run_job(
                &f.conn,
                &f.root,
                pdfium(),
                None,
                &job,
                &AtomicBool::new(false),
            )
            .unwrap(),
        ));
    }
    let a = outcomes.iter().find(|(id, _)| id == "a").unwrap();
    assert!(matches!(a.1, Outcome::Waiting(_)), "{a:?}");
    assert_eq!(f.state("a"), "queued");
    assert_eq!(f.state("b"), "done");
    let attempts: i64 = f
        .conn
        .query_row("SELECT attempts FROM ocr_job WHERE doc_id = 'a'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(attempts, 0, "waiting is not a failed attempt");
}

#[test]
fn intake_goes_before_backlog_and_new_content_requeues() {
    let f = fx();
    f.add("old", "old.pdf", 1, true);
    queue::enqueue_missing(&f.conn, queue::BACKLOG).unwrap();
    f.add("new", "new.pdf", 1, true);
    queue::enqueue_missing(&f.conn, queue::INTAKE).unwrap();
    assert_eq!(queue::claim(&f.conn).unwrap().unwrap().doc_id, "new");
    assert_eq!(queue::claim(&f.conn).unwrap().unwrap().doc_id, "old");
    assert_eq!(queue::enqueue_missing(&f.conn, queue::INTAKE).unwrap(), 0);
    f.conn
        .execute("UPDATE document SET hash = 'changed' WHERE id = 'old'", [])
        .unwrap();
    assert_eq!(queue::enqueue_missing(&f.conn, queue::INTAKE).unwrap(), 1);
    assert_eq!(f.state("old"), "queued");
}

#[test]
fn workers_index_the_backlog() {
    let f = fx();
    for i in 0..4 {
        f.add(&format!("s{i}"), &format!("scan{i}.pdf"), 2, false);
    }
    f.add("t", "letter.pdf", 1, true);
    let engine = Arc::new(Fake::new());
    let e2 = engine.clone();
    let q = queue::Queue::start(
        queue::Config {
            catalog: f.catalog.clone(),
            root: f.root.clone(),
            pdfium_dir: filer_pdf::pdf::find_pdfium(Path::new("")).unwrap(),
            threads: 3,
            device_id: "dev".into(),
        },
        Arc::new(move || Ok(e2.clone() as Arc<dyn OcrEngine>)),
        || {},
    )
    .unwrap();
    let t = Instant::now();
    loop {
        let c = Counts::load(&f.conn).unwrap();
        if c.pending() == 0 && c.indexed + c.done == 5 {
            // The 4 scans' fixture rows carry made-up hashes, so their text layer is skipped
            // ("changed outside Vibe Files") and they end done like the text PDF.
            assert_eq!(c.failed, 0);
            break;
        }
        assert!(t.elapsed() < Duration::from_secs(60), "{c:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(q.stop_and_wait(Duration::from_secs(10)));
    assert_eq!(engine.calls.load(Ordering::SeqCst), 8);
    assert_eq!(f.hits("12-3456789").len(), 8);
    assert_eq!(q.status().engine.as_deref(), Ok("fake"));
}

/// A page with a few words of its own is read by OCR (too little text to trust), but the text
/// layer never repeats the words it already has.
#[test]
fn short_own_text_is_not_repeated_by_ocr() {
    let Ok(engine) = vf_ocr::Tesseract::locate(None) else {
        assert!(
            std::env::var_os("VF_SKIP_TESSERACT").is_some(),
            "tesseract.exe not found"
        );
        return;
    };
    let f = fx();
    filer_pdf::pdf::write_test_pdf(&f.root.join("cover.pdf"), &["Bravo one"]);
    f.conn
        .execute(
            "INSERT INTO document (id, rel_path, hash, size, filed_at, device_id, updated_at)
             VALUES ('c', 'cover.pdf', 'hash-c', 1, 't', 'dev', 't')",
            [],
        )
        .unwrap();
    queue::enqueue_missing(&f.conn, queue::INTAKE).unwrap();
    let job = queue::claim(&f.conn).unwrap().unwrap();
    let r = queue::run_job(
        &f.conn,
        &f.root,
        pdfium(),
        Some(&engine),
        &job,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        r,
        Outcome::Indexed {
            pages: 1,
            ocr_pages: 0
        },
        "nothing new to layer"
    );
    let (source, text): (String, String) = f
        .conn
        .query_row(
            "SELECT source, text FROM page_text WHERE hash = 'hash-c'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(source, "pdf");
    assert_eq!(text.trim(), "Bravo one");
}

/// QA I10: a document the app crashes on is failed after three tries, not read on every start.
#[test]
fn a_document_that_crashes_the_app_fails_after_three_tries() {
    let f = fx();
    f.add("d1", "scan.pdf", 1, false);
    queue::enqueue_missing(&f.conn, queue::BACKLOG).unwrap();
    for _ in 0..3 {
        // Claimed, then the app dies mid-page: the next start puts it back.
        assert!(queue::claim(&f.conn).unwrap().is_some());
        assert_eq!(queue::recover(&f.conn).unwrap(), 1);
    }
    assert!(queue::claim(&f.conn).unwrap().is_none());
    assert_eq!(f.state("d1"), "failed");
}
