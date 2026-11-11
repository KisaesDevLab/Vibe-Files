//! The firm's Vibe Portal in the app (PA-37): the Share window (Browse), the client's portal
//! users (client panel), Admin › Portal and Admin › Portal requests, and the timed fetch of
//! client uploads into the Portal inbox. Every call to the portal runs on a thread; the window
//! shows "Working…" meanwhile.
//!
//! Audit (QA 2026-11-09): each portal change is sent to [`PortalState`]'s channel by the thread
//! the moment the portal accepted it, and the app records what the channel holds every frame.
//! Closing a window, or an error part-way through a batch, never loses the record of what was
//! already done.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use egui::RichText;
use vf_core::audit::AuditEntry;
use vf_portal::api::{AccessRequest, Person, PortalFile, Users};
use vf_portal::{PortalSettings, settings};

use crate::portal as core;
use crate::ui::browse::Env;
use crate::ui::{theme, widgets};

/// Runs `f` on a thread; the window repaints when it is done.
fn spawn<T: Send + 'static>(
    ctx: &egui::Context,
    f: impl FnOnce() -> T + Send + 'static,
) -> Receiver<T> {
    let (tx, rx) = mpsc::channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let _ = tx.send(f());
        ctx.request_repaint();
    });
    rx
}

/// Records audit rows in one transaction; the message when it fails (never swallowed).
fn record(env: &mut Env, label: &str, entries: Vec<AuditEntry>) -> Option<String> {
    if entries.is_empty() {
        return None;
    }
    let mut tx = vf_core::tx::Transaction::new(label);
    for e in entries {
        tx = tx.audit(e);
    }
    tx.commit(
        env.conn,
        &mut vf_core::tx::NoFileOps,
        &vf_core::tx::Actor::device(env.device_id),
    )
    .err()
    .map(|e| format!(" This was NOT recorded in the audit trail: {e}"))
}

fn cabinet_id(root: &Path) -> Option<String> {
    vf_fs::cabinet::read(root).ok().map(|c| c.cabinet_id)
}

/// Where a thread sends the audit row of each portal change it made.
#[derive(Clone)]
pub struct AuditOut(Sender<AuditEntry>);

impl AuditOut {
    fn send(&self, e: AuditEntry) {
        let _ = self.0.send(e);
    }
}

// ======================================================================================
// State kept with Browse: settings, the timed fetch, the marks, the open windows.
// ======================================================================================

/// The portal's line in Intake's strip (PA-38).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortalLine {
    /// A fetch is running (or was asked for) on this PC.
    pub fetching: bool,
    /// "last fetched 10:42: 2 files received · every 15 minutes".
    pub text: String,
}

/// What one fetch did, in a few words.
fn fetch_result(r: &core::FetchReport) -> String {
    let n = r.received.len();
    if n > 0 {
        format!("{n} file{} received", if n == 1 { "" } else { "s" })
    } else if r.busy {
        "another PC was fetching".into()
    } else if let Some(p) = r.problems.first() {
        format!("problem: {p}")
    } else {
        "nothing new".into()
    }
}

/// The strip's words: the last fetch on this PC and the timer.
fn line_text(last: Option<&(String, String)>, fetch_minutes: u32) -> String {
    let last = match last {
        Some((at, what)) => format!("last fetched {at}: {what}"),
        None => "not fetched yet on this PC".into(),
    };
    let timer = if fetch_minutes == 0 {
        "by hand only (Admin › Portal sets a timer)".to_string()
    } else {
        format!("every {fetch_minutes} minutes")
    };
    format!("{last} · {timer}")
}

pub struct PortalState {
    settings: Option<PortalSettings>,
    read_at: Option<Instant>,
    fetch: Option<Receiver<core::FetchReport>>,
    fetched_at: Option<Instant>,
    marks: Option<Receiver<Option<HashSet<String>>>>,
    /// Cabinet documents on the portal (Browse's marks), as of the last read.
    pub on_portal: HashSet<String>,
    pub share: Option<ShareWindow>,
    /// The document pane's Send secure link… (Vibe-Portal P68).
    pub send_link: Option<SendLinkWindow>,
    pub people: Option<PeopleWindow>,
    /// "Fetch uploads now" (Admin › Portal, or Intake's strip).
    pub fetch_now: bool,
    /// The last fetch on this PC: (local time, what it did), for Intake's strip.
    last_fetch: Option<(String, String)>,
    /// Read the marks again (after the Share window closed); fetches nothing.
    pub refresh_marks: bool,
    /// A line for Browse's notice bar.
    pub notice: Option<String>,
    /// The last problem shown: the same one is not shown again until it changes.
    last_problem: Option<String>,
    audit_tx: Sender<AuditEntry>,
    audit_rx: Receiver<AuditEntry>,
}

impl Default for PortalState {
    fn default() -> Self {
        let (audit_tx, audit_rx) = mpsc::channel();
        Self {
            settings: None,
            read_at: None,
            fetch: None,
            fetched_at: None,
            marks: None,
            on_portal: HashSet::new(),
            share: None,
            send_link: None,
            people: None,
            fetch_now: false,
            last_fetch: None,
            refresh_marks: false,
            notice: None,
            last_problem: None,
            audit_tx,
            audit_rx,
        }
    }
}

impl PortalState {
    /// The portal settings, read from the cabinet at most every 30 s (another PC may change
    /// them); `None` until the firm sets them in Admin › Portal.
    pub fn settings(&mut self, root: &Path) -> Option<&PortalSettings> {
        if self
            .read_at
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(30))
        {
            self.read_at = Some(Instant::now());
            self.settings = cabinet_id(root).and_then(|id| settings::load(root, &id));
            if self.settings.is_none() {
                self.on_portal.clear();
            }
        }
        self.settings.as_ref()
    }

    /// Reads the settings again now (after Admin › Portal saved them).
    pub fn reread(&mut self) {
        self.read_at = None;
        self.fetched_at = None;
    }

    pub fn is_set(&mut self, root: &Path) -> bool {
        self.settings(root).is_some()
    }

    /// Intake's line, or `None` when no portal is set up (PA-38).
    pub fn intake_line(&mut self, root: &Path) -> Option<PortalLine> {
        let every = self.settings(root)?.fetch_minutes;
        Some(PortalLine {
            fetching: self.fetch.is_some() || self.fetch_now,
            text: line_text(self.last_fetch.as_ref(), every),
        })
    }

    pub fn audit_out(&self) -> AuditOut {
        AuditOut(self.audit_tx.clone())
    }

    /// The timed fetch of client uploads, and the audit rows of every portal change made
    /// since the last frame (fetches, and what the windows' threads did).
    pub fn tick(&mut self, ctx: &egui::Context, root: &Path) -> Vec<AuditEntry> {
        let mut audit: Vec<AuditEntry> = self.audit_rx.try_iter().collect();
        if let Some(rx) = &self.marks
            && let Ok(set) = rx.try_recv()
        {
            self.marks = None;
            if let Some(set) = set {
                self.on_portal = set;
            }
        }
        if let Some(rx) = &self.fetch
            && let Ok(report) = rx.try_recv()
        {
            self.fetch = None;
            self.last_fetch = Some((
                chrono::Local::now().format("%H:%M").to_string(),
                fetch_result(&report),
            ));
            if let Some(set) = report.on_portal {
                self.on_portal = set;
            }
            for r in &report.received {
                audit.push(AuditEntry::new(
                    "portal_receive",
                    None,
                    serde_json::json!({
                        "client_id": r.client_id,
                        "name": r.name,
                        "saved_as": r.saved_as,
                        "from": r.from,
                        "note": r.note,
                        "size": r.size,
                    }),
                ));
            }
            let n = report.received.len();
            match report.problems.first() {
                // A problem is said once, until it changes or a fetch works again.
                Some(p) if self.last_problem.as_deref() != Some(p.as_str()) => {
                    self.last_problem = Some(p.clone());
                    self.notice = Some(format!("Portal: {p}"));
                }
                Some(_) => {}
                None => self.last_problem = None,
            }
            if n > 0 {
                self.notice = Some(format!(
                    "Portal: {n} file{} received into {}.",
                    if n == 1 { "" } else { "s" },
                    core::INBOX
                ));
            }
        }
        let Some(s) = self.settings(root).cloned() else {
            return audit;
        };
        let every = Duration::from_secs(u64::from(s.fetch_minutes) * 60);
        let due = std::mem::take(&mut self.fetch_now)
            || (s.fetch_minutes > 0 && self.fetched_at.is_none_or(|t| t.elapsed() >= every));
        if due && self.fetch.is_none() {
            self.fetched_at = Some(Instant::now());
            let root = root.to_path_buf();
            let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            let s2 = s.clone();
            self.fetch = Some(spawn(ctx, move || core::fetch(&root, &s2, &now)));
        } else if std::mem::take(&mut self.refresh_marks) && self.marks.is_none() {
            let s2 = s.clone();
            self.marks = Some(spawn(ctx, move || {
                core::client(&s2)
                    .all_files()
                    .ok()
                    .map(|f| f.into_iter().map(|f| f.vf_document_id).collect())
            }));
        }
        if s.fetch_minutes > 0 {
            ctx.request_repaint_after(Duration::from_secs(30));
        }
        audit
    }
}

// ======================================================================================
// The invitation form (Share window and portal users window).
// ======================================================================================

#[derive(Default)]
struct InviteForm {
    email: String,
    phone: String,
    name: String,
    role: String,
}

impl InviteForm {
    /// Draws the form (opened when `open`); `true` when "Send invitation" was pressed.
    fn show(&mut self, ui: &mut egui::Ui, busy: bool, open: bool) -> bool {
        let mut send = false;
        egui::CollapsingHeader::new("Invite someone…")
            .open(open.then_some(true))
            .show(ui, |ui| {
            egui::Grid::new("portal-invite")
                .num_columns(2)
                .show(ui, |ui| {
                    for (label, value) in [
                        ("Email", &mut self.email),
                        ("Mobile number", &mut self.phone),
                        ("Name", &mut self.name),
                        ("Role (e.g. Spouse)", &mut self.role),
                    ] {
                        let l = ui.label(label);
                        ui.text_edit_singleline(value).labelled_by(l.id);
                        ui.end_row();
                    }
                });
            ui.label(widgets::caption(
                "The invitation goes to the email when there is one, else by text. It works for 7 days. A mobile number given with an email is used only once the person confirms it.",
            ));
            let ok = !busy && !(self.email.trim().is_empty() && self.phone.trim().is_empty());
            if ui
                .add_enabled(ok, egui::Button::new("Send invitation"))
                .clicked()
            {
                send = true;
            }
        });
        send
    }

    fn email(&self) -> Option<String> {
        Some(self.email.trim().to_string()).filter(|x| !x.is_empty())
    }

    fn phone(&self) -> Option<String> {
        Some(self.phone.trim().to_string()).filter(|x| !x.is_empty())
    }
}

