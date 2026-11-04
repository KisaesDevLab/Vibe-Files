//! Admin › Folder import (PHASES P10): adopt the cabinet's existing folders in place. Scan
//! proposes a client for each top-level folder and a doc type and year for each file
//! (`vf_fs::import::plan`); the person reviews the folders, then Import binds them, catalogs,
//! sets the metadata and queues OCR on a background thread (its own catalog connection). The
//! remainder is listed and exported as CSV for handling by hand. Undo this import reverses it.

use std::path::Path;
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::RichText;
use vf_core::db::Storage;
use vf_fs::import::{self, Applied, ClientPick, Options, Plan};

use crate::ui::browse::Env;
use crate::ui::{theme, widgets};

/// Rows of the remainder shown on the page (the CSV has all of them).
const SHOWN: usize = 300;

#[derive(Default)]
pub struct ImportPage {
    pub plan: Option<Plan>,
    pub rename: bool,
    job: Option<Receiver<Result<Applied, String>>>,
    /// The last import on this cabinet (from `_Vibe/import_last.json`).
    pub last: Option<Applied>,
    loaded_last: bool,
    pub notice: Option<String>,
    pub error: Option<String>,
    /// The catalog changed: Browse re-reads.
    pub changed: bool,
}

impl ImportPage {
    pub fn running(&self) -> bool {
        self.job.is_some()
    }

    fn scan(&mut self, env: &Env) {
        self.error = None;
        self.notice = None;
        match import::plan(env.conn, env.root) {
            Ok(p) => self.plan = Some(p),
            Err(e) => self.error = Some(format!("Scan failed: {e}")),
        }
    }

    fn start(&mut self, ctx: &egui::Context, catalog: &Path, root: &Path, device: &str) {
        let Some(plan) = self.plan.clone() else {
            return;
        };
        let opts = Options {
            rename: self.rename,
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let (catalog, root, device, ctx) = (
            catalog.to_path_buf(),
            root.to_path_buf(),
            device.to_string(),
            ctx.clone(),
        );
        std::thread::Builder::new()
            .name("folder-import".into())
            .spawn(move || {
                let r = vf_core::db::open(&catalog, Storage::Local)
                    .map_err(|e| e.to_string())
                    .and_then(|mut conn| {
                        let mut a = import::apply(&mut conn, &root, &plan, &opts, &device)?;
                        a.ocr_queued =
                            vf_ocr::queue::enqueue_missing(&conn, vf_ocr::queue::BACKLOG)
                                .unwrap_or(0);
                        Ok(a)
                    });
                let _ = tx.send(r);
                ctx.request_repaint();
            })
            .ok();
        self.job = Some(rx);
    }

    fn poll(&mut self) {
        let Some(rx) = &self.job else { return };
        let r = match rx.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("the import stopped".into()),
        };
        self.job = None;
        self.changed = true;
        match r {
            Ok(a) => {
                tracing::info!(
                    folders = a.bound,
                    documents = a.documents,
                    mapped = a.mapped,
                    "folder import"
                );
                self.notice = Some(format!(
                    "Imported: {} folders bound{}{}, {} documents, {} with client, doc type and year; {} PDFs queued for OCR.{}",
                    a.bound,
                    if a.renamed > 0 {
                        format!(" ({} renamed)", a.renamed)
                    } else {
                        String::new()
                    },
                    if a.new_clients > 0 {
                        format!(", {} new clients", a.new_clients)
                    } else {
                        String::new()
                    },
                    a.documents,
                    a.mapped,
                    a.ocr_queued,
                    if a.guessed > 0 {
                        format!(
                            " {} have a doc type or year guessed from a file or folder name (tag {}): they are not disposed of until someone sets them.",
                            a.guessed,
                            vf_fs::docmeta::GUESSED_TAG
                        )
                    } else {
                        String::new()
                    }
                ));
                self.last = Some(a);
                self.plan = None;
            }
            Err(e) => {
                tracing::warn!(kind = crate::logging::kind(&e), "folder import failed");
                self.error = Some(format!("Import failed: {e}"));
            }
        }
    }
}

