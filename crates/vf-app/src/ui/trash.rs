//! Admin › Trash (PHASES P05 step 3): what was deleted, restore it, empty it with the admin password
//! (QUESTIONS Q-29). Trash older than 90 days empties on its own when a cabinet opens (Q-15);
//! documents under a legal hold stay until it is released.

use std::collections::BTreeSet;

use egui::{RichText, TextEdit};
use vf_fs::trash::{self, Trashed};

use crate::ui::browse::Env;
use crate::ui::{theme, widgets};

#[derive(Default)]
pub struct TrashPage {
    /// What is in the trash; `None`: read again.
    items: Option<Vec<Trashed>>,
    /// Ticked doc ids.
    picked: BTreeSet<String>,
    /// The admin password typed to empty the trash.
    pin: String,
    pub notice: Option<String>,
    pub error: Option<String>,
    /// The catalog changed (restore / empty): Browse re-reads.
    pub changed: bool,
    /// Restore or Empty running on its thread (each file is moved or deleted on the share;
    /// owner 2026-11-12: a spinner, not a frozen window).
    job: Option<crate::ui::job::Job<Done>>,
}

/// What Restore and Empty give back from their thread.
enum Done {
    Restored(Result<Vec<String>, String>),
    /// Documents emptied, and how many of those asked for are under a legal hold.
    Emptied(Result<usize, String>, usize),
}

impl TrashPage {
    pub fn refresh(&mut self) {
        self.items = None;
    }

    /// `work` on its own thread with its own catalog connection; here when no catalog file is
    /// known.
    fn run(
        &mut self,
        ctx: &egui::Context,
        env: &mut Env,
        catalog: Option<&std::path::Path>,
        what: String,
        work: impl FnOnce(&mut vf_core::db::Connection, &std::path::Path, &str) -> Done + Send + 'static,
    ) {
        let (root, device) = (env.root.to_path_buf(), env.device_id.to_string());
        let Some(catalog) = catalog.map(std::path::Path::to_path_buf) else {
            let d = work(env.conn, &root, &device);
            self.finished(d);
            return;
        };
        match crate::ui::job::Job::start(ctx, what, move || {
            match vf_core::db::open(&catalog, vf_core::db::Storage::Local) {
                Ok(mut conn) => work(&mut conn, &root, &device),
                Err(e) => Done::Restored(Err(e.to_string())),
            }
        }) {
            Ok(j) => self.job = Some(j),
            Err(e) => self.error = Some(e),
        }
    }

    fn finished(&mut self, d: Done) {
        match d {
            Done::Restored(Ok(placed)) => {
                self.notice = Some(format!(
                    "Restored {} documents: {}",
                    placed.len(),
                    placed.join(", ")
                ));
                self.error = None;
                self.changed = true;
                self.picked.clear();
            }
            Done::Emptied(Ok(n), held) => {
                tracing::info!(documents = n, held, "trash emptied by hand");
                self.notice = Some(if held > 0 {
                    format!("Emptied {n} documents; {held} under a legal hold stay.")
                } else {
                    format!("Emptied {n} documents.")
                });
                self.error = None;
                self.changed = true;
                self.picked.clear();
            }
            Done::Restored(Err(e)) | Done::Emptied(Err(e), _) => self.error = Some(e),
        }
        self.refresh();
    }
}

