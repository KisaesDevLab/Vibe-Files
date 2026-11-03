//! Cabinet wizard: open an existing cabinet or create one (P01 step 1). Shown until a cabinet
//! is open.

use egui::{Color32, RichText, TextEdit};

use vf_security::keys::MIN_PASSPHRASE_LEN;

use crate::ui::theme;

pub const ERROR: Color32 = Color32::from_rgb(0xB4, 0x23, 0x18);

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
    pub passphrase: String,
    pub confirm: String,
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
            passphrase: String::new(),
            confirm: String::new(),
            error: None,
        }
    }

    /// Drops the passphrases from memory once they have been used.
    pub fn clear_secrets(&mut self) {
        self.passphrase.clear();
        self.confirm.clear();
    }
}

/// Draws the wizard. Returns the mode to run when its button was pressed.
pub fn show(ui: &mut egui::Ui, w: &mut Wizard) -> Option<Mode> {
    let mut submit = None;
    egui::Frame::new()
        .fill(theme::PANE)
        .stroke(egui::Stroke::new(1.0, theme::BORDER_PANE))
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
            field(ui, "Recovery passphrase", |ui, label| {
                ui.add(
                    TextEdit::singleline(&mut w.passphrase)
                        .password(true)
                        .desired_width(400.0),
                )
                .labelled_by(label);
            });
            if w.mode == Mode::Create {
                field(ui, "Confirm passphrase", |ui, label| {
                    ui.add(
                        TextEdit::singleline(&mut w.confirm)
                            .password(true)
                            .desired_width(400.0),
                    )
                    .labelled_by(label);
                });
                ui.label(
                    RichText::new(format!(
                        "At least {MIN_PASSPHRASE_LEN} characters. The passphrase opens this \
                         cabinet on any PC. It cannot be recovered if lost: keep it with the \
                         firm's records."
                    ))
                    .small()
                    .color(theme::INK_MUTED),
                );
                ui.label(
                    RichText::new(
                        "The folder may already hold client folders; they are indexed, not moved.",
                    )
                    .small()
                    .color(theme::INK_MUTED),
                );
            }
            ui.add_space(12.0);
            if let Some(e) = &w.error {
                ui.label(RichText::new(e).color(ERROR));
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
    let l = ui.label(RichText::new(label).color(theme::INK_SECONDARY));
    add(ui, l.id);
    ui.add_space(8.0);
}
