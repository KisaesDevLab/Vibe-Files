//! Rename a client, and add, rename or delete a folder in a client folder (PA-29, owner
//! 2026-11-05): from the client panel and the Browse tree's right-click menu
//! (`vf_fs::folders`, one Transaction each, on the undo stack).

use egui::{RichText, TextEdit};
use vf_fs::folders::{self, ClientRename};

use crate::ui::browse::Env;
use crate::ui::theme;
use crate::ui::widgets::{self, DialogOutcome};

/// What is being asked for. Paths are cabinet-relative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderAsk {
    /// Client uuid and its current name.
    RenameClient {
        client_id: String,
        name: String,
    },
    NewFolder {
        parent: String,
    },
    RenameFolder {
        rel: String,
    },
    DeleteFolder {
        rel: String,
    },
}

pub struct FolderDialog {
    pub ask: FolderAsk,
    pub name: String,
    /// Rename client: the preview, recomputed when the name changes.
    plan: Option<Result<ClientRename, String>>,
    pub error: Option<String>,
    focused: bool,
}

impl FolderDialog {
    pub fn new(ask: FolderAsk) -> Self {
        let name = match &ask {
            FolderAsk::RenameClient { name, .. } => name.clone(),
            FolderAsk::RenameFolder { rel } => vf_core::browse::leaf(rel).to_owned(),
            _ => String::new(),
        };
        Self {
            ask,
            name,
            plan: None,
            error: None,
            focused: false,
        }
    }
}

/// The message to show when done.
pub type Done = String;

pub fn show(ctx: &egui::Context, d: &mut FolderDialog, env: &mut Env) -> DialogOutcome<Done> {
    let mut out = DialogOutcome::Open;
    let title = match &d.ask {
        FolderAsk::RenameClient { .. } => "Rename client",
        FolderAsk::NewFolder { .. } => "New folder",
        FolderAsk::RenameFolder { .. } => "Rename folder",
        FolderAsk::DeleteFolder { .. } => "Delete folder",
    };
    egui::Modal::new(egui::Id::unique("folder-dialog")).show(ctx, |ui| {
        ui.set_width(520.0);
        ui.heading(title);
        ui.add_space(8.0);
        let mut go = false;
        match d.ask.clone() {
            FolderAsk::DeleteFolder { rel } => {
                ui.label(format!("Delete the empty folder {rel}?"));
                ui.label(widgets::caption(
                    "Only a folder with no files in it is deleted (empty folders inside it go too). Undo brings it back.",
                ));
            }
            ask => {
                if let FolderAsk::NewFolder { parent } = &ask {
                    ui.label(widgets::caption(format!("In {parent}")));
                }
                let l = ui.label(match ask {
                    FolderAsk::RenameClient { .. } => "Client name",
                    _ => "Folder name",
                });
                let r = ui.add(TextEdit::singleline(&mut d.name).desired_width(f32::INFINITY));
                let r = r.labelled_by(l.id);
                if !d.focused {
                    r.request_focus();
                    d.focused = true;
                }
                if r.changed() {
                    d.plan = None;
                    d.error = None;
                }
                go = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if let FolderAsk::RenameClient { client_id, .. } = &ask {
                    if d.plan.is_none() {
                        d.plan = Some(folders::plan_client_rename(
                            env.conn, env.root, client_id, &d.name,
                        ));
                    }
                    ui.add_space(6.0);
                    match d.plan.as_ref() {
                        Some(Ok(p)) => match &p.folder {
                            Some((old, new)) => {
                                ui.label(widgets::caption("The client folder is renamed too:"));
                                ui.label(RichText::new(old).monospace().small());
                                ui.label(RichText::new(format!("→ {new}")).monospace().small());
                            }
                            None => {
                                ui.label(widgets::caption("The client folder keeps its name."));
                            }
                        },
                        Some(Err(e)) => {
                            ui.label(RichText::new(e).color(theme::ink_muted()));
                        }
                        None => {}
                    }
                    ui.label(widgets::caption(
                        "Importing the client list again puts the tax software's name back.",
                    ));
                }
            }
        }
        ui.add_space(6.0);
        widgets::message_line(ui, d.error.as_deref());
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let (label, ready) = match &d.ask {
                FolderAsk::RenameClient { .. } => {
                    ("Rename", matches!(d.plan, Some(Ok(_))))
                }
                FolderAsk::NewFolder { .. } => ("Create folder", !d.name.trim().is_empty()),
                FolderAsk::RenameFolder { .. } => ("Rename", !d.name.trim().is_empty()),
                FolderAsk::DeleteFolder { .. } => ("Delete folder", true),
            };
            if ui
                .add_enabled(ready, widgets::primary_button(label))
                .clicked()
                || (go && ready)
            {
                match run(d, env) {
                    Ok(msg) => out = DialogOutcome::Done(msg),
                    Err(e) => d.error = Some(e),
                }
            }
            if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                out = DialogOutcome::Cancelled;
            }
        });
    });
    out
}

fn run(d: &FolderDialog, env: &mut Env) -> Result<String, String> {
    match &d.ask {
        FolderAsk::RenameClient { .. } => {
            let Some(Ok(p)) = &d.plan else {
                return Err("Nothing to rename.".into());
            };
            folders::rename_client(env.conn, env.root, p, env.device_id)?;
            Ok(match &p.folder {
                Some((_, new)) => format!(
                    "Renamed the client to {}; its folder is now {new}.",
                    p.new_name
                ),
                None => format!("Renamed the client to {}.", p.new_name),
            })
        }
        FolderAsk::NewFolder { parent } => {
            let rel = folders::new_folder(env.conn, env.root, parent, &d.name, env.device_id)?;
            Ok(format!("Created {rel}."))
        }
        FolderAsk::RenameFolder { rel } => {
            let new = folders::rename_folder(env.conn, env.root, rel, &d.name, env.device_id)?;
            Ok(format!("Renamed {rel} to {new}."))
        }
        FolderAsk::DeleteFolder { rel } => {
            folders::delete_folder(env.conn, env.root, rel, env.device_id)?;
            Ok(format!("Deleted {rel}."))
        }
    }
}
