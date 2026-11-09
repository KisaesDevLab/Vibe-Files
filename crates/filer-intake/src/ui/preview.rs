//! Preview tab (UI_SPEC §3): the batch plan (one row per queued PDF) with client, rule, year,
//! destination and status; per-row fixes (right-click a file for Skip / Split… / Change client);
//! batch year, collision default and keep-original switch; the Commit modal (UI_SPEC §4).
//! A side pane shows the pages of the row last clicked, and ticked rows can be given one client
//! or one rule at once (owner 2026-11-04).

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Receiver;

use chrono::Datelike;
use egui::{Align, Color32, CornerRadius, Frame, Layout, Margin, RichText, Stroke};
use egui_extras::{Column, TableBuilder};

use crate::app::AppState;
use crate::commit::{self, CombinedOriginal, CommitInput, CommitReport, SplitCommit};
use crate::config::Level;
use crate::intake::{QueuedFile, Source};
use crate::parse::{YEARS_AHEAD, YEARS_BACK};
use crate::pdf;
use crate::plan::{
    self, BatchSettings, ClientMatch, CollisionChoice, CollisionWith, CopyMatch, CopyProposal,
    CopyStatus, FolderAction, PlanInput, PlanOutput, Proposal, Rescan, RowEdits, RowStatus,
    RuleSource,
};
use crate::ui::peek::{self, PagePane};
use crate::ui::split::{PieceSpec, SplitContext, SplitDialog, SplitOutcome};
use crate::ui::theme::{
    self, PillKind, banner, file_name, human_size, label_caps, mono, pal, pill, plural,
    primary_button, strong,
};
use crate::ui::widgets::{PickerOutcome, client_picker_or_new};
use crate::worker;

/// The side pane opens by itself in a window at least this wide, so the table keeps the room its
/// columns need; narrower, "Preview pane" opens it.
const PANE_AUTO_WIDTH: f32 = 1800.0;

/// (file, page count or `None` when unreadable).
type PageCounts = Vec<(PathBuf, (u64, Option<usize>))>;

pub struct PreviewScreen {
    /// "My inbox" tab (this person's own inboxes) instead of "Shared inbox"; only offered
    /// when the host gave personal inboxes. Each tab is its own batch.
    mine: bool,
    /// Files in this batch, in queue order. `None`: take the whole queue on the next frame.
    batch_files: Option<Vec<PathBuf>>,
    edits: HashMap<PathBuf, RowEdits>,
    batch: BatchSettings,
    proposals: Vec<Proposal>,
    job: Option<Receiver<PlanOutput>>,
    /// Fingerprint of the inputs of the last plan started.
    built_for: Option<u64>,
    rescan: Rescan,
    /// Row whose client is being picked, and the search text.
    picker: Option<(PathBuf, String, PickFor)>,
    /// File name being edited in place (V5).
    renaming: Option<Renaming>,
    /// "Change folder…" dialog (V6).
    folder_pick: Option<FolderPick>,
    /// Rows whose copy sub-rows are folded away.
    folded: Vec<PathBuf>,
    /// Page counts of queued PDFs (`None`: unreadable), read on a worker (UI_SPEC §3 `· N pages`).
    page_counts: HashMap<PathBuf, (u64, Option<usize>)>,
    page_job: Option<Receiver<PageCounts>>,
    /// Open the summary when the plan being built arrives.
    summary_pending: bool,
    summary_open: bool,
    commit_job: Option<CommitJob>,
    /// Combined PDFs built in data/tmp, each standing in for its member rows.
    groups: Vec<Group>,
    /// Ticked rows, in the order they were ticked (X86).
    selected: Vec<PathBuf>,
    combine: Option<CombineDialog>,
    merge_job: Option<Receiver<Result<Group, String>>>,
    /// Measured height of each table line, used from the next frame (the table's rows have a
    /// height before they are drawn).
    row_heights: HashMap<LineKey, f32>,
    /// The table's width bucket last frame; a new one shares the columns out again.
    table_width_key: Option<i64>,
    /// Images being turned into PDFs (V25), by image path.
    convert_jobs: Vec<(PathBuf, Receiver<Result<Group, String>>)>,
    /// Images filed as they are: "Keep as image" on the row, or the conversion failed.
    keep_image: std::collections::HashSet<PathBuf>,
    /// When converted images were last checked for changes on disk.
    stamp_checked: Option<std::time::Instant>,
    /// Split PDFs: their pieces (in data/tmp) stand in for the original row.
    splits: Vec<SplitGroup>,
    split_dialog: Option<SplitDialog>,
    split_job: Option<Receiver<Result<SplitGroup, String>>>,
    /// Split originals already partly filed (data/partial_splits.json), and the log generation
    /// it was read at.
    partial: Vec<PathBuf>,
    partial_generation: Option<u64>,
    /// The row under the mouse pointer (Del skips it).
    hovered: Option<PathBuf>,
    message: Option<(Level, String)>,
    /// The row whose pages the side pane shows (the file name last clicked; the first row
    /// until then).
    focused: Option<PathBuf>,
    /// The side pane is shown (this frame).
    pane_open: bool,
    /// Turned on or off with "Preview pane"; until then it is open when the window has room.
    pane_choice: Option<bool>,
    pane: Option<PagePane>,
}

/// A combined PDF and the queued files it was made from.
#[derive(Debug, Clone)]
struct Group {
    merged: PathBuf,
    members: Vec<QueuedFile>,
    /// Each member's size and time when combined; Commit refuses if a member changed since.
    stamps: Vec<CombinedOriginal>,
    /// Size of the combined PDF (kept so the UI never reads the disk for it).
    size: u64,
    pages: usize,
    added: chrono::DateTime<chrono::Local>,
    /// Edits the combined row starts with (from the first member).
    edits: RowEdits,
    /// An image turned into a PDF (V25), not files combined: one member, labelled as converted.
    converted: bool,
}

/// A PDF split into pieces.
#[derive(Debug, Clone)]
struct SplitGroup {
    original: QueuedFile,
    /// Pieces at split time; the original leaves the inbox only if all are filed at once.
    total: usize,
    /// The original as it was when split; Commit refuses its pieces if it changed since.
    stamp: CombinedOriginal,
    pieces: Vec<Piece>,
}

#[derive(Debug, Clone)]
struct Piece {
    path: PathBuf,
    pages: std::ops::Range<usize>,
    size: u64,
    edits: RowEdits,
}

/// The Combine dialog: files in order and the output name.
struct CombineDialog {
    order: Vec<PathBuf>,
    name: String,
    client_id: String,
    /// Page count per file, read on a worker when the dialog opens.
    pages: HashMap<PathBuf, usize>,
    pages_job: Option<Receiver<Vec<(PathBuf, usize)>>>,
}

/// What the client picker is choosing.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PickFor {
    Primary,
    /// The client of one copy (its key).
    Copy(String),
    /// A new copy ("+ Copy to…").
    NewCopy,
    /// Every ticked row.
    Ticked,
}

/// A commit running on a worker thread.
struct CommitJob {
    rx: Receiver<CommitReport>,
    progress: Arc<AtomicUsize>,
    total: usize,
}

impl Default for PreviewScreen {
    fn default() -> Self {
        Self {
            mine: false,
            batch_files: None,
            edits: HashMap::new(),
            batch: BatchSettings::default(),
            proposals: Vec::new(),
            job: None,
            built_for: None,
            rescan: Rescan::NotYet,
            picker: None,
            renaming: None,
            folder_pick: None,
            summary_pending: false,
            summary_open: false,
            commit_job: None,
            groups: Vec::new(),
            selected: Vec::new(),
            combine: None,
            merge_job: None,
            row_heights: HashMap::new(),
            table_width_key: None,
            convert_jobs: Vec::new(),
            keep_image: std::collections::HashSet::new(),
            stamp_checked: None,
            splits: Vec::new(),
            split_dialog: None,
            split_job: None,
            partial: Vec::new(),
            partial_generation: None,
            hovered: None,
            folded: Vec::new(),
            page_counts: HashMap::new(),
            page_job: None,
            message: None,
            focused: None,
            pane_open: false,
            pane_choice: None,
            pane: None,
        }
    }
}

impl PreviewScreen {
    #[cfg(test)]
    pub fn proposals(&self) -> &[Proposal] {
        &self.proposals
    }

    #[cfg(test)]
    pub fn plan_idle(&self) -> bool {
        self.job.is_none()
    }

