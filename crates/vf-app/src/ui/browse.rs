//! Browse tab (UI_SPEC §1, P01 step 2): client tree · document grid · inspector.
//!
//! Reads a [`Snapshot`] of the catalog once; tree expansion, folder changes, sort and filter work
//! on it in memory. [`Browse::reload`] re-reads after anything changes the catalog.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use egui::{Align, Key, Layout, RichText, Sense};
use egui_extras::{Column, TableBuilder};
use vf_core::browse::{DocRow, IdList, Index, Scope, Snapshot, year_node_label};
use vf_core::db::Connection;

use crate::config::{Favorite, Prefs};
use crate::shell::{self, ShellAction};
use crate::ui::client_panel::{self, RelForm};
use crate::ui::cover_sheet::{self, CoverSheetDialog};
use crate::ui::import_csv::{self, ImportDialog};
use crate::ui::meta_editor::{self, MetaForm};
use crate::ui::new_client::{self, NewClientDialog};
use crate::ui::rename::{self as rename_ui, RenameDialog};
use crate::ui::widgets::{DialogOutcome, status_rank, year_label};
use crate::ui::{theme, widgets};

/// What Browse needs from the open cabinet to change things.
pub struct Env<'a> {
    pub conn: &'a mut Connection,
    pub root: &'a Path,
    pub device_id: &'a str,
}

pub const ROW_H: f32 = 32.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortCol {
    Name,
    DocType,
    Year,
    Status,
    Size,
    Modified,
    FiledBy,
    Hash,
}

impl SortCol {
    const ALL: [(SortCol, &'static str); 8] = [
        (SortCol::Name, "Name"),
        (SortCol::DocType, "Doc type"),
        (SortCol::Year, "Year"),
        (SortCol::Status, "Status"),
        (SortCol::Size, "Size"),
        (SortCol::Modified, "Modified"),
        (SortCol::FiledBy, "Filed by"),
        (SortCol::Hash, "Hash"),
    ];
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Node {
    Client(usize),
    /// A folder inside the client folder (`Income Tax/2025`).
    Folder(usize, String),
    /// A year in the year view.
    Year(usize, Option<i32>),
}

/// Documents being dragged from the grid (PA-23): their ids.
#[derive(Debug, Clone)]
struct DocDrag(Vec<String>);

/// Asking whether to move dragged documents into a folder.
struct MoveAsk {
    /// Cabinet-relative destination folder.
    dest: String,
    rows: Vec<vf_fs::rename::Row>,
}

/// What the folder-listing thread found: each client folder's folders, and files in client
/// folders that are not in the catalog (copied in through Explorer).
struct Listing {
    dirs: DiskDirs,
    new_files: Vec<vf_fs::sweep::Unknown>,
}

/// A combine running on its thread: the sources, the merged file, the result.
struct Combining {
    docs: Vec<DocRow>,
    tmp: PathBuf,
    rx: std::sync::mpsc::Receiver<Result<(), String>>,
}

/// Client folders' sub-folders on disk, by client folder (cabinet-relative).
type DiskDirs = Vec<(String, Vec<String>)>;

pub struct Browse {
    /// Each client's folders on disk, by its folder, as last listed; listed again on a thread
    /// after a reload (not more than every 2 s), so a reload never walks the share on the UI
    /// thread (QA A6).
    disk_dirs: std::collections::HashMap<String, Vec<String>>,
    disk_job: Option<std::sync::mpsc::Receiver<Listing>>,
    /// Files the last listing found in client folders and not in the catalog: added on the
    /// next frame (PA-23).
    new_files: Vec<vf_fs::sweep::Unknown>,
    /// A drop on a folder waiting for "Move" (PA-23).
    move_ask: Option<MoveAsk>,
    disk_listed: Option<std::time::Instant>,
    combining: Option<Combining>,
    /// For a repaint when a listing arrives.
    egui_ctx: Option<egui::Context>,
    pub snap: Snapshot,
    pub ix: Index,
    pub scope: Scope,
    expanded: HashSet<Node>,
    /// `expanded` by client id while the catalog is re-read.
    expanded_ids: Vec<(String, Option<String>)>,
    pub client_filter: String,
    pub name_filter: String,
    pub sort: (SortCol, bool),
    /// Doc indices shown in the grid, filtered and sorted.
    pub visible: Vec<usize>,
    /// Selected doc indices.
    pub selected: BTreeSet<usize>,
    /// Keyboard cursor and shift-select anchor, as positions in `visible`.
    pub cursor: Option<usize>,
    anchor: Option<usize>,
    scroll_to: Option<usize>,
    pub error: Option<String>,
    /// Shown above the grid until dismissed (the rebuild report after create/first open).
    pub notice: Option<String>,
    /// The `+ New client` dialog, while open.
    pub new_client: Option<NewClientDialog>,
    /// The "Import clients CSV…" dialog, while open.
    pub import: Option<ImportDialog>,
    /// The client inspector's add-relationship form.
    pub rel_form: RelForm,
    /// The document inspector's metadata form (reset when the selection changes).
    pub meta_form: MetaForm,
    /// This Windows login's undo stack, newest first (P05 step 4); `None` = not read yet.
    undo_stack: Option<Vec<vf_core::tx::Undoable>>,
    /// Ctrl+Z pressed outside a text field.
    undo_requested: bool,
    /// The selected document's versions (P05 step 1), for the document they were read for.
    versions: Option<(String, Vec<vf_fs::versions::Version>)>,
    /// "Move to trash" chosen for the selection (P05 step 3).
    pub trash_requested: bool,
    /// Asking "Move N documents to the trash?" (N = 2+).
    trash_asking: Option<usize>,
    /// The person said yes for N documents.
    trash_confirm: Option<usize>,
    /// The cabinet root, for the folders on disk in the tree (`set_root`).
    root: Option<PathBuf>,
    /// "Scan" clicked: the app starts it with Intake's scanner (owner 2026-11-04).
    pub scan_request: bool,
    /// A scan started from here is running: its outcome is shown here when it ends.
    pub scan_watching: bool,
    /// Documents dropped on a tree folder: (ids, cabinet-relative folder), planned in `show`.
    drop_request: Option<(Vec<String>, String)>,
    /// Legal holds covering what is shown (P07 step 4), for the selection / client they were
    /// read for.
    holds_seen: Option<(String, Vec<vf_fs::holds::Hold>)>,
    /// The reason typed for a new hold, while the form is open.
    hold_reason: Option<String>,
    /// A hold being released (asked first), and the PIN typed.
    hold_releasing: Option<(String, String)>,
    hold_pin: String,
    /// The `Rename…` dialog, while open.
    pub rename: Option<RenameDialog>,
    /// A PDF to show in the Viewer (taken by the app).
    pub view_request: Option<crate::ui::viewer::Doc>,
    /// `Combine` pressed: merged on the next frame (needs the catalog).
    combine_request: bool,
    /// Pinned / Recent / Favorites, from this PC's config; `prefs_dirty` asks the app to save.
    pub prefs: Prefs,
    pub prefs_dirty: bool,
    /// Document counts per entry of [`Browse::views`] and per favorite, and the start of
    /// "this week"; recomputed by `recount` when the catalog or the lists change.
    view_counts: [usize; 5],
    fav_counts: Vec<usize>,
    /// The firm's saved searches (`_Vibe/saved_searches.json`), set by the app.
    pub saved: Vec<vf_index::saved::SavedSearch>,
    /// A saved search clicked in the tree: run on the next frame (needs the catalog).
    saved_open: Option<usize>,
    /// A saved search to delete (needs the cabinet root).
    saved_delete: Option<String>,
    week_start: String,
    /// Hands files to Windows; tests replace it to record instead of launching apps.
    pub shell: ShellFn,
    /// A file action asked for this frame (doc index), run where the cabinet root is known.
    file_request: Option<(usize, FileAct)>,
    /// The `Print cover sheet` dialog, while open.
    pub cover: Option<CoverSheetDialog>,
    /// Where cover sheets are written (`<data>/sheets`).
    pub sheets_dir: std::path::PathBuf,
    /// B2 sync state for the `Sync to B2 ▾` menu and the client's pill (P11 step 10); `None`
    /// when sync is not set up or not unlocked: both are hidden.
    pub sync: Option<SyncView>,
    /// A sync action chosen here (taken by the app).
    pub sync_request: Option<SyncRequest>,
}

/// What Browse shows of sync.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SyncView {
    /// Pending documents per client id.
    pub pending: std::collections::BTreeMap<String, usize>,
    pub conflicts: usize,
    /// The client a running push or pull is for (`Some("")` = all).
    pub running_for: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SyncRequest {
    Push(String),
    Pull,
    Verify,
    /// Open the Sync tab, filtered to this client.
    Show(Option<String>),
}

/// How Browse hands files to Windows ([`shell::run`], or a recorder in tests).
pub type ShellFn = Box<dyn FnMut(&ShellAction) -> Result<(), String>>;

/// What to do with one document's file (P01 step 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileAct {
    /// PDFs in the Viewer tab (P04); other files in their registered app.
    Open,
    /// In the app Windows has for its type (a PDF in Acrobat, …).
    OpenExternal,
    /// Show it in Explorer.
    Reveal,
    /// Put its full path on the clipboard.
    CopyPath,
}

impl Browse {
    pub fn new(snap: Snapshot) -> Self {
        let ix = Index::build(&snap);
        let mut b = Browse {
            disk_dirs: Default::default(),
            new_files: vec![],
            move_ask: None,
            disk_job: None,
            disk_listed: None,
            combining: None,
            egui_ctx: None,
            snap,
            ix,
            scope: Scope::Cabinet,
            expanded: HashSet::new(),
            expanded_ids: vec![],
            client_filter: String::new(),
            name_filter: String::new(),
            sort: (SortCol::Name, true),
            visible: Vec::new(),
            selected: BTreeSet::new(),
            cursor: None,
            anchor: None,
            scroll_to: None,
            error: None,
            notice: None,
            new_client: None,
            import: None,
            rel_form: RelForm::new(),
            meta_form: MetaForm::default(),
            undo_stack: None,
            undo_requested: false,
            trash_asking: None,
            hold_releasing: None,
            hold_pin: String::new(),
            trash_confirm: None,
            versions: None,
            trash_requested: false,
            root: None,
            drop_request: None,
            scan_request: false,
            scan_watching: false,
            holds_seen: None,
            hold_reason: None,
            rename: None,
            prefs: Prefs::default(),
            prefs_dirty: false,
            view_counts: [0; 5],
            fav_counts: Vec::new(),
            saved: Vec::new(),
            view_request: None,
            combine_request: false,
            saved_open: None,
            saved_delete: None,
            week_start: week_start_utc(),
            shell: Box::new(shell::run),
            file_request: None,
            cover: None,
            sheets_dir: std::path::PathBuf::new(),
            sync: None,
            sync_request: None,
        };
        b.recount();
        b.refresh();
        b
    }

    pub fn load(conn: &Connection) -> Self {
        match vf_core::browse::load(conn) {
            Ok(s) => Self::new(s),
            Err(e) => {
                let mut b = Self::new(Snapshot::default());
                b.error = Some(format!("The catalog could not be read: {e}"));
                b
            }
        }
    }

    /// Adds each client's folders on disk to the tree (empty template folders included), when
    /// Browse knows the cabinet root, and looks for files copied into client folders through
    /// Explorer (added on the next frame; PA-23). Listed on a thread (QA A6).
    fn add_disk_folders(&mut self) {
        self.apply_disk_dirs();
        self.list_disk(false);
    }

    /// Starts a folder listing on its thread: after a reload (`again` = false; not more than
    /// every 2 s), or every 30 s while Browse is shown (`again` = true).
    fn list_disk(&mut self, again: bool) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let clients: Vec<(String, String)> = self
            .snap
            .clients
            .iter()
            .filter_map(|c| {
                c.folder_rel
                    .clone()
                    .filter(|r| !r.is_empty())
                    .map(|r| (r, c.id.clone()))
            })
            .collect();
        let known: HashSet<String> = self
            .snap
            .docs
            .iter()
            .map(|d| d.rel_path.to_lowercase())
            .collect();
        let hashes: HashSet<String> = self.snap.docs.iter().map(|d| d.hash.clone()).collect();
        let list = move || -> Listing {
            let dirs = clients
                .iter()
                .map(|(rel, _)| {
                    let mut dirs = Vec::new();
                    leaf_dirs(&root.join(rel), "", 0, &mut dirs);
                    (rel.clone(), dirs)
                })
                .collect();
            let new_files = vf_fs::sweep::find_new(&root, &clients, &known, &hashes);
            Listing { dirs, new_files }
        };
        if cfg!(test) {
            // Tests: at once, so the tree is complete on the next frame.
            let got = list();
            self.take_listing(got);
            return;
        }
        let wait = std::time::Duration::from_secs(if again { 30 } else { 2 });
        let fresh = self.disk_listed.is_some_and(|t| t.elapsed() < wait);
        if self.disk_job.is_some() || fresh {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = self.egui_ctx.clone();
        let started = std::thread::Builder::new()
            .name("browse-folders".into())
            .spawn(move || {
                let _ = tx.send(list());
                if let Some(c) = ctx {
                    c.request_repaint();
                }
            });
        if started.is_ok() {
            self.disk_job = Some(rx);
        }
    }

    /// The cached folders, on the current tree.
    fn apply_disk_dirs(&mut self) {
        for (ci, c) in self.snap.clients.iter().enumerate() {
            if let Some(dirs) = c.folder_rel.as_deref().and_then(|r| self.disk_dirs.get(r)) {
                self.ix.add_folders(ci, dirs);
            }
        }
    }

    fn take_listing(&mut self, got: Listing) {
        self.disk_dirs = got.dirs.into_iter().collect();
        self.disk_listed = Some(std::time::Instant::now());
        self.new_files = got.new_files;
        let before = self.ix.clients.clone();
        self.apply_disk_dirs();
        if self.ix.clients != before {
            self.refresh();
        }
    }

