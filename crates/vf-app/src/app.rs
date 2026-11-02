//! Top-level app: the cabinet wizard until a cabinet is open, then tab routing over the shared
//! chrome.

use std::path::Path;

use vf_security::keys::Kdf;

use crate::session::Session;
use crate::ui::browse::{self, Browse};
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
pub struct Workspace {
    pub session: Session,
    pub notice: Option<String>,
    pub browse: Browse,
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
        Self {
            session,
            notice: None,
            browse,
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
        egui::CentralPanel::default().frame(pane()).show(ui, |ui| {
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
        ui.label(
            egui::RichText::new(match n {
                0 => "Nothing selected".to_owned(),
                1 => "1 document selected".to_owned(),
                n => format!("{n} documents selected"),
            })
            .strong(),
        );
        self.browse.detail_ui(ui, &self.session.conn);
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
                w.browse.refresh(&w.session.conn);
                let s = &w.session;
                let root = s.root.display().to_string();
                let who = s.actor.user.clone().unwrap_or_default();
                chrome::header(ui, Some((&s.cabinet.firm_name, &root)), "Local", &who);
                chrome::tab_bar(ui, &mut self.tab, 0);
                let tab = self.tab;
                chrome::footer(ui, &w.footer_segments(tab), "Catalog open");
                match tab {
                    Tab::Browse => w.browse_ui(ui),
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