    /// Images in the queue are turned into PDFs on a worker when Setup says so (V25); each
    /// becomes a one-member group, so the combine path files the PDF, parks the image in
    /// `_combined`, and Undo puts it back.
    fn convert_images(&mut self, ctx: &egui::Context, state: &AppState) {
        let mut i = 0;
        while i < self.convert_jobs.len() {
            let result = match self.convert_jobs[i].1.try_recv() {
                Ok(r) => r,
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    i += 1;
                    continue;
                }
                // The worker stopped without an answer (a decoder panic): never wait on it.
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Err("the conversion stopped unexpectedly".to_string())
                }
            };
            let (image, _) = self.convert_jobs.remove(i);
            let queued = state.intake.queue().iter().any(|q| q.path == image);
            match result {
                Ok(mut group) if queued && state.settings.images_to_pdf => {
                    tracing::info!(image = %image.display(), pages = group.pages, "image to PDF");
                    // What was set on the image's row while it converted carries over.
                    if let Some(e) = self.edits.get(&image) {
                        group.edits = e.clone();
                    }
                    self.edits.insert(group.merged.clone(), group.edits.clone());
                    if let Some(k) = self.selected.iter().position(|p| p == &image) {
                        self.selected[k] = group.merged.clone();
                    }
                    if self.focused.as_ref() == Some(&image) {
                        self.focused = Some(group.merged.clone());
                    }
                    self.groups.push(group);
                }
                Ok(group) => remove_tmp_file(&group.merged),
                Err(e) => {
                    tracing::warn!(image = %image.display(), error = %e, "image to PDF failed");
                    self.keep_image.insert(image.clone());
                    self.message = Some((
                        Level::Info,
                        format!(
                            "{} could not be turned into a PDF ({e}); it is filed as it is.",
                            file_name(&image)
                        ),
                    ));
                }
            }
        }
        // Kept-as-image files that left the queue are forgotten: a later file of the same name
        // (scanners reuse names) is converted again.
        if !self.keep_image.is_empty() {
            let queue = state.intake.queue();
            self.keep_image
                .retain(|p| queue.iter().any(|q| &q.path == p));
        }
        // An image changed on disk since it was converted: its PDF is stale, so it is made
        // again (looked at every few seconds, not every frame).
        if self
            .stamp_checked
            .is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(3))
            && self.commit_job.is_none()
        {
            self.stamp_checked = Some(std::time::Instant::now());
            let stale: Vec<PathBuf> = self
                .groups
                .iter()
                .filter(|g| g.converted && !g.stamps.iter().all(|s| s.unchanged()))
                .map(|g| g.merged.clone())
                .collect();
            for merged in stale {
                tracing::info!(pdf = %merged.display(), "image changed: converted again");
                self.discard_group(&merged, false);
            }
        }
        if !state.settings.images_to_pdf {
            // Switched off: converted rows go back to their images.
            let converted: Vec<PathBuf> = self
                .groups
                .iter()
                .filter(|g| g.converted)
                .map(|g| g.merged.clone())
                .collect();
            for merged in converted {
                self.groups.retain(|g| g.merged != merged);
                self.edits.remove(&merged);
                remove_tmp_file(&merged);
            }
            return;
        }
        if self.commit_job.is_some() {
            return;
        }
        // A few at a time; the rest start as these finish.
        const AT_ONCE: usize = 4;
        let waiting: Vec<QueuedFile> = state
            .intake
            .queue()
            .iter()
            .filter(|q| {
                q.kind == crate::intake::Kind::Other
                    && crate::imagepdf::convertible(&q.path)
                    && !self.keep_image.contains(&q.path)
                    && !self.convert_jobs.iter().any(|(p, _)| p == &q.path)
                    && !self
                        .groups
                        .iter()
                        .any(|g| g.members.iter().any(|m| m.path == q.path))
            })
            .take(AT_ONCE.saturating_sub(self.convert_jobs.len()))
            .cloned()
            .collect();
        for image in waiting {
            let edits = self.edits.get(&image.path).cloned().unwrap_or_default();
            let tmp = state
                .data_dir
                .join(pdf::TMP_DIR)
                .join(uuid::Uuid::new_v4().simple().to_string());
            // Named like the image with .pdf, so the plan reads the same ID, rule and year.
            let merged = tmp.join(
                image
                    .path
                    .with_extension("pdf")
                    .file_name()
                    .unwrap_or_default(),
            );
            let path = image.path.clone();
            let job = worker::background(ctx, move || {
                std::fs::create_dir_all(&tmp)
                    .map_err(|e| format!("cannot create {}: {e}", tmp.display()))?;
                let stamp = match CombinedOriginal::stamp(&image.path, image.source.clone()) {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = std::fs::remove_dir(&tmp);
                        return Err(format!("cannot read it: {e}"));
                    }
                };
                match crate::imagepdf::image_to_pdf(&image.path, &merged) {
                    Ok(pages) => Ok(Group {
                        size: std::fs::metadata(&merged).map_or(0, |m| m.len()),
                        merged,
                        members: vec![image],
                        stamps: vec![stamp],
                        pages,
                        added: chrono::Local::now(),
                        edits,
                        converted: true,
                    }),
                    Err(e) => {
                        let _ = std::fs::remove_dir(&tmp);
                        Err(e)
                    }
                }
            });
            self.convert_jobs.push((path, job));
        }
    }

    #[cfg(test)]
    pub fn can_commit_for_test(&self) -> bool {
        self.can_commit()
    }

    /// A commit is filing right now.
    pub fn committing(&self) -> bool {
        self.commit_job.is_some()
    }

    #[cfg(test)]
    pub fn edits_for_test(&self, path: &Path) -> Option<&RowEdits> {
        self.edits.get(path)
    }

    #[cfg(test)]
    pub fn pane_for_test(&self) -> Option<(&Path, usize, bool)> {
        self.pane
            .as_ref()
            .map(|p| (p.path(), p.page(), p.has_image()))
    }

    /// The side pane has read a file that is not a PDF (tests).
    #[cfg(test)]
    pub fn pane_contents_ready(&self) -> bool {
        self.pane.as_ref().is_some_and(|p| p.contents_ready())
    }

    #[cfg(test)]
    pub fn picker_open(&self) -> bool {
        self.picker.is_some()
    }

    #[cfg(test)]
    pub fn summary_is_open(&self) -> bool {
        self.summary_open
    }

    #[cfg(test)]
    pub fn group_count(&self) -> usize {
        self.groups.len()
    }

    #[cfg(test)]
    pub fn can_combine_for_test(&self) -> Option<String> {
        self.combine_blocker()
    }

    #[cfg(test)]
    pub fn split_ready(&self) -> Option<bool> {
        self.split_dialog.as_ref().map(|d| d.ready())
    }

    #[cfg(test)]
    pub fn split_pieces(&self) -> Option<Vec<(std::ops::Range<usize>, Option<String>)>> {
        self.split_dialog.as_ref().map(|d| d.pieces())
    }

    #[cfg(test)]
    pub fn split_thumbs(&self) -> usize {
        self.split_dialog.as_ref().map_or(0, |d| d.thumb_count())
    }

    #[cfg(test)]
    pub fn split_count(&self) -> usize {
        self.splits.len()
    }

    #[cfg(test)]
    pub fn combining(&self) -> bool {
        self.combine.is_some() || self.merge_job.is_some()
    }

    /// Temp PDFs this session still uses (combinations and split pieces), kept by the Recovery
    /// panel's Delete leftovers.
    pub fn temp_files(&self) -> Vec<PathBuf> {
        self.groups
            .iter()
            .map(|g| g.merged.clone())
            .chain(
                self.splits
                    .iter()
                    .flat_map(|s| s.pieces.iter().map(|p| p.path.clone())),
            )
            .collect()
    }

    /// Enter: same as clicking Commit (opens the summary; Confirm still needs a click).
    pub fn press_enter(&mut self) {
        if self.can_commit() {
            self.request_commit();
        }
    }

    /// Rebuilds the plan first so the Commit summary reflects the disk right now.
    fn request_commit(&mut self) {
        self.built_for = None;
        self.summary_pending = true;
        self.message = None;
    }

    /// Del: skip (or unskip) the row under the mouse.
    pub fn press_delete(&mut self) {
        // The table is disabled while filing.
        if self.commit_job.is_some() {
            return;
        }
        let Some(path) = self.hovered.clone() else {
            return;
        };
        if !self.proposals.iter().any(|p| p.file.path == path) {
            return;
        }
        if self.group_of(&path).is_some() {
            return;
        }
        let e = self.edit(&path);
        e.skip = !e.skip;
    }

    /// Combined file -> its members as they were when combined, for the commit.
    fn group_map(&self) -> HashMap<PathBuf, Vec<CombinedOriginal>> {
        self.groups
            .iter()
            .map(|g| (g.merged.clone(), g.stamps.clone()))
            .collect()
    }

    fn group_of(&self, merged: &Path) -> Option<&Group> {
        self.groups.iter().find(|g| g.merged == merged)
    }

    /// Deletes a combined file the app built and brings its members back as rows.
    /// `keep_image`: a converted image's PDF was given up by the person ("Keep as image"), so
    /// the image is not converted again; otherwise (it left the queue, it changed) it may be.
    fn discard_group(&mut self, merged: &Path, keep_image: bool) {
        if keep_image
            && let Some(g) = self.group_of(merged)
            && g.converted
        {
            let image = g.members[0].path.clone();
            self.keep_image.insert(image);
        }
        self.groups.retain(|g| g.merged != merged);
        self.edits.remove(merged);
        // Our own temp file (CLAUDE.md §7: app-created).
        let _ = std::fs::remove_file(merged);
        if let Some(dir) = merged.parent() {
            let _ = std::fs::remove_dir(dir);
        }
    }

    /// Files of the batch that are still queued, in batch order. A combined PDF takes the place
    /// of its first member; the other members are hidden.
    fn batch(&self, state: &AppState) -> Vec<QueuedFile> {
        let Some(paths) = &self.batch_files else {
            return Vec::new();
        };
        let queue: std::collections::HashMap<&Path, &QueuedFile> = state
            .intake
            .queue()
            .iter()
            .map(|q| (q.path.as_path(), q))
            .collect();
        let mut out = Vec::new();
        for p in paths {
            if let Some(g) = self
                .groups
                .iter()
                .find(|g| g.members.iter().any(|m| &m.path == p))
            {
                if g.members[0].path == *p {
                    out.push(QueuedFile {
                        path: g.merged.clone(),
                        size: g.size,
                        added: g.added,
                        source: Source::Manual,
                        kind: crate::intake::Kind::Pdf,
                    });
                }
                continue;
            }
            if let Some(s) = self.splits.iter().find(|s| &s.original.path == p) {
                for piece in &s.pieces {
                    out.push(QueuedFile {
                        path: piece.path.clone(),
                        size: piece.size,
                        added: s.original.added,
                        source: Source::Manual,
                        kind: crate::intake::Kind::Pdf,
                    });
                }
                continue;
            }
            if let Some(q) = queue.get(p.as_path()) {
                out.push((*q).clone());
            }
        }
        out
    }

    fn split_of_piece(&self, piece: &Path) -> Option<&SplitGroup> {
        self.splits
            .iter()
            .find(|s| s.pieces.iter().any(|p| p.path == piece))
    }

    /// Deletes a split's remaining pieces (app-made temp files) and brings the original back.
    fn discard_split(&mut self, original: &Path, data_dir: &Path) {
        let Some(i) = self.splits.iter().position(|s| s.original.path == original) else {
            return;
        };
        let split = self.splits.remove(i);
        for piece in &split.pieces {
            self.edits.remove(&piece.path);
            let _ = std::fs::remove_file(&piece.path);
            commit::remove_tmp_dirs(data_dir, &piece.path);
        }
    }

    /// Combinations whose members are no longer all queued are discarded.
    fn prune_groups(&mut self, state: &AppState) {
        let gone: Vec<PathBuf> = self
            .splits
            .iter()
            .filter(|s| {
                !state
                    .intake
                    .queue()
                    .iter()
                    .any(|q| q.path == s.original.path)
            })
            .map(|s| s.original.path.clone())
            .collect();
        for original in gone {
            self.discard_split(&original, &state.data_dir);
        }
        let queue = state.intake.queue();
        let stale: Vec<PathBuf> = self
            .groups
            .iter()
            .filter(|g| {
                !g.members
                    .iter()
                    .all(|m| queue.iter().any(|q| q.path == m.path))
            })
            .map(|g| g.merged.clone())
            .collect();
        for merged in stale {
            self.discard_group(&merged, false);
        }
        // Combined rows and split pieces live in data/tmp, not the queue: their ticks count too
        // (V3: they pick what Commit files).
        let selected = std::mem::take(&mut self.selected);
        self.selected = selected
            .into_iter()
            .filter(|p| {
                queue.iter().any(|q| &q.path == p)
                    || self.group_of(p).is_some()
                    || self.split_of_piece(p).is_some()
            })
            .collect();
    }

    fn fingerprint(&self, state: &AppState, files: &[QueuedFile]) -> u64 {
        let mut h = DefaultHasher::new();
        for f in files {
            f.path.hash(&mut h);
        }
        // Not the page texts themselves (up to a page of text per file, every frame).
        state.sniffer.generation().hash(&mut h);
        let mut edits: Vec<_> = self.edits.iter().collect();
        edits.sort_by(|a, b| a.0.cmp(b.0));
        edits.hash(&mut h);
        self.batch.hash(&mut h);
        self.selected.hash(&mut h);
        self.rescan.hash(&mut h);
        self.partial.hash(&mut h);
        if let Some(list) = state.clients.list() {
            (Arc::as_ptr(list) as usize).hash(&mut h);
        }
        state.rules.active_rules().hash(&mut h);
        state.template.hash(&mut h);
        let root = state.settings.root.as_deref();
        if let Some(scan) = state.folders.scan_of(root) {
            scan.scanned_at.hash(&mut h);
            for b in &scan.folders {
                (&b.path, b.status, &b.client_id).hash(&mut h);
            }
        }
        root.hash(&mut h);
        state.settings.id_pattern.hash(&mut h);
        state.settings.ultratax.hash(&mut h);
        state.settings.fuzzy_suggest.to_bits().hash(&mut h);
        state.settings.fuzzy_auto.to_bits().hash(&mut h);
        state.sniffer.enabled().hash(&mut h);
        state.relationships_generation.hash(&mut h);
        h.finish()
    }

    fn start_build(&mut self, ctx: &egui::Context, state: &AppState, files: Vec<QueuedFile>) {
        let sniff = files
            .iter()
            .filter_map(|f| {
                state
                    .sniffer
                    .get(&f.path)
                    .map(|t| (f.path.clone(), t.clone()))
            })
            .collect();
        let input = PlanInput {
            files,
            edits: self.edits.clone(),
            // Ticked rows are numbered first (V3).
            batch: BatchSettings {
                ticked: self.selected.clone(),
                ..self.batch.clone()
            },
            clients: state.clients.list().cloned(),
            rules: state.rules.active_rules().to_vec(),
            template: state.template.clone(),
            folders: state
                .folders
                .scan_of(state.settings.root.as_deref())
                .cloned(),
            root: state.settings.root.clone(),
            id_pattern: state.settings.id_pattern.clone(),
            ultratax: state.settings.ultratax.clone(),
            fuzzy_suggest: state.settings.fuzzy_suggest,
            fuzzy_auto: state.settings.fuzzy_auto,
            current_year: chrono::Local::now().year(),
            sniff_enabled: state.sniffer.enabled(),
            sniff,
            rescan: self.rescan,
            partial: self.partial.clone(),
            relationships: Arc::new(state.relationships.clone()),
        };
        self.job = Some(worker::background(ctx, move || plan::build_plan(&input)));
    }

    /// Keeps the plan in step with its inputs; collects finished builds.
    fn update_plan(&mut self, ctx: &egui::Context, state: &mut AppState) {
        if self.commit_job.is_none() {
            self.prune_groups(state);
        }
        if self.partial_generation != Some(state.ops_generation) {
            self.partial_generation = Some(state.ops_generation);
            self.partial = commit::load_partial(&state.data_dir)
                .into_iter()
                .map(|p| p.original)
                .collect();
        }
        // New arrivals wait for Refresh while there is a batch to review (X63); with nothing
        // left in the batch, the whole queue becomes the batch.
        if self.batch_files.is_none() || self.batch(state).is_empty() {
            let mine = self.mine && !state.personal_inboxes.is_empty();
            self.batch_files = Some(
                state
                    .intake
                    .queue()
                    .iter()
                    .filter(|q| state.is_personal(q) == mine)
                    .map(|q| q.path.clone())
                    .collect(),
            );
        }
        if self.rescan == Rescan::Running && state.folders.scanning().is_none() {
            self.rescan = Rescan::Done;
        }
        if let Some(rx) = &self.job
            && let Ok(out) = rx.try_recv()
        {
            self.job = None;
            self.proposals = out.proposals;
            for p in &out.needs_sniff {
                state.sniffer.request(p);
            }
            if out.needs_rescan && self.rescan == Rescan::NotYet {
                match state.scan_blocker() {
                    None => {
                        state.start_scan(ctx);
                        self.rescan = Rescan::Running;
                    }
                    Some(_) if state.folders.scanning().is_some() => self.rescan = Rescan::Running,
                    Some(_) => self.rescan = Rescan::Done,
                }
            }
        }
        if self.job.is_none() {
            let files = self.batch(state);
            let fp = self.fingerprint(state, &files);
            if self.built_for != Some(fp) {
                self.built_for = Some(fp);
                self.start_build(ctx, state, files);
            } else if self.summary_pending {
                self.summary_pending = false;
                self.summary_open = self.can_commit();
            }
        }
    }

    fn can_commit(&self) -> bool {
        self.job.is_none()
            && self.commit_job.is_none()
            && self.convert_jobs.is_empty()
            && !self.commit_rows().any(|p| p.status.blocks_commit())
            && self.commit_rows().any(|p| p.status == RowStatus::Ready)
    }

    /// Some rows are ticked: Commit files only those, and only they can block it (V3).
    fn ticked_only(&self) -> bool {
        self.proposals
            .iter()
            .any(|p| self.selected.contains(&p.file.path))
    }

    /// The rows Commit works on: the ticked ones, or all of them when none is ticked (V3).
    fn commit_rows(&self) -> impl Iterator<Item = &Proposal> {
        let ticked: std::collections::HashSet<&PathBuf> = self.selected.iter().collect();
        let only = self.proposals.iter().any(|p| ticked.contains(&p.file.path));
        self.proposals
            .iter()
            .filter(move |p| !only || ticked.contains(&p.file.path))
    }

    fn edit(&mut self, path: &Path) -> &mut RowEdits {
        self.edits.entry(path.to_path_buf()).or_default()
    }

    /// A year entered on a row also becomes the batch year if there is none yet (sticky).
    fn set_row_year(&mut self, path: &Path, year: Option<u16>) {
        self.edit(path).year = year;
        if self.batch.year.is_none() && year.is_some() {
            self.batch.year = year;
        }
    }

    /// Starts filing the Ready rows (the summary was confirmed).
    fn start_commit(&mut self, ctx: &egui::Context, state: &AppState) {
        let Some(root) = state.settings.root.clone() else {
            self.message = Some((Level::Error, "Set the client root in Setup.".into()));
            return;
        };
        let rows: Vec<Proposal> = self.commit_rows().cloned().collect();
        let input = CommitInput::from_plan(
            &rows,
            &self.group_map(),
            &self
                .splits
                .iter()
                .map(|s| SplitCommit {
                    original: s.stamp.clone(),
                    pieces: s.pieces.iter().map(|p| p.path.clone()).collect(),
                    total: s.total,
                })
                .collect::<Vec<_>>(),
            commit::CommitTarget {
                root,
                first_inbox: state.settings.inboxes.first().cloned(),
                data_dir: state.data_dir.clone(),
                firm_id: state.settings.firm_id.unwrap_or_default(),
                template: state.template.clone(),
            },
        );
        let progress = Arc::new(AtomicUsize::new(0));
        let total = input.items.len();
        let p = Arc::clone(&progress);
        let rx = worker::background(ctx, move || commit::run_commit(&input, &p));
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        self.commit_job = Some(CommitJob {
            rx,
            progress,
            total,
        });
        self.message = None;
    }

    fn poll_commit(&mut self, ctx: &egui::Context, state: &mut AppState) {
        let Some(job) = &self.commit_job else { return };
        let Ok(report) = job.rx.try_recv() else {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return;
        };
        self.commit_job = None;
        let mut failed = Vec::new();
        let mut filed = 0;
        for o in &report.outcomes {
            match &o.result {
                Ok(_) => {
                    filed += 1;
                    if let Some(s) = self.split_of_piece(&o.src).map(|s| s.original.path.clone()) {
                        if let Some(split) = self.splits.iter_mut().find(|x| x.original.path == s) {
                            split.pieces.retain(|p| p.path != o.src);
                        }
                    } else if let Some(g) = self.group_of(&o.src).cloned() {
                        // The originals went to _combined (X84). One that couldn't be moved stays
                        // queued but marked Skip: its pages are already filed.
                        for m in &g.members {
                            if report.unparked.contains(&m.path) {
                                self.edit(&m.path).skip = true;
                                if g.converted {
                                    // Already filed as a PDF: not converted (and filed) again.
                                    self.keep_image.insert(m.path.clone());
                                }
                            } else {
                                state.intake.remove(&m.path);
                                self.edits.remove(&m.path);
                            }
                        }
                        self.groups.retain(|x| x.merged != g.merged);
                    } else {
                        state.intake.remove(&o.src);
                    }
                    self.edits.remove(&o.src);
                }
                Err(e) => failed.push(format!("{}: {e}", file_name(&o.src))),
            }
        }
        // Splits: the original left the inbox if all pieces were filed. A split with no pieces
        // left whose original is still there (it couldn't be moved) is marked Skip.
        for original in &report.split_done {
            state.intake.remove(original);
            self.edits.remove(original);
        }
        let emptied: Vec<PathBuf> = self
            .splits
            .iter()
            .filter(|s| s.pieces.is_empty())
            .map(|s| s.original.path.clone())
            .collect();
        for original in emptied {
            self.splits.retain(|s| s.original.path != original);
            if !report.split_done.contains(&original) {
                self.edit(&original).skip = true;
            }
        }
        failed.extend(report.warnings.iter().cloned());
        state.last_batch = Some(report.last_batch);
        state.ops_generation += 1;
        if report.folders_changed {
            state.start_scan(ctx);
            self.rescan = Rescan::Running;
        }
        self.message = Some(if failed.is_empty() {
            (
                Level::Info,
                format!(
                    "Filed {filed} file{}. Undo is on the History tab.",
                    plural(filed)
                ),
            )
        } else {
            (
                Level::Error,
                format!(
                    "Filed {filed} file{}; {} problem{}: {}",
                    plural(filed),
                    failed.len(),
                    plural(failed.len()),
                    failed.join(" | ")
                ),
            )
        });
    }

    /// Thumbnails in memory while the Split screen is open (footer, UI_SPEC §10).
    pub fn split_thumbs_in_memory(&self) -> Option<usize> {
        self.split_dialog.as_ref().map(|d| d.thumb_count())
    }

    pub fn show(&mut self, ui: &mut egui::Ui, state: &mut AppState) {
        let ctx = ui.ctx().clone();
        self.poll_commit(&ctx, state);
        self.convert_images(&ctx, state);
        self.update_plan(&ctx, state);
        self.poll_split();
        self.count_pages(&ctx);
        // The Split screen takes the tab's body while open (UI_SPEC §10).
        if self.split_dialog.is_some() {
            self.split_screen(ui, state);
            return;
        }

        ui.add_space(4.0);
        for (inbox, e) in state.intake.watch_errors() {
            banner(
                ui,
                Level::Error,
                &format!("Not watching {}: {e}", inbox.display()),
            );
        }
        self.inbox_tabs(ui, state);
        self.toolbar_ui(ui, state);
        self.arrivals_ui(ui, state);
        if let Some(job) = &self.commit_job {
            let done = job.progress.load(Ordering::Relaxed);
            ui.add(
                egui::ProgressBar::new(done as f32 / job.total.max(1) as f32)
                    .text(format!("Filing… {done}/{}", job.total)),
            );
        }
        if let Some(msg) = &self.message {
            banner(ui, msg.0, &msg.1);
        }
        ui.add_space(4.0);

        let files = self.batch(state);
        let filing = self.commit_job.is_some();
        self.pane_open = self
            .pane_choice
            .unwrap_or(state.pdfium_dir.is_some() && ui.available_width() >= PANE_AUTO_WIDTH);
        self.ticked_ui(ui, state, filing);
        self.keep_focus();
        peek::follow(
            &mut self.pane,
            &ctx,
            self.focused.as_deref().filter(|_| self.pane_open),
            state.pdfium_dir.clone(),
        );
        let table_height = (ui.available_height() - 44.0).max(160.0);
        let pane_w = if self.pane_open {
            peek::PANE_WIDTH + 24.0
        } else {
            0.0
        };
        let table_w = (ui.available_width() - pane_w).max(200.0);
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(table_w, table_height),
                Layout::top_down(Align::Min),
                |ui| {
                    let p = pal(ui);
                    Frame::new()
                        .fill(p.card)
                        .stroke(Stroke::new(1.0, p.border))
                        .corner_radius(CornerRadius::same(6))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.set_min_height(table_height - 2.0);
                            ui.add_enabled_ui(!filing, |ui| {
                                if files.is_empty() {
                                    Frame::new().inner_margin(Margin::same(16)).show(ui, |ui| {
                                        ui.label(RichText::new("No files waiting.").color(p.muted));
                                    });
                                } else {
                                    self.table_ui(ui, state, table_height - 40.0);
                                }
                            });
                            self.unsupported_ui(ui, state);
                        });
                },
            );
            if self.pane_open {
                let p = pal(ui);
                Frame::new()
                    .fill(p.card)
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_min_height(table_height - 18.0);
                        ui.set_max_height(table_height - 18.0);
                        let pages = self
                            .focused
                            .as_ref()
                            .and_then(|f| self.page_counts.get(f))
                            .and_then(|(_, n)| *n);
                        ui.vertical(|ui| {
                            peek::show(ui, &mut self.pane, pages, state.pdfium_dir.is_some())
                        });
                    });
            }
        });
        ui.add_space(6.0);
        self.bottom_bar_ui(ui);

        self.picker_ui(&ctx, state);
        self.folder_pick_ui(&ctx);
        self.combine_ui(&ctx, state);
        if self.summary_ui(&ctx, state) {
            self.start_commit(&ctx, state);
        }
    }

    /// Queue summary on the left; batch year, collision default, keep-original switch, Browse…,
    /// Combine and Rescan root on the right (UI_SPEC §3).
    fn toolbar_ui(&mut self, ui: &mut egui::Ui, state: &mut AppState) {
        let p = pal(ui);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.label(strong("Queue"));
            let inboxes = state.intake.watching().len();
            let zips = state.routed.len();
            let mut text = format!(
                " · {} pending · {inboxes} inbox{}",
                self.proposals.len(),
                if inboxes == 1 { "" } else { "es" }
            );
            if zips > 0 {
                text.push_str(&format!(" + {zips} zip"));
            }
            ui.label(RichText::new(text).color(p.muted));
            if self.job.is_some() {
                ui.add_space(8.0);
                ui.spinner();
            }
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let pane = ui
                    .selectable_label(self.pane_open, "Preview pane")
                    .on_hover_text("Show the pages of the file last clicked beside the list");
                if pane.clicked() {
                    self.pane_choice = Some(!self.pane_open);
                }
                let blocker = state.scan_blocker();
                let rescan = ui.add_enabled(blocker.is_none(), egui::Button::new("Rescan root"));
                let rescan = match blocker {
                    Some(why) => rescan.on_disabled_hover_text(why),
                    None => rescan,
                };
                if rescan.clicked() {
                    state.start_scan(ui.ctx());
                }
                let blocker = self.combine_blocker();
                let combine = ui
                    .add_enabled(
                        blocker.is_none() && self.merge_job.is_none(),
                        egui::Button::new("Combine"),
                    )
                    .on_hover_text("Select 2+ rows with the same client");
                let combine = match &blocker {
                    Some(why) => combine.on_disabled_hover_text(why),
                    None => combine,
                };
                if combine.clicked() {
                    self.open_combine(ui.ctx());
                }
                if self.merge_job.is_some() {
                    ui.spinner();
                }
                ui.menu_button("Browse…", |ui| {
                    if ui.button("Add files…").clicked() {
                        ui.close();
                        if let Some(files) = rfd::FileDialog::new()
                            .add_filter("PDF", &["pdf"])
                            .add_filter("All files", &["*"])
                            .pick_files()
                        {
                            state.intake.add_paths(files);
                        }
                    }
                    if ui.button("Add folder…").clicked() {
                        ui.close();
                        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                            state.intake.add_paths(vec![dir]);
                        }
                    }
                });
                ui.separator();
                let all_on = !self.proposals.is_empty()
                    && self.proposals.iter().all(|pr| {
                        self.edits
                            .get(&pr.file.path)
                            .is_some_and(|e| e.keep_original_name)
                    });
                ui.label("Keep all original names");
                let mut on = all_on;
                if theme::switch(ui, &mut on, "Keep all original names").changed() {
                    let paths: Vec<PathBuf> = self
                        .proposals
                        .iter()
                        .map(|pr| pr.file.path.clone())
                        .collect();
                    for path in paths {
                        self.edit(&path).keep_original_name = on;
                    }
                }
                ui.add_space(8.0);
                let mut choice = self.batch.collision;
                let collision = egui::ComboBox::from_id_salt("on_collision")
                    .width(110.0)
                    .selected_text(choice.label())
                    .show_ui(ui, |ui| {
                        for c in CollisionChoice::ALL {
                            ui.selectable_value(&mut choice, c, c.label());
                        }
                    });
                self.batch.collision = choice;
                let l = ui.label(RichText::new("On collision").color(p.muted));
                collision.response.labelled_by(l.id);
                ui.add_space(8.0);
                let mut year = self.batch.year;
                let missing = year.is_none();
                let combo = year_combo(ui, "batch_year", &mut year, None, missing);
                if year != self.batch.year {
                    self.batch.year = year;
                }
                let l = ui.label(RichText::new("Batch year").color(p.muted));
                combo.labelled_by(l.id);
            });
        });
    }

    /// Shared inbox | My inbox, when the host gave this person inboxes of their own. Switching
    /// starts that tab's batch (not while filing).
    fn inbox_tabs(&mut self, ui: &mut egui::Ui, state: &AppState) {
        if state.personal_inboxes.is_empty() {
            return;
        }
        let (mut shared, mut mine) = (0, 0);
        for q in state.intake.queue() {
            if state.is_personal(q) {
                mine += 1;
            } else {
                shared += 1;
            }
        }
        let filing = self.commit_job.is_some();
        ui.add_enabled_ui(!filing, |ui| {
            ui.horizontal(|ui| {
                for (on, text) in [
                    (false, format!("Shared inbox ({shared})")),
                    (true, format!("My inbox ({mine})")),
                ] {
                    if ui.selectable_label(self.mine == on, text).clicked() && self.mine != on {
                        self.show_mine(on);
                    }
                }
            });
        });
        ui.add_space(4.0);
    }

    /// Switches the tab: its own batch, nothing ticked.
    pub fn show_mine(&mut self, on: bool) {
        self.mine = on;
        self.batch_files = None;
        self.selected.clear();
        self.focused = None;
    }

    /// "N new files arrived · Refresh" (the batch is kept while reviewing, X63).
    fn arrivals_ui(&mut self, ui: &mut egui::Ui, state: &AppState) {
        let in_batch = self.batch_files.as_deref().unwrap_or(&[]);
        let mine = self.mine && !state.personal_inboxes.is_empty();
        let new = state
            .intake
            .queue()
            .iter()
            .filter(|q| state.is_personal(q) == mine && !in_batch.contains(&q.path))
            .count();
        if new > 0 {
            ui.horizontal(|ui| {
                pill(ui, PillKind::Info, format!("{new} new file{}", plural(new)));
                ui.label(RichText::new("arrived while you were reviewing.").color(pal(ui).muted));
                if ui.link("Refresh").clicked() {
                    self.batch_files = None;
                }
            });
        }
    }

    /// The row for `file` gets client `client_id` (the host created it, "New client…").
    // Used by a host that embeds Filer (Vibe Files), not by this program.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_client(&mut self, file: &Path, client_id: &str) {
        self.edit(file).client_id = Some(client_id.to_string());
    }

    /// Rows whose file name no rule names (the rule came from the page text or the catch-all,
    /// and none was chosen on the row): what a host may read the file for (Vibe Files' form
    /// reading).
    // Used by a host that embeds Filer (Vibe Files), not by this program.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn unnamed_files(&self) -> Vec<PathBuf> {
        self.proposals
            .iter()
            .filter(|p| matches!(p.rule_source, RuleSource::PageText | RuleSource::CatchAll))
            .filter(|p| {
                !self
                    .edits
                    .get(&p.file.path)
                    .is_some_and(|e| e.rule_id.is_some())
            })
            .map(|p| p.file.path.clone())
            .collect()
    }

    /// What a host read from `file`: the client when the row has none, the year when it has
    /// none. Nothing a person chose on the row is changed.
    // Used by a host that embeds Filer (Vibe Files), not by this program.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn suggest(&mut self, file: &Path, client_id: Option<&str>, year: Option<u16>) {
        let Some(p) = self.proposals.iter().find(|p| p.file.path == file) else {
            return;
        };
        let (has_client, has_year) = (p.client.is_some(), p.year.is_some());
        let e = self.edit(file);
        if let Some(c) = client_id
            && !has_client
            && e.client_id.is_none()
        {
            e.client_id = Some(c.to_string());
        }
        if let Some(y) = year
            && !has_year
            && e.year.is_none()
        {
            e.year = Some(y);
        }
    }

    /// Files the host took out of the inbox: no longer ticked, focused or edited.
    // Used by a host that embeds Filer (Vibe Files), not by this program.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn forget(&mut self, files: &[PathBuf]) {
        self.selected.retain(|s| !files.contains(s));
        for f in files {
            self.edits.remove(f);
        }
        if self.focused.as_ref().is_some_and(|f| files.contains(f)) {
            self.focused = None;
        }
    }

    /// The pane's row: the one last clicked while it is in the batch, else the first row.
    fn keep_focus(&mut self) {
        let listed = |f: &PathBuf| self.proposals.iter().any(|p| &p.file.path == f);
        if !self.focused.as_ref().is_some_and(listed) {
            self.focused = self.proposals.first().map(|p| p.file.path.clone());
        }
    }

    /// With rows ticked: one client or one rule for all of them (owner 2026-11-04).
    fn ticked_ui(&mut self, ui: &mut egui::Ui, state: &mut AppState, filing: bool) {
        let ticked: Vec<PathBuf> = self
            .selected
            .iter()
            .filter(|s| self.proposals.iter().any(|p| &p.file.path == *s))
            .cloned()
            .collect();
        if ticked.is_empty() {
            return;
        }
        let p = pal(ui);
        let n = ticked.len();
        ui.add_enabled_ui(!filing, |ui| {
            ui.horizontal(|ui| {
                ui.label(strong(format!("{n} ticked")));
                ui.label(RichText::new("·").color(p.muted));
                let set = ui
                    .add_enabled(
                        state.clients.list().is_some(),
                        egui::Button::new("Set client…"),
                    )
                    .on_hover_text("Choose one client for every ticked file")
                    .on_disabled_hover_text("Load the client list first (Setup)");
                set.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Button, true, "Set client for ticked")
                });
                if set.clicked() {
                    self.picker = Some((ticked[0].clone(), String::new(), PickFor::Ticked));
                }
                let rules: Vec<(String, String)> = state
                    .rules
                    .active_rules()
                    .iter()
                    .map(|r| (r.id.clone(), r.name.clone()))
                    .collect();
                let names: Vec<String> = rules.iter().map(|(_, n)| n.clone()).collect();
                let menu = ui.menu_button("Apply rule…", |ui| {
                    ui.set_min_width(280.0);
                    let key = ui.make_persistent_id("ticked_rule_search");
                    if let Some(k) = crate::ui::widgets::search_list(ui, key, &names, None) {
                        for t in &ticked {
                            self.edit(t).rule_id = Some(rules[k].0.clone());
                        }
                        ui.close();
                    }
                });
                let r = menu.response;
                r.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Button, true, "Apply rule to ticked")
                });
                r.on_hover_text("File every ticked file by this rule (its folder and name)");
                // Only files from an inbox: a combined file, a split piece or a zip entry is a
                // file Filer made, a dropped-in file is not the inbox's (QA 2026-11-05).
                let from_inbox: Vec<PathBuf> = ticked
                    .iter()
                    .filter(|t| {
                        self.proposals.iter().any(|p| {
                            &p.file.path == *t && matches!(p.file.source, Source::Inbox(_))
                        })
                    })
                    .cloned()
                    .collect();
                if state.host_trash
                    && ui
                        .add_enabled(!from_inbox.is_empty(), egui::Button::new("Move to trash"))
                        .on_hover_text(if from_inbox.len() == ticked.len() {
                            "Take every ticked file out of the inbox into the trash".to_string()
                        } else {
                            format!(
                                "Take the {} ticked files from the inbox into the trash (combined, split or zip rows are left)",
                                from_inbox.len()
                            )
                        })
                        .clicked()
                {
                    state.trash_request = Some(from_inbox);
                }
                if ui.button("Untick all").clicked() {
                    self.selected.clear();
                }
            });
        });
        ui.add_space(4.0);
    }

    fn open_combine(&mut self, ctx: &egui::Context) {
        let first = &self.selected[0];
        let p = self.proposals.iter().find(|p| &p.file.path == first);
        let default_name = p
            .and_then(|p| p.dest.as_ref())
            .and_then(|d| d.file_name())
            .map_or_else(|| file_name(first), |n| n.to_string_lossy().into_owned());
        let client_id = p
            .and_then(|p| p.client.as_ref())
            .map(|c| c.id.clone())
            .unwrap_or_default();
        let files = self.selected.clone();
        let pages_job = Some(worker::background(ctx, move || {
            files
                .into_iter()
                .filter_map(|f| pdf::page_count(&f).ok().map(|n| (f, n)))
                .collect()
        }));
        self.combine = Some(CombineDialog {
            order: self.selected.clone(),
            name: default_name,
            client_id,
            pages: HashMap::new(),
            pages_job,
        });
    }

    /// Counts, keyboard hint and the Commit button (UI_SPEC §3 bottom bar).
    fn bottom_bar_ui(&mut self, ui: &mut egui::Ui) {
        let p = pal(ui);
        // Ticked rows only, when some are ticked (V3).
        let ticked = self.ticked_only();
        let count = |f: &dyn Fn(&Proposal) -> bool| self.commit_rows().filter(|x| f(x)).count();
        let ready = count(&|x| x.status == RowStatus::Ready);
        let collisions = count(&|x| x.status == RowStatus::Ready && x.collides());
        let ready_copies: usize = self
            .commit_rows()
            .filter(|x| x.status == RowStatus::Ready)
            .flat_map(|x| &x.copies)
            .filter(|c| c.status == CopyStatus::Ready)
            .count();
        let needs = |n: usize, what: &str| {
            if n == 1 {
                format!("needs {what}")
            } else {
                format!("need {what}")
            }
        };
        let need_client = count(&|x| x.status == RowStatus::NeedsClient);
        let need_year = count(&|x| x.status == RowStatus::NeedsYear);
        let errors = count(&|x| matches!(x.status, RowStatus::Error(_)));
        let collisions_text = if collisions == 1 {
            "collision resolved".to_string()
        } else {
            "collisions resolved".to_string()
        };
        // (count, words, colour of the number): ready green, copies blue, needs amber (§3).
        let parts: Vec<(usize, String, Color32)> = vec![
            (ready - collisions, "ready".into(), p.pill(PillKind::Ok).1),
            (
                ready_copies,
                if ready_copies == 1 { "copy" } else { "copies" }.into(),
                p.pill(PillKind::Info).1,
            ),
            (need_client, needs(need_client, "a client"), p.attention),
            (need_year, needs(need_year, "a year"), p.attention),
            (collisions, collisions_text, p.text),
            (errors, "with errors".into(), p.pill(PillKind::Err).1),
            (
                count(&|x| matches!(x.status, RowStatus::Pending(_))),
                "waiting".into(),
                p.text,
            ),
            (
                count(&|x| x.status == RowStatus::Paused),
                "paused".into(),
                p.text,
            ),
            (
                count(&|x| x.status == RowStatus::Skipped),
                "skipped".into(),
                p.text,
            ),
        ];
        let blocking = count(&|x| x.status.blocks_commit());
        ui.horizontal(|ui| {
            let mut first = true;
            for (n, what, color) in parts {
                if n == 0 && !(what == "ready" && first) {
                    continue;
                }
                if !first {
                    ui.label(RichText::new("·").color(p.muted));
                }
                first = false;
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(strong(n.to_string()).color(color));
                ui.label(RichText::new(what).color(p.muted));
                ui.spacing_mut().item_spacing.x = 8.0;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let text = if ticked {
                    commit_label(ready, ready_copies).replacen(
                        &format!("{ready} file{}", plural(ready)),
                        &format!("{ready} selected"),
                        1,
                    )
                } else {
                    commit_label(ready, ready_copies)
                };
                let button = ui.add_enabled(self.can_commit(), primary_button(ui, text));
                let button = if self.can_commit() {
                    button
                } else if ticked {
                    button.on_disabled_hover_text("Every ticked row must be Ready, Skipped or Paused.")
                } else {
                    button.on_disabled_hover_text(
                        "Every row must be Ready, Skipped or Paused. Or tick the rows to commit.",
                    )
                };
                if button.clicked() {
                    self.request_commit();
                }
                let hint = if blocking > 0 && ticked {
                    format!(
                        "Resolve {blocking} ticked row{} or untick {} · Enter opens commit summary",
                        plural(blocking),
                        if blocking == 1 { "it" } else { "them" }
                    )
                } else if blocking > 0 {
                    format!(
                        "Resolve {blocking} row{}, or tick the rows to commit · Enter opens commit summary · Del skips row",
                        plural(blocking)
                    )
                } else if ticked {
                    "Ticked rows only · Enter opens commit summary".to_string()
                } else {
                    "Enter opens commit summary · Del skips row".to_string()
                };
                ui.label(RichText::new(hint).size(12.0).color(p.muted));
            });
        });
    }
    fn open_split(&mut self, ctx: &egui::Context, state: &AppState, path: &Path) {
        let Some(file) = state
            .intake
            .queue()
            .iter()
            .find(|q| q.path == path)
            .cloned()
        else {
            return;
        };
        let Some(clients) = state.clients.list().cloned() else {
            self.message = Some((Level::Error, "Import the client list first.".into()));
            return;
        };
        let Some(row) = self.proposals.iter().find(|p| p.file.path == path) else {
            return;
        };
        let edits = self.edits.get(path).cloned().unwrap_or_default();
        let cx = SplitContext {
            clients,
            relationships: Arc::new(state.relationships.clone()),
            rules: state.rules.active_rules().to_vec(),
            parsed: row.parsed.clone(),
            year: row.year,
            rule_override: edits.rule_id.clone(),
            keep_original_name: edits.keep_original_name,
            sniff: state.sniffer.enabled(),
            id_pattern: state.settings.id_pattern.clone(),
            fuzzy_suggest: state.settings.fuzzy_suggest,
            fuzzy_auto: f64::from(state.settings.fuzzy_auto),
            current_year: chrono::Local::now().year(),
        };
        self.split_dialog = Some(SplitDialog::open(ctx, file, cx, state.pdfium_dir.clone()));
    }

    /// Reads the page count of rows that don't have one yet, a batch at a time on a worker.
    fn count_pages(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.page_job {
            match rx.try_recv() {
                Ok(counts) => {
                    self.page_counts.extend(counts);
                    self.page_job = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.page_job = None,
            }
        }
        // Keyed by size too, so a file that comes back changed under the same name is re-read.
        let missing: Vec<(PathBuf, u64)> = self
            .proposals
            .iter()
            .filter(|p| {
                p.file.kind == crate::intake::Kind::Pdf
                    && self
                        .page_counts
                        .get(&p.file.path)
                        .is_none_or(|(size, _)| *size != p.file.size)
            })
            .map(|p| (p.file.path.clone(), p.file.size))
            .collect();
        if !missing.is_empty() {
            if self.page_counts.len() > 4 * self.proposals.len() + 64 {
                let live: HashSet<&PathBuf> = self.proposals.iter().map(|p| &p.file.path).collect();
                self.page_counts.retain(|k, _| live.contains(k));
            }
            self.page_job = Some(worker::background(ctx, move || {
                missing
                    .into_iter()
                    .map(|(p, size)| {
                        let n = pdf::page_count(&p).ok();
                        (p, (size, n))
                    })
                    .collect()
            }));
        }
    }

    /// Takes a finished split (its pieces replace the original row).
    fn poll_split(&mut self) {
        if let Some(rx) = &self.split_job
            && let Ok(result) = rx.try_recv()
        {
            self.split_job = None;
            match result {
                Ok(split) => {
                    tracing::info!(file = %split.original.path.display(), pieces = split.pieces.len(), "split");
                    for piece in &split.pieces {
                        self.edits.insert(piece.path.clone(), piece.edits.clone());
                    }
                    self.selected.retain(|p| p != &split.original.path);
                    self.splits.push(split);
                }
                Err(e) => self.message = Some((Level::Error, format!("Could not split: {e}"))),
            }
        }
    }

    fn split_screen(&mut self, ui: &mut egui::Ui, state: &AppState) {
        let Some(dialog) = &mut self.split_dialog else {
            return;
        };
        match dialog.show(ui) {
            SplitOutcome::Open => {}
            SplitOutcome::Cancel => self.split_dialog = None,
            SplitOutcome::Apply(pieces) => {
                let dialog = self.split_dialog.take().expect("open");
                let ctx = ui.ctx().clone();
                self.start_split(&ctx, state, dialog.file, pieces);
            }
        }
    }

    /// Writes the pieces into data/tmp/<id>/<n>/ on a worker thread.
    fn start_split(
        &mut self,
        ctx: &egui::Context,
        state: &AppState,
        original: QueuedFile,
        pieces: Vec<PieceSpec>,
    ) {
        if self.split_job.is_some() {
            return;
        }
        let base_edits = self.edits.get(&original.path).cloned().unwrap_or_default();
        let tmp = state
            .data_dir
            .join(pdf::TMP_DIR)
            .join(uuid::Uuid::new_v4().simple().to_string());
        self.split_job = Some(worker::background(ctx, move || {
            let stamp = CombinedOriginal::stamp(&original.path, original.source.clone())
                .map_err(|e| format!("cannot read the original: {e}"))?;
            // Each piece keeps the original's name (so the plan reads the same rule and year)
            // in its own folder.
            let name = original.path.file_name().ok_or("no file name")?.to_owned();
            let outs: Vec<PathBuf> = (1..=pieces.len())
                .map(|k| tmp.join(k.to_string()).join(&name))
                .collect();
            for o in &outs {
                if let Some(d) = o.parent() {
                    std::fs::create_dir_all(d)
                        .map_err(|e| format!("cannot create {}: {e}", d.display()))?;
                }
            }
            let ranges: Vec<std::ops::Range<usize>> =
                pieces.iter().map(|p| p.pages.clone()).collect();
            if let Err(e) = pdf::split_pdf(&original.path, &ranges, &outs) {
                // split_pdf removed its partial pieces; remove the empty folders too.
                for o in &outs {
                    if let Some(d) = o.parent() {
                        let _ = std::fs::remove_dir(d);
                    }
                }
                let _ = std::fs::remove_dir(&tmp);
                return Err(e);
            }
            let total = pieces.len();
            let pieces = pieces
                .into_iter()
                .zip(outs)
                .map(|(spec, path)| {
                    let PieceSpec {
                        pages,
                        client_id: client,
                        filename,
                    } = spec;
                    let mut edits = base_edits.clone();
                    edits.skip = false;
                    edits.filename = filename;
                    edits.manual_client_only = client.is_none();
                    edits.client_id = client;
                    edits.split_of = Some(original.path.clone());
                    Piece {
                        size: std::fs::metadata(&path).map_or(0, |m| m.len()),
                        path,
                        pages,
                        edits,
                    }
                })
                .collect();
            Ok(SplitGroup {
                original,
                total,
                stamp,
                pieces,
            })
        }));
    }

    /// Why the ticked rows can't be combined, if they can't.
    fn combine_blocker(&self) -> Option<String> {
        if self.selected.len() < 2 {
            return Some("Tick two or more rows of the same client.".into());
        }
        let mut client: Option<&str> = None;
        for path in &self.selected {
            let Some(p) = self.proposals.iter().find(|p| &p.file.path == path) else {
                return Some("Wait for the rows to update.".into());
            };
            if self.group_of(path).is_some() {
                return Some("A combined row can't be combined again.".into());
            }
            if self.split_of_piece(path).is_some() {
                return Some("A piece of a split can't be combined.".into());
            }
            if p.file.kind != crate::intake::Kind::Pdf {
                return Some("Only PDFs can be combined.".into());
            }
            let Some(c) = &p.client else {
                return Some("Every ticked row needs a client.".into());
            };
            match client {
                None => client = Some(&c.id),
                Some(id) if id != c.id => {
                    return Some("The ticked rows have different clients.".into());
                }
                Some(_) => {}
            }
        }
        None
    }

    fn combine_ui(&mut self, ctx: &egui::Context, state: &mut AppState) {
        if let Some(rx) = &self.merge_job
            && let Ok(result) = rx.try_recv()
        {
            self.merge_job = None;
            match result {
                Ok(group) => {
                    tracing::info!(file = %group.merged.display(), members = group.members.len(), pages = group.pages, "combined");
                    self.edits.insert(group.merged.clone(), group.edits.clone());
                    self.selected.clear();
                    self.groups.push(group);
                }
                Err(e) => self.message = Some((Level::Error, format!("Could not combine: {e}"))),
            }
        }
        // What the first file resolves to: client, rule and destination folder.
        let first = self.combine.as_ref().and_then(|d| {
            let path = d.order.first()?;
            let p = self.proposals.iter().find(|p| &p.file.path == path)?;
            let client = p.client.as_ref().map(|c| (c.name.clone(), c.id.clone()));
            let root = state.settings.root.clone().unwrap_or_default();
            let dir = p.dest.as_ref().and_then(|d| d.parent()).map(|d| {
                d.strip_prefix(&root).map_or_else(
                    |_| d.display().to_string(),
                    |r| format!("…\\{}\\", r.display()),
                )
            });
            Some((client, p.rule_name.clone(), dir))
        });
        let Some(dialog) = &mut self.combine else {
            return;
        };
        if let Some(rx) = &dialog.pages_job
            && let Ok(counts) = rx.try_recv()
        {
            dialog.pages = counts.into_iter().collect();
            dialog.pages_job = None;
        }
        let mut start = false;
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("combine_dialog"))
            .backdrop_color(egui::Color32::from_rgba_unmultiplied(27, 31, 36, 90))
            .show(ctx, |ui| {
                let p = pal(ui);
                ui.set_width(620.0);
                let n = dialog.order.len();
                ui.label(strong(format!("Combine {n} PDFs")).size(16.0));
                let total: usize = dialog
                    .order
                    .iter()
                    .filter_map(|path| dialog.pages.get(path))
                    .sum();
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.label(RichText::new("All selected rows resolve to").color(p.muted));
                    if let Some((Some((name, id)), _, _)) = &first {
                        ui.label(strong(name));
                        ui.label(mono(id).color(p.muted));
                    }
                    if dialog.pages_job.is_none() {
                        ui.label(RichText::new(format!("· {total} pages total")).color(p.muted));
                    }
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    label_caps(ui, "Page order");
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new("Drag to reorder · first file sets the default name")
                                .size(12.0)
                                .color(p.muted),
                        );
                    });
                });
                let mut moved: Option<(usize, usize)> = None;
                let mut removed: Option<usize> = None;
                for (i, path) in dialog.order.iter().enumerate() {
                    let row = Frame::new()
                        .stroke(Stroke::new(1.0, p.border))
                        .corner_radius(CornerRadius::same(4))
                        .inner_margin(Margin::symmetric(10, 6))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.dnd_drag_source(egui::Id::unique(("combine_drag", path)), i, |ui| {
                                    drag_dots(ui);
                                })
                                .response
                                .on_hover_text("Drag to reorder");
                                index_badge(ui, i + 1);
                                let name = ui.label(mono(file_name(path)).color(p.text));
                                let a11y = format!("{}. {}", i + 1, file_name(path));
                                name.widget_info(|| {
                                    egui::WidgetInfo::labeled(egui::Role::Label, true, a11y.clone())
                                });
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui
                                        .add_enabled(n > 2, egui::Button::new("Remove").small())
                                        .on_disabled_hover_text("A combination needs two or more files.")
                                        .clicked()
                                    {
                                        removed = Some(i);
                                    }
                                    if let Some(pages) = dialog.pages.get(path) {
                                        ui.label(
                                            RichText::new(format!("{pages} page{}", plural(*pages)))
                                                .size(12.0)
                                                .color(p.muted),
                                        );
                                    }
                                });
                            });
                        });
                    if row.response.dnd_hover_payload::<usize>().is_some() {
                        ui.painter().hline(
                            row.response.rect.x_range(),
                            row.response.rect.top(),
                            Stroke::new(2.0, p.accent),
                        );
                    }
                    if let Some(from) = row.response.dnd_release_payload::<usize>() {
                        moved = Some((*from, i));
                    }
                }
                if let Some((from, to)) = moved {
                    let item = dialog.order.remove(from);
                    dialog.order.insert(to.min(dialog.order.len()), item);
                }
                if let Some(i) = removed {
                    dialog.order.remove(i);
                }
                ui.add_space(8.0);
                egui::Grid::new("combine_fields")
                    .num_columns(2)
                    .spacing([16.0, 8.0])
                    .show(ui, |ui| {
                        let l = label_caps(ui, "Output name");
                        ui.add(
                            egui::TextEdit::singleline(&mut dialog.name)
                                .desired_width(ui.available_width()),
                        )
                        .labelled_by(l.id);
                        ui.end_row();
                        label_caps(ui, "Rule");
                        if let Some((_, rule, dir)) = &first {
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                ui.label(rule);
                                if let Some(dir) = dir {
                                    ui.label(RichText::new("→").color(p.muted));
                                    ui.label(mono(dir).color(p.text));
                                }
                            });
                        }
                        ui.end_row();
                        label_caps(ui, "After commit");
                        ui.add(
                            egui::Label::new(
                                RichText::new(
                                    "The originals move to the inbox's _combined folder only once the \
                                     merged file lands and verifies. Undo restores them.",
                                )
                                .size(12.0)
                                .color(p.muted),
                            )
                            .wrap(),
                        );
                        ui.end_row();
                    });
                ui.add_space(6.0);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let small = |t: &str| RichText::new(t).size(12.0).color(p.muted);
                    ui.label(small("Merged with lopdf into "));
                    ui.label(mono("data\\tmp\\").size(11.0).color(p.muted));
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let n = dialog.order.len();
                        if ui
                            .add(primary_button(ui, format!("Combine · replaces {n} rows")))
                            .clicked()
                        {
                            start = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                });
            });
        if start {
            let dialog = self.combine.take().expect("open");
            self.start_merge(ctx, state, dialog);
        } else if close || modal.should_close() {
            self.combine = None;
        }
    }

    /// Builds the combined PDF in data/tmp/<id>/ on a worker thread.
    fn start_merge(&mut self, ctx: &egui::Context, state: &AppState, dialog: CombineDialog) {
        let queue = state.intake.queue();
        let members: Vec<QueuedFile> = dialog
            .order
            .iter()
            .filter_map(|p| queue.iter().find(|q| &q.path == p).cloned())
            .collect();
        if members.len() != dialog.order.len() {
            self.message = Some((
                Level::Error,
                "A ticked file is no longer in the queue.".into(),
            ));
            return;
        }
        let mut edits = self
            .edits
            .get(&members[0].path)
            .cloned()
            .unwrap_or_default();
        edits.client_id = Some(dialog.client_id.clone());
        edits.skip = false;
        edits.keep_original_name = false;
        // The name shown in the dialog is used as is, whatever order the files ended up in.
        let name = dialog.name.trim().to_string();
        edits.filename = (!name.is_empty()).then_some(name);
        let tmp = state
            .data_dir
            .join(pdf::TMP_DIR)
            .join(uuid::Uuid::new_v4().simple().to_string());
        // Named like the first member so the plan reads the same ID, rule and year from it.
        let merged = tmp.join(members[0].path.file_name().unwrap_or_default());
        self.merge_job = Some(worker::background(ctx, move || {
            std::fs::create_dir_all(&tmp)
                .map_err(|e| format!("cannot create {}: {e}", tmp.display()))?;
            let stamps = members
                .iter()
                .map(|m| CombinedOriginal::stamp(&m.path, m.source.clone()))
                .collect::<std::io::Result<Vec<_>>>()
                .map_err(|e| format!("cannot read an original: {e}"))?;
            let inputs: Vec<(PathBuf, String)> = members
                .iter()
                .map(|m| (m.path.clone(), file_name(&m.path)))
                .collect();
            match pdf::merge_pdfs(&inputs, &merged) {
                Ok(pages) => Ok(Group {
                    size: std::fs::metadata(&merged).map_or(0, |m| m.len()),
                    merged,
                    members,
                    stamps,
                    pages,
                    added: chrono::Local::now(),
                    edits,
                    converted: false,
                }),
                Err(e) => {
                    let _ = std::fs::remove_dir(&tmp);
                    Err(e)
                }
            }
        }));
    }

    fn table_ui(&mut self, ui: &mut egui::Ui, state: &mut AppState, height: f32) {
        let p = pal(ui);
        let root = state.settings.root.clone();
        let rules: Vec<(String, String)> = state
            .rules
            .active_rules()
            .iter()
            .map(|r| (r.id.clone(), r.name.clone()))
            .collect();
        let mut actions: Vec<RowAction> = Vec::new();
        let mut hovered: Option<PathBuf> = None;
        let batch_year = self.batch.year;
        ui.style_mut().spacing.scroll = egui::style::ScrollStyle::solid();
        // Ticks pick the rows to commit (V3) and the rows to combine.
        let selectable: Vec<PathBuf> = self
            .proposals
            .iter()
            .map(|pr| pr.file.path.clone())
            .collect();
        let all_ticked =
            !selectable.is_empty() && selectable.iter().all(|s| self.selected.contains(s));
        let mut select_all: Option<bool> = None;
        let ctx = RowCtx {
            pal: p,
            rules: &rules,
            root: root.as_deref(),
            groups: &self.groups,
            splits: &self.splits,
            splitting: self.split_job.is_some() || self.split_dialog.is_some(),
            selected: &self.selected,
            batch_year,
            folded: &self.folded,
            page_counts: &self.page_counts,
            renaming: self.renaming.as_ref(),
            host_new_client: state.host_new_client,
            host_trash: state.host_trash,
            host_notes: &state.host_notes,
        };
        // The table's lines: each file, then its copies and its "+ Copy to…" line.
        let mut lines: Vec<TableLine> = Vec::new();
        for (i, pr) in self.proposals.iter().enumerate() {
            lines.push(TableLine::Main(i));
            if !self.folded.contains(&pr.file.path)
                && (!pr.copies.is_empty()
                    || matches!(pr.status, RowStatus::Ready | RowStatus::NeedsYear))
                && pr.client.is_some()
            {
                lines.extend((0..pr.copies.len()).map(|k| TableLine::Copy(i, k)));
                if !pr.copies.is_empty() || !pr.copy_notes.is_empty() {
                    lines.push(TableLine::AddCopy(i));
                }
            }
        }
        let key = |l: &TableLine| -> LineKey {
            match *l {
                TableLine::Main(i) => (self.proposals[i].file.path.clone(), 0, 0),
                TableLine::Copy(i, k) => (self.proposals[i].file.path.clone(), 1, k),
                TableLine::AddCopy(i) => (self.proposals[i].file.path.clone(), 2, 0),
            }
        };
        let keys: Vec<LineKey> = lines.iter().map(key).collect();
        let heights: Vec<f32> = keys
            .iter()
            .map(|k| {
                self.row_heights
                    .get(k)
                    .copied()
                    .unwrap_or(if k.1 == 0 { 52.0 } else { 34.0 })
            })
            .collect();
        let mut measured: Vec<f32> = vec![0.0; lines.len()];
        let spacing = ui.spacing().item_spacing;
        // The table follows the space it has (the columns' least widths, about 800 points,
        // are the floor; below that it scrolls sideways).
        let total = ui.available_width().max(800.0);
        // Widths a person dragged are kept until the space changes (the side pane opens, the
        // window is resized); then the columns share it out again. The table keeps its id, so
        // its scroll position stays.
        let width_key = (total / 40.0).round() as i64;
        let reset_widths = self.table_width_key != Some(width_key);
        egui::ScrollArea::horizontal()
            .id_salt("preview_hscroll")
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.allocate_ui(egui::vec2(total, height), |ui| {
                    // Cells touch, so row tints and lines run unbroken; each cell pads itself.
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                    let free = (total - 28.0 - 86.0).max(686.0);
                    let col = |share: f32, min: f32| {
                        Column::initial(free * share).at_least(min).clip(true)
                    };
                    let builder = TableBuilder::new(ui).id_salt("preview_table");
                    if reset_widths {
                        builder.reset();
                    }
                    builder
                        .cell_layout(Layout::top_down(Align::Min))
                        .resizable(true)
                        .vscroll(true)
                        .max_scroll_height(height - 30.0)
                        .auto_shrink([false, true])
                        .column(Column::exact(28.0).resizable(false))
                        .column(col(0.22, 140.0))
                        .column(col(0.19, 140.0))
                        .column(col(0.12, 100.0))
                        .column(Column::exact(86.0).resizable(false).clip(true))
                        .column(
                            Column::remainder()
                                .at_least(180.0)
                                .resizable(false)
                                .clip(true),
                        )
                        .column(col(0.18, 110.0))
                        .header(30.0, |mut header| {
                            let fill = |ui: &mut egui::Ui| {
                                ui.painter().rect_filled(ui.max_rect(), 0.0, p.tint_copy);
                            };
                            header.col(|ui| {
                                fill(ui);
                                ui.add_space(6.0);
                                ui.horizontal(|ui| {
                                    ui.add_space(6.0);
                                    let mut all = all_ticked;
                                    let resp = ui.add(egui::Checkbox::without_text(&mut all));
                                    resp.widget_info(|| {
                                        egui::WidgetInfo::selected(
                                            egui::Role::CheckBox,
                                            true,
                                            all,
                                            "Select all",
                                        )
                                    });
                                    if resp.changed() {
                                        select_all = Some(all);
                                    }
                                });
                            });
                            for t in ["File", "Client", "Rule", "Year", "Destination", "Status"] {
                                header.col(|ui| {
                                    fill(ui);
                                    ui.add_space(8.0);
                                    ui.horizontal(|ui| {
                                        ui.add_space(6.0);
                                        label_caps(ui, t);
                                    });
                                });
                            }
                        })
                        .body(|body| {
                            let mut draw = |mut row: egui_extras::TableRow<'_, '_>| {
                                let n = row.index();
                                let pr = match lines[n] {
                                    TableLine::Main(i)
                                    | TableLine::Copy(i, _)
                                    | TableLine::AddCopy(i) => &self.proposals[i],
                                };
                                let path = &pr.file.path;
                                let attention = matches!(
                                    pr.status,
                                    RowStatus::NeedsClient
                                        | RowStatus::NeedsYear
                                        | RowStatus::Error(_)
                                        | RowStatus::Paused
                                );
                                let (fill, line) = match lines[n] {
                                    TableLine::Main(_) => {
                                        let fill = if self.pane_open
                                            && self.focused.as_ref() == Some(path)
                                        {
                                            // The row shown in the side pane.
                                            Some(p.accent.gamma_multiply(0.12))
                                        } else if attention {
                                            Some(p.tint_attention)
                                        } else {
                                            None
                                        };
                                        (fill, LineStyle::Solid)
                                    }
                                    TableLine::Copy(_, k) if k + 1 < pr.copies.len() => {
                                        (Some(p.tint_copy), LineStyle::Dashed)
                                    }
                                    _ => (Some(p.tint_copy), LineStyle::Solid),
                                };
                                let mut cells = Cells {
                                    row: &mut row,
                                    id: egui::Id::unique(("preview_line", &keys[n])),
                                    col: 0,
                                    fill,
                                    line,
                                    border: p.border,
                                    spacing,
                                    height: 0.0,
                                    hovered: false,
                                };
                                match lines[n] {
                                    TableLine::Main(_) => row_ui(
                                        &mut cells,
                                        pr,
                                        self.edits.get(path),
                                        &ctx,
                                        &mut actions,
                                    ),
                                    TableLine::Copy(_, k) => copy_row_ui(
                                        &mut cells,
                                        pr,
                                        &pr.copies[k],
                                        &ctx,
                                        &mut actions,
                                    ),
                                    TableLine::AddCopy(_) => {
                                        add_copy_line(&mut cells, pr, &mut actions)
                                    }
                                }
                                measured[n] = cells.height;
                                // Layer-aware: a popup or dialog over the row does not count.
                                // Only the file's own row: Del there skips the file, not from
                                // its copy lines.
                                if cells.hovered && matches!(lines[n], TableLine::Main(_)) {
                                    hovered = Some(path.clone());
                                }
                            };
                            // Few rows: all of them are laid out (and reachable by tests and
                            // screen readers); many: only the ones in view.
                            if lines.len() <= LAY_OUT_ALL_UP_TO {
                                let mut body = body;
                                for h in &heights {
                                    body.row(*h, &mut draw);
                                }
                            } else {
                                body.heterogeneous_rows(heights.iter().copied(), draw);
                            }
                        });
                });
            });
        self.table_width_key = Some(width_key);
        // A name being typed on a row scrolled out of view (only lines in view are drawn in a
        // long queue) is kept, as clicking away would.
        if let Some(r) = &self.renaming
            && let Some(n) = keys.iter().position(|k| k.1 == 0 && k.0 == r.path)
            && measured[n] <= 0.0
        {
            actions.push(RowAction::RenameDone);
        }
        // Row heights follow their content: measured this frame, used from the next.
        let mut changed = false;
        for (k, h) in keys.iter().zip(&measured) {
            if *h <= 0.0 {
                continue; // not laid out (out of view)
            }
            let e = self.row_heights.entry(k.clone()).or_insert(0.0);
            if (*e - *h).abs() > 0.5 {
                *e = *h;
                changed = true;
            }
        }
        if changed {
            ui.ctx().request_repaint();
        }
        if self.row_heights.len() > keys.len() * 2 + 64 {
            let live: std::collections::HashSet<&LineKey> = keys.iter().collect();
            self.row_heights.retain(|k, _| live.contains(k));
        }
        if let Some(all) = select_all {
            self.selected = if all { selectable } else { Vec::new() };
        }
        self.hovered = hovered;
        for a in actions {
            match a {
                RowAction::SetClient(path, id) => self.edit(&path).client_id = Some(id),
                RowAction::StartRename(path) => self.start_rename(&path),
                RowAction::RenameText(t) => {
                    if let Some(r) = &mut self.renaming {
                        r.text = t;
                    }
                }
                RowAction::RenameFocused => {
                    if let Some(r) = &mut self.renaming {
                        r.focus = false;
                    }
                }
                RowAction::RenameDone => {
                    if let Some(r) = self.renaming.take() {
                        let mut text = r.text.trim().to_string();
                        // The extension is shown beside the field; typing it too must not
                        // double it ("Report.pdf" → "Report.pdf", not "Report.pdf.pdf").
                        if !r.ext.is_empty()
                            && let Some((stem, e)) = text.rsplit_once('.')
                            && e.eq_ignore_ascii_case(&r.ext)
                        {
                            text = stem.trim_end().to_string();
                        }
                        let e = self.edit(&r.path);
                        if text.is_empty() {
                            e.filename = None;
                        } else {
                            e.filename = Some(if r.ext.is_empty() {
                                text
                            } else {
                                format!("{text}.{}", r.ext)
                            });
                            e.keep_original_name = false;
                        }
                    }
                }
                RowAction::RenameCancel => self.renaming = None,
                RowAction::ResetName(path) => self.edit(&path).filename = None,
                RowAction::PickFolder(path) => self.open_folder_pick(ui.ctx(), state, &path),
                RowAction::ResetFolder(path) => self.edit(&path).dest_rel = None,
                RowAction::BuildRule(name) => {
                    state.rule_test = Some(name);
                    state.goto = Some(crate::app::Tab::Rules);
                }
                RowAction::PickClient(path, query) => {
                    self.picker = Some((path, query, PickFor::Primary))
                }
                RowAction::PickCopy(path, key, query) => {
                    self.picker = Some((path, query, PickFor::Copy(key)))
                }
                RowAction::AddCopy(path) => {
                    self.picker = Some((path, String::new(), PickFor::NewCopy))
                }
                RowAction::CopyClient(path, key, id) => {
                    self.edit(&path).copy_clients.insert(key, id);
                }
                RowAction::RemoveCopy(path, key) => {
                    self.edit(&path).removed_copies.insert(key);
                }
                RowAction::CopyCollision(path, key, c) => {
                    self.edit(&path).copy_collisions.insert(key, c);
                }
                RowAction::Fold(path) => {
                    if let Some(i) = self.folded.iter().position(|p| p == &path) {
                        self.folded.remove(i);
                    } else {
                        self.folded.push(path);
                    }
                }
                RowAction::RemoveRouted(path) => state.intake.remove_routed(&path),
                RowAction::Trash(path) => state.trash_request = Some(vec![path]),
                RowAction::AddRelationship(parent, query) => {
                    state.relationship_prefill = Some((parent, query));
                    state.goto = Some(crate::app::Tab::Relationships);
                }
                RowAction::SetRule(path, id) => self.edit(&path).rule_id = Some(id),
                RowAction::Focus(path) => self.focused = Some(path),
                RowAction::Open(path) => {
                    if state.host_open {
                        state.open_request = Some(path);
                    } else {
                        crate::ui::widgets::open_file(&path);
                    }
                }
                RowAction::NewClient(file, name, id) => {
                    state.new_client_request =
                        Some(crate::app::NewClientRequest { file, name, id });
                }
                RowAction::SetYear(path, y) => self.set_row_year(&path, y),
                RowAction::AllMissingYear(y) => self.batch.year = Some(y),
                RowAction::SetCollision(path, c) => self.edit(&path).collision = Some(c),
                RowAction::KeepOriginal(path, on) => self.edit(&path).keep_original_name = on,
                RowAction::Skip(path, on) => {
                    if self.group_of(&path).is_some() {
                        // Skip on a combined row discards the combination (X85).
                        self.discard_group(&path, true);
                    } else {
                        self.edit(&path).skip = on;
                    }
                }
                RowAction::OpenSplit(path) => self.open_split(ui.ctx(), state, &path),
                RowAction::DiscardSplit(original) => self.discard_split(&original, &state.data_dir),
                RowAction::Select(path, on) => {
                    self.selected.retain(|p| p != &path);
                    if on {
                        self.selected.push(path);
                    }
                }
                RowAction::GotoConflicts => state.goto = Some(crate::app::Tab::Conflicts),
            }
        }
    }

    /// The collapsed "Unsupported (n)" line at the bottom of the table card.
    fn unsupported_ui(&mut self, ui: &mut egui::Ui, state: &mut AppState) {
        let unsupported = state.intake.unsupported().to_vec();
        if unsupported.is_empty() {
            return;
        }
        let p = pal(ui);
        let names: Vec<String> = unsupported.iter().map(|f| file_name(&f.path)).collect();
        Frame::new()
            .fill(p.tint_copy)
            .inner_margin(Margin::symmetric(12, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                // One line, cut to the table's width: a long list of names written out in full
                // widened the table past the window, hiding its scroll bar and the side pane
                // (owner 2026-11-09, 21 QuickBooks backups).
                let id = ui.make_persistent_id("unsupported_files");
                egui::collapsing_header::CollapsingState::load_with_default_open(
                    ui.ctx(),
                    id,
                    false,
                )
                .show_header(ui, |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!(
                                "Unsupported ({}) · inbox drops are PDF only; zips go to Import — {}",
                                unsupported.len(),
                                names.join(", ")
                            ))
                            .size(12.0)
                            .color(p.muted),
                        )
                        .truncate(),
                    );
                })
                .body(|ui| {
                    // A long list scrolls inside the strip instead of pushing the table up.
                    egui::ScrollArea::vertical()
                        .id_salt("unsupported_list")
                        .max_height(160.0)
                        .show(ui, |ui| {
                            for f in &unsupported {
                                ui.horizontal(|ui| {
                                    if ui.small_button("Ignore").clicked() {
                                        state.intake.ignore_unsupported(&f.path);
                                    }
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(format!(
                                                "{}   {}",
                                                file_name(&f.path),
                                                parent(&f.path)
                                            ))
                                            .monospace()
                                            .size(12.0),
                                        )
                                        .truncate(),
                                    );
                                });
                            }
                        });
                });
            });
    }

    /// V5: edit the destination file name in place; the extension stays.
    fn start_rename(&mut self, path: &Path) {
        let Some(p) = self.proposals.iter().find(|p| p.file.path == path) else {
            return;
        };
        let current = self
            .edits
            .get(path)
            .and_then(|e| e.filename.clone())
            .or_else(|| {
                p.dest
                    .as_ref()
                    .and_then(|d| d.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .unwrap_or_default();
        let ext = p.parsed.ext.clone();
        let text = match current.rsplit_once('.') {
            Some((stem, e)) if !ext.is_empty() && e.eq_ignore_ascii_case(&ext) => stem.to_string(),
            _ => current,
        };
        self.renaming = Some(Renaming {
            path: path.to_path_buf(),
            text,
            ext,
            focus: true,
        });
    }

    /// V6: opens the folder list for the row's client and reads the client folder on a worker.
    fn open_folder_pick(&mut self, ctx: &egui::Context, state: &AppState, path: &Path) {
        let Some(p) = self.proposals.iter().find(|p| p.file.path == path) else {
            return;
        };
        let Some(client) = &p.client else { return };
        let year = p.year.or(self.batch.year);
        let client_dir = state
            .folders
            .bound_folder(state.settings.root.as_deref(), &client.id)
            .map(Path::to_path_buf)
            .or_else(|| match &p.folder {
                Some(FolderAction::Exists(f) | FolderAction::WillBind(f)) => Some(f.clone()),
                _ => None,
            });
        let current = self.edits.get(path).and_then(|e| e.dest_rel.clone());
        let rule_dir = match (&p.dest, &p.folder) {
            (Some(d), Some(f)) => d
                .parent()
                .and_then(|d| d.strip_prefix(f.path()).ok())
                .map(Path::to_path_buf),
            _ => None,
        };
        let text = current
            .clone()
            .or(rule_dir)
            .map(|r| r.display().to_string())
            .unwrap_or_default();
        let disk_job = client_dir
            .clone()
            .map(|dir| worker::background(ctx, move || list_subfolders(&dir, 3)));
        self.folder_pick = Some(FolderPick {
            path: path.to_path_buf(),
            client: client.name.clone(),
            template: crate::template::folder_list(&state.template, client, year),
            client_dir,
            on_disk: Vec::new(),
            disk_job,
            text,
            has_override: current.is_some(),
        });
    }

    fn folder_pick_ui(&mut self, ctx: &egui::Context) {
        let Some(fp) = &mut self.folder_pick else {
            return;
        };
        if let Some(rx) = &fp.disk_job
            && let Ok(list) = rx.try_recv()
        {
            fp.on_disk = list;
            fp.disk_job = None;
        }
        let mut all: Vec<(PathBuf, bool)> = fp
            .template
            .iter()
            .map(|t| (t.clone(), fp.on_disk.iter().any(|d| same_rel(d, t))))
            .collect();
        for d in &fp.on_disk {
            if !all.iter().any(|(t, _)| same_rel(t, d)) {
                all.push((d.clone(), true));
            }
        }
        all.sort_by_key(|(t, _)| t.to_string_lossy().to_lowercase());
        let mut done: Option<Option<PathBuf>> = None;
        let mut cancel = false;
        let modal = egui::Modal::new(egui::Id::unique("folder_pick")).show(ctx, |ui| {
            let p = pal(ui);
            ui.set_width(560.0);
            ui.heading(format!("Folder for \"{}\"", file_name(&fp.path)));
            ui.label(
                RichText::new(match &fp.client_dir {
                    Some(d) => format!("Inside {} · {}", fp.client, d.display()),
                    None => format!("Inside {} · the client folder will be created", fp.client),
                })
                .size(12.0)
                .color(p.muted),
            );
            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .max_height(300.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (rel, exists) in &all {
                        let shown = rel.display().to_string();
                        ui.horizontal(|ui| {
                            let sel = same_rel(Path::new(fp.text.trim()), rel);
                            if ui.selectable_label(sel, mono(&shown)).clicked() {
                                fp.text = shown.clone();
                            }
                            if !exists {
                                ui.label(
                                    RichText::new("created on commit").size(11.0).color(p.muted),
                                );
                            }
                        });
                    }
                    if fp.disk_job.is_some() {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(RichText::new("Reading the client folder…").color(p.muted));
                        });
                    }
                });
            ui.add_space(6.0);
            let label = ui.label("Folder (type a new name to create it, e.g. Permanent\\Articles)");
            ui.add(egui::TextEdit::singleline(&mut fp.text).desired_width(f32::INFINITY))
                .labelled_by(label.id);
            let typed = PathBuf::from(fp.text.trim());
            let check = if fp.text.trim().is_empty() {
                Err("Pick or type a folder".to_string())
            } else {
                crate::plan::checked_rel(&typed)
            };
            match &check {
                Err(e) => {
                    ui.label(RichText::new(e).size(12.0).color(p.pill(PillKind::Err).1));
                }
                Ok(rel) if !all.iter().any(|(t, ok)| *ok && same_rel(t, rel)) => {
                    ui.label(
                        RichText::new("New folder: created on commit, removed by Undo if empty.")
                            .size(12.0)
                            .color(p.muted),
                    );
                }
                Ok(_) => {}
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(check.is_ok(), primary_button(ui, "Use this folder"))
                    .clicked()
                    && let Ok(rel) = &check
                {
                    done = Some(Some(rel.clone()));
                }
                if fp.has_override && ui.button("Use the rule's folder").clicked() {
                    done = Some(None);
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        if modal.should_close() {
            cancel = true;
        }
        let path = fp.path.clone();
        if let Some(rel) = done {
            self.edit(&path).dest_rel = rel;
            self.folder_pick = None;
        } else if cancel {
            self.folder_pick = None;
        }
    }

    fn picker_ui(&mut self, ctx: &egui::Context, state: &mut AppState) {
        let Some((path, query, target)) = &mut self.picker else {
            return;
        };
        let Some(list) = state.clients.list().cloned() else {
            self.picker = None;
            return;
        };
        let title = if *target == PickFor::Ticked {
            let n = self.selected.len();
            format!("Client for {n} ticked file{}", plural(n))
        } else {
            format!("Client for \"{}\"", file_name(path))
        };
        let offer_new = state.host_new_client && *target == PickFor::Primary;
        match client_picker_or_new(ctx, &title, query, &list, offer_new) {
            PickerOutcome::Open => {}
            PickerOutcome::Cancel => self.picker = None,
            PickerOutcome::NewClient(typed) => {
                let file = path.clone();
                let id = self
                    .proposals
                    .iter()
                    .find(|p| p.file.path == file)
                    .and_then(|p| p.parsed.id.clone());
                state.new_client_request = Some(crate::app::NewClientRequest {
                    file,
                    name: typed,
                    id,
                });
                self.picker = None;
            }
            PickerOutcome::Chosen(c) => {
                let (path, target) = (path.clone(), target.clone());
                match target {
                    PickFor::Primary => self.edit(&path).client_id = Some(c.id),
                    PickFor::Copy(key) => {
                        self.edit(&path).copy_clients.insert(key, c.id);
                    }
                    PickFor::NewCopy => self.edit(&path).added_copies.push(c.id),
                    PickFor::Ticked => {
                        for p in self.selected.clone() {
                            self.edit(&p).client_id = Some(c.id.clone());
                        }
                    }
                }
                self.picker = None;
            }
        }
    }

    /// Commit modal (UI_SPEC §4). Returns true when the user confirmed.
    fn summary_ui(&mut self, ctx: &egui::Context, state: &AppState) -> bool {
        if !self.summary_open {
            return false;
        }
        // References, not clones: this runs every frame while the dialog is open.
        let rows: Vec<&Proposal> = self.commit_rows().collect();
        let s = plan::summarize(rows.iter().copied());
        let ready: Vec<&Proposal> = rows
            .iter()
            .copied()
            .filter(|p| p.status == RowStatus::Ready)
            .collect();
        let root = state.settings.root.clone().unwrap_or_default();
        let short = |path: &Path| {
            path.strip_prefix(&root).map_or_else(
                |_| path.display().to_string(),
                |r| format!("…\\{}\\", r.display()),
            )
        };
        let mut created: Vec<String> = Vec::new();
        let mut bound: Vec<String> = Vec::new();
        for p in &ready {
            match &p.folder {
                Some(FolderAction::WillCreate(f)) if !created.contains(&short(f)) => {
                    created.push(short(f))
                }
                Some(FolderAction::WillBind(f)) if !bound.contains(&short(f)) => {
                    bound.push(short(f))
                }
                _ => {}
            }
        }
        let mut cross: Vec<(String, usize)> = Vec::new();
        for p in ready.iter().filter(|p| p.cross_drive) {
            let from = state
                .settings
                .inboxes
                .iter()
                .find(|i| p.file.path.starts_with(i))
                .map_or_else(|| parent(&p.file.path), |i| i.display().to_string());
            match cross.iter_mut().find(|(f, _)| *f == from) {
                Some((_, n)) => *n += 1,
                None => cross.push((from, 1)),
            }
        }
        let kept_both: Vec<String> = ready
            .iter()
            .filter(|p| p.renamed)
            .filter_map(|p| p.dest.as_deref().map(short_file(&root)))
            .collect();
        let overwrites: Vec<String> = ready
            .iter()
            .filter(|p| p.overwrites())
            .filter_map(|p| p.dest.as_deref().map(short_file(&root)))
            .collect();
        let mut copy_rules: Vec<String> = Vec::new();
        let mut copy_clients: Vec<String> = Vec::new();
        for p in &ready {
            for c in p.copies.iter().filter(|c| c.status == CopyStatus::Ready) {
                if !copy_rules.contains(&p.rule_name) {
                    copy_rules.push(p.rule_name.clone());
                }
                if let Some(cl) = &c.client
                    && !copy_clients.contains(&cl.name)
                {
                    copy_clients.push(cl.name.clone());
                }
            }
        }
        if copy_clients.len() > 3 {
            let rest = copy_clients.len() - 3;
            copy_clients.truncate(3);
            copy_clients.push(format!("+{rest} more"));
        }
        let copy_clients = vec![copy_clients.join(", ")];
        let mut from_zip: Vec<(String, usize)> = Vec::new();
        for p in &ready {
            if let Source::Zip { zip, .. } = &p.file.source {
                let name = file_name(zip);
                match from_zip.iter_mut().find(|(z, _)| *z == name) {
                    Some((_, n)) => *n += 1,
                    None => from_zip.push((name, 1)),
                }
            }
        }
        let profile = state.rules.active.clone();
        let year = self.batch.year.map_or_else(
            || "no batch year".to_string(),
            |y| format!("batch year {y}"),
        );

        let mut close = false;
        let mut confirm = false;
        let modal = egui::Modal::new(egui::Id::unique("commit_summary"))
            .backdrop_color(egui::Color32::from_rgba_unmultiplied(27, 31, 36, 90))
            .show(ctx, |ui| {
                let p = pal(ui);
                ui.set_width(640.0);
                ui.label(strong(commit_title(s.files, s.related_copies)).size(16.0));
                ui.label(
                    RichText::new(format!(
                        "Profile {profile} · {year} · nothing has been written yet"
                    ))
                    .color(p.muted),
                );
                ui.add_space(10.0);
                ui.columns(5, |cols| {
                    tile(&mut cols[0], s.moves, "moves (same volume)", p.text);
                    tile(&mut cols[1], s.copies, "copy · verify · delete", p.accent);
                    tile(&mut cols[2], s.related_copies, "copies to related clients", p.accent);
                    tile(
                        &mut cols[3],
                        s.new_client_folders,
                        "folder trees to create",
                        p.accent,
                    );
                    tile(
                        &mut cols[4],
                        s.overwrites,
                        &format!("overwrites · {} kept both", s.renamed),
                        p.pill(PillKind::Err).1,
                    );
                });
                ui.add_space(10.0);
                ui.spacing_mut().item_spacing.y = 8.0;
                let line = |ui: &mut egui::Ui, kind, name: &str, detail: &str, note: &str| {
                    ui.horizontal_top(|ui| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(96.0, 0.0),
                            Layout::top_down(Align::Min),
                            |ui| {
                                pill(ui, kind, name);
                            },
                        );
                        let width = ui.available_width();
                        let mut job = egui::text::LayoutJob::default();
                        job.wrap.max_width = width;
                        if !detail.is_empty() {
                            job.append(
                                detail,
                                0.0,
                                egui::TextFormat::simple(
                                    egui::TextStyle::Monospace.resolve(ui.style()),
                                    p.text,
                                ),
                            );
                        }
                        if !note.is_empty() {
                            job.append(
                                note,
                                if detail.is_empty() { 0.0 } else { 8.0 },
                                egui::TextFormat::simple(egui::FontId::proportional(12.0), p.muted),
                            );
                        }
                        ui.add(egui::Label::new(job).wrap());
                    });
                };
                if s.related_copies > 0 {
                    line(
                        ui,
                        PillKind::Info,
                        "Copies first",
                        "",
                        &format!(
                            "{} → {} · each written, SHA-256 verified, then the primary moves; a failed copy rolls back and leaves the source in place",
                            copy_rules.join(", "),
                            more(&copy_clients),
                        ),
                    );
                }
                if !created.is_empty() {
                    line(
                        ui,
                        PillKind::Info,
                        "Will create",
                        &created.join(" · "),
                        "+ template trees + sentinels",
                    );
                }
                if !bound.is_empty() {
                    line(
                        ui,
                        PillKind::Info,
                        "Will bind",
                        &bound.join(" · "),
                        "existing folders get a sentinel; nothing is moved",
                    );
                }
                for (from, n) in &cross {
                    line(
                        ui,
                        PillKind::Info,
                        "Cross-drive",
                        &format!("{from} → {}", root.display()),
                        &format!(
                            "{n} file{} · hash checked before the source is deleted",
                            plural(*n)
                        ),
                    );
                }
                if !kept_both.is_empty() {
                    line(ui, PillKind::Neutral, "Kept both", &more(&kept_both), "");
                }
                if !overwrites.is_empty() {
                    line(
                        ui,
                        PillKind::Err,
                        "Overwrite",
                        &more(&overwrites),
                        "the old file is kept until the next batch",
                    );
                }
                if s.new_sub_folders > 0 {
                    line(
                        ui,
                        PillKind::Neutral,
                        "Sub-folders",
                        "",
                        &format!(
                            "{} new sub-folder{}",
                            s.new_sub_folders,
                            plural(s.new_sub_folders)
                        ),
                    );
                }
                if !from_zip.is_empty() {
                    let n: usize = from_zip.iter().map(|(_, n)| n).sum();
                    let names: Vec<String> = from_zip.iter().map(|(z, _)| z.clone()).collect();
                    line(
                        ui,
                        PillKind::Neutral,
                        "From zip",
                        "",
                        &format!(
                            "{n} file{} from {} · temp folder removed once every entry has left the queue",
                            plural(n),
                            names.join(", ")
                        ),
                    );
                }
                line(
                    ui,
                    PillKind::Neutral,
                    "Skip",
                    "",
                    &format!("{} row{}", s.skipped, plural(s.skipped)),
                );
                if s.paused > 0 {
                    line(
                        ui,
                        PillKind::Warn,
                        "Paused",
                        "",
                        &format!(
                            "{} row{} stay in the queue (duplicate folder)",
                            s.paused,
                            plural(s.paused)
                        ),
                    );
                }
                ui.add_space(8.0);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let small = |t: &str| RichText::new(t).size(12.0).color(p.muted);
                    ui.label(small("Every step is written to "));
                    ui.label(mono("ops.jsonl").size(11.0).color(p.muted));
                    ui.label(small(". Undo is available until the next batch."));
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let text = commit_label(s.files, s.related_copies);
                        let commit = ui.add(primary_button(ui, text));
                        commit.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::Button, true, "Confirm commit")
                        });
                        if commit.clicked() {
                            confirm = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                });
            });
        if confirm {
            self.summary_open = false;
            return true;
        }
        if close || modal.should_close() {
            self.summary_open = false;
        }
        false
    }
}