    /// Takes a finished folder listing (every frame), and starts the next one every 30 s.
    fn poll_disk(&mut self) {
        if let Some(rx) = &self.disk_job {
            match rx.try_recv() {
                Ok(got) => {
                    self.disk_job = None;
                    self.take_listing(got);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.disk_job = None,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        } else if !cfg!(test) {
            self.list_disk(true);
            if let Some(c) = &self.egui_ctx {
                c.request_repaint_after(std::time::Duration::from_secs(30));
            }
        }
    }

    /// Adds the files the last listing found (copied into client folders through Explorer) as
    /// documents, so they show without waiting for the weekly sweep (PA-23, owner 2026-11-04).
    fn add_new_files(&mut self, env: &mut Env) {
        if self.new_files.is_empty() {
            return;
        }
        let found = std::mem::take(&mut self.new_files);
        match vf_fs::sweep::adopt_new(env.conn, env.root, env.device_id, &found) {
            Ok(added) if added.is_empty() => {}
            Ok(added) => {
                self.reload(env.conn);
                let names: Vec<&str> = added
                    .iter()
                    .take(3)
                    .map(|p| p.rsplit('/').next().unwrap_or(p))
                    .collect();
                let more = if added.len() > 3 {
                    format!(" and {} more", added.len() - 3)
                } else {
                    String::new()
                };
                self.notice = Some(format!(
                    "Added {} new file{} found in client folders: {}{more}. Set the doc type in the inspector.",
                    added.len(),
                    if added.len() == 1 { "" } else { "s" },
                    names.join(", ")
                ));
            }
            Err(e) => self.error = Some(format!("New files in client folders not added: {e}")),
        }
    }

    /// Sets the cabinet root (the tree then shows folders on disk too) and re-reads the tree.
    pub fn set_root(&mut self, root: &Path) {
        self.root = Some(root.to_path_buf());
        self.ix = Index::build(&self.snap);
        self.add_disk_folders();
    }

    /// Re-reads the catalog, keeping scope (when it still exists) and selection by document id.
    pub fn reload(&mut self, conn: &Connection) {
        let keep: HashSet<String> = self
            .selected
            .iter()
            .map(|&i| self.snap.docs[i].id.clone())
            .collect();
        let old_client = self.scope_client().map(|c| self.snap.clients[c].id.clone());
        self.expanded_ids = self
            .expanded
            .iter()
            .filter_map(|n| match n {
                Node::Client(c) => Some((self.snap.clients.get(*c)?.id.clone(), None)),
                Node::Folder(c, f) => {
                    Some((self.snap.clients.get(*c)?.id.clone(), Some(f.clone())))
                }
                Node::Year(c, y) => Some((
                    self.snap.clients.get(*c)?.id.clone(),
                    Some(format!(
                        "\u{1}{}",
                        y.map_or(String::new(), |y| y.to_string())
                    )),
                )),
            })
            .collect();
        match vf_core::browse::load(conn) {
            Ok(s) => {
                self.snap = s;
                self.ix = Index::build(&self.snap);
                self.add_disk_folders();
                self.error = None;
            }
            Err(e) => {
                self.error = Some(format!("The catalog could not be read: {e}"));
                return;
            }
        }
        let new_client =
            old_client.and_then(|id| self.snap.clients.iter().position(|c| c.id == id));
        self.scope = match (&self.scope, new_client) {
            (Scope::Client(_), Some(c)) => Scope::Client(c),
            (Scope::Folder { folder, .. }, Some(c)) => Scope::Folder {
                client: c,
                folder: folder.clone(),
            },
            (Scope::Year { year, .. }, Some(c)) => Scope::Year {
                client: c,
                year: *year,
            },
            (Scope::YearFolder { year, folder, .. }, Some(c)) => Scope::YearFolder {
                client: c,
                year: *year,
                folder: folder.clone(),
            },
            (Scope::Unassigned, _) => Scope::Unassigned,
            (
                s @ (Scope::NeedsDocType
                | Scope::NeedsTaxYear
                | Scope::ReceivedSince(_)
                | Scope::Ids { .. }
                | Scope::Saved { .. }),
                _,
            ) => s.clone(),
            _ => Scope::Cabinet,
        };
        // Keep what was open: client indices can shift when a client is added or removed.
        let ids: Vec<&str> = self.snap.clients.iter().map(|c| c.id.as_str()).collect();
        let old_ids = std::mem::take(&mut self.expanded_ids);
        self.expanded = old_ids
            .into_iter()
            .filter_map(|(id, f)| {
                let c = ids.iter().position(|i| *i == id)?;
                Some(match f {
                    None => Node::Client(c),
                    Some(f) => match f.strip_prefix('\u{1}') {
                        Some(y) => Node::Year(c, y.parse().ok()),
                        None => Node::Folder(c, f),
                    },
                })
            })
            .collect();
        self.undo_stack = None;
        self.versions = None;
        self.holds_seen = None;
        self.recount();
        self.selected = self
            .snap
            .docs
            .iter()
            .enumerate()
            .filter(|(_, d)| keep.contains(&d.id))
            .map(|(i, _)| i)
            .collect();
        self.refresh();
    }

    fn scope_client(&self) -> Option<usize> {
        match &self.scope {
            Scope::Client(c)
            | Scope::Folder { client: c, .. }
            | Scope::Year { client: c, .. }
            | Scope::YearFolder { client: c, .. } => Some(*c),
            _ => None,
        }
    }

    /// Shows `scope` in the grid and clears the selection.
    pub fn set_scope(&mut self, scope: Scope) {
        if self.scope != scope {
            self.trash_asking = None;
            self.scope = scope;
            self.selected.clear();
            self.cursor = None;
            self.anchor = None;
            self.refresh();
        }
    }

    /// The Views section: (label, scope, warn when non-zero), in display order.
    fn views(&self) -> [(&'static str, Scope, bool); 5] {
        [
            (
                IdList::Recent.label(),
                self.ids_scope(IdList::Recent),
                false,
            ),
            (
                IdList::Pinned.label(),
                self.ids_scope(IdList::Pinned),
                false,
            ),
            ("Needs doc type", Scope::NeedsDocType, true),
            ("Needs tax year", Scope::NeedsTaxYear, true),
            (
                "Received this week",
                Scope::ReceivedSince(self.week_start.clone()),
                false,
            ),
        ]
    }

    /// Recounts the Views and Favorites (full scans: only when the catalog or lists change,
    /// not per sort or filter keystroke).
    fn recount(&mut self) {
        self.week_start = week_start_utc();
        let views = self.views();
        for (i, (_, v, _)) in views.iter().enumerate() {
            self.view_counts[i] = self.ix.docs_in(&self.snap, v).len();
        }
        self.fav_counts = self
            .prefs
            .favorites
            .iter()
            .map(|f| {
                self.favorite_scope(f)
                    .map(|t| self.ix.docs_in(&self.snap, &t).len())
                    .unwrap_or(0)
            })
            .collect();
    }

    /// Recomputes `visible` from scope, name filter and sort.
    pub fn refresh(&mut self) {
        if let Scope::Ids { list, .. } = &self.scope {
            // Pinned / Recent follow the lists as they change.
            self.scope = self.ids_scope(*list);
        }
        let needle = self.name_filter.trim().to_lowercase();
        let mut v = self.ix.docs_in(&self.snap, &self.scope);
        if !needle.is_empty() {
            v.retain(|&i| self.snap.docs[i].name().to_lowercase().contains(&needle));
        }
        let docs = &self.snap.docs;
        let (col, asc) = self.sort;
        if col == SortCol::Name {
            // One lower-case key per row, not two allocations per comparison.
            v.sort_by_cached_key(|&i| (docs[i].name().to_lowercase(), docs[i].rel_path.clone()));
            if !asc {
                v.reverse();
            }
        } else {
            v.sort_by(|&a, &b| {
                let (a, b) = (&docs[a], &docs[b]);
                let o = match col {
                    SortCol::Name => unreachable!("sorted above"),
                    SortCol::DocType => a.doc_type.cmp(&b.doc_type),
                    SortCol::Year => a.tax_year.cmp(&b.tax_year),
                    SortCol::Status => status_rank(&a.status).cmp(&status_rank(&b.status)),
                    SortCol::Size => a.size.cmp(&b.size),
                    SortCol::Modified => a.updated_at.cmp(&b.updated_at),
                    SortCol::FiledBy => a.filed_by.cmp(&b.filed_by),
                    SortCol::Hash => a.hash.cmp(&b.hash),
                }
                .then_with(|| a.rel_path.cmp(&b.rel_path));
                if asc { o } else { o.reverse() }
            });
        }
        self.visible = v;
        let mut shown = vec![false; self.snap.docs.len()];
        for &i in &self.visible {
            shown[i] = true;
        }
        self.selected.retain(|&i| shown[i]);
        if self.cursor.is_some_and(|c| c >= self.visible.len()) {
            self.cursor = None;
        }
        // Checked on its own: a cursor still in range does not make the anchor so (QA A1).
        if self.anchor.is_some_and(|a| a >= self.visible.len()) {
            self.anchor = None;
        }
    }

    fn ids_scope(&self, list: IdList) -> Scope {
        let ids = match list {
            IdList::Pinned => self.prefs.pinned.clone(),
            IdList::Recent => self.prefs.recent.clone(),
        };
        Scope::Ids { list, ids }
    }

    /// The selected PDFs, in grid order, merged into one new document beside the first (one
    /// bookmark per source; Filer's `merge_pdfs`). The sources are not changed.
    fn combine_requested(&mut self, env: &mut Env) {
        if !std::mem::take(&mut self.combine_request) || self.combining.is_some() {
            return;
        }
        let docs: Vec<DocRow> = self
            .visible
            .iter()
            .filter(|i| self.selected.contains(i))
            .map(|&i| self.snap.docs[i].clone())
            .collect();
        let started = (|| -> Result<Combining, String> {
            docs.first().ok_or("select the documents to combine")?;
            let inputs: Vec<(std::path::PathBuf, String)> = docs
                .iter()
                .map(|d| {
                    (
                        env.root.join(&d.rel_path),
                        d.name().trim_end_matches(".pdf").to_string(),
                    )
                })
                .collect();
            let tmp = vf_fs::content::tmp_file(env.root, "pdf")?;
            let (tx, rx) = std::sync::mpsc::channel();
            let out = tmp.clone();
            // Every page of every source is read and written: on its own thread (QA A6).
            let work = move || filer_pdf::pdf::merge_pdfs(&inputs, &out).map(drop);
            if cfg!(test) {
                let _ = tx.send(work());
            } else {
                let ctx = self.egui_ctx.clone();
                std::thread::Builder::new()
                    .name("browse-combine".into())
                    .spawn(move || {
                        let _ = tx.send(work());
                        if let Some(c) = ctx {
                            c.request_repaint();
                        }
                    })
                    .map_err(|e| e.to_string())?;
            }
            Ok(Combining { docs, tmp, rx })
        })();
        match started {
            Ok(c) => {
                self.combining = Some(c);
                self.notice = Some("Combining…".into());
            }
            Err(e) => self.error = Some(format!("Not combined: {e}")),
        }
    }

    /// Takes a finished combine (every frame): the merged file becomes a new document.
    fn finish_combine(&mut self, env: &mut Env) {
        let Some(c) = &self.combining else { return };
        let merged = match c.rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err("the combine stopped unexpectedly".into())
            }
        };
        let Combining { docs, tmp, .. } = self.combining.take().expect("checked");
        let r = merged
            .inspect_err(|_| {
                let _ = std::fs::remove_file(&tmp);
            })
            .and_then(|()| {
                let first = docs.first().ok_or("no documents")?;
                let dir = first
                    .rel_path
                    .rsplit_once('/')
                    .map(|(d, _)| d)
                    .unwrap_or("");
                let name = format!("Combined {}.pdf", first.name().trim_end_matches(".pdf"));
                vf_fs::content::add(
                    env.conn,
                    env.root,
                    env.device_id,
                    &tmp,
                    vf_fs::content::NewDoc {
                        rel_path: if dir.is_empty() {
                            name
                        } else {
                            format!("{dir}/{name}")
                        },
                        client_id: first.client_id.clone(),
                        doc_type_id: None,
                        tax_year: first.tax_year,
                        source: None,
                        tags: vec!["combined".into()],
                        label: "combine".into(),
                        detail: serde_json::json!({
                            "sources": docs.iter().map(|d| d.id.clone()).collect::<Vec<_>>(),
                        }),
                    },
                )
            });
        match r {
            Ok(added) => {
                self.reload(env.conn);
                self.notice = Some(format!(
                    "Combined {} documents into {} (one bookmark each); the originals are not changed.",
                    docs.len(),
                    added.rel_path
                ));
            }
            Err(e) => {
                self.notice = None;
                self.error = Some(format!("Not combined: {e}"));
            }
        }
    }

    /// Runs a saved search clicked in the tree (its results become the grid) and deletes one
    /// asked for.
    fn saved_requests(&mut self, env: &Env) {
        if let Some(i) = self.saved_open.take()
            && let Some(s) = self.saved.get(i).cloned()
        {
            match vf_index::search::search(env.conn, &s.query, 100_000) {
                Ok(hits) => self.set_scope(Scope::Saved {
                    name: s.name,
                    ids: hits.into_iter().map(|h| h.doc_id).collect(),
                }),
                Err(e) => self.error = Some(format!("The saved search did not run: {e}")),
            }
        }
        if let Some(id) = self.saved_delete.take() {
            match vf_index::saved::remove(env.root, &id) {
                Ok(()) => self.saved = vf_index::saved::load(env.root),
                Err(e) => self.error = Some(format!("Could not delete the saved search: {e}")),
            }
        }
    }

    /// Sets the Pinned / Recent / Favorites lists (from config) and recounts.
    pub fn set_prefs(&mut self, prefs: Prefs) {
        self.prefs = prefs;
        self.recount();
        self.refresh();
    }

    /// After any change to `prefs`: asks the app to save, recounts, refreshes.
    fn prefs_changed(&mut self) {
        self.prefs_dirty = true;
        self.recount();
        self.refresh();
    }

    /// The document toolbar file actions act on: the cursor row when it is selected, else the
    /// first selected one.
    fn target_doc(&self) -> Option<usize> {
        self.cursor
            .and_then(|c| self.visible.get(c).copied())
            .filter(|i| self.selected.contains(i))
            .or_else(|| self.selected.iter().next().copied())
    }

    /// Runs a file action on document `di`.
    fn run_file_act(&mut self, ctx: &egui::Context, root: &Path, di: usize, act: FileAct) {
        let d = &self.snap.docs[di];
        let path = shell::full_path(root, &d.rel_path);
        let id = d.id.clone();
        let result = match act {
            FileAct::Open if d.rel_path.to_lowercase().ends_with(".pdf") => {
                self.view_request = Some(crate::ui::viewer::Doc::at(
                    root,
                    &d.id,
                    &d.rel_path,
                    0,
                    vec![],
                ));
                Ok(())
            }
            FileAct::Open | FileAct::OpenExternal => (self.shell)(&ShellAction::Open(path)),
            FileAct::Reveal => (self.shell)(&ShellAction::Reveal(path)),
            FileAct::CopyPath => {
                ctx.copy_text(path.display().to_string());
                self.notice = Some(format!("Copied {}", path.display()));
                Ok(())
            }
        };
        match result {
            Ok(()) if matches!(act, FileAct::Open | FileAct::OpenExternal) => {
                self.prefs.touch(&[id]);
                self.prefs_changed();
            }
            Ok(()) => {}
            Err(e) => self.error = Some(e),
        }
    }

    /// Ids of the selected documents.
    pub fn selected_ids(&self) -> Vec<String> {
        self.selected
            .iter()
            .map(|&i| self.snap.docs[i].id.clone())
            .collect()
    }

    /// The tree scope a favorite points at, when its client is still here. Favorites saved
    /// before the tree mirrored the disk (PA-23) named a year: a year favorite opens the client,
    /// a folder favorite its folder for that year when there is one.
    fn favorite_scope(&self, f: &Favorite) -> Option<Scope> {
        let ci = |id: &str| self.snap.clients.iter().position(|c| c.id == id);
        Some(match f {
            Favorite::Client { client } => Scope::Client(ci(client)?),
            Favorite::Year { client, year } => Scope::Year {
                client: ci(client)?,
                year: *year,
            },
            Favorite::Folder {
                client,
                year: None,
                folder,
            } => Scope::Folder {
                client: ci(client)?,
                folder: folder.clone(),
            },
            Favorite::Folder {
                client,
                year,
                folder,
            } => Scope::YearFolder {
                client: ci(client)?,
                year: *year,
                folder: folder.clone(),
            },
        })
    }

    /// The favorite for a tree scope (client or folder).
    fn as_favorite(&self, s: &Scope) -> Option<Favorite> {
        let id = |c: usize| self.snap.clients[c].id.clone();
        Some(match s {
            Scope::Client(c) => Favorite::Client { client: id(*c) },
            Scope::Folder { client, folder } => Favorite::Folder {
                client: id(*client),
                year: None,
                folder: folder.clone(),
            },
            Scope::Year { client, year } => Favorite::Year {
                client: id(*client),
                year: *year,
            },
            Scope::YearFolder {
                client,
                year,
                folder,
            } => Favorite::Folder {
                client: id(*client),
                year: *year,
                folder: folder.clone(),
            },
            _ => return None,
        })
    }

    fn favorite_label(&self, f: &Favorite) -> String {
        let Some(s) = self.favorite_scope(f) else {
            return "(client no longer in the cabinet)".into();
        };
        let name = |c: usize| self.snap.clients[c].name.clone();
        match s {
            Scope::Client(c) => name(c),
            Scope::Folder { client, folder } => {
                format!("{} · {}", name(client), folder.replace('/', " › "))
            }
            Scope::Year { client, year } => {
                format!("{} · {}", name(client), year_node_label(year))
            }
            Scope::YearFolder {
                client,
                year,
                folder,
            } => format!(
                "{} · {} · {}",
                name(client),
                year_node_label(year),
                year_folder_label(&folder)
            ),
            _ => String::new(),
        }
    }

    /// Plans moving `ids` into `dest` (a folder dropped on) and asks first (PA-23).
    fn ask_move(&mut self, conn: &Connection, root: &Path, ids: Vec<String>, dest: String) {
        match vf_fs::moveto::plan(conn, root, &ids, &dest) {
            Ok(rows) if rows.iter().any(vf_fs::rename::Row::renames) => {
                self.move_ask = Some(MoveAsk { dest, rows });
            }
            Ok(rows) => {
                let why = rows.iter().find_map(|r| match &r.outcome {
                    vf_fs::rename::Outcome::Skip(w) => Some(w.clone()),
                    _ => None,
                });
                self.notice = Some(match why {
                    Some(w) => format!("Not moved: {w}."),
                    None => "Already in that folder.".into(),
                });
            }
            Err(e) => self.error = Some(format!("Not moved: {e}")),
        }
    }

    pub fn sort_by(&mut self, col: SortCol) {
        self.sort = if self.sort.0 == col {
            (col, !self.sort.1)
        } else {
            (col, true)
        };
        self.refresh();
    }

    /// Row click: plain = only this row; Ctrl = toggle; Shift = range from the anchor.
    pub fn click_row(&mut self, pos: usize, m: egui::Modifiers) {
        let doc = self.visible[pos];
        if m.shift {
            let a = self
                .anchor
                .filter(|&a| a < self.visible.len())
                .unwrap_or(pos);
            if !m.command {
                self.selected.clear();
            }
            let (lo, hi) = (a.min(pos), a.max(pos));
            self.selected.extend(self.visible[lo..=hi].iter().copied());
        } else if m.command {
            if !self.selected.remove(&doc) {
                self.selected.insert(doc);
            }
            self.anchor = Some(pos);
        } else {
            self.selected.clear();
            self.selected.insert(doc);
            self.anchor = Some(pos);
        }
        self.cursor = Some(pos);
    }

    pub fn toggle_row(&mut self, pos: usize) {
        let doc = self.visible[pos];
        if !self.selected.remove(&doc) {
            self.selected.insert(doc);
        }
        self.cursor = Some(pos);
        self.anchor = Some(pos);
    }

    /// Grid keys when no text field has focus: ↑/↓ (Shift extends), Space toggles, Ctrl+A, Esc.
    pub fn keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() || self.visible.is_empty() {
            return;
        }
        let last = self.visible.len() - 1;
        if ctx.input(|i| i.key_pressed(Key::Enter))
            && let Some(c) = self.cursor
        {
            self.file_request = Some((self.visible[c], FileAct::Open));
        }
        let (down, up, space, all, undo, esc, home, end, m) = ctx.input(|i| {
            (
                i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::Space),
                i.modifiers.command && i.key_pressed(Key::A),
                i.modifiers.command && i.key_pressed(Key::Z),
                i.key_pressed(Key::Escape),
                i.key_pressed(Key::Home),
                i.key_pressed(Key::End),
                i.modifiers,
            )
        });
        let target = match (down, up, home, end, self.cursor) {
            (true, _, _, _, None) | (_, true, _, _, None) | (_, _, true, _, _) => Some(0),
            (_, _, _, true, _) => Some(last),
            (true, _, _, _, Some(c)) => Some((c + 1).min(last)),
            (_, true, _, _, Some(c)) => Some(c.saturating_sub(1)),
            _ => None,
        };
        if let Some(t) = target {
            self.click_row(
                t,
                egui::Modifiers {
                    shift: m.shift,
                    ..Default::default()
                },
            );
            self.scroll_to = Some(t);
        }
        if space && let Some(c) = self.cursor {
            self.toggle_row(c);
        }
        if all {
            self.selected = self.visible.iter().copied().collect();
        }
        if esc {
            self.selected.clear();
        }
        if undo {
            self.undo_requested = true;
        }
    }

