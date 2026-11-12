//! Setup tab (UI_SPEC §5). Edits a draft of the settings and the folder template; one Save writes
//! both, and is enabled only when the draft has no errors.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

use egui::{Align, CornerRadius, Frame, Layout, Margin, RichText, Sense, Stroke};

use crate::app::{AppState, Tab};
use crate::clients;
use crate::config::{self, FUZZY_THRESHOLD_RANGE, Issue, Level, Settings, ThemeChoice, ZOOM_RANGE};
use crate::sentinel::Status;
use crate::template::{self, BUILTIN_TOKENS, TemplateNode};
use crate::ui::csv_wizard::{CsvWizard, Outcome};
use crate::ui::theme::{self, PillKind, banner, label_caps, mono, pal, pill, strong, token_pill};
use crate::worker;

type PathKey = (Option<PathBuf>, Vec<PathBuf>);

pub struct SetupScreen {
    draft: Settings,
    template_draft: TemplateNode,
    /// Root + inboxes the latest path check was started for.
    requested: Option<PathKey>,
    /// The latest path check while it runs (an older one still running is dropped).
    pending: Option<Receiver<(PathKey, Vec<Issue>)>>,
    /// Result of the newest finished path check, with the key it was run on.
    path_issues: Option<(PathKey, Vec<Issue>)>,
    message: Option<(Level, String)>,
    wizard: Option<CsvWizard>,
    /// Folder template node the buttons act on (path of child indexes; empty = client folder).
    selected_node: Vec<usize>,
    /// Node being renamed and the text typed so far.
    renaming: Option<(Vec<usize>, String)>,
    /// "Also file" as typed (V25); read into the draft's file types as it changes.
    file_types_text: String,
}

/// The file types as Setup shows them: `docx, xlsx, jpg`.
fn file_types_text(types: &[String]) -> String {
    types.join(", ")
}

