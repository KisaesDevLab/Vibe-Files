//! Conflicts tab (UI_SPEC §12): top-level folders under the root that are not cleanly bound to one
//! client, with stat tiles and per-row actions (Bind, Rebind, Keep first / second, Ignore).

use std::path::{Path, PathBuf};

use egui::{Align, CornerRadius, Frame, Layout, Margin, RichText, Stroke};

use crate::app::AppState;
use crate::clients::{Client, ClientList};
use crate::sentinel::{FolderBinding, Status};
use crate::ui::setup::banner;
use crate::ui::theme::{self, PillKind, label_caps, mono, pal, pill, primary_button, strong};
use crate::ui::widgets::{PickerOutcome, client_picker};

#[derive(Default)]
pub struct ConflictsScreen {
    picker: Option<Picker>,
    show_ignored: bool,
    /// Chosen candidate per fuzzy folder (client ID), when not the best one.
    fuzzy_choice: std::collections::HashMap<PathBuf, String>,
    /// "Bind all exact name matches" confirmation is open (V9).
    bind_all_open: bool,
    /// "Rename to template name" dialog (V9).
    rename: Option<Vec<RenameRow>>,
    rename_job: Option<std::sync::mpsc::Receiver<crate::sentinel::RenameReport>>,
    message: Option<(crate::config::Level, String)>,
}

/// One bound folder whose name differs from the template's client-folder name (V9).
struct RenameRow {
    client_id: String,
    from: PathBuf,
    to: PathBuf,
    /// The new name is taken: can't be ticked.
    taken: bool,
    tick: bool,
}

/// "Choose client…" dialog for one folder; `rebind` sets the old sentinel aside first.
struct Picker {
    folder: PathBuf,
    query: String,
    rebind: bool,
}

enum Action {
    Bind(PathBuf, Client),
    Pick(PathBuf, String, bool),
    Keep(PathBuf, Vec<PathBuf>),
    Ignore(Vec<PathBuf>),
    Unignore(PathBuf),
}

