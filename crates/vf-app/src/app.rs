//! Top-level app: tab routing over the shared chrome. Each tab is a stub until its phase.

use std::path::PathBuf;
use std::sync::mpsc;

use vf_security::keys::KdfParams;

use crate::config::{self, Config};
use crate::session::{self, CreateRequest, Opened};
use crate::ui::admin::{self, Admin};
use crate::ui::browse::{self, Browse};
use crate::ui::wizard::{self, Mode, Wizard};
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
            Tab::Browse => "Browse shows the client tree once a cabinet is open.",
            Tab::Intake => "Intake mounts the vendored Filer Desktop engine (P02).",
            Tab::Search => "Full-text search arrives with OCR and indexing (P03).",
            Tab::Viewer => "The PDF viewer arrives in P04.",
            Tab::Sync => "B2 sync arrives in P11.",
            Tab::Admin => "Users, doc types, audit and compliance settings.",
        }
    }
}

/// What the central area shows: the wizard until a cabinet is open.
pub enum Screen {
    Wizard(Wizard),
    /// Create/open runs off the UI thread (Argon2 and the first rebuild take a while).
    Busy {
        what: &'static str,
        rx: mpsc::Receiver<Result<Opened, String>>,
        back: Wizard,
    },
    Open(Box<OpenCabinet>),
}

/// An open cabinet and its tab state.
pub struct OpenCabinet {
    pub opened: Opened,
    pub browse: Browse,
    /// Built the first time the Admin tab is shown.
    pub admin: Option<Admin>,
}

pub struct VibeFilesApp {
    pub tab: Tab,
    pub data: PathBuf,
    pub config: Config,
    pub screen: Screen,
    /// Argon2id cost for new cabinets (tests lower it).
    pub kdf: KdfParams,
}

impl VibeFilesApp {
    pub fn new(ctx: &egui::Context, data: PathBuf) -> Self {
        theme::install(ctx);
        let (config, error) = match config::load(&data) {
            Ok(c) => (c, None),
            Err(e) => (
                Config {
                    device_id: vf_core::new_id(),
                    last_cabinet: None,
                    prefs: Default::default(),
                },
                Some(format!("Settings could not be read: {e:#}")),
            ),
        };
        let mut w = Wizard::new(config.last_cabinet.as_deref());
        w.error = error;
        Self {
            tab: Tab::Browse,
            data,
            config,
            screen: Screen::Wizard(w),
            kdf: KdfParams::DEFAULT,
        }
    }

    fn start(&mut self, mode: Mode, ctx: &egui::Context) {
        let Screen::Wizard(w) = &mut self.screen else {
            return;
        };
        let root = PathBuf::from(w.root.trim());
        if mode == Mode::Create {
            let req = CreateRequest {
                root: &root,
                name: &w.name,
                passphrase: &w.passphrase,
                confirm: &w.confirm,
            };
            if let Err(e) = session::check_create(&req) {
                w.error = Some(e);
                return;
            }
        } else if root.as_os_str().is_empty() {
            w.error = Some("Choose the cabinet folder.".into());
            return;
        }
        let (tx, rx) = mpsc::channel();
        let (data, device, kdf) = (self.data.clone(), self.config.device_id.clone(), self.kdf);
        let (name, pass, confirm) = (w.name.clone(), w.passphrase.clone(), w.confirm.clone());
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = match mode {
                Mode::Create => session::create(
                    &data,
                    &device,
                    &CreateRequest {
                        root: &root,
                        name: &name,
                        passphrase: &pass,
                        confirm: &confirm,
                    },
                    kdf,
                ),
                Mode::Open => session::open(&data, &device, &root, &pass),
            };
            let _ = tx.send(r.map_err(|e| format!("{e:#}")));
            ctx.request_repaint();
        });
        let mut back = w.clone();
        back.clear_secrets();
        back.error = None;
        self.screen = Screen::Busy {
            what: match mode {
                Mode::Create => "Creating the cabinet and indexing its folders…",
                Mode::Open => "Opening the cabinet…",
            },
            rx,
            back,
        };
    }

    /// Picks up a finished create/open.
    fn poll(&mut self) {
        let Screen::Busy { rx, back, .. } = &mut self.screen else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("The work stopped unexpectedly.".into()),
        };
        match result {
            Ok(opened) => {
                self.config.last_cabinet = Some(opened.root.clone());
                // The cabinet is open either way; a settings write failure only loses "last".
                let _ = config::save(&self.data, &self.config);
                let mut browse = Browse::load(&opened.conn);
                browse.set_prefs(self.config.prefs.clone());
                browse.notice = opened.rebuild.as_ref().map(|r| format!("Indexed: {r}"));
                self.screen = Screen::Open(Box::new(OpenCabinet {
                    opened,
                    browse,
                    admin: None,
                }));
            }
            Err(e) => {
                let mut w = back.clone();
                w.error = Some(e);
                self.screen = Screen::Wizard(w);
            }
        }
    }

    pub fn cabinet_name(&self) -> Option<String> {
        match &self.screen {
            Screen::Open(o) => Some(o.opened.cabinet.display_name(&o.opened.root)),
            _ => None,
        }
    }
}

