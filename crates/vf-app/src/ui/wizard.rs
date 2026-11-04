//! Cabinet wizard: open an existing cabinet or create one (P01 step 1). Shown until a cabinet
//! is open.

use egui::{RichText, TextEdit};

use crate::ui::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Open,
    Create,
}

#[derive(Debug, Clone)]
pub struct Wizard {
    pub mode: Mode,
    pub root: String,
    pub name: String,
    pub error: Option<String>,
}

impl Wizard {
    /// Open mode on the last cabinet when there is one, else Create.
    pub fn new(last_cabinet: Option<&std::path::Path>) -> Self {
        Self {
            mode: if last_cabinet.is_some() {
                Mode::Open
            } else {
                Mode::Create
            },
            root: last_cabinet
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            name: String::new(),
            error: None,
        }
    }
}

/// Draws the wizard. Returns the mode to run when its button was pressed.
pub fn show(ui: &mut egui::Ui, w: &mut Wizard) -> Option<Mode> {
    let mut submit = None;
    egui::Frame::new()
        .fill(theme::pane())
        .stroke(egui::Stroke::new(1.0, theme::border_pane()))
        .inner_margin(24)
        .corner_radius(6)
        .show(ui, |ui| {
            ui.set_max_width(560.0);
            ui.heading("Cabinet");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut w.mode, Mode::Open, "Open existing");
                ui.selectable_value(&mut w.mode, Mode::Create, "Create new");
            });
            ui.add_space(12.0);

            field(ui, "Cabinet folder", |ui, label| {
                ui.horizontal(|ui| {
                    ui.add(
                        TextEdit::singleline(&mut w.root)
                            .desired_width(400.0)
                            .hint_text(r"C:\Clients or \\server\share\Clients"),
                    )
                    .labelled_by(label);
                    if ui.button("Browse…").clicked()
                        && let Some(p) = rfd::FileDialog::new().pick_folder()
                    {
                        w.root = p.display().to_string();
                    }
                });
            });
            if w.mode == Mode::Create {
                field(ui, "Cabinet name", |ui, label| {
                    ui.add(TextEdit::singleline(&mut w.name).desired_width(400.0))
                        .labelled_by(label);
                });
            }
            if w.mode == Mode::Create {
                ui.label(
                    RichText::new(
                        "The folder may already hold client folders; they are indexed, not moved.",
                    )
                    .small()
                    .color(theme::ink_muted()),
                );
            }
            ui.add_space(12.0);
            if let Some(e) = &w.error {
                ui.label(RichText::new(e).color(crate::ui::theme::error()));
                ui.add_space(8.0);
            }
            let label = match w.mode {
                Mode::Open => "Open cabinet",
                Mode::Create => "Create cabinet",
            };
            if ui.button(label).clicked() {
                submit = Some(w.mode);
            }
        });
    submit
}

fn field(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui, egui::Id)) {
    let l = ui.label(RichText::new(label).color(theme::ink_secondary()));
    add(ui, l.id);
    ui.add_space(8.0);
}