/// One table row: a folder, or every folder of one duplicate client.
enum Row<'a> {
    Single(&'a FolderBinding),
    Duplicate(Vec<&'a FolderBinding>),
}

impl ConflictsScreen {
    pub fn show(&mut self, ui: &mut egui::Ui, state: &mut AppState) {
        ui.add_space(4.0);
        let Some(root) = state.settings.root.clone() else {
            ui.label("Choose and save the client root in Setup first.");
            return;
        };
        let scan = state.folders.scan_of(Some(&root)).cloned();
        let ignored_count = scan.as_ref().map_or(0, |s| {
            s.folders
                .iter()
                .filter(|b| b.status == Status::Ignored)
                .count()
        });
        self.header_ui(ui, state, &root, scan.as_ref(), ignored_count);
        self.poll_rename(state);
        if let Some((level, msg)) = state.folders.note() {
            banner(ui, *level, msg);
        }
        if let Some((level, msg)) = &self.message {
            banner(ui, *level, msg);
        }
        let Some(scan) = scan else {
            ui.add_space(8.0);
            ui.label("This root has not been scanned yet. Click Rescan root.");
            return;
        };
        tiles_ui(ui, &scan.folders);
        ui.add_space(10.0);

        let clients = state.clients.list().cloned();
        let busy = state.folders.binding_in_progress();
        // Relationships per client ID: an orphan's are dangling too (UI_SPEC §12).
        let mut rel_counts: std::collections::HashMap<String, usize> = Default::default();
        for r in &state.relationships.records {
            *rel_counts.entry(r.parent_id.clone()).or_default() += 1;
            *rel_counts.entry(r.child_id.clone()).or_default() += 1;
        }
        let rows = rows(&scan.folders, self.show_ignored);
        let mut action = None;
        table_ui(ui, |ui| {
            if rows.is_empty() {
                ui.add_space(8.0);
                ui.label("Every folder is bound to a client.");
                ui.add_space(8.0);
            }
            for (i, row) in rows.iter().enumerate() {
                if i > 0 {
                    ui.separator();
                }
                self.row_ui(ui, row, clients.as_deref(), &rel_counts, busy, &mut action);
            }
        });

        let data_dir = state.data_dir.clone();
        match action {
            Some(Action::Bind(folder, client)) => {
                let firm = state.settings.firm_id.unwrap_or_default();
                state
                    .folders
                    .bind(ui.ctx(), data_dir, root.clone(), folder, client, firm);
            }
            Some(Action::Pick(folder, query, rebind)) => {
                self.picker = Some(Picker {
                    folder,
                    query,
                    rebind,
                });
            }
            Some(Action::Keep(keep, others)) => {
                state
                    .folders
                    .keep_duplicate(ui.ctx(), data_dir, root.clone(), keep, others);
            }
            Some(Action::Ignore(folders)) => {
                for f in folders {
                    state.folders.ignore(&data_dir, &f);
                }
            }
            Some(Action::Unignore(folder)) => state.folders.unignore(&data_dir, &folder),
            None => {}
        }
        self.picker_ui(ui.ctx(), state, &root);
        self.bind_all_ui(ui.ctx(), state, &root);
        self.rename_ui(ui.ctx(), state, &root);
    }

    /// Unbound folders that name exactly one client (ID in the name, or the exact name), whose
    /// client has no folder yet (V9).
    fn exact_matches(state: &AppState, root: &Path) -> Vec<(PathBuf, Client)> {
        let (Some(scan), Some(list)) = (state.folders.scan_of(Some(root)), state.clients.list())
        else {
            return Vec::new();
        };
        let bound: Vec<&str> = scan
            .folders
            .iter()
            .filter(|b| b.status == Status::Bound)
            .filter_map(|b| b.client_id.as_deref())
            .collect();
        let mut out: Vec<(PathBuf, Client)> = scan
            .folders
            .iter()
            .filter(|b| {
                b.status == Status::UnboundNamed
                    || (b.status == Status::UnboundFuzzy && b.score.is_some_and(|s| s >= 0.9999))
            })
            .filter_map(|b| {
                let id = b.client_id.as_deref()?;
                if bound.contains(&id) {
                    return None;
                }
                Some((b.path.clone(), list.match_by_id(id)?.clone()))
            })
            .collect();
        // A client two folders point at is left for the user.
        let ids: Vec<String> = out.iter().map(|(_, c)| c.id.clone()).collect();
        out.retain(|(_, c)| ids.iter().filter(|i| **i == c.id).count() == 1);
        out
    }

    /// Bound folders under the root whose name isn't the template's client-folder name (V9).
    /// `check_disk` looks whether each new name is taken: a file-system call per folder, so the
    /// header (every frame, maybe a UNC root) leaves it off and the dialog turns it on.
    fn rename_rows(state: &AppState, root: &Path, check_disk: bool) -> Vec<RenameRow> {
        let (Some(scan), Some(list)) = (state.folders.scan_of(Some(root)), state.clients.list())
        else {
            return Vec::new();
        };
        let mut rows: Vec<RenameRow> = scan
            .folders
            .iter()
            .filter(|b| b.status == Status::Bound && b.path.parent() == Some(root))
            .filter_map(|b| {
                let client = list.match_by_id(b.client_id.as_deref()?)?;
                let name = crate::template::client_folder_name(&state.template, client).ok()?;
                let current = b.path.file_name()?.to_string_lossy().into_owned();
                if current.eq_ignore_ascii_case(&name) {
                    return None;
                }
                let to = root.join(&name);
                let taken = check_disk && to.exists();
                Some(RenameRow {
                    client_id: client.id.clone(),
                    from: b.path.clone(),
                    to,
                    taken,
                    tick: !taken,
                })
            })
            .collect();
        rows.sort_by_key(|r| r.from.to_string_lossy().to_lowercase());
        rows
    }

    fn bind_all_ui(&mut self, ctx: &egui::Context, state: &mut AppState, root: &Path) {
        if !self.bind_all_open {
            return;
        }
        let pairs = Self::exact_matches(state, root);
        let mut go = false;
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("bind_all")).show(ctx, |ui| {
            let p = pal(ui);
            ui.set_width(620.0);
            ui.heading(format!("Bind {} folder{}?", pairs.len(), if pairs.len() == 1 { "" } else { "s" }));
            ui.label(
                RichText::new(
                    "Writes _Vibe\\client.json into each folder. Nothing is moved or renamed. \
                     Folders whose name is only close to a client's stay in the list for one click each.",
                )
                .color(p.muted),
            );
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                egui::Grid::new("bind_all_list").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                    for (folder, client) in &pairs {
                        ui.label(mono(folder.file_name().unwrap_or_default().to_string_lossy()));
                        ui.label(format!("→ {} ({})", client.name, client.id));
                        ui.end_row();
                    }
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let label = format!("Bind {} folders", pairs.len());
                if ui.add_enabled(!pairs.is_empty(), primary_button(ui, label)).clicked() {
                    go = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if go {
            let firm = state.settings.firm_id.unwrap_or_default();
            let data_dir = state.data_dir.clone();
            state
                .folders
                .bind_many(ctx, data_dir, root.to_path_buf(), pairs, firm);
            self.bind_all_open = false;
        } else if close || modal.should_close() {
            self.bind_all_open = false;
        }
    }

    fn rename_ui(&mut self, ctx: &egui::Context, state: &mut AppState, root: &Path) {
        let Some(rows) = &mut self.rename else { return };
        let mut go = false;
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("rename_folders")).show(ctx, |ui| {
            let p = pal(ui);
            ui.set_width(720.0);
            ui.heading("Rename client folders to the template name");
            banner(
                ui,
                crate::config::Level::Error,
                "Shortcuts, mapped paths or other programs that point at the old folder names stop working after the rename.",
            );
            if crate::undo::blocker(state.last_batch.as_ref()).is_none() {
                banner(
                    ui,
                    crate::config::Level::Error,
                    "The rename becomes the last batch: the commit before it can no longer be undone.",
                );
            }
            ui.label(
                RichText::new(format!(
                    "The template's client folder name is {} (Setup). Undo last batch renames them back.",
                    state.template.name
                ))
                .size(12.0)
                .color(p.muted),
            );
            ui.add_space(6.0);
            if rows.is_empty() {
                ui.label("Every bound folder already has the template's name.");
            }
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                for r in rows.iter_mut() {
                    ui.horizontal(|ui| {
                        let old = r.from.file_name().unwrap_or_default().to_string_lossy().into_owned();
                        let new = r.to.file_name().unwrap_or_default().to_string_lossy().into_owned();
                        ui.add_enabled(!r.taken, egui::Checkbox::without_text(&mut r.tick))
                            .widget_info(|| {
                                egui::WidgetInfo::selected(
                                    egui::Role::CheckBox,
                                    !r.taken,
                                    r.tick,
                                    format!("Rename {old}"),
                                )
                            });
                        ui.label(mono(&old));
                        ui.label(RichText::new("→").color(p.muted));
                        ui.label(mono(&new));
                        if r.taken {
                            pill(ui, PillKind::Err, "name taken")
                                .on_hover_text("A folder with the new name already exists; this one is left as it is.");
                        }
                    });
                }
            });
            ui.add_space(8.0);
            let n = rows.iter().filter(|r| r.tick && !r.taken).count();
            ui.horizontal(|ui| {
                let label = format!("Rename {n} folder{}", if n == 1 { "" } else { "s" });
                let blocked = state.committing;
                let b = ui.add_enabled(n > 0 && !blocked, primary_button(ui, label));
                let b = if blocked {
                    b.on_disabled_hover_text("Wait for the commit to finish.")
                } else {
                    b
                };
                if b.clicked() {
                    go = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if go {
            let renames: Vec<(String, PathBuf, PathBuf)> = rows
                .iter()
                .filter(|r| r.tick && !r.taken)
                .map(|r| (r.client_id.clone(), r.from.clone(), r.to.clone()))
                .collect();
            let (data_dir, root) = (state.data_dir.clone(), root.to_path_buf());
            self.rename_job = Some(crate::worker::background(ctx, move || {
                crate::sentinel::rename_folders(&data_dir, &root, &renames)
            }));
            self.rename = None;
        } else if close || modal.should_close() {
            self.rename = None;
        }
    }

    fn poll_rename(&mut self, state: &mut AppState) {
        let Some(rx) = &self.rename_job else { return };
        let Ok(report) = rx.try_recv() else { return };
        self.rename_job = None;
        let moves: Vec<(PathBuf, PathBuf)> = report
            .done
            .iter()
            .map(|r| (r.from.clone(), r.to.clone()))
            .collect();
        let data_dir = state.data_dir.clone();
        state.folders.follow_renames(&data_dir, &moves);
        if !report.done.is_empty() {
            state.last_batch = crate::commit::load_last_batch(&data_dir);
        }
        state.ops_generation += 1;
        let n = report.done.len();
        self.message = Some(if report.failed.is_empty() {
            (
                crate::config::Level::Info,
                format!(
                    "Renamed {n} folder{}. Undo last batch in History renames them back.",
                    if n == 1 { "" } else { "s" }
                ),
            )
        } else {
            (
                crate::config::Level::Error,
                format!(
                    "Renamed {n}; not renamed: {}",
                    report
                        .failed
                        .iter()
                        .map(|(p, e)| format!("{} ({e})", p.display()))
                        .collect::<Vec<_>>()
                        .join("; ")
                ),
            )
        });
    }

    fn header_ui(
        &mut self,
        ui: &mut egui::Ui,
        state: &mut AppState,
        root: &Path,
        scan: Option<&crate::sentinel::FoldersFile>,
        ignored: usize,
    ) {
        let p = pal(ui);
        ui.horizontal(|ui| {
            ui.label(strong("Folders that are not bound to a client").size(15.0));
            if let Some(s) = scan {
                let local = s.scanned_at.with_timezone(&chrono::Local);
                let when = if local.date_naive() == chrono::Local::now().date_naive() {
                    format!("today {}", local.format("%-I:%M %p"))
                } else {
                    local.format("%b %-d, %-I:%M %p").to_string()
                };
                ui.label(
                    RichText::new(format!(" · from the scan of {} {when}", root.display()))
                        .size(12.0)
                        .color(p.muted),
                );
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let blocker = state.scan_blocker();
                let b = ui.add_enabled(blocker.is_none(), egui::Button::new("Rescan root"));
                let b = match blocker {
                    Some(why) => b.on_disabled_hover_text(why),
                    None => b,
                };
                if b.clicked() {
                    state.start_scan(ui.ctx());
                }
                if let Some((done, total)) = state.folders.scanning() {
                    ui.label(format!("Scanning… {done}/{total}"));
                    ui.spinner();
                }
                ui.checkbox(&mut self.show_ignored, format!("Show ignored ({ignored})"));
            });
        });
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Folders named without a sentinel or \"(ID)\":")
                    .size(12.0)
                    .color(p.muted),
            );
            // A scan finishing after a rename would put the old folder names back in the index.
            let busy = state.folders.binding_in_progress()
                || state.folders.scanning().is_some()
                || self.rename_job.is_some();
            let exact = Self::exact_matches(state, root).len();
            if ui
                .add_enabled(
                    !busy && exact > 0,
                    egui::Button::new(format!("Bind all exact name matches ({exact})")),
                )
                .on_hover_text("Write a sentinel into every folder that names exactly one client.")
                .on_disabled_hover_text("No unbound folder names exactly one client.")
                .clicked()
            {
                self.bind_all_open = true;
            }
            let renames = Self::rename_rows(state, root, false);
            if ui
                .add_enabled(
                    !busy && !renames.is_empty(),
                    egui::Button::new(format!("Rename to template name ({})…", renames.len())),
                )
                .on_hover_text(
                    "Rename bound client folders to the template's name, e.g. add \"(ID)\".",
                )
                .on_disabled_hover_text("Every bound folder already has the template's name.")
                .clicked()
            {
                self.rename = Some(Self::rename_rows(state, root, true));
            }
        });
        ui.add_space(6.0);
    }

    fn row_ui(
        &mut self,
        ui: &mut egui::Ui,
        row: &Row<'_>,
        clients: Option<&ClientList>,
        rel_counts: &std::collections::HashMap<String, usize>,
        busy: bool,
        action: &mut Option<Action>,
    ) {
        let p = pal(ui);
        let widths = column_widths(ui.available_width());
        let bg = ui.painter().add(egui::Shape::Noop);
        let row_resp = ui.horizontal(|ui| {
            ui.set_min_height(40.0);
            cell(ui, widths[0], |ui| {
                let (kind, text) = match row {
                    Row::Duplicate(_) => (PillKind::Err, "Duplicate"),
                    Row::Single(b) => match b.status {
                        Status::UnboundNamed => (PillKind::Warn, "Unbound · named"),
                        Status::UnboundFuzzy => (PillKind::Warn, "Unbound · fuzzy"),
                        Status::Orphan => (PillKind::Err, "Orphan"),
                        Status::Unparseable => (PillKind::Err, "Unparseable"),
                        Status::Ignored => (PillKind::Neutral, "Ignored"),
                        _ => (PillKind::Neutral, "No match"),
                    },
                };
                pill(ui, kind, text);
            });
            cell(ui, widths[1], |ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    match row {
                        Row::Single(b) => {
                            ui.label(mono(b.name()).color(p.text));
                            if let Some(note) = folder_note(b, rel_counts) {
                                ui.label(RichText::new(note).size(11.0).color(p.muted));
                            }
                        }
                        Row::Duplicate(group) => {
                            for b in group {
                                ui.label(mono(b.name()).color(p.text));
                            }
                            let id = group[0].client_id.clone().unwrap_or_default();
                            let who = if group.len() == 2 {
                                "Both".to_string()
                            } else {
                                format!("All {}", group.len())
                            };
                            ui.label(
                                RichText::new(format!(
                                    "{who} carry sentinel client_id {id} · routes and copies to {id} are paused until resolved"
                                ))
                                .size(11.0)
                                .color(p.muted),
                            );
                        }
                    }
                });
            });
            cell(ui, widths[2], |ui| self.candidate_ui(ui, row, clients));
            cell(ui, widths[3], |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    ui.horizontal_wrapped(|ui| self.actions_ui(ui, row, clients, action));
                });
            });
        });
        if matches!(row, Row::Duplicate(_)) {
            let r = row_resp.response.rect.expand2(egui::vec2(10.0, 4.0));
            ui.painter()
                .set(bg, egui::Shape::rect_filled(r, 0.0, p.tint_danger));
        }
    }

    fn candidate_ui(&mut self, ui: &mut egui::Ui, row: &Row<'_>, clients: Option<&ClientList>) {
        let p = pal(ui);
        let client = |id: Option<&str>| id.and_then(|id| clients.and_then(|c| c.match_by_id(id)));
        match row {
            Row::Duplicate(group) => {
                if let Some(c) = client(group[0].client_id.as_deref()) {
                    client_label(ui, c);
                }
            }
            Row::Single(b) => match b.status {
                Status::UnboundNamed => {
                    if let Some(c) = client(b.client_id.as_deref()) {
                        client_label(ui, c);
                        pill(ui, PillKind::Ok, "ID match");
                    }
                }
                Status::UnboundFuzzy => {
                    let Some(list) = clients else { return };
                    let options = fuzzy_options(list, b);
                    let chosen = self
                        .fuzzy_choice
                        .get(&b.path)
                        .cloned()
                        .or_else(|| b.client_id.clone())
                        .unwrap_or_default();
                    let text = options
                        .iter()
                        .find(|(_, c)| c.id == chosen)
                        .map(|(s, c)| format!("{} · {s:.2}", c.name))
                        .unwrap_or_else(|| "Other…".into());
                    let mut pick = chosen.clone();
                    egui::ComboBox::from_id_salt(("fuzzy", &b.path))
                        .width(220.0)
                        .selected_text(text)
                        .show_ui(ui, |ui| {
                            for (s, c) in &options {
                                ui.selectable_value(
                                    &mut pick,
                                    c.id.clone(),
                                    format!("{} · {s:.2}", c.name),
                                );
                            }
                            ui.selectable_value(&mut pick, OTHER.to_string(), "Other…");
                        });
                    if pick != chosen {
                        self.fuzzy_choice.insert(b.path.clone(), pick);
                    }
                }
                Status::Orphan | Status::Unparseable => {
                    ui.label(
                        RichText::new("Client may have been removed from the master list")
                            .color(p.muted),
                    );
                }
                _ => {}
            },
        }
    }

    fn actions_ui(
        &mut self,
        ui: &mut egui::Ui,
        row: &Row<'_>,
        clients: Option<&ClientList>,
        action: &mut Option<Action>,
    ) {
        let client = |id: &str| clients.and_then(|c| c.match_by_id(id)).cloned();
        match row {
            Row::Duplicate(group) => {
                for (i, b) in group.iter().enumerate() {
                    let label = match i {
                        0 => "Keep first".to_string(),
                        1 => "Keep second".to_string(),
                        n => format!("Keep #{}", n + 1),
                    };
                    let button = if i == 0 {
                        primary_button(ui, label)
                    } else {
                        egui::Button::new(label)
                    };
                    if ui
                        .add(button)
                        .on_hover_text(b.path.display().to_string())
                        .clicked()
                    {
                        let others = group
                            .iter()
                            .filter(|o| o.path != b.path)
                            .map(|o| o.path.clone())
                            .collect();
                        *action = Some(Action::Keep(b.path.clone(), others));
                    }
                }
                if ui.button("Ignore").clicked() {
                    *action = Some(Action::Ignore(
                        group.iter().map(|b| b.path.clone()).collect(),
                    ));
                }
            }
            Row::Single(b) => {
                let query = strip_id_suffix(&b.name());
                match b.status {
                    Status::UnboundNamed => {
                        if let Some(c) = b.client_id.as_deref().and_then(client)
                            && ui
                                .add(primary_button(ui, "Bind · write sentinel"))
                                .clicked()
                        {
                            *action = Some(Action::Bind(b.path.clone(), c));
                        }
                    }
                    Status::UnboundFuzzy => {
                        let chosen = self
                            .fuzzy_choice
                            .get(&b.path)
                            .cloned()
                            .or_else(|| b.client_id.clone());
                        if ui.add(primary_button(ui, "Bind")).clicked() {
                            *action = match chosen.as_deref() {
                                Some(OTHER) | None => {
                                    Some(Action::Pick(b.path.clone(), query.clone(), false))
                                }
                                Some(id) => client(id).map(|c| Action::Bind(b.path.clone(), c)),
                            };
                        }
                    }
                    Status::Orphan | Status::Unparseable => {
                        if ui.button("Rebind to…").clicked() {
                            *action = Some(Action::Pick(b.path.clone(), query.clone(), true));
                        }
                    }
                    Status::Ignored => {
                        if ui.button("Stop ignoring").clicked() {
                            *action = Some(Action::Unignore(b.path.clone()));
                        }
                        return;
                    }
                    _ => {
                        if ui.button("Choose client…").clicked() {
                            *action = Some(Action::Pick(b.path.clone(), query.clone(), false));
                        }
                    }
                }
                if ui.button("Ignore").clicked() {
                    *action = Some(Action::Ignore(vec![b.path.clone()]));
                }
            }
        }
    }

    fn picker_ui(&mut self, ctx: &egui::Context, state: &mut AppState, root: &Path) {
        let Some(picker) = &mut self.picker else {
            return;
        };
        let Some(clients) = state.clients.list().cloned() else {
            self.picker = None;
            return;
        };
        let name = picker
            .folder
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let title = if picker.rebind {
            format!("Rebind \"{name}\" to a client")
        } else {
            format!("Bind \"{name}\" to a client")
        };
        match client_picker(ctx, &title, &mut picker.query, &clients) {
            PickerOutcome::Open => {}
            PickerOutcome::Cancel => self.picker = None,
            PickerOutcome::Chosen(client) => {
                let (folder, rebind) = (picker.folder.clone(), picker.rebind);
                self.picker = None;
                let firm = state.settings.firm_id.unwrap_or_default();
                let data_dir = state.data_dir.clone();
                let root = root.to_path_buf();
                if rebind {
                    state
                        .folders
                        .rebind(ctx, data_dir, root, folder, client, firm);
                } else {
                    state
                        .folders
                        .bind(ctx, data_dir, root, folder, client, firm);
                }
            }
        }
    }
}

