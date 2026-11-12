//! `_Vibe/client.json` sentinels, the root scan that matches top-level folders to clients, and
//! the folder index (`folders.json`) the rest of the app looks client folders up in.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Receiver;

use chrono::{DateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::clients::{Client, ClientList};
use crate::config::Level;
use crate::datafile::{load_json, save_json};
use crate::worker;

pub const VIBE_DIR: &str = "_Vibe";
pub const SENTINEL_FILE: &str = "client.json";
pub const FOLDERS_FILE: &str = "folders.json";
pub const SENTINEL_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Sentinel

/// T&B's `SentinelV1`, field for field. Written once when the app creates or binds a client
/// folder; never rewritten.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sentinel {
    pub version: u32,
    /// The client's uuid (`Client::uuid`).
    pub client_id: Uuid,
    /// `Settings::firm_id`.
    pub firm_id: Uuid,
    /// The client's CSV ID.
    pub tax_software_id: String,
    pub display_name_at_creation: String,
    pub created_at: DateTime<Utc>,
    /// Always null from this app.
    pub created_by: Option<String>,
}

pub fn sentinel_path(folder: &Path) -> PathBuf {
    folder.join(VIBE_DIR).join(SENTINEL_FILE)
}

/// `Ok(None)` when the folder has no sentinel; `Err` when it has one that can't be read or is
/// not in `SentinelV1` shape (the scan reports it as unparseable).
pub fn read_sentinel(folder: &Path) -> Result<Option<Sentinel>, String> {
    match std::fs::read_to_string(sentinel_path(folder)) {
        Ok(text) => match serde_json::from_str::<Sentinel>(&text) {
            Ok(s) if s.version == SENTINEL_VERSION => Ok(Some(s)),
            Ok(s) => Err(format!("sentinel version {} is not supported", s.version)),
            Err(e) => Err(format!("sentinel is not in SentinelV1 shape: {e}")),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("sentinel cannot be read: {e}")),
    }
}

/// `path` (which must exist) is inside `root`, after resolving both.
pub fn ensure_inside(root: &Path, path: &Path) -> Result<(), String> {
    let root_c = std::fs::canonicalize(root)
        .map_err(|e| format!("cannot open the client root {}: {e}", root.display()))?;
    let path_c =
        std::fs::canonicalize(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    if path_c.starts_with(&root_c) && path_c != root_c {
        Ok(())
    } else {
        Err(format!(
            "{} is not inside the client root {}",
            path.display(),
            root.display()
        ))
    }
}

/// Writes `_Vibe/client.json` into `folder` for `client`. Refuses if a sentinel already exists,
/// the folder is not inside `root`, or the client or firm has no uuid.
pub fn write_sentinel(
    root: &Path,
    folder: &Path,
    client: &Client,
    firm_id: Uuid,
) -> Result<Sentinel, String> {
    use std::io::Write;
    if client.uuid.is_nil() || firm_id.is_nil() {
        return Err(format!(
            "client {} or the firm has no ID yet; re-import the client list",
            client.id
        ));
    }
    ensure_inside(root, folder)?;
    let dir = folder.join(VIBE_DIR);
    match std::fs::create_dir(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("cannot create {}: {e}", dir.display())),
    }
    let sentinel = Sentinel {
        version: SENTINEL_VERSION,
        client_id: client.uuid,
        firm_id,
        tax_software_id: client.id.clone(),
        display_name_at_creation: client.name.clone(),
        created_at: Utc::now(),
        created_by: None,
    };
    let path = sentinel_path(folder);
    let json = serde_json::to_vec_pretty(&sentinel).map_err(|e| e.to_string())?;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::AlreadyExists => {
                format!("{} already has a sentinel", folder.display())
            }
            _ => format!("cannot write {}: {e}", path.display()),
        })?;
    f.write_all(&json)
        .and_then(|()| f.sync_all())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(sentinel)
}

/// Renames `_Vibe/client.json` in `folder` to `client.<kind>-<time>.json` in the same folder. The
/// old sentinel is kept, never deleted (X123, X124). Returns the new path.
pub fn set_aside_sentinel(root: &Path, folder: &Path, kind: &str) -> Result<PathBuf, String> {
    ensure_inside(root, folder)?;
    let src = sentinel_path(folder);
    let dir = folder.join(VIBE_DIR);
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let mut dst = dir.join(format!("client.{kind}-{stamp}.json"));
    let mut n = 2;
    while dst.exists() {
        dst = dir.join(format!("client.{kind}-{stamp}-{n}.json"));
        n += 1;
    }
    std::fs::rename(&src, &dst).map_err(|e| format!("cannot set aside {}: {e}", src.display()))?;
    Ok(dst)
}

/// One Conflicts action in ops.jsonl (its own batch id; these are not undoable commits).
fn log_action(
    data_dir: &Path,
    batch: Uuid,
    op: crate::oplog::Op,
    client_id: Option<&str>,
    src: Option<&Path>,
    dst: Option<&Path>,
) {
    let rec = crate::oplog::OpRecord {
        client_id: client_id.map(str::to_string),
        src: src.map(Path::to_path_buf),
        dst: dst.map(Path::to_path_buf),
        ..crate::oplog::OpRecord::new(batch, op)
    };
    if let Err(e) = crate::oplog::append(&crate::oplog::path(data_dir), &rec) {
        tracing::error!(error = %e, "could not log a Conflicts action to ops.jsonl");
    }
}

/// Bind or Rebind on a worker: optionally set the folder's current sentinel aside, then write a
/// new one; both steps are logged.
fn bind_job(
    data_dir: &Path,
    root: &Path,
    folder: &Path,
    client: &Client,
    firm_id: Uuid,
    set_aside: Option<&str>,
) -> Result<(PathBuf, String), String> {
    let batch = Uuid::new_v4();
    if let Some(kind) = set_aside {
        let old = sentinel_path(folder);
        let kept = set_aside_sentinel(root, folder, kind)?;
        log_action(
            data_dir,
            batch,
            crate::oplog::Op::SentinelSetAside,
            None,
            Some(&old),
            Some(&kept),
        );
    }
    let s = write_sentinel(root, folder, client, firm_id)?;
    log_action(
        data_dir,
        batch,
        crate::oplog::Op::Sentinel,
        Some(&s.tax_software_id),
        None,
        Some(&sentinel_path(folder)),
    );
    Ok((folder.to_path_buf(), s.tax_software_id))
}

