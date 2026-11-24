//! Top-level app: tab routing over the shared chrome. Each tab is a stub until its phase.

use std::path::{Path, PathBuf};
use std::sync::mpsc;

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
    Viewer,
    Sync,
    Admin,
}

impl Tab {
    /// CLAUDE.md conventions: tabs in exactly this order. (No Search tab since Q-113, owner
    /// 2026-11-15: documents are found from Browse.)
    pub const ALL: [Tab; 5] = [Tab::Browse, Tab::Intake, Tab::Viewer, Tab::Sync, Tab::Admin];

    pub fn name(self) -> &'static str {
        match self {
            Tab::Browse => "Browse",
            Tab::Intake => "Intake",

            Tab::Viewer => "Viewer",
            Tab::Sync => "Sync",
            Tab::Admin => "Admin",
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Tab::Browse => "Browse shows the client tree once a cabinet is open.",
            Tab::Intake => "Intake files documents into the open cabinet.",
            Tab::Viewer => "Open a document from Browse.",
            Tab::Sync => "Two-way sync with B2.",
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
        rx: mpsc::Receiver<Result<(Opened, Prepared), String>>,
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
    pub viewer: crate::ui::viewer::Viewer,
    /// The audit chain check: on open, then weekly (P05 step 2).
    pub chain: crate::ui::audit::ChainCheck,
    /// When new audit rows were last copied to `_Vibe/audit/<month>.jsonl`.
    mirrored_at: Option<std::time::Instant>,
    /// The copy being written, on its own thread.
    /// (An error with whether the catalog was only busy.)
    mirror_job: Option<crate::ui::job::Job<Result<usize, (String, bool)>>>,
    /// Copies in a row that found the catalog busy: said once there are many.
    mirror_busy: u32,
    /// A shared catalog (Q-67): its `data_version` when last seen, and when it was looked at;
    /// another session's change shows "Changes by others: Refresh" in Browse (Q-102).
    data_version: (i64, Option<std::time::Instant>),
    /// Browse's count of reloads when `data_version` was last taken as seen.
    seen_reloads: u64,
    /// Until when a change to the catalog is taken for this session's own: its background
    /// work (B2 sync, the sweep, a batch being recorded, the audit copy) writes on
    /// connections of its own, which `data_version` cannot tell from another session's.
    own_writes_until: Option<std::time::Instant>,
    /// Wait before the next mirror write: 2 s, doubling to 2 min after failures, so a share
    /// that went away is not waited on every 2 s (QA A7).
    mirror_wait: std::time::Duration,
    /// The integrity sweep (P05 step 5): weekly, and from Admin.
    pub sweep: crate::ui::sweep::SweepState,
    /// B2 sync (P11).
    pub sync: crate::ui::sync::SyncTab,
    /// Several servers (PA-36): the other servers' changes, read now and then.
    pub servers: crate::server::Watch,
    /// The admin password was given in this session (PA-24): Admin and Sync settings open
    /// until Vibe Files closes or Lock Admin.
    pub admin_unlocked: bool,
    /// The lock screen's field and message.
    admin_typed: String,
    admin_msg: Option<String>,
    /// Startup self-checks (P13 step 4), while they run.
    checks: Option<mpsc::Receiver<Vec<crate::selfcheck::Check>>>,
    /// The cabinet's first index on this PC, running behind the window (PA-60), and when
    /// Browse last read the catalog for it.
    indexing: Option<(crate::indexer::Indexer, std::time::Instant)>,
    /// The client folder shown that the index has been asked to take next.
    index_wait: Option<String>,
    /// This login's id in the cabinet, held while the cabinet is open (PA-69).
    _login: Option<vf_fs::lock::Held>,
    /// Rows an earlier build made for clients' notes files being taken out of the catalog
    /// (PA-76), behind the window: it reads every document's path.
    note_rows: Option<crate::ui::job::Job<Result<usize, String>>>,
    /// The rest of opening ([`ready`]), behind the window (PA-82).
    ready: Option<crate::ui::job::Job<Result<Readied, String>>>,
}

/// Whether a document shared through the portal goes to B2 first (Q-103), as B2 is now.
fn b2_first(
    sync: &crate::ui::sync::SyncTab,
    catalog: &std::path::Path,
    root: &std::path::Path,
    cabinet_id: &str,
    config: &Config,
) -> crate::ui::portal::B2First {
    match &sync.session {
        Some(s) => crate::ui::portal::B2First::Ready {
            sync: Box::new(s.clone()),
            catalog: catalog.to_path_buf(),
            root: root.to_path_buf(),
        },
        None if config.sync.configured_for(cabinet_id) => crate::ui::portal::B2First::Locked,
        None => crate::ui::portal::B2First::NotSetUp,
    }
}

pub struct VibeFilesApp {
    pub tab: Tab,
    pub data: PathBuf,
    pub config: Config,
    pub screen: Screen,
    /// The update check (P13 step 6), while it runs.
    update: Option<mpsc::Receiver<Option<vf_sync::update::Release>>>,
    /// A newer release, shown on Browse once.
    update_notice: Option<String>,
    /// This PC's text size has been applied to the window.
    zoom_applied: bool,
    /// The footer's access requests were clicked before Admin was opened: it opens on Portal
    /// requests (review 2026-11-24).
    open_requests: bool,
    /// The Help window (PA-25).
    pub help: crate::ui::help::HelpState,
    /// The window is closing after the Viewer's question was answered (not asked again).
    closing: bool,
    /// "Tip VibeMyFirm" was pressed (tests).
    tip_clicked: bool,
    /// The portal staff page the header's Portal button opened (tests).
    portal_opened: Option<String>,
    /// This login's own inbox as the firm's `firm.toml` gave it at this start (Q-112): the
    /// folder is made when a cabinet opens, on the opening thread.
    firm_inbox: Option<PathBuf>,
}

/// The line under the opening screen's spinner: the step, and how long it has taken once
/// that is worth saying.
pub(crate) fn opening_line(step: &str, since: std::time::Duration) -> String {
    match since.as_secs() {
        0..=2 => format!("{step}…"),
        s @ 3..=119 => format!("{step}… ({s} s)"),
        s => format!("{step}… ({} min)", s / 60),
    }
}

/// What the footer says is going on behind the window: the index of the cabinet's files
/// (`listing`: client folders done, of how many, documents so far), else the other servers'
/// changes being read.
pub(crate) fn behind_the_window(
    listing: Option<(usize, usize, usize)>,
    others: bool,
    copy: bool,
) -> Option<String> {
    match listing {
        Some((done, all, docs)) => Some(format!(
            "Indexing: {done} of {all} client folders, {docs} documents"
        )),
        None if others => Some("Reading changes made on other servers".into()),
        // After an index: said until it is made, so the program is not closed on it.
        None if copy => Some(
            "Leaving a copy of the index for the other computers: keep Vibe Files open until this line goes"
                .into(),
        ),
        None => None,
    }
}

/// Asks GitHub for the latest release, once per start, on a thread (never in tests: no network).
fn check_for_update(enabled: bool) -> Option<mpsc::Receiver<Option<vf_sync::update::Release>>> {
    if !enabled || cfg!(test) {
        return None;
    }
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("update-check".into())
        .spawn(move || {
            let r = match vf_sync::update::check(env!("CARGO_PKG_VERSION")) {
                Ok(r) => r,
                Err(e) => {
                    tracing::info!(kind = crate::logging::kind(&e), "update check failed");
                    None
                }
            };
            let _ = tx.send(r);
        })
        .ok();
    Some(rx)
}

impl VibeFilesApp {
    pub fn new(ctx: &egui::Context, data: PathBuf) -> Self {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(Path::to_path_buf));
        Self::new_in(ctx, data, exe_dir.as_deref())
    }

    /// [`Self::new`] with the folder `firm.toml` is read from (beside the exe).
    pub fn new_in(ctx: &egui::Context, data: PathBuf, firm_dir: Option<&Path>) -> Self {
        let (config, error) = match config::load(&data) {
            Ok(c) => (c, None),
            // Never saved over: the unreadable file is kept aside, the device id carried over.
            Err(e) => match config::set_aside_bad(&data) {
                Ok((c, aside)) => (
                    c,
                    Some(format!(
                        "Settings could not be read ({e:#}). They were kept as {} and fresh settings made; this PC keeps its identity.",
                        aside.display()
                    )),
                ),
                Err(e2) => (
                    Config {
                        device_id: vf_core::new_id(),
                        last_cabinet: None,
                        prefs: Default::default(),
                        ingest: Default::default(),
                        ocr: Default::default(),
                        viewer: Default::default(),
                        look: Default::default(),
                        sync: Default::default(),
                        update_check: true,
                    },
                    Some(format!("Settings could not be read: {e:#} ({e2:#})")),
                ),
            },
        };
        // This PC's theme from the first paint (no flash of the light one).
        theme::set_dark(config.look.dark);
        theme::install(ctx);
        // A login's first start: the firm's cabinet (firm.toml beside the exe, owner
        // 2026-11-12), when the admin named one; its own last cabinet after that.
        let firm = firm_dir.map(config::firm).unwrap_or_default();
        let start = config.last_cabinet.clone().or_else(|| firm.cabinet());
        // What this login has not set comes from the firm (Q-112): its own inbox folder, the
        // Scans and print capture folders. The inbox folder is made when the cabinet opens.
        let mut config = config;
        let before = config.ingest.clone();
        firm.apply(
            &mut config,
            &vf_core::windows_login().unwrap_or_default(),
            &config::windows_user(),
        );
        let firm_inbox = (config.ingest.inboxes_by_user != before.inboxes_by_user)
            .then(|| firm.my_inbox_for(&config::windows_user()))
            .flatten();
        let mut w = Wizard::new(start.as_deref());
        w.error = error;
        let update = check_for_update(config.update_check && firm.update_check());
        let mut app = Self {
            tab: Tab::Browse,
            data,
            config,
            screen: Screen::Wizard(w),
            update,
            update_notice: None,
            zoom_applied: false,
            open_requests: false,
            help: Default::default(),
            closing: false,
            tip_clicked: false,
            portal_opened: None,
            firm_inbox,
        };
        // Tests drive the wizard themselves; `open_last` has its own test.
        if !cfg!(test) {
            app.open_last(ctx);
        }
        app
    }

