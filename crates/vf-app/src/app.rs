//! Top-level app: the cabinet wizard until a cabinet is open, then tab routing over the shared
//! chrome.

use std::path::Path;

use vf_security::keys::Kdf;

use crate::session::Session;
use crate::ui::admin::{self, Admin};
use crate::ui::browse::{self, Browse};
use crate::ui::clients::{
    self as client_ui, ClientEdit, ImportDialog, NewClientDialog, Outcome, RelForm,
};
use crate::ui::wizard::Wizard;
use crate::ui::{chrome, theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Browse,
    Intake,
    Search,
    Viewer,
    Sync,
    Admin,
}

impl Tab {
    /// CLAUDE.md conventions: tabs in exactly this order.
    pub const ALL: [Tab; 6] = [
        Tab::Browse,
        Tab::Intake,
        Tab::Search,
        Tab::Viewer,
        Tab::Sync,
        Tab::Admin,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Tab::Browse => "Browse",
            Tab::Intake => "Intake",
            Tab::Search => "Search",
            Tab::Viewer => "Viewer",
            Tab::Sync => "Sync",
            Tab::Admin => "Admin",
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Tab::Browse => "Create your first client or run the import wizard.",
            Tab::Intake => "Intake mounts the vendored Filer Desktop engine (P02).",
            Tab::Search => "Full-text search arrives with OCR and indexing (P03).",
            Tab::Viewer => "The PDF viewer arrives in P04.",
            Tab::Sync => "B2 sync arrives in P11.",
            Tab::Admin => "Users, doc types, audit and compliance settings.",
        }
    }
}

/// An open cabinet and the per-tab state that goes with it.
pub enum Dialog {
    NewClient(NewClientDialog),
    Import(Box<ImportDialog>),
}

pub struct Workspace {
    pub session: Session,
    pub notice: Option<String>,
    pub browse: Browse,
    pub dialog: Option<Dialog>,
    /// The cabinet's folder template (`_Vibe/template.json`, Filer format).
    pub template: filer_core::template::TemplateNode,
    pub rel_form: RelForm,
    /// The last transaction this session committed: `(tx id, label)`.
    pub last_tx: Option<(String, String)>,
    pub admin: Admin,
    /// Live doc types (pickers), refreshed with the browse generation.
    pub doc_types: Vec<vf_core::doctype::DocType>,
    doc_types_gen: i64,
    pub meta_form: crate::ui::meta::MetaForm,
}

fn pane() -> egui::Frame {
    egui::Frame::new()
        .fill(theme::PANE)
        .stroke(egui::Stroke::new(1.0, theme::BORDER_PANE))
        .inner_margin(egui::Margin::same(10))
}

impl Workspace {
    pub fn new(session: Session) -> Self {
        let mut browse = Browse::default();
        browse.refresh(&session.conn);
        let template = vf_fs::clients::template(&session.root);
        Self {
            session,
            notice: None,
            browse,
            dialog: None,
            template,
            rel_form: RelForm::default(),
            last_tx: None,
            admin: Admin::default(),
            doc_types: Vec::new(),
            doc_types_gen: -1,
            meta_form: Default::default(),
        }
    }

    /// Undoes the last transaction this session committed (the per-user undo stack is P05).
    pub fn undo_last(&mut self) {
        let Some((tx, label)) = self.last_tx.clone() else {
            return;
        };
        let s = &mut self.session;
        let r = vf_fs::fileops::FilerFileOps::new(&[&s.root])
            .map_err(|e| vf_core::Error::Tx(e.to_string()))
            .and_then(|mut fs| vf_core::tx::undo(&mut s.conn, &mut fs, &s.actor, &tx));
        match r {
            Ok(rows) => {
                let dir = s.root.join(vf_fs::cabinet::VIBE_DIR).join("audit");
                let _ = vf_core::audit::mirror_append(&dir, &rows);
                let _ = vf_fs::clients::mirror_relationships(&s.conn, &s.root);
                self.notice = Some(format!("Undone: {label}."));
                self.last_tx = None;
            }
            Err(e) => self.notice = Some(format!("Cannot undo {label}: {e}")),
        }
    }