/// Typed file types: split on commas and spaces, dots and case dropped, each once.
pub fn parse_file_types(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in text.split([',', ' ', ';']) {
        let t = t.trim().trim_start_matches('.').to_ascii_lowercase();
        if !t.is_empty() && !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

impl SetupScreen {
    pub fn new(state: &AppState) -> Self {
        Self {
            draft: state.settings.clone(),
            template_draft: state.template.clone(),
            requested: None,
            pending: None,
            path_issues: None,
            message: None,
            wizard: None,
            selected_node: Vec::new(),
            renaming: None,
            file_types_text: file_types_text(&state.settings.file_types),
        }
    }

    fn path_key(&self) -> PathKey {
        (self.draft.root.clone(), self.draft.inboxes.clone())
    }

    /// Starts a background path check when root/inboxes changed; collects finished results.
    fn poll_path_check(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.pending
            && let Ok(result) = rx.try_recv()
        {
            self.path_issues = Some(result);
            self.pending = None;
        }
        let key = self.path_key();
        if self.requested.as_ref() != Some(&key) {
            self.requested = Some(key.clone());
            self.pending = Some(worker::background(ctx, move || {
                let issues = config::check_paths(key.0.as_deref(), &key.1);
                (key, issues)
            }));
        }
    }

    #[cfg(test)]
    pub fn wizard_open(&self) -> bool {
        self.wizard.is_some()
    }

    #[cfg(test)]
    pub fn path_check_done(&self) -> bool {
        self.current_issues().is_some()
    }

    #[cfg(test)]
    pub fn template_child_names(&self) -> Vec<String> {
        self.template_draft
            .children
            .iter()
            .map(|c| c.name.clone())
            .collect()
    }

    /// Setup has edits that are not saved yet.
    pub fn is_dirty(&self, state: &AppState) -> bool {
        !self.draft.same_setup(&state.settings) || self.template_draft != state.template
    }

    /// The draft has been fully checked and has no errors.
    pub fn can_save(&self) -> bool {
        self.current_issues()
            .is_some_and(|l| l.iter().all(|i| i.level != Level::Error))
    }

    /// `None` while the path check for the current draft is still running.
    fn current_issues(&self) -> Option<Vec<Issue>> {
        let (key, path_issues) = self.path_issues.as_ref()?;
        if *key != self.path_key() {
            return None;
        }
        let mut all = path_issues.clone();
        all.extend(config::check_fields(&self.draft));
        let tokens = clients::extra_tokens(&self.draft.column_map);
        all.extend(
            template::validate_tree(&self.template_draft, &tokens)
                .into_iter()
                .map(|e| Issue::error(format!("Folder template: {e}"))),
        );
        Some(all)
    }

    /// The finished path check found the drafted root usable (exists, is a folder, writable).
    fn root_writable(&self) -> bool {
        let Some((key, issues)) = &self.path_issues else {
            return false;
        };
        key.0.is_some()
            && key.0 == self.draft.root
            && !issues.iter().any(|i| {
                i.level == Level::Error
                    && (i.msg.starts_with("Client root ")
                        || i.msg.starts_with("Choose the client root"))
            })
    }

    /// `inbox` is on a different drive or share than the drafted root (copy, verify, delete).
    fn cross_drive(&self, inbox: &Path) -> bool {
        let Some(root) = &self.draft.root else {
            return false;
        };
        match (config::volume_key(root), config::volume_key(inbox)) {
            (Some(a), Some(b)) => a != b,
            _ => false,
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, state: &mut AppState) {
        self.poll_path_check(ui.ctx());
        let issues = self.current_issues();

        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.add_space(4.0);
                self.top_banners(ui, state);
                ui.scope(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    ui.columns(2, |cols| {
                        let a = card_content(&mut cols[0], |ui| self.root_card(ui, state));
                        let b = card_content(&mut cols[1], |ui| self.inboxes_card(ui));
                        paint_card_row(cols, [a, b]);
                    });
                    ui.add_space(6.0);
                    ui.columns(2, |cols| {
                        let a = card_content(&mut cols[0], |ui| self.client_list_card(ui, state));
                        let b = card_content(&mut cols[1], |ui| self.parsing_card(ui));
                        paint_card_row(cols, [a, b]);
                    });
                });
                ui.add_space(6.0);
                setup_card(ui, |ui| self.ultratax_card(ui));
                ui.add_space(6.0);
                setup_card(ui, |ui| self.template_card(ui));
                ui.add_space(6.0);
                setup_card(ui, |ui| display_card(ui, state));
                ui.add_space(12.0);
                self.save_row(ui, state, issues);
            });
    }

    fn top_banners(&self, ui: &mut egui::Ui, state: &AppState) {
        if !state.pdf_enabled() {
            banner(
                ui,
                Level::Error,
                "pdfium.dll was not found next to the program, so PDF features \
                 (text sniffing, combine, split, thumbnails) are turned off. \
                 Put pdfium.dll in the same folder as the exe and restart.",
            );
        }
        if !state.settings_savable {
            banner(
                ui,
                Level::Error,
                "config.json could not be read, so defaults are shown. Saving will keep the \
                 old file as config.json.corrupt-<time> and write a new one.",
            );
        }
        if let Some(e) = &state.template_error {
            banner(
                ui,
                Level::Error,
                &format!(
                    "{e} Saving will keep the old file as template.json.corrupt-<time> and write \
                     a new one."
                ),
            );
        }
        let cross: Vec<String> = self
            .draft
            .inboxes
            .iter()
            .filter(|i| self.cross_drive(i))
            .map(|i| i.display().to_string())
            .collect();
        if !cross.is_empty() {
            let p = pal(ui);
            let (bg, fg) = p.pill(PillKind::Warn);
            Frame::new()
                .fill(bg)
                .stroke(Stroke::new(1.0, p.attention.gamma_multiply(0.6)))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::symmetric(16, 10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        warning_icon(ui, fg);
                        ui.label(strong("Inbox on a different volume than root.").color(fg));
                        ui.label(RichText::new("Files from").color(fg));
                        ui.label(mono(cross.join(", ")).color(fg));
                        ui.label(
                            RichText::new(
                                "will be copied, SHA-256 verified, then deleted from the inbox.",
                            )
                            .color(fg),
                        );
                    });
                });
            ui.add_space(6.0);
        }
    }

    fn root_card(&mut self, ui: &mut egui::Ui, state: &mut AppState) {
        card_title(ui, "Client root", "");
        ui.horizontal(|ui| {
            let w = ui.available_width() - 96.0;
            let writable = self.root_writable().then_some((PillKind::Ok, "writable"));
            path_field(
                ui,
                &path_text(self.draft.root.as_deref()),
                writable,
                false,
                w,
            );
            if ui.button("Change…").clicked()
                && let Some(p) = pick_folder(self.draft.root.as_deref())
            {
                self.draft.root = Some(p);
            }
        });
        ui.add_space(2.0);
        scan_line(ui, state);
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            rescan_button(ui, state);
            if ui.button("Open data folder").clicked()
                && let Err(e) = std::process::Command::new("explorer")
                    .arg(&state.data_dir)
                    .spawn()
            {
                self.message = Some((Level::Error, format!("Could not open Explorer: {e}")));
            }
            if let Some((done, total)) = state.folders.scanning() {
                ui.spinner();
                ui.label(format!("Scanning… {done}/{total}"));
            }
        });
    }

    fn inboxes_card(&mut self, ui: &mut egui::Ui) {
        card_title(
            ui,
            "Inboxes",
            " · watched while the app is open · zips go to Import",
        );
        let mut remove = None;
        for (i, inbox) in self.draft.inboxes.iter().enumerate() {
            let cross = self.cross_drive(inbox);
            ui.horizontal(|ui| {
                let w = ui.available_width() - 84.0;
                let pill = cross.then_some((PillKind::Warn, "cross-drive"));
                path_field(ui, &inbox.display().to_string(), pill, cross, w);
                if ui.button("Remove").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            self.draft.inboxes.remove(i);
        }
        ui.checkbox(&mut self.draft.inbox_subfolders, "Include sub-folders")
            .on_hover_text(
                "On: files in folders inside an inbox are queued too. Off: only files directly \
                 in the inbox. A person's own inbox (My inbox) only ever takes the files directly \
                 in it.",
            );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let l = ui.label("Also file").on_hover_text(
                "File types queued for filing besides PDF, separated by commas. Filed by their \
                 name like PDFs (no page reading, combine or split). Program files (.exe, .bat \
                 …) are never filed.",
            );
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.file_types_text)
                    .desired_width(ui.available_width() - 150.0)
                    .hint_text("docx, xlsx, jpg"),
            );
            let edit = edit.labelled_by(l.id);
            if edit.changed() {
                self.draft.file_types = parse_file_types(&self.file_types_text);
            }
            if ui
                .button("Default list")
                .on_hover_text("Office files, images, text and mail")
                .clicked()
            {
                self.draft.file_types = crate::config::DEFAULT_FILE_TYPES
                    .iter()
                    .map(|t| t.to_string())
                    .collect();
                self.file_types_text = file_types_text(&self.draft.file_types);
            }
        });
        let blocked: Vec<&String> = self
            .draft
            .file_types
            .iter()
            .filter(|t| crate::zipimport::BLOCKED.contains(&t.as_str()))
            .collect();
        if !blocked.is_empty() {
            ui.label(
                RichText::new(format!(
                    "Never filed (program files): {}",
                    blocked
                        .iter()
                        .map(|t| t.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
                .size(11.0)
                .color(pal(ui).muted),
            );
        }
        ui.checkbox(&mut self.draft.images_to_pdf, "File images as PDFs")
            .on_hover_text(
                "JPEG, PNG, TIFF (every page), BMP and GIF images are filed as PDFs, a phone photo \
                 turned upright. The image goes to the inbox's _combined folder; Undo puts it \
                 back. Off: images are filed as they are.",
            );
        ui.horizontal(|ui| {
            if ui.button("Add inbox…").clicked()
                && let Some(p) = pick_folder(None)
                && !self.draft.inboxes.contains(&p)
            {
                self.draft.inboxes.push(p);
            }
            ui.label(
                RichText::new("An inbox cannot be inside the client root (except in a top-level _ . or ~ folder).")
                    .size(11.0)
                    .color(pal(ui).muted),
            );
        });
    }

    fn client_list_card(&mut self, ui: &mut egui::Ui, state: &mut AppState) {
        card_title(ui, "Client master list", "");
        let idle = !state.clients.busy();
        ui.horizontal(|ui| {
            let w = ui.available_width() - 120.0;
            let csv = state
                .settings
                .csv_path
                .as_deref()
                .map_or_else(|| "(no CSV chosen)".into(), |p| p.display().to_string());
            path_field(ui, &csv, None, false, w);
            if ui
                .add_enabled(idle, egui::Button::new("Choose CSV…"))
                .clicked()
                && let Some(p) = rfd::FileDialog::new()
                    .add_filter("CSV", &["csv", "txt"])
                    .pick_file()
            {
                self.open_wizard(ui.ctx(), p, &state.settings);
            }
        });
        let muted = pal(ui).muted;
        let stats = match state.clients.list() {
            Some(list) => {
                let mut s = format!(
                    "{} clients · imported {}",
                    theme::thousands(list.len()),
                    list.imported_at
                        .with_timezone(&chrono::Local)
                        .format("%b %-d, %-I:%M %p")
                );
                match list.skipped_rows {
                    0 => {}
                    1 => s.push_str(" · 1 row skipped (blank ID or name)"),
                    n => s.push_str(&format!(" · {n} rows skipped (blank ID or name)")),
                }
                match state.relationships.dangling(list).len() {
                    0 => {}
                    1 => s.push_str(" · 1 relationship now dangling"),
                    n => s.push_str(&format!(" · {n} relationships now dangling")),
                }
                s
            }
            None => "No client list imported yet.".into(),
        };
        ui.label(RichText::new(stats).size(12.0).color(muted));
        if let Some((level, msg)) = state.clients.note() {
            banner(ui, *level, msg);
        }
        let map = &state.settings.column_map;
        if let (Some(id), Some(name)) = (&map.id, &map.name) {
            ui.horizontal_wrapped(|ui| {
                label_caps(ui, "Columns");
                pill(ui, PillKind::Neutral, format!("ID → {id}"));
                pill(ui, PillKind::Neutral, format!("Name → {name}"));
                if let Some(partner) = &map.partner {
                    pill(ui, PillKind::Neutral, format!("Partner → {partner}"));
                }
                if !map.extras.is_empty() {
                    pill(
                        ui,
                        PillKind::Info,
                        format!("+ {} as tokens", map.extras.join(", ")),
                    );
                }
            });
        }
        ui.horizontal(|ui| {
            if let Some(csv) = state.settings.csv_path.clone() {
                if ui
                    .add_enabled(idle, egui::Button::new("Refresh clients"))
                    .clicked()
                {
                    state.clients.refresh(
                        ui.ctx(),
                        state.data_dir.clone(),
                        csv.clone(),
                        state.settings.column_map.clone(),
                    );
                }
                if ui
                    .add_enabled(idle, egui::Button::new("Re-map columns…"))
                    .clicked()
                {
                    self.open_wizard(ui.ctx(), csv, &state.settings);
                }
            }
            if !idle {
                ui.spinner();
            }
        });

        if let Some(result) = state.clients.take_wizard_result()
            && let Some(wizard) = &mut self.wizard
        {
            match result {
                Ok(()) => self.wizard = None,
                Err(e) => wizard.import_failed(e),
            }
        }
        if let Some(wizard) = &mut self.wizard {
            match wizard.show(ui.ctx()) {
                Outcome::Open => {}
                Outcome::Cancel => self.wizard = None,
                Outcome::Import { table, map } => {
                    state
                        .clients
                        .import(ui.ctx(), state.data_dir.clone(), table, map);
                }
            }
        }
    }

    /// UltraTax PDF names: the same position settings as UltraTax's "File Name Variables".
    fn ultratax_card(&mut self, ui: &mut egui::Ui) {
        use crate::parse::{K1Position, UltraTaxLayout, UtField};
        card_title(
            ui,
            "UltraTax file names",
            " · tried before ClientName_ID_rest · set like UltraTax's File Name Variables",
        );
        let p = pal(ui);
        let layout = &mut self.draft.ultratax;
        ui.checkbox(&mut layout.enabled, "Read UltraTax file names");
        if !layout.enabled {
            return;
        }
        ui.add_space(4.0);
        let pos_text = |v: Option<u8>| v.map_or("None".to_string(), |n| format!("Position {n}"));
        egui::Grid::new("setup_ultratax")
            .num_columns(2)
            .spacing([16.0, 6.0])
            .show(ui, |ui| {
                for f in UtField::ALL {
                    let l = ui.label(f.label());
                    let slot = layout.position_mut(f);
                    egui::ComboBox::from_id_salt(("ut_pos", f.label()))
                        .selected_text(pos_text(*slot))
                        .width(160.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(slot, None, "None");
                            for n in 1..=UtField::ALL.len() as u8 {
                                ui.selectable_value(slot, Some(n), format!("Position {n}"));
                            }
                        })
                        .response
                        .labelled_by(l.id);
                    ui.end_row();
                }
                let l = ui.label("K1 Recipient Name");
                egui::ComboBox::from_id_salt("ut_k1")
                    .selected_text(layout.k1.label())
                    .width(160.0)
                    .show_ui(ui, |ui| {
                        for k in K1Position::ALL {
                            ui.selectable_value(&mut layout.k1, k, k.label());
                        }
                    })
                    .response
                    .labelled_by(l.id);
                ui.end_row();
            });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            label_caps(ui, "Example");
            match layout.validate() {
                Ok(()) => {
                    ui.label(mono(layout.example()).color(p.text));
                }
                Err(e) => {
                    ui.label(RichText::new(e).color(p.pill(PillKind::Err).1));
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Restore Default").clicked() {
                    *layout = UltraTaxLayout::default();
                }
            });
        });
        ui.label(
            RichText::new(
                "The parts become tokens for rule file names: {EntityType}, {Document}, \
                 {K1Recipient}, {YearEnd}, {DatePrinted} (plus {Client}, {ID}, {Year}). The \
                 client ID is looked up in the client list as written.",
            )
            .size(11.0)
            .color(p.muted),
        );
    }

    fn parsing_card(&mut self, ui: &mut egui::Ui) {
        card_title(ui, "Filename parsing", "");
        let p = pal(ui);
        egui::Grid::new("setup_parsing")
            .num_columns(2)
            .spacing([16.0, 10.0])
            .show(ui, |ui| {
                let id_label = label_caps(ui, "ID pattern");
                ui.horizontal(|ui| {
                    let valid = crate::parse::validate_id_pattern(&self.draft.id_pattern).is_ok();
                    let field = ui
                        .add(
                            egui::TextEdit::singleline(&mut self.draft.id_pattern)
                                .font(egui::TextStyle::Monospace)
                                .desired_width(ui.available_width() - 90.0),
                        )
                        .labelled_by(id_label.id);
                    if !valid {
                        attention_border(ui, field.rect);
                    }
                    if valid {
                        pill(ui, PillKind::Ok, "valid regex");
                    } else {
                        pill(ui, PillKind::Err, "invalid");
                    }
                });
                ui.end_row();

                label_caps(ui, "Convention");
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.label(mono("ClientName_").color(p.muted));
                    ui.label(theme::mono_strong("ID").color(p.accent));
                    ui.label(mono("_rest.pdf").color(p.muted));
                });
                ui.end_row();

                let order_bad = self.draft.fuzzy_auto < self.draft.fuzzy_suggest;
                let suggest_label = label_caps(ui, "Suggest at");
                let suggest = ui
                    .add(
                        egui::DragValue::new(&mut self.draft.fuzzy_suggest)
                            .range(FUZZY_THRESHOLD_RANGE)
                            .speed(0.01)
                            .fixed_decimals(2),
                    )
                    .labelled_by(suggest_label.id);
                if order_bad {
                    attention_border(ui, suggest.rect);
                }
                ui.end_row();

                let auto_label = label_caps(ui, "Auto-resolve at");
                let auto = ui
                    .add(
                        egui::DragValue::new(&mut self.draft.fuzzy_auto)
                            .range(FUZZY_THRESHOLD_RANGE)
                            .speed(0.01)
                            .fixed_decimals(2),
                    )
                    .labelled_by(auto_label.id);
                if order_bad {
                    attention_border(ui, auto.rect);
                }
                ui.end_row();
            });
        ui.label(
            RichText::new(
                "Between the two a name match is suggested and needs a click; at or above \
                 auto-resolve it files with a fuzzy pill.",
            )
            .size(11.0)
            .color(p.muted),
        );
    }

    fn template_card(&mut self, ui: &mut egui::Ui) {
        let p = pal(ui);
        let selected_is_root = self.selected_node.is_empty();
        let mut action = None;
        ui.horizontal(|ui| {
            ui.label(strong("Folder template"));
            ui.label(
                RichText::new(" · created on confirm, year folders only when a route needs them")
                    .color(p.muted),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Reset to default").clicked() {
                    action = Some(TreeAction::Reset);
                }
                if ui
                    .add_enabled(!selected_is_root, egui::Button::new("Delete"))
                    .clicked()
                {
                    action = Some(TreeAction::Delete);
                }
                if ui.button("Rename").clicked() {
                    action = Some(TreeAction::Rename);
                }
                if ui.button("Add child").clicked() {
                    action = Some(TreeAction::AddChild);
                }
            });
        });
        ui.add_space(6.0);
        ui.columns(2, |cols| {
            let mut path = Vec::new();
            let mut clicked = None;
            let mut commit_rename = false;
            tree_rows(
                &mut cols[0],
                &self.template_draft,
                &mut path,
                &self.selected_node,
                &mut self.renaming,
                &mut clicked,
                &mut commit_rename,
            );
            if let Some(c) = clicked {
                self.selected_node = c;
            }
            if commit_rename {
                action = Some(TreeAction::CommitRename);
            }
            tokens_panel(&mut cols[1], &self.draft.column_map);
        });
        self.apply_tree_action(action);
    }

    fn apply_tree_action(&mut self, action: Option<TreeAction>) {
        match action {
            Some(TreeAction::AddChild) => {
                if let Some(node) = node_at(&mut self.template_draft, &self.selected_node) {
                    node.children.push(TemplateNode::new("New folder"));
                    let mut path = self.selected_node.clone();
                    path.push(node.children.len() - 1);
                    self.renaming = Some((path.clone(), "New folder".into()));
                    self.selected_node = path;
                }
            }
            Some(TreeAction::Rename) => {
                if let Some(node) = node_at(&mut self.template_draft, &self.selected_node) {
                    self.renaming = Some((self.selected_node.clone(), node.name.clone()));
                }
            }
            Some(TreeAction::CommitRename) => {
                if let Some((path, text)) = self.renaming.take()
                    && let Some(node) = node_at(&mut self.template_draft, &path)
                    && !text.trim().is_empty()
                {
                    node.name = text.trim().to_string();
                }
            }
            Some(TreeAction::Delete) => {
                if let Some((last, parent)) = self.selected_node.clone().split_last()
                    && let Some(node) = node_at(&mut self.template_draft, parent)
                    && *last < node.children.len()
                {
                    node.children.remove(*last);
                    self.selected_node = parent.to_vec();
                    self.renaming = None;
                }
            }
            Some(TreeAction::Reset) => {
                self.template_draft = template::default_tree();
                self.selected_node.clear();
                self.renaming = None;
            }
            None => {}
        }
    }

    fn save_row(&mut self, ui: &mut egui::Ui, state: &mut AppState, issues: Option<Vec<Issue>>) {
        match &issues {
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Checking folders…");
                });
            }
            Some(list) => {
                // Cross-drive notes are the banner at the top.
                for issue in list.iter().filter(|i| !i.msg.contains("(cross-drive:")) {
                    banner(ui, issue.level, &issue.msg);
                }
            }
        }
        let dirty = self.is_dirty(state) || !state.settings_savable || !state.template_savable;
        let has_errors = issues
            .as_ref()
            .is_none_or(|l| l.iter().any(|i| i.level == Level::Error));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let save = ui.add_enabled(dirty && !has_errors, theme::primary_button(ui, "Save"));
            let save = if has_errors && dirty {
                save.on_disabled_hover_text("Fix the errors above first.")
            } else {
                save
            };
            if save.clicked() {
                self.save(state);
            }
            if ui.add_enabled(dirty, egui::Button::new("Revert")).clicked() {
                self.draft = state.settings.clone();
                self.file_types_text = file_types_text(&self.draft.file_types);
                self.template_draft = state.template.clone();
                self.selected_node.clear();
                self.renaming = None;
                self.message = None;
            }
            if dirty {
                ui.label(RichText::new("Not saved yet").color(pal(ui).muted));
            }
        });
        if let Some((level, msg)) = &self.message {
            ui.add_space(4.0);
            banner(ui, *level, msg);
        }
    }

    /// The client CSV was imported through the wizard and saved: mirror the CSV path and column
    /// map into the draft so they don't count as unsaved edits.
    pub fn sync_csv(&mut self, settings: &Settings) {
        self.draft.csv_path = settings.csv_path.clone();
        self.draft.column_map = settings.column_map.clone();
    }

    /// The client list was imported but the CSV path and column map could not be written to
    /// `config.json`. Keep them as unsaved edits (Save and the close prompt then pick them up)
    /// and say so.
    pub fn stage_csv(&mut self, source: PathBuf, column_map: config::ColumnMap, error: &str) {
        self.draft.csv_path = Some(source);
        self.draft.column_map = column_map;
        self.message = Some((
            Level::Error,
            format!(
                "The client list was imported, but its CSV location and columns could not be \
                 saved to config.json ({error}). Click Save to keep them."
            ),
        ));
    }

    /// Opens the import wizard on `csv`, preselecting the saved column map where it still fits.
    pub fn open_wizard(&mut self, ctx: &egui::Context, csv: PathBuf, settings: &Settings) {
        self.wizard = Some(CsvWizard::open(ctx, csv, settings.column_map.clone()));
    }

    /// Writes the settings and, if changed, the template. Returns true when everything was saved.
    pub fn save(&mut self, state: &mut AppState) -> bool {
        let data_dir = state.data_dir.clone();
        if !state.settings_savable {
            if let Err(e) = set_aside(&Settings::path(&data_dir)) {
                self.message = Some((
                    Level::Error,
                    format!("Could not set aside the unreadable config.json: {e}"),
                ));
                return false;
            }
            state.settings_savable = true;
        }
        let mut next = self.draft.clone();
        next.window = state.settings.window;
        next.display = state.settings.display;
        if let Err(e) = next.save(&data_dir) {
            tracing::error!(error = %e, "saving settings failed");
            self.message = Some((Level::Error, format!("Saving failed: {e}")));
            return false;
        }
        state.settings = next;

        if self.template_draft != state.template || !state.template_savable {
            if !state.template_savable {
                if let Err(e) = set_aside(&template::path(&data_dir)) {
                    self.message = Some((
                        Level::Error,
                        format!("Could not set aside the unreadable template.json: {e}"),
                    ));
                    return false;
                }
                state.template_savable = true;
                state.template_error = None;
            }
            if let Err(e) = template::save(&data_dir, &self.template_draft) {
                tracing::error!(error = %e, "saving template failed");
                self.message = Some((
                    Level::Error,
                    format!("Settings saved, but the folder template could not be: {e}"),
                ));
                return false;
            }
            state.template = self.template_draft.clone();
        }
        tracing::info!("settings saved");
        self.message = Some((Level::Info, "Settings saved.".into()));
        true
    }
}

