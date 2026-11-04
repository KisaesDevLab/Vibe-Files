//! Executes a batch: creates client folders and sub-folders, writes sentinels, moves files
//! (rename on one volume, copy + SHA-256 verify + delete across volumes), keeps a copy of any
//! overwritten file, logs every step to `ops.jsonl`, and records `last_batch.json` for undo.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::clients::Client;
use crate::intake::{COMBINED_DIR, Source};
use crate::oplog::{self, Op, OpRecord, OpResult};
use crate::plan::{CollisionChoice, FolderAction, Proposal, RowStatus};
use crate::sentinel;
use crate::template::{self, TemplateNode};
use crate::zipimport::Extractor;

pub const LAST_BATCH_FILE: &str = "last_batch.json";
pub const OVERWRITTEN_DIR: &str = "overwritten";
/// Parked originals of combined PDFs when no inbox is set (X84 fallback).
pub const COMBINED_DATA_DIR: &str = "combined";
/// Note on a move, copy or zip entry whose name got a ` (2)` (Keep both); History shows a pill.
pub const KEPT_BOTH: &str = "kept both";

/// Copies of split originals, kept until the next batch commits so Undo can restore them (X93).
pub const SPLIT_ORIGINALS_DIR: &str = "split-originals";
/// Originals of splits that were only partly filed (survives restarts).
pub const PARTIAL_SPLITS_FILE: &str = "partial_splits.json";

// ---------------------------------------------------------------------------
// File operations

/// Renames `src` to `dst` without ever replacing an existing file. Fails with
/// `ERROR_NOT_SAME_DEVICE` (17) when they are on different volumes.
pub fn move_no_replace(src: &Path, dst: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
    let wide =
        |p: &Path| -> Vec<u16> { verbatim(p).as_os_str().encode_wide().chain([0]).collect() };
    let (s, d) = (wide(src), wide(dst));
    // SAFETY: both are valid NUL-terminated UTF-16 strings that outlive the call.
    let ok = unsafe { MoveFileExW(s.as_ptr(), d.as_ptr(), MOVEFILE_WRITE_THROUGH) };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// `\\?\` form of a long absolute path, so Win32 calls accept it (X104: destinations are still
/// limited to 240 characters; this is for sources that are already long). std's own file
/// functions do this by themselves.
fn verbatim(p: &Path) -> PathBuf {
    let s = p.as_os_str().to_string_lossy();
    if s.len() < template::MAX_PATH_LEN || s.starts_with(r"\\?\") || !p.is_absolute() {
        return p.to_path_buf();
    }
    // `\\?\` turns off Win32 normalization, so normalize first (`/` to `\`, `.` and `..`).
    let Ok(full) = std::path::absolute(p) else {
        return p.to_path_buf();
    };
    let s = full.as_os_str().to_string_lossy();
    match s.strip_prefix(r"\\") {
        Some(unc) => PathBuf::from(format!(r"\\?\UNC\{unc}")),
        None => PathBuf::from(format!(r"\\?\{s}")),
    }
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    Move,
    CopyVerifyDelete,
}

/// Moves `src` to `dst` (which must not exist). Same volume: rename. Otherwise copy to a temp
/// file next to `dst`, compare SHA-256, rename into place, then delete `src`. If `src` cannot be
/// deleted, the verified copy is removed again so nothing is duplicated.
/// `force_copy` takes the copy path even on one volume (tests).
pub fn transfer(
    src: &Path,
    dst: &Path,
    force_copy: bool,
) -> Result<(Method, u64, Option<String>), String> {
    let bytes = std::fs::metadata(src)
        .map_err(|e| format!("cannot read {}: {e}", src.display()))?
        .len();
    if !force_copy {
        match move_no_replace(src, dst) {
            Ok(()) => return Ok((Method::Move, bytes, None)),
            Err(e) if e.raw_os_error() == Some(17) => {}
            Err(e) => return Err(format!("cannot move to {}: {e}", dst.display())),
        }
    }
    let (_, sha) = copy_verified(src, dst)?;
    // The copy is verified; only now may the original go.
    if let Err(e) = std::fs::remove_file(src) {
        let _ = std::fs::remove_file(dst);
        return Err(format!(
            "copied and verified, but the original could not be removed ({e}); the copy was taken back"
        ));
    }
    Ok((Method::CopyVerifyDelete, bytes, Some(sha)))
}

/// Copies `src` to `dst` (which must not exist) through a temp file next to it, compares SHA-256,
/// then renames it into place. `src` is never touched. Returns (bytes, sha256). A partial copy is
/// removed again.
pub fn copy_verified(src: &Path, dst: &Path) -> Result<(u64, String), String> {
    let bytes = std::fs::metadata(src)
        .map_err(|e| format!("cannot read {}: {e}", src.display()))?
        .len();
    let name = dst
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dst.with_file_name(format!("~vfd-partial-{}-{name}", Uuid::new_v4().simple()));
    let copy = || -> Result<String, String> {
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|e| format!("cannot create {}: {e}", tmp.display()))?;
        let mut input =
            std::fs::File::open(src).map_err(|e| format!("cannot open {}: {e}", src.display()))?;
        std::io::copy(&mut input, &mut out).map_err(|e| format!("copy failed: {e}"))?;
        out.sync_all().map_err(|e| format!("copy failed: {e}"))?;
        drop(out);
        let a = sha256_file(src).map_err(|e| format!("cannot hash the original: {e}"))?;
        let b = sha256_file(&tmp).map_err(|e| format!("cannot hash the copy: {e}"))?;
        if a != b {
            return Err("the copy does not match the original (SHA-256)".into());
        }
        move_no_replace(&tmp, dst).map_err(|e| format!("cannot put the copy in place: {e}"))?;
        Ok(a)
    };
    match copy() {
        Ok(sha) => Ok((bytes, sha)),
        Err(e) => {
            // Our own partial copy; never the user's file.
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// `name (2).ext`, `name (3).ext`, ... next to `path`, the first that doesn't exist.
pub fn free_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path.extension().map(|e| e.to_string_lossy().into_owned());
    let dir = path.parent().unwrap_or(Path::new(""));
    (2..)
        .map(|n| match &ext {
            Some(e) => dir.join(format!("{stem} ({n}).{e}")),
            None => dir.join(format!("{stem} ({n})")),
        })
        .find(|p| !p.exists())
        .expect("unbounded range")
}

// ---------------------------------------------------------------------------
// last_batch.json

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FiledItem {
    pub src: PathBuf,
    pub dst: PathBuf,
    pub method: Method,
    pub bytes: u64,
    pub client_id: String,
    /// Where Undo puts the file: its inbox path, or the first inbox for files added by hand
    /// (X76). `None` when there was no inbox to return a hand-added file to, and for combined
    /// files (Undo deletes those and restores `merged` instead).
    pub return_to: Option<PathBuf>,
    /// For a combined PDF: its originals and where they were parked (X84).
    #[serde(default)]
    pub merged: Vec<MovedOriginal>,
    /// A combined PDF (even if none of its originals could be parked).
    #[serde(default)]
    pub combined: bool,
    /// A piece of a split PDF: the original's path. Undo deletes the piece.
    #[serde(default)]
    pub piece_of: Option<PathBuf>,
    /// Copies written to related clients before this file moved (P07a).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub copies: Vec<FiledCopy>,
    /// For a file sent to the queue from a zip: the zip (P07b).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_zip: Option<PathBuf>,
}

/// A copy the batch wrote; Undo removes it only if its hash still matches.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FiledCopy {
    pub dst: PathBuf,
    pub client_id: String,
    pub bytes: u64,
    pub sha256: String,
}

/// An original of a combined PDF, as it was when the combination was made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CombinedOriginal {
    pub path: PathBuf,
    pub source: Source,
    pub size: u64,
    pub modified: Option<std::time::SystemTime>,
}