fn invite_audit(client_id: &str, by_email: bool, role: &str) -> AuditEntry {
    AuditEntry::new(
        "portal_invite",
        None,
        serde_json::json!({
            "client_id": client_id,
            "via": if by_email { "email" } else { "text" },
            "role": role,
        }),
    )
}

// ======================================================================================
// Share window (Browse › Share…).
// ======================================================================================

#[derive(Debug, Clone)]
pub struct DocRef {
    pub id: String,
    pub name: String,
    pub path: std::path::PathBuf,
    /// sha256 of the cabinet file (the catalog's hash).
    pub hash: String,
    pub tax_year: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct ClientRef {
    pub id: String,
    pub code: String,
    pub name: String,
}

struct Loaded {
    users: Users,
    /// The portal copy of each document, in `docs` order.
    files: Vec<Option<PortalFile>>,
}

enum Outcome {
    Loaded(Loaded),
    Done(String),
}

/// Who sees the selected documents now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sees {
    All,
    Some,
    None,
}

fn sees(f: &Option<PortalFile>, id: &str) -> bool {
    f.as_ref()
        .is_some_and(|f| f.users.iter().any(|x| x.identity_id == id && x.visible))
}

/// What Save sends for one document: the people whose tick was changed in this window and
/// whose state on this document differs (QA 2026-11-09: never the people left alone).
pub fn visibility_changes(
    file: &Option<PortalFile>,
    chosen: &BTreeMap<String, bool>,
) -> BTreeMap<String, bool> {
    chosen
        .iter()
        .filter(|(id, w)| sees(file, id) != **w)
        .map(|(id, w)| (id.clone(), *w))
        .collect()
}

pub struct ShareWindow {
    client: ClientRef,
    docs: Vec<DocRef>,
    loaded: Option<Loaded>,
    /// The ticks changed in this window (identity → shown).
    chosen: BTreeMap<String, bool>,
    /// Send the cabinet's newer content for documents changed since they were shared.
    update: bool,
    invite: InviteForm,
    confirm_remove: bool,
    job: Option<Receiver<Result<Outcome, String>>>,
    audit: AuditOut,
    msg: Option<String>,
    err: Option<String>,
    pub closed: bool,
}

fn load_job(
    ctx: &egui::Context,
    s: PortalSettings,
    client: ClientRef,
    docs: Vec<DocRef>,
) -> Receiver<Result<Outcome, String>> {
    spawn(ctx, move || {
        let p = core::client(&s);
        p.put_client(&client.id, &client.code, &client.name, None)
            .map_err(|e| e.message())?;
        let users = p.users(&client.id).map_err(|e| e.message())?;
        let files = p.files_for_client(&client.id).map_err(|e| e.message())?;
        let files = docs
            .iter()
            .map(|d| files.iter().find(|f| f.vf_document_id == d.id).cloned())
            .collect();
        Ok(Outcome::Loaded(Loaded { users, files }))
    })
}

impl ShareWindow {
    pub fn new(
        ctx: &egui::Context,
        s: &PortalSettings,
        client: ClientRef,
        docs: Vec<DocRef>,
        audit: AuditOut,
    ) -> Self {
        let job = Some(load_job(ctx, s.clone(), client.clone(), docs.clone()));
        Self {
            client,
            docs,
            loaded: None,
            chosen: BTreeMap::new(),
            update: true,
            invite: InviteForm::default(),
            confirm_remove: false,
            job,
            audit,
            msg: None,
            err: None,
            closed: false,
        }
    }

    fn reload(&mut self, ctx: &egui::Context, s: &PortalSettings) {
        self.job = Some(load_job(
            ctx,
            s.clone(),
            self.client.clone(),
            self.docs.clone(),
        ));
    }

    fn state(l: &Loaded, id: &str) -> Sees {
        let n = l.files.iter().filter(|f| sees(f, id)).count();
        if n == 0 {
            Sees::None
        } else if n == l.files.len() {
            Sees::All
        } else {
            Sees::Some
        }
    }

    fn changed_docs(&self, l: &Loaded) -> usize {
        self.docs
            .iter()
            .zip(&l.files)
            .filter(|(d, f)| f.as_ref().is_some_and(|f| f.sha256 != d.hash))
            .count()
    }

    fn apply(&mut self, ctx: &egui::Context, s: &PortalSettings) {
        let Some(l) = &self.loaded else { return };
        let chosen = self.chosen.clone();
        let update = self.update;
        let client = self.client.clone();
        let work: Vec<(DocRef, Option<PortalFile>)> = self
            .docs
            .iter()
            .cloned()
            .zip(l.files.iter().cloned())
            .collect();
        let s = s.clone();
        let audit = self.audit.clone();
        self.msg = None;
        self.err = None;
        self.job = Some(spawn(ctx, move || {
            let p = core::client(&s);
            let mut changed = 0usize;
            let mut sent = 0usize;
            for (d, file) in work {
                let vis = visibility_changes(&file, &chosen);
                let wants_it = vis.values().any(|&v| v);
                let (file_id, added, updated) = match &file {
                    // Nobody newly chosen: a document not on the portal stays off it.
                    None if !wants_it => continue,
                    None => {
                        let a = p
                            .add(&client.id, &d.id, &d.name, d.tax_year, &d.path)
                            .map_err(|e| format!("{}: {}", d.name, e.message()))?;
                        let mut updated = false;
                        if let vf_portal::api::Added::Changed(f) = &a
                            && update
                        {
                            p.replace(f, &d.name, &d.path)
                                .map_err(|e| format!("{}: {}", d.name, e.message()))?;
                            updated = true;
                        }
                        sent += 1;
                        (a.file_id().to_string(), true, updated)
                    }
                    Some(f) => {
                        let mut updated = false;
                        if update && f.sha256 != d.hash {
                            p.replace(&f.file_id, &d.name, &d.path)
                                .map_err(|e| format!("{}: {}", d.name, e.message()))?;
                            updated = true;
                            sent += 1;
                        }
                        (f.file_id.clone(), false, updated)
                    }
                };
                if added || updated {
                    audit.send(AuditEntry::new(
                        "portal_share",
                        Some(&d.id),
                        serde_json::json!({ "client_id": client.id, "sent": added, "updated": updated }),
                    ));
                }
                if !vis.is_empty() {
                    p.set_visibility(&file_id, &vis)
                        .map_err(|e| format!("{}: {}", d.name, e.message()))?;
                    let shown: Vec<&String> = vis.iter().filter(|v| *v.1).map(|v| v.0).collect();
                    let hidden: Vec<&String> = vis.iter().filter(|v| !*v.1).map(|v| v.0).collect();
                    audit.send(AuditEntry::new(
                        "portal_share",
                        Some(&d.id),
                        serde_json::json!({
                            "client_id": client.id,
                            "shown_to": shown,
                            "hidden_from": hidden,
                        }),
                    ));
                }
                if added || updated || !vis.is_empty() {
                    changed += 1;
                }
            }
            Ok(Outcome::Done(if changed == 0 {
                "Nothing to change.".to_string()
            } else if sent > 0 {
                format!(
                    "Saved on the portal ({sent} file{} sent).",
                    if sent == 1 { "" } else { "s" }
                )
            } else {
                "Saved on the portal.".to_string()
            }))
        }));
    }

    fn remove(&mut self, ctx: &egui::Context, s: &PortalSettings) {
        let Some(l) = &self.loaded else { return };
        let work: Vec<(String, String)> = self
            .docs
            .iter()
            .zip(&l.files)
            .filter_map(|(d, f)| f.as_ref().map(|f| (d.id.clone(), f.file_id.clone())))
            .collect();
        let client = self.client.id.clone();
        let s = s.clone();
        let audit = self.audit.clone();
        self.job = Some(spawn(ctx, move || {
            let p = core::client(&s);
            for (doc, file) in work {
                p.remove(&file).map_err(|e| e.message())?;
                audit.send(AuditEntry::new(
                    "portal_remove",
                    Some(&doc),
                    serde_json::json!({ "client_id": client }),
                ));
            }
            Ok(Outcome::Done(
                "Removed from the portal (the cabinet keeps the files).".into(),
            ))
        }));
    }

    fn send_invite(&mut self, ctx: &egui::Context, s: &PortalSettings) {
        let f = std::mem::take(&mut self.invite);
        let client = self.client.id.clone();
        let s = s.clone();
        let audit = self.audit.clone();
        self.job = Some(spawn(ctx, move || {
            core::client(&s)
                .invite(
                    &client,
                    f.email().as_deref(),
                    f.phone().as_deref(),
                    f.name.trim(),
                    f.role.trim(),
                )
                .map_err(|e| e.message())?;
            audit.send(invite_audit(&client, f.email().is_some(), f.role.trim()));
            Ok(Outcome::Done(
                "Invitation sent. They appear here once they accept.".into(),
            ))
        }));
    }