/// Renames an unreadable data file to `<name>.corrupt-<time>` so a fresh one can be written.
pub fn set_aside(path: &Path) -> std::io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let kept = path.with_file_name(format!("{name}.corrupt-{stamp}"));
    std::fs::rename(path, &kept)?;
    tracing::warn!(kept = %kept.display(), "unreadable file set aside");
    Ok(())
}

/// A small warning triangle with "!" (egui's fonts have no ⚠ glyph).
fn warning_icon(ui: &mut egui::Ui, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), Sense::hover());
    let c = rect.center();
    let pts = vec![
        egui::pos2(c.x, rect.top() + 1.0),
        egui::pos2(rect.right() - 0.5, rect.bottom() - 1.0),
        egui::pos2(rect.left() + 0.5, rect.bottom() - 1.0),
    ];
    ui.painter()
        .add(egui::Shape::closed_line(pts, Stroke::new(1.3, color)));
    ui.painter().line_segment(
        [egui::pos2(c.x, c.y - 2.0), egui::pos2(c.x, c.y + 2.0)],
        Stroke::new(1.3, color),
    );
    ui.painter()
        .circle_filled(egui::pos2(c.x, c.y + 4.0), 0.9, color);
}

/// A Setup card on its own (full width).
fn setup_card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let p = pal(ui);
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

