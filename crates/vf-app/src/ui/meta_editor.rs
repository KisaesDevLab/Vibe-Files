//! Inspector metadata fields (P01 step 5): doc type, tax year, status, source, tags for the
//! selected documents. Each field starts as "keep" (showing the value, or `Mixed (n)`); only
//! fields the user sets are applied, to every selected document, in one Transaction
//! (`vf_fs::docmeta::edit`).

use std::collections::BTreeSet;

use egui::{RichText, TextEdit};
use vf_core::browse::DocRow;
use vf_fs::docmeta::{MetaEdit, SOURCES, STATUSES, parse_tags};

use crate::ui::theme;

/// The fields being edited. `None` = keep each document's value.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MetaForm {
    /// The selection this form was opened for (reset when it changes).
    pub for_selection: BTreeSet<usize>,
    pub edit: MetaEdit,
    /// Tags as typed; applied when `tags_touched`.
    pub tags_text: String,
    pub tags_touched: bool,
}

impl MetaForm {
    pub fn for_docs(sel: &BTreeSet<usize>, docs: &[&DocRow]) -> Self {
        let tags: BTreeSet<Vec<String>> = docs.iter().map(|d| d.tags.clone()).collect();
        MetaForm {
            for_selection: sel.clone(),
            edit: MetaEdit::default(),
            tags_text: match tags.len() {
                1 => tags.into_iter().next().unwrap_or_default().join(", "),
                _ => String::new(),
            },
            tags_touched: false,
        }
    }

    /// The edit to apply, with tags parsed.
    pub fn to_edit(&self) -> MetaEdit {
        let mut e = self.edit.clone();
        if self.tags_touched {
            e.tags = Some(parse_tags(&self.tags_text));
        }
        e
    }
}

/// One value when all agree, else `Mixed (n)`.
fn summary<T: Ord + Clone>(vals: impl Iterator<Item = T>, label: impl Fn(&T) -> String) -> String {
    let set: BTreeSet<T> = vals.collect();
    match set.len() {
        0 => "—".into(),
        1 => label(set.iter().next().expect("one")),
        n => format!("Mixed ({n})"),
    }
}

fn year_label(y: Option<i32>) -> String {
    y.map(|y| y.to_string())
        .unwrap_or_else(|| "Permanent".into())
}

