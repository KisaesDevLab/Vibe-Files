//! Browse tab (PHASES P01 step 2, UI_SPEC §1): client tree · document grid · inspector.

use std::collections::{BTreeSet, HashMap, HashSet};

use egui::{Align, Layout, RichText, Sense};
use egui_extras::{Column, TableBuilder};
use vf_core::browse::{self, Bucket, BucketRow, ClientRow, DocRow, FolderRow, Scope};
use vf_core::db::Connection;
use vf_core::views::{self, Favorite, View};

use crate::ui::format;
use crate::ui::theme;
use crate::ui::widgets::{mono, muted, section, status_pill};

/// A tree position.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Node {
    Client(String),
    Bucket(String, Bucket),
    Folder(String, Bucket, String),
    Unfiled,
    View(View),
}

impl Node {
    pub fn client_id(&self) -> Option<&str> {
        match self {
            Node::Client(c) | Node::Bucket(c, _) | Node::Folder(c, _, _) => Some(c),
            Node::Unfiled | Node::View(_) => None,
        }
    }

    /// A favorite's tree position.
    pub fn from_favorite(f: &Favorite) -> Node {
        let bucket = f.bucket.as_deref().map(|b| match b.parse::<i64>() {
            Ok(y) => Bucket::Year(y),
            Err(_) => Bucket::Permanent,
        });
        match (bucket, &f.folder) {
            (None, _) => Node::Client(f.client_id.clone()),
            (Some(b), None) => Node::Bucket(f.client_id.clone(), b),
            (Some(b), Some(folder)) => Node::Folder(f.client_id.clone(), b, folder.clone()),
        }
    }