/// Folders to bind (folder, client) in one go: "Bind all exact name matches" (V9). Each is
/// bound like a single Bind; one that fails is reported and the rest continue.
pub fn bind_all_job(
    data_dir: &Path,
    root: &Path,
    pairs: &[(PathBuf, Client)],
    firm_id: Uuid,
) -> Vec<Bound> {
    pairs
        .iter()
        .map(|(folder, client)| {
            bind_job(data_dir, root, folder, client, firm_id, None)
                .map_err(|e| format!("{}: {e}", folder.display()))
        })
        .collect()
}

/// What "Rename to template name" did (V9).
#[derive(Debug, Default)]
pub struct RenameReport {
    pub done: Vec<crate::commit::RenamedDir>,
    pub failed: Vec<(PathBuf, String)>,
}

/// Renames bound client folders under `root` (from → to, both directly under the root, same
/// volume). A target that exists or a folder that can't be renamed (in use) is skipped and
/// reported. Each rename is logged; when any succeeded, they become the last batch for Undo.
pub fn rename_folders(
    data_dir: &Path,
    root: &Path,
    renames: &[(String, PathBuf, PathBuf)],
) -> RenameReport {
    let batch = Uuid::new_v4();
    let mut report = RenameReport::default();
    for (client_id, from, to) in renames {
        let check = || -> Result<(), String> {
            if !from.is_dir() {
                return Err("the folder is gone".into());
            }
            if from.parent() != Some(root) || to.parent() != Some(root) {
                return Err("only folders directly under the root are renamed".into());
            }
            ensure_inside(root, from)?;
            if to.exists() {
                return Err(format!("{} already exists", to.display()));
            }
            std::fs::rename(from, to).map_err(|e| format!("cannot rename: {e}"))
        };
        let result = check();
        let rec = crate::oplog::OpRecord {
            client_id: Some(client_id.clone()),
            src: Some(from.clone()),
            dst: Some(to.clone()),
            result: if result.is_ok() {
                crate::oplog::OpResult::Ok
            } else {
                crate::oplog::OpResult::Error
            },
            error: result.clone().err(),
            ..crate::oplog::OpRecord::new(batch, crate::oplog::Op::RenameDir)
        };
        if let Err(e) = crate::oplog::append(&crate::oplog::path(data_dir), &rec) {
            tracing::error!(error = %e, "cannot append to ops.jsonl");
        }
        match result {
            Ok(()) => {
                tracing::info!(from = %from.display(), to = %to.display(), "client folder renamed");
                report.done.push(crate::commit::RenamedDir {
                    client_id: client_id.clone(),
                    from: from.clone(),
                    to: to.clone(),
                });
            }
            Err(e) => report.failed.push((from.clone(), e)),
        }
    }
    if !report.done.is_empty() {
        let lb = crate::commit::LastBatch {
            batch,
            committed_at: chrono::Utc::now(),
            in_progress: false,
            undone: false,
            items: Vec::new(),
            created_dirs: Vec::new(),
            created_files: Vec::new(),
            backups: Vec::new(),
            split_originals: Vec::new(),
            zip_entries: Vec::new(),
            zip: None,
            routed_zips: Vec::new(),
            renamed_dirs: report.done.clone(),
        };
        if let Err(e) = crate::commit::save_last_batch(data_dir, &lb) {
            report
                .failed
                .push((data_dir.to_path_buf(), format!("last_batch.json: {e}")));
        }
    }
    report
}

// ---------------------------------------------------------------------------
// Scan

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Sentinel present and its client is in the master list.
    Bound,
    /// No sentinel; the client ID in the folder name is in the master list.
    UnboundNamed,
    /// No sentinel; the folder name is close to a client's name.
    UnboundFuzzy,
    /// Sentinel names a client that is not in the master list.
    Orphan,
    /// A sentinel that can't be read or is not in `SentinelV1` shape.
    Unparseable,
    /// Two or more folders have sentinels for the same client.
    Duplicate,
    /// No sentinel and no matching client.
    Unmatched,
    /// On the persisted ignore list.
    Ignored,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FolderBinding {
    pub path: PathBuf,
    /// Bound/Duplicate/Orphan: the sentinel's client (CSV ID). Unbound*: the candidate client.
    pub client_id: Option<String>,
    pub status: Status,
    /// Name similarity for `UnboundFuzzy`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    /// Extra detail for the Conflicts panel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl FolderBinding {
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// What a scan needs. Cloned into the worker thread.
#[derive(Clone)]
pub struct ScanInput {
    pub root: PathBuf,
    pub clients: Arc<ClientList>,
    pub id_pattern: String,
    pub threshold: f32,
    pub ignored: Vec<PathBuf>,
}

/// Progress counters a scan updates while it runs.
#[derive(Default)]
pub struct Progress {
    pub done: AtomicUsize,
    pub total: AtomicUsize,
}

/// Names the scan leaves out entirely: `_x`, `.x`, `~x` (X28).
fn skipped_name(name: &str) -> bool {
    name.starts_with(['_', '.', '~'])
}

fn hidden_or_system(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    meta.file_attributes() & (HIDDEN | SYSTEM) != 0
}

pub fn same_path(a: &Path, b: &Path) -> bool {
    a.as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&b.as_os_str().to_string_lossy())
}