/// "Commit 8 files" or "Commit 8 files + 3 copies".
fn commit_label(files: usize, copies: usize) -> String {
    let mut t = format!("Commit {files} file{}", plural(files));
    if copies > 0 {
        t.push_str(&format!(
            " + {copies} cop{}",
            if copies == 1 { "y" } else { "ies" }
        ));
    }
    t
}

/// "Commit 8 files?" or "Commit 8 files and 3 copies?".
fn commit_title(files: usize, copies: usize) -> String {
    let mut t = format!("Commit {files} file{}", plural(files));
    if copies > 0 {
        t.push_str(&format!(
            " and {copies} cop{}",
            if copies == 1 { "y" } else { "ies" }
        ));
    }
    t.push('?');
    t
}

/// A count tile in the Commit modal.
fn tile(ui: &mut egui::Ui, n: usize, label: &str, color: egui::Color32) {
    let p = pal(ui);
    Frame::new()
        .fill(p.tint_copy)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.with_layout(Layout::top_down(Align::Min), |ui| {
                ui.label(strong(n.to_string()).size(20.0).color(color));
                ui.label(RichText::new(label).size(11.0).color(p.muted));
            });
        });
}

/// `…\<path under root>` for a file.
fn short_file(root: &Path) -> impl Fn(&Path) -> String + '_ {
    move |path: &Path| {
        path.strip_prefix(root).map_or_else(
            |_| path.display().to_string(),
            |r| format!("…\\{}", r.display()),
        )
    }
}