const OTHER: &str = "\u{0}other";

/// Rows in scan order; duplicate folders of one client grouped into one row.
fn rows(folders: &[FolderBinding], show_ignored: bool) -> Vec<Row<'_>> {
    let mut out: Vec<Row<'_>> = Vec::new();
    let mut seen_dup: Vec<&str> = Vec::new();
    for b in folders {
        match b.status {
            Status::Bound => {}
            Status::Ignored if !show_ignored => {}
            Status::Duplicate => {
                let id = b.client_id.as_deref().unwrap_or("");
                if seen_dup.contains(&id) {
                    continue;
                }
                seen_dup.push(id);
                out.push(Row::Duplicate(
                    folders
                        .iter()
                        .filter(|o| {
                            o.status == Status::Duplicate && o.client_id.as_deref() == Some(id)
                        })
                        .collect(),
                ));
            }
            _ => out.push(Row::Single(b)),
        }
    }
    out
}

/// Four stat tiles (UI_SPEC §12).
fn tiles_ui(ui: &mut egui::Ui, folders: &[FolderBinding]) {
    let p = pal(ui);
    let count = |f: &dyn Fn(Status) -> bool| folders.iter().filter(|b| f(b.status)).count();
    let bound = count(&|s| s == Status::Bound);
    let unbound = count(&|s| matches!(s, Status::UnboundNamed | Status::UnboundFuzzy));
    let orphan = count(&|s| matches!(s, Status::Orphan | Status::Unparseable));
    let mut dup_ids: Vec<&str> = folders
        .iter()
        .filter(|b| b.status == Status::Duplicate)
        .filter_map(|b| b.client_id.as_deref())
        .collect();
    dup_ids.sort_unstable();
    dup_ids.dedup();
    let tiles = [
        (
            bound,
            "Bound · sentinel matches master",
            p.pill(PillKind::Ok).1,
        ),
        (
            unbound,
            "Unbound · ID or name found",
            p.pill(PillKind::Warn).1,
        ),
        (
            orphan,
            "Orphan · sentinel ID not in master",
            p.pill(PillKind::Err).1,
        ),
        (
            dup_ids.len(),
            "Duplicate · same sentinel in two folders",
            p.pill(PillKind::Err).1,
        ),
    ];
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        for (n, label, color) in tiles {
            Frame::new()
                .fill(p.card)
                .stroke(Stroke::new(1.0, p.border))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::symmetric(14, 10))
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(strong(theme::thousands(n)).size(20.0).color(color));
                        ui.label(RichText::new(label).size(11.0).color(p.muted));
                    });
                });
        }
    });
}

