//! The OCR job queue (P03 step 2): one `ocr_job` row per PDF document, kept in the catalog,
//! so it survives a restart. Each page's text is saved as soon as it is read (`page_text`, by
//! content hash): a job cut short resumes at the first page without text.
//!
//! Priority: documents already in the catalog when the queue starts are backlog (1); anything
//! that arrives afterwards (intake) is 0 and goes first.
//!
//! Workers are plain threads (N = cores − 1), each with its own catalog connection: the work
//! is a blocking render plus a child process, so an async runtime adds nothing (STATE notes
//! the step text's "tokio").

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pdfium_render::prelude::{PdfRenderConfig, Pdfium};
use vf_core::db::{Connection, OptionalExtension, Storage, params};

use crate::engine::{OcrEngine, OcrError};

/// Page resolution for OCR.
pub const DPI: u32 = 300;
/// A page whose own text has at least this many non-space characters is not OCR'd.
pub const TEXT_MIN_CHARS: usize = 20;
/// Longest side of a rendered page, pixels (a 36" plan sheet would be 10 800 at 300 dpi).
const MAX_SIDE: f32 = 6000.0;
/// Attempts before a job is left failed.
const MAX_ATTEMPTS: i64 = 3;

pub const BACKLOG: i64 = 1;
pub const INTAKE: i64 = 0;

/// A claimed job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub doc_id: String,
    pub hash: String,
    pub rel_path: String,
}

/// Queues every live PDF document that has no job for its current content. Returns how many.
pub fn enqueue_missing(conn: &Connection, priority: i64) -> vf_core::Result<usize> {
    let now = vf_core::now_rfc3339();
    let n = conn.execute(
        "INSERT INTO ocr_job (doc_id, hash, priority, state, queued_at, updated_at)
         SELECT d.id, d.hash, ?1, 'queued', ?2, ?2 FROM document d
         WHERE d.deleted_at IS NULL AND lower(d.rel_path) LIKE '%.pdf'
           AND NOT EXISTS (SELECT 1 FROM ocr_job j WHERE j.doc_id = d.id AND j.hash = d.hash)
         ON CONFLICT(doc_id) DO UPDATE SET
           hash = excluded.hash, priority = excluded.priority, state = 'queued', pages = NULL,
           ocr_pages = 0, attempts = 0, error = NULL, queued_at = excluded.queued_at,
           updated_at = excluded.updated_at",
        params![priority, now],
    )?;
    Ok(n)
}

/// After a crash or a quit mid-job: running jobs go back to the queue (their finished pages
/// are kept).
pub fn recover(conn: &Connection) -> vf_core::Result<usize> {
    Ok(conn.execute(
        "UPDATE ocr_job SET state = 'queued' WHERE state = 'running'",
        [],
    )?)
}

/// Takes the next job (intake first, then oldest).
pub fn claim(conn: &Connection) -> vf_core::Result<Option<Job>> {
    let now = vf_core::now_rfc3339();
    let job = conn
        .query_row(
            "UPDATE ocr_job SET state = 'running', attempts = attempts + 1, updated_at = ?1
             WHERE doc_id = (SELECT doc_id FROM ocr_job WHERE state = 'queued'
                             ORDER BY priority, queued_at LIMIT 1)
             RETURNING doc_id, hash",
            [&now],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .optional()?;
    let Some((doc_id, hash)) = job else {
        return Ok(None);
    };
    let rel_path: Option<String> = conn
        .query_row(
            "SELECT rel_path FROM document WHERE id = ?1 AND deleted_at IS NULL",
            [&doc_id],
            |r| r.get(0),
        )
        .optional()?;
    match rel_path {
        Some(rel_path) => Ok(Some(Job {
            doc_id,
            hash,
            rel_path,
        })),
        None => {
            set_state(conn, &doc_id, "skipped", Some("document removed"))?;
            claim(conn)
        }
    }
}

fn set_state(
    conn: &Connection,
    doc_id: &str,
    state: &str,
    error: Option<&str>,
) -> vf_core::Result<()> {
    conn.execute(
        "UPDATE ocr_job SET state = ?2, error = ?3, updated_at = ?4 WHERE doc_id = ?1",
        params![doc_id, state, error, vf_core::now_rfc3339()],
    )?;
    Ok(())
}

/// How a job ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Every page has text; indexed. `ocr_pages` pages were read by OCR.
    Indexed {
        pages: usize,
        ocr_pages: usize,
    },
    /// The stop flag was raised; finished pages are kept, the job stays running until
    /// [`recover`].
    Stopped,
    /// Pages need OCR and no engine is available: back in the queue.
    Waiting(String),
    Failed(String),
}