/// The first item, plus "+N more".
/// Deletes a PDF the app made in data/tmp and its folder when that is empty.
fn remove_tmp_file(path: &Path) {
    let _ = std::fs::remove_file(path);
    if let Some(dir) = path.parent() {
        let _ = std::fs::remove_dir(dir);
    }
}

fn more(items: &[String]) -> String {
    match items.len() {
        0 => String::new(),
        1 => items[0].clone(),
        n => format!("{} +{} more", items[0], n - 1),
    }
}

/// Up to this many table lines every line is laid out each frame; above it only the lines in
/// view are (a long queue stays quick to draw).
const LAY_OUT_ALL_UP_TO: usize = 80;

/// One line of the Preview table.
#[derive(Clone, Copy)]
enum TableLine {
    /// A file's row (proposal index).
    Main(usize),
    /// Copy `k` of a file's row.
    Copy(usize, usize),
    /// "+ Copy to…" under a file's copies.
    AddCopy(usize),
}

/// A table line by its file, its kind (0 row, 1 copy, 2 add-copy) and copy index: what its
/// measured height and its widgets' ids are kept by.
type LineKey = (PathBuf, u8, usize);

/// The rule under a table line.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LineStyle {
    Solid,
    /// Between copies of one file (UI_SPEC §1).
    Dashed,
}