    /// `(bucket, folder)` as stored in a favorite.
    pub fn favorite_key(&self) -> Option<(&str, Option<String>, Option<String>)> {
        let b = |b: &Bucket| match b {
            Bucket::Permanent => "perm".to_owned(),
            Bucket::Year(y) => y.to_string(),
        };
        match self {
            Node::Client(c) => Some((c, None, None)),
            Node::Bucket(c, bk) => Some((c, Some(b(bk)), None)),
            Node::Folder(c, bk, f) => Some((c, Some(b(bk)), Some(f.clone()))),
            Node::Unfiled | Node::View(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Col {
    Name,
    DocType,
    Year,
    Status,
    Size,
    Modified,
    FiledBy,
    Hash,
}

impl Col {
    const ALL: [Col; 8] = [
        Col::Name,
        Col::DocType,
        Col::Year,
        Col::Status,
        Col::Size,
        Col::Modified,
        Col::FiledBy,
        Col::Hash,
    ];

    fn label(self) -> &'static str {
        match self {
            Col::Name => "Name",
            Col::DocType => "Doc type",
            Col::Year => "Year",
            Col::Status => "Status",
            Col::Size => "Size",
            Col::Modified => "Modified",
            Col::FiledBy => "Filed by",
            Col::Hash => "Hash",
        }
    }

    fn cmp(self, a: &DocRow, b: &DocRow) -> std::cmp::Ordering {
        match self {
            Col::Name => a.name().to_lowercase().cmp(&b.name().to_lowercase()),
            Col::DocType => a.doc_type.cmp(&b.doc_type),
            Col::Year => a.tax_year.cmp(&b.tax_year),
            Col::Status => status_rank(&a.status).cmp(&status_rank(&b.status)),
            Col::Size => a.size.cmp(&b.size),
            Col::Modified => a.updated_at.cmp(&b.updated_at),
            Col::FiledBy => a.filed_by.cmp(&b.filed_by),
            Col::Hash => a.hash.cmp(&b.hash),
        }
        .then_with(|| a.rel_path.cmp(&b.rel_path))
    }
}

fn status_rank(s: &str) -> usize {
    format::STATUSES.iter().position(|x| *x == s).unwrap_or(9)
}

/// What the grid view was computed from: generation, node, filter, sort.
type ViewKey = (i64, Option<Node>, String, Option<(Col, bool)>);

/// What the Browse tab asks the workspace to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Open(String),
    AddFavorite,
    RemoveFavorite(String),
}

#[derive(Default)]
pub struct Browse {
    /// Whose pins, favorites and recent opens the Views section shows.
    pub user: String,
    view_counts: Vec<(View, usize)>,
    favorites: Vec<Favorite>,
    /// This user's pinned document ids.
    pinned: HashSet<String>,
    /// Every live client (the relationship picker), unfiltered.
    all_clients: Vec<ClientRow>,
    /// Single-document details: (generation, doc id, versions, last activity).
    detail: Option<(i64, String, i64, Vec<browse::Activity>)>,
    pub client_filter: String,
    pub grid_filter: String,
    pub expanded: HashSet<Node>,
    pub node: Option<Node>,
    /// Documents of `node`, unfiltered.
    all: Vec<DocRow>,
    /// `all` filtered by `grid_filter` and sorted by `sort`.
    pub rows: Vec<DocRow>,
    pub sort: Option<(Col, bool)>,
    /// Selected document ids (stable across sorting and refresh).
    pub selected: BTreeSet<String>,
    anchor: Option<usize>,
    pub cursor: Option<usize>,
    scroll_to: Option<usize>,
    generation: i64,
    loaded: Option<(i64, Option<Node>)>,
    view_key: Option<ViewKey>,
    clients: Vec<ClientRow>,
    clients_key: Option<(i64, String)>,
    buckets: HashMap<String, Vec<BucketRow>>,
    folders: HashMap<(String, Bucket), Vec<FolderRow>>,
    pub totals: (i64, i64),
    unfiled: i64,
    pub error: Option<String>,
}

impl Browse {
    pub fn for_user(user: String) -> Self {
        Self {
            user,
            ..Self::default()
        }
    }

    /// Re-reads whatever the catalog changed since `generation` was last seen (the caller reads
    /// it once per frame). Cheap when nothing did.
    pub fn refresh(&mut self, conn: &Connection, generation: i64) {
        let r = self.try_refresh(conn, generation);
        if let Err(e) = r {
            self.error = Some(e.to_string());
        }
    }

    pub fn generation(&self) -> i64 {
        self.generation
    }

    pub fn is_pinned(&self, doc_id: &str) -> bool {
        self.pinned.contains(doc_id)
    }

    pub fn all_clients(&self) -> &[ClientRow] {
        &self.all_clients
    }

    fn try_refresh(&mut self, conn: &Connection, generation: i64) -> vf_core::Result<()> {
        if generation != self.generation || self.clients_key.is_none() {
            self.generation = generation;
            self.buckets.clear();
            self.folders.clear();
            self.totals = browse::totals(conn)?;
            self.view_counts = views::counts(conn, &self.user)?;
            self.favorites = views::favorites(conn, &self.user)?;
            self.pinned = views::ids(conn, &self.user, View::Pinned)?
                .into_iter()
                .collect();
            self.all_clients = browse::clients(conn, "")?;
            self.unfiled = conn.query_row(
                "SELECT count(*) FROM document WHERE client_id IS NULL AND deleted_at IS NULL",
                [],
                |r| r.get(0),
            )?;
        }
        let ck = (generation, self.client_filter.clone());
        if self.clients_key.as_ref() != Some(&ck) {
            self.clients = if self.client_filter.trim().is_empty() {
                self.all_clients.clone()
            } else {
                browse::clients(conn, &self.client_filter)?
            };
            self.clients_key = Some(ck);
        }
        for n in &self.expanded {
            match n {
                Node::Client(c) if !self.buckets.contains_key(c) => {
                    self.buckets.insert(c.clone(), browse::buckets(conn, c)?);
                }
                Node::Bucket(c, b) if !self.folders.contains_key(&(c.clone(), *b)) => {
                    self.folders
                        .insert((c.clone(), *b), browse::folders(conn, c, *b)?);
                }
                _ => {}
            }
        }
        let lk = (generation, self.node.clone());
        if self.loaded.as_ref() != Some(&lk) {
            self.all = match &self.node {
                None => Vec::new(),
                Some(Node::View(v)) => {
                    let ids = views::ids(conn, &self.user, *v)?;
                    let mut docs = browse::documents(conn, &Scope::Ids(ids.clone()))?;
                    // Keep the view's own order (Recent: newest first).
                    let pos: HashMap<&str, usize> = ids
                        .iter()
                        .enumerate()
                        .map(|(i, id)| (id.as_str(), i))
                        .collect();
                    docs.sort_by_key(|d| pos.get(d.id.as_str()).copied().unwrap_or(usize::MAX));
                    docs
                }
                Some(Node::Client(c)) => browse::documents(conn, &Scope::Client(c.clone()))?,
                Some(Node::Bucket(c, b)) => browse::documents(conn, &Scope::Bucket(c.clone(), *b))?,
                Some(Node::Folder(c, b, f)) => {
                    browse::documents(conn, &Scope::Folder(c.clone(), *b, f.clone()))?
                }
                Some(Node::Unfiled) => browse::documents(conn, &Scope::Unfiled)?,
            };
            self.loaded = Some(lk);
        }
        let vk = (
            generation,
            self.node.clone(),
            self.grid_filter.clone(),
            self.sort,
        );
        if self.view_key.as_ref() != Some(&vk) {
            let f = self.grid_filter.trim().to_lowercase();
            let mut rows: Vec<DocRow> = self
                .all
                .iter()
                .filter(|d| {
                    f.is_empty()
                        || d.rel_path.to_lowercase().contains(&f)
                        || d.doc_type
                            .as_deref()
                            .unwrap_or("")
                            .to_lowercase()
                            .contains(&f)
                        || d.tags.to_lowercase().contains(&f)
                })
                .cloned()
                .collect();
            if let Some((col, asc)) = self.sort {
                rows.sort_by(|a, b| {
                    let o = col.cmp(a, b);
                    if asc { o } else { o.reverse() }
                });
            }
            let live: HashSet<&str> = rows.iter().map(|d| d.id.as_str()).collect();
            self.selected.retain(|id| live.contains(id.as_str()));
            self.rows = rows;
            if self.cursor.is_some_and(|c| c >= self.rows.len()) {
                self.cursor = None;
            }
            self.view_key = Some(vk);
        }
        self.error = None;
        Ok(())
    }

    pub fn select_node(&mut self, n: Node) {
        if self.node.as_ref() != Some(&n) {
            self.selected.clear();
            self.cursor = None;
            self.anchor = None;
        }
        self.node = Some(n);
    }

    pub fn selected_rows(&self) -> Vec<&DocRow> {
        self.rows
            .iter()
            .filter(|d| self.selected.contains(&d.id))
            .collect()
    }

    pub fn client_name(&self, id: &str) -> Option<&str> {
        self.clients
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.name.as_str())
    }

    pub fn clients(&self) -> &[ClientRow] {
        &self.clients
    }

    fn click_row(&mut self, i: usize, m: egui::Modifiers) {
        let id = self.rows[i].id.clone();
        if m.shift
            && let Some(a) = self.anchor
        {
            if !m.command {
                self.selected.clear();
            }
            let (lo, hi) = (a.min(i), a.max(i));
            for r in &self.rows[lo..=hi] {
                self.selected.insert(r.id.clone());
            }
        } else if m.command {
            if !self.selected.remove(&id) {
                self.selected.insert(id);
            }
            self.anchor = Some(i);
        } else {
            self.selected.clear();
            self.selected.insert(id);
            self.anchor = Some(i);
        }
        self.cursor = Some(i);
    }

    /// Arrow keys, Space, Ctrl+A, Enter, while no text field has focus.
    fn keys(&mut self, ui: &egui::Ui) -> Option<Action> {
        if self.rows.is_empty() || ui.memory(|m| m.focused().is_some()) {
            return None;
        }
        let (down, up, space, all, enter, m) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::Space),
                i.modifiers.command && i.key_pressed(egui::Key::A),
                i.key_pressed(egui::Key::Enter),
                i.modifiers,
            )
        });
        let last = self.rows.len() - 1;
        if down || up {
            let next = match (self.cursor, down) {
                (None, _) => 0,
                (Some(c), true) => (c + 1).min(last),
                (Some(c), false) => c.saturating_sub(1),
            };
            if m.shift {
                if self.anchor.is_none() {
                    self.anchor = self.cursor.or(Some(next));
                }
                self.click_row(next, egui::Modifiers::SHIFT);
            } else {
                self.click_row(next, egui::Modifiers::NONE);
            }
            self.scroll_to = Some(next);
        }
        if space && let Some(c) = self.cursor {
            let id = self.rows[c].id.clone();
            if !self.selected.remove(&id) {
                self.selected.insert(id);
            }
        }
        if all {
            self.selected = self.rows.iter().map(|d| d.id.clone()).collect();
        }
        if enter && let Some(c) = self.cursor {
            return Some(Action::Open(self.rows[c].id.clone()));
        }
        None
    }

    /// The tree (left pane).
    pub fn tree_ui(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        let mut action = None;
        section(ui, "Views");
        for (v, n) in self.view_counts.clone() {
            let node = Node::View(v);
            let selected = self.node.as_ref() == Some(&node);
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                let mut text = RichText::new(v.label());
                if selected {
                    text = text.color(theme::ACCENT_DEFAULT);
                } else if v.is_attention() && n > 0 {
                    text = text.color(theme::WARN_FG);
                }
                if ui
                    .add(egui::Button::selectable(selected, text).frame(false))
                    .clicked()
                {
                    self.select_node(node.clone());
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(mono(n.to_string()).small().color(theme::INK_MUTED));
                });
            });
        }
        if !self.favorites.is_empty() {
            section(ui, "Favorites");
            for f in self.favorites.clone() {
                let node = Node::from_favorite(&f);
                let selected = self.node.as_ref() == Some(&node);
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    let text = RichText::new(&f.label);
                    let text = if selected {
                        text.color(theme::ACCENT_DEFAULT)
                    } else {
                        text
                    };
                    if ui
                        .add(egui::Button::selectable(selected, text).frame(false))
                        .clicked()
                    {
                        if let Some(c) = node.client_id() {
                            self.expanded.insert(Node::Client(c.to_owned()));
                        }
                        if let Node::Folder(c, b, _) = &node {
                            self.expanded.insert(Node::Bucket(c.clone(), *b));
                        }
                        self.select_node(node.clone());
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .small_button("×")
                            .on_hover_text("Remove from favorites")
                            .clicked()
                        {
                            action = Some(Action::RemoveFavorite(f.id.clone()));
                        }
                    });
                });
            }
        }
        section(ui, "Cabinet");
        let l = ui.label(muted(format!("{} clients", self.clients.len())));
        ui.add(
            egui::TextEdit::singleline(&mut self.client_filter)
                .hint_text("Find client by name or ID")
                .desired_width(f32::INFINITY),
        )
        .labelled_by(l.id);
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt("tree")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.clients.is_empty() {
                    ui.label(muted("Create your first client or run the import wizard."));
                }
                let clients = self.clients.clone();
                for c in &clients {
                    let node = Node::Client(c.id.clone());
                    let open = self.expanded.contains(&node);
                    if self.tree_row(ui, 0, Some(open), &node, &c.name, &c.ext_id, true) {
                        self.toggle(node.clone());
                    }
                    if !open {
                        continue;
                    }
                    let buckets = self.buckets.get(&c.id).cloned().unwrap_or_default();
                    for b in buckets {
                        let bn = Node::Bucket(c.id.clone(), b.bucket);
                        let bopen = self.expanded.contains(&bn);
                        let label = b.bucket.label();
                        if self.tree_row(
                            ui,
                            1,
                            Some(bopen),
                            &bn,
                            &label,
                            &b.docs.to_string(),
                            false,
                        ) {
                            self.toggle(bn.clone());
                        }
                        if !bopen {
                            continue;
                        }
                        let folders = self
                            .folders
                            .get(&(c.id.clone(), b.bucket))
                            .cloned()
                            .unwrap_or_default();
                        for f in folders {
                            let label = if f.folder.is_empty() {
                                "(client folder)".to_owned()
                            } else {
                                f.folder.clone()
                            };
                            let fnode = Node::Folder(c.id.clone(), b.bucket, f.folder.clone());
                            self.tree_row(ui, 2, None, &fnode, &label, &f.docs.to_string(), false);
                        }
                    }
                }
                if self.unfiled > 0 {
                    ui.add_space(6.0);
                    let n = self.unfiled.to_string();
                    self.tree_row(
                        ui,
                        0,
                        None,
                        &Node::Unfiled,
                        "Not in a client folder",
                        &n,
                        false,
                    );
                }
            });
        action
    }

    /// One tree row; returns true when it should toggle open/closed.
    #[allow(clippy::too_many_arguments)]
    fn tree_row(
        &mut self,
        ui: &mut egui::Ui,
        depth: usize,
        open: Option<bool>,
        node: &Node,
        label: &str,
        right: &str,
        client: bool,
    ) -> bool {
        let selected = self.node.as_ref() == Some(node);
        let mut toggle = false;
        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 14.0);
            let (c, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), Sense::hover());
            if let Some(o) = open {
                chevron(ui.painter(), c, o);
            }
            let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), Sense::hover());
            ui.painter().rect_filled(
                r,
                2,
                if client {
                    theme::FOLDER_CLIENT
                } else {
                    theme::FOLDER_SUB
                },
            );
            let text = RichText::new(label);
            let text = if selected {
                text.color(theme::ACCENT_DEFAULT)
            } else {
                text
            };
            let resp = ui.add(egui::Button::selectable(selected, text).frame(false));
            if resp.clicked() {
                if selected || !open.unwrap_or(true) {
                    toggle = open.is_some();
                }
                self.select_node(node.clone());
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(mono(right).small().color(theme::INK_MUTED));
            });
        });
        toggle
    }

    fn toggle(&mut self, n: Node) {
        if !self.expanded.remove(&n) {
            self.expanded.insert(n);
        }
    }

    pub fn breadcrumb(&self) -> String {
        let Some(n) = &self.node else {
            return "Cabinet".into();
        };
        let name = |c: &str| self.client_name(c).unwrap_or("?").to_owned();
        match n {
            Node::Client(c) => format!("Cabinet / {}", name(c)),
            Node::Bucket(c, b) => format!("Cabinet / {} / {}", name(c), b.label()),
            Node::Folder(c, b, f) => format!("Cabinet / {} / {} / {f}", name(c), b.label()),
            Node::Unfiled => "Cabinet / Not in a client folder".into(),
            Node::View(v) => format!("Views / {}", v.label()),
        }
    }

    /// Label for a new favorite of the current node.
    pub fn favorite_label(&self) -> Option<String> {
        let n = self.node.as_ref()?;
        n.favorite_key()?;
        Some(
            self.breadcrumb()
                .trim_start_matches("Cabinet / ")
                .replace(" / ", " · "),
        )
    }

    pub fn is_favorite(&self, n: &Node) -> bool {
        self.favorites.iter().any(|f| Node::from_favorite(f) == *n)
    }

    /// The grid (main pane).
    pub fn grid_ui(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        let mut action = self.keys(ui);
        ui.horizontal(|ui| {
            ui.label(RichText::new(self.breadcrumb()).color(theme::INK_SECONDARY));
            if let Some(n) = self.node.clone()
                && n.favorite_key().is_some()
                && !self.is_favorite(&n)
                && ui.small_button("Add to favorites").clicked()
            {
                action = Some(Action::AddFavorite);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.grid_filter)
                        .hint_text("Filter this folder")
                        .desired_width(200.0),
                );
                ui.label(muted(format!(
                    "{} documents · {} selected",
                    self.rows.len(),
                    self.selected.len()
                )));
            });
        });
        ui.separator();
        if self.node.is_none() {
            ui.label(muted("Pick a client, year or folder on the left."));
            return action;
        }
        let mut sort_click = None;
        let mut row_click: Option<(usize, egui::Modifiers)> = None;
        let mut toggle_click = None;
        let mut open_click = None;
        let mut table = TableBuilder::new(ui)
            .id_salt("grid")
            .striped(false)
            .sense(Sense::click())
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::exact(22.0))
            .column(Column::remainder().at_least(180.0).clip(true))
            .column(Column::initial(140.0).clip(true).resizable(true))
            .column(Column::initial(52.0))
            .column(Column::initial(84.0))
            .column(Column::initial(70.0))
            .column(Column::initial(124.0))
            .column(Column::initial(84.0).clip(true))
            .column(Column::initial(76.0));
        if let Some(i) = self.scroll_to.take() {
            table = table.scroll_to_row(i, None);
        }
        let sort = self.sort;
        table
            .header(26.0, |mut h| {
                h.col(|_| {});
                for c in Col::ALL {
                    h.col(|ui| {
                        let arrow = match sort {
                            Some((s, true)) if s == c => " ↑",
                            Some((s, false)) if s == c => " ↓",
                            _ => "",
                        };
                        let t = RichText::new(format!("{}{arrow}", c.label()))
                            .small()
                            .strong()
                            .color(theme::INK_SECONDARY);
                        if ui.add(egui::Button::new(t).frame(false)).clicked() {
                            sort_click = Some(c);
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(30.0, self.rows.len(), |mut row| {
                    let i = row.index();
                    let d = &self.rows[i];
                    let sel = self.selected.contains(&d.id);
                    row.set_selected(sel);
                    row.col(|ui| {
                        let mut s = sel;
                        if ui.checkbox(&mut s, "").on_hover_text("Select").changed() {
                            toggle_click = Some(i);
                        }
                    });
                    row.col(|ui| {
                        let mut t = mono(d.name());
                        if d.doc_type_id.is_none() {
                            t = t.color(theme::WARN_FG);
                        }
                        ui.add(egui::Label::new(t).truncate().selectable(false));
                    });
                    row.col(|ui| {
                        ui.label(d.doc_type.clone().unwrap_or_else(|| "—".into()));
                    });
                    row.col(|ui| {
                        ui.label(d.tax_year.map_or("Perm.".into(), |y| y.to_string()));
                    });
                    row.col(|ui| {
                        status_pill(ui, &d.status);
                    });
                    row.col(|ui| {
                        ui.label(mono(format::size(d.size)));
                    });
                    row.col(|ui| {
                        ui.label(mono(format::local_time(&d.updated_at)).small());
                    });
                    row.col(|ui| {
                        ui.label(d.filed_by.clone().unwrap_or_default());
                    });
                    row.col(|ui| {
                        ui.label(mono(d.hash_short()).color(theme::INK_MUTED));
                    });
                    let r = row.response();
                    if r.double_clicked() {
                        open_click = Some(i);
                    } else if r.clicked() {
                        row_click = Some((i, r.ctx.input(|i| i.modifiers)));
                    }
                });
            });
        if let Some(c) = sort_click {
            self.sort = match self.sort {
                Some((s, asc)) if s == c => Some((c, !asc)),
                _ => Some((c, true)),
            };
        }
        if let Some(i) = toggle_click {
            self.click_row(i, egui::Modifiers::COMMAND);
        }
        if let Some((i, m)) = row_click {
            self.click_row(i, m);
        }
        if let Some(i) = open_click {
            self.click_row(i, egui::Modifiers::NONE);
            action = Some(Action::Open(self.rows[i].id.clone()));
        }
        action
    }

    /// The read-only part of the inspector: details of a single selected document.
    pub fn detail_ui(&mut self, ui: &mut egui::Ui, conn: &Connection) {
        let sel = self.selected_rows();
        let [d] = sel.as_slice() else {
            return;
        };
        let d = (*d).clone();
        let fresh = self
            .detail
            .as_ref()
            .is_some_and(|(g, id, _, _)| *g == self.generation && *id == d.id);
        if !fresh {
            let versions = browse::version_count(conn, &d.id).unwrap_or(0);
            let acts = browse::recent_activity(conn, &d.id, 3).unwrap_or_default();
            self.detail = Some((self.generation, d.id.clone(), versions, acts));
        }
        let (_, _, versions, acts) = self.detail.clone().expect("filled above");
        section(ui, "Document");
        ui.label(mono(d.name()).strong());
        egui::Grid::new("detail")
            .num_columns(2)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                let kv = |ui: &mut egui::Ui, k: &str, v: RichText| {
                    ui.label(muted(k));
                    ui.add(egui::Label::new(v).wrap());
                    ui.end_row();
                };
                kv(ui, "SHA-256", mono(&d.hash).small());
                kv(ui, "Size", mono(format::size(d.size)));
                kv(ui, "Versions", RichText::new(versions.to_string()));
                kv(
                    ui,
                    "Filed",
                    RichText::new(format!(
                        "{} by {}",
                        format::local_time(&d.filed_at),
                        d.filed_by.as_deref().unwrap_or("?")
                    )),
                );
                if !d.tags.is_empty() {
                    kv(ui, "Tags", RichText::new(&d.tags));
                }
                kv(ui, "Path", mono(&d.rel_path).small());
            });
        section(ui, "Recent activity");
        if acts.is_empty() {
            ui.label(muted("No activity recorded for this document yet."));
        }
        for a in acts {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    mono(format::local_hm(&a.ts))
                        .small()
                        .color(theme::INK_MUTED),
                );
                ui.label(format!(
                    "{} · {}",
                    a.action,
                    a.user.as_deref().unwrap_or("system")
                ));
            });
        }
    }
}

