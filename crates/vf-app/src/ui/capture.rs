//! Admin › Print capture & parsers: the editor for the parser profiles (owner 2026-11-03,
//! option 3). Each profile reads a printed file's name (client ID, tax year, document name);
//! its rules map the document name to a doc type, and each doc type's prints are filed in the
//! doc type's default folder or a folder chosen here. Saving writes the profile file
//! (`_Vibe\parsers\*.toml`, shared by every PC) and the Filer rules that file the prints
//! (`vf_ingest::parsers::sync_filer_rules`).

use std::path::{Path, PathBuf};

use egui::{RichText, TextEdit};
use vf_fs::doctypes::DocType;
use vf_ingest::parsers::{self, DocRule, Profile, Trial};

use crate::ui::widgets;
use crate::ui::{theme, wizard::ERROR};

#[derive(Default)]
pub struct CapturePage {
    loaded: bool,
    files: Vec<(PathBuf, Profile)>,
    errors: Vec<String>,
    pick: usize,
    /// The profile being edited (a copy of `files[pick]` until saved).
    draft: Option<Profile>,
    test: String,
    trial: Option<Trial>,
    notice: Option<String>,
    error: Option<String>,
}

impl CapturePage {
    /// Reads the profile files again (after a save, or Reload).
    pub fn reload(&mut self, root: &Path) {
        let _ = parsers::seed_if_missing(root);
        let (files, errors) = parsers::load_files(root);
        self.files = files;
        self.errors = errors;
        self.pick = self.pick.min(self.files.len().saturating_sub(1));
        self.draft = None;
        self.trial = None;
        self.loaded = true;
    }

    fn dirty(&self) -> bool {
        match (&self.draft, self.files.get(self.pick)) {
            (Some(d), Some((_, p))) => d != p,
            _ => false,
        }
    }
}

/// What is wrong with `p` before it can be saved, if anything.
fn problem(p: &Profile, types: &[DocType]) -> Option<String> {
    if p.name.trim().is_empty() {
        return Some("The profile needs a name.".into());
    }
    match regex::Regex::new(&p.pattern) {
        Err(e) => return Some(format!("File name pattern: {e}")),
        Ok(r) if !r.capture_names().flatten().any(|n| n == "id") => {
            return Some("The file name pattern needs a (?P<id>…) group for the client ID.".into());
        }
        Ok(_) => {}
    }
    for (i, d) in p.doc.iter().enumerate() {
        if d.pattern.trim().is_empty() {
            return Some(format!(
                "Rule {}: say what the document name contains.",
                i + 1
            ));
        }
        if let Err(e) = regex::Regex::new(&d.pattern) {
            return Some(format!("Rule {}: {e}", i + 1));
        }
        if !types.iter().any(|t| t.name == d.doc_type) {
            return Some(format!("Rule {}: choose a doc type.", i + 1));
        }
    }
    None
}

