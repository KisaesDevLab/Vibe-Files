//! Browse tab (UI_SPEC §1, P01 step 2): client tree · document grid · inspector.
//!
//! Reads a [`Snapshot`] of the catalog once; tree expansion, folder changes, sort and filter work
//! on it in memory. [`Browse::reload`] re-reads after anything changes the catalog.

use std::collections::{BTreeSet, HashSet};
use std::path::Path;

use egui::{Align, Key, Layout, RichText, Sense};
use egui_extras::{Column, TableBuilder};
use vf_core::browse::{DocRow, IdList, Index, Scope, Snapshot};
use vf_core::db::Connection;

use crate::config::{Favorite, Prefs};
use crate::ui::client_panel::{self, RelForm};
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
    Year(usize, Option<i32>),
}

pub struct Browse {
    pub snap: Snapshot,
    pub ix: Index,
    pub scope: Scope,
    expanded: HashSet<Node>,
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
    /// Newest undoable metadata edit by this device; `None` = not looked up yet.
    undo_tx: Option<Option<String>>,
    /// Ctrl+Z pressed outside a text field.
    undo_requested: bool,
    /// The `Rename…` dialog, while open.
    pub rename: Option<RenameDialog>,
    /// Pinned / Recent / Favorites, from this PC's config; `prefs_dirty` asks the app to save.
    pub prefs: Prefs,
    pub prefs_dirty: bool,
    /// Document counts per entry of [`Browse::views`] and per favorite, and the start of
    /// "this week"; recomputed by `recount` when the catalog or the lists change.
    view_counts: [usize; 5],
    fav_counts: Vec<usize>,
    week_start: String,
}

impl Browse {
    pub fn new(snap: Snapshot) -> Self {
        let ix = Index::build(&snap);
        let mut b = Browse {
            snap,
            ix,
            scope: Scope::Cabinet,
            expanded: HashSet::new(),
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
            undo_tx: None,
            undo_requested: false,
            rename: None,
            prefs: Prefs::default(),
            prefs_dirty: false,
            view_counts: [0; 5],
            fav_counts: Vec::new(),
            week_start: week_start_utc(),
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

    /// Re-reads the catalog, keeping scope (when it still exists) and selection by document id.
    pub fn reload(&mut self, conn: &Connection) {
        let keep: HashSet<String> = self
            .selected
            .iter()
            .map(|&i| self.snap.docs[i].id.clone())
            .collect();
        let old_client = self.scope_client().map(|c| self.snap.clients[c].id.clone());
        match vf_core::browse::load(conn) {
            Ok(s) => {
                self.snap = s;
                self.ix = Index::build(&self.snap);
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
            (Scope::Year { year, .. }, Some(c)) => Scope::Year {
                client: c,
                year: *year,
            },
            (Scope::Folder { year, folder, .. }, Some(c)) => Scope::Folder {
                client: c,
                year: *year,
                folder: folder.clone(),
            },
            (Scope::Unassigned, _) => Scope::Unassigned,
            (
                s @ (Scope::NeedsDocType
                | Scope::NeedsTaxYear
                | Scope::ReceivedSince(_)
                | Scope::Ids { .. }),
                _,
            ) => s.clone(),
            _ => Scope::Cabinet,
        };
        self.expanded.clear();
        self.undo_tx = None;
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
            Scope::Client(c) | Scope::Year { client: c, .. } | Scope::Folder { client: c, .. } => {
                Some(*c)
            }
            _ => None,
        }
    }

    /// Shows `scope` in the grid and clears the selection.
    pub fn set_scope(&mut self, scope: Scope) {
        if self.scope != scope {
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

    /// Ids of the selected documents.
    pub fn selected_ids(&self) -> Vec<String> {
        self.selected
            .iter()
            .map(|&i| self.snap.docs[i].id.clone())
            .collect()
    }

    /// The tree scope a favorite points at, when its client is still here.
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
                year,
                folder,
            } => Scope::Folder {
                client: ci(client)?,
                year: *year,
                folder: folder.clone(),
            },
        })
    }

