//! Intake: watches the inboxes (with their sub-folders unless that is turned off), takes files added by drag-drop or Browse, and
//! queues each file once it is stable: same size on two 1-second checks and openable with
//! exclusive share. Non-PDF files are queued as unsupported.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};

/// Stability check interval.
pub const TICK: Duration = Duration::from_secs(1);
/// Full re-listing of the inboxes, in case watcher events were missed (X55).
pub const RECHECK: Duration = Duration::from_secs(30);
const DEBOUNCE: Duration = Duration::from_millis(500);
/// Guard against junction loops when walking folders.
const MAX_DEPTH: usize = 32;
/// Where combined originals are parked after filing (X84); never picked up again.
pub const COMBINED_DIR: &str = "_combined";

/// The path is inside a `_combined` folder.
pub fn in_combined_dir(path: &Path) -> bool {
    path.components()
        .any(|c| c.as_os_str().eq_ignore_ascii_case(COMBINED_DIR))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Found in this inbox (or one of its sub-folders).
    Inbox(PathBuf),
    /// Added by drag-drop or Browse from outside the inboxes.
    Manual,
    /// Extracted from a zip sent to the queue (P07b): the zip and the entry's path inside it.
    Zip { zip: PathBuf, entry: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Pdf,
    /// A `.zip`: an Import draft (P07b), not queued for filing.
    Zip,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedFile {
    pub path: PathBuf,
    pub size: u64,
    pub added: DateTime<Local>,
    pub source: Source,
    pub kind: Kind,
}

pub fn kind_of(path: &Path) -> Kind {
    match path.extension() {
        Some(e) if e.eq_ignore_ascii_case("pdf") => Kind::Pdf,
        Some(e) if e.eq_ignore_ascii_case("zip") => Kind::Zip,
        _ => Kind::Unsupported,
    }
}

/// Files never queued: hidden/system, Office lock files (`~$x`), dot files, Windows clutter,
/// and partial downloads.
fn skipped(path: &Path, meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    if meta.file_attributes() & 0x6 != 0 {
        return true;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    name.starts_with("~$")
        || name.starts_with('.')
        || name == COMBINED_DIR
        || name == "thumbs.db"
        || name == "desktop.ini"
        || [".tmp", ".part", ".crdownload", ".partial"]
            .iter()
            .any(|ext| name.ends_with(ext))
}

// ---------------------------------------------------------------------------
// Stability tracking (no threads, no timers: the worker drives it)

/// What a probe found at a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seen {
    Missing,
    NotAFile,
    File(u64),
}

#[derive(Debug)]
struct Candidate {
    source: Source,
    last_size: Option<u64>,
}

/// Candidates waiting to become stable, and paths already queued.
#[derive(Debug, Default)]
pub struct Tracker {
    candidates: HashMap<PathBuf, Candidate>,
    emitted: HashSet<PathBuf>,
}

impl Tracker {
    /// Starts watching `path` unless it is already a candidate or already queued. `seen` is the
    /// first observation, so the next tick can already confirm stability.
    pub fn offer(&mut self, path: PathBuf, source: Source, seen: Seen) {
        if self.emitted.contains(&path) || self.candidates.contains_key(&path) {
            return;
        }
        let last_size = match seen {
            Seen::File(s) => Some(s),
            Seen::Missing | Seen::NotAFile => return,
        };
        self.candidates
            .insert(path, Candidate { source, last_size });
    }

    /// One stability check. A candidate is ready when its size is non-zero and unchanged since
    /// the previous check, and `unlocked` (exclusive open) succeeds. `unlocked` is only tried
    /// for stable files, so a writer is not disturbed while it is still writing.
    pub fn tick(
        &mut self,
        seen: impl Fn(&Path) -> Seen,
        unlocked: impl Fn(&Path) -> bool,
    ) -> Vec<(PathBuf, Source, u64)> {
        let mut ready = Vec::new();
        let mut gone = Vec::new();
        for (path, c) in &mut self.candidates {
            match seen(path) {
                Seen::Missing | Seen::NotAFile => gone.push(path.clone()),
                Seen::File(size) => {
                    if size > 0 && c.last_size == Some(size) && unlocked(path) {
                        ready.push((path.clone(), c.source.clone(), size));
                    } else {
                        c.last_size = Some(size);
                    }
                }
            }
        }
        for p in gone {
            self.candidates.remove(&p);
        }
        for (p, _, _) in &ready {
            self.candidates.remove(p);
            self.emitted.insert(p.clone());
        }
        ready.sort_by(|a, b| a.0.cmp(&b.0));
        ready
    }

    pub fn was_queued(&self, path: &Path) -> bool {
        self.emitted.contains(path)
    }

    /// Lets `path` be queued again (it left the queue).
    pub fn forget(&mut self, path: &Path) {
        self.emitted.remove(path);
        self.candidates.remove(path);
    }

    /// Forgets every candidate and queued path for which `drop` is true; returns the queued ones.
    pub fn drop_where(&mut self, drop: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
        self.candidates.retain(|p, _| !drop(p));
        let gone: Vec<PathBuf> = self.emitted.iter().filter(|p| drop(p)).cloned().collect();
        for p in &gone {
            self.emitted.remove(p);
        }
        gone
    }

    /// Queued paths that no longer exist; they are forgotten and returned.
    pub fn prune(&mut self, exists: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
        let gone: Vec<PathBuf> = self
            .emitted
            .iter()
            .filter(|p| !exists(p))
            .cloned()
            .collect();
        for p in &gone {
            self.emitted.remove(p);
        }
        gone
    }

    #[cfg(test)]
    fn waiting(&self) -> usize {
        self.candidates.len()
    }
}

fn probe(path: &Path) -> Seen {
    match std::fs::metadata(path) {
        Ok(m) if m.is_file() => Seen::File(m.len()),
        Ok(_) => Seen::NotAFile,
        Err(_) => Seen::Missing,
    }
}

/// Opening with no sharing fails while any other process has the file open.
fn exclusive_open(path: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .is_ok()
}

/// Every file under `dir` down to `max_depth` folder levels (0 = only `dir` itself), skipping
/// hidden/system folders and skipped files.
fn walk_files(dir: &Path, depth: usize, max_depth: usize, out: &mut Vec<PathBuf>) {
    if depth > max_depth {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if skipped(&path, &meta) {
            continue;
        }
        if meta.is_dir() {
            walk_files(&path, depth + 1, max_depth, out);
        } else if meta.is_file() {
            out.push(path);
        }
    }
}

/// Which inbox `path` is in (by its canonical path, `canon[i]` for `inboxes[i]`), and whether it
/// is in a sub-folder of that inbox rather than directly in it.
fn locate(inboxes: &[PathBuf], canon: &[PathBuf], path: &Path) -> (Source, bool) {
    let c = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    match canon.iter().position(|i| c.starts_with(i)) {
        Some(i) => (
            Source::Inbox(inboxes[i].clone()),
            c.parent() != Some(canon[i].as_path()),
        ),
        None => (Source::Manual, false),
    }
}

// ---------------------------------------------------------------------------
// Worker

enum Msg {
    /// The inboxes, and whether their sub-folders count.
    SetInboxes(Vec<PathBuf>, bool),
    Add(Vec<PathBuf>),
    Forget(PathBuf),
    Fs(notify_debouncer_mini::DebounceEventResult),
    Stop,
}

/// What the worker reports to the UI.
#[derive(Debug)]
pub enum Event {
    Queued(QueuedFile),
    /// A queued file disappeared (moved or deleted outside the app).
    Gone(PathBuf),
    /// Inboxes now being watched, and the ones that could not be watched with why.
    Watching {
        ok: Vec<PathBuf>,
        failed: Vec<(PathBuf, String)>,
    },
}

struct Worker {
    inboxes: Vec<PathBuf>,
    /// `inboxes`, canonicalized once when they are set.
    canon: Vec<PathBuf>,
    subfolders: bool,
    watchers:
        Vec<notify_debouncer_mini::Debouncer<notify_debouncer_mini::notify::RecommendedWatcher>>,
    tracker: Tracker,
    self_tx: Sender<Msg>,
    out: Sender<Event>,
    ctx: egui::Context,
}

impl Worker {
    fn send(&self, e: Event) {
        let _ = self.out.send(e);
        self.ctx.request_repaint();
    }

    /// The inbox `path` is in, if any, and whether it is in one of its sub-folders.
    fn locate(&self, path: &Path) -> (Source, bool) {
        locate(&self.inboxes, &self.canon, path)
    }

    fn offer_file(&mut self, path: PathBuf, source: Source) {
        if let Ok(meta) = std::fs::metadata(&path)
            && !skipped(&path, &meta)
        {
            let seen = probe(&path);
            self.tracker.offer(path, source, seen);
        }
    }

    fn offer_tree(&mut self, path: &Path, source: Source, max_depth: usize) {
        let mut files = Vec::new();
        walk_files(path, 0, max_depth, &mut files);
        for f in files {
            self.offer_file(f, source.clone());
        }
    }

    fn list_inboxes(&mut self) {
        let depth = if self.subfolders { MAX_DEPTH } else { 0 };
        for inbox in self.inboxes.clone() {
            self.offer_tree(&inbox, Source::Inbox(inbox.clone()), depth);
        }
    }

    fn set_inboxes(&mut self, inboxes: Vec<PathBuf>, subfolders: bool) {
        self.watchers.clear();
        self.canon = inboxes
            .iter()
            .map(|i| std::fs::canonicalize(i).unwrap_or_else(|_| i.clone()))
            .collect();
        self.inboxes = inboxes;
        self.subfolders = subfolders;
        if !subfolders {
            // Files already found in sub-folders leave the queue.
            let (inboxes, canon) = (&self.inboxes, &self.canon);
            let gone = self.tracker.drop_where(|p| locate(inboxes, canon, p).1);
            for p in gone {
                self.send(Event::Gone(p));
            }
        }
        let mode = if subfolders {
            notify_debouncer_mini::notify::RecursiveMode::Recursive
        } else {
            notify_debouncer_mini::notify::RecursiveMode::NonRecursive
        };
        let mut ok = Vec::new();
        let mut failed = Vec::new();
        for inbox in self.inboxes.clone() {
            let tx = self.self_tx.clone();
            let watch = notify_debouncer_mini::new_debouncer(DEBOUNCE, move |r| {
                let _ = tx.send(Msg::Fs(r));
            })
            .and_then(|mut d| {
                d.watcher().watch(&inbox, mode)?;
                Ok(d)
            });
            match watch {
                Ok(d) => {
                    self.watchers.push(d);
                    ok.push(inbox);
                }
                Err(e) => {
                    tracing::warn!(inbox = %inbox.display(), error = %e, "cannot watch inbox");
                    failed.push((inbox, e.to_string()));
                }
            }
        }
        tracing::info!(watching = ok.len(), failed = failed.len(), "inboxes set");
        self.send(Event::Watching { ok, failed });
        self.list_inboxes();
    }

    fn on_fs(&mut self, events: Vec<notify_debouncer_mini::DebouncedEvent>) {
        for ev in events {
            let path = ev.path;
            if in_combined_dir(&path) {
                continue;
            }
            match std::fs::metadata(&path) {
                Ok(m) if m.is_dir() || m.is_file() => {
                    let (source, in_sub) = self.locate(&path);
                    // Sub-folders off: only files directly in an inbox.
                    if !self.subfolders
                        && matches!(source, Source::Inbox(_))
                        && (m.is_dir() || in_sub)
                    {
                        continue;
                    }
                    if m.is_dir() {
                        self.offer_tree(&path, source, MAX_DEPTH);
                    } else {
                        self.offer_file(path, source);
                    }
                }
                Ok(_) => {}
                Err(_) => {
                    if self.tracker.was_queued(&path) {
                        self.tracker.forget(&path);
                        self.send(Event::Gone(path));
                    }
                }
            }
        }
    }

    fn tick(&mut self) {
        for (path, source, size) in self.tracker.tick(probe, exclusive_open) {
            let kind = kind_of(&path);
            self.send(Event::Queued(QueuedFile {
                path,
                size,
                added: Local::now(),
                source,
                kind,
            }));
        }
    }

    fn recheck(&mut self) {
        for gone in self.tracker.prune(|p| p.exists()) {
            self.send(Event::Gone(gone));
        }
        self.list_inboxes();
    }

    fn run(mut self, rx: Receiver<Msg>) {
        let mut next_tick = Instant::now() + TICK;
        let mut next_recheck = Instant::now() + RECHECK;
        loop {
            let wait = next_tick.saturating_duration_since(Instant::now());
            match rx.recv_timeout(wait) {
                Ok(Msg::SetInboxes(v, sub)) => self.set_inboxes(v, sub),
                Ok(Msg::Add(paths)) => {
                    for p in paths {
                        let (source, _) = self.locate(&p);
                        if p.is_dir() {
                            self.offer_tree(&p, source, MAX_DEPTH);
                        } else {
                            self.offer_file(p, source);
                        }
                    }
                }
                Ok(Msg::Forget(p)) => self.tracker.forget(&p),
                Ok(Msg::Fs(Ok(events))) => self.on_fs(events),
                Ok(Msg::Fs(Err(e))) => tracing::warn!(error = %e, "inbox watcher error"),
                Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {}
            }
            let now = Instant::now();
            if now >= next_tick {
                self.tick();
                next_tick = now + TICK;
            }
            if now >= next_recheck {
                self.recheck();
                next_recheck = now + RECHECK;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// UI side

/// The queue the Preview tab shows, fed by the worker thread.
pub struct IntakeManager {
    tx: Option<Sender<Msg>>,
    rx: Option<Receiver<Event>>,
    inboxes: Option<(Vec<PathBuf>, bool)>,
    watching: Vec<PathBuf>,
    watch_errors: Vec<(PathBuf, String)>,
    queue: Vec<QueuedFile>,
    unsupported: Vec<QueuedFile>,
    /// Zips waiting on the Import tab (P07b).
    zips: Vec<QueuedFile>,
}

impl IntakeManager {
    /// Not running until [`Self::start`].
    pub fn new() -> Self {
        Self {
            tx: None,
            rx: None,
            inboxes: None,
            watching: Vec::new(),
            watch_errors: Vec::new(),
            queue: Vec::new(),
            unsupported: Vec::new(),
            zips: Vec::new(),
        }
    }

    pub fn start(&mut self, ctx: &egui::Context) {
        if self.tx.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let (out_tx, out_rx) = channel();
        let worker = Worker {
            inboxes: Vec::new(),
            canon: Vec::new(),
            subfolders: true,
            watchers: Vec::new(),
            tracker: Tracker::default(),
            self_tx: tx.clone(),
            out: out_tx,
            ctx: ctx.clone(),
        };
        std::thread::Builder::new()
            .name("intake".into())
            .spawn(move || worker.run(rx))
            .expect("spawn intake thread");
        self.tx = Some(tx);
        self.rx = Some(out_rx);
    }

    fn send(&self, msg: Msg) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(msg);
        }
    }

    /// Watches these inboxes, with their sub-folders when `subfolders` (restarting the watchers
    /// only when something changed).
    pub fn set_inboxes(&mut self, inboxes: &[PathBuf], subfolders: bool) {
        if self
            .inboxes
            .as_ref()
            .is_some_and(|(v, s)| v == inboxes && *s == subfolders)
        {
            return;
        }
        self.inboxes = Some((inboxes.to_vec(), subfolders));
        self.send(Msg::SetInboxes(inboxes.to_vec(), subfolders));
    }

    /// Files or folders (recursively) from drag-drop or Browse.
    pub fn add_paths(&mut self, paths: Vec<PathBuf>) {
        if !paths.is_empty() {
            self.send(Msg::Add(paths));
        }
    }

    /// Takes worker events. Call every frame.
    pub fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        let events: Vec<Event> = rx.try_iter().collect();
        for e in events {
            match e {
                Event::Queued(f) => {
                    tracing::info!(file = %f.path.display(), kind = ?f.kind, "queued");
                    let list = match f.kind {
                        Kind::Pdf => &mut self.queue,
                        Kind::Zip => &mut self.zips,
                        Kind::Unsupported => &mut self.unsupported,
                    };
                    if !list.iter().any(|q| q.path == f.path) {
                        list.push(f);
                    }
                }
                Event::Gone(p) => {
                    self.queue.retain(|q| q.path != p);
                    self.unsupported.retain(|q| q.path != p);
                    self.zips.retain(|q| q.path != p);
                }
                Event::Watching { ok, failed } => {
                    self.watching = ok;
                    self.watch_errors = failed;
                }
            }
        }
    }

    pub fn queue(&self) -> &[QueuedFile] {
        &self.queue
    }

    pub fn unsupported(&self) -> &[QueuedFile] {
        &self.unsupported
    }

    /// Files extracted from a zip (Send to queue, P07b), queued as they are (any type).
    pub fn add_routed(&mut self, files: Vec<QueuedFile>) {
        for f in files {
            if !self.queue.iter().any(|q| q.path == f.path) {
                self.queue.push(f);
            }
        }
    }

    /// Takes a zip-routed file out of the queue (the user removed it; its temp file stays).
    pub fn remove_routed(&mut self, path: &Path) {
        self.queue.retain(|q| q.path != path);
    }

    /// Zips waiting on the Import tab, oldest first.
    pub fn zips(&self) -> &[QueuedFile] {
        &self.zips
    }

    /// A zip was imported or its draft discarded: no longer listed. A discarded zip that stays in
    /// the inbox is not listed again this session.
    pub fn drop_zip(&mut self, path: &Path) {
        self.zips.retain(|q| q.path != path);
    }

    /// A zip sent to the queue whose files all left it unfiled: listed on Import again.
    pub fn relist_zip(&mut self, file: QueuedFile) {
        if !self.zips.iter().any(|q| q.path == file.path) {
            self.zips.push(file);
        }
    }

    pub fn watching(&self) -> &[PathBuf] {
        &self.watching
    }

    pub fn watch_errors(&self) -> &[(PathBuf, String)] {
        &self.watch_errors
    }

    /// Hides an unsupported file for this session. It stays known to the worker, so it is not
    /// listed again unless it changes path.
    pub fn ignore_unsupported(&mut self, path: &Path) {
        self.unsupported.retain(|q| q.path != path);
    }

    /// Takes a file out of the queue and lets it be queued again later (used after commit/skip).
    pub fn remove(&mut self, path: &Path) {
        self.queue.retain(|q| q.path != path);
        self.send(Msg::Forget(path.to_path_buf()));
    }
}

impl Default for IntakeManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for IntakeManager {
    fn drop(&mut self) {
        self.send(Msg::Stop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::io::Write;

    #[test]
    fn tracker_needs_two_equal_sizes_and_unlocked() {
        let mut t = Tracker::default();
        let size = RefCell::new(100u64);
        let locked = RefCell::new(false);
        t.offer("a.pdf".into(), Source::Manual, Seen::File(50));
        // Grew since the first look: not ready, remembers 100.
        let r = t.tick(|_| Seen::File(*size.borrow()), |_| !*locked.borrow());
        assert!(r.is_empty());
        // Same size but still open by the writer: not ready.
        *locked.borrow_mut() = true;
        assert!(
            t.tick(|_| Seen::File(*size.borrow()), |_| !*locked.borrow())
                .is_empty()
        );
        // Released: ready, once.
        *locked.borrow_mut() = false;
        let r = t.tick(|_| Seen::File(*size.borrow()), |_| !*locked.borrow());
        assert_eq!(r, vec![(PathBuf::from("a.pdf"), Source::Manual, 100)]);
        assert!(t.tick(|_| Seen::File(100), |_| true).is_empty());
        // Already queued: offering again does nothing until forgotten.
        t.offer("a.pdf".into(), Source::Manual, Seen::File(100));
        assert_eq!(t.waiting(), 0);
        t.forget(Path::new("a.pdf"));
        t.offer("a.pdf".into(), Source::Manual, Seen::File(100));
        assert_eq!(t.waiting(), 1);
    }

    #[test]
    fn tracker_drops_missing_and_empty_files_wait() {
        let mut t = Tracker::default();
        t.offer("gone.pdf".into(), Source::Manual, Seen::File(10));
        t.offer("empty.pdf".into(), Source::Manual, Seen::File(0));
        t.offer("dir".into(), Source::Manual, Seen::NotAFile);
        let r = t.tick(
            |p| {
                if p == Path::new("gone.pdf") {
                    Seen::Missing
                } else {
                    Seen::File(0)
                }
            },
            |_| true,
        );
        assert!(r.is_empty());
        assert_eq!(t.waiting(), 1, "only the empty file is still waiting");
    }

    #[test]
    fn prune_reports_vanished_queued_files() {
        let mut t = Tracker::default();
        t.offer("x.pdf".into(), Source::Manual, Seen::File(5));
        t.tick(|_| Seen::File(5), |_| true);
        assert_eq!(t.prune(|_| true), Vec::<PathBuf>::new());
        assert_eq!(t.prune(|_| false), vec![PathBuf::from("x.pdf")]);
        assert!(!t.was_queued(Path::new("x.pdf")));
    }

    #[test]
    fn kinds_and_skips() {
        assert_eq!(kind_of(Path::new("a.PDF")), Kind::Pdf);
        assert_eq!(kind_of(Path::new("a.docx")), Kind::Unsupported);
        assert_eq!(kind_of(Path::new("noext")), Kind::Unsupported);
    }

    fn manager(ctx: &egui::Context, inboxes: &[PathBuf]) -> IntakeManager {
        let mut m = IntakeManager::new();
        m.start(ctx);
        m.set_inboxes(inboxes, true);
        m
    }

    /// Polls until `done` or `limit`; returns the time it took.
    fn wait_for(
        m: &mut IntakeManager,
        limit: Duration,
        done: impl Fn(&IntakeManager) -> bool,
    ) -> Option<Duration> {
        let start = Instant::now();
        while start.elapsed() < limit {
            m.poll();
            if done(m) {
                return Some(start.elapsed());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }

    fn queued(m: &IntakeManager, name: &str) -> bool {
        m.queue()
            .iter()
            .any(|q| q.path.file_name().unwrap() == name)
    }

    #[test]
    fn existing_and_new_inbox_files_are_queued_within_3s() {
        let tmp = tempfile::tempdir().unwrap();
        let inbox = tmp.path().join("inbox");
        std::fs::create_dir_all(inbox.join("sub")).unwrap();
        std::fs::write(inbox.join("old.pdf"), b"%PDF-1.4 old").unwrap();
        std::fs::write(inbox.join("sub").join("deep.pdf"), b"%PDF-1.4 deep").unwrap();
        std::fs::write(inbox.join("notes.txt"), b"hello").unwrap();
        std::fs::write(inbox.join("~$lock.pdf"), b"x").unwrap();
        let ctx = egui::Context::default();
        let mut m = manager(&ctx, std::slice::from_ref(&inbox));

        wait_for(&mut m, Duration::from_secs(5), |m| {
            queued(m, "old.pdf") && queued(m, "deep.pdf") && !m.unsupported().is_empty()
        })
        .expect("existing files queued");
        assert_eq!(m.watching(), std::slice::from_ref(&inbox));
        assert!(!queued(&m, "~$lock.pdf"));
        assert_eq!(m.unsupported()[0].path, inbox.join("notes.txt"));
        assert_eq!(m.queue()[0].source, Source::Inbox(inbox.clone()));

        // Done-when: a new PDF dropped into the inbox shows up within 3 s.
        std::fs::write(inbox.join("new.pdf"), b"%PDF-1.4 new").unwrap();
        let took = wait_for(&mut m, Duration::from_secs(6), |m| queued(m, "new.pdf"))
            .expect("new file queued");
        assert!(took < Duration::from_secs(3), "took {took:?}");

        // Removed outside the app: leaves the queue.
        std::fs::remove_file(inbox.join("new.pdf")).unwrap();
        wait_for(&mut m, Duration::from_secs(5), |m| !queued(m, "new.pdf")).expect("gone");
    }

    #[test]
    fn file_being_written_is_not_queued_until_complete() {
        let tmp = tempfile::tempdir().unwrap();
        let inbox = tmp.path().join("inbox");
        std::fs::create_dir_all(&inbox).unwrap();
        let ctx = egui::Context::default();
        let mut m = manager(&ctx, std::slice::from_ref(&inbox));
        wait_for(&mut m, Duration::from_secs(3), |m| !m.watching().is_empty()).unwrap();

        let path = inbox.join("slow.pdf");
        let mut f = std::fs::File::create(&path).unwrap();
        for _ in 0..4 {
            f.write_all(&[b'x'; 1000]).unwrap();
            f.flush().unwrap();
            std::thread::sleep(Duration::from_millis(700));
            m.poll();
            assert!(!queued(&m, "slow.pdf"), "queued while still being written");
        }
        // Size now stable, but the writer still holds the file open.
        std::thread::sleep(Duration::from_millis(2500));
        m.poll();
        assert!(
            !queued(&m, "slow.pdf"),
            "queued while the writer still had it open"
        );
        drop(f);
        let took = wait_for(&mut m, Duration::from_secs(5), |m| queued(m, "slow.pdf"))
            .expect("queued after the writer closed it");
        assert!(took < Duration::from_secs(3), "took {took:?}");
        assert_eq!(m.queue()[0].size, 4000);
    }

    #[test]
    fn manual_folder_add_is_recursive_and_marked_manual() {
        let tmp = tempfile::tempdir().unwrap();
        let inbox = tmp.path().join("inbox");
        let elsewhere = tmp.path().join("desktop");
        std::fs::create_dir_all(&inbox).unwrap();
        std::fs::create_dir_all(elsewhere.join("batch")).unwrap();
        std::fs::write(elsewhere.join("a.pdf"), b"%PDF a").unwrap();
        std::fs::write(elsewhere.join("batch").join("b.pdf"), b"%PDF b").unwrap();
        std::fs::write(inbox.join("in.pdf"), b"%PDF in").unwrap();
        let ctx = egui::Context::default();
        let mut m = manager(&ctx, std::slice::from_ref(&inbox));
        m.add_paths(vec![elsewhere.clone(), inbox.join("in.pdf")]);
        wait_for(&mut m, Duration::from_secs(5), |m| m.queue().len() == 3).expect("all queued");
        let source = |name: &str| {
            m.queue()
                .iter()
                .find(|q| q.path.file_name().unwrap() == name)
                .unwrap()
                .source
                .clone()
        };
        assert_eq!(source("a.pdf"), Source::Manual);
        assert_eq!(source("b.pdf"), Source::Manual);
        assert_eq!(source("in.pdf"), Source::Inbox(inbox.clone()));
        // Adding the same files again does not duplicate them.
        m.add_paths(vec![elsewhere.join("a.pdf")]);
        std::thread::sleep(Duration::from_millis(2200));
        m.poll();
        assert_eq!(m.queue().len(), 3);
        // Once removed (filed or skipped), the same path can be queued again.
        m.remove(&elsewhere.join("a.pdf"));
        assert_eq!(m.queue().len(), 2);
        m.add_paths(vec![elsewhere.join("a.pdf")]);
        wait_for(&mut m, Duration::from_secs(5), |m| m.queue().len() == 3).expect("re-queued");
    }

    /// Sub-folders off: only files directly in the inbox; turning it off drops sub-folder files
    /// already queued, turning it back on finds them again.
    #[test]
    fn subfolders_off_takes_only_top_level_files() {
        let tmp = tempfile::tempdir().unwrap();
        let inbox = tmp.path().join("inbox");
        std::fs::create_dir_all(inbox.join("sub")).unwrap();
        std::fs::write(inbox.join("top.pdf"), b"%PDF-1.4 top").unwrap();
        std::fs::write(inbox.join("sub").join("deep.pdf"), b"%PDF-1.4 deep").unwrap();
        let ctx = egui::Context::default();
        let mut m = manager(&ctx, std::slice::from_ref(&inbox));
        wait_for(&mut m, Duration::from_secs(5), |m| {
            queued(m, "top.pdf") && queued(m, "deep.pdf")
        })
        .expect("both queued with sub-folders on");

        m.set_inboxes(std::slice::from_ref(&inbox), false);
        wait_for(&mut m, Duration::from_secs(5), |m| !queued(m, "deep.pdf"))
            .expect("sub-folder file left the queue");
        assert!(queued(&m, "top.pdf"));

        // New files: one in a new sub-folder (ignored), one at the top (queued).
        std::fs::create_dir_all(inbox.join("later")).unwrap();
        std::fs::write(inbox.join("later").join("skip.pdf"), b"%PDF-1.4 skip").unwrap();
        std::fs::write(inbox.join("sub").join("skip2.pdf"), b"%PDF-1.4 skip2").unwrap();
        std::fs::write(inbox.join("new.pdf"), b"%PDF-1.4 new").unwrap();
        wait_for(&mut m, Duration::from_secs(6), |m| queued(m, "new.pdf")).expect("top file");
        std::thread::sleep(Duration::from_millis(2500));
        m.poll();
        assert!(!queued(&m, "skip.pdf") && !queued(&m, "skip2.pdf") && !queued(&m, "deep.pdf"));

        m.set_inboxes(std::slice::from_ref(&inbox), true);
        wait_for(&mut m, Duration::from_secs(5), |m| {
            queued(m, "deep.pdf") && queued(m, "skip.pdf") && queued(m, "skip2.pdf")
        })
        .expect("sub-folder files back with sub-folders on");
    }

    #[test]
    fn missing_inbox_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope");
        let ctx = egui::Context::default();
        let mut m = manager(&ctx, std::slice::from_ref(&missing));
        wait_for(&mut m, Duration::from_secs(3), |m| {
            !m.watch_errors().is_empty()
        })
        .expect("watch error reported");
        assert_eq!(m.watch_errors()[0].0, missing);
    }
}