/// Client ID at the end of a folder name: `Name (1234)` or `Name_1234`. Returns the ID and the
/// name with the ID part removed.
fn id_in_name<'a>(re: &Regex, name: &'a str) -> Option<(String, &'a str)> {
    let c = re.captures(name)?;
    let id = c.name("p").or_else(|| c.name("u"))?.as_str().to_string();
    let start = c.get(0)?.start();
    Some((id, name[..start].trim_end()))
}

/// `name` without a trailing client ID (`Name (1234)` / `Name_1234`), as the scan reads it.
pub fn name_without_id(re: &Regex, name: &str) -> String {
    id_in_name(re, name)
        .map_or(name.trim_end(), |(_, rest)| rest)
        .to_string()
}

/// Finds a client ID at the end of a folder name (see [`id_in_name`]).
pub fn id_regex(id_pattern: &str) -> Result<Regex, String> {
    Regex::new(&format!(
        r"(?:\s*\((?P<p>{id_pattern})\)\s*$)|(?:_(?P<u>{id_pattern})$)"
    ))
    .map_err(|e| format!("the client ID pattern is not valid: {e}"))
}

/// Classifies every top-level folder of `root` (see [`Status`]).
pub fn scan_root(input: &ScanInput, progress: &Progress) -> Result<Vec<FolderBinding>, String> {
    let re = id_regex(&input.id_pattern)?;
    let entries = std::fs::read_dir(&input.root)
        .map_err(|e| format!("cannot read the client root {}: {e}", input.root.display()))?;
    let mut folders: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("cannot read the client root: {e}"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_dir() || skipped_name(&name) || hidden_or_system(&meta) {
            continue;
        }
        folders.push(entry.path());
    }
    folders.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    progress.total.store(folders.len(), Ordering::Relaxed);

    // Each folder's sentinel is a separate read (a round trip on a network share): read them on
    // a few threads, each filling its own slice so the order stays the sorted one.
    let mut slots: Vec<Option<FolderBinding>> = (0..folders.len()).map(|_| None).collect();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
    let chunk = folders.len().div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        for (paths, slots) in folders.chunks(chunk).zip(slots.chunks_mut(chunk)) {
            let re = &re;
            scope.spawn(move || {
                for (path, slot) in paths.iter().zip(slots) {
                    *slot = Some(classify(input, re, path.clone()));
                    progress.done.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
    });
    let mut out: Vec<FolderBinding> = slots.into_iter().flatten().collect();
    mark_duplicates(&mut out);
    note_already_bound(&mut out, &input.clients);
    Ok(out)
}

fn classify(input: &ScanInput, re: &Regex, path: PathBuf) -> FolderBinding {
    let binding = |client_id: Option<String>, status, score, note: Option<String>| FolderBinding {
        path: path.clone(),
        client_id,
        status,
        score,
        note,
    };
    if input.ignored.iter().any(|i| same_path(i, &path)) {
        return binding(None, Status::Ignored, None, None);
    }
    match read_sentinel(&path) {
        Err(e) => return binding(None, Status::Unparseable, None, Some(e)),
        Ok(Some(s)) => {
            // Bound by the CSV ID (X114); a different uuid is reported, not fatal.
            return match input.clients.match_by_id(&s.tax_software_id) {
                Some(c) => {
                    let note = (c.uuid != s.client_id).then(|| {
                        format!(
                            "Sentinel client_id {} differs from this app's ID for {} ({}).",
                            s.client_id, c.name, c.id
                        )
                    });
                    binding(Some(s.tax_software_id), Status::Bound, None, note)
                }
                None => {
                    let note = format!(
                        "Sentinel says client_id {} ({}), which is not in the client list.",
                        s.tax_software_id, s.display_name_at_creation
                    );
                    binding(Some(s.tax_software_id), Status::Orphan, None, Some(note))
                }
            };
        }
        Ok(None) => {}
    }

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (name_part, unknown_id) = match id_in_name(re, &name) {
        Some((id, _)) if input.clients.match_by_id(&id).is_some() => {
            return binding(Some(id), Status::UnboundNamed, None, None);
        }
        Some((id, rest)) => (rest.to_string(), Some(id)),
        None => (name.clone(), None),
    };
    let unknown_note =
        unknown_id.map(|id| format!("ID {id} in the folder name is not in the client list."));
    match input.clients.fuzzy_by_name(&name_part, 1).first() {
        Some((score, c)) if *score >= f64::from(input.threshold) => binding(
            Some(c.id.clone()),
            Status::UnboundFuzzy,
            Some(*score),
            unknown_note,
        ),
        _ => binding(None, Status::Unmatched, None, unknown_note),
    }
}

/// Folders whose sentinels name the same client are all marked Duplicate.
fn mark_duplicates(out: &mut [FolderBinding]) {
    let mut by_client: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, b) in out.iter().enumerate() {
        if b.status == Status::Bound
            && let Some(id) = &b.client_id
        {
            by_client.entry(id.clone()).or_default().push(i);
        }
    }
    for idx in by_client.values().filter(|v| v.len() > 1) {
        for &i in idx {
            let others: Vec<String> = idx
                .iter()
                .filter(|&&j| j != i)
                .map(|&j| out[j].name())
                .collect();
            out[i].status = Status::Duplicate;
            out[i].note = Some(format!("Also bound to this client: {}.", others.join(", ")));
        }
    }
}

/// Unbound candidates for a client that already has a bound folder get a note.
fn note_already_bound(out: &mut [FolderBinding], clients: &ClientList) {
    let mut bound: HashMap<String, String> = HashMap::new();
    for b in out.iter().filter(|b| b.status == Status::Bound) {
        if let Some(id) = &b.client_id {
            bound.entry(id.clone()).or_insert_with(|| b.name());
        }
    }
    for b in out.iter_mut() {
        if !matches!(b.status, Status::UnboundNamed | Status::UnboundFuzzy) {
            continue;
        }
        if let Some(id) = &b.client_id
            && let Some(folder) = bound.get(id)
        {
            let name = clients.match_by_id(id).map_or("", |c| c.name.as_str());
            b.note = Some(format!("{name} ({id}) is already bound to \"{folder}\"."));
        }
    }
}

// ---------------------------------------------------------------------------
// folders.json and the ignore list

/// The last scan of one root.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FoldersFile {
    pub root: PathBuf,
    pub scanned_at: DateTime<Utc>,
    pub folders: Vec<FolderBinding>,
    /// Folders the user chose to ignore in Conflicts (full paths, any root).
    #[serde(default)]
    pub ignored: Vec<PathBuf>,
}

