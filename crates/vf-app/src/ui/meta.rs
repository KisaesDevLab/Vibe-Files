//! Inspector metadata fields (PHASES P01 step 5): doc type, tax year, status, source, tags over
//! the selection. A field the selection disagrees on shows "Mixed" and is left alone unless the
//! user picks a value.

use std::collections::BTreeSet;

use vf_core::browse::DocRow;
use vf_core::doctype::DocType;
use vf_core::meta::{self, MetaChange};

use crate::ui::format;
use crate::ui::widgets::{banner, muted, primary};

/// One field: `None` = the selection is mixed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fields {
    pub doc_type: Option<Option<String>>,
    pub year: Option<Option<i64>>,
    pub status: Option<String>,
    pub source: Option<Option<String>>,
    pub tags: Option<String>,
}

#[derive(Debug, Default)]
pub struct MetaForm {
    pub fields: Fields,
    initial: Fields,
    /// Selection + catalog generation the form was filled from.
    key: Option<(i64, Vec<String>)>,
    pub error: Option<String>,
}

fn common<T: Clone + PartialEq>(rows: &[&DocRow], f: impl Fn(&DocRow) -> T) -> Option<T> {
    let first = f(rows.first()?);
    rows.iter().all(|r| f(r) == first).then_some(first)
}

impl MetaForm {
    /// Re-fills from the selection when it (or the catalog) changed.
    pub fn sync(&mut self, generation: i64, rows: &[&DocRow]) {
        let key = (
            generation,
            rows.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
        );
        if self.key.as_ref() == Some(&key) {
            return;
        }
        let f = Fields {
            doc_type: common(rows, |r| r.doc_type_id.clone()),
            year: common(rows, |r| r.tax_year),
            status: common(rows, |r| r.status.clone()),
            source: common(rows, |r| r.source.clone()),
            tags: common(rows, |r| r.tags.clone()),
        };
        self.fields = f.clone();
        self.initial = f;
        self.key = Some(key);
        self.error = None;
    }

    /// Only the fields the user changed.
    pub fn change(&self) -> MetaChange {
        let (f, i) = (&self.fields, &self.initial);
        MetaChange {
            doc_type_id: (f.doc_type != i.doc_type)
                .then(|| f.doc_type.clone())
                .flatten(),
            tax_year: (f.year != i.year).then_some(f.year).flatten(),
            status: (f.status != i.status).then(|| f.status.clone()).flatten(),
            source: (f.source != i.source).then(|| f.source.clone()).flatten(),
            tags: (f.tags != i.tags)
                .then(|| f.tags.as_deref().map(meta::parse_tags))
                .flatten(),
        }
    }

    /// Draws the fields; returns the change when "Apply" is clicked.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        rows: &[&DocRow],
        types: &[DocType],
    ) -> Option<MetaChange> {
        let n = rows.len();
        let mixed = |what: &str| -> String {
            let k: BTreeSet<String> = rows.iter().map(|r| what_of(r, what)).collect();
            format!("Mixed ({})", k.len())
        };
        egui::Grid::new("meta_fields")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                let l = ui.label(muted("Doc type"));
                let text = match &self.fields.doc_type {
                    None => mixed("doc_type"),
                    Some(None) => "None".into(),
                    Some(Some(id)) => types
                        .iter()
                        .find(|t| &t.id == id)
                        .map_or("?".into(), |t| t.name.clone()),
                };
                egui::ComboBox::from_id_salt("meta_doctype")
                    .selected_text(text)
                    .width(180.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.fields.doc_type, Some(None), "None");
                        for t in types.iter().filter(|t| !t.archived) {
                            ui.selectable_value(
                                &mut self.fields.doc_type,
                                Some(Some(t.id.clone())),
                                &t.name,
                            );
                        }
                    })
                    .response
                    .labelled_by(l.id);
                ui.end_row();

                let l = ui.label(muted("Tax year"));
                let text = match self.fields.year {
                    None => mixed("year"),
                    Some(None) => "Permanent".into(),
                    Some(Some(y)) => y.to_string(),
                };
                let this_year = chrono::Datelike::year(&chrono::Local::now()) as i64;
                egui::ComboBox::from_id_salt("meta_year")
                    .selected_text(text)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.fields.year, Some(None), "Permanent");
                        for y in (this_year - 12..=this_year + 1).rev() {
                            ui.selectable_value(
                                &mut self.fields.year,
                                Some(Some(y)),
                                y.to_string(),
                            );
                        }
                    })
                    .response
                    .labelled_by(l.id);
                ui.end_row();

                let l = ui.label(muted("Status"));
                let text = self
                    .fields
                    .status
                    .as_deref()
                    .map_or_else(|| mixed("status"), |s| format::status_label(s).into());
                egui::ComboBox::from_id_salt("meta_status")
                    .selected_text(text)
                    .show_ui(ui, |ui| {
                        for s in meta::STATUSES {
                            ui.selectable_value(
                                &mut self.fields.status,
                                Some(s.to_owned()),
                                format::status_label(s),
                            );
                        }
                    })
                    .response
                    .labelled_by(l.id);
                ui.end_row();

                let l = ui.label(muted("Source"));
                let text = match &self.fields.source {
                    None => mixed("source"),
                    Some(None) => "Not set".into(),
                    Some(Some(s)) => format::source_label(s).into(),
                };
                egui::ComboBox::from_id_salt("meta_source")
                    .selected_text(text)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.fields.source, Some(None), "Not set");
                        for s in meta::SOURCES {
                            ui.selectable_value(
                                &mut self.fields.source,
                                Some(Some(s.to_owned())),
                                format::source_label(s),
                            );
                        }
                    })
                    .response
                    .labelled_by(l.id);
                ui.end_row();

                let l = ui.label(muted("Tags"));
                let mut tags = self.fields.tags.clone().unwrap_or_default();
                let hint = if self.fields.tags.is_none() {
                    mixed("tags")
                } else {
                    "comma separated".into()
                };
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut tags)
                            .hint_text(hint)
                            .desired_width(180.0),
                    )
                    .labelled_by(l.id)
                    .changed()
                {
                    self.fields.tags = Some(tags);
                }
                ui.end_row();
            });
        if let Some(e) = &self.error {
            banner(ui, e, true);
        }
        let change = self.change();
        let label = format!("Apply to {n} (one transaction)");
        let clicked = ui
            .add_enabled(!change.is_empty(), primary(&label))
            .on_disabled_hover_text("Change a field first.")
            .clicked();
        clicked.then_some(change)
    }
}

fn what_of(r: &DocRow, what: &str) -> String {
    match what {
        "doc_type" => format!("{:?}", r.doc_type_id),
        "year" => format!("{:?}", r.tax_year),
        "status" => r.status.clone(),
        "source" => format!("{:?}", r.source),
        _ => r.tags.clone(),
    }
}
