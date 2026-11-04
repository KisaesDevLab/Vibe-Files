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
use vf_ingest::ultratax::{self, Action, Layout, Var};

use crate::ui::widgets;
use crate::ui::{theme, wizard::error};

#[derive(Default)]
pub struct CapturePage {
    loaded: bool,
    files: Vec<(PathBuf, Profile)>,
    errors: Vec<String>,
    pick: usize,
    /// The profile being edited (a copy of `files[pick]` until saved).
    draft: Option<Profile>,
    /// The file the draft is of, and that file's profile when editing began: a reload finds
    /// the draft's profile by path, and a save over a file changed meanwhile (another PC) is
    /// refused (QA3 #1).
    draft_of: Option<(PathBuf, Profile)>,
    test: String,
    trial: Option<Trial>,
    notice: Option<String>,
    error: Option<String>,
    /// UltraTax editor: Options (false) or Document Names (true), as UltraTax's tabs.
    ut_tab: bool,
}

impl CapturePage {
    /// Reads the profile files again. Unsaved edits are kept (only Save and Discard end them;
    /// QA2 U2), still on their own file however the list changed.
    pub fn reload(&mut self, root: &Path) {
        let keep = self.dirty();
        let _ = parsers::seed_if_missing(root);
        let (files, errors) = parsers::load_files(root);
        self.files = files;
        self.errors = errors;
        let at = self
            .draft_of
            .as_ref()
            .and_then(|(path, _)| self.files.iter().position(|(p, _)| p == path));
        match (keep, at) {
            (true, Some(i)) => self.pick = i,
            (true, None) => {
                self.draft = None;
                self.draft_of = None;
                self.error = Some(
                    "The profile being edited is no longer there: its changes were dropped.".into(),
                );
            }
            (false, _) => {
                self.draft = None;
                self.draft_of = None;
            }
        }
        self.pick = self.pick.min(self.files.len().saturating_sub(1));
        self.trial = None;
        self.loaded = true;
    }

    fn dirty(&self) -> bool {
        match (&self.draft, &self.draft_of) {
            (Some(d), Some((_, base))) => d != base,
            _ => false,
        }
    }
}

/// What is wrong with `p` before it can be saved, if anything.
fn problem(p: &Profile, types: &[DocType]) -> Option<String> {
    if p.name.trim().is_empty() {
        return Some("The profile needs a name.".into());
    }
    if let Some(l) = &p.layout {
        if let Some(e) = l.problem() {
            return Some(e);
        }
        let mut seen = std::collections::HashSet::new();
        for r in &p.documents {
            let mut own = r.names();
            own.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
            for n in own {
                if !seen.insert(n.to_ascii_lowercase()) {
                    return Some(format!(
                        "{n} is the name of two documents: UltraTax prints each under its own name."
                    ));
                }
            }
        }
        for r in &p.documents {
            if let Some(e) = ultratax::rename_problem(r, p.short_names) {
                return Some(e);
            }
            if !r.doc_type.is_empty()
                && r.doc_type != ultratax::K1_BY_FORM
                && !types.iter().any(|t| t.name == r.doc_type)
            {
                return Some(format!(
                    "{}: {} is not a doc type.",
                    ultratax::label_of(&r.default_name),
                    r.doc_type
                ));
            }
        }
        return p
            .doc
            .iter()
            .enumerate()
            .find_map(|(i, d)| doc_rule_problem(i, d, types));
    }
    match regex::Regex::new(&p.pattern) {
        Err(e) => return Some(format!("File name pattern: {e}")),
        Ok(r) if !r.capture_names().flatten().any(|n| n == "id") => {
            return Some("The file name pattern needs a (?P<id>…) group for the client ID.".into());
        }
        Ok(_) => {}
    }
    p.doc
        .iter()
        .enumerate()
        .find_map(|(i, d)| doc_rule_problem(i, d, types))
}