impl CombinedOriginal {
    /// Reads the size and modification time now.
    pub fn stamp(path: &Path, source: Source) -> std::io::Result<Self> {
        let m = std::fs::metadata(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            source,
            size: m.len(),
            modified: m.modified().ok(),
        })
    }

    /// The file on disk is still the one that was combined.
    fn unchanged(&self) -> bool {
        std::fs::metadata(&self.path)
            .is_ok_and(|m| m.len() == self.size && m.modified().ok() == self.modified)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MovedOriginal {
    pub original: PathBuf,
    /// In a `_combined` folder.
    pub kept: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Backup {
    /// Where the overwritten file was.
    pub original: PathBuf,
    /// Where it was kept (under data/overwritten/<batch>).
    pub kept: PathBuf,
}

/// What the most recent commit did, for undo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LastBatch {
    pub batch: Uuid,
    pub committed_at: DateTime<Utc>,
    /// Still being written by a commit (a crash leaves this true; P10 shows it).
    pub in_progress: bool,
    pub undone: bool,
    pub items: Vec<FiledItem>,
    /// Folders the batch created, in creation order (parents first).
    pub created_dirs: Vec<PathBuf>,
    /// Sentinel files the batch wrote.
    pub created_files: Vec<PathBuf>,
    pub backups: Vec<Backup>,
    /// Split originals moved out of the inbox once all their pieces were filed (X93).
    #[serde(default)]
    pub split_originals: Vec<MovedOriginal>,
    /// Files unpacked from a zip (P07b Unpack); Undo removes them only if unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub zip_entries: Vec<FiledCopy>,
    /// The zip the batch imported and where its undo copy is kept (P07b).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zip: Option<ZipSource>,
    /// Zips sent to the queue whose files this batch finished filing: deleted, kept for undo.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routed_zips: Vec<ZipSource>,
    /// Client folders renamed to the template's name (Conflicts, V9); Undo renames them back.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub renamed_dirs: Vec<RenamedDir>,
}

/// A client folder renamed by "Rename to template name" (V9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenamedDir {
    pub client_id: String,
    pub from: PathBuf,
    pub to: PathBuf,
}

/// A zip imported by a batch: deleted after the import, kept for undo (P07b).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ZipSource {
    pub original: PathBuf,
    /// The verified copy under data/tmp/undo.
    pub kept: PathBuf,
}

pub fn last_batch_path(data_dir: &Path) -> PathBuf {
    data_dir.join(LAST_BATCH_FILE)
}

pub fn load_last_batch(data_dir: &Path) -> Option<LastBatch> {
    let text = std::fs::read_to_string(last_batch_path(data_dir)).ok()?;
    match serde_json::from_str(&text) {
        Ok(b) => Some(b),
        Err(e) => {
            tracing::error!(error = %e, "last_batch.json unreadable");
            None
        }
    }
}

pub fn save_last_batch(data_dir: &Path, lb: &LastBatch) -> anyhow::Result<()> {
    crate::datafile::save_json(&last_batch_path(data_dir), lb)
}

// ---------------------------------------------------------------------------
// Commit

/// One Ready row of the plan.
#[derive(Debug, Clone)]
pub struct CommitItem {
    pub src: PathBuf,
    pub dest: PathBuf,
    pub folder: FolderAction,
    pub client: Client,
    /// The user chose Overwrite for an existing destination.
    pub overwrite: bool,
    pub source: Source,
    /// For a combined PDF (in data/tmp): the original files, in order.
    pub merged_from: Vec<CombinedOriginal>,
    /// For a piece of a split PDF (in data/tmp): the original's path.
    pub split_of: Option<PathBuf>,
    /// Copies to related clients, written before the move (P07a).
    pub copies: Vec<CopyItem>,
    /// The destination was numbered (Keep both); logged so History can say so.
    pub kept_both: bool,
    /// Empty folders created along with the file (next year's folder, V11).
    pub also_dirs: Vec<PathBuf>,
}

/// One copy of a filed file (P07a).
#[derive(Debug, Clone)]
pub struct CopyItem {
    /// The file to copy when it is not the item's own (the unsplit original, `on_split: Whole`).
    pub src: Option<PathBuf>,
    /// The destination was numbered (Keep both).
    pub kept_both: bool,
    pub dest: PathBuf,
    pub folder: FolderAction,
    pub client: Client,
    pub overwrite: bool,
}

/// A split PDF whose pieces are in this commit.
#[derive(Debug, Clone)]
pub struct SplitCommit {
    /// The original as it was when it was split.
    pub original: CombinedOriginal,
    /// Every piece still waiting to be filed.
    pub pieces: Vec<PathBuf>,
    /// How many pieces the original was split into. The original leaves the inbox only when all
    /// of them are filed in one commit (X94).
    pub total: usize,
}

/// A split original some of whose pages were filed while the rest were not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartialSplit {
    pub original: PathBuf,
    pub batch: Uuid,
}

