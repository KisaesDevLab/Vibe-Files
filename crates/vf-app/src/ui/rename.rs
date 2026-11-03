//! `Rename…` (P01 step 6): a Filer filename template applied to the selected documents, with a
//! preview of every new name before one Transaction renames them (`vf_fs::rename`).

use egui::{RichText, TextEdit};
use vf_fs::rename::{self, DEFAULT_TEMPLATE, Outcome, Row};

use crate::ui::browse::Env;
use crate::ui::{theme, wizard::ERROR};

pub struct RenameDialog {
    pub ids: Vec<String>,
    pub template: String,
    /// Recomputed when the template changes.
    preview: Option<Result<Vec<Row>, String>>,
    pub error: Option<String>,
}

impl RenameDialog {
    pub fn new(ids: Vec<String>) -> Self {
        Self {
            ids,
            template: DEFAULT_TEMPLATE.into(),
            preview: None,
            error: None,
        }
    }
}

pub enum DialogOutcome {
    Open,
    Cancelled,
    Renamed(usize),
}

fn file_name(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

pub fn show(ctx: &egui::Context, d: &mut RenameDialog, env: &mut Env) -> DialogOutcome {
    if d.preview.is_none() {
        let year = chrono::Datelike::year(&chrono::Local::now());
        d.preview = Some(rename::plan(env.conn, env.root, &d.ids, &d.template, year));
    }
    let mut out = DialogOutcome::Open;
    egui::Modal::new(egui::Id::unique("rename-docs")).show(ctx, |ui| {
        ui.set_width(720.0);
        ui.heading(format!("Rename {} documents", d.ids.len()));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let l = ui.label("File name template");
            let r = ui
                .add(
                    TextEdit::singleline(&mut d.template)
                        .font(egui::TextStyle::Monospace)
                        .desired_width(420.0),
                )
                .labelled_by(l.id);
            if r.changed() {
                d.preview = None;
                d.error = None;
            }
        });
        ui.label(
            RichText::new(
                "Tokens: {Client} {ID} {Year} {DocType} {Rest} {Original} {Ext}, and the client \
                 list's columns. Files stay in their folders; a taken name gets \" (2)\".",
            )
            .small()
            .color(theme::INK_MUTED),
        );
        ui.add_space(8.0);
        let mut renames = 0;
        // Fixed size: an auto-sized modal re-centers and moves its buttons.
        egui::ScrollArea::vertical()
            .id_salt("rename-preview")
            .min_scrolled_height(300.0)
            .max_height(300.0)
            .auto_shrink([false, false])
            .show(ui, |ui| match &d.preview {
                Some(Ok(rows)) => {
                    egui::Grid::new("rename-rows")
                        .num_columns(3)
                        .striped(true)
                        .spacing([12.0, 4.0])
                        .show(ui, |ui| {
                            for h in ["NOW", "NEW NAME", ""] {
                                ui.label(RichText::new(h).small().color(theme::INK_MUTED));
                            }
                            ui.end_row();
                            for r in rows {
                                ui.label(RichText::new(file_name(&r.old_rel)).monospace().small());
                                let new = r.new_rel.as_deref().map(file_name).unwrap_or("—");
                                ui.label(RichText::new(new).monospace().small());
                                let note = match &r.outcome {
                                    Outcome::Rename => String::new(),
                                    Outcome::KeepBoth => "name taken: keep both".into(),
                                    Outcome::Same => "no change".into(),
                                    Outcome::Skip(why) => format!("skipped: {why}"),
                                };
                                ui.label(RichText::new(note).small().color(theme::INK_MUTED));
                                ui.end_row();
                                if r.renames() {
                                    renames += 1;
                                }
                            }
                        });
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(theme::INK_MUTED));
                }
                None => {}
            });
        ui.add_space(6.0);
        let msg = d.error.as_deref().unwrap_or("");
        ui.add_sized(
            [ui.available_width(), 36.0],
            egui::Label::new(RichText::new(msg).color(ERROR)).wrap(),
        );
        ui.horizontal(|ui| {
            let go = ui.add_enabled(
                renames > 0,
                egui::Button::new(
                    RichText::new(format!("Rename {renames} files (one transaction)"))
                        .color(egui::Color32::WHITE),
                )
                .fill(theme::ACCENT_DEFAULT),
            );
            if go.clicked()
                && let Some(Ok(rows)) = &d.preview
            {
                match rename::commit(env.conn, env.root, rows, env.device_id) {
                    Ok((_, n)) => out = DialogOutcome::Renamed(n),
                    Err(e) => {
                        d.error = Some(e);
                        d.preview = None; // the disk may have changed: preview again
                    }
                }
            }
            if ui.button("Cancel").clicked() {
                out = DialogOutcome::Cancelled;
            }
        });
    });
    out
}
