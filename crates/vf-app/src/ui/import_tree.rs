//! Admin › Folder import (PHASES P10): adopt the cabinet's existing folders in place. Scan
//! proposes a client for each top-level folder and a doc type and year for each file
//! (`vf_fs::import::plan`); the person reviews the folders, then Import binds them, catalogs,
//! sets the metadata and queues OCR on a background thread (its own catalog connection). The
//! remainder is listed and exported as CSV for handling by hand. Undo this import reverses it.

use std::path::Path;
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::RichText;
use vf_core::db::Storage;
use vf_fs::import::{self, Applied, ClientPick, Options, Plan};

use crate::ui::browse::Env;
use crate::ui::{theme, widgets};

/// Rows of the remainder shown on the page (the CSV has all of them).
const SHOWN: usize = 300;

/// Files an import reads the text of by default (PA-34: not ~900K files at once).
const READ_UP_TO: usize = 50_000;

/// What Scan and Undo give back from their thread (owner 2026-11-12: no frozen window).
enum Side {
    /// The folders and their clients (the files come next).
    Scanned(Result<(Plan, Vec<(String, String)>), String>),
    /// Every file, for the folders scanned.
    Files(Result<import::Files, String>),
    /// The kept scan's files, read from the PC (`None`: not there, or not this scan's).
    KeptFiles(Option<Vec<import::FileRow>>),
    Undone(Result<(), String>),
    /// The work could not start (the catalog would not open).
    Failed(String),
}

/// The scan kept on this PC (owner 2026-11-12): the folders with the person's choices (small,
/// saved as they change) and the files (large, saved once when read).
const SAVED_FOLDERS: &str = "folder-import-folders";
const SAVED_FILES: &str = "folder-import-files";

#[derive(serde::Serialize, serde::Deserialize)]
struct SavedFolders {
    /// This scan's id: its files part is used only when it carries the same (QA 2026-11-12).
    scan_id: String,
    root: std::path::PathBuf,
    at: String,
    folders: Vec<import::FolderRow>,
    clients: Vec<import::ClientPick>,
    skipped: Vec<String>,
    files_read: bool,
    rename: bool,
    default_type: Option<String>,
    default_source: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SavedFiles {
    scan_id: String,
    files: Vec<import::FileRow>,
}

/// What a running scan has read: Filer's folder count (sentinels), then files by folder.
#[derive(Default)]
struct ScanCounts {
    folders: filer_intake::sentinel::Progress,
    files: import::ScanProgress,
}

#[derive(Default)]
pub struct ImportPage {
    pub plan: Option<Plan>,
    pub rename: bool,
    /// For files the scan found no doc type for (owner 2026-11-05): a doc type (id) and a
    /// source to give them.
    pub default_type: Option<String>,
    pub default_source: Option<String>,
    /// The cabinet's doc types (id, name), read at the scan.
    types: Vec<(String, String)>,
    /// A new client's ID being typed on its row (folder row → text).
    id_drafts: std::collections::HashMap<usize, String>,
    /// Folders shown: a match of at least this many % (PA-28).
    pub min_pct: u8,
    /// Folders already bound are not shown.
    pub hide_bound: bool,
    /// Folders with no client are not shown.
    pub hide_none: bool,
    /// Only folders whose client has another folder (owner 2026-11-12: "3 Way" and "3-Way"),
    /// to set Merge or New client on them.
    pub only_shared: bool,
    /// The row whose Bind box was clicked last, for Shift-click ranges.
    last_tick: Option<usize>,
    job: Option<Receiver<Result<Applied, String>>>,
    /// A scan or an undo running on its thread, and what it says while it runs.
    side: Option<(&'static str, Receiver<Side>)>,
    /// The running scan's counts, and its Stop.
    counts: std::sync::Arc<ScanCounts>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// The kept scan: looked for once; when it was made; what was saved last (not saved again
    /// unchanged), checked every few seconds.
    saved_looked: bool,
    pub scanned_at: Option<String>,
    /// The scan on screen, and whether its files part is saved on the PC yet.
    scan_id: Option<String>,
    files_saved: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// The import is done: the kept scan is removed (where the catalog is known).
    forget_on_poll: bool,
    saved_folders: Option<String>,
    saved_checked: Option<std::time::Instant>,
    /// Read the text of the imported PDFs (OCR); unset = yes up to [`READ_UP_TO`] files.
    read_text: Option<bool>,
    /// Folders done of the running import, and how many it has (PA-34).
    progress: std::sync::Arc<(
        std::sync::atomic::AtomicUsize,
        std::sync::atomic::AtomicUsize,
    )>,
    /// The last import on this cabinet (from `_Vibe/import_last.json`).
    pub last: Option<Applied>,
    loaded_last: bool,
    pub notice: Option<String>,
    pub error: Option<String>,
    /// The catalog changed: Browse re-reads.
    pub changed: bool,
}

impl ImportPage {
    pub fn running(&self) -> bool {
        self.job.is_some() || self.side.is_some()
    }

