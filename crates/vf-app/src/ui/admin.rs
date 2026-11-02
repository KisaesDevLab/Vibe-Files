//! Admin tab (UI_SPEC §6). P01 brings the doc type catalog editor (PHASES P01 step 4); the other
//! nav items arrive with their phases.

use egui::RichText;
use vf_core::doctype::{DocType, DocTypeEdit};

use crate::ui::theme;
use crate::ui::widgets::{banner, field, mono, muted, primary, section};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Page {
    #[default]
    DocTypes,
    Capture,
    Audit,
}

/// Print-capture changes the page asks for.
pub enum CaptureAction {
    Change(std::path::PathBuf),
    UseDefault,
}

#[derive(Default)]
pub struct Admin {
    pub page: Page,
    pub show_archived: bool,
    pub edit: Option<DocTypeEdit>,
    pub error: Option<String>,
    /// Last "Verify chain now" result: `(ok, text)`.
    pub verified: Option<(bool, String)>,
}

/// Changes the doc type page asks for.
pub enum DocTypeAction {
    Save(DocTypeEdit),
    Archive(String, bool),
}

impl Admin {
    pub fn nav_ui(&mut self, ui: &mut egui::Ui, doc_types: usize) {
        section(ui, "Firm");
        ui.selectable_value(
            &mut self.page,
            Page::DocTypes,
            format!("Doc types ({doc_types})"),
        );
        for later in [
            "Users & roles (P06)",
            "Client access (P06)",
            "Devices (P09)",
        ] {
            ui.add_enabled(false, egui::Button::new(later).frame(false));
        }
        section(ui, "Intake");
        ui.selectable_value(&mut self.page, Page::Capture, "Print capture & parsers");
        ui.add_enabled(
            false,
            egui::Button::new("Scan profiles (P02 step 2)").frame(false),
        );
        section(ui, "Compliance");
        ui.selectable_value(&mut self.page, Page::Audit, "Audit trail");
        for later in [
            "Retention classes (P07)",
            "Legal holds (P07)",
            "§7216 consents (P07)",
        ] {
            ui.add_enabled(false, egui::Button::new(later).frame(false));
        }
    }