impl eframe::App for VibeFilesApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll();
        let name = self.cabinet_name();
        chrome::header(ui, name.as_deref());
        chrome::tab_bar(ui, &mut self.tab, 0);
        let mut segments = vec![format!("vibe-files {}", env!("CARGO_PKG_VERSION"))];
        if let Screen::Open(o) = &self.screen {
            segments.insert(
                0,
                format!(
                    "{} documents · {} clients",
                    o.browse.snap.docs.len(),
                    o.browse.snap.clients.len()
                ),
            );
        }
        chrome::footer(ui, &segments, name.as_deref().unwrap_or("No cabinet open"));
        let ctx = ui.ctx().clone();
        let mut submit = None;
        match &mut self.screen {
            Screen::Open(o) if matches!(self.tab, Tab::Browse | Tab::Admin) => {
                let OpenCabinet {
                    opened,
                    browse: tab_browse,
                    admin: tab_admin,
                } = &mut **o;
                let mut env = browse::Env {
                    conn: &mut opened.conn,
                    root: &opened.root,
                    device_id: &self.config.device_id,
                };
                if self.tab == Tab::Admin {
                    let a = tab_admin.get_or_insert_with(|| Admin::load(&env));
                    admin::show(ui, a, &mut env);
                } else {
                    if tab_admin
                        .as_mut()
                        .is_some_and(|a| std::mem::take(&mut a.changed))
                    {
                        tab_browse.reload(env.conn);
                    }
                    browse::show(ui, tab_browse, &mut env);
                }
            }
            screen => {
                egui::CentralPanel::default().show(ui, |ui| match screen {
                    Screen::Wizard(w) => submit = wizard::show(ui, w),
                    Screen::Busy { what, .. } => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(*what);
                        });
                    }
                    Screen::Open(_) => {
                        ui.heading(self.tab.name());
                        ui.label(self.tab.placeholder());
                    }
                });
            }
        }
        if let Screen::Open(o) = &mut self.screen
            && std::mem::take(&mut o.browse.prefs_dirty)
        {
            self.config.prefs = o.browse.prefs.clone();
            if let Err(e) = config::save(&self.data, &self.config) {
                o.browse.error = Some(format!("Pinned/Favorites could not be saved: {e:#}"));
            }
        }
        if let Some(mode) = submit {
            self.start(mode, &ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::ui::test_util::type_into;
    use std::path::Path;

    use super::*;
    use crate::session::tests::FAST;
    use egui::accesskit::Role;
    use egui_kittest::HarnessBuilder;
    use egui_kittest::kittest::Queryable;

    fn harness(data: &Path) -> egui_kittest::Harness<'static, VibeFilesApp> {
        let data = data.to_owned();
        let mut h = HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 900.0])
            .build_eframe(move |cc| VibeFilesApp::new(&cc.egui_ctx, data.clone()));
        h.state_mut().kdf = FAST;
        h.run();
        h
    }

    fn click(h: &mut egui_kittest::Harness<'static, VibeFilesApp>, label: &str) {
        h.get_by_role_and_label(Role::Button, label).click();
        h.step();
    }

    /// Steps until the background create/open is done.
    fn settle(h: &mut egui_kittest::Harness<'static, VibeFilesApp>) {
        for _ in 0..3000 {
            h.step();
            if !matches!(h.state().screen, Screen::Busy { .. }) {
                h.run();
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("create/open never finished");
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
    fn opens_on_browse_and_every_tab_switches() {
        let tmp = tempfile::tempdir().unwrap();
        let mut h = harness(tmp.path());
        assert_eq!(h.state().tab, Tab::Browse);
        for t in Tab::ALL {
            h.get_by_role_and_label(Role::Button, t.name()).click();
            h.run();
            assert_eq!(h.state().tab, t);
        }
    }

    /// P01 step 1: the wizard creates a cabinet whose passphrase opens it on another PC.
    #[test]
    fn wizard_creates_a_cabinet() {
        let tmp = tempfile::tempdir().unwrap();
        let (data, root) = (tmp.path().join("data"), tmp.path().join("Clients"));
        let mut h = harness(&data);
        click(&mut h, "Create new");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
        type_into(&mut h, "Cabinet name", "Kisaes Clients");
        type_into(&mut h, "Recovery passphrase", "correct horse battery");
        type_into(&mut h, "Confirm passphrase", "correct horse battery");
        click(&mut h, "Create cabinet");
        settle(&mut h);

        assert!(matches!(h.state().screen, Screen::Open(_)), "cabinet open");
        assert_eq!(h.state().cabinet_name().as_deref(), Some("Kisaes Clients"));
        assert!(h.query_all_by_label("Kisaes Clients").count() >= 1);
        let cab = vf_fs::cabinet::read(&root).unwrap();
        assert_eq!(cab.name.as_deref(), Some("Kisaes Clients"));
        assert!(vf_fs::cabinet::recovery_key_file(&root).is_file());
        assert!(vf_fs::doctypes::path(&root).is_file(), "doc types seeded");
        let Screen::Open(o) = &h.state().screen else {
            unreachable!()
        };
        let n: i64 = o
            .opened
            .conn
            .query_row(
                "SELECT count(*) FROM doc_type WHERE deleted_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n as usize, vf_fs::doctypes::SEED.len());
        // Pins made in Browse are saved to this PC's config.toml.
        if let Screen::Open(o) = &mut h.state_mut().screen {
            o.browse.prefs.pinned.push("doc-1".into());
            o.browse.prefs_dirty = true;
        }
        h.run();
        assert_eq!(config::load(&data).unwrap().prefs.pinned, ["doc-1"]);
        assert!(data.join(session::CATALOG_FILE).is_file());
        assert_eq!(
            config::load(&data).unwrap().last_cabinet.as_deref(),
            Some(root.as_path())
        );
        drop(h);
        // The passphrase alone opens it from an empty data folder.
        let other = tmp.path().join("pc2");
        assert!(session::open(&other, "dev-2", &root, "correct horse battery").is_ok());
    }

    #[test]
    fn wizard_reports_a_wrong_passphrase_then_opens() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Clients");
        let req = CreateRequest {
            root: &root,
            name: "Firm",
            passphrase: "correct horse battery",
            confirm: "correct horse battery",
        };
        drop(session::create(&tmp.path().join("pc1"), "dev-1", &req, FAST).unwrap());

        let mut h = harness(&tmp.path().join("pc2"));
        click(&mut h, "Open existing");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
        type_into(&mut h, "Recovery passphrase", "not the passphrase");
        click(&mut h, "Open cabinet");
        settle(&mut h);
        let Screen::Wizard(w) = &h.state().screen else {
            panic!("should stay on the wizard");
        };
        assert!(w.error.as_deref().unwrap().contains("passphrase is wrong"));
        assert!(w.passphrase.is_empty(), "passphrase cleared after use");

        type_into(&mut h, "Recovery passphrase", "correct horse battery");
        click(&mut h, "Open cabinet");
        settle(&mut h);
        assert_eq!(h.state().cabinet_name().as_deref(), Some("Firm"));
    }

    #[test]
    fn wizard_checks_fields_before_any_work() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Clients");
        let mut h = harness(&tmp.path().join("data"));
        click(&mut h, "Create new");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
        type_into(&mut h, "Cabinet name", "Firm");
        type_into(&mut h, "Recovery passphrase", "short");
        type_into(&mut h, "Confirm passphrase", "short");
        click(&mut h, "Create cabinet");
        h.run();
        let Screen::Wizard(w) = &h.state().screen else {
            panic!("should stay on the wizard");
        };
        assert!(w.error.as_deref().unwrap().contains("at least 12"));
        assert!(!root.exists(), "nothing written");
    }
}
