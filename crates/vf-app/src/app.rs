//! Top-level app: tab routing over the shared chrome. Each tab is a stub until its phase.

use std::path::PathBuf;
use std::sync::mpsc;

use vf_security::keys::KdfParams;

use crate::config::{self, Config};
use crate::session::{self, CreateRequest, Opened};
use crate::ui::admin::{self, Admin};
use crate::ui::browse::{self, Browse};
use crate::ui::intake::IntakeTab;
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
            Tab::Intake => "Intake files documents into the open cabinet.",
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
    /// The mounted Filer Desktop app, built the first time Intake is shown.
    pub intake: IntakeTab,
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
                    ingest: Default::default(),
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
            Ok(mut opened) => {
                // A crash between Filer's commit and the catalog write is caught up here.
                let mut intake = IntakeTab::default();
                let root = opened.root.clone();
                intake.reconcile(&mut opened.conn, &root, &self.config.device_id);
                self.config.last_cabinet = Some(opened.root.clone());
                // The cabinet is open either way; a settings write failure only loses "last".
                let _ = config::save(&self.data, &self.config);
                let mut browse = Browse::load(&opened.conn);
                browse.sheets_dir = self.data.join("sheets");
                browse.set_prefs(self.config.prefs.clone());
                browse.notice = opened.rebuild.as_ref().map(|r| format!("Indexed: {r}"));
                self.screen = Screen::Open(Box::new(OpenCabinet {
                    opened,
                    browse,
                    admin: None,
                    intake,
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
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.poll();
        let name = self.cabinet_name();
        chrome::header(ui, name.as_deref());
        let badge = match &self.screen {
            Screen::Open(o) => o.intake.ingest.badge(),
            _ => 0,
        };
        chrome::tab_bar(ui, &mut self.tab, badge);
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
            Screen::Open(o) if self.tab == Tab::Intake => {
                let o = &mut **o;
                o.intake.show(
                    ui,
                    frame,
                    &o.opened.root,
                    &o.opened.cabinet,
                    &self.config.ingest,
                    &o.browse.snap,
                    &o.browse.sheets_dir,
                );
                o.intake
                    .reconcile(&mut o.opened.conn, &o.opened.root, &self.config.device_id);
            }
            Screen::Open(o) if matches!(self.tab, Tab::Browse | Tab::Admin) => {
                let OpenCabinet {
                    opened,
                    browse: tab_browse,
                    admin: tab_admin,
                    intake: o_intake,
                } = &mut **o;
                let mut env = browse::Env {
                    conn: &mut opened.conn,
                    root: &opened.root,
                    device_id: &self.config.device_id,
                };
                if self.tab == Tab::Admin {
                    let a = tab_admin.get_or_insert_with(|| Admin::load(&env));
                    a.capture = self.config.ingest.print_capture.clone();
                    admin::show(ui, a, &mut env);
                    if let Some(dir) = a.open_request.take()
                        && let Err(e) = (tab_browse.shell)(&crate::shell::ShellAction::Open(dir))
                    {
                        a.error = Some(e);
                    }
                    if let Some(c) = a.capture_changed.take() {
                        let mut ingest = self.config.ingest.clone();
                        ingest.print_capture = c;
                        o_intake.ingest.changed = Some(ingest);
                    }
                } else {
                    let intake_changed = std::mem::take(&mut o_intake.catalog_changed);
                    if intake_changed
                        || tab_admin
                            .as_mut()
                            .is_some_and(|a| std::mem::take(&mut a.changed))
                    {
                        tab_browse.reload(env.conn);
                    }
                    if let Some(m) = o_intake.message.take() {
                        tab_browse.notice = Some(m);
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
        if let Screen::Open(o) = &mut self.screen {
            let o = &mut **o;
            o.intake.ingest.tick(
                &ctx,
                &o.opened.root,
                &self.config.device_id,
                &self.config.ingest,
            );
            if let Some(p) = o.intake.ingest.open_request.take()
                && let Err(e) = (o.browse.shell)(&crate::shell::ShellAction::Open(p))
            {
                o.intake.ingest.warning = Some(format!("The sheet did not open: {e}"));
            }
            if let Some(ingest) = o.intake.ingest.changed.take() {
                self.config.ingest = ingest;
                if let Err(e) = config::save(&self.data, &self.config) {
                    o.intake.ingest.warning = Some(format!("Settings could not be saved: {e:#}"));
                }
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

    /// P02 step 1 end to end: a batch Filer commits while Intake is open is recorded in the
    /// catalog and shows in Browse; Filer's Undo takes it out again. Filer's own commit and undo
    /// run here directly (what its Confirm and Undo buttons call; its Preview screen is covered by
    /// filer-intake's own tests).
    #[test]
    fn intake_batches_reach_the_catalog_and_undo() {
        use std::sync::atomic::AtomicUsize;

        use filer_intake::commit::{CommitInput, CommitItem, run_commit};
        use filer_intake::plan::FolderAction;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Clients");
        let inbox = tmp.path().join("Scans");
        std::fs::create_dir_all(&inbox).unwrap();
        let req = CreateRequest {
            root: &root,
            name: "Firm",
            passphrase: "correct horse battery",
            confirm: "correct horse battery",
        };
        drop(session::create(&tmp.path().join("pc1"), "dev-1", &req, FAST).unwrap());
        let mut h = harness(&tmp.path().join("pc1b"));
        click(&mut h, "Open existing");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
        type_into(&mut h, "Recovery passphrase", "correct horse battery");
        click(&mut h, "Open cabinet");
        settle(&mut h);
        click(&mut h, "Intake");
        h.step();

        // Filer files one scan for a new client.
        let client = filer_core::clients::Client {
            id: "1042".into(),
            uuid: uuid::Uuid::new_v4(),
            name: "Anderson Farms LLC".into(),
            extra: Default::default(),
        };
        let folder = root.join("Anderson Farms LLC (1042)");
        let src = inbox.join("w2.pdf");
        std::fs::write(&src, "W-2 2025").unwrap();
        let cab = vf_fs::cabinet::read(&root).unwrap();
        let input = CommitInput {
            batch: uuid::Uuid::new_v4(),
            items: vec![CommitItem {
                src,
                dest: folder.join("Income Tax/2025/w2.pdf"),
                folder: FolderAction::WillCreate(folder.clone()),
                client,
                overwrite: false,
                source: filer_intake::intake::Source::Inbox(inbox.clone()),
                merged_from: vec![],
                split_of: None,
                copies: vec![],
                kept_both: false,
                also_dirs: vec![],
            }],
            skipped: vec![],
            root: root.clone(),
            first_inbox: Some(inbox.clone()),
            data_dir: vf_fs::clients::master_dir(&root),
            firm_id: uuid::Uuid::parse_str(&cab.firm_id).unwrap(),
            template: filer_core::template::default_tree(),
            splits: vec![],
            force_copy: false,
        };
        let rep = run_commit(&input, &AtomicUsize::new(0));
        assert!(rep.warnings.is_empty(), "{:?}", rep.warnings);
        h.step();
        click(&mut h, "Browse");
        h.step();
        let docs = |h: &egui_kittest::Harness<'static, VibeFilesApp>| match &h.state().screen {
            Screen::Open(o) => (o.browse.snap.docs.len(), o.browse.notice.clone()),
            _ => unreachable!(),
        };
        let (n, notice) = docs(&h);
        assert_eq!(n, 1, "the filed W-2 is in the catalog");
        assert_eq!(notice.as_deref(), Some("Intake filed 1 new documents."));

        // Filer's Undo; the catalog follows when Intake next looks.
        filer_intake::undo::run_undo(&vf_fs::clients::master_dir(&root), rep.last_batch, false);
        click(&mut h, "Intake");
        h.step();
        click(&mut h, "Browse");
        h.step();
        let (n, notice) = docs(&h);
        assert_eq!(n, 0, "undone batch tombstoned");
        assert_eq!(notice.as_deref(), Some("Intake's last batch was undone."));
        assert!(inbox.join("w2.pdf").exists());
    }

    /// P02 step 1: the Intake tab mounts Filer Desktop on the open cabinet: its data folder is
    /// `<cabinet>/_Vibe/` (Q-19), and its settings carry this cabinet's root and firm id.
    #[test]
    fn intake_tab_mounts_filer_on_the_cabinet() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Clients");
        let req = CreateRequest {
            root: &root,
            name: "Firm",
            passphrase: "correct horse battery",
            confirm: "correct horse battery",
        };
        drop(session::create(&tmp.path().join("pc1"), "dev-1", &req, FAST).unwrap());
        let mut h = harness(&tmp.path().join("pc1b"));
        click(&mut h, "Open existing");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
        type_into(&mut h, "Recovery passphrase", "correct horse battery");
        click(&mut h, "Open cabinet");
        settle(&mut h);

        click(&mut h, "Intake");
        for _ in 0..10 {
            h.step();
        }
        // Filer's own tabs are there (it opens on Preview once a root is set).
        for t in ["Preview", "Setup", "Rules", "Relationships"] {
            assert!(
                h.query_by_label(t).is_some(),
                "Filer tab {t} mounted in Intake"
            );
        }
        let (s, savable) =
            filer_intake::config::Settings::load_or_default(&vf_fs::clients::master_dir(&root));
        assert!(savable);
        assert_eq!(s.root.as_deref(), Some(root.as_path()));
        let cab = vf_fs::cabinet::read(&root).unwrap();
        assert_eq!(s.firm_id.map(|u| u.to_string()), Some(cab.firm_id));
        assert!(root.join("_Vibe/config.json").is_file());
        // Back to Browse: Vibe's own tabs still work after Filer's theme and panels.
        click(&mut h, "Browse");
        h.step();
        assert_eq!(h.state().tab, Tab::Browse);
    }

    /// P02 steps 3 and 6: a file dropped in this PC's Scans folder is processed in the
    /// background and handed to Filer's inbox; the Intake tab counts it, silently.
    #[test]
    fn scans_folder_feeds_filers_inbox_and_the_badge_counts() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Clients");
        let (scans, inbox) = (tmp.path().join("Scans"), tmp.path().join("Inbox"));
        std::fs::create_dir_all(&scans).unwrap();
        std::fs::create_dir_all(&inbox).unwrap();
        let req = CreateRequest {
            root: &root,
            name: "Firm",
            passphrase: "correct horse battery",
            confirm: "correct horse battery",
        };
        drop(session::create(&tmp.path().join("pc1"), "dev-1", &req, FAST).unwrap());
        let master = vf_fs::clients::master_dir(&root);
        let (mut fs, _) = filer_intake::config::Settings::load_or_default(&master);
        fs.inboxes = vec![inbox.clone()];
        fs.save(&master).unwrap();

        let data = tmp.path().join("pc1b");
        let mut h = harness(&data);
        h.state_mut().config.ingest.scans = Some(scans.clone());
        click(&mut h, "Open existing");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
        type_into(&mut h, "Recovery passphrase", "correct horse battery");
        click(&mut h, "Open cabinet");
        settle(&mut h);

        filer_pdf::pdf::write_test_pdf(&scans.join("Acme_1001_2025 W-2.pdf"), &["W-2 2025"]);
        let start = std::time::Instant::now();
        let delivered = inbox.join("Acme_1001_2025 W-2.pdf");
        // Counted while it waits in Scans, and again once it is in Filer's inbox.
        while !(delivered.is_file() && h.query_by_label("Intake (1)").is_some()) {
            assert!(
                start.elapsed() < std::time::Duration::from_secs(30),
                "the badge never showed the delivered file"
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
            h.step();
        }
        assert!(
            !scans.join("Acme_1001_2025 W-2.pdf").exists(),
            "original kept aside"
        );
        assert!(scans.join(vf_ingest::pipeline::PROCESSED_DIR).is_dir());
        let Screen::Open(o) = &h.state().screen else {
            unreachable!()
        };
        assert_eq!(o.intake.ingest.delivered, 1);
        assert!(o.intake.ingest.log[0].starts_with("Acme_1001_2025 W-2.pdf →"));
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