    /// The favorite for a tree scope (client, year or folder).
    fn as_favorite(&self, s: &Scope) -> Option<Favorite> {
        let id = |c: usize| self.snap.clients[c].id.clone();
        Some(match s {
            Scope::Client(c) => Favorite::Client { client: id(*c) },
            Scope::Year { client, year } => Favorite::Year {
                client: id(*client),
                year: *year,
            },
            Scope::Folder {
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
            Scope::Year { client, year } => format!("{} · {}", name(client), year_label(year)),
            Scope::Folder {
                client,
                year,
                folder,
            } => format!(
                "{} · {} · {}",
                name(client),
                year_label(year),
                folder_label(&folder)
            ),
            _ => String::new(),
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
            let a = self.anchor.unwrap_or(pos);
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

fn folder_label(f: &str) -> &str {
    if f.is_empty() { "(client folder)" } else { f }
}

/// Draws the whole tab (three panes). Call where the central panel would go.
pub fn show(ui: &mut egui::Ui, b: &mut Browse, env: &mut Env) {
    b.keys(ui.ctx());
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
    ui.add_space(8.0);
    ui.label(RichText::new("CABINET").small().color(theme::INK_MUTED));
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
                        &year_label(y.year),
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
                        let here = Scope::Folder {
                            client: ci,
                            year: y.year,
                            folder: f.clone(),
                        };
                        let (_, resp) =
                            tree_row(ui, 2, None, false, folder_label(f), *n, b.scope == here);
                        fav_menu(&resp, here.clone(), &mut fav_scope);
                        if resp.clicked() {
                            pick = Some(here);
                        }
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
        Scope::Client(c) => format!("Cabinet / {}", client(*c)),
        Scope::Year { client: c, year } => {
            format!("Cabinet / {} / {}", client(*c), year_label(*year))
        }
        Scope::Folder {
            client: c,
            year,
            folder,
        } => format!(
            "Cabinet / {} / {} / {}",
            client(*c),
            year_label(*year),
            folder_label(folder)
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
    ui.horizontal(|ui| {
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
        ui.separator();
        let rename = ui
            .add_enabled(!b.selected.is_empty(), egui::Button::new("Rename…"))
            .on_disabled_hover_text("Select the documents to rename first.");
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
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(breadcrumb(b)).monospace());
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
    let mut toggled: Option<usize> = None;
    let mut sort: Option<SortCol> = None;
    let mods = ui.input(|i| i.modifiers);
    let mut table = TableBuilder::new(ui)
        .striped(false)
        .sense(Sense::click())
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
                if row.response().clicked() {
                    clicked = Some((pos, mods));
                }
            });
        });
    if let Some(c) = sort {
        b.sort_by(c);
    }
    if let Some(p) = toggled {
        b.toggle_row(p);
    } else if let Some((p, m)) = clicked {
        b.click_row(p, m);
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
    if b.selected.is_empty()
        && let Some(c) = b.scope_client()
    {
        client_panel::show(ui, b, env, c);
        return;
    }
    if b.meta_form.for_selection != b.selected {
        let docs = b.selected_docs();
        b.meta_form = MetaForm::for_docs(&b.selected, &docs, &b.snap.doc_types);
    }
    if b.undo_tx.is_none() {
        b.undo_tx = Some(vf_core::tx::last_undoable(
            env.conn,
            env.device_id,
            &vf_fs::docmeta::UNDOABLE,
        ));
    }
    let mut undo = std::mem::take(&mut b.undo_requested);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{} documents selected", b.selected.len()))
                .strong()
                .size(14.0),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let can = matches!(b.undo_tx, Some(Some(_)));
            if ui
                .add_enabled(can, egui::Button::new("Undo last"))
                .on_hover_text("Undo the last edit or rename made on this PC (Ctrl+Z)")
                .clicked()
            {
                undo = true;
            }
        });
    });
    if undo && let Some(Some(tx)) = b.undo_tx.clone() {
        match vf_fs::fileops::undo(env.conn, env.root, &tx, env.device_id) {
            Ok(()) => {
                b.reload(env.conn);
                b.notice = Some("Undid the last change.".into());
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
            });
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
    use vf_core::db::{CatalogKey, Storage};

    /// A catalog with `clients` clients x `per` documents spread over 2023-2025 and Permanent,
    /// in the Filer template folders. Rows only (no files): Browse reads the catalog.
    fn seed(clients: usize, per: usize) -> (tempfile::TempDir, Connection) {
        let tmp = tempfile::tempdir().unwrap();
        let mut conn = vf_core::db::open(
            &tmp.path().join("c.db"),
            &CatalogKey([9; 32]),
            Storage::Local,
        )
        .unwrap();
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
        let (tmp, conn) = seed(clients, per);
        let b = Browse::load(&conn);
        let mut h = HarnessBuilder::default()
            .with_size([1440.0, 900.0])
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

    #[test]
    fn tree_drills_from_client_to_folder() {
        let mut h = harness(3, 8);
        assert_eq!(h.state().b.visible.len(), 24, "starts on the whole cabinet");
        click(&mut h, "Client 0001  1001");
        assert_eq!(h.state().b.scope, Scope::Client(1));
        assert_eq!(h.state().b.visible.len(), 8);
        click(&mut h, "2025");
        assert_eq!(h.state().b.visible.len(), 4);
        click(&mut h, "Payroll");
        assert_eq!(
            h.state().b.scope,
            Scope::Folder {
                client: 1,
                year: Some(2025),
                folder: "Payroll".into()
            }
        );
        assert_eq!(h.state().b.visible.len(), 2);
        click(&mut h, "Permanent");
        assert_eq!(h.state().b.visible.len(), 2);
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

    /// P01 step 3: the CSV import keeps the master list, skips TIN columns, reports matching
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
        by_label(&mut h, "SSN"); // disabled: no effect
        assert_eq!(h.state().b.import.as_ref().unwrap().map.extras, ["Partner"]);
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

    /// P01 done-when: tree expansion under 100 ms at 5,000 documents (50 clients x 100).
    #[test]
    fn tree_expands_fast_at_5000_documents() {
        let mut h = harness(50, 100);
        assert_eq!(h.state().b.snap.docs.len(), 5000);
        let mut worst = Duration::ZERO;
        for c in [0, 17, 42] {
            let label = format!("Client {c:04}  {}", 1000 + c);
            h.get_by_role_and_label(Role::Button, &label).click();
            // The frame that handles the click and draws the expanded node and its grid.
            let t = Instant::now();
            h.step();
            worst = worst.max(t.elapsed());
            h.run();
            assert_eq!(h.state().b.visible.len(), 100);
        }
        eprintln!("worst tree expansion at 5,000 documents: {worst:?}");
        assert!(worst < Duration::from_millis(100), "{worst:?}");
    }
}