    /// Opens the cabinet this PC opened last, without the wizard (owner 2026-11-04). When it
    /// cannot be opened (share down, folder moved) the wizard shows with the reason; while it
    /// opens, "Open a different cabinet" goes to the wizard.
    pub fn open_last(&mut self, ctx: &egui::Context) {
        let Screen::Wizard(w) = &self.screen else {
            return;
        };
        // The last cabinet, or the firm's (the only way the wizard starts with a path).
        if !w.root.trim().is_empty() && w.error.is_none() {
            self.start(Mode::Open, ctx);
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
        let (data, device) = (self.data.clone(), self.config.device_id.clone());
        let name = w.name.clone();
        let ctx = ctx.clone();
        let firm_inbox = self.firm_inbox.clone();
        let staging: Vec<PathBuf> = [
            self.config.ingest.scans.clone(),
            self.config.ingest.print_capture.clone(),
        ]
        .into_iter()
        .flatten()
        .collect();
        std::thread::spawn(move || {
            // The login's own inbox the firm named (Q-112): made and checked here, off the
            // window's thread (it is usually on a share), by the rules of one added by hand.
            let inbox_problem =
                firm_inbox
                    .as_ref()
                    .filter(|_| mode == Mode::Open)
                    .and_then(|dir| {
                        std::fs::create_dir_all(dir)
                            .map_err(|e| format!("{} could not be made: {e}", dir.display()))
                            .and_then(|()| {
                                crate::ui::ingest::check_firm_inbox(dir, &root, &staging)
                            })
                            .err()
                    });
            // A session host is wiped overnight and its settings with it (PA-69): the login
            // works under the id the cabinet remembers for it, so its Intake folder and what
            // it did yesterday are still its own.
            session::step("Signing this login in to the cabinet");
            let (device, login) = if mode == Mode::Open && crate::server::server_mode() {
                vf_fs::identity::login_session(
                    &root,
                    &vf_core::windows_login().unwrap_or_default(),
                    &device,
                )
            } else {
                (device, None)
            };
            let r = match mode {
                Mode::Create => session::create(
                    &data,
                    &device,
                    &CreateRequest {
                        root: &root,
                        name: &name,
                    },
                ),
                Mode::Open => session::open_staged(&data, &device, &root),
            };
            // The rest of opening (print capture rules, the client list, legal holds, the
            // trash: it reads and writes the share) runs behind the window once the cabinet
            // shows ([`ready`], PA-82): nobody waits for it at "Opening the cabinet…".
            let r = r.map(|o| {
                let p = Prepared {
                    device,
                    login,
                    inbox_problem,
                };
                (o, p)
            });
            session::steps_done();
            let _ = tx.send(r.map_err(|e| format!("{e:#}")));
            ctx.request_repaint();
        });
        let mut back = w.clone();
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

    /// Picks up a finished create/open (and [`prepare`]).
    fn poll(&mut self, ctx: &egui::Context) {
        let Screen::Busy { rx, back, .. } = &mut self.screen else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("The work stopped unexpectedly.".into()),
        };
        match result {
            Ok((
                mut opened,
                Prepared {
                    device,
                    login,
                    inbox_problem,
                },
            )) => {
                // Saved with the settings below.
                self.config.device_id = device;
                // The firm's inbox for this login cannot be used: not kept, and said below.
                if inbox_problem.is_some() {
                    self.config
                        .ingest
                        .set_my_inboxes(&vf_core::windows_login().unwrap_or_default(), vec![]);
                    self.firm_inbox = None;
                }
                let caught_up = opened.caught_up;
                // The catalog has the clients only (PA-60): the documents are cataloged
                // behind the window, the client being looked at first.
                let indexing = opened.staged.take().and_then(|staged| {
                    if cfg!(test) {
                        // Tests: here and now, so the cabinet is whole on the first frame.
                        let shared = crate::indexer::Shared::new(&staged);
                        crate::indexer::run(
                            &mut opened.conn,
                            &opened.root.clone(),
                            &self.config.device_id,
                            &staged,
                            &shared,
                            &|| {},
                        );
                        None
                    } else {
                        Some((
                            crate::indexer::Indexer::start(
                                ctx,
                                &opened.catalog,
                                &opened.root,
                                &self.config.device_id,
                                staged,
                            ),
                            std::time::Instant::now(),
                        ))
                    }
                });
                // A crash between Filer's commit and the catalog write is caught up here.
                let mut intake = IntakeTab::default();
                let root = opened.root.clone();
                let catalog = opened.catalog.clone();
                if indexing.is_none() {
                    intake.reconcile(&mut opened.conn, &catalog, &root, &self.config.device_id);
                }
                self.config.last_cabinet = Some(opened.root.clone());
                // The cabinet is open either way; a settings write failure only loses "last".
                let _ = config::save(&self.data, &self.config);
                let mut browse = Browse::load(&opened.conn);
                browse.set_root(&opened.root);
                browse.catalog = Some(opened.catalog.clone());
                browse.indexing = indexing.as_ref().map(|(ix, _)| ix.shared.clone());
                browse.sheets_dir = self.data.join("sheets");
                browse.set_prefs(self.config.prefs.clone().for_user(&config::windows_user()));
                browse.saved = vf_index::saved::load(&opened.root);
                if let Some(e) = inbox_problem {
                    browse.error = Some(format!(
                        "Your own inbox from firm.toml (my_inbox) is not used: {e}"
                    ));
                }
                if let Some(e) = &opened.problem {
                    browse.error = Some(e.clone());
                }
                browse.notice = opened.rebuild.as_ref().map(|r| match &opened.set_aside {
                    Some(old) => format!(
                        "The catalog is no longer encrypted: the old one was kept as {} and the cabinet indexed again. {r}",
                        old.display()
                    ),
                    None => format!("Indexed: {r}"),
                });
                let chain = crate::ui::audit::ChainCheck::later();
                // The rest of opening, behind the window (PA-82), on its own connection.
                let folders_behind = opened.folders_behind;
                let ready = {
                    let (catalog, root, device) = (
                        opened.catalog.clone(),
                        opened.root.clone(),
                        self.config.device_id.clone(),
                    );
                    crate::ui::job::Job::start(ctx, "Getting the cabinet ready…", move || {
                        // Tried again when the catalog is busy (the index writes meanwhile):
                        // not "the housekeeping did not run: database is locked" in red.
                        let mut conn = patient(|| {
                            vf_core::db::open(&catalog, vf_core::db::Storage::Local)
                                .map_err(|e| e.to_string())
                        })?;
                        Ok(ready(&mut conn, &root, &device, folders_behind))
                    })
                    .ok()
                };
                let checks = {
                    let (tx, rx) = mpsc::channel();
                    let (catalog, root, ctx) =
                        (opened.catalog.clone(), opened.root.clone(), ctx.clone());
                    let synced = self.config.sync.configured_for(&opened.cabinet.cabinet_id);
                    let run = move || {
                        vf_core::db::open(&catalog, vf_core::db::Storage::Local)
                            .map(|c| crate::selfcheck::run(&c, &root, synced))
                            .unwrap_or_else(|e| {
                                // Busy: nothing was checked and nothing is wrong. Else it
                                // is the catalog that could not be opened, said as that
                                // (not as a failed integrity check).
                                if e.is_busy() {
                                    return vec![];
                                }
                                vec![crate::selfcheck::Check {
                                    name: "Catalog",
                                    ok: false,
                                    detail: format!(
                                        "could not be opened for the startup check: {e}"
                                    ),
                                }]
                            })
                    };
                    if cfg!(test) {
                        // Tests: at once (a result arriving between frames would repaint at
                        // a random moment).
                        let _ = tx.send(run());
                    } else {
                        std::thread::Builder::new()
                            .name("self-check".into())
                            .spawn(move || {
                                let _ = tx.send(run());
                                ctx.request_repaint();
                            })
                            .ok();
                    }
                    Some(rx)
                };
                crate::cloudmode::check_conflicts(ctx, &opened.root);
                let mut sweep = crate::ui::sweep::SweepState::default();
                // The weekly sweep reads files: on the main computer only (PA-60), and by the
                // cabinet's own note of the last one, so a host reset each night does not
                // start it every morning.
                // Not while the index runs behind the window: started when it is done.
                if indexing.is_none()
                    && vf_fs::sweep::due_for(&opened.conn, &opened.root)
                    && crate::server::runs_here(&opened.root)
                {
                    sweep.start(ctx, &opened.catalog, &opened.root);
                }
                let note_rows = {
                    let (catalog, device) = (opened.catalog.clone(), self.config.device_id.clone());
                    crate::ui::job::Job::start(ctx, "Tidying the catalog…", move || {
                        let mut conn = vf_core::db::open(&catalog, vf_core::db::Storage::Local)
                            .map_err(|e| e.to_string())?;
                        vf_fs::fcinfo::forget_rows(&mut conn, &device)
                    })
                    .ok()
                };
                self.screen = Screen::Open(Box::new(OpenCabinet {
                    note_rows,
                    ready,
                    chain,
                    mirrored_at: None,
                    mirror_job: None,
                    mirror_busy: 0,
                    data_version: (0, None),
                    seen_reloads: 0,
                    own_writes_until: None,
                    mirror_wait: std::time::Duration::from_secs(2),
                    sweep,
                    sync: Default::default(),
                    servers: if caught_up {
                        Default::default()
                    } else {
                        crate::server::Watch::due_now()
                    },
                    checks,
                    opened,
                    browse,
                    admin: None,
                    intake,

                    viewer: Default::default(),

                    admin_unlocked: false,
                    admin_typed: String::new(),
                    admin_msg: None,
                    indexing,
                    index_wait: None,
                    _login: login,
                }));
            }
            Err(e) => {
                let mut w = back.clone();
                w.error = Some(e);
                self.screen = Screen::Wizard(w);
            }
        }
    }

    /// Closing the window with annotations unsaved in the Viewer asks first (owner
    /// 2026-11-04): the Viewer is shown with Save and quit / Discard and quit / Keep editing.
    fn guard_close(&mut self, ctx: &egui::Context) {
        let Screen::Open(o) = &mut self.screen else {
            return;
        };
        if std::mem::take(&mut o.viewer.quit_ok) {
            self.closing = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if !self.closing && ctx.input(|i| i.viewport().close_requested()) && o.viewer.unsaved() > 0
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.tab = Tab::Viewer;
            o.viewer.quit_asked = true;
        }
    }

    pub fn cabinet_name(&self) -> Option<String> {
        match &self.screen {
            Screen::Open(o) => Some(o.opened.cabinet.display_name(&o.opened.root)),
            _ => None,
        }
    }
}

/// The cabinet closes (another is opened, or Vibe Files exits): the audit rows of the last
/// seconds go to the cabinet's copy now. On a session host the catalog is gone by morning.
impl Drop for OpenCabinet {
    fn drop(&mut self) {
        // The whole cabinet's sync goes to another server at once, not after the lease lapses
        // (PA-99).
        crate::lease::release();
        let dir = self.opened.root.join("_Vibe").join("audit");
        if vf_core::audit::mirror_due(&self.opened.conn) {
            let _ = vf_core::audit::mirror_pending(&self.opened.conn, &dir);
        }
    }
}

impl eframe::App for VibeFilesApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.poll(ui.ctx());
        self.guard_close(ui.ctx());
        let name = self.cabinet_name();
        // Text size: the header's buttons, or Ctrl + / Ctrl - (egui's own zoom keys); kept in
        // this PC's settings (owner 2026-11-04).
        let ctx_zoom = (ui.ctx().zoom_factor() * 100.0).round() as u16;
        let mut size = self.config.look.text_size;
        if ctx_zoom != size && self.zoom_applied {
            size = ctx_zoom;
        }
        // The theme: Vibe's Light / Dark button sets it; Filer's theme button on Intake may set
        // it too, including "follow Windows", which is then kept (review 2026-11-04).
        let ctx_dark = ui.ctx().theme() == egui::Theme::Dark;
        let follows_windows =
            ui.ctx().options(|o| o.theme_preference) == egui::ThemePreference::System;
        let (mut dark, mut system) = (self.config.look.dark, self.config.look.system);
        if self.zoom_applied {
            if follows_windows {
                system = true;
                dark = ctx_dark;
            } else if ctx_dark != dark || system {
                system = false;
                dark = ctx_dark;
            }
        }
        crate::ui::theme::set_dark(dark);
        let shown = dark;
        let mut help = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F1));
        let mut tip = false;
        // The portal's staff sign-in page, when the open cabinet has a portal set up.
        let staff_page = match &mut self.screen {
            Screen::Open(o) => o
                .browse
                .portal
                .settings(&o.opened.root)
                .map(|s| format!("{}/staff", s.url.trim_end_matches('/')))
                // Only a web page the browser is opened on (a test portal on http is not).
                .filter(|u| u.starts_with("https://")),
            _ => None,
        };
        let mut open_portal = false;
        chrome::header(
            ui,
            name.as_deref(),
            &mut size,
            &mut dark,
            &mut help,
            &mut tip,
            staff_page.is_some(),
            &mut open_portal,
        );
        if open_portal && let Some(url) = &staff_page {
            self.portal_opened = Some(url.clone());
            // Tests never start a browser.
            if !cfg!(test)
                && let Err(e) = crate::shell::open_url(url)
            {
                tracing::info!(error = %e, "portal staff page not opened");
            }
        }
        if tip {
            self.tip_clicked = true;
            // Tests never start a browser.
            if !cfg!(test)
                && let Err(e) = crate::shell::open_url(crate::shell::TIP_URL)
            {
                tracing::info!(error = %e, "tip page not opened");
            }
        }
        if help {
            let tab = matches!(self.screen, Screen::Open(_)).then_some(self.tab);
            self.help.open_at(crate::help::for_tab(tab));
        }
        crate::ui::help::show(ui.ctx(), &mut self.help);
        if dark != shown {
            system = false; // the header's button chooses explicitly
        }
        let look_changed = size != self.config.look.text_size
            || dark != self.config.look.dark
            || system != self.config.look.system;
        if look_changed || !self.zoom_applied {
            let first = !self.zoom_applied;
            self.zoom_applied = true;
            ui.ctx().set_zoom_factor(f32::from(size) / 100.0);
            if system {
                if first {
                    ui.ctx().set_theme(egui::ThemePreference::System);
                }
                crate::ui::theme::set_dark(ui.ctx().theme() == egui::Theme::Dark);
            } else {
                crate::ui::theme::set_theme(ui.ctx(), dark);
            }
            if look_changed {
                self.config.look.text_size = size;
                self.config.look.dark = dark;
                self.config.look.system = system;
                let _ = config::save(&self.data, &self.config);
            }
        }
        let badge = match &self.screen {
            Screen::Open(o) => o.intake.ingest.badge(),
            _ => 0,
        };
        chrome::tab_bar(ui, &mut self.tab, badge);
        if let Screen::Open(o) = &mut self.screen {
            o.chain.tick(ui.ctx(), &o.opened.catalog);
            if let Some(line) =
                o.sweep
                    .poll(&mut o.opened.conn, &o.opened.root, &self.config.device_id)
            {
                o.browse.notice = Some(line);
            }
            if std::mem::take(&mut o.sweep.changed) {
                o.browse.reload(&o.opened.conn);
            }
            match crate::ui::job::take(&mut o.note_rows) {
                Some(Ok(Ok(n))) if n > 0 => o.browse.reload(&o.opened.conn),
                Some(Ok(Err(e))) | Some(Err(e)) => {
                    tracing::warn!(kind = crate::logging::kind(&e), "notes files' rows");
                }
                _ => {}
            }
            match crate::ui::job::take(&mut o.ready) {
                Some(Ok(Ok(r))) => {
                    if let Some(e) = r.rules_problem {
                        add_error(
                            &mut o.browse.error,
                            format!("Print capture (Admin › Print capture & parsers): {e}"),
                        );
                    }
                    match r.emptied {
                        Ok(0) => {}
                        Ok(n) => {
                            tracing::info!(documents = n, "old trash emptied");
                            o.browse.notice = Some(format!(
                                "Emptied {n} documents that were in the trash over {} days.",
                                vf_fs::trash::AUTO_EMPTY_DAYS
                            ));
                        }
                        Err(e) => {
                            tracing::warn!(
                                kind = crate::logging::kind(&e),
                                "emptying old trash failed"
                            );
                            add_error(&mut o.browse.error, format!("Emptying old trash: {e}"));
                        }
                    }
                    // What it changed in the catalog (the undo list, held and emptied
                    // documents) shows.
                    if r.changed {
                        o.browse.reload(&o.opened.conn);
                    }
                }
                Some(Ok(Err(e))) | Some(Err(e)) => {
                    tracing::warn!(kind = crate::logging::kind(&e), "cabinet not made ready");
                    add_error(
                        &mut o.browse.error,
                        format!("The cabinet's housekeeping at opening did not run: {e}"),
                    );
                }
                None => {}
            }
            // (And for the other servers' changes left for behind the window, Q-112.)
            o.sweep.hold = o.indexing.is_some() || o.servers.first_pending();
            // The index behind the window (PA-60): the client being looked at goes next, and
            // Browse reads the catalog when that client is there, now and then meanwhile, and
            // when the index is done.
            if let Some((ix, read_at)) = &mut o.indexing {
                let sh = ix.shared.clone();
                let shown = o.browse.shown_folder();
                let mut reload = false;
                if let Some(f) = &o.index_wait
                    && sh.listed(f)
                {
                    o.index_wait = None;
                    reload = true;
                }
                if let Some(f) = shown.filter(|f| !sh.listed(f)) {
                    sh.ask(&f);
                    o.index_wait = Some(f);
                }
                if let Some(e) = sh.take_error() {
                    tracing::warn!(kind = crate::logging::kind(&e), "listing the cabinet");
                }
                let (done, all, docs) = sh.progress();
                if sh.finished() {
                    o.browse.index_line = None;
                    o.browse.indexing = None;
                    o.index_wait = None;
                    o.indexing = None;
                    // The other servers start from this catalog's copy: made again now, and
                    // noted in the catalog until it is (the program may be closed first).
                    crate::server::owe_seed(&o.opened.conn);
                    o.servers.seed_after_index();
                    o.browse.reload(&o.opened.conn);
                    if o.browse.notice.is_none() {
                        o.browse.notice = Some(format!(
                            "The cabinet's files are listed: {docs} documents in {all} folders."
                        ));
                    }
                    if vf_fs::sweep::due_for(&o.opened.conn, &o.opened.root)
                        && crate::server::runs_here(&o.opened.root)
                    {
                        o.sweep.start(ui.ctx(), &o.opened.catalog, &o.opened.root);
                    }
                    // What Intake filed meanwhile is recorded now.
                    o.intake.reconcile(
                        &mut o.opened.conn,
                        &o.opened.catalog,
                        &o.opened.root,
                        &self.config.device_id,
                    );
                } else if sh.waiting() {
                    o.browse.index_line = Some(format!(
                        "Listing the cabinet's files is waiting and will try again: the cabinet, or this computer's catalog, could not be reached just now ({done} of {all} client folders listed)."
                    ));
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_secs(1));
                } else {
                    o.browse.index_line = Some(match &o.index_wait {
                        Some(_) => format!(
                            "Listing this client's files… ({done} of {all} client folders listed so far)"
                        ),
                        None => format!(
                            "Listing the cabinet's files: {done} of {all} client folders, {docs} documents so far. Clients open as usual; search and counts are complete when this is done."
                        ),
                    });
                    if reload || read_at.elapsed() >= std::time::Duration::from_secs(20) {
                        *read_at = std::time::Instant::now();
                        o.browse.reload(&o.opened.conn);
                    }
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(300));
                }
            }
            if o.sync.running()
                || o.sweep.running()
                || o.mirror_job.is_some()
                || o.intake.recording()
                || o.servers.busy()
            {
                o.own_writes_until =
                    Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
            }
            // Another session on this PC changed the shared catalog: said in Browse (looked
            // at every 3 s at most).
            if o.opened.shared
                && o.data_version
                    .1
                    .is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(3))
            {
                o.data_version.1 = Some(std::time::Instant::now());
                if let Ok(v) = o
                    .opened
                    .conn
                    .query_row("PRAGMA data_version", [], |r| r.get::<_, i64>(0))
                {
                    // (The index behind the window changes it all the time: read above.)
                    // A change since Browse last read the catalog is someone else's (this
                    // session's own work ends with a reload): said, not reloaded under the
                    // person's hands (owner 2026-11-13, Q-102).
                    if o.browse.reloads != o.seen_reloads {
                        o.seen_reloads = o.browse.reloads;
                    } else if o.data_version.0 != 0
                        && v != o.data_version.0
                        && o.indexing.is_none()
                        && o.own_writes_until
                            .is_none_or(|t| std::time::Instant::now() > t)
                    {
                        o.browse.others_changed = true;
                    }
                    o.data_version.0 = v;
                }
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_secs(3));
            }
            // Intake's reconcile runs on its own thread; its result is taken on any tab.
            if o.intake.poll_reconcile() {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(200));
            }
            // The firm's Vibe Portal (PA-37): client uploads fetched into the Portal inbox on a
            // timer, on whatever tab is open.
            if let Some(a) = &mut o.admin {
                if a.portal.audit.is_none() {
                    a.portal.audit = Some(o.browse.portal.audit_out());
                }
                // PA-106: documents shared on Approve come from the catalog, through B2 first
                // as a Browse share does (Browse's view of B2 is kept each frame).
                if a.portal.catalog.is_none() {
                    a.portal.catalog = Some(o.opened.catalog.clone());
                }
                if let Some(n) = a.portal.waiting.take() {
                    o.browse.portal.requests_read(n);
                }
                if a.invite_list.app_audit.is_none() {
                    a.invite_list.app_audit = Some(o.browse.portal.audit_out());
                }
                if a.todo_templates.app_audit.is_none() {
                    a.todo_templates.app_audit = Some(o.browse.portal.audit_out());
                }
                if a.todo_templates.app_copies.is_none() {
                    a.todo_templates.app_copies = Some(o.browse.portal.todo_out());
                }
                if !a.balances.connected() {
                    a.balances
                        .connect(o.browse.portal.audit_out(), o.browse.portal.balance_out());
                }
                if std::mem::take(&mut a.portal.changed) {
                    o.browse.portal.reread();
                }
                if std::mem::take(&mut a.portal.fetch_now) {
                    o.browse.portal.fetch_now = true;
                }
            }
            // Intake's "Fetch uploads now" (PA-38): open to everyone, unlike Admin › Portal.
            if std::mem::take(&mut o.intake.ingest.fetch_portal) {
                o.browse.portal.fetch_now = true;
            }
            if o.browse.portal.lists_from.is_none() {
                o.browse.portal.lists_from =
                    Some((o.opened.catalog.clone(), o.opened.sync_device.clone()));
            }
            let received = o.browse.portal.tick(ui.ctx(), &o.opened.root);
            o.intake.ingest.portal = o.browse.portal.intake_line(&o.opened.root);
            if !received.is_empty() {
                let mut tx = vf_core::tx::Transaction::new("portal_receive").record_only();
                for e in received {
                    tx = tx.audit(e);
                }
                if let Err(e) = tx.commit(
                    &mut o.opened.conn,
                    &mut vf_core::tx::NoFileOps,
                    &vf_core::tx::Actor::device(&self.config.device_id),
                ) {
                    o.browse.error = Some(format!(
                        "Files received from the portal were NOT recorded in the audit trail: {e}"
                    ));
                }
            }
            // PA-103: to-do lists read are copied into their client's folder.
            let copies = o.browse.portal.take_todo_copies();
            if !copies.is_empty()
                && let Err(e) = crate::ui::todo::write_copies(
                    &mut o.opened.conn,
                    &o.opened.root,
                    &self.config.device_id,
                    copies,
                )
            {
                o.browse.error = Some(format!(
                    "A to-do list could not be copied into its client's folder: {e}"
                ));
            }
            // Addendum §16: balances read are copied into their client's folder.
            let reads = o.browse.portal.take_balance_reads();
            if !reads.is_empty()
                && let Err(e) = crate::ui::balance::record_reads(
                    &mut o.opened.conn,
                    &o.opened.root,
                    &self.config.device_id,
                    reads,
                )
            {
                o.browse.error = Some(format!(
                    "A balance could not be copied into its client's folder: {e}"
                ));
            }
            if let Some(n) = o.browse.portal.notice.take() {
                o.browse.notice = Some(n);
            }
            // The audit trail's copy in the cabinet (CLAUDE.md layout: _Vibe/audit/*.jsonl).
            // On its own thread with its own connection: it writes to the share.
            if o.mirror_job.is_none() && o.mirrored_at.is_none_or(|t| t.elapsed() >= o.mirror_wait)
            {
                o.mirrored_at = Some(std::time::Instant::now());
            }
            if o.mirror_job.is_none()
                && o.mirrored_at.is_some_and(|t| t.elapsed().as_millis() < 50)
                && vf_core::audit::mirror_due(&o.opened.conn)
            {
                let dir = o.opened.root.join("_Vibe").join("audit");
                let catalog = o.opened.catalog.clone();
                let work = move || {
                    vf_core::db::open(&catalog, vf_core::db::Storage::Local)
                        .and_then(|conn| vf_core::audit::mirror_pending(&conn, &dir))
                        .map_err(|e| (e.to_string(), e.is_busy()))
                };
                o.mirror_job = crate::ui::job::Job::start(ui.ctx(), "", work).ok();
            }
            if let Some(r) = crate::ui::job::take(&mut o.mirror_job) {
                let r = r.map_err(|e| (e, false)).and_then(|r| r);
                // The copy's own mark in the catalog is not a change by others (Q-102).
                o.data_version.0 = 0;
                o.mirror_wait = match r {
                    Ok(_) => std::time::Duration::from_secs(2),
                    Err(_) => (o.mirror_wait * 2).min(std::time::Duration::from_secs(120)),
                };
                match r {
                    Ok(_) => o.mirror_busy = 0,
                    Err((e, busy)) => {
                        tracing::warn!(kind = crate::logging::kind(&e), "audit mirror not written");
                        // The catalog busy (the index writes behind the window, another
                        // session saves): the copy is tried again in a moment and nothing is
                        // lost: not a problem to show in red (owner 2026-11-16, while the
                        // index ran). Told by SQLite's own code. Busy many times in a row is
                        // no longer a moment: said.
                        o.mirror_busy = if busy { o.mirror_busy + 1 } else { 0 };
                        if !busy || o.mirror_busy >= 10 {
                            o.browse.error = Some(format!(
                                "The audit trail's copy in the cabinet could not be written: {e}"
                            ));
                        }
                    }
                }
            }
            if let Some(problem) = o.chain.problem() {
                egui::Panel::top("vf-audit-banner")
                    .frame(
                        egui::Frame::new()
                            .fill(crate::ui::theme::soft(egui::Color32::from_rgb(
                                0xfd, 0xec, 0xea,
                            )))
                            .inner_margin(egui::Margin::symmetric(16, 8)),
                    )
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(problem)
                                .color(crate::ui::theme::error())
                                .strong(),
                        );
                    });
            }
        }
        // The version and the build's commit (a build identifier, not a link: PA-100).
        let mut segments = vec![format!(
            "vibe-files {} ({})",
            env!("CARGO_PKG_VERSION"),
            env!("VF_COMMIT")
        )];
        // Work going on behind the window, said in the footer with a spinner on every tab
        // (owner 2026-11-15, Q-112).
        let mut working = None;
        // PA-106: access requests waiting on the portal, on every PC.
        let mut link = None;
        if let Screen::Open(o) = &mut self.screen {
            let o = &mut **o;
            segments.insert(
                0,
                format!(
                    "{} documents · {} clients",
                    o.browse.document_total(),
                    o.browse.snap.clients.len()
                ),
            );
            working = behind_the_window(
                o.indexing.as_ref().map(|(ix, _)| ix.shared.progress()),
                o.servers.busy(),
                o.servers.leaving_copy(),
            );
            link = match o.browse.portal.requests_waiting {
                Some(1) => Some("1 portal access request waiting".to_string()),
                Some(n) if n > 1 => Some(format!("{n} portal access requests waiting")),
                _ => None,
            };
        }
        if chrome::footer(
            ui,
            &segments,
            working.as_deref(),
            link.as_deref(),
            name.as_deref().unwrap_or("No cabinet open"),
        ) {
            self.tab = Tab::Admin;
            match &mut self.screen {
                Screen::Open(o) if o.admin.is_some() => {
                    if let Some(a) = &mut o.admin {
                        a.page = crate::ui::admin::Page::PortalRequests;
                        a.portal.refresh_requests();
                    }
                }
                _ => self.open_requests = true,
            }
        }
        let ctx = ui.ctx().clone();
        let mut submit = None;
        let mut to_wizard = false;
        match &mut self.screen {
            Screen::Open(o) if self.tab == Tab::Intake => {
                let o = &mut **o;
                let setup_locked = !o.admin_unlocked && vf_fs::trash::has_pin(&o.opened.root);
                o.intake.partner = o.browse.prefs.partner.clone();
                o.intake.show(
                    ui,
                    frame,
                    &o.opened.root,
                    &o.opened.cabinet,
                    &self.config.ingest,
                    &o.browse.snap,
                    &o.browse.sheets_dir,
                    &mut o.opened.conn,
                    &self.config.device_id,
                    setup_locked,
                );
                // A partner chosen in Intake's review is Browse's choice too (kept per user).
                if let Some(p) = o.intake.partner_change.take() {
                    o.browse.set_partner(p);
                }
                // Not while the index runs behind the window (PA-60): a batch is recorded
                // against the whole catalog.
                if o.indexing.is_none() && !o.servers.first_pending() {
                    o.intake.reconcile(
                        &mut o.opened.conn,
                        &o.opened.catalog,
                        &o.opened.root,
                        &self.config.device_id,
                    );
                }
            }
            Screen::Open(o) if self.tab == Tab::Viewer => {
                let o = &mut **o;
                let initials = self.config.viewer.initials();
                let tick =
                    crate::ui::viewer::tick_color_index(self.config.viewer.tick_color.as_deref());
                o.viewer.tick_color = tick;
                let tick_size =
                    crate::ui::viewer::tick_size_index(self.config.viewer.tick_size.as_deref());
                o.viewer.tick_size = tick_size;
                o.viewer.tesseract = self.config.ocr.tesseract.clone();
                crate::ui::viewer::show(
                    ui,
                    &mut o.viewer,
                    &mut crate::ui::viewer::Env {
                        conn: &mut o.opened.conn,
                        root: &o.opened.root,
                        device_id: &self.config.device_id,
                        initials: &initials,
                        clients: &o.browse.snap.clients,
                    },
                );
                if std::mem::take(&mut o.viewer.closed) {
                    // Closing a document goes back to Browse (owner 2026-11-04).
                    self.tab = Tab::Browse;
                }
                if std::mem::take(&mut o.viewer.catalog_changed) {
                    o.browse.reload(&o.opened.conn);
                }
                // Initials, tickmark color and size are this PC's settings (owner 2026-11-04,
                // size 2026-11-11).
                let signed = o.viewer.signed.take();
                if signed.is_some()
                    || o.viewer.tick_color != tick
                    || o.viewer.tick_size != tick_size
                {
                    if signed.is_some() {
                        self.config.viewer.initials = signed;
                    }
                    self.config.viewer.tick_color =
                        Some(crate::ui::viewer::tick_color_name(o.viewer.tick_color).into());
                    self.config.viewer.tick_size =
                        Some(crate::ui::viewer::tick_size_name(o.viewer.tick_size).into());
                    if let Err(e) = config::save(&self.data, &self.config) {
                        o.viewer.error = Some(format!("Settings not saved: {e:#}"));
                    }
                }
                if let Some(p) = o.viewer.open_request.take()
                    && let Err(e) = (o.browse.shell)(&crate::shell::ShellAction::Open(p))
                {
                    o.viewer.error = Some(e);
                }
            }
            Screen::Open(o) if self.tab == Tab::Sync => {
                let o = &mut **o;
                let clients: Vec<(String, String)> = o
                    .browse
                    .snap
                    .clients
                    .iter()
                    .map(|c| (c.id.clone(), c.name.clone()))
                    .collect();
                let cabinet_id = o.opened.cabinet.cabinet_id.clone();
                let admin = o.admin_unlocked || !vf_fs::trash::has_pin(&o.opened.root);
                crate::ui::sync::show(
                    ui,
                    &mut o.sync,
                    &mut browse::Env {
                        conn: &mut o.opened.conn,
                        root: &o.opened.root,
                        device_id: &o.opened.sync_device,
                    },
                    crate::ui::sync::View {
                        catalog: &o.opened.catalog,
                        cabinet_id: &cabinet_id,
                        cfg: &self.config.sync,
                        clients: &clients,
                        admin,
                    },
                );
            }
            Screen::Open(o) if matches!(self.tab, Tab::Browse | Tab::Admin) => {
                let OpenCabinet {
                    opened,
                    browse: tab_browse,
                    admin: tab_admin,
                    intake: o_intake,
                    viewer: o_viewer,
                    chain: o_chain,
                    sweep: o_sweep,
                    sync: o_sync,
                    admin_unlocked,
                    admin_typed,
                    admin_msg,
                    ready: o_ready,
                    ..
                } = &mut **o;
                let mut env = browse::Env {
                    conn: &mut opened.conn,
                    root: &opened.root,
                    device_id: &self.config.device_id,
                };
                // The admin password (PA-24): Admin waits behind the lock screen, but for the
                // pages the admin left open (PA-64), which are listed with "Unlock Admin…".
                let admin_locked = self.tab == Tab::Admin
                    && !*admin_unlocked
                    && vf_fs::trash::has_pin(&opened.root);
                let some_open = admin_locked && {
                    let a = tab_admin.get_or_insert_with(|| Admin::load(&env));
                    !a.open.is_empty() && !a.unlock_requested
                };
                if let Some(j) = o_ready.as_ref().filter(|_| self.tab == Tab::Admin) {
                    // The legal holds placed on other PCs are still being read (PA-82):
                    // nothing is emptied from the trash or disposed of before they are.
                    egui::CentralPanel::default().show(ui, |ui| {
                        j.show(ui);
                        ui.label(crate::ui::widgets::caption(
                            "Admin opens once the cabinet's legal holds and trash have been read.",
                        ));
                    });
                } else if admin_locked && !some_open {
                    let mut back = false;
                    let can_go_back = tab_admin.as_ref().is_some_and(|a| !a.open.is_empty());
                    egui::CentralPanel::default().show(ui, |ui| {
                        if let crate::ui::admin_password::Unlock::Unlocked =
                            crate::ui::admin_password::lock_screen(
                                ui,
                                admin_typed,
                                admin_msg,
                                &mut env,
                            )
                        {
                            *admin_unlocked = true;
                            back = true;
                        }
                        if can_go_back {
                            ui.vertical_centered(|ui| {
                                ui.add_space(8.0);
                                if ui.button("Back to the open pages").clicked() {
                                    back = true;
                                }
                            });
                        }
                    });
                    if back && let Some(a) = tab_admin.as_mut() {
                        a.unlock_requested = false;
                    }
                } else if self.tab == Tab::Admin {
                    let a = tab_admin.get_or_insert_with(|| Admin::load(&env));
                    if std::mem::take(&mut self.open_requests) {
                        a.page = crate::ui::admin::Page::PortalRequests;
                        a.portal.refresh_requests();
                    }
                    // Documents shared on Approve go to B2 first too (PA-106), as B2 is now.
                    if a.page == crate::ui::admin::Page::PortalRequests {
                        a.portal.b2 = b2_first(
                            o_sync,
                            &opened.catalog,
                            &opened.root,
                            &opened.cabinet.cabinet_id,
                            &self.config,
                        );
                    }
                    a.capture = self.config.ingest.print_capture.clone();
                    a.chain = Some(o_chain.clone());
                    admin::show(
                        ui,
                        a,
                        &mut env,
                        admin::SweepEnv {
                            state: o_sweep,
                            catalog: &opened.catalog,
                        },
                        admin_locked,
                    );
                    if std::mem::take(&mut a.lock_requested) {
                        *admin_unlocked = false;
                        // As the cabinet has it now (another PC's admin may have changed it).
                        a.open = vf_fs::trash::open_pages(&opened.root);
                        a.unlock_requested = false;
                    }
                    if std::mem::take(&mut a.audit.verify_requested) {
                        o_chain.verify_now();
                    }
                    if let Some(dir) = a.open_request.take()
                        && let Err(e) = (tab_browse.shell)(&crate::shell::ShellAction::Open(dir))
                    {
                        a.error = Some(e);
                    }
                    if std::mem::take(&mut a.rules_changed) {
                        o_intake.rules_changed();
                    }
                    if let Some(c) = a.capture_changed.take() {
                        let mut ingest = self.config.ingest.clone();
                        ingest.print_capture = c;
                        o_intake.ingest.changed = Some(ingest);
                    }
                } else {
                    let intake_changed = std::mem::take(&mut o_intake.catalog_changed);
                    if intake_changed
                        || tab_admin.as_mut().is_some_and(|a| {
                            a.poll();
                            std::mem::take(&mut a.changed)
                        })
                    {
                        tab_browse.reload(env.conn);
                    }
                    if let Some(m) = o_intake.message.take() {
                        tab_browse.notice = Some(m);
                    }
                    tab_browse.sync = o_sync.session.is_some().then(|| browse::SyncView {
                        pending: o_sync.pending.clone(),
                        conflicts: o_sync.conflicts.len(),
                        running_for: o_sync.running_for.clone(),
                    });
                    // A portal share goes to B2 first when B2 is set up (Q-103).
                    tab_browse.b2_first = b2_first(
                        o_sync,
                        &opened.catalog,
                        &opened.root,
                        &opened.cabinet.cabinet_id,
                        &self.config,
                    );
                    tab_browse.admin_unlocked = *admin_unlocked;
                    browse::show(ui, tab_browse, &mut env);
                    // Browse's Scan: Intake's scanner, profile and Scans folder (owner
                    // 2026-11-04); the outcome lands in Intake's log and on Browse.
                    if std::mem::take(&mut tab_browse.scan_request) {
                        // A client's folder shown: the scan goes straight into it (owner
                        // 2026-11-04); otherwise to the Scans folder and Intake.
                        let target = tab_browse.scan_target();
                        let mut private = None;
                        let started = match &target {
                            Some(_) => vf_fs::content::tmp_file(&opened.root, "scan")
                                .map(|p| p.with_extension(""))
                                .and_then(|d| {
                                    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
                                    Ok(d)
                                })
                                .and_then(|d| {
                                    private = Some(d.clone());
                                    o_intake.ingest.start_scan_into(
                                        ui.ctx(),
                                        &self.config.ingest,
                                        d,
                                    )
                                }),
                            None => o_intake.ingest.start_scan(ui.ctx(), &self.config.ingest),
                        };
                        match started {
                            Ok(name) => {
                                tab_browse.notice = Some(match &target {
                                    Some(t) => format!(
                                        "Scanning on {name}… It is filed into {} when done.",
                                        t.folder.replace('/', " / ")
                                    ),
                                    None => format!(
                                        "Scanning on {name}… The pages go to the Scans folder and then to Intake."
                                    ),
                                });
                                tab_browse.scan_watching = true;
                                tab_browse.scan_target = target.zip(private);
                            }
                            Err(e) => {
                                if let Some(d) = private {
                                    let _ = std::fs::remove_dir(d);
                                }
                                tab_browse.error = Some(format!("Not scanned: {e}"));
                            }
                        }
                    }
                    if std::mem::take(&mut tab_browse.scan_stop_request) {
                        o_intake.ingest.stop_scan();
                    }
                    if tab_browse.scan_watching && !o_intake.ingest.scanning() {
                        tab_browse.scan_watching = false;
                        match tab_browse.scan_target.take() {
                            Some((t, dir)) => match o_intake.ingest.take_finished_scan(&dir) {
                                Some(Ok(pdf)) => file_browse_scan(
                                    tab_browse,
                                    &mut env,
                                    &t,
                                    &dir,
                                    &pdf,
                                    self.config.ingest.scans.as_deref(),
                                ),
                                Some(Err(e)) => {
                                    let _ = std::fs::remove_dir_all(&dir);
                                    tab_browse.error = Some(format!("Not scanned: {e}"));
                                }
                                None => {
                                    // Stopped with nothing: the private folder is empty.
                                    let _ = std::fs::remove_dir(&dir);
                                    if let Some(l) = o_intake.ingest.last_line() {
                                        tab_browse.notice = Some(l.to_string());
                                    }
                                }
                            },
                            None => {
                                if let Some(l) = o_intake.ingest.last_line() {
                                    tab_browse.notice = Some(l.to_string());
                                }
                            }
                        }
                    }
                    if let Some(d) = tab_browse.view_request.take() {
                        o_viewer.open(d);
                        self.tab = Tab::Viewer;
                    }
                    if let Some(r) = tab_browse.sync_request.take() {
                        use crate::ui::sync::Op;
                        match r {
                            browse::SyncRequest::Push(c) => o_sync.start(
                                ui.ctx(),
                                &opened.catalog,
                                &opened.root,
                                Op::Push(Some(c)),
                            ),
                            browse::SyncRequest::PushDocs(ids) => o_sync.start(
                                ui.ctx(),
                                &opened.catalog,
                                &opened.root,
                                Op::PushDocs(ids),
                            ),
                            browse::SyncRequest::RemoveDocs(ids) => o_sync.start(
                                ui.ctx(),
                                &opened.catalog,
                                &opened.root,
                                Op::RemoveDocs(ids),
                            ),
                            browse::SyncRequest::Pull => {
                                o_sync.start(ui.ctx(), &opened.catalog, &opened.root, Op::Pull)
                            }
                            browse::SyncRequest::Verify => {
                                o_sync.start(ui.ctx(), &opened.catalog, &opened.root, Op::Verify)
                            }
                            browse::SyncRequest::Show(c) => {
                                o_sync.filter_client = c;
                                self.tab = Tab::Sync;
                            }
                        }
                    }
                }
            }
            screen => {
                egui::CentralPanel::default().show(ui, |ui| match screen {
                    Screen::Wizard(w) => submit = wizard::show(ui, w),
                    Screen::Busy { what, back, .. } => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(*what);
                        });
                        // What is being waited for, and for how long (owner 2026-11-15: a
                        // session host's log cannot always be reached).
                        if let Some((step, since)) = session::current_step() {
                            ui.label(crate::ui::widgets::caption(opening_line(step, since)));
                        }
                        // A first index lists every client folder (PA-60: it reads no file):
                        // say how far it is (owner 2026-11-05).
                        use std::sync::atomic::Ordering;
                        let p = &vf_fs::rebuild::PROGRESS;
                        let (dirs, found, read) = (
                            p.folders.load(Ordering::Relaxed),
                            p.found.load(Ordering::Relaxed),
                            p.read.load(Ordering::Relaxed),
                        );
                        if found > 0 {
                            ui.label(crate::ui::widgets::caption(format!(
                                "Indexing: {read} of {found} files listed ({dirs} client folders). Files are listed, not read; later opens skip this."
                            )));
                        } else if dirs > 0 {
                            ui.label(crate::ui::widgets::caption(format!(
                                "Indexing: listing files ({dirs} client folders so far)…"
                            )));
                        }
                        ui.ctx()
                            .request_repaint_after(std::time::Duration::from_millis(250));
                        if back.mode == Mode::Open {
                            ui.add_space(8.0);
                            if ui.button("Open a different cabinet").clicked() {
                                to_wizard = true;
                            }
                        }
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
            o.intake.ingest.tesseract = self.config.ocr.tesseract.clone();
            o.intake.ingest.tick(
                &ctx,
                &o.opened.root,
                &self.config.device_id,
                &self.config.ingest,
            );
            o.intake.ingest.check_dupes(
                &ctx,
                &o.opened.catalog,
                &o.opened.root,
                &self.config.device_id,
            );
            if let Some(p) = o.intake.ingest.open_request.take()
                && let Err(e) = (o.browse.shell)(&crate::shell::ShellAction::Open(p))
            {
                o.intake.ingest.warning = Some(format!("The file did not open: {e}"));
            }
            if let Some(ingest) = o.intake.ingest.changed.take() {
                self.config.ingest = ingest;
                if let Err(e) = config::save(&self.data, &self.config) {
                    o.intake.ingest.warning = Some(format!("Settings could not be saved: {e:#}"));
                }
            }
        }
        if let Some(rx) = &self.update
            && let Ok(r) = rx.try_recv()
        {
            self.update = None;
            self.update_notice = r.map(|r| {
                format!(
                    "Vibe Files {} is available (this is {}): {} — check the download against the release's SHA256SUMS.txt, or `winget upgrade Kisaes.VibeFiles`.",
                    r.version,
                    env!("CARGO_PKG_VERSION"),
                    r.url
                )
            });
        }
        if let Screen::Open(o) = &mut self.screen
            && o.browse.notice.is_none()
            && let Some(n) = self.update_notice.take()
        {
            o.browse.notice = Some(n);
        }
        if let Screen::Open(o) = &mut self.screen
            && o.browse.notice.is_none()
            && let Some(n) = crate::cloudmode::take_conflicts()
        {
            o.browse.notice = Some(n);
        }
        if let Screen::Open(o) = &mut self.screen {
            let o = &mut **o;
            // The other servers' changes wait for the index behind the window (they catalog
            // the files they find); so does B2 sync, but for the documents chosen.
            let indexed = o.indexing.is_none();
            if indexed
                && let Some(w) = o.opened.writer.clone()
                && o.servers.tick(
                    &ctx,
                    &o.opened.catalog,
                    &o.opened.root,
                    &w,
                    &self.config.device_id,
                    o.browse.client_shown(),
                )
            {
                o.browse.reload(&o.opened.conn);
            }
            o.sync.hold = !indexed || o.servers.first_pending();
            o.sync.tick(
                &ctx,
                &browse::Env {
                    conn: &mut o.opened.conn,
                    root: &o.opened.root,
                    device_id: &o.opened.sync_device,
                },
                &o.opened.catalog,
                &self.config.sync,
            );
            if std::mem::take(&mut o.sync.changed) {
                o.browse.reload(&o.opened.conn);
            }
            if let Some(rx) = &o.checks
                && let Ok(checks) = rx.try_recv()
            {
                o.checks = None;
                let failed: Vec<String> = checks
                    .iter()
                    .filter(|c| !c.ok)
                    .map(|c| format!("{}: {}", c.name, c.detail))
                    .collect();
                for c in &checks {
                    tracing::info!(check = c.name, ok = c.ok, "startup self-check");
                }
                if !failed.is_empty() {
                    add_error(
                        &mut o.browse.error,
                        format!("Startup check: {}", failed.join(" · ")),
                    );
                }
            }
            if let Some(c) = o.sync.config_changed.take() {
                self.config.sync = c;
                if let Err(e) = config::save(&self.data, &self.config) {
                    o.sync.error = Some(format!("Settings could not be saved: {e:#}"));
                }
            }
        }
        if let Screen::Open(o) = &mut self.screen
            && std::mem::take(&mut o.browse.prefs_dirty)
        {
            let mut prefs = o.browse.prefs.clone();
            // Several people on one server share config.toml: the others' partner choices as
            // the file has them now, with this user's put in.
            if let Ok(on_disk) = config::load(&self.data) {
                prefs.partner_by_user = on_disk.prefs.partner_by_user;
                prefs.hidden_columns_by_user = on_disk.prefs.hidden_columns_by_user;
                prefs.explorer_by_user = on_disk.prefs.explorer_by_user;
            }
            prefs.store_user(&config::windows_user());
            o.browse.prefs.explorer_by_user = prefs.explorer_by_user.clone();
            o.browse.prefs.partner_by_user = prefs.partner_by_user.clone();
            o.browse.prefs.hidden_columns_by_user = prefs.hidden_columns_by_user.clone();
            self.config.prefs = prefs;
            if let Err(e) = config::save(&self.data, &self.config) {
                o.browse.error = Some(format!("Pinned/Favorites could not be saved: {e:#}"));
            }
        }
        if let Some(mode) = submit {
            self.start(mode, &ctx);
        }
        // The opening carries on on its thread; its result is dropped with the receiver.
        if to_wizard && let Screen::Busy { back, .. } = &self.screen {
            self.screen = Screen::Wizard(back.clone());
        }
    }
}

