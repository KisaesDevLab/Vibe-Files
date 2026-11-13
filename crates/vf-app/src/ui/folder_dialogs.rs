//! Rename a client, and add, rename or delete a folder in a client folder (PA-29, owner
//! 2026-11-05): from the client panel and the Browse tree's right-click menu
//! (`vf_fs::folders`, one Transaction each, on the undo stack).

use egui::{RichText, TextEdit};
use vf_fs::folders::{self, ClientIdChange, ClientRename};

use crate::ui::browse::Env;
use crate::ui::job::{self, Job};
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
    /// Client uuid and its current ID (owner 2026-11-12: clients made from folder names get
    /// their real ID here).
    SetClientId {
        client_id: String,
        ext_id: String,
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
    /// Set ID: the preview, recomputed when the ID changes.
    id_plan: Option<Result<ClientIdChange, String>>,
    pub error: Option<String>,
    focused: bool,
    /// The change being made (owner 2026-11-13: setting an ID took seconds with no sign of
    /// life): on its own thread, the dialog showing a spinner meanwhile.
    job: Option<Job<Result<Done, String>>>,
}

/// What the buttons' work needs, taken from the dialog so it can run on its own thread.
struct Work {
    ask: FolderAsk,
    name: String,
    plan: Option<ClientRename>,
    id_plan: Option<ClientIdChange>,
}

impl FolderDialog {
    pub fn new(ask: FolderAsk) -> Self {
        let name = match &ask {
            FolderAsk::RenameClient { name, .. } => name.clone(),
            FolderAsk::SetClientId { ext_id, .. } => ext_id.clone(),
            FolderAsk::RenameFolder { rel } => vf_core::browse::leaf(rel).to_owned(),
            _ => String::new(),
        };
        Self {
            ask,
            name,
            plan: None,
            id_plan: None,
            error: None,
            focused: false,
            job: None,
        }
    }
}

/// The message to show when done.
pub type Done = String;

pub fn show(ctx: &egui::Context, d: &mut FolderDialog, env: &mut Env) -> DialogOutcome<Done> {
    let mut out = DialogOutcome::Open;
    if let Some(r) = job::take(&mut d.job) {
        match r.and_then(|r| r) {
            Ok(msg) => out = DialogOutcome::Done(msg),
            Err(e) => d.error = Some(e),
        }
    }
    let busy = d.job.is_some();
    let title = match &d.ask {
        FolderAsk::RenameClient { .. } => "Rename client",
        FolderAsk::SetClientId { .. } => "Set client ID",
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
                    FolderAsk::SetClientId { .. } => "Client ID",
                    _ => "Folder name",
                });
                let r = ui.add_enabled(
                    !busy,
                    TextEdit::singleline(&mut d.name).desired_width(f32::INFINITY),
                );
                let r = r.labelled_by(l.id);
                if !d.focused {
                    r.request_focus();
                    d.focused = true;
                }
                if r.changed() {
                    d.plan = None;
                    d.id_plan = None;
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
                if let FolderAsk::SetClientId { client_id, .. } = &ask {
                    if d.id_plan.is_none() {
                        d.id_plan = Some(folders::plan_client_id(
                            env.conn, env.root, client_id, &d.name,
                        ));
                    }
                    ui.add_space(6.0);
                    match d.id_plan.as_ref() {
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
                        "The client list, relationships, the folder's sentinel and the client CSV Intake uses take the new ID. Not on the undo list: set the ID again to change it back.",
                    ));
                }
            }
        }
        ui.add_space(6.0);
        widgets::message_line(ui, d.error.as_deref());
        ui.add_space(8.0);
        if let Some(j) = &d.job {
            j.show(ui);
        }
        ui.horizontal(|ui| {
            let (label, ready) = match &d.ask {
                FolderAsk::RenameClient { .. } => {
                    ("Rename", matches!(d.plan, Some(Ok(_))))
                }
                FolderAsk::SetClientId { .. } => ("Set ID", matches!(d.id_plan, Some(Ok(_)))),
                FolderAsk::NewFolder { .. } => ("Create folder", !d.name.trim().is_empty()),
                FolderAsk::RenameFolder { .. } => ("Rename", !d.name.trim().is_empty()),
                FolderAsk::DeleteFolder { .. } => ("Delete folder", true),
            };
            let pressed = ui
                .add_enabled(ready && !busy, widgets::primary_button(label))
                .clicked()
                || (go && ready && !busy);
            if pressed && let Some(msg) = start(ctx, d, env) {
                out = DialogOutcome::Done(msg);
            }
            // The change cannot be stopped halfway: Cancel waits for it.
            if !busy
                && (ui.button("Cancel").clicked()
                    || ui.input(|i| i.key_pressed(egui::Key::Escape)))
            {
                out = DialogOutcome::Cancelled;
            }
        });
    });
    out
}