/// The white table card with its header row and footnote.
fn table_ui(ui: &mut egui::Ui, body: impl FnOnce(&mut egui::Ui)) {
    let p = pal(ui);
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            Frame::new()
                .fill(p.tint_copy)
                .inner_margin(Margin::symmetric(10, 2))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let widths = column_widths(ui.available_width());
                    ui.horizontal(|ui| {
                        for (w, t) in
                            widths
                                .iter()
                                .zip(["Status", "Folder", "Candidate client", "Action"])
                        {
                            cell(ui, *w, |ui| {
                                label_caps(ui, t);
                            });
                        }
                    });
                });
            egui::ScrollArea::vertical()
                .id_salt("conflicts_rows")
                .max_height((ui.available_height() - 50.0).max(120.0))
                .show(ui, |ui| {
                    Frame::new()
                        .inner_margin(Margin::symmetric(10, 4))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            body(ui);
                        });
                });
            Frame::new()
                .fill(p.tint_copy)
                .inner_margin(Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        let small = |t: &str| RichText::new(t).size(12.0).color(p.muted);
                        ui.label(small("Bind writes "));
                        ui.label(mono("_Vibe\\client.json").size(11.0).color(p.muted));
                        ui.label(small(
                            " (T&B-compatible v1: client_id, firm_id, tax_software_id, \
                             display_name_at_creation, created_at). No folder or document is \
                             moved or renamed here.",
                        ));
                    });
                });
        });
}

