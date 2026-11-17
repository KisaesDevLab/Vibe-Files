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

/// Names the staged file being processed, in its staging folder, with how many runs in a row
/// stopped on it: `<name>\t<count>`. One marker per PC (`<device id>`), so PCs sharing a folder
/// never act on each other's. A run that stops on the same file twice in a row (a crash it
/// causes, not a quit) has it set aside rather than tried on every start (QA I4, QA2 #1/#13).
pub fn marker(dir: &Path, device_id: &str) -> PathBuf {
    dir.join(format!(".vf-processing-{}.txt", here(device_id)))
}

/// This computer's name (a login keeps one device id on every session host it signs in
/// to: what is "this copy's own" in a shared staging folder is the id on this computer).
fn computer() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_default()
}

/// `device_id` on this computer, as a part of a file name.
fn here(device_id: &str) -> String {
    format!("{device_id}-{}", computer())
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect()
}

/// Runs in a row that stopped on a file before it is set aside.
pub const STOPS_TO_SET_ASIDE: u32 = 2;

fn read_marker(m: &Path) -> Option<(String, u32)> {
    let t = std::fs::read_to_string(m).ok()?;
    let (name, n) = t.trim().split_once('\t').unwrap_or((t.trim(), "1"));
    Some((name.to_string(), n.trim().parse().unwrap_or(1)))
}

/// At start: the file earlier runs stopped on is set aside once they did so
/// [`STOPS_TO_SET_ASIDE`] times in a row; after one stop (a quit mid-file) it is tried again.
pub fn recover_marker(dir: &Path, device_id: &str) -> Option<Event> {
    let m = marker(dir, device_id);
    let (name, count) = read_marker(&m)?;
    let staged = dir.join(&name);
    if name.is_empty() || !staged.is_file() {
        let _ = std::fs::remove_file(&m);
        return None;
    }
    if count < STOPS_TO_SET_ASIDE {
        return None; // kept: the next run counts on from it
    }
    let _ = std::fs::remove_file(&m);
    let error = "Vibe Files stopped while processing this file more than once; it was set aside so it is not tried again. Open it to check it, then put it back in the folder to retry.".to_string();
    pipeline::set_aside(&staged, &error).ok()?;
    Some(Event::Failed { staged, error })
}

/// Size and modified time: a file left in place after it was handled is skipped until it changes.
fn stamp(p: &Path) -> Option<(u64, std::time::SystemTime)> {
    let m = std::fs::metadata(p).ok()?;
    Some((m.len(), m.modified().ok()?))
}

/// Files handled but left in a staging folder (open in another program), kept across restarts
/// so a restart does not deliver them again (QA2 #3): `<name>\t<size>\t<mtime secs>` lines,
/// one file per PC (QA3 E8).
pub fn handled_file(dir: &Path, device_id: &str) -> PathBuf {
    dir.join(format!(".vf-handled-{}.txt", here(device_id)))
}

fn secs(t: std::time::SystemTime) -> u64 {
    t.duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The handled list of `dir`, for entries whose file is still there unchanged.
pub fn load_handled(dir: &Path, device_id: &str) -> HashMap<PathBuf, (u64, u64)> {
    std::fs::read_to_string(handled_file(dir, device_id))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut it = l.split('\t');
            let (n, len, t) = (
                it.next()?,
                it.next()?.parse().ok()?,
                it.next()?.parse().ok()?,
            );
            Some((dir.join(n), (len, t)))
        })
        .filter(|(p, s)| stamp(p).map(|(l, t)| (l, secs(t))) == Some(*s))
        .collect()
}

fn save_handled(dir: &Path, device_id: &str, all: &HashMap<PathBuf, (u64, u64)>) {
    let mut lines: Vec<String> = all
        .iter()
        .filter(|(p, _)| p.parent() == Some(dir))
        .filter_map(|(p, (l, t))| Some(format!("{}\t{l}\t{t}", p.file_name()?.to_string_lossy())))
        .collect();
    lines.sort();
    let f = handled_file(dir, device_id);
    if lines.is_empty() {
        let _ = std::fs::remove_file(f);
    } else {
        let _ = std::fs::write(f, lines.join("\n"));
    }
}