    /// Draws the window.
    pub fn show(&mut self, ctx: &egui::Context, s: &PortalSettings) {
        if let Some(rx) = &self.job
            && let Ok(r) = rx.try_recv()
        {
            self.job = None;
            match r {
                Ok(Outcome::Loaded(l)) => {
                    self.chosen.clear();
                    self.loaded = Some(l);
                }
                Ok(Outcome::Done(msg)) => {
                    self.msg = Some(msg);
                    self.reload(ctx, s);
                }
                Err(e) => {
                    self.err = Some(e);
                    // What was done before the error is on the portal (and audited): show it.
                    self.reload(ctx, s);
                }
            }
        }
        let busy = self.job.is_some();
        let mut open = true;
        // A button pressed while the window is drawn, acted on after.
        let mut pending: Option<&'static str> = None;
        let title = format!("Share on the portal — {}", self.client.name);
        egui::Window::new(title)
            .id(egui::Id::unique("portal-share"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(520.0)
            .show(ctx, |ui| {
                for (i, d) in self.docs.iter().enumerate() {
                    let state = match self.loaded.as_ref().map(|l| &l.files[i]) {
                        None => String::new(),
                        Some(None) => "not on the portal".into(),
                        Some(Some(f)) if f.sha256 != d.hash => "changed since it was shared".into(),
                        Some(Some(f)) => {
                            format!("on the portal since {}", widgets::local_time(&f.shared_at))
                        }
                    };
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(&d.name).strong());
                        ui.label(widgets::caption(state));
                    });
                }
                ui.separator();
                let Some(l) = &self.loaded else {
                    if !busy && self.err.is_some() {
                        if ui.button("Try again").clicked() {
                            pending = Some("reload");
                        }
                    } else {
                        ui.label(widgets::caption("Reading the portal…"));
                    }
                    widgets::message_line(ui, self.err.as_deref());
                    return;
                };
                let single = self.docs.len() == 1;
                if l.users.users.is_empty() {
                    ui.label(format!(
                        "Nobody has portal access to {} yet: invite someone below.",
                        self.client.name
                    ));
                } else {
                    ui.label(RichText::new("Who sees it").strong());
                    for u in &l.users.users {
                        let now = Self::state(l, &u.identity_id);
                        let mut on = self
                            .chosen
                            .get(&u.identity_id)
                            .copied()
                            .unwrap_or(now == Sees::All);
                        ui.horizontal_wrapped(|ui| {
                            let label = if u.role_label.is_empty() {
                                u.label()
                            } else {
                                format!("{} ({})", u.label(), u.role_label)
                            };
                            let r = ui.add_enabled(
                                u.active && !busy,
                                egui::Checkbox::new(&mut on, label),
                            );
                            if r.changed() {
                                self.chosen.insert(u.identity_id.clone(), on);
                            }
                            if !u.active {
                                ui.label(widgets::caption("turned off"));
                            } else if now == Sees::Some && !self.chosen.contains_key(&u.identity_id)
                            {
                                ui.label(widgets::caption("sees some of these (left as they are)"));
                            } else if single && let Some(Some(f)) = l.files.first() {
                                let seen = f.users.iter().find(|x| x.identity_id == u.identity_id);
                                let t = match seen {
                                    Some(x) if x.visible => match &x.viewed_at {
                                        Some(t) => format!("opened {}", widgets::local_time(t)),
                                        None => "not opened yet".into(),
                                    },
                                    _ => String::new(),
                                };
                                ui.label(widgets::caption(t));
                            }
                        });
                    }
                }
                for inv in &l.users.invitations {
                    let who = inv.email.clone().or(inv.phone.clone()).unwrap_or_default();
                    ui.label(widgets::caption(format!(
                        "{who}: {}",
                        if inv.status == "expired" {
                            "invitation expired"
                        } else {
                            "invited, not accepted yet"
                        }
                    )));
                }
                let changed = self.changed_docs(l);
                if changed > 0 {
                    ui.checkbox(
                        &mut self.update,
                        format!("Update on portal: send the cabinet's newer version ({changed})"),
                    );
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!busy, widgets::primary_button("Save"))
                        .clicked()
                    {
                        pending = Some("apply");
                    }
                    let on_portal = l.files.iter().any(Option::is_some);
                    if on_portal
                        && !self.confirm_remove
                        && ui
                            .add_enabled(!busy, egui::Button::new("Remove from portal"))
                            .clicked()
                    {
                        self.confirm_remove = true;
                    }
                    if busy {
                        ui.spinner();
                        ui.label(widgets::caption("Working…"));
                    }
                });
                if self.confirm_remove {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(
                                "Remove from the portal for everyone? Secure links sent to people outside the client stop giving it too.",
                            )
                                .color(theme::warn()),
                        );
                        if ui.button("Remove").clicked() {
                            pending = Some("remove");
                        }
                        if ui.button("Keep").clicked() {
                            self.confirm_remove = false;
                        }
                    });
                }
                ui.separator();
                if self.invite.show(ui, busy, false) {
                    pending = Some("invite");
                }
                if let Some(m) = &self.msg {
                    ui.label(m.as_str());
                }
                widgets::message_line(ui, self.err.as_deref());
            });
        match pending {
            Some("reload") => {
                self.err = None;
                self.reload(ctx, s);
            }
            Some("apply") => self.apply(ctx, s),
            Some("remove") => {
                self.confirm_remove = false;
                self.remove(ctx, s);
            }
            Some("invite") => self.send_invite(ctx, s),
            _ => {}
        }
        if !open {
            self.closed = true;
        }
    }
}

// ======================================================================================
// A client's portal users (client panel › Portal users…).
// ======================================================================================

/// The users read again (when that worked), and what the action did.
type PeopleDone = (Option<Users>, Result<String, String>);

/// A search of the portal's people: what was typed, and who matched (or why it failed).
type PeopleFound = (String, Result<Vec<Person>, String>);

/// How many people the portal answers with at most (Vibe-Portal `PEOPLE_SHOWN`).
const PEOPLE_SHOWN: usize = 20;

/// The invitation form filled in for someone already on the portal: their email (else their
/// mobile, as an invitation with both waits for the mobile to be confirmed) and their name.
fn invite_for(p: &Person) -> InviteForm {
    let email = p.email.clone().unwrap_or_default();
    InviteForm {
        phone: if email.is_empty() {
            p.phone.clone().unwrap_or_default()
        } else {
            String::new()
        },
        email,
        name: p.name.clone(),
        role: String::new(),
    }
}

/// A failed search in plain words; a portal from before the search says to update it.
fn people_error(e: &vf_portal::api::PortalError) -> String {
    if e.status() == Some(404) {
        "This portal can't search its people yet: update Vibe Portal, or invite them by their email or mobile number.".into()
    } else {
        e.message()
    }
}

pub struct PeopleWindow {
    client: ClientRef,
    users: Option<Users>,
    invite: InviteForm,
    /// The "Find" box of "Someone who already uses the portal…".
    find: String,
    finding: Option<Receiver<PeopleFound>>,
    found: Option<(String, Vec<Person>)>,
    job: Option<Receiver<PeopleDone>>,
    audit: AuditOut,
    msg: Option<String>,
    err: Option<String>,
    pub closed: bool,
}

impl PeopleWindow {
    pub fn new(
        ctx: &egui::Context,
        s: &PortalSettings,
        client: ClientRef,
        audit: AuditOut,
    ) -> Self {
        let mut w = Self {
            client,
            users: None,
            invite: InviteForm::default(),
            find: String::new(),
            finding: None,
            found: None,
            job: None,
            audit,
            msg: None,
            err: None,
            closed: false,
        };
        w.run(ctx, s, None, String::new(), |_| Ok(()));
        w
    }

    /// Runs `act` on a thread (recording `entry` once the portal took it), then reads the
    /// users again, also after a failure.
    fn run(
        &mut self,
        ctx: &egui::Context,
        s: &PortalSettings,
        entry: Option<AuditEntry>,
        msg: String,
        act: impl FnOnce(&vf_portal::Portal) -> vf_portal::api::Result<()> + Send + 'static,
    ) {
        let client = self.client.clone();
        let s = s.clone();
        let audit = self.audit.clone();
        self.err = None;
        self.job = Some(spawn(ctx, move || {
            let p = core::client(&s);
            let done = p
                .put_client(&client.id, &client.code, &client.name, None)
                .and_then(|()| act(&p))
                .map(|()| {
                    if let Some(e) = entry {
                        audit.send(e);
                    }
                    msg
                })
                .map_err(|e| e.message());
            (p.users(&client.id).ok(), done)
        }));
    }

    pub fn show(&mut self, ctx: &egui::Context, s: &PortalSettings) {
        if let Some(rx) = &self.job
            && let Ok((users, done)) = rx.try_recv()
        {
            self.job = None;
            if users.is_some() {
                self.users = users;
            }
            match done {
                Ok(m) if !m.is_empty() => self.msg = Some(m),
                Ok(_) => {}
                Err(e) => self.err = Some(e),
            }
        }
        if let Some(rx) = &self.finding
            && let Ok((q, r)) = rx.try_recv()
        {
            self.finding = None;
            match r {
                Ok(people) => self.found = Some((q, people)),
                Err(e) => {
                    self.found = None;
                    self.err = Some(e);
                }
            }
        }
        let finding = self.finding.is_some();
        let busy = self.job.is_some();
        let mut open = true;
        let mut act: Option<(String, bool)> = None;
        let mut withdraw: Option<String> = None;
        let mut resend: Option<(Option<String>, Option<String>, String, String)> = None;
        egui::Window::new(format!("Portal users — {}", self.client.name))
            .id(egui::Id::unique("portal-people"))
            .open(&mut open)
            .collapsible(false)
            .default_width(520.0)
            .show(ctx, |ui| {
                match &self.users {
                    None => {
                        ui.label(widgets::caption("Reading the portal…"));
                    }
                    Some(u) => {
                        if u.users.is_empty() && u.invitations.is_empty() {
                            ui.label("Nobody has portal access to this client yet.");
                        }
                        for x in &u.users {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(x.label()).strong());
                                let contact =
                                    x.email.clone().or(x.phone.clone()).unwrap_or_default();
                                ui.label(widgets::caption(contact));
                                if !x.role_label.is_empty() {
                                    ui.label(widgets::caption(&x.role_label));
                                }
                                let last = x
                                    .last_sign_in_at
                                    .as_deref()
                                    .map(widgets::local_time)
                                    .unwrap_or_else(|| "never".into());
                                ui.label(widgets::caption(format!("last sign-in {last}")));
                                let (text, to) = if x.active {
                                    ("Turn off", false)
                                } else {
                                    ("Turn on", true)
                                };
                                if ui
                                    .add_enabled(!busy, egui::Button::new(text).small().min_size(crate::ui::widgets::MIN_TARGET))
                                    .clicked()
                                {
                                    act = Some((x.identity_id.clone(), to));
                                }
                            });
                        }
                        for i in &u.invitations {
                            ui.horizontal_wrapped(|ui| {
                                let who = i.email.clone().or(i.phone.clone()).unwrap_or_default();
                                ui.label(who);
                                ui.label(widgets::caption(if i.status == "expired" {
                                    "invitation expired".to_string()
                                } else {
                                    format!("invited, until {}", widgets::local_time(&i.expires_at))
                                }));
                                if ui
                                    .add_enabled(!busy, egui::Button::new("Resend").small().min_size(crate::ui::widgets::MIN_TARGET))
                                    .clicked()
                                {
                                    resend = Some((
                                        i.email.clone(),
                                        i.phone.clone(),
                                        i.name.clone(),
                                        i.role_label.clone(),
                                    ));
                                }
                                if ui
                                    .add_enabled(!busy, egui::Button::new("Withdraw").small().min_size(crate::ui::widgets::MIN_TARGET))
                                    .clicked()
                                {
                                    withdraw = Some(i.invitation_id.clone());
                                }
                            });
                        }
                    }
                }
                ui.separator();
                // Someone already on the portal (another client's): found by name, email or
                // mobile, their details go into the invitation (Vibe-Portal P63; owner
                // 2026-11-11). They still accept it, and keep one login.
                let mut picked: Option<Person> = None;
                let mut search = false;
                egui::CollapsingHeader::new("Someone who already uses the portal…")
                    .id_salt("portal-people-find")
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let l = ui.label("Find");
                            let r = ui
                                .add(
                                    egui::TextEdit::singleline(&mut self.find)
                                        .hint_text("name, email or mobile")
                                        .desired_width(220.0),
                                )
                                .labelled_by(l.id);
                            let ok = self.find.trim().chars().count() >= 2 && !finding;
                            let enter =
                                r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            if ui.add_enabled(ok, egui::Button::new("Search")).clicked()
                                || (ok && enter)
                            {
                                search = true;
                            }
                        });
                        if finding {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(widgets::caption("Searching the portal…"));
                            });
                        }
                        match &self.found {
                            Some((q, people)) if people.is_empty() => {
                                ui.label(widgets::caption(format!(
                                    "Nobody on the portal matches \"{q}\"."
                                )));
                            }
                            Some((_, people)) => {
                                for p in people {
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label(RichText::new(p.label()).strong());
                                        let contact = p
                                            .email
                                            .clone()
                                            .or(p.phone.clone())
                                            .unwrap_or_default();
                                        if contact != p.label() {
                                            ui.label(widgets::caption(contact));
                                        }
                                        if !p.clients.is_empty() {
                                            let names: Vec<&str> =
                                                p.clients.iter().map(|c| c.name.as_str()).collect();
                                            ui.label(widgets::caption(names.join(" · ")));
                                        }
                                        if p.sees(&self.client.id) {
                                            ui.label(widgets::caption("already has access"));
                                        } else if ui
                                            .add_enabled(!busy, egui::Button::new("Invite").small().min_size(crate::ui::widgets::MIN_TARGET))
                                            .on_hover_text(
                                                "Fill in the invitation below with their details",
                                            )
                                            .clicked()
                                        {
                                            picked = Some(p.clone());
                                        }
                                    });
                                }
                                if people.len() >= PEOPLE_SHOWN {
                                    ui.label(widgets::caption(
                                        "Only the first 20 are shown: type more to narrow it.",
                                    ));
                                }
                            }
                            None => {}
                        }
                    });
                if search {
                    let q = self.find.trim().to_string();
                    let s = s.clone();
                    self.err = None;
                    self.finding = Some(spawn(ctx, move || {
                        let r = core::client(&s).people(&q).map_err(|e| people_error(&e));
                        (q, r)
                    }));
                }
                let open_invite = picked.is_some();
                if let Some(p) = picked {
                    self.invite = invite_for(&p);
                    self.msg = Some(format!(
                        "{} is filled in below: add the role and press Send invitation. They accept it as usual and keep one login.",
                        p.label()
                    ));
                }
                if self.invite.show(ui, busy, open_invite) {
                    let f = std::mem::take(&mut self.invite);
                    resend = Some((
                        f.email(),
                        f.phone(),
                        f.name.trim().to_string(),
                        f.role.trim().to_string(),
                    ));
                }
                if busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(widgets::caption("Working…"));
                    });
                }
                if let Some(m) = &self.msg {
                    ui.label(m.as_str());
                }
                widgets::message_line(ui, self.err.as_deref());
            });
        let client = self.client.id.clone();
        if let Some((id, on)) = act {
            let entry = AuditEntry::new(
                if on {
                    "portal_user_on"
                } else {
                    "portal_user_off"
                },
                None,
                serde_json::json!({ "client_id": client, "identity_id": id }),
            );
            let c = client.clone();
            self.run(ctx, s, Some(entry), String::new(), move |p| {
                p.set_user_active(&c, &id, on)
            });
        } else if let Some(inv) = withdraw {
            let entry = AuditEntry::new(
                "portal_invite_withdrawn",
                None,
                serde_json::json!({ "client_id": client, "invitation_id": inv }),
            );
            let c = client.clone();
            self.run(
                ctx,
                s,
                Some(entry),
                "Invitation withdrawn.".into(),
                move |p| p.withdraw_invite(&c, &inv),
            );
        } else if let Some((email, phone, name, role)) = resend {
            let entry = invite_audit(&client, email.is_some(), &role);
            let c = client.clone();
            self.run(ctx, s, Some(entry), "Invitation sent.".into(), move |p| {
                p.invite(&c, email.as_deref(), phone.as_deref(), &name, &role)
                    .map(|_| ())
            });
        }
        if !open {
            self.closed = true;
        }
    }
}