    /// Runs `work` on its own thread with its own catalog connection (in tests, at once);
    /// [`Self::poll_side`] takes the result.
    fn run_side(
        &mut self,
        ctx: &egui::Context,
        what: &'static str,
        catalog: &Path,
        work: impl FnOnce(&mut vf_core::db::Connection) -> Side + Send + 'static,
    ) {
        self.error = None;
        self.notice = None;
        let (tx, rx) = std::sync::mpsc::channel();
        let (catalog, ctx2) = (catalog.to_path_buf(), ctx.clone());
        let job = move || {
            let r = match vf_core::db::open(&catalog, Storage::Local) {
                Ok(mut conn) => work(&mut conn),
                Err(e) => Side::Failed(e.to_string()),
            };
            let _ = tx.send(r);
            ctx2.request_repaint();
        };
        if cfg!(test) {
            job();
        } else if let Err(e) = std::thread::Builder::new()
            .name("folder-import-side".into())
            .spawn(job)
        {
            self.error = Some(format!("{what} could not start: {e}"));
            return;
        }
        self.side = Some((what, rx));
    }

    fn scan(&mut self, ctx: &egui::Context, env: &Env, catalog: &Path) {
        // The kept scan is replaced only once the new one has its folders (QA 2026-11-12: a
        // failed scan would have lost hours of ticks).
        let root = env.root.to_path_buf();
        self.counts = Default::default();
        self.stop = Default::default();
        let counts = self.counts.clone();
        self.run_side(ctx, "Reading the client folders", catalog, move |conn| {
            Side::Scanned(import::plan_folders(conn, &root, &counts.folders).map(|p| {
                let types = vf_fs::doctypes::load(&root)
                    .ok()
                    .flatten()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|t| !t.archived)
                    .map(|t| (t.id, t.name))
                    .collect();
                (p, types)
            }))
        });
    }

    /// The scan's second part: the files, several folders at a time, with Stop.
    fn read_files(&mut self, ctx: &egui::Context, env: &Env, catalog: &Path) {
        let Some(plan) = self.plan.clone() else {
            return;
        };
        let root = env.root.to_path_buf();
        let (counts, stop) = (self.counts.clone(), self.stop.clone());
        self.run_side(ctx, "Reading the files", catalog, move |conn| {
            Side::Files(import::read_files(conn, &root, &plan, &counts.files, &stop))
        });
    }

    fn undo(&mut self, ctx: &egui::Context, env: &Env, catalog: &Path, a: Applied) {
        let (root, device) = (env.root.to_path_buf(), env.device_id.to_string());
        self.run_side(ctx, "Undoing the last import…", catalog, move |conn| {
            Side::Undone(import::undo(conn, &root, &a, &device))
        });
    }

    fn poll_side(&mut self, ctx: &egui::Context, env: &Env, catalog: &Path) {
        let Some((_, rx)) = &self.side else { return };
        let r = match rx.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Side::Undone(Err("the work stopped".into())),
        };
        self.side = None;
        match r {
            Side::Scanned(Ok((p, types))) => {
                forget_saved(catalog);
                self.plan = Some(p);
                self.last_tick = None;
                self.id_drafts.clear();
                self.types = types;
                self.scanned_at = Some(vf_core::now_rfc3339());
                self.scan_id = Some(vf_core::new_id());
                self.files_saved = Default::default();
                self.saved_folders = None;
                // The folders are on screen: the files are read behind them.
                self.read_files(ctx, env, catalog);
            }
            Side::Scanned(Err(e)) => self.error = Some(format!("Scan failed: {e}")),
            Side::Files(Ok(files)) => {
                if !files.unreadable.is_empty() {
                    self.notice = Some(format!(
                        "{} folders could not be read and were skipped (their files are not in the scan): {}",
                        files.unreadable.len(),
                        files
                            .unreadable
                            .iter()
                            .take(3)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join("; ")
                    ));
                }
                if let (Some(plan), Some(id)) = (self.plan.as_mut(), self.scan_id.clone()) {
                    files.apply(plan);
                    save_files(catalog, &id, plan, self.files_saved.clone());
                    self.saved_folders = None;
                }
            }
            Side::KeptFiles(Some(files)) => {
                if let Some(plan) = self.plan.as_mut() {
                    let n = plan.folders.len();
                    let mut counts = vec![0; n];
                    // Rows of folders that are not in this scan are dropped (never applied to
                    // the wrong folder).
                    let files: Vec<import::FileRow> = files
                        .into_iter()
                        .filter(|f| f.folder.is_none_or(|i| i < n))
                        .collect();
                    for f in &files {
                        if let Some(i) = f.folder {
                            counts[i] += 1;
                        }
                    }
                    import::Files {
                        files,
                        counts,
                        unreadable: vec![],
                    }
                    .apply(plan);
                    self.files_saved
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    self.saved_folders = None;
                }
            }
            Side::KeptFiles(None) => {
                self.notice = Some(
                    "The kept scan's files are not on this PC: Read the files again to import."
                        .into(),
                );
            }
            Side::Failed(e) => self.error = Some(format!("Could not start: {e}")),
            Side::Files(Err(e)) if e == "stopped" => {
                self.notice = Some(
                    "Stopped: the folders are listed, their files were not read. Read the files to import.".into(),
                );
            }
            Side::Files(Err(e)) => self.error = Some(format!("The files could not be read: {e}")),
            Side::Undone(Ok(())) => {
                self.notice = Some("The import was undone.".into());
                self.last = None;
                self.plan = None;
                self.scanned_at = None;
                forget_saved(catalog);
                self.changed = true;
            }
            Side::Undone(Err(e)) => self.error = Some(format!("Undo failed: {e}")),
        }
    }