/// Column widths for a row `total` wide, leaving room for the gaps between the four cells.
fn column_widths(total: f32) -> [f32; 4] {
    let status = 150.0;
    let rest = (total - status - 3.0 * 8.0).max(400.0);
    [status, rest * 0.42, rest * 0.30, rest * 0.28]
}

/// A fixed-width table cell.
fn cell(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 0.0),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_width(width);
            add(ui);
        },
    );
}

fn client_label(ui: &mut egui::Ui, c: &Client) {
    let p = pal(ui);
    ui.spacing_mut().item_spacing.x = 4.0;
    ui.label(&c.name);
    ui.label(mono(&c.id).color(p.muted));
}

/// The muted line under a folder name.
fn folder_note(
    b: &FolderBinding,
    rel_counts: &std::collections::HashMap<String, usize>,
) -> Option<String> {
    let extra = b
        .note
        .as_ref()
        .map(|n| format!(" · {n}"))
        .unwrap_or_default();
    match b.status {
        Status::UnboundNamed => Some(format!(
            "ID {} found in folder name · no _Vibe\\client.json{extra}",
            b.client_id.as_deref().unwrap_or("?"),
        )),
        Status::UnboundFuzzy => Some(format!("No ID in name · best name match shown{extra}")),
        Status::Orphan => {
            let id = b.client_id.as_deref().unwrap_or("?");
            let n = rel_counts.get(id).copied().unwrap_or(0);
            let mut text = format!("Sentinel says client_id {id} · not in current CSV");
            if n > 0 {
                text.push_str(&format!(
                    " · {n} relationship{} also dangling",
                    if n == 1 { "" } else { "s" }
                ));
            }
            Some(text)
        }
        _ => b.note.clone(),
    }
}