/// Lays out a card's content with a placeholder for its background, which
/// [`paint_card_row`] fills once both cards in the row are measured.
fn card_content(
    ui: &mut egui::Ui,
    add: impl FnOnce(&mut egui::Ui),
) -> (egui::layers::ShapeIdx, egui::Rect) {
    let bg = ui.painter().add(egui::Shape::Noop);
    let rect = Frame::new()
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        })
        .response
        .rect;
    (bg, rect)
}

/// Paints both cards of a row at the taller card's height.
fn paint_card_row(cols: &mut [egui::Ui], cards: [(egui::layers::ShapeIdx, egui::Rect); 2]) {
    let p = pal(&cols[0]);
    let height = cards[0].1.height().max(cards[1].1.height());
    for (ui, (bg, rect)) in cols.iter_mut().zip(cards) {
        let full = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), height));
        ui.painter().set(
            bg,
            egui::epaint::RectShape::new(
                full,
                CornerRadius::same(6),
                p.card,
                Stroke::new(1.0, p.border),
                egui::StrokeKind::Inside,
            ),
        );
        // Reserve the extra height so the row below starts under the taller card.
        ui.allocate_rect(full, Sense::hover());
    }
}

/// Theme and font size (V1, V2). They apply and save at once, without Setup's Save.
fn display_card(ui: &mut egui::Ui, state: &mut AppState) {
    card_title(ui, "Display", " · applies and saves right away");
    let d = &mut state.settings.display;
    ui.horizontal(|ui| {
        let label = ui.label("Theme");
        let mut theme = d.theme;
        let resp = egui::ComboBox::from_id_salt("setup_theme")
            .selected_text(theme.label())
            .show_ui(ui, |ui| {
                for t in [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark] {
                    ui.selectable_value(&mut theme, t, t.label());
                }
            });
        resp.response.labelled_by(label.id);
        d.theme = theme;
        ui.add_space(24.0);
        let label = ui.label("Font size");
        let mut pct = (d.zoom * 100.0).round() as i32;
        let (lo, hi) = (
            (ZOOM_RANGE.start() * 100.0).round() as i32,
            (ZOOM_RANGE.end() * 100.0).round() as i32,
        );
        if ui
            .add_enabled(pct > lo, egui::Button::new("A−"))
            .on_hover_text("Smaller (Ctrl+Minus)")
            .clicked()
        {
            pct = (pct - 10).max(lo);
        }
        ui.add(
            egui::Slider::new(&mut pct, lo..=hi)
                .step_by(5.0)
                .suffix(" %"),
        )
        .labelled_by(label.id);
        if ui
            .add_enabled(pct < hi, egui::Button::new("A+"))
            .on_hover_text("Larger (Ctrl+Plus)")
            .clicked()
        {
            pct = (pct + 10).min(hi);
        }
        if ui
            .add_enabled(pct != 100, egui::Button::new("Reset"))
            .on_hover_text("100 % (Ctrl+0)")
            .clicked()
        {
            pct = 100;
        }
        d.zoom = pct as f32 / 100.0;
    });
    ui.label(
        RichText::new("Ctrl+Plus, Ctrl+Minus and Ctrl+0 work on every screen.")
            .size(12.0)
            .color(pal(ui).muted),
    );
}

