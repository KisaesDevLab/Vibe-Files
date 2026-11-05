//! Browse › right-click › Move to… / Copy to… (owner 2026-11-05, Q-74): pick a cabinet (this
//! one or another Vibe Files cabinet), a client, and a folder of that client. The preview and
//! the confirm are Browse's own ("Move N documents?").

use std::path::{Path, PathBuf};

use egui::RichText;
use vf_fs::copymove::{Dest, Mode};

use crate::ui::{theme, widgets};

pub struct PickDest {
    pub ids: Vec<String>,
    pub mode: Mode,
    /// Another cabinet's root; `None` = this cabinet.
    pub other: Option<PathBuf>,
    /// (label, client folder) of the cabinet chosen.
    clients: Vec<(String, String)>,
    client: Option<usize>,
    folders: Vec<String>,
    folder: Option<usize>,
    pub error: Option<String>,
}

pub enum PickOutcome {
    Open,
    Cancelled,
    Chosen(Dest),
}

impl PickDest {
    /// `clients`: this cabinet's clients as (label, client folder).
    pub fn new(ids: Vec<String>, mode: Mode, clients: Vec<(String, String)>) -> PickDest {
        PickDest {
            ids,
            mode,
            other: None,
            clients,
            client: None,
            folders: vec![],
            folder: None,
            error: None,
        }
    }

    fn pick_client(&mut self, root: &Path, i: Option<usize>) {
        self.client = i;
        self.folder = None;
        self.folders = i
            .and_then(|i| self.clients.get(i))
            .map(|(_, f)| vf_fs::copymove::folders_in(root, f))
            .unwrap_or_default();
        if !self.folders.is_empty() {
            self.folder = Some(0);
        }
    }
}

/// The dialog, every frame while open. `root`: this cabinet; `here`: its clients.
pub fn show(
    ctx: &egui::Context,
    p: &mut PickDest,
    root: &Path,
    here: &[(String, String)],
) -> PickOutcome {
    let mut out = PickOutcome::Open;
    let cab_root = p.other.clone().unwrap_or_else(|| root.to_path_buf());
    egui::Modal::new(egui::Id::unique("move-to-dialog")).show(ctx, |ui| {
        ui.set_width(560.0);
        let n = p.ids.len();
        ui.heading(format!(
            "{} {n} document{} to…",
            match p.mode {
                Mode::Move => "Move",
                Mode::Copy => "Copy",
            },
            if n == 1 { "" } else { "s" }
        ));
        ui.add_space(4.0);
        // Cabinet.
        ui.horizontal(|ui| {
            ui.label("Cabinet");
            if ui.radio(p.other.is_none(), "This cabinet").clicked() && p.other.is_some() {
                p.other = None;
                p.clients = here.to_vec();
                p.pick_client(root, None);
            }
            if ui.radio(p.other.is_some(), "Another cabinet…").clicked()
                && let Some(dir) = rfd::FileDialog::new().pick_folder()
            {
                if vf_fs::cabinet::read(&dir).is_err() {
                    p.error = Some(format!(
                        "{} is not a Vibe Files cabinet (it has no _Vibe\\cabinet.json).",
                        dir.display()
                    ));
                } else {
                    p.error = None;
                    p.clients = vf_fs::copymove::client_folders(&dir)
                        .into_iter()
                        .map(|f| (f.clone(), f))
                        .collect();
                    p.pick_client(&dir, None);
                    p.other = Some(dir);
                }
            }
        });
        if let Some(o) = &p.other {
            ui.label(widgets::caption(o.display().to_string()));
        }
        // Client.
        ui.horizontal(|ui| {
            let l = ui.label("Client");
            let shown = p
                .client
                .and_then(|i| p.clients.get(i))
                .map(|c| c.0.clone())
                .unwrap_or_else(|| "Pick a client".into());
            let labels: Vec<String> = p.clients.iter().map(|c| c.0.clone()).collect();
            let (resp, pick) = widgets::search_combo(
                ui,
                ("move-to-client", p.other.is_some()),
                380.0,
                shown,
                || labels,
                p.client,
                None,
            );
            resp.labelled_by(l.id);
            if let Some(i) = pick {
                let base = p.other.clone().unwrap_or_else(|| root.to_path_buf());
                p.pick_client(&base, i);
            }
        });
        // Folder.
        ui.label("Folder");
        egui::ScrollArea::vertical()
            .id_salt("move-to-folders")
            .max_height(260.0)
            .show(ui, |ui| {
                if p.folders.is_empty() {
                    ui.label(widgets::caption("Pick a client to see its folders."));
                }
                let client_depth = p
                    .folders
                    .first()
                    .map(|f| f.matches('/').count())
                    .unwrap_or(0);
                for (i, f) in p.folders.iter().enumerate() {
                    let depth = f.matches('/').count() - client_depth;
                    let name = f.rsplit('/').next().unwrap_or(f);
                    let label = format!("{}{name}", "    ".repeat(depth));
                    if ui.selectable_label(p.folder == Some(i), label).clicked() {
                        p.folder = Some(i);
                    }
                }
            });
        if let Some(e) = &p.error {
            ui.label(RichText::new(e).color(theme::error()));
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let chosen = p.folder.and_then(|i| p.folders.get(i)).cloned();
            if ui
                .add_enabled(chosen.is_some(), widgets::primary_button("Next"))
                .clicked()
                && let Some(folder) = chosen
            {
                out = PickOutcome::Chosen(Dest {
                    root: cab_root.clone(),
                    folder,
                });
            }
            if ui.button("Cancel").clicked() {
                out = PickOutcome::Cancelled;
            }
        });
    });
    out
}