    /// Re-reads catalog-derived state when anything changed.
    pub fn refresh(&mut self) {
        self.browse.refresh(&self.session.conn);
        let generation = vf_core::browse::generation(&self.session.conn).unwrap_or(-2);
        if generation != self.doc_types_gen {
            self.doc_types = vf_core::doctype::list(&self.session.conn, true).unwrap_or_default();
            self.doc_types_gen = generation;
        }
    }

    fn admin_ui(&mut self, ui: &mut egui::Ui) {
        let live = self.doc_types.iter().filter(|t| !t.archived).count();
        egui::Panel::left("admin_nav")
            .resizable(false)
            .exact_size(248.0)
            .frame(pane())
            .show(ui, |ui| self.admin.nav_ui(ui, live));
        egui::CentralPanel::default()
            .frame(pane())
            .show(ui, |ui| match self.admin.page {
                admin::Page::DocTypes => {
                    let types = self.doc_types.clone();
                    if let Some(a) = self.admin.doc_types_ui(ui, &types) {
                        let s = &mut self.session;
                        let r = match a {
                            admin::DocTypeAction::Save(e) => {
                                vf_core::doctype::save(&mut s.conn, &s.actor, &e)
                                    .map(|(_, d)| (d, "doc type"))
                            }
                            admin::DocTypeAction::Archive(id, to) => {
                                vf_core::doctype::set_archived(&mut s.conn, &s.actor, &id, to)
                                    .map(|d| (d, "archive doc type"))
                            }
                        };
                        match r {
                            Ok((done, label)) => {
                                self.committed(&done, label);
                                self.admin.edit = None;
                                self.admin.error = None;
                            }
                            Err(e) => self.admin.error = Some(e.to_string()),
                        }
                    }
                }
                admin::Page::Audit => self.admin.audit_ui(ui, &self.session.conn),
            });
    }