/// Bold card title with an optional muted continuation.
fn card_title(ui: &mut egui::Ui, title: &str, rest: &str) {
    let p = pal(ui);
    let mut job = egui::text::LayoutJob::default();
    let body = egui::TextStyle::Body.resolve(ui.style());
    job.append(
        title,
        0.0,
        egui::TextFormat::simple(
            egui::FontId::new(body.size, egui::FontFamily::Name(theme::SEMIBOLD.into())),
            p.text,
        ),
    );
    job.append(rest, 0.0, egui::TextFormat::simple(body, p.muted));
    ui.label(job);
    ui.add_space(4.0);
}

/// A read-only path shown like a field, with an optional pill at its right end.
fn path_field(
    ui: &mut egui::Ui,
    text: &str,
    pill_at_end: Option<(PillKind, &str)>,
    attention: bool,
    width: f32,
) {
    let p = pal(ui);
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(
            1.0,
            if attention { p.attention } else { p.border },
        ))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(10, 5))
        .show(ui, |ui| {
            ui.set_width((width - 20.0).max(60.0));
            ui.horizontal(|ui| {
                ui.add(egui::Label::new(mono(text).color(p.text)).truncate())
                    .on_hover_text(text);
                if let Some((kind, t)) = pill_at_end {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        pill(ui, kind, t);
                    });
                }
            });
        });
}