/// The editor. Returns whether Filer's rules changed (Intake must reload them).
pub fn show(
    ui: &mut egui::Ui,
    p: &mut CapturePage,
    root: &Path,
    types: &[DocType],
    folders: &[String],
) -> bool {
    if !p.loaded {
        p.reload(root);
    }
    let mut rules_changed = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Parser profiles").strong());
        ui.label(widgets::caption(
            r"Shared by every PC (_Vibe\parsers). Saving also updates Intake's rules.",
        ));
    });
    for e in &p.errors {
        ui.label(RichText::new(e).color(ERROR));
    }
    if p.files.is_empty() {
        ui.label("No parser profiles.");
        return false;
    }
    if p.files.len() > 1 {
        let names: Vec<String> = p.files.iter().map(|(_, f)| f.name.clone()).collect();
        let mut pick = p.pick;
        egui::ComboBox::from_label("Profile")
            .selected_text(names[pick].clone())
            .show_ui(ui, |ui| {
                for (i, n) in names.iter().enumerate() {
                    ui.selectable_value(&mut pick, i, n);
                }
            });
        if pick != p.pick && !p.dirty() {
            p.pick = pick;
            p.draft = None;
        }
    }
    let saved = p.files[p.pick].1.clone();
    ui.label(
        RichText::new(format!(
            "{} · {} rules{}",
            saved.name,
            saved.doc.len(),
            if saved.enabled { "" } else { " · off" }
        ))
        .size(15.0),
    );
    ui.add_space(4.0);
    ui.label(RichText::new("Try a file name").strong());
    ui.horizontal(|ui| {
        let l = ui.label("Printed file name");
        let r = ui
            .add(
                TextEdit::singleline(&mut p.test)
                    .hint_text("Client name_2025_1040_Engagement Letter_ID1234.pdf")
                    .desired_width(460.0),
            )
            .labelled_by(l.id);
        let go = ui.button("Try").clicked()
            || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
        if go && !p.test.trim().is_empty() {
            p.trial = Some(parsers::trial(root, p.test.trim()));
        }
    });
    ui.label(widgets::caption(
        "Uses the saved profiles and Intake's saved rules.",
    ));
    if let Some(t) = &p.trial {
        trial_view(ui, t);
    }
    ui.add_space(8.0);
    let draft = p.draft.get_or_insert_with(|| saved.clone());

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let l = ui.label("Name");
        ui.add(TextEdit::singleline(&mut draft.name).desired_width(200.0))
            .labelled_by(l.id);
        ui.checkbox(&mut draft.enabled, "On");
    });
    ui.horizontal(|ui| {
        let l = ui.label("File name pattern");
        ui.add(
            TextEdit::singleline(&mut draft.pattern)
                .font(egui::TextStyle::Monospace)
                .desired_width(f32::INFINITY),
        )
        .labelled_by(l.id);
    });
    ui.label(widgets::caption(
        "A regular expression over the printed file name. Groups: id (client ID, required), year, doc (the document name the rules below read), recipient (a K-1's partner).",
    ));

    ui.add_space(10.0);
    ui.label(RichText::new("Document name → doc type").strong());
    ui.label(widgets::caption(
        "Top to bottom; the first rule whose text is found in the document name sets the doc type. Text is a regular expression; (?i) at the start ignores case.",
    ));
    let mut action: Option<(usize, i32)> = None; // (row, -1 up / 1 down / 0 remove)
    egui::Grid::new("capture-doc-rules")
        .num_columns(5)
        .spacing([10.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            for h in ["", "Document name contains", "Doc type", "Filed in", ""] {
                ui.label(RichText::new(h).color(theme::INK_MUTED));
            }
            ui.end_row();
            let n = draft.doc.len();
            for i in 0..n {
                let num = ui.label(format!("Rule {}", i + 1));
                ui.add_sized(
                    [320.0, 22.0],
                    TextEdit::singleline(&mut draft.doc[i].pattern)
                        .font(egui::TextStyle::Monospace),
                )
                .labelled_by(num.id);
                let known = types.iter().any(|t| t.name == draft.doc[i].doc_type);
                let shown = if known {
                    draft.doc[i].doc_type.clone()
                } else if draft.doc[i].doc_type.is_empty() {
                    "Choose…".into()
                } else {
                    format!("{} (not a doc type)", draft.doc[i].doc_type)
                };
                let name = format!("Rule {} doc type", i + 1);
                egui::ComboBox::from_id_salt(("capture-doc-type", i))
                    .selected_text(shown)
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        for t in types {
                            ui.selectable_value(
                                &mut draft.doc[i].doc_type,
                                t.name.clone(),
                                &t.name,
                            );
                        }
                    })
                    .response
                    .widget_info(|| {
                        egui::WidgetInfo::labeled(egui::accesskit::Role::ComboBox, true, &name)
                    });
                let folder = parsers::folder_for(draft, &draft.doc[i].doc_type, types);
                ui.label(if folder.is_empty() {
                    widgets::caption("Intake's rules decide")
                } else {
                    RichText::new(folder).monospace()
                });
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(i > 0, egui::Button::new("↑").small())
                        .on_hover_text("Move up")
                        .clicked()
                    {
                        action = Some((i, -1));
                    }
                    if ui
                        .add_enabled(i + 1 < n, egui::Button::new("↓").small())
                        .on_hover_text("Move down")
                        .clicked()
                    {
                        action = Some((i, 1));
                    }
                    if ui
                        .small_button("Remove")
                        .on_hover_text(format!("Remove rule {}", i + 1))
                        .clicked()
                    {
                        action = Some((i, 0));
                    }
                });
                ui.end_row();
            }
        });
    match action {
        Some((i, 0)) => {
            draft.doc.remove(i);
        }
        Some((i, -1)) => draft.doc.swap(i, i - 1),
        Some((i, _)) => draft.doc.swap(i, i + 1),
        None => {}
    }
    if ui.button("Add rule").clicked() {
        draft.doc.push(DocRule {
            pattern: String::new(),
            doc_type: String::new(),
        });
    }

    // Folders: one row per doc type the rules use.
    let mut used: Vec<String> = Vec::new();
    for d in &draft.doc {
        if !d.doc_type.is_empty() && !used.contains(&d.doc_type) {
            used.push(d.doc_type.clone());
        }
    }
    ui.add_space(10.0);
    ui.label(RichText::new("Where each doc type is filed").strong());
    ui.label(widgets::caption(
        "Below the client's folder. \"Doc type's default\" follows Admin › Doc types; K-1 prints with a recipient still go through Intake's K-1 rules (copies to the partners).",
    ));
    egui::Grid::new("capture-folders")
        .num_columns(2)
        .spacing([10.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            for t in &used {
                let l = ui.label(t);
                let default = types
                    .iter()
                    .find(|x| &x.name == t)
                    .map(|x| x.default_folder.clone())
                    .unwrap_or_default();
                let default_text = if default.is_empty() {
                    "Doc type's default (none: Intake's rules decide)".to_string()
                } else {
                    format!("Doc type's default ({default})")
                };
                let current = draft.folders.get(t).cloned();
                let mut chosen = current.clone();
                egui::ComboBox::from_id_salt(("capture-folder", t.as_str()))
                    .selected_text(chosen.clone().unwrap_or_else(|| default_text.clone()))
                    .width(320.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut chosen, None, &default_text);
                        for f in folders {
                            ui.selectable_value(&mut chosen, Some(f.clone()), f);
                        }
                    })
                    .response
                    .labelled_by(l.id);
                if chosen != current {
                    match chosen {
                        Some(f) => {
                            draft.folders.insert(t.clone(), f);
                        }
                        None => {
                            draft.folders.remove(t);
                        }
                    }
                }
                ui.end_row();
            }
        });
    // Overrides for doc types no rule uses any more are dropped on save.
    let dirty = *draft != saved;
    let issue = problem(draft, types);
    ui.add_space(8.0);
    let mut save = false;
    let mut discard = false;
    ui.horizontal(|ui| {
        save = ui
            .add_enabled(dirty && issue.is_none(), widgets::primary_button("Save"))
            .clicked();
        discard = ui
            .add_enabled(dirty, egui::Button::new("Discard"))
            .clicked();
        if dirty && let Some(i) = &issue {
            ui.label(RichText::new(i).color(ERROR));
        }
    });
    let to_save = save.then(|| draft.clone());
    if discard {
        p.draft = None;
    }
    if let Some(mut d) = to_save {
        d.folders
            .retain(|t, _| d.doc.iter().any(|r| &r.doc_type == t));
        let path = p.files[p.pick].0.clone();
        match parsers::save_profile(&path, &d).and_then(|()| parsers::sync_filer_rules(root)) {
            Ok(changed) => {
                rules_changed = changed;
                p.error = None;
                p.notice = Some(if changed {
                    "Saved. Intake files these prints by the new folders.".into()
                } else {
                    "Saved.".into()
                });
                p.reload(root);
            }
            Err(e) => p.error = Some(e),
        }
    }
    widgets::message_line(ui, p.notice.as_deref());
    if let Some(e) = &p.error {
        ui.label(RichText::new(e).color(ERROR));
    }

    rules_changed
}

