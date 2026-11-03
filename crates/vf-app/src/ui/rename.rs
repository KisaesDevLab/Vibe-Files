//! `Rename…` (P01 step 6): a Filer filename template applied to the selected documents, with a
//! preview of every new name before one Transaction renames them (`vf_fs::rename`).

use egui::{RichText, TextEdit};
use vf_fs::rename::{DEFAULT_TEMPLATE, Outcome, Planner, Row};

use crate::ui::browse::Env;
use crate::ui::widgets::{self, DialogOutcome};

const ROW_H: f32 = 18.0;

pub struct RenameDialog {
    pub ids: Vec<String>,
    pub template: String,
    /// Documents, tokens and folder listings, read once when the dialog first draws.
    planner: Option<Result<Planner, String>>,
    /// Rows for the current template and how many of them rename; recomputed when it changes.
    preview: Option<Result<(Vec<Row>, usize), String>>,
    pub error: Option<String>,
}

impl RenameDialog {
    pub fn new(ids: Vec<String>) -> Self {
        Self {
            ids,
            template: DEFAULT_TEMPLATE.into(),
            planner: None,
            preview: None,
            error: None,
        }
    }
}

fn note(o: &Outcome) -> String {
    match o {
        Outcome::Rename(_) => String::new(),
        Outcome::KeepBoth(_) => "name taken: keep both".into(),
        Outcome::Same => "no change".into(),
        Outcome::Skip(why) => format!("skipped: {why}"),
    }
}

/// Returns the number of files renamed when done.
pub fn show(ctx: &egui::Context, d: &mut RenameDialog, env: &mut Env) -> DialogOutcome<usize> {
    if d.planner.is_none() {
        let year = chrono::Datelike::year(&chrono::Local::now());
        d.planner = Some(Planner::load(env.conn, env.root, &d.ids, year));
    }
    if d.preview.is_none() {
        d.preview = Some(match d.planner.as_ref().expect("loaded above") {
            Ok(p) => p.plan(env.root, &d.template).map(|rows| {
                let n = rows.iter().filter(|r| r.renames()).count();
                (rows, n)
            }),
            Err(e) => Err(e.clone()),
        });
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
        ui.label(widgets::caption(
            "Tokens: {Client} {ID} {Year} {DocType} {Rest} {Original} {Ext}, and the client \
             list's columns. Files stay in their folders; a taken name gets \" (2)\".",
        ));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            for (h, w) in [("NOW", 250.0), ("NEW NAME", 250.0), ("", 180.0)] {
                ui.add_sized([w, ROW_H], egui::Label::new(widgets::caption(h)));
            }
        });
        let renames = match &d.preview {
            Some(Ok((rows, n))) => {
                // Only the visible rows are laid out (a selection can be the whole cabinet).
                egui::ScrollArea::vertical()
                    .id_salt("rename-preview")
                    .min_scrolled_height(300.0)
                    .max_height(300.0)
                    .auto_shrink([false, false])
                    .show_rows(ui, ROW_H, rows.len(), |ui, range| {
                        for r in &rows[range] {
                            ui.horizontal(|ui| {
                                let mono = |t: &str| RichText::new(t).monospace().small();
                                ui.add_sized(
                                    [250.0, ROW_H],
                                    egui::Label::new(mono(widgets::file_name(&r.old_rel)))
                                        .truncate(),
                                );
                                let new = r.new_rel().map(widgets::file_name).unwrap_or("—");
                                ui.add_sized(
                                    [250.0, ROW_H],
                                    egui::Label::new(mono(new)).truncate(),
                                );
                                ui.add_sized(
                                    [180.0, ROW_H],
                                    egui::Label::new(widgets::caption(note(&r.outcome))).truncate(),
                                );
                            });
                        }
                    });
                *n
            }
            Some(Err(e)) => {
                widgets::fixed_body(ui, "rename-preview", 300.0, |ui| {
                    ui.label(widgets::caption(e.as_str()));
                });
                0
            }
            None => 0,
        };
        ui.add_space(6.0);
        widgets::message_line(ui, d.error.as_deref());
        ui.horizontal(|ui| {
            let go = ui.add_enabled(
                renames > 0,
                widgets::primary_button(format!("Rename {renames} files (one transaction)")),
            );
            if go.clicked()
                && let Some(Ok((rows, _))) = &d.preview
            {
                match vf_fs::rename::commit(env.conn, env.root, rows, env.device_id) {
                    Ok((_, n)) => out = DialogOutcome::Done(n),
                    Err(e) => {
                        d.error = Some(e);
                        // The disk may have changed: read it again.
                        d.planner = None;
                        d.preview = None;
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