    fn selected_docs(&self) -> Vec<&DocRow> {
        self.selected.iter().map(|&i| &self.snap.docs[i]).collect()
    }
}

/// Monday 00:00 this week, local time, as RFC 3339 UTC ("Received this week").
pub fn week_start_utc() -> String {
    use chrono::{Datelike, Local, TimeZone};
    let today = Local::now().date_naive();
    let monday = today - chrono::Duration::days(today.weekday().num_days_from_monday() as i64);
    let start = Local
        .from_local_datetime(&monday.and_hms_opt(0, 0, 0).expect("midnight"))
        .earliest()
        .unwrap_or_else(Local::now);
    start
        .with_timezone(&chrono::Utc)
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

/// A year view folder's name: "(client folder)" for documents directly in it.
fn year_folder_label(f: &str) -> &str {
    if f.is_empty() { "(client folder)" } else { f }
}

/// Draws the whole tab (three panes). Call where the central panel would go.
pub fn show(ui: &mut egui::Ui, b: &mut Browse, env: &mut Env) {
    if b.egui_ctx.is_none() {
        b.egui_ctx = Some(ui.ctx().clone());
    }
    b.poll_disk();
    b.add_new_files(env);
    b.finish_combine(env);
    b.keys(ui.ctx());
    b.saved_requests(env);
    b.combine_requested(env);
    egui::Panel::left("browse-tree")
        .default_size(272.0)
        .size_range(200.0..=480.0)
        .frame(egui::Frame::new().fill(theme::PANE).inner_margin(8))
        .show(ui, |ui| tree(ui, b));
    egui::Panel::right("browse-inspector")
        .exact_size(320.0)
        .resizable(false)
        .frame(egui::Frame::new().fill(theme::PANE).inner_margin(12))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("inspector-scroll")
                .show(ui, |ui| inspector(ui, b, env))
        });
    egui::CentralPanel::default().show(ui, |ui| grid(ui, b, env.conn));
    drag_label(ui.ctx());
    if let Some((ids, dest)) = b.drop_request.take() {
        b.ask_move(env.conn, env.root, ids, dest);
    }
    move_dialog(ui.ctx(), b, env);
    if let Some(d) = &mut b.new_client {
        match new_client::show(ui.ctx(), d, env) {
            DialogOutcome::Open => {}
            DialogOutcome::Cancelled => b.new_client = None,
            DialogOutcome::Done(new_client::Created { client_id, folder }) => {
                b.new_client = None;
                b.reload(env.conn);
                if let Some(c) = b.snap.clients.iter().position(|c| c.id == client_id) {
                    b.set_scope(Scope::Client(c));
                    b.expanded.insert(Node::Client(c));
                }
                b.notice = Some(format!("Created {folder}."));
            }
        }
    }
    if let Some((di, act)) = b.file_request.take() {
        b.run_file_act(ui.ctx(), env.root, di, act);
    }
    if let Some(d) = &mut b.rename {
        match rename_ui::show(ui.ctx(), d, env) {
            DialogOutcome::Open => {}
            DialogOutcome::Cancelled => b.rename = None,
            DialogOutcome::Done(n) => {
                b.prefs.touch(&d.ids);
                b.prefs_dirty = true;
                b.rename = None;
                b.reload(env.conn);
                b.notice = Some(format!("Renamed {n} files (one transaction)."));
            }
        }
    }
    if let Some(d) = &mut b.cover {
        match cover_sheet::show(ui.ctx(), d, &b.sheets_dir) {
            DialogOutcome::Open => {}
            DialogOutcome::Cancelled => b.cover = None,
            DialogOutcome::Done(p) => {
                b.cover = None;
                b.notice = Some(match (b.shell)(&ShellAction::Open(p.clone())) {
                    Ok(()) => format!("Cover sheet opened to print ({}).", p.display()),
                    Err(e) => format!("Cover sheet saved to {} but did not open: {e}", p.display()),
                });
            }
        }
    }
    if let Some(d) = &mut b.import {
        match import_csv::show(ui.ctx(), d, env) {
            DialogOutcome::Open => {}
            DialogOutcome::Cancelled => b.import = None,
            DialogOutcome::Done(msg) => {
                b.import = None;
                b.reload(env.conn);
                b.notice = Some(msg);
            }
        }
    }
}

/// The ID to prefill in `+ New client`: the selected client when it has no folder yet.
fn folderless_selected(b: &Browse) -> Option<String> {
    match b.scope {
        Scope::Client(c) if b.snap.clients[c].folder_rel.is_none() => {
            Some(b.snap.clients[c].ext_id.clone())
        }
        _ => None,
    }
}

fn tree(ui: &mut egui::Ui, b: &mut Browse) {
    let mut pick: Option<Scope> = None;
    let mut fav_remove: Option<usize> = None;
    ui.label(widgets::caption("VIEWS"));
    for (i, (label, scope, warn)) in b.views().into_iter().enumerate() {
        let sel = std::mem::discriminant(&b.scope) == std::mem::discriminant(&scope)
            && match (&b.scope, &scope) {
                (Scope::Ids { list: a, .. }, Scope::Ids { list: c, .. }) => a == c,
                _ => true,
            };
        if tree_row(ui, 0, None, warn, label, b.view_counts[i], sel)
            .1
            .clicked()
        {
            pick = Some(scope);
        }
    }
    if !b.prefs.favorites.is_empty() {
        ui.label(widgets::caption("FAVORITES"));
        for (i, f) in b.prefs.favorites.iter().enumerate() {
            let target = b.favorite_scope(f);
            let label = b.favorite_label(f);
            let sel = target.as_ref() == Some(&b.scope);
            let n = b.fav_counts.get(i).copied().unwrap_or(0);
            let (_, resp) = tree_row(ui, 0, None, false, &label, n, sel);
            if resp.clicked()
                && let Some(t) = target
            {
                pick = Some(t);
            }
            resp.context_menu(|ui| {
                if ui.button("Remove from favorites").clicked() {
                    fav_remove = Some(i);
                    ui.close();
                }
            });
        }
    }
    if !b.saved.is_empty() {
        ui.label(widgets::caption("SAVED SEARCHES"));
        for i in 0..b.saved.len() {
            let name = b.saved[i].name.clone();
            // Selected: its hit count (the grid shows its results).
            let shown = match &b.scope {
                Scope::Saved { name: cur, ids } if *cur == name => Some(ids.len()),
                _ => None,
            };
            let (sel, n) = (shown.is_some(), shown.unwrap_or(0));
            let (_, resp) = tree_row(ui, 0, None, false, &name, n, sel);
            if resp.clicked() {
                b.saved_open = Some(i);
            }
            let id = b.saved[i].id.clone();
            resp.context_menu(|ui| {
                if ui.button("Delete saved search").clicked() {
                    b.saved_delete = Some(id.clone());
                    ui.close();
                }
            });
        }
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("CABINET").small().color(theme::INK_MUTED));
        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
            // Folders as on disk, or by tax year (owner 2026-11-04); kept on this PC.
            let by_year = b.prefs.tree_by_year;
            if ui
                .selectable_label(by_year, "By year")
                .on_hover_text("Client, then tax year, then folder")
                .clicked()
                && !by_year
            {
                b.prefs.tree_by_year = true;
                b.prefs_changed();
            }
            if ui
                .selectable_label(!by_year, "Folders")
                .on_hover_text("The client's folders as they are on disk")
                .clicked()
                && by_year
            {
                b.prefs.tree_by_year = false;
                b.prefs_changed();
            }
        });
    });
    ui.add(
        egui::TextEdit::singleline(&mut b.client_filter)
            .hint_text("Find client by name or ID")
            .desired_width(f32::INFINITY),
    );
    let needle = b.client_filter.trim().to_lowercase();
    let shown: Vec<usize> = (0..b.snap.clients.len())
        .filter(|&i| {
            let c = &b.snap.clients[i];
            needle.is_empty()
                || c.name.to_lowercase().contains(&needle)
                || c.ext_id.to_lowercase().contains(&needle)
        })
        .collect();
    ui.label(
        RichText::new(format!("{} clients", shown.len()))
            .small()
            .color(theme::INK_MUTED),
    );
    ui.add_space(4.0);
    let mut toggle: Option<Node> = None;
    // Right-click on a client, year or folder: add it to favorites.
    let fav_menu = |resp: &egui::Response, scope: Scope, out: &mut Option<Scope>| {
        resp.context_menu(|ui| {
            if ui.button("Add to favorites").clicked() {
                *out = Some(scope);
                ui.close();
            }
        });
    };
    let mut fav_scope: Option<Scope> = None;
    // A drop of dragged documents on a client or folder row: (ids, cabinet-relative folder).
    let mut dropped: Option<(Vec<String>, String)> = None;
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            if tree_row(
                ui,
                0,
                None,
                false,
                "All documents",
                b.snap.docs.len(),
                b.scope == Scope::Cabinet,
            )
            .1
            .clicked()
            {
                pick = Some(Scope::Cabinet);
            }
            for &ci in &shown {
                let c = &b.snap.clients[ci];
                let node = &b.ix.clients[ci];
                let open = b.expanded.contains(&Node::Client(ci));
                let label = format!("{}  {}", c.name, c.ext_id);
                let (chev, resp) = tree_row(
                    ui,
                    0,
                    Some(open),
                    false,
                    &label,
                    node.count,
                    b.scope == Scope::Client(ci),
                );
                if let Some(cf) = &c.folder_rel {
                    drop_target(&resp, cf, &mut dropped);
                }
                fav_menu(&resp, Scope::Client(ci), &mut fav_scope);
                if chev {
                    toggle = Some(Node::Client(ci));
                }
                if resp.clicked() {
                    pick = Some(Scope::Client(ci));
                    if !open {
                        toggle = Some(Node::Client(ci));
                    }
                }
                if !open {
                    continue;
                }
                if b.prefs.tree_by_year {
                    for y in &node.years {
                        let ynode = Node::Year(ci, y.year);
                        let yopen = b.expanded.contains(&ynode);
                        let here = Scope::Year {
                            client: ci,
                            year: y.year,
                        };
                        let (chev, resp) = tree_row(
                            ui,
                            1,
                            Some(yopen),
                            false,
                            &year_node_label(y.year),
                            y.count,
                            b.scope == here,
                        );
                        fav_menu(&resp, here.clone(), &mut fav_scope);
                        if chev {
                            toggle = Some(ynode.clone());
                        }
                        if resp.clicked() {
                            pick = Some(here);
                            if !yopen {
                                toggle = Some(ynode.clone());
                            }
                        }
                        if !yopen {
                            continue;
                        }
                        for (f, n) in &y.folders {
                            let here = Scope::YearFolder {
                                client: ci,
                                year: y.year,
                                folder: f.clone(),
                            };
                            let (_, resp) = tree_row(
                                ui,
                                2,
                                None,
                                false,
                                year_folder_label(f),
                                *n,
                                b.scope == here,
                            );
                            // A drop files into the real folder: the year's own when the
                            // template has one (`Income Tax/2025`), else the folder itself.
                            if let Some(cf) = &c.folder_rel {
                                let real = match y.year {
                                    Some(yr) if node.folders.contains_key(&format!("{f}/{yr}")) => {
                                        format!("{f}/{yr}")
                                    }
                                    _ => f.clone(),
                                };
                                if !real.is_empty() {
                                    drop_target(&resp, &format!("{cf}/{real}"), &mut dropped);
                                }
                            }
                            fav_menu(&resp, here.clone(), &mut fav_scope);
                            if resp.clicked() {
                                pick = Some(here);
                            }
                        }
                    }
                    continue;
                }
                // The client folder's folders, as on disk: depth-first, open ones expanded.
                let mut stack: Vec<(String, usize)> = node
                    .children("")
                    .into_iter()
                    .rev()
                    .map(|(f, _)| (f.to_owned(), 1))
                    .collect();
                while let Some((f, depth)) = stack.pop() {
                    let n = node.folders.get(&f).copied().unwrap_or(0);
                    let kids = node.has_children(&f);
                    let fnode = Node::Folder(ci, f.clone());
                    let fopen = kids && b.expanded.contains(&fnode);
                    let here = Scope::Folder {
                        client: ci,
                        folder: f.clone(),
                    };
                    let (chev, resp) = tree_row(
                        ui,
                        depth,
                        kids.then_some(fopen),
                        false,
                        vf_core::browse::leaf(&f),
                        n,
                        b.scope == here,
                    );
                    if let Some(cf) = &c.folder_rel {
                        drop_target(&resp, &format!("{cf}/{f}"), &mut dropped);
                    }
                    fav_menu(&resp, here.clone(), &mut fav_scope);
                    if chev {
                        toggle = Some(fnode.clone());
                    }
                    if resp.clicked() {
                        pick = Some(here);
                        if kids && !fopen {
                            toggle = Some(fnode);
                        }
                    }
                    if fopen {
                        stack.extend(
                            node.children(&f)
                                .into_iter()
                                .rev()
                                .map(|(k, _)| (k.to_owned(), depth + 1)),
                        );
                    }
                }
            }
            if b.ix.unassigned > 0
                && tree_row(
                    ui,
                    0,
                    None,
                    false,
                    "Not in a client folder",
                    b.ix.unassigned,
                    b.scope == Scope::Unassigned,
                )
                .1
                .clicked()
            {
                pick = Some(Scope::Unassigned);
            }
        });
    if let Some(n) = toggle
        && !b.expanded.remove(&n)
    {
        b.expanded.insert(n);
    }
    if let Some(f) = fav_scope.and_then(|s| b.as_favorite(&s))
        && !b.prefs.favorites.contains(&f)
    {
        b.prefs.favorites.push(f);
        b.prefs_changed();
    }
    if let Some(i) = fav_remove {
        b.prefs.favorites.remove(i);
        b.prefs_changed();
    }
    if let Some(s) = pick {
        b.set_scope(s);
    }
    if let Some((ids, dest)) = dropped {
        b.drop_request = Some((ids, dest));
    }
}

