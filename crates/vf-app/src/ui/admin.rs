//! Admin tab (UI_SPEC §6). P01 step 4 builds the nav and the Doc types page; the other pages
//! arrive with their phases.

use std::collections::HashMap;

use egui::{RichText, TextEdit};
use vf_fs::doctypes::{self, DocType, Edit};

use crate::ui::browse::Env;
use crate::ui::widgets;
use crate::ui::{theme, wizard::ERROR};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    DocTypes,
    PrintCapture,
    Audit,
    Trash,
    Sweep,
    Retention,
    Disposition,
    Holds,
    Safeguards,
    Import,
}

/// UI_SPEC §6 nav, grouped. Only Doc types is built in P01.
const NAV: &[(&str, &[&str])] = &[
    (
        "FIRM",
        &[
            "Users & roles",
            "Client access (ACL)",
            "Devices",
            "Doc types",
            "Folder templates",
            "Folder import",
        ],
    ),
    ("INTAKE", &["Print capture & parsers", "Scan profiles"]),
    (
        "COMPLIANCE",
        &[
            "Audit trail",
            "Retention classes",
            "Disposition",
            "Legal holds",
            "§7216 consents",
            "Safeguards appendix",
        ],
    ),
    ("SECURITY", &["Keys & recovery", "Integrity sweep", "Trash"]),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Editing {
    Closed,
    New,
    /// The doc type id.
    Existing(String),
}

impl Editing {
    fn id(&self) -> Option<&str> {
        match self {
            Editing::Existing(id) => Some(id),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct Form {
    name: String,
    folder: String,
    visible: bool,
    /// Retention class id.
    retention: Option<String>,
}

pub struct Admin {
    pub page: Page,
    pub types: Vec<DocType>,
    /// Live documents per doc type id.
    usage: HashMap<String, i64>,
    /// Template folders below the client folder (`Income Tax/{Year}`, …) for the folder picker.
    folders: Vec<String>,
    /// Retention classes, for the doc type editor and the Retention classes page.
    pub classes: Vec<vf_fs::retention::Class>,
    /// Retention classes page (P07 step 1).
    pub retention: crate::ui::retention::RetentionPage,
    /// Disposition and Legal holds pages (P07 steps 2-4).
    pub disposition: crate::ui::disposition::DispositionPage,
    pub holds: crate::ui::disposition::HoldsPage,
    pub safeguards: crate::ui::disposition::SafeguardsPage,
    /// Folder import (P10).
    pub import: crate::ui::import_tree::ImportPage,
    /// The editor above the table.
    pub editing: Editing,
    /// Asking before removing a doc type documents use.
    confirm_remove: bool,
    form: Form,
    pub error: Option<String>,
    /// Set when the catalog changed; Browse re-reads before it next draws.
    pub changed: bool,
    /// This PC's print capture folder (the app fills it from `config.toml` each frame).
    pub capture: Option<std::path::PathBuf>,
    /// A new capture folder chosen here; the app saves it.
    pub capture_changed: Option<Option<std::path::PathBuf>>,
    /// A folder to open in Explorer (the app runs it through Browse's shell).
    pub open_request: Option<std::path::PathBuf>,
    /// The parser profile editor (Print capture & parsers).
    pub capture_page: crate::ui::capture::CapturePage,
    /// Filer's rules were rewritten (a profile or a doc type's folder changed): Intake reloads.
    pub rules_changed: bool,
    /// Audit trail page (P05 step 2).
    pub audit: crate::ui::audit::AuditPage,
    /// The app's last chain check, shown on the Audit trail page.
    pub chain: Option<crate::ui::audit::ChainCheck>,
    /// Trash page (P05 step 3).
    pub trash: crate::ui::trash::TrashPage,
}

impl Admin {
    pub fn load(env: &Env) -> Self {
        let mut a = Admin {
            page: Page::DocTypes,
            types: Vec::new(),
            usage: HashMap::new(),
            folders: Vec::new(),
            classes: Vec::new(),
            retention: Default::default(),
            disposition: Default::default(),
            holds: Default::default(),
            safeguards: Default::default(),
            import: Default::default(),
            editing: Editing::Closed,
            confirm_remove: false,
            form: Form::default(),
            error: None,
            changed: false,
            capture: None,
            capture_changed: None,
            open_request: None,
            capture_page: Default::default(),
            rules_changed: false,
            audit: Default::default(),
            chain: None,
            trash: Default::default(),
        };
        a.reload(env);
        a
    }

    pub(crate) fn reload(&mut self, env: &Env) {
        match doctypes::load(env.root) {
            Ok(t) => self.types = t.unwrap_or_default(),
            Err(e) => self.error = Some(e),
        }
        match vf_fs::retention::load(env.root) {
            Ok(c) => self.classes = c.unwrap_or_default(),
            Err(e) => self.error = Some(e),
        }
        self.usage.clear();
        if let Ok(mut st) = env.conn.prepare(
            "SELECT doc_type_id, count(*) FROM document
             WHERE deleted_at IS NULL AND doc_type_id IS NOT NULL GROUP BY doc_type_id",
        ) && let Ok(rows) = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        {
            self.usage.extend(rows.flatten());
        }
        let (tree, _, _) =
            filer_core::template::load_or_seed(&vf_fs::clients::master_dir(env.root));
        self.folders = filer_core::template::folder_templates(&tree);
        self.capture_page.reload(env.root);
    }

    /// Opens the editor on `id`, or on a new doc type.
    fn select(&mut self, id: Option<String>) {
        self.form = id
            .as_ref()
            .and_then(|id| self.types.iter().find(|t| &t.id == id))
            .map(|t| Form {
                name: t.name.clone(),
                folder: t.default_folder.clone(),
                visible: t.client_visible,
                retention: t.retention.clone(),
            })
            .unwrap_or_default();
        self.editing = match id {
            Some(id) => Editing::Existing(id),
            None => Editing::New,
        };
        self.error = None;
    }

    /// Saves one edit to `doc_types.json`, then the catalog (one Transaction).
    fn run(&mut self, env: &mut Env, edit: Edit) -> bool {
        let r = doctypes::apply(env.root, edit)
            .and_then(|()| doctypes::sync(env.conn, env.root, env.device_id, "doc_types_edit"));
        match r {
            Ok(_) => {
                self.changed = true;
                // A doc type's default folder decides where its prints are filed.
                match vf_ingest::parsers::sync_filer_rules(env.root) {
                    Ok(c) => self.rules_changed |= c,
                    Err(e) => self.error = Some(format!("Intake's rules: {e}")),
                }
                self.reload(env);
                true
            }
            Err(e) => {
                self.error = Some(e);
                false
            }
        }
    }
}

/// The app's integrity sweep (it runs on a schedule, so it lives with the open cabinet) and the
/// catalog file its thread opens.
pub struct SweepEnv<'a> {
    pub state: &'a mut crate::ui::sweep::SweepState,
    pub catalog: &'a std::path::Path,
}

pub fn show(ui: &mut egui::Ui, a: &mut Admin, env: &mut Env, sweep: SweepEnv) {
    egui::Panel::left("admin-nav")
        .exact_size(248.0)
        .resizable(false)
        .frame(egui::Frame::new().fill(theme::PANE).inner_margin(10))
        .show(ui, |ui| nav(ui, a));
    egui::CentralPanel::default().show(ui, |ui| match a.page {
        Page::DocTypes => doc_types(ui, a, env),
        Page::PrintCapture => {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| print_capture(ui, a, env));
        }
        Page::Audit => crate::ui::audit::show(ui, &mut a.audit, env, a.chain.as_ref()),
        Page::Sweep => crate::ui::sweep::show(ui, sweep.state, env, sweep.catalog),
        Page::Disposition => {
            crate::ui::disposition::show(ui, &mut a.disposition, env);
            if std::mem::take(&mut a.disposition.changed) {
                a.changed = true;
            }
            if let Some(p) = a.disposition.open_request.take() {
                a.open_request = Some(p);
            }
        }
        Page::Holds => crate::ui::disposition::holds(ui, &mut a.holds, env),
        Page::Import => {
            crate::ui::import_tree::show(ui, &mut a.import, env, sweep.catalog);
            if std::mem::take(&mut a.import.changed) {
                a.changed = true;
            }
        }
        Page::Safeguards => {
            crate::ui::disposition::safeguards(ui, &mut a.safeguards, env);
            if let Some(p) = a.safeguards.open_request.take() {
                a.open_request = Some(p);
            }
        }
        Page::Retention => {
            if crate::ui::retention::show(ui, &mut a.retention, &a.classes, &a.types, env) {
                a.changed = true;
                a.reload(env);
            }
        }
        Page::Trash => {
            crate::ui::trash::show(ui, &mut a.trash, env);
            if std::mem::take(&mut a.trash.changed) {
                a.changed = true;
            }
        }
    });
}

fn nav(ui: &mut egui::Ui, a: &mut Admin) {
    for (group, items) in NAV {
        ui.add_space(6.0);
        ui.label(RichText::new(*group).small().color(theme::INK_MUTED));
        for item in *items {
            if *item == "Doc types" {
                let label = format!("Doc types ({})", a.types.len());
                if ui
                    .selectable_label(a.page == Page::DocTypes, label)
                    .clicked()
                {
                    a.page = Page::DocTypes;
                }
            } else if *item == "Print capture & parsers" {
                if ui
                    .selectable_label(a.page == Page::PrintCapture, *item)
                    .clicked()
                {
                    a.page = Page::PrintCapture;
                }
            } else if *item == "Disposition" {
                if ui
                    .selectable_label(a.page == Page::Disposition, *item)
                    .clicked()
                {
                    a.page = Page::Disposition;
                    a.disposition.refresh();
                }
            } else if *item == "Safeguards appendix" {
                if ui
                    .selectable_label(a.page == Page::Safeguards, *item)
                    .clicked()
                {
                    a.page = Page::Safeguards;
                }
            } else if *item == "Folder import" {
                if ui.selectable_label(a.page == Page::Import, *item).clicked() {
                    a.page = Page::Import;
                }
            } else if *item == "Legal holds" {
                if ui.selectable_label(a.page == Page::Holds, *item).clicked() {
                    a.page = Page::Holds;
                }
            } else if *item == "Retention classes" {
                if ui
                    .selectable_label(a.page == Page::Retention, *item)
                    .clicked()
                {
                    a.page = Page::Retention;
                }
            } else if *item == "Integrity sweep" {
                if ui.selectable_label(a.page == Page::Sweep, *item).clicked() {
                    a.page = Page::Sweep;
                }
            } else if *item == "Trash" {
                if ui.selectable_label(a.page == Page::Trash, *item).clicked() {
                    a.page = Page::Trash;
                    a.trash.refresh();
                }
            } else if *item == "Audit trail" {
                if ui.selectable_label(a.page == Page::Audit, *item).clicked() {
                    a.page = Page::Audit;
                    a.audit.refresh();
                }
            } else {
                ui.add_enabled(false, egui::Button::selectable(false, *item))
                    .on_disabled_hover_text("Planned for version 2.");
            }
        }
    }
}

fn doc_types(ui: &mut egui::Ui, a: &mut Admin, env: &mut Env) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Doc types").strong().size(16.0));
        ui.label(widgets::caption(
            "Kept in _Vibe\\doc_types.json and the catalog; rebuild restores it.",
        ));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(widgets::primary_button("Add doc type")).clicked() {
                a.select(None);
            }
        });
    });
    ui.add_space(8.0);

    // Editor first so the table below can take the rest of the height.
    if a.editing != Editing::Closed {
        editor(ui, a, env);
        ui.add_space(8.0);
        ui.separator();
    }

    let mut pick: Option<String> = None;
    let mut mv: Option<(String, i32)> = None;
    egui::ScrollArea::vertical()
        .id_salt("doc-types")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("doc-type-grid")
                .num_columns(5)
                .striped(true)
                .spacing([16.0, 6.0])
                .show(ui, |ui| {
                    for h in [
                        "NAME",
                        "DEFAULT FOLDER",
                        "CLIENT-VISIBLE",
                        "DOCUMENTS",
                        "ORDER",
                    ] {
                        ui.label(RichText::new(h).small().color(theme::INK_MUTED));
                    }
                    ui.end_row();
                    let n = a.types.len();
                    for (i, t) in a.types.iter().enumerate() {
                        let sel = a.editing.id() == Some(t.id.as_str());
                        if ui.selectable_label(sel, &t.name).clicked() {
                            pick = Some(t.id.clone());
                        }
                        let folder = if t.default_folder.is_empty() {
                            "—"
                        } else {
                            &t.default_folder
                        };
                        ui.label(RichText::new(folder).monospace().small());
                        ui.label(if t.client_visible { "Yes" } else { "" });
                        ui.label(a.usage.get(&t.id).copied().unwrap_or(0).to_string());
                        ui.horizontal(|ui| {
                            if order_button(ui, i > 0, "Up", &format!("Move {} up", t.name)) {
                                mv = Some((t.id.clone(), -1));
                            }
                            if order_button(ui, i + 1 < n, "Down", &format!("Move {} down", t.name))
                            {
                                mv = Some((t.id.clone(), 1));
                            }
                        });
                        ui.end_row();
                    }
                });
        });
    if let Some(id) = pick {
        a.select(Some(id));
    }
    if let Some((id, by)) = mv {
        a.run(env, Edit::Move(id, by));
    }
}