/// Reads every page of the job's PDF that has no saved text yet, then indexes the document.
pub fn run_job(
    conn: &Connection,
    root: &Path,
    pdfium: &Pdfium,
    engine: Option<&dyn OcrEngine>,
    job: &Job,
    stop: &AtomicBool,
) -> vf_core::Result<Outcome> {
    let outcome = read_pages(conn, root, pdfium, engine, job, stop)?;
    match &outcome {
        Outcome::Indexed { pages, ocr_pages } => {
            let texts = page_texts(conn, &job.hash)?;
            vf_index::fts::replace_doc(conn, &job.doc_id, &texts)?;
            conn.execute(
                "UPDATE ocr_job SET state = ?2, pages = ?3, ocr_pages = ?4, error = NULL,
                 updated_at = ?5 WHERE doc_id = ?1 AND hash = ?6",
                params![
                    job.doc_id,
                    if *ocr_pages > 0 { "indexed" } else { "done" },
                    *pages as i64,
                    *ocr_pages as i64,
                    vf_core::now_rfc3339(),
                    job.hash
                ],
            )?;
        }
        Outcome::Stopped => {}
        Outcome::Waiting(why) => {
            conn.execute(
                "UPDATE ocr_job SET state = 'queued', attempts = max(attempts - 1, 0), error = ?2,
                 queued_at = ?3, updated_at = ?3 WHERE doc_id = ?1",
                params![job.doc_id, why, vf_core::now_rfc3339()],
            )?;
        }
        Outcome::Failed(e) => {
            let attempts: i64 = conn.query_row(
                "SELECT attempts FROM ocr_job WHERE doc_id = ?1",
                [&job.doc_id],
                |r| r.get(0),
            )?;
            let state = if attempts >= MAX_ATTEMPTS {
                "failed"
            } else {
                "queued"
            };
            set_state(conn, &job.doc_id, state, Some(e))?;
        }
    }
    Ok(outcome)
}

/// Saved text of each page of `hash`, in page order.
pub fn page_texts(conn: &Connection, hash: &str) -> vf_core::Result<Vec<(u32, String)>> {
    let mut st = conn.prepare("SELECT page, text FROM page_text WHERE hash = ?1 ORDER BY page")?;
    let v = st
        .query_map([hash], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(v)
}

fn read_pages(
    conn: &Connection,
    root: &Path,
    pdfium: &Pdfium,
    engine: Option<&dyn OcrEngine>,
    job: &Job,
    stop: &AtomicBool,
) -> vf_core::Result<Outcome> {
    let path = root.join(&job.rel_path);
    let doc = match pdfium.load_pdf_from_file(&path, None) {
        Ok(d) => d,
        Err(e) => return Ok(Outcome::Failed(format!("cannot open the PDF: {e}"))),
    };
    let done: std::collections::HashMap<u32, String> = {
        let mut st = conn.prepare("SELECT page, source FROM page_text WHERE hash = ?1")?;
        st.query_map([&job.hash], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?
    };
    let pages = doc.pages();
    let count = pages.len() as usize;
    let mut ocr_pages = done.values().filter(|s| *s == "ocr").count();
    for (i, page) in pages.iter().enumerate() {
        let i = i as u32;
        if done.contains_key(&i) {
            continue;
        }
        if stop.load(Ordering::Relaxed) {
            return Ok(Outcome::Stopped);
        }
        let own = page.text().map(|t| t.all()).unwrap_or_default();
        if own.chars().filter(|c| !c.is_whitespace()).count() >= TEXT_MIN_CHARS {
            save_page(conn, &job.hash, i, "pdf", &own, None)?;
            continue;
        }
        let Some(engine) = engine else {
            return Ok(Outcome::Waiting("no OCR engine on this PC".into()));
        };
        let (w, h) = (page.width().value, page.height().value);
        let scale = (DPI as f32 / 72.0).min(MAX_SIDE / w.max(h).max(1.0));
        let dpi = (scale * 72.0).round() as u32;
        let img = match page
            .render_with_config(&PdfRenderConfig::new().set_target_width((w * scale) as i32))
            .and_then(|b| b.as_image())
        {
            Ok(i) => i.to_luma8(),
            Err(e) => {
                return Ok(Outcome::Failed(format!(
                    "cannot render page {}: {e}",
                    i + 1
                )));
            }
        };
        match engine.recognize(&img, dpi) {
            Ok(p) => {
                let words = serde_json::to_string(&p).unwrap_or_default();
                save_page(conn, &job.hash, i, "ocr", &p.text(), Some((&words, dpi)))?;
                ocr_pages += 1;
            }
            Err(OcrError::Unavailable(e)) => return Ok(Outcome::Waiting(e)),
            Err(OcrError::Failed(e)) => {
                return Ok(Outcome::Failed(format!("page {}: {e}", i + 1)));
            }
        }
    }
    Ok(Outcome::Indexed {
        pages: count,
        ocr_pages,
    })
}

fn save_page(
    conn: &Connection,
    hash: &str,
    page: u32,
    source: &str,
    text: &str,
    ocr: Option<(&str, u32)>,
) -> vf_core::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO page_text (hash, page, source, text, words, dpi)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![hash, page, source, text, ocr.map(|o| o.0), ocr.map(|o| o.1)],
    )?;
    Ok(())
}