    fn start(&mut self, ctx: &egui::Context, catalog: &Path, root: &Path, device: &str) {
        let Some(plan) = self.plan.clone() else {
            return;
        };
        let read = self.read_text.unwrap_or(plan.files.len() <= READ_UP_TO);
        let opts = Options {
            rename: self.rename,
            default_doc_type: self.default_type.clone(),
            default_source: self.default_source.clone(),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        self.progress = Default::default();
        let progress = self.progress.clone();
        let (catalog, root, device, ctx) = (
            catalog.to_path_buf(),
            root.to_path_buf(),
            device.to_string(),
            ctx.clone(),
        );
        std::thread::Builder::new()
            .name("folder-import".into())
            .spawn(move || {
                let r = vf_core::db::open(&catalog, Storage::Local)
                    .map_err(|e| e.to_string())
                    .and_then(|mut conn| {
                        let started = vf_core::now_rfc3339();
                        // Not read: the cabinet's text recognition turns to new filings only
                        // before the import, so its files are not queued as they come in.
                        if !read
                            && vf_ocr::queue::reads(&conn).map_err(|e| e.to_string())?
                                == vf_ocr::queue::Reads::All
                        {
                            vf_ocr::queue::set_reads(
                                &conn,
                                &vf_ocr::queue::Reads::NewSince(vf_core::now_rfc3339()),
                            )
                            .map_err(|e| e.to_string())?;
                        }
                        let mut a = import::apply_with(
                            &mut conn,
                            &root,
                            &plan,
                            &opts,
                            &device,
                            &progress.0,
                            &progress.1,
                        )?;
                        a.ocr_queued = if read {
                            vf_ocr::queue::enqueue_every(&conn, vf_ocr::queue::BACKLOG, &started)
                                .unwrap_or(0)
                        } else {
                            0
                        };
                        Ok(a)
                    });
                let _ = tx.send(r);
                ctx.request_repaint();
            })
            .ok();
        self.job = Some(rx);
    }

    fn poll(&mut self) {
        let Some(rx) = &self.job else { return };
        let r = match rx.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("the import stopped".into()),
        };
        self.job = None;
        self.changed = true;
        match r {
            Ok(a) => {
                tracing::info!(
                    folders = a.bound,
                    documents = a.documents,
                    mapped = a.mapped,
                    "folder import"
                );
                self.notice = Some(format!(
                    "Imported: {}{} folders bound{}{}, {} documents, {} with client, doc type and year; {} PDFs queued for OCR.{}",
                    if a.merged > 0 {
                        format!("{} folders merged into their client's folder, ", a.merged)
                    } else {
                        String::new()
                    },
                    a.bound,
                    if a.renamed > 0 {
                        format!(" ({} renamed)", a.renamed)
                    } else {
                        String::new()
                    },
                    if a.new_clients > 0 {
                        format!(", {} new clients", a.new_clients)
                    } else {
                        String::new()
                    },
                    a.documents,
                    a.mapped,
                    a.ocr_queued,
                    if a.guessed > 0 {
                        format!(
                            " {} have a doc type or year guessed from a file or folder name (tag {}): they are not disposed of until someone sets them.",
                            a.guessed,
                            vf_fs::docmeta::GUESSED_TAG
                        )
                    } else {
                        String::new()
                    }
                ));
                self.last = Some(a);
                self.plan = None;
                self.scanned_at = None;
                self.forget_on_poll = true;
            }
            Err(e) => {
                tracing::warn!(kind = crate::logging::kind(&e), "folder import failed");
                // The cabinet may have changed since the scan (a folder bound or renamed
                // elsewhere): the kept scan goes, so Import is never tried on it again.
                self.error = Some(format!(
                    "Import failed: {e}. Scan the cabinet again before importing (folders done are kept)."
                ));
                self.plan = None;
                self.scanned_at = None;
                self.forget_on_poll = true;
                // What it did before failing (a merge, a batch) can be undone at once.
                self.loaded_last = false;
            }
        }
    }
}

impl ImportPage {
    /// The kept scan: loaded once when there is none on screen (for this cabinet), and the
    /// folders part saved again when the person's choices changed (every few seconds).
    fn keep_saved(&mut self, ctx: &egui::Context, env: &Env, catalog: &Path) {
        if std::mem::take(&mut self.forget_on_poll) {
            forget_saved(catalog);
            self.saved_folders = None;
        }
        if !std::mem::replace(&mut self.saved_looked, true)
            && self.plan.is_none()
            && let Some(saved) = load_saved(catalog, env.root)
        {
            let mut plan = import::Plan {
                folders: saved.folders,
                clients: saved.clients,
                skipped: saved.skipped,
                ..Default::default()
            };
            for r in &mut plan.folders {
                r.files = 0;
            }
            self.plan = Some(plan);
            self.scan_id = Some(saved.scan_id.clone());
            self.files_saved = Default::default();
            // The files (large) are read on a thread, the folders already on screen.
            if saved.files_read
                && let Some((_, files_path)) = saved_paths(catalog)
            {
                let id = saved.scan_id.clone();
                self.run_side(ctx, "Loading the kept scan", catalog, move |_| {
                    let files = std::fs::read(&files_path)
                        .ok()
                        .and_then(|b| serde_json::from_slice::<SavedFiles>(&b).ok())
                        .filter(|f| f.scan_id == id)
                        .map(|f| f.files);
                    Side::KeptFiles(files)
                });
            }
            self.rename = saved.rename;
            self.default_type = saved.default_type;
            self.default_source = saved.default_source;
            self.scanned_at = Some(saved.at);
            self.types = vf_fs::doctypes::load(env.root)
                .ok()
                .flatten()
                .unwrap_or_default()
                .into_iter()
                .filter(|t| !t.archived)
                .map(|t| (t.id, t.name))
                .collect();
        }
        if self
            .saved_checked
            .is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(3))
        {
            return;
        }
        self.saved_checked = Some(std::time::Instant::now());
        let (Some(plan), Some(at), Some(id)) = (&self.plan, &self.scanned_at, &self.scan_id) else {
            return;
        };
        let saved = SavedFolders {
            scan_id: id.clone(),
            root: env.root.to_path_buf(),
            at: at.clone(),
            folders: plan.folders.clone(),
            clients: plan.clients.clone(),
            skipped: plan.skipped.clone(),
            // Only once the files part is on disk (QA 2026-11-12).
            files_read: plan.files_read
                && self.files_saved.load(std::sync::atomic::Ordering::Relaxed),
            rename: self.rename,
            default_type: self.default_type.clone(),
            default_source: self.default_source.clone(),
        };
        let Ok(text) = serde_json::to_string(&saved) else {
            return;
        };
        if self.saved_folders.as_deref() != Some(text.as_str()) {
            if let Some((folders, _)) = saved_paths(catalog) {
                let _ = vf_fs::cabinet::write_replace(&folders, text.as_bytes());
            }
            self.saved_folders = Some(text);
        }
    }
}

