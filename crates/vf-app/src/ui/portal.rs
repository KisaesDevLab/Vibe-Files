//! The firm's Vibe Portal in the app (PA-37): the Share window (Browse), the client's portal
//! users (client panel), Admin › Portal and Admin › Portal requests, and the timed fetch of
//! client uploads into the Portal inbox. Every call to the portal runs on a thread; the window
//! shows "Working…" meanwhile. Each change is recorded in the audit trail.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use egui::RichText;
use vf_core::audit::AuditEntry;
use vf_portal::api::{AccessRequest, PortalFile, Users};
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

// ======================================================================================
// State kept with Browse: settings, the timed fetch, the marks, the open windows.
// ======================================================================================

#[derive(Default)]
pub struct PortalState {
    settings: Option<PortalSettings>,
    read_at: Option<Instant>,
    fetch: Option<Receiver<core::FetchReport>>,
    fetched_at: Option<Instant>,
    /// Cabinet documents on the portal (Browse's marks), as of the last fetch.
    pub on_portal: HashSet<String>,
    pub share: Option<ShareWindow>,
    pub people: Option<PeopleWindow>,
    /// Admin › Portal's "Fetch now".
    pub fetch_now: bool,
    /// A line for Browse's notice bar.
    pub notice: Option<String>,
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