/// Makes a tree row a drop target for documents dragged from the grid: highlighted while they
/// hover, and the drop recorded.
fn drop_target(resp: &egui::Response, dest: &str, out: &mut Option<(Vec<String>, String)>) {
    if resp.dnd_hover_payload::<DocDrag>().is_some() {
        resp.ctx.debug_painter().rect_stroke(
            resp.rect,
            3.0,
            egui::Stroke::new(2.0, theme::ACCENT_DEFAULT),
            egui::StrokeKind::Inside,
        );
    }
    if let Some(p) = resp.dnd_release_payload::<DocDrag>() {
        *out = Some((p.0.clone(), dest.to_owned()));
    }
}

/// One tree row: indent, an expand chevron when `chevron` is `Some(open)`, the label, and the
/// count right-aligned (`warn` colors a non-zero count). Returns (chevron clicked, label).
fn tree_row(
    ui: &mut egui::Ui,
    depth: usize,
    chevron: Option<bool>,
    warn: bool,
    label: &str,
    count: usize,
    sel: bool,
) -> (bool, egui::Response) {
    ui.horizontal(|ui| {
        ui.add_space(depth as f32 * 16.0);
        let (rect, chev) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), Sense::click());
        if let Some(open) = chevron {
            // Painted, so it says what it is for screen readers (QA A15).
            let name = format!("{} {label}", if open { "Collapse" } else { "Expand" });
            chev.widget_info(|| {
                egui::WidgetInfo::labeled(egui::accesskit::Role::Button, true, &name)
            });
            let c = rect.center();
            let pts = if open {
                vec![
                    c + egui::vec2(-4.0, -2.0),
                    c + egui::vec2(4.0, -2.0),
                    c + egui::vec2(0.0, 3.0),
                ]
            } else {
                vec![
                    c + egui::vec2(-2.0, -4.0),
                    c + egui::vec2(3.0, 0.0),
                    c + egui::vec2(-2.0, 4.0),
                ]
            };
            ui.painter().add(egui::Shape::convex_polygon(
                pts,
                theme::INK_MUTED,
                egui::Stroke::NONE,
            ));
        }
        let resp = ui.selectable_label(sel, label);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let c = if warn && count > 0 {
                theme::WARN
            } else {
                theme::INK_MUTED
            };
            ui.label(RichText::new(count.to_string()).small().color(c));
        });
        (chevron.is_some() && chev.clicked(), resp)
    })
    .inner
}

/// `Sync to B2 ▾` (P11 step 10), scoped to the current client. Hidden without sync.
fn sync_menu(ui: &mut egui::Ui, b: &mut Browse) {
    let Some(v) = &b.sync else { return };
    let client = b.scope_client().map(|c| b.snap.clients[c].id.clone());
    let idle = v.running_for.is_none();
    let mut req = None;
    ui.separator();
    // `Sync to B2 ▾`: the fonts have no ▾, so it is painted (as the tree's chevrons are).
    let menu = ui.menu_button("Sync to B2    ", |ui| {
        if ui
            .add_enabled(
                idle && client.is_some(),
                egui::Button::new("Push this client now"),
            )
            .on_disabled_hover_text("Choose a client in the tree (or wait for the running sync).")
            .clicked()
        {
            req = client.clone().map(SyncRequest::Push);
            ui.close();
        }
        if ui
            .add_enabled(idle, egui::Button::new("Pull latest"))
            .clicked()
        {
            req = Some(SyncRequest::Pull);
            ui.close();
        }
        if ui
            .add_enabled(idle, egui::Button::new("Verify (sampled)"))
            .clicked()
        {
            req = Some(SyncRequest::Verify);
            ui.close();
        }
        if ui.button("Show in Sync tab").clicked() {
            req = Some(SyncRequest::Show(client.clone()));
            ui.close();
        }
    });
    // The spaces leave room for the painted ▾; the name read out has none (QA A15).
    menu.response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::accesskit::Role::Button, true, "Sync to B2")
    });
    let r = menu.response.rect;
    let c = egui::pos2(r.right() - 10.0, r.center().y);
    ui.painter().add(egui::Shape::convex_polygon(
        vec![
            c + egui::vec2(-4.0, -2.0),
            c + egui::vec2(4.0, -2.0),
            c + egui::vec2(0.0, 3.0),
        ],
        theme::INK_SECONDARY,
        egui::Stroke::NONE,
    ));
    if req.is_some() {
        b.sync_request = req;
    }
}

/// The current client's sync state beside the breadcrumb: `4 pending` / `in sync` /
/// `conflict` / `pushing` / `pulling`; a click opens the Sync tab on that client.
fn sync_pill(ui: &mut egui::Ui, b: &mut Browse) {
    let (Some(v), Some(c)) = (&b.sync, b.scope_client()) else {
        return;
    };
    let id = b.snap.clients[c].id.clone();
    let n = v.pending.get(&id).copied().unwrap_or(0);
    let rgb = |c: u32| egui::Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8);
    let (blue, amber, green) = (
        (rgb(0xE6EEF9), rgb(0x1F56A8)),
        (rgb(0xFBEFD5), rgb(0x7A4F07)),
        (rgb(0xDDF2E4), rgb(0x1D5C38)),
    );
    let (text, (bg, fg)) = match v.running_for.as_deref() {
        Some(r) if r == id => ("pushing".to_string(), blue),
        Some("") => ("pulling".to_string(), blue),
        _ if v.conflicts > 0 => ("conflict".to_string(), amber),
        _ if n > 0 => (format!("{n} pending"), amber),
        _ => ("in sync".to_string(), green),
    };
    let pill = crate::ui::widgets::pill(ui, &text, bg, fg)
        .on_hover_text("B2 sync of this client: click for the Sync tab")
        .interact(Sense::click());
    pill.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::accesskit::Role::Button,
            true,
            format!("B2 sync: {text}"),
        )
    });
    if pill.clicked() {
        b.sync_request = Some(SyncRequest::Show(Some(id)));
    }
}

/// Undoes this login's newest change (Ctrl+Z with the client panel showing).
fn undo_newest(b: &mut Browse, env: &mut Env) {
    let top = vf_fs::fileops::undo_stack(env.conn, env.device_id)
        .ok()
        .and_then(|s| s.into_iter().next());
    let Some(u) = top else { return };
    match vf_fs::fileops::undo(env.conn, env.root, &u.tx_id, env.device_id) {
        Ok(()) => {
            b.reload(env.conn);
            b.notice = Some(format!("Undid {}.", undo_label(&u)));
        }
        Err(e) => b.error = Some(format!("Undo failed: {e}")),
    }
    b.undo_stack = None;
}

fn breadcrumb(b: &Browse) -> String {
    let client = |c: usize| {
        let c = &b.snap.clients[c];
        c.folder_rel.clone().unwrap_or_else(|| c.name.clone())
    };
    match &b.scope {
        Scope::Cabinet => "Cabinet".into(),
        Scope::Unassigned => "Cabinet / Not in a client folder".into(),
        Scope::NeedsDocType => "Views / Needs doc type".into(),
        Scope::NeedsTaxYear => "Views / Needs tax year".into(),
        Scope::ReceivedSince(_) => "Views / Received this week".into(),
        Scope::Ids { list, .. } => format!("Views / {}", list.label()),
        Scope::Saved { name, .. } => format!("Saved searches / {name}"),
        Scope::Client(c) => format!("Cabinet / {}", client(*c)),
        Scope::Folder { client: c, folder } => {
            format!("Cabinet / {} / {}", client(*c), folder.replace('/', " / "))
        }
        Scope::Year { client: c, year } => {
            format!("Cabinet / {} / {}", client(*c), year_node_label(*year))
        }
        Scope::YearFolder {
            client: c,
            year,
            folder,
        } => format!(
            "Cabinet / {} / {} / {}",
            client(*c),
            year_node_label(*year),
            year_folder_label(folder).replace('/', " / ")
        ),
    }
}

fn grid(ui: &mut egui::Ui, b: &mut Browse, conn: &Connection) {
    if let Some(e) = &b.error {
        ui.label(RichText::new(e).color(crate::ui::wizard::ERROR));
    }
    let mut dismiss = false;
    if let Some(n) = &b.notice {
        egui::Frame::new()
            .fill(widgets::rgb(0xE6EEF9))
            .inner_margin(8)
            .corner_radius(4)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(n).color(theme::INK_SECONDARY));
                    ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                        dismiss = ui.button("Dismiss").clicked();
                    });
                });
            });
        ui.add_space(6.0);
    }
    if dismiss {
        b.notice = None;
    }
    ui.horizontal_wrapped(|ui| {
        if ui.add(widgets::primary_button("+ New client")).clicked() {
            b.new_client = Some(NewClientDialog::new(folderless_selected(b)));
        }
        if ui.button("Import clients CSV…").clicked() {
            let known = b
                .snap
                .clients
                .iter()
                .filter_map(|c| Some((c.ext_id.clone(), uuid::Uuid::parse_str(&c.id).ok()?)))
                .collect();
            b.import = Some(ImportDialog::new(known));
        }
        if ui
            .button("Print cover sheet")
            .on_hover_text("A barcode sheet that files the scanned document after it to a client.")
            .clicked()
        {
            let client = b.scope_client().map(|c| b.snap.clients[c].id.clone());
            let doc_type = b.root.as_deref().and_then(vf_fs::doctypes::cover_default);
            b.cover = Some(CoverSheetDialog::new(
                &b.snap,
                client.as_deref(),
                doc_type.as_deref(),
            ));
        }
        if ui
            .button("Scan")
            .on_hover_text(
                "Scan with this PC's scanner and profile (set on the Intake tab). The pages go to the Scans folder; a barcode cover sheet files them, otherwise they wait in Intake.",
            )
            .clicked()
        {
            b.scan_request = true;
        }
        sync_menu(ui, b);
        ui.separator();
        let rename = ui
            .add_enabled(!b.selected.is_empty(), egui::Button::new("Rename…"))
            .on_disabled_hover_text("Select the documents to rename first.");
        let pdfs = b
            .selected
            .iter()
            .filter(|&&i| b.snap.docs[i].rel_path.to_lowercase().ends_with(".pdf"))
            .count();
        if ui
            .add_enabled(
                pdfs >= 2 && pdfs == b.selected.len(),
                egui::Button::new("Combine"),
            )
            .on_hover_text(
                "One new PDF of the selected ones, in grid order, a bookmark per document.",
            )
            .on_disabled_hover_text("Select two or more PDFs.")
            .clicked()
        {
            b.combine_request = true;
        }
        if rename.clicked() {
            b.rename = Some(RenameDialog::new(b.selected_ids()));
        }
        let pinned: HashSet<&str> = b.prefs.pinned.iter().map(String::as_str).collect();
        let all_pinned = !b.selected.is_empty()
            && b.selected
                .iter()
                .all(|&i| pinned.contains(b.snap.docs[i].id.as_str()));
        let pin = ui
            .add_enabled(
                !b.selected.is_empty(),
                egui::Button::new(if all_pinned { "Unpin" } else { "Pin" }),
            )
            .on_hover_text("Pinned documents are listed under Views on this PC.");
        if pin.clicked() {
            let ids = b.selected_ids();
            b.prefs.toggle_pins(&ids);
            b.prefs_changed();
        }
        ui.separator();
        let target = b.target_doc();
        for (label, act) in [
            ("Open in Explorer", FileAct::Reveal),
            ("Copy path", FileAct::CopyPath),
        ] {
            let r = ui
                .add_enabled(target.is_some(), egui::Button::new(label))
                .on_disabled_hover_text("Select a document first.");
            if r.clicked()
                && let Some(di) = target
            {
                b.file_request = Some((di, act));
            }
        }
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(breadcrumb(b)).monospace());
        sync_pill(ui, b);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let r = ui.add(
                egui::TextEdit::singleline(&mut b.name_filter)
                    .hint_text("Filter this folder")
                    .desired_width(200.0),
            );
            if r.changed() {
                b.refresh();
            }
            if ui
                .button("Refresh")
                .on_hover_text("Re-read the catalog")
                .clicked()
            {
                b.reload(conn);
            }
            ui.label(
                RichText::new(format!(
                    "{} documents · {} selected",
                    b.visible.len(),
                    b.selected.len()
                ))
                .color(theme::INK_MUTED),
            );
        });
    });
    ui.add_space(6.0);
    if b.snap.docs.is_empty() {
        ui.label("Create your first client or run the import wizard.");
        return;
    }

    let mut clicked: Option<(usize, egui::Modifiers)> = None;
    let mut file_req: Option<(usize, FileAct)> = None;
    let mut trash_row: Option<usize> = None;
    let mut toggled: Option<usize> = None;
    let mut sort: Option<SortCol> = None;
    let mods = ui.input(|i| i.modifiers);
    // Cell text must not be selectable: a selectable label takes the click (to select text)
    // and the row never sees it, so double-click-to-open would not work on the file name.
    let selectable = ui.style().interaction.selectable_labels;
    ui.style_mut().interaction.selectable_labels = false;
    let mut table = TableBuilder::new(ui)
        .striped(false)
        .sense(Sense::click_and_drag())
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::exact(24.0))
        .column(
            Column::initial(320.0)
                .at_least(160.0)
                .clip(true)
                .resizable(true),
        )
        .column(Column::initial(120.0).clip(true).resizable(true))
        .column(Column::exact(80.0))
        .column(Column::exact(90.0))
        .column(Column::exact(72.0))
        .column(Column::exact(120.0))
        .column(Column::initial(90.0).clip(true))
        .column(Column::remainder().at_least(80.0));
    if let Some(r) = b.scroll_to.take() {
        table = table.scroll_to_row(r, None);
    }
    table
        .header(28.0, |mut h| {
            h.col(|_| {});
            for (col, name) in SortCol::ALL {
                h.col(|ui| {
                    let t = RichText::new(name.to_uppercase())
                        .small()
                        .color(theme::INK_MUTED);
                    if ui.add(egui::Button::new(t).frame(false)).clicked() {
                        sort = Some(col);
                    }
                    if b.sort.0 == col {
                        sort_arrow(ui, b.sort.1);
                    }
                });
            }
        })
        .body(|body| {
            body.rows(ROW_H, b.visible.len(), |mut row| {
                let pos = row.index();
                let di = b.visible[pos];
                let d = &b.snap.docs[di];
                let mut sel = b.selected.contains(&di);
                row.set_selected(sel);
                row.col(|ui| {
                    let r = ui.add(egui::Checkbox::without_text(&mut sel));
                    let now = sel;
                    r.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::accesskit::Role::CheckBox,
                            true,
                            now,
                            format!("Select {}", d.name()),
                        )
                    });
                    if r.changed() {
                        toggled = Some(pos);
                    }
                });
                row.col(|ui| {
                    let mut t = RichText::new(d.name()).monospace();
                    if d.doc_type.is_none() {
                        t = t.color(theme::WARN);
                    }
                    ui.label(t);
                    if let Some(u) = &d.locked_by {
                        widgets::lock_pill(ui, u);
                    }
                    if d.doc_type.is_none() {
                        widgets::pill(
                            ui,
                            "Needs doc type",
                            widgets::rgb(0xFBEFD5),
                            widgets::rgb(0x7A4F07),
                        );
                    }
                });
                row.col(|ui| {
                    ui.label(d.doc_type.as_deref().unwrap_or("—"));
                });
                row.col(|ui| {
                    ui.label(year_label(d.tax_year));
                });
                row.col(|ui| {
                    widgets::status_pill(ui, &d.status);
                });
                row.col(|ui| {
                    ui.label(RichText::new(widgets::size(d.size)).monospace());
                });
                row.col(|ui| {
                    ui.label(widgets::local_time(&d.updated_at));
                });
                row.col(|ui| {
                    ui.label(d.filed_by.as_deref().unwrap_or("—"));
                });
                row.col(|ui| {
                    ui.label(
                        RichText::new(&d.hash[..d.hash.len().min(8)])
                            .monospace()
                            .color(theme::INK_MUTED),
                    );
                });
                let resp = row.response();
                // Drag rows onto a folder in the tree to move them (PA-23): the selection when
                // this row is in it, otherwise this row.
                if resp.drag_started() {
                    let ids: Vec<String> = if sel {
                        b.visible
                            .iter()
                            .filter(|i| b.selected.contains(i))
                            .map(|&i| b.snap.docs[i].id.clone())
                            .collect()
                    } else {
                        vec![d.id.clone()]
                    };
                    egui::DragAndDrop::set_payload(&resp.ctx, DocDrag(ids));
                }
                if resp.double_clicked() {
                    file_req = Some((di, FileAct::Open));
                } else if resp.clicked() {
                    clicked = Some((pos, mods));
                }
                resp.context_menu(|ui| {
                    for (label, act) in [
                        ("Open", FileAct::Open),
                        ("Open in PDF app", FileAct::OpenExternal),
                        ("Open in Explorer", FileAct::Reveal),
                        ("Copy path", FileAct::CopyPath),
                    ] {
                        if ui.button(label).clicked() {
                            file_req = Some((di, act));
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.button("Move to trash").clicked() {
                        trash_row = Some(di);
                        ui.close();
                    }
                });
            });
        });
    ui.style_mut().interaction.selectable_labels = selectable;
    if let Some(c) = sort {
        b.sort_by(c);
    }
    if let Some(p) = toggled {
        b.toggle_row(p);
    } else if let Some((p, m)) = clicked {
        b.click_row(p, m);
    }
    if file_req.is_some() {
        b.file_request = file_req;
    }
    if let Some(di) = trash_row {
        if !b.selected.contains(&di) {
            b.selected = [di].into_iter().collect();
        }
        b.trash_requested = true;
    }
}

