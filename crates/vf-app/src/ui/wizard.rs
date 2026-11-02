//! Cabinet open/create wizard (PHASES P01 step 1): create a new cabinet, attach this PC to an
//! existing one, or unlock this PC's catalog with the recovery passphrase.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::RichText;
use vf_fs::rebuild::RebuildReport;
use vf_security::keys::{Kdf, MIN_PASSPHRASE_CHARS};

use crate::session::{self, LocalState, Session};
use crate::ui::theme;
use crate::ui::widgets::{banner, field, mono, muted, primary};

pub type Opened = (Session, Option<RebuildReport>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Choose,
    Create,
    Attach,
    Unlock,
}

pub struct Wizard {
    data: PathBuf,
    kdf: Kdf,
    pub state: LocalState,
    pub mode: Mode,
    pub root: String,
    pub firm: String,
    pub pass: String,
    pub pass2: String,
    pub error: Option<String>,
    job: Option<Receiver<anyhow::Result<Opened>>>,
}

impl Wizard {
    pub fn new(data: &Path, kdf: Kdf) -> Self {
        let state = session::local_state(data);
        let mode = match state {
            LocalState::Locked { .. } => Mode::Unlock,
            _ => Mode::Choose,
        };
        Self {
            data: data.to_path_buf(),
            kdf,
            state,
            mode,
            root: String::new(),
            firm: String::new(),
            pass: String::new(),
            pass2: String::new(),
            error: None,
            job: None,
        }
    }

    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    fn start(&mut self, ctx: &egui::Context) {
        let (tx, rx) = std::sync::mpsc::channel();
        let (data, kdf, mode) = (self.data.clone(), self.kdf, self.mode);
        let root = PathBuf::from(self.root.trim());
        let (firm, pass) = (self.firm.clone(), self.pass.clone());
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = match mode {
                Mode::Create => session::create_cabinet(&data, &root, &firm, &pass, kdf)
                    .map(|(s, r)| (s, Some(r))),
                Mode::Attach => {
                    session::attach_cabinet(&data, &root, &pass, kdf).map(|(s, r)| (s, Some(r)))
                }
                Mode::Unlock | Mode::Choose => session::unlock(&data, &pass).map(|s| (s, None)),
            };
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        self.error = None;
        self.job = Some(rx);
    }

    /// Validates the form; `Some(message)` blocks the primary button.
    fn problem(&self) -> Option<String> {
        if self.mode == Mode::Unlock {
            return self
                .pass
                .is_empty()
                .then(|| "Enter the recovery passphrase.".into());
        }
        if self.root.trim().is_empty() {
            return Some("Choose the cabinet folder.".into());
        }
        if self.mode == Mode::Create && self.firm.trim().is_empty() {
            return Some("Enter the firm name.".into());
        }
        if self.pass.chars().count() < MIN_PASSPHRASE_CHARS {
            return Some(format!(
                "The recovery passphrase needs at least {MIN_PASSPHRASE_CHARS} characters."
            ));
        }
        if self.mode == Mode::Create && self.pass != self.pass2 {
            return Some("The two passphrases differ.".into());
        }
        None
    }

