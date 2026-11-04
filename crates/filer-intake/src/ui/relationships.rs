//! Relationships tab (UI_SPEC §9): search a client on the left; on the right the clients it is the
//! parent of and the clients it is a child of, with role, note, delete and an add row. Every edit
//! is saved at once. CSV import (merge) and export.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use egui::{Align, CornerRadius, Frame, Layout, Margin, RichText, Stroke};

use crate::app::AppState;
use crate::clients::{self, Client, ClientList, CsvTable};
use crate::config::Level;
use crate::relationships::{self, ROLES, RelColumns, Relationship};
use crate::ui::theme::banner;
use crate::ui::theme::{PillKind, label_caps, mono, pal, pill, primary_button, strong, thousands};
use crate::ui::widgets::{PickerOutcome, client_picker, search_clients};
use crate::worker;

/// Which side of the selected client a list or add row is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    /// The selected client is the parent; rows are its children.
    Children,
    /// The selected client is the child; rows are its parents.
    Parents,
}

/// The add row under each list.
#[derive(Debug, Clone)]
struct AddRow {
    query: String,
    chosen: Option<Client>,
    role: String,
    note: String,
}

impl AddRow {
    fn new(role: &str) -> Self {
        Self {
            query: String::new(),
            chosen: None,
            role: role.into(),
            note: String::new(),
        }
    }
}

pub struct RelationshipsScreen {
    search: String,
    selected: Option<String>,
    add_children: AddRow,
    add_parents: AddRow,
    /// Record index being re-linked, and which ID of it is replaced.
    relink: Option<(usize, Side, String)>,
    import: Option<Import>,
    message: Option<(Level, String)>,
    /// Opened from Preview's "Add relationship…": go back there after the add.
    return_to_preview: bool,
}

impl Default for RelationshipsScreen {
    fn default() -> Self {
        Self {
            search: String::new(),
            selected: None,
            add_children: AddRow::new("partner"),
            add_parents: AddRow::new("related"),
            relink: None,
            import: None,
            message: None,
            return_to_preview: false,
        }
    }
}

enum Edit {
    Role(usize, String),
    Note(usize, String),
    Remove(usize),
    Add(Relationship),
    Relink(usize, Side),
}