/// A painted ▲/▼ (the bundled fonts have no such glyph).
fn sort_arrow(ui: &mut egui::Ui, asc: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), Sense::hover());
    let c = rect.center();
    let (tip, base) = if asc { (-3.0, 3.0) } else { (3.0, -3.0) };
    ui.painter().add(egui::Shape::convex_polygon(
        vec![
            c + egui::vec2(0.0, tip),
            c + egui::vec2(4.0, base),
            c + egui::vec2(-4.0, base),
        ],
        theme::INK_MUTED,
        egui::Stroke::NONE,
    ));
}

fn inspector(ui: &mut egui::Ui, b: &mut Browse, env: &mut Env) {
    legal_holds(ui, b, env);
    if b.selected.is_empty()
        && let Some(c) = b.scope_client()
    {
        // Ctrl+Z is acted on now wherever the inspector is (never held for a later click).
        if std::mem::take(&mut b.undo_requested) {
            undo_newest(b, env);
        }
        client_panel::show(ui, b, env, c);
        return;
    }
    if b.meta_form.for_selection != b.selected {
        let docs = b.selected_docs();
        b.meta_form = MetaForm::for_docs(&b.selected, &docs, &b.snap.doc_types);
    }
    if b.undo_stack.is_none() {
        b.undo_stack =
            Some(vf_fs::fileops::undo_stack(env.conn, env.device_id).unwrap_or_default());
    }
    let top = b.undo_stack.as_ref().and_then(|s| s.first()).cloned();
    let mut undo = std::mem::take(&mut b.undo_requested);
    let n = b.selected.len();
    ui.label(
        RichText::new(format!(
            "{n} document{} selected",
            if n == 1 { "" } else { "s" }
        ))
        .strong()
        .size(14.0),
    );
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let stack = b.undo_stack.clone().unwrap_or_default();
            ui.add_enabled_ui(!stack.is_empty(), |ui| {
                ui.menu_button(format!("History ({})", stack.len()), |ui| {
                    ui.label(widgets::caption(
                        "Your last changes, newest first. Undo takes back the newest.",
                    ));
                    for (i, u) in stack.iter().enumerate() {
                        let text = format!("{} · {}", widgets::local_time(&u.ts), undo_label(u));
                        if i == 0 {
                            if ui.button(format!("Undo: {text}")).clicked() {
                                undo = true;
                                ui.close();
                            }
                        } else {
                            ui.label(RichText::new(text).color(theme::INK_MUTED));
                        }
                    }
                });
            });
            let hover = match &top {
                Some(u) => format!("Undo {} (Ctrl+Z)", undo_label(u)),
                None => "Nothing to undo".into(),
            };
            if ui
                .add_enabled(top.is_some(), egui::Button::new("Undo last"))
                .on_hover_text(hover)
                .clicked()
            {
                undo = true;
            }
        });
    });
    if undo && let Some(u) = top {
        match vf_fs::fileops::undo(env.conn, env.root, &u.tx_id, env.device_id) {
            Ok(()) => {
                b.reload(env.conn);
                b.notice = Some(format!("Undid {}.", undo_label(&u)));
            }
            Err(e) => b.error = Some(format!("Undo failed: {e}")),
        }
        b.meta_form.for_selection.clear();
        return;
    }
    ui.add_space(8.0);
    if b.selected.is_empty() {
        ui.label(RichText::new("Select documents to see their details.").color(theme::INK_MUTED));
        return;
    }
    meta_editor::fields(ui, &mut b.meta_form, &b.snap.doc_types);
    ui.add_space(10.0);
    let can_apply = !b.meta_form.to_edit().is_empty();
    let go = widgets::primary_button(format!("Apply to {} (one transaction)", b.selected.len()));
    let apply = ui.add_enabled(can_apply, go).clicked();
    if ui
        .button(format!("Move {} to trash", b.selected.len()))
        .on_hover_text("One transaction: Undo puts them back; Admin › Trash restores them later.")
        .clicked()
    {
        b.trash_requested = true;
    }
    // Two or more ask first (owner 2026-11-03); one goes straight (Undo brings it back).
    if b.trash_requested && b.selected.len() > 1 && b.trash_confirm != Some(b.selected.len()) {
        b.trash_requested = false;
        b.trash_asking = Some(b.selected.len());
    }
    if let Some(n) = b.trash_asking {
        let mut answer = None;
        egui::Modal::new(egui::Id::unique("trash-confirm")).show(ui.ctx(), |ui| {
            ui.set_width(380.0);
            ui.heading(format!("Move {n} documents to the trash?"));
            ui.label("Undo last puts them back; Admin › Trash restores them for 90 days.");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add(widgets::primary_button(format!("Yes, move {n} to trash")))
                    .clicked()
                {
                    answer = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    answer = Some(false);
                }
            });
        });
        match answer {
            Some(true) => {
                b.trash_asking = None;
                b.trash_confirm = Some(n);
                b.trash_requested = true;
            }
            Some(false) => b.trash_asking = None,
            None => {}
        }
    }
    if std::mem::take(&mut b.trash_requested) {
        b.trash_confirm = None;
        let ids: Vec<String> = b
            .selected
            .iter()
            .map(|&i| b.snap.docs[i].id.clone())
            .collect();
        match vf_fs::trash::delete(env.conn, env.root, env.device_id, &ids, "deleted") {
            Ok(_) => {
                b.reload(env.conn);
                b.selected.clear();
                b.notice = Some(format!(
                    "Moved {} documents to the trash (Undo puts them back).",
                    ids.len()
                ));
            }
            Err(e) => b.error = Some(e),
        }
        return;
    }
    if apply {
        let ids: Vec<String> = b
            .selected
            .iter()
            .map(|&i| b.snap.docs[i].id.clone())
            .collect();
        let edit = b.meta_form.to_edit();
        match vf_fs::docmeta::edit(env.conn, env.root, &ids, &edit, env.device_id) {
            Ok((_, n)) => {
                b.prefs.touch(&ids);
                b.prefs_dirty = true;
                b.reload(env.conn);
                b.notice = Some(format!("Updated {n} documents (one transaction)."));
                b.meta_form.for_selection.clear();
            }
            Err(e) => b.error = Some(e),
        }
        return;
    }
    let docs = b.selected_docs();
    if let [d] = docs.as_slice() {
        ui.add_space(12.0);
        ui.separator();
        ui.label(RichText::new(d.name()).monospace().strong());
        egui::Grid::new("inspector-detail")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                let kv = |ui: &mut egui::Ui, k: &str, v: RichText| {
                    ui.label(RichText::new(k).small().color(theme::INK_MUTED));
                    ui.add(egui::Label::new(v).wrap());
                    ui.end_row();
                };
                kv(ui, "SHA-256", RichText::new(&d.hash).monospace().small());
                kv(ui, "Versions", RichText::new(d.versions.to_string()));
                kv(
                    ui,
                    "Lock",
                    RichText::new(
                        d.locked_by
                            .clone()
                            .unwrap_or_else(|| "Not checked out".into()),
                    ),
                );
                kv(ui, "Size", RichText::new(widgets::size(d.size)));
                kv(
                    ui,
                    "Modified",
                    RichText::new(widgets::local_time(&d.updated_at)),
                );
                kv(ui, "Path", RichText::new(&d.rel_path).monospace().small());
                // P08: what the form recognizer read (TINs last 4 only).
                let party = |n: Option<String>, t: Option<String>| -> String {
                    match (n, t) {
                        (Some(n), Some(t)) => format!("{n} (…{t})"),
                        (Some(n), None) => n,
                        (None, Some(t)) => format!("…{t}"),
                        (None, None) => "—".into(),
                    }
                };
                let read: Option<String> = env
                    .conn
                    .query_row(
                        "SELECT form, tax_year, payer_name, payer_tin4, recipient_name,
                                recipient_tin4, confidence
                         FROM document_extract WHERE doc_id = ?1",
                        [&d.id],
                        |r| {
                            let year: Option<i64> = r.get(1)?;
                            let conf: f64 = r.get(6)?;
                            Ok(format!(
                                "{}{} · from {} · for {} · {:.0}% sure",
                                r.get::<_, String>(0)?.to_uppercase(),
                                year.map(|y| format!(" {y}")).unwrap_or_default(),
                                party(r.get(2)?, r.get(3)?),
                                party(r.get(4)?, r.get(5)?),
                                conf * 100.0
                            ))
                        },
                    )
                    .ok();
                if let Some(line) = read {
                    kv(ui, "Recognized", RichText::new(line));
                }
            });
        let (id, name) = (d.id.clone(), d.name().to_string());
        versions_list(ui, b, env, &id, &name);
    }
}

/// P07 step 4: the legal holds covering the client in view or the selected documents, as a
/// banner with Release, and "Place legal hold…" for them.
fn legal_holds(ui: &mut egui::Ui, b: &mut Browse, env: &mut Env) {
    let client = if b.selected.is_empty() {
        b.scope_client().map(|c| b.snap.clients[c].id.clone())
    } else {
        None
    };
    let docs: Vec<String> = b
        .selected
        .iter()
        .map(|&i| b.snap.docs[i].id.clone())
        .collect();
    if client.is_none() && docs.is_empty() {
        return;
    }
    let key = format!("{client:?}{docs:?}");
    if b.holds_seen.as_ref().is_none_or(|(k, _)| *k != key) {
        let found = vf_fs::holds::active_for(env.root, env.conn, client.as_deref(), &docs);
        b.holds_seen = Some((key, found));
    }
    let holds = b
        .holds_seen
        .as_ref()
        .map(|(_, h)| h.clone())
        .unwrap_or_default();
    let mut release = None;
    for h in &holds {
        egui::Frame::new()
            .fill(egui::Color32::from_rgb(0xfd, 0xec, 0xea))
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "Legal hold: {} (since {})",
                            h.reason,
                            widgets::local_time(&h.since)
                        ))
                        .color(crate::ui::wizard::ERROR)
                        .strong(),
                    );
                    ui.label(widgets::caption(
                        "Nothing here is disposed of or emptied from the trash while it holds.",
                    ));
                    if ui.small_button("Release hold").clicked() {
                        b.hold_releasing = Some((h.id.clone(), h.reason.clone()));
                        b.hold_pin.clear();
                    }
                });
            });
        ui.add_space(4.0);
    }
    if let Some((id, reason)) = b.hold_releasing.clone() {
        let mut pin = std::mem::take(&mut b.hold_pin);
        match crate::ui::disposition::confirm_release(ui, env, &reason, &mut pin) {
            Some(true) => {
                b.hold_releasing = None;
                release = Some(id);
            }
            Some(false) => b.hold_releasing = None,
            None => {}
        }
        b.hold_pin = pin;
    }
    if let Some(id) = release {
        match vf_fs::holds::release(env.conn, env.root, env.device_id, &id) {
            Ok(()) => b.notice = Some("Legal hold released.".into()),
            Err(e) => b.error = Some(e),
        }
        b.holds_seen = None;
    }
    let what = match (&client, docs.len()) {
        (Some(_), _) => "this client".to_string(),
        (None, 1) => "this document".to_string(),
        (None, n) => format!("these {n} documents"),
    };
    match b.hold_reason.as_mut() {
        None => {
            if ui
                .small_button(format!("Place legal hold on {what}…"))
                .clicked()
            {
                b.hold_reason = Some(String::new());
            }
        }
        Some(reason) => {
            let mut place = false;
            let mut cancel = false;
            ui.horizontal(|ui| {
                let l = ui.label("Hold reason");
                ui.add(
                    egui::TextEdit::singleline(reason)
                        .hint_text("matter, examination, request")
                        .desired_width(200.0),
                )
                .labelled_by(l.id);
            });
            ui.horizontal(|ui| {
                place = ui.button("Place hold").clicked();
                cancel = ui.button("Cancel").clicked();
            });
            if place {
                let target = match &client {
                    Some(c) => vf_fs::holds::Target::Client(c.clone()),
                    None => vf_fs::holds::Target::Docs(docs.clone()),
                };
                let reason = reason.clone();
                match vf_fs::holds::place(env.conn, env.root, env.device_id, target, &reason) {
                    Ok(_) => {
                        b.notice = Some(format!("Legal hold placed on {what}."));
                        b.hold_reason = None;
                        b.holds_seen = None;
                    }
                    Err(e) => b.error = Some(e),
                }
            } else if cancel {
                b.hold_reason = None;
            }
        }
    }
    ui.add_space(6.0);
}

