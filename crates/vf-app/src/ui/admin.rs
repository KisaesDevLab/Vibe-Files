//! Admin tab (UI_SPEC §6). P01 step 4 builds the nav and the Doc types page; the other pages
//! arrive with their phases.

use std::collections::HashMap;

use egui::{RichText, TextEdit};
use vf_fs::doctypes::{self, DocType, Edit};

use crate::ui::browse::Env;
use crate::ui::theme;
use crate::ui::widgets;

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
    /// Admin › Merge cabinets (owner 2026-11-05).
    Merge,
    /// Admin › Admin password (PA-24).
    Password,
    /// Admin › Devices (PA-24).
    Devices,
    /// Admin › Folder templates (PA-24).
    Templates,
    /// Admin › Portal (PA-37).
    Portal,
    /// Admin › Portal requests (PA-37).
    PortalRequests,
}

/// UI_SPEC §6 nav, grouped: the pages that are built. Version 2 pages (Users & roles, Client
/// access, Scan profiles, §7216 consents, Keys & recovery) are not listed until they exist
/// (owner 2026-11-04: no greyed-out items).
const NAV: &[(&str, &[&str])] = &[
    (
        "FIRM",
        &[
            "Devices",
            "Doc types",
            "Folder templates",
            "Folder import",
            "Merge cabinets",
        ],
    ),
    ("INTAKE", &["Print capture & parsers"]),
    ("PORTAL", &["Portal", "Portal requests"]),
    (
        "COMPLIANCE",
        &[
            "Audit trail",
            "Retention classes",
            "Disposition",
            "Legal holds",
            "Safeguards appendix",
        ],
    ),
    ("SECURITY", &["Admin password", "Integrity sweep", "Trash"]),
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
    /// Picked first on new cover sheets.
    cover_default: bool,
    /// Archived: left out of pick lists.
    archived: bool,
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
    pub merge: crate::ui::merge_page::MergePage,
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
    /// Admin password page (PA-24).
    pub password: crate::ui::admin_password::PasswordPage,
    /// "Lock Admin" pressed (the app ends the unlocked session).
    pub lock_requested: bool,
    pub devices: crate::ui::devices::DevicesPage,
    pub templates: crate::ui::templates::TemplatesPage,
    /// Admin › Portal and Portal requests (PA-37).
    pub portal: crate::ui::portal::AdminPortal,
    /// Doc types: archived ones listed too.
    pub show_archived: bool,
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
            merge: Default::default(),
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
            password: Default::default(),
            lock_requested: false,
            devices: Default::default(),
            templates: Default::default(),
            portal: Default::default(),
            show_archived: false,
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
                cover_default: t.cover_default,
                archived: t.archived,
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
        // Print capture profiles name doc types: a rename follows, a removal waits (QA2 U3).
        // Compared as saved (trimmed): a stray space is not a rename (QA3 #2).
        let renamed = match &edit {
            Edit::Update(t) => self
                .types
                .iter()
                .find(|x| x.id == t.id && x.name.trim() != t.name.trim())
                .map(|old| (old.name.clone(), t.name.trim().to_string())),
            _ => None,
        };
        if let Some((old, _)) = &renamed {
            // The profiles must all read before anything changes, so a rename never stops
            // half way (QA3 #7); the K-1 doc types behind "K-1 (by entity type)" are named in the
            // program, not in the profile, and cannot follow a rename (QA3 #8).
            let (_, errs) = vf_ingest::parsers::load_files(env.root);
            if !errs.is_empty() {
                self.error = Some(format!(
                    "Not renamed: a print capture profile cannot be read ({}).",
                    errs.join("; ")
                ));
                return false;
            }
            if vf_ingest::parsers::fixed_name_in_use(env.root, old) {
                self.error = Some(format!(
                    "{old} is used by print capture's \"K-1 (by entity type)\" rows under this name: it cannot be renamed."
                ));
                return false;
            }
        }
        if let Edit::Remove(id) = &edit
            && let Some(t) = self.types.iter().find(|x| &x.id == id)
        {
            let users = vf_ingest::parsers::profiles_using(env.root, &t.name);
            if !users.is_empty() {
                self.error = Some(format!(
                    "{} is used by print capture ({}): change those rows in Admin › Print capture & parsers first.",
                    t.name,
                    users.join(", ")
                ));
                return false;
            }
        }
        let r = doctypes::apply(env.root, edit)
            .and_then(|()| doctypes::sync(env.conn, env.root, env.device_id, "doc_types_edit"));
        match r {
            Ok(_) => {
                self.changed = true;
                let mut msgs: Vec<String> = Vec::new();
                if let Some((old, new)) = renamed
                    && let Err(e) = vf_ingest::parsers::rename_doc_type(env.root, &old, &new)
                {
                    msgs.push(format!("Print capture still names {old}: {e}"));
                }
                // A doc type's default folder decides where its prints are filed.
                match vf_ingest::parsers::sync_filer_rules_report(env.root) {
                    Ok(s) => {
                        self.rules_changed |= s.changed;
                        if !s.problems.is_empty() {
                            msgs.push(format!("Print capture: {}", s.problems.join("; ")));
                        }
                    }
                    Err(e) => msgs.push(format!("Print capture: {e}")),
                }
                self.reload(env);
                // Every message kept, after the reload (QA3 #7).
                if !msgs.is_empty() {
                    self.error = Some(msgs.join(" "));
                }
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
    let has_password = vf_fs::trash::has_pin(env.root);
    egui::Panel::left("admin-nav")
        .exact_size(248.0)
        .resizable(false)
        .frame(egui::Frame::new().fill(theme::pane()).inner_margin(10))
        .show(ui, |ui| nav(ui, a, has_password));
    egui::CentralPanel::default().show(ui, |ui| match a.page {
        Page::DocTypes => doc_types(ui, a, env),
        Page::PrintCapture => {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| print_capture(ui, a, env));
        }
        Page::Audit => {
            crate::ui::audit::show(ui, &mut a.audit, env, sweep.catalog, a.chain.as_ref())
        }
        Page::Sweep => crate::ui::sweep::show(ui, sweep.state, env, sweep.catalog),
        Page::Disposition => {
            crate::ui::disposition::show(ui, &mut a.disposition, env, sweep.catalog);
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
        Page::Merge => {
            crate::ui::merge_page::show(ui, &mut a.merge, env, sweep.catalog);
            if std::mem::take(&mut a.merge.changed) {
                a.changed = true;
            }
        }
        Page::Safeguards => {
            crate::ui::disposition::safeguards(ui, &mut a.safeguards, env, sweep.catalog);
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
        Page::Password => crate::ui::admin_password::show(ui, &mut a.password, env),
        Page::Portal => {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    crate::ui::portal::admin_settings(ui, &mut a.portal, env)
                });
        }
        Page::PortalRequests => {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    crate::ui::portal::admin_requests(ui, &mut a.portal, env)
                });
        }
        Page::Devices => crate::ui::devices::show(ui, &mut a.devices, env),
        Page::Templates => {
            crate::ui::templates::show(ui, &mut a.templates, env);
            if std::mem::take(&mut a.templates.changed) {
                // Doc types offer the template's folders; Intake files with the tree.
                a.reload(env);
                a.rules_changed = true;
            }
        }
        Page::Trash => {
            crate::ui::trash::show(ui, &mut a.trash, env, Some(sweep.catalog));
            if std::mem::take(&mut a.trash.changed) {
                a.changed = true;
            }
        }
    });
}