/// A small button whose accessible name says which row it acts on.
fn order_button(ui: &mut egui::Ui, enabled: bool, text: &str, name: &str) -> bool {
    let r = ui.add_enabled(enabled, egui::Button::new(text).small());
    let name = name.to_owned();
    r.widget_info(|| egui::WidgetInfo::labeled(egui::accesskit::Role::Button, enabled, &name));
    r.on_hover_text(name.clone()).clicked()
}

fn editor(ui: &mut egui::Ui, a: &mut Admin, env: &mut Env) {
    ui.label(
        RichText::new(if a.editing == Editing::New {
            "New doc type"
        } else {
            "Edit doc type"
        })
        .strong()
        .size(14.0),
    );
    egui::Grid::new("doc-type-form")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            let l = ui.label("Doc type name");
            ui.add(TextEdit::singleline(&mut a.form.name).desired_width(280.0))
                .labelled_by(l.id);
            ui.end_row();
            let l = ui.label("Default folder");
            ui.horizontal(|ui| {
                ui.add(
                    TextEdit::singleline(&mut a.form.folder)
                        .hint_text("e.g. Income Tax/{Year}")
                        .desired_width(220.0)
                        .font(egui::TextStyle::Monospace),
                )
                .labelled_by(l.id);
                let pick = ui.label(RichText::new("or").color(theme::INK_MUTED));
                egui::ComboBox::from_id_salt("doc-type-folder-pick")
                    .selected_text("Pick from template")
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(false, "No default").clicked() {
                            a.form.folder.clear();
                        }
                        for f in &a.folders {
                            if ui.selectable_label(a.form.folder == *f, f).clicked() {
                                a.form.folder = f.clone();
                            }
                        }
                    })
                    .response
                    .labelled_by(pick.id)
                    .on_hover_text("Folders of the cabinet template");
            });
            ui.end_row();
            ui.label("");
            ui.checkbox(&mut a.form.visible, "Client-visible");
            ui.end_row();
            let l = ui.label("Retention");
            let shown = a
                .form
                .retention
                .as_ref()
                .and_then(|id| a.classes.iter().find(|c| &c.id == id))
                .map_or("None (never disposed)".to_string(), |c| {
                    format!("{} · {}", c.name, c.describe())
                });
            egui::ComboBox::from_id_salt("doc-type-retention")
                .selected_text(shown)
                .width(320.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut a.form.retention, None, "None (never disposed)");
                    for c in &a.classes {
                        ui.selectable_value(
                            &mut a.form.retention,
                            Some(c.id.clone()),
                            format!("{} · {}", c.name, c.describe()),
                        );
                    }
                })
                .response
                .labelled_by(l.id);
            ui.end_row();
        });
    if let Err(e) = doctypes::check_fields(&a.form.name, &a.form.folder) {
        ui.label(RichText::new(e).color(theme::INK_MUTED));
    }
    let used = a
        .editing
        .id()
        .and_then(|id| a.usage.get(id).copied())
        .unwrap_or(0);
    if let Some(e) = &a.error {
        ui.label(RichText::new(e).color(ERROR));
    }
    ui.horizontal(|ui| {
        let ok = doctypes::check_fields(&a.form.name, &a.form.folder).is_ok();
        if a.editing == Editing::New {
            if ui.add_enabled(ok, egui::Button::new("Add")).clicked() {
                let f = a.form.clone();
                if a.run(
                    env,
                    Edit::Add {
                        name: f.name,
                        default_folder: f.folder,
                        client_visible: f.visible,
                        retention: f.retention,
                    },
                ) {
                    let id = a.types.last().map(|t| t.id.clone());
                    a.select(id);
                }
            }
        } else if let Some(id) = a.editing.id().map(str::to_owned) {
            if ui
                .add_enabled(ok, egui::Button::new("Save changes"))
                .clicked()
            {
                let f = a.form.clone();
                a.run(
                    env,
                    Edit::Update(DocType {
                        id: id.clone(),
                        name: f.name,
                        default_folder: f.folder,
                        client_visible: f.visible,
                        retention: f.retention,
                    }),
                );
            }
            let remove = ui.button("Remove");
            let remove = if used > 0 {
                remove.on_hover_text(format!(
                    "{used} documents use it; they keep it, shown as deleted."
                ))
            } else {
                remove
            };
            // Used by documents: asked first (it is not on the undo stack).
            if remove.clicked() {
                if used > 0 {
                    a.confirm_remove = true;
                } else if a.run(env, Edit::Remove(id.clone())) {
                    a.editing = Editing::Closed;
                }
            }
            if a.confirm_remove && used > 0 {
                ui.label(
                    RichText::new(format!(
                        "{used} documents use this doc type. Remove it anyway? They keep it, shown as deleted; it cannot be undone from Browse."
                    ))
                    .color(ERROR),
                );
                ui.horizontal(|ui| {
                    if ui.button("Yes, remove").clicked() {
                        a.confirm_remove = false;
                        if a.run(env, Edit::Remove(id.clone())) {
                            a.editing = Editing::Closed;
                        }
                    }
                    if ui.button("Keep it").clicked() {
                        a.confirm_remove = false;
                    }
                });
            }
        }
        if ui.button("Close").clicked() {
            a.editing = Editing::Closed;
            a.error = None;
        }
    });
}

