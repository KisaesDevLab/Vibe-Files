//! Admin › Trash (PHASES P05 step 3): what was deleted, restore it, empty it with the firm PIN
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
    pin: String,
    /// Setting or changing the PIN.
    new_pin: String,
    new_pin_again: String,
    changing: bool,
    pub notice: Option<String>,
    pub error: Option<String>,
    /// The catalog changed (restore / empty): Browse re-reads.
    pub changed: bool,
}

impl TrashPage {
    pub fn refresh(&mut self) {
        self.items = None;
    }
}

pub fn show(ui: &mut egui::Ui, p: &mut TrashPage, env: &mut Env) {
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
    ui.horizontal(|ui| {
        let restore = ui.add_enabled(
            !p.picked.is_empty(),
            egui::Button::new(format!("Restore {}", p.picked.len())),
        );
        if restore.clicked() {
            let chosen: Vec<Trashed> = items
                .iter()
                .filter(|t| p.picked.contains(&t.doc_id))
                .cloned()
                .collect();
            match trash::restore(env.conn, env.root, env.device_id, &chosen) {
                Ok(placed) => {
                    p.notice = Some(format!(
                        "Restored {} documents: {}",
                        placed.len(),
                        placed.join(", ")
                    ));
                    p.error = None;
                    p.changed = true;
                    p.picked.clear();
                    p.refresh();
                }
                Err(e) => p.error = Some(e),
            }
        }
        ui.separator();
        if has_pin {
            let l = ui.label("Firm PIN");
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
                    !items.is_empty() && !p.pin.is_empty(),
                    egui::Button::new(what),
                )
                .on_hover_text(
                    "Deletes the files for good (held documents stay). Cannot be undone.",
                );
            if go.clicked() {
                if !trash::check_pin(env.root, &p.pin) {
                    p.error = Some(match vf_fs::trash::pin_wait() {
                        0 => "That PIN is not right.".to_string(),
                        w => format!("Too many wrong PINs: try again in {w} s."),
                    });
                } else {
                    let chosen: Vec<Trashed> = items
                        .iter()
                        .filter(|t| p.picked.is_empty() || p.picked.contains(&t.doc_id))
                        .cloned()
                        .collect();
                    let held = chosen.iter().filter(|t| t.held).count();
                    match trash::purge(
                        env.conn,
                        env.root,
                        env.device_id,
                        &chosen,
                        "emptied by hand",
                    ) {
                        Ok(n) => {
                            tracing::info!(documents = n, held, "trash emptied by hand");
                            p.notice = Some(if held > 0 {
                                format!("Emptied {n} documents; {held} under a legal hold stay.")
                            } else {
                                format!("Emptied {n} documents.")
                            });
                            p.error = None;
                            p.changed = true;
                            p.picked.clear();
                        }
                        Err(e) => p.error = Some(e),
                    }
                    p.refresh();
                }
                p.pin.clear();
            }
            if ui.small_button("Change PIN").clicked() {
                p.changing = !p.changing;
            }
        } else {
            ui.label(widgets::caption(
                "Set a firm PIN to empty the trash by hand (anyone who knows it can).",
            ));
        }
    });
    if !has_pin || p.changing {
        ui.horizontal(|ui| {
            if has_pin {
                let l = ui.label("Current PIN");
                ui.add(
                    TextEdit::singleline(&mut p.pin)
                        .password(true)
                        .desired_width(80.0),
                )
                .labelled_by(l.id);
            }
            let l = ui.label("New PIN");
            ui.add(
                TextEdit::singleline(&mut p.new_pin)
                    .password(true)
                    .desired_width(80.0),
            )
            .labelled_by(l.id);
            let l = ui.label("Again");
            ui.add(
                TextEdit::singleline(&mut p.new_pin_again)
                    .password(true)
                    .desired_width(80.0),
            )
            .labelled_by(l.id);
            if ui.button("Set PIN").clicked() {
                if p.new_pin != p.new_pin_again {
                    p.error = Some("The two PINs differ.".into());
                } else {
                    let current = has_pin.then_some(p.pin.as_str());
                    match trash::set_pin(env.conn, env.root, env.device_id, current, &p.new_pin) {
                        Ok(()) => {
                            tracing::info!("firm PIN set");
                            p.notice = Some("The firm PIN is set.".into());
                            p.error = None;
                            p.changing = false;
                        }
                        Err(e) => p.error = Some(e),
                    }
                }
                p.pin.clear();
                p.new_pin.clear();
                p.new_pin_again.clear();
            }
        });
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