    /// Every app-level commit lands here: mirror the audit rows to `_Vibe/audit/*.jsonl` and
    /// remember the transaction for Undo.
    pub fn committed(&mut self, c: &vf_core::tx::Committed, label: &str) {
        let dir = self
            .session
            .root
            .join(vf_fs::cabinet::VIBE_DIR)
            .join("audit");
        if let Err(e) = vf_core::audit::mirror_append(&dir, &c.audit) {
            self.notice = Some(format!(
                "Saved, but the audit mirror could not be written: {e}"
            ));
        }
        self.last_tx = Some((c.tx_id.clone(), label.to_owned()));
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .add(crate::ui::widgets::primary("+ New client"))
                .clicked()
            {
                self.dialog = Some(Dialog::NewClient(NewClientDialog::default()));
            }
            if ui.button("Import clients…").clicked()
                && let Some(p) = rfd::FileDialog::new()
                    .add_filter("CSV", &["csv", "txt"])
                    .pick_file()
            {
                self.dialog = Some(Dialog::Import(Box::new(ImportDialog::new(p))));
            }
        });
        ui.add_space(4.0);
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.dialog else { return };
        let s = &mut self.session;
        match d {
            Dialog::NewClient(nc) => match nc.ui(ctx, &self.template) {
                Outcome::Open => {}
                Outcome::Cancel => self.dialog = None,
                Outcome::Done(c) => {
                    match vf_fs::clients::create_client(
                        &mut s.conn,
                        &s.root,
                        &s.cabinet,
                        &self.template,
                        &c,
                        &s.actor,
                    ) {
                        Ok((id, done)) => {
                            self.committed(&done, &format!("new client {}", c.name.trim()));
                            self.dialog = None;
                            self.browse.select_node(browse::Node::Client(id));
                        }
                        Err(e) => nc.error = Some(e.to_string()),
                    }
                }
            },
            Dialog::Import(im) => match im.ui(ctx) {
                Outcome::Open => {}
                Outcome::Cancel => self.dialog = None,
                Outcome::Done(client_ui::ImportStep::Preview) => {
                    im.plan = Some(im.list().and_then(|list| {
                        vf_fs::clients::plan_import(&s.conn, &s.root, &self.template, &list)
                            .map_err(|e| e.to_string())
                    }));
                }
                Outcome::Done(client_ui::ImportStep::Commit(create)) => {
                    let Some(Ok(plan)) = &im.plan else { return };
                    match vf_fs::clients::commit_import(
                        &mut s.conn,
                        &s.root,
                        &s.cabinet,
                        &self.template,
                        plan,
                        create,
                        &s.actor,
                    ) {
                        Ok((rep, done)) => {
                            if let Some(done) = done {
                                self.committed(&done, "import clients");
                            }
                            self.notice = Some(format!(
                                "Clients imported: {} added, {} updated, {} folders bound, {} created, {} without a folder.",
                                rep.added,
                                rep.updated,
                                rep.bound,
                                rep.created_folders,
                                rep.without_folder
                            ));
                            self.dialog = None;
                        }
                        Err(e) => im.error = Some(e.to_string()),
                    }
                }
            },
        }
    }

    fn client_edit(&mut self, client_id: &str, e: ClientEdit) {
        let s = &mut self.session;
        let r = match e {
            ClientEdit::EntityType(t) => {
                vf_fs::clients::set_entity_type(&mut s.conn, &s.actor, client_id, t.as_deref())
                    .map(|d| (d, "entity type"))
            }
            ClientEdit::AddRel {
                parent,
                child,
                kind,
                share,
                note,
            } => vf_fs::clients::add_relationship(
                &mut s.conn,
                &s.root,
                &s.actor,
                &parent,
                &child,
                &kind,
                share,
                &note,
            )
            .map(|d| (d, "add relationship")),
            ClientEdit::RemoveRel {
                parent,
                child,
                kind,
            } => vf_fs::clients::remove_relationship(
                &mut s.conn,
                &s.root,
                &s.actor,
                &parent,
                &child,
                &kind,
            )
            .map(|d| (d, "remove relationship")),
        };
        match r {
            Ok((done, label)) => {
                self.committed(&done, label);
                self.rel_form = RelForm::default();
            }
            Err(e) => self.rel_form.error = Some(e.to_string()),
        }
    }

    pub fn footer_segments(&self, tab: Tab) -> Vec<String> {
        match tab {
            Tab::Browse => vec![
                format!(
                    "{} documents · {} clients",
                    self.browse.totals.0, self.browse.totals.1
                ),
                "Catalog: this PC".into(),
            ],
            _ => vec![format!("vibe-files {}", env!("CARGO_PKG_VERSION"))],
        }
    }

    fn browse_ui(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("tree")
            .resizable(true)
            .default_size(272.0)
            .frame(pane())
            .show(ui, |ui| self.browse.tree_ui(ui));
        egui::Panel::right("inspector")
            .resizable(true)
            .default_size(320.0)
            .frame(pane())
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("inspector")
                    .show(ui, |ui| self.inspector_ui(ui));
            });
        self.dialogs(ui.ctx());
        let undo_key = self.dialog.is_none()
            && ui.memory(|m| m.focused().is_none())
            && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Z));
        if undo_key {
            self.undo_last();
        }
        egui::CentralPanel::default().frame(pane()).show(ui, |ui| {
            self.toolbar(ui);
            if let Some(n) = self.notice.clone() {
                ui.horizontal(|ui| {
                    ui.label(crate::ui::widgets::muted(n));
                    if ui.small_button("Dismiss").clicked() {
                        self.notice = None;
                    }
                });
            }
            if let Some(e) = &self.browse.error {
                crate::ui::widgets::banner(ui, e, true);
            }
            if let Some(a) = self.browse.grid_ui(ui) {
                self.handle(a);
            }
        });
    }

    fn inspector_ui(&mut self, ui: &mut egui::Ui) {
        let n = self.browse.selected.len();
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(match n {
                    0 => "Nothing selected".to_owned(),
                    1 => "1 document selected".to_owned(),
                    n => format!("{n} documents selected"),
                })
                .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let tip = self
                    .last_tx
                    .as_ref()
                    .map_or("Nothing to undo".to_owned(), |(_, l)| {
                        format!("Undo {l} (Ctrl+Z)")
                    });
                if ui
                    .add_enabled(self.last_tx.is_some(), egui::Button::new("Undo last"))
                    .on_hover_text(&tip)
                    .on_disabled_hover_text(&tip)
                    .clicked()
                {
                    self.undo_last();
                }
            });
        });
        if n > 0 {
            let generation = vf_core::browse::generation(&self.session.conn).unwrap_or(0);
            let rows: Vec<vf_core::browse::DocRow> =
                self.browse.selected_rows().into_iter().cloned().collect();
            let refs: Vec<&vf_core::browse::DocRow> = rows.iter().collect();
            self.meta_form.sync(generation, &refs);
            if let Some(change) = self.meta_form.ui(ui, &refs, &self.doc_types) {
                let ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
                let s = &mut self.session;
                match vf_core::meta::bulk_edit(&mut s.conn, &s.actor, &ids, &change) {
                    Ok(Some(done)) => {
                        self.committed(&done, &format!("edit of {} documents", ids.len()))
                    }
                    Ok(None) => {}
                    Err(e) => self.meta_form.error = Some(e.to_string()),
                }
            }
        }
        self.browse.detail_ui(ui, &self.session.conn);
        if n == 0
            && let Some(cid) = self
                .browse
                .node
                .as_ref()
                .and_then(|n| n.client_id())
                .map(str::to_owned)
            && let Some(client) = self.browse.clients().iter().find(|c| c.id == cid).cloned()
        {
            let conn = &self.session.conn;
            let rels = vf_fs::clients::relationships(conn, &cid).unwrap_or_default();
            let others: Vec<(String, String, String)> = vf_core::browse::clients(conn, "")
                .unwrap_or_default()
                .into_iter()
                .map(|c| (c.id, c.name, c.ext_id))
                .collect();
            if let Some(e) =
                client_ui::client_panel(ui, &client, &rels, &others, &mut self.rel_form)
            {
                self.client_edit(&cid, e);
            }
        }
    }

    fn handle(&mut self, a: browse::Action) {
        match a {
            browse::Action::Open(_) => {}
        }
    }
}