fn label_of(table: &[(&str, &'static str)], k: Option<&str>) -> String {
    k.and_then(|k| table.iter().find(|(kk, _)| *kk == k))
        .map(|(_, l)| (*l).to_owned())
        .unwrap_or_else(|| "—".into())
}

/// Draws the four fields + tags. `doc_types`: (id, name) in list order.
pub fn fields(
    ui: &mut egui::Ui,
    f: &mut MetaForm,
    docs: &[&DocRow],
    doc_types: &[(String, String)],
) {
    let this_year = chrono::Datelike::year(&chrono::Local::now());
    let type_name = |id: &Option<String>| {
        id.as_ref()
            .and_then(|id| doc_types.iter().find(|(i, _)| i == id))
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| "—".into())
    };
    egui::Grid::new("meta-fields")
        .num_columns(2)
        .spacing([12.0, 10.0])
        .show(ui, |ui| {
            // Doc type
            ui.vertical(|ui| {
                let l = ui.label(RichText::new("DOC TYPE").small().color(theme::INK_MUTED));
                let shown = match &f.edit.doc_type {
                    Some(t) => type_name(t),
                    None => summary(docs.iter().map(|d| d.doc_type_id.clone()), |t| type_name(t)),
                };
                egui::ComboBox::from_id_salt("meta-doc-type")
                    .selected_text(shown)
                    .width(130.0)
                    .height(320.0)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(f.edit.doc_type.is_none(), "Keep")
                            .clicked()
                        {
                            f.edit.doc_type = None;
                        }
                        if ui
                            .selectable_label(f.edit.doc_type == Some(None), "None")
                            .clicked()
                        {
                            f.edit.doc_type = Some(None);
                        }
                        for (id, name) in doc_types {
                            let on = f.edit.doc_type.as_ref() == Some(&Some(id.clone()));
                            if ui.selectable_label(on, name).clicked() {
                                f.edit.doc_type = Some(Some(id.clone()));
                            }
                        }
                    })
                    .response
                    .labelled_by(l.id);
            });
            // Tax year
            ui.vertical(|ui| {
                let l = ui.label(RichText::new("TAX YEAR").small().color(theme::INK_MUTED));
                let shown = match f.edit.tax_year {
                    Some(y) => year_label(y),
                    None => summary(docs.iter().map(|d| d.tax_year), |y| year_label(*y)),
                };
                egui::ComboBox::from_id_salt("meta-year")
                    .selected_text(shown)
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(f.edit.tax_year.is_none(), "Keep")
                            .clicked()
                        {
                            f.edit.tax_year = None;
                        }
                        if ui
                            .selectable_label(f.edit.tax_year == Some(None), "Permanent")
                            .clicked()
                        {
                            f.edit.tax_year = Some(None);
                        }
                        for y in (this_year - 15..=this_year + 1).rev() {
                            if ui
                                .selectable_label(f.edit.tax_year == Some(Some(y)), y.to_string())
                                .clicked()
                            {
                                f.edit.tax_year = Some(Some(y));
                            }
                        }
                    })
                    .response
                    .labelled_by(l.id);
            });
            ui.end_row();
            // Status
            ui.vertical(|ui| {
                let l = ui.label(RichText::new("STATUS").small().color(theme::INK_MUTED));
                let shown = match &f.edit.status {
                    Some(s) => label_of(&STATUSES, Some(s)),
                    None => summary(docs.iter().map(|d| d.status.clone()), |s| {
                        label_of(&STATUSES, Some(s))
                    }),
                };
                egui::ComboBox::from_id_salt("meta-status")
                    .selected_text(shown)
                    .width(130.0)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(f.edit.status.is_none(), "Keep")
                            .clicked()
                        {
                            f.edit.status = None;
                        }
                        for (k, lbl) in STATUSES {
                            let on = f.edit.status.as_deref() == Some(k);
                            if ui.selectable_label(on, lbl).clicked() {
                                f.edit.status = Some(k.into());
                            }
                        }
                    })
                    .response
                    .labelled_by(l.id);
            });
            // Source
            ui.vertical(|ui| {
                let l = ui.label(RichText::new("SOURCE").small().color(theme::INK_MUTED));
                let shown = match &f.edit.source {
                    Some(s) => label_of(&SOURCES, s.as_deref()),
                    None => summary(docs.iter().map(|d| d.source.clone()), |s| {
                        label_of(&SOURCES, s.as_deref())
                    }),
                };
                egui::ComboBox::from_id_salt("meta-source")
                    .selected_text(shown)
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(f.edit.source.is_none(), "Keep")
                            .clicked()
                        {
                            f.edit.source = None;
                        }
                        if ui
                            .selectable_label(f.edit.source == Some(None), "Not set")
                            .clicked()
                        {
                            f.edit.source = Some(None);
                        }
                        for (k, lbl) in SOURCES {
                            let on = f.edit.source.as_ref() == Some(&Some(k.to_string()));
                            if ui.selectable_label(on, lbl).clicked() {
                                f.edit.source = Some(Some(k.into()));
                            }
                        }
                    })
                    .response
                    .labelled_by(l.id);
            });
            ui.end_row();
        });
    ui.add_space(6.0);
    let l = ui.label(RichText::new("TAGS").small().color(theme::INK_MUTED));
    let mixed = docs.iter().map(|d| &d.tags).collect::<BTreeSet<_>>().len() > 1;
    let r = ui
        .add(
            TextEdit::singleline(&mut f.tags_text)
                .hint_text(if mixed {
                    "Mixed: typing here replaces every selected document's tags"
                } else {
                    "Comma separated"
                })
                .desired_width(f32::INFINITY),
        )
        .labelled_by(l.id);
    if r.changed() {
        f.tags_touched = true;
    }
}
