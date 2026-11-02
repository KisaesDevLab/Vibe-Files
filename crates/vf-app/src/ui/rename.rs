//! Bulk rename dialog (PHASES P01 step 6): Filer token template → preview → one transaction.

use egui::RichText;
use vf_fs::rename::{self, RenameRow};

use crate::ui::clients::Outcome;
use crate::ui::theme;
use crate::ui::widgets::{banner, mono, muted, primary};

pub struct RenameDialog {
    pub ids: Vec<String>,
    extra_tokens: Vec<String>,
    pub template: String,
    pub rows: Option<Vec<RenameRow>>,
    previewed: String,
    pub error: Option<String>,
}

pub enum Step {
    Preview,
    Commit,
}

impl RenameDialog {
    pub fn new(ids: Vec<String>, extra_tokens: Vec<String>) -> Self {
        Self {
            ids,
            extra_tokens,
            template: rename::DEFAULT_TEMPLATE.into(),
            rows: None,
            previewed: String::new(),
            error: None,
        }
    }

    pub fn set_preview(&mut self, rows: Vec<RenameRow>) {
        self.previewed = self.template.clone();
        self.rows = Some(rows);
    }

    pub fn ui(&mut self, ctx: &egui::Context) -> Outcome<Step> {
        egui::Modal::new(egui::Id::unique("bulk_rename"))
            .show(ctx, |ui| {
                ui.set_width(760.0_f32.min(ctx.content_rect().width() - 48.0));
                ui.label(RichText::new(format!("Rename {} documents", self.ids.len())).size(16.0).strong());
                let l = ui.label("File name template");
                ui.add(
                    egui::TextEdit::singleline(&mut self.template)
                        .font(egui::TextStyle::Monospace)
                        .desired_width(f32::INFINITY),
                )
                .labelled_by(l.id);
                ui.label(muted(
                    "Tokens (Filer Desktop): {Client} {ID} {Year} {DocType} {Rest} {Original} {Ext} and CSV columns. Keep {Ext}.",
                ));
                let problems = rename::check_template(&self.template, &self.extra_tokens);
                for p in &problems {
                    banner(ui, p, true);
                }
                let stale = self.previewed != self.template;
                let mut out = Outcome::Open;
                if let (Some(rows), false) = (&self.rows, stale) {
                    let n = rows.iter().filter(|r| r.will_rename()).count();
                    ui.label(muted(format!(
                        "{n} will be renamed · {} kept both · {} cannot be renamed",
                        rows.iter().filter(|r| r.kept_both).count(),
                        rows.iter().filter(|r| r.to_rel.is_err()).count()
                    )));
                    egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                        egui::Grid::new("rename_preview").num_columns(2).striped(true).show(ui, |ui| {
                            for r in rows {
                                ui.label(mono(r.from_rel.rsplit('/').next().unwrap_or("")).small());
                                match &r.to_rel {
                                    Ok(t) if *t == r.from_rel => {
                                        ui.label(muted("unchanged"));
                                    }
                                    Ok(t) => {
                                        let name = t.rsplit('/').next().unwrap_or("");
                                        let mut text = mono(format!("→ {name}")).small();
                                        if r.kept_both {
                                            text = text.color(theme::WARN_FG);
                                        }
                                        ui.label(text);
                                    }
                                    Err(e) => {
                                        ui.label(RichText::new(e).small().color(theme::ERR_FG));
                                    }
                                }
                                ui.end_row();
                            }
                        });
                    });
                    if let Some(e) = &self.error {
                        banner(ui, e, true);
                    }
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(n > 0, primary(&format!("Rename {n} (one transaction)")))
                            .clicked()
                        {
                            out = Outcome::Done(Step::Commit);
                        }
                        if ui.button("Cancel").clicked() {
                            out = Outcome::Cancel;
                        }
                    });
                } else {
                    ui.horizontal(|ui| {
                        if ui.add_enabled(problems.is_empty(), primary("Preview")).clicked() {
                            out = Outcome::Done(Step::Preview);
                        }
                        if ui.button("Cancel").clicked() {
                            out = Outcome::Cancel;
                        }
                    });
                }
                out
            })
            .inner
    }
}