pub fn load_partial(data_dir: &Path) -> Vec<PartialSplit> {
    std::fs::read_to_string(data_dir.join(PARTIAL_SPLITS_FILE))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_partial(data_dir: &Path, list: &[PartialSplit]) -> anyhow::Result<()> {
    crate::datafile::save_json(&data_dir.join(PARTIAL_SPLITS_FILE), list)
}

/// Forgets the partly-filed marks a batch made (its undo removed those pieces again).
pub fn remove_partial_batch(data_dir: &Path, batch: Uuid) -> anyhow::Result<()> {
    let mut list = load_partial(data_dir);
    let before = list.len();
    list.retain(|p| p.batch != batch);
    if list.len() != before {
        save_partial(data_dir, &list)?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct CommitInput {
    pub batch: Uuid,
    pub items: Vec<CommitItem>,
    /// Skipped rows, logged as `skip`.
    pub skipped: Vec<(PathBuf, Option<String>)>,
    pub root: PathBuf,
    pub first_inbox: Option<PathBuf>,
    pub data_dir: PathBuf,
    pub firm_id: Uuid,
    pub template: TemplateNode,
    pub splits: Vec<SplitCommit>,
    pub force_copy: bool,
}

/// Where and how a batch is filed (from the saved settings).
#[derive(Debug, Clone)]
pub struct CommitTarget {
    pub root: PathBuf,
    pub first_inbox: Option<PathBuf>,
    pub data_dir: PathBuf,
    pub firm_id: Uuid,
    pub template: TemplateNode,
}

impl CommitInput {
    /// Ready rows become items, Skipped rows are logged as skips; other rows are ignored
    /// (Commit is only offered when there are none).
    pub fn from_plan(
        proposals: &[Proposal],
        groups: &HashMap<PathBuf, Vec<CombinedOriginal>>,
        splits: &[SplitCommit],
        target: CommitTarget,
    ) -> Self {
        let CommitTarget {
            root,
            first_inbox,
            data_dir,
            firm_id,
            template,
        } = target;
        let mut items = Vec::new();
        let mut skipped = Vec::new();
        for p in proposals {
            match (&p.status, &p.dest, &p.folder, &p.client) {
                (RowStatus::Ready, Some(dest), Some(folder), Some(client)) => {
                    items.push(CommitItem {
                        src: p.file.path.clone(),
                        dest: dest.clone(),
                        folder: folder.clone(),
                        client: client.clone(),
                        overwrite: p.overwrites(),
                        kept_both: p.renamed,
                        also_dirs: p.next_year_dir.iter().cloned().collect(),
                        source: p.file.source.clone(),
                        merged_from: groups.get(&p.file.path).cloned().unwrap_or_default(),
                        split_of: splits
                            .iter()
                            .find(|s| s.pieces.contains(&p.file.path))
                            .map(|s| s.original.path.clone()),
                        copies: p
                            .copies
                            .iter()
                            .filter(|c| c.status == crate::plan::CopyStatus::Ready)
                            .filter_map(|c| {
                                Some(CopyItem {
                                    src: c.whole_of.clone(),
                                    kept_both: c.renamed,
                                    dest: c.dest.clone()?,
                                    folder: c.folder.clone()?,
                                    client: c.client.clone()?,
                                    overwrite: c.collides()
                                        && c.collision_choice == CollisionChoice::Overwrite,
                                })
                            })
                            .collect(),
                    })
                }
                (RowStatus::Skipped, ..) => {
                    skipped.push((p.file.path.clone(), p.client.as_ref().map(|c| c.id.clone())))
                }
                _ => {}
            }
        }
        Self {
            batch: Uuid::new_v4(),
            items,
            skipped,
            root,
            first_inbox,
            data_dir,
            firm_id,
            template,
            splits: splits.to_vec(),
            force_copy: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileOutcome {
    pub src: PathBuf,
    pub result: Result<PathBuf, String>,
}

#[derive(Debug, Clone)]
pub struct CommitReport {
    pub outcomes: Vec<FileOutcome>,
    /// Client folders were created or bound (the folder index should be rescanned).
    pub folders_changed: bool,
    pub last_batch: LastBatch,
    /// Problems outside individual files (e.g. the log or last_batch.json could not be written).
    pub warnings: Vec<String>,
    /// Originals of combined PDFs that were filed but could not be moved out of the inbox. They
    /// must not be filed again (their pages are already in the combined PDF).
    pub unparked: Vec<PathBuf>,
    /// Split originals whose pieces were all filed (they left the inbox).
    pub split_done: Vec<PathBuf>,
}

struct Ctx<'a> {
    input: &'a CommitInput,
    log: PathBuf,
    lb: LastBatch,
    folders_done: HashMap<PathBuf, Result<(), String>>,
    warnings: Vec<String>,
    unparked: Vec<PathBuf>,
}

impl Ctx<'_> {
    /// A log entry for this batch with everything but `op` empty and result OK.
    fn entry(&self, op: Op) -> OpRecord {
        OpRecord::new(self.input.batch, op)
    }

    fn log(&mut self, rec: OpRecord) {
        if let Err(e) = oplog::append(&self.log, &rec) {
            tracing::error!(error = %e, "cannot append to ops.jsonl");
            self.warnings
                .push(format!("ops.jsonl could not be written: {e}"));
        }
    }

    fn save(&mut self) {
        if let Err(e) = save_last_batch(&self.input.data_dir, &self.lb) {
            tracing::error!(error = %e, "cannot write last_batch.json");
            self.warnings
                .push(format!("last_batch.json could not be written: {e}"));
        }
    }

    fn mkdir(&mut self, dir: &Path, client_id: &str) -> Result<(), String> {
        std::fs::create_dir(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        self.lb.created_dirs.push(dir.to_path_buf());
        let rec = OpRecord {
            client_id: Some(client_id.to_string()),
            dst: Some(dir.to_path_buf()),
            ..self.entry(Op::Mkdir)
        };
        self.log(rec);
        Ok(())
    }

    /// Creates every missing folder from `base` (which exists) down to `dir`.
    fn mkdirs_below(&mut self, base: &Path, dir: &Path, client_id: &str) -> Result<(), String> {
        let rel = dir
            .strip_prefix(base)
            .map_err(|_| format!("{} is not inside {}", dir.display(), base.display()))?;
        let mut cur = base.to_path_buf();
        for part in rel.components() {
            cur.push(part);
            if !cur.is_dir() {
                self.mkdir(&cur.clone(), client_id)?;
            }
        }
        Ok(())
    }

    fn write_sentinel(&mut self, folder: &Path, client: &Client) -> Result<(), String> {
        let vibe = folder.join(sentinel::VIBE_DIR);
        let vibe_existed = vibe.is_dir();
        sentinel::write_sentinel(&self.input.root, folder, client, self.input.firm_id)?;
        if !vibe_existed {
            self.lb.created_dirs.push(vibe);
        }
        let file = sentinel::sentinel_path(folder);
        self.lb.created_files.push(file.clone());
        let rec = OpRecord {
            client_id: Some(client.id.to_string()),
            dst: Some(file.to_path_buf()),
            ..self.entry(Op::Sentinel)
        };
        self.log(rec);
        Ok(())
    }

    /// Makes sure the client folder exists and is bound, once per folder per batch.
    fn prepare_folder(&mut self, item: &CommitItem) -> Result<(), String> {
        self.prepare(&item.folder, &item.client)
    }

    fn prepare(&mut self, folder: &FolderAction, client: &Client) -> Result<(), String> {
        let key = folder.path().to_path_buf();
        if let Some(done) = self.folders_done.get(&key) {
            return done.clone();
        }
        let result = match folder {
            FolderAction::Exists(p) => {
                if p.is_dir() {
                    Ok(())
                } else {
                    Err(format!(
                        "the client folder {} is gone; rescan the root",
                        p.display()
                    ))
                }
            }
            FolderAction::WillBind(p) => self.write_sentinel(p, client),
            FolderAction::WillCreate(p) => self.create_client_folder(p, client),
        };
        self.folders_done.insert(key, result.clone());
        result
    }

    fn create_client_folder(&mut self, folder: &Path, client: &Client) -> Result<(), String> {
        if folder.exists() {
            return Err(format!(
                "{} appeared after Preview; rescan the root and bind it in Conflicts",
                folder.display()
            ));
        }
        self.mkdir(folder, &client.id)?;
        let dirs = template::initial_dirs(&self.input.template, client)
            .map_err(|e| format!("cannot apply the folder template: {e}"))?;
        for rel in dirs.iter().skip(1) {
            let d = self.input.root.join(rel);
            if !d.is_dir() {
                self.mkdir(&d, &client.id)?;
            }
        }
        self.write_sentinel(folder, client)
    }

    fn file_one(&mut self, item: &CommitItem) -> Result<PathBuf, String> {
        let id = item.client.id.clone();
        if let Some(original) = &item.split_of
            && let Some(split) = self
                .input
                .splits
                .iter()
                .find(|s| &s.original.path == original)
            && !split.original.unchanged()
        {
            return Err(format!(
                "{} changed (or is gone) since it was split; discard the split and split again",
                original.display()
            ));
        }
        if let Some(changed) = item.merged_from.iter().find(|o| !o.unchanged()) {
            return Err(format!(
                "{} changed (or is gone) since it was combined; discard the combination and combine again",
                changed.path.display()
            ));
        }
        self.prepare_folder(item)?;
        let dir = item
            .dest
            .parent()
            .ok_or_else(|| "the destination has no folder".to_string())?
            .to_path_buf();
        self.mkdirs_below(item.folder.path(), &dir, &id)?;
        sentinel::ensure_inside(&self.input.root, &dir)?;

        if item.dest.exists() && !item.overwrite {
            return Err(format!(
                "{} appeared after Preview; nothing was overwritten",
                item.dest.display()
            ));
        }
        // Copies first (P07a); if anything after them fails they are rolled back.
        let backups_from = self.lb.backups.len();
        let copies = self.write_copies(item)?;
        if item.dest.exists()
            && let Err(e) = self.keep_overwritten(&item.dest, &id)
        {
            self.rollback(&copies, backups_from, &id);
            return Err(e);
        }
        match transfer(&item.src, &item.dest, self.input.force_copy) {
            Ok((method, bytes, sha)) => {
                let op = match method {
                    Method::Move => Op::Move,
                    Method::CopyVerifyDelete => Op::CopyVerifyDelete,
                };
                let note = match &item.source {
                    Source::Zip { zip, entry } => Some(format!(
                        "{} › {entry}",
                        zip.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    )),
                    _ => None,
                };
                let note = match (note, item.kept_both) {
                    (Some(n), true) => Some(format!("{n} · {KEPT_BOTH}")),
                    (None, true) => Some(KEPT_BOTH.to_string()),
                    (n, false) => n,
                };
                let rec = OpRecord {
                    client_id: Some(id.to_string()),
                    src: Some(item.src.to_path_buf()),
                    dst: Some(item.dest.to_path_buf()),
                    bytes: Some(bytes),
                    sha256: sha,
                    note,
                    ..self.entry(op)
                };
                self.log(rec);
                // Next year's empty folder (V11), only once the file is filed; a failure is
                // reported, the file stays filed.
                for extra in &item.also_dirs {
                    if let Err(e) = self
                        .mkdirs_below(item.folder.path(), extra, &id)
                        .and_then(|()| sentinel::ensure_inside(&self.input.root, extra))
                    {
                        self.warnings.push(format!("{}: {e}", extra.display()));
                    }
                }
                let merged = if item.merged_from.is_empty() {
                    Vec::new()
                } else {
                    self.park_originals(item)
                };
                let return_to = if !item.merged_from.is_empty() {
                    None
                } else {
                    match &item.source {
                        // Zip-routed files go back to their temp file (P07b).
                        Source::Inbox(_) | Source::Zip { .. } => Some(item.src.clone()),
                        Source::Manual => self
                            .input
                            .first_inbox
                            .as_ref()
                            .zip(item.src.file_name())
                            .map(|(inbox, name)| inbox.join(name)),
                    }
                };
                self.lb.items.push(FiledItem {
                    src: item.src.clone(),
                    dst: item.dest.clone(),
                    method,
                    bytes,
                    client_id: id,
                    return_to,
                    merged,
                    combined: !item.merged_from.is_empty(),
                    piece_of: item.split_of.clone(),
                    copies,
                    from_zip: match &item.source {
                        Source::Zip { zip, .. } => Some(zip.clone()),
                        _ => None,
                    },
                });
                if item.split_of.is_some() {
                    remove_tmp_dirs(&self.input.data_dir, &item.src);
                }
                Ok(item.dest.clone())
            }
            Err(e) => {
                self.rollback(&copies, backups_from, &id);
                Err(e)
            }
        }
    }

    /// Writes every copy of `item`, verified. On the first failure the copies already written are
    /// removed again and the source stays where it is.
    fn write_copies(&mut self, item: &CommitItem) -> Result<Vec<FiledCopy>, String> {
        let backups_from = self.lb.backups.len();
        let mut written: Vec<FiledCopy> = Vec::new();
        for c in &item.copies {
            let result = (|| -> Result<FiledCopy, String> {
                self.prepare(&c.folder, &c.client)?;
                let dir = c
                    .dest
                    .parent()
                    .ok_or_else(|| "the copy has no folder".to_string())?
                    .to_path_buf();
                self.mkdirs_below(c.folder.path(), &dir, &c.client.id)?;
                sentinel::ensure_inside(&self.input.root, &dir)?;
                if c.dest.exists() {
                    if !c.overwrite {
                        return Err(format!(
                            "{} appeared after Preview; nothing was overwritten",
                            c.dest.display()
                        ));
                    }
                    self.keep_overwritten(&c.dest, &c.client.id)?;
                }
                let src = c.src.as_ref().unwrap_or(&item.src);
                let (bytes, sha) = copy_verified(src, &c.dest)?;
                let rec = OpRecord {
                    client_id: Some(c.client.id.clone()),
                    src: Some(src.clone()),
                    dst: Some(c.dest.clone()),
                    bytes: Some(bytes),
                    sha256: Some(sha.clone()),
                    copy_of: Some(item.client.id.clone()),
                    note: c.kept_both.then(|| KEPT_BOTH.to_string()),
                    ..self.entry(Op::Copy)
                };
                self.log(rec);
                Ok(FiledCopy {
                    dst: c.dest.clone(),
                    client_id: c.client.id.clone(),
                    bytes,
                    sha256: sha,
                })
            })();
            match result {
                Ok(f) => written.push(f),
                Err(e) => {
                    self.rollback(&written, backups_from, &item.client.id);
                    return Err(format!(
                        "copy to {} failed: {e}; the source stays",
                        c.client.name
                    ));
                }
            }
        }
        Ok(written)
    }

    /// Removes copies written for a file whose later step failed, and puts back any file this
    /// file's steps had moved aside to overwrite.
    fn rollback(&mut self, written: &[FiledCopy], backups_from: usize, primary: &str) {
        for c in written {
            // Allowed (CLAUDE.md §7): a copy being rolled back in the same batch.
            let result = std::fs::remove_file(&c.dst);
            let rec = OpRecord {
                client_id: Some(c.client_id.clone()),
                dst: Some(c.dst.clone()),
                copy_of: Some(primary.to_string()),
                result: if result.is_ok() {
                    OpResult::Ok
                } else {
                    OpResult::Error
                },
                error: result.as_ref().err().map(|e| e.to_string()),
                ..self.entry(Op::Rollback)
            };
            self.log(rec);
            if let Err(e) = result {
                self.warnings.push(format!(
                    "the copy {} could not be removed after a failure: {e}",
                    c.dst.display()
                ));
            }
        }
        for b in self
            .lb
            .backups
            .split_off(backups_from.min(self.lb.backups.len()))
        {
            if let Err(e) = move_no_replace(&b.kept, &b.original) {
                self.warnings.push(format!(
                    "{} could not be put back ({e}); it is kept at {}",
                    b.original.display(),
                    b.kept.display()
                ));
            }
        }
    }

    /// After a combined PDF is filed: moves each original into a `_combined` folder (its
    /// inbox's; hand-added files: the first inbox's; with no inbox, `data/combined/<batch>` so
    /// nothing is written outside root/inboxes/data; X84) and removes the empty temp folder the
    /// combined file was built in. Originals that can't be moved are listed in `unparked`.
    fn park_originals(&mut self, item: &CommitItem) -> Vec<MovedOriginal> {
        let mut moved = Vec::new();
        for o in &item.merged_from {
            let dir = match &o.source {
                Source::Inbox(inbox) => inbox.join(COMBINED_DIR),
                Source::Manual | Source::Zip { .. } => match &self.input.first_inbox {
                    Some(inbox) => inbox.join(COMBINED_DIR),
                    None => self
                        .input
                        .data_dir
                        .join(COMBINED_DATA_DIR)
                        .join(self.input.batch.to_string()),
                },
            };
            let parked = (|| -> Result<PathBuf, String> {
                if !dir.is_dir() {
                    let mut missing = Vec::new();
                    let mut cur = dir.as_path();
                    while !cur.is_dir() {
                        missing.push(cur.to_path_buf());
                        cur = cur.parent().ok_or("no parent folder")?;
                    }
                    for d in missing.into_iter().rev() {
                        std::fs::create_dir(&d)
                            .map_err(|e| format!("cannot create {}: {e}", d.display()))?;
                        self.lb.created_dirs.push(d);
                    }
                }
                let name = o.path.file_name().ok_or("no file name")?;
                let kept = free_path(&dir.join(name));
                let (_, bytes, sha) = transfer(&o.path, &kept, self.input.force_copy)?;
                let rec = OpRecord {
                    client_id: Some(item.client.id.clone()),
                    src: Some(o.path.clone()),
                    dst: Some(kept.clone()),
                    bytes: Some(bytes),
                    sha256: sha,
                    ..self.entry(Op::Merge)
                };
                self.log(rec);
                Ok(kept)
            })();
            match parked {
                Ok(kept) => moved.push(MovedOriginal {
                    original: o.path.clone(),
                    kept,
                }),
                Err(e) => {
                    self.warnings.push(format!(
                        "{} is in the filed combined PDF but could not be moved out of the inbox ({e}); it is marked Skip so it is not filed twice",
                        o.path.display()
                    ));
                    self.unparked.push(o.path.clone());
                }
            }
        }
        if let Some(tmp_dir) = item.src.parent()
            && tmp_dir.starts_with(self.input.data_dir.join(crate::pdf::TMP_DIR))
        {
            let _ = std::fs::remove_dir(tmp_dir);
        }
        moved
    }

    /// Moves an existing destination file into data/overwritten/<batch>/ (X74).
    fn keep_overwritten(&mut self, dest: &Path, client_id: &str) -> Result<(), String> {
        let dir = self
            .input
            .data_dir
            .join(OVERWRITTEN_DIR)
            .join(self.input.batch.to_string());
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let name = dest
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let kept = free_path(&dir.join(format!("{}-{name}", self.lb.backups.len() + 1)));
        let (_, bytes, sha) = transfer(dest, &kept, self.input.force_copy)
            .map_err(|e| format!("cannot keep a copy of the file being overwritten: {e}"))?;
        let rec = OpRecord {
            client_id: Some(client_id.to_string()),
            src: Some(dest.to_path_buf()),
            dst: Some(kept.to_path_buf()),
            bytes: Some(bytes),
            sha256: sha,
            ..self.entry(Op::Backup)
        };
        self.log(rec);
        self.lb.backups.push(Backup {
            original: dest.to_path_buf(),
            kept,
        });
        Ok(())
    }
}

/// Removes the overwritten-file copies of earlier batches (undo is last-batch only, X74). Only
/// files, then empty folders; never a recursive delete.
fn prune_overwritten(data_dir: &Path, keep: Uuid) {
    prune_batch_dirs(&data_dir.join(OVERWRITTEN_DIR), keep);
    prune_batch_dirs(&data_dir.join(SPLIT_ORIGINALS_DIR), keep);
}

/// Removes the files in each `<base>/<batch>/` except `keep`'s, then the empty folders.
fn prune_batch_dirs(base: &Path, keep: Uuid) {
    let Ok(batches) = std::fs::read_dir(base) else {
        return;
    };
    for b in batches.flatten() {
        let dir = b.path();
        if !dir.is_dir() || b.file_name().to_string_lossy() == keep.to_string() {
            continue;
        }
        if let Ok(files) = std::fs::read_dir(&dir) {
            for f in files.flatten() {
                if f.path().is_file() {
                    let _ = std::fs::remove_file(f.path());
                }
            }
        }
        let _ = std::fs::remove_dir(&dir);
    }
}

/// Empty temp folders a split piece was built in (`data/tmp/<id>/<n>/`).
pub fn remove_tmp_dirs(data_dir: &Path, piece: &Path) {
    let tmp = data_dir.join(crate::pdf::TMP_DIR);
    let mut dir = piece.parent();
    while let Some(d) = dir {
        if d == tmp || !d.starts_with(&tmp) {
            break;
        }
        match std::fs::remove_dir(d) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => break, // not empty yet: other pieces still there
        }
        dir = d.parent();
    }
}

/// For each split whose pieces were all filed now, moves the original out of the inbox into
/// data/split-originals/<batch>/ (X93). Otherwise the original stays and a warning says so (X94).
fn finish_splits(ctx: &mut Ctx<'_>, outcomes: &[FileOutcome]) -> Vec<PathBuf> {
    let mut done = Vec::new();
    for split in ctx.input.splits.clone() {
        let filed = split
            .pieces
            .iter()
            .filter(|p| outcomes.iter().any(|o| &o.src == *p && o.result.is_ok()))
            .count();
        let original = &split.original.path;
        let complete = split.pieces.len() == split.total && filed == split.pieces.len();
        if !complete {
            if filed > 0 {
                ctx.warnings.push(format!(
                    "{filed} of {} pieces of {} were filed; the original stays in the inbox, marked as partly filed",
                    split.total,
                    original.display()
                ));
                let mut list = load_partial(&ctx.input.data_dir);
                list.push(PartialSplit {
                    original: original.clone(),
                    batch: ctx.input.batch,
                });
                if let Err(e) = save_partial(&ctx.input.data_dir, &list) {
                    ctx.warnings
                        .push(format!("partial_splits.json could not be written: {e}"));
                }
            }
            continue;
        }
        let dir = ctx
            .input
            .data_dir
            .join(SPLIT_ORIGINALS_DIR)
            .join(ctx.input.batch.to_string());
        let kept = std::fs::create_dir_all(&dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))
            .and_then(|()| {
                let name = original.file_name().ok_or("no file name")?;
                let kept = free_path(&dir.join(name));
                transfer(original, &kept, ctx.input.force_copy)?;
                Ok(kept)
            });
        match kept {
            Ok(kept) => {
                let rec = OpRecord {
                    src: Some(original.clone()),
                    dst: Some(kept.clone()),
                    bytes: Some(split.original.size),
                    ..ctx.entry(Op::Split)
                };
                ctx.log(rec);
                ctx.lb.split_originals.push(MovedOriginal {
                    original: original.clone(),
                    kept,
                });
                done.push(original.clone());
            }
            Err(e) => ctx.warnings.push(format!(
                "all pieces of {} were filed, but the original could not be moved out of the inbox ({e}); it is marked Skip",
                original.display()
            )),
        }
    }
    done
}

// ---------------------------------------------------------------------------
// Zip import, Unpack mode (P07b)

/// Where undo copies of imported zips are kept (data/tmp/undo/<batch>.zip).
pub const UNDO_ZIPS_DIR: &str = "tmp/undo";

/// One zip entry to unpack.
#[derive(Debug, Clone)]
pub struct UnpackEntry {
    pub index: usize,
    /// Path inside the zip (`a/b/c.pdf`), also the path below the destination folder.
    pub rel: String,
    pub dest: PathBuf,
    /// What to do if `dest` exists when unpacking.
    pub choice: CollisionChoice,
}

#[derive(Debug, Clone)]
pub struct UnpackInput {
    pub zip: PathBuf,
    pub client: Client,
    pub folder: FolderAction,
    /// The client folder, or the sub-folder chosen in the draft.
    pub dest_dir: PathBuf,
    pub entries: Vec<UnpackEntry>,
    /// Entries left out by the pre-scan (junk, blocked, unsafe), for the summary.
    pub left_out: String,
    pub target: CommitTarget,
}

#[derive(Debug, Clone)]
pub struct UnpackReport {
    pub written: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
    /// The unpack stopped early (destination unwritable); nothing after it was tried.
    pub stopped: Option<String>,
    pub zip_deleted: bool,
    pub folders_changed: bool,
    pub last_batch: LastBatch,
    pub warnings: Vec<String>,
}

/// Unpacks a zip into a client folder keeping its folder structure: each entry through a temp
/// file with SHA-256 and a rename, collisions as chosen, every step logged. Then the zip is
/// copied (verified) to data/tmp/undo and removed from where it was. Undo removes unchanged
/// entries, created folders if empty, and puts the zip back.
pub fn run_unpack(input: &UnpackInput, progress: &AtomicUsize) -> UnpackReport {
    let batch = Uuid::new_v4();
    let t = &input.target;
    prune_overwritten(&t.data_dir, batch);
    prune_undo_zips(&t.data_dir, batch);
    let commit_input = CommitInput {
        batch,
        items: Vec::new(),
        skipped: Vec::new(),
        root: t.root.clone(),
        first_inbox: t.first_inbox.clone(),
        data_dir: t.data_dir.clone(),
        firm_id: t.firm_id,
        template: t.template.clone(),
        splits: Vec::new(),
        force_copy: false,
    };
    let mut ctx = Ctx {
        input: &commit_input,
        log: oplog::path(&t.data_dir),
        lb: LastBatch {
            batch,
            committed_at: Utc::now(),
            in_progress: true,
            undone: false,
            items: Vec::new(),
            created_dirs: Vec::new(),
            created_files: Vec::new(),
            backups: Vec::new(),
            split_originals: Vec::new(),
            zip_entries: Vec::new(),
            zip: None,
            routed_zips: Vec::new(),
            renamed_dirs: Vec::new(),
        },
        folders_done: HashMap::new(),
        warnings: Vec::new(),
        unparked: Vec::new(),
    };
    ctx.save();
    let id = input.client.id.clone();
    let mut written = 0;
    let mut skipped = 0;
    let mut errors = Vec::new();
    let mut stopped = None;
    let folders_changed = !matches!(input.folder, FolderAction::Exists(_));

    match ctx.prepare(&input.folder, &input.client) {
        Err(e) => stopped = Some(e),
        Ok(()) => match Extractor::open(&input.zip) {
            Err(e) => stopped = Some(e),
            Ok(mut zip) => {
                for entry in &input.entries {
                    progress.fetch_add(1, Ordering::Relaxed);
                    if let Err(e) = crate::template::checked_join(Path::new(""), &entry.dest) {
                        errors.push(format!("{}: {e}", entry.rel));
                        continue;
                    }
                    let Some(parent) = entry.dest.parent().map(Path::to_path_buf) else {
                        errors.push(format!("{}: no folder", entry.rel));
                        continue;
                    };
                    // A folder that can't be made means the destination is unwritable: stop.
                    if let Err(e) = ctx
                        .mkdirs_below(input.folder.path(), &parent, &id)
                        .and_then(|()| sentinel::ensure_inside(&t.root, &parent))
                    {
                        stopped = Some(e);
                        break;
                    }
                    let mut dest = entry.dest.clone();
                    let mut renamed = false;
                    if dest.exists() {
                        match entry.choice {
                            CollisionChoice::Skip => {
                                skipped += 1;
                                let rec = OpRecord {
                                    client_id: Some(id.clone()),
                                    src: Some(input.zip.clone()),
                                    dst: Some(dest.clone()),
                                    note: Some(entry.rel.clone()),
                                    ..ctx.entry(Op::Skip)
                                };
                                ctx.log(rec);
                                continue;
                            }
                            CollisionChoice::Overwrite => {
                                if let Err(e) = ctx.keep_overwritten(&dest, &id) {
                                    errors.push(format!("{}: {e}", entry.rel));
                                    continue;
                                }
                            }
                            CollisionChoice::KeepBoth => {
                                dest = free_path(&dest);
                                renamed = true;
                            }
                        }
                    }
                    match zip.extract(entry.index, &dest) {
                        Ok((bytes, sha)) => {
                            written += 1;
                            let rec = OpRecord {
                                client_id: Some(id.clone()),
                                src: Some(input.zip.clone()),
                                dst: Some(dest.clone()),
                                bytes: Some(bytes),
                                sha256: Some(sha.clone()),
                                note: Some(if renamed {
                                    format!("{} · {KEPT_BOTH}", entry.rel)
                                } else {
                                    entry.rel.clone()
                                }),
                                ..ctx.entry(Op::ZipEntry)
                            };
                            ctx.log(rec);
                            ctx.lb.zip_entries.push(FiledCopy {
                                dst: dest,
                                client_id: id.clone(),
                                bytes,
                                sha256: sha,
                            });
                            ctx.save();
                        }
                        Err(e) => {
                            let rec = OpRecord {
                                client_id: Some(id.clone()),
                                src: Some(input.zip.clone()),
                                dst: Some(dest.clone()),
                                result: OpResult::Error,
                                error: Some(e.clone()),
                                note: Some(entry.rel.clone()),
                                ..ctx.entry(Op::ZipEntry)
                            };
                            ctx.log(rec);
                            errors.push(format!("{}: {e}", entry.rel));
                            if zip.over_total() {
                                stopped = Some(e);
                                break;
                            }
                        }
                    }
                }
            }
        },
    }

    // The zip goes once something was unpacked, after a verified copy is kept for undo.
    let mut zip_deleted = false;
    if written > 0 && stopped.is_none() {
        let dir = t.data_dir.join(UNDO_ZIPS_DIR);
        let kept = dir.join(format!("{batch}.zip"));
        // Moved, or copied and hash-verified before the original goes (CLAUDE.md §7, X139).
        let keep = std::fs::create_dir_all(&dir)
            .map_err(|e| e.to_string())
            .and_then(|()| transfer(&input.zip, &kept, false));
        match keep {
            Ok(_) => {
                zip_deleted = true;
                ctx.lb.zip = Some(ZipSource {
                    original: input.zip.clone(),
                    kept,
                });
            }
            Err(e) => ctx.warnings.push(format!(
                "no undo copy of the zip could be kept ({e}); it stays where it is"
            )),
        }
    }
    let mut note = format!("unpack · {written} written");
    if skipped > 0 {
        note.push_str(&format!(" · {skipped} skipped"));
    }
    if !input.left_out.is_empty() {
        note.push_str(&format!(" · {}", input.left_out));
    }
    if !errors.is_empty() {
        note.push_str(&format!(" · {} failed", errors.len()));
    }
    note.push_str(if zip_deleted {
        " · zip deleted, undo copy kept"
    } else {
        " · zip kept"
    });
    let rec = OpRecord {
        client_id: Some(id.clone()),
        src: Some(input.zip.clone()),
        dst: Some(input.dest_dir.clone()),
        result: if stopped.is_some() {
            OpResult::Error
        } else {
            OpResult::Ok
        },
        error: stopped.clone(),
        note: Some(note),
        ..ctx.entry(Op::ZipImport)
    };
    ctx.log(rec);
    ctx.lb.in_progress = false;
    ctx.save();
    UnpackReport {
        written,
        skipped,
        errors,
        stopped,
        zip_deleted,
        folders_changed,
        last_batch: ctx.lb,
        warnings: ctx.warnings,
    }
}

/// Removes undo copies of zips from earlier batches (same lifetime as last_batch.json).
fn prune_undo_zips(data_dir: &Path, keep: Uuid) {
    let Ok(entries) = std::fs::read_dir(data_dir.join(UNDO_ZIPS_DIR)) else {
        return;
    };
    // `<batch>.zip` (Unpack) and `<batch>-<n>.zip` (zips sent to the queue).
    let keep = keep.to_string();
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if p.is_file() && name.ends_with(".zip") && !name.starts_with(&keep) {
            // Our own undo copy of an older batch.
            let _ = std::fs::remove_file(&p);
        }
    }
}

/// Runs the batch. Each file is independent: a failure is logged and reported and the rest
/// continue (X75). `progress` counts finished files.
pub fn run_commit(input: &CommitInput, progress: &AtomicUsize) -> CommitReport {
    prune_overwritten(&input.data_dir, input.batch);
    prune_undo_zips(&input.data_dir, input.batch);
    let mut ctx = Ctx {
        input,
        log: oplog::path(&input.data_dir),
        lb: LastBatch {
            batch: input.batch,
            committed_at: Utc::now(),
            in_progress: true,
            undone: false,
            items: Vec::new(),
            created_dirs: Vec::new(),
            created_files: Vec::new(),
            backups: Vec::new(),
            split_originals: Vec::new(),
            zip_entries: Vec::new(),
            zip: None,
            routed_zips: Vec::new(),
            renamed_dirs: Vec::new(),
        },
        folders_done: HashMap::new(),
        warnings: Vec::new(),
        unparked: Vec::new(),
    };
    ctx.save();
    tracing::info!(batch = %input.batch, files = input.items.len(), "commit started");

    let mut outcomes = Vec::with_capacity(input.items.len());
    for item in &input.items {
        let result = ctx.file_one(item);
        if let Err(e) = &result {
            tracing::warn!(file = %item.src.display(), error = %e, "file not filed");
            let rec = OpRecord {
                client_id: Some(item.client.id.to_string()),
                src: Some(item.src.to_path_buf()),
                dst: Some(item.dest.to_path_buf()),
                result: OpResult::Error,
                error: Some(e.to_string()),
                ..ctx.entry(Op::Move)
            };
            ctx.log(rec);
        }
        outcomes.push(FileOutcome {
            src: item.src.clone(),
            result,
        });
        ctx.save();
        progress.fetch_add(1, Ordering::Relaxed);
    }
    let split_done = finish_splits(&mut ctx, &outcomes);
    for (src, client) in &input.skipped {
        let rec = OpRecord {
            client_id: client.as_deref().map(str::to_string),
            src: Some(src.to_path_buf()),
            ..ctx.entry(Op::Skip)
        };
        ctx.log(rec);
    }
    ctx.lb.in_progress = false;
    ctx.save();
    let folders_changed = !ctx.lb.created_files.is_empty();
    tracing::info!(
        batch = %input.batch,
        filed = outcomes.iter().filter(|o| o.result.is_ok()).count(),
        failed = outcomes.iter().filter(|o| o.result.is_err()).count(),
        "commit finished"
    );
    CommitReport {
        outcomes,
        folders_changed,
        last_batch: ctx.lb,
        warnings: ctx.warnings,
        unparked: ctx.unparked,
        split_done,
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[test]
    fn transfer_moves_and_never_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.pdf"), dir.path().join("b.pdf"));
        std::fs::write(&a, b"one").unwrap();
        assert_eq!(transfer(&a, &b, false).unwrap().0, Method::Move);
        assert!(!a.exists() && b.exists());
        std::fs::write(&a, b"two").unwrap();
        assert!(
            transfer(&a, &b, false).is_err(),
            "existing destination is never replaced"
        );
        assert_eq!(std::fs::read(&b).unwrap(), b"one");
        assert!(a.exists());
    }

    #[test]
    fn copy_path_verifies_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.pdf"), dir.path().join("b.pdf"));
        std::fs::write(&a, b"payload").unwrap();
        let (method, bytes, sha) = transfer(&a, &b, true).unwrap();
        assert_eq!((method, bytes), (Method::CopyVerifyDelete, 7));
        assert_eq!(sha.unwrap(), sha256_file(&b).unwrap());
        assert!(!a.exists());
        // Destination exists: no partial file left behind, original kept.
        std::fs::write(&a, b"again").unwrap();
        assert!(transfer(&a, &b, true).is_err());
        assert!(a.exists());
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("~vfd-partial"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn source_that_cannot_be_deleted_is_not_duplicated() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.pdf"), dir.path().join("b.pdf"));
        std::fs::write(&a, b"locked").unwrap();
        // Hold the source open without delete sharing: copying works, deleting does not.
        use std::os::windows::fs::OpenOptionsExt;
        let _hold = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1) // FILE_SHARE_READ only
            .open(&a)
            .unwrap();
        let err = transfer(&a, &b, true).unwrap_err();
        assert!(err.contains("taken back"), "{err}");
        assert!(a.exists() && !b.exists());
    }

    #[test]
    fn long_paths_get_the_verbatim_prefix() {
        let short = Path::new(r"C:\a\b.pdf");
        assert_eq!(verbatim(short), short);
        let long = PathBuf::from(format!(r"C:\{}\b.pdf", "x".repeat(250)));
        assert!(verbatim(&long).to_string_lossy().starts_with(r"\\?\C:\"));
        let unc = PathBuf::from(format!(r"\\server\share\{}.pdf", "y".repeat(250)));
        assert!(
            verbatim(&unc)
                .to_string_lossy()
                .starts_with(r"\\?\UNC\server\share\")
        );
        // Forward slashes and `..` are normalized before the prefix turns normalization off.
        let mixed = PathBuf::from(format!(r"C:/{}/sub/../b.pdf", "x".repeat(250)));
        assert_eq!(
            verbatim(&mixed),
            PathBuf::from(format!(r"\\?\C:\{}\b.pdf", "x".repeat(250)))
        );
        // A real move between long paths works.
        let dir = tempfile::tempdir().unwrap();
        let deep = dir.path().join("d".repeat(120)).join("e".repeat(120));
        std::fs::create_dir_all(&deep).unwrap();
        let (a, b) = (deep.join("a.pdf"), deep.join("b.pdf"));
        std::fs::write(&a, b"x").unwrap();
        assert!(a.as_os_str().len() > 240);
        move_no_replace(&a, &b).unwrap();
        assert!(b.exists() && !a.exists());
    }

    #[test]
    fn free_path_numbers() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.pdf");
        assert_eq!(free_path(&p), p);
        std::fs::write(&p, b"1").unwrap();
        assert_eq!(free_path(&p), dir.path().join("x (2).pdf"));
    }
}
