//! Admin › Retention classes (PHASES P07 step 1): the schedule, editable. Kept in
//! `_Vibe\retention.json` and the catalog; each doc type picks its class on the Doc types page.

use egui::{RichText, TextEdit};
use vf_fs::doctypes::DocType;
use vf_fs::retention::{self, Class, Edit};

use crate::ui::browse::Env;
use crate::ui::{theme, widgets};

const ANCHORS: [(&str, &str); 4] = [
    ("tax_year_end", "From the end of the tax year"),
    ("engagement_close", "From the engagement's close"),
    ("filed_at", "From the day it was filed"),
    ("never", "Kept permanently"),
];

#[derive(Default)]
pub struct RetentionPage {
    /// The class being edited (`None` with `adding`: a new one).
    editing: Option<String>,
    adding: bool,
    name: String,
    years: String,
    anchor: String,
    pub error: Option<String>,
}

impl RetentionPage {
    fn open(&mut self, c: Option<&Class>) {
        self.error = None;
        match c {
            Some(c) => {
                self.editing = Some(c.id.clone());
                self.adding = false;
                self.name = c.name.clone();
                self.years = c.years.map(|y| y.to_string()).unwrap_or_default();
                self.anchor = c.anchor.clone();
            }
            None => {
                self.editing = None;
                self.adding = true;
                self.name.clear();
                self.years = "7".into();
                self.anchor = "tax_year_end".into();
            }
        }
    }
}

/// Draws the page; returns whether the schedule changed (the caller reloads and resyncs).
pub fn show(
    ui: &mut egui::Ui,
    p: &mut RetentionPage,
    classes: &[Class],
    types: &[DocType],
    env: &mut Env,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Retention classes").strong().size(16.0));
        ui.label(widgets::caption(
            "Kept in _Vibe\\retention.json and the catalog. Doc types pick their class; a document without a doc type is never disposed.",
        ));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(widgets::primary_button("Add class")).clicked() {
                p.open(None);
            }
        });
    });
    ui.add_space(6.0);
    egui::Grid::new("retention-classes")
        .striped(true)
        .num_columns(3)
        .spacing([16.0, 6.0])
        .show(ui, |ui| {
            for h in ["Class", "Kept", "Doc types"] {
                ui.label(RichText::new(h).small().strong());
            }
            ui.end_row();
            for c in classes {
                if ui
                    .selectable_label(p.editing.as_deref() == Some(c.id.as_str()), &c.name)
                    .clicked()
                {
                    p.open(Some(c));
                }
                ui.label(c.describe());
                let n = types
                    .iter()
                    .filter(|t| t.retention.as_deref() == Some(c.id.as_str()))
                    .count();
                ui.label(n.to_string());
                ui.end_row();
            }
        });
    let unclassified: Vec<&str> = types
        .iter()
        .filter(|t| t.retention.is_none())
        .map(|t| t.name.as_str())
        .collect();
    if !unclassified.is_empty() {
        ui.label(widgets::caption(format!(
            "Doc types with no class (never disposed): {}",
            unclassified.join(", ")
        )));
    }
    if p.editing.is_none() && !p.adding {
        return false;
    }
    ui.separator();
    ui.label(
        RichText::new(if p.adding { "New class" } else { "Edit class" })
            .strong()
            .size(14.0),
    );
    egui::Grid::new("retention-form")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            let l = ui.label("Class name");
            ui.add(TextEdit::singleline(&mut p.name).desired_width(240.0))
                .labelled_by(l.id);
            ui.end_row();
            let l = ui.label("Counted");
            egui::ComboBox::from_id_salt("retention-anchor")
                .selected_text(
                    ANCHORS
                        .iter()
                        .find(|(a, _)| *a == p.anchor)
                        .map_or("", |(_, t)| t),
                )
                .show_ui(ui, |ui| {
                    for (a, t) in ANCHORS {
                        ui.selectable_value(&mut p.anchor, a.to_string(), t);
                    }
                })
                .response
                .labelled_by(l.id);
            ui.end_row();
            if p.anchor != "never" {
                let l = ui.label("Years");
                ui.add(TextEdit::singleline(&mut p.years).desired_width(60.0))
                    .labelled_by(l.id);
                ui.end_row();
            }
        });
    let years = p.years.trim().parse::<i32>().ok();
    let check = retention::check_fields(&p.name, years, &p.anchor);
    if let Err(e) = &check {
        ui.label(RichText::new(e).color(theme::ink_muted()));
    }
    if let Some(e) = &p.error {
        ui.label(RichText::new(e).color(crate::ui::theme::error()));
    }
    let mut edit = None;
    ui.horizontal(|ui| {
        let label = if p.adding { "Add" } else { "Save" };
        if ui
            .add_enabled(check.is_ok(), egui::Button::new(label))
            .clicked()
        {
            edit = Some(match &p.editing {
                Some(id) => Edit::Update(Class {
                    id: id.clone(),
                    name: p.name.clone(),
                    years,
                    anchor: p.anchor.clone(),
                }),
                None => Edit::Add {
                    name: p.name.clone(),
                    years,
                    anchor: p.anchor.clone(),
                },
            });
        }
        if let Some(id) = p.editing.clone()
            && ui.button("Remove").clicked()
        {
            edit = Some(Edit::Remove(id));
        }
        if ui.button("Close").clicked() {
            p.editing = None;
            p.adding = false;
        }
    });
    if let Some(e) = edit {
        let removing = matches!(e, Edit::Remove(_));
        let r = retention::apply(env.root, e)
            .and_then(|()| retention::sync(env.conn, env.root, env.device_id, "retention_edit"));
        match r {
            Ok(_) => {
                changed = true;
                p.error = None;
                if removing || p.adding {
                    p.editing = None;
                    p.adding = false;
                }
            }
            Err(e) => p.error = Some(e),
        }
    }
    changed
}