/// Leaf folders under `dir` (relative, `/`-separated), skipping `_Vibe` and hidden or
/// temporary ones, at most 4 levels deep.
fn leaf_dirs(dir: &Path, prefix: &str, depth: usize, out: &mut Vec<String>) {
    if depth > 4 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut any = false;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name.starts_with('~') || name == vf_fs::cabinet::VIBE_DIR {
            continue;
        }
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            any = true;
            let rel = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            leaf_dirs(&e.path(), &rel, depth + 1, out);
        }
    }
    if !any && !prefix.is_empty() {
        out.push(prefix.to_string());
    }
}

/// An undo stack entry in words.
fn undo_label(u: &vf_core::tx::Undoable) -> String {
    let what = match u.label.as_str() {
        "doc_meta_edit" => "metadata edit",
        "doc_rename" => "rename",
        "doc_move" => "move",
        "trash" | "trash_duplicate" => "move to trash",
        "trash_restore" => "restore from trash",
        "restore_version" => "version restore",
        "annotate" => "annotations",
        "stamp" => "stamp",
        "pages" => "page edit",
        "redact" => "redaction",
        "flatten" => "flatten",
        "pdfa" => "PDF/A conversion",
        "compress" => "compression",
        "client_create" => "new client",
        other => other,
    };
    match u.summary.split_once(" · ") {
        Some((_, docs)) => format!("{what} ({docs})"),
        None => what.to_string(),
    }
}

/// What a click in the Versions list asks for.
enum VersionAct {
    Open(String),
    Restore(String),
    SaveCopy(String),
}

/// P05 step 1: the document's earlier contents, newest first, with Open, Restore and Save a
/// copy.
fn versions_list(ui: &mut egui::Ui, b: &mut Browse, env: &mut Env, doc_id: &str, name: &str) {
    if b.versions.as_ref().is_none_or(|(id, _)| id != doc_id) {
        match vf_fs::versions::list(env.conn, env.root, doc_id) {
            Ok(v) => b.versions = Some((doc_id.to_string(), v)),
            Err(e) => {
                b.error = Some(e);
                b.versions = Some((doc_id.to_string(), vec![]));
            }
        }
    }
    let Some((_, versions)) = &b.versions else {
        return;
    };
    ui.add_space(10.0);
    ui.label(RichText::new("Earlier versions").strong());
    if versions.is_empty() {
        ui.label(widgets::caption(
            "None yet. Every change of the content keeps the one before it here.",
        ));
        return;
    }
    let mut act = None;
    for (i, v) in versions.iter().enumerate() {
        ui.push_id(("version", i), |ui| {
            ui.label(format!(
                "{} · {} · {}",
                widgets::local_time(&v.created),
                v.reason.as_deref().unwrap_or("earlier content"),
                widgets::size(v.size)
            ));
            ui.horizontal(|ui| {
                if !v.kept {
                    ui.label(widgets::caption("content not kept")).on_hover_text(
                        "Changed outside Vibe Files: the content before that change was never seen.",
                    );
                    return;
                }
                if ui
                    .small_button("Open")
                    .on_hover_text("Open a read-only copy")
                    .clicked()
                {
                    act = Some(VersionAct::Open(v.hash.clone()));
                }
                if ui
                    .small_button("Restore")
                    .on_hover_text(
                        "Make this the current content again (the current one is kept as a version)",
                    )
                    .clicked()
                {
                    act = Some(VersionAct::Restore(v.hash.clone()));
                }
                if ui.small_button("Save a copy…").clicked() {
                    act = Some(VersionAct::SaveCopy(v.hash.clone()));
                }
            });
        });
        ui.add_space(4.0);
    }
    match act {
        Some(VersionAct::Open(hash)) => {
            let dir = std::env::temp_dir().join("Vibe Files versions");
            let r = vf_fs::versions::open_copy(env.root, &hash, name, &dir)
                .and_then(|p| (b.shell)(&ShellAction::Open(p)));
            if let Err(e) = r {
                b.error = Some(e);
            }
        }
        Some(VersionAct::Restore(hash)) => {
            match vf_fs::versions::restore(env.conn, env.root, env.device_id, doc_id, &hash) {
                Ok(Ok(_)) => {
                    b.reload(env.conn);
                    b.notice = Some(
                        "Restored the earlier version; the content it replaced is kept as a version."
                            .into(),
                    );
                }
                Ok(Err(why)) => b.error = Some(format!("Not restored: {why}.")),
                Err(e) => b.error = Some(e),
            }
            b.versions = None;
        }
        Some(VersionAct::SaveCopy(hash)) => {
            if let Some(dest) = rfd::FileDialog::new().set_file_name(name).save_file() {
                match vf_fs::versions::save_copy(env.root, &hash, &dest) {
                    Ok(()) => b.notice = Some(format!("Saved {}", dest.display())),
                    Err(e) => b.error = Some(e),
                }
            }
        }
        None => {}
    }
}

/// While documents are dragged: "Move N documents" beside the pointer.
fn drag_label(ctx: &egui::Context) {
    let Some(p) = egui::DragAndDrop::payload::<DocDrag>(ctx) else {
        return;
    };
    let Some(pos) = ctx.pointer_latest_pos() else {
        return;
    };
    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
    let n = p.0.len();
    egui::Area::new(egui::Id::unique("browse-drag-label"))
        .order(egui::Order::Tooltip)
        .fixed_pos(pos + egui::vec2(14.0, 10.0))
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.label(if n == 1 {
                    "Move 1 document to a folder".to_string()
                } else {
                    format!("Move {n} documents to a folder")
                });
            });
        });
}