/// Admin › Print capture & parsers (P02 step 5).
fn print_capture(ui: &mut egui::Ui, a: &mut Admin, env: &mut Env) {
    ui.label(RichText::new("Print capture & parsers").strong().size(16.0));
    ui.add_space(8.0);
    ui.label(
        "Anything a PDF printer saves into the print capture folder goes to Intake as the firm's document. A file name a parser profile reads (UltraTax CS: <Client name>_<Tax year>_<Form>_<Document>[_<K-1 recipient>]_<Client ID>.pdf) is filed to that client with the profile's doc type and year, in the folder set below.",
    );
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let l = ui.label(RichText::new("Print capture folder (this PC)").strong());
        let shown = a
            .capture
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "not set".into());
        ui.label(RichText::new(shown).monospace()).labelled_by(l.id);
        if ui.button("Choose…").clicked()
            && let Some(dir) = rfd::FileDialog::new().pick_folder()
        {
            let inboxes = filer_intake::config::Settings::load_or_default(
                &vf_fs::clients::master_dir(env.root),
            )
            .0
            .inboxes;
            match crate::ui::ingest::check_staging(&dir, env.root, &inboxes) {
                Ok(()) => a.capture_changed = Some(Some(dir)),
                Err(e) => a.error = Some(e),
            }
        }
        if a.capture.is_some() && ui.button("Stop capturing").clicked() {
            a.capture_changed = Some(None);
        }
    });
    if let Some(e) = &a.error {
        ui.label(RichText::new(e).color(ERROR));
    }
    ui.add_space(8.0);
    ui.label(RichText::new("Setting up the printer").strong());
    for step in [
        "1. Make a folder for captures on this PC, outside the cabinet and outside Filer's inboxes, and choose it above.",
        "2. UltraTax CS: Setup › Office Configuration › Print Options › PDF File Options: save PDFs to that folder, with the file name in the order the profile reads: client name, tax year, form, document name, then the client ID last (e.g. Anderson Farms LLC_2025_1040_Engagement Letter_ANDE1042.pdf). Try a name below to check.",
        "3. Any other program: print to \"Microsoft Print to PDF\" (or any PDF printer) and save into that folder. Without a parser match the file still goes to Intake, marked as the firm's.",
    ] {
        ui.label(widgets::caption(step));
    }
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        if ui.button("Open parsers folder").clicked() {
            a.open_request = Some(vf_ingest::parsers::dir(env.root));
        }
        if ui.button("Reload").clicked() {
            a.capture_page.reload(env.root);
        }
    });
    let folders = a.folders.clone();
    if crate::ui::capture::show(ui, &mut a.capture_page, env.root, &a.types, &folders) {
        a.rules_changed = true;
    }
}