// ======================================================================================
// Admin › Portal and Admin › Portal requests.
// ======================================================================================

/// Requests read again (None after a failed read), and what the action did.
type RequestsDone = (Option<Vec<AccessRequest>>, Result<(), String>);

#[derive(Default)]
pub struct AdminPortal {
    loaded: bool,
    url: String,
    key: String,
    minutes: String,
    status: Option<Receiver<Result<String, String>>>,
    requests: Option<Vec<AccessRequest>>,
    req_job: Option<Receiver<RequestsDone>>,
    /// Per request: the client chosen and the role typed.
    pick: BTreeMap<String, (Option<usize>, String)>,
    clients: Vec<ClientRef>,
    /// The portal settings and the received list, read at most every 10 s (the cabinet is
    /// often on a share; QA 2026-11-09).
    cached_settings: Option<PortalSettings>,
    received: Vec<vf_portal::inbox::Received>,
    read_at: Option<Instant>,
    confirm_stop: bool,
    /// Audit rows of request decisions when there is no app channel (tests).
    local_audit: Option<Receiver<AuditEntry>>,
    msg: Option<String>,
    err: Option<String>,
    /// Where the request threads send audit rows (the app's portal state); `None` in tests,
    /// where they are recorded here.
    pub audit: Option<AuditOut>,
    /// Admin › Portal changed the settings: Browse should read them again.
    pub changed: bool,
    /// "Fetch now" was pressed.
    pub fetch_now: bool,
}

impl AdminPortal {
    #[cfg(test)]
    pub fn fill(&mut self, url: &str, key: &str, minutes: &str) {
        self.loaded = true;
        self.url = url.into();
        self.key = key.into();
        self.minutes = minutes.into();
    }

    #[cfg(test)]
    pub fn error(&self) -> Option<&str> {
        self.err.as_deref()
    }

    /// Reads the requests (and the client list) again when the page is next shown.
    pub fn refresh_requests(&mut self) {
        if self.req_job.is_none() {
            self.requests = None;
        }
        self.clients.clear();
        self.err = None;
        self.read_at = None;
    }

    fn load(&mut self, root: &Path) {
        if self
            .read_at
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(10))
        {
            self.read_at = Some(Instant::now());
            self.cached_settings = cabinet_id(root).and_then(|id| settings::load(root, &id));
            self.received = core::recent_received(root, 30);
        }
        if self.loaded {
            return;
        }
        self.loaded = true;
        let s = self.cached_settings.as_ref();
        self.url = s.map(|s| s.url.clone()).unwrap_or_default();
        self.key = s.map(|s| s.key.clone()).unwrap_or_default();
        self.minutes = s
            .map_or(settings::DEFAULT_FETCH_MINUTES, |s| s.fetch_minutes)
            .to_string();
    }
}

/// Admin › Portal: the address, the key, Test, and what clients sent lately.
pub fn admin_settings(ui: &mut egui::Ui, a: &mut AdminPortal, env: &mut Env) {
    a.load(env.root);
    if let Some(rx) = &a.status
        && let Ok(r) = rx.try_recv()
    {
        a.status = None;
        match r {
            Ok(m) => {
                a.msg = Some(m);
                a.err = None;
            }
            Err(e) => a.err = Some(e),
        }
    }
    ui.heading("Portal");
    ui.label(widgets::caption(
        "Your firm's Vibe Portal: share files with clients and receive theirs. The address and key \
         are kept in the cabinet for every PC; make the key on the portal's staff page \
         (Settings › Portal keys) or on the appliance (sudo vibe portal key create).",
    ));
    ui.add_space(6.0);
    egui::Grid::new("portal-settings")
        .num_columns(2)
        .show(ui, |ui| {
            let l = ui.label("Portal address");
            ui.add(
                egui::TextEdit::singleline(&mut a.url)
                    .hint_text("https://files.yourfirm.com")
                    .desired_width(360.0),
            )
            .labelled_by(l.id);
            ui.end_row();
            let l = ui.label("Portal key");
            ui.add(
                egui::TextEdit::singleline(&mut a.key)
                    .password(true)
                    .hint_text("vpk_…")
                    .desired_width(360.0),
            )
            .labelled_by(l.id);
            ui.end_row();
            let l = ui.label("Fetch client uploads every (minutes, 0 = by hand)");
            ui.add(egui::TextEdit::singleline(&mut a.minutes).desired_width(60.0))
                .labelled_by(l.id);
            ui.end_row();
        });
    let busy = a.status.is_some();
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!busy, widgets::primary_button("Save"))
            .clicked()
        {
            save(a, env);
        }
        if ui.add_enabled(!busy, egui::Button::new("Test")).clicked() {
            match cabinet_id(env.root)
                .ok_or_else(|| "The cabinet cannot be read.".to_string())
                .and_then(|id| {
                    PortalSettings::new(&id, &a.url, &a.key, a.minutes.trim().parse().unwrap_or(5))
                }) {
                Ok(s) => {
                    a.msg = None;
                    a.err = None;
                    a.status = Some(spawn(ui.ctx(), move || {
                        core::client(&s)
                            .status()
                            .map(|st| {
                                format!(
                                    "Connected to {}'s portal (version {}). {}",
                                    st.firm_name,
                                    st.version,
                                    st.storage_words()
                                )
                            })
                            .map_err(|e| e.message())
                    }));
                }
                Err(e) => a.err = Some(e),
            }
        }
        if ui
            .add_enabled(!busy, egui::Button::new("Fetch uploads now"))
            .clicked()
        {
            a.fetch_now = true;
            a.msg = Some("Fetching; new files appear in Intake's Portal inbox.".into());
        }
        if !a.confirm_stop
            && ui
                .add_enabled(!busy, egui::Button::new("Stop using the portal"))
                .on_hover_text("Every PC stops calling the portal. The portal keeps its files.")
                .clicked()
        {
            a.confirm_stop = true;
        }
        if busy {
            ui.spinner();
        }
    });
    if a.confirm_stop {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Stop using the portal on every PC?").color(theme::warn()));
            if ui.button("Stop").clicked() {
                a.confirm_stop = false;
                stop(a, env);
            }
            if ui.button("Keep using it").clicked() {
                a.confirm_stop = false;
            }
        });
    }
    if let Some(m) = &a.msg {
        ui.label(m.as_str());
    }
    widgets::message_line(ui, a.err.as_deref());
    ui.separator();
    ui.label(RichText::new("Received from clients lately").strong());
    if a.received.is_empty() {
        ui.label(widgets::caption("Nothing yet."));
    }
    egui::ScrollArea::vertical()
        .max_height(280.0)
        .show(ui, |ui| {
            for r in &a.received {
                ui.horizontal_wrapped(|ui| {
                    ui.label(widgets::local_time(&r.at));
                    ui.label(RichText::new(&r.name).strong());
                    ui.label(widgets::caption(format!("{} · from {}", r.client, r.from)));
                    if !r.note.is_empty() {
                        ui.label(format!("“{}”", r.note));
                    }
                });
            }
        });
}

fn stop(a: &mut AdminPortal, env: &mut Env) {
    a.msg = None;
    a.err = None;
    // Said when it fails (a read-only share): every PC would otherwise keep using it.
    if let Err(e) = settings::clear(env.root) {
        a.err = Some(format!("Not stopped: {e}"));
        return;
    }
    let unrecorded = record(
        env,
        "portal_settings",
        vec![AuditEntry::new(
            "portal_settings",
            None,
            serde_json::json!({ "stopped": true }),
        )],
    )
    .unwrap_or_default();
    a.url.clear();
    a.key.clear();
    a.changed = true;
    a.read_at = None;
    a.msg = Some(format!("The portal is no longer used.{unrecorded}"));
}