/// "1,131 folders bound · 4 need attention in Conflicts · last scan today 9:12 AM".
fn scan_line(ui: &mut egui::Ui, state: &mut AppState) {
    let muted = pal(ui).muted;
    let small = |t: String| RichText::new(t).size(12.0).color(muted);
    match state.folders.scan_of(state.settings.root.as_deref()) {
        Some(f) => {
            let bound = f
                .folders
                .iter()
                .filter(|b| b.status == Status::Bound)
                .count();
            let attention = f
                .folders
                .iter()
                .filter(|b| !matches!(b.status, Status::Bound | Status::Ignored))
                .count();
            let local = f.scanned_at.with_timezone(&chrono::Local);
            let when = if local.date_naive() == chrono::Local::now().date_naive() {
                format!("today {}", local.format("%-I:%M %p"))
            } else {
                local.format("%b %-d, %-I:%M %p").to_string()
            };
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(small(format!(
                    "{} folders bound · {attention} need attention in",
                    theme::thousands(bound)
                )));
                if ui.link(RichText::new("Conflicts").size(12.0)).clicked() {
                    state.goto = Some(Tab::Conflicts);
                }
                ui.label(small(format!("· last scan {when}")));
            });
        }
        None => {
            ui.label(small("Not scanned yet.".into()));
        }
    }
}