struct ScanJob {
    rx: Receiver<Result<FoldersFile, String>>,
    progress: Arc<Progress>,
}

/// A resolved duplicate: the kept folder and the folders whose sentinel was set aside.
type KeepDone = (PathBuf, Vec<PathBuf>);
/// One bind's result: (folder, client ID) or why it failed.
type Bound = Result<(PathBuf, String), String>;

/// The folder index plus scan and bind actions.
pub struct FoldersManager {
    file: Option<FoldersFile>,
    ignored: Vec<PathBuf>,
    scan: Option<ScanJob>,
    bind_job: Option<Receiver<Result<(PathBuf, String), String>>>,
    /// Keep one duplicate folder: (kept folder, folders whose sentinel was set aside).
    keep_job: Option<Receiver<Result<KeepDone, String>>>,
    /// "Bind all exact name matches" (V9).
    bind_all: Option<Receiver<Vec<Bound>>>,
    note: Option<(Level, String)>,
}

impl FoldersManager {
    /// Loads `folders.json`, which also holds the ignore list. Small local file, read at startup.
    pub fn load(data_dir: &Path) -> Self {
        let mut note = None;
        let file = load_json::<FoldersFile>(&data_dir.join(FOLDERS_FILE)).unwrap_or_else(|e| {
            tracing::error!(error = %e, "folders.json unreadable; a rescan will rebuild it");
            note = Some((
                Level::Error,
                format!("folders.json could not be read ({e}). Rescan the root."),
            ));
            None
        });
        let ignored = file.as_ref().map(|f| f.ignored.clone()).unwrap_or_default();
        Self {
            file,
            ignored,
            scan: None,
            bind_job: None,
            keep_job: None,
            bind_all: None,
            note,
        }
    }

    /// The last scan, if it was of `root`.
    pub fn scan_of(&self, root: Option<&Path>) -> Option<&FoldersFile> {
        self.file
            .as_ref()
            .filter(|f| root.is_some_and(|r| same_path(&f.root, r)))
    }

    #[cfg(test)]
    pub fn ignored(&self) -> &[PathBuf] {
        &self.ignored
    }

    pub fn note(&self) -> Option<&(Level, String)> {
        self.note.as_ref()
    }

    pub fn scanning(&self) -> Option<(usize, usize)> {
        self.scan.as_ref().map(|s| {
            (
                s.progress.done.load(Ordering::Relaxed),
                s.progress.total.load(Ordering::Relaxed),
            )
        })
    }

    pub fn binding_in_progress(&self) -> bool {
        self.bind_job.is_some() || self.keep_job.is_some() || self.bind_all.is_some()
    }

    /// Binds every (folder, client) on a worker (V9).
    pub fn bind_many(
        &mut self,
        ctx: &egui::Context,
        data_dir: PathBuf,
        root: PathBuf,
        pairs: Vec<(PathBuf, Client)>,
        firm_id: Uuid,
    ) {
        if self.binding_in_progress() || pairs.is_empty() {
            return;
        }
        self.bind_all = Some(worker::background(ctx, move || {
            bind_all_job(&data_dir, &root, &pairs, firm_id)
        }));
    }

    /// Folders renamed (or renamed back by Undo): the index follows them (V9).
    pub fn follow_renames(&mut self, data_dir: &Path, moves: &[(PathBuf, PathBuf)]) {
        let Some(f) = &mut self.file else { return };
        for (from, to) in moves {
            for b in f.folders.iter_mut().filter(|b| same_path(&b.path, from)) {
                b.path = to.clone();
            }
        }
        self.persist(data_dir);
    }

    /// Folder bound to `client_id` in the last scan of `root`.
    pub fn bound_folder(&self, root: Option<&Path>, client_id: &str) -> Option<&Path> {
        self.scan_of(root)?
            .folders
            .iter()
            .find(|b| b.status == Status::Bound && b.client_id.as_deref() == Some(client_id))
            .map(|b| b.path.as_path())
    }

    /// Starts a scan on a worker thread; the result replaces `folders.json`.
    pub fn start_scan(&mut self, ctx: &egui::Context, data_dir: PathBuf, mut input: ScanInput) {
        if self.scan.is_some() {
            return;
        }
        input.ignored = self.ignored.clone();
        let progress = Arc::new(Progress::default());
        let p = Arc::clone(&progress);
        let rx = worker::background(ctx, move || {
            let folders = scan_root(&input, &p)?;
            let file = FoldersFile {
                root: input.root.clone(),
                scanned_at: Utc::now(),
                folders,
                ignored: input.ignored.clone(),
            };
            save_json(&data_dir.join(FOLDERS_FILE), &file)
                .map_err(|e| format!("cannot write folders.json: {e}"))?;
            Ok(file)
        });
        // The worker repaints only when done; keep the progress counter moving meanwhile.
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        self.scan = Some(ScanJob { rx, progress });
    }

    /// Writes a sentinel binding `folder` to `client` on a worker thread.
    pub fn bind(
        &mut self,
        ctx: &egui::Context,
        data_dir: PathBuf,
        root: PathBuf,
        folder: PathBuf,
        client: Client,
        firm_id: Uuid,
    ) {
        self.start_bind(ctx, data_dir, root, folder, client, firm_id, None);
    }