fn save(a: &mut AdminPortal, env: &mut Env) {
    a.msg = None;
    a.err = None;
    let Some(id) = cabinet_id(env.root) else {
        a.err = Some("The cabinet cannot be read.".into());
        return;
    };
    let minutes = match a.minutes.trim().parse::<u32>() {
        Ok(m) => m,
        Err(_) => {
            a.err = Some("Minutes must be a whole number (0 = fetch by hand).".into());
            return;
        }
    };
    let s = match PortalSettings::new(&id, &a.url, &a.key, minutes) {
        Ok(s) => s,
        Err(e) => {
            a.err = Some(e);
            return;
        }
    };
    if let Err(e) = settings::save(env.root, &s) {
        a.err = Some(format!("Not saved: {e}"));
        return;
    }
    let inbox = match core::register_inbox(env.root) {
        Ok(true) => format!(
            " {} was added to Intake's inboxes (Reload Intake to see it).",
            core::INBOX
        ),
        Ok(false) => String::new(),
        Err(e) => format!(" The Portal inbox could not be set up: {e}"),
    };
    let unrecorded = record(
        env,
        "portal_settings",
        vec![AuditEntry::new(
            "portal_settings",
            None,
            serde_json::json!({ "url": s.url, "fetch_minutes": s.fetch_minutes }),
        )],
    )
    .unwrap_or_default();
    a.url = s.url.clone();
    a.changed = true;
    a.read_at = None;
    a.msg = Some(format!("Saved for every PC.{inbox}{unrecorded}"));
}

fn load_clients(env: &mut Env) -> Vec<ClientRef> {
    let mut stmt = match env
        .conn
        .prepare("SELECT id, ext_id, name FROM client WHERE deleted_at IS NULL ORDER BY name")
    {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    stmt.query_map([], |r| {
        Ok(ClientRef {
            id: r.get(0)?,
            code: r.get(1)?,
            name: r.get(2)?,
        })
    })
    .map(|rows| rows.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

/// Admin › Portal requests: confirmed "Request access" forms; link each to a client, or deny.
pub fn admin_requests(ui: &mut egui::Ui, a: &mut AdminPortal, env: &mut Env) {
    a.load(env.root);
    let Some(s) = a.cached_settings.clone() else {
        ui.heading("Portal requests");
        ui.label("Set up the portal in Admin › Portal first.");
        return;
    };
    if a.clients.is_empty() {
        a.clients = load_clients(env);
    }
    if let Some(rx) = &a.req_job
        && let Ok((list, done)) = rx.try_recv()
    {
        a.req_job = None;
        if let Err(e) = done {
            a.err = Some(e);
        }
        if let Some(l) = list {
            a.requests = Some(l);
        }
    }
    if a.requests.is_none() && a.req_job.is_none() && a.err.is_none() {
        let s2 = s.clone();
        a.req_job = Some(spawn(ui.ctx(), move || {
            match core::client(&s2).access_requests() {
                Ok(l) => (Some(l), Ok(())),
                Err(e) => (None, Err(e.message())),
            }
        }));
    }
    ui.heading("Portal requests");
    ui.label(widgets::caption(
        "People who asked for portal access and confirmed their email or phone. Link each to the \
         client whose files they should see, or deny it. Only the confirmed contact is used.",
    ));
    let busy = a.req_job.is_some();
    if ui
        .add_enabled(!busy, egui::Button::new("Refresh"))
        .clicked()
    {
        a.err = None;
        a.requests = None;
    }
    let mut decision: Option<(String, Option<(ClientRef, String)>)> = None;
    match &a.requests {
        None => {
            ui.label(widgets::caption("Reading the portal…"));
        }
        Some(list) if list.is_empty() => {
            ui.label("No requests waiting.");
        }
        Some(list) => {
            let labels: Vec<String> = a
                .clients
                .iter()
                .map(|c| {
                    if c.code.is_empty() {
                        c.name.clone()
                    } else {
                        format!("{} ({})", c.name, c.code)
                    }
                })
                .collect();
            for r in list {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.label(RichText::new(&r.name).strong());
                    ui.label(format!(
                        "Confirmed: {}    Asked for: {}",
                        r.confirmed_contact.as_deref().unwrap_or("?"),
                        r.client_as
                    ));
                    if !r.note.is_empty() {
                        ui.label(widgets::caption(format!("“{}”", r.note)));
                    }
                    let (pick, role) = a.pick.entry(r.id.clone()).or_insert((None, String::new()));
                    ui.horizontal_wrapped(|ui| {
                        let shown = pick
                            .and_then(|i| labels.get(i).cloned())
                            .unwrap_or_else(|| "Choose the client…".into());
                        let l2 = labels.clone();
                        let (_, chosen) = widgets::search_combo(
                            ui,
                            ("portal-req", &r.id),
                            280.0,
                            shown,
                            move || l2,
                            *pick,
                            None,
                        );
                        if let Some(c) = chosen {
                            *pick = c;
                        }
                        let l = ui.label("Role");
                        ui.add(egui::TextEdit::singleline(role).desired_width(100.0))
                            .labelled_by(l.id);
                        let ok = pick.is_some() && !busy;
                        if ui.add_enabled(ok, egui::Button::new("Approve")).clicked()
                            && let Some(c) = pick.and_then(|i| a.clients.get(i))
                        {
                            decision =
                                Some((r.id.clone(), Some((c.clone(), role.trim().to_string()))));
                        }
                        if ui.add_enabled(!busy, egui::Button::new("Deny")).clicked() {
                            decision = Some((r.id.clone(), None));
                        }
                    });
                });
            }
        }
    }
    if let Some((req, to)) = decision {
        let out = a.audit.clone();
        // Without the app's channel (tests), the row comes back here to be recorded.
        let (tx, rx) = mpsc::channel::<AuditEntry>();
        let local = out.is_none();
        let audit = out.unwrap_or(AuditOut(tx));
        a.err = None;
        a.req_job = Some(spawn(ui.ctx(), move || {
            let p = core::client(&s);
            let done = match &to {
                Some((c, role)) => p
                    .put_client(&c.id, &c.code, &c.name, None)
                    .and_then(|()| p.approve(&req, &c.id, role))
                    .map(|()| {
                        audit.send(AuditEntry::new(
                            "portal_approve",
                            None,
                            serde_json::json!({ "request": req, "client_id": c.id, "role": role }),
                        ));
                    }),
                None => p.deny(&req).map(|()| {
                    audit.send(AuditEntry::new(
                        "portal_deny",
                        None,
                        serde_json::json!({ "request": req }),
                    ));
                }),
            }
            .map_err(|e| e.message());
            (p.access_requests().ok(), done)
        }));
        if local {
            a.local_audit = Some(rx);
        }
    }
    if let Some(rx) = &a.local_audit {
        let rows: Vec<AuditEntry> = rx.try_iter().collect();
        if let Some(e) = record(env, "portal", rows) {
            a.err = Some(e);
        }
    }
    widgets::message_line(ui, a.err.as_deref());
}

// ======================================================================================
// Send secure link window (the document pane's Send secure link…, Vibe-Portal P68).
// ======================================================================================

/// Days a new secure link works (owner 2026-11-11: view and download, 7 days).
pub const LINK_DAYS: u32 = 7;
/// The most days Vibe Files offers (the portal allows more; the owner's choice).
pub const LINK_MAX_DAYS: u32 = 30;

/// Why the portal refused the link itself, in plain words, and what was already done on the
/// portal for it: `added`, the document was put there shown to nobody (Send again does not
/// send it twice); `updated`, the client's people's copy was brought up to date.
fn link_error(e: &vf_portal::api::PortalError, added: bool, updated: bool) -> String {
    // An unknown route answers 404 not_found: a portal from before P68.
    let words = if e.status() == Some(404) && e.code() == Some("not_found") {
        "This portal cannot send secure links from Vibe Files yet: update Vibe Portal, or send it from the portal's staff page.".to_string()
    } else {
        e.message()
    };
    if added {
        format!(
            "{words} The document is on the portal now, shown to none of the client's people; Send again does not send it twice."
        )
    } else if updated {
        format!(
            "{words} The portal's copy was already brought up to date: the client's people have the newer version."
        )
    } else {
        words
    }
}

/// The client's portal copy of the document, as the window found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortalCopy {
    /// Not on the portal for this client: sent first, shown to nobody.
    Absent,
    /// On the portal as it is in the cabinet; `shown`: the client's people who see it.
    Same { file_id: String, shown: usize },
    /// On the portal, older than the cabinet's.
    Older { file_id: String, shown: usize },
}

/// What the portal has of `doc` for `client`: this client's files only (a copy the document
/// left under another client, having moved since, is not this client's).
fn copy_of(
    p: &vf_portal::api::Portal,
    client: &ClientRef,
    d: &DocRef,
) -> Result<PortalCopy, String> {
    let files = p.files_for_client(&client.id).map_err(|e| e.message())?;
    Ok(match files.into_iter().find(|f| f.vf_document_id == d.id) {
        None => PortalCopy::Absent,
        Some(f) => {
            let shown = f.users.iter().filter(|u| u.visible).count();
            if f.sha256 == d.hash {
                PortalCopy::Same {
                    file_id: f.file_id,
                    shown,
                }
            } else {
                PortalCopy::Older {
                    file_id: f.file_id,
                    shown,
                }
            }
        }
    })
}

enum LinkJob {
    Found(PortalCopy),
    /// The portal's copy changed between the window's look and Send: nothing sent.
    Changed(PortalCopy),
    Sent(String),
}

/// Sends one document of a client to someone outside the client by a secure link: the
/// document goes to the portal first when it is not there (shown to nobody); an older portal
/// copy is brought up to date first unless the client's people see it and the person chose not
/// to change what they see; then the portal emails the link. Nothing is sent from a copy the
/// person was not shown.
pub struct SendLinkWindow {
    client: ClientRef,
    doc: DocRef,
    pub email: String,
    pub name: String,
    pub organization: String,
    pub message: String,
    pub can_download: bool,
    pub watermark: bool,
    pub days: u32,
    /// The client consented to sending this to this person (IRS 7216): asked every time, and
    /// again when the address changes.
    pub consent: bool,
    consent_for: String,
    /// The portal copy, once read.
    pub copy: Option<PortalCopy>,
    /// An older copy the client's people see: send the cabinet's newer version (they see it
    /// too and are told it was updated). Off unless chosen.
    pub update_shared: bool,
    job: Option<Receiver<Result<LinkJob, String>>>,
    /// The job running sends a link (not only reads the portal).
    sending: bool,
    audit: AuditOut,
    pub msg: Option<String>,
    pub err: Option<String>,
    pub closed: bool,
}