fn rescan_button(ui: &mut egui::Ui, state: &mut AppState) {
    let blocker = state.scan_blocker();
    let button = ui.add_enabled(blocker.is_none(), egui::Button::new("Rescan root"));
    let button = match blocker {
        Some(why) => button.on_disabled_hover_text(why),
        None => button,
    };
    if button.clicked() {
        state.start_scan(ui.ctx());
    }
}

enum TreeAction {
    AddChild,
    Rename,
    CommitRename,
    Delete,
    Reset,
}

/// One row per template node: folder icon, name with `{Token}`s highlighted, a trailing `\`.
fn tree_rows(
    ui: &mut egui::Ui,
    node: &TemplateNode,
    path: &mut Vec<usize>,
    selected: &[usize],
    renaming: &mut Option<(Vec<usize>, String)>,
    clicked: &mut Option<Vec<usize>>,
    commit_rename: &mut bool,
) {
    let p = pal(ui);
    let is_selected = selected == path.as_slice();
    let row_h = 24.0;
    ui.spacing_mut().item_spacing.y = 0.0;
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), row_h), Sense::click());
    if is_selected {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(3), p.accent.gamma_multiply(0.10));
    }
    let x0 = rect.left() + 6.0 + path.len() as f32 * 18.0;
    // Folder icon.
    let icon = egui::Rect::from_min_size(
        egui::pos2(x0, rect.center().y - 5.0),
        egui::vec2(13.0, 10.0),
    );
    let stroke = Stroke::new(1.2, p.muted);
    ui.painter().rect_stroke(
        icon,
        CornerRadius::same(1),
        stroke,
        egui::StrokeKind::Inside,
    );
    ui.painter().line_segment(
        [
            icon.left_top() + egui::vec2(0.0, 0.0),
            icon.left_top() + egui::vec2(5.0, -2.0),
        ],
        stroke,
    );
    let text_pos = egui::pos2(x0 + 20.0, rect.top());
    let editing = renaming.as_ref().is_some_and(|(r, _)| r == path);
    if editing {
        let text_rect = egui::Rect::from_min_max(
            egui::pos2(text_pos.x, rect.top() + 1.0),
            egui::pos2(rect.right().min(text_pos.x + 260.0), rect.bottom() - 1.0),
        );
        let (_, text) = renaming.as_mut().expect("editing");
        let edit = ui.put(
            text_rect,
            egui::TextEdit::singleline(text)
                .font(egui::TextStyle::Monospace)
                .id_salt(("template_rename", path.clone())),
        );
        edit.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Folder name"));
        if !edit.has_focus() && !edit.lost_focus() {
            edit.request_focus();
        }
        if edit.lost_focus() {
            *commit_rename = true;
        }
    } else {
        let job = name_job(ui, &node.name, path.is_empty(), p);
        let galley = ui.painter().layout_job(job);
        let name = node.name.clone();
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name.clone()));
        ui.painter().galley(
            egui::pos2(text_pos.x, rect.center().y - galley.size().y / 2.0),
            galley,
            p.text,
        );
    }
    if resp.clicked() {
        *clicked = Some(path.clone());
    }
    if resp.double_clicked() {
        *renaming = Some((path.clone(), node.name.clone()));
    }
    for (i, child) in node.children.iter().enumerate() {
        path.push(i);
        tree_rows(ui, child, path, selected, renaming, clicked, commit_rename);
        path.pop();
    }
}