/// A claim on a staged file older than this was left by a copy of Vibe Files that stopped: it
/// is taken over.
pub const CLAIM_STALE: Duration = Duration::from_secs(60 * 60);

/// The claim file of a staged file: `.vf-claim-<name>.txt` beside it (dot files are not staged).
pub fn claim_file(p: &Path) -> Option<PathBuf> {
    let name = p.file_name()?.to_string_lossy().into_owned();
    Some(p.with_file_name(format!(".vf-claim-{name}.txt")))
}

/// How long staged files wait while the cabinet's lists cannot be read ([`passing`]).
const PASSING_FOR: Duration = Duration::from_secs(300);

/// Whether an error from reading the cabinet's lists is of the cabinet at this moment
/// (someone else saving a shared file, the share not answering), not a lasting one.
fn passing(error: &str) -> bool {
    let e = error.to_ascii_lowercase();
    [
        "right now",
        "just now",
        "cannot be reached",
        "database is locked",
        "database is busy",
        "cannot read the client list",
        "used by another process",
    ]
    .iter()
    .any(|w| e.contains(w))
}

/// A claim marked done: the file was delivered by the PC named after it.
const DONE: &str = "done:";

/// Claims a staged file for this copy of Vibe Files, so two copies watching the same folder
/// (two logins on the office server, two PCs on one network folder) never both deliver it
/// (owner 2026-11-04: "no duplicate processing"). The claim is an atomic create beside the
/// file; a stale one (older than [`CLAIM_STALE`], or this PC's own from a run that stopped) is
/// taken over. `None` = another copy has it.
pub fn claim(p: &Path, device_id: &str) -> Option<PathBuf> {
    claim_on(p, device_id, &computer())
}

/// [`claim`] as the copy of `device_id` running on `computer`. A claim is this copy's own
/// (left by a run of it that stopped) only when it names this device **and** this computer:
/// one login signed in on two session hosts has one device id, and each took the other's
/// claim for its own and delivered the same file (QA 2026-11-17). On one computer a login
/// runs one copy at a time (the data folder's lock), so there it is a run that stopped.
pub fn claim_on(p: &Path, device_id: &str, computer: &str) -> Option<PathBuf> {
    let me = format!("{device_id}@{computer}");
    let c = claim_file(p)?;
    for _ in 0..2 {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&c)
        {
            Ok(mut f) => {
                use std::io::Write;
                let _ = f.write_all(me.as_bytes());
                // Taken while another copy finished it: nothing left to do.
                if !p.is_file() {
                    let _ = std::fs::remove_file(&c);
                    return None;
                }
                return Some(c);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Delivered already (its original could not be moved out): never again.
                if std::fs::read_to_string(&c).is_ok_and(|t| t.starts_with(DONE)) {
                    return None;
                }
                // (A claim from before the computer was written in it names the device alone.)
                let mine = std::fs::read_to_string(&c)
                    .is_ok_and(|t| t.trim() == me || t.trim() == device_id);
                let stale = std::fs::metadata(&c)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|age| age > CLAIM_STALE);
                if !(mine || stale) || std::fs::remove_file(&c).is_err() {
                    return None;
                }
            }
            Err(_) => return None,
        }
    }
    None
}

/// [`pipeline::process`] with the marker written around it and a panic turned into an error.
fn process_guarded(ctx: &Context<'_>, p: &Path, origin: Origin) -> Result<Processed, String> {
    let m = p.parent().map(|d| marker(d, ctx.device_id));
    if let (Some(m), Some(n)) = (&m, p.file_name()) {
        let n = n.to_string_lossy().into_owned();
        let count = read_marker(m)
            .filter(|(name, _)| *name == n)
            .map_or(1, |(_, c)| c + 1);
        let _ = std::fs::write(m, format!("{n}\t{count}"));
    }
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pipeline::process(ctx, p, origin)
    }))
    .unwrap_or_else(|_| Err("processing failed unexpectedly (internal error)".into()));
    if let Some(m) = &m {
        let _ = std::fs::remove_file(m);
    }
    r
}