/// Starts the dialog's change on its own thread with its own catalog connection (a catalog
/// with no file takes it at once on this one, and its message is returned).
fn start(ctx: &egui::Context, d: &mut FolderDialog, env: &mut Env) -> Option<Done> {
    let w = Work {
        ask: d.ask.clone(),
        name: d.name.clone(),
        plan: d.plan.clone().and_then(Result::ok),
        id_plan: d.id_plan.clone().and_then(Result::ok),
    };
    d.error = None;
    let catalog = env.conn.path().filter(|p| !p.is_empty());
    let Some(catalog) = catalog.map(std::path::PathBuf::from) else {
        return match run(&w, env.conn, env.root, env.device_id) {
            Ok(msg) => Some(msg),
            Err(e) => {
                d.error = Some(e);
                None
            }
        };
    };
    let what = match &w.ask {
        FolderAsk::RenameClient { .. } => "Renaming the client…",
        FolderAsk::SetClientId { .. } => "Setting the ID…",
        FolderAsk::NewFolder { .. } => "Creating the folder…",
        FolderAsk::RenameFolder { .. } => "Renaming the folder…",
        FolderAsk::DeleteFolder { .. } => "Deleting the folder…",
    };
    let (root, device) = (env.root.to_path_buf(), env.device_id.to_string());
    match Job::start(ctx, what, move || {
        let mut conn =
            vf_core::db::open(&catalog, vf_core::db::Storage::Local).map_err(|e| e.to_string())?;
        run(&w, &mut conn, &root, &device)
    }) {
        Ok(j) => d.job = Some(j),
        Err(e) => d.error = Some(e),
    }
    None
}

fn run(
    d: &Work,
    conn: &mut vf_core::db::Connection,
    root: &std::path::Path,
    device_id: &str,
) -> Result<String, String> {
    match &d.ask {
        FolderAsk::RenameClient { .. } => {
            let Some(p) = &d.plan else {
                return Err("Nothing to rename.".into());
            };
            folders::rename_client(conn, root, p, device_id)?;
            Ok(match &p.folder {
                Some((_, new)) => format!(
                    "Renamed the client to {}; its folder is now {new}.",
                    p.new_name
                ),
                None => format!("Renamed the client to {}.", p.new_name),
            })
        }
        FolderAsk::SetClientId { .. } => {
            let Some(p) = &d.id_plan else {
                return Err("Nothing to change.".into());
            };
            let note = folders::set_client_id(conn, root, p, device_id)?;
            let mut msg = match &p.folder {
                Some((_, new)) => {
                    format!("{} is now {}; its folder is now {new}.", p.name, p.new_id)
                }
                None => format!("{} is now {}.", p.name, p.new_id),
            };
            if let Some(n) = note {
                msg.push(' ');
                msg.push_str(&n);
            }
            Ok(msg)
        }
        FolderAsk::NewFolder { parent } => {
            let rel = folders::new_folder(conn, root, parent, &d.name, device_id)?;
            Ok(format!("Created {rel}."))
        }
        FolderAsk::RenameFolder { rel } => {
            let new = folders::rename_folder(conn, root, rel, &d.name, device_id)?;
            Ok(format!("Renamed {rel} to {new}."))
        }
        FolderAsk::DeleteFolder { rel } => {
            folders::delete_folder(conn, root, rel, device_id)?;
            Ok(format!("Deleted {rel}."))
        }
    }
}