pub enum Screen {
    Wizard(Box<Wizard>),
    Open(Box<Workspace>),
}

pub struct VibeFilesApp {
    pub tab: Tab,
    pub screen: Screen,
}

impl VibeFilesApp {
    pub fn new(ctx: &egui::Context, data: &Path) -> Self {
        Self::with_kdf(ctx, data, Kdf::DEFAULT)
    }

    pub fn with_kdf(ctx: &egui::Context, data: &Path, kdf: Kdf) -> Self {
        theme::install(ctx);
        Self {
            tab: Tab::Browse,
            screen: Screen::Wizard(Box::new(Wizard::new(data, kdf))),
        }
    }

    pub fn open(&mut self, session: Session) {
        self.screen = Screen::Open(Box::new(Workspace::new(session)));
        self.tab = Tab::Browse;
    }

    pub fn workspace(&mut self) -> Option<&mut Workspace> {
        match &mut self.screen {
            Screen::Open(w) => Some(w),
            Screen::Wizard(_) => None,
        }
    }
}

impl eframe::App for VibeFilesApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        match &mut self.screen {
            Screen::Wizard(w) => {
                let version = format!("vibe-files {}", env!("CARGO_PKG_VERSION"));
                chrome::header(ui, None, "", "");
                chrome::footer(ui, &[version], "No cabinet open");
                if let Some((session, report)) = w.ui(ui) {
                    let notice = report.map(|r| format!("Catalog built: {r}"));
                    self.open(session);
                    if let Some(w) = self.workspace() {
                        w.notice = notice;
                    }
                }
            }
            Screen::Open(w) => {
                w.refresh();
                let s = &w.session;
                let root = s.root.display().to_string();
                let who = s.actor.user.clone().unwrap_or_default();
                chrome::header(ui, Some((&s.cabinet.firm_name, &root)), "Local", &who);
                chrome::tab_bar(ui, &mut self.tab, 0);
                let tab = self.tab;
                chrome::footer(ui, &w.footer_segments(tab), "Catalog open");
                match tab {
                    Tab::Browse => w.browse_ui(ui),
                    Tab::Admin => w.admin_ui(ui),
                    _ => {
                        egui::CentralPanel::default().show(ui, |ui| {
                            ui.heading(tab.name());
                            ui.label(tab.placeholder());
                        });
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::HarnessBuilder;
    use egui_kittest::kittest::Queryable;

    fn harness(data: &Path) -> egui_kittest::Harness<'static, VibeFilesApp> {
        let data = data.to_path_buf();
        HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 900.0])
            .build_eframe(move |cc| VibeFilesApp::with_kdf(&cc.egui_ctx, &data, Kdf::FAST))
    }