pub fn show(
    ui: &mut egui::Ui,
    p: &mut TrashPage,
    env: &mut Env,
    catalog: Option<&std::path::Path>,
) {
    if let Some(r) = crate::ui::job::take(&mut p.job) {
        p.finished(r.unwrap_or_else(|e| Done::Restored(Err(e))));
    }
    if p.items.is_none() {
        match trash::list(env.conn, env.root) {
            Ok(v) => p.items = Some(v),
            Err(e) => {
                p.error = Some(e);
                p.items = Some(vec![]);
            }
        }
        let live: BTreeSet<String> = p.items.iter().flatten().map(|t| t.doc_id.clone()).collect();
        p.picked.retain(|id| live.contains(id));
    }
    let items = p.items.clone().unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Trash").strong().size(16.0));
        ui.label(widgets::caption(format!(
            "Deleted documents, in _Vibe\\trash. Emptied on their own after {} days; held documents stay.",
            trash::AUTO_EMPTY_DAYS
        )));
    });
    ui.add_space(6.0);
    let has_pin = trash::has_pin(env.root);
    let idle = p.job.is_none();
    ui.horizontal(|ui| {
        let restore = ui.add_enabled(
            idle && !p.picked.is_empty(),
            egui::Button::new(format!("Restore {}", p.picked.len())),
        );
        if restore.clicked() {
            let chosen: Vec<Trashed> = items
                .iter()
                .filter(|t| p.picked.contains(&t.doc_id))
                .cloned()
                .collect();
            p.run(
                ui.ctx(),
                env,
                catalog,
                format!("Restoring {} documents…", chosen.len()),
                move |conn, root, device| {
                    Done::Restored(trash::restore(conn, root, device, &chosen))
                },
            );
        }
        ui.separator();
        if has_pin {
            let l = ui.label("Password");
            ui.add(
                TextEdit::singleline(&mut p.pin)
                    .password(true)
                    .desired_width(80.0),
            )
            .labelled_by(l.id);
            let what = if p.picked.is_empty() {
                "Empty trash".to_string()
            } else {
                format!("Empty {} for good", p.picked.len())
            };
            let go = ui
                .add_enabled(
                    idle && !items.is_empty() && !p.pin.is_empty(),
                    egui::Button::new(what),
                )
                .on_hover_text(
                    "Deletes the files for good (held documents stay). Cannot be undone.",
                );
            if go.clicked() {
                if !trash::check_pin(env.root, &p.pin) {
                    p.error = Some(match vf_fs::trash::pin_wait() {
                        0 => "That admin password is not right.".to_string(),
                        w => format!("Too many wrong passwords: try again in {w} s."),
                    });
                } else {
                    let chosen: Vec<Trashed> = items
                        .iter()
                        .filter(|t| p.picked.is_empty() || p.picked.contains(&t.doc_id))
                        .cloned()
                        .collect();
                    let held = chosen.iter().filter(|t| t.held).count();
                    p.run(
                        ui.ctx(),
                        env,
                        catalog,
                        format!("Emptying {} documents from the trash…", chosen.len()),
                        move |conn, root, device| {
                            Done::Emptied(
                                trash::purge(conn, root, device, &chosen, "emptied by hand"),
                                held,
                            )
                        },
                    );
                }
                p.pin.clear();
            }
        } else {
            ui.label(widgets::caption(
                "Set an admin password (Admin › Admin password) to empty the trash by hand.",
            ));
        }
    });
    if let Some(j) = &p.job {
        j.show(ui);
    }
    if let Some(n) = &p.notice {
        ui.label(RichText::new(n).color(theme::ink_secondary()));
    }
    if let Some(e) = &p.error {
        ui.label(RichText::new(e).color(crate::ui::theme::error()));
    }
    ui.separator();
    if items.is_empty() {
        ui.label(RichText::new("The trash is empty.").color(theme::ink_muted()));
        return;
    }
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            egui::Grid::new("trash-rows")
                .striped(true)
                .num_columns(4)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    for h in ["Was at", "Deleted", "Why", "Size"] {
                        ui.label(RichText::new(h).small().strong());
                    }
                    ui.end_row();
                    for t in &items {
                        let mut on = p.picked.contains(&t.doc_id);
                        ui.horizontal(|ui| {
                            let name = RichText::new(&t.rel_path).monospace().small();
                            if ui.checkbox(&mut on, name).changed() {
                                if on {
                                    p.picked.insert(t.doc_id.clone());
                                } else {
                                    p.picked.remove(&t.doc_id);
                                }
                            }
                            if t.held {
                                ui.label(
                                    RichText::new("legal hold")
                                        .small()
                                        .color(crate::ui::theme::error()),
                                );
                            }
                        });
                        ui.label(widgets::local_time(&t.deleted_at));
                        ui.label(t.reason.as_deref().unwrap_or("—"));
                        ui.label(widgets::size(t.size));
                        ui.end_row();
                    }
                });
        });
}
