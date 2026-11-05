//! Admin › Merge cabinets (owner 2026-11-05, Q-62, Q-65): another client cabinet's folders
//! moved into this one (`vf_fs::merge`): scan, check the clients, merge on a thread with a
//! count, undo the last merge.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::RichText;
use vf_core::db::Storage;
use vf_fs::merge::{self, MergePlan, Merged};

use crate::ui::browse::Env;
use crate::ui::{theme, widgets};

/// A merge running: its result, files done, files in all.
type Job = (Receiver<Result<Merged, String>>, Arc<AtomicUsize>, usize);

#[derive(Default)]
pub struct MergePage {
    pub source: String,
    pub plan: Option<MergePlan>,
    /// "Merge" pressed: asked to confirm.
    confirm: bool,
    job: Option<Job>,
    pub last: Option<Merged>,
    loaded_last: bool,
    pub notice: Option<String>,
    pub error: Option<String>,
    /// The catalog changed: Browse re-reads.
    pub changed: bool,
}

impl MergePage {
    pub fn running(&self) -> bool {
        self.job.is_some()
    }

    fn start(&mut self, ctx: &egui::Context, catalog: &Path, root: &Path, device: &str) {
        let Some(plan) = self.plan.clone() else {
            return;
        };
        let total = plan
            .clients
            .iter()
            .filter(|c| c.include)
            .map(|c| c.files.len())
            .sum();
        let done = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = std::sync::mpsc::channel();
        let (catalog, root, device, ctx, d) = (
            catalog.to_path_buf(),
            root.to_path_buf(),
            device.to_string(),
            ctx.clone(),
            done.clone(),
        );
        std::thread::Builder::new()
            .name("cabinet-merge".into())
            .spawn(move || {
                let r = vf_core::db::open(&catalog, Storage::Local)
                    .map_err(|e| e.to_string())
                    .and_then(|mut conn| merge::apply(&mut conn, &root, &plan, &device, &d));
                let _ = tx.send(r);
                ctx.request_repaint();
            })
            .ok();
        self.job = Some((rx, done, total));
    }

    fn poll(&mut self) {
        let Some((rx, _, _)) = &self.job else { return };
        let r = match rx.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("the merge stopped".into()),
        };
        self.job = None;
        self.changed = true;
        self.plan = None;
        match r {
            Ok(m) => {
                self.notice = Some(format!(
                    "Merged {} clients ({} new): {} files moved, {} kept beside a different file of the same name, {} already here (left in the source).{}",
                    m.clients,
                    m.new_clients,
                    m.moved,
                    m.renamed,
                    m.left_same,
                    if m.failed.is_empty() {
                        String::new()
                    } else {
                        format!(" {} could not be read and were left.", m.failed.len())
                    }
                ));
                self.last = Some(m);
            }
            Err(e) => self.error = Some(format!("The merge stopped: {e}")),
        }
    }
}