    /// Orphan or unparseable folder: sets its sentinel aside as `client.orphaned-<time>.json`,
    /// then binds it to `client` (X123).
    pub fn rebind(
        &mut self,
        ctx: &egui::Context,
        data_dir: PathBuf,
        root: PathBuf,
        folder: PathBuf,
        client: Client,
        firm_id: Uuid,
    ) {
        self.start_bind(
            ctx,
            data_dir,
            root,
            folder,
            client,
            firm_id,
            Some("orphaned"),
        );
    }

    /// Duplicate folders: keeps `keep` bound and sets the other folders' sentinels aside as
    /// `client.duplicate-<time>.json` (X124). No files move.
    pub fn keep_duplicate(
        &mut self,
        ctx: &egui::Context,
        data_dir: PathBuf,
        root: PathBuf,
        keep: PathBuf,
        others: Vec<PathBuf>,
    ) {
        if self.binding_in_progress() {
            return;
        }
        self.keep_job = Some(worker::background(ctx, move || {
            let batch = Uuid::new_v4();
            let mut done = Vec::new();
            for folder in others {
                let old = sentinel_path(&folder);
                let kept = set_aside_sentinel(&root, &folder, "duplicate")?;
                log_action(
                    &data_dir,
                    batch,
                    crate::oplog::Op::SentinelSetAside,
                    None,
                    Some(&old),
                    Some(&kept),
                );
                done.push(folder);
            }
            Ok((keep, done))
        }));
    }

    #[allow(clippy::too_many_arguments)]
    fn start_bind(
        &mut self,
        ctx: &egui::Context,
        data_dir: PathBuf,
        root: PathBuf,
        folder: PathBuf,
        client: Client,
        firm_id: Uuid,
        set_aside: Option<&'static str>,
    ) {
        if self.binding_in_progress() {
            return;
        }
        let already = self.file.as_ref().and_then(|f| {
            f.folders.iter().find(|b| {
                b.status == Status::Bound && b.client_id.as_deref() == Some(client.id.as_str())
            })
        });
        if let Some(b) = already {
            self.note = Some((
                Level::Error,
                format!(
                    "{} ({}) is already bound to \"{}\". Each client can have one folder.",
                    client.name,
                    client.id,
                    b.name()
                ),
            ));
            return;
        }
        self.bind_job = Some(worker::background(ctx, move || {
            bind_job(&data_dir, &root, &folder, &client, firm_id, set_aside)
        }));
    }

    /// Adds `folder` to the persisted ignore list.
    pub fn ignore(&mut self, data_dir: &Path, folder: &Path) {
        if !self.ignored.iter().any(|i| same_path(i, folder)) {
            self.ignored.push(folder.to_path_buf());
        }
        self.set_status(folder, Status::Ignored, None, None);
        self.persist(data_dir);
    }

    /// Removes `folder` from the ignore list. It shows as unmatched until the next scan.
    pub fn unignore(&mut self, data_dir: &Path, folder: &Path) {
        self.ignored.retain(|i| !same_path(i, folder));
        self.set_status(
            folder,
            Status::Unmatched,
            None,
            Some("Rescan to classify this folder.".into()),
        );
        self.persist(data_dir);
    }

    fn set_status(
        &mut self,
        folder: &Path,
        status: Status,
        client_id: Option<String>,
        note: Option<String>,
    ) {
        if let Some(f) = &mut self.file
            && let Some(b) = f.folders.iter_mut().find(|b| same_path(&b.path, folder))
        {
            b.status = status;
            b.client_id = client_id;
            b.score = None;
            b.note = note;
        }
    }

    fn persist(&mut self, data_dir: &Path) {
        let Some(f) = &mut self.file else { return };
        f.ignored = self.ignored.clone();
        if let Err(e) = save_json(&data_dir.join(FOLDERS_FILE), f) {
            tracing::error!(error = %e, "saving folders.json failed");
            self.note = Some((Level::Error, format!("Could not save folders.json: {e}.")));
        }
    }