impl RelationshipsScreen {
    pub fn show(&mut self, ui: &mut egui::Ui, state: &mut AppState) {
        let p = pal(ui);
        // "Add relationship…" from Preview: open on the parent with the add row filled.
        if let Some((parent, query)) = state.relationship_prefill.take() {
            self.selected = Some(parent);
            self.add_children = AddRow::new("shareholder");
            self.add_children.query = query;
            self.search.clear();
            self.return_to_preview = true;
        }
        ui.add_space(4.0);
        if let Some(e) = &state.relationships_error {
            banner(ui, Level::Error, e);
        }
        let Some(list) = state.clients.list().cloned() else {
            ui.label("Import the client list in Setup first.");
            return;
        };
        let dangling = state.relationships.dangling(&list);
        self.header_ui(ui, state, &list, dangling.len());
        if let Some((level, msg)) = &self.message {
            banner(ui, *level, msg);
        }
        ui.add_space(6.0);

        let mut edits: Vec<Edit> = Vec::new();
        ui.horizontal_top(|ui| {
            let height = ui.available_height();
            ui.allocate_ui_with_layout(
                egui::vec2(340.0, height),
                Layout::top_down(Align::Min),
                |ui| self.left_ui(ui, state, &list, height),
            );
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());
                egui::ScrollArea::vertical()
                    .id_salt("relationships_right")
                    .auto_shrink(false)
                    .show(ui, |ui| match self.selected.clone() {
                        None => {
                            ui.label(
                                RichText::new("Pick a client on the left to see its relationships.")
                                    .color(p.muted),
                            );
                        }
                        Some(id) => {
                            let name = list
                                .match_by_id(&id)
                                .map_or_else(|| format!("ID {id}"), |c| c.name.clone());
                            for side in [Side::Children, Side::Parents] {
                                self.side_ui(ui, state, &list, &id, &name, side, &mut edits);
                                ui.add_space(10.0);
                            }
                            ui.label(
                                RichText::new(
                                    "Stored once, parent → child. \"spouse\" is a role, not mirrored. \
                                     Both IDs must exist in the master list; a refresh that drops one \
                                     flags the row instead of deleting it.",
                                )
                                .size(12.0)
                                .color(p.muted),
                            );
                        }
                    });
            });
        });
        self.apply(state, edits);
        self.relink_ui(ui.ctx(), state, &list);
        self.import_ui(ui.ctx(), state, &list);
    }

    fn header_ui(
        &mut self,
        ui: &mut egui::Ui,
        state: &AppState,
        list: &ClientList,
        dangling: usize,
    ) {
        let p = pal(ui);
        ui.horizontal(|ui| {
            // One label, so its text is not just "Relationships" (the tab's name).
            let mut job = egui::text::LayoutJob::default();
            job.append(
                "Relationships",
                0.0,
                egui::TextFormat::simple(
                    egui::FontId::new(
                        15.0,
                        egui::FontFamily::Name(crate::ui::theme::SEMIBOLD.into()),
                    ),
                    p.text,
                ),
            );
            let mut rest = format!(" · {} records", thousands(state.relationships.len()));
            if dangling > 0 {
                rest.push_str(&format!(
                    " · {dangling} dangling after the {} CSV refresh",
                    list.imported_at
                        .with_timezone(&chrono::Local)
                        .format("%b %-d")
                ));
            }
            rest.push_str(" · saved on every edit");
            job.append(
                &rest,
                0.0,
                egui::TextFormat::simple(egui::FontId::proportional(12.0), p.muted),
            );
            ui.label(job);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Export CSV").clicked() {
                    self.export(state);
                }
                if ui.button("Import CSV…").clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("CSV", &["csv", "txt"])
                        .pick_file()
                {
                    self.import = Some(Import::open(ui.ctx(), path));
                }
            });
        });
    }

    fn left_ui(&mut self, ui: &mut egui::Ui, state: &AppState, list: &ClientList, height: f32) {
        let p = pal(ui);
        Frame::new()
            .fill(p.card)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(CornerRadius::same(6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(height - 2.0);
                Frame::new().inner_margin(Margin::same(12)).show(ui, |ui| {
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut self.search)
                            .hint_text("ID or fuzzy name")
                            .desired_width(f32::INFINITY),
                    );
                    field.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Find a client")
                    });
                });
                // Matches, or the clients that have relationships when the search is empty.
                let results: Vec<(Option<f64>, &Client)> = if self.search.trim().is_empty() {
                    let mut with: Vec<&Client> = list
                        .clients
                        .iter()
                        .filter(|c| state.relationships.related_count(&c.id) > 0)
                        .collect();
                    with.sort_by_key(|c| c.name.to_lowercase());
                    with.into_iter().take(200).map(|c| (None, c)).collect()
                } else {
                    search_clients(list, &self.search, 30)
                        .into_iter()
                        .filter(|(s, _)| s.is_none_or(|s| s >= 0.6))
                        .collect()
                };
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    let what = if self.search.trim().is_empty() {
                        format!("{} with relationships", results.len())
                    } else {
                        format!("ID or fuzzy name · {} matches", results.len())
                    };
                    ui.label(RichText::new(what).size(11.0).color(p.muted));
                });
                ui.add_space(4.0);
                let detail_h = 120.0;
                egui::ScrollArea::vertical()
                    .id_salt("relationships_results")
                    .max_height((height - 100.0 - detail_h).max(80.0))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for (score, c) in results {
                            let selected = self.selected.as_deref() == Some(c.id.as_str());
                            let row = Frame::new()
                                .fill(if selected {
                                    p.accent.gamma_multiply(0.08)
                                } else {
                                    egui::Color32::TRANSPARENT
                                })
                                .inner_margin(Margin::symmetric(12, 8))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        let name = ui.add(
                                            egui::Label::new(&c.name)
                                                .truncate()
                                                .sense(egui::Sense::click()),
                                        );
                                        if name.clicked() {
                                            self.selected = Some(c.id.clone());
                                        }
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                if let Some(s) = score {
                                                    ui.label(
                                                        RichText::new(format!("{s:.2}"))
                                                            .size(11.0)
                                                            .color(p.muted),
                                                    );
                                                }
                                                let n = state.relationships.related_count(&c.id);
                                                if n > 0 {
                                                    pill(
                                                        ui,
                                                        PillKind::Neutral,
                                                        format!("{n} related"),
                                                    );
                                                }
                                                ui.label(mono(&c.id).color(p.muted));
                                            },
                                        );
                                    });
                                });
                            let resp = ui.interact(
                                row.response.rect,
                                egui::Id::unique(("rel_result", &c.id)),
                                egui::Sense::click(),
                            );
                            if resp.clicked() {
                                self.selected = Some(c.id.clone());
                            }
                            ui.painter().hline(
                                row.response.rect.x_range(),
                                row.response.rect.bottom(),
                                Stroke::new(1.0, p.border.gamma_multiply(0.6)),
                            );
                        }
                    });
                // Selected client panel.
                ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                    Frame::new()
                        .fill(p.tint_copy)
                        .inner_margin(Margin::same(12))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            // The panel is pinned with a bottom-up layout; its content reads top-down.
                            ui.with_layout(Layout::top_down(Align::Min), |ui| {
                                label_caps(ui, "Selected");
                                match self.selected.as_deref().and_then(|id| list.match_by_id(id)) {
                                    Some(c) => {
                                        ui.label(strong(&c.name));
                                        ui.label(mono(format!("ID {}", c.id)).color(p.text));
                                        if !c.extra.is_empty() {
                                            let extras: Vec<String> = c
                                                .extra
                                                .iter()
                                                .filter(|(_, v)| !v.is_empty())
                                                .map(|(k, v)| format!("{k} {v}"))
                                                .collect();
                                            ui.label(
                                                RichText::new(extras.join(" · "))
                                                    .size(12.0)
                                                    .color(p.muted),
                                            );
                                        }
                                        let folder = state
                                            .folders
                                            .bound_folder(state.settings.root.as_deref(), &c.id)
                                            .map(|f| f.display().to_string());
                                        ui.label(
                                            RichText::new(match folder {
                                                Some(f) => format!("Folder bound · {f}"),
                                                None => "No folder".into(),
                                            })
                                            .size(12.0)
                                            .color(p.muted),
                                        );
                                    }
                                    None => {
                                        ui.label(RichText::new("Nobody selected").color(p.muted));
                                    }
                                }
                            });
                        });
                });
            });
    }

    #[allow(clippy::too_many_arguments)]
    fn side_ui(
        &mut self,
        ui: &mut egui::Ui,
        state: &AppState,
        list: &ClientList,
        id: &str,
        name: &str,
        side: Side,
        edits: &mut Vec<Edit>,
    ) {
        let p = pal(ui);
        let (title, note) = match side {
            Side::Children => (
                "Owners / related of this client",
                format!(" · {name} is the parent"),
            ),
            Side::Parents => (
                "This client is related to",
                format!(" · {name} is the child"),
            ),
        };
        let rows: Vec<(usize, &Relationship)> = state
            .relationships
            .records
            .iter()
            .enumerate()
            .filter(|(_, r)| match side {
                Side::Children => r.parent_id == id,
                Side::Parents => r.child_id == id,
            })
            .collect();
        // Role and actions fixed (Re-link… + Remove must fit); client and note share the rest.
        let widths = |w: f32| {
            let rest = (w - 130.0 - 160.0).max(200.0);
            [rest * 0.55, 130.0, rest * 0.45, 160.0]
        };
        Frame::new()
            .fill(p.card)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(CornerRadius::same(6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                Frame::new()
                    .inner_margin(Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            ui.label(strong(title));
                            ui.label(RichText::new(note).color(p.muted));
                            if side == Side::Children {
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.label(
                                        RichText::new("K-1 copies go to these")
                                            .size(11.0)
                                            .color(p.muted),
                                    );
                                });
                            }
                        });
                    });
                // Margins (2 × 12) and the gaps between the four cells.
                let total = ui.available_width() - 24.0 - 3.0 * 8.0;
                let w = widths(total);
                Frame::new()
                    .fill(p.tint_copy)
                    .inner_margin(Margin::symmetric(12, 5))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            for (width, t) in w.iter().zip(["Client", "Role", "Note", ""]) {
                                cell(ui, *width, |ui| {
                                    label_caps(ui, t);
                                });
                            }
                        });
                    });
                for (i, r) in rows {
                    let other = match side {
                        Side::Children => &r.child_id,
                        Side::Parents => &r.parent_id,
                    };
                    let client = list.match_by_id(other);
                    let dangling = client.is_none() || list.match_by_id(id).is_none();
                    let row = Frame::new()
                        .fill(if dangling {
                            p.tint_danger
                        } else {
                            egui::Color32::TRANSPARENT
                        })
                        .inner_margin(Margin::symmetric(12, 5))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                cell(ui, w[0], |ui| match client {
                                    Some(c) => {
                                        ui.label(&c.name);
                                        ui.label(mono(&c.id).color(p.muted));
                                    }
                                    None => {
                                        ui.label(
                                            RichText::new(format!(
                                                "ID {other} · not in master list"
                                            ))
                                            .color(p.pill(PillKind::Err).1),
                                        );
                                        pill(ui, PillKind::Err, "dangling");
                                    }
                                });
                                cell(ui, w[1], |ui| {
                                    if dangling {
                                        ui.label(RichText::new(&r.role).color(p.muted));
                                    } else {
                                        let mut role = r.role.clone();
                                        role_combo(
                                            ui,
                                            ("rel_role", i),
                                            &mut role,
                                            &format!("Role of {other}"),
                                        );
                                        if role != r.role {
                                            edits.push(Edit::Role(i, role));
                                        }
                                    }
                                });
                                cell(ui, w[2], |ui| {
                                    if dangling {
                                        ui.label(RichText::new(&r.note).color(p.muted));
                                    } else {
                                        let id_salt = egui::Id::unique(("rel_note", i));
                                        let mut note = ui
                                            .data(|d| d.get_temp::<String>(id_salt))
                                            .unwrap_or_else(|| r.note.clone());
                                        let edit = ui.add(
                                            egui::TextEdit::singleline(&mut note)
                                                .hint_text("note")
                                                .desired_width(f32::INFINITY),
                                        );
                                        let a11y = format!("Note for {other}");
                                        edit.widget_info(|| {
                                            egui::WidgetInfo::labeled(
                                                egui::Role::TextInput,
                                                true,
                                                a11y.clone(),
                                            )
                                        });
                                        if edit.has_focus() {
                                            ui.data_mut(|d| d.insert_temp(id_salt, note.clone()));
                                        } else {
                                            ui.data_mut(|d| d.remove::<String>(id_salt));
                                        }
                                        if edit.lost_focus() && note != r.note {
                                            edits.push(Edit::Note(i, note));
                                        }
                                    }
                                });
                                cell(ui, w[3], |ui| {
                                    if dangling {
                                        if ui.small_button("Re-link…").clicked() {
                                            edits.push(Edit::Relink(i, side));
                                        }
                                        if ui.small_button("Remove").clicked() {
                                            edits.push(Edit::Remove(i));
                                        }
                                    } else {
                                        let x = ui
                                            .small_button("×")
                                            .on_hover_text("Remove this relationship");
                                        let a11y = format!("Remove relationship with {other}");
                                        x.widget_info(|| {
                                            egui::WidgetInfo::labeled(
                                                egui::Role::Button,
                                                true,
                                                a11y.clone(),
                                            )
                                        });
                                        if x.clicked() {
                                            edits.push(Edit::Remove(i));
                                        }
                                    }
                                });
                            });
                        });
                    ui.painter().hline(
                        row.response.rect.x_range(),
                        row.response.rect.bottom(),
                        Stroke::new(1.0, p.border.gamma_multiply(0.6)),
                    );
                }
                // Add row.
                let add = match side {
                    Side::Children => &mut self.add_children,
                    Side::Parents => &mut self.add_parents,
                };
                Frame::new()
                    .inner_margin(Margin::symmetric(12, 6))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let mut submit = false;
                        ui.horizontal(|ui| {
                            cell(ui, w[0], |ui| {
                                let field = ui.add(
                                    egui::TextEdit::singleline(&mut add.query)
                                        .hint_text("Add client (ID or name)…")
                                        .desired_width(f32::INFINITY),
                                );
                                let a11y = match side {
                                    Side::Children => "Add a child client",
                                    Side::Parents => "Add a parent client",
                                };
                                field.widget_info(|| {
                                    egui::WidgetInfo::labeled(egui::Role::TextInput, true, a11y)
                                });
                                if field.changed() {
                                    add.chosen = None;
                                }
                                if field.lost_focus()
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter))
                                {
                                    submit = true;
                                }
                            });
                            cell(ui, w[1], |ui| {
                                let a11y = match side {
                                    Side::Children => "Role for the new child",
                                    Side::Parents => "Role for the new parent",
                                };
                                role_combo(ui, ("rel_add_role", side as u8), &mut add.role, a11y);
                            });
                            cell(ui, w[2], |ui| {
                                let note = ui.add(
                                    egui::TextEdit::singleline(&mut add.note)
                                        .hint_text("note")
                                        .desired_width(f32::INFINITY),
                                );
                                note.widget_info(|| {
                                    egui::WidgetInfo::labeled(
                                        egui::Role::TextInput,
                                        true,
                                        "Note for the new relationship",
                                    )
                                });
                                if note.lost_focus()
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter))
                                {
                                    submit = true;
                                }
                            });
                            cell(ui, w[3], |ui| {
                                ui.label(RichText::new("Enter").size(11.0).color(p.muted));
                            });
                        });
                        // Suggestions while typing (an exact ID first).
                        if add.chosen.is_none() && !add.query.trim().is_empty() {
                            let found = search_clients(list, &add.query, 6);
                            ui.horizontal_wrapped(|ui| {
                                for (score, c) in found.iter().filter(|(_, c)| c.id != id).take(6) {
                                    let text = match score {
                                        Some(s) => format!("{} {} · {s:.2}", c.name, c.id),
                                        None => format!("{} {} · ID match", c.name, c.id),
                                    };
                                    if ui.small_button(text).clicked() {
                                        add.query = format!("{} ({})", c.name, c.id);
                                        add.chosen = Some((*c).clone());
                                    }
                                }
                            });
                        }
                        if submit {
                            // Enter picks the chosen client, else an exact ID, else a close name.
                            let target = add.chosen.clone().or_else(|| {
                                let found = search_clients(list, &add.query, 1);
                                found.first().and_then(|(s, c)| {
                                    (s.is_none_or(|s| s >= f64::from(state.settings.fuzzy_suggest))
                                        && c.id != id)
                                        .then(|| (*c).clone())
                                })
                            });
                            match target {
                                Some(c) => {
                                    let (parent_id, child_id) = match side {
                                        Side::Children => (id.to_string(), c.id.clone()),
                                        Side::Parents => (c.id.clone(), id.to_string()),
                                    };
                                    edits.push(Edit::Add(Relationship {
                                        parent_id,
                                        child_id,
                                        role: add.role.clone(),
                                        note: add.note.trim().to_string(),
                                    }));
                                    let role = add.role.clone();
                                    *add = AddRow::new(&role);
                                }
                                None => {
                                    self.message = Some((
                                        Level::Error,
                                        "Pick a client from the suggestions, or type its ID."
                                            .into(),
                                    ));
                                }
                            }
                        }
                    });
            });
    }

    fn apply(&mut self, state: &mut AppState, edits: Vec<Edit>) {
        if edits.is_empty() {
            return;
        }
        let before = state.relationships.clone();
        let added = edits.iter().any(|e| matches!(e, Edit::Add(_)));
        for e in edits {
            match e {
                Edit::Role(i, role) => {
                    if let Some(r) = state.relationships.records.get_mut(i) {
                        r.role = role;
                    }
                }
                Edit::Note(i, note) => {
                    if let Some(r) = state.relationships.records.get_mut(i) {
                        r.note = note;
                    }
                }
                Edit::Remove(i) => {
                    if i < state.relationships.records.len() {
                        state.relationships.records.remove(i);
                    }
                }
                Edit::Add(r) => {
                    if let Err(e) = state.relationships.add(r) {
                        self.message = Some((Level::Error, e.to_string()));
                        return;
                    }
                }
                Edit::Relink(i, side) => {
                    let id = state.relationships.records.get(i).map(|r| match side {
                        Side::Children => r.child_id.clone(),
                        Side::Parents => r.parent_id.clone(),
                    });
                    if let Some(id) = id {
                        self.relink = Some((i, side, id));
                    }
                    return;
                }
            }
        }
        if let Err(e) = state.save_relationships() {
            state.relationships = before;
            self.message = Some((Level::Error, e));
        } else {
            self.message = None;
            if added && std::mem::take(&mut self.return_to_preview) {
                state.goto = Some(crate::app::Tab::Preview);
            }
        }
    }

    fn relink_ui(&mut self, ctx: &egui::Context, state: &mut AppState, list: &ClientList) {
        let Some((i, side, old)) = &mut self.relink else {
            return;
        };
        let title = format!("Re-link ID {old}");
        match client_picker(ctx, &title, old, list) {
            PickerOutcome::Open => {}
            PickerOutcome::Cancel => self.relink = None,
            PickerOutcome::NewClient(_) => {}
            PickerOutcome::Chosen(c) => {
                let (i, side) = (*i, *side);
                self.relink = None;
                let before = state.relationships.clone();
                if let Some(r) = state.relationships.records.get_mut(i) {
                    match side {
                        Side::Children => r.child_id = c.id.clone(),
                        Side::Parents => r.parent_id = c.id.clone(),
                    }
                }
                if let Err(e) = state.save_relationships() {
                    state.relationships = before;
                    self.message = Some((Level::Error, e));
                }
            }
        }
    }

    fn export(&mut self, state: &AppState) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("CSV", &["csv"])
            .set_file_name("relationships.csv")
            .save_file()
        else {
            return;
        };
        let result = state
            .relationships
            .to_csv()
            .map_err(|e| e.to_string())
            .and_then(|csv| std::fs::write(&path, csv).map_err(|e| e.to_string()));
        self.message = Some(match result {
            Ok(()) => (
                Level::Info,
                format!("Exported {} relationships.", state.relationships.len()),
            ),
            Err(e) => (Level::Error, format!("Could not export: {e}")),
        });
    }

    fn import_ui(&mut self, ctx: &egui::Context, state: &mut AppState, list: &ClientList) {
        let Some(import) = &mut self.import else {
            return;
        };
        match import.show(ctx, list) {
            ImportOutcome::Open => {}
            ImportOutcome::Cancel => self.import = None,
            ImportOutcome::Merge(rows, blank, unknown) => {
                self.import = None;
                let before = state.relationships.clone();
                let (added, dup) = state.relationships.merge(rows);
                if let Err(e) = state.save_relationships() {
                    state.relationships = before;
                    self.message = Some((Level::Error, e));
                    return;
                }
                let mut msg = format!(
                    "Imported {added} relationship{}",
                    if added == 1 { "" } else { "s" }
                );
                if dup > 0 {
                    msg.push_str(&format!(" · {dup} already there"));
                }
                if blank > 0 {
                    msg.push_str(&format!(" · {blank} incomplete rows skipped"));
                }
                if unknown > 0 {
                    msg.push_str(&format!(
                        " · {unknown} rows skipped (ID not in the client list)"
                    ));
                }
                self.message = Some((Level::Info, format!("{msg}.")));
            }
        }
    }
}