/// The cells of one table line, filled left to right. Each cell gets the line's tint and rule,
/// padding, the window's spacing back, and ids by the line's file (not its place: a menu left
/// open while the queue re-plans stays on its file, QA 2026-11-05). The tallest cell's height is
/// kept for the next frame.
struct Cells<'r, 'a, 'b> {
    row: &'r mut egui_extras::TableRow<'a, 'b>,
    id: egui::Id,
    col: usize,
    fill: Option<egui::Color32>,
    line: LineStyle,
    border: egui::Color32,
    spacing: egui::Vec2,
    height: f32,
    hovered: bool,
}

impl Cells<'_, '_, '_> {
    fn cell<R>(&mut self, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
        let id = self.id.with(self.col);
        self.col += 1;
        let (fill, line, border, spacing) = (self.fill, self.line, self.border, self.spacing);
        let mut out = None;
        let mut hovered = false;
        let (used, _) = self.row.col(|ui| {
            let rect = ui.max_rect();
            if let Some(f) = fill {
                ui.painter().rect_filled(rect, 0.0, f);
            }
            let y = rect.bottom() - 0.5;
            match line {
                LineStyle::Solid => {
                    let stroke = Stroke::new(1.0, border.gamma_multiply(0.6));
                    ui.painter().hline(rect.x_range(), y, stroke);
                }
                LineStyle::Dashed => {
                    let stroke = Stroke::new(1.0, border.gamma_multiply(0.4));
                    let mut x = rect.left();
                    while x < rect.right() {
                        let end = (x + 4.0).min(rect.right());
                        ui.painter().hline(x..=end, y, stroke);
                        x += 8.0;
                    }
                }
            }
            hovered = ui.rect_contains_pointer(rect);
            ui.scope_builder(egui::UiBuilder::new().scope_id(id), |ui| {
                ui.spacing_mut().item_spacing = spacing;
                // Clipped columns truncate text by default; the rows grow to fit instead.
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                Frame::new()
                    .inner_margin(Margin {
                        left: 6,
                        right: 6,
                        top: 6,
                        bottom: 7,
                    })
                    .show(ui, |ui| {
                        out = Some(add(ui));
                    });
            });
        });
        self.height = self.height.max(used.height());
        self.hovered |= hovered;
        out.expect("a table cell is always drawn")
    }
}

