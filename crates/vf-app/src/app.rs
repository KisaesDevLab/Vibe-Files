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
    /// Wait before the next mirror write: 2 s, doubling to 2 min after failures, so a share
    /// that went away is not waited on every 2 s (QA A7).
    mirror_wait: std::time::Duration,
    /// The integrity sweep (P05 step 5): weekly, and from Admin.
    pub sweep: crate::ui::sweep::SweepState,
    /// B2 sync (P11).
    pub sync: crate::ui::sync::SyncTab,
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
            B::Client(ci) | B::Year { client: ci, .. } if *ci < snap.clients.len() => {
                cx.client = Some(label(*ci));
            }
            B::Folder {
                client,
                year,
                folder,
            } if *client < snap.clients.len() => {
                cx.client = Some(label(*client));
                let base = snap.clients[*client].folder_rel.clone().unwrap_or_default();
                cx.folder = Some((
                    if base.is_empty() {
                        folder.clone()
                    } else {
                        format!("{base}/{folder}")
                    },
                    *year,
                    format!("{folder} · {}", crate::ui::widgets::year_label(*year)),
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
        theme::install(ctx);
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
                        sync: Default::default(),
                        update_check: true,
                    },
                    Some(format!("Settings could not be read: {e:#} ({e2:#})")),
                ),
            },
        };
        let mut w = Wizard::new(config.last_cabinet.as_deref());
        w.error = error;
        let update = check_for_update(config.update_check);
        Self {
            tab: Tab::Browse,
            data,
            config,
            screen: Screen::Wizard(w),
            update,
            update_notice: None,
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

    /// Picks up a finished create/open.
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
            Ok(mut opened) => {
                // A crash between Filer's commit and the catalog write is caught up here.
                let mut intake = IntakeTab::default();
                let root = opened.root.clone();
                intake.reconcile(&mut opened.conn, &root, &self.config.device_id);
                self.config.last_cabinet = Some(opened.root.clone());
                // The cabinet is open either way; a settings write failure only loses "last".
                let _ = config::save(&self.data, &self.config);
                // This workstation in the cabinet's list (the Safeguards appendix lists them).
                if let Err(e) =
                    vf_fs::safeguards::register_device(&opened.root, &self.config.device_id)
                {
                    tracing::warn!(
                        kind = crate::logging::kind(&e),
                        "workstation not registered"
                    );
                }
                // Legal holds placed on other workstations first (P07): nothing they cover may
                // be emptied below.
                if let Err(e) =
                    vf_fs::holds::sync(&mut opened.conn, &opened.root, &self.config.device_id)
                {
                    tracing::warn!(kind = crate::logging::kind(&e), "legal holds not synced");
                }
                // Trash older than 90 days empties on its own (Q-15; held documents stay).
                let emptied = vf_fs::trash::auto_empty(
                    &mut opened.conn,
                    &opened.root,
                    &self.config.device_id,
                );
                let mut browse = Browse::load(&opened.conn);
                browse.set_root(&opened.root);
                browse.sheets_dir = self.data.join("sheets");
                browse.set_prefs(self.config.prefs.clone());
                browse.saved = vf_index::saved::load(&opened.root);
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
                    Err(e) => browse.error = Some(format!("Emptying old trash: {e}")),
                }
                let ocr = crate::ui::ocr::OcrState::start(
                    ctx,
                    &opened,
                    &self.config.ocr,
                    &self.config.device_id,
                );
                let chain = crate::ui::audit::ChainCheck::run(&opened.conn);
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
                    mirror_wait: std::time::Duration::from_secs(2),
                    sweep,
                    sync: Default::default(),
                    checks,
                    opened,
                    browse,
                    admin: None,
                    intake,
                    ocr,
                    search: Default::default(),
                    viewer: Default::default(),
                    indexed_seen: (0, None),
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
        self.poll(ui.ctx());
        let name = self.cabinet_name();
        chrome::header(ui, name.as_deref());
        let badge = match &self.screen {
            Screen::Open(o) => o.intake.ingest.badge(),
            _ => 0,
        };
        chrome::tab_bar(ui, &mut self.tab, badge);
        if let Screen::Open(o) = &mut self.screen {
            o.chain.tick(&o.opened.conn);
            if let Some(line) =
                o.sweep
                    .poll(&mut o.opened.conn, &o.opened.root, &self.config.device_id)
            {
                o.browse.notice = Some(line);
            }
            if std::mem::take(&mut o.sweep.changed) {
                o.browse.reload(&o.opened.conn);
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
                            .fill(egui::Color32::from_rgb(0xfd, 0xec, 0xea))
                            .inner_margin(egui::Margin::symmetric(16, 8)),
                    )
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(problem).color(wizard::ERROR).strong());
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
                    &o.opened.conn,
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
                if std::mem::take(&mut o.viewer.catalog_changed) {
                    o.browse.reload(&o.opened.conn);
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
                crate::ui::sync::show(
                    ui,
                    &mut o.sync,
                    &mut browse::Env {
                        conn: &mut o.opened.conn,
                        root: &o.opened.root,
                        device_id: &self.config.device_id,
                    },
                    crate::ui::sync::View {
                        catalog: &o.opened.catalog,
                        cabinet_id: &cabinet_id,
                        cfg: &self.config.sync,
                        clients: &clients,
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
                    ..
                } = &mut **o;
                let mut env = browse::Env {
                    conn: &mut opened.conn,
                    root: &opened.root,
                    device_id: &self.config.device_id,
                };
                if self.tab == Tab::Admin {
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
                    if std::mem::take(&mut a.audit.verify_requested) {
                        *o_chain = crate::ui::audit::ChainCheck::run(env.conn);
                    }
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
                    tab_browse.sync = o_sync.session.is_some().then(|| browse::SyncView {
                        pending: o_sync.pending.clone(),
                        conflicts: o_sync.conflicts.len(),
                        running_for: o_sync.running_for.clone(),
                    });
                    browse::show(ui, tab_browse, &mut env);
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
                o.intake.ingest.warning = Some(format!("The sheet did not open: {e}"));
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
            o.sync.tick(
                &ctx,
                &browse::Env {
                    conn: &mut o.opened.conn,
                    root: &o.opened.root,
                    device_id: &self.config.device_id,
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
                    o.browse.error = Some(format!("Startup check: {}", failed.join(" · ")));
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
        for _ in 0..3 {
            h.step();
        }
        h.run();
        assert!(
            h.query_by_label("4 pending").is_some(),
            "the pill counts the client's documents"
        );
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
        h.run();
        assert!(h.query_by_label("in sync").is_some());
        assert!(
            h.query_by_label("Nothing waiting: this PC and the bucket agree.")
                .is_some()
        );
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
        while !(delivered.is_file()
            && !original.exists()
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
        h.run();
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
        h.run();
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
        h.run();
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
        h.run();
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
        let dir = filer_pdf::pdf::find_pdfium(Path::new("")).unwrap();
        let pdfium = filer_pdf::pdf::pdfium(&dir).unwrap();
        let texts = |p: &std::path::Path| -> Vec<String> {
            // The Viewer's render thread shares pdfium; under a loaded test run an open can
            // fail for a moment, so try a few times before failing.
            let mut tries = 0;
            let d = loop {
                match pdfium.load_pdf_from_file(p, None) {
                    Ok(d) => break d,
                    Err(e) if tries == 50 => panic!("cannot open {}: {e}", p.display()),
                    Err(_) => {
                        tries += 1;
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                }
            };
            d.pages()
                .iter()
                .map(|pg| vf_pdf::text::page_text(&pg).trim().to_string())
                .collect()
        };

        h.get_by_label("a.pdf").click();
        h.run();
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

        // Browse: select a.pdf and b.pdf, Combine.
        click(&mut h, "Browse");
        h.run();
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
            ["Alpha one", "Alpha two", "Alpha three", "Bravo one"]
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