    #[test]
    fn six_tabs_in_spec_order() {
        let names: Vec<_> = Tab::ALL.iter().map(|t| t.name()).collect();
        assert_eq!(
            names,
            ["Browse", "Intake", "Search", "Viewer", "Sync", "Admin"]
        );
    }

    #[test]
    fn empty_data_folder_offers_create_and_open() {
        let tmp = tempfile::tempdir().unwrap();
        let mut h = harness(&tmp.path().join("data"));
        h.run();
        h.get_by_label("Create new cabinet");
        h.get_by_label("Open existing cabinet");
    }

    #[test]
    fn wizard_creates_a_cabinet_and_opens_browse() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Cabinet");
        std::fs::create_dir(&root).unwrap();
        let mut h = harness(&tmp.path().join("data"));
        h.run();
        h.get_by_label("Create new cabinet").click();
        h.run();
        let Screen::Wizard(w) = &mut h.state_mut().screen else {
            panic!("wizard expected")
        };
        w.root = root.display().to_string();
        w.firm = "Kisaes LLC".into();
        w.pass = crate::session::tests::PASS.into();
        w.pass2 = crate::session::tests::PASS.into();
        h.run();
        h.get_by_label("Create cabinet").click();
        for _ in 0..200 {
            h.step();
            if matches!(h.state().screen, Screen::Open(_)) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(matches!(h.state().screen, Screen::Open(_)));
        h.run();
        h.get_by_label("Kisaes LLC");
        for t in Tab::ALL {
            h.get_by_role_and_label(egui::accesskit::Role::Button, t.name())
                .click();
            h.run();
            assert_eq!(h.state().tab, t);
        }
    }

    fn open_fixture() -> (
        tempfile::TempDir,
        egui_kittest::Harness<'static, VibeFilesApp>,
    ) {
        let (tmp, s) = crate::session::tests::fixture_cabinet();
        let mut h = harness(&tmp.path().join("unused"));
        h.state_mut().open(s);
        h.run();
        (tmp, h)
    }

    #[test]
    fn browse_tree_to_grid_to_inspector() {
        let (_tmp, mut h) = open_fixture();
        let w = h.state_mut().workspace().unwrap();
        assert_eq!(w.browse.totals, (60, 5));
        let first = w.browse.clients_for_test()[0].clone();
        h.run();
        h.get_by_label(&first.name).click();
        h.run();
        assert!(h.state_mut().workspace().unwrap().browse.rows.len() == first.docs as usize);
        h.get_by_role_and_label(egui::accesskit::Role::Button, "2025")
            .click();
        h.run();
        let n2025 = h.state_mut().workspace().unwrap().browse.rows.len();
        assert!(n2025 > 0);
        h.get_by_label("Income Tax/2025").click();
        h.run();
        // Sort by size, then select a row by keyboard and by click.
        h.get_by_label("Size").click();
        h.run();
        h.key_press(egui::Key::ArrowDown);
        h.run();
        assert_eq!(h.state_mut().workspace().unwrap().browse.selected.len(), 1);
        h.get_by_label("1 document selected");
        h.get_by_label("SHA-256");
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        h.run();
        let w = h.state_mut().workspace().unwrap();
        let rows = w.browse.rows.clone();
        assert_eq!(w.browse.selected.len(), rows.len());
        let sizes: Vec<i64> = rows.iter().map(|d| d.size).collect();
        assert!(sizes.windows(2).all(|p| p[0] <= p[1]), "sorted by size");
    }

    #[test]
    fn locked_catalog_asks_for_the_passphrase() {
        let (tmp, s) = crate::session::tests::new_cabinet();
        let data = s.data.clone();
        drop(s);
        let mut h = harness(&data);
        h.run();
        h.get_by_label("Unlock the catalog");
        drop(tmp);
    }
}
