//! The background thread that watches this PC's staging folders and runs [`pipeline::process`]
//! on each file once it has stopped growing. Polls once a second (works the same on local
//! folders and SMB shares); never touches a file still being written.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::pipeline::{self, Context, Origin, Processed};

/// What the worker should watch. Sent again whenever it changes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub root: PathBuf,
    /// Filer's first inbox; `None` = nowhere to deliver (files wait).
    pub inbox: Option<PathBuf>,
    pub folders: Vec<(PathBuf, Origin)>,
    pub device_id: String,
    pub pdfium_dir: Option<PathBuf>,
    /// Tesseract as configured (`config.toml [ocr]`), for recognizing scanned forms.
    pub tesseract: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Processed(Processed),
    Failed { staged: PathBuf, error: String },
}

pub struct Worker {
    config: Arc<Mutex<Config>>,
    events: Receiver<Event>,
    /// Files waiting in the staging folders (for the Intake badge).
    waiting: Arc<AtomicUsize>,
}

impl Worker {
    /// Starts the thread; `wake` is called after each event (to repaint the UI).
    pub fn start(config: Config, wake: impl Fn() + Send + 'static) -> Worker {
        let config = Arc::new(Mutex::new(config));
        let waiting = Arc::new(AtomicUsize::new(0));
        let (tx, events) = channel();
        let (c, w) = (config.clone(), waiting.clone());
        std::thread::Builder::new()
            .name("vf-ingest".into())
            .spawn(move || run(c, w, tx, wake))
            .expect("spawn the ingest thread");
        Worker {
            config,
            events,
            waiting,
        }
    }

    pub fn set_config(&self, config: Config) {
        *self.config.lock().expect("config lock") = config;
    }

    pub fn events(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }

    pub fn waiting(&self) -> usize {
        self.waiting.load(Ordering::Relaxed)
    }
}

/// Files directly in `dir` that a staging folder should process.
pub fn staged_files(dir: &Path) -> Vec<(PathBuf, u64)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return vec![];
    };
    rd.filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_lowercase();
            let m = e.metadata().ok()?;
            let skip = !m.is_file()
                || name.starts_with('.')
                || name.starts_with("~$")
                || name == "thumbs.db"
                || name == "desktop.ini"
                || [".tmp", ".part", ".crdownload", ".partial", ".error.txt"]
                    .iter()
                    .any(|x| name.ends_with(x));
            (!skip).then(|| (e.path(), m.len()))
        })
        .collect()
}

fn run(config: Arc<Mutex<Config>>, waiting: Arc<AtomicUsize>, tx: Sender<Event>, wake: impl Fn()) {
    // Size seen on the previous pass: a file is processed when it has the same, non-zero size
    // twice in a row and can be opened for writing (no one else is still writing it).
    let mut sizes: HashMap<PathBuf, u64> = HashMap::new();
    loop {
        if Arc::strong_count(&config) == 1 {
            return; // the Worker was dropped
        }
        let cfg = config.lock().expect("config lock").clone();
        let mut seen: HashMap<PathBuf, u64> = HashMap::new();
        let mut ready: Vec<(PathBuf, Origin)> = Vec::new();
        for (dir, origin) in &cfg.folders {
            for (p, len) in staged_files(dir) {
                if len > 0 && sizes.get(&p) == Some(&len) && openable(&p) {
                    ready.push((p.clone(), *origin));
                }
                seen.insert(p, len);
            }
        }
        waiting.store(seen.len(), Ordering::Relaxed);
        sizes = seen;
        if let (Some(inbox), false) = (&cfg.inbox, ready.is_empty()) {
            let pdfium = cfg
                .pdfium_dir
                .as_deref()
                .and_then(|d| filer_pdf::pdf::pdfium(d).ok());
            let tesseract = vf_ocr::Tesseract::locate(cfg.tesseract.as_deref()).ok();
            let ctx = Context {
                root: &cfg.root,
                inbox,
                device_id: &cfg.device_id,
                pdfium,
                ocr: tesseract.as_ref().map(|t| t as &dyn vf_ocr::OcrEngine),
            };
            for (p, origin) in ready {
                let ev = match pipeline::process(&ctx, &p, origin) {
                    Ok(done) => Event::Processed(done),
                    Err(error) => {
                        let _ = pipeline::set_aside(&p, &error);
                        Event::Failed { staged: p, error }
                    }
                };
                sizes.retain(|k, _| k.exists());
                if tx.send(ev).is_err() {
                    return; // the app is gone
                }
                wake();
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn openable(p: &Path) -> bool {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(p)
        .is_ok()
}