fn doc_rule_problem(i: usize, d: &DocRule, types: &[DocType]) -> Option<String> {
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
        ui.label(RichText::new(e).color(error()));
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
        if pick != p.pick {
            if p.dirty() {
                p.notice = None;
                p.error = Some("Save or discard the changes to this profile first.".into());
            } else {
                p.pick = pick;
                p.draft = None;
                p.draft_of = None;
                p.notice = None;
                p.error = None;
            }
        }
    }
    let saved = p.files[p.pick].1.clone();
    ui.label(
        RichText::new(format!(
            "{} · {}{}",
            saved.name,
            if saved.layout.is_some() {
                format!("{} documents", saved.documents.len())
            } else {
                format!("{} rules", saved.doc.len())
            },
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
    if p.draft.is_none() {
        p.draft_of = Some((p.files[p.pick].0.clone(), saved.clone()));
    }
    let draft = p.draft.get_or_insert_with(|| saved.clone());

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let l = ui.label("Name");
        ui.add(TextEdit::singleline(&mut draft.name).desired_width(200.0))
            .labelled_by(l.id);
        ui.checkbox(&mut draft.enabled, "On");
    });
    if draft.layout.is_none() {
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
    }

    if draft.layout.is_some() {
        ultratax_editor(ui, &mut p.ut_tab, draft, types, folders);
        ui.add_space(8.0);
        egui::CollapsingHeader::new("Documents not in UltraTax's list")
            .id_salt("capture-other-docs")
            .show(ui, |ui| doc_rules(ui, draft, types, folders));
    } else {
        doc_rules(ui, draft, types, folders);
    }
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
            ui.label(RichText::new(i).color(error()));
        }
    });
    let to_save = save.then(|| draft.clone());
    if discard {
        p.draft = None;
        p.draft_of = None;
        p.reload(root);
        p.notice = None;
        p.error = None;
    }
    if let Some(mut d) = to_save {
        d.folders
            .retain(|t, _| d.doc.iter().any(|r| &r.doc_type == t));
        let (path, base) = p.draft_of.clone().expect("a draft has its file");
        // Changed on disk since editing began (another PC): not saved over (QA3 #1).
        let on_disk = parsers::load_files(root)
            .0
            .into_iter()
            .find(|(fp, _)| *fp == path)
            .map(|(_, pr)| pr);
        let changed_meanwhile = match on_disk {
            Some(now) if now != base => Err(
                "the profile was changed on another PC since you began: Discard to see it, then make your changes again".to_string(),
            ),
            None => Err("the profile file is no longer there".to_string()),
            _ => Ok(()),
        };
        // The rules it gives are checked before anything is written (QA2 U1).
        let r = changed_meanwhile
            .and_then(|()| parsers::check_profile(root, &path, &d))
            .and_then(|()| parsers::save_profile(&path, &d))
            .and_then(|()| parsers::sync_filer_rules_report(root));
        match r {
            Ok(s) => {
                rules_changed = s.changed;
                p.draft = None;
                p.draft_of = None;
                p.reload(root);
                p.error = (!s.problems.is_empty()).then(|| s.problems.join("; "));
                p.notice = Some(if s.changed {
                    "Saved. Intake files these prints by the new folders.".into()
                } else {
                    "Saved.".into()
                });
            }
            Err(e) => {
                p.notice = None;
                p.error = Some(format!("Not saved: {e}"));
            }
        }
    }
    widgets::message_line(ui, p.notice.as_deref());
    if let Some(e) = &p.error {
        ui.label(RichText::new(e).color(error()));
    }

    rules_changed
}

fn trial_view(ui: &mut egui::Ui, t: &Trial) {
    egui::Grid::new("capture-trial")
        .num_columns(2)
        .spacing([12.0, 3.0])
        .show(ui, |ui| {
            let row = |ui: &mut egui::Ui, k: &str, v: RichText| {
                ui.label(RichText::new(k).color(theme::ink_muted()));
                ui.label(v);
                ui.end_row();
            };
            match &t.parsed {
                None => row(
                    ui,
                    "Profile",
                    RichText::new("no profile reads this name: it goes to Intake as it is")
                        .color(error()),
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
                            None => RichText::new("no rule matched").color(error()),
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
                    Err(e) => RichText::new(e).color(error()),
                },
            );
        });
}