impl SendLinkWindow {
    pub fn new(
        ctx: &egui::Context,
        s: &PortalSettings,
        client: ClientRef,
        doc: DocRef,
        audit: AuditOut,
    ) -> Self {
        let mut w = Self {
            client,
            doc,
            email: String::new(),
            name: String::new(),
            organization: String::new(),
            message: String::new(),
            can_download: true,
            watermark: true,
            days: LINK_DAYS,
            consent: false,
            consent_for: String::new(),
            copy: None,
            update_shared: false,
            job: None,
            sending: false,
            audit,
            msg: None,
            err: None,
            closed: false,
        };
        w.read(ctx, s, true);
        w
    }

    /// A link is being sent: the window stays, and no other replaces it.
    pub fn sending(&self) -> bool {
        self.sending
    }

    fn ready(&self) -> bool {
        let e = self.email.trim();
        self.job.is_none()
            && self.copy.is_some()
            && self.consent
            && e.contains('@')
            && !e.contains(char::is_whitespace)
    }

    /// Reads the portal copy (`first`: the client is put on the portal first, as Share… does).
    fn read(&mut self, ctx: &egui::Context, s: &PortalSettings, first: bool) {
        let (s2, c2, d2) = (s.clone(), self.client.clone(), self.doc.clone());
        self.sending = false;
        self.job = Some(spawn(ctx, move || {
            let p = core::client(&s2);
            if first {
                p.put_client(&c2.id, &c2.code, &c2.name, None)
                    .map_err(|e| e.message())?;
            }
            copy_of(&p, &c2, &d2).map(LinkJob::Found)
        }));
    }

    fn send(&mut self, ctx: &egui::Context, s: &PortalSettings) {
        let Some(seen) = self.copy.clone() else {
            return;
        };
        let (s, client, d, audit) = (
            s.clone(),
            self.client.clone(),
            self.doc.clone(),
            self.audit.clone(),
        );
        let update_shared = self.update_shared;
        let link = vf_portal::api::OutsideLink {
            file_ids: vec![],
            email: self.email.trim().to_string(),
            name: self.name.trim().to_string(),
            organization: self.organization.trim().to_string(),
            message: self.message.trim().to_string(),
            can_download: self.can_download,
            watermark: self.watermark,
            days: self.days.clamp(1, LINK_MAX_DAYS),
        };
        self.msg = None;
        self.err = None;
        self.sending = true;
        self.job = Some(spawn(ctx, move || {
            let p = core::client(&s);
            let plain = |e: vf_portal::api::PortalError| e.message();
            // Read again now: another PC may have shared or changed it since the window looked.
            // A copy other than the one shown is shown first; nothing is sent from it unseen.
            let now = copy_of(&p, &client, &d)?;
            if now != seen {
                return Ok(LinkJob::Changed(now));
            }
            let (file_id, added, updated) = match now {
                PortalCopy::Same { file_id, .. } => (file_id, false, false),
                // The client's people see it: their copy changes only when chosen.
                PortalCopy::Older { file_id, shown } if shown > 0 && !update_shared => {
                    (file_id, false, false)
                }
                PortalCopy::Older { file_id, .. } => {
                    p.replace(&file_id, &d.name, &d.path).map_err(plain)?;
                    (file_id, false, true)
                }
                PortalCopy::Absent => {
                    match p
                        .add(&client.id, &d.id, &d.name, d.tax_year, &d.path)
                        .map_err(plain)?
                    {
                        vf_portal::api::Added::New(f) => (f, true, false),
                        // Another PC put this same content there a moment ago.
                        vf_portal::api::Added::Existing(f) => (f, false, false),
                        // Another PC put another version there a moment ago: shown first.
                        vf_portal::api::Added::Changed(_) => {
                            return copy_of(&p, &client, &d).map(LinkJob::Changed);
                        }
                    }
                }
            };
            if added || updated {
                audit.send(AuditEntry::new(
                    "portal_share",
                    Some(&d.id),
                    serde_json::json!({
                        "client_id": client.id, "sent": added, "updated": updated,
                        "for_secure_link": true,
                    }),
                ));
            }
            let link = vf_portal::api::OutsideLink {
                file_ids: vec![file_id],
                ..link
            };
            let until = p
                .send_outside(&client.id, &link)
                .map_err(|e| link_error(&e, added, updated))?;
            audit.send(AuditEntry::new(
                "portal_secure_link",
                Some(&d.id),
                serde_json::json!({
                    "client_id": client.id,
                    "to": link.email,
                    "name": link.name,
                    "organization": link.organization,
                    "days": link.days,
                    "can_download": link.can_download,
                    "watermark": link.watermark,
                    "expires_at": until,
                    "client_consent_confirmed": true,
                }),
            ));
            Ok(LinkJob::Sent(format!(
                "Secure link sent to {} (works until {}).",
                link.email,
                widgets::local_time(&until)
            )))
        }));
    }