/// Where this Windows user's kept scan is (beside the catalog; one per user, as several people
/// share one catalog on a server: QA 2026-11-12): the folders part, the files part.
fn saved_paths(catalog: &Path) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let dir = catalog.parent()?;
    let user: String = crate::config::windows_user()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    Some((
        dir.join(format!("{SAVED_FOLDERS}-{user}.json")),
        dir.join(format!("{SAVED_FILES}-{user}.json")),
    ))
}

/// The kept scan's folders part for `root`, when there is one.
fn load_saved(catalog: &Path, root: &Path) -> Option<SavedFolders> {
    let (folders, _) = saved_paths(catalog)?;
    let saved: SavedFolders = serde_json::from_slice(&std::fs::read(folders).ok()?).ok()?;
    (saved.root == root).then_some(saved)
}

/// The files part, saved once when read (on its thread: it can be large); `saved` says when
/// it is on disk, and only then does the folders part say the files are read.
fn save_files(
    catalog: &Path,
    scan_id: &str,
    plan: &Plan,
    saved: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    let Some((_, path)) = saved_paths(catalog) else {
        return;
    };
    let body = SavedFiles {
        scan_id: scan_id.to_string(),
        files: plan.files.clone(),
    };
    let write = move || {
        if let Ok(bytes) = serde_json::to_vec(&body)
            && vf_fs::cabinet::write_replace(&path, &bytes).is_ok()
        {
            saved.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    };
    if cfg!(test) {
        write();
    } else {
        let _ = std::thread::Builder::new()
            .name("folder-import-save".into())
            .spawn(write);
    }
}

/// The kept scan removed (a new scan, an import done or failed, an undo).
fn forget_saved(catalog: &Path) {
    if let Some((folders, files)) = saved_paths(catalog) {
        let _ = std::fs::remove_file(folders);
        let _ = std::fs::remove_file(files);
    }
}

/// Whether the filter shows folder row `r`: matched at least `min_pct` (bound folders always
/// pass it), and not hidden as bound or as matching no client.
fn shows(r: &import::FolderRow, min_pct: u8, hide_bound: bool, hide_none: bool) -> bool {
    if r.bound {
        return !hide_bound;
    }
    if r.client.is_none() {
        return !hide_none && min_pct == 0;
    }
    (r.confidence * 100.0).round() >= f32::from(min_pct)
}

pub fn show(ui: &mut egui::Ui, p: &mut ImportPage, env: &mut Env, catalog: &Path) {
    if !p.loaded_last {
        p.last = import::load_last(env.root);
        p.loaded_last = true;
    }
    p.poll();
    p.poll_side(ui.ctx(), env, catalog);
    p.keep_saved(ui.ctx(), env, catalog);
    ui.heading("Folder import");
    ui.label(widgets::caption(
        "Adopts the folders already in this cabinet (a FileCenter cabinet's drawers, an old firm \
         tree) as client folders, in place: files are not moved. Each top-level folder is matched \
         to a client; each file gets a doc type and tax year where its name or folder says so.",
    ));
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let scan = ui.add_enabled(!p.running(), egui::Button::new("Scan the cabinet"));
        if scan.clicked() {
            p.scan(ui.ctx(), env, catalog);
        }
        if let Some(a) = p.last.clone()
            && !p.running()
            && ui
                .button("Undo the last import")
                .on_hover_text(format!(
                    "Removes the {} sentinels it wrote (and renames back), and the doc types and years it set.{}",
                    a.bound,
                    if a.merged > 0 {
                        " Merged files go back to their folders (refused if one was changed since)."
                    } else {
                        " Files stay where they are."
                    }
                ))
                .clicked()
        {
            p.undo(ui.ctx(), env, catalog, a);
        }
        if let Some((what, _)) = &p.side {
            use std::sync::atomic::Ordering::Relaxed;
            ui.spinner();
            // How far it has got (owner 2026-11-12: about 7,000 folders).
            let (done, total) = match *what {
                "Reading the client folders" => (
                    p.counts.folders.done.load(Relaxed),
                    p.counts.folders.total.load(Relaxed),
                ),
                "Reading the files" => (
                    p.counts.files.done.load(Relaxed),
                    p.counts.files.total.load(Relaxed),
                ),
                _ => (0, 0),
            };
            ui.label(if total > 0 {
                format!("{what}: {done} of {total} folders…")
            } else {
                format!("{what}…")
            });
            if *what == "Reading the files" {
                let stopping = p.stop.load(Relaxed);
                if ui
                    .add_enabled(!stopping, egui::Button::new("Stop"))
                    .on_hover_text("The folders stay listed; their files are not read")
                    .clicked()
                {
                    p.stop.store(true, Relaxed);
                }
            }
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
        }
        // Folders listed, files not read (stopped, or a kept scan): read them now.
        if !p.running()
            && p.plan.as_ref().is_some_and(|pl| !pl.files_read)
            && ui
                .button("Read the files")
                .on_hover_text("Each folder's files, for their doc types and years; needed to import")
                .clicked()
        {
            p.counts = Default::default();
            p.stop = Default::default();
            p.read_files(ui.ctx(), env, catalog);
        }
        if let Some(at) = &p.scanned_at
            && !p.running()
        {
            ui.label(widgets::caption(format!(
                "Scan of {} (Scan the cabinet for a fresh one)",
                widgets::local_time(at)
            )));
        }
        if p.job.is_some() {
            ui.spinner();
            use std::sync::atomic::Ordering::Relaxed;
            let (done, total) = (p.progress.0.load(Relaxed), p.progress.1.load(Relaxed));
            // Inside the batch (owner 2026-11-12: "0 of 3323" for an hour looked stuck): the
            // files found and read so far.
            let rb = &vf_fs::rebuild::PROGRESS;
            let (found, read) = (rb.found.load(Relaxed), rb.read.load(Relaxed));
            let batches = total.div_ceil(import::BATCH).max(1);
            let batch = (done / import::BATCH + 1).min(batches);
            ui.label(if total == 0 {
                "Importing: binding folders, cataloging and reading files…".into()
            } else if found == 0 {
                format!(
                    "Importing batch {batch} of {batches} ({done} of {total} folders done): tying folders to clients, listing their files…"
                )
            } else {
                format!(
                    "Importing batch {batch} of {batches} ({done} of {total} folders done): {} of {} files read…",
                    read.min(found),
                    found
                )
            });
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
        }
    });
    if let Some(n) = &p.notice {
        ui.label(RichText::new(n).color(theme::ink()));
    }
    if let Some(e) = &p.error {
        ui.label(RichText::new(e).color(crate::ui::theme::error()));
    }
    let Some(plan) = p.plan.as_mut() else {
        return;
    };
    ui.add_space(8.0);
    let mapped = plan.auto_mapped();
    let total = plan.files.len();
    // Folders bound, and folders merged into another (a Merge with no other folder of its
    // client does nothing, so is not counted).
    let in_merge: std::collections::HashSet<usize> = import::merge_targets(plan)
        .into_iter()
        .flat_map(|(t, s)| std::iter::once(t).chain(s))
        .collect();
    let to_bind = plan
        .folders
        .iter()
        .enumerate()
        .filter(|(i, r)| {
            !r.bound && r.client.is_some() && (r.include || (r.merge && in_merge.contains(i)))
        })
        .count();
    // Files in bound (or to-be-bound) folders with a doc type or year to set.
    let to_type = plan
        .files
        .iter()
        .filter(|f| {
            f.folder.is_some_and(|i| {
                let r = &plan.folders[i];
                r.bound || (r.include && r.client.is_some())
            }) && ((f.doc_type.is_some() && f.type_confidence >= import::AUTO)
                || !f.year_from.is_empty()
                || p.default_type.is_some()
                || p.default_source.is_some())
        })
        .count();
    // Files the scan found no doc type for: the defaults below are for them.
    let untyped = plan
        .files
        .iter()
        .filter(|f| f.doc_type.is_none() || f.type_confidence < import::AUTO)
        .count();
    ui.label(format!(
        "{} folders ({} already bound, {} to bind) · {} files · {} fully mapped ({:.0}%) · {} for manual handling",
        plan.folders.len(),
        plan.folders.iter().filter(|r| r.bound).count(),
        to_bind,
        total,
        mapped,
        if total == 0 { 0.0 } else { mapped as f64 * 100.0 / total as f64 },
        total - mapped
    ));
    ui.add_space(6.0);

    // No client list, or folders no client matches (Q-63): point to the CSV import, and offer
    // a client made from each folder's name.
    let unmatched = plan
        .folders
        .iter()
        .filter(|r| !r.bound && r.client.is_none())
        .count();
    if plan.clients.is_empty() {
        ui.label(
            RichText::new(
                "No client list is imported yet. Import it first (Browse › Import clients CSV…) so folders are matched to your clients by name and ID, then scan again; or create clients from the folder names below.",
            )
            .color(theme::warn()),
        );
    }
    if unmatched > 0 {
        let make = ui
            .button(format!("Create a client for each unmatched folder ({unmatched})"))
            .on_hover_text(
                "The folder name becomes the client name and an ID is made from it (you can change it on the row); the clients are added when you import.",
            );
        if make.clicked() {
            p.id_drafts.clear();
            match import::new_clients_from_folders(env.root, plan) {
                Ok(n) => p.notice = Some(format!("{n} new clients proposed; check their IDs.")),
                Err(e) => p.error = Some(e),
            }
        }
    }
    // Weak matches made clients of their own, in bulk (owner 2026-11-12): under the "Show
    // matches from" %. Folders sharing a client are left to the filter below.
    let shared = import::shared_clients(plan);
    let weak = plan
        .folders
        .iter()
        .filter(|r| {
            !r.bound
                && !r.merge
                && r.client
                    .as_ref()
                    .is_some_and(|c| !c.new && !shared.contains(&c.ext_id.to_uppercase()))
                && (r.confidence * 100.0).round() < f32::from(p.min_pct)
        })
        .count();
    if p.min_pct > 0
        && weak > 0
        && ui
            .button(format!(
                "Create a client for each folder matched under {}% ({weak})",
                p.min_pct
            ))
            .on_hover_text(
                "Each such folder gets a new client named after it, with an ID made from its name (change it on the row), instead of the client it looks like. Set the % in \"Show matches from\".",
            )
            .clicked()
    {
        p.id_drafts.clear();
        match import::new_clients_under(env.root, plan, p.min_pct) {
            Ok(n) => p.notice = Some(format!("{n} new clients proposed; check their IDs.")),
            Err(e) => p.error = Some(e),
        }
    }
    // Filter by match % and kind (PA-28); ticking acts on the folders shown.
    let shared = import::shared_clients(plan);
    let in_shared = |r: &import::FolderRow| {
        r.client
            .as_ref()
            .is_some_and(|c| shared.contains(&c.ext_id.to_uppercase()))
    };
    let shown: Vec<usize> = (0..plan.folders.len())
        .filter(|&i| {
            let r = &plan.folders[i];
            if p.only_shared {
                in_shared(r)
            } else {
                shows(r, p.min_pct, p.hide_bound, p.hide_none)
            }
        })
        .collect();
    // Which folder each merge goes into, for the rows.
    let mut merge_into: std::collections::HashMap<usize, String> = Default::default();
    let mut kept: std::collections::HashSet<usize> = Default::default();
    for (t, sources) in import::merge_targets(plan) {
        kept.insert(t);
        for sidx in sources {
            merge_into.insert(sidx, plan.folders[t].rel.clone());
        }
    }
    let mut new_client_row: Option<usize> = None;
    let can_tick = |r: &import::FolderRow| !r.bound && r.client.is_some();
    ui.horizontal_wrapped(|ui| {
        ui.label("Show matches from");
        ui.add(
            egui::DragValue::new(&mut p.min_pct)
                .range(0..=100)
                .speed(1.0)
                .suffix("%"),
        )
        .on_hover_text("Only folders matched at least this well");
        ui.label("up");
        ui.checkbox(&mut p.hide_bound, "Hide already bound");
        ui.checkbox(&mut p.hide_none, "Hide folders with no match");
        ui.checkbox(&mut p.only_shared, "Only folders sharing a client")
            .on_hover_text(
                "Folders matched to a client that another folder is matched to (\"3 Way\" and \"3-Way\"): choose Merge (its files go into the client's folder: the bound one, else the one with more files) or New client on each",
            );
        ui.label(widgets::caption(format!(
            "Showing {} of {} folders",
            shown.len(),
            plan.folders.len()
        )));
    });
    ui.horizontal(|ui| {
        let tickable: Vec<usize> = shown
            .iter()
            .copied()
            .filter(|&i| can_tick(&plan.folders[i]))
            .collect();
        if ui
            .add_enabled(!tickable.is_empty(), egui::Button::new("Tick all shown"))
            .clicked()
        {
            for &i in &tickable {
                plan.folders[i].include = true;
            }
        }
        if ui
            .add_enabled(!tickable.is_empty(), egui::Button::new("Untick all shown"))
            .clicked()
        {
            for &i in &tickable {
                plan.folders[i].include = false;
            }
        }
    });
    let shift = ui.input(|i| i.modifiers.shift);
    let mut id_edit: Option<(usize, String)> = None;

    let clients = plan.clients.clone();
    egui::ScrollArea::vertical()
        .id_salt("import-folders")
        .max_height(320.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            egui::Grid::new("import-folder-grid")
                .striped(true)
                .num_columns(6)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    // Bind header: ticks or unticks every folder shown.
                    let tickable: Vec<usize> = shown
                        .iter()
                        .copied()
                        .filter(|&i| can_tick(&plan.folders[i]))
                        .collect();
                    let mut all = !tickable.is_empty()
                        && tickable.iter().all(|&i| plan.folders[i].include);
                    let mixed = !all && tickable.iter().any(|&i| plan.folders[i].include);
                    let head = ui
                        .add_enabled(
                            !tickable.is_empty(),
                            egui::Checkbox::new(&mut all, RichText::new("Bind").strong())
                                .indeterminate(mixed),
                        )
                        .on_hover_text("Tick or untick every folder shown");
                    if head.changed() {
                        for &i in &tickable {
                            plan.folders[i].include = all;
                        }
                    }
                    for h in ["Folder", "Client", "Found by", "Match", "Files"] {
                        ui.label(RichText::new(h).strong());
                    }
                    ui.end_row();
                    let mut ticked: Option<(usize, bool)> = None;
                    for &i in &shown {
                        let r = &mut plan.folders[i];
                        if r.bound {
                            ui.label("✓");
                        } else {
                            let resp = ui
                                .add_enabled(
                                    r.client.is_some(),
                                    egui::Checkbox::without_text(&mut r.include),
                                )
                                .on_hover_text(
                                    "Bind this folder to the client (Shift-click: the folders between too)",
                                );
                            if resp.changed() {
                                ticked = Some((i, r.include));
                            }
                        }
                        let folder = ui.label(&r.rel);
                        if r.bound {
                            ui.label(r.client.as_ref().map(|c| c.name.as_str()).unwrap_or("?"));
                        } else {
                            let shown = r
                                .client
                                .as_ref()
                                .map(|c| {
                                    if c.new {
                                        format!("{} ({}) — new", c.name, c.ext_id)
                                    } else {
                                        format!("{} ({})", c.name, c.ext_id)
                                    }
                                })
                                .unwrap_or_else(|| "(choose)".into());
                            let mut chosen: Option<Option<ClientPick>> = None;
                            let current = r
                                .client
                                .as_ref()
                                .and_then(|x| clients.iter().position(|c| c.ext_id == x.ext_id));
                            let (cr, pick) = widgets::search_combo(
                                ui,
                                ("import-client", i),
                                260.0,
                                shown,
                                || {
                                    clients
                                        .iter()
                                        .map(|c| format!("{} ({})", c.name, c.ext_id))
                                        .collect()
                                },
                                current,
                                Some("(none)"),
                            );
                            cr.labelled_by(folder.id);
                            if let Some(p) = pick {
                                chosen = Some(p.map(|k| clients[k].clone()));
                            }
                            // A new client's ID, typed on the row (Q-63); Enter or leaving the
                            // box sets it.
                            if let Some(c) = r.client.as_ref().filter(|c| c.new) {
                                let d = p.id_drafts.entry(i).or_insert_with(|| c.ext_id.clone());
                                let name = format!("New client ID for {}", r.rel);
                                let resp = ui
                                    .add(egui::TextEdit::singleline(d).desired_width(90.0))
                                    .on_hover_text("This new client's ID");
                                resp.widget_info(|| {
                                    egui::WidgetInfo::labeled(
                                        egui::accesskit::Role::TextInput,
                                        true,
                                        &name,
                                    )
                                });
                                if resp.lost_focus() && !d.eq_ignore_ascii_case(&c.ext_id) {
                                    id_edit = Some((i, d.clone()));
                                }
                            }
                            if let Some(c) = chosen {
                                r.include = c.is_some();
                                r.how = "chosen".into();
                                r.confidence = if c.is_some() { 1.0 } else { 0.0 };
                                // The template's name for the client picked (Rename uses it).
                                r.template_name = c
                                    .as_ref()
                                    .and_then(|c| import::template_name_for(env.root, c, &r.rel));
                                r.client = c;
                                p.id_drafts.remove(&i);
                            }
                        }
                        // A folder sharing its client: Merge into the folder kept, or a new
                        // client of its own (owner 2026-11-12).
                        let sharing = !r.bound && in_shared(r);
                        ui.horizontal(|ui| {
                            if let Some(t) = merge_into.get(&i) {
                                ui.label(widgets::caption(format!("merges into {t}")));
                            } else if kept.contains(&i) {
                                ui.label(widgets::caption("kept: the others merge into it"));
                            } else if r.merge {
                                ui.label(widgets::caption(
                                    "tick Merge on the client's other folder too",
                                ));
                            } else {
                                ui.label(widgets::caption(&r.how));
                            }
                            if sharing {
                                let m = ui
                                    .checkbox(&mut r.merge, "Merge")
                                    .on_hover_text(
                                        "Tick Merge on each of the client's folders to combine: the bound one, else the one with more files, is kept; the others' files go into it and they are removed. Undo the last import puts them back.",
                                    );
                                m.widget_info(|| {
                                    egui::WidgetInfo::selected(
                                        egui::accesskit::Role::CheckBox,
                                        true,
                                        r.merge,
                                        format!("Merge {}", r.rel),
                                    )
                                });
                                let nc = ui
                                    .small_button("New client")
                                    .on_hover_text("A client of its own, named after this folder");
                                nc.widget_info(|| {
                                    egui::WidgetInfo::labeled(
                                        egui::accesskit::Role::Button,
                                        true,
                                        format!("New client for {}", r.rel),
                                    )
                                });
                                if nc.clicked() {
                                    new_client_row = Some(i);
                                }
                            }
                        });
                        ui.label(if r.bound {
                            String::new()
                        } else {
                            format!("{:.0}%", r.confidence * 100.0)
                        });
                        ui.label(r.files.to_string());
                        ui.end_row();
                    }
                    if let Some((i, on)) = ticked {
                        // Shift-click: the shown folders between the last one clicked and this.
                        if shift
                            && let Some(a) = p.last_tick
                            && let (Some(pa), Some(pi)) = (
                                shown.iter().position(|&x| x == a),
                                shown.iter().position(|&x| x == i),
                            )
                        {
                            for &j in &shown[pa.min(pi)..=pa.max(pi)] {
                                if can_tick(&plan.folders[j]) {
                                    plan.folders[j].include = on;
                                }
                            }
                        }
                        p.last_tick = Some(i);
                    }
                });
        });
    if let Some(i) = new_client_row {
        if let Err(e) = import::new_client_for(env.root, plan, i) {
            p.error = Some(e);
        }
        p.id_drafts.remove(&i);
    }
    if let Some((i, id)) = id_edit {
        if let Err(e) = import::set_new_id(env.root, plan, i, &id) {
            p.error = Some(e);
        }
        p.id_drafts.remove(&i);
    }
    ui.add_space(6.0);
    ui.checkbox(
        &mut p.rename,
        "Rename bound folders to the firm template's name (\"{Client} ({ID})\"); files inside are not moved",
    );
    let mut read = p.read_text.unwrap_or(plan.files.len() <= READ_UP_TO);
    if ui
        .checkbox(
            &mut read,
            "Read the text of the imported PDFs (OCR), so the words on their pages can be searched",
        )
        .on_hover_text(format!(
            "Unticked (the default above {READ_UP_TO} files), file names and details are still \
             searchable, and text recognition reads only documents filed from now on (Admin \
             \u{203a} Devices)."
        ))
        .changed()
    {
        p.read_text = Some(read);
    }
    if untyped > 0 {
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("{untyped} files with no doc type found get:"));
            let l = ui.label(widgets::caption("Doc type"));
            let shown = p
                .default_type
                .as_ref()
                .and_then(|id| p.types.iter().find(|(i, _)| i == id))
                .map_or("Leave unset".to_string(), |(_, n)| n.clone());
            let current = p
                .default_type
                .as_ref()
                .and_then(|id| p.types.iter().position(|(i, _)| i == id));
            let types = &p.types;
            let (r, pick) = widgets::search_combo(
                ui,
                "import-default-type",
                200.0,
                shown,
                || types.iter().map(|(_, n)| n.clone()).collect(),
                current,
                Some("Leave unset"),
            );
            r.labelled_by(l.id).on_hover_text(
                "Given to every file the scan found no doc type for; flagged to check",
            );
            if let Some(k) = pick {
                p.default_type = k.map(|k| p.types[k].0.clone());
            }
            let l = ui.label(widgets::caption("Source"));
            let sources = &vf_fs::docmeta::SOURCES;
            let shown = p
                .default_source
                .as_deref()
                .and_then(|s| sources.iter().find(|(k, _)| *k == s))
                .map_or("Leave unset", |(_, n)| *n)
                .to_string();
            let current = p
                .default_source
                .as_deref()
                .and_then(|s| sources.iter().position(|(k, _)| *k == s));
            let (r, pick) = widgets::search_combo(
                ui,
                "import-default-source",
                140.0,
                shown,
                || sources.iter().map(|(_, n)| n.to_string()).collect(),
                current,
                Some("Leave unset"),
            );
            r.labelled_by(l.id);
            if let Some(k) = pick {
                p.default_source = k.map(|k| sources[k].0.to_string());
            }
        });
    }
    ui.horizontal(|ui| {
        let label = if to_bind > 0 {
            // Ticked folders the filter hides are imported too: say so.
            let hidden = p.plan.as_ref().map_or(0, |plan| {
                plan.folders
                    .iter()
                    .filter(|r| {
                        r.include
                            && !r.bound
                            && r.client.is_some()
                            && !shows(r, p.min_pct, p.hide_bound, p.hide_none)
                    })
                    .count()
            });
            if hidden > 0 {
                format!("Import {to_bind} folders ({hidden} not shown)")
            } else {
                format!("Import {to_bind} folders")
            }
        } else {
            format!("Set doc types and years ({to_type} files)")
        };
        let files_read = p.plan.as_ref().is_some_and(|pl| pl.files_read);
        let go = ui
            .add_enabled(
                (to_bind > 0 || to_type > 0) && !p.running() && files_read,
                widgets::primary_button(label),
            )
            .on_disabled_hover_text(if files_read {
                "Nothing to import, or the scan is running"
            } else {
                "The files are being read (or were not): Read the files first"
            });
        if go.clicked() {
            p.start(ui.ctx(), catalog, env.root, env.device_id);
        }
        if ui.button("Export the remainder (CSV)…").clicked()
            && let Some(plan) = &p.plan
            && let Some(dest) = rfd::FileDialog::new()
                .set_file_name("Folder import remainder.csv")
                .add_filter("CSV", &["csv"])
                .save_file()
        {
            match std::fs::write(&dest, plan.report_csv()) {
                Ok(()) => p.notice = Some(format!("Saved {}", dest.display())),
                Err(e) => p.error = Some(format!("Cannot save: {e}")),
            }
        }
    });

    let Some(plan) = &p.plan else { return };
    let rest = plan.remainder();
    ui.add_space(8.0);
    ui.label(RichText::new(format!("For manual handling: {} files", rest.len())).strong());
    ui.label(widgets::caption(
        "After the import, set these in Browse (select, then the inspector) or file them through Intake.",
    ));
    egui::ScrollArea::vertical()
        .id_salt("import-rest")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("import-rest-grid")
                .striped(true)
                .num_columns(3)
                .show(ui, |ui| {
                    for (f, gaps) in rest.iter().take(SHOWN) {
                        ui.label(&f.rel);
                        let missing: Vec<&str> = gaps.iter().map(|g| g.label()).collect();
                        ui.label(widgets::caption(missing.join(", ")));
                        ui.label(widgets::caption(&f.type_how));
                        ui.end_row();
                    }
                });
            if rest.len() > SHOWN {
                ui.label(widgets::caption(format!(
                    "… and {} more in the CSV.",
                    rest.len() - SHOWN
                )));
            }
        });
}