    /// Draws the wizard; returns the session once a cabinet is open.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<Opened> {
        if let Some(rx) = &self.job {
            match rx.try_recv() {
                Ok(Ok(opened)) => {
                    self.job = None;
                    return Some(opened);
                }
                Ok(Err(e)) => {
                    self.job = None;
                    self.error = Some(format!("{e:#}"));
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.job = None;
                    self.error = Some("The cabinet could not be opened.".into());
                }
            }
        }
        let ctx = ui.ctx().clone();
        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(48.0);
                ui.set_max_width(520.0);
                egui::Frame::new()
                    .fill(theme::PANE)
                    .stroke(egui::Stroke::new(1.0, theme::BORDER_PANE))
                    .corner_radius(4)
                    .inner_margin(24)
                    .show(ui, |ui| {
                        ui.set_width(472.0);
                        self.body(ui, &ctx);
                    });
            });
        });
        None
    }

    fn body(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
            if let LocalState::Broken(msg) = &self.state {
                ui.label(
                    RichText::new("This PC's catalog needs attention")
                        .size(16.0)
                        .strong(),
                );
                banner(ui, msg, true);
                return;
            }
            match self.mode {
                Mode::Choose => {
                    ui.label(RichText::new("Open a cabinet").size(16.0).strong());
                    ui.label(muted(
                        "A cabinet is the folder that holds your client folders. \
                         Vibe Files keeps its catalog on this PC.",
                    ));
                    ui.add_space(12.0);
                    if ui.add(primary("Create new cabinet")).clicked() {
                        self.mode = Mode::Create;
                    }
                    ui.add_space(6.0);
                    if ui.button("Open existing cabinet").clicked() {
                        self.mode = Mode::Attach;
                    }
                    ui.label(muted(
                        "Use this on a second PC, or after moving Vibe Files to a new data folder.",
                    ));
                }
                Mode::Create | Mode::Attach => self.form(ui, ctx),
                Mode::Unlock => {
                    ui.label(RichText::new("Unlock the catalog").size(16.0).strong());
                    if let LocalState::Locked { cabinet } = &self.state {
                        ui.label(mono(cabinet.display().to_string()).color(theme::INK_MUTED));
                    }
                    ui.add_space(8.0);
                    let r = field(
                        ui,
                        "Recovery passphrase",
                        egui::TextEdit::singleline(&mut self.pass)
                            .password(true)
                            .desired_width(f32::INFINITY),
                    );
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.label(muted(
                        "Until Windows sign-in unlock arrives (P06), the catalog opens with the \
                         passphrase set when the cabinet was created.",
                    ));
                    self.actions(ui, ctx, "Unlock", enter);
                }
            }
        });
    }

    fn form(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let create = self.mode == Mode::Create;
        ui.label(
            RichText::new(if create {
                "Create new cabinet"
            } else {
                "Open existing cabinet"
            })
            .size(16.0)
            .strong(),
        );
        ui.add_space(8.0);
        let label = ui.label(if create {
            "Cabinet folder (an existing folder: local, or a share like \\\\NAS\\Cabinet)"
        } else {
            "Cabinet folder (holds _Vibe\\cabinet.json)"
        });
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.root)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(380.0),
            )
            .labelled_by(label.id);
            if ui.button("Browse…").clicked()
                && let Some(dir) = rfd::FileDialog::new().pick_folder()
            {
                self.root = dir.display().to_string();
            }
        });
        if create {
            field(
                ui,
                "Firm name",
                egui::TextEdit::singleline(&mut self.firm).desired_width(f32::INFINITY),
            );
        }
        field(
            ui,
            "Recovery passphrase",
            egui::TextEdit::singleline(&mut self.pass)
                .password(true)
                .desired_width(f32::INFINITY),
        );
        if create {
            field(
                ui,
                "Type it again",
                egui::TextEdit::singleline(&mut self.pass2)
                    .password(true)
                    .desired_width(f32::INFINITY),
            );
            ui.label(muted(
                "Write the passphrase down and keep it somewhere safe. Without it the catalog \
                 cannot be opened; the files themselves stay readable in Explorer.",
            ));
        } else {
            ui.label(muted(
                "Sets this PC's own catalog passphrase. The catalog is rebuilt from the folder \
                 tree, so this can take a while on a large cabinet.",
            ));
        }
        let label = if create {
            "Create cabinet"
        } else {
            "Open cabinet"
        };
        self.actions(ui, ctx, label, false);
    }

    fn actions(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, label: &str, enter: bool) {
        ui.add_space(12.0);
        if let Some(e) = &self.error {
            banner(ui, e, true);
            ui.add_space(6.0);
        }
        let problem = self.problem();
        ui.horizontal(|ui| {
            if self.busy() {
                ui.spinner();
                ui.label(if self.mode == Mode::Unlock {
                    "Unlocking…"
                } else {
                    "Building the catalog…"
                });
                return;
            }
            let go = ui
                .add_enabled(problem.is_none(), primary(label))
                .on_disabled_hover_text(problem.clone().unwrap_or_default())
                .clicked();
            if (go || (enter && problem.is_none())) && !self.busy() {
                self.start(ctx);
            }
            if self.mode != Mode::Unlock && ui.button("Back").clicked() {
                self.mode = Mode::Choose;
                self.error = None;
            }
        });
        if let Some(p) = problem
            && !self.busy()
        {
            ui.label(muted(p));
        }
    }
}