pub fn show(ui: &mut egui::Ui, p: &mut MergePage, env: &mut Env, catalog: &Path) {
    if !p.loaded_last {
        p.last = merge::load_last(env.root);
        p.loaded_last = true;
    }
    p.poll();
    ui.heading("Merge cabinets");
    ui.label(widgets::caption(
        "Moves another Vibe Files cabinet's client folders into this one. A client with the same ID here is the same client; its files go to the same sub-folders of its folder here. A different file already at the same place: the incoming one gets \" (2)\". The same file already here: left in the other cabinet. Each document keeps its doc type, year and tags; the other cabinet's audit trail is kept as history. Folders left empty there are removed.",
    ));
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let l = ui.label("Other cabinet's folder");
        ui.add(egui::TextEdit::singleline(&mut p.source).desired_width(360.0))
            .labelled_by(l.id);
        if ui.button("Browse…").clicked()
            && let Some(d) = rfd::FileDialog::new().pick_folder()
        {
            p.source = d.display().to_string();
        }
        if ui
            .add_enabled(
                !p.running() && !p.source.trim().is_empty(),
                egui::Button::new("Scan"),
            )
            .clicked()
        {
            p.error = None;
            p.notice = None;
            match merge::plan(env.conn, env.root, &PathBuf::from(p.source.trim())) {
                Ok(pl) => p.plan = Some(pl),
                Err(e) => p.error = Some(e),
            }
        }
        if let Some(m) = p.last.clone()
            && !p.running()
            && ui
                .button("Undo the last merge")
                .on_hover_text(format!(
                    "Moves the {} files back to the other cabinet",
                    m.moved
                ))
                .clicked()
        {
            match merge::undo(env.conn, env.root, &m, env.device_id) {
                Ok(()) => {
                    p.notice = Some(
                        "The merge was undone: the files are back in the other cabinet.".into(),
                    );
                    p.last = None;
                    p.changed = true;
                }
                Err(e) => p.error = Some(format!("Undo failed: {e}")),
            }
        }
    });
    if let Some(n) = &p.notice {
        ui.label(RichText::new(n).color(theme::ink()));
    }
    if let Some(e) = &p.error {
        ui.label(RichText::new(e).color(theme::error()));
    }
    if let Some((_, done, total)) = &p.job {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(format!(
                "Merging: {} of {total} files…",
                done.load(Ordering::Relaxed)
            ));
        });
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(250));
        return;
    }
    let Some(plan) = p.plan.as_mut() else {
        return;
    };
    ui.add_space(8.0);
    let ticked = plan.clients.iter().filter(|c| c.include).count();
    let files: usize = plan
        .clients
        .iter()
        .filter(|c| c.include)
        .map(|c| c.files.len())
        .sum();
    ui.label(format!(
        "{} client folders ({} new clients here) · {} files{}{}",
        plan.clients.len(),
        plan.clients.iter().filter(|c| c.new_client).count(),
        plan.clients.iter().map(|c| c.files.len()).sum::<usize>(),
        if plan.unbound.is_empty() {
            String::new()
        } else {
            format!(
                " · {} folders are not client folders and stay there ({})",
                plan.unbound.len(),
                plan.unbound.join(", ")
            )
        },
        if plan.loose > 0 {
            format!(" · {} loose files at its top stay there", plan.loose)
        } else {
            String::new()
        }
    ));
    ui.horizontal(|ui| {
        if ui.button("Tick all").clicked() {
            plan.clients.iter_mut().for_each(|c| c.include = true);
        }
        if ui.button("Untick all").clicked() {
            plan.clients.iter_mut().for_each(|c| c.include = false);
        }
    });
    egui::ScrollArea::vertical()
        .id_salt("merge-clients")
        .max_height(360.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            egui::Grid::new("merge-grid")
                .striped(true)
                .num_columns(5)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    for h in [
                        "Merge",
                        "Their folder",
                        "Client here",
                        "Files",
                        "Already here",
                    ] {
                        ui.label(RichText::new(h).strong());
                    }
                    ui.end_row();
                    for c in plan.clients.iter_mut() {
                        let name = format!("Merge {}", c.src_folder);
                        ui.checkbox(&mut c.include, "").widget_info(|| {
                            egui::WidgetInfo::labeled(egui::accesskit::Role::CheckBox, true, &name)
                        });
                        ui.label(&c.src_folder);
                        ui.label(match &c.dest_folder {
                            Some(f) => format!("{} ({}) · {f}", c.name, c.ext_id),
                            None if c.new_client => {
                                format!("{} ({}) — new client", c.name, c.ext_id)
                            }
                            None => format!("{} ({}) — folder made", c.name, c.ext_id),
                        });
                        ui.label(c.files.len().to_string());
                        ui.label(if c.clashes > 0 {
                            c.clashes.to_string()
                        } else {
                            String::new()
                        });
                        ui.end_row();
                    }
                });
        });
    ui.add_space(6.0);
    let mut go = false;
    if p.confirm {
        ui.label(
            RichText::new(format!(
                "Move {files} files of {ticked} clients out of the other cabinet into this one?"
            ))
            .strong(),
        );
        ui.horizontal(|ui| {
            if ui.add(widgets::primary_button("Yes, merge")).clicked() {
                p.confirm = false;
                go = true;
            }
            if ui.button("Cancel").clicked() {
                p.confirm = false;
            }
        });
    } else if ui
        .add_enabled(
            ticked > 0,
            widgets::primary_button(format!("Merge {ticked} clients ({files} files)")),
        )
        .clicked()
    {
        p.confirm = true;
    }
    if go {
        p.start(ui.ctx(), catalog, env.root, env.device_id);
    }
}