    /// Collects finished scans and binds. Call every frame.
    pub fn poll(&mut self, ctx: &egui::Context, data_dir: &Path) {
        if let Some(job) = &self.scan {
            match job.rx.try_recv() {
                Ok(Ok(file)) => {
                    let n = file.folders.len();
                    tracing::info!(folders = n, root = %file.root.display(), "root scanned");
                    self.note = Some((Level::Info, format!("Scanned {n} folders.")));
                    self.file = Some(file);
                    self.scan = None;
                }
                Ok(Err(e)) => {
                    tracing::error!(error = %e, "root scan failed");
                    self.note = Some((Level::Error, format!("Scan failed: {e}")));
                    self.scan = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.note = Some((
                        Level::Error,
                        "Scan stopped unexpectedly (see filer.log).".into(),
                    ));
                    self.scan = None;
                }
            }
        }
        if let Some(rx) = &self.bind_job
            && let Ok(result) = rx.try_recv()
        {
            self.bind_job = None;
            match result {
                Ok((folder, client_id)) => {
                    tracing::info!(folder = %folder.display(), client = %client_id, "folder bound");
                    self.set_status(&folder, Status::Bound, Some(client_id), None);
                    self.note = Some((Level::Info, format!("Bound \"{}\".", folder.display())));
                    self.persist(data_dir);
                }
                Err(e) => {
                    tracing::error!(error = %e, "bind failed");
                    self.note = Some((Level::Error, format!("Could not bind: {e}")));
                }
            }
        }
        if let Some(rx) = &self.bind_all
            && let Ok(results) = rx.try_recv()
        {
            self.bind_all = None;
            let mut bound = 0;
            let mut failed = Vec::new();
            for r in results {
                match r {
                    Ok((folder, client_id)) => {
                        self.set_status(&folder, Status::Bound, Some(client_id), None);
                        bound += 1;
                    }
                    Err(e) => failed.push(e),
                }
            }
            tracing::info!(bound, failed = failed.len(), "bind all");
            self.note = Some(if failed.is_empty() {
                (Level::Info, format!("Bound {bound} folders."))
            } else {
                (
                    Level::Error,
                    format!(
                        "Bound {bound} folders; {} failed: {}",
                        failed.len(),
                        failed.join("; ")
                    ),
                )
            });
            self.persist(data_dir);
        }
        if let Some(rx) = &self.keep_job
            && let Ok(result) = rx.try_recv()
        {
            self.keep_job = None;
            match result {
                Ok((keep, others)) => {
                    tracing::info!(kept = %keep.display(), others = others.len(), "duplicate resolved");
                    let id = self.file.as_ref().and_then(|f| {
                        f.folders
                            .iter()
                            .find(|b| same_path(&b.path, &keep))
                            .and_then(|b| b.client_id.clone())
                    });
                    self.set_status(&keep, Status::Bound, id, None);
                    for o in &others {
                        self.set_status(
                            o,
                            Status::Unmatched,
                            None,
                            Some("Sentinel set aside as client.duplicate-….json. Rescan to classify this folder.".into()),
                        );
                    }
                    self.note = Some((Level::Info, format!("Kept \"{}\".", keep.display())));
                    self.persist(data_dir);
                }
                Err(e) => {
                    tracing::error!(error = %e, "keeping a duplicate failed");
                    self.note = Some((
                        Level::Error,
                        format!("Could not resolve the duplicate: {e}"),
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// V9: a rename onto an existing name is refused and nothing is saved for Undo.
    #[test]
    fn rename_folders_skips_a_taken_name() {
        let tmp = tempfile::tempdir().unwrap();
        let (data, root) = (tmp.path().join("data"), tmp.path().join("root"));
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(root.join("Smith")).unwrap();
        std::fs::create_dir_all(root.join("Smith (1001)")).unwrap();
        let r = rename_folders(
            &data,
            &root,
            &[("1001".into(), root.join("Smith"), root.join("Smith (1001)"))],
        );
        assert!(r.done.is_empty());
        assert_eq!(r.failed.len(), 1);
        assert!(root.join("Smith").is_dir());
        assert!(crate::commit::load_last_batch(&data).is_none());
        // Outside the root: refused.
        let r = rename_folders(
            &data,
            &root,
            &[(
                "1001".into(),
                root.join("Smith"),
                tmp.path().join("Smith (1001)"),
            )],
        );
        assert_eq!(r.failed.len(), 1);
        assert!(root.join("Smith").is_dir());
    }
    use crate::config::ColumnMap;
    use std::collections::BTreeMap;

    fn client(id: &str, name: &str) -> Client {
        Client {
            uuid: uuid::Uuid::new_v4(),
            id: id.into(),
            name: name.into(),
            extra: BTreeMap::new(),
        }
    }

    fn clients() -> Arc<ClientList> {
        let dir = tempfile::tempdir().unwrap();
        let csv = dir.path().join("c.csv");
        std::fs::write(
            &csv,
            "id,name\n1001,\"Smith, John\"\n1002,Acme LLC\n1003,\"Jones, Mary\"\n1004,Baker Farms\n",
        )
        .unwrap();
        let table = crate::clients::read_csv(&csv).unwrap();
        let map = ColumnMap {
            partner: None,
            id: Some("id".into()),
            name: Some("name".into()),
            extras: vec![],
        };
        Arc::new(crate::clients::apply_mapping(&table, &map).unwrap())
    }

    fn mkdir(root: &Path, name: &str) -> PathBuf {
        let p = root.join(name);
        std::fs::create_dir(&p).unwrap();
        p
    }

    fn set_hidden(p: &Path) {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = p.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: valid NUL-terminated path.
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::SetFileAttributesW(wide.as_ptr(), 0x2)
        };
        assert_ne!(ok, 0);
    }

    fn input(root: &Path, ignored: Vec<PathBuf>) -> ScanInput {
        ScanInput {
            root: root.to_path_buf(),
            clients: clients(),
            id_pattern: crate::parse::DEFAULT_ID_PATTERN.into(),
            threshold: 0.90,
            ignored,
        }
    }

    fn status_of<'a>(out: &'a [FolderBinding], name: &str) -> &'a FolderBinding {
        out.iter()
            .find(|b| b.name() == name)
            .unwrap_or_else(|| panic!("{name} not in scan: {out:?}"))
    }

    #[test]
    fn sentinel_write_read_and_refusals() {
        let tmp = tempfile::tempdir().unwrap();
        let root = mkdir(tmp.path(), "root");
        let folder = mkdir(&root, "Acme LLC (1002)");
        let firm = Uuid::new_v4();
        let acme = client("1002", "Acme LLC");
        let s = write_sentinel(&root, &folder, &acme, firm).unwrap();
        assert_eq!(s.version, 1);
        assert_eq!((s.client_id, s.firm_id), (acme.uuid, firm));
        assert_eq!(s.tax_software_id, "1002");
        assert_eq!(read_sentinel(&folder).unwrap(), Some(s.clone()));
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(sentinel_path(&folder)).unwrap())
                .unwrap();
        // T&B SentinelV1, field for field.
        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "client_id",
                "created_at",
                "created_by",
                "display_name_at_creation",
                "firm_id",
                "tax_software_id",
                "version"
            ]
        );
        assert!(json["created_by"].is_null());
        // Never overwritten.
        let again = write_sentinel(&root, &folder, &client("1001", "Other"), firm);
        assert!(again.unwrap_err().contains("already has a sentinel"));
        assert_eq!(
            read_sentinel(&folder).unwrap().unwrap().tax_software_id,
            "1002"
        );
        // Outside the root, or the root itself: refused.
        let outside = mkdir(tmp.path(), "elsewhere");
        assert!(write_sentinel(&root, &outside, &client("1", "X"), firm).is_err());
        assert!(write_sentinel(&root, &root, &client("1", "X"), firm).is_err());
        assert!(!sentinel_path(&outside).exists());
        // No uuid yet: refused.
        let mut no_uuid = client("1001", "Smith");
        no_uuid.uuid = Uuid::nil();
        let fresh = mkdir(&root, "Smith");
        assert!(write_sentinel(&root, &fresh, &no_uuid, firm).is_err());
    }