/// Jobs by state, for the status line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counts {
    pub queued: usize,
    pub running: usize,
    pub indexed: usize,
    pub done: usize,
    pub failed: usize,
}

impl Counts {
    pub fn load(conn: &Connection) -> vf_core::Result<Counts> {
        let mut c = Counts::default();
        let mut st = conn.prepare("SELECT state, count(*) FROM ocr_job GROUP BY state")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        for row in rows {
            let (state, n) = row?;
            let n = n as usize;
            match state.as_str() {
                "queued" => c.queued = n,
                "running" => c.running = n,
                "indexed" => c.indexed = n,
                "done" | "skipped" => c.done += n,
                "failed" => c.failed = n,
                _ => {}
            }
        }
        Ok(c)
    }

    /// Documents still waiting to be searchable.
    pub fn pending(&self) -> usize {
        self.queued + self.running
    }
}

// ---------------------------------------------------------------------------------------------
// Worker threads

pub struct Config {
    pub catalog: PathBuf,
    pub root: PathBuf,
    pub pdfium_dir: PathBuf,
    /// Configured `tesseract.exe` (or folder).
    pub tesseract: Option<PathBuf>,
    pub threads: usize,
    /// This workstation, for the write-back Transactions.
    pub device_id: String,
}

/// What the workers are doing, for the UI.
#[derive(Debug, Clone)]
pub struct Status {
    /// Engine version line, or why there is none.
    pub engine: Result<String, String>,
    /// Documents indexed since the queue started.
    pub indexed: usize,
    /// Text layers written since the queue started.
    pub written: usize,
    pub last_error: Option<String>,
}

impl Default for Status {
    fn default() -> Self {
        Status {
            engine: Err("not started".into()),
            indexed: 0,
            written: 0,
            last_error: None,
        }
    }
}

pub struct Queue {
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<Status>>,
    alive: Arc<AtomicUsize>,
}

/// The shared engine, and when making one was last tried.
type EngineSlot = Mutex<(Option<Arc<dyn OcrEngine>>, Option<Instant>)>;

/// Engine factory: called at start and again every minute while there is none.
pub type MakeEngine = Arc<dyn Fn() -> Result<Arc<dyn OcrEngine>, String> + Send + Sync>;

/// The default factory: Tesseract from `configured`, else the usual places.
pub fn tesseract_factory(configured: Option<PathBuf>) -> MakeEngine {
    Arc::new(move || {
        let t = crate::Tesseract::locate(configured.as_deref()).map_err(|e| e.to_string())?;
        Ok(Arc::new(t) as Arc<dyn OcrEngine>)
    })
}

impl Queue {
    /// Recovers interrupted jobs, queues the backlog, and starts the workers.
    pub fn start(
        config: Config,
        make_engine: MakeEngine,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Queue, String> {
        let conn = open(&config)?;
        recover(&conn).map_err(|e| e.to_string())?;
        enqueue_missing(&conn, BACKLOG).map_err(|e| e.to_string())?;
        drop(conn);
        let pdfium = filer_pdf::pdf::pdfium(&config.pdfium_dir)?;
        let stop = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(Status::default()));
        let alive = Arc::new(AtomicUsize::new(0));
        let config = Arc::new(config);
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let engine: Arc<EngineSlot> = Arc::new(Mutex::new((None, None)));
        for n in 0..config.threads.max(1) {
            let (stop, status, alive, config, wake, engine, make_engine) = (
                stop.clone(),
                status.clone(),
                alive.clone(),
                config.clone(),
                wake.clone(),
                engine.clone(),
                make_engine.clone(),
            );
            alive.fetch_add(1, Ordering::SeqCst);
            std::thread::Builder::new()
                .name(format!("vf-ocr-{n}"))
                .spawn(move || {
                    worker(
                        n,
                        &config,
                        pdfium,
                        &engine,
                        &make_engine,
                        &stop,
                        &status,
                        &*wake,
                    );
                    alive.fetch_sub(1, Ordering::SeqCst);
                })
                .map_err(|e| e.to_string())?;
        }
        Ok(Queue {
            stop,
            status,
            alive,
        })
    }