#[cfg(test)]
mod tests {
    use crate::ui::test_util::{click, type_into};
    use egui_kittest::kittest::Queryable;
    use egui_kittest::{Harness, HarnessBuilder};
    use vf_core::db::{Connection, Storage};

    use super::*;

    struct State {
        a: Admin,
        conn: Connection,
        root: std::path::PathBuf,
        sweep: crate::ui::sweep::SweepState,
        catalog: std::path::PathBuf,
        _tmp: tempfile::TempDir,
    }

    fn harness() -> Harness<'static, State> {
        crate::ui::test_util::pdfium_first();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("cabinet");
        std::fs::create_dir_all(&root).unwrap();
        let mut conn = vf_core::db::open(&tmp.path().join("c.db"), Storage::Local).unwrap();
        doctypes::seed_if_missing(&root).unwrap();
        vf_fs::retention::seed_if_missing(&root).unwrap();
        vf_fs::retention::sync(&mut conn, &root, "dev", "retention_seed").unwrap();
        let a = {
            let env = Env {
                conn: &mut conn,
                root: &root,
                device_id: "dev",
            };
            Admin::load(&env)
        };
        let mut h = HarnessBuilder::default()
            .with_size([1440.0, 900.0])
            .build_ui_state(
                |ui, s: &mut State| {
                    let mut env = Env {
                        conn: &mut s.conn,
                        root: &s.root,
                        device_id: "dev",
                    };
                    show(
                        ui,
                        &mut s.a,
                        &mut env,
                        SweepEnv {
                            state: &mut s.sweep,
                            catalog: &s.catalog,
                        },
                    )
                },
                State {
                    a,
                    conn,
                    root,
                    sweep: Default::default(),
                    catalog: tmp.path().join("c.db"),
                    _tmp: tmp,
                },
            );
        crate::ui::theme::install(&h.ctx);
        h.run();
        h
    }

    fn live(h: &Harness<'static, State>, name: &str) -> i64 {
        h.state()
            .conn
            .query_row(
                "SELECT count(*) FROM doc_type WHERE name = ?1 AND deleted_at IS NULL",
                [name],
                |r| r.get(0),
            )
            .unwrap()
    }

    #[test]
    fn nav_shows_the_seeded_count() {
        let h = harness();
        let label = format!("Doc types ({})", doctypes::SEED.len());
        assert!(h.query_by_label(&label).is_some());
        assert!(h.query_by_label("8879 e-file authorization").is_some());
    }

    /// P02 step 5: the Print capture page lists the seeded UltraTax CS profile.
    #[test]
    fn print_capture_page_lists_the_ultratax_profile() {
        let mut h = harness();
        click(&mut h, "Print capture & parsers");
        assert_eq!(h.state().a.page, Page::PrintCapture);
        assert!(h.query_by_label_contains("UltraTax CS ·").is_some());
        assert!(
            h.query_by_label("not set").is_some(),
            "no capture folder yet"
        );
        assert!(
            vf_ingest::parsers::dir(&h.state().root)
                .join("ultratax.toml")
                .is_file()
        );
    }

    /// Option 3 (owner 2026-11-03): the editor shows where each doc type is filed, and the
    /// test box follows a printed name through Intake's rules to its folder.
    #[test]
    fn print_capture_editor_tries_a_name() {
        let mut h = harness();
        let root = h.state().root.clone();
        let csv = root.with_file_name("clients.csv");
        std::fs::write(
            &csv,
            "ID,Name
ANDE1042,Anderson Farms LLC
",
        )
        .unwrap();
        let map = filer_core::config::ColumnMap {
            id: Some("ID".into()),
            name: Some("Name".into()),
            extras: vec![],
        };
        vf_fs::clients::import_csv(&root, &csv, &map, &Default::default()).unwrap();
        let master = vf_fs::clients::master_dir(&root);
        let mut st = filer_intake::config::Settings::load_or_default(&master).0;
        st.id_pattern = "[A-Z0-9]{4,}".into();
        st.save(&master).unwrap();
        vf_ingest::parsers::seed_if_missing(&root).unwrap();
        vf_ingest::parsers::sync_filer_rules(&root).unwrap();
        click(&mut h, "Print capture & parsers");
        // UltraTax's Options tab: the office's layout and its example.
        assert!(
            h.query_by_label_contains(
                "John Smith_2025_1040NR_Tax_Return_Pat Partner*_THOMREUT.pdf"
            )
            .is_some()
        );
        // Document Names: the preparer copy row, with its folder and filing choices.
        h.get_by_label("Document Names").click();
        // The wide grid settles its columns over a few frames.
        h.run_steps(12);
        assert!(h.query_by_label("Tax Return (preparer copy)").is_some());
        assert!(h.query_by_label("PreparerCopyTaxReturn filing").is_some());
        assert!(
            h.query_by_label("ArchiveK1Package copy to partner")
                .is_some()
        );
        type_into(
            &mut h,
            "Printed file name",
            "Anderson Farms LLC_2025_1040_Engagement Letter_ANDE1042.pdf",
        );
        click(&mut h, "Try");
        assert!(
            h.query_by_label_contains("<client folder>/Correspondence/2025/")
                .is_some()
        );
        assert!(
            h.query_by_label("Engagement letter · UltraTax CS")
                .is_some()
        );
    }

    /// P05 step 2: the Audit trail page lists the rows, filters them, asks for a verify, and
    /// its PDF export reads back.
    #[test]
    fn audit_trail_page_lists_filters_and_exports() {
        let mut h = harness();
        click(&mut h, "Audit trail");
        assert_eq!(h.state().a.page, Page::Audit);
        let n = h
            .state()
            .conn
            .query_row("SELECT count(*) FROM audit", [], |r| r.get::<_, i64>(0))
            .unwrap();
        assert!(n > 0);
        assert!(h.query_by_label(&format!("{n} rows.")).is_some());
        h.state_mut().a.audit.filter.action = Some("no-such-action".into());
        h.state_mut().a.audit.refresh();
        h.run();
        assert!(h.query_by_label("0 rows.").is_some());
        click(&mut h, "Clear");
        click(&mut h, "Verify now");
        assert!(h.state().a.audit.verify_requested);

        let s = h.state();
        let rows = vf_core::audit::query(&s.conn, &Default::default(), None).unwrap();
        let chain = crate::ui::audit::ChainCheck::run(&s.conn);
        assert!(chain.problem().is_none(), "{}", chain.summary());
        let bytes = crate::ui::audit::pdf(&rows, "all rows", Some(&chain));
        let d = lopdf::Document::load_mem(&bytes).unwrap();
        let text = d.extract_text(&[1]).unwrap();
        assert!(text.contains("Vibe Files audit trail"), "{text}");
        assert!(text.contains("intact"), "{text}");
    }

    /// P05 step 3: set the firm PIN, restore from the trash, empty it with the PIN.
    #[test]
    fn trash_page_restores_and_empties_with_the_pin() {
        let mut h = harness();
        let trash_one = |h: &mut Harness<'static, State>| {
            let s = h.state_mut();
            let rel = "A (1)/2025/w2.pdf";
            if !s.root.join(rel).exists() {
                std::fs::create_dir_all(s.root.join("A (1)/2025")).unwrap();
                std::fs::write(s.root.join(rel), b"w2").unwrap();
                s.conn
                    .execute(
                        "INSERT OR IGNORE INTO document (id, tax_year, rel_path, hash, size, filed_at, device_id, updated_at)
                         VALUES ('d1', 2025, ?1, 'h', 2, 't', 'dev', 't')",
                        [rel],
                    )
                    .unwrap();
            }
            vf_fs::trash::delete(&mut s.conn, &s.root, "dev", &["d1".into()], "deleted").unwrap();
            s.a.trash.refresh();
        };
        trash_one(&mut h);
        click(&mut h, "Trash");
        assert!(h.query_by_label("A (1)/2025/w2.pdf").is_some());
        type_into(&mut h, "New PIN", "482193");
        type_into(&mut h, "Again", "482193");
        click(&mut h, "Set PIN");
        assert!(
            vf_fs::trash::has_pin(&h.state().root),
            "{:?}",
            h.state().a.trash.error
        );

        h.get_by_label("A (1)/2025/w2.pdf").click();
        h.run();
        click(&mut h, "Restore 1");
        assert!(h.state().root.join("A (1)/2025/w2.pdf").is_file());
        assert!(h.query_by_label("The trash is empty.").is_some());

        trash_one(&mut h);
        h.run();
        type_into(&mut h, "Firm PIN", "000000");
        click(&mut h, "Empty trash");
        assert_eq!(
            h.state().a.trash.error.as_deref(),
            Some("That PIN is not right.")
        );
        type_into(&mut h, "Firm PIN", "482193");
        click(&mut h, "Empty trash");
        assert_eq!(
            h.state().a.trash.notice.as_deref(),
            Some("Emptied 1 documents.")
        );
        assert!(h.query_by_label("The trash is empty.").is_some());
        assert!(
            !vf_fs::cabinet::trash_dir(&h.state().root)
                .join("d1")
                .exists()
        );
    }

    /// P05 step 5: Sweep now finds a file edited outside Vibe Files and keeps it as a new
    /// version; an unknown file waits for a click.
    #[test]
    fn integrity_sweep_page_adopts_an_outside_edit() {
        let mut h = harness();
        let w2 = {
            let s = h.state_mut();
            let folder = s.root.join("Anderson Farms LLC (ANDE1042)");
            std::fs::create_dir_all(folder.join("Income Tax/2025")).unwrap();
            filer_intake::sentinel::write_sentinel(
                &s.root,
                &folder,
                &filer_core::clients::Client {
                    id: "ANDE1042".into(),
                    uuid: uuid::Uuid::new_v4(),
                    name: "Anderson Farms LLC".into(),
                    extra: Default::default(),
                },
                uuid::Uuid::new_v4(),
            )
            .unwrap();
            let w2 = folder.join("Income Tax/2025/w2.pdf");
            std::fs::write(&w2, b"w2").unwrap();
            let hash = filer_intake::commit::sha256_file(&w2).unwrap();
            s.conn
                .execute(
                    "INSERT INTO document (id, tax_year, rel_path, hash, size, filed_at, device_id, updated_at)
                     VALUES ('d1', 2025, 'Anderson Farms LLC (ANDE1042)/Income Tax/2025/w2.pdf', ?1, 2, 't', 'dev', 't')",
                    [hash],
                )
                .unwrap();
            std::fs::write(&w2, b"w2 signed elsewhere").unwrap();
            std::fs::write(folder.join("Income Tax/2025/notice.pdf"), b"notice").unwrap();
            w2
        };
        click(&mut h, "Integrity sweep");
        assert!(h.query_by_label("No sweep yet.").is_some());
        h.get_by_role_and_label(egui::accesskit::Role::Button, "Sweep now")
            .click();
        h.step(); // the spinner keeps repainting: step, do not run
        let start = std::time::Instant::now();
        loop {
            h.step();
            let s = h.state_mut();
            let (conn, root) = (&mut s.conn, s.root.clone());
            if s.sweep.poll(conn, &root, "dev").is_some() || s.sweep.drift.is_some() {
                break;
            }
            assert!(
                start.elapsed() < std::time::Duration::from_secs(20),
                "{:?}",
                s.sweep.error
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        h.run();
        assert!(
            h.query_by_label("Edited outside Vibe Files, kept as new versions (1)")
                .is_some()
        );
        assert!(h.query_by_label("Not in the catalog (1)").is_some());
        let s = h.state();
        let hash: String = s
            .conn
            .query_row("SELECT hash FROM document WHERE id = 'd1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(hash, filer_intake::commit::sha256_file(&w2).unwrap());
        let n: i64 = s
            .conn
            .query_row(
                "SELECT count(*) FROM audit WHERE action = 'external_modify' AND doc_id = 'd1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
        assert!(!vf_fs::sweep::due(&s.conn), "the sweep time is recorded");
    }

    /// P10: Scan proposes the folder's client, Import binds it on its own thread, Undo reverses.
    #[test]
    fn folder_import_binds_and_undoes() {
        let mut h = harness();
        {
            let s = h.state_mut();
            vf_fs::cabinet::create(
                &s.root,
                &vf_fs::cabinet::CabinetFile {
                    cabinet_id: vf_core::new_id(),
                    firm_id: uuid::Uuid::new_v4().to_string(),
                    schema_version: vf_core::db::SCHEMA_VERSION,
                    layout_version: 1,
                    created: vf_core::now_rfc3339(),
                    name: None,
                },
            )
            .unwrap();
            let csv = s.root.parent().unwrap().join("clients.csv");
            std::fs::write(
                &csv,
                "ID,Name
HALV1001,\"Halvorsen, Dana\"
",
            )
            .unwrap();
            let map = filer_core::config::ColumnMap {
                id: Some("ID".into()),
                name: Some("Name".into()),
                extras: vec![],
            };
            vf_fs::clients::import_csv(&s.root, &csv, &map, &Default::default()).unwrap();
            let d = s.root.join("Dana Halvorsen").join("TY2023");
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("W2 Acme.pdf"), b"invented").unwrap();
            vf_fs::doctypes::sync(&mut s.conn, &s.root, "dev", "seed").unwrap();
        }
        click(&mut h, "Folder import");
        click(&mut h, "Scan the cabinet");
        assert!(h.query_by_label_contains("1 fully mapped (100%)").is_some());
        h.get_by_role_and_label(egui::accesskit::Role::Button, "Import 1 folders")
            .click();
        h.step();
        let start = std::time::Instant::now();
        while h.state().a.import.last.is_none() {
            h.step();
            assert!(
                start.elapsed() < std::time::Duration::from_secs(20),
                "{:?}",
                h.state().a.import.error
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        h.run();
        assert!(
            h.query_by_label_contains("Imported: 1 folders bound")
                .is_some()
        );
        let s = h.state();
        assert!(s.root.join("Dana Halvorsen/_Vibe/client.json").is_file());
        let (t, y): (String, i64) = s
            .conn
            .query_row(
                "SELECT t.name, d.tax_year FROM document d JOIN doc_type t ON t.id = d.doc_type_id",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((t.as_str(), y), ("W-2", 2023));
        click(&mut h, "Undo the last import");
        assert!(h.query_by_label("The import was undone.").is_some());
        let s = h.state();
        assert!(!s.root.join("Dana Halvorsen/_Vibe/client.json").exists());
        assert!(s.root.join("Dana Halvorsen/TY2023/W2 Acme.pdf").is_file());
    }

    /// P07 step 1: the seeded schedule is listed and editable; a doc type picks its class.
    #[test]
    fn retention_classes_page_lists_and_edits_the_schedule() {
        let mut h = harness();
        click(&mut h, "Retention classes");
        assert!(h.query_by_label("Returns & workpapers").is_some());
        assert_eq!(
            h.query_all_by_label("7 years from the end of the tax year")
                .count(),
            2,
            "returns and notices"
        );
        click(&mut h, "Add class");
        type_into(&mut h, "Class name", "Payroll records");
        click(&mut h, "Add");
        assert!(
            h.state().a.retention.error.is_none(),
            "{:?}",
            h.state().a.retention.error
        );
        assert!(h.query_by_label("Payroll records").is_some());
        let n: i64 = h
            .state()
            .conn
            .query_row(
                "SELECT count(*) FROM retention_class WHERE deleted_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 7);
        let w2 = h.state().a.types.iter().find(|t| t.name == "W-2").unwrap();
        assert!(w2.retention.is_some(), "seeded doc types have their class");
    }

    /// P07 steps 2-4: an old W-2 is on the report; approving it puts it in the wait list;
    /// the hold page lists a hold.
    #[test]
    fn disposition_page_approves_and_holds_page_lists() {
        let mut h = harness();
        {
            let s = h.state_mut();
            s.conn
                .execute(
                    "INSERT INTO document (id, doc_type_id, tax_year, rel_path, hash, size, filed_at, device_id, updated_at)
                     VALUES ('d1', (SELECT id FROM doc_type WHERE name = 'W-2'), 2015, 'B/2015/w2.pdf', 'h', 2,
                             '2016-02-01T00:00:00Z', 'dev', 't')",
                    [],
                )
                .unwrap();
        }
        click(&mut h, "Disposition");
        h.get_by_label("B/2015/w2.pdf").click();
        h.run();
        click(&mut h, "Approve 1");
        // Asked first (QA P7): nothing approved until "Yes".
        assert!(
            vf_fs::disposition::pending(&h.state().conn)
                .unwrap()
                .is_empty()
        );
        assert!(
            h.query_by_label_contains("Approve 1 documents for destruction?")
                .is_some()
        );
        click(&mut h, "Yes, approve 1");
        let s = h.state();
        assert!(
            s.a.disposition.error.is_none(),
            "{:?}",
            s.a.disposition.error
        );
        let pending = vf_fs::disposition::pending(&s.conn).unwrap();
        assert_eq!(pending.len(), 1);
        assert!(
            h.query_by_label("Nothing has reached the end of its retention.")
                .is_some()
        );

        {
            let s = h.state_mut();
            vf_fs::holds::place(
                &mut s.conn,
                &s.root,
                "dev",
                vf_fs::holds::Target::Docs(vec!["d1".into()]),
                "subpoena",
            )
            .unwrap();
        }
        click(&mut h, "Legal holds");
        assert!(
            h.query_by_label_contains("1 documents · subpoena")
                .is_some()
        );
        click(&mut h, "Release");
        assert!(h.state().a.holds.notice.is_none(), "asked first");
        click(&mut h, "Yes, release");
        assert_eq!(
            h.state().a.holds.notice.as_deref(),
            Some("Legal hold released.")
        );
    }

    #[test]
    fn add_edit_and_remove_a_doc_type() {
        let mut h = harness();
        click(&mut h, "Add doc type");
        type_into(&mut h, "Doc type name", "Crypto statement");
        type_into(&mut h, "Default folder", "Income Tax/{Year}");
        click(&mut h, "Add");
        assert_eq!(live(&h, "Crypto statement"), 1, "{:?}", h.state().a.error);
        let file = std::fs::read_to_string(doctypes::path(&h.state().root)).unwrap();
        assert!(file.contains("Crypto statement"));

        // Selected after adding: rename it.
        type_into(&mut h, "Doc type name", " (1099-DA)");
        click(&mut h, "Save changes");
        assert_eq!(
            live(&h, "Crypto statement (1099-DA)"),
            1,
            "{:?}",
            h.state().a.error
        );

        click(&mut h, "Remove");
        assert_eq!(live(&h, "Crypto statement (1099-DA)"), 0);
        assert!(h.state().a.changed);
    }

    #[test]
    fn duplicate_name_is_refused() {
        let mut h = harness();
        click(&mut h, "Add doc type");
        type_into(&mut h, "Doc type name", "W-2");
        click(&mut h, "Add");
        let e = h.state().a.error.clone().unwrap();
        assert!(e.contains("two doc types"), "{e}");
    }

    #[test]
    fn reorder_moves_one_place() {
        let mut h = harness();
        h.get_by_label("Move W-2 down").click();
        h.run();
        assert_eq!(h.state().a.types[1].name, "W-2");
        assert_eq!(h.state().a.types[0].name, "1099-INT");
    }
}