    /// Many folders (more than one thread's share): sorted like the folder list, every one
    /// classified once, duplicates and "already bound" notes as before.
    #[test]
    fn parallel_scan_keeps_order_and_marks() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let cl = clients();
        let firm = Uuid::new_v4();
        for i in 0..60 {
            mkdir(root, &format!("Folder {i:02}"));
        }
        for name in ["Dup A", "Dup Z"] {
            let d = mkdir(root, name);
            write_sentinel(root, &d, cl.match_by_id("1004").unwrap(), firm).unwrap();
        }
        let smith = mkdir(root, "Smith, John (1001)");
        write_sentinel(root, &smith, cl.match_by_id("1001").unwrap(), firm).unwrap();
        mkdir(root, "Smith John_1001");
        let progress = Progress::default();
        let out = scan_root(&input(root, vec![]), &progress).unwrap();
        assert_eq!(out.len(), 64);
        assert_eq!(progress.done.load(Ordering::Relaxed), 64);
        let names: Vec<String> = out.iter().map(|b| b.name().to_lowercase()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
        assert_eq!(status_of(&out, "Dup A").status, Status::Duplicate);
        assert_eq!(
            status_of(&out, "Dup A").note.as_deref(),
            Some("Also bound to this client: Dup Z.")
        );
        assert_eq!(
            status_of(&out, "Smith John_1001").note.as_deref(),
            Some("Smith, John (1001) is already bound to \"Smith, John (1001)\".")
        );
    }

    #[test]
    fn scan_classifies_every_state() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let cl = clients();
        let firm = Uuid::new_v4();
        let bound = mkdir(root, "Smith, John (1001)");
        write_sentinel(root, &bound, cl.match_by_id("1001").unwrap(), firm).unwrap();
        mkdir(root, "Acme LLC (1002)");
        mkdir(root, "Jonez Mary");
        let orphan = mkdir(root, "Old Client");
        write_sentinel(root, &orphan, &client("9999", "Gone Away"), firm).unwrap();
        for name in ["Baker A", "Baker B"] {
            let d = mkdir(root, name);
            write_sentinel(root, &d, cl.match_by_id("1004").unwrap(), firm).unwrap();
        }
        mkdir(root, "Random Stuff");
        mkdir(root, "Unknown_5555");
        let broken = mkdir(root, "Broken");
        std::fs::create_dir(broken.join(VIBE_DIR)).unwrap();
        std::fs::write(sentinel_path(&broken), "{ nope").unwrap();
        // A v0.1.0 sentinel (firm_tag shape) is unparseable too (X112).
        let old = mkdir(root, "Old Shape");
        std::fs::create_dir(old.join(VIBE_DIR)).unwrap();
        std::fs::write(
            sentinel_path(&old),
            r#"{"schema_version":1,"client_id":"1002","firm_tag":"KISAES","display_name_at_creation":"Acme LLC","created_at":"2026-11-01T00:00:00Z"}"#,
        )
        .unwrap();
        let ignored = mkdir(root, "Scanner Junk");
        // Left out entirely.
        for name in ["_Archive", ".git", "~tmp"] {
            mkdir(root, name);
        }
        set_hidden(&mkdir(root, "Hidden Stuff"));
        std::fs::write(root.join("notes.txt"), "x").unwrap();

        let progress = Progress::default();
        let out = scan_root(&input(root, vec![ignored]), &progress).unwrap();

