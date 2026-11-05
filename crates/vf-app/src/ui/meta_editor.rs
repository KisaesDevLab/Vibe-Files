//! Inspector metadata fields (P01 step 5): doc type, tax year, status, source, tags for the
//! selected documents. Each field starts as "keep" (showing the value, or `Mixed (n)`); only
//! fields the user sets are applied, to every selected document, in one Transaction
//! (`vf_fs::docmeta::edit`).

use std::collections::BTreeSet;

use egui::TextEdit;
use vf_core::browse::DocRow;
use vf_fs::docmeta::{MetaEdit, SOURCES, STATUSES, parse_tags};

use crate::ui::widgets::{self, label_of, year_label};

/// The fields being edited. `None` = keep each document's value.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MetaForm {
    /// The selection this form was opened for (reset when it changes).
    pub for_selection: BTreeSet<usize>,
    pub edit: MetaEdit,
    /// Tags as typed; applied when `tags_touched`.
    pub tags_text: String,
    pub tags_touched: bool,
    /// What each field shows while kept (the value, or `Mixed (n)`), worked out once per
    /// selection: doc type, tax year, status, source.
    shown: [String; 4],
    tags_mixed: bool,
}

/// One value when all agree, else `Mixed (n)`.
fn summary<T: Ord>(vals: impl Iterator<Item = T>, label: impl Fn(&T) -> String) -> String {
    let set: BTreeSet<T> = vals.collect();
    match set.len() {
        0 => "—".into(),
        1 => label(set.iter().next().expect("one")),
        n => format!("Mixed ({n})"),
    }
}

impl MetaForm {
    pub fn for_docs(
        sel: &BTreeSet<usize>,
        docs: &[&DocRow],
        doc_types: &[(String, String)],
    ) -> Self {
        let tags: BTreeSet<&Vec<String>> = docs.iter().map(|d| &d.tags).collect();
        MetaForm {
            for_selection: sel.clone(),
            edit: MetaEdit::default(),
            tags_text: match tags.len() {
                1 => tags
                    .into_iter()
                    .next()
                    .map(|t| t.join(", "))
                    .unwrap_or_default(),
                _ => String::new(),
            },
            tags_touched: false,
            shown: [
                summary(docs.iter().map(|d| d.doc_type_id.as_deref()), |t| {
                    type_name(doc_types, *t)
                }),
                summary(docs.iter().map(|d| d.tax_year), |y| year_label(*y)),
                summary(docs.iter().map(|d| d.status.as_str()), |s| {
                    label_of(&STATUSES, Some(s), "—").to_owned()
                }),
                summary(docs.iter().map(|d| d.source.as_deref()), |s| {
                    label_of(&SOURCES, *s, "—").to_owned()
                }),
            ],
            tags_mixed: docs.iter().map(|d| &d.tags).collect::<BTreeSet<_>>().len() > 1,
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

fn type_name(doc_types: &[(String, String)], id: Option<&str>) -> String {
    id.and_then(|id| doc_types.iter().find(|(i, _)| i == id))
        .map(|(_, n)| n.clone())
        .unwrap_or_else(|| "—".into())
}

/// A captioned dropdown for one field: "Keep" (shows `kept`, the current value or `Mixed (n)`),
/// then `choices`. `slot` is the field of the edit (`None` = keep).
fn keep_combo<T: PartialEq + Clone>(
    ui: &mut egui::Ui,
    caption: &str,
    width: f32,
    kept: &str,
    slot: &mut Option<T>,
    choices: &[(T, String)],
) {
    ui.vertical(|ui| {
        let l = ui.label(widgets::caption(caption));
        let shown = match slot {
            Some(v) => choices
                .iter()
                .find(|(c, _)| c == v)
                .map(|(_, l)| l.clone())
                .unwrap_or_default(),
            None => kept.to_owned(),
        };
        let current = slot
            .as_ref()
            .and_then(|v| choices.iter().position(|(c, _)| c == v));
        let (r, pick) = widgets::search_combo(
            ui,
            caption,
            width,
            shown,
            || choices.iter().map(|(_, l)| l.clone()).collect(),
            current,
            Some("Keep"),
        );
        r.labelled_by(l.id);
        if let Some(k) = pick {
            *slot = k.map(|k| choices[k].0.clone());
        }
    });
}

/// Draws the four fields + tags. `doc_types`: (id, name) in list order.
pub fn fields(ui: &mut egui::Ui, f: &mut MetaForm, doc_types: &[(String, String)]) {
    let this_year = chrono::Datelike::year(&chrono::Local::now());
    let types: Vec<(Option<String>, String)> = std::iter::once((None, "None".to_owned()))
        .chain(
            doc_types
                .iter()
                .map(|(id, n)| (Some(id.clone()), n.clone())),
        )
        .collect();
    let years: Vec<(Option<i32>, String)> = std::iter::once((None, "Permanent".to_owned()))
        .chain(
            (this_year - 15..=this_year + 1)
                .rev()
                .map(|y| (Some(y), y.to_string())),
        )
        .collect();
    let statuses: Vec<(String, String)> = STATUSES
        .iter()
        .map(|(k, l)| (k.to_string(), l.to_string()))
        .collect();
    let sources: Vec<(Option<String>, String)> = std::iter::once((None, "Not set".to_owned()))
        .chain(
            SOURCES
                .iter()
                .map(|(k, l)| (Some(k.to_string()), l.to_string())),
        )
        .collect();
    egui::Grid::new("meta-fields")
        .num_columns(2)
        .spacing([12.0, 10.0])
        .show(ui, |ui| {
            keep_combo(
                ui,
                "DOC TYPE",
                130.0,
                &f.shown[0],
                &mut f.edit.doc_type,
                &types,
            );
            keep_combo(
                ui,
                "TAX YEAR",
                110.0,
                &f.shown[1],
                &mut f.edit.tax_year,
                &years,
            );
            ui.end_row();
            keep_combo(
                ui,
                "STATUS",
                130.0,
                &f.shown[2],
                &mut f.edit.status,
                &statuses,
            );
            keep_combo(
                ui,
                "SOURCE",
                110.0,
                &f.shown[3],
                &mut f.edit.source,
                &sources,
            );
            ui.end_row();
        });
    ui.add_space(6.0);
    let l = ui.label(widgets::caption("TAGS"));
    let r = ui
        .add(
            TextEdit::singleline(&mut f.tags_text)
                .hint_text(if f.tags_mixed {
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