    pub fn show(&mut self, ctx: &egui::Context, s: &PortalSettings) {
        if let Some(rx) = &self.job
            && let Ok(r) = rx.try_recv()
        {
            self.job = None;
            let was_sending = std::mem::take(&mut self.sending);
            match r {
                Ok(LinkJob::Found(c)) => self.copy = Some(c),
                Ok(LinkJob::Changed(c)) => {
                    self.copy = Some(c);
                    self.update_shared = false;
                    self.err = Some(
                        "The portal's copy changed since this window looked: check it above, then press Send again. Nothing was sent.".into(),
                    );
                }
                Ok(LinkJob::Sent(m)) => {
                    self.msg = Some(m);
                    // A second link is a new decision: asked again.
                    self.consent = false;
                    self.copy = None;
                    self.read(ctx, s, false);
                }
                Err(e) => {
                    self.err = Some(e);
                    // What the portal has now (a link that failed after the document went up).
                    if was_sending {
                        self.copy = None;
                        self.read(ctx, s, false);
                    }
                }
            }
        }
        let busy = self.job.is_some();
        let mut open = true;
        let mut go = false;
        let mut again = false;
        egui::Window::new(format!("Send secure link — {}", self.doc.name))
            .id(egui::Id::unique("portal-send-link"))
            .open(&mut open)
            .collapsible(false)
            .default_width(480.0)
            .show(ctx, |ui| {
                ui.label(widgets::caption(format!(
                    "Sends this document of {} to someone outside the client (a lender, a lawyer). They get an email with a private link, and a code sent to that address opens it.",
                    self.client.name
                )));
                match &self.copy {
                    None if busy => {
                        ui.label(widgets::caption("Reading the portal…"));
                    }
                    None => {
                        // The portal could not be read: try again without reopening.
                        if ui.button("Read the portal again").clicked() {
                            again = true;
                        }
                    }
                    Some(PortalCopy::Absent) => {
                        ui.label(widgets::caption(
                            "Not on the portal yet: it is sent there first, shown to none of the client's portal people.",
                        ));
                    }
                    Some(PortalCopy::Same { .. }) => {
                        ui.label(widgets::caption("On the portal as it is in the cabinet."));
                    }
                    Some(PortalCopy::Older { shown: 0, .. }) => {
                        ui.label(widgets::caption(
                            "The portal's copy is older than the cabinet's: the newer one goes up first.",
                        ));
                    }
                    Some(PortalCopy::Older { shown, .. }) => {
                        let n = *shown;
                        ui.label(
                            RichText::new(format!(
                                "The portal's copy is older than the cabinet's, and {n} of the client's portal people see it."
                            ))
                            .color(theme::warn()),
                        );
                        ui.add_enabled(
                            !busy,
                            egui::Checkbox::new(
                                &mut self.update_shared,
                                "Send the cabinet's newer version (the client's people see it too and are told it was updated)",
                            ),
                        );
                        if !self.update_shared {
                            ui.label(widgets::caption(
                                "Unticked: the link gives the portal's older copy, and what the client sees does not change.",
                            ));
                        }
                    }
                }
                ui.add_space(4.0);
                egui::Grid::new("portal-send-link-grid")
                    .num_columns(2)
                    .show(ui, |ui| {
                        for (label, value) in [
                            ("Their email", &mut self.email),
                            ("Their name", &mut self.name),
                            ("Organization", &mut self.organization),
                        ] {
                            let l = ui.label(label);
                            ui.add_enabled(!busy, egui::TextEdit::singleline(value))
                                .labelled_by(l.id);
                            ui.end_row();
                        }
                        let l = ui.label("Message");
                        ui.add_enabled(
                            !busy,
                            egui::TextEdit::multiline(&mut self.message).desired_rows(3),
                        )
                        .labelled_by(l.id);
                        ui.end_row();
                        let l = ui.label("Days the link works");
                        ui.add_enabled(
                            !busy,
                            egui::DragValue::new(&mut self.days).range(1..=LINK_MAX_DAYS),
                        )
                        .labelled_by(l.id);
                        ui.end_row();
                    });
                ui.add_enabled(
                    !busy,
                    egui::Checkbox::new(&mut self.can_download, "Allow download (not only view)"),
                )
                .on_hover_text("View only works for PDFs and pictures the page can show");
                ui.add_enabled(
                    !busy,
                    egui::Checkbox::new(&mut self.watermark, "Watermark PDFs with their name"),
                );
                ui.separator();
                // The consent is for this address: changing it asks again.
                if self.email.trim() != self.consent_for {
                    self.consent = false;
                }
                let tick = ui
                    .add_enabled(
                        !busy,
                        egui::Checkbox::new(
                            &mut self.consent,
                            "The client has consented to sending this to this person",
                        ),
                    )
                    .on_hover_text(
                        "Tax return information may be given to someone else only with the client's consent (IRS section 7216). Recorded in the audit trail with your name.",
                    );
                if tick.changed() {
                    self.consent_for = self.email.trim().to_string();
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(self.ready(), widgets::primary_button("Send"))
                        .clicked()
                    {
                        go = true;
                    }
                    if self.sending {
                        ui.spinner();
                        ui.label(widgets::caption("Sending…"));
                    }
                });
                if let Some(m) = &self.msg {
                    ui.label(m.as_str());
                }
                widgets::message_line(ui, self.err.as_deref());
            });
        if go {
            self.send(ctx, s);
        } else if again {
            self.err = None;
            self.read(ctx, s, true);
        }
        // Not closed while a link is being sent: its result (and its audit row) is shown here.
        // Reading the portal does not hold it open.
        if !open && !self.sending {
            self.closed = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;
    use vf_portal::api::FileUser;

    /// PA-38: Intake's portal line says what the last fetch here did and how often it runs.
    #[test]
    fn intake_line_reads_in_plain_words() {
        let mut r = core::FetchReport::default();
        assert_eq!(fetch_result(&r), "nothing new");
        r.problems.push("The portal cannot be reached".into());
        assert_eq!(fetch_result(&r), "problem: The portal cannot be reached");
        r.busy = true;
        assert_eq!(fetch_result(&r), "another PC was fetching");
        let one = vf_portal::inbox::Received {
            at: String::new(),
            upload_id: "u".into(),
            client_id: "c".into(),
            client: "Smith, John".into(),
            name: "W-2.pdf".into(),
            saved_as: "x".into(),
            note: String::new(),
            from: "a".into(),
            size: 1,
        };
        r.received = vec![one.clone()];
        assert_eq!(fetch_result(&r), "1 file received");
        r.received.push(one);
        assert_eq!(fetch_result(&r), "2 files received");

        assert_eq!(
            line_text(None, 15),
            "not fetched yet on this PC · every 15 minutes"
        );
        assert_eq!(
            line_text(Some(&("10:42".into(), "nothing new".into())), 0),
            "last fetched 10:42: nothing new · by hand only (Admin › Portal sets a timer)"
        );
    }

    /// Without portal settings Intake shows no portal line.
    #[test]
    fn no_portal_no_intake_line() {
        let t = tempfile::tempdir().unwrap();
        assert!(PortalState::default().intake_line(t.path()).is_none());
    }

    fn file(seen_by: &[&str]) -> Option<PortalFile> {
        Some(PortalFile {
            file_id: "f".into(),
            client_id: "c".into(),
            vf_document_id: "d".into(),
            name: "a.pdf".into(),
            size: 1,
            sha256: "h".into(),
            tax_year: None,
            shared_at: "2026-11-09T00:00:00Z".into(),
            shared_by: "x".into(),
            content_updated_at: None,
            users: seen_by
                .iter()
                .map(|id| FileUser {
                    identity_id: (*id).into(),
                    visible: true,
                    viewed_at: None,
                    first_viewed_at: None,
                })
                .collect(),
        })
    }

    /// A portal on this PC answering by path; every request is kept as "METHOD path body".
    fn fake_portal(
        answer: fn(&str, &str) -> (u16, String),
    ) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use std::io::{BufRead, BufReader, Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut first = String::new();
                if r.read_line(&mut first).is_err() {
                    continue;
                }
                let mut len = 0usize;
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0; len];
                let _ = r.read_exact(&mut body);
                let mut parts = first.split_whitespace();
                let (method, path) = (
                    parts.next().unwrap_or("").to_string(),
                    parts.next().unwrap_or("").to_string(),
                );
                let body = String::from_utf8_lossy(&body).to_string();
                log.lock().unwrap().push(format!("{method} {path} {body}"));
                let (status, text) = answer(&method, &path);
                let mut w = s;
                let _ = write!(
                    w,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                    text.len()
                );
            }
        });
        (url, seen)
    }

    /// Steps until `label` shows (the portal answers on a thread).
    fn wait_for<S>(h: &mut egui_kittest::Harness<'_, S>, label: &str) {
        let start = Instant::now();
        while h.query_by_label_contains(label).is_none() {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "never showed {label}"
            );
            std::thread::sleep(Duration::from_millis(20));
            h.step();
        }
    }

    /// Vibe-Portal P63 (owner 2026-11-11): someone who already uses the portal for another
    /// client is found by name and invited with their own details; someone who already sees
    /// this client cannot be invited again.
    #[test]
    fn someone_already_on_the_portal_is_found_and_invited() {
        const CLIENT: &str = "0199aaaa-0000-7000-8000-000000000001";
        let (url, seen) = fake_portal(|method, path| {
            match (method, path) {
            ("GET", p) if p.starts_with("/api/firm/v1/people") => (
                200,
                r#"{"people":[
                  {"identityId":"j","name":"Jane Doe","email":"jane@example.test","phone":"+15552010001",
                   "clients":[{"id":"other","name":"Doe Farms LLC"}]},
                  {"identityId":"b","name":"Bob Roe","email":"bob@example.test","phone":null,
                   "clients":[{"id":"0199AAAA-0000-7000-8000-000000000001","name":"Alerte Systems Inc."}]}
                ]}"#
                .into(),
            ),
            ("GET", _) => (200, r#"{"users":[],"invitations":[]}"#.into()),
            ("POST", _) => (201, r#"{"invitationId":"i9"}"#.into()),
            _ => (200, r#"{"ok":true}"#.into()),
        }
        });
        let s = PortalSettings {
            version: 1,
            cabinet_id: "c".into(),
            url,
            key: "vpk_test".into(),
            fetch_minutes: 0,
        };
        let (tx, audit) = mpsc::channel();
        let w = PeopleWindow::new(
            &egui::Context::default(),
            &s,
            ClientRef {
                id: CLIENT.into(),
                code: "ALER1176".into(),
                name: "Alerte Systems Inc.".into(),
            },
            AuditOut(tx),
        );
        let mut h = egui_kittest::Harness::builder()
            .with_size([700.0, 700.0])
            .build_ui_state(move |ui, w: &mut PeopleWindow| w.show(ui.ctx(), &s), w);
        wait_for(&mut h, "Nobody has portal access");
        h.get_by_label("Someone who already uses the portal…")
            .click();
        h.run();
        crate::ui::test_util::type_into(&mut h, "Find", "doe");
        h.get_by_label("Search").click();
        wait_for(&mut h, "Jane Doe");
        assert!(
            seen.lock()
                .unwrap()
                .iter()
                .any(|r| r.starts_with("GET /api/firm/v1/people?q=doe ")),
            "{:?}",
            seen.lock().unwrap()
        );
        // Their other clients show; Bob already sees this client.
        assert!(h.query_by_label("Doe Farms LLC").is_some());
        assert!(h.query_by_label("already has access").is_some());
        assert_eq!(h.query_all_by_label("Invite").count(), 1);
        h.get_by_label("Invite").click();
        h.run();
        assert_eq!(h.state().invite.email, "jane@example.test");
        assert_eq!(h.state().invite.phone, "", "the email is used, not both");
        assert_eq!(h.state().invite.name, "Jane Doe");
        crate::ui::test_util::type_into(&mut h, "Role (e.g. Spouse)", "Spouse");
        h.get_by_label("Send invitation").click();
        wait_for(&mut h, "Invitation sent.");
        let invite = seen
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.starts_with(&format!("POST /api/firm/v1/clients/{CLIENT}/invites")))
            .cloned()
            .expect("invitation sent");
        let body: serde_json::Value =
            serde_json::from_str(invite.splitn(3, ' ').nth(2).unwrap()).unwrap();
        assert_eq!(
            body,
            serde_json::json!({"email":"jane@example.test","name":"Jane Doe","roleLabel":"Spouse"})
        );
        assert_eq!(audit.try_iter().count(), 1, "the invitation is audited");
    }

    /// The window under test, the requests the fake portal saw, the audit rows, the folder.
    type LinkTest = (
        egui_kittest::Harness<'static, SendLinkWindow>,
        std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        mpsc::Receiver<AuditEntry>,
        tempfile::TempDir,
    );

    /// The cabinet file's content and its SHA-256.
    const DOC_BODY: &[u8] = b"%PDF-1.4 invented";
    const DOC_SHA: &str = "d3dd4eff6b51659883754487ab7a122ab89ccecc30afad6c7e34947decefbb3f";

    /// The portal's answer for the document: one copy for `client`, with `sha` and `visible`
    /// people.
    fn portal_copy(client: &str, sha: &str, visible: usize) -> String {
        let users: Vec<String> = (0..visible)
            .map(|i| {
                format!(
                    r#"{{"identityId":"p{i}","visible":true,"viewedAt":null,"firstViewedAt":null}}"#
                )
            })
            .collect();
        format!(
            r#"{{"files":[{{"fileId":"f9","clientId":"{client}","vfDocumentId":"doc-1","name":"Return 2025.pdf","size":17,"sha256":"{sha}","taxYear":2025,"sharedAt":"2026-11-01T00:00:00Z","sharedBy":"PC kurt","contentUpdatedAt":null,"users":[{}]}}]}}"#,
            users.join(",")
        )
    }

    fn link_window(answer: fn(&str, &str) -> (u16, String)) -> LinkTest {
        let (url, seen) = fake_portal(answer);
        let s = PortalSettings {
            version: 1,
            cabinet_id: "c".into(),
            url,
            key: "vpk_test".into(),
            fetch_minutes: 0,
        };
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("Return 2025.pdf");
        std::fs::write(&path, DOC_BODY).unwrap();
        assert_eq!(vf_portal::api::file_sha256(&path).unwrap(), DOC_SHA);
        let (tx, audit) = mpsc::channel();
        let w = SendLinkWindow::new(
            &egui::Context::default(),
            &s,
            ClientRef {
                id: "AB-1".into(),
                code: "1001".into(),
                name: "Smith, John".into(),
            },
            DocRef {
                id: "doc-1".into(),
                name: "Return 2025.pdf".into(),
                path,
                hash: DOC_SHA.into(),
                tax_year: Some(2025),
            },
            AuditOut(tx),
        );
        let h = egui_kittest::Harness::builder()
            .with_size([700.0, 800.0])
            .build_ui_state(move |ui, w: &mut SendLinkWindow| w.show(ui.ctx(), &s), w);
        (h, seen, audit, t)
    }

    fn fill_and_send(h: &mut egui_kittest::Harness<'static, SendLinkWindow>) {
        crate::ui::test_util::type_into(h, "Their email", "pat@first-bank.test");
        h.get_by_label("The client has consented to sending this to this person")
            .click();
        h.run();
        h.get_by_label("Send").click();
    }

    fn requests(seen: &std::sync::Arc<std::sync::Mutex<Vec<String>>>, start: &str) -> Vec<String> {
        seen.lock()
            .unwrap()
            .iter()
            .filter(|r| r.starts_with(start))
            .cloned()
            .collect()
    }

    /// The file ids of the link the window asked for.
    fn link_files(seen: &std::sync::Arc<std::sync::Mutex<Vec<String>>>) -> serde_json::Value {
        let r = requests(seen, "POST /api/firm/v1/shares");
        let body: serde_json::Value =
            serde_json::from_str(r[0].splitn(3, ' ').nth(2).unwrap()).unwrap();
        body["fileIds"].clone()
    }

    /// Vibe-Portal P68 (owner 2026-11-11): Send secure link… puts a document that is not on the
    /// portal there first (shown to nobody), then has the portal email the link; view and
    /// download for 7 days and a watermark unless changed; the client's consent ticked each time,
    /// and again when the address changes.
    #[test]
    fn a_secure_link_sends_the_document_first() {
        let (mut h, seen, audit, _t) = link_window(|method, path| match (method, path) {
            ("GET", p) if p.starts_with("/api/firm/v1/files?clientId=AB-1") => {
                (200, r#"{"files":[]}"#.into())
            }
            ("POST", p) if p.starts_with("/api/firm/v1/files") => {
                (201, r#"{"fileId":"f1"}"#.into())
            }
            ("POST", "/api/firm/v1/shares") => (
                201,
                r#"{"shareId":"s1","expiresAt":"2026-11-18T15:00:00.000Z"}"#.into(),
            ),
            _ => (200, r#"{"ok":true}"#.into()),
        });
        wait_for(&mut h, "Not on the portal yet");
        crate::ui::test_util::type_into(&mut h, "Their email", "pat@first-bank.test");
        crate::ui::test_util::type_into(&mut h, "Their name", "Pat Roe");
        // Not without the client's consent.
        h.get_by_label("Send").click();
        h.run();
        assert!(
            requests(&seen, "POST").is_empty(),
            "nothing sent without consent"
        );
        h.get_by_label("The client has consented to sending this to this person")
            .click();
        h.run();
        assert!(h.state().consent);
        // Another address: asked again.
        crate::ui::test_util::type_into(&mut h, "Their email", "x");
        assert!(!h.state().consent, "the consent was for another address");
        h.state_mut().email = "pat@first-bank.test".into();
        h.run();
        h.get_by_label("The client has consented to sending this to this person")
            .click();
        h.run();
        h.get_by_label("Send").click();
        wait_for(&mut h, "Secure link sent to pat@first-bank.test");
        let all = seen.lock().unwrap().clone();
        let upload = all
            .iter()
            .position(|r| r.starts_with("POST /api/firm/v1/files"))
            .expect("uploaded");
        let link = all
            .iter()
            .position(|r| r.starts_with("POST /api/firm/v1/shares"))
            .expect("link made");
        assert!(upload < link, "{all:?}");
        assert!(
            !all.iter().any(|r| r.contains("/visibility")),
            "shown to nobody: {all:?}"
        );
        let body: serde_json::Value =
            serde_json::from_str(all[link].splitn(3, ' ').nth(2).unwrap()).unwrap();
        assert_eq!(body["fileIds"], serde_json::json!(["f1"]));
        assert_eq!(body["clientId"], "AB-1");
        assert_eq!(body["canDownload"], true);
        assert_eq!(body["watermark"], true);
        assert_eq!(body["days"], 7);
        assert_eq!(body["name"], "Pat Roe");
        let rows: Vec<AuditEntry> = audit.try_iter().collect();
        let kinds: Vec<&str> = rows.iter().map(|e| e.action.as_str()).collect();
        assert_eq!(kinds, ["portal_share", "portal_secure_link"]);
        assert_eq!(rows[1].detail["client_consent_confirmed"], true);
        assert_eq!(rows[1].detail["to"], "pat@first-bank.test");
        // Asked again for the next link.
        assert!(!h.state().consent);
    }

    /// QA: an older copy the client's people see is not changed unless chosen; chosen, the
    /// newer version goes up before the link.
    #[test]
    fn an_older_copy_the_client_sees_changes_only_when_chosen() {
        fn answer(method: &str, path: &str) -> (u16, String) {
            match (method, path) {
                ("GET", p) if p.starts_with("/api/firm/v1/files?clientId=AB-1") => {
                    (200, portal_copy("AB-1", &"0".repeat(64), 2))
                }
                ("POST", "/api/firm/v1/shares") => (
                    201,
                    r#"{"shareId":"s1","expiresAt":"2026-11-18T15:00:00.000Z"}"#.into(),
                ),
                _ => (200, r#"{"ok":true}"#.into()),
            }
        }
        // Left unticked: the link gives the portal's copy, nothing replaced.
        let (mut h, seen, _audit, _t) = link_window(answer);
        wait_for(&mut h, "2 of the client's portal people see it");
        assert!(!h.state().update_shared);
        fill_and_send(&mut h);
        wait_for(&mut h, "Secure link sent");
        assert!(
            requests(&seen, "PUT /api/firm/v1/files/").is_empty(),
            "{:?}",
            seen.lock().unwrap()
        );
        assert!(requests(&seen, "POST /api/firm/v1/files").is_empty());
        assert_eq!(link_files(&seen), serde_json::json!(["f9"]));
        // Ticked: the newer version first.
        let (mut h, seen, audit, _t) = link_window(answer);
        wait_for(&mut h, "2 of the client's portal people see it");
        h.get_by_label_contains("Send the cabinet's newer version")
            .click();
        h.run();
        fill_and_send(&mut h);
        wait_for(&mut h, "Secure link sent");
        let all = seen.lock().unwrap().clone();
        let replace = all
            .iter()
            .position(|r| r.starts_with("PUT /api/firm/v1/files/f9/content"))
            .expect("replaced");
        let link = all
            .iter()
            .position(|r| r.starts_with("POST /api/firm/v1/shares"))
            .unwrap();
        assert!(replace < link);
        let first = audit.try_iter().next().unwrap();
        assert_eq!(first.detail["updated"], true);
    }

    /// QA: a copy filed under another client (the document moved since) is not this client's:
    /// the document is sent for this client, the other client's copy is left alone.
    #[test]
    fn a_copy_under_another_client_is_left_alone() {
        let (mut h, seen, _audit, _t) = link_window(|method, path| match (method, path) {
            // The document's own lookup would find the other client's copy; this client has none.
            ("GET", p) if p.contains("vfDocumentId") => {
                (200, portal_copy("OTHER-CLIENT", "1".repeat(64).as_str(), 1))
            }
            ("GET", p) if p.starts_with("/api/firm/v1/files?clientId=AB-1") => {
                (200, r#"{"files":[]}"#.into())
            }
            ("POST", p) if p.starts_with("/api/firm/v1/files") => {
                (201, r#"{"fileId":"f2"}"#.into())
            }
            ("POST", "/api/firm/v1/shares") => (
                201,
                r#"{"shareId":"s1","expiresAt":"2026-11-18T15:00:00.000Z"}"#.into(),
            ),
            _ => (200, r#"{"ok":true}"#.into()),
        });
        wait_for(&mut h, "Not on the portal yet");
        fill_and_send(&mut h);
        wait_for(&mut h, "Secure link sent");
        assert!(requests(&seen, "PUT /api/firm/v1/files/").is_empty());
        let add = requests(&seen, "POST /api/firm/v1/files");
        assert!(add[0].contains("clientId=AB-1"), "{add:?}");
        assert_eq!(link_files(&seen), serde_json::json!(["f2"]));
    }

    /// Code review: the copy changed between the window's look and Send (another PC shared an
    /// older version with the client's people): nothing is sent, the new state is shown.
    #[test]
    fn a_copy_changed_before_send_is_shown_first() {
        static LOOKS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let (mut h, seen, audit, _t) = link_window(|method, path| match (method, path) {
            ("GET", p) if p.starts_with("/api/firm/v1/files?clientId=AB-1") => {
                if LOOKS.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    (200, portal_copy("AB-1", DOC_SHA, 0))
                } else {
                    (200, portal_copy("AB-1", &"0".repeat(64), 2))
                }
            }
            _ => (200, r#"{"ok":true}"#.into()),
        });
        wait_for(&mut h, "On the portal as it is in the cabinet");
        fill_and_send(&mut h);
        wait_for(&mut h, "changed since this window looked");
        assert!(
            requests(&seen, "POST").is_empty(),
            "{:?}",
            seen.lock().unwrap()
        );
        assert!(requests(&seen, "PUT /api/firm/v1/files/").is_empty());
        assert!(
            h.query_by_label_contains("2 of the client's portal people see it")
                .is_some()
        );
        assert_eq!(audit.try_iter().count(), 0);
    }

    /// Code review: a failed first read can be tried again without reopening the window.
    #[test]
    fn a_failed_read_can_be_tried_again() {
        static TRIES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let (mut h, _seen, _audit, _t) = link_window(|method, path| match (method, path) {
            ("GET", p) if p.starts_with("/api/firm/v1/files?clientId=AB-1") => {
                if TRIES.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    (503, r#"{"error":"unavailable"}"#.into())
                } else {
                    (200, r#"{"files":[]}"#.into())
                }
            }
            _ => (200, r#"{"ok":true}"#.into()),
        });
        wait_for(&mut h, "Read the portal again");
        h.get_by_label("Read the portal again").click();
        wait_for(&mut h, "Not on the portal yet");
        assert!(h.state().err.is_none());
    }

    /// Already on the portal as it is in the cabinet: not sent again. A portal from before P68
    /// says to update it, and a copy that was already there is not called hidden.
    #[test]
    fn a_secure_link_to_a_document_already_there_and_an_old_portal() {
        let (mut h, seen, audit, _t) = link_window(|method, path| match (method, path) {
            ("GET", p) if p.starts_with("/api/firm/v1/files?clientId=AB-1") => {
                (200, portal_copy("AB-1", DOC_SHA, 1))
            }
            ("POST", "/api/firm/v1/shares") => (404, r#"{"error":"not_found"}"#.into()),
            _ => (200, r#"{"ok":true}"#.into()),
        });
        wait_for(&mut h, "On the portal as it is in the cabinet");
        fill_and_send(&mut h);
        wait_for(&mut h, "update Vibe Portal");
        assert!(!h.state().err.as_deref().unwrap().contains("shown to none"));
        assert!(requests(&seen, "POST /api/firm/v1/files").is_empty());
        assert_eq!(audit.try_iter().count(), 0, "no link, no row");
    }

    /// QA: a refusal before the link (the file removed meanwhile) is not taken for an old
    /// portal.
    #[test]
    fn a_refused_update_is_not_called_an_old_portal() {
        let e = vf_portal::api::PortalError::Http {
            status: 400,
            code: "view_only_needs_preview".into(),
        };
        assert!(link_error(&e, false, false).contains("allow download"));
        let e = vf_portal::api::PortalError::Http {
            status: 404,
            code: "not_found".into(),
        };
        assert!(link_error(&e, true, false).contains("shown to none"));
        // A failed link after the client's copy was updated says so.
        assert!(link_error(&e, false, true).contains("the client's people have the newer version"));
        assert!(!e.message().contains("update Vibe Portal"));
    }

    /// A portal from before the search says to update it.
    #[test]
    fn an_old_portal_says_to_update_it() {
        let e = vf_portal::api::PortalError::Http {
            status: 404,
            code: "not_found".into(),
        };
        assert!(people_error(&e).contains("update Vibe Portal"));
    }

    #[test]
    fn save_sends_only_the_ticks_changed_in_the_window() {
        // Spouse sees doc A, not doc B; only Mary's tick was changed.
        let a = file(&["spouse"]);
        let b = file(&[]);
        let chosen: BTreeMap<String, bool> = [("mary".to_string(), true)].into();
        assert_eq!(
            visibility_changes(&a, &chosen),
            [("mary".to_string(), true)].into()
        );
        assert_eq!(
            visibility_changes(&b, &chosen),
            [("mary".to_string(), true)].into()
        );
        // Spouse left alone is never sent (QA 2026-11-09: no silent unsharing).
        assert!(!visibility_changes(&a, &chosen).contains_key("spouse"));
        // Unticking someone hides only where they see it.
        let off: BTreeMap<String, bool> = [("spouse".to_string(), false)].into();
        assert_eq!(visibility_changes(&a, &off), off);
        assert!(visibility_changes(&b, &off).is_empty());
        // A document not on the portal: only people newly chosen.
        assert!(visibility_changes(&None, &off).is_empty());
    }
}