fn run(config: Arc<Mutex<Config>>, waiting: Arc<AtomicUsize>, tx: Sender<Event>, wake: impl Fn()) {
    // Size seen on the previous pass: a file is processed when it has the same, non-zero size
    // twice in a row and can be opened for writing (no one else is still writing it).
    let mut sizes: HashMap<PathBuf, u64> = HashMap::new();
    // Since when the cabinet's lists could not be read for the files that are ready.
    let mut waited: Option<std::time::Instant> = None;
    // Handled, but could not be moved out of the folder (open in another program): not handled
    // again until it changes (QA I5).
    let mut stuck: HashMap<PathBuf, (u64, u64)> = HashMap::new();
    let mut checked: std::collections::HashSet<PathBuf> = Default::default();
    loop {
        if Arc::strong_count(&config) == 1 {
            return; // the Worker was dropped
        }
        let cfg = config.lock().expect("config lock").clone();
        for (dir, _) in &cfg.folders {
            if checked.insert(dir.clone()) {
                stuck.extend(load_handled(dir, &cfg.device_id));
                if let Some(ev) = recover_marker(dir, &cfg.device_id) {
                    if tx.send(ev).is_err() {
                        return;
                    }
                    wake();
                }
            }
        }
        let before = stuck.len();
        stuck.retain(|p, s| stamp(p).map(|(l, t)| (l, secs(t))) == Some(*s));
        if stuck.len() != before {
            for (dir, _) in &cfg.folders {
                save_handled(dir, &cfg.device_id, &stuck);
            }
        }
        let mut seen: HashMap<PathBuf, u64> = HashMap::new();
        let mut ready: Vec<(PathBuf, Origin)> = Vec::new();
        let mut dirs_seen: std::collections::HashSet<PathBuf> = Default::default();
        for (dir, origin) in &cfg.folders {
            // One folder named twice (as Scans and as print capture): read once.
            let key = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.clone());
            if !dirs_seen.insert(key) {
                continue;
            }
            for (p, len) in staged_files(dir) {
                if len > 0 && sizes.get(&p) == Some(&len) && !stuck.contains_key(&p) && openable(&p)
                {
                    ready.push((p.clone(), *origin));
                }
                seen.insert(p, len);
            }
        }
        waiting.store(seen.len(), Ordering::Relaxed);
        sizes = seen;
        // What every file needs of the cabinet first (the client list, the doc types), looked
        // at before any file is touched: when it cannot be read just now (another PC saving
        // it, the share for a moment) the files wait for the next pass, for a few minutes.
        // Processed then, each failed and went to the failed folder for good (QA
        // 2026-11-17). Before any work: nothing is delivered or read twice.
        let ready = if ready.is_empty() {
            ready
        } else {
            match pipeline::cabinet_ready(&cfg.root) {
                Err(e)
                    if passing(&e)
                        && waited.get_or_insert_with(std::time::Instant::now).elapsed()
                            < PASSING_FOR =>
                {
                    Vec::new()
                }
                // Read again: the next trouble is waited for anew.
                Ok(()) => {
                    waited = None;
                    ready
                }
                // Waited long enough (or not a passing kind): the files go on, and go on at
                // every pass from now on, not after another wait each time.
                Err(_) => ready,
            }
        };
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
                // Another copy of Vibe Files is on it (or just finished it): leave it.
                let Some(claimed) = claim(&p, &cfg.device_id) else {
                    continue;
                };
                let ev = match process_guarded(&ctx, &p, origin) {
                    Ok(done) => Event::Processed(done),
                    Err(error) => {
                        let _ = pipeline::set_aside(&p, &error);
                        Event::Failed {
                            staged: p.clone(),
                            error,
                        }
                    }
                };
                if matches!(ev, Event::Processed(_)) && p.is_file() {
                    // Delivered, but the original could not be moved out (open elsewhere): the
                    // claim stays, marked done, so no other PC delivers it again (QA
                    // 2026-11-04).
                    let _ = std::fs::write(&claimed, format!("{DONE}{}", cfg.device_id));
                } else {
                    let _ = std::fs::remove_file(&claimed);
                }
                let (Event::Processed(Processed { staged, .. }) | Event::Failed { staged, .. }) =
                    &ev;
                if let Some((l, t)) = stamp(staged) {
                    stuck.insert(staged.clone(), (l, secs(t)));
                    if let Some(dir) = staged.parent() {
                        save_handled(dir, &cfg.device_id, &stuck);
                    }
                }
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