/// A role dropdown: the seeded roles plus the current one if it is custom.
fn role_combo(ui: &mut egui::Ui, id: impl egui::AsIdSalt, role: &mut String, a11y: &str) {
    let resp = egui::ComboBox::from_id_salt(id)
        .width(110.0)
        .selected_text(role.clone())
        .show_ui(ui, |ui| {
            if !ROLES.contains(&role.as_str()) && !role.is_empty() {
                let current = role.clone();
                ui.selectable_value(role, current.clone(), current);
            }
            for r in ROLES {
                ui.selectable_value(role, r.to_string(), r);
            }
        })
        .response;
    let name = a11y.to_string();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, name.clone()));
}

/// A fixed-width cell in a row.
fn cell(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 22.0),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_width(width);
            add(ui);
        },
    );
}

// ---------------------------------------------------------------------------
// CSV import

enum ImportOutcome {
    Open,
    Cancel,
    /// Rows to merge, incomplete rows, rows with unknown IDs.
    Merge(Vec<Relationship>, usize, usize),
}

struct Import {
    path: PathBuf,
    loading: Option<Receiver<Result<CsvTable, String>>>,
    table: Option<CsvTable>,
    error: Option<String>,
    cols: RelColumns,
}

impl Import {
    fn open(ctx: &egui::Context, path: PathBuf) -> Self {
        let p = path.clone();
        let rx = worker::background(ctx, move || {
            clients::read_csv(&p).map_err(|e| e.to_string())
        });
        Self {
            path,
            loading: Some(rx),
            table: None,
            error: None,
            cols: RelColumns::default(),
        }
    }