/// The `[[doc]]` rules (document name → doc type) and the folder of each doc type they use.
fn doc_rules(ui: &mut egui::Ui, draft: &mut Profile, types: &[DocType], folders: &[String]) {
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
                ui.label(RichText::new(h).color(theme::ink_muted()));
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
                    if named(
                        ui.add_enabled(i > 0, egui::Button::new("↑").small())
                            .on_hover_text("Move up"),
                        i > 0,
                        format!("Move rule {} up", i + 1),
                    )
                    .clicked()
                    {
                        action = Some((i, -1));
                    }
                    if named(
                        ui.add_enabled(i + 1 < n, egui::Button::new("↓").small())
                            .on_hover_text("Move down"),
                        i + 1 < n,
                        format!("Move rule {} down", i + 1),
                    )
                    .clicked()
                    {
                        action = Some((i, 1));
                    }
                    if named(
                        ui.small_button("Remove")
                            .on_hover_text(format!("Remove rule {}", i + 1)),
                        true,
                        format!("Remove rule {}", i + 1),
                    )
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
}

/// UltraTax's PDF File Options, as its two tabs: Options (the file name variables and their
/// positions) and Document Names (each document with its custom name, and here also its doc
/// type, filing, folder and file name).
fn ultratax_editor(
    ui: &mut egui::Ui,
    tab: &mut bool,
    draft: &mut Profile,
    types: &[DocType],
    folders: &[String],
) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.selectable_value(tab, false, "Options");
        ui.selectable_value(tab, true, "Document Names");
    });
    ui.separator();
    if *tab {
        document_names(ui, draft, types, folders);
    } else {
        options(ui, draft);
    }
}

fn options(ui: &mut egui::Ui, draft: &mut Profile) {
    let example_doc = draft
        .documents
        .iter()
        .find(|r| r.default_name == "ArchivedTaxReturn")
        .map_or("Tax_Return".to_string(), |r| r.printed_name().to_string());
    let Some(layout) = draft.layout.as_mut() else {
        return;
    };
    ui.label(widgets::caption(
        "Set these as in UltraTax: Setup › Office Configuration › Print Options › PDF File Options › Options. All variables are separated by an underscore.",
    ));
    ui.add_space(6.0);
    let n = Var::ALL.len() as u8;
    let pos_text = |p: u8| {
        if p == 0 {
            "None".to_string()
        } else {
            format!("Position {p}")
        }
    };
    egui::Grid::new("ut-variables")
        .num_columns(2)
        .spacing([24.0, 6.0])
        .show(ui, |ui| {
            ui.label(RichText::new("File Name Variables").color(theme::ink_muted()));
            ui.label(RichText::new("Variable Position").color(theme::ink_muted()));
            ui.end_row();
            for v in Var::ALL {
                let l = ui.label(v.label());
                let mut p = layout.position(v);
                egui::ComboBox::from_id_salt(("ut-pos", v.label()))
                    .selected_text(pos_text(p))
                    .width(160.0)
                    .show_ui(ui, |ui| {
                        for k in 0..=n {
                            ui.selectable_value(&mut p, k, pos_text(k));
                        }
                    })
                    .response
                    .labelled_by(l.id);
                if p != layout.position(v) {
                    layout.set_position(v, p);
                }
                ui.end_row();
            }
            let l = ui.label("K1 Recipient Name");
            egui::ComboBox::from_id_salt("ut-k1")
                .selected_text(if layout.k1_recipient_after_document {
                    "After Document Name"
                } else {
                    "None"
                })
                .width(160.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut layout.k1_recipient_after_document, false, "None");
                    ui.selectable_value(
                        &mut layout.k1_recipient_after_document,
                        true,
                        "After Document Name",
                    );
                })
                .response
                .labelled_by(l.id);
            ui.end_row();
        });
    ui.add_space(6.0);
    let mut separate = layout.postal_code_after.is_some();
    if ui
        .checkbox(
            &mut separate,
            "Print separate PDF files for each federal and state return",
        )
        .changed()
    {
        layout.postal_code_after = separate.then_some(Var::TaxYear);
    }
    if let Some(after) = layout.postal_code_after {
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            let l = ui.label("Include Postal Code");
            let mut a = after;
            egui::ComboBox::from_id_salt("ut-postal")
                .selected_text(format!("After {}", a.label()))
                .width(220.0)
                .show_ui(ui, |ui| {
                    for v in layout.order() {
                        ui.selectable_value(&mut a, v, format!("After {}", v.label()));
                    }
                })
                .response
                .labelled_by(l.id);
            layout.postal_code_after = Some(a);
        });
    }
    ui.add_space(6.0);
    match layout.problem() {
        Some(e) => {
            ui.label(RichText::new(e).color(error()));
        }
        None => {
            ui.horizontal(|ui| {
                ui.label("File Name Example");
                ui.label(RichText::new(layout.example(&example_doc)).monospace());
            });
        }
    }
    if ui.button("Restore Default").clicked() {
        *layout = Layout::default();
    }
}