        let s = |n| status_of(&out, n);
        assert_eq!(s("Smith, John (1001)").status, Status::Bound);
        assert_eq!(s("Acme LLC (1002)").status, Status::UnboundNamed);
        assert_eq!(s("Acme LLC (1002)").client_id.as_deref(), Some("1002"));
        let fuzzy = s("Jonez Mary");
        assert_eq!(fuzzy.status, Status::UnboundFuzzy);
        assert_eq!(fuzzy.client_id.as_deref(), Some("1003"));
        assert!(fuzzy.score.unwrap() >= 0.90);
        assert_eq!(s("Old Client").status, Status::Orphan);
        assert_eq!(s("Old Client").client_id.as_deref(), Some("9999"));
        assert_eq!(s("Baker A").status, Status::Duplicate);
        assert_eq!(s("Baker B").status, Status::Duplicate);
        assert!(s("Baker A").note.as_ref().unwrap().contains("Baker B"));
        assert_eq!(s("Random Stuff").status, Status::Unmatched);
        assert_eq!(s("Unknown_5555").status, Status::Unmatched);
        assert!(s("Unknown_5555").note.as_ref().unwrap().contains("5555"));
        assert_eq!(s("Broken").status, Status::Unparseable);
        assert_eq!(s("Old Shape").status, Status::Unparseable);
        assert_eq!(s("Scanner Junk").status, Status::Ignored);
        assert_eq!(out.len(), 11, "{out:?}");
        assert_eq!(progress.done.load(Ordering::Relaxed), 11);
    }

    #[test]
    fn named_folder_for_already_bound_client_gets_note() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let cl = clients();
        let bound = mkdir(root, "Acme");
        write_sentinel(
            root,
            &bound,
            cl.match_by_id("1002").unwrap(),
            Uuid::new_v4(),
        )
        .unwrap();
        mkdir(root, "Acme copy_1002");
        let out = scan_root(&input(root, vec![]), &Progress::default()).unwrap();
        let named = status_of(&out, "Acme copy_1002");
        assert_eq!(named.status, Status::UnboundNamed);
        assert!(named.note.as_ref().unwrap().contains("already bound"));
    }

    fn wait(m: &mut FoldersManager, ctx: &egui::Context, data: &Path) {
        for _ in 0..500 {
            m.poll(ctx, data);
            if m.scanning().is_none() && !m.binding_in_progress() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("manager job never finished");
    }

    #[test]
    fn rebind_orphan_and_keep_duplicate_set_sentinels_aside() {
        let tmp = tempfile::tempdir().unwrap();
        let data = mkdir(tmp.path(), "data");
        let root = mkdir(tmp.path(), "root");
        let cl = clients();
        let firm = Uuid::new_v4();
        let orphan = mkdir(&root, "Old Client");
        write_sentinel(&root, &orphan, &client("9999", "Gone"), firm).unwrap();
        let (a, b) = (mkdir(&root, "Baker A"), mkdir(&root, "Baker B"));
        for d in [&a, &b] {
            write_sentinel(&root, d, cl.match_by_id("1004").unwrap(), firm).unwrap();
        }
        let ctx = egui::Context::default();
        let mut m = FoldersManager::load(&data);
        m.start_scan(&ctx, data.clone(), input(&root, vec![]));
        wait(&mut m, &ctx, &data);

        // Rebind: old sentinel kept as client.orphaned-*.json, new one for Jones.
        let jones = cl.match_by_id("1003").unwrap().clone();
        m.rebind(
            &ctx,
            data.clone(),
            root.clone(),
            orphan.clone(),
            jones,
            firm,
        );
        wait(&mut m, &ctx, &data);
        assert_eq!(
            read_sentinel(&orphan).unwrap().unwrap().tax_software_id,
            "1003"
        );
        let kept: Vec<String> = std::fs::read_dir(orphan.join(VIBE_DIR))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("client.orphaned-"))
            .collect();
        assert_eq!(kept.len(), 1, "old sentinel kept");
        assert_eq!(m.bound_folder(Some(&root), "1003"), Some(orphan.as_path()));

        // Keep first: B's sentinel set aside, A bound, B unmatched; nothing deleted.
        m.keep_duplicate(&ctx, data.clone(), root.clone(), a.clone(), vec![b.clone()]);
        wait(&mut m, &ctx, &data);
        assert!(read_sentinel(&b).unwrap().is_none());
        assert!(std::fs::read_dir(b.join(VIBE_DIR)).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("client.duplicate-")
        }));
        assert_eq!(m.bound_folder(Some(&root), "1004"), Some(a.as_path()));
        let ops = crate::oplog::read(&crate::oplog::path(&data))
            .unwrap()
            .records;
        let set_aside = ops
            .iter()
            .filter(|r| r.op == crate::oplog::Op::SentinelSetAside)
            .count();
        assert_eq!(set_aside, 2, "both set-asides logged");
    }

    #[test]
    fn id_in_folder_name_forms() {
        let re = id_regex(r"\d{4,}").unwrap();
        assert_eq!(
            id_in_name(&re, "Smith, John (1234)"),
            Some(("1234".into(), "Smith, John"))
        );
        assert_eq!(
            id_in_name(&re, "Smith_1234"),
            Some(("1234".into(), "Smith"))
        );
        assert_eq!(id_in_name(&re, "Smith 1234"), None);
        assert_eq!(id_in_name(&re, "(12) Smith"), None);
        // The default pattern also takes UltraTax letter IDs.
        let re = id_regex(crate::parse::DEFAULT_ID_PATTERN).unwrap();
        assert_eq!(
            id_in_name(&re, "John Smith (THOMREUT)"),
            Some(("THOMREUT".into(), "John Smith"))
        );
        // The Conflicts search starts from the name without its ID.
        assert_eq!(name_without_id(&re, "Smith, John (1234)"), "Smith, John");
        assert_eq!(name_without_id(&re, "Acme_1002"), "Acme");
        assert_eq!(name_without_id(&re, "Random Stuff"), "Random Stuff");
        assert_eq!(name_without_id(&re, "Under_score"), "Under_score");
    }

    #[test]
    fn manager_ignore_bind_and_lookup() {
        let tmp = tempfile::tempdir().unwrap();
        let data = mkdir(tmp.path(), "data");
        let root = mkdir(tmp.path(), "root");
        let acme = mkdir(&root, "Acme LLC (1002)");
        let junk = mkdir(&root, "Junk");
        let ctx = egui::Context::default();
        let mut m = FoldersManager::load(&data);
        m.start_scan(&ctx, data.clone(), input(&root, vec![]));
        while m.scanning().is_some() {
            m.poll(&ctx, &data);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(m.scan_of(Some(&root)).unwrap().folders.len(), 2);
        assert!(m.scan_of(Some(&data)).is_none(), "scan of another root");

        m.ignore(&data, &junk);
        m.bind(
            &ctx,
            data.clone(),
            root.clone(),
            acme.clone(),
            client("1002", "Acme LLC"),
            Uuid::new_v4(),
        );
        while m.binding_in_progress() {
            m.poll(&ctx, &data);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(m.bound_folder(Some(&root), "1002"), Some(acme.as_path()));

        // Both persisted: a fresh manager sees the same state.
        let again = FoldersManager::load(&data);
        assert_eq!(
            again.bound_folder(Some(&root), "1002"),
            Some(acme.as_path())
        );
        assert_eq!(again.ignored().len(), 1);
        let on_disk: FoldersFile =
            serde_json::from_str(&std::fs::read_to_string(data.join(FOLDERS_FILE)).unwrap())
                .unwrap();
        assert_eq!(
            on_disk.ignored,
            vec![junk.clone()],
            "ignore list is in folders.json"
        );
        let junk_status = again
            .scan_of(Some(&root))
            .unwrap()
            .folders
            .iter()
            .find(|b| b.path == junk)
            .unwrap()
            .status;
        assert_eq!(junk_status, Status::Ignored);

        // A second folder for the same client is refused without touching disk.
        let other = mkdir(&root, "Acme again");
        m.bind(
            &ctx,
            data.clone(),
            root.clone(),
            other.clone(),
            client("1002", "Acme LLC"),
            Uuid::new_v4(),
        );
        assert!(!m.binding_in_progress());
        assert!(m.note().unwrap().1.contains("already bound"));
        assert!(!sentinel_path(&other).exists());
    }
}