/// Adds `msg` to a message line without hiding what it already says (QA3 #14).
fn add_error(line: &mut Option<String>, msg: String) {
    *line = Some(match line.take() {
        Some(old) if !old.is_empty() => format!("{old} · {msg}"),
        _ => msg,
    });
}

/// What [`ready`] found and did.
pub struct Readied {
    /// Print capture rules not in line (shown on Browse).
    rules_problem: Option<String>,
    /// Documents the 90-day trash rule emptied.
    emptied: Result<usize, String>,
    /// The catalog was changed: Browse reads it again.
    changed: bool,
}

/// What the opening thread hands over with the open cabinet.
pub struct Prepared {
    /// The id this login works under: on a session host, the one the cabinet remembers for
    /// the login (PA-69), which may not be the one in this host's settings.
    device: String,
    /// That id held for this session (another session of the login open elsewhere at the same
    /// time works under its host's own).
    login: Option<vf_fs::lock::Held>,
    /// Why the login's own inbox named in `firm.toml` cannot be used (Q-112).
    inbox_problem: Option<String>,
}

/// Removes what the check-out build left (Q-52): rows in the catalog's `lock` table (nothing
/// writes them any more) and `_Vibe/locks/*.lock`, `_Vibe/intake.lock` on the server.
fn clear_check_out_leftovers(conn: &mut vf_core::db::Connection, root: &std::path::Path) {
    if let Err(e) = conn.execute("DELETE FROM lock", []) {
        tracing::warn!(error = %e, "old check-outs not cleared");
    }
    let vibe = root.join(vf_fs::cabinet::VIBE_DIR);
    let _ = std::fs::remove_file(vibe.join("intake.lock"));
    if let Ok(rd) = std::fs::read_dir(vibe.join("locks")) {
        for e in rd.flatten() {
            if e.path().extension().is_some_and(|x| x == "lock") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let _ = std::fs::remove_dir(vibe.join("locks"));
}

/// The work of opening a cabinet that reads and writes the share or the whole catalog: print
/// capture rules, this workstation's registration, legal holds, the old trash. Runs behind
/// the window on its own connection (PA-82, owner 2026-11-16: it kept the opening screen up;
/// before, on the opening thread, QA A6). How long each part took goes to the log.
fn ready(
    conn: &mut vf_core::db::Connection,
    root: &Path,
    device_id: &str,
    folders_behind: bool,
) -> Readied {
    let mut at = std::time::Instant::now();
    let mut lap = |part: &'static str| {
        tracing::info!(
            part,
            ms = at.elapsed().as_millis() as u64,
            "opening: behind the window"
        );
        at = std::time::Instant::now();
    };
    let root = root.to_path_buf();
    let before = conn.total_changes();
    // Client folders Intake's folder index has and this catalog has not (PA-83): indexed now
    // from their listing, those and no others. Not done (an error, or the app closed first):
    // the scan is not marked, so the next open looks again.
    if folders_behind {
        match patient(|| {
            vf_fs::rebuild::take_folders_behind(conn, &root, device_id).map_err(|e| e.to_string())
        }) {
            Ok(r) => tracing::info!(
                clients = r.clients,
                docs_new = r.docs_new,
                docs_tombstoned = r.docs_tombstoned,
                unread = r.unreadable.len(),
                not_taken = vf_fs::rebuild::folders_not_taken(conn).len(),
                "client folders behind Intake's list indexed"
            ),
            // (Nothing was marked: the next open looks again, and what was already tried and
            // could not be taken is still remembered.)
            Err(e) => tracing::warn!(
                kind = crate::logging::kind(&e),
                "client folders behind Intake's list not read"
            ),
        }
        lap("folders behind Intake's list");
    }
    // Print captures are filed by their doc types' folders (Admin › Print capture): Filer's
    // rules are brought in line before Intake first loads them.
    let _ = vf_ingest::parsers::seed_if_missing(&root);
    // Shown on Browse, not only logged (QA2 U6).
    let rules_problem = match vf_ingest::parsers::sync_filer_rules_report(&root) {
        Ok(s) if s.problems.is_empty() => None,
        Ok(s) => Some(s.problems.join("; ")),
        Err(e) => Some(e),
    };
    if let Some(e) = &rules_problem {
        tracing::warn!(
            kind = crate::logging::kind(e),
            "print capture rules not written"
        );
    }
    // Check-out was built and removed the same day (Q-52): a catalog that copied other PCs'
    // check-outs, and lock files on the server, would otherwise refuse edits for good.
    lap("print capture rules");
    clear_check_out_leftovers(conn, &root);
    lap("old check-outs");
    // Intake's client box lists Filer's client list: clients known only from their folders
    // are put in it (owner 2026-11-04: no client to pick in Intake).
    if let Err(e) = vf_fs::clients::ensure_listed(conn, &root) {
        tracing::warn!(kind = crate::logging::kind(&e), "client list not completed");
    }
    lap("client list");
    // This workstation in the cabinet's list (the Safeguards appendix lists them).
    if let Err(e) =
        vf_fs::safeguards::register_device(&root, device_id, crate::server::server_mode())
    {
        tracing::warn!(
            kind = crate::logging::kind(&e),
            "workstation not registered"
        );
    }
    // Legal holds placed on other workstations first (P07): nothing they cover may be emptied
    // below.
    lap("workstation list");
    let held = patient(|| vf_fs::holds::sync(conn, &root, device_id));
    if let Err(e) = &held {
        tracing::warn!(kind = crate::logging::kind(e), "legal holds not synced");
    }
    lap("legal holds");
    // This login's undo list as the cabinet has it (Q-105): a catalog made this morning
    // still offers yesterday's changes to undo.
    let undo = patient(|| vf_fs::undolist::restore(conn, &root, device_id));
    if let Err(e) = &undo {
        tracing::warn!(kind = crate::logging::kind(e), "undo list not restored");
    }
    lap("undo list");
    // Intake folders nobody has used for a month (hosts given a new id at every reset left
    // one each) are set aside, so they are not read at every open and every batch.
    let idle = vf_fs::intake::archive_idle(&root, device_id);
    if idle > 0 {
        tracing::info!(idle, "idle Intake folders set aside");
    }
    // Trash older than 90 days empties on its own (Q-15; held documents stay).
    lap("idle Intake folders");
    let emptied = patient(|| vf_fs::trash::auto_empty(conn, &root, device_id));
    lap("old trash");
    // Browse reads the catalog again only when a row was written here.
    let changed = conn.total_changes() != before;
    Readied {
        rules_problem,
        emptied,
        changed,
    }
}

/// `work` again, a few times, when the catalog was busy: behind the window it writes while
/// the index does (PA-82), and a write that lost its turn is no problem to show in red.
pub(crate) fn patient<T>(mut work: impl FnMut() -> Result<T, String>) -> Result<T, String> {
    let mut tries = 0;
    loop {
        match work() {
            Err(e) if tries < 5 && crate::selfcheck::is_busy_text(&e) => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
            r => return r,
        }
    }
}

/// Files a scan started from Browse (in the private folder `dir`) into the folder shown. The
/// scanned pages are never lost (QA 2026-11-04): when filing fails, the PDF goes to this PC's
/// Scans folder for Intake, or stays where it is and the message says where.
fn file_browse_scan(
    b: &mut Browse,
    env: &mut browse::Env<'_>,
    t: &browse::ScanTarget,
    dir: &std::path::Path,
    pdf: &std::path::Path,
    scans: Option<&std::path::Path>,
) {
    let name = pdf
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Scan.pdf".into());
    // content::add removes its input when it fails: it is given a copy.
    let keep = dir.join(format!("kept-{name}"));
    if let Err(e) = std::fs::copy(pdf, &keep) {
        b.error = Some(format!(
            "Scanned, but not filed: {e}. The scan is at {}.",
            pdf.display()
        ));
        return;
    }
    let r = vf_fs::content::add(
        env.conn,
        env.root,
        env.device_id,
        pdf,
        vf_fs::content::NewDoc {
            rel_path: format!("{}/{name}", t.folder),
            client_id: Some(t.client_id.clone()),
            doc_type_id: None,
            tax_year: t.tax_year,
            source: Some("client".into()),
            tags: vec![],
            label: "scan".into(),
            detail: serde_json::json!({ "scanned_into": t.folder }),
        },
    );
    match r {
        Ok(added) => {
            let _ = std::fs::remove_dir_all(dir);
            b.reload(env.conn);
            b.notice = Some(format!(
                "Scanned into {}. Set its doc type in the details.",
                added.rel_path.replace('/', " / ")
            ));
        }
        Err(e) => {
            let moved = scans.and_then(|s| {
                let to = s.join(&name);
                (!to.exists() && std::fs::rename(&keep, &to).is_ok()).then_some(to)
            });
            b.error = Some(match moved {
                Some(to) => format!(
                    "Scanned, but not filed: {e}. The scan went to the Scans folder for Intake ({}).",
                    to.display()
                ),
                None => format!(
                    "Scanned, but not filed: {e}. The scan is kept at {}.",
                    keep.display()
                ),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::ui::test_util::type_into;
    use std::path::Path;

    use super::*;
    use egui::accesskit::Role;
    use egui_kittest::HarnessBuilder;
    use egui_kittest::kittest::Queryable;

    fn harness(data: &Path) -> egui_kittest::Harness<'static, VibeFilesApp> {
        crate::ui::test_util::pdfium_first();
        let data = data.to_owned();
        let mut h = HarnessBuilder::default()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 900.0])
            .build_eframe(move |cc| VibeFilesApp::new(&cc.egui_ctx, data.clone()));
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
    fn five_tabs_in_spec_order() {
        let names: Vec<_> = Tab::ALL.iter().map(|t| t.name()).collect();
        assert_eq!(names, ["Browse", "Intake", "Viewer", "Sync", "Admin"]);
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

    /// P01 step 1 (PA-18: no passphrase): the wizard creates a cabinet another PC can open.
    #[test]
    fn wizard_creates_a_cabinet() {
        let tmp = tempfile::tempdir().unwrap();
        let (data, root) = (tmp.path().join("data"), tmp.path().join("Clients"));
        let mut h = harness(&data);
        click(&mut h, "Create new");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
        type_into(&mut h, "Cabinet name", "Kisaes Clients");
        click(&mut h, "Create cabinet");
        settle(&mut h);

        assert!(matches!(h.state().screen, Screen::Open(_)), "cabinet open");
        assert_eq!(h.state().cabinet_name().as_deref(), Some("Kisaes Clients"));
        assert!(h.query_all_by_label("Kisaes Clients").count() >= 1);
        let cab = vf_fs::cabinet::read(&root).unwrap();
        assert_eq!(cab.name.as_deref(), Some("Kisaes Clients"));
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
        // Another PC opens it from an empty data folder, no passphrase.
        let other = tmp.path().join("pc2");
        assert!(session::open(&other, "dev-2", &root).is_ok());
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
        };
        drop(session::create(&tmp.path().join("pc1"), "dev-1", &req).unwrap());
        let mut h = harness(&tmp.path().join("pc1b"));
        click(&mut h, "Open existing");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
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

    /// P11 done-when 5: from Browse, `Sync to B2 ▾ → Push this client now` pushes only that
    /// client's pending documents and the breadcrumb pill goes from `4 pending` to `in sync`
    /// without leaving the tab. (An in-memory bucket stands in for B2.)
    #[test]
    fn push_this_client_from_browse_turns_the_pill_in_sync() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Clients");
        let req = CreateRequest {
            root: &root,
            name: "Firm",
        };
        let data = tmp.path().join("pc1");
        {
            let mut o = session::create(&data, "dev-1", &req).unwrap();
            for (id, name) in [("1042", "Anderson Farms LLC"), ("2002", "Bell Ranch Inc")] {
                let p = vf_fs::newclient::plan(
                    &o.conn,
                    &root,
                    &vf_fs::newclient::NewClient {
                        partner: String::new(),
                        ext_id: id.into(),
                        name: name.into(),
                        entity_type: None,
                    },
                )
                .unwrap();
                vf_fs::newclient::create(&mut o.conn, &root, &p, "dev-1").unwrap();
            }
            let put = |rel: &str, body: &str| {
                let p = root.join(rel);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(p, body).unwrap();
            };
            for n in 0..4 {
                put(
                    &format!("Anderson Farms LLC (1042)/Income Tax/2025/doc {n}.pdf"),
                    &format!("invented {n}"),
                );
            }
            put("Bell Ranch Inc (2002)/Payroll/2025/941.pdf", "b");
            vf_fs::rebuild::rebuild(&mut o.conn, &root, "dev-1").unwrap();
        }
        let mut h = harness(&data);
        click(&mut h, "Open existing");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
        click(&mut h, "Open cabinet");
        settle(&mut h);
        // Sync hidden until it is set up and unlocked.
        assert!(h.query_by_label_contains("Sync to B2").is_none());
        let (anderson, cabinet_id) = {
            let Screen::Open(o) = &mut h.state_mut().screen else {
                panic!("open")
            };
            let remote =
                vf_sync::Remote::new(vf_sync::Remote::memory(), &o.opened.cabinet.cabinet_id);
            let keys = vf_sync::setup::init(
                &o.opened.root,
                &remote,
                "correct horse battery staple",
                vf_sync::setup::DEFAULT_KDF,
            )
            .unwrap();
            o.sync.session = Some(vf_sync::Sync {
                remote,
                keys,
                cabinet_id: o.opened.cabinet.cabinet_id.clone(),
                device_id: "dev-1".into(),
                device_name: "OFFICE-PC".into(),
            });
            let i = o
                .browse
                .snap
                .clients
                .iter()
                .position(|c| c.ext_id == "1042")
                .unwrap();
            o.browse.set_scope(vf_core::browse::Scope::Client(i));
            (
                o.browse.snap.clients[i].id.clone(),
                o.opened.cabinet.cabinet_id.clone(),
            )
        };
        let _ = cabinet_id;
        // The counts are made on their own thread (PA-34).
        let start = std::time::Instant::now();
        while h.query_by_label("4 pending").is_none() {
            h.step();
            assert!(
                start.elapsed() < crate::ui::test_util::PATIENCE,
                "the pill counts the client's documents"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        h.run();
        h.get_by_role_and_label(Role::Button, "Sync to B2").click();
        h.run();
        h.get_by_role_and_label(Role::Button, "Push this client now")
            .click();
        h.step();
        let start = std::time::Instant::now();
        while h.query_by_label("in sync").is_none() {
            h.step();
            assert!(
                start.elapsed() < crate::ui::test_util::PATIENCE,
                "push did not finish"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(h.state().tab, Tab::Browse, "without leaving the tab");
        let Screen::Open(o) = &h.state().screen else {
            panic!("open")
        };
        let pending = vf_sync::catalog::pending(
            &o.opened.conn,
            &vf_sync::rules::Rules::load_or_default(&o.opened.root),
        )
        .unwrap();
        assert_eq!(pending.get(&anderson), None);
        assert_eq!(
            pending.values().sum::<usize>(),
            1,
            "Bell Ranch's document still waits"
        );
    }

    /// P11 step 8: the Sync tab offers set-up until sync is configured; unlocked, it pushes and
    /// shows the result.
    #[test]
    fn sync_tab_sets_up_and_pushes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Clients");
        // A recent tax year (from its folder): what goes to B2 (PA-53) sends it.
        let year = chrono::Datelike::year(&chrono::Local::now());
        std::fs::create_dir_all(root.join(format!("Loose/{year}"))).unwrap();
        std::fs::write(root.join(format!("Loose/{year}/a.pdf")), "invented").unwrap();
        let req = CreateRequest {
            root: &root,
            name: "Firm",
        };
        let data = tmp.path().join("pc1");
        drop(session::create(&data, "dev-1", &req).unwrap());
        let mut h = harness(&data);
        click(&mut h, "Open existing");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
        click(&mut h, "Open cabinet");
        settle(&mut h);
        click(&mut h, "Sync");
        h.run();
        assert!(h.query_by_label("Set up sync").is_some());
        assert!(h.query_by_label("Connect").is_some());
        {
            let app = h.state_mut();
            let Screen::Open(o) = &mut app.screen else {
                panic!("open")
            };
            let id = o.opened.cabinet.cabinet_id.clone();
            let remote = vf_sync::Remote::new(vf_sync::Remote::memory(), &id);
            let keys = vf_sync::setup::init(
                &o.opened.root,
                &remote,
                "correct horse battery staple",
                vf_sync::setup::DEFAULT_KDF,
            )
            .unwrap();
            app.config.sync.endpoint = "https://s3.example.invalid".into();
            app.config.sync.bucket = "firm".into();
            app.config.sync.cabinet_id = id.clone();
            app.config.sync.app_key_sealed = vf_sync::setup::seal_secret(&keys, "app-key");
            o.sync.session = Some(vf_sync::Sync {
                remote,
                keys,
                cabinet_id: id,
                // The id the Sync tab reads this PC's last push under.
                device_id: o.opened.sync_device.clone(),
                device_name: "OFFICE-PC".into(),
            });
        }
        h.run();
        click(&mut h, "Push now");
        let start = std::time::Instant::now();
        while h.query_by_label_contains("Pushed 1 documents").is_none() {
            h.step();
            assert!(start.elapsed() < crate::ui::test_util::PATIENCE);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // The counts are made on their own thread (PA-34): wait for them.
        let start = std::time::Instant::now();
        while h.query_by_label("in sync").is_none()
            || h.query_by_label("Nothing waiting: this PC and the bucket agree.")
                .is_none()
        {
            h.step();
            // (Failed now and then under a full run, never alone, with no word of what the
            // page showed: said here, and given longer than a loaded machine needs.)
            if start.elapsed() >= std::time::Duration::from_secs(90) {
                let Screen::Open(o) = &h.state().screen else {
                    panic!("open")
                };
                panic!(
                    "counts never refreshed: last push {:?}, last pull {:?}, {} conflicts, error {:?}, notice {:?}, in sync shown {}, nothing waiting shown {}",
                    o.sync.last_push,
                    o.sync.last_pull,
                    o.sync.conflicts.len(),
                    o.sync.error,
                    o.sync.notice,
                    h.query_by_label("in sync").is_some(),
                    h.query_by_label("Nothing waiting: this PC and the bucket agree.")
                        .is_some(),
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        // Thousands of clients pending (owner 2026-11-12): what goes to B2 and the settings
        // stay above the list, and the list lays out only the rows in view.
        {
            let Screen::Open(o) = &mut h.state_mut().screen else {
                panic!("open")
            };
            o.sync.pending = (0..5000).map(|i| (format!("C{i:05}"), 3)).collect();
        }
        h.run();
        assert!(h.query_by_label("What goes to B2").is_some());
        // PA-63: the earlier rule, shown as a list of two rules.
        assert!(h.query_by_label("Recent tax years").is_some());
        assert!(h.query_by_label("Permanent files").is_some());
        assert!(h.query_by_label("Add rule").is_some());
        let pushes = h.query_all_by_label("Push").count();
        assert!(pushes > 0 && pushes < 60, "{pushes} rows laid out");
        // A third rule: a folder ticked, every tax year. Saving writes the whole list.
        let root = {
            let Screen::Open(o) = &h.state().screen else {
                panic!("open")
            };
            o.opened.root.clone()
        };
        assert!(vf_fs::clients::load_b2(&root).unwrap().rules.is_none());
        click(&mut h, "Add rule");
        h.run();
        type_into(&mut h, "Rule name", "Tax returns");
        // A tick box and a radio button, not buttons.
        h.get_by_label("Income Tax").click();
        h.run();
        h.get_by_label("Any tax year").click();
        h.run();
        // The editor's buttons are below the fold of the test window.
        h.get_by_label("Save rule").scroll_to_me();
        h.run();
        click(&mut h, "Save rule");
        h.run();
        let rules = vf_fs::clients::load_b2(&root).unwrap().rules.unwrap();
        assert_eq!(rules.len(), 3, "{rules:?}");
        assert_eq!(rules[2].name, "Tax returns");
        assert_eq!(rules[2].folders, ["Income Tax"]);
        assert_eq!(rules[2].years, vf_fs::clients::B2Years::Any);
        assert!(h.query_by_label("Tax returns").is_some());
        assert!(h.query_by_label("New rule").is_none(), "the editor closed");
        // PA-63: the Sync page's client table. Every client listed, one found by part of its
        // name, ticked, and set to Never send in one save.
        {
            let Screen::Open(o) = &mut h.state_mut().screen else {
                panic!("open")
            };
            for (id, name) in [("1042", "Anderson Farms LLC"), ("2002", "Bell Ranch Inc")] {
                let p = vf_fs::newclient::plan(
                    &o.opened.conn,
                    &root,
                    &vf_fs::newclient::NewClient {
                        partner: String::new(),
                        ext_id: id.into(),
                        name: name.into(),
                        entity_type: None,
                    },
                )
                .unwrap();
                vf_fs::newclient::create(&mut o.opened.conn, &root, &p, "dev-1").unwrap();
            }
            o.browse.reload(&o.opened.conn);
        }
        h.run();
        h.get_by_label("Tick all listed").scroll_to_me();
        h.run();
        h.get_by_label("All clients").click();
        h.run();
        assert!(h.query_by_label("Anderson Farms LLC").is_some());
        assert!(h.query_by_label("Bell Ranch Inc").is_some());
        type_into(&mut h, "Find a client", "bell");
        h.run();
        assert!(h.query_by_label("Anderson Farms LLC").is_none());
        assert!(h.query_by_label("Bell Ranch Inc").is_some());
        click(&mut h, "Tick all listed");
        h.run();
        assert!(h.query_by_label("1 listed · 1 ticked").is_some());
        // The button, not the Show choice of the same name.
        click(&mut h, "Never send");
        h.run();
        let choices = vf_fs::clients::load_b2(&root).unwrap().clients;
        assert_eq!(choices.len(), 1, "{choices:?}");
        assert_eq!(choices.values().next().map(String::as_str), Some("never"));
        // Back to the firm's rules.
        click(&mut h, "Use the rules");
        h.run();
        assert!(vf_fs::clients::load_b2(&root).unwrap().clients.is_empty());
    }

    /// Closing the window with annotations unsaved asks first: the window stays, the Viewer
    /// shows the question; Discard and quit closes it (owner 2026-11-04).
    #[test]
    fn closing_with_unsaved_annotations_asks_first() {
        use egui::{ViewportCommand, ViewportEvent, ViewportId};
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Clients");
        let req = CreateRequest {
            root: &root,
            name: "Firm",
        };
        drop(session::create(&tmp.path().join("pc1"), "dev-1", &req).unwrap());
        let mut h = harness(&tmp.path().join("pc1b"));
        click(&mut h, "Open existing");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
        click(&mut h, "Open cabinet");
        settle(&mut h);
        let commands = |h: &egui_kittest::Harness<'static, VibeFilesApp>| {
            h.output()
                .viewport_output
                .get(&ViewportId::ROOT)
                .map(|v| v.commands.clone())
                .unwrap_or_default()
        };
        let close = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>| {
            h.input_mut()
                .viewports
                .entry(ViewportId::ROOT)
                .or_default()
                .events
                .push(ViewportEvent::Close);
            h.step();
        };
        // Nothing unsaved: not held.
        close(&mut h);
        assert!(
            !commands(&h)
                .iter()
                .any(|c| matches!(c, ViewportCommand::CancelClose))
        );
        if let Screen::Open(o) = &mut h.state_mut().screen {
            o.viewer.pending.push(vf_pdf::annot::Annotation {
                page: 0,
                kind: vf_pdf::annot::Kind::Note {
                    at: [72.0, 700.0],
                    text: "Invented note".into(),
                },
                color: vf_pdf::annot::YELLOW,
                author: "KUR".into(),
            });
        }
        close(&mut h);
        assert!(
            commands(&h)
                .iter()
                .any(|c| matches!(c, ViewportCommand::CancelClose))
        );
        h.run_steps(3);
        assert_eq!(h.state().tab, Tab::Viewer);
        h.get_by_label("Discard and quit").click();
        let mut closed = false;
        for _ in 0..4 {
            h.step();
            closed |= commands(&h)
                .iter()
                .any(|c| matches!(c, ViewportCommand::Close));
        }
        assert!(closed, "the window closes after Discard and quit");
    }

    /// Owner 2026-11-11: with a portal set up, the header's Portal button (left of Tip) opens
    /// the portal's staff sign-in page.
    #[test]
    fn portal_button_opens_the_staff_page() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Clients");
        let req = CreateRequest {
            root: &root,
            name: "Firm",
        };
        let o = session::create(&tmp.path().join("pc1"), "dev-1", &req).unwrap();
        vf_portal::settings::save(
            &root,
            &vf_portal::settings::PortalSettings {
                version: 1,
                cabinet_id: o.cabinet.cabinet_id.clone(),
                url: "https://files.example-cpa.test".into(),
                key: "vpk_test".into(),
                fetch_minutes: 0,
                record: Default::default(),
                b2_list: false,
                todo_lists: false,
                balances: false,
                approve_share: Default::default(),
            },
        )
        .unwrap();
        drop(o);
        let mut h = harness(&tmp.path().join("pc1b"));
        click(&mut h, "Open existing");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
        click(&mut h, "Open cabinet");
        settle(&mut h);
        click(&mut h, "Portal");
        assert_eq!(
            h.state().portal_opened.as_deref(),
            Some("https://files.example-cpa.test/staff")
        );
    }

    /// PA-25: F1 (or Help) opens the article about the tab shown; searching and links move
    /// between articles, Back returns.
    #[test]
    fn help_opens_on_the_tab_and_moves_between_articles() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Clients");
        let req = CreateRequest {
            root: &root,
            name: "Firm",
        };
        drop(session::create(&tmp.path().join("pc1"), "dev-1", &req).unwrap());
        let mut h = harness(&tmp.path().join("pc1b"));
        click(&mut h, "Open existing");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
        click(&mut h, "Open cabinet");
        settle(&mut h);

        h.key_press(egui::Key::F1);
        h.run_steps(3);
        assert!(h.state().help.open);
        assert_eq!(h.state().help.current, Some("browse"));
        let title = crate::help::article("browse").unwrap().title();
        assert!(h.query_all_by_label(title).count() >= 1);

        // Search, then open the ten-key article from the results.
        h.get_by_label("Search help").click();
        h.run_steps(2);
        h.get_by_label("Search help").type_text("tape");
        h.run_steps(2);
        let tape = crate::help::article("ten-key").unwrap().title();
        h.get_all_by_label(tape).next().unwrap().click();
        h.run_steps(3);
        assert_eq!(h.state().help.current, Some("ten-key"));
        click(&mut h, "Back");
        h.run_steps(2);
        assert_eq!(h.state().help.current, Some("browse"));

        // No portal set up: no Portal button (owner 2026-11-11).
        assert!(h.query_by_label("Portal").is_none());
        // The tip button (owner 2026-11-04): pressed, the Ko-fi page is opened (not in tests).
        click(&mut h, "Tip VibeMyFirm");
        assert!(h.state().tip_clicked);
        assert_eq!(crate::shell::TIP_URL, "https://ko-fi.com/vibemyfirm");
        assert!(crate::shell::open_url("file:///C:/x.exe").is_err());

        // On another tab, the Help button opens that tab's article.
        h.state_mut().help.open = false;
        click(&mut h, "Viewer");
        click(&mut h, "Help");
        h.run_steps(3);
        assert_eq!(h.state().help.current, Some("viewer"));
    }

    /// Waits for `ok` on Intake's Preview, pressing **Preview** again every few seconds: a
    /// press on a frame that is being drawn again is lost (one run in ten of the whole suite
    /// waited for ever after a single press). If it never comes, says whether the row for
    /// `file` was on the screen at all (the press was lost, or the plan never came).
    fn preview_until(
        h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
        what: &str,
        file: &str,
        ok: &dyn Fn(&egui_kittest::Harness<'static, VibeFilesApp>) -> bool,
    ) {
        let start = std::time::Instant::now();
        let mut pressed = start;
        while !ok(h) {
            assert!(
                start.elapsed() < crate::ui::test_util::PATIENCE,
                "{what}: never. Row for {file} on screen: {}; Preview tab on screen: {}",
                h.query_all_by_label_contains(file).next().is_some(),
                h.query_by_label("Preview").is_some()
            );
            if pressed.elapsed() > std::time::Duration::from_secs(3) {
                if let Some(p) = h.query_by_label("Preview") {
                    p.click();
                }
                pressed = std::time::Instant::now();
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
            h.step();
        }
    }

    /// Intake › Setup is behind the admin password (owner 2026-11-04); a file whose client is
    /// not found offers "New client…": Vibe's New client dialog creates the client and the row
    /// gets it.
    #[test]
    fn intake_setup_locks_and_a_missing_client_is_created_from_the_row() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Clients");
        let inbox = tmp.path().join("Inbox");
        std::fs::create_dir_all(&inbox).unwrap();
        let req = CreateRequest {
            root: &root,
            name: "Firm",
        };
        drop(session::create(&tmp.path().join("pc1"), "dev-1", &req).unwrap());
        let master = vf_fs::clients::master_dir(&root);
        let (mut fs, _) = filer_intake::config::Settings::load_or_default(&master);
        fs.inboxes = vec![inbox.clone()];
        fs.save(&master).unwrap();
        filer_pdf::pdf::write_test_pdf(&inbox.join("Newco LLC_NEWC2002_2025 W-2.pdf"), &["W-2"]);
        let mut h = harness(&tmp.path().join("pc1b"));
        click(&mut h, "Open existing");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
        click(&mut h, "Open cabinet");
        settle(&mut h);
        if let Screen::Open(o) = &mut h.state_mut().screen {
            vf_fs::trash::set_pin(&mut o.opened.conn, &root, "dev-1", None, "482193").unwrap();
        }
        h.state_mut().tab = Tab::Intake;
        h.run_steps(10);
        h.get_by_label("Setup").click();
        // (Waited for, not counted in frames: Filer's screen loads on its own threads, and
        // a busy machine took more than four frames now and then.)
        let start = std::time::Instant::now();
        while h
            .query_by_label_contains("locked by the admin password")
            .is_none()
        {
            assert!(
                start.elapsed() < crate::ui::test_util::PATIENCE,
                "Setup locked"
            );
            std::thread::sleep(std::time::Duration::from_millis(30));
            h.step();
        }
        // The row with no client: New client… opens Vibe's dialog, filled from the name.
        h.get_by_label("Preview").click();
        let label = "New client for Newco LLC_NEWC2002_2025 W-2.pdf";
        preview_until(&mut h, "row planned", "Newco LLC_NEWC2002", &|h| {
            h.query_by_label(label).is_some()
        });
        // Pressed until the dialog is there (a press that lands while the row is still
        // being planned again does nothing; seen on a busy machine).
        let start = std::time::Instant::now();
        while h
            .query_by_label("Create client (one transaction)")
            .is_none()
        {
            assert!(
                start.elapsed() < crate::ui::test_util::PATIENCE,
                "New client dialog"
            );
            if let Some(b) = h.query_by_label(label) {
                b.click();
            }
            h.run_steps(4);
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        h.get_by_label("Create client (one transaction)").click();
        h.run_steps(4);
        let start = std::time::Instant::now();
        loop {
            h.step();
            let Screen::Open(o) = &h.state().screen else {
                unreachable!()
            };
            let made: Option<String> = o
                .opened
                .conn
                .query_row(
                    "SELECT folder_rel FROM client WHERE ext_id = 'NEWC2002'",
                    [],
                    |r| r.get(0),
                )
                .ok();
            if let Some(made) = &made
                && h.query_by_label(label).is_none()
            {
                assert!(root.join(made).is_dir(), "the client's folder");
                break;
            }
            assert!(
                start.elapsed() < crate::ui::test_util::PATIENCE,
                "the row got its new client"
            );
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
    }

    /// Q-61 (owner 2026-11-05): a file in Intake's inbox whose name no rule names has its form
    /// read (the row shows "read"; filing it will set the doc type and year); right-click ›
    /// Move to trash takes a file into the cabinet trash, restorable to the inbox.
    #[test]
    fn intake_reads_unnamed_forms_and_trashes_a_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Clients");
        let inbox = tmp.path().join("Inbox");
        std::fs::create_dir_all(&inbox).unwrap();
        let req = CreateRequest {
            root: &root,
            name: "Firm",
        };
        drop(session::create(&tmp.path().join("pc1"), "dev-1", &req).unwrap());
        let master = vf_fs::clients::master_dir(&root);
        let (mut fs, _) = filer_intake::config::Settings::load_or_default(&master);
        fs.inboxes = vec![inbox.clone()];
        fs.save(&master).unwrap();
        let form = inbox.join("scan0001.pdf");
        let junk = inbox.join("junk.pdf");
        filer_pdf::pdf::write_test_pdf(&form, &["Form W-2 Wage and Tax Statement 2025"]);
        filer_pdf::pdf::write_test_pdf(&junk, &["nothing to see"]);
        let mut h = harness(&tmp.path().join("pc1b"));
        click(&mut h, "Open existing");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
        click(&mut h, "Open cabinet");
        settle(&mut h);
        h.state_mut().tab = Tab::Intake;
        h.run_steps(10);
        h.get_by_label("Preview").click();
        let wait =
            |h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
             what: &str,
             ok: &dyn Fn(&egui_kittest::Harness<'static, VibeFilesApp>) -> bool| {
                let start = std::time::Instant::now();
                while !ok(h) {
                    assert!(start.elapsed() < crate::ui::test_util::PATIENCE, "{what}");
                    std::thread::sleep(std::time::Duration::from_millis(30));
                    h.step();
                }
            };
        preview_until(&mut h, "the form read", "scan0001.pdf", &|h| {
            h.query_by_label("read").is_some()
        });
        let sha = filer_intake::commit::sha256_file(&form).unwrap();
        let hint = vf_fs::hints::load(&root)
            .get(&sha)
            .cloned()
            .expect("hint kept");
        assert_eq!(hint.tax_year, Some(2025));
        assert!(hint.extract.is_some_and(|x| x.form == "w2"));

        // Move to trash: out of the inbox, into the cabinet trash.
        wait(&mut h, "junk listed", &|h| {
            h.query_by_label("junk.pdf").is_some()
        });
        h.get_by_label("junk.pdf").click_secondary();
        h.run_steps(3);
        h.get_by_label("Move to trash").click();
        h.run_steps(3);
        wait(&mut h, "trashed", &|_| !junk.exists());
        let Screen::Open(o) = &h.state().screen else {
            unreachable!()
        };
        let listed = vf_fs::trash::list(&o.opened.conn, &root).unwrap();
        assert!(listed.iter().any(|t| t.rel_path.ends_with("junk.pdf")
            && t.reason.as_deref() == Some(vf_fs::trash::FROM_INTAKE)));
    }

    /// Q-64 (owner 2026-11-05): a person's own inbox, kept for their Windows login only, gives
    /// Intake a My inbox tab with its own queue; Move to trash from it restores back there.
    #[test]
    fn intake_has_a_my_inbox_tab_for_this_login() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Clients");
        let inbox = tmp.path().join("Inbox");
        let mine = tmp.path().join("Mine");
        std::fs::create_dir_all(&inbox).unwrap();
        std::fs::create_dir_all(&mine).unwrap();
        let req = CreateRequest {
            root: &root,
            name: "Firm",
        };
        drop(session::create(&tmp.path().join("pc1"), "dev-1", &req).unwrap());
        let master = vf_fs::clients::master_dir(&root);
        let (mut fs, _) = filer_intake::config::Settings::load_or_default(&master);
        fs.inboxes = vec![inbox.clone()];
        fs.save(&master).unwrap();
        filer_pdf::pdf::write_test_pdf(&inbox.join("shared.pdf"), &["x"]);
        let own = mine.join("own.pdf");
        filer_pdf::pdf::write_test_pdf(&own, &["y"]);
        let mut h = harness(&tmp.path().join("pc1b"));
        let login = vf_core::windows_login().unwrap_or_default();
        h.state_mut()
            .config
            .ingest
            .set_my_inboxes(&login, vec![mine.clone()]);
        click(&mut h, "Open existing");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
        click(&mut h, "Open cabinet");
        settle(&mut h);
        h.state_mut().tab = Tab::Intake;
        h.run_steps(10);
        h.get_by_label("Preview").click();
        let wait =
            |h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
             what: &str,
             ok: &dyn Fn(&egui_kittest::Harness<'static, VibeFilesApp>) -> bool| {
                let start = std::time::Instant::now();
                while !ok(h) {
                    assert!(start.elapsed() < crate::ui::test_util::PATIENCE, "{what}");
                    std::thread::sleep(std::time::Duration::from_millis(30));
                    h.step();
                }
            };
        preview_until(&mut h, "both tabs", "pdf", &|h| {
            h.query_by_label("My inbox (1)").is_some()
        });
        assert!(h.query_by_label("Shared inbox (1)").is_some());
        assert!(
            h.query_by_label("own.pdf").is_none(),
            "not on the shared tab"
        );
        // The tab pressed until its file is listed (a press that lands while the tabs are
        // still being drawn again does nothing; seen on a busy machine).
        let start = std::time::Instant::now();
        while h.query_by_label("own.pdf").is_none() {
            assert!(
                start.elapsed() < crate::ui::test_util::PATIENCE,
                "my file listed"
            );
            if let Some(tab) = h.query_by_label("My inbox (1)") {
                tab.click();
            }
            h.run_steps(3);
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        assert!(h.query_by_label("shared.pdf").is_none());
        h.get_by_label("own.pdf").click_secondary();
        h.run_steps(3);
        h.get_by_label("Move to trash").click();
        h.run_steps(3);
        wait(&mut h, "trashed", &|_| !own.exists());
        let Screen::Open(o) = &mut h.state_mut().screen else {
            unreachable!()
        };
        let it = vf_fs::trash::list(&o.opened.conn, &root)
            .unwrap()
            .into_iter()
            .find(|t| t.rel_path.ends_with("own.pdf"))
            .unwrap();
        assert_eq!(it.reason.as_deref(), Some(vf_fs::trash::FROM_MY_INBOX));
        vf_fs::trash::restore(&mut o.opened.conn, &root, "dev-1", &[it]).unwrap();
        assert!(own.is_file(), "back in my inbox");
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
        };
        drop(session::create(&tmp.path().join("pc1"), "dev-1", &req).unwrap());
        let mut h = harness(&tmp.path().join("pc1b"));
        click(&mut h, "Open existing");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
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
        };
        drop(session::create(&tmp.path().join("pc1"), "dev-1", &req).unwrap());
        let master = vf_fs::clients::master_dir(&root);
        let (mut fs, _) = filer_intake::config::Settings::load_or_default(&master);
        fs.inboxes = vec![inbox.clone()];
        fs.save(&master).unwrap();

        let data = tmp.path().join("pc1b");
        let mut h = harness(&data);
        h.state_mut().config.ingest.scans = Some(scans.clone());
        click(&mut h, "Open existing");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
        click(&mut h, "Open cabinet");
        settle(&mut h);

        filer_pdf::pdf::write_test_pdf(&scans.join("Acme_1001_2025 W-2.pdf"), &["W-2 2025"]);
        let start = std::time::Instant::now();
        let delivered = inbox.join("Acme_1001_2025 W-2.pdf");
        // Counted while it waits in Scans, and again once it is in Filer's inbox.
        // The original is moved aside just after the file reaches the inbox.
        let original = scans.join("Acme_1001_2025 W-2.pdf");
        let counted = |h: &egui_kittest::Harness<'static, VibeFilesApp>| match &h.state().screen {
            Screen::Open(o) => o.intake.ingest.delivered == 1,
            _ => false,
        };
        // The worker's event can come a frame after the file lands: wait for both.
        while !(delivered.is_file()
            && !original.exists()
            && counted(&h)
            && h.query_by_label("Intake (1)").is_some())
        {
            assert!(
                start.elapsed() < crate::ui::test_util::PATIENCE,
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

    /// PHASES P03 done-when (before filing): a scan whose content is already in the cabinet is
    /// flagged in the Intake strip once it reaches Filer's inbox, and can be taken out again.
    #[test]
    fn scanned_duplicate_is_flagged_before_filer_files_it() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Clients");
        let (scans, inbox) = (tmp.path().join("Scans"), tmp.path().join("Inbox"));
        std::fs::create_dir_all(root.join("Old")).unwrap();
        std::fs::create_dir_all(&scans).unwrap();
        std::fs::create_dir_all(&inbox).unwrap();
        filer_pdf::pdf::write_test_pdf(&root.join("Old/receipt.pdf"), &["Receipt 42"]);
        let req = CreateRequest {
            root: &root,
            name: "Firm",
        };
        drop(session::create(&tmp.path().join("pc1"), "dev-1", &req).unwrap());
        let master = vf_fs::clients::master_dir(&root);
        let (mut fs, _) = filer_intake::config::Settings::load_or_default(&master);
        fs.inboxes = vec![inbox.clone()];
        fs.save(&master).unwrap();

        let mut h = harness(&tmp.path().join("pc1b"));
        h.state_mut().config.ingest.scans = Some(scans.clone());
        click(&mut h, "Open existing");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
        click(&mut h, "Open cabinet");
        settle(&mut h);
        click(&mut h, "Intake");

        std::fs::copy(root.join("Old/receipt.pdf"), scans.join("receipt scan.pdf")).unwrap();
        let start = std::time::Instant::now();
        while h
            .query_by_label_contains("is already in the cabinet as Old/receipt.pdf")
            .is_none()
        {
            if start.elapsed() > crate::ui::test_util::PATIENCE {
                let Screen::Open(o) = &h.state().screen else {
                    unreachable!()
                };
                let docs: Vec<String> = o
                    .browse
                    .snap
                    .docs
                    .iter()
                    .map(|d| d.rel_path.clone())
                    .collect();
                panic!(
                    "never flagged: inbox {:?}, docs {docs:?}, log {:?}, pre {:?}",
                    std::fs::read_dir(&inbox)
                        .unwrap()
                        .map(|e| e.unwrap().file_name())
                        .collect::<Vec<_>>(),
                    o.intake.ingest.log,
                    o.intake.ingest.pre_dupes
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
            h.step();
        }
        assert!(inbox.join("receipt scan.pdf").is_file());
        h.get_by_label("Remove from inbox").click();
        h.step();
        h.step();
        assert!(!inbox.join("receipt scan.pdf").exists());
        let kept: Vec<_> = std::fs::read_dir(scans.join("_duplicates"))
            .unwrap()
            .map(|d| d.unwrap().path().join("receipt scan.pdf"))
            .collect();
        assert!(kept.len() == 1 && kept[0].is_file(), "{kept:?}");
        assert!(
            h.query_by_label_contains("is already in the cabinet")
                .is_none()
        );
    }

    /// Q-116 (owner 2026-11-15): a scan has no text, so the ten-key tape finds nothing to pick.
    /// OCR › This page reads it, the text is saved into the PDF as a new version (the scan is
    /// kept), and the amounts can be picked. A page that has text is not read again.
    #[test]
    fn ocr_gives_a_scanned_page_text_to_pick() {
        /// Two amounts, wherever the picture is.
        struct TwoAmounts;
        impl vf_ocr::OcrEngine for TwoAmounts {
            fn name(&self) -> &str {
                "test"
            }
            fn recognize(
                &self,
                page: &image::GrayImage,
                _dpi: u32,
            ) -> Result<vf_ocr::OcrPage, vf_ocr::OcrError> {
                let (w, h) = (page.width(), page.height());
                let word = |text: &str, line: u32| vf_ocr::Word {
                    bbox: [w / 10, h / 10 * (line + 1), w / 5, h / 40],
                    text: text.into(),
                    conf: 95.0,
                    line,
                };
                Ok(vf_ocr::OcrPage {
                    width: w,
                    height: h,
                    words: vec![word("1,284.17", 0), word("45.00", 1)],
                })
            }
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Clients");
        std::fs::create_dir_all(root.join("Old")).unwrap();
        // A scan: one page that is a picture and nothing else.
        vf_ingest::images::frames_to_pdf(
            vec![vf_ingest::images::Frame {
                rgb: image::RgbImage::from_pixel(850, 1100, image::Rgb([255, 255, 255])),
                dpi: Some(100.0),
                orientation: 1,
            }],
            false,
            &root.join("Old/scan.pdf"),
        )
        .unwrap();
        let req = CreateRequest {
            root: &root,
            name: "Firm",
        };
        drop(session::create(&tmp.path().join("pc1"), "dev-1", &req).unwrap());
        let mut h = harness(&tmp.path().join("pc1"));
        click(&mut h, "Open existing");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
        click(&mut h, "Open cabinet");
        settle(&mut h);
        h.get_by_label("scan.pdf").click();
        h.run_steps(4);
        h.key_press(egui::Key::Enter);
        h.step();
        h.step();
        fn viewer<'a>(
            h: &'a mut egui_kittest::Harness<'static, VibeFilesApp>,
        ) -> &'a mut crate::ui::viewer::Viewer {
            let Screen::Open(o) = &mut h.state_mut().screen else {
                unreachable!()
            };
            &mut o.viewer
        }
        let wait = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
                    what: &str,
                    ok: &dyn Fn(&mut crate::ui::viewer::Viewer) -> bool| {
            let start = std::time::Instant::now();
            loop {
                h.step();
                if ok(viewer(h)) {
                    break;
                }
                assert!(start.elapsed() < crate::ui::test_util::PATIENCE, "{what}");
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        };
        wait(&mut h, "never rendered", &|v| v.ready());
        viewer(&mut h).ocr_engine = Some(std::sync::Arc::new(TwoAmounts));
        // Nothing to pick on the scan, and the tape says why.
        click(&mut h, "10-key");
        viewer(&mut h).tape.pick = true;
        wait(&mut h, "the words never came", &|v| {
            v.words_known_for_test(0)
        });
        assert_eq!(viewer(&mut h).pick_amounts_for_test(0), 0);
        h.run_steps(3);
        assert!(
            h.query_by_label_contains("no text to pick from").is_some(),
            "the tape says the page is a scan"
        );
        let versions = |h: &egui_kittest::Harness<'static, VibeFilesApp>| -> i64 {
            let Screen::Open(o) = &h.state().screen else {
                unreachable!()
            };
            o.opened
                .conn
                .query_row("SELECT count(*) FROM version", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(versions(&h), 0);
        click(&mut h, "OCR this page");
        h.run_steps(4);
        assert_eq!(versions(&h), 1, "the scan is kept as the previous version");
        wait(&mut h, "no words after OCR", &|v| {
            v.pick_amounts_for_test(0) >= 2
        });
        assert_eq!(viewer(&mut h).tape.tape.lines().len(), 2);
        let by: String = {
            let Screen::Open(o) = &h.state().screen else {
                unreachable!()
            };
            o.opened
                .conn
                .query_row("SELECT created_by FROM version", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(by, vf_fs::cabinet::TEXT_LAYER_VERSION_BY);
        // Again, from the toolbar's menu: the page has its text, nothing is written.
        click(&mut h, "OCR");
        h.run_steps(2);
        click(&mut h, "Whole document");
        h.run_steps(4);
        assert_eq!(versions(&h), 1);
        assert!(
            h.query_by_label_contains("already has text").is_some(),
            "said, not read twice"
        );
    }

    /// Owner 2026-11-16: an amount on the ten-key tape is clicked, typed over, and the tape
    /// is worked out again; a total is not typed over; text that is no amount changes nothing.
    #[test]
    fn ten_key_amount_is_changed_and_the_tape_recalculated() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Clients");
        std::fs::create_dir_all(root.join("Old")).unwrap();
        filer_pdf::pdf::write_test_pdf(&root.join("Old/interest.pdf"), &["Interest income"]);
        let req = CreateRequest {
            root: &root,
            name: "Firm",
        };
        drop(session::create(&tmp.path().join("pc1"), "dev-1", &req).unwrap());
        let mut h = harness(&tmp.path().join("pc1"));
        click(&mut h, "Open existing");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
        click(&mut h, "Open cabinet");
        settle(&mut h);
        h.get_by_label("interest.pdf").click();
        h.run_steps(4);
        h.key_press(egui::Key::Enter);
        let start = std::time::Instant::now();
        loop {
            h.step();
            let Screen::Open(o) = &h.state().screen else {
                unreachable!()
            };
            if o.viewer.ready() {
                break;
            }
            assert!(
                start.elapsed() < crate::ui::test_util::PATIENCE,
                "never rendered"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        click(&mut h, "10-key");
        if let Screen::Open(o) = &mut h.state_mut().screen {
            o.viewer.tape.tape.keys("10 + 20 + T");
        }
        h.run_steps(3);
        let amounts = |h: &egui_kittest::Harness<'static, VibeFilesApp>| -> Vec<i64> {
            let Screen::Open(o) = &h.state().screen else {
                unreachable!()
            };
            o.viewer
                .tape
                .tape
                .lines()
                .iter()
                .map(|l| l.amount)
                .collect()
        };
        assert_eq!(amounts(&h), [1000, 2000, 3000]);
        // Click the amount, type the new one, Enter: the total follows.
        h.get_by_label("Amount 20.00 of line 2").click();
        h.run_steps(3);
        h.get_by_label("Amount of line 2").type_text("25.50");
        h.step();
        h.key_press(egui::Key::Enter);
        h.run_steps(3);
        assert_eq!(amounts(&h), [1000, 2550, 3550]);
        assert!(
            h.query_by_label("Amount of line 2").is_none(),
            "the box is gone"
        );
        // The total is worked out, not typed: a click on it opens no box.
        h.get_by_label("Amount 35.50 of line 3").click();
        h.run_steps(3);
        assert!(h.query_by_label("Amount of line 3").is_none());
        // Not an amount: said, and the line is as it was. Esc gives a change up.
        h.get_by_label("Amount 10.00 of line 1").click();
        h.run_steps(3);
        h.get_by_label("Amount of line 1").type_text("ten");
        h.step();
        h.key_press(egui::Key::Enter);
        h.run_steps(3);
        assert_eq!(amounts(&h), [1000, 2550, 3550]);
        assert!(h.query_by_label_contains("is not an amount").is_some());
        h.get_by_label("Amount 10.00 of line 1").click();
        h.run_steps(3);
        h.get_by_label("Amount of line 1").type_text("99");
        h.step();
        h.key_press(egui::Key::Escape);
        h.run_steps(3);
        assert_eq!(amounts(&h), [1000, 2550, 3550]);
        // QA 2026-11-17: Undo pressed with a box open takes the change back, and the box's
        // text is not written onto the tape that came back.
        h.get_by_label("Amount 25.50 of line 2").click();
        h.run_steps(3);
        assert!(h.query_by_label("Amount of line 2").is_some());
        let undo: Vec<_> = h.query_all_by_label("Undo").collect();
        assert_eq!(undo.len(), 1, "the tape's Undo");
        undo[0].click();
        h.run_steps(3);
        assert_eq!(amounts(&h), [1000, 2000, 3000]);
        assert!(
            h.query_by_label("Amount of line 2").is_none(),
            "the box is gone"
        );
        // An emptied box changes nothing and says nothing.
        if let Screen::Open(o) = &mut h.state_mut().screen {
            o.viewer.error = None;
        }
        h.get_by_label("Amount 20.00 of line 2").click();
        h.run_steps(3);
        h.key_press(egui::Key::Backspace);
        h.key_press(egui::Key::Enter);
        h.run_steps(3);
        assert_eq!(amounts(&h), [1000, 2000, 3000]);
        let Screen::Open(o) = &h.state().screen else {
            unreachable!()
        };
        assert_eq!(o.viewer.error, None);
    }

    /// PA-14: a tape picked off the page, totalled, pasted and saved is a real annotation in
    /// the file, its lines remember their source boxes, and it is kept in the history.
    #[test]
    fn ten_key_tape_picks_pastes_and_saves() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Clients");
        std::fs::create_dir_all(root.join("Old")).unwrap();
        filer_pdf::pdf::write_test_pdf(
            &root.join("Old/interest.pdf"),
            &["Interest income 1,284.17 Bond interest 45.00"],
        );
        let req = CreateRequest {
            root: &root,
            name: "Firm",
        };
        drop(session::create(&tmp.path().join("pc1"), "dev-1", &req).unwrap());
        let mut h = harness(&tmp.path().join("pc1"));
        click(&mut h, "Open existing");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
        click(&mut h, "Open cabinet");
        settle(&mut h);
        h.get_by_label("interest.pdf").click();
        h.run_steps(4); // the preview pane renders on its own thread
        h.key_press(egui::Key::Enter);
        h.step();
        h.step();
        let start = std::time::Instant::now();
        loop {
            h.step();
            let Screen::Open(o) = &h.state().screen else {
                unreachable!()
            };
            if o.viewer.ready() {
                break;
            }
            assert!(
                start.elapsed() < crate::ui::test_util::PATIENCE,
                "never rendered"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        click(&mut h, "10-key");
        h.step();
        h.step();
        if let Screen::Open(o) = &mut h.state_mut().screen {
            o.viewer.tape.pick = true;
            o.viewer.tape.tape.name = "Interest tie-out".into();
        }
        // The page's words arrive from the renderer.
        let start = std::time::Instant::now();
        let picked = loop {
            h.step();
            let Screen::Open(o) = &mut h.state_mut().screen else {
                unreachable!()
            };
            if o.viewer.pick_amounts_for_test(0) >= 2 {
                break o.viewer.tape.tape.lines().len();
            }
            assert!(start.elapsed() < crate::ui::test_util::PATIENCE, "no words");
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert_eq!(picked, 2);
        {
            let Screen::Open(o) = &mut h.state_mut().screen else {
                unreachable!()
            };
            let t = &mut o.viewer.tape.tape;
            assert!(t.lines()[0].source.is_some(), "lines keep their source box");
            assert!(
                t.lines()[0].label.contains("Interest income"),
                "{:?}",
                t.lines()[0].label
            );
            t.press(vf_core::tape::Key::Total);
            assert_eq!(vf_core::tape::format_cents(t.grand()), "1,329.17");
            o.viewer.paste_for_test(0, [300.0, 500.0], "KWK");
            assert_eq!(o.viewer.pending.len(), 1);
        }
        // VF_SCREENS_DIR set: the 10-key tab as it looks (review aid).
        if let Ok(dir) = std::env::var("VF_SCREENS_DIR") {
            h.run_steps(4);
            if let Ok(img) = h.render() {
                let _ = img.save(std::path::Path::new(&dir).join("41-viewer-10-key.png"));
            }
        }
        h.step();
        click(&mut h, "Save");
        h.step();
        let pdf = root.join("Old/interest.pdf");
        let tapes = vf_pdf::annot::read_tapes(&pdf).unwrap();
        assert_eq!(tapes.len(), 1);
        assert!(tapes[0].data.contains("\"source\""), "linked lines");
        assert_eq!(
            vf_fs::tapes::history(&root, "x", "Old/interest.pdf").len(),
            1
        );
        let Screen::Open(o) = &h.state().screen else {
            unreachable!()
        };
        let audited: i64 = o
            .opened
            .conn
            .query_row(
                "SELECT count(*) FROM audit WHERE action = 'tape_paste'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(audited, 1);

        // A second tape pasted on the same spot goes beside the first, never over it (owner
        // 2026-11-04).
        let start = std::time::Instant::now();
        loop {
            h.step();
            let Screen::Open(o) = &h.state().screen else {
                unreachable!()
            };
            if o.viewer.ready() && o.viewer.tape_on_file_for_test() == 1 {
                break;
            }
            assert!(start.elapsed() < crate::ui::test_util::PATIENCE, "reloaded");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if let Screen::Open(o) = &mut h.state_mut().screen {
            o.viewer.paste_for_test(0, [300.0, 500.0], "KWK");
            assert_eq!(o.viewer.pending.len(), 1);
        }
        h.step();
        click(&mut h, "Save");
        h.step();
        let tapes = vf_pdf::annot::read_tapes(&pdf).unwrap();
        assert_eq!(tapes.len(), 2);
        let (a, b) = (tapes[0].rect, tapes[1].rect);
        assert!(
            a[3] <= b[1] || b[3] <= a[1] || a[2] <= b[0] || b[2] <= a[0],
            "the tapes do not overlap: {a:?} {b:?}"
        );
    }

    /// P04 steps 1-3: a PDF opened from Browse shows in the Viewer; Save writes pending
    /// annotations as a new version (old kept); Save as files a new document beside it.
    #[test]
    fn viewer_saves_annotations_as_a_version_and_as_a_new_document() {
        use vf_pdf::annot::{Annotation, Kind, Tick};
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Clients");
        std::fs::create_dir_all(root.join("Old")).unwrap();
        filer_pdf::pdf::write_test_pdf(
            &root.join("Old/letter.pdf"),
            &["Engagement letter page one"],
        );
        let req = CreateRequest {
            root: &root,
            name: "Firm",
        };
        drop(session::create(&tmp.path().join("pc1"), "dev-1", &req).unwrap());
        let mut h = harness(&tmp.path().join("pc1"));
        click(&mut h, "Open existing");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
        click(&mut h, "Open cabinet");
        settle(&mut h);

        // Browse: select the document, Enter opens it in the Viewer.
        h.get_by_label("letter.pdf").click();
        h.run_steps(4); // the preview pane renders on its own thread
        h.key_press(egui::Key::Enter);
        // The Viewer keeps repainting while it loads: steps, not `run`.
        h.step();
        h.step();
        assert_eq!(h.state().tab, Tab::Viewer);
        let start = std::time::Instant::now();
        loop {
            h.step();
            let Screen::Open(o) = &h.state().screen else {
                unreachable!()
            };
            if o.viewer.ready() {
                break;
            }
            assert!(
                start.elapsed() < crate::ui::test_util::PATIENCE,
                "never rendered"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let (doc_id, old_hash) = {
            let Screen::Open(o) = &h.state().screen else {
                unreachable!()
            };
            let doc = o.viewer.doc.as_ref().unwrap();
            let id = doc.doc_id.clone();
            // Cataloged from the folder listing (PA-60): not fingerprinted until needed, so
            // the content the old version must have is what the file holds now.
            let hash: Option<String> = o
                .opened
                .conn
                .query_row("SELECT hash FROM document WHERE id = ?1", [&id], |r| {
                    r.get(0)
                })
                .unwrap();
            let hash =
                hash.unwrap_or_else(|| filer_intake::commit::sha256_file(&doc.path).unwrap());
            (id, hash)
        };
        // Letter tickmarks A-Z, and the tickmark color and initials kept in this PC's
        // settings (owner 2026-11-04).
        h.get_by_label("Z").click();
        h.step();
        {
            let Screen::Open(o) = &mut h.state_mut().screen else {
                unreachable!()
            };
            assert_eq!(
                o.viewer.tool,
                crate::ui::viewer::Tool::Tickmark(Tick::Letter(25))
            );
            o.viewer.tool = crate::ui::viewer::Tool::Select;
        }
        // Tall enough that the tool panel's last list opens below its button.
        h.set_size(egui::vec2(1440.0, 1100.0));
        h.step();
        h.step();
        h.get_by_role_and_label(Role::ComboBox, "Tickmark color")
            .click();
        h.step();
        h.step();
        h.get_by_label("Purple").click();
        h.step();
        h.step();
        let saved = config::load(&tmp.path().join("pc1")).unwrap();
        assert_eq!(saved.viewer.tick_color.as_deref(), Some("Purple"));
        // Tickmark size, kept the same way (owner 2026-11-11).
        h.get_by_role_and_label(Role::ComboBox, "Tickmark size")
            .click();
        h.step();
        h.step();
        h.get_by_label("Small (16 pt)").click();
        h.step();
        h.step();
        let saved = config::load(&tmp.path().join("pc1")).unwrap();
        assert_eq!(saved.viewer.tick_size.as_deref(), Some("Small"));
        // Initials are kept as they are typed: going to another tab without leaving the box
        // does not lose them (owner 2026-11-11).
        let before = saved.viewer.initials();
        type_into(&mut h, "Signed", "Q");
        h.get_by_label("Stamps").click();
        h.step();
        h.step();
        let saved = config::load(&tmp.path().join("pc1")).unwrap();
        assert_eq!(saved.viewer.initials, Some(format!("{before}Q")));
        h.get_by_label("Annotate").click();
        h.step();
        h.step();
        let pend = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, kind| {
            if let Screen::Open(o) = &mut h.state_mut().screen {
                o.viewer.pending.push(Annotation {
                    page: 0,
                    kind,
                    color: vf_pdf::annot::ACCENT,
                    author: "KWK".into(),
                });
            }
        };
        pend(
            &mut h,
            Kind::Tickmark {
                tick: Tick::Check,
                at: [100.0, 600.0],
                size: 28.0,
            },
        );
        pend(
            &mut h,
            Kind::Stamp {
                text: "CLIENT COPY".into(),
                at: [300.0, 700.0],
                size: 16.0,
            },
        );
        h.run();
        h.get_by_label("2 unsaved");
        // A placed tickmark is made one size smaller from the Unsaved list, and dragged to
        // a new place; the stamp stays where it was (owner 2026-11-11).
        h.get_by_label("-").click();
        h.step();
        let (from, to) = {
            let Screen::Open(o) = &h.state().screen else {
                unreachable!()
            };
            assert!(matches!(
                o.viewer.pending[0].kind,
                Kind::Tickmark { size, .. } if size == 22.0
            ));
            let (_, rect, s, g) = o.viewer.drawn_pages[0];
            let from = crate::ui::viewer::screen(rect, s, &g, [100.0, 600.0]);
            (from, from + egui::vec2(60.0, 30.0))
        };
        let pointer = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, e| {
            h.input_mut().events.push(e);
            h.step();
        };
        pointer(&mut h, egui::Event::PointerMoved(from));
        pointer(
            &mut h,
            egui::Event::PointerButton {
                pos: from,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        );
        for k in 1..=6 {
            pointer(
                &mut h,
                egui::Event::PointerMoved(from + (to - from) * (k as f32 / 6.0)),
            );
        }
        pointer(
            &mut h,
            egui::Event::PointerButton {
                pos: to,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        );
        {
            let Screen::Open(o) = &h.state().screen else {
                unreachable!()
            };
            let (_, rect, s, g) = o.viewer.drawn_pages[0];
            let Kind::Tickmark { at, .. } = o.viewer.pending[0].kind else {
                unreachable!()
            };
            let now = crate::ui::viewer::screen(rect, s, &g, at);
            assert!((now - to).length() < 1.0, "moved to {now:?}, wanted {to:?}");
            assert!(matches!(
                o.viewer.pending[1].kind,
                Kind::Stamp { at, .. } if at == [300.0, 700.0]
            ));
            assert_eq!(o.viewer.pending.len(), 2, "a drag adds nothing");
        }
        click(&mut h, "Save");
        h.step();
        let path = root.join("Old/letter.pdf");
        let found = vf_pdf::annot::read(&path).unwrap();
        assert_eq!(found.len(), 2, "written into the file");
        {
            let Screen::Open(o) = &h.state().screen else {
                unreachable!()
            };
            let (hash, kept): (String, i64) = o
                .opened
                .conn
                .query_row(
                    "SELECT d.hash, (SELECT count(*) FROM version v WHERE v.doc_id = d.id
                       AND v.hash = ?2 AND v.created_by = 'annotate')
                     FROM document d WHERE d.id = ?1",
                    [&doc_id, &old_hash],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_ne!(hash, old_hash);
            assert_eq!(kept, 1, "the previous content is a version");
            assert!(o.viewer.pending.is_empty());
            assert!(
                vf_fs::cabinet::version_path(&root, &old_hash).is_file(),
                "old bytes kept"
            );
        }

        // Save as: a new document, the file just saved untouched.
        // (Not compared byte for byte: the OCR worker may add its text layer meanwhile.)
        pend(
            &mut h,
            Kind::Note {
                at: [72.0, 500.0],
                text: "Signed copy".into(),
            },
        );
        h.run_steps(4); // the renderer may still be drawing: fixed steps, not run()
        click(&mut h, "Save as…");
        h.step();
        // The OCR worker may be swapping the file in at this moment: read again shortly.
        let annots = (0..50)
            .find_map(|_| {
                vf_pdf::annot::read(&path).ok().or_else(|| {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    None
                })
            })
            .expect("the saved file reads");
        assert_eq!(annots.len(), 2, "Save as leaves the file just saved alone");
        let copy = root.join("Old/letter (2).pdf");
        assert_eq!(
            vf_pdf::annot::read(&copy).unwrap().len(),
            3,
            "2 saved + the note"
        );
        let Screen::Open(o) = &h.state().screen else {
            unreachable!()
        };
        let tag: String = o
            .opened
            .conn
            .query_row(
                "SELECT t.tag FROM document d JOIN document_tag t ON t.doc_id = d.id
                 WHERE d.rel_path = 'Old/letter (2).pdf'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tag, format!("copy-of:{doc_id}"));
        assert_eq!(
            o.viewer.doc.as_ref().unwrap().name,
            "letter (2).pdf",
            "the copy is open"
        );

        // Step 4 on the copy: Flatten, then PDF/A (each a new version).
        click(&mut h, "Flatten");
        h.step();
        assert!(vf_pdf::annot::read(&copy).unwrap().is_empty(), "burnt in");
        click(&mut h, "PDF/A");
        h.step();
        {
            let Screen::Open(o) = &h.state().screen else {
                unreachable!()
            };
            assert_eq!(o.viewer.error, None);
        }
        assert_eq!(vf_pdf::pdfa::check(&copy).unwrap(), Vec::<String>::new());
        let Screen::Open(o) = &h.state().screen else {
            unreachable!()
        };
        let kept: i64 = o
            .opened
            .conn
            .query_row(
                "SELECT count(*) FROM version v JOIN document d ON d.id = v.doc_id
                 WHERE d.rel_path = 'Old/letter (2).pdf' AND v.created_by IN ('flatten', 'pdfa')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kept, 2, "each step kept the previous version");
        // Close (owner 2026-11-04): asks first with unsaved annotations, then empties the Viewer.
        if let Screen::Open(o) = &mut h.state_mut().screen {
            o.viewer.pending.push(Annotation {
                page: 0,
                kind: Kind::Tickmark {
                    tick: Tick::Letter(0),
                    at: [100.0, 100.0],
                    size: 28.0,
                },
                color: vf_pdf::annot::TICK_GREEN,
                author: "KWK".into(),
            });
        }
        h.step();
        click(&mut h, "Close");
        h.step();
        h.run_steps(4);
        click(&mut h, "Keep editing");
        h.step();
        click(&mut h, "Close");
        h.run_steps(4);
        click(&mut h, "Discard and close");
        h.step();
        let Screen::Open(o) = &h.state().screen else {
            unreachable!()
        };
        assert!(o.viewer.doc.is_none() && o.viewer.pending.is_empty());
        assert_eq!(h.state().tab, Tab::Browse, "closing goes back to Browse");
    }

    /// P04 step 5 through the Viewer: auto-find, apply & verify, the redacted copy opens.
    #[test]
    fn viewer_redacts_an_ssn_into_a_new_document() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Clients");
        std::fs::create_dir_all(root.join("Old")).unwrap();
        vf_fixtures::write_pdf(
            &root.join("Old/W-2.pdf"),
            &[
                "W-2 - tax year 2025".into(),
                "Payer EIN: 07-1234567".into(),
                "Recipient SSN: 987-65-4321".into(),
            ],
        )
        .unwrap();
        let req = CreateRequest {
            root: &root,
            name: "Firm",
        };
        drop(session::create(&tmp.path().join("pc1"), "dev-1", &req).unwrap());
        let mut h = harness(&tmp.path().join("pc1"));
        click(&mut h, "Open existing");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
        click(&mut h, "Open cabinet");
        settle(&mut h);
        h.get_by_label("W-2.pdf").click();
        h.run_steps(4); // the preview pane renders on its own thread
        h.key_press(egui::Key::Enter);
        let wait = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
                    what: &str,
                    done: &dyn Fn(&crate::ui::viewer::Viewer) -> bool| {
            let start = std::time::Instant::now();
            loop {
                h.step();
                if let Screen::Open(o) = &h.state().screen
                    && done(&o.viewer)
                {
                    return;
                }
                assert!(
                    start.elapsed() < crate::ui::test_util::PATIENCE,
                    "never: {what}"
                );
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        };
        wait(&mut h, "rendered", &|v| v.ready());
        h.get_by_label("Redact").click();
        h.step();
        h.get_by_label("Auto-find patterns").click();
        wait(&mut h, "auto-find results", &|v| v.pii_found.is_some());
        {
            let Screen::Open(o) = &h.state().screen else {
                unreachable!()
            };
            let f = o.viewer.pii_found.as_ref().unwrap();
            let picked: Vec<(vf_pdf::pii::Kind, bool)> =
                f.iter().map(|(f, on)| (f.kind, *on)).collect();
            assert_eq!(
                picked,
                [
                    (vf_pdf::pii::Kind::Ssn, true),
                    (vf_pdf::pii::Kind::Ein, false)
                ],
                "SSN checked, EIN left off"
            );
        }
        h.step();
        h.get_by_label("Apply 1 redaction & verify").click();
        h.step();
        h.step();
        let Screen::Open(o) = &h.state().screen else {
            unreachable!()
        };
        assert_eq!(o.viewer.error, None);
        assert_eq!(o.viewer.doc.as_ref().unwrap().name, "W-2 (redacted).pdf");
        let out = root.join("Old/W-2 (redacted).pdf");
        let dir = filer_pdf::pdf::find_pdfium(Path::new("")).unwrap();
        let text = filer_pdf::pdf::page_texts(filer_pdf::pdf::pdfium(&dir).unwrap(), &out)
            .unwrap()
            .join(" ");
        assert!(
            !text.contains("987-65-4321") && text.contains("07-1234567"),
            "{text}"
        );
    }

    /// PA-27: a form filled in the Viewer's Form tab and saved (still fillable), a flattened
    /// copy beside it; pages picked from thumbnails in Pages…, and a page deleted from its
    /// right-click menu.
    #[test]
    fn form_filled_and_pages_picked_in_the_viewer() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Clients");
        std::fs::create_dir_all(root.join("Old")).unwrap();
        vf_pdf::forms::write_test_form(&root.join("Old/form.pdf"));
        // A signed copy (Q-59): a signature field with a value.
        {
            use lopdf::dictionary;
            let p = root.join("Old/signed.pdf");
            vf_pdf::forms::write_test_form(&p);
            let mut d = lopdf::Document::load(&p).unwrap();
            let sig = d.add_object(dictionary! { "Type" => "Sig" });
            d.add_object(dictionary! {
                "FT" => "Sig", "T" => lopdf::Object::string_literal("sig"), "V" => sig,
            });
            d.save(&p).unwrap();
        }
        filer_pdf::pdf::write_test_pdf(
            &root.join("Old/a.pdf"),
            &["Alpha one", "Alpha two", "Alpha three", "Alpha four"],
        );
        let req = CreateRequest {
            root: &root,
            name: "Firm",
        };
        drop(session::create(&tmp.path().join("pc1"), "dev-1", &req).unwrap());
        let mut h = harness(&tmp.path().join("pc1"));
        click(&mut h, "Open existing");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
        click(&mut h, "Open cabinet");
        settle(&mut h);
        let open = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, name: &str| {
            click(h, "Browse");
            h.run_steps(4);
            h.get_by_label(name).click();
            h.run_steps(4);
            h.key_press(egui::Key::Enter);
            let start = std::time::Instant::now();
            loop {
                h.step();
                if let Screen::Open(o) = &h.state().screen
                    && o.viewer.ready()
                    && o.viewer.doc.as_ref().is_some_and(|d| d.name == name)
                {
                    break;
                }
                assert!(start.elapsed() < crate::ui::test_util::PATIENCE);
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        };
        let value = |name: &str| -> String {
            vf_pdf::forms::read(&root.join("Old/form.pdf"))
                .unwrap()
                .into_iter()
                .find(|f| f.name == name)
                .unwrap()
                .value
        };

        open(&mut h, "form.pdf");
        if let Screen::Open(o) = &mut h.state_mut().screen {
            assert_eq!(o.viewer.fields().len(), 6);
            o.viewer.panel = crate::ui::viewer::Panel::Form;
        }
        h.run_steps(3);
        // VF_SCREENS_DIR set: the screen as it looks (review aid).
        let shot = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, name: &str| {
            if let Ok(dir) = std::env::var("VF_SCREENS_DIR") {
                h.run_steps(6);
                if let Ok(img) = h.render() {
                    let _ = img.save(std::path::Path::new(&dir).join(name));
                }
            }
        };
        // Typed on the page (the same field in the list carries the same name).
        h.get_all_by_label("Client name").next().unwrap().click();
        h.step();
        h.get_all_by_label("Client name")
            .next()
            .unwrap()
            .type_text("Sample Client");
        h.step();
        h.get_by_label("signed").click();
        h.run_steps(2);
        if let Screen::Open(o) = &mut h.state_mut().screen {
            o.viewer.set_field("year", "2025".into());
            o.viewer.set_field("form_id", "changed".into()); // read-only: ignored
            assert_eq!(o.viewer.unsaved(), 3, "{:?}", o.viewer.form);
        }
        shot(&mut h, "42-viewer-form.png");
        click(&mut h, "Save");
        h.run_steps(3);
        if let Screen::Open(o) = &h.state().screen {
            assert_eq!(o.viewer.unsaved(), 0, "{:?}", o.viewer.error);
        }
        assert_eq!(value("client_name"), "Sample Client");
        assert_eq!(value("signed"), "Yes");
        assert_eq!(value("year"), "2025");
        assert_eq!(value("form_id"), "F-100");

        // A flattened copy: a new document, no fields; this one keeps its unsaved entry. (The
        // saved file is read again first: its fields are back once the page shows.)
        let start = std::time::Instant::now();
        loop {
            h.step();
            if let Screen::Open(o) = &h.state().screen
                && o.viewer.ready()
                && o.viewer.fields().len() == 6
            {
                break;
            }
            assert!(start.elapsed() < crate::ui::test_util::PATIENCE);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if let Screen::Open(o) = &mut h.state_mut().screen {
            o.viewer.set_field("client_name", "Other Sample".into());
            assert_eq!(o.viewer.unsaved(), 1);
        }
        h.run_steps(2);
        click(&mut h, "Save flattened copy");
        h.run_steps(2);
        let flat = root.join("Old/form (flattened).pdf");
        assert!(flat.exists());
        assert!(vf_pdf::forms::read(&flat).unwrap().is_empty());
        let burnt = lopdf::Document::load(&flat).unwrap().objects.values().any(|o| {
            matches!(o, lopdf::Object::Stream(s) if String::from_utf8_lossy(&s.content).contains("(Other Sample) Tj"))
        });
        assert!(burnt, "the typed value is on the flattened page");
        assert_eq!(value("client_name"), "Sample Client", "this file unchanged");
        if let Screen::Open(o) = &mut h.state_mut().screen {
            assert_eq!(o.viewer.unsaved(), 1);
            o.viewer.set_field("client_name", "Sample Client".into());
            assert_eq!(
                o.viewer.unsaved(),
                0,
                "typing the saved value back is no change"
            );
            o.viewer.panel = crate::ui::viewer::Panel::Annotate;
        }

        // Pages…: page 1 is picked; click page 3, then page 1 again: only page 3 is left.
        open(&mut h, "a.pdf");
        click(&mut h, "Pages…");
        h.run_steps(3);
        h.get_by_label("Page 3").click();
        h.run_steps(2);
        h.get_by_label("Page 1").click();
        h.run_steps(2);
        if let Screen::Open(o) = &h.state().screen {
            assert_eq!(o.viewer.pages_dialog.as_ref().unwrap().pages, "3");
        }
        shot(&mut h, "43-viewer-pages-picker.png");
        click(&mut h, "Delete pages");
        h.run_steps(3);
        let texts = |p: &std::path::Path| -> Vec<String> {
            let d = lopdf::Document::load(p).unwrap();
            (1..=d.get_pages().len() as u32)
                .map(|n| d.extract_text(&[n]).unwrap_or_default().trim().to_string())
                .collect()
        };
        assert_eq!(
            texts(&root.join("Old/a.pdf")),
            ["Alpha one", "Alpha two", "Alpha four"]
        );

        // Right-click a thumbnail: Delete page 2.
        let start = std::time::Instant::now();
        while h.query_by_label("Page 2 thumbnail").is_none() {
            h.step();
            assert!(start.elapsed() < crate::ui::test_util::PATIENCE);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        h.get_by_label("Page 2 thumbnail").click_secondary();
        h.run_steps(3);
        click(&mut h, "Delete page 2");
        h.run_steps(3);
        assert_eq!(texts(&root.join("Old/a.pdf")), ["Alpha one", "Alpha four"]);

        // A signed PDF: Save asks first; "Save as a new document" leaves the signed file alone.
        open(&mut h, "signed.pdf");
        let before = std::fs::read(root.join("Old/signed.pdf")).unwrap();
        if let Screen::Open(o) = &mut h.state_mut().screen {
            o.viewer.set_field("client_name", "Sample".into());
        }
        h.run_steps(2);
        click(&mut h, "Save");
        h.run_steps(2);
        click(&mut h, "Save as a new document");
        h.run_steps(3);
        assert_eq!(std::fs::read(root.join("Old/signed.pdf")).unwrap(), before);
        if let Screen::Open(o) = &h.state().screen {
            let d = o.viewer.doc.as_ref().unwrap();
            assert_ne!(d.name, "signed.pdf", "the new document is open");
            assert_eq!(
                vf_pdf::forms::read(&d.path)
                    .unwrap()
                    .iter()
                    .find(|f| f.name == "client_name")
                    .unwrap()
                    .value,
                "Sample"
            );
        }
    }

    /// P04 step 6: rotate, extract and split from the Viewer's Pages window, Combine from
    /// Browse; originals untouched except the rotate (a new version).
    #[test]
    fn page_tools_in_the_viewer_and_combine_in_browse() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Clients");
        std::fs::create_dir_all(root.join("Old")).unwrap();
        filer_pdf::pdf::write_test_pdf(
            &root.join("Old/a.pdf"),
            &["Alpha one", "Alpha two", "Alpha three"],
        );
        filer_pdf::pdf::write_test_pdf(&root.join("Old/b.pdf"), &["Bravo one"]);
        let req = CreateRequest {
            root: &root,
            name: "Firm",
        };
        drop(session::create(&tmp.path().join("pc1"), "dev-1", &req).unwrap());
        let mut h = harness(&tmp.path().join("pc1"));
        click(&mut h, "Open existing");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
        click(&mut h, "Open cabinet");
        settle(&mut h);
        // Page text read with lopdf: pdfium is shared with the Viewer's render thread, and
        // opening through it failed now and then under a fully loaded test run.
        let texts = |p: &std::path::Path| -> Vec<String> {
            let d = lopdf::Document::load(p)
                .unwrap_or_else(|e| panic!("cannot open {}: {e}", p.display()));
            let n = d.get_pages().len() as u32;
            (1..=n)
                .map(|i| d.extract_text(&[i]).unwrap_or_default().trim().to_string())
                .collect()
        };

        h.get_by_label("a.pdf").click();
        h.run_steps(4); // the preview pane renders on its own thread
        h.key_press(egui::Key::Enter);
        let start = std::time::Instant::now();
        loop {
            h.step();
            if let Screen::Open(o) = &h.state().screen
                && o.viewer.ready()
            {
                break;
            }
            assert!(start.elapsed() < crate::ui::test_util::PATIENCE);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let open_pages = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, pages: &str| {
            // The last page edit reloads the document on the render thread: wait for it, or
            // the next edit is refused (failed under a loaded test run).
            let start = std::time::Instant::now();
            while !matches!(&h.state().screen, Screen::Open(o) if o.viewer.ready()) {
                assert!(
                    start.elapsed() < crate::ui::test_util::PATIENCE,
                    "viewer ready"
                );
                std::thread::sleep(std::time::Duration::from_millis(20));
                h.step();
            }
            if let Screen::Open(o) = &mut h.state_mut().screen {
                o.viewer.pages_dialog = Some(crate::ui::viewer::PagesDialog {
                    pages: pages.into(),
                    ..Default::default()
                });
            }
            // A new window takes a frame to size itself.
            h.step();
            h.step();
        };
        open_pages(&mut h, "2");
        click(&mut h, "Rotate right");
        h.step();
        let d = lopdf::Document::load(root.join("Old/a.pdf")).unwrap();
        let ids: Vec<_> = d.get_pages().values().copied().collect();
        assert_eq!(vf_pdf::layer::page_rotate(&d, ids[1]), 90);

        open_pages(&mut h, "2-3");
        click(&mut h, "Extract");
        h.step();
        assert_eq!(
            texts(&root.join("Old/a p2-3.pdf")),
            ["Alpha two", "Alpha three"]
        );

        open_pages(&mut h, "3, 1-2");
        click(&mut h, "Split into documents");
        h.step();
        assert_eq!(texts(&root.join("Old/a (part 1).pdf")), ["Alpha three"]);
        assert_eq!(
            texts(&root.join("Old/a (part 2).pdf")),
            ["Alpha one", "Alpha two"]
        );
        assert_eq!(texts(&root.join("Old/a.pdf")).len(), 3, "kept whole");

        // A blank page after page 1 (owner 2026-11-04): a new version with four pages.
        open_pages(&mut h, "1");
        if let Screen::Open(o) = &mut h.state_mut().screen
            && let Some(d) = &mut o.viewer.pages_dialog
        {
            d.blank_after = "1".into();
        }
        click(&mut h, "Insert blank page");
        h.step();
        let t = texts(&root.join("Old/a.pdf"));
        assert_eq!(t.len(), 4, "{t:?}");
        assert_eq!(t[0], "Alpha one");
        assert!(t[1].trim().is_empty(), "the blank page: {t:?}");

        // Browse: select a.pdf and b.pdf, Combine (the preview renders in the background:
        // fixed steps, not run()).
        click(&mut h, "Browse");
        h.run_steps(8);
        if let Screen::Open(o) = &mut h.state_mut().screen {
            let b = &mut o.browse;
            let pick: Vec<usize> = b
                .visible
                .iter()
                .copied()
                .filter(|&i| ["Old/a.pdf", "Old/b.pdf"].contains(&b.snap.docs[i].rel_path.as_str()))
                .collect();
            b.selected = pick.into_iter().collect();
        }
        h.run();
        click(&mut h, "Combine");
        h.run();
        let combined = root.join("Old/Combined a.pdf");
        assert_eq!(
            texts(&combined),
            ["Alpha one", "", "Alpha two", "Alpha three", "Bravo one"]
        );
        let marks = lopdf::Document::load(&combined)
            .unwrap()
            .get_toc()
            .map(|t| t.toc.len())
            .unwrap_or(0);
        assert_eq!(marks, 2, "a bookmark per source");
    }

    /// P05 step 2: the chain is checked when the cabinet opens; a tampered row shows a banner on
    /// every tab until the chain verifies again.
    #[test]
    fn tampered_audit_trail_shows_a_banner() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Clients");
        let req = CreateRequest {
            root: &root,
            name: "Firm",
        };
        drop(session::create(&tmp.path().join("pc1"), "dev-1", &req).unwrap());
        let mut h = harness(&tmp.path().join("pc1"));
        click(&mut h, "Open existing");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
        click(&mut h, "Open cabinet");
        settle(&mut h);
        assert!(h.query_by_label_contains("does not verify").is_none());
        // The rows so far are copied to the cabinet's audit folder.
        let mirrored = std::fs::read_dir(root.join("_Vibe/audit"))
            .map(|d| d.flatten().count())
            .unwrap_or(0);
        assert!(mirrored > 0, "audit JSONL mirror written");
        if let Screen::Open(o) = &mut h.state_mut().screen {
            let c = &o.opened.conn;
            c.execute_batch(
                "DROP TRIGGER audit_no_update;
                 UPDATE audit SET detail = '{\"edited\":true}' WHERE seq = 1;",
            )
            .unwrap();
            // As if a week had passed since the check at open (or, on a PC up for less than
            // a week, check again now).
            match o
                .chain
                .at
                .checked_sub(std::time::Duration::from_secs(8 * 24 * 3600))
            {
                Some(t) => o.chain.at = t,
                None => o.chain = crate::ui::audit::ChainCheck::run(&o.opened.conn),
            }
        }
        h.run();
        assert!(
            h.query_by_label_contains("does not verify at row 1")
                .is_some()
        );
    }

    /// The cabinet this PC opened last opens at start without the wizard (owner 2026-11-04);
    /// "Open a different cabinet" goes back to the wizard.
    #[test]
    fn last_cabinet_opens_without_the_wizard() {
        let tmp = tempfile::tempdir().unwrap();
        let (data, root) = (tmp.path().join("data"), tmp.path().join("Clients"));
        drop(
            session::create(
                &data,
                "dev-1",
                &CreateRequest {
                    root: &root,
                    name: "Test Cabinet",
                },
            )
            .unwrap(),
        );
        let mut c = config::load(&data).unwrap();
        c.last_cabinet = Some(root.clone());
        config::save(&data, &c).unwrap();

        let mut h = harness(&data);
        let ctx = h.ctx.clone();
        h.state_mut().open_last(&ctx);
        assert!(matches!(h.state().screen, Screen::Busy { .. }));
        h.step();
        // The opening line under the spinner changes as the open goes on, and the button
        // moves with it: a click aimed at the frame before can miss (this failed now and
        // then under a full run). Pressed until it takes, or the cabinet has opened.
        for _ in 0..20 {
            if !matches!(h.state().screen, Screen::Busy { .. }) {
                break;
            }
            if h.query_by_label("Open a different cabinet").is_some() {
                click(&mut h, "Open a different cabinet");
            } else {
                h.step();
            }
        }
        assert!(matches!(
            h.state().screen,
            Screen::Wizard(_) | Screen::Open(_)
        ));
        // The abandoned opening lets go of the data folder before the next one.
        std::thread::sleep(std::time::Duration::from_millis(500));
        // And straight through when left alone.
        h.state_mut().open_last(&ctx);
        settle(&mut h);
        assert!(matches!(h.state().screen, Screen::Open(_)));
    }

    /// A login with no cabinet yet opens the firm's, named in firm.toml beside the exe (owner
    /// 2026-11-12), without the opening screen; then it is this login's last cabinet.
    #[test]
    fn a_new_login_opens_the_firms_cabinet() {
        let tmp = tempfile::tempdir().unwrap();
        let (data, root) = (tmp.path().join("data"), tmp.path().join("Clients"));
        let made = tmp.path().join("made");
        drop(
            session::create(
                &made,
                "dev-0",
                &CreateRequest {
                    root: &root,
                    name: "Firm Cabinet",
                },
            )
            .unwrap(),
        );
        let exe = tmp.path().join("app");
        std::fs::create_dir_all(&exe).unwrap();
        std::fs::write(
            exe.join(config::FIRM_FILE),
            format!("cabinet = '{}'\n", root.display()),
        )
        .unwrap();
        crate::ui::test_util::pdfium_first();
        let (d, e) = (data.clone(), exe.clone());
        let mut h = HarnessBuilder::default()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 900.0])
            .build_eframe(move |cc| VibeFilesApp::new_in(&cc.egui_ctx, d.clone(), Some(&e)));
        h.run();
        let ctx = h.ctx.clone();
        h.state_mut().open_last(&ctx);
        settle(&mut h);
        assert!(matches!(h.state().screen, Screen::Open(_)));
        assert_eq!(
            config::load(&data).unwrap().last_cabinet.as_deref(),
            Some(root.as_path())
        );
    }

    /// Dark mode (owner 2026-11-04): the header's button switches the theme, this PC keeps
    /// it, and the theme colors follow.
    #[test]
    fn dark_button_switches_and_keeps_the_theme() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let mut h = harness(&data);
        assert_eq!(h.ctx.theme(), egui::Theme::Light);
        click(&mut h, "Dark");
        h.run();
        assert_eq!(h.ctx.theme(), egui::Theme::Dark);
        assert!(config::load(&data).unwrap().look.dark);
        // The colors follow (the theme flag is per thread: no other test can flip it).
        assert!(crate::ui::theme::is_dark());
        click(&mut h, "Light");
        h.run();
        assert_eq!(h.ctx.theme(), egui::Theme::Light);
        assert!(!config::load(&data).unwrap().look.dark);
        // "Follow Windows" picked with Filer's theme button is kept, not replaced.
        h.ctx.set_theme(egui::ThemePreference::System);
        h.run();
        assert!(config::load(&data).unwrap().look.system);
        assert_eq!(
            h.ctx.options(|o| o.theme_preference),
            egui::ThemePreference::System
        );
        // Vibe's own button chooses explicitly again.
        let label = if h.ctx.theme() == egui::Theme::Dark {
            "Light"
        } else {
            "Dark"
        };
        click(&mut h, label);
        h.run();
        assert!(!config::load(&data).unwrap().look.system);
        assert_ne!(
            h.ctx.options(|o| o.theme_preference),
            egui::ThemePreference::System
        );
    }

    /// A scan started from Browse with a client folder shown is filed straight into it, with
    /// the folder's year (owner 2026-11-04).
    #[test]
    fn browse_scan_is_filed_into_the_folder_shown() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Clients");
        let req = CreateRequest {
            root: &root,
            name: "Firm",
        };
        let mut o = session::create(&tmp.path().join("pc1"), "dev-1", &req).unwrap();
        let p = vf_fs::newclient::plan(
            &o.conn,
            &root,
            &vf_fs::newclient::NewClient {
                partner: String::new(),
                ext_id: "INVE1001".into(),
                name: "Invented Family".into(),
                entity_type: None,
            },
        )
        .unwrap();
        vf_fs::newclient::create(&mut o.conn, &root, &p, "dev-1").unwrap();
        drop(o);
        let mut h = harness(&tmp.path().join("pc1"));
        click(&mut h, "Open existing");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
        click(&mut h, "Open cabinet");
        settle(&mut h);
        // As if a scan from Browse had just finished: its PDF in a private folder.
        let scan_dir = root.join("_Vibe").join("tmp").join("scan-test");
        std::fs::create_dir_all(&scan_dir).unwrap();
        let pdf = scan_dir.join("Scan 2026-11-04 120000.pdf");
        filer_pdf::pdf::write_test_pdf(&pdf, &["Invented scanned page"]);
        let folder = format!("{}/Income Tax/2026", p.folder_rel);
        if let Screen::Open(o) = &mut h.state_mut().screen {
            o.browse.scan_watching = true;
            o.browse.scan_target = Some((
                crate::ui::browse::ScanTarget {
                    client_id: p.client.uuid.to_string(),
                    folder: folder.clone(),
                    tax_year: Some(2026),
                },
                scan_dir.clone(),
            ));
            // An Intake scan finished after it is not taken for it (QA 2026-11-04).
            let other = root.join("Scans");
            o.intake
                .ingest
                .finished_scans
                .push((other.clone(), Ok(other.join("other.pdf"))));
            o.intake
                .ingest
                .finished_scans
                .push((scan_dir.clone(), Ok(pdf.clone())));
        }
        h.run_steps(3);
        let Screen::Open(o) = &h.state().screen else {
            unreachable!()
        };
        let (rel, year): (String, Option<i32>) = o
            .opened
            .conn
            .query_row(
                "SELECT rel_path, tax_year FROM document WHERE rel_path LIKE '%Scan 2026-11-04%'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("filed");
        assert_eq!(rel, format!("{folder}/Scan 2026-11-04 120000.pdf"));
        assert_eq!(year, Some(2026));
        assert!(root.join(&rel).is_file());
        assert!(!scan_dir.exists(), "the private scan folder is cleared");
    }

    /// A relationship saved in Browse reaches Intake in place (owner 2026-11-12, Filer V36):
    /// no Reload Intake bar, Filer not loaded again, its copy of the file the new one.
    #[test]
    fn a_relationship_saved_in_browse_reaches_intake_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Clients");
        let req = CreateRequest {
            root: &root,
            name: "Firm",
        };
        drop(session::create(&tmp.path().join("pc1"), "dev-1", &req).unwrap());
        let mut h = harness(&tmp.path().join("pc1"));
        click(&mut h, "Open existing");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
        click(&mut h, "Open cabinet");
        settle(&mut h);
        click(&mut h, "Intake");
        h.run_steps(8);
        let master = vf_fs::clients::master_dir(&root);
        filer_core::relationships::save(
            &master,
            &filer_core::relationships::Relationships {
                records: vec![filer_core::relationships::Relationship {
                    parent_id: "2002".into(),
                    child_id: "1001".into(),
                    role: "Shareholder".into(),
                    note: String::new(),
                }],
            },
        )
        .unwrap();
        // Intake looks at the shared files every 2 seconds.
        std::thread::sleep(std::time::Duration::from_millis(2100));
        h.run_steps(8);
        let Screen::Open(o) = &h.state().screen else {
            unreachable!()
        };
        assert_eq!(o.intake.mounts, 1, "not loaded again");
        assert!(h.query_by_label("Reload Intake").is_none());
        let pc = vf_fs::intake::pc_dir(&root, &h.state().config.device_id);
        let (rels, _, _) = filer_core::relationships::load(&pc);
        assert_eq!(rels.records.len(), 1, "Intake's copy is the new one");
    }

    /// One Partner choice for Browse and Intake's review (owner 2026-11-12, Filer V38): set in
    /// Browse it filters the review; changed in the review it is Browse's again.
    #[test]
    fn intakes_partner_filter_is_browses_choice() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Clients");
        let req = CreateRequest {
            root: &root,
            name: "Firm",
        };
        drop(session::create(&tmp.path().join("pc1"), "dev-1", &req).unwrap());
        let mut h = harness(&tmp.path().join("pc1"));
        click(&mut h, "Open existing");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
        click(&mut h, "Open cabinet");
        settle(&mut h);
        if let Screen::Open(o) = &mut h.state_mut().screen {
            o.browse.set_partner(Some("KW".into()));
        }
        click(&mut h, "Intake");
        h.run_steps(8);
        // The review's Partner box shows Browse's choice.
        h.get_by_role_and_label(Role::ComboBox, "Partner").click();
        h.run_steps(3);
        h.get_by_label("All partners").click();
        // Waited for and pressed again, not counted in frames: a press on a frame that is
        // being drawn again is lost (one run in twenty of the whole suite).
        let start = std::time::Instant::now();
        let mut pressed = start;
        let partner = |h: &egui_kittest::Harness<'static, VibeFilesApp>| match &h.state().screen {
            Screen::Open(o) => o.browse.prefs.partner.clone(),
            _ => unreachable!(),
        };
        while partner(&h).is_some() {
            assert!(
                start.elapsed() < crate::ui::test_util::PATIENCE,
                "Browse follows the review"
            );
            if pressed.elapsed() > std::time::Duration::from_secs(2) {
                if h.query_by_label("All partners").is_none() {
                    h.get_by_role_and_label(Role::ComboBox, "Partner").click();
                    h.run_steps(3);
                }
                if let Some(a) = h.query_by_label("All partners") {
                    a.click();
                }
                pressed = std::time::Instant::now();
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
            h.step();
        }
    }

    /// Filer's screen loads once on the Intake tab, not on every frame (regression found
    /// 2026-11-04); what the removed check-out build left is cleared at open (Q-52).
    #[test]
    fn intake_loads_once() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Clients");
        let req = CreateRequest {
            root: &root,
            name: "Firm",
        };
        let o = session::create(&tmp.path().join("pc1"), "dev-1", &req).unwrap();
        // What the removed check-out build could leave (Q-52).
        o.conn
            .execute_batch(
                "INSERT INTO document (id, rel_path, hash, size, filed_at, device_id, updated_at)
                   VALUES ('d1', 'A/x.pdf', 'h', 1, 't', 'dev-1', 't');
                 INSERT INTO lock (doc_id, user, device_id, since) VALUES ('d1', 'anna on PC2', 'pc2', 't');",
            )
            .unwrap();
        drop(o);
        let vibe = root.join("_Vibe");
        std::fs::create_dir_all(vibe.join("locks")).unwrap();
        std::fs::write(vibe.join("locks").join("d1.lock"), "{}").unwrap();
        std::fs::write(vibe.join("intake.lock"), "{}").unwrap();
        let mut h = harness(&tmp.path().join("pc1"));
        click(&mut h, "Open existing");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
        click(&mut h, "Open cabinet");
        settle(&mut h);
        click(&mut h, "Intake");
        h.run_steps(8);
        let Screen::Open(o) = &h.state().screen else {
            unreachable!()
        };
        assert_eq!(o.intake.mounts, 1, "{}", o.intake.debug_state());
        // The check-out leftovers are gone: nothing can refuse an edit for a lock no one can
        // release.
        let locks: i64 = o
            .opened
            .conn
            .query_row("SELECT count(*) FROM lock", [], |r| r.get(0))
            .unwrap();
        assert_eq!(locks, 0);
        assert!(!vibe.join("intake.lock").exists());
        assert!(!vibe.join("locks").exists());
    }

    /// PA-24: with an admin password set, Admin and the Sync settings wait for it (wrong ones
    /// refused and audited); Lock Admin locks again.
    #[test]
    fn admin_password_locks_admin_and_sync_settings() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Clients");
        let req = CreateRequest {
            root: &root,
            name: "Firm",
        };
        let mut o = session::create(&tmp.path().join("pc1"), "dev-1", &req).unwrap();
        vf_fs::trash::set_pin(&mut o.conn, &root, "dev-1", None, "invented-pass-1").unwrap();
        drop(o);
        let mut h = harness(&tmp.path().join("pc1"));
        click(&mut h, "Open existing");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
        click(&mut h, "Open cabinet");
        settle(&mut h);
        // Sync: the set-up form waits for the password.
        click(&mut h, "Sync");
        h.run();
        assert!(h.query_by_label("Sync is not set up on this PC.").is_some());
        assert!(h.query_by_label("Connect").is_none());
        // Admin: the lock screen.
        click(&mut h, "Admin");
        h.run();
        assert!(h.query_by_label("Admin is locked").is_some());
        type_into(&mut h, "Admin password", "wrong-password");
        click(&mut h, "Unlock");
        h.run();
        assert!(
            h.query_by_label("That admin password is not right.")
                .is_some()
        );
        type_into(&mut h, "Admin password", "invented-pass-1");
        click(&mut h, "Unlock");
        h.run();
        assert!(h.query_by_label("Admin is locked").is_none());
        assert!(h.query_by_label("Lock Admin").is_some());
        click(&mut h, "Sync");
        h.run();
        assert!(
            h.query_by_label("Connect").is_some(),
            "set-up open once unlocked"
        );
        {
            let Screen::Open(o) = &h.state().screen else {
                unreachable!()
            };
            let n = |a: &str| -> i64 {
                o.opened
                    .conn
                    .query_row("SELECT count(*) FROM audit WHERE action = ?1", [a], |r| {
                        r.get(0)
                    })
                    .unwrap()
            };
            assert_eq!((n("admin_unlock_refused"), n("admin_unlock")), (1, 1));
        }
        click(&mut h, "Admin");
        click(&mut h, "Lock Admin");
        h.run();
        assert!(h.query_by_label("Admin is locked").is_some());
        // PA-64: the admin leaves two pages open; locked, only those are listed.
        type_into(&mut h, "Admin password", "invented-pass-1");
        click(&mut h, "Unlock");
        h.run();
        // The Admin list scrolls: its last pages are below a 900 px window (PA-103).
        h.get_by_role_and_label(Role::Button, "Admin password")
            .scroll_to_me();
        h.run();
        click(&mut h, "Admin password");
        h.run();
        assert!(h.query_by_label("Open while Admin is locked").is_some());
        // The tick box, not the page of the same name in the list.
        use egui_kittest::kittest::NodeT;
        for name in ["Devices", "Audit trail"] {
            let tick = |h: &egui_kittest::Harness<'static, VibeFilesApp>, press: bool| {
                let boxes: Vec<_> = h
                    .query_all_by_label(name)
                    .filter(|n| n.accesskit_node().role() == egui::accesskit::Role::CheckBox)
                    .collect();
                assert_eq!(boxes.len(), 1, "{name}");
                if press {
                    boxes[0].click();
                } else {
                    // The list scrolls: a page low in it is brought into view first.
                    boxes[0].scroll_to_me();
                }
            };
            tick(&h, false);
            h.run();
            tick(&h, true);
            h.run();
        }
        assert_eq!(
            vf_fs::trash::open_pages(&root),
            ["Devices".to_string(), "Audit trail".to_string()]
        );
        // Back to the top of the scrolled list.
        h.get_by_role_and_label(Role::Button, "Lock Admin")
            .scroll_to_me();
        h.run();
        click(&mut h, "Lock Admin");
        h.run();
        assert!(h.query_by_label("Admin is locked").is_none());
        assert!(h.query_by_label("Unlock Admin…").is_some());
        // Devices is the page shown too (its heading), so more than one.
        assert!(h.query_all_by_label("Devices").count() >= 1);
        assert!(h.query_by_label("Audit trail").is_some());
        for hidden in [
            "Doc types (0)",
            "Merge cabinets",
            "Admin password",
            "Lock Admin",
        ] {
            assert!(h.query_by_label(hidden).is_none(), "{hidden}");
        }
        // Unlock Admin… asks for the password; Back returns to the open pages.
        click(&mut h, "Unlock Admin…");
        h.run();
        assert!(h.query_by_label("Admin is locked").is_some());
        click(&mut h, "Back to the open pages");
        h.run();
        assert!(h.query_by_label("Unlock Admin…").is_some());
        click(&mut h, "Unlock Admin…");
        h.run();
        type_into(&mut h, "Admin password", "invented-pass-1");
        click(&mut h, "Unlock");
        h.run();
        assert!(h.query_by_label("Lock Admin").is_some());
        assert!(h.query_by_label("Merge cabinets").is_some());
    }

    /// Text size (owner 2026-11-04): the header's A+ / A- change it and this PC keeps it.
    #[test]
    fn text_size_buttons_change_and_keep_the_size() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let mut h = harness(&data);
        click(&mut h, "A+");
        h.run();
        assert_eq!(h.state().config.look.text_size, 110);
        assert!((h.ctx.zoom_factor() - 1.1).abs() < 0.001);
        assert_eq!(config::load(&data).unwrap().look.text_size, 110);
        click(&mut h, "A-");
        h.run();
        assert_eq!(config::load(&data).unwrap().look.text_size, 100);
    }

    #[test]
    fn wizard_checks_fields_before_any_work() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Clients");
        let mut h = harness(&tmp.path().join("data"));
        click(&mut h, "Create new");
        type_into(&mut h, "Cabinet folder", &root.display().to_string());
        click(&mut h, "Create cabinet");
        h.run();
        let Screen::Wizard(w) = &h.state().screen else {
            panic!("should stay on the wizard");
        };
        assert!(
            w.error
                .as_deref()
                .unwrap()
                .contains("Give the cabinet a name")
        );
        assert!(!root.exists(), "nothing written");
    }
}