fn document_names(ui: &mut egui::Ui, draft: &mut Profile, types: &[DocType], folders: &[String]) {
    ui.label(widgets::caption(
        "Custom names as set in UltraTax (PDF File Options › Document Names); prints with either the default or the custom name are read. \"Amended\" or \"Superseded\" in front of a name is read too. For each document: its doc type, whether it is filed, the folder, and the name it is filed under (blank = as printed).",
    ));
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label("File names");
        ui.radio_value(
            &mut draft.short_names,
            false,
            "As printed (Stockton Family Seed LLC_2025_1120S_Tax_Return_STOC3418.pdf)",
        );
        ui.radio_value(
            &mut draft.short_names,
            true,
            "Short (2025 1120S Tax_Return.pdf; K-1s end with the partner)",
        );
    });
    ui.label(widgets::caption("A row's own File name overrides this."));
    let short = draft.short_names;
    ui.add_space(6.0);
    let default_of = |name: &str| {
        types
            .iter()
            .find(|t| t.name == name)
            .map(|t| t.default_folder.clone())
            .unwrap_or_default()
    };
    let mut group = "";
    egui::ScrollArea::horizontal()
        .id_salt("ut-documents-scroll")
        .show(ui, |ui| {
    egui::Grid::new("ut-documents")
        .num_columns(7)
        .spacing([8.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            for h in [
                "Default Document Name",
                "Custom Name",
                "Doc type",
                "Filing",
                "Folder",
                "File name",
                "Partner copy",
            ] {
                ui.label(RichText::new(h).color(theme::ink_muted()));
            }
            ui.end_row();
            for r in draft.documents.iter_mut() {
                let g = ultratax::group_of(&r.default_name);
                if g != group {
                    group = g;
                    ui.label(RichText::new(g).strong());
                    ui.end_row();
                }
                let label = ultratax::label_of(&r.default_name).to_string();
                let l = ui.label(&label);
                ui.add_sized(
                    [160.0, 22.0],
                    TextEdit::singleline(&mut r.custom_name).hint_text(r.default_name.clone()),
                )
                .labelled_by(l.id);
                // Doc type
                let shown = if r.doc_type.is_empty() {
                    "None".to_string()
                } else {
                    r.doc_type.clone()
                };
                egui::ComboBox::from_id_salt(("ut-type", label.as_str()))
                        .truncate()
                    .selected_text(shown)
                    .width(190.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut r.doc_type, String::new(), "None");
                        ui.selectable_value(
                            &mut r.doc_type,
                            ultratax::K1_BY_FORM.to_string(),
                            ultratax::K1_BY_FORM,
                        );
                        for t in types {
                            ui.selectable_value(&mut r.doc_type, t.name.clone(), &t.name);
                        }
                    })
                    .response
                    .widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::accesskit::Role::ComboBox,
                            true,
                            format!("{label} doc type"),
                        )
                    });
                // Filing
                egui::ComboBox::from_id_salt(("ut-action", label.as_str()))
                    .selected_text(match r.action {
                        Action::File => "File",
                        Action::Skip => "Don't file",
                    })
                    .width(90.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut r.action, Action::File, "File");
                        ui.selectable_value(&mut r.action, Action::Skip, "Don't file");
                    })
                    .response
                    .widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::accesskit::Role::ComboBox,
                            true,
                            format!("{label} filing"),
                        )
                    });
                // Folder
                let default = if r.doc_type == ultratax::K1_BY_FORM {
                    default_of("K-1 (1065)")
                } else {
                    default_of(&r.doc_type)
                };
                let default_text = if default.is_empty() {
                    "Default: Income Tax/{Year}".to_string()
                } else {
                    format!("Default: {default}")
                };
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let bad = ultratax::folder_problem(&r.folder);
                    ui.add_sized(
                        [200.0, 22.0],
                        TextEdit::singleline(&mut r.folder)
                            .hint_text(default_text.clone())
                            .text_color_opt(bad.as_ref().map(|_| error())),
                    )
                    .on_hover_text(bad.unwrap_or_else(|| {
                        "Below the client folder; type a new one (e.g. Income Tax/{Year}/Government Copy) or pick one. Blank: the doc type's folder.".into()
                    }))
                    .widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::accesskit::Role::TextInput,
                            true,
                            format!("{label} folder"),
                        )
                    });
                    ui.menu_button("…", |ui| {
                        if ui.button(&default_text).clicked() {
                            r.folder.clear();
                            ui.close();
                        }
                        for f in folders {
                            if ui.button(f).clicked() {
                                r.folder = f.clone();
                                ui.close();
                            }
                        }
                    })
                    .response
                    .widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::accesskit::Role::Button,
                            true,
                            format!("{label} pick folder"),
                        )
                    });
                });
                // File name
                let bad = ultratax::rename_problem(r, short);
                // What a blank File name gives (the profile's choice).
                let hint = ultratax::Row {
                    rename: String::new(),
                    ..r.clone()
                }
                .template(short);
                {
                    let t = ui
                        .add_sized(
                            [170.0, 22.0],
                            TextEdit::singleline(&mut r.rename)
                                .hint_text(hint)
                                .text_color_opt(bad.as_ref().map(|_| error())),
                        )
                        .on_hover_text(bad.clone().unwrap_or_else(|| {
                            "Blank: the file name chosen above. Tokens: {Printed} {Document} {DocType} {Client} {ID} {Year} {Form} {Recipient} {State} {Amended}".into()
                        }));
                    t.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::accesskit::Role::TextInput,
                            true,
                            format!("{label} file name"),
                        )
                    });
                }
                // Copy to partner (K-1 rows)
                if r.default_name.contains("K1") {
                    ui.add_enabled(
                        r.action == Action::File,
                        egui::Checkbox::without_text(&mut r.copy_to_partner),
                    )
                    .on_hover_text("Also copy it to the partner's folder (a related client, or one in the client list)")
                    .widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::accesskit::Role::CheckBox,
                            true,
                            r.copy_to_partner,
                            format!("{label} copy to partner"),
                        )
                    });
                } else {
                    ui.label("");
                }
                ui.end_row();
            }
        });
        });
    ui.add_space(4.0);
    ui.label(widgets::caption(
        "File name tokens: {Printed} the name as printed · {Document} the document name · {DocType} · {Client} · {ID} · {Year} · {Form} the entity type · {Recipient} the K-1 partner · {State} · {Amended}. A name must keep {Document} or {Printed}; a K-1 that copies to the partner keeps {Recipient} (or {Printed}).",
    ));
    for r in &draft.documents {
        if let Some(e) = ultratax::rename_problem(r, draft.short_names) {
            ui.label(RichText::new(e).color(error()));
        }
    }
    if ui.button("Restore Default").clicked() {
        draft.documents = ultratax::default_rows();
    }
}

/// `r` with a screen-reader name (for buttons whose text is a symbol).
fn named(r: egui::Response, enabled: bool, name: String) -> egui::Response {
    r.widget_info(|| egui::WidgetInfo::labeled(egui::accesskit::Role::Button, enabled, &name));
    r
}