pub fn show(ui: &mut egui::Ui, p: &mut ImportPage, env: &mut Env, catalog: &Path) {
    if !p.loaded_last {
        p.last = import::load_last(env.root);
        p.loaded_last = true;
    }
    p.poll();
    ui.heading("Folder import");
    ui.label(widgets::caption(
        "Adopts the folders already in this cabinet (a FileCenter cabinet's drawers, an old firm \
         tree) as client folders, in place: files are not moved. Each top-level folder is matched \
         to a client; each file gets a doc type and tax year where its name or folder says so.",
    ));
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let scan = ui.add_enabled(!p.running(), egui::Button::new("Scan the cabinet"));
        if scan.clicked() {
            p.scan(env);
        }
        if let Some(a) = p.last.clone()
            && !p.running()
            && ui
                .button("Undo the last import")
                .on_hover_text(format!(
                    "Removes the {} sentinels it wrote (and renames back), and the doc types and years it set. Files stay where they are.",
                    a.bound
                ))
                .clicked()
        {
            match import::undo(env.conn, env.root, &a, env.device_id) {
                Ok(()) => {
                    p.notice = Some("The import was undone.".into());
                    p.last = None;
                    p.plan = None;
                    p.changed = true;
                }
                Err(e) => p.error = Some(format!("Undo failed: {e}")),
            }
        }
        if p.running() {
            ui.spinner();
            ui.label("Importing: binding folders, cataloging and hashing files…");
        }
    });
    if let Some(n) = &p.notice {
        ui.label(RichText::new(n).color(theme::ink()));
    }
    if let Some(e) = &p.error {
        ui.label(RichText::new(e).color(crate::ui::theme::error()));
    }
    let Some(plan) = p.plan.as_mut() else {
        return;
    };
    ui.add_space(8.0);
    let mapped = plan.auto_mapped();
    let total = plan.files.len();
    let to_bind = plan
        .folders
        .iter()
        .filter(|r| r.include && !r.bound && r.client.is_some())
        .count();
    // Files in bound (or to-be-bound) folders with a doc type or year to set.
    let to_type = plan
        .files
        .iter()
        .filter(|f| {
            f.folder.is_some_and(|i| {
                let r = &plan.folders[i];
                r.bound || (r.include && r.client.is_some())
            }) && ((f.doc_type.is_some() && f.type_confidence >= import::AUTO)
                || !f.year_from.is_empty())
        })
        .count();
    ui.label(format!(
        "{} folders ({} already bound, {} to bind) · {} files · {} fully mapped ({:.0}%) · {} for manual handling",
        plan.folders.len(),
        plan.folders.iter().filter(|r| r.bound).count(),
        to_bind,
        total,
        mapped,
        if total == 0 { 0.0 } else { mapped as f64 * 100.0 / total as f64 },
        total - mapped
    ));
    ui.add_space(6.0);

    let clients = plan.clients.clone();
    egui::ScrollArea::vertical()
        .id_salt("import-folders")
        .max_height(320.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            egui::Grid::new("import-folder-grid")
                .striped(true)
                .num_columns(6)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    for h in ["Bind", "Folder", "Client", "Found by", "Sure", "Files"] {
                        ui.label(RichText::new(h).strong());
                    }
                    ui.end_row();
                    for (i, r) in plan.folders.iter_mut().enumerate() {
                        if r.bound {
                            ui.label("✓");
                        } else {
                            ui.add_enabled(
                                r.client.is_some(),
                                egui::Checkbox::without_text(&mut r.include),
                            )
                            .on_hover_text("Bind this folder to the client");
                        }
                        let folder = ui.label(&r.rel);
                        if r.bound {
                            ui.label(r.client.as_ref().map(|c| c.name.as_str()).unwrap_or("?"));
                        } else {
                            let shown = r
                                .client
                                .as_ref()
                                .map(|c| {
                                    if c.new {
                                        format!("{} ({}) — new", c.name, c.ext_id)
                                    } else {
                                        format!("{} ({})", c.name, c.ext_id)
                                    }
                                })
                                .unwrap_or_else(|| "(choose)".into());
                            let mut chosen: Option<Option<ClientPick>> = None;
                            egui::ComboBox::from_id_salt(("import-client", i))
                                .selected_text(shown)
                                .width(260.0)
                                .height(320.0)
                                .show_ui(ui, |ui| {
                                    if ui.selectable_label(r.client.is_none(), "(none)").clicked() {
                                        chosen = Some(None);
                                    }
                                    for c in &clients {
                                        let on =
                                            r.client.as_ref().is_some_and(|x| x.ext_id == c.ext_id);
                                        if ui
                                            .selectable_label(
                                                on,
                                                format!("{} ({})", c.name, c.ext_id),
                                            )
                                            .clicked()
                                        {
                                            chosen = Some(Some(c.clone()));
                                        }
                                    }
                                })
                                .response
                                .labelled_by(folder.id);
                            if let Some(c) = chosen {
                                r.include = c.is_some();
                                r.how = "chosen".into();
                                r.confidence = if c.is_some() { 1.0 } else { 0.0 };
                                r.template_name = None;
                                r.client = c;
                            }
                        }
                        ui.label(widgets::caption(&r.how));
                        ui.label(if r.bound {
                            String::new()
                        } else {
                            format!("{:.0}%", r.confidence * 100.0)
                        });
                        ui.label(r.files.to_string());
                        ui.end_row();
                    }
                });
        });
    ui.add_space(6.0);
    ui.checkbox(
        &mut p.rename,
        "Rename bound folders to the firm template's name (\"{Client} ({ID})\"); files inside are not moved",
    );
    ui.horizontal(|ui| {
        let label = if to_bind > 0 {
            format!("Import {to_bind} folders")
        } else {
            format!("Set doc types and years ({to_type} files)")
        };
        let go = ui.add_enabled(
            (to_bind > 0 || to_type > 0) && !p.running(),
            widgets::primary_button(label),
        );
        if go.clicked() {
            p.start(ui.ctx(), catalog, env.root, env.device_id);
        }
        if ui.button("Export the remainder (CSV)…").clicked()
            && let Some(plan) = &p.plan
            && let Some(dest) = rfd::FileDialog::new()
                .set_file_name("Folder import remainder.csv")
                .add_filter("CSV", &["csv"])
                .save_file()
        {
            match std::fs::write(&dest, plan.report_csv()) {
                Ok(()) => p.notice = Some(format!("Saved {}", dest.display())),
                Err(e) => p.error = Some(format!("Cannot save: {e}")),
            }
        }
    });

    let Some(plan) = &p.plan else { return };
    let rest = plan.remainder();
    ui.add_space(8.0);
    ui.label(RichText::new(format!("For manual handling: {} files", rest.len())).strong());
    ui.label(widgets::caption(
        "After the import, set these in Browse (select, then the inspector) or file them through Intake.",
    ));
    egui::ScrollArea::vertical()
        .id_salt("import-rest")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("import-rest-grid")
                .striped(true)
                .num_columns(3)
                .show(ui, |ui| {
                    for (f, gaps) in rest.iter().take(SHOWN) {
                        ui.label(&f.rel);
                        let missing: Vec<&str> = gaps.iter().map(|g| g.label()).collect();
                        ui.label(widgets::caption(missing.join(", ")));
                        ui.label(widgets::caption(&f.type_how));
                        ui.end_row();
                    }
                });
            if rest.len() > SHOWN {
                ui.label(widgets::caption(format!(
                    "… and {} more in the CSV.",
                    rest.len() - SHOWN
                )));
            }
        });
}
