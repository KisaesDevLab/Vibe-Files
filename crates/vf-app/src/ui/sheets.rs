//! "Print cover sheet" and "Print 10 blank separators" (PHASES P02 step 3).

use egui::RichText;
use vf_ingest::cover::{self, Sheet};

use crate::ui::clients::Outcome;
use crate::ui::widgets::{banner, muted, primary};

pub struct CoverDialog {
    pub client_id: String,
    pub client_name: String,
    pub ext_id: String,
    pub doc_type: String,
    pub year: Option<i64>,
    pub error: Option<String>,
}

impl CoverDialog {
    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        doc_types: &[vf_core::doctype::DocType],
    ) -> Outcome<Sheet> {
        egui::Modal::new(egui::Id::unique("cover_sheet"))
            .show(ctx, |ui| {
                ui.set_width(440.0);
                ui.label(RichText::new("Cover sheet").size(16.0).strong());
                ui.label(format!("{} ({})", self.client_name, self.ext_id));
                let l = ui.label("Doc type");
                let text = if self.doc_type.is_empty() {
                    "Any (pick at intake)".to_owned()
                } else {
                    self.doc_type.clone()
                };
                egui::ComboBox::from_id_salt("cover_doctype")
                    .selected_text(text)
                    .width(260.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.doc_type,
                            String::new(),
                            "Any (pick at intake)",
                        );
                        for t in doc_types.iter().filter(|t| !t.archived) {
                            ui.selectable_value(&mut self.doc_type, t.name.clone(), &t.name);
                        }
                    })
                    .response
                    .labelled_by(l.id);
                let l = ui.label("Tax year");
                let this_year = chrono::Datelike::year(&chrono::Local::now()) as i64;
                egui::ComboBox::from_id_salt("cover_year")
                    .selected_text(
                        self.year
                            .map_or("None (permanent)".into(), |y| y.to_string()),
                    )
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.year, None, "None (permanent)");
                        for y in (this_year - 6..=this_year + 1).rev() {
                            ui.selectable_value(&mut self.year, Some(y), y.to_string());
                        }
                    })
                    .response
                    .labelled_by(l.id);
                ui.label(muted(
                    "Put the sheet on top of the pages and scan the stack into the Scans folder; \
                     each sheet starts a new document.",
                ));
                if let Some(e) = &self.error {
                    banner(ui, e, true);
                }
                let mut out = Outcome::Open;
                ui.horizontal(|ui| {
                    if ui.add(primary("Make PDF and open it")).clicked() {
                        out = Outcome::Done(Sheet::cover(
                            &self.client_id,
                            &self.client_name,
                            &self.ext_id,
                            &self.doc_type,
                            self.year,
                        ));
                    }
                    if ui.button("Cancel").clicked() {
                        out = Outcome::Cancel;
                    }
                });
                out
            })
            .inner
    }
}

/// Writes `sheets` to `<data>/sheets/<name>.pdf` and opens it for printing.
pub fn make_and_open(
    data: &std::path::Path,
    name: &str,
    sheets: &[Sheet],
) -> Result<std::path::PathBuf, String> {
    let p = data.join("sheets").join(format!("{name}.pdf"));
    let p = filer_intake::commit::free_path(&p);
    cover::write_sheets(&p, sheets)?;
    crate::shell::open(&p)?;
    Ok(p)
}