fn trial_view(ui: &mut egui::Ui, t: &Trial) {
    egui::Grid::new("capture-trial")
        .num_columns(2)
        .spacing([12.0, 3.0])
        .show(ui, |ui| {
            let row = |ui: &mut egui::Ui, k: &str, v: RichText| {
                ui.label(RichText::new(k).color(theme::INK_MUTED));
                ui.label(v);
                ui.end_row();
            };
            match &t.parsed {
                None => row(
                    ui,
                    "Profile",
                    RichText::new("no profile reads this name: it goes to Intake as it is")
                        .color(ERROR),
                ),
                Some(pr) => {
                    row(ui, "Profile", RichText::new(&pr.profile));
                    row(ui, "Client ID", RichText::new(&pr.client_id).monospace());
                    row(
                        ui,
                        "Tax year",
                        RichText::new(pr.year.map_or("—".into(), |y| y.to_string())),
                    );
                    row(
                        ui,
                        "Document",
                        RichText::new(pr.doc.clone().unwrap_or_default()),
                    );
                    row(
                        ui,
                        "Doc type",
                        match &pr.doc_type {
                            Some(d) => RichText::new(d),
                            None => RichText::new("no rule matched").color(ERROR),
                        },
                    );
                }
            }
            row(
                ui,
                "Reaches Intake as",
                RichText::new(&t.delivered).monospace(),
            );
            row(ui, "Intake rule", RichText::new(&t.rule));
            row(
                ui,
                "Filed as",
                match &t.destination {
                    Ok(d) => RichText::new(format!("<client folder>/{d}")).monospace(),
                    Err(e) => RichText::new(e).color(ERROR),
                },
            );
        });
}