/// Best name matches for a fuzzy folder, the scan's candidate first.
fn fuzzy_options<'a>(list: &'a ClientList, b: &FolderBinding) -> Vec<(f64, &'a Client)> {
    let query = strip_id_suffix(&b.name());
    let mut out = list.fuzzy_by_name(&query, 5);
    if let Some(id) = &b.client_id
        && let Some(pos) = out.iter().position(|(_, c)| &c.id == id)
    {
        let first = out.remove(pos);
        out.insert(0, first);
    }
    out
}

/// Folder name without a trailing `(1234)` / `_1234`, as a starting search.
fn strip_id_suffix(name: &str) -> String {
    let trimmed = name.trim_end();
    if let Some(open) = trimmed.rfind('(')
        && trimmed.ends_with(')')
    {
        return trimmed[..open].trim_end().to_string();
    }
    if let Some(us) = trimmed.rfind('_')
        && trimmed[us + 1..].chars().all(|c| c.is_ascii_digit())
        && us + 1 < trimmed.len()
    {
        return trimmed[..us].to_string();
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::strip_id_suffix;

    #[test]
    fn search_starts_from_folder_name() {
        assert_eq!(strip_id_suffix("Smith, John (1234)"), "Smith, John");
        assert_eq!(strip_id_suffix("Acme_1002"), "Acme");
        assert_eq!(strip_id_suffix("Random Stuff"), "Random Stuff");
        assert_eq!(strip_id_suffix("Under_score"), "Under_score");
    }
}