    pub fn doc_types_ui(&mut self, ui: &mut egui::Ui, types: &[DocType]) -> Option<DocTypeAction> {
        let mut action = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Doc types").size(16.0).strong());
            ui.label(muted(
                "Each type carries its default folder (Filer template tokens).",
            ));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(primary("Add doc type")).clicked() {
                    self.edit = Some(DocTypeEdit::default());
                    self.error = None;
                }
                ui.checkbox(&mut self.show_archived, "Show archived");
            });
        });
        if let Some(e) = &self.error {
            banner(ui, e, true);
        }
        let mut cancel = false;
        if let Some(edit) = &mut self.edit {
            egui::Frame::new()
                .fill(theme::FOOTER)
                .inner_margin(10)
                .corner_radius(4)
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(if edit.id.is_some() {
                            "Edit doc type"
                        } else {
                            "New doc type"
                        })
                        .strong(),
                    );
                    field(
                        ui,
                        "Name",
                        egui::TextEdit::singleline(&mut edit.name).desired_width(320.0),
                    );
                    field(
                        ui,
                        "Default folder (e.g. Income Tax/{Year})",
                        egui::TextEdit::singleline(&mut edit.default_folder_tpl)
                            .font(egui::TextStyle::Monospace)
                            .desired_width(320.0),
                    );
                    ui.checkbox(
                        &mut edit.client_visible,
                        "Client visible (portal, delivery)",
                    );
                    ui.horizontal(|ui| {
                        if ui.add(primary("Save")).clicked() {
                            action = Some(DocTypeAction::Save(edit.clone()));
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });
                });
        }
        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .id_salt("doctypes")
            .show(ui, |ui| {
                egui::Grid::new("doctype_grid")
                    .num_columns(4)
                    .striped(true)
                    .spacing([16.0, 6.0])
                    .show(ui, |ui| {
                        for h in ["Name", "Default folder", "Client visible", ""] {
                            ui.label(
                                RichText::new(h)
                                    .small()
                                    .strong()
                                    .color(theme::INK_SECONDARY),
                            );
                        }
                        ui.end_row();
                        for t in types.iter().filter(|t| self.show_archived || !t.archived) {
                            let name = if t.archived {
                                RichText::new(format!("{} (archived)", t.name))
                                    .color(theme::INK_MUTED)
                            } else {
                                RichText::new(&t.name)
                            };
                            ui.label(name);
                            ui.label(
                                mono(t.default_folder_tpl.clone().unwrap_or_default()).small(),
                            );
                            ui.label(if t.client_visible { "Yes" } else { "" });
                            ui.horizontal(|ui| {
                                if ui.small_button("Edit").clicked() {
                                    self.edit = Some(DocTypeEdit {
                                        id: Some(t.id.clone()),
                                        name: t.name.clone(),
                                        default_folder_tpl: t
                                            .default_folder_tpl
                                            .clone()
                                            .unwrap_or_default(),
                                        client_visible: t.client_visible,
                                    });
                                    self.error = None;
                                }
                                let (label, to) = if t.archived {
                                    ("Restore", false)
                                } else {
                                    ("Archive", true)
                                };
                                if ui.small_button(label).clicked() {
                                    action = Some(DocTypeAction::Archive(t.id.clone(), to));
                                }
                            });
                            ui.end_row();
                        }
                    });
            });
        action
    }

    /// Print capture folder and parser profiles (P02 step 5).
    pub fn capture_ui(
        &mut self,
        ui: &mut egui::Ui,
        folder: &std::path::Path,
        is_default: bool,
        parsers: &std::path::Path,
    ) -> Option<CaptureAction> {
        let mut action = None;
        ui.label(RichText::new("Print capture & parsers").size(16.0).strong());
        ui.label(muted(
            "PDFs a PDF printer saves here enter Intake as firm documents. A parser profile reads \
             the client ID, year and form from the file name and files them with no edits.",
        ));
        section(ui, "Capture folder");
        ui.label(mono(folder.display().to_string()));
        ui.horizontal(|ui| {
            if ui.button("Open folder").clicked() {
                let _ = std::fs::create_dir_all(folder);
                if let Err(e) = crate::shell::open(folder) {
                    self.error = Some(e);
                }
            }
            if ui.button("Change…").clicked()
                && let Some(d) = rfd::FileDialog::new().pick_folder()
            {
                action = Some(CaptureAction::Change(d));
            }
            if !is_default && ui.button("Use the default").clicked() {
                action = Some(CaptureAction::UseDefault);
            }
        });
        section(ui, "Set up Microsoft Print to PDF");
        for (i, step) in [
            "In your tax software, print the return and choose the printer \"Microsoft Print to PDF\" (or any PDF printer).",
            "In the Save dialog, open the capture folder above. Pin it to Quick access so it is one click next time.",
            "Keep the file name your tax software suggests, e.g. 1042_2025_Federal Return.pdf (client ID, year, form).",
            "Save. Within a few seconds the file is in Intake, Ready to confirm.",
        ]
        .iter()
        .enumerate()
        {
            ui.label(format!("{}. {step}", i + 1));
        }
        section(ui, "Parser profiles");
        ui.label(muted(format!(
            "Editable TOML files in {} (regex on the file name; groups id, year). Order decides \
             which profile tries first.",
            parsers.display()
        )));
        let (profiles, errors) = vf_fs::capture::load_profiles(parsers);
        egui::Grid::new("parsers")
            .num_columns(3)
            .striped(true)
            .show(ui, |ui| {
                for h in ["Profile", "Status", "File"] {
                    ui.label(
                        RichText::new(h)
                            .small()
                            .strong()
                            .color(theme::INK_SECONDARY),
                    );
                }
                ui.end_row();
                for p in &profiles {
                    ui.label(&p.profile.name);
                    ui.label(&p.profile.status);
                    ui.label(
                        mono(p.file.file_name().unwrap_or_default().to_string_lossy()).small(),
                    );
                    ui.end_row();
                }
            });
        for e in errors {
            banner(ui, &e, true);
        }
        if ui.button("Open profiles folder").clicked()
            && let Err(e) = crate::shell::open(parsers)
        {
            self.error = Some(e);
        }
        if let Some(e) = &self.error {
            banner(ui, e, true);
        }
        action
    }

    /// Audit trail page: verify the chain on demand (full table and filters come in P07).
    pub fn audit_ui(&mut self, ui: &mut egui::Ui, conn: &vf_core::db::Connection) {
        ui.label(RichText::new("Audit trail").size(16.0).strong());
        ui.label(muted(
            "Hash-chained, append-only · also on disk at _Vibe\\audit\\<yyyy-mm>.jsonl",
        ));
        if ui.add(primary("Verify chain now")).clicked() {
            self.verified = Some(match vf_core::audit::verify(conn) {
                Ok(r) if r.ok() => (true, format!("Chain verified · {} rows", r.rows)),
                Ok(r) => {
                    let (seq, why) = r.broken.unwrap_or_default();
                    (false, format!("Chain BROKEN at seq {seq}: {why}"))
                }
                Err(e) => (false, e.to_string()),
            });
        }
        if let Some((ok, text)) = &self.verified {
            banner(ui, text, !ok);
        }
    }
}