fn nav(ui: &mut egui::Ui, a: &mut Admin, has_password: bool) {
    if has_password {
        if ui
            .button("Lock Admin")
            .on_hover_text("Ask for the admin password again on this PC.")
            .clicked()
        {
            a.lock_requested = true;
        }
    } else if ui
        .add(egui::Button::new(
            RichText::new("No admin password: set one").color(theme::warn()),
        ))
        .on_hover_text("Anyone can change Admin and Sync settings until one is set.")
        .clicked()
    {
        a.page = Page::Password;
    }
    for (group, items) in NAV {
        ui.add_space(6.0);
        ui.label(RichText::new(*group).small().color(theme::ink_muted()));
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
            } else if *item == "Portal" {
                if ui.selectable_label(a.page == Page::Portal, *item).clicked() {
                    a.page = Page::Portal;
                }
            } else if *item == "Portal requests" {
                if ui
                    .selectable_label(a.page == Page::PortalRequests, *item)
                    .clicked()
                {
                    a.page = Page::PortalRequests;
                    a.portal.refresh_requests();
                }
            } else if *item == "Folder import" {
                if ui.selectable_label(a.page == Page::Import, *item).clicked() {
                    a.page = Page::Import;
                }
            } else if *item == "Merge cabinets" {
                if ui.selectable_label(a.page == Page::Merge, *item).clicked() {
                    a.page = Page::Merge;
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
            } else if *item == "Folder templates" {
                if ui
                    .selectable_label(a.page == Page::Templates, *item)
                    .clicked()
                {
                    a.page = Page::Templates;
                }
            } else if *item == "Devices" {
                if ui
                    .selectable_label(a.page == Page::Devices, *item)
                    .clicked()
                {
                    a.page = Page::Devices;
                }
            } else if *item == "Admin password" {
                if ui
                    .selectable_label(a.page == Page::Password, *item)
                    .clicked()
                {
                    a.page = Page::Password;
                }
            } else if *item == "Trash" {
                if ui.selectable_label(a.page == Page::Trash, *item).clicked() {
                    a.page = Page::Trash;
                    a.trash.refresh();
                }
            } else if *item == "Audit trail"
                && ui.selectable_label(a.page == Page::Audit, *item).clicked()
            {
                a.page = Page::Audit;
                a.audit.refresh();
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
            let archived = a.types.iter().filter(|t| t.archived).count();
            if archived > 0 {
                ui.checkbox(&mut a.show_archived, format!("Show archived ({archived})"));
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
                        ui.label(RichText::new(h).small().color(theme::ink_muted()));
                    }
                    ui.end_row();
                    let n = a.types.len();
                    for (i, t) in a.types.iter().enumerate() {
                        if t.archived && !a.show_archived {
                            continue;
                        }
                        let sel = a.editing.id() == Some(t.id.as_str());
                        let name = if t.archived {
                            RichText::new(format!("{} (archived)", t.name))
                                .color(theme::ink_muted())
                        } else {
                            RichText::new(&t.name)
                        };
                        if ui.selectable_label(sel, name).clicked() {
                            pick = Some(t.id.clone());
                        }
                        let folder = if t.default_folder.is_empty() {
                            "—"
                        } else {
                            &t.default_folder
                        };
                        ui.label(RichText::new(folder).monospace().small());
                        ui.label(match (t.client_visible, t.cover_default) {
                            (true, true) => RichText::new("Yes · cover default"),
                            (true, false) => RichText::new("Yes"),
                            (false, true) => RichText::new("No · cover default"),
                            // Said, not left blank (UI review 2026-11-11).
                            (false, false) => RichText::new("No").color(theme::ink_muted()),
                        });
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
    let r = ui.add_enabled(
        enabled,
        egui::Button::new(text)
            .small()
            .min_size(crate::ui::widgets::MIN_TARGET),
    );
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
                let pick = ui.label(RichText::new("or").color(theme::ink_muted()));
                let current = a.folders.iter().position(|f| *f == a.form.folder);
                let folders = &a.folders;
                let (r, chosen) = crate::ui::widgets::search_combo(
                    ui,
                    "doc-type-folder-pick",
                    160.0,
                    "Pick from template",
                    || folders.clone(),
                    current,
                    Some("No default"),
                );
                r.labelled_by(pick.id)
                    .on_hover_text("Folders of the cabinet template");
                match chosen {
                    Some(Some(k)) => a.form.folder = a.folders[k].clone(),
                    Some(None) => a.form.folder.clear(),
                    None => {}
                }
            });
            ui.end_row();
            ui.label("");
            ui.checkbox(&mut a.form.visible, "Client-visible");
            ui.end_row();
            ui.label("");
            ui.add_enabled(
                !a.form.archived,
                egui::Checkbox::new(&mut a.form.cover_default, "Default on cover sheets"),
            )
            .on_hover_text(
                "Print cover sheet starts with this doc type. Only one doc type can be the default.",
            );
            ui.end_row();
            ui.label("");
            if ui
                .checkbox(&mut a.form.archived, "Archived (hidden from pick lists)")
                .on_hover_text(
                    "Not offered when choosing a doc type. Documents that have it keep it and its retention still applies. Untick to bring it back.",
                )
                .changed()
                && a.form.archived
            {
                a.form.cover_default = false;
            }
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
        ui.label(RichText::new(e).color(theme::ink_muted()));
    }
    let used = a
        .editing
        .id()
        .and_then(|id| a.usage.get(id).copied())
        .unwrap_or(0);
    if let Some(e) = &a.error {
        ui.label(RichText::new(e).color(crate::ui::theme::error()));
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
                    let added = a.types.last().cloned();
                    if f.cover_default
                        && let Some(t) = &added
                    {
                        a.run(
                            env,
                            Edit::Update(DocType {
                                cover_default: true,
                                ..t.clone()
                            }),
                        );
                    }
                    a.select(added.map(|t| t.id));
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
                        cover_default: f.cover_default,
                        archived: f.archived,
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
                    .color(crate::ui::theme::error()),
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
        ui.label(RichText::new(e).color(crate::ui::theme::error()));
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
            partner: None,
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

    /// PA-37: Admin › Portal checks the key, saves for every PC, adds the Portal inbox, audits
    /// without the key, and stops only when confirmed.
    #[test]
    fn portal_page_saves_for_every_pc_and_adds_the_portal_inbox() {
        let mut h = harness();
        let root = h.state().root.clone();
        vf_fs::cabinet::create(
            &root,
            &vf_fs::cabinet::CabinetFile {
                cabinet_id: "cab-portal".into(),
                firm_id: "firm".into(),
                schema_version: 1,
                layout_version: 1,
                created: "2026-11-09T00:00:00Z".into(),
                name: None,
            },
        )
        .unwrap();
        click(&mut h, "Portal");
        assert_eq!(h.state().a.page, Page::Portal);
        // A wrong key is refused, nothing saved.
        h.state_mut()
            .a
            .portal
            .fill("files.example.test", "not-a-key", "5");
        click(&mut h, "Save");
        assert!(h.state().a.portal.error().unwrap().contains("vpk_"));
        assert!(vf_portal::settings::load(&root, "cab-portal").is_none());
        // A right one is saved in the cabinet, the Portal inbox joins Intake's inboxes, and the
        // change is in the audit trail (without the key).
        let key = "vpk_abcdefghijklmnopqrstuvwxyz0123456789";
        h.state_mut().a.portal.fill("files.example.test", key, "10");
        click(&mut h, "Save");
        let s = vf_portal::settings::load(&root, "cab-portal").unwrap();
        assert_eq!(
            (s.url.as_str(), s.key.as_str(), s.fetch_minutes),
            ("https://files.example.test", key, 10)
        );
        assert!(h.state().a.portal.changed);
        let (fs, _) =
            filer_intake::config::Settings::load_or_default(&vf_fs::clients::master_dir(&root));
        assert!(fs.inboxes.contains(&root.join("_Portal inbox")));
        let detail: String = h
            .state()
            .conn
            .query_row(
                "SELECT detail FROM audit WHERE action = 'portal_settings' ORDER BY seq DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            detail.contains("files.example.test") && !detail.contains("vpk_"),
            "{detail}"
        );
        // Stopping removes it for every PC.
        click(&mut h, "Stop using the portal");
        // It asks first: nothing stopped yet.
        assert!(vf_portal::settings::load(&root, "cab-portal").is_some());
        click(&mut h, "Stop");
        assert!(vf_portal::settings::load(&root, "cab-portal").is_none());
    }

    /// PA-43: Admin › Portal lists what was fetched and not filed; one missing from the inbox can
    /// be fetched again; Dismiss asks first.
    #[test]
    fn portal_page_lists_uploads_not_filed() {
        let mut h = harness();
        let root = h.state().root.clone();
        vf_fs::cabinet::create(
            &root,
            &vf_fs::cabinet::CabinetFile {
                cabinet_id: "cab-portal".into(),
                firm_id: "firm".into(),
                schema_version: 1,
                layout_version: 1,
                created: "2026-11-09T00:00:00Z".into(),
                name: None,
            },
        )
        .unwrap();
        click(&mut h, "Portal");
        let key = "vpk_abcdefghijklmnopqrstuvwxyz0123456789";
        h.state_mut().a.portal.fill("files.example.test", key, "0");
        click(&mut h, "Save");
        let at = |w: crate::portal::Where, id: &str| {
            crate::portal::Pending {
            upload: serde_json::from_value(serde_json::json!({
                "uploadId": id, "clientId": "c", "clientCode": "1001",
                "clientName": "Smith, John", "name": format!("{id}.pdf"), "originalName": null,
                "size": 1, "sha256": "x", "note": "", "batchId": null,
                "uploadedAt": "2026-11-09T10:00:00.000Z", "fetchedAt": "2026-11-09T10:05:00.000Z",
                "fetchedBy": "PC1", "from": {"identityId": "i", "name": "M", "email": null, "phone": null}
            }))
            .unwrap(),
            at: w,
            saved_as: None,
        }
        };
        h.state_mut()
            .a
            .portal
            .show_filing(crate::portal::FiledReport {
                pending: vec![
                    at(crate::portal::Where::InInbox, "waiting"),
                    at(crate::portal::Where::Missing, "lost"),
                ],
                keep_days: 30,
                cap_days: 180,
                ..Default::default()
            });
        h.run_steps(3);
        assert!(h.query_by_label("Fetched, not filed yet").is_some());
        assert!(h.query_by_label("lost.pdf").is_some());
        assert!(
            h.query_by_label("not in the Portal inbox, and Intake has not filed it")
                .is_some()
        );
        // Only the missing one can be fetched again.
        assert_eq!(h.query_all_by_label("Fetch again").count(), 1);
        assert_eq!(h.query_all_by_label("Dismiss…").count(), 2);
        h.query_all_by_label("Dismiss…").next().unwrap().click();
        h.run_steps(3);
        assert!(
            h.query_by_label("Dismiss it? The portal deletes its copy 30 days later.")
                .is_some()
        );
        click(&mut h, "Keep it");
        assert!(h.query_by_label("Keep it").is_none());
        assert_eq!(h.query_all_by_label("Dismiss…").count(), 2);
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

    /// Doc types can be archived: hidden from the list until "Show archived", and from the
    /// catalog's pick lists (owner 2026-11-04).
    #[test]
    fn doc_type_archives_and_hides() {
        let mut h = harness();
        h.get_by_label("8879 e-file authorization").click();
        h.run();
        h.get_by_label("Archived (hidden from pick lists)").click();
        h.run();
        click(&mut h, "Save changes");
        let t = h
            .state()
            .a
            .types
            .iter()
            .find(|t| t.name == "8879 e-file authorization")
            .unwrap()
            .clone();
        assert!(t.archived, "{:?}", h.state().a.error);
        h.state_mut().a.editing = Editing::Closed;
        h.run();
        assert!(
            h.query_by_label("8879 e-file authorization (archived)")
                .is_none()
        );
        h.get_by_label("Show archived (1)").click();
        h.run();
        assert!(
            h.query_by_label("8879 e-file authorization (archived)")
                .is_some()
        );
        let snap = vf_core::browse::load(&h.state().conn).unwrap();
        assert!(
            !snap
                .pickable_doc_types(None)
                .iter()
                .any(|(id, _)| *id == t.id)
        );
    }

    /// PA-24: Folder templates edits Filer's template.json; a new client then gets the folder.
    #[test]
    fn folder_templates_edit_reaches_new_clients() {
        let mut h = harness();
        click(&mut h, "Folder templates");
        h.get_by_label("{Client} ({ID})").click();
        h.run();
        click(&mut h, "Add sub-folder");
        h.run();
        h.state_mut().a.templates.name = "Engagements".into();
        h.run();
        click(&mut h, "Rename");
        click(&mut h, "Save template");
        let root = h.state().root.clone();
        let (tree, _, _) = filer_core::template::load_or_seed(&vf_fs::clients::master_dir(&root));
        assert!(
            tree.children.iter().any(|c| c.name == "Engagements"),
            "{:?}",
            h.state().a.templates.error
        );
        // New clients are made from this list (vf_fs::newclient reads the same file).
        assert!(
            filer_core::template::folder_templates(&tree)
                .iter()
                .any(|f| f == "Engagements")
        );
    }

    /// PA-24: Devices lists the workstations; this PC is marked and cannot be removed; another
    /// can, and is audited.
    #[test]
    fn devices_page_lists_and_removes() {
        let mut h = harness();
        {
            let s = h.state_mut();
            vf_fs::safeguards::register_device(&s.root, "dev", false).unwrap();
            vf_fs::safeguards::register_device(&s.root, "old-pc", false).unwrap();
        }
        click(&mut h, "Devices");
        let both = vf_fs::safeguards::devices(&h.state().root).len();
        assert_eq!(both, 2);
        assert!(h.query_all_by_label("Remove").count() == 1, "not this PC");
        click(&mut h, "Remove");
        assert_eq!(vf_fs::safeguards::devices(&h.state().root).len(), 1);
        let n: i64 = h
            .state()
            .conn
            .query_row(
                "SELECT count(*) FROM audit WHERE action = 'device_removed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    /// P05 step 3: set the admin password, restore from the trash, empty it with the password.
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
        // The admin password (PA-24; it was the firm PIN) is set on its own page.
        click(&mut h, "Admin password");
        type_into(&mut h, "New password", "482193");
        type_into(&mut h, "New password again", "482193");
        click(&mut h, "Set admin password");
        click(&mut h, "Trash");
        assert!(
            vf_fs::trash::has_pin(&h.state().root),
            "{:?}",
            h.state().a.password.error
        );

        h.get_by_label("A (1)/2025/w2.pdf").click();
        h.run();
        click(&mut h, "Restore 1");
        assert!(h.state().root.join("A (1)/2025/w2.pdf").is_file());
        assert!(h.query_by_label("The trash is empty.").is_some());

        trash_one(&mut h);
        h.run();
        type_into(&mut h, "Password", "000000");
        click(&mut h, "Empty trash");
        assert_eq!(
            h.state().a.trash.error.as_deref(),
            Some("That admin password is not right.")
        );
        type_into(&mut h, "Password", "482193");
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

        // Adopt runs as a job (owner 2026-11-12): the unknown file becomes a document of the
        // folder's client (in the catalog, as a rebuild would have it).
        {
            let s = h.state_mut();
            let folder = s.root.join("Anderson Farms LLC (ANDE1042)");
            let id = filer_intake::sentinel::read_sentinel(&folder)
                .unwrap()
                .unwrap()
                .client_id
                .to_string();
            s.conn
                .execute(
                    "INSERT INTO client (id, ext_id, name, folder_rel, device_id, updated_at)
                     VALUES (?1, 'ANDE1042', 'Anderson Farms LLC', 'Anderson Farms LLC (ANDE1042)', 'dev', 't')",
                    [id],
                )
                .unwrap();
        }
        h.get_by_label("Anderson Farms LLC (ANDE1042)/Income Tax/2025/notice.pdf")
            .click();
        h.run();
        click(&mut h, "Adopt 1 where they are");
        h.run();
        let s = h.state();
        assert_eq!(
            s.sweep.notice.as_deref(),
            Some("1 files adopted."),
            "{:?}",
            s.sweep.error
        );
        let n: i64 = s
            .conn
            .query_row(
                "SELECT count(*) FROM document WHERE rel_path LIKE '%notice.pdf' AND deleted_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
        assert!(h.query_by_label("Not in the catalog (1)").is_none());
    }

    /// Owner 2026-11-05 (Q-62, Q-65): Admin › Merge cabinets scans another cabinet, lists its
    /// clients and, confirmed, moves their files into this one.
    #[test]
    fn merge_cabinets_page_moves_another_cabinets_clients_in() {
        let mut h = harness();
        let make = |root: &std::path::Path| {
            vf_fs::cabinet::create(
                root,
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
        };
        let other = h.state().root.parent().unwrap().join("Other cabinet");
        make(&h.state().root);
        make(&other);
        let mut oc = vf_core::db::open(
            &other.parent().unwrap().join("other.db"),
            vf_core::db::Storage::Local,
        )
        .unwrap();
        let np = vf_fs::newclient::plan(
            &oc,
            &other,
            &vf_fs::newclient::NewClient {
                partner: String::new(),
                ext_id: "ACME1002".into(),
                name: "Acme LLC".into(),
                entity_type: None,
            },
        )
        .unwrap();
        vf_fs::newclient::create(&mut oc, &other, &np, "dev").unwrap();
        let src = other.join(&np.folder_rel).join("Correspondence/letter.pdf");
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, b"invented").unwrap();

        click(&mut h, "Merge cabinets");
        type_into(
            &mut h,
            "Other cabinet's folder",
            &other.display().to_string(),
        );
        click(&mut h, "Scan");
        assert!(
            h.query_by_label("Acme LLC (ACME1002) — new client")
                .is_some()
        );
        click(&mut h, "Merge 1 clients (1 files)");
        h.get_by_role_and_label(egui::accesskit::Role::Button, "Yes, merge")
            .click();
        h.step();
        let start = std::time::Instant::now();
        while h.state().a.merge.last.is_none() {
            h.step();
            assert!(
                start.elapsed() < std::time::Duration::from_secs(20),
                "{:?}",
                h.state().a.merge.error
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        h.run();
        assert!(
            h.query_by_label_contains("Merged 1 clients (1 new)")
                .is_some()
        );
        assert!(!src.exists(), "moved out of the other cabinet");
        let s = h.state();
        let rel: String = s
            .conn
            .query_row(
                "SELECT rel_path FROM document WHERE filed_by = 'merge'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(rel.ends_with("Correspondence/letter.pdf"), "{rel}");
        assert!(s.root.join(&rel).is_file());
    }

    /// Q-63 (owner 2026-11-05): with no client list the scan says so, and creates a client
    /// from each unmatched folder's name; a row's ID can be changed; Import adds them.
    #[test]
    fn folder_import_creates_clients_from_folder_names() {
        use egui::accesskit::Role;
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
            for f in ["1 Farm Poultry LLC/Income Tax/2022", "Acme Ranch"] {
                std::fs::create_dir_all(s.root.join(f)).unwrap();
            }
            std::fs::write(
                s.root.join("1 Farm Poultry LLC/Income Tax/2022/1 Farm Poultry LLC_2022_1065_Tax_Return.pdf"),
                b"invented",
            )
            .unwrap();
        }
        click(&mut h, "Folder import");
        click(&mut h, "Scan the cabinet");
        assert!(
            h.query_by_label_contains("No client list is imported yet")
                .is_some()
        );
        click(&mut h, "Create a client for each unmatched folder (2)");
        let ids = |h: &Harness<'static, State>| -> Vec<String> {
            h.state()
                .a
                .import
                .plan
                .as_ref()
                .unwrap()
                .folders
                .iter()
                .filter_map(|r| r.client.as_ref().map(|c| c.ext_id.clone()))
                .collect()
        };
        assert_eq!(ids(&h), ["1FAR1001", "ACME1001"]);
        // Change one row's ID.
        type_into(&mut h, "New client ID for Acme Ranch", "9");
        h.key_press(egui::Key::Enter);
        h.run();
        let acme = ids(&h)[1].clone();
        assert!(acme != "ACME1001" && acme.contains('9'), "{acme}");
        h.get_by_role_and_label(Role::Button, "Import 2 folders")
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
        let s = h.state();
        let list = vf_fs::clients::load(&s.root).unwrap();
        let mut got: Vec<(String, String)> = list
            .clients()
            .iter()
            .map(|c| (c.id.clone(), c.name.clone()))
            .collect();
        got.sort();
        assert_eq!(
            got,
            [
                ("1FAR1001".to_string(), "1 Farm Poultry LLC".to_string()),
                (acme.clone(), "Acme Ranch".to_string())
            ]
        );
        assert!(s.root.join("Acme Ranch/_Vibe/client.json").is_file());
    }

    /// PA-28: the folder list filtered by match %, ticked in bulk and by Shift-click.
    #[test]
    fn folder_import_filters_by_match_and_ticks_in_bulk() {
        use egui::accesskit::Role;
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
                "ID,Name\nHALV1001,\"Halvorsen, Dana\"\nLIND1002,Lindqvist\nODUY1003,\"Oduya, Mara\"\nTOMA1004,\"Tomas, Pell\"\n",
            )
            .unwrap();
            let map = filer_core::config::ColumnMap {
                partner: None,
                id: Some("ID".into()),
                name: Some("Name".into()),
                extras: vec![],
            };
            vf_fs::clients::import_csv(&s.root, &csv, &map, &Default::default()).unwrap();
            for f in [
                "Archive Misc",
                "Dana Halvorsen",
                "Lindqvist",
                "Mara Oduya",
                "Pell Tomas",
            ] {
                std::fs::create_dir_all(s.root.join(f)).unwrap();
            }
        }
        click(&mut h, "Folder import");
        click(&mut h, "Scan the cabinet");
        assert!(h.query_by_label("Showing 5 of 5 folders").is_some());
        let ticked = |h: &Harness<'static, State>| -> Vec<String> {
            h.state()
                .a
                .import
                .plan
                .as_ref()
                .unwrap()
                .folders
                .iter()
                .filter(|r| r.include)
                .map(|r| r.rel.clone())
                .collect()
        };
        // Two-word names that agree are ticked; the one-word name is not.
        assert_eq!(ticked(&h), ["Dana Halvorsen", "Mara Oduya", "Pell Tomas"]);

        // 90 % and up: the three sure ones.
        h.state_mut().a.import.min_pct = 90;
        h.run();
        assert!(h.query_by_label("Showing 3 of 5 folders").is_some());
        click(&mut h, "Untick all shown");
        assert!(ticked(&h).is_empty());

        // All shown again: click one, Shift-click another: the folders between are ticked too.
        h.state_mut().a.import.min_pct = 0;
        h.run();
        // Row boxes, by their hover text: Archive Misc, Dana Halvorsen, Lindqvist, Mara Oduya,
        // Pell Tomas.
        let row_box = |h: &Harness<'static, State>, k: usize, shift: bool| {
            let rows: Vec<_> = h
                .get_all_by_label(
                    "Bind this folder to the client (Shift-click: the folders between too)",
                )
                .collect();
            assert_eq!(rows.len(), 5);
            if shift {
                rows[k].click_modifiers(egui::Modifiers::SHIFT);
            } else {
                rows[k].click();
            }
        };
        row_box(&h, 1, false);
        h.run();
        row_box(&h, 4, true);
        h.run();
        assert_eq!(
            ticked(&h),
            ["Dana Halvorsen", "Lindqvist", "Mara Oduya", "Pell Tomas"]
        );

        // Ticked folders the filter hides still import, and the button says so.
        h.state_mut().a.import.min_pct = 90;
        h.run();
        assert!(h.query_by_label("Import 4 folders (1 not shown)").is_some());
        // The Bind header unticks every folder shown.
        h.get_by_role_and_label(Role::CheckBox, "Bind").click();
        h.run();
        assert_eq!(ticked(&h), ["Lindqvist"]);
        assert!(h.query_by_label("Import 1 folders (1 not shown)").is_some());
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
                partner: None,
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

        // Edit… on a row changes its years (owner 2026-11-05).
        click(&mut h, "Edit Correspondence");
        assert_eq!(h.state().a.retention.years, "4");
        h.state_mut().a.retention.years = "6".into();
        h.run();
        click(&mut h, "Save");
        assert!(h.state().a.retention.error.is_none());
        assert!(
            h.query_by_label("6 years from the end of the tax year")
                .is_some()
        );
        let years: i64 = h
            .state()
            .conn
            .query_row(
                "SELECT years FROM retention_class WHERE name = 'Correspondence' AND deleted_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(years, 6);
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