    pub fn status(&self) -> Status {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Asks the workers to stop after their current page.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    /// Stops and waits up to `timeout` for the workers to finish their page.
    pub fn stop_and_wait(&self, timeout: Duration) -> bool {
        self.stop();
        let t = Instant::now();
        while self.alive.load(Ordering::SeqCst) > 0 {
            if t.elapsed() > timeout {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        true
    }
}

impl Drop for Queue {
    fn drop(&mut self) {
        self.stop();
    }
}

fn open(config: &Config) -> Result<Connection, String> {
    vf_core::db::open(&config.catalog, Storage::Local).map_err(|e| e.to_string())
}

/// Default worker count: cores − 1, at least 1.
pub fn default_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(1))
        .unwrap_or(1)
        .max(1)
}

#[allow(clippy::too_many_arguments)]
fn worker(
    n: usize,
    config: &Config,
    pdfium: &Pdfium,
    engine: &EngineSlot,
    make_engine: &MakeEngine,
    stop: &AtomicBool,
    status: &Mutex<Status>,
    wake: &(dyn Fn() + Send + Sync),
) {
    let mut conn = match open(config) {
        Ok(c) => c,
        Err(e) => {
            if let Ok(mut s) = status.lock() {
                s.last_error = Some(e);
            }
            return;
        }
    };
    let mut last_scan = Instant::now();
    let mut last_writeback: Option<Instant> = None;
    while !stop.load(Ordering::Relaxed) {
        // New arrivals: worker 0 looks every 2 s.
        if n == 0 && last_scan.elapsed() >= Duration::from_secs(2) {
            last_scan = Instant::now();
            let _ = enqueue_missing(&conn, INTAKE);
        }
        // Text layers held back (Filer's undoable batch, a lock): worker 0 retries every 30 s.
        if n == 0 && last_writeback.is_none_or(|t| t.elapsed() >= Duration::from_secs(30)) {
            last_writeback = Some(Instant::now());
            let (written, errors) =
                crate::writeback::write_ready(&mut conn, &config.root, &config.device_id);
            if let Ok(mut s) = status.lock() {
                s.written += written;
                if let Some(e) = errors.into_iter().last() {
                    s.last_error = Some(e);
                }
            }
        }
        let eng = {
            let mut g = match engine.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            let due = g.0.is_none() && g.1.is_none_or(|t| t.elapsed() >= Duration::from_secs(60));
            if due {
                g.1 = Some(Instant::now());
                match make_engine() {
                    Ok(e) => {
                        let name = e.name().to_string();
                        g.0 = Some(e);
                        if let Ok(mut s) = status.lock() {
                            s.engine = Ok(name);
                        }
                    }
                    Err(e) => {
                        if let Ok(mut s) = status.lock() {
                            s.engine = Err(e);
                        }
                    }
                }
                wake();
            }
            g.0.clone()
        };
        let job = match claim(&conn) {
            Ok(Some(j)) => j,
            Ok(None) => {
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
            Err(e) => {
                if let Ok(mut s) = status.lock() {
                    s.last_error = Some(e.to_string());
                }
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
        };
        let r = run_job(&conn, &config.root, pdfium, eng.as_deref(), &job, stop);
        if let Ok(Outcome::Indexed { ocr_pages, .. }) = &r
            && *ocr_pages > 0
        {
            let undoable = crate::writeback::filer_undoable(&config.root);
            let w = crate::writeback::write(
                &mut conn,
                &config.root,
                &config.device_id,
                &job.doc_id,
                &undoable,
            );
            if let Ok(mut s) = status.lock() {
                match w {
                    Ok(crate::writeback::Written::Done { .. }) => s.written += 1,
                    Ok(crate::writeback::Written::Held(_)) => {}
                    Err(e) => s.last_error = Some(format!("{}: text layer: {e}", job.rel_path)),
                }
            }
        }
        if let Ok(mut s) = status.lock() {
            match &r {
                Ok(Outcome::Indexed { .. }) => s.indexed += 1,
                Ok(Outcome::Failed(e)) => s.last_error = Some(format!("{}: {e}", job.rel_path)),
                Err(e) => s.last_error = Some(e.to_string()),
                _ => {}
            }
        }
        if r.is_err() {
            // A catalog error mid-job (busy, disk): back to the queue, finished pages kept.
            let _ = conn.execute(
                "UPDATE ocr_job SET state = 'queued', updated_at = ?2 WHERE doc_id = ?1 AND state = 'running'",
                params![job.doc_id, vf_core::now_rfc3339()],
            );
            std::thread::sleep(Duration::from_millis(500));
        }
        wake();
        if let Ok(Outcome::Waiting(_)) = r {
            // Nothing can be read without the engine; don't spin over the queue.
            std::thread::sleep(Duration::from_secs(5));
        }
    }
}