/// A small filled triangle: pointing down when open, right when closed. (Painted, because the
/// bundled IBM Plex fonts have no triangle glyphs.)
fn chevron(p: &egui::Painter, r: egui::Rect, open: bool) {
    let c = r.center();
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
    p.add(egui::Shape::convex_polygon(
        pts,
        theme::INK_MUTED,
        egui::Stroke::NONE,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(names: &[&str]) -> Browse {
        Browse {
            rows: names
                .iter()
                .enumerate()
                .map(|(i, n)| DocRow {
                    id: format!("d{i}"),
                    client_id: None,
                    rel_path: format!("x/{n}"),
                    doc_type_id: None,
                    doc_type: None,
                    tax_year: None,
                    status: "received".into(),
                    source: None,
                    size: i as i64,
                    hash: "ab".repeat(32),
                    filed_by: None,
                    filed_at: "t".into(),
                    updated_at: "t".into(),
                    tags: String::new(),
                })
                .collect(),
            ..Browse::default()
        }
    }

    #[test]
    fn click_ctrl_click_shift_click() {
        let mut b = rows(&["a", "b", "c", "d", "e"]);
        b.click_row(1, egui::Modifiers::NONE);
        assert_eq!(b.selected, BTreeSet::from(["d1".into()]));
        b.click_row(3, egui::Modifiers::COMMAND);
        assert_eq!(b.selected.len(), 2);
        b.click_row(3, egui::Modifiers::COMMAND);
        assert_eq!(b.selected, BTreeSet::from(["d1".into()]));
        b.click_row(1, egui::Modifiers::NONE);
        b.click_row(4, egui::Modifiers::SHIFT);
        let want: BTreeSet<String> = ["d1", "d2", "d3", "d4"].map(String::from).into();
        assert_eq!(b.selected, want);
    }
}
