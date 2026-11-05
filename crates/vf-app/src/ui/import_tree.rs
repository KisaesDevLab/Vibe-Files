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
    /// For files the scan found no doc type for (owner 2026-11-05): a doc type (id) and a
    /// source to give them.
    pub default_type: Option<String>,
    pub default_source: Option<String>,
    /// The cabinet's doc types (id, name), read at the scan.
    types: Vec<(String, String)>,
    /// A new client's ID being typed on its row (folder row → text).
    id_drafts: std::collections::HashMap<usize, String>,
    /// Folders shown: a match of at least this many % (PA-28).
    pub min_pct: u8,
    /// Folders already bound are not shown.
    pub hide_bound: bool,
    /// Folders with no client are not shown.
    pub hide_none: bool,
    /// The row whose Bind box was clicked last, for Shift-click ranges.
    last_tick: Option<usize>,
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
            Ok(p) => {
                self.plan = Some(p);
                self.last_tick = None;
                self.id_drafts.clear();
                self.types = vf_fs::doctypes::load(env.root)
                    .ok()
                    .flatten()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|t| !t.archived)
                    .map(|t| (t.id, t.name))
                    .collect();
            }
            Err(e) => self.error = Some(format!("Scan failed: {e}")),
        }
    }

    fn start(&mut self, ctx: &egui::Context, catalog: &Path, root: &Path, device: &str) {
        let Some(plan) = self.plan.clone() else {
            return;
        };
        let opts = Options {
            rename: self.rename,
            default_doc_type: self.default_type.clone(),
            default_source: self.default_source.clone(),
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

/// Whether the filter shows folder row `r`: matched at least `min_pct` (bound folders always
/// pass it), and not hidden as bound or as matching no client.
fn shows(r: &import::FolderRow, min_pct: u8, hide_bound: bool, hide_none: bool) -> bool {
    if r.bound {
        return !hide_bound;
    }
    if r.client.is_none() {
        return !hide_none && min_pct == 0;
    }
    (r.confidence * 100.0).round() >= f32::from(min_pct)
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
                || !f.year_from.is_empty()
                || p.default_type.is_some()
                || p.default_source.is_some())
        })
        .count();
    // Files the scan found no doc type for: the defaults below are for them.
    let untyped = plan
        .files
        .iter()
        .filter(|f| f.doc_type.is_none() || f.type_confidence < import::AUTO)
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

    // No client list, or folders no client matches (Q-63): point to the CSV import, and offer
    // a client made from each folder's name.
    let unmatched = plan
        .folders
        .iter()
        .filter(|r| !r.bound && r.client.is_none())
        .count();
    if plan.clients.is_empty() {
        ui.label(
            RichText::new(
                "No client list is imported yet. Import it first (Browse › Import clients CSV…) so folders are matched to your clients by name and ID, then scan again; or create clients from the folder names below.",
            )
            .color(theme::warn()),
        );
    }
    if unmatched > 0 {
        let make = ui
            .button(format!("Create a client for each unmatched folder ({unmatched})"))
            .on_hover_text(
                "The folder name becomes the client name and an ID is made from it (you can change it on the row); the clients are added when you import.",
            );
        if make.clicked() {
            p.id_drafts.clear();
            match import::new_clients_from_folders(env.root, plan) {
                Ok(n) => p.notice = Some(format!("{n} new clients proposed; check their IDs.")),
                Err(e) => p.error = Some(e),
            }
        }
    }
    // Filter by match % and kind (PA-28); ticking acts on the folders shown.
    let shown: Vec<usize> = (0..plan.folders.len())
        .filter(|&i| shows(&plan.folders[i], p.min_pct, p.hide_bound, p.hide_none))
        .collect();
    let can_tick = |r: &import::FolderRow| !r.bound && r.client.is_some();
    ui.horizontal_wrapped(|ui| {
        ui.label("Show matches from");
        ui.add(
            egui::DragValue::new(&mut p.min_pct)
                .range(0..=100)
                .speed(1.0)
                .suffix("%"),
        )
        .on_hover_text("Only folders matched at least this well");
        ui.label("up");
        ui.checkbox(&mut p.hide_bound, "Hide already bound");
        ui.checkbox(&mut p.hide_none, "Hide folders with no match");
        ui.label(widgets::caption(format!(
            "Showing {} of {} folders",
            shown.len(),
            plan.folders.len()
        )));
    });
    ui.horizontal(|ui| {
        let tickable: Vec<usize> = shown
            .iter()
            .copied()
            .filter(|&i| can_tick(&plan.folders[i]))
            .collect();
        if ui
            .add_enabled(!tickable.is_empty(), egui::Button::new("Tick all shown"))
            .clicked()
        {
            for &i in &tickable {
                plan.folders[i].include = true;
            }
        }
        if ui
            .add_enabled(!tickable.is_empty(), egui::Button::new("Untick all shown"))
            .clicked()
        {
            for &i in &tickable {
                plan.folders[i].include = false;
            }
        }
    });
    let shift = ui.input(|i| i.modifiers.shift);
    let mut id_edit: Option<(usize, String)> = None;

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
                    // Bind header: ticks or unticks every folder shown.
                    let tickable: Vec<usize> = shown
                        .iter()
                        .copied()
                        .filter(|&i| can_tick(&plan.folders[i]))
                        .collect();
                    let mut all = !tickable.is_empty()
                        && tickable.iter().all(|&i| plan.folders[i].include);
                    let mixed = !all && tickable.iter().any(|&i| plan.folders[i].include);
                    let head = ui
                        .add_enabled(
                            !tickable.is_empty(),
                            egui::Checkbox::new(&mut all, RichText::new("Bind").strong())
                                .indeterminate(mixed),
                        )
                        .on_hover_text("Tick or untick every folder shown");
                    if head.changed() {
                        for &i in &tickable {
                            plan.folders[i].include = all;
                        }
                    }
                    for h in ["Folder", "Client", "Found by", "Match", "Files"] {
                        ui.label(RichText::new(h).strong());
                    }
                    ui.end_row();
                    let mut ticked: Option<(usize, bool)> = None;
                    for &i in &shown {
                        let r = &mut plan.folders[i];
                        if r.bound {
                            ui.label("✓");
                        } else {
                            let resp = ui
                                .add_enabled(
                                    r.client.is_some(),
                                    egui::Checkbox::without_text(&mut r.include),
                                )
                                .on_hover_text(
                                    "Bind this folder to the client (Shift-click: the folders between too)",
                                );
                            if resp.changed() {
                                ticked = Some((i, r.include));
                            }
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
                            let current = r
                                .client
                                .as_ref()
                                .and_then(|x| clients.iter().position(|c| c.ext_id == x.ext_id));
                            let (cr, pick) = widgets::search_combo(
                                ui,
                                ("import-client", i),
                                260.0,
                                shown,
                                || {
                                    clients
                                        .iter()
                                        .map(|c| format!("{} ({})", c.name, c.ext_id))
                                        .collect()
                                },
                                current,
                                Some("(none)"),
                            );
                            cr.labelled_by(folder.id);
                            if let Some(p) = pick {
                                chosen = Some(p.map(|k| clients[k].clone()));
                            }
                            // A new client's ID, typed on the row (Q-63); Enter or leaving the
                            // box sets it.
                            if let Some(c) = r.client.as_ref().filter(|c| c.new) {
                                let d = p.id_drafts.entry(i).or_insert_with(|| c.ext_id.clone());
                                let name = format!("New client ID for {}", r.rel);
                                let resp = ui
                                    .add(egui::TextEdit::singleline(d).desired_width(90.0))
                                    .on_hover_text("This new client's ID");
                                resp.widget_info(|| {
                                    egui::WidgetInfo::labeled(
                                        egui::accesskit::Role::TextInput,
                                        true,
                                        &name,
                                    )
                                });
                                if resp.lost_focus() && !d.eq_ignore_ascii_case(&c.ext_id) {
                                    id_edit = Some((i, d.clone()));
                                }
                            }
                            if let Some(c) = chosen {
                                r.include = c.is_some();
                                r.how = "chosen".into();
                                r.confidence = if c.is_some() { 1.0 } else { 0.0 };
                                // The template's name for the client picked (Rename uses it).
                                r.template_name = c
                                    .as_ref()
                                    .and_then(|c| import::template_name_for(env.root, c, &r.rel));
                                r.client = c;
                                p.id_drafts.remove(&i);
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
                    if let Some((i, on)) = ticked {
                        // Shift-click: the shown folders between the last one clicked and this.
                        if shift
                            && let Some(a) = p.last_tick
                            && let (Some(pa), Some(pi)) = (
                                shown.iter().position(|&x| x == a),
                                shown.iter().position(|&x| x == i),
                            )
                        {
                            for &j in &shown[pa.min(pi)..=pa.max(pi)] {
                                if can_tick(&plan.folders[j]) {
                                    plan.folders[j].include = on;
                                }
                            }
                        }
                        p.last_tick = Some(i);
                    }
                });
        });
    if let Some((i, id)) = id_edit {
        if let Err(e) = import::set_new_id(env.root, plan, i, &id) {
            p.error = Some(e);
        }
        p.id_drafts.remove(&i);
    }
    ui.add_space(6.0);
    ui.checkbox(
        &mut p.rename,
        "Rename bound folders to the firm template's name (\"{Client} ({ID})\"); files inside are not moved",
    );
    if untyped > 0 {
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("{untyped} files with no doc type found get:"));
            let l = ui.label(widgets::caption("Doc type"));
            let shown = p
                .default_type
                .as_ref()
                .and_then(|id| p.types.iter().find(|(i, _)| i == id))
                .map_or("Leave unset".to_string(), |(_, n)| n.clone());
            let current = p
                .default_type
                .as_ref()
                .and_then(|id| p.types.iter().position(|(i, _)| i == id));
            let types = &p.types;
            let (r, pick) = widgets::search_combo(
                ui,
                "import-default-type",
                200.0,
                shown,
                || types.iter().map(|(_, n)| n.clone()).collect(),
                current,
                Some("Leave unset"),
            );
            r.labelled_by(l.id).on_hover_text(
                "Given to every file the scan found no doc type for; flagged to check",
            );
            if let Some(k) = pick {
                p.default_type = k.map(|k| p.types[k].0.clone());
            }
            let l = ui.label(widgets::caption("Source"));
            let sources = &vf_fs::docmeta::SOURCES;
            let shown = p
                .default_source
                .as_deref()
                .and_then(|s| sources.iter().find(|(k, _)| *k == s))
                .map_or("Leave unset", |(_, n)| *n)
                .to_string();
            let current = p
                .default_source
                .as_deref()
                .and_then(|s| sources.iter().position(|(k, _)| *k == s));
            let (r, pick) = widgets::search_combo(
                ui,
                "import-default-source",
                140.0,
                shown,
                || sources.iter().map(|(_, n)| n.to_string()).collect(),
                current,
                Some("Leave unset"),
            );
            r.labelled_by(l.id);
            if let Some(k) = pick {
                p.default_source = k.map(|k| sources[k].0.to_string());
            }
        });
    }
    ui.horizontal(|ui| {
        let label = if to_bind > 0 {
            // Ticked folders the filter hides are imported too: say so.
            let hidden = p.plan.as_ref().map_or(0, |plan| {
                plan.folders
                    .iter()
                    .filter(|r| {
                        r.include
                            && !r.bound
                            && r.client.is_some()
                            && !shows(r, p.min_pct, p.hide_bound, p.hide_none)
                    })
                    .count()
            });
            if hidden > 0 {
                format!("Import {to_bind} folders ({hidden} not shown)")
            } else {
                format!("Import {to_bind} folders")
            }
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