    fn show(&mut self, ctx: &egui::Context, list: &ClientList) -> ImportOutcome {
        if let Some(rx) = &self.loading
            && let Ok(result) = rx.try_recv()
        {
            self.loading = None;
            match result {
                Ok(t) => {
                    self.cols = RelColumns::guess(&t.headers);
                    self.table = Some(t);
                }
                Err(e) => self.error = Some(e),
            }
        }
        let mut outcome = ImportOutcome::Open;
        let modal = egui::Modal::new(egui::Id::unique("relationships_import"))
            .backdrop_color(egui::Color32::from_rgba_unmultiplied(27, 31, 36, 90))
            .show(ctx, |ui| {
                let p = pal(ui);
                ui.set_width(520.0);
                ui.label(strong("Import relationships").size(16.0));
                ui.label(mono(self.path.display().to_string()).color(p.muted));
                ui.add_space(8.0);
                if let Some(e) = &self.error {
                    banner(ui, Level::Error, &format!("Cannot read this CSV: {e}"));
                }
                let Some(table) = &self.table else {
                    if self.error.is_none() {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Reading CSV…");
                        });
                    }
                    if ui.button("Cancel").clicked() {
                        outcome = ImportOutcome::Cancel;
                    }
                    return;
                };
                egui::Grid::new("rel_import_cols")
                    .num_columns(2)
                    .spacing([16.0, 8.0])
                    .show(ui, |ui| {
                        for (label, value, optional) in [
                            ("Parent ID", &mut self.cols.parent, false),
                            ("Child ID", &mut self.cols.child, false),
                            ("Role", &mut self.cols.role, false),
                            ("Note", &mut self.cols.note, true),
                        ] {
                            let l = label_caps(ui, label);
                            egui::ComboBox::from_id_salt(("rel_col", label))
                                .width(180.0)
                                .selected_text(value.clone().unwrap_or_else(|| "(none)".into()))
                                .show_ui(ui, |ui| {
                                    if optional {
                                        ui.selectable_value(value, None, "(none)");
                                    }
                                    for h in table.headers.iter().filter(|h| !h.is_empty()) {
                                        ui.selectable_value(value, Some(h.clone()), h);
                                    }
                                })
                                .response
                                .labelled_by(l.id);
                            ui.end_row();
                        }
                    });
                let rows = relationships::rows_from_csv(table, &self.cols, list);
                ui.add_space(6.0);
                match &rows {
                    Ok(r) => {
                        ui.label(
                            RichText::new(format!(
                                "{} rows to merge · {} incomplete · {} with an ID not in the client list",
                                r.rows.len(),
                                r.blank,
                                r.unknown
                            ))
                            .size(12.0)
                            .color(p.muted),
                        );
                    }
                    Err(e) => banner(ui, Level::Error, e),
                }
                ui.label(
                    RichText::new("Merged into the existing relationships; nothing is replaced or removed.")
                        .size(12.0)
                        .color(p.muted),
                );
                ui.separator();
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let n = rows.as_ref().map_or(0, |r| r.rows.len());
                        if ui
                            .add_enabled(n > 0, primary_button(ui, format!("Merge {n} relationships")))
                            .clicked()
                            && let Ok(r) = rows
                        {
                            outcome = ImportOutcome::Merge(r.rows, r.blank, r.unknown);
                        }
                        if ui.button("Cancel").clicked() {
                            outcome = ImportOutcome::Cancel;
                        }
                    });
                });
            });
        if matches!(outcome, ImportOutcome::Open) && modal.should_close() {
            outcome = ImportOutcome::Cancel;
        }
        outcome
    }
}