/// What every row needs besides its own proposal.
struct RowCtx<'a> {
    pal: &'static crate::ui::theme::Palette,
    rules: &'a [(String, String)],
    root: Option<&'a Path>,
    groups: &'a [Group],
    splits: &'a [SplitGroup],
    /// A split is being chosen or written: no second one.
    splitting: bool,
    selected: &'a [PathBuf],
    batch_year: Option<u16>,
    folded: &'a [PathBuf],
    page_counts: &'a HashMap<PathBuf, (u64, Option<usize>)>,
    renaming: Option<&'a Renaming>,
    /// The host can create clients ("New client…").
    host_new_client: bool,
    /// The host takes files out to its trash ("Move to trash").
    host_trash: bool,
    /// What the host read from a file, shown on its row.
    host_notes: &'a HashMap<PathBuf, String>,
}

/// The file name being edited in a row's Destination cell (V5).
struct Renaming {
    path: PathBuf,
    /// Name without the extension, which stays.
    text: String,
    ext: String,
    /// Focus the field on the next frame.
    focus: bool,
}

/// "Change folder…" (V6): the template's folders and the ones on disk under the client folder.
struct FolderPick {
    path: PathBuf,
    client: String,
    client_dir: Option<PathBuf>,
    template: Vec<PathBuf>,
    on_disk: Vec<PathBuf>,
    disk_job: Option<Receiver<Vec<PathBuf>>>,
    text: String,
    has_override: bool,
}

enum RowAction {
    SetClient(PathBuf, String),
    PickClient(PathBuf, String),
    SetRule(PathBuf, String),
    SetYear(PathBuf, Option<u16>),
    AllMissingYear(u16),
    SetCollision(PathBuf, CollisionChoice),
    KeepOriginal(PathBuf, bool),
    Skip(PathBuf, bool),
    Select(PathBuf, bool),
    OpenSplit(PathBuf),
    DiscardSplit(PathBuf),
    GotoConflicts,
    PickCopy(PathBuf, String, String),
    AddCopy(PathBuf),
    CopyClient(PathBuf, String, String),
    RemoveCopy(PathBuf, String),
    CopyCollision(PathBuf, String, CollisionChoice),
    Fold(PathBuf),
    /// Open Relationships on the parent with the add row filled (parent ID, child to look for).
    AddRelationship(String, String),
    /// A zip-routed file leaves the queue without being filed (P07b).
    RemoveRouted(PathBuf),
    /// The host takes the file out of the inbox into its trash.
    Trash(PathBuf),
    /// Start editing the file name (V5).
    StartRename(PathBuf),
    RenameText(String),
    RenameFocused,
    /// Enter or click away: keep the typed name (empty = the rule's name again).
    RenameDone,
    RenameCancel,
    ResetName(PathBuf),
    /// Open "Change folder…" (V6).
    PickFolder(PathBuf),
    ResetFolder(PathBuf),
    /// Open the Rules tab with this file name in the test box (V10).
    BuildRule(String),
    /// Show this row's pages in the side pane.
    Focus(PathBuf),
    /// Double-click on the file name: open the file (V24).
    Open(PathBuf),
    /// "New client…": (file, name in the file name, ID in the file name) for the host.
    NewClient(PathBuf, String, Option<String>),
}