    /// The timed fetch of client uploads. Returns audit rows for what arrived.
    pub fn tick(&mut self, ctx: &egui::Context, root: &Path) -> Vec<AuditEntry> {
        let mut audit = Vec::new();
        if let Some(rx) = &self.fetch
            && let Ok(report) = rx.try_recv()
        {
            self.fetch = None;
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
            self.notice = match (n, report.problems.first()) {
                (0, None) => self.notice.take(),
                (_, Some(p)) => Some(format!("Portal: {p}")),
                (n, None) => Some(format!(
                    "Portal: {n} file{} received into {}.",
                    if n == 1 { "" } else { "s" },
                    core::INBOX
                )),
            };
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
            let minutes = s.fetch_minutes;
            self.fetch = Some(spawn(ctx, move || core::fetch(&root, &s, &now)));
            if minutes > 0 {
                ctx.request_repaint_after(Duration::from_secs(30));
            }
        } else if s.fetch_minutes > 0 {
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
    open: bool,
    email: String,
    phone: String,
    name: String,
    role: String,
}

impl InviteForm {
    /// Draws the form; `true` when "Send invitation" was pressed.
    fn show(&mut self, ui: &mut egui::Ui, busy: bool) -> bool {
        let mut send = false;
        egui::CollapsingHeader::new("Invite someone…")
            .open(self.open.then_some(true))
            .show(ui, |ui| {
                egui::Grid::new("portal-invite").num_columns(2).show(ui, |ui| {
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
                    "The invitation goes to the email when there is one, else by text. It works for 7 days.",
                ));
                let ok = !busy && !(self.email.trim().is_empty() && self.phone.trim().is_empty());
                if ui.add_enabled(ok, egui::Button::new("Send invitation")).clicked() {
                    send = true;
                }
            });
        self.open = false;
        send
    }

    fn clear(&mut self) {
        *self = Self::default();
    }
}

fn invite_audit(client_id: &str, f: &InviteForm) -> AuditEntry {
    AuditEntry::new(
        "portal_invite",
        None,
        serde_json::json!({
            "client_id": client_id,
            "via": if f.email.trim().is_empty() { "text" } else { "email" },
            "role": f.role.trim(),
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
    Done { audit: Vec<AuditEntry>, msg: String },
}

pub struct ShareWindow {
    client: ClientRef,
    docs: Vec<DocRef>,
    loaded: Option<Loaded>,
    /// Who should see the documents (identity → shown).
    want: BTreeMap<String, bool>,
    /// Send the cabinet's newer content for documents changed since they were shared.
    update: bool,
    invite: InviteForm,
    confirm_remove: bool,
    job: Option<Receiver<Result<Outcome, String>>>,
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
    ) -> Self {
        let job = Some(load_job(ctx, s.clone(), client.clone(), docs.clone()));
        Self {
            client,
            docs,
            loaded: None,
            want: BTreeMap::new(),
            update: true,
            invite: InviteForm::default(),
            confirm_remove: false,
            job,
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

    /// Who sees the documents now: shown to a person only if shown in every selected document.
    fn current(&self, l: &Loaded) -> BTreeMap<String, bool> {
        l.users
            .users
            .iter()
            .map(|u| {
                let all = !l.files.is_empty()
                    && l.files.iter().all(|f| {
                        f.as_ref().is_some_and(|f| {
                            f.users
                                .iter()
                                .any(|x| x.identity_id == u.identity_id && x.visible)
                        })
                    });
                (u.identity_id.clone(), all)
            })
            .collect()
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
        let before = self.current(l);
        let want = self.want.clone();
        let update = self.update;
        let client = self.client.clone();
        let work: Vec<(DocRef, Option<PortalFile>)> = self
            .docs
            .iter()
            .cloned()
            .zip(l.files.iter().cloned())
            .collect();
        let s = s.clone();
        self.msg = None;
        self.err = None;
        self.job = Some(spawn(ctx, move || {
            let p = core::client(&s);
            let mut audit = Vec::new();
            let mut sent = 0usize;
            for (d, file) in work {
                // Only the people whose choice changed (or everyone chosen, for a new file).
                let mut vis: BTreeMap<String, bool> = BTreeMap::new();
                for (id, &w) in &want {
                    let was = file
                        .as_ref()
                        .is_some_and(|f| f.users.iter().any(|x| &x.identity_id == id && x.visible));
                    if w != was && (w || before.get(id).copied().unwrap_or(false) || was) {
                        vis.insert(id.clone(), w);
                    }
                }
                let anyone = want.values().any(|&v| v);
                let (file_id, added, updated) = match &file {
                    None if !anyone => continue,
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
                if !vis.is_empty() {
                    p.set_visibility(&file_id, &vis)
                        .map_err(|e| format!("{}: {}", d.name, e.message()))?;
                }
                if added || updated || !vis.is_empty() {
                    let shown: Vec<&String> = vis.iter().filter(|v| *v.1).map(|v| v.0).collect();
                    let hidden: Vec<&String> = vis.iter().filter(|v| !*v.1).map(|v| v.0).collect();
                    audit.push(AuditEntry::new(
                        "portal_share",
                        Some(&d.id),
                        serde_json::json!({
                            "client_id": client.id,
                            "shown_to": shown,
                            "hidden_from": hidden,
                            "sent": added,
                            "updated": updated,
                        }),
                    ));
                }
            }
            let msg = if audit.is_empty() {
                "Nothing to change.".to_string()
            } else {
                format!(
                    "Saved on the portal{}.",
                    if sent > 0 {
                        format!(" ({sent} file{} sent)", if sent == 1 { "" } else { "s" })
                    } else {
                        String::new()
                    }
                )
            };
            Ok(Outcome::Done { audit, msg })
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
        self.job = Some(spawn(ctx, move || {
            let p = core::client(&s);
            let mut audit = Vec::new();
            for (doc, file) in work {
                p.remove(&file).map_err(|e| e.message())?;
                audit.push(AuditEntry::new(
                    "portal_remove",
                    Some(&doc),
                    serde_json::json!({ "client_id": client }),
                ));
            }
            Ok(Outcome::Done {
                audit,
                msg: "Removed from the portal (the cabinet keeps the files).".into(),
            })
        }));
    }

    fn send_invite(&mut self, ctx: &egui::Context, s: &PortalSettings) {
        let f = std::mem::take(&mut self.invite);
        let entry = invite_audit(&self.client.id, &f);
        let client = self.client.id.clone();
        let s = s.clone();
        self.job = Some(spawn(ctx, move || {
            core::client(&s)
                .invite(
                    &client,
                    Some(f.email.as_str()),
                    Some(f.phone.as_str()),
                    f.name.trim(),
                    f.role.trim(),
                )
                .map_err(|e| e.message())?;
            Ok(Outcome::Done {
                audit: vec![entry],
                msg: "Invitation sent. They appear here once they accept.".into(),
            })
        }));
    }

    /// Draws the window; audit rows to record when a change finished.
    pub fn show(&mut self, ctx: &egui::Context, env: &mut Env, s: &PortalSettings) {
        if let Some(rx) = &self.job
            && let Ok(r) = rx.try_recv()
        {
            self.job = None;
            match r {
                Ok(Outcome::Loaded(l)) => {
                    self.want = self.current(&l);
                    self.loaded = Some(l);
                }
                Ok(Outcome::Done { audit, msg }) => {
                    let unrecorded = record(env, "portal", audit).unwrap_or_default();
                    self.msg = Some(format!("{msg}{unrecorded}"));
                    self.reload(ctx, s);
                }
                Err(e) => self.err = Some(e),
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
                // The documents and their state on the portal.
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
                        let mut on = self.want.get(&u.identity_id).copied().unwrap_or(false);
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
                                self.want.insert(u.identity_id.clone(), on);
                            }
                            if !u.active {
                                ui.label(widgets::caption("turned off"));
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
                            RichText::new("Remove from the portal for everyone?")
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
                if self.invite.show(ui, busy) {
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

/// The users read again, audit rows to record, and a message.
type PeopleDone = (Option<Users>, Vec<AuditEntry>, String);

pub struct PeopleWindow {
    client: ClientRef,
    users: Option<Users>,
    invite: InviteForm,
    job: Option<Receiver<Result<PeopleDone, String>>>,
    msg: Option<String>,
    err: Option<String>,
    pub closed: bool,
}

impl PeopleWindow {
    pub fn new(ctx: &egui::Context, s: &PortalSettings, client: ClientRef) -> Self {
        let mut w = Self {
            client,
            users: None,
            invite: InviteForm::default(),
            job: None,
            msg: None,
            err: None,
            closed: false,
        };
        w.run(ctx, s, Vec::new(), String::new(), |_| Ok(()));
        w
    }

    /// Runs `act` on a thread, then reads the users again.
    fn run(
        &mut self,
        ctx: &egui::Context,
        s: &PortalSettings,
        audit: Vec<AuditEntry>,
        msg: String,
        act: impl FnOnce(&vf_portal::Portal) -> vf_portal::api::Result<()> + Send + 'static,
    ) {
        let client = self.client.clone();
        let s = s.clone();
        self.err = None;
        self.job = Some(spawn(ctx, move || {
            let p = core::client(&s);
            p.put_client(&client.id, &client.code, &client.name, None)
                .map_err(|e| e.message())?;
            act(&p).map_err(|e| e.message())?;
            let users = p.users(&client.id).map_err(|e| e.message())?;
            Ok((Some(users), audit, msg))
        }));
    }

    pub fn show(&mut self, ctx: &egui::Context, env: &mut Env, s: &PortalSettings) {
        if let Some(rx) = &self.job
            && let Ok(r) = rx.try_recv()
        {
            self.job = None;
            match r {
                Ok((users, audit, msg)) => {
                    let unrecorded = record(env, "portal", audit).unwrap_or_default();
                    if !msg.is_empty() || !unrecorded.is_empty() {
                        self.msg = Some(format!("{msg}{unrecorded}"));
                    }
                    self.users = users;
                }
                Err(e) => self.err = Some(e),
            }
        }
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
                                    .add_enabled(!busy, egui::Button::new(text).small())
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
                                    .add_enabled(!busy, egui::Button::new("Resend").small())
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
                                    .add_enabled(!busy, egui::Button::new("Withdraw").small())
                                    .clicked()
                                {
                                    withdraw = Some(i.invitation_id.clone());
                                }
                            });
                        }
                    }
                }
                ui.separator();
                if self.invite.show(ui, busy) {
                    let f = std::mem::take(&mut self.invite);
                    resend = Some((
                        Some(f.email.trim().to_string()).filter(|x| !x.is_empty()),
                        Some(f.phone.trim().to_string()).filter(|x| !x.is_empty()),
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
            self.run(ctx, s, vec![entry], String::new(), move |p| {
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
                vec![entry],
                "Invitation withdrawn.".into(),
                move |p| p.withdraw_invite(&c, &inv),
            );
        } else if let Some((email, phone, name, role)) = resend {
            let entry = invite_audit(
                &client,
                &InviteForm {
                    email: email.clone().unwrap_or_default(),
                    role: role.clone(),
                    ..Default::default()
                },
            );
            let c = client.clone();
            self.invite.clear();
            self.run(ctx, s, vec![entry], "Invitation sent.".into(), move |p| {
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

/// The requests read again, and audit rows to record.
type RequestsDone = (Option<Vec<AccessRequest>>, Vec<AuditEntry>);

#[derive(Default)]
pub struct AdminPortal {
    loaded: bool,
    url: String,
    key: String,
    minutes: String,
    status: Option<Receiver<Result<String, String>>>,
    requests: Option<Vec<AccessRequest>>,
    req_job: Option<Receiver<Result<RequestsDone, String>>>,
    /// Per request: the client chosen and the role typed.
    pick: BTreeMap<String, (Option<usize>, String)>,
    clients: Vec<ClientRef>,
    msg: Option<String>,
    err: Option<String>,
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
    }

    fn load(&mut self, root: &Path) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        let s = cabinet_id(root).and_then(|id| settings::load(root, &id));
        self.url = s.as_ref().map(|s| s.url.clone()).unwrap_or_default();
        self.key = s.as_ref().map(|s| s.key.clone()).unwrap_or_default();
        self.minutes = s
            .as_ref()
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
         are kept in the cabinet for every PC; they come from the portal's installer \
         (sudo vibe portal key create).",
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
                                    "Connected to {}'s portal (version {}).",
                                    st.firm_name, st.version
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
        if ui
            .add_enabled(!busy, egui::Button::new("Stop using the portal"))
            .on_hover_text("Every PC stops calling the portal. The portal keeps its files.")
            .clicked()
        {
            settings::clear(env.root);
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
            a.msg = Some(format!("The portal is no longer used.{unrecorded}"));
        }
        if busy {
            ui.spinner();
        }
    });
    if let Some(m) = &a.msg {
        ui.label(m.as_str());
    }
    widgets::message_line(ui, a.err.as_deref());
    ui.separator();
    ui.label(RichText::new("Received from clients lately").strong());
    let rows = core::recent_received(env.root, 30);
    if rows.is_empty() {
        ui.label(widgets::caption("Nothing yet."));
    }
    egui::ScrollArea::vertical()
        .max_height(280.0)
        .show(ui, |ui| {
            for r in rows {
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
    let Some(s) = cabinet_id(env.root).and_then(|id| settings::load(env.root, &id)) else {
        ui.heading("Portal requests");
        ui.label("Set up the portal in Admin › Portal first.");
        return;
    };
    if a.clients.is_empty() {
        a.clients = load_clients(env);
    }
    if let Some(rx) = &a.req_job
        && let Ok(r) = rx.try_recv()
    {
        a.req_job = None;
        match r {
            Ok((list, audit)) => {
                let unrecorded = record(env, "portal", audit).unwrap_or_default();
                if !unrecorded.is_empty() {
                    a.err = Some(unrecorded);
                }
                if let Some(l) = list {
                    a.requests = Some(l);
                }
            }
            Err(e) => a.err = Some(e),
        }
    }
    if a.requests.is_none() && a.req_job.is_none() {
        let s2 = s.clone();
        a.req_job = Some(spawn(ui.ctx(), move || {
            core::client(&s2)
                .access_requests()
                .map(|l| (Some(l), Vec::new()))
                .map_err(|e| e.message())
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
        a.requests = None;
    }
    let mut decision: Option<(String, Option<(String, String)>)> = None;
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
                                Some((r.id.clone(), Some((c.id.clone(), role.trim().to_string()))));
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
        let client = to
            .as_ref()
            .and_then(|(id, _)| a.clients.iter().find(|c| &c.id == id).cloned());
        a.req_job = Some(spawn(ui.ctx(), move || {
            let p = core::client(&s);
            let entry = match (&to, &client) {
                (Some((_, role)), Some(c)) => {
                    p.put_client(&c.id, &c.code, &c.name, None)
                        .map_err(|e| e.message())?;
                    p.approve(&req, &c.id, role).map_err(|e| e.message())?;
                    AuditEntry::new(
                        "portal_approve",
                        None,
                        serde_json::json!({ "request": req, "client_id": c.id, "role": role }),
                    )
                }
                _ => {
                    p.deny(&req).map_err(|e| e.message())?;
                    AuditEntry::new("portal_deny", None, serde_json::json!({ "request": req }))
                }
            };
            let list = p.access_requests().map_err(|e| e.message())?;
            Ok((Some(list), vec![entry]))
        }));
    }
    widgets::message_line(ui, a.err.as_deref());
}