/// Folder name in monospace with `{Token}` parts in the accent colour and a trailing ` \`.
fn name_job(ui: &egui::Ui, name: &str, _root: bool, p: &theme::Palette) -> egui::text::LayoutJob {
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let mut job = egui::text::LayoutJob::default();
    let mut rest = name;
    while let Some(start) = rest.find('{') {
        let end = rest[start..].find('}').map(|e| start + e + 1);
        let Some(end) = end else { break };
        job.append(
            &rest[..start],
            0.0,
            egui::TextFormat::simple(font.clone(), p.text),
        );
        job.append(
            &rest[start..end],
            0.0,
            egui::TextFormat::simple(font.clone(), p.accent),
        );
        rest = &rest[end..];
    }
    job.append(rest, 0.0, egui::TextFormat::simple(font.clone(), p.text));
    job.append(" \\", 0.0, egui::TextFormat::simple(font, p.muted));
    job
}

/// "Tokens available": core tokens neutral, CSV tokens info, plus the copy and sanitizer notes.
fn tokens_panel(ui: &mut egui::Ui, map: &config::ColumnMap) {
    let muted = pal(ui).muted;
    label_caps(ui, "Tokens available");
    ui.add_space(2.0);
    ui.horizontal_wrapped(|ui| {
        for t in BUILTIN_TOKENS {
            token_pill(ui, PillKind::Neutral, format!("{{{t}}}"));
        }
        for t in clients::extra_tokens(map) {
            token_pill(ui, PillKind::Info, format!("{{{t}}}"));
        }
    });
    ui.add_space(4.0);
    ui.label(
        RichText::new(
            "Copy destinations in Rules also get {Copy.Client}, {Copy.ID}, {Copy.Role} and {Copy.col}.",
        )
        .size(12.0)
        .color(muted),
    );
    ui.label(
        RichText::new(
            "Names are sanitized for Windows: reserved device names, trailing dots and spaces, \
             and paths over 240 characters are rejected at save time.",
        )
        .size(12.0)
        .color(muted),
    );
}

fn node_at<'a>(root: &'a mut TemplateNode, path: &[usize]) -> Option<&'a mut TemplateNode> {
    let mut node = root;
    for &i in path {
        node = node.children.get_mut(i)?;
    }
    Some(node)
}

fn path_text(p: Option<&Path>) -> String {
    p.map_or_else(|| "(not set)".to_string(), |p| p.display().to_string())
}

fn pick_folder(start: Option<&Path>) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new();
    if let Some(dir) = start {
        dialog = dialog.set_directory(dir);
    }
    dialog.pick_folder()
}

/// Amber border around a field that needs attention (UI_SPEC §1).
fn attention_border(ui: &egui::Ui, rect: egui::Rect) {
    ui.painter().rect_stroke(
        rect,
        3.0,
        egui::Stroke::new(1.5, pal(ui).attention),
        egui::StrokeKind::Outside,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(dir: &Path, settings: Settings, savable: bool) -> AppState {
        AppState::load(dir.to_path_buf(), settings, savable, None)
    }

    #[test]
    fn save_persists_draft_and_keeps_window() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings {
            window: Some(config::WindowState {
                x: 1.0,
                y: 2.0,
                width: 3.0,
                height: 4.0,
            }),
            ..Settings::default()
        };
        let mut st = state(dir.path(), settings, true);
        let mut screen = SetupScreen::new(&st);
        screen.draft.root = Some(dir.path().to_path_buf());
        screen.draft.fuzzy_suggest = 0.95;
        screen.draft.window = None;
        assert!(screen.is_dirty(&st));
        assert!(screen.save(&mut st));
        assert!(!screen.is_dirty(&st));

        let (loaded, ok) = Settings::load_or_default(dir.path());
        assert!(ok);
        assert_eq!(loaded, st.settings);
        assert_eq!(loaded.fuzzy_suggest, 0.95);
        assert_eq!(loaded.root.as_deref(), Some(dir.path()));
        assert!(loaded.window.is_some());
    }

    #[test]
    fn save_sets_aside_unreadable_config() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(Settings::path(dir.path()), "{ broken").unwrap();
        let (settings, savable) = Settings::load_or_default(dir.path());
        assert!(!savable);
        let mut st = state(dir.path(), settings, savable);
        let mut screen = SetupScreen::new(&st);
        assert!(screen.save(&mut st));

        assert!(st.settings_savable);
        assert!(Settings::load_or_default(dir.path()).1);
        let kept: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("config.json.corrupt-"))
            .collect();
        assert_eq!(kept.len(), 1);
        let text = std::fs::read_to_string(dir.path().join(&kept[0])).unwrap();
        assert_eq!(text, "{ broken");
    }

    #[test]
    fn template_edits_are_dirty_and_saved_with_settings() {
        let dir = tempfile::tempdir().unwrap();
        let mut st = state(dir.path(), Settings::default(), true);
        let mut screen = SetupScreen::new(&st);
        assert!(!screen.is_dirty(&st));
        screen
            .template_draft
            .children
            .push(TemplateNode::new("Payroll"));
        assert!(screen.is_dirty(&st));
        assert!(screen.save(&mut st));
        assert!(!screen.is_dirty(&st));
        let (reloaded, ok, _) = template::load_or_seed(dir.path());
        assert!(ok);
        assert!(reloaded.children.iter().any(|c| c.name == "Payroll"));
    }

    #[test]
    fn tree_add_and_delete() {
        let mut tree = template::default_tree();
        node_at(&mut tree, &[1])
            .unwrap()
            .children
            .push(TemplateNode::new("Extensions"));
        assert_eq!(tree.children[1].children.len(), 2);
        assert!(node_at(&mut tree, &[19]).is_none());
    }
}