/// "Move N documents to …?" after a drop on a tree folder; Move commits in one Transaction.
fn move_dialog(ctx: &egui::Context, b: &mut Browse, env: &mut Env) {
    let Some(ask) = &b.move_ask else { return };
    use vf_fs::rename::Outcome;
    let moving = ask.rows.iter().filter(|r| r.renames()).count();
    let kept: Vec<&str> = ask
        .rows
        .iter()
        .filter_map(|r| match &r.outcome {
            Outcome::KeepBoth(p) => Some(p.rsplit('/').next().unwrap_or(p)),
            _ => None,
        })
        .collect();
    let skipped: Vec<String> = ask
        .rows
        .iter()
        .filter_map(|r| match &r.outcome {
            Outcome::Skip(w) => Some(format!(
                "{}: {w}",
                r.old_rel.rsplit('/').next().unwrap_or("")
            )),
            _ => None,
        })
        .collect();
    let mut answer = None;
    egui::Modal::new(egui::Id::unique("move-confirm")).show(ctx, |ui| {
        ui.set_width(460.0);
        ui.heading(if moving == 1 {
            "Move 1 document?".to_string()
        } else {
            format!("Move {moving} documents?")
        });
        ui.label(format!("To {}", ask.dest.replace('/', " / ")));
        if !kept.is_empty() {
            ui.label(format!(
                "A file with the same name is there already, so these keep both: {}",
                kept.join(", ")
            ));
        }
        for w in skipped.iter().take(5) {
            ui.label(RichText::new(format!("Not moved: {w}")).color(theme::WARN));
        }
        ui.label(widgets::caption("Undo last (Ctrl+Z) puts them back."));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.add(widgets::primary_button("Move")).clicked() {
                answer = Some(true);
            }
            if ui.button("Cancel").clicked() {
                answer = Some(false);
            }
        });
    });
    match answer {
        Some(true) => {
            let ask = b.move_ask.take().expect("checked");
            match vf_fs::moveto::commit(env.conn, env.root, &ask.rows, env.device_id) {
                Ok((_, n)) => {
                    let ids: Vec<String> = ask.rows.iter().map(|r| r.doc_id.clone()).collect();
                    b.prefs.touch(&ids);
                    b.prefs_dirty = true;
                    b.reload(env.conn);
                    b.notice = Some(format!(
                        "Moved {n} document{} to {}.",
                        if n == 1 { "" } else { "s" },
                        ask.dest.replace('/', " / ")
                    ));
                }
                Err(e) => b.error = Some(format!("Not moved: {e}")),
            }
        }
        Some(false) => b.move_ask = None,
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use crate::ui::test_util::{click, type_into};
    use std::time::{Duration, Instant};

    use super::*;
    use egui::accesskit::Role;
    use egui_kittest::kittest::Queryable;
    use egui_kittest::{Harness, HarnessBuilder};
    use vf_core::db::Storage;

    /// A catalog with `clients` clients x `per` documents spread over 2023-2025 and Permanent,
    /// in the Filer template folders. Rows only (no files): Browse reads the catalog.
    fn seed(clients: usize, per: usize) -> (tempfile::TempDir, Connection) {
        let tmp = tempfile::tempdir().unwrap();
        let mut conn = vf_core::db::open(&tmp.path().join("c.db"), Storage::Local).unwrap();
        let tx = conn.transaction().unwrap();
        for c in 0..clients {
            let (id, ext) = (format!("c{c:04}"), format!("{}", 1000 + c));
            let name = format!("Client {c:04}");
            let folder = format!("{name} ({ext})");
            tx.execute(
                "INSERT INTO client (id, ext_id, name, folder_rel, device_id, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 'd', '2026-11-02T00:00:00Z')",
                (&id, &ext, &name, &folder),
            )
            .unwrap();
            for n in 0..per {
                let (year, dir) = match n % 4 {
                    0 => (None, "Permanent/Setup".to_string()),
                    1 => (Some(2025), "Income Tax/2025".to_string()),
                    2 => (Some(2025), "Payroll/2025".to_string()),
                    _ => {
                        let y = 2023 + (n / 4 % 2) as i32;
                        (Some(y), format!("Income Tax/{y}"))
                    }
                };
                tx.execute(
                    "INSERT INTO document (id, client_id, tax_year, rel_path, hash, size, filed_at,
                                           device_id, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, '2026-11-02T00:00:00Z', 'd',
                             '2026-11-02T00:00:00Z')",
                    (
                        format!("{id}-{n}"),
                        &id,
                        year,
                        format!("{folder}/{dir}/doc {n:03}.pdf"),
                        format!("{:064x}", c * 1000 + n),
                        (n as i64 + 1) * 1000,
                    ),
                )
                .unwrap();
            }
        }
        tx.commit().unwrap();
        (tmp, conn)
    }

    struct State {
        b: Browse,
        conn: Connection,
        root: std::path::PathBuf,
        _tmp: tempfile::TempDir,
    }

    fn harness(clients: usize, per: usize) -> Harness<'static, State> {
        crate::ui::test_util::pdfium_first();
        let (tmp, conn) = seed(clients, per);
        let b = Browse::load(&conn);
        let mut h = HarnessBuilder::default()
            .with_size([1440.0, 900.0])
            // Real frame times: double-clicks need two clicks within 0.3 s of simulated time.
            .with_step_dt(1.0 / 60.0)
            .build_ui_state(
                |ui, s: &mut State| {
                    let mut env = Env {
                        conn: &mut s.conn,
                        root: &s.root,
                        device_id: "dev-test",
                    };
                    show(ui, &mut s.b, &mut env)
                },
                State {
                    b,
                    conn,
                    root: tmp.path().join("cabinet"),
                    _tmp: tmp,
                },
            );
        // The app's fonts, so a glyph they lack fails here as it would in the app.
        crate::ui::theme::install(&h.ctx);
        h.run();
        h
    }

    /// P03 step 6: a saved search is a virtual folder in the tree, shared through the cabinet.
    #[test]
    fn saved_search_is_a_virtual_folder() {
        use vf_index::search::{Query, Year};
        let mut h = harness(3, 8);
        let root = h.state().root.clone();
        std::fs::create_dir_all(&root).unwrap();
        vf_index::saved::add(
            &root,
            "Everything 2025",
            &Query {
                year: Some(Year::Year(2025)),
                ..Query::default()
            },
            "dev-test",
        )
        .unwrap();
        h.state_mut().b.saved = vf_index::saved::load(&root);
        h.run();
        click(&mut h, "Everything 2025");
        h.run();
        let b = &h.state().b;
        assert!(matches!(&b.scope, Scope::Saved { name, .. } if name == "Everything 2025"));
        assert_eq!(b.visible.len(), 12, "half of 3 × 8 are 2025");
        assert!(
            b.visible
                .iter()
                .all(|&i| b.snap.docs[i].tax_year == Some(2025))
        );
        assert_eq!(breadcrumb(b), "Saved searches / Everything 2025");
    }

    /// A client with nothing filed yet shows its folders as they are on disk (PA-23).
    #[test]
    fn tree_shows_empty_template_folders() {
        let mut h = harness(1, 0);
        {
            let s = h.state_mut();
            let folder = s.root.join("Client 0000 (1000)");
            for d in [
                "Income Tax/2025",
                "Bookkeeping",
                "Financials",
                "Permanent/Setup",
                "_Vibe",
            ] {
                std::fs::create_dir_all(folder.join(d)).unwrap();
            }
            let root = s.root.clone();
            s.b.set_root(&root);
        }
        h.run();
        click(&mut h, "Client 0000  1000");
        for f in ["Income Tax", "Bookkeeping", "Financials", "Permanent"] {
            assert!(h.query_by_label(f).is_some(), "{f}");
        }
        assert!(h.query_by_label("_Vibe").is_none());
        assert!(h.query_by_label("2025").is_none(), "Income Tax is closed");
        click(&mut h, "Income Tax");
        assert!(h.query_by_label("2025").is_some());
        click(&mut h, "Permanent");
        assert!(h.query_by_label("Setup").is_some());
        click(&mut h, "Setup");
        assert_eq!(
            breadcrumb(&h.state().b),
            "Cabinet / Client 0000 (1000) / Permanent / Setup"
        );
    }

    #[test]
    fn tree_drills_from_client_to_folder() {
        let mut h = harness(3, 8);
        assert_eq!(h.state().b.visible.len(), 24, "starts on the whole cabinet");
        click(&mut h, "Client 0001  1001");
        assert_eq!(h.state().b.scope, Scope::Client(1));
        assert_eq!(h.state().b.visible.len(), 8);
        // A folder shows what is in it and below it.
        click(&mut h, "Income Tax");
        assert_eq!(h.state().b.visible.len(), 4);
        click(&mut h, "2025");
        assert_eq!(
            h.state().b.scope,
            Scope::Folder {
                client: 1,
                folder: "Income Tax/2025".into()
            }
        );
        assert_eq!(h.state().b.visible.len(), 2);
        assert_eq!(
            breadcrumb(&h.state().b),
            "Cabinet / Client 0001 (1001) / Income Tax / 2025"
        );
        click(&mut h, "Payroll");
        assert_eq!(h.state().b.visible.len(), 2);
        click(&mut h, "Permanent");
        assert_eq!(h.state().b.visible.len(), 2);
    }

    /// The tree toggles between the folders on disk and by tax year (owner 2026-11-04).
    #[test]
    fn tree_toggles_to_the_year_view() {
        let mut h = harness(2, 8);
        click(&mut h, "By year");
        assert!(h.state().b.prefs.tree_by_year);
        click(&mut h, "Client 0001  1001");
        // Years newest first, "No tax year" for the Permanent/Setup documents.
        assert!(h.query_by_label("No tax year").is_some());
        click(&mut h, "2025");
        assert_eq!(
            h.state().b.scope,
            Scope::Year {
                client: 1,
                year: Some(2025)
            }
        );
        assert_eq!(h.state().b.visible.len(), 4);
        click(&mut h, "Payroll");
        assert_eq!(h.state().b.visible.len(), 2);
        assert_eq!(
            breadcrumb(&h.state().b),
            "Cabinet / Client 0001 (1001) / 2025 / Payroll"
        );
        click(&mut h, "Folders");
        assert!(!h.state().b.prefs.tree_by_year);
        assert!(h.query_by_label("Income Tax").is_some());
    }

    /// Browse has a Scan button (owner 2026-11-04); the app starts it with Intake's scanner.
    #[test]
    fn scan_button_asks_the_app_to_scan() {
        let mut h = harness(1, 0);
        click(&mut h, "Scan");
        assert!(h.state().b.scan_request);
    }

    /// Documents dropped on a tree folder are moved there after "Move" (PA-23).
    #[test]
    fn dropping_documents_on_a_folder_moves_them() {
        let mut h = harness(1, 2);
        let (ids, dest) = {
            let s = h.state_mut();
            let folder = s.root.join("Client 0000 (1000)");
            std::fs::create_dir_all(folder.join("Financials")).unwrap();
            for d in &s.b.snap.docs {
                let p = s.root.join(&d.rel_path);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, &d.id).unwrap();
            }
            let ids: Vec<String> = s.b.snap.docs.iter().map(|d| d.id.clone()).collect();
            (ids, "Client 0000 (1000)/Financials".to_string())
        };
        h.state_mut().b.drop_request = Some((ids, dest));
        h.run();
        assert!(h.query_by_label("Move 2 documents?").is_some());
        click(&mut h, "Move");
        let s = h.state();
        assert!(s.b.move_ask.is_none());
        assert!(
            s.b.snap
                .docs
                .iter()
                .all(|d| d.rel_path.starts_with("Client 0000 (1000)/Financials/")),
            "{:?}",
            s.b.error
        );
        assert!(
            s.root
                .join("Client 0000 (1000)/Financials/doc 000.pdf")
                .is_file()
        );
    }

    /// A file copied into a client folder through Explorer shows without the weekly sweep
    /// (owner report 2026-11-04).
    #[test]
    fn files_copied_into_a_client_folder_are_added() {
        let mut h = harness(1, 0);
        {
            let s = h.state_mut();
            let fin = s.root.join("Client 0000 (1000)/Financials");
            std::fs::create_dir_all(&fin).unwrap();
            std::fs::write(fin.join("invented balance sheet.pdf"), "invented").unwrap();
            let root = s.root.clone();
            s.b.set_root(&root);
        }
        h.run();
        h.run();
        let s = h.state();
        let names: Vec<&str> = s.b.snap.docs.iter().map(|d| d.name()).collect();
        assert_eq!(names, ["invented balance sheet.pdf"]);
        assert!(
            s.b.notice
                .as_deref()
                .unwrap_or("")
                .contains("Added 1 new file")
        );
    }

    #[test]
    fn client_filter_and_name_filter() {
        let mut h = harness(12, 4);
        h.state_mut().b.client_filter = "1011".into();
        h.run();
        assert!(h.query_by_label("Client 0011  1011").is_some());
        assert!(h.query_by_label("Client 0001  1001").is_none());
        let b = &mut h.state_mut().b;
        b.name_filter = "doc 002".into();
        b.refresh();
        assert_eq!(b.visible.len(), 12);
    }

    #[test]
    fn click_ctrl_shift_selection_and_keys() {
        let mut h = harness(1, 10);
        let b = &mut h.state_mut().b;
        let none = egui::Modifiers::NONE;
        b.click_row(2, none);
        assert_eq!(b.selected.len(), 1);
        b.click_row(
            5,
            egui::Modifiers {
                shift: true,
                ..none
            },
        );
        assert_eq!(b.selected.len(), 4, "rows 2..=5");
        let ctrl = egui::Modifiers {
            command: true,
            ..none
        };
        b.click_row(8, ctrl);
        assert_eq!(b.selected.len(), 5);
        b.click_row(8, ctrl);
        assert_eq!(b.selected.len(), 4, "ctrl-click toggles off");

        b.click_row(0, none);
        h.key_press(Key::ArrowDown);
        h.run();
        h.key_press_modifiers(egui::Modifiers::SHIFT, Key::ArrowDown);
        h.run();
        let b = &h.state().b;
        assert_eq!(b.cursor, Some(2));
        assert_eq!(b.selected.len(), 2, "rows 1..=2 after shift+down");
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
        h.run();
        assert_eq!(h.state().b.selected.len(), 10);
        h.key_press(Key::Escape);
        h.run();
        assert!(h.state().b.selected.is_empty());
    }

    #[test]
    fn header_click_sorts_and_reverses() {
        let mut h = harness(1, 6);
        let sizes = |h: &Harness<'static, State>| -> Vec<i64> {
            let b = &h.state().b;
            b.visible.iter().map(|&i| b.snap.docs[i].size).collect()
        };
        click(&mut h, "SIZE");
        assert!(sizes(&h).is_sorted());
        click(&mut h, "SIZE");
        assert!(sizes(&h).iter().rev().is_sorted());
    }

    #[test]
    fn inspector_summarizes_mixed_values() {
        let mut h = harness(1, 4);
        let b = &mut h.state_mut().b;
        b.selected = b.visible.iter().copied().collect();
        h.run();
        assert!(h.query_by_label("4 documents selected").is_some());
        // Permanent, 2025, 2023.
        let year = h.get_by_role_and_label(Role::ComboBox, "TAX YEAR");
        assert!(
            year.value().as_deref() == Some("Mixed (3)") || h.query_by_label("Mixed (3)").is_some(),
            "tax year shows Mixed (3): {:?}",
            year.value()
        );
    }

    /// QA A1: a shift-click after the grid shrank (anchor past its end) does not panic.
    #[test]
    fn shift_click_after_the_grid_shrinks() {
        let mut h = harness(1, 6);
        let b = &mut h.state_mut().b;
        assert_eq!(b.visible.len(), 6);
        b.click_row(5, egui::Modifiers::NONE); // anchor 5
        b.click_row(1, egui::Modifiers::SHIFT); // cursor 1
        b.cursor = Some(0);
        // The grid shrinks to one document (a name filter): the anchor no longer fits.
        b.name_filter = "doc 000".into();
        b.refresh();
        assert!(b.visible.len() < 6, "{}", b.visible.len());
        b.click_row(0, egui::Modifiers::SHIFT);
        assert!(!b.selected.is_empty());
    }

    #[test]
    fn reload_keeps_scope_and_selection_by_id() {
        let mut h = harness(2, 4);
        let s = h.state_mut();
        s.b.set_scope(Scope::Client(1));
        let pick = s.b.visible[1];
        let id = s.b.snap.docs[pick].id.clone();
        s.b.selected.insert(pick);
        s.conn
            .execute(
                "UPDATE document SET deleted_at = 't' WHERE id = 'c0000-0'",
                [],
            )
            .unwrap();
        s.b.reload(&s.conn);
        assert_eq!(s.b.scope, Scope::Client(1));
        let sel: Vec<_> =
            s.b.selected
                .iter()
                .map(|&i| s.b.snap.docs[i].id.clone())
                .collect();
        assert_eq!(sel, [id]);
        assert_eq!(s.b.snap.docs.len(), 7);
    }

    /// P01 step 3: `+ New client` previews the template folders, then creates them.
    #[test]
    fn new_client_dialog_previews_and_creates() {
        let mut h = harness(0, 0);
        let root = h.state().root.clone();
        vf_fs::cabinet::create(
            &root,
            &vf_fs::cabinet::CabinetFile {
                cabinet_id: vf_core::new_id(),
                firm_id: vf_core::new_id(),
                schema_version: vf_core::db::SCHEMA_VERSION,
                layout_version: 1,
                created: vf_core::now_rfc3339(),
                name: None,
            },
        )
        .unwrap();
        click(&mut h, "+ New client");
        type_into(&mut h, "Client ID", "1042");
        type_into(&mut h, "Client name", "Anderson Farms LLC");
        h.run();
        assert!(
            h.query_by_label("Anderson Farms LLC (1042)/Permanent/Setup")
                .is_some(),
            "preview lists the template folders"
        );
        assert!(
            !root.join("Anderson Farms LLC (1042)").exists(),
            "nothing yet"
        );
        // The dialog holds still: the button is where it was a frame ago.
        let at = |h: &Harness<'static, State>| {
            h.get_by_role_and_label(Role::Button, "Create client (one transaction)")
                .rect()
        };
        let before = at(&h);
        h.step();
        assert_eq!(at(&h), before, "dialog moved between frames");
        click(&mut h, "Create client (one transaction)");

        let b = &h.state().b;
        assert!(b.new_client.is_none(), "dialog closed");
        assert_eq!(b.snap.clients.len(), 1);
        assert_eq!(b.scope, Scope::Client(0));
        assert_eq!(
            b.notice.as_deref(),
            Some("Created Anderson Farms LLC (1042).")
        );
        assert!(
            root.join("Anderson Farms LLC (1042)/_Vibe/client.json")
                .is_file()
        );
    }

    /// An empty catalog over a real (empty) cabinet folder.
    fn cabinet_harness() -> Harness<'static, State> {
        let h = harness(0, 0);
        vf_fs::cabinet::create(
            &h.state().root,
            &vf_fs::cabinet::CabinetFile {
                cabinet_id: vf_core::new_id(),
                firm_id: vf_core::new_id(),
                schema_version: vf_core::db::SCHEMA_VERSION,
                layout_version: 1,
                created: vf_core::now_rfc3339(),
                name: None,
            },
        )
        .unwrap();
        h
    }

    fn write_csv(h: &Harness<'static, State>, text: &str) -> String {
        let p = h.state().root.parent().unwrap().join("clients.csv");
        std::fs::write(&p, text).unwrap();
        p.display().to_string()
    }

    /// P01 step 3: the CSV import keeps the master list, keeps only TIN last-4s, reports matching
    /// folders that have no sentinel.
    #[test]
    fn import_clients_csv() {
        let mut h = cabinet_harness();
        std::fs::create_dir(h.state().root.join("Acme LLC (1001)")).unwrap();
        let csv = write_csv(
            &h,
            "ClientID,Name,Partner,SSN\n1001,Acme LLC,KW,000-00-0001\n1002,Baker Trust,AB,000-00-0002\n",
        );
        click(&mut h, "Import clients CSV…");
        type_into(&mut h, "CSV file", &csv);
        click(&mut h, "Read");
        let d = h.state().b.import.as_ref().unwrap();
        assert_eq!(d.map.id.as_deref(), Some("ClientID"), "guessed");
        assert_eq!(d.map.name.as_deref(), Some("Name"));
        let by_label = |h: &mut Harness<'static, State>, l: &str| {
            h.get_by_label(l).click();
            h.run();
        };
        by_label(&mut h, "Partner");
        // A TIN column is never an extra; it can only keep its last 4 digits (P08).
        by_label(&mut h, "SSN: keep the last 4 digits only");
        let d = h.state().b.import.as_ref().unwrap();
        assert_eq!(d.map.extras, ["Partner"]);
        assert_eq!(d.tin.as_deref(), Some("SSN"));
        click(&mut h, "Import 2 rows (one transaction)");

        let b = &h.state().b;
        assert!(b.import.is_none(), "dialog closed");
        let n = b.notice.as_deref().unwrap();
        assert!(
            n.contains("Imported 2 clients (2 without a folder yet)"),
            "{n}"
        );
        assert!(n.contains("Acme LLC (1001) (ID 1001)"), "{n}");
        assert_eq!(b.snap.clients.len(), 2);
        let raw = std::fs::read_to_string(h.state().root.join("_Vibe/clients.json")).unwrap();
        assert!(
            raw.contains("Partner") && !raw.contains("000-00-0001"),
            "no TINs"
        );
    }

    /// P01 step 3: entity type and relationships from the client inspector.
    #[test]
    fn client_inspector_edits_entity_type_and_relationships() {
        let mut h = cabinet_harness();
        let csv = write_csv(&h, "ClientID,Name\n1001,Acme LLC\n1002,Baker Trust\n");
        click(&mut h, "Import clients CSV…");
        type_into(&mut h, "CSV file", &csv);
        click(&mut h, "Read");
        click(&mut h, "Import 2 rows (one transaction)");
        click(&mut h, "Acme LLC  1001");
        assert!(
            h.query_by_label("No folder yet (from the client list).")
                .is_some()
        );

        h.get_by_label("ENTITY TYPE").click();
        h.run();
        click(&mut h, "S corporation");
        let acme = |h: &Harness<'static, State>| {
            h.state()
                .b
                .snap
                .clients
                .iter()
                .find(|c| c.ext_id == "1001")
                .unwrap()
                .clone()
        };
        assert_eq!(acme(&h).entity_type.as_deref(), Some("s_corp"));

        h.get_by_label("New relationship…").click();
        h.run();
        type_into(&mut h, "Owner client ID", "1002");
        type_into(&mut h, "Role", "shareholder");
        type_into(&mut h, "Share %", "40");
        click(&mut h, "Add relationship");
        let b = &h.state().b;
        assert_eq!(b.snap.relationships.len(), 1, "{:?}", b.rel_form.error);
        assert_eq!(b.snap.relationships[0].share_pct, Some(40.0));
        assert!(
            h.query_by_label("Baker Trust (1002) is shareholder · 40%")
                .is_some()
        );
        let m = vf_fs::clients::load(&h.state().root).unwrap();
        assert_eq!(m.rels.records[0].parent_id, "1001");
        assert_eq!(m.rels.records[0].child_id, "1002");

        click(&mut h, "Remove");
        assert!(h.state().b.snap.relationships.is_empty());
        assert!(
            vf_fs::clients::load(&h.state().root)
                .unwrap()
                .rels
                .records
                .is_empty()
        );
    }

    /// P01 step 5: bulk edit from the inspector, then Undo last.
    /// P05 step 1: the inspector lists a document's earlier versions; Restore brings one back
    /// and keeps the content it replaces.
    #[test]
    fn inspector_lists_versions_and_restores_one() {
        let mut h = harness(1, 1);
        {
            let s = h.state_mut();
            let rel = s.b.snap.docs[0].rel_path.clone();
            let id = s.b.snap.docs[0].id.clone();
            let path = s.root.join(&rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"first content").unwrap();
            let hash = filer_intake::commit::sha256_file(&path).unwrap();
            s.conn
                .execute(
                    "UPDATE document SET hash = ?2, size = 13 WHERE id = ?1",
                    [&id, &hash],
                )
                .unwrap();
            let tmp = vf_fs::content::tmp_file(&s.root, "pdf").unwrap();
            std::fs::write(&tmp, b"second content").unwrap();
            vf_fs::content::replace(
                &mut s.conn,
                &s.root,
                "dev-test",
                &id,
                &tmp,
                vf_fs::content::Change {
                    label: "annotate",
                    created_by: "annotate",
                    reason: "before annotations",
                    detail: serde_json::json!({}),
                    extra: vec![],
                    based_on: None,
                },
            )
            .unwrap()
            .unwrap();
            s.b.reload(&s.conn);
            s.b.selected = s.b.visible.iter().copied().collect();
        }
        h.run();
        assert!(h.query_by_label("Earlier versions").is_some());
        assert!(h.query_by_label_contains("before annotations").is_some());
        h.get_by_role_and_label(Role::Button, "Restore")
            .scroll_to_me();
        h.run();
        click(&mut h, "Restore");
        let s = h.state();
        assert!(s.b.error.is_none(), "{:?}", s.b.error);
        assert!(s.b.notice.is_some(), "restore ran: {:?}", s.b.notice);
        let path = s.root.join(&s.b.snap.docs[0].rel_path);
        assert_eq!(std::fs::read(&path).unwrap(), b"first content");
        assert_eq!(
            vf_fs::versions::list(&s.conn, &s.root, &s.b.snap.docs[0].id)
                .unwrap()
                .len(),
            2,
            "the replaced content is a version too"
        );
    }

    /// P05 step 3: Move to trash takes the selection out of the cabinet in one transaction.
    #[test]
    fn move_to_trash_from_the_inspector() {
        let mut h = harness(1, 2);
        let rels: Vec<String> = {
            let s = h.state_mut();
            let rels: Vec<String> = s.b.snap.docs.iter().map(|d| d.rel_path.clone()).collect();
            for r in &rels {
                let p = s.root.join(r);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, b"x").unwrap();
            }
            s.b.selected = s.b.visible.iter().copied().collect();
            rels
        };
        h.run();
        click(&mut h, "Move 2 to trash");
        // Two or more ask first (owner 2026-11-03).
        assert!(h.query_by_label("Move 2 documents to the trash?").is_some());
        assert_eq!(h.state().b.snap.docs.len(), 2, "nothing moved yet");
        click(&mut h, "Yes, move 2 to trash");
        let s = h.state();
        assert!(s.b.error.is_none(), "{:?}", s.b.error);
        assert!(s.b.snap.docs.is_empty());
        assert!(rels.iter().all(|r| !s.root.join(r).exists()));
        assert_eq!(vf_fs::trash::list(&s.conn, &s.root).unwrap().len(), 2);

        // P05 step 4: the undo stack holds it; Undo last puts both back.
        h.run();
        click(&mut h, "History (1)");
        assert!(
            h.query_by_label_contains("move to trash (2 documents)")
                .is_some()
        );
        h.key_press(egui::Key::Escape);
        h.run();
        click(&mut h, "Undo last");
        let s = h.state();
        assert!(s.b.error.is_none(), "{:?}", s.b.error);
        assert_eq!(
            s.b.notice.as_deref(),
            Some("Undid move to trash (2 documents).")
        );
        assert_eq!(s.b.snap.docs.len(), 2);
        assert!(rels.iter().all(|r| s.root.join(r).is_file()));
    }

    /// P07 step 4: a hold placed on the selection shows as a banner and can be released.
    #[test]
    fn legal_hold_banner_on_the_selection() {
        let mut h = harness(1, 2);
        {
            let s = h.state_mut();
            std::fs::create_dir_all(s.root.join("_Vibe")).unwrap();
            s.b.selected = s.b.visible.iter().copied().collect();
        }
        h.run();
        click(&mut h, "Place legal hold on these 2 documents…");
        type_into(&mut h, "Hold reason", "IRS exam 2023");
        click(&mut h, "Place hold");
        assert!(h.state().b.error.is_none(), "{:?}", h.state().b.error);
        assert!(
            h.query_by_label_contains("Legal hold: IRS exam 2023")
                .is_some()
        );
        let s = h.state();
        let d = &s.b.snap.docs[0];
        assert!(vf_fs::holds::covers(&s.root, None, &d.rel_path, &d.hash));
        click(&mut h, "Release hold");
        // Asked first, naming the hold (QA P8).
        assert!(
            h.query_by_label_contains("Release the legal hold")
                .is_some()
        );
        assert!(
            h.query_by_label_contains("Legal hold:").is_some(),
            "still held"
        );
        click(&mut h, "Yes, release");
        assert!(h.query_by_label_contains("Legal hold:").is_none());
        assert_eq!(h.state().b.notice.as_deref(), Some("Legal hold released."));
    }

    #[test]
    fn inspector_applies_to_the_selection_and_undoes() {
        let mut h = harness(1, 6);
        {
            let s = h.state_mut();
            std::fs::create_dir_all(&s.root).unwrap();
            vf_fs::doctypes::seed_if_missing(&s.root).unwrap();
            vf_fs::doctypes::sync(&mut s.conn, &s.root, "dev-test", "seed").unwrap();
            s.b.reload(&s.conn);
            s.b.selected = s.b.visible.iter().copied().collect();
        }
        h.run();
        h.get_by_role_and_label(Role::ComboBox, "STATUS").click();
        h.run();
        click(&mut h, "Final");
        click(&mut h, "Apply to 6 (one transaction)");
        let finals = |h: &Harness<'static, State>| {
            h.state()
                .b
                .snap
                .docs
                .iter()
                .filter(|d| d.status == "final")
                .count()
        };
        assert_eq!(finals(&h), 6, "{:?}", h.state().b.error);
        assert_eq!(
            h.state().b.notice.as_deref(),
            Some("Updated 6 documents (one transaction).")
        );
        click(&mut h, "Undo last");
        assert_eq!(finals(&h), 0, "{:?}", h.state().b.error);
    }

    /// P01 step 6: `Rename…` previews Filer-template names, renames in one transaction, undoes.
    #[test]
    fn rename_dialog_previews_renames_and_undoes() {
        let mut h = harness(1, 3);
        let rels: Vec<String> = {
            let s = h.state_mut();
            for d in &s.b.snap.docs {
                let p = s.root.join(&d.rel_path);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, &d.id).unwrap();
            }
            s.b.selected = s.b.visible.iter().copied().collect();
            s.b.snap.docs.iter().map(|d| d.rel_path.clone()).collect()
        };
        h.run();
        click(&mut h, "Rename…");
        h.get_by_label("File name template").click();
        h.step();
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
        h.get_by_label("File name template")
            .type_text("Acme {Rest}.{Ext}");
        h.run();
        assert!(h.query_by_label("Acme doc 000.pdf").is_some(), "preview");
        assert!(
            std::path::Path::new(&h.state().root.join(&rels[0])).exists(),
            "not yet"
        );
        click(&mut h, "Rename 3 files (one transaction)");
        assert_eq!(
            h.state().b.notice.as_deref(),
            Some("Renamed 3 files (one transaction)."),
            "{:?}",
            h.state().b.rename.as_ref().and_then(|d| d.error.clone())
        );
        let renamed = |h: &Harness<'static, State>| {
            h.state()
                .b
                .snap
                .docs
                .iter()
                .filter(|d| d.name().starts_with("Acme "))
                .count()
        };
        assert_eq!(renamed(&h), 3);
        click(&mut h, "Undo last");
        assert_eq!(renamed(&h), 0, "{:?}", h.state().b.error);
        assert!(
            h.state().root.join(&rels[0]).exists(),
            "file back under its old name"
        );
    }

    /// P01 step 7: Views (Needs doc type / tax year, Received this week, Pinned) and favorites.
    #[test]
    fn views_pins_and_favorites() {
        let mut h = harness(2, 8);
        {
            let s = h.state_mut();
            // One document with no year outside a Permanent folder; three filed just now.
            s.conn
                .execute(
                    "UPDATE document SET tax_year = NULL WHERE id = 'c0000-1'",
                    [],
                )
                .unwrap();
            s.conn
                .execute(
                    "UPDATE document SET filed_at = ?1 WHERE id IN ('c0001-1', 'c0001-2', 'c0001-3')",
                    [vf_core::now_rfc3339()],
                )
                .unwrap();
            s.b.reload(&s.conn);
        }
        h.run();
        click(&mut h, "Needs doc type");
        assert_eq!(h.state().b.visible.len(), 16, "no doc types yet");
        click(&mut h, "Needs tax year");
        assert_eq!(h.state().b.visible.len(), 1);
        click(&mut h, "Received this week");
        assert_eq!(h.state().b.visible.len(), 3);

        // Pin two, see them under Pinned, unpin.
        click(&mut h, "All documents");
        {
            let b = &mut h.state_mut().b;
            b.selected = b.visible[..2].iter().copied().collect();
        }
        h.run();
        click(&mut h, "Pin");
        assert_eq!(h.state().b.prefs.pinned.len(), 2);
        click(&mut h, "Pinned");
        assert_eq!(h.state().b.visible.len(), 2);
        {
            let b = &mut h.state_mut().b;
            b.selected = b.visible.iter().copied().collect();
        }
        h.run();
        click(&mut h, "Unpin");
        assert!(h.state().b.prefs.pinned.is_empty());
        assert_eq!(h.state().b.visible.len(), 0, "Pinned view follows the list");

        // Right-click a client: add to favorites; it appears under FAVORITES and opens it.
        h.get_by_role_and_label(Role::Button, "Client 0001  1001")
            .click_secondary();
        h.run();
        click(&mut h, "Add to favorites");
        assert_eq!(h.state().b.prefs.favorites.len(), 1);
        assert!(h.state().b.prefs_dirty, "asks the app to save config");
        click(&mut h, "All documents");
        click(&mut h, "Client 0001");
        assert_eq!(h.state().b.scope, Scope::Client(1));
        h.get_by_role_and_label(Role::Button, "Client 0001")
            .click_secondary();
        h.run();
        click(&mut h, "Remove from favorites");
        assert!(h.state().b.prefs.favorites.is_empty());
    }

    /// P02 step 3: `Print cover sheet` for the client in view writes a sheet whose barcode
    /// names that client and year, and opens it to print.
    #[test]
    fn print_cover_sheet_for_the_client_in_view() {
        use std::cell::RefCell;
        use std::rc::Rc;
        let mut h = harness(2, 1);
        let calls: Rc<RefCell<Vec<ShellAction>>> = Rc::default();
        {
            let rec = calls.clone();
            let s = h.state_mut();
            s.b.shell = Box::new(move |a| {
                rec.borrow_mut().push(a.clone());
                Ok(())
            });
            s.b.sheets_dir = s._tmp.path().join("sheets");
        }
        click(&mut h, "Client 0001  1001");
        click(&mut h, "Print cover sheet");
        // The tax year starts at last year (owner 2026-11-04).
        {
            use chrono::Datelike;
            let d = h.state_mut().b.cover.as_mut().unwrap();
            assert_eq!(d.year, (chrono::Local::now().year() - 1).to_string());
            d.year = "2025".into();
        }
        h.run();
        click(&mut h, "Create cover sheet");
        assert!(h.state().b.cover.is_none(), "dialog closed");
        let ShellAction::Open(pdf) = calls.borrow()[0].clone() else {
            panic!("opened the sheet");
        };
        assert!(pdf.starts_with(h.state()._tmp.path().join("sheets")));
        let dir = filer_pdf::pdf::find_pdfium(std::path::Path::new("")).expect("pdfium");
        let pdfium = filer_pdf::pdf::pdfium(&dir).unwrap();
        let pages = vf_ingest::stack::scan_pages(pdfium, &pdf).unwrap();
        let cover = pages[0]
            .sheet
            .clone()
            .and_then(|s| s.0)
            .expect("a cover sheet");
        assert_eq!(cover.client_id, "1001");
        assert_eq!(cover.tax_year, Some(vf_ingest::sheet::TaxYear::Year(2025)));
    }

    /// P01 step 8: double-click / Enter open the file, Open in Explorer reveals it, Copy path
    /// copies it; opening adds the document to Recent. The shell is recorded, nothing launches.
    #[test]
    fn open_reveal_and_copy_path() {
        use std::cell::RefCell;
        use std::rc::Rc;
        let mut h = harness(1, 3);
        let calls: Rc<RefCell<Vec<ShellAction>>> = Rc::default();
        {
            let rec = calls.clone();
            h.state_mut().b.shell = Box::new(move |a| {
                rec.borrow_mut().push(a.clone());
                Ok(())
            });
        }
        h.run();
        let (name, rel, id) = {
            let b = &h.state().b;
            let d = &b.snap.docs[b.visible[0]];
            (d.name().to_owned(), d.rel_path.clone(), d.id.clone())
        };
        let full = shell::full_path(&h.state().root, &rel);

        // Double-click the first row's file name (once selected, the inspector shows the name
        // too: click the grid's copy, found by where it was).
        let at = h.get_by_label(&name).rect();
        h.get_by_label(&name).click();
        h.step();
        h.get_all_by_label(&name)
            .find(|n| n.rect() == at)
            .expect("grid row still there")
            .click();
        h.run();
        // A PDF opens in the Viewer (P04), not in Windows' app.
        assert!(calls.borrow().is_empty());
        let v = h.state_mut().b.view_request.take().expect("viewer asked");
        assert_eq!(
            (v.path.as_path(), v.doc_id.as_str()),
            (full.as_path(), id.as_str())
        );
        assert_eq!(
            h.state().b.prefs.recent,
            std::slice::from_ref(&id),
            "opened → Recent"
        );
        assert!(h.state().b.prefs_dirty);

        // Enter on the cursor row opens it too.
        h.key_press(Key::Enter);
        h.run();
        assert!(h.state_mut().b.view_request.take().is_some());
        assert!(calls.borrow().is_empty());

        click(&mut h, "Open in Explorer");
        assert_eq!(
            calls.borrow().last(),
            Some(&ShellAction::Reveal(full.clone()))
        );
        click(&mut h, "Copy path");
        assert_eq!(
            h.state().b.notice.as_deref(),
            Some(format!("Copied {}", full.display()).as_str())
        );
        assert!(full.to_string_lossy().contains('\\'), "Windows separators");
    }

    /// P01 done-when: tree expansion under 100 ms at 5,000 documents (50 clients x 100).
    #[test]
    fn tree_expands_fast_at_5000_documents() {
        let mut h = harness(50, 100);
        assert_eq!(h.state().b.snap.docs.len(), 5000);
        let mut worst = Duration::ZERO;
        // Best of three neighbouring clients per spot: the code's time, not a busy machine's
        // (the test suite runs OCR and image work in parallel).
        for group in [[0, 1, 2], [17, 18, 19], [42, 43, 44]] {
            let mut best = Duration::MAX;
            for c in group {
                let label = format!("Client {c:04}  {}", 1000 + c);
                h.get_by_role_and_label(Role::Button, &label).click();
                // The frame that handles the click and draws the expanded node and its grid.
                let t = Instant::now();
                h.step();
                best = best.min(t.elapsed());
                h.run();
                assert_eq!(h.state().b.visible.len(), 100);
            }
            worst = worst.max(best);
        }
        eprintln!("worst tree expansion at 5,000 documents: {worst:?}");
        assert!(worst < Duration::from_millis(100), "{worst:?}");
    }
}
