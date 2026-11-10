//! Top-level app: tab routing over the shared chrome. Each tab is a stub until its phase.

use std::path::PathBuf;
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
            Tab::Search => "Search the text of every document.",
            Tab::Viewer => "Open a document from Search.",
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
    /// OCR workers (P03).
    pub ocr: crate::ui::ocr::OcrState,
    pub search: crate::ui::search::SearchTab,
    pub viewer: crate::ui::viewer::Viewer,
    /// OCR's indexed count when Search last re-ran, and when: while OCR works through a
    /// backlog, Search runs again at most every 3 s, not on every document.
    indexed_seen: (usize, Option<std::time::Instant>),
    /// The audit chain check: on open, then weekly (P05 step 2).
    pub chain: crate::ui::audit::ChainCheck,
    /// When new audit rows were last copied to `_Vibe/audit/<month>.jsonl`.
    mirrored_at: Option<std::time::Instant>,
    /// A shared catalog (Q-67): its `data_version` when last seen, and when it was looked at;
    /// another session's change makes Browse read the catalog again.
    data_version: (i64, Option<std::time::Instant>),
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
}

impl OpenCabinet {
    /// What Search's "This client" / "This folder" mean: Browse's selected node.
    pub fn search_context(&self) -> crate::ui::search::Context {
        use vf_core::browse::Scope as B;
        let snap = &self.browse.snap;
        let label = |ci: usize| {
            let c = &snap.clients[ci];
            (c.id.clone(), format!("{} ({})", c.name, c.ext_id))
        };
        let mut cx = crate::ui::search::Context::default();
        match &self.browse.scope {
            B::Client(ci) | B::Year { client: ci, .. } | B::YearFolder { client: ci, .. }
                if *ci < snap.clients.len() =>
            {
                cx.client = Some(label(*ci));
            }
            B::Folder { client, folder } if *client < snap.clients.len() => {
                cx.client = Some(label(*client));
                let base = snap.clients[*client].folder_rel.clone().unwrap_or_default();
                cx.folder = Some((
                    if base.is_empty() {
                        folder.clone()
                    } else {
                        format!("{base}/{folder}")
                    },
                    None,
                    folder.replace('/', " › "),
                ));
            }
            _ => {}
        }
        cx
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
    /// The Help window (PA-25).
    pub help: crate::ui::help::HelpState,
    /// The window is closing after the Viewer's question was answered (not asked again).
    closing: bool,
    /// "Tip VibeMyFirm" was pressed (tests).
    tip_clicked: bool,
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
        let mut w = Wizard::new(config.last_cabinet.as_deref());
        w.error = error;
        let update = check_for_update(config.update_check);
        let mut app = Self {
            tab: Tab::Browse,
            data,
            config,
            screen: Screen::Wizard(w),
            update,
            update_notice: None,
            zoom_applied: false,
            help: Default::default(),
            closing: false,
            tip_clicked: false,
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
        if self.config.last_cabinet.is_some() && w.error.is_none() {
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
        std::thread::spawn(move || {
            let r = match mode {
                Mode::Create => session::create(
                    &data,
                    &device,
                    &CreateRequest {
                        root: &root,
                        name: &name,
                    },
                ),
                Mode::Open => session::open(&data, &device, &root),
            };
            // The rest of opening (it reads and writes the share) here too, not on the UI
            // thread once the cabinet shows (QA A6).
            let r = r.map(|mut o| {
                let p = prepare(&mut o, &device);
                (o, p)
            });
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
                    rules_problem,
                    emptied,
                    chain,
                },
            )) => {
                // A crash between Filer's commit and the catalog write is caught up here.
                let mut intake = IntakeTab::default();
                let root = opened.root.clone();
                let catalog = opened.catalog.clone();
                intake.reconcile(&mut opened.conn, &catalog, &root, &self.config.device_id);
                self.config.last_cabinet = Some(opened.root.clone());
                // The cabinet is open either way; a settings write failure only loses "last".
                let _ = config::save(&self.data, &self.config);
                let mut browse = Browse::load(&opened.conn);
                browse.set_root(&opened.root);
                browse.catalog = Some(opened.catalog.clone());
                browse.sheets_dir = self.data.join("sheets");
                browse.set_prefs(self.config.prefs.clone());
                browse.saved = vf_index::saved::load(&opened.root);
                if let Some(e) = rules_problem {
                    browse.error = Some(format!(
                        "Print capture (Admin › Print capture & parsers): {e}"
                    ));
                }
                browse.notice = opened.rebuild.as_ref().map(|r| match &opened.set_aside {
                    Some(old) => format!(
                        "The catalog is no longer encrypted: the old one was kept as {} and the cabinet indexed again. {r}",
                        old.display()
                    ),
                    None => format!("Indexed: {r}"),
                });
                match &emptied {
                    Ok(n) if *n > 0 => tracing::info!(documents = n, "old trash emptied"),
                    Err(e) => {
                        tracing::warn!(kind = crate::logging::kind(e), "emptying old trash failed")
                    }
                    _ => {}
                }
                match emptied {
                    Ok(0) => {}
                    Ok(n) => {
                        browse.notice = Some(format!(
                            "Emptied {n} documents that were in the trash over {} days.",
                            vf_fs::trash::AUTO_EMPTY_DAYS
                        ))
                    }
                    Err(e) => add_error(&mut browse.error, format!("Emptying old trash: {e}")),
                }
                let ocr = crate::ui::ocr::OcrState::start(
                    ctx,
                    &opened,
                    &self.config.ocr,
                    &self.config.device_id,
                );
                let checks = {
                    let (tx, rx) = mpsc::channel();
                    let (catalog, root, ctx) =
                        (opened.catalog.clone(), opened.root.clone(), ctx.clone());
                    let synced = self.config.sync.configured_for(&opened.cabinet.cabinet_id);
                    let run = move || {
                        vf_core::db::open(&catalog, vf_core::db::Storage::Local)
                            .map(|c| crate::selfcheck::run(&c, &root, synced))
                            .unwrap_or_else(|e| {
                                vec![crate::selfcheck::Check {
                                    name: "Catalog integrity",
                                    ok: false,
                                    detail: e.to_string(),
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
                let mut sweep = crate::ui::sweep::SweepState::default();
                if vf_fs::sweep::due(&opened.conn) {
                    sweep.start(ctx, &opened.catalog, &opened.root);
                }
                self.screen = Screen::Open(Box::new(OpenCabinet {
                    chain,
                    mirrored_at: None,
                    data_version: (0, None),
                    mirror_wait: std::time::Duration::from_secs(2),
                    sweep,
                    sync: Default::default(),
                    servers: Default::default(),
                    checks,
                    opened,
                    browse,
                    admin: None,
                    intake,
                    ocr,
                    search: Default::default(),
                    viewer: Default::default(),
                    indexed_seen: (0, None),
                    admin_unlocked: false,
                    admin_typed: String::new(),
                    admin_msg: None,
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
        chrome::header(
            ui,
            name.as_deref(),
            &mut size,
            &mut dark,
            &mut help,
            &mut tip,
        );
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
            // Another session on this PC changed the shared catalog: read it again (at most
            // every 3 s).
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
                    if o.data_version.0 != 0 && v != o.data_version.0 {
                        o.browse.reload(&o.opened.conn);
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
            let received = o.browse.portal.tick(ui.ctx(), &o.opened.root);
            o.intake.ingest.portal = o.browse.portal.intake_line(&o.opened.root);
            if !received.is_empty() {
                let mut tx = vf_core::tx::Transaction::new("portal_receive");
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
            if let Some(n) = o.browse.portal.notice.take() {
                o.browse.notice = Some(n);
            }
            // The audit trail's copy in the cabinet (CLAUDE.md layout: _Vibe/audit/*.jsonl).
            if o.mirrored_at.is_none_or(|t| t.elapsed() >= o.mirror_wait) {
                o.mirrored_at = Some(std::time::Instant::now());
                let dir = o.opened.root.join("_Vibe").join("audit");
                let r = vf_core::audit::mirror_pending(&o.opened.conn, &dir);
                o.mirror_wait = match r {
                    Ok(_) => std::time::Duration::from_secs(2),
                    Err(_) => (o.mirror_wait * 2).min(std::time::Duration::from_secs(120)),
                };
                if let Err(e) = r {
                    tracing::warn!(
                        kind = crate::logging::kind(&e.to_string()),
                        "audit mirror not written"
                    );
                    o.browse.error = Some(format!(
                        "The audit trail's copy in the cabinet could not be written: {e}"
                    ));
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
        let mut segments = vec![format!("vibe-files {}", env!("CARGO_PKG_VERSION"))];
        if let Screen::Open(o) = &mut self.screen {
            let o = &mut **o;
            segments.insert(0, o.ocr.footer(&o.opened.conn));
            segments.insert(
                0,
                format!(
                    "{} documents · {} clients",
                    o.browse.document_total(),
                    o.browse.snap.clients.len()
                ),
            );
        }
        chrome::footer(ui, &segments, name.as_deref().unwrap_or("No cabinet open"));
        let ctx = ui.ctx().clone();
        let mut submit = None;
        let mut to_wizard = false;
        match &mut self.screen {
            Screen::Open(o) if self.tab == Tab::Intake => {
                let o = &mut **o;
                let setup_locked = !o.admin_unlocked && vf_fs::trash::has_pin(&o.opened.root);
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
                o.intake.reconcile(
                    &mut o.opened.conn,
                    &o.opened.catalog,
                    &o.opened.root,
                    &self.config.device_id,
                );
            }
            Screen::Open(o) if self.tab == Tab::Search => {
                let o = &mut **o;
                let cx = o.search_context();
                let indexed = o.ocr.queue.as_ref().map_or(0, |q| {
                    let s = q.status();
                    s.indexed + s.written
                });
                // New OCR text, or any change to the catalog (every change writes an audit
                // row): the results are run again, so a renamed or trashed document is never
                // shown at its old place (QA A11). Looked at once a second.
                let (seen, at) = o.indexed_seen;
                if at.is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(1)) {
                    let changes: usize = o
                        .opened
                        .conn
                        .query_row("SELECT coalesce(max(seq), 0) FROM audit", [], |r| {
                            r.get::<_, i64>(0)
                        })
                        .map_or(0, |n| n as usize);
                    let generation = indexed.wrapping_add(changes);
                    if generation != seen && at.is_some() {
                        o.search.refresh();
                    }
                    o.indexed_seen = (generation, Some(std::time::Instant::now()));
                }
                crate::ui::search::show(
                    ui,
                    &mut o.search,
                    &o.opened.catalog,
                    &o.opened.root,
                    &o.browse.snap,
                    &cx,
                    &self.config.device_id,
                );
                if std::mem::take(&mut o.search.saved_changed) {
                    o.browse.saved = vf_index::saved::load(&o.opened.root);
                }
                if let Some(doc) = o.search.open.take() {
                    o.viewer.open(doc);
                    self.tab = Tab::Viewer;
                }
            }
            Screen::Open(o) if self.tab == Tab::Viewer => {
                let o = &mut **o;
                let initials = self.config.viewer.initials();
                let tick =
                    crate::ui::viewer::tick_color_index(self.config.viewer.tick_color.as_deref());
                o.viewer.tick_color = tick;
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
                // Initials and tickmark color are this PC's settings (owner 2026-11-04).
                let signed = o.viewer.signed.take();
                if signed.is_some() || o.viewer.tick_color != tick {
                    if signed.is_some() {
                        self.config.viewer.initials = signed;
                    }
                    self.config.viewer.tick_color =
                        Some(crate::ui::viewer::tick_color_name(o.viewer.tick_color).into());
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
                    ..
                } = &mut **o;
                let mut env = browse::Env {
                    conn: &mut opened.conn,
                    root: &opened.root,
                    device_id: &self.config.device_id,
                };
                // The admin password (PA-24): Admin waits behind the lock screen.
                if self.tab == Tab::Admin && !*admin_unlocked && vf_fs::trash::has_pin(&opened.root)
                {
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
                        }
                    });
                } else if self.tab == Tab::Admin {
                    let a = tab_admin.get_or_insert_with(|| Admin::load(&env));
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
                    );
                    if std::mem::take(&mut a.lock_requested) {
                        *admin_unlocked = false;
                    }
                    if std::mem::take(&mut a.audit.verify_requested) {
                        *o_chain = crate::ui::audit::ChainCheck::run(env.conn);
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
                        || tab_admin
                            .as_mut()
                            .is_some_and(|a| std::mem::take(&mut a.changed))
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
                        // A first index reads every file: say how far it is (owner 2026-11-05).
                        use std::sync::atomic::Ordering;
                        let p = &vf_fs::rebuild::PROGRESS;
                        let (dirs, found, read) = (
                            p.folders.load(Ordering::Relaxed),
                            p.found.load(Ordering::Relaxed),
                            p.read.load(Ordering::Relaxed),
                        );
                        if found > 0 {
                            ui.label(crate::ui::widgets::caption(format!(
                                "Indexing: {read} of {found} files read ({dirs} client folders). The first open on a PC reads every file once; later opens are quick."
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
            o.intake.ingest.check_dupes(&o.opened.conn);
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
        if let Screen::Open(o) = &mut self.screen {
            let o = &mut **o;
            if let Some(w) = o.opened.writer.clone()
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
            self.config.prefs = o.browse.prefs.clone();
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

/// What opening a cabinet does after the catalog is open, on the opening thread.
pub struct Prepared {
    /// Print capture rules not in line (shown on Browse).
    rules_problem: Option<String>,
    /// Documents the 90-day trash rule emptied.
    emptied: Result<usize, String>,
    chain: crate::ui::audit::ChainCheck,
}

/// The work of opening a cabinet that reads and writes the share or the whole catalog: print
/// capture rules, this workstation's registration, legal holds, the old trash, the audit chain.
/// Runs on the opening thread, so the window never freezes after the cabinet shows (QA A6).
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

fn prepare(opened: &mut Opened, device_id: &str) -> Prepared {
    let root = opened.root.clone();
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
    clear_check_out_leftovers(&mut opened.conn, &root);
    // Intake's client box lists Filer's client list: clients known only from their folders
    // are put in it (owner 2026-11-04: no client to pick in Intake).
    if let Err(e) = vf_fs::clients::ensure_listed(&opened.conn, &root) {
        tracing::warn!(kind = crate::logging::kind(&e), "client list not completed");
    }
    // This workstation in the cabinet's list (the Safeguards appendix lists them).
    if let Err(e) = vf_fs::safeguards::register_device(&root, device_id) {
        tracing::warn!(
            kind = crate::logging::kind(&e),
            "workstation not registered"
        );
    }
    // Legal holds placed on other workstations first (P07): nothing they cover may be emptied
    // below.
    if let Err(e) = vf_fs::holds::sync(&mut opened.conn, &root, device_id) {
        tracing::warn!(kind = crate::logging::kind(&e), "legal holds not synced");
    }
    // Trash older than 90 days empties on its own (Q-15; held documents stay).
    let emptied = vf_fs::trash::auto_empty(&mut opened.conn, &root, device_id);
    let chain = crate::ui::audit::ChainCheck::later();
    Prepared {
        rules_problem,
        emptied,
        chain,
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
                start.elapsed() < std::time::Duration::from_secs(20),
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
                start.elapsed() < std::time::Duration::from_secs(30),
                "push did not finish"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(h.state().tab, Tab::Browse, "without leaving the tab");
        let Screen::Open(o) = &h.state().screen else {
            panic!("open")
        };
        let pending = vf_sync::catalog::pending(&o.opened.conn).unwrap();
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
        std::fs::create_dir_all(root.join("Loose")).unwrap();
        std::fs::write(root.join("Loose/a.pdf"), "invented").unwrap();
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
                device_id: "dev-1".into(),
                device_name: "OFFICE-PC".into(),
            });
        }
        h.run();
        click(&mut h, "Push now");
        let start = std::time::Instant::now();
        while h.query_by_label_contains("Pushed 1 documents").is_none() {
            h.step();
            assert!(start.elapsed() < std::time::Duration::from_secs(30));
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // The counts are made on their own thread (PA-34): wait for them.
        let start = std::time::Instant::now();
        while h.query_by_label("in sync").is_none()
            || h.query_by_label("Nothing waiting: this PC and the bucket agree.")
                .is_none()
        {
            h.step();
            assert!(
                start.elapsed() < std::time::Duration::from_secs(30),
                "counts never refreshed"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
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
        h.run_steps(4);
        assert!(
            h.query_by_label_contains("locked by the admin password")
                .is_some(),
            "Setup locked"
        );
        // The row with no client: New client… opens Vibe's dialog, filled from the name.
        h.get_by_label("Preview").click();
        let label = "New client for Newco LLC_NEWC2002_2025 W-2.pdf";
        let start = std::time::Instant::now();
        while h.query_by_label(label).is_none() {
            assert!(
                start.elapsed() < std::time::Duration::from_secs(20),
                "row planned"
            );
            std::thread::sleep(std::time::Duration::from_millis(30));
            h.step();
        }
        h.get_by_label(label).click();
        h.run_steps(4);
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
                start.elapsed() < std::time::Duration::from_secs(20),
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
                    assert!(
                        start.elapsed() < std::time::Duration::from_secs(30),
                        "{what}"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(30));
                    h.step();
                }
            };
        wait(&mut h, "the form read", &|h| {
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
                    assert!(
                        start.elapsed() < std::time::Duration::from_secs(30),
                        "{what}"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(30));
                    h.step();
                }
            };
        wait(&mut h, "both tabs", &|h| {
            h.query_by_label("My inbox (1)").is_some()
        });
        assert!(h.query_by_label("Shared inbox (1)").is_some());
        assert!(
            h.query_by_label("own.pdf").is_none(),
            "not on the shared tab"
        );
        h.get_by_label("My inbox (1)").click();
        wait(&mut h, "my file listed", &|h| {
            h.query_by_label("own.pdf").is_some()
        });
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

    /// PHASES P03 done-when: the fixture cabinet is fully indexed (scans read by Tesseract) and
    /// a query returns in under 50 ms; an EIN that appears only in a scanned image finds its
    /// document, which opens in the Viewer at that page with the EIN highlighted.
    #[test]
    fn ein_only_in_a_scan_is_found_and_opens_at_its_page() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Clients");
        let mut fx = vf_fixtures::generate(&root, 11).unwrap();
        let dir = filer_pdf::pdf::find_pdfium(Path::new("")).unwrap();
        vf_fixtures::add_scans(&root, &mut fx, filer_pdf::pdf::pdfium(&dir).unwrap(), 11).unwrap();
        std::fs::remove_dir_all(root.join(vf_fs::cabinet::VIBE_DIR)).unwrap();
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

        // Every document indexed; the scans read by OCR and given their text layer.
        let total = fx.docs.len() + fx.scans.len();
        let start = std::time::Instant::now();
        loop {
            let Screen::Open(o) = &h.state().screen else {
                unreachable!()
            };
            let c = vf_ocr::queue::Counts::load(&o.opened.conn).unwrap();
            if c.done == total {
                assert_eq!((c.failed, c.indexed), (0, 0));
                break;
            }
            assert!(
                start.elapsed() < std::time::Duration::from_secs(300),
                "OCR never finished: {c:?} {:?}",
                o.ocr.queue.as_ref().map(|q| q.status())
            );
            std::thread::sleep(std::time::Duration::from_millis(200));
            h.step();
        }
        let scan = fx.scans[2].clone();
        {
            let Screen::Open(o) = &h.state().screen else {
                unreachable!()
            };
            let q = vf_index::search::Query {
                text: scan.ein.clone(),
                ..Default::default()
            };
            vf_index::search::search(&o.opened.conn, &q, 200).unwrap();
            let t = std::time::Instant::now();
            let hits = vf_index::search::search(&o.opened.conn, &q, 200).unwrap();
            let ms = t.elapsed().as_millis();
            assert!(ms < 50, "query took {ms} ms");
            assert_eq!(hits.len(), 1, "{hits:?}");
            assert_eq!(hits[0].rel_path, scan.rel_path);
            assert_eq!(hits[0].page, Some(scan.ein_page as u32));
        }
        click(&mut h, "Search");
        type_into(&mut h, "Search for", &scan.ein);
        // The search runs on its own thread once typing pauses (PA-34).
        let start = std::time::Instant::now();
        loop {
            h.step();
            let Screen::Open(o) = &h.state().screen else {
                unreachable!()
            };
            if !o.search.results.is_empty() && !o.search.searching() {
                break;
            }
            assert!(start.elapsed() < std::time::Duration::from_secs(20));
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        h.run_steps(2);
        {
            let Screen::Open(o) = &h.state().screen else {
                unreachable!()
            };
            assert_eq!(o.search.results.len(), 1);
            assert_eq!(o.search.results[0].page, Some(scan.ein_page as u32));
        }
        h.get_by_label("Scanned 1099-MISC 2025.pdf").click();
        h.step();
        assert_eq!(h.state().tab, Tab::Viewer);
        let start = std::time::Instant::now();
        loop {
            h.step();
            let Screen::Open(o) = &h.state().screen else {
                unreachable!()
            };
            if o.viewer.ready() {
                let d = o.viewer.doc.as_ref().unwrap();
                assert_eq!(d.page, scan.ein_page);
                assert_eq!(d.path, root.join(&scan.rel_path));
                assert!(!o.viewer.hits().is_empty(), "the EIN is highlighted");
                break;
            }
            assert!(start.elapsed() < std::time::Duration::from_secs(20));
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        h.get_by_label(&format!("Page {} of {}", scan.ein_page + 1, scan.pages));
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
            if start.elapsed() > std::time::Duration::from_secs(30) {
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
                start.elapsed() < std::time::Duration::from_secs(20),
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
            assert!(
                start.elapsed() < std::time::Duration::from_secs(20),
                "no words"
            );
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
            assert!(
                start.elapsed() < std::time::Duration::from_secs(20),
                "reloaded"
            );
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
                start.elapsed() < std::time::Duration::from_secs(20),
                "never rendered"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let (doc_id, old_hash) = {
            let Screen::Open(o) = &h.state().screen else {
                unreachable!()
            };
            let id = o.viewer.doc.as_ref().unwrap().doc_id.clone();
            let hash: String = o
                .opened
                .conn
                .query_row("SELECT hash FROM document WHERE id = ?1", [&id], |r| {
                    r.get(0)
                })
                .unwrap();
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
        h.get_by_role_and_label(Role::ComboBox, "Tickmark color")
            .click();
        h.step();
        h.step();
        h.get_by_label("Purple").click();
        h.step();
        h.step();
        let saved = config::load(&tmp.path().join("pc1")).unwrap();
        assert_eq!(saved.viewer.tick_color.as_deref(), Some("Purple"));
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
                    start.elapsed() < std::time::Duration::from_secs(20),
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
                assert!(start.elapsed() < std::time::Duration::from_secs(20));
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
            assert!(start.elapsed() < std::time::Duration::from_secs(20));
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
            assert!(start.elapsed() < std::time::Duration::from_secs(20));
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
            assert!(start.elapsed() < std::time::Duration::from_secs(20));
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let open_pages = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, pages: &str| {
            // The last page edit reloads the document on the render thread: wait for it, or
            // the next edit is refused (failed under a loaded test run).
            let start = std::time::Instant::now();
            while !matches!(&h.state().screen, Screen::Open(o) if o.viewer.ready()) {
                assert!(
                    start.elapsed() < std::time::Duration::from_secs(30),
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
        if matches!(h.state().screen, Screen::Busy { .. }) {
            click(&mut h, "Open a different cabinet");
            assert!(matches!(h.state().screen, Screen::Wizard(_)));
        }
        // The abandoned opening lets go of the data folder before the next one.
        std::thread::sleep(std::time::Duration::from_millis(500));
        // And straight through when left alone.
        h.state_mut().open_last(&ctx);
        settle(&mut h);
        assert!(matches!(h.state().screen, Screen::Open(_)));
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