/// One table row. Cells are top-aligned; text cells start a little lower so their first line sits
/// level with the text in the year and rule controls.
fn row_ui(
    cells: &mut Cells<'_, '_, '_>,
    p: &Proposal,
    edits: Option<&RowEdits>,
    ctx: &RowCtx<'_>,
    actions: &mut Vec<RowAction>,
) {
    let pl = ctx.pal;
    let path = &p.file.path;
    let group = ctx.groups.iter().find(|g| &g.merged == path);
    let split = ctx.splits.iter().find_map(|s| {
        s.pieces
            .iter()
            .find(|pc| &pc.path == path)
            .map(|pc| (s, pc))
    });
    let skipped_by_user = edits.is_some_and(|e| e.skip);
    const TEXT_TOP: f32 = 4.0;

    // Select.
    cells.cell(|ui| {
        let mut ticked = ctx.selected.contains(path);
        let resp = ui.add(egui::Checkbox::without_text(&mut ticked));
        let name = format!("Select {}", file_name(path));
        resp.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::CheckBox, true, ticked, name.clone())
        });
        if resp.changed() {
            actions.push(RowAction::Select(path.clone(), ticked));
        }
    });

    // File (right-click for the row menu, X125).
    cells.cell(|ui| {
        ui.add_space(TEXT_TOP);
        let (text, sub, hover) = match (group, split) {
            (_, Some((s, pc))) => {
                let pages = if pc.pages.len() == 1 {
                    format!("Page {}", pc.pages.start + 1)
                } else {
                    format!("Pages {}–{}", pc.pages.start + 1, pc.pages.end)
                };
                (
                    format!("{pages} of {}", file_name(&s.original.path)),
                    Some("split piece".to_string()),
                    s.original.path.display().to_string(),
                )
            }
            (Some(g), None) if g.converted => (
                file_name(&g.members[0].path),
                Some(format!("image → PDF ({} page{})", g.pages, plural(g.pages))),
                format!(
                    "{}\nFiled as a PDF; the image goes to the inbox's _combined folder.",
                    g.members[0].path.display()
                ),
            ),
            (Some(g), None) => {
                let names: Vec<String> = g.members.iter().map(|m| file_name(&m.path)).collect();
                (
                    format!("Combined: {} files ({} pages)", g.members.len(), g.pages),
                    names.first().cloned(),
                    names.join("\n"),
                )
            }
            (None, None) => match &p.file.source {
                Source::Zip { zip, entry } => (
                    file_name(path),
                    Some(format!("zip › {}", file_name(zip))),
                    format!("{entry}\nfrom {}", zip.display()),
                ),
                Source::Manual => (
                    file_name(path),
                    Some("added by hand".to_string()),
                    format!("{}\n{}", path.display(), human_size(p.file.size)),
                ),
                Source::Inbox(_) => (
                    file_name(path),
                    None,
                    format!("{}\n{}", path.display(), human_size(p.file.size)),
                ),
            },
        };
        let resp = ui
            .horizontal_top(|ui| {
                if !p.copies.is_empty() {
                    let folded = ctx.folded.contains(path);
                    let t = fold_toggle(ui, folded).on_hover_text(if folded {
                        "Show copies"
                    } else {
                        "Hide copies"
                    });
                    let name = format!("Copies of {}", file_name(path));
                    t.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::Button, true, name.clone())
                    });
                    if t.clicked() {
                        actions.push(RowAction::Fold(path.clone()));
                    }
                }
                let name = ui
                    .add(
                        egui::Label::new(mono(text).color(pl.text))
                            .wrap()
                            .sense(egui::Sense::click()),
                    )
                    .on_hover_text(format!("{hover}\nDouble-click to open"));
                if let Some((_, Some(n))) = ctx.page_counts.get(path)
                    && *n > 1
                {
                    ui.label(
                        RichText::new(format!("· {n} pages"))
                            .size(12.0)
                            .color(pl.muted),
                    );
                }
                name
            })
            .inner;
        if let Some(sub) = sub {
            ui.label(RichText::new(sub).size(11.0).color(pl.muted));
        }
        if resp.clicked() {
            actions.push(RowAction::Focus(path.clone()));
        }
        if resp.double_clicked() {
            actions.push(RowAction::Open(path.clone()));
        }
        resp.context_menu(|ui| {
            let skip_text = if group.is_some_and(|g| g.converted) {
                "Keep as image (not a PDF)"
            } else if group.is_some() {
                "Discard combination"
            } else if skipped_by_user {
                "Unskip row"
            } else {
                "Skip row"
            };
            if ui.button(skip_text).clicked() {
                actions.push(RowAction::Skip(path.clone(), !skipped_by_user));
                ui.close();
            }
            if let Some((s, _)) = split {
                if ui.button("Discard split").clicked() {
                    actions.push(RowAction::DiscardSplit(s.original.path.clone()));
                    ui.close();
                }
            } else if group.is_none()
                && p.file.kind == crate::intake::Kind::Pdf
                && ui
                    .add_enabled(!ctx.splitting, egui::Button::new("Split…"))
                    .clicked()
            {
                actions.push(RowAction::OpenSplit(path.clone()));
                ui.close();
            }
            if p.client.is_some()
                && matches!(p.status, RowStatus::Ready | RowStatus::NeedsYear)
                && ui.button("Copy to another client…").clicked()
            {
                actions.push(RowAction::AddCopy(path.clone()));
                ui.close();
            }
            if matches!(p.file.source, Source::Zip { .. })
                && ui.button("Remove from queue").clicked()
            {
                actions.push(RowAction::RemoveRouted(path.clone()));
                ui.close();
            }
            if ui.button("Change client…").clicked() {
                actions.push(RowAction::PickClient(path.clone(), p.parsed.name.clone()));
                ui.close();
            }
            if ctx.host_trash
                && group.is_none()
                && split.is_none()
                && matches!(p.file.source, Source::Inbox(_))
                && ui.button("Move to trash").clicked()
            {
                actions.push(RowAction::Trash(path.clone()));
                ui.close();
            }
            if p.client.is_some() {
                ui.separator();
                if p.dest.is_some() && ui.button("Rename…").clicked() {
                    actions.push(RowAction::StartRename(path.clone()));
                    ui.close();
                }
                if edits.is_some_and(|e| e.filename.is_some())
                    && ui.button("Use the rule's name").clicked()
                {
                    actions.push(RowAction::ResetName(path.clone()));
                    ui.close();
                }
                if ui.button("Change folder…").clicked() {
                    actions.push(RowAction::PickFolder(path.clone()));
                    ui.close();
                }
                if ui.button("Build a rule from this file…").clicked() {
                    actions.push(RowAction::BuildRule(file_name(path)));
                    ui.close();
                }
                if edits.is_some_and(|e| e.dest_rel.is_some())
                    && ui.button("Use the rule's folder").clicked()
                {
                    actions.push(RowAction::ResetFolder(path.clone()));
                    ui.close();
                }
            }
        });
    });

    // Client.
    cells.cell(|ui| {
        if p.client.is_some() {
            ui.add_space(TEXT_TOP);
        }
        match &p.client {
            Some(c) => {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.label(&c.name);
                    ui.label(mono(&c.id).color(pl.muted));
                    if let ClientMatch::Fuzzy(score) = p.client_match {
                        pill(ui, PillKind::Info, format!("fuzzy {score:.2}")).on_hover_text(
                            "Matched by name, not by ID. Check it's the right client.",
                        );
                    }
                    // V8: a name match or a row in error can be given another client.
                    if matches!(p.client_match, ClientMatch::Fuzzy(_))
                        || matches!(p.status, RowStatus::Error(_))
                    {
                        let link = ui.link(RichText::new("Change…").size(12.0));
                        let name = format!("Change client of {}", file_name(path));
                        link.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::Link, true, name.clone())
                        });
                        if link.clicked() {
                            actions
                                .push(RowAction::PickClient(path.clone(), p.parsed.name.clone()));
                        }
                    }
                });
            }
            None => {
                let why = match &p.parsed.id {
                    None => "No ID in filename".to_string(),
                    Some(id) => format!("ID {id} not in the client list"),
                };
                match &p.suggestion {
                    Some((score, c)) => {
                        ui.label(RichText::new(why).size(11.0).color(pl.muted));
                        let resp = ui.add(egui::Button::new(suggestion_job(ui, &c.name, *score)));
                        let name = format!("Use {} ({})", c.name, c.id);
                        resp.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::Button, true, name.clone())
                        });
                        if resp.clicked() {
                            actions.push(RowAction::SetClient(path.clone(), c.id.clone()));
                        }
                    }
                    None => {
                        let text = if p.parsed.id.is_none() {
                            "No ID · no name match".to_string()
                        } else {
                            why
                        };
                        ui.label(RichText::new(text).size(11.0).color(pl.muted));
                    }
                }
                if ui.small_button("Choose client…").clicked() {
                    actions.push(RowAction::PickClient(path.clone(), p.parsed.name.clone()));
                }
                if ctx.host_new_client {
                    let b = ui
                        .small_button("New client…")
                        .on_hover_text("Not in the client list: create the client for this file");
                    let name = format!("New client for {}", file_name(path));
                    b.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::Button, true, name.clone())
                    });
                    if b.clicked() {
                        actions.push(RowAction::NewClient(
                            path.clone(),
                            p.parsed.name.clone(),
                            p.parsed.id.clone(),
                        ));
                    }
                }
            }
        }
    });

    // Rule: the name opens a menu to override it.
    cells.cell(|ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.visuals_mut().widgets.inactive.bg_stroke = Stroke::NONE;
            ui.visuals_mut().widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
            let resp = ui.menu_button(p.rule_name.clone(), |ui| {
                ui.set_min_width(280.0);
                let names: Vec<String> = ctx.rules.iter().map(|(_, n)| n.clone()).collect();
                let current = ctx
                    .rules
                    .iter()
                    .position(|(id, _)| p.rule_id.as_deref() == Some(id.as_str()));
                let key = ui.make_persistent_id(("rule_search", path));
                if let Some(k) = crate::ui::widgets::search_list(ui, key, &names, current) {
                    actions.push(RowAction::SetRule(path.clone(), ctx.rules[k].0.clone()));
                    ui.close();
                }
            });
            resp.response
                .on_hover_text("Choose another rule for this file");
            if p.rule_source == RuleSource::PageText {
                pill(ui, PillKind::Neutral, "sniffed").on_hover_text(
                    "No rule matched the file name, so this rule was picked from the first page's text.",
                );
            }
            if let Some(note) = ctx.host_notes.get(path) {
                pill(ui, PillKind::Neutral, "read").on_hover_text(note.as_str());
            }
        });
    });

    // Year: amber when missing.
    cells.cell(|ui| {
        let mut year = p.year;
        // By file, not place: an open year menu stays on its file through a re-plan.
        let resp = year_combo(ui, ("year", path), &mut year, None, p.year.is_none());
        let name = format!("Year for {}", file_name(path));
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, name.clone()));
        if year != p.year {
            actions.push(RowAction::SetYear(path.clone(), year));
        }
    });

    // Destination, or what is missing.
    cells.cell(|ui| {
        if !p.collides() {
            ui.add_space(TEXT_TOP);
        }
        destination_cell(ui, p, edits, ctx, actions);
    });

    // Status pills.
    cells.cell(|ui| {
        ui.add_space(TEXT_TOP);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
            match &p.status {
                RowStatus::Ready if p.collides() => {
                    pill(ui, PillKind::Err, "Collision").on_hover_text(collision_hover(
                        p.collision_with.as_ref(),
                        p.collision_choice,
                        p.dest.as_deref(),
                    ));
                }
                RowStatus::Ready => {
                    pill(ui, PillKind::Ok, "Ready")
                        .on_hover_text("Will be filed when you commit.");
                }
                RowStatus::Skipped if skipped_by_user => {
                    pill(ui, PillKind::Neutral, "Skipped").on_hover_text(
                        "Skipped by you (Del or the row menu): it stays in the inbox. Press Del again to unskip.",
                    );
                }
                RowStatus::Skipped => {
                    let why = if p.collides() {
                        "A file with this name is already there and the choice is Skip: this file stays where it is."
                    } else {
                        "Left out of this batch: it stays where it is."
                    };
                    pill(ui, PillKind::Neutral, "Skipped").on_hover_text(why);
                }
                RowStatus::NeedsClient => {
                    let why = match &p.parsed.id {
                        Some(id) if p.client.is_none() => format!(
                            "ID {id} is not in the client list. Click the suggestion or Choose client…"
                        ),
                        None if p.client.is_none() => {
                            "The file name has no client ID. Click the suggestion or Choose client…"
                                .to_string()
                        }
                        _ => "A copy of this file needs a client: pick it on the copy row."
                            .to_string(),
                    };
                    pill(ui, PillKind::Warn, "Needs client").on_hover_text(why);
                }
                RowStatus::NeedsYear => {
                    pill(ui, PillKind::Warn, "Needs year").on_hover_text(
                        "No year in the file name and the rule's folder or name uses {Year}. Pick a year, use the batch year, or Change folder… to a folder without one.",
                    );
                }
                RowStatus::Paused => {
                    pill(ui, PillKind::Warn, "Paused · duplicate folder").on_hover_text(
                        "This client is bound to two folders. Resolve it in Conflicts; the file stays in the queue.",
                    );
                }
                RowStatus::Pending(why) => {
                    ui.spinner();
                    ui.label(RichText::new(why).size(11.0).color(pl.muted));
                }
                RowStatus::Error(e) => {
                    pill(ui, PillKind::Err, "Error").on_hover_text(e);
                }
            }
            match &p.folder {
                Some(FolderAction::WillCreate(f)) => {
                    pill(ui, PillKind::Info, "Will create").on_hover_text(format!(
                        "This client has no folder yet. {} and its template folders are created when you commit.",
                        f.display()
                    ));
                }
                Some(FolderAction::WillBind(f)) => {
                    pill(ui, PillKind::Info, "Will bind").on_hover_text(format!(
                        "{} is named for this client but has no _Vibe\\client.json yet. It is written when you commit.",
                        f.display()
                    ));
                }
                _ => {}
            }
            if p.cross_drive {
                pill(ui, PillKind::Info, "Cross-drive").on_hover_text(
                    "The inbox and the client folder are on different drives: the file is copied, its SHA-256 checked, then the original is deleted.",
                );
            }
            if group.is_some_and(|g| g.converted) {
                pill(ui, PillKind::Info, "Image → PDF").on_hover_text(
                    "Filed as a PDF made from the image. The image goes to the inbox's _combined folder after filing; Undo puts it back. Right-click › Keep as image to file the image itself.",
                );
            } else if group.is_some() {
                pill(ui, PillKind::Info, "Combined").on_hover_text(
                    "Several PDFs merged into one. The originals go to the inbox's _combined folder after filing.",
                );
            }
            if p.file.kind != crate::intake::Kind::Pdf {
                pill(ui, PillKind::Neutral, "no PDF tools")
                    .on_hover_text("Not a PDF: no text sniff, combine or split");
            }
            let copies = p.copies.iter().filter(|c| c.status != CopyStatus::Skipped).count();
            if copies > 0 {
                pill(ui, PillKind::Info, format!("+{copies} cop{}", if copies == 1 { "y" } else { "ies" }))
                    .on_hover_text("Byte copies to related clients, written and hash-checked before this file moves. Listed under the row.");
            }
        });
    });
}

/// Hover text of a Collision pill (V4): what is already there and what the choice does.
fn collision_hover(
    with: Option<&CollisionWith>,
    choice: CollisionChoice,
    dest: Option<&Path>,
) -> String {
    let what = match with {
        Some(CollisionWith::Disk {
            path,
            bytes,
            modified,
        }) => {
            let mut t = format!(
                "A file with this name is already in the folder:\n{}",
                path.display()
            );
            let mut facts = Vec::new();
            if let Some(b) = bytes {
                facts.push(human_size(*b));
            }
            if let Some(m) = modified {
                facts.push(format!("changed {}", m.format("%-m/%-d/%Y %-I:%M %p")));
            }
            if !facts.is_empty() {
                t.push_str(&format!("\n({})", facts.join(" · ")));
            }
            t
        }
        Some(CollisionWith::Batch { path, row }) => format!(
            "Another file in this batch goes to the same name first:\n{}\n(from {})",
            path.display(),
            file_name(row)
        ),
        None => "A file with this name is already there.".to_string(),
    };
    let new_name = dest
        .and_then(|d| d.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let does = match (choice, with) {
        (_, Some(CollisionWith::Batch { .. })) => {
            format!("Two files of one batch are never overwritten: this one is saved as \"{new_name}\".")
        }
        (CollisionChoice::KeepBoth, _) => {
            format!("Keep both: this file is saved as \"{new_name}\"; the existing file is not touched.")
        }
        (CollisionChoice::Overwrite, _) => "Overwrite: the existing file is moved to data\\overwritten (Undo puts it back) and this file takes its place.".to_string(),
        (CollisionChoice::Skip, _) => "Skip: this file stays where it is.".to_string(),
    };
    format!(
        "{what}\n\n{does}\nChange it with Overwrite / Keep both / Skip in the Destination column."
    )
}

/// The in-place file name editor (V5): Enter or clicking away keeps it, Esc cancels.
fn rename_field(ui: &mut egui::Ui, r: &Renaming, actions: &mut Vec<RowAction>) {
    let mut text = r.text.clone();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let width = (ui.available_width() - 60.0).max(80.0);
        let out = egui::TextEdit::singleline(&mut text)
            .id_salt(("rename", &r.path))
            .font(egui::TextStyle::Monospace)
            .desired_width(width)
            .show(ui);
        let resp = out.response;
        if r.focus {
            // Opens with the whole name selected, so typing replaces it (like Explorer).
            let mut state = out.state;
            let all = egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(text.chars().count()),
            );
            state.cursor.set_char_range(Some(all));
            state.store(ui.ctx(), resp.id);
        }
        let name = format!("New name for {}", file_name(&r.path));
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, name.clone()));
        if !r.ext.is_empty() {
            ui.label(mono(format!(".{}", r.ext)).color(pal(ui).muted));
        }
        if r.focus {
            resp.request_focus();
            actions.push(RowAction::RenameFocused);
        }
        if text != r.text {
            actions.push(RowAction::RenameText(text.clone()));
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            actions.push(RowAction::RenameCancel);
        } else if resp.lost_focus() {
            actions.push(RowAction::RenameDone);
        }
    });
    ui.label(
        RichText::new("Enter keeps it · Esc cancels · empty = the rule's name")
            .size(11.0)
            .color(pal(ui).muted),
    );
}

/// Folders under `dir` down to `depth` levels, relative to it; `_Vibe` and hidden ones left out.
fn list_subfolders(dir: &Path, depth: usize) -> Vec<PathBuf> {
    fn walk(base: &Path, dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if depth == 0 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('_') || name.starts_with('.') {
                continue;
            }
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                let p = e.path();
                if let Ok(rel) = p.strip_prefix(base) {
                    out.push(rel.to_path_buf());
                }
                walk(base, &p, depth - 1, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, depth, &mut out);
    out
}

/// Same relative folder, ignoring case and separators.
fn same_rel(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        p.components()
            .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
            .collect::<Vec<_>>()
    };
    norm(a) == norm(b)
}

/// Destination path (with the keep-original toggle), the collision control, or what the row is
/// waiting for.
fn destination_cell(
    ui: &mut egui::Ui,
    p: &Proposal,
    edits: Option<&RowEdits>,
    ctx: &RowCtx<'_>,
    actions: &mut Vec<RowAction>,
) {
    let pl = pal(ui);
    let path = &p.file.path;
    match &p.status {
        RowStatus::NeedsYear => {
            let y = ctx
                .batch_year
                .unwrap_or_else(|| chrono::Local::now().year() as u16);
            ui.label(RichText::new("Needs a year —").color(pl.text));
            if ui
                .link(format!("use batch year {y} for all missing"))
                .clicked()
            {
                actions.push(RowAction::AllMissingYear(y));
            }
            return;
        }
        // A row whose own client is known but a copy's isn't still shows its destination.
        RowStatus::NeedsClient if p.client.is_none() || p.dest.is_none() => {
            ui.label(RichText::new("Pick a client to compute").color(pl.muted));
            return;
        }
        RowStatus::Paused => {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(RichText::new("Resolve the duplicate folder in").color(pl.muted));
                if ui.link("Conflicts").clicked() {
                    actions.push(RowAction::GotoConflicts);
                }
            });
            return;
        }
        RowStatus::Error(e) => {
            ui.add(egui::Label::new(RichText::new(e).color(pl.pill(PillKind::Err).1)).wrap());
            return;
        }
        RowStatus::Pending(_) => return,
        _ => {}
    }
    let Some(dest) = &p.dest else {
        if p.status == RowStatus::Skipped {
            ui.label(RichText::new("Left where it is").color(pl.muted));
        }
        return;
    };
    if let Some(r) = ctx.renaming.filter(|r| &r.path == path) {
        rename_field(ui, r, actions);
        return;
    }
    if let Some(CollisionWith::Batch { row, .. }) = &p.collision_with {
        // Two files of one batch are always numbered (no choice to make).
        ui.label(mono(format!("Same name as {}", file_name(row))).color(pl.pill(PillKind::Warn).1));
        ui.label(
            mono(format!(
                "→ {}",
                dest.file_name().unwrap_or_default().to_string_lossy()
            ))
            .color(pl.text),
        )
        .on_hover_text(dest.display().to_string());
        return;
    }
    if p.collides() {
        ui.label(mono("File already exists").color(pl.pill(PillKind::Err).1))
            .on_hover_text(dest.display().to_string());
        let mut choice = p.collision_choice;
        let options = [
            (CollisionChoice::Overwrite, "Overwrite"),
            (CollisionChoice::KeepBoth, "Keep both"),
            (CollisionChoice::Skip, "Skip"),
        ];
        if theme::segmented(ui, &mut choice, &options) {
            actions.push(RowAction::SetCollision(path.clone(), choice));
        }
        return;
    }
    let shown = ctx
        .root
        .and_then(|r| dest.strip_prefix(r).ok())
        .unwrap_or(dest);
    let dir = shown
        .parent()
        .map(|d| d.display().to_string())
        .unwrap_or_default();
    let name = shown
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    ui.horizontal_top(|ui| {
        let text_w = ui.available_width() - 30.0;
        ui.allocate_ui_with_layout(
            egui::vec2(text_w, 0.0),
            Layout::top_down(Align::Min),
            |ui| {
                let font = egui::TextStyle::Monospace.resolve(ui.style());
                let bold = egui::FontId::new(
                    font.size,
                    egui::FontFamily::Name(theme::MONO_SEMIBOLD.into()),
                );
                let mut job = egui::text::LayoutJob::default();
                job.wrap.max_width = text_w;
                job.append(
                    &format!("…\\{dir}\\"),
                    0.0,
                    egui::TextFormat::simple(font, pl.text),
                );
                job.append(&name, 0.0, egui::TextFormat::simple(bold, pl.text));
                // V5: click the name to rename the file.
                let resp = ui
                    .add(egui::Label::new(job).wrap().sense(egui::Sense::click()))
                    .on_hover_text(format!("{}\nClick to rename", dest.display()));
                let name = format!("Rename {}", file_name(path));
                resp.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Button, true, name.clone())
                });
                if resp.clicked() {
                    actions.push(RowAction::StartRename(path.clone()));
                }
                let typed = edits.is_some_and(|e| e.filename.is_some());
                let moved = edits.is_some_and(|e| e.dest_rel.is_some());
                if typed || moved {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        if typed {
                            pill(ui, PillKind::Neutral, "renamed").on_hover_text(
                                "Name typed by you. Right-click → Use the rule's name.",
                            );
                        }
                        if moved {
                            pill(ui, PillKind::Neutral, "folder picked").on_hover_text(
                                "Folder picked by you. Right-click → Use the rule's folder.",
                            );
                        }
                    });
                }
            },
        );
        let mut original = edits.is_some_and(|e| e.keep_original_name);
        let resp = theme::switch_small(
            ui,
            &mut original,
            &format!("Keep original name: {}", file_name(path)),
        )
        .on_hover_text("Keep original name");
        if resp.changed() {
            actions.push(RowAction::KeepOriginal(path.clone(), original));
        }
    });
}

/// One copy of a row (P07a, UI_SPEC §3): `copy · role`, its client, destination and status, ×.
fn copy_row_ui(
    cells: &mut Cells<'_, '_, '_>,
    pr: &Proposal,
    c: &CopyProposal,
    ctx: &RowCtx<'_>,
    actions: &mut Vec<RowAction>,
) {
    const TEXT_TOP: f32 = 4.0;
    let pl = ctx.pal;
    let path = &pr.file.path;
    let who = c
        .client
        .as_ref()
        .map(|x| x.name.clone())
        .or_else(|| c.looked_for.clone())
        .unwrap_or_else(|| "unassigned".into());
    cells.cell(|_| {});
    cells.cell(|ui| {
        ui.add_space(TEXT_TOP);
        ui.horizontal(|ui| {
            branch_icon(ui);
            let text = if c.role.is_empty() {
                "copy".to_string()
            } else {
                format!("copy · {}", c.role)
            };
            ui.label(RichText::new(text).size(12.0).color(pl.muted));
            if c.whole_of.is_some() {
                pill(ui, PillKind::Neutral, "whole PDF").on_hover_text(
                    "On split: Whole. The unsplit original (all pages) is copied here once.",
                );
            }
        });
    });
    cells.cell(|ui| match &c.client {
        Some(cl) => {
            ui.add_space(TEXT_TOP);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(&cl.name);
                ui.label(mono(&cl.id).color(pl.muted));
                if let CopyMatch::Name { related: false, .. } = c.client_match {
                    pill(ui, PillKind::Info, "not related");
                    if let Some(primary) = &pr.client
                        && ui.link("Add relationship…").clicked()
                    {
                        actions.push(RowAction::AddRelationship(
                            primary.id.clone(),
                            cl.id.clone(),
                        ));
                    }
                }
            });
        }
        None => {
            let why = match &c.looked_for {
                Some(n) => format!("\"{n}\" not found"),
                None => "Pick the copy's client".into(),
            };
            ui.label(RichText::new(why).size(11.0).color(pl.muted));
            if let Some((score, s)) = &c.suggestion {
                let resp = ui.add(egui::Button::new(suggestion_job(ui, &s.name, *score)));
                let name = format!("Use {} ({}) for the copy", s.name, s.id);
                resp.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Button, true, name.clone())
                });
                if resp.clicked() {
                    actions.push(RowAction::CopyClient(
                        path.clone(),
                        c.key.clone(),
                        s.id.clone(),
                    ));
                }
            }
            ui.horizontal_wrapped(|ui| {
                let pick = ui.small_button("Choose client…");
                let name = format!("Choose the copy client for {}", file_name(path));
                pick.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Button, true, name.clone())
                });
                if pick.clicked() {
                    actions.push(RowAction::PickCopy(
                        path.clone(),
                        c.key.clone(),
                        c.looked_for.clone().unwrap_or_default(),
                    ));
                }
                if let (Some(primary), Some(n)) = (&pr.client, &c.looked_for)
                    && ui.link("Add relationship…").clicked()
                {
                    actions.push(RowAction::AddRelationship(primary.id.clone(), n.clone()));
                }
            });
        }
    });
    cells.cell(|ui| {
        ui.add_space(TEXT_TOP);
        ui.label(RichText::new(format!("{} copy", pr.rule_name)).color(pl.muted));
    });
    cells.cell(|ui| {
        ui.add_space(TEXT_TOP);
        ui.label(pr.year.map_or_else(|| "—".to_string(), |y| y.to_string()));
    });
    cells.cell(|ui| match (&c.status, &c.dest) {
        (CopyStatus::Pending(why), _) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new(why).size(11.0).color(pl.muted));
            });
        }
        (CopyStatus::Error(e), _) => {
            ui.add(egui::Label::new(RichText::new(e).color(pl.pill(PillKind::Err).1)).wrap());
        }
        (_, Some(dest)) if c.collides() => {
            ui.label(mono("File already exists").color(pl.pill(PillKind::Err).1))
                .on_hover_text(dest.display().to_string());
            let mut choice = c.collision_choice;
            let options = [
                (CollisionChoice::Overwrite, "Overwrite"),
                (CollisionChoice::KeepBoth, "Keep both"),
                (CollisionChoice::Skip, "Skip"),
            ];
            if theme::segmented(ui, &mut choice, &options) {
                actions.push(RowAction::CopyCollision(
                    path.clone(),
                    c.key.clone(),
                    choice,
                ));
            }
        }
        (_, Some(dest)) => {
            ui.add_space(TEXT_TOP);
            let shown = ctx
                .root
                .and_then(|r| dest.strip_prefix(r).ok())
                .unwrap_or(dest);
            let dir = shown
                .parent()
                .map(|d| d.display().to_string())
                .unwrap_or_default();
            let name = shown
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let font = egui::TextStyle::Monospace.resolve(ui.style());
            let bold = egui::FontId::new(
                font.size,
                egui::FontFamily::Name(theme::MONO_SEMIBOLD.into()),
            );
            let mut job = egui::text::LayoutJob::default();
            job.wrap.max_width = ui.available_width();
            job.append(
                &format!("…\\{dir}\\"),
                0.0,
                egui::TextFormat::simple(font, pl.text),
            );
            job.append(&name, 0.0, egui::TextFormat::simple(bold, pl.text));
            ui.add(egui::Label::new(job).wrap())
                .on_hover_text(dest.display().to_string());
        }
        _ => {
            ui.add_space(TEXT_TOP);
            ui.label(RichText::new("Pick a client to compute").color(pl.muted));
        }
    });
    cells.cell(|ui| {
                    ui.add_space(TEXT_TOP);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                        match &c.status {
                            CopyStatus::Ready if c.collides() => {
                                pill(ui, PillKind::Err, "Collision").on_hover_text(
                                    collision_hover(
                                        c.collision_with.as_ref(),
                                        c.collision_choice,
                                        c.dest.as_deref(),
                                    ),
                                );
                            }
                            CopyStatus::Ready => {
                                pill(ui, PillKind::Ok, "Ready")
                                    .on_hover_text("This copy is written when you commit.");
                            }
                            CopyStatus::NeedsClient => {
                                pill(ui, PillKind::Warn, "Needs client").on_hover_text(
                                    "Pick the client this copy goes to (Choose…), or × to leave it out.",
                                );
                            }
                            CopyStatus::Skipped => {
                                pill(ui, PillKind::Neutral, "Skipped").on_hover_text(
                                    "Not written in this batch (removed, Skip chosen, or the same file is already there).",
                                );
                            }
                            CopyStatus::Pending(_) => {}
                            CopyStatus::Error(e) => {
                                pill(ui, PillKind::Err, "Error").on_hover_text(e);
                            }
                        }
                        if let Some(FolderAction::WillCreate(f)) = &c.folder {
                            pill(ui, PillKind::Info, "Will create").on_hover_text(format!(
                                "This client has no folder yet. {} is created when you commit.",
                                f.display()
                            ));
                        }
                        let x = ui
                            .small_button("×")
                            .on_hover_text("Remove this copy for this batch");
                        let name = format!("Remove copy: {who}");
                        x.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::Button, true, name.clone())
                        });
                        if x.clicked() {
                            actions.push(RowAction::RemoveCopy(path.clone(), c.key.clone()));
                        }
                    });
                });
}

/// "+ Copy to…" and the notes about copies that were not made, under a row's copies.
fn add_copy_line(cells: &mut Cells<'_, '_, '_>, pr: &Proposal, actions: &mut Vec<RowAction>) {
    cells.cell(|_| {});
    cells.cell(|ui| {
        let add = ui
            .link("+ Copy to…")
            .on_hover_text("Add a copy to another client for this batch");
        let name = format!("Copy {} to another client", file_name(&pr.file.path));
        add.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, name.clone()));
        if add.clicked() {
            actions.push(RowAction::AddCopy(pr.file.path.clone()));
        }
    });
    for _ in 2..5 {
        cells.cell(|_| {});
    }
    cells.cell(|ui| {
        ui.horizontal_wrapped(|ui| {
            for n in &pr.copy_notes {
                pill(ui, PillKind::Info, n).on_hover_text(n.as_str());
                if n == "no related clients"
                    && let Some(primary) = &pr.client
                    && ui.link("Add relationship…").clicked()
                {
                    actions.push(RowAction::AddRelationship(
                        primary.id.clone(),
                        String::new(),
                    ));
                }
            }
        });
    });
    cells.cell(|_| {});
}

/// A small triangle that folds a row's copies away (egui's fonts have no ▾ glyph).
fn fold_toggle(ui: &mut egui::Ui, folded: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(14.0, 16.0), egui::Sense::click());
    let c = if resp.hovered() {
        pal(ui).text
    } else {
        pal(ui).muted
    };
    let m = rect.center();
    let pts = if folded {
        vec![
            egui::pos2(m.x - 2.5, m.y - 4.0),
            egui::pos2(m.x + 3.0, m.y),
            egui::pos2(m.x - 2.5, m.y + 4.0),
        ]
    } else {
        vec![
            egui::pos2(m.x - 4.0, m.y - 2.5),
            egui::pos2(m.x + 4.0, m.y - 2.5),
            egui::pos2(m.x, m.y + 3.0),
        ]
    };
    ui.painter()
        .add(egui::Shape::convex_polygon(pts, c, Stroke::NONE));
    resp
}

/// The small "↳" branch in front of a copy row.
fn branch_icon(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
    let c = pal(ui).muted;
    let s = Stroke::new(1.2, c);
    let x = rect.left() + 3.0;
    ui.painter().line_segment(
        [
            egui::pos2(x, rect.top() + 1.0),
            egui::pos2(x, rect.center().y),
        ],
        s,
    );
    ui.painter().line_segment(
        [
            egui::pos2(x, rect.center().y),
            egui::pos2(rect.right() - 1.0, rect.center().y),
        ],
        s,
    );
}

/// "Whitfield Farms LLC  0.91" with the score as a small info pill.
fn suggestion_job(ui: &egui::Ui, name: &str, score: f64) -> egui::text::LayoutJob {
    let p = pal(ui);
    let body = egui::TextStyle::Body.resolve(ui.style());
    let (bg, fg) = p.pill(PillKind::Info);
    let mut job = egui::text::LayoutJob::default();
    job.append(name, 0.0, egui::TextFormat::simple(body, p.text));
    let mut score_fmt = egui::TextFormat::simple(
        egui::FontId::new(11.0, egui::FontFamily::Name(theme::SEMIBOLD.into())),
        fg,
    );
    score_fmt.background = bg;
    job.append(&format!(" {score:.2} "), 8.0, score_fmt);
    job
}

/// Six dots, the drag handle.
fn drag_dots(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 16.0), egui::Sense::drag());
    let c = pal(ui).muted;
    for col in 0..2 {
        for row in 0..3 {
            let pos = egui::pos2(
                rect.left() + 2.5 + col as f32 * 5.0,
                rect.top() + 3.0 + row as f32 * 5.0,
            );
            ui.painter().circle_filled(pos, 1.2, c);
        }
    }
}

/// The small round number in front of each file in the Combine order.
fn index_badge(ui: &mut egui::Ui, n: usize) {
    let p = pal(ui);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), 9.0, p.pill(PillKind::Neutral).0);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        n.to_string(),
        egui::FontId::new(11.0, egui::FontFamily::Name(theme::SEMIBOLD.into())),
        p.pill(PillKind::Neutral).1,
    );
}

/// Year picker over the accepted window (newest first), with "none". Amber border when `missing`.
fn year_combo(
    ui: &mut egui::Ui,
    id: impl egui::AsIdSalt,
    year: &mut Option<u16>,
    label: Option<egui::Id>,
    missing: bool,
) -> egui::Response {
    let now = chrono::Local::now().year();
    let resp = ui
        .scope(|ui| {
            if missing {
                let amber = pal(ui).attention;
                ui.visuals_mut().widgets.inactive.bg_stroke = Stroke::new(1.0, amber);
                ui.visuals_mut().widgets.hovered.bg_stroke = Stroke::new(1.0, amber);
            }
            egui::ComboBox::from_id_salt(id)
                .selected_text(year.map_or("—".to_string(), |y| y.to_string()))
                .width(62.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(year, None, "—");
                    for y in ((now - YEARS_BACK)..=(now + YEARS_AHEAD)).rev() {
                        ui.selectable_value(year, Some(y as u16), y.to_string());
                    }
                })
                .response
        })
        .inner;
    match label {
        Some(l) => resp.labelled_by(l),
        None => resp,
    }
}
fn parent(p: &Path) -> String {
    p.parent()
        .map(|d| d.display().to_string())
        .unwrap_or_default()
}
