//! The firm's Vibe Portal in the app (PA-37): the Share window (Browse), the client's portal
//! users (client panel), Admin › Portal and Admin › Portal requests, and the timed fetch of
//! client uploads into the Portal inbox. Every call to the portal runs on a thread; the window
//! shows "Working…" meanwhile.
//!
//! Audit (QA 2026-11-09): each portal change is sent to [`PortalState`]'s channel by the thread
//! the moment the portal accepted it, and the app records what the channel holds every frame.
//! Closing a window, or an error part-way through a batch, never loses the record of what was
//! already done.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use egui::RichText;
use vf_core::audit::AuditEntry;
use vf_portal::api::{AccessRequest, AccessRequests, Check, Person, PortalFile, Users};
use vf_portal::record::{self, DateOrder, RecordColumns, Written};
use vf_portal::{PortalSettings, settings};

use crate::portal as core;
use crate::ui::browse::Env;
use crate::ui::widgets::UiExt as _;
use crate::ui::{theme, widgets};

/// Whether a document goes to B2 before it is shared through the portal (owner 2026-11-13,
/// Q-103: "everything should run through B2 first, unless B2 is not set up").
#[derive(Clone, Default)]
pub enum B2First {
    /// B2 sync is not set up for this cabinet on this PC: the share goes ahead without it.
    #[default]
    NotSetUp,
    /// Set up, but locked on this PC (the passphrase was not given): the share waits.
    Locked,
    /// The session to push with, the catalog and the cabinet.
    Ready {
        sync: Box<vf_sync::Sync>,
        catalog: std::path::PathBuf,
        root: std::path::PathBuf,
    },
}

impl B2First {
    /// Sends these documents (catalog ids) to B2 before they are shared. An error stops the
    /// share: nothing reaches the portal that is not in B2.
    fn push(&self, ids: &[String]) -> Result<(), String> {
        if ids.is_empty() {
            return Ok(());
        }
        match self {
            B2First::NotSetUp => Ok(()),
            B2First::Locked => Err(
                "Not shared: shared documents go to B2 first, and B2 sync is locked on this PC. Unlock it on the Sync tab, then share again.".into(),
            ),
            B2First::Ready {
                sync,
                catalog,
                root,
            } => crate::ui::sync::push_docs_now(catalog, root, sync, ids)
                .map_err(|e| format!("Not shared: the push to B2 did not finish ({e}).")),
        }
    }
}

/// Runs `f` on a thread; the window repaints when it is done.
pub(crate) fn spawn<T: Send + 'static>(
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
pub(crate) fn record(env: &mut Env, label: &str, entries: Vec<AuditEntry>) -> Option<String> {
    if entries.is_empty() {
        return None;
    }
    // (One try: the connection itself waits ten seconds for another writer, and this is the
    // window's thread.)
    let mut tx = vf_core::tx::Transaction::new(label).record_only();
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
    pub(crate) fn new(tx: Sender<AuditEntry>) -> Self {
        Self(tx)
    }

    pub(crate) fn send(&self, e: AuditEntry) {
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
    /// A client's to-do lists (PA-103).
    pub todo: Option<crate::ui::todo::TodoWindow>,
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
    /// The last word about uploads fetched and not filed (PA-43), said once until it changes.
    last_waiting: Option<String>,
    /// The catalog file and the device id B2 sync uses for it: where the lists of documents in
    /// B2 are read from (PA-80). Set by the app once a cabinet is open.
    pub lists_from: Option<(std::path::PathBuf, String)>,
    /// The lists being sent to the portal, when they were last sent, and the last problem said.
    lists: Option<Receiver<core::ListReport>>,
    lists_at: Option<Instant>,
    last_list_problem: Option<String>,
    audit_tx: Sender<AuditEntry>,
    audit_rx: Receiver<AuditEntry>,
    /// To-do lists read, to be copied into their client's folder (PA-103).
    todo_tx: Sender<crate::ui::todo::TodoCopy>,
    todo_rx: Receiver<crate::ui::todo::TodoCopy>,
    /// Access requests waiting on the portal, as of the last look (PA-106: said in the footer
    /// on every PC); `None` until read, or when the portal could not say.
    pub requests_waiting: Option<usize>,
    requests_job: Option<Receiver<Option<usize>>>,
    requests_at: Option<Instant>,
    /// A client's Balance owed window (Addendum §16).
    pub balance: Option<crate::ui::balance::BalanceWindow>,
    /// Every client's balances, as last read on this PC (every few minutes on every PC, for
    /// the client panel's line and the notice of one left unpaid).
    pub balances: Option<vf_portal::balance::Balances>,
    balances_job: Option<Receiver<Option<vf_portal::balance::Balances>>>,
    balances_at: Option<Instant>,
    /// Balances already seen expired on this PC: a newly expired one is said once.
    expired_seen: Option<HashSet<String>>,
    /// Balances read (the poll and the windows), to be copied into their client folders.
    balance_tx: Sender<Vec<vf_portal::balance::Balance>>,
    balance_rx: Receiver<Vec<vf_portal::balance::Balance>>,
}

/// Minutes between reads of the balances (every PC; Addendum §16).
pub const BALANCES_MINUTES: u64 = 5;

/// Minutes between looks at the access requests waiting (every PC; PA-106).
pub const REQUESTS_MINUTES: u64 = 5;

impl Default for PortalState {
    fn default() -> Self {
        let (audit_tx, audit_rx) = mpsc::channel();
        let (todo_tx, todo_rx) = mpsc::channel();
        let (balance_tx, balance_rx) = mpsc::channel();
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
            todo: None,
            fetch_now: false,
            last_fetch: None,
            refresh_marks: false,
            notice: None,
            last_problem: None,
            last_waiting: None,
            lists_from: None,
            lists: None,
            lists_at: None,
            last_list_problem: None,
            audit_tx,
            audit_rx,
            todo_tx,
            todo_rx,
            requests_waiting: None,
            requests_job: None,
            requests_at: None,
            balance: None,
            balances: None,
            balances_job: None,
            balances_at: None,
            expired_seen: None,
            balance_tx,
            balance_rx,
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
        self.lists_at = None;
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

    /// Where to-do windows send the lists they read, for the client folder's copy (PA-103).
    pub fn todo_out(&self) -> crate::ui::todo::CopyOut {
        crate::ui::todo::CopyOut(self.todo_tx.clone())
    }

    /// Where balance windows send what they read, for the client folders' copies (§16).
    pub fn balance_out(&self) -> crate::ui::balance::ReadOut {
        crate::ui::balance::ReadOut(self.balance_tx.clone())
    }

    /// The balances read since the last frame (the app copies them with audit rows).
    pub fn take_balance_reads(&self) -> Vec<Vec<vf_portal::balance::Balance>> {
        self.balance_rx.try_iter().collect()
    }

    /// Read the balances at the next frame (after a change, and the walkthrough).
    pub fn look_at_balances_now(&mut self) {
        self.balances_at = None;
    }

    /// The to-do copies waiting to be written (the app writes them with an audit row).
    pub fn take_todo_copies(&self) -> Vec<crate::ui::todo::TodoCopy> {
        self.todo_rx.try_iter().collect()
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
            if let Some(f) = &report.filing {
                for name in &f.reported {
                    audit.push(AuditEntry::new(
                        "portal_upload_filed",
                        None,
                        serde_json::json!({ "name": name }),
                    ));
                }
                // An older portal's "update it" is for Admin › Portal, not every fetch.
                if f.problems.iter().all(|p| p != vf_portal::api::NEEDS_P74) {
                    let note = core::waiting_note(f, chrono::Utc::now());
                    if note != self.last_waiting {
                        self.last_waiting.clone_from(&note);
                        if let Some(n) = note {
                            self.notice = Some(n);
                        }
                    }
                }
            }
            if n > 0 {
                self.notice = Some(format!(
                    "Portal: {n} file{} received into {}.",
                    if n == 1 { "" } else { "s" },
                    core::INBOX
                ));
            }
        }
        self.take_lists(&mut audit);
        let Some(s) = self.settings(root).cloned() else {
            return audit;
        };
        let every = Duration::from_secs(u64::from(s.fetch_minutes) * 60);
        // The timer runs on the server holding the sync lease (owner 2026-11-20, Q-133);
        // Fetch uploads now works anywhere.
        let mut due = s.fetch_minutes > 0 && self.fetched_at.is_none_or(|t| t.elapsed() >= every);
        if due && crate::lease::applies(root) && !crate::lease::sync_here(root) {
            // Another server's turn: looked at again at the next interval, not every frame.
            self.fetched_at = Some(Instant::now());
            due = false;
        }
        let due = std::mem::take(&mut self.fetch_now) || due;
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
        if s.fetch_minutes > 0 || s.b2_list {
            ctx.request_repaint_after(Duration::from_secs(30));
        }
        self.tick_lists(ctx, root, &s);
        self.tick_requests(ctx, &s);
        self.tick_balances(ctx, &s);
        audit
    }

    /// Reads every client's balances every few minutes, on every PC (Addendum §16): the
    /// client panel's line follows them, what was read is copied into the client folders, and
    /// a balance newly left unpaid past its date is said once on this PC.
    fn tick_balances(&mut self, ctx: &egui::Context, s: &PortalSettings) {
        if !s.balances {
            self.balances = None;
            self.balance = None;
            return;
        }
        if let Some(rx) = &self.balances_job
            && let Ok(read) = rx.try_recv()
        {
            self.balances_job = None;
            if let Some(read) = read {
                let expired: Vec<&vf_portal::balance::Balance> = read
                    .balances
                    .iter()
                    .filter(|b| b.state == vf_portal::balance::EXPIRED)
                    .collect();
                match &mut self.expired_seen {
                    // The first read on this PC: what had expired before is not news.
                    None => {
                        self.expired_seen = Some(expired.iter().map(|b| b.id.clone()).collect());
                    }
                    Some(seen) => {
                        let new: Vec<&&vf_portal::balance::Balance> = expired
                            .iter()
                            .filter(|b| seen.insert(b.id.clone()))
                            .collect();
                        if let Some(b) = new.first() {
                            let more = if new.len() > 1 {
                                format!(" (and {} more)", new.len() - 1)
                            } else {
                                String::new()
                            };
                            self.notice = Some(format!(
                                "Portal: a balance of {} left the portal unpaid{more}. See Admin › Portal › Balances.",
                                vf_portal::balance::money(b.amount_cents),
                            ));
                        }
                    }
                }
                let _ = self.balance_tx.send(read.balances.clone());
                self.balances = Some(read);
            }
        }
        let every = Duration::from_secs(BALANCES_MINUTES * 60);
        if self.balances_job.is_none() && self.balances_at.is_none_or(|t| t.elapsed() >= every) {
            self.balances_at = Some(Instant::now());
            let s2 = s.clone();
            self.balances_job = Some(spawn(ctx, move || core::client(&s2).balances().ok()));
        }
    }

    /// Looks at the access requests waiting every few minutes, on every PC (PA-106): the
    /// footer says how many.
    fn tick_requests(&mut self, ctx: &egui::Context, s: &PortalSettings) {
        if let Some(rx) = &self.requests_job
            && let Ok(n) = rx.try_recv()
        {
            self.requests_job = None;
            self.requests_waiting = n;
        }
        let every = Duration::from_secs(REQUESTS_MINUTES * 60);
        if self.requests_job.is_none() && self.requests_at.is_none_or(|t| t.elapsed() >= every) {
            self.requests_at = Some(Instant::now());
            let s2 = s.clone();
            self.requests_job = Some(spawn(ctx, move || {
                core::client(&s2)
                    .access_requests()
                    .ok()
                    .map(|l| l.requests.len())
            }));
        }
        ctx.request_repaint_after(Duration::from_secs(60));
    }

    /// Look at the access requests at the next frame (the walkthrough, not the 5 minutes).
    #[cfg(test)]
    pub fn look_at_requests_now(&mut self) {
        self.requests_at = None;
    }

    /// How many requests are waiting, as Admin › Portal requests just read them.
    pub fn requests_read(&mut self, n: usize) {
        self.requests_waiting = Some(n);
        self.requests_at = Some(Instant::now());
    }

    /// What a send of the lists of documents in B2 did, when one has ended (PA-80): the audit
    /// row, and a word in Browse when something went wrong. Taken whether or not the portal is
    /// still set up: a send may end after Stop using the portal.
    fn take_lists(&mut self, audit: &mut Vec<AuditEntry>) {
        let Some(rx) = &self.lists else { return };
        let Ok(rep) = rx.try_recv() else { return };
        self.lists = None;
        if rep.sent + rep.cleared > 0 {
            audit.push(AuditEntry::new(
                "portal_b2_list",
                None,
                serde_json::json!({ "clients_sent": rep.sent, "clients_cleared": rep.cleared }),
            ));
        }
        match list_problem(&rep) {
            // Said once, until it changes or a send works again.
            Some(p) if self.last_list_problem.as_deref() != Some(p.as_str()) => {
                self.last_list_problem = Some(p.clone());
                self.notice = Some(format!("Portal: {p}"));
            }
            Some(_) => {}
            None => self.last_list_problem = None,
        }
    }

    /// The lists of documents in B2, sent to the portal every 10 minutes when the firm ticked
    /// it in Admin › Portal (PA-80): only the clients whose list changed go.
    fn tick_lists(&mut self, ctx: &egui::Context, root: &Path, s: &PortalSettings) {
        if !s.b2_list {
            if self.lists.is_none() {
                self.last_list_problem = None;
            }
            return;
        }
        let every = Duration::from_secs(core::LIST_MINUTES * 60);
        if self.lists.is_some() || self.lists_at.is_some_and(|t| t.elapsed() < every) {
            return;
        }
        let Some((catalog, device)) = self.lists_from.clone() else {
            return;
        };
        self.lists_at = Some(Instant::now());
        let root = root.to_path_buf();
        self.lists = Some(spawn(ctx, move || {
            core::send_lists(&root, &catalog, &device)
        }));
    }
}

/// "Acme LLC" or "3 clients".
fn who(names: &[String]) -> String {
    match names {
        [one] => one.clone(),
        many => format!("{} clients", many.len()),
    }
}

/// What went wrong with a send of the lists of documents in B2, in words for Browse's notice:
/// everything that applies, each with what to do about it.
fn list_problem(r: &core::ListReport) -> Option<String> {
    if r.old_portal {
        return Some(
            "the portal cannot show the documents in B2 yet. Update the portal (on the appliance: \
             sudo vibe upgrade portal)."
                .into(),
        );
    }
    let mut parts: Vec<String> = Vec::new();
    if let Some(e) = &r.unremoved {
        parts.push(format!(
            "the setting was unticked while lists of documents in B2 were being sent, and they \
             could not be removed from the portal ({e}). In Admin › Portal tick Show the \
             documents in B2 on the portal's staff page, save, then untick it and save."
        ));
    }
    if let Some(p) = &r.problem {
        parts.push(format!(
            "the lists of documents in B2 were not all sent: {}{}",
            p.trim_end_matches('.'),
            "."
        ));
    }
    let own = |names: &[String]| {
        if names.len() == 1 {
            "its list was"
        } else {
            "their lists were"
        }
    };
    let has = |names: &[String]| if names.len() == 1 { "has" } else { "have" };
    if !r.too_many.is_empty() {
        parts.push(format!(
            "{} {} more than {} documents in B2, the most the portal lists for one client; {} \
             not sent, and the portal shows the last list sent, if any.",
            who(&r.too_many),
            has(&r.too_many),
            r.max_docs,
            own(&r.too_many)
        ));
    }
    if !r.too_long.is_empty() {
        parts.push(format!(
            "{} {} a document in B2 whose folder and name together are longer than {} \
             characters; {} not sent until that document is renamed or moved.",
            who(&r.too_long),
            has(&r.too_long),
            r.max_path.unwrap_or(0),
            own(&r.too_long)
        ));
    }
    if !r.refused.is_empty() {
        parts.push(format!(
            "the portal did not take the list of documents in B2 of {}{}",
            who(&r.refused),
            if r.sent > 0 {
                "; the other clients' lists were sent."
            } else {
                "."
            }
        ));
    }
    if parts.is_empty() {
        return None;
    }
    // The notice starts "Portal: ": the first part follows it in small letters, the rest are
    // sentences of their own.
    let mut out = parts.remove(0);
    for p in parts {
        let mut c = p.chars();
        if let Some(f) = c.next() {
            out.push(' ');
            out.extend(f.to_uppercase());
            out.push_str(c.as_str());
        }
    }
    Some(out)
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
    fn show(
        &mut self,
        ui: &mut egui::Ui,
        busy: bool,
        open: bool,
        button: &str,
        note: Option<&str>,
    ) -> bool {
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
            match note {
                Some(n) => {
                    ui.label(RichText::new(n).color(theme::health_ok()));
                }
                None => {
                    ui.label(widgets::caption(
                        "The invitation goes to the email when there is one, else by text. It works for 7 days. A mobile number given with an email is used only once the person confirms it.",
                    ));
                }
            }
            let ok = !busy && !(self.email.trim().is_empty() && self.phone.trim().is_empty());
            if ui
                .add_enabled(ok, egui::Button::new(button))
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

/// The contact typed in an invitation form, as the portal keeps it (the email when it is one,
/// else the mobile number), for finding someone already on the portal.
fn typed_contact(f: &InviteForm) -> Option<String> {
    vf_fs::contacts::normal_email(&f.email).or_else(|| {
        if f.email.trim().is_empty() {
            vf_fs::contacts::normal_phone(&f.phone)
        } else {
            None
        }
    })
}

/// Who an invitation is for: the name, else the email, else the mobile number.
fn invitee(i: &vf_portal::api::Invitation) -> String {
    if !i.name.trim().is_empty() {
        i.name.trim().to_string()
    } else {
        i.email.clone().or(i.phone.clone()).unwrap_or_default()
    }
}

pub(crate) fn invite_audit(client_id: &str, by_email: bool, role: &str) -> AuditEntry {
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
    /// The firm's name on the portal, for What they receive.
    firm: String,
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

/// What Save sends for one document about invitations not accepted yet: those whose tick was
/// changed in this window and differ from what the file is held for now.
pub fn invite_changes(
    file: &Option<PortalFile>,
    chosen: &BTreeMap<String, bool>,
) -> BTreeMap<String, bool> {
    let held = |id: &str| {
        file.as_ref()
            .is_some_and(|f| f.invites.iter().any(|i| i == id))
    };
    chosen
        .iter()
        .filter(|(id, w)| held(id) != **w)
        .map(|(id, w)| (id.clone(), *w))
        .collect()
}

/// Sends the documents to the portal as needed and sets who sees them: people on the portal
/// (`chosen`) and invitations not accepted yet (`chosen_inv`, shown when they accept). Returns
/// how many documents changed and how many files were sent.
#[allow(clippy::too_many_arguments)]
fn share_docs(
    p: &vf_portal::api::Portal,
    b2: &B2First,
    client: &ClientRef,
    work: Vec<(DocRef, Option<PortalFile>)>,
    chosen: &BTreeMap<String, bool>,
    chosen_inv: &BTreeMap<String, bool>,
    update: bool,
    audit: &AuditOut,
) -> Result<(usize, usize), String> {
    // To B2 first (Q-103): what is on the portal already, and what someone is newly chosen
    // to see.
    let to_b2: Vec<String> = work
        .iter()
        .filter(|(_, file)| {
            file.is_some()
                || visibility_changes(file, chosen).values().any(|&v| v)
                || invite_changes(file, chosen_inv).values().any(|&v| v)
        })
        .map(|(d, _)| d.id.clone())
        .collect();
    b2.push(&to_b2)?;
    let mut changed = 0usize;
    let mut sent = 0usize;
    for (d, file) in work {
        let vis = visibility_changes(&file, chosen);
        let inv = invite_changes(&file, chosen_inv);
        let wants_it = vis.values().any(|&v| v) || inv.values().any(|&v| v);
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
        if !vis.is_empty() || !inv.is_empty() {
            p.set_visibility_with(&file_id, &vis, &inv)
                .map_err(|e| format!("{}: {}", d.name, e.message()))?;
            let on = |m: &BTreeMap<String, bool>, v: bool| -> Vec<String> {
                m.iter()
                    .filter(|x| *x.1 == v)
                    .map(|x| x.0.clone())
                    .collect()
            };
            let mut detail = serde_json::json!({
                "client_id": client.id,
                "shown_to": on(&vis, true),
                "hidden_from": on(&vis, false),
            });
            if !inv.is_empty() {
                detail["held_for_invitations"] = serde_json::json!(on(&inv, true));
                detail["taken_from_invitations"] = serde_json::json!(on(&inv, false));
            }
            audit.send(AuditEntry::new("portal_share", Some(&d.id), detail));
        }
        if added || updated || !vis.is_empty() || !inv.is_empty() {
            changed += 1;
        }
    }
    Ok((changed, sent))
}

/// " (N files sent)" for the end of a message, or nothing.
fn files_sent(sent: usize) -> String {
    match sent {
        0 => String::new(),
        1 => " (1 file sent)".into(),
        n => format!(" ({n} files sent)"),
    }
}

pub struct ShareWindow {
    /// Whether the documents go to B2 before they are shared (Q-103).
    b2: B2First,
    client: ClientRef,
    docs: Vec<DocRef>,
    loaded: Option<Loaded>,
    /// The ticks changed in this window (identity → shown).
    chosen: BTreeMap<String, bool>,
    /// The same for invitations not accepted yet (invitation id → held for them).
    chosen_inv: BTreeMap<String, bool>,
    /// Invitations to send again on Save (owner 2026-11-24: resend and share in one step).
    resend: std::collections::BTreeSet<String>,
    /// Send the cabinet's newer content for documents changed since they were shared.
    update: bool,
    invite: InviteForm,
    /// The contact typed in Invite someone…, and the portal login it belongs to, if any
    /// (owner 2026-11-24: someone already on the portal is added at once, P113).
    known: Option<(String, Option<Person>)>,
    looking: Option<Receiver<(String, Option<Person>)>>,
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
        let firm = p.status().map(|st| st.firm_name).unwrap_or_default();
        Ok(Outcome::Loaded(Loaded { users, files, firm }))
    })
}

impl ShareWindow {
    pub fn new(
        ctx: &egui::Context,
        s: &PortalSettings,
        client: ClientRef,
        docs: Vec<DocRef>,
        audit: AuditOut,
        b2: B2First,
    ) -> Self {
        let job = Some(load_job(ctx, s.clone(), client.clone(), docs.clone()));
        Self {
            b2,
            client,
            docs,
            loaded: None,
            chosen: BTreeMap::new(),
            chosen_inv: BTreeMap::new(),
            resend: Default::default(),
            update: true,
            invite: InviteForm::default(),
            known: None,
            looking: None,
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
        let chosen_inv = self.chosen_inv.clone();
        let update = self.update;
        let client = self.client.clone();
        let work: Vec<(DocRef, Option<PortalFile>)> = self
            .docs
            .iter()
            .cloned()
            .zip(l.files.iter().cloned())
            .collect();
        // Sent again after the files are held for them: the portal moves what was held for the
        // old invitation to the new one (P98).
        let resend: Vec<vf_portal::api::Invitation> = l
            .users
            .invitations
            .iter()
            .filter(|i| self.resend.contains(&i.invitation_id))
            .cloned()
            .collect();
        let s = s.clone();
        let audit = self.audit.clone();
        let b2 = self.b2.clone();
        self.msg = None;
        self.err = None;
        self.job = Some(spawn(ctx, move || {
            let p = core::client(&s);
            let (changed, sent) =
                share_docs(&p, &b2, &client, work, &chosen, &chosen_inv, update, &audit)?;
            let mut again = Vec::new();
            for i in &resend {
                p.invite(
                    &client.id,
                    i.email.as_deref(),
                    i.phone.as_deref(),
                    &i.name,
                    &i.role_label,
                )
                .map_err(|e| format!("Resending to {}: {}", invitee(i), e.message()))?;
                let mut entry = invite_audit(&client.id, i.email.is_some(), &i.role_label);
                entry.detail["resent"] = serde_json::json!(true);
                audit.send(entry);
                again.push(invitee(i));
            }
            let mut msg = if changed == 0 && again.is_empty() {
                "Nothing to change.".to_string()
            } else if changed == 0 {
                String::new()
            } else {
                format!("Saved on the portal{}.", files_sent(sent))
            };
            if !again.is_empty() {
                if !msg.is_empty() {
                    msg.push(' ');
                }
                msg.push_str(&format!(
                    "Invitation sent again to {}; what was chosen for them is theirs when they accept.",
                    again.join(", ")
                ));
            }
            Ok(Outcome::Done(msg))
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

    /// The portal login the contact typed in Invite someone… belongs to, as last looked up.
    fn known_login(&self) -> Option<&Person> {
        let key = typed_contact(&self.invite)?;
        match &self.known {
            Some((k, p)) if *k == key => p.as_ref(),
            _ => None,
        }
    }

    /// Looks up the contact typed (once per contact, on a thread) among the portal's people.
    fn look_up_typed(&mut self, ctx: &egui::Context, s: &PortalSettings) {
        if let Some(rx) = &self.looking
            && let Ok(found) = rx.try_recv()
        {
            self.looking = None;
            self.known = Some(found);
        }
        let Some(key) = typed_contact(&self.invite) else {
            return;
        };
        if self.looking.is_some() || self.known.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        let s = s.clone();
        self.looking = Some(spawn(ctx, move || {
            let found = core::client(&s).people(&key).ok().and_then(|people| {
                people.into_iter().find(|p| {
                    p.email.as_deref() == Some(key.as_str())
                        || p.phone.as_deref() == Some(key.as_str())
                })
            });
            (key, found)
        }));
    }

    /// Invites someone and, in the same step, holds these documents for them: they see them
    /// when they accept (owner 2026-11-24). Someone who already has a portal login is added
    /// to the client at once instead, and the documents are shown to them (P113).
    fn send_invite(&mut self, ctx: &egui::Context, s: &PortalSettings) {
        let Some(l) = &self.loaded else { return };
        let login = self
            .known_login()
            .filter(|p| !p.sees(&self.client.id))
            .cloned();
        let f = std::mem::take(&mut self.invite);
        let client = self.client.clone();
        let work: Vec<(DocRef, Option<PortalFile>)> = self
            .docs
            .iter()
            .cloned()
            .zip(l.files.iter().cloned())
            .collect();
        let update = self.update;
        let s = s.clone();
        let audit = self.audit.clone();
        let b2 = self.b2.clone();
        self.msg = None;
        self.err = None;
        self.job = Some(spawn(ctx, move || {
            let p = core::client(&s);
            if let Some(person) = login {
                p.put_client(&client.id, &client.code, &client.name, None)
                    .map_err(|e| e.message())?;
                p.add_person(&client.id, &person.identity_id, f.role.trim())
                    .map_err(|e| {
                        if e.status() == Some(404) && e.code() == Some("not_found") {
                            "This portal cannot add someone at once yet: update it to 0.5.2, or send an invitation.".to_string()
                        } else {
                            e.message()
                        }
                    })?;
                audit.send(AuditEntry::new(
                    "portal_user_added",
                    None,
                    serde_json::json!({
                        "client_id": client.id,
                        "identity_id": person.identity_id,
                        "role": f.role.trim(),
                    }),
                ));
                let shown = BTreeMap::from([(person.identity_id.clone(), true)]);
                let n = work.len();
                let (_, sent) = share_docs(
                    &p,
                    &b2,
                    &client,
                    work,
                    &shown,
                    &BTreeMap::new(),
                    update,
                    &audit,
                )
                .map_err(|e| format!("{} was added, but sharing failed: {e}", person.label()))?;
                return Ok(Outcome::Done(format!(
                    "{} was added to this client{} and sees {} now; the portal told them.",
                    person.label(),
                    files_sent(sent),
                    if n == 1 {
                        "this document"
                    } else {
                        "these documents"
                    }
                )));
            }
            let id = p
                .invite(
                    &client.id,
                    f.email().as_deref(),
                    f.phone().as_deref(),
                    f.name.trim(),
                    f.role.trim(),
                )
                .map_err(|e| e.message())?;
            audit.send(invite_audit(&client.id, f.email().is_some(), f.role.trim()));
            let held = BTreeMap::from([(id, true)]);
            let n = work.len();
            let (_, sent) = share_docs(
                &p,
                &b2,
                &client,
                work,
                &BTreeMap::new(),
                &held,
                update,
                &audit,
            )
            .map_err(|e| format!("The invitation was sent, but sharing failed: {e}"))?;
            Ok(Outcome::Done(format!(
                "Invitation sent{}. {} theirs when they accept.",
                files_sent(sent),
                if n == 1 {
                    "This document is"
                } else {
                    "These documents are"
                }
            )))
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
                    self.chosen_inv.clear();
                    self.resend.clear();
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
        self.look_up_typed(ctx, s);
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
                if !l.users.invitations.is_empty() {
                    ui.add_space(4.0);
                    ui.label(RichText::new("Invited, not accepted yet").strong());
                }
                // Owner 2026-11-24: share with them and resend the invitation in one Save.
                for inv in &l.users.invitations {
                    let id = &inv.invitation_id;
                    let held = l
                        .files
                        .iter()
                        .filter(|f| f.as_ref().is_some_and(|f| f.invites.contains(id)))
                        .count();
                    let mut on = self
                        .chosen_inv
                        .get(id)
                        .copied()
                        .unwrap_or(held == l.files.len());
                    let expired = inv.status == "expired";
                    let contact = inv.email.clone().or(inv.phone.clone()).unwrap_or_default();
                    let who = if inv.name.trim().is_empty() || inv.name.trim() == contact {
                        contact
                    } else {
                        format!("{} ({contact})", inv.name.trim())
                    };
                    ui.horizontal_wrapped(|ui| {
                        let r = ui.add_enabled(!busy, egui::Checkbox::new(&mut on, who));
                        if r.changed() {
                            self.chosen_inv.insert(id.clone(), on);
                            // An expired invitation can't be accepted: sharing with them sends
                            // it again.
                            if expired && on {
                                self.resend.insert(id.clone());
                            }
                        }
                        ui.label(widgets::caption(if expired {
                            "invitation expired"
                        } else if held > 0 && held < l.files.len() && !self.chosen_inv.contains_key(id) {
                            "gets some of these when they accept"
                        } else if on {
                            "gets these when they accept"
                        } else {
                            "invited"
                        }));
                        let mut again = self.resend.contains(id);
                        if ui
                            .add_enabled(!busy, egui::Checkbox::new(&mut again, "Resend invitation"))
                            .on_hover_text("Sends a new link (the old one stops working); what is chosen for them goes with it")
                            .changed()
                        {
                            if again {
                                self.resend.insert(id.clone());
                            } else {
                                self.resend.remove(id);
                            }
                        }
                    });
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
                // Someone who already has a portal login is shown, and added at once.
                let here = self.client.id.clone();
                let login = self.known_login();
                let (button, note) = match login {
                    Some(p) if p.sees(&here) => (
                        "Send invitation and share".to_string(),
                        Some(format!(
                            "{} already sees this client: tick them above and press Save.",
                            p.label()
                        )),
                    ),
                    Some(p) => {
                        let others: Vec<&str> =
                            p.clients.iter().map(|c| c.name.as_str()).collect();
                        let sees = if others.is_empty() {
                            String::new()
                        } else {
                            format!(" (sees: {})", others.join(", "))
                        };
                        (
                            "Add to this client and share".to_string(),
                            Some(format!(
                                "{} already has a portal login{sees}: they are added at once, with no invitation to accept, and told by the portal.",
                                p.label()
                            )),
                        )
                    }
                    None => ("Send invitation and share".to_string(), None),
                };
                let seen_here = login.is_some_and(|p| p.sees(&here));
                // What they receive (owner 2026-11-24: read only).
                if typed_contact(&self.invite).is_some() && !seen_here {
                    let firm = if l.firm.trim().is_empty() {
                        "Your accounting firm".to_string()
                    } else {
                        l.firm.clone()
                    };
                    let url = s.url.trim_end_matches('/');
                    let (subject, body) = match login {
                        Some(_) => vf_portal::api::added_words(&firm, &self.client.name, url),
                        None => vf_portal::api::invitation_words(
                            &firm,
                            &self.client.name,
                            &format!("{url}/invite#t=…"),
                        ),
                    };
                    egui::CollapsingHeader::new("What they receive")
                        .id_salt("share-what-they-receive")
                        .show(ui, |ui| {
                            if login.is_some() {
                                ui.label(widgets::caption(
                                    "By their own choice of email or text:",
                                ));
                                ui.label(RichText::new(&subject).strong());
                                ui.label(body.as_str());
                            } else if self.invite.email().is_some() {
                                ui.label(RichText::new(format!("Email · {subject}")).strong());
                                ui.label(body.as_str());
                            } else {
                                ui.label(RichText::new("Text message").strong());
                                ui.label(vf_portal::api::text_message(&body));
                            }
                        });
                }
                if self.invite.show(ui, busy || seen_here, false, &button, note.as_deref()) {
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
                                    egui::TextEdit::singleline(&mut self.find).margin(crate::ui::widgets::SEARCH_MARGIN)
                                        .hint_text("name, email or mobile")
                                        .desired_width(220.0),
                                )
                                .labelled_by(l.id);
                            crate::ui::widgets::clear_x(ui, &r, &mut self.find);
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
                if self.invite.show(ui, busy, open_invite, "Send invitation", None) {
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
type RequestsDone = (Option<AccessRequests>, Result<String, String>);

/// How staff checked who is asking, when the request does not match the client record (Vibe
/// Portal Addendum 01, layer B): the portal's word, what Approve shows, and what the note is
/// (`None`: no note asked). A text or email to the contact on the request is not a choice:
/// that contact is what is being checked.
const CHECKS: [(&str, &str, Option<&str>); 3] = [
    (
        "callback",
        "Called the number in our records",
        Some("Who you spoke to"),
    ),
    ("in_person", "Met in person", None),
    (
        "other",
        "Known to me, other",
        Some("How you know this person"),
    ),
];

/// The longest note the portal takes with a check, counted as the portal counts it (UTF-16
/// units: an emoji is two).
const NOTE_MAX: usize = 300;

/// Cuts `s` to at most `max` UTF-16 units, at a whole character.
fn limit_utf16(s: &mut String, max: usize) {
    let mut units = 0;
    for (at, c) in s.char_indices() {
        units += c.len_utf16();
        if units > max {
            s.truncate(at);
            return;
        }
    }
}

/// The portal's flags on an ID, in words (as its staff page and approval PDF say them).
fn flag_words(flag: &str) -> String {
    match flag {
        "expired" => "The ID has expired".into(),
        "name_differs" => "The name on the ID differs from the name on the request".into(),
        "unreadable" => "The barcode on the back could not be read".into(),
        "no_barcode" => "The person said the ID has no barcode".into(),
        other => other.replace('_', " "),
    }
}

#[derive(Default)]
pub struct AdminPortal {
    loaded: bool,
    url: String,
    key: String,
    minutes: String,
    status: Option<Receiver<Result<String, String>>>,
    requests: Option<Vec<AccessRequest>>,
    /// The portal's "Check before approving" is on (as of the last read of the requests).
    check_required: bool,
    req_job: Option<Receiver<RequestsDone>>,
    /// The client-list columns chosen for the client's email, mobile and date of birth
    /// (Admin › Portal), as typed until Save.
    columns: RecordColumns,
    /// The client list's extra columns (token names), and each client's row by its ID.
    extras: Vec<String>,
    rows: record::Rows,
    /// The client list was read for `extras` and `rows`.
    record_read: bool,
    /// Why the client list could not be read (no request can then be compared with it).
    record_err: Option<String>,
    /// The client list being read, on its thread.
    record_job: Option<crate::ui::job::Job<Result<RecordRead, String>>>,
    /// How the date-of-birth column writes its dates, worked out once for the column and the
    /// firm's choice it was worked out for (the whole list is looked at).
    written: Option<(Option<String>, DateOrder, Written)>,
    /// The saved settings the form was filled from, or last saved here: Save writes only when
    /// the cabinet still has these (another PC's change is not written over; review 2026-11-15).
    shown: Option<PortalSettings>,
    /// The request whose "How did you check?" is open, the choice made and the note typed.
    asking: Option<String>,
    how: (Option<usize>, String),
    /// Per request: the client chosen and the role typed.
    pick: BTreeMap<String, (Option<usize>, String)>,
    /// Per request: the clients its contact or name leads to, and what is ticked (PA-106).
    suggest: BTreeMap<String, Suggest>,
    /// The approval the open "How did you check?" is for.
    ask_targets: Option<Approval>,
    /// Every client's Portal contacts and names, read with the client list (PA-106).
    dir: Vec<vf_fs::contacts::ClientContacts>,
    /// The catalog file, for the documents shared on Approve (set by the app).
    pub catalog: Option<std::path::PathBuf>,
    /// How many requests the page last read, for the footer (taken by the app).
    pub waiting: Option<usize>,
    /// The portal's version and features, looked at once per visit of Admin › Portal (owner
    /// 2026-11-24: an older portal is said there, once).
    version_job: Option<Receiver<Option<vf_portal::api::Status>>>,
    version_seen: Option<Option<vf_portal::api::Status>>,
    /// Whether shared documents go to B2 first (set by the app).
    pub b2: B2First,
    /// What Approve offers to share, as chosen until Save; and the doc types to choose from.
    approve_share: settings::ApproveShare,
    doc_types: Vec<(String, String)>,
    clients: Vec<ClientRef>,
    /// The portal settings and the received list, read at most every 10 s (the cabinet is
    /// often on a share; QA 2026-11-09).
    cached_settings: Option<PortalSettings>,
    received: Vec<vf_portal::inbox::Received>,
    read_at: Option<Instant>,
    confirm_stop: bool,
    /// "Show the documents in B2 on the portal's staff page", as ticked until Save (PA-80).
    b2_list: bool,
    /// "To-do lists", as ticked until Save (PA-103).
    todo_lists: bool,
    /// "Balance owed", as ticked until Save (Addendum §16).
    balances: bool,
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
    /// Uploads fetched and not filed (PA-43), as of the last check.
    filing: Option<core::FiledReport>,
    filing_job: Option<Receiver<FilingDone>>,
    filing_at: Option<Instant>,
    /// The upload whose Dismiss is being confirmed.
    confirm_dismiss: Option<String>,
}

/// A check of what was filed, after Fetch again or Dismiss when one was asked for.
struct FilingDone {
    report: core::FiledReport,
    /// What the action did (`Err` when it was refused); empty for a plain check.
    said: Result<String, String>,
    audit: Option<AuditEntry>,
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

    /// Shows `report` as the last check of what was filed, without asking the portal.
    #[cfg(test)]
    pub fn show_filing(&mut self, report: core::FiledReport) {
        self.filing = Some(report);
        self.filing_at = Some(Instant::now());
    }

    /// Reads the requests (and the client list) again when the page is next shown.
    pub fn refresh_requests(&mut self) {
        if self.req_job.is_none() {
            self.requests = None;
        }
        self.clients.clear();
        self.record_read = false;
        // The clients are read again: what was picked by its place in the list, and a question
        // open for it, no longer stand.
        self.pick.clear();
        self.asking = None;
        self.how = (None, String::new());
        self.err = None;
        self.read_at = None;
    }

    /// Admin › Portal was entered: the portal's version is looked at again.
    pub fn look_at_version_again(&mut self) {
        if self.version_job.is_none() {
            self.version_seen = None;
        }
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
        self.shown = self.cached_settings.clone();
        let s = self.cached_settings.as_ref();
        self.url = s.map(|s| s.url.clone()).unwrap_or_default();
        self.key = s.map(|s| s.key.clone()).unwrap_or_default();
        self.minutes = s
            .map_or(settings::DEFAULT_FETCH_MINUTES, |s| s.fetch_minutes)
            .to_string();
        self.columns = s.map(|s| s.record.clone()).unwrap_or_default();
        self.b2_list = s.is_some_and(|s| s.b2_list);
        self.todo_lists = s.is_some_and(|s| s.todo_lists);
        self.balances = s.is_some_and(|s| s.balances);
        self.approve_share = s.map(|s| s.approve_share.clone()).unwrap_or_default();
        self.adopt_old_columns(root);
    }

    /// Owner 2026-11-24 (PA-106): requests are checked against the Portal contacts. Settings
    /// that still name client-list columns for the email or mobile have them moved over once:
    /// each client's value into its contacts, and the columns into Portal contacts; then the
    /// settings keep the date of birth column only.
    fn adopt_old_columns(&mut self, root: &Path) {
        let Some(s) = self.cached_settings.clone() else {
            return;
        };
        if s.record.email.is_none() && s.record.mobile.is_none() {
            return;
        }
        let moved = vf_fs::contacts::adopt_record_columns(
            root,
            s.record.email.as_deref(),
            s.record.mobile.as_deref(),
        )
        .and_then(|n| {
            let mut new = s.clone();
            new.record.email = None;
            new.record.mobile = None;
            vf_fs::lock::with(root, "portal", "the portal settings", || {
                if settings::load(root, &s.cabinet_id).as_ref() != Some(&s) {
                    return Err(STALE.to_string());
                }
                settings::save(root, &new)
            })?;
            Ok((n, new))
        });
        match moved {
            Ok((n, new)) => {
                self.columns = new.record.clone();
                self.cached_settings = Some(new.clone());
                self.shown = Some(new);
                self.msg = Some(format!(
                    "Access requests are now checked against each client's Portal contacts: the email and mobile columns chosen here were moved over ({n} clients)."
                ));
                self.read_record_again();
            }
            Err(e) => {
                self.err = Some(format!(
                    "The email and mobile columns could not be moved to Portal contacts: {e}"
                ))
            }
        }
    }

    /// How `column` writes its dates, for the firm's choice `order`: worked out from the whole
    /// list the first time, then kept until the list or the choice changes.
    fn written_for(&mut self, column: Option<String>, order: DateOrder) -> Written {
        if let Some((c, o, w)) = &self.written
            && (c, o) == (&column, &order)
        {
            return *w;
        }
        let w = self.rows.written(column.as_deref(), order);
        self.written = Some((column, order, w));
        w
    }

    /// Reads the client record requests are checked against: the client list's extra columns
    /// and each client's row. Once per visit, and again after [`Self::read_record_again`]
    /// (Refresh, Save, Read the client list again): the list can be large and the cabinet is
    /// often on a share, so not on the 10-second timer, and on its own thread with a spinner
    /// (code review 2026-11-14: it froze the window on a slow share).
    fn load_record(&mut self, ctx: &egui::Context, root: &Path) {
        if !self.record_read && self.record_job.is_none() {
            self.record_read = true;
            let root = root.to_path_buf();
            match crate::ui::job::Job::start(ctx, "Reading the client list…", move || {
                read_record(&root)
            }) {
                Ok(j) => self.record_job = Some(j),
                Err(e) => {
                    self.record_err = Some(format!("The client list could not be read: {e}"));
                }
            }
        }
        let Some(done) = crate::ui::job::take(&mut self.record_job) else {
            return;
        };
        self.written = None;
        match done.and_then(|r| r) {
            Ok((extras, rows, dir)) => {
                self.record_err = None;
                self.extras = extras;
                self.rows = rows;
                self.dir = dir;
                // Found again against the contacts just read.
                self.suggest.clear();
            }
            Err(e) => {
                self.record_err = Some(format!("The client list could not be read: {e}"));
                self.extras.clear();
                self.rows.clear();
                self.dir.clear();
                self.suggest.clear();
            }
        }
    }

    /// The client list is read again when the page is next drawn.
    fn read_record_again(&mut self) {
        self.record_read = false;
    }
}

/// The client list's extra columns (token names, sorted), each client's row by its ID, and
/// every client's Portal contacts and names (PA-106).
type RecordRead = (
    Vec<String>,
    record::Rows,
    Vec<vf_fs::contacts::ClientContacts>,
);

/// Reads the client list from the cabinet (asks the share: never on the window's thread).
fn read_record(root: &Path) -> Result<RecordRead, String> {
    let master = vf_fs::clients::load(root).map_err(|e| e.to_string())?;
    let mut extras: Vec<String> = Vec::new();
    let mut rows = record::Rows::default();
    for c in master.clients() {
        for k in c.extra.keys() {
            if !extras.contains(k) {
                extras.push(k.clone());
            }
        }
        rows.insert(&c.id, c.extra.clone());
    }
    extras.sort_by_key(|k| k.to_lowercase());
    // No client list yet: nobody to find a request among.
    let dir = vf_fs::contacts::directory(root).unwrap_or_default();
    Ok((extras, rows, dir))
}

/// What Approve offers to share with someone given access (PA-106): doc types and years.
fn approve_share_choice(ui: &mut egui::Ui, a: &mut AdminPortal, env: &mut Env) {
    if a.doc_types.is_empty() {
        a.doc_types = env
            .conn
            .prepare("SELECT id, name FROM doc_type WHERE deleted_at IS NULL ORDER BY sort, name")
            .and_then(|mut st| st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
            .unwrap_or_default();
    }
    ui.add_space(6.0);
    egui::CollapsingHeader::new(format!(
        "On Approve, offer to share documents ({} doc types)",
        a.approve_share.doc_types.len()
    ))
    .id_salt("portal-approve-share")
    .show(ui, |ui| {
        ui.label(widgets::caption(
            "When a request is approved, the client's documents of these doc types are listed \
             under Also share with them: those of the most recent tax years ticked, older ones \
             unticked. The person sees the ones left ticked as soon as they sign in.",
        ));
        ui.horizontal(|ui| {
            let l = ui.label("Most recent tax years");
            ui.add(egui::DragValue::new(&mut a.approve_share.years).range(1..=10))
                .labelled_by(l.id);
        });
        for (id, name) in &a.doc_types {
            let mut on = a.approve_share.doc_types.contains(id);
            if ui.checkbox(&mut on, name).changed() {
                if on {
                    a.approve_share.doc_types.push(id.clone());
                } else {
                    a.approve_share.doc_types.retain(|x| x != id);
                }
            }
        }
    });
}

/// One of the column choices of Admin › Portal: "(none)" or a client-list column.
fn column_choice(ui: &mut egui::Ui, label: &str, extras: &[String], chosen: &mut Option<String>) {
    let l = ui.label(label);
    egui::ComboBox::from_id_salt(("portal-column", label))
        .width(220.0)
        .selected_text(chosen.clone().unwrap_or_else(|| "(none)".into()))
        .show_ui(ui, |ui| {
            ui.selectable_value(chosen, None, "(none)");
            for e in extras {
                ui.selectable_value(chosen, Some(e.clone()), e.as_str());
            }
            // A column chosen earlier that the client list no longer has stays shown.
            if let Some(c) = chosen.clone()
                && !extras.contains(&c)
            {
                ui.selectable_value(
                    chosen,
                    Some(c.clone()),
                    format!("{c} (not in the client list)"),
                );
            }
        })
        .response
        .labelled_by(l.id);
    ui.end_row();
}

/// What Admin › Portal says of a portal older than Invite from client list, request matching
/// and adding a login at once need; nothing for one that has them.
fn old_portal_words(st: &vf_portal::api::Status) -> Option<String> {
    (!st.has_contacts()).then(|| {
        format!(
            "This portal is {}: update it to {} or later for Invite from client list, matching access requests by Portal contacts, and adding someone with a login at once from Share.",
            st.version,
            vf_portal::api::CONTACTS_VERSION
        )
    })
}

/// Admin › Portal: the address, the key, Test, and what clients sent lately.
pub fn admin_settings(ui: &mut egui::Ui, a: &mut AdminPortal, env: &mut Env) {
    a.load(env.root);
    a.load_record(ui.ctx(), env.root);
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
    // An older portal, said once here (owner 2026-11-24).
    if let Some(rx) = &a.version_job
        && let Ok(st) = rx.try_recv()
    {
        a.version_job = None;
        a.version_seen = Some(st);
    }
    if a.version_seen.is_none()
        && a.version_job.is_none()
        && let Some(s) = a.cached_settings.clone()
    {
        a.version_job = Some(spawn(ui.ctx(), move || core::client(&s).status().ok()));
    }
    if let Some(Some(st)) = &a.version_seen
        && let Some(w) = old_portal_words(st)
    {
        ui.label(RichText::new(w).color(theme::warn()));
    }
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
    ui.add_space(6.0);
    ui.checkbox(
        &mut a.b2_list,
        "Show the documents in B2 on the portal's staff page",
    );
    ui.label(widgets::caption(
        "Sends the portal a list of the documents this cabinet has in B2, by client: where each \
         is in the client's folder, its size and tax year. Staff then see every client with \
         documents in B2, and those documents' names, on the staff page. Only names are sent, \
         never a document; clients see nothing of it. The list follows B2 sync, up to 10 minutes \
         behind, while Vibe Files is open on the computer B2 sync runs on. Unticking it removes \
         the lists from the portal. Needs Vibe Portal 0.3.15 or later, and this version of Vibe \
         Files on every PC.",
    ));
    ui.add_space(6.0);
    ui.checkbox(
        &mut a.todo_lists,
        "To-do lists: ask clients for documents and answers on the portal",
    );
    ui.label(widgets::caption(
        "For every PC of this cabinet: a client's To-do lists… (in the client's details) and \
         Admin › To-do templates. Each answer a client gives on the portal is kept with its date, \
         time, internet address, browser and person, for a PDF report, and Vibe Files keeps a \
         copy of each list in the client's folder (_Vibe\\todo). The portal must have them turned \
         on too (its Settings › To-do lists). Needs Vibe Portal 0.5 or later.",
    ));
    ui.add_space(6.0);
    ui.checkbox(
        &mut a.balances,
        "Balance owed: post a balance a client pays on the portal",
    );
    ui.label(widgets::caption(
        "For every PC of this cabinet: a client's Balance owed… (in the client's details) and \
         Admin › Portal › Balances. The client pays by card or bank account on Stripe's page, \
         into the firm's own Stripe account; a paid balance's receipt, with its log, is filed in \
         the client's Billing folder with the doc type Payment receipt (added when this is \
         saved). Vibe Files keeps a copy of each balance in the client's folder \
         (_Vibe\\balance.jsonl), never sent to B2. The portal must have Stripe set up and \
         balance cards turned on (its Settings › Payments). Needs Vibe Portal 0.6 or later.",
    ));
    ui.add_space(6.0);
    ui.label(RichText::new("Checking access requests").strong());
    ui.label(widgets::caption(
        "A request is looked up in every client's Portal contacts (the client CSV's, or typed in \
         a client's panel): a client whose taxpayer or spouse has the email or mobile the person \
         confirmed is offered for one-click approval, unless the portal found the name on the \
         ID different from the name on the request, or the date of birth on the ID differs from \
         the client list's. A date of birth never counts alone. Nothing from the client list \
         is sent to the portal, only what matched. Choose the column with the client's date of \
         birth (ticked as a token in Admin › Clients) to compare it.",
    ));
    egui::Grid::new("portal-record-columns")
        .num_columns(2)
        .show(ui, |ui| {
            column_choice(ui, "Client's date of birth", &a.extras, &mut a.columns.dob);
            let l = ui.label("Dates are written");
            let words = |o: DateOrder| match o {
                DateOrder::Auto => "Work it out from the list",
                DateOrder::MonthFirst => "Month first (1/6/1990 is 6 January)",
                DateOrder::DayFirst => "Day first (6/1/1990 is 6 January)",
            };
            egui::ComboBox::from_id_salt("portal-date-order")
                .width(220.0)
                .selected_text(words(a.columns.dates))
                .show_ui(ui, |ui| {
                    for o in [DateOrder::Auto, DateOrder::MonthFirst, DateOrder::DayFirst] {
                        ui.selectable_value(&mut a.columns.dates, o, words(o));
                    }
                })
                .response
                .labelled_by(l.id);
            ui.end_row();
        });
    approve_share_choice(ui, a, env);
    // What "Work it out" finds in the chosen column, so the firm can see it is right.
    if a.columns.dates == DateOrder::Auto && a.columns.dob.is_some() {
        ui.label(widgets::caption(
            match a.written_for(a.columns.dob.clone(), DateOrder::Auto) {
                Written::DayFirst => {
                    "The date of birth column writes the day first (it has several dates like \
                     25/12/1985)."
                }
                Written::MonthFirst => {
                    "The date of birth column is read month first. If your list writes the day \
                     first and no day in it is over 12, choose Day first."
                }
                Written::Mixed => {
                    "The date of birth column does not say clearly how its dates are written: \
                     it has dates written both ways, or only one or two with the day first. A \
                     date that could be either (6/1/1990) is not compared: choose how they are \
                     written, or tidy the list."
                }
            },
        ));
    }
    if ui
        .button("Read the client list again")
        .on_hover_text("After ticking a column as a token in Admin › Clients.")
        .clicked()
    {
        a.read_record_again();
    }
    if let Some(j) = &a.record_job {
        j.show(ui);
    }
    widgets::message_line(ui, a.record_err.as_deref());
    let busy = a.status.is_some();
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!busy, widgets::primary_button("Save"))
            .clicked()
        {
            save(a, ui.ctx(), env);
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
                                let mut m = format!(
                                    "Connected to {}'s portal (version {}). {}",
                                    st.firm_name,
                                    st.version,
                                    st.storage_words()
                                );
                                if let Some(w) = old_portal_words(&st) {
                                    m.push(' ');
                                    m.push_str(&w);
                                }
                                m
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
                stop(a, ui.ctx(), env);
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
    unfiled(ui, a, env);
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

/// How often Admin › Portal reads the list of uploads not filed while it is shown.
const FILING_EVERY: Duration = Duration::from_secs(60);

/// Starts a check of what was filed, after `act` (Fetch again, Dismiss) when given.
fn start_filing(
    ui: &egui::Ui,
    a: &mut AdminPortal,
    root: &Path,
    s: PortalSettings,
    act: Option<(core::Pending, bool)>,
) {
    a.filing_at = Some(Instant::now());
    let root = root.to_path_buf();
    a.filing_job = Some(spawn(ui.ctx(), move || {
        let (said, audit) = match act {
            None => (Ok(String::new()), None),
            Some((p, again)) => {
                let portal = core::client(&s);
                let u = &p.upload.upload;
                if again {
                    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
                    match vf_portal::filed::fetch_again(&root, &portal, &p.upload, &now) {
                        Ok(r) => (
                            Ok(format!("{} is back in {}.", u.name, core::INBOX)),
                            Some(AuditEntry::new(
                                "portal_receive_again",
                                None,
                                serde_json::json!({
                                    "upload_id": u.upload_id, "client_id": u.client_id,
                                    "name": r.name, "saved_as": r.saved_as, "size": r.size,
                                }),
                            )),
                        ),
                        Err(e) => (Err(format!("{}: {e}", u.name)), None),
                    }
                } else {
                    match portal.mark_dismissed(&u.upload_id) {
                        Ok(()) => (
                            Ok(format!("{} was dismissed.", u.name)),
                            Some(AuditEntry::new(
                                "portal_upload_dismissed",
                                None,
                                serde_json::json!({
                                    "upload_id": u.upload_id, "client_id": u.client_id,
                                    "name": u.name, "where": format!("{:?}", p.at),
                                }),
                            )),
                        ),
                        Err(e) => (Err(format!("{}: {}", u.name, e.message())), None),
                    }
                }
            }
        };
        FilingDone {
            report: core::check_filed(&root, &s),
            said,
            audit,
        }
    }));
}

/// Admin › Portal: what Vibe Files fetched and has not filed; Fetch again and Dismiss (PA-43).
fn unfiled(ui: &mut egui::Ui, a: &mut AdminPortal, env: &mut Env) {
    let Some(s) = a.cached_settings.clone() else {
        return;
    };
    if let Some(rx) = &a.filing_job
        && let Ok(done) = rx.try_recv()
    {
        a.filing_job = None;
        let mut rows: Vec<AuditEntry> = done
            .report
            .reported
            .iter()
            .map(|name| {
                AuditEntry::new(
                    "portal_upload_filed",
                    None,
                    serde_json::json!({ "name": name }),
                )
            })
            .collect();
        rows.extend(done.audit);
        a.filing = Some(done.report);
        let unrecorded = record(env, "portal_inbox", rows).unwrap_or_default();
        match done.said {
            Ok(m) if m.is_empty() => {}
            Ok(m) => {
                a.msg = Some(format!("{m}{unrecorded}"));
                a.err = None;
            }
            Err(e) => a.err = Some(e),
        }
    }
    let busy = a.filing_job.is_some();
    // Tests show a report of their own (`show_filing`) rather than calling a portal.
    if !busy && !cfg!(test) && a.filing_at.is_none_or(|t| t.elapsed() >= FILING_EVERY) {
        start_filing(ui, a, env.root, s.clone(), None);
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new("Fetched, not filed yet").strong());
        if busy {
            ui.spinner();
        } else if ui.small_btn("Check again").clicked() {
            a.filing_at = None;
        }
    });
    let Some(f) = a.filing.clone() else {
        ui.label(widgets::caption("Reading the portal…"));
        return;
    };
    if let Some(p) = f.problems.first()
        && f.pending.is_empty()
    {
        ui.label(widgets::caption(p.as_str()));
        return;
    }
    ui.label(widgets::caption(format!(
        "The portal keeps each upload until Intake files it (or you dismiss it here), then {} days;          one never filed is cleared {} days after it was fetched. If the Portal inbox lost a file,          Fetch again puts it back.",
        f.keep_days, f.cap_days
    )));
    if f.pending.is_empty() {
        ui.label(widgets::caption(
            "Nothing: Intake has filed everything fetched.",
        ));
        return;
    }
    let mut act: Option<(core::Pending, bool)> = None;
    egui::ScrollArea::vertical()
        .id_salt("portal-unfiled")
        .max_height(220.0)
        .show(ui, |ui| {
            for p in &f.pending {
                let u = &p.upload.upload;
                ui.horizontal_wrapped(|ui| {
                    ui.label(widgets::local_time(&p.upload.fetched_at));
                    ui.label(RichText::new(&u.name).strong());
                    ui.label(widgets::caption(format!(
                        "{} · fetched by {}",
                        u.client_name,
                        p.upload.fetched_by.as_deref().unwrap_or("?")
                    )));
                    let (words, warn) = match p.at {
                        core::Where::InInbox => ("in the Portal inbox", false),
                        core::Where::Missing => {
                            ("not in the Portal inbox, and Intake has not filed it", true)
                        }
                        core::Where::NotReceived => ("this cabinet has no record of it", true),
                    };
                    ui.label(if warn {
                        RichText::new(words).color(theme::warn())
                    } else {
                        widgets::caption(words)
                    });
                    if p.at != core::Where::InInbox
                        && ui
                            .add_enabled(!busy, egui::Button::new("Fetch again"))
                            .on_hover_text(format!("Put it in {} again for Intake.", core::INBOX))
                            .clicked()
                    {
                        act = Some((p.clone(), true));
                    }
                    if a.confirm_dismiss.as_deref() == Some(u.upload_id.as_str()) {
                        ui.label(
                            RichText::new(format!(
                                "Dismiss it? The portal deletes its copy {} days later.",
                                f.keep_days
                            ))
                            .color(theme::warn()),
                        );
                        if ui
                            .add_enabled(!busy, egui::Button::new("Dismiss"))
                            .clicked()
                        {
                            a.confirm_dismiss = None;
                            act = Some((p.clone(), false));
                        }
                        if ui.button("Keep it").clicked() {
                            a.confirm_dismiss = None;
                        }
                    } else if ui
                        .add_enabled(!busy, egui::Button::new("Dismiss…"))
                        .on_hover_text(
                            "Not to be filed (junk, a duplicate): the portal stops keeping it.",
                        )
                        .clicked()
                    {
                        a.confirm_dismiss = Some(u.upload_id.clone());
                    }
                });
            }
        });
    if let Some(act) = act {
        a.msg = None;
        a.err = None;
        start_filing(ui, a, env.root, s, Some(act));
    }
}

fn stop(a: &mut AdminPortal, ctx: &egui::Context, env: &mut Env) {
    a.msg = None;
    a.err = None;
    // The lists of documents in B2 leave the portal with it (PA-80): read before the key goes.
    // Whether or not the setting is ticked now: an untick whose removal failed left them there.
    // Settings that are there and cannot be read just now: nothing is stopped on a guess
    // (they read as "not set": the file was removed and the lists of documents stayed on
    // the portal under "The portal is no longer used."; QA 2026-11-17).
    if settings::unreadable(env.root) {
        a.err = Some("Not stopped: the portal settings in the cabinet could not be read just now. Try again in a moment.".into());
        return;
    }
    let listing = cabinet_id(env.root).and_then(|id| settings::load(env.root, &id));
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
    a.shown = None;
    a.changed = true;
    a.read_at = None;
    let said = format!("The portal is no longer used.{unrecorded}");
    match listing {
        Some(s) if s.b2_list => clear_lists(a, ctx, s, said, AfterFailure::SetUpAgain),
        // Not ticked: there are lists only if an earlier untick could not remove them (it said
        // so then). Tried once more, without a word when the portal still cannot be reached.
        Some(s) => clear_lists(a, ctx, s, said, AfterFailure::Quiet),
        None => a.msg = Some(said),
    }
}

/// What Save says when another PC changed the portal settings while this page was open.
const STALE: &str = "Someone else changed the portal settings while this page was open. They are \
                     shown again now: make your change again.";

fn save(a: &mut AdminPortal, ctx: &egui::Context, env: &mut Env) {
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
        Ok(s) => s
            .with_record(RecordColumns {
                email: None,
                mobile: None,
                ..a.columns.clone()
            })
            .with_b2_list(a.b2_list)
            .with_todo_lists(a.todo_lists)
            .with_balances(a.balances)
            .with_approve_share(a.approve_share.clone()),
        Err(e) => {
            a.err = Some(e);
            return;
        }
    };
    // The portal the lists were being sent to, when they were: they leave it when the setting
    // is unticked, and when the address or key changes (the new portal gets its own).
    let old_listing = a.shown.clone().filter(|s| s.b2_list);
    // Saved only when the cabinet's settings are still the ones this form was filled from: a
    // change made on another PC meanwhile is not written over (as the B2 rules, PA-68).
    let saved = vf_fs::lock::with(env.root, "portal", "the portal settings", || {
        // Not readable is not "changed by someone else".
        if settings::unreadable(env.root) {
            return Err("The portal settings in the cabinet could not be read just now: nothing was saved. Try again in a moment.".to_string());
        }
        if settings::load(env.root, &id) != a.shown {
            return Err(STALE.to_string());
        }
        settings::save(env.root, &s)
    });
    if let Err(e) = saved {
        if e == STALE {
            // Shown again from the cabinet when the page is next drawn.
            a.loaded = false;
            a.read_at = None;
            a.err = Some(e);
        } else {
            a.err = Some(format!("Not saved: {e}"));
        }
        return;
    }
    a.shown = Some(s.clone());
    a.read_record_again();
    // Addendum §16: a receipt needs its doc type (Billing/{Year}) in the list and the catalog.
    let receipt_type = if s.balances {
        match vf_fs::doctypes::ensure(
            env.root,
            vf_portal::inbox::RECEIPT_DOC_TYPE,
            "Billing/{Year}",
            true,
        ) {
            Ok(true) => {
                match vf_fs::doctypes::sync(env.conn, env.root, env.device_id, "doc_types_edit") {
                    Ok(_) => " The doc type Payment receipt was added.".to_string(),
                    Err(e) => format!(
                        " The doc type Payment receipt was added but not yet in the catalog: {e}"
                    ),
                }
            }
            Ok(false) => String::new(),
            Err(e) => format!(" The doc type Payment receipt could not be added: {e}"),
        }
    } else {
        String::new()
    };
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
            serde_json::json!({
                "url": s.url,
                "fetch_minutes": s.fetch_minutes,
                "b2_list": s.b2_list,
                "todo_lists": s.todo_lists,
                "balances": s.balances,
                "record_columns": {
                    "email": s.record.email,
                    "mobile": s.record.mobile,
                    "dob": s.record.dob,
                    "dates": s.record.dates,
                },
            }),
        )],
    )
    .unwrap_or_default();
    a.url = s.url.clone();
    a.changed = true;
    a.read_at = None;
    let said = format!("Saved for every PC.{inbox}{receipt_type}{unrecorded}");
    match old_listing {
        // Unticked, or another portal now: the lists leave the portal they were sent to, now,
        // not when someone remembers them. (A new key for the same portal: they stay.)
        Some(old) if !s.b2_list || old.url != s.url => {
            let after = if old.url != s.url {
                AfterFailure::OldPortal
            } else {
                AfterFailure::TickAndUntick
            };
            let said = if s.b2_list {
                format!("{said} {LISTED_SOON}")
            } else {
                said
            };
            clear_lists(a, ctx, old, said, after);
        }
        // Ticked now: whether this portal can take the lists is said here, where it was ticked.
        None if s.b2_list => {
            a.msg = Some(said.clone());
            a.status = Some(spawn(ctx, move || {
                match core::client(&s).cabinet_state() {
                    Err(e) if e.message() == vf_portal::listing::NEEDS_P90 => Err(
                        "The portal cannot show the documents in B2 yet: update the portal (on \
                         the appliance: sudo vibe upgrade portal). The setting is saved, and the \
                         lists are sent once it is updated."
                            .to_string(),
                    ),
                    // Reached or not, the send itself says what goes wrong.
                    _ => Ok(format!("{said} {LISTED_SOON}")),
                }
            }));
        }
        _ => a.msg = Some(said),
    }
}

/// Said when the setting is saved ticked.
const LISTED_SOON: &str = "The computer B2 sync runs on sends the lists of documents in B2 within about 10 minutes, while Vibe Files is open there.";

/// What to say when the lists could not be removed from the portal.
#[derive(Clone, Copy)]
enum AfterFailure {
    /// The portal is still set up: the setting can be ticked and unticked again.
    TickAndUntick,
    /// The address was changed: they are on the portal used before.
    OldPortal,
    /// Stop using the portal: the address and key are gone from the cabinet.
    SetUpAgain,
    /// Stop using the portal with the setting not ticked: most likely there are none.
    Quiet,
}

/// Removes the lists of documents in B2 from the portal `s` (the setting was unticked, the
/// address changed, or the portal is no longer used), on a thread. `said` is shown at once and
/// with what came of it; a removal that failed is shown as a problem, with what to do.
fn clear_lists(
    a: &mut AdminPortal,
    ctx: &egui::Context,
    s: PortalSettings,
    said: String,
    after: AfterFailure,
) {
    a.msg = Some(said.clone());
    a.status = Some(spawn(ctx, move || {
        match core::client(&s).clear_cabinet_lists() {
            Ok(0) => Ok(said),
            Ok(n) => Ok(format!(
                "{said} The lists of documents in B2 were removed from the portal ({n} client{}).",
                if n == 1 { "" } else { "s" }
            )),
            // An older portal has none to remove.
            Err(e) if e.status() == Some(404) => Ok(said),
            Err(_) if matches!(after, AfterFailure::Quiet) => Ok(format!(
                "{said} If lists of documents in B2 were ever sent to this portal, they may \
                 still be there."
            )),
            Err(e) => Err(format!(
                "The lists of documents in B2 could NOT be removed from the portal ({}). {}",
                e.message(),
                match after {
                    AfterFailure::TickAndUntick =>
                        "Tick the setting, save, then untick it and save again.".to_string(),
                    AfterFailure::OldPortal => format!(
                        "They are still on the portal used before ({}). To remove them, enter \
                         that address and its key again, tick the setting, save, untick it, and \
                         save.",
                        s.url
                    ),
                    AfterFailure::SetUpAgain | AfterFailure::Quiet =>
                        "To remove them, enter the portal address and key again, tick the \
                         setting, save, untick it, save, and then stop using the portal."
                            .to_string(),
                }
            )),
        }
    }));
}

pub(crate) fn load_clients(env: &mut Env) -> Vec<ClientRef> {
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

/// What Approve does for a request, given the client picked.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ApproveStep {
    /// The request matches the client record: one step, saying what matched.
    Matched(Check),
    /// No match and the portal requires a check, or a match with the ID's name flagged: ask
    /// how staff checked.
    Ask,
    /// No match, and the portal asks for nothing.
    Plain,
}

fn approve_step(cmp: Option<&record::Comparison>, check_required: bool) -> ApproveStep {
    let matched = cmp.map(record::Comparison::matched).unwrap_or_default();
    if !matched.is_empty() {
        // The portal flagged the name on the ID: the match is not enough, whatever the portal
        // asks for (owner 2026-11-15, Q-111). An expired ID is only shown.
        if cmp.is_some_and(|c| c.name_differs) {
            ApproveStep::Ask
        } else {
            ApproveStep::Matched(Check::record_match(&matched))
        }
    } else if check_required {
        ApproveStep::Ask
    } else {
        ApproveStep::Plain
    }
}

/// What Approve sends for a request (PA-106): the clients with the role at each, how the
/// person was checked, the documents to share with them once they have access, and the
/// contact they confirmed to keep in a client's Portal contacts.
#[derive(Debug, Clone)]
struct Approval {
    clients: Vec<(ClientRef, String)>,
    check: Option<Check>,
    /// (client, document) to share.
    share: Vec<(ClientRef, ShareDoc)>,
    /// (client id, contact field, value) to save as typed by hand.
    save: Vec<(String, &'static str, String)>,
}

/// A decision on a request: approve (see [`Approval`]), or deny.
type Decision = (String, Option<Approval>);

/// A document Approve offers to share (PA-106, owner 2026-11-24).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareDoc {
    pub doc_id: String,
    pub rel_path: String,
    pub tax_year: Option<i32>,
    /// Of the most recent tax years chosen in Admin › Portal (or with no tax year): ticked to
    /// start; an older one is listed unticked.
    pub recent: bool,
}

/// The documents of `client` Approve offers to share, by the firm's choice in Admin › Portal:
/// those of the chosen doc types, newest first; per doc type, the most recent `years` tax years
/// (and documents with no tax year) are `recent`. At most 30.
fn share_offer(
    conn: &vf_core::db::Connection,
    client_id: &str,
    how: &settings::ApproveShare,
) -> Vec<ShareDoc> {
    if how.doc_types.is_empty() {
        return Vec::new();
    }
    let marks = vec!["?"; how.doc_types.len()].join(",");
    let sql = format!(
        "SELECT id, rel_path, tax_year, doc_type_id FROM document \
         WHERE client_id = ? AND deleted_at IS NULL AND doc_type_id IN ({marks}) \
         ORDER BY tax_year IS NULL, tax_year DESC, updated_at DESC LIMIT 200"
    );
    let params = vf_core::db::params_from_iter(
        std::iter::once(client_id.to_string()).chain(how.doc_types.iter().cloned()),
    );
    let rows: Vec<(String, String, Option<i32>, String)> = conn
        .prepare(&sql)
        .and_then(|mut st| {
            st.query_map(params, |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect()
        })
        .unwrap_or_default();
    // Each doc type's most recent years.
    let mut years: BTreeMap<&str, Vec<i32>> = BTreeMap::new();
    for (_, _, y, t) in &rows {
        if let Some(y) = y {
            let v = years.entry(t.as_str()).or_default();
            if !v.contains(y) {
                v.push(*y);
            }
        }
    }
    let keep = usize::from(how.years.max(1));
    rows.iter()
        .take(30)
        .map(|(id, rel, y, t)| ShareDoc {
            doc_id: id.clone(),
            rel_path: rel.clone(),
            tax_year: *y,
            recent: match y {
                None => true,
                Some(y) => years
                    .get(t.as_str())
                    .is_some_and(|v| v.iter().take(keep).any(|x| x == y)),
            },
        })
        .collect()
}

/// What a request's person would be saved as, when the contact they confirmed is not on file
/// for them: the contact field (taxpayer's or spouse's email or cell) and the value.
fn contact_to_save(confirmed: &str, spouse: bool) -> Option<(&'static str, String)> {
    let c = confirmed.trim();
    if c.is_empty() {
        return None;
    }
    let key = match (c.contains('@'), spouse) {
        (true, false) => "email",
        (true, true) => "spouse_email",
        (false, false) => "cell",
        (false, true) => "spouse_cell",
    };
    Some((key, c.to_string()))
}

/// The documents offered on Approve, each with its client.
type Offer = Vec<(ClientRef, ShareDoc)>;

/// The suggestions shown for one request, and what was ticked.
#[derive(Debug, Clone, Default)]
struct Suggest {
    found: Vec<vf_fs::contacts::Found>,
    /// Client ids ticked.
    ticked: BTreeSet<String>,
    /// Keep the confirmed contact in the clients' Portal contacts (name leads; ticked to start).
    save: bool,
    /// The same for a client picked by hand under Other client…: unticked to start, since it
    /// may replace the contact on file (review 2026-11-24: a bookkeeper approved by hand).
    save_other: bool,
    /// The documents offered, for the clients they were worked out for.
    offer: Option<(Vec<String>, Offer)>,
    /// Documents ticked (doc ids).
    share: BTreeSet<String>,
}

/// Admin › Portal requests: confirmed "Request access" forms; link each to a client, or deny.
/// The contact the person confirmed is looked up in every client's Portal contacts (PA-106):
/// clients that have it are offered ticked with the person's role there, one click to approve;
/// when none has it, clients whose people have the name typed are offered as leads, approved
/// only after staff say how they checked. Any other client can still be picked. What the
/// barcode on the person's ID said and the portal's flags are shown; a date of birth on the ID
/// that differs from the record asks how staff checked.
pub fn admin_requests(ui: &mut egui::Ui, a: &mut AdminPortal, env: &mut Env) {
    a.load(env.root);
    a.load_record(ui.ctx(), env.root);
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
        match done {
            Err(e) => a.err = Some(e),
            Ok(m) if !m.is_empty() => a.msg = Some(m),
            Ok(_) => {}
        }
        if let Some(l) = list {
            a.check_required = l.check_required;
            // A question stays open (with its note) until its request is decided: a refusal
            // by the portal leaves it to be tried again.
            if let Some(id) = &a.asking
                && !l.requests.iter().any(|r| &r.id == id)
            {
                a.asking = None;
                a.ask_targets = None;
                a.how = (None, String::new());
            }
            a.suggest
                .retain(|id, _| l.requests.iter().any(|r| &r.id == id));
            a.waiting = Some(l.requests.len());
            a.requests = Some(l.requests);
        }
    }
    if a.requests.is_none() && a.req_job.is_none() && a.err.is_none() {
        let s2 = s.clone();
        a.req_job = Some(spawn(ui.ctx(), move || {
            match core::client(&s2).access_requests() {
                Ok(l) => (Some(l), Ok(String::new())),
                Err(e) => (None, Err(e.message())),
            }
        }));
    }
    ui.heading("Portal requests");
    ui.label(widgets::caption(
        "People who asked for portal access and confirmed their email or phone. The contact they \
         confirmed is looked up in every client's Portal contacts; link each request to the \
         clients whose files they should see, or deny it. Only the confirmed contact is used.",
    ));
    widgets::message_line(ui, a.record_err.as_deref());
    if let Some(j) = &a.record_job {
        j.show(ui);
    }
    if a.check_required {
        ui.label(widgets::caption(
            "Your portal asks for a check before approving: a request that matches the client \
             record approves in one step (unless the name on the ID differs); for any other, \
             say how you checked.",
        ));
    }
    // Nothing is approved while the client list is being read: a request would be compared
    // with an empty record.
    let busy = a.req_job.is_some() || a.record_job.is_some();
    if ui
        .add_enabled(!busy, egui::Button::new("Refresh"))
        .clicked()
    {
        a.err = None;
        a.msg = None;
        a.requests = None;
        a.asking = None;
        a.ask_targets = None;
        a.how = (None, String::new());
        // The clients and the client list may have changed too.
        a.clients = load_clients(env);
        a.pick.clear();
        a.suggest.clear();
        a.read_record_again();
    }
    if let Some(m) = &a.msg {
        ui.label(RichText::new(m.as_str()).color(theme::health_ok()));
    }
    // How the list writes its dates of birth: looked up once, not for every request drawn.
    let written = a.written_for(s.record.dob.clone(), s.record.dates);
    // Only the date of birth is compared with the client list's columns; the email and mobile
    // are the Portal contacts' (PA-106). A cabinet whose settings still name the old columns
    // (before they were moved over) compares them as before.
    let dob_only = RecordColumns {
        email: None,
        mobile: None,
        dob: s.record.dob.clone(),
        dates: s.record.dates,
    };
    let legacy = s.record.email.is_some() || s.record.mobile.is_some();
    let mut decision: Option<Decision> = None;
    // Ask how they were checked: for (request, the approval it would make).
    let mut ask: Option<Option<(String, Option<Approval>)>> = None;
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
                let confirmed = r.confirmed_contact.clone().unwrap_or_default();
                let by_email = confirmed.contains('@');
                let contact_word = if by_email { "email" } else { "cell" };
                // The clients the request may be for, worked out once per request.
                if !a.suggest.contains_key(&r.id) {
                    let found = vf_fs::contacts::find(&confirmed, &r.name, &a.dir);
                    let ticked = found.iter().map(|f| f.client_id.clone()).collect();
                    a.suggest.insert(
                        r.id.clone(),
                        Suggest {
                            found,
                            ticked,
                            save: true,
                            ..Default::default()
                        },
                    );
                }
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
                    // What the barcode on the back of the ID said, and the portal's flags. The
                    // pictures are on the portal's staff page and in the approval PDF.
                    for f in &r.id_flags {
                        ui.label(RichText::new(flag_words(f)).color(theme::warn()).strong());
                    }
                    if let Some(scan) = &r.id_scan {
                        ui.label(widgets::caption(
                            "Read from the barcode on the back of their ID. A barcode can be \
                             made by anyone: compare it with the pictures on the portal's staff \
                             page.",
                        ));
                        ui.label(format!(
                            "Name on the ID: {}    Date of birth: {}    Expires: {}",
                            scan.full_name(),
                            scan.dob.as_deref().unwrap_or("-"),
                            scan.expires.as_deref().unwrap_or("-"),
                        ));
                        let address = scan.address_line();
                        if !address.is_empty() || !scan.issuer.is_empty() {
                            ui.label(format!("Address: {address}    Issued by: {}", scan.issuer));
                        }
                    }
                    let asking_this = a.asking.as_deref() == Some(r.id.as_str());
                    let empty = BTreeMap::new();
                    let dob_of = |code: &str| {
                        record::compare_in(r, a.rows.get(code).unwrap_or(&empty), &dob_only, written)
                    };
                    let name_differs = r.id_flags.iter().any(|f| f == record::FLAG_NAME_DIFFERS);

                    // ---- The clients the contact or the name leads to (PA-106).
                    let sug = a.suggest.get_mut(&r.id).expect("inserted above");
                    let by_contact = sug
                        .found
                        .iter()
                        .any(|f| f.by != vf_fs::contacts::FoundBy::Name);
                    if !sug.found.is_empty() {
                        ui.add_space(4.0);
                        if by_contact {
                            ui.label(
                                RichText::new(format!(
                                    "The {contact_word} they confirmed is on file for:"
                                ))
                                .color(theme::health_ok())
                                .strong(),
                            );
                        } else {
                            ui.label(
                                RichText::new(format!(
                                    "Not a match: the {contact_word} they confirmed is not on file. Their name is on file for:"
                                ))
                                .color(theme::warn())
                                .strong(),
                            );
                        }
                        let mut differs = false;
                        for f in &sug.found {
                            let how = match f.by {
                                vf_fs::contacts::FoundBy::Email => format!("{}'s email on file", f.person),
                                vf_fs::contacts::FoundBy::Cell => format!("{}'s cell on file", f.person),
                                vf_fs::contacts::FoundBy::Name => f.person.clone(),
                            };
                            let mut on = sug.ticked.contains(&f.client_id);
                            ui.horizontal_wrapped(|ui| {
                                let label = format!("{} ({}) as {}: {how}", f.client, f.code, f.role);
                                if ui
                                    .add_enabled(!busy && !asking_this, egui::Checkbox::new(&mut on, label))
                                    .changed()
                                {
                                    if on {
                                        sug.ticked.insert(f.client_id.clone());
                                    } else {
                                        sug.ticked.remove(&f.client_id);
                                    }
                                }
                                let cmp = dob_of(&f.code);
                                match cmp.dob {
                                    record::Line::Matches => {
                                        ui.label(widgets::caption("date of birth on the ID matches the record"));
                                    }
                                    record::Line::Differs => {
                                        if on {
                                            differs = true;
                                        }
                                        ui.label(
                                            RichText::new("the date of birth on the ID differs from the record")
                                                .small()
                                                .color(theme::warn()),
                                        );
                                    }
                                    _ => {}
                                }
                            });
                        }
                        if by_contact && name_differs {
                            ui.label(
                                RichText::new(
                                    "The name on the ID differs from the name on the request, so Approve asks how you checked.",
                                )
                                .color(theme::warn()),
                            );
                        }
                        if by_contact && differs {
                            ui.label(
                                RichText::new(
                                    "A date of birth differs, so Approve asks how you checked.",
                                )
                                .color(theme::warn()),
                            );
                        }
                        if !by_contact {
                            ui.checkbox(
                                &mut sug.save,
                                format!(
                                    "Keep {confirmed} as their {contact_word} in the client's Portal contacts"
                                ),
                            );
                        }
                    }

                    // ---- What to share with them on Approve.
                    let (pick, role) = a.pick.entry(r.id.clone()).or_insert((None, String::new()));
                    let chosen_client = pick.and_then(|i| a.clients.get(i)).cloned();
                    let targets: Vec<ClientRef> = if !sug.ticked.is_empty() {
                        sug.found
                            .iter()
                            .filter(|f| sug.ticked.contains(&f.client_id))
                            .map(|f| ClientRef {
                                id: f.client_id.clone(),
                                code: f.code.clone(),
                                name: f.client.clone(),
                            })
                            .collect()
                    } else {
                        chosen_client.clone().into_iter().collect()
                    };
                    let keys: Vec<String> = targets.iter().map(|c| c.id.clone()).collect();
                    if !s.approve_share.doc_types.is_empty()
                        && sug.offer.as_ref().is_none_or(|(k, _)| *k != keys)
                    {
                        let docs: Vec<(ClientRef, ShareDoc)> = targets
                            .iter()
                            .flat_map(|c| {
                                share_offer(env.conn, &c.id, &s.approve_share)
                                    .into_iter()
                                    .map(move |d| (c.clone(), d))
                            })
                            .collect();
                        sug.share = docs
                            .iter()
                            .filter(|(_, d)| d.recent)
                            .map(|(_, d)| d.doc_id.clone())
                            .collect();
                        sug.offer = Some((keys.clone(), docs));
                    }
                    if let Some((_, docs)) = &sug.offer
                        && !targets.is_empty()
                    {
                        if docs.is_empty() {
                            ui.label(widgets::caption(
                                "No documents of the doc types Admin › Portal offers to share.",
                            ));
                        } else {
                            ui.label(RichText::new("Also share with them:").strong());
                            for (c, d) in docs {
                                let mut on = sug.share.contains(&d.doc_id);
                                let name = widgets::file_name(&d.rel_path);
                                let mut label = match d.tax_year {
                                    Some(y) => format!("{y} {name}"),
                                    None => name.to_string(),
                                };
                                if targets.len() > 1 {
                                    label = format!("{} · {label}", c.code);
                                }
                                if !d.recent {
                                    label.push_str("   (older)");
                                }
                                if ui
                                    .add_enabled(!busy && !asking_this, egui::Checkbox::new(&mut on, label))
                                    .changed()
                                {
                                    if on {
                                        sug.share.insert(d.doc_id.clone());
                                    } else {
                                        sug.share.remove(&d.doc_id);
                                    }
                                }
                            }
                        }
                    }
                    // The approval the ticks make, with its check filled in later.
                    let share: Vec<(ClientRef, ShareDoc)> = sug
                        .offer
                        .as_ref()
                        .map(|(_, d)| {
                            d.iter()
                                .filter(|(_, d)| sug.share.contains(&d.doc_id))
                                .cloned()
                                .collect()
                        })
                        .unwrap_or_default();

                    // ---- Approve the clients ticked.
                    let ticked: Vec<&vf_fs::contacts::Found> = sug
                        .found
                        .iter()
                        .filter(|f| sug.ticked.contains(&f.client_id))
                        .collect();
                    if !ticked.is_empty() {
                        let clients: Vec<(ClientRef, String)> = ticked
                            .iter()
                            .map(|f| {
                                (
                                    ClientRef {
                                        id: f.client_id.clone(),
                                        code: f.code.clone(),
                                        name: f.client.clone(),
                                    },
                                    f.role.clone(),
                                )
                            })
                            .collect();
                        let save: Vec<(String, &'static str, String)> = if by_contact || !sug.save {
                            Vec::new()
                        } else {
                            ticked
                                .iter()
                                .filter_map(|f| {
                                    contact_to_save(&confirmed, f.spouse)
                                        .map(|(k, v)| (f.client_id.clone(), k, v))
                                })
                                .collect()
                        };
                        let dobs: Vec<record::Line> = ticked.iter().map(|f| dob_of(&f.code).dob).collect();
                        let needs_check = !by_contact
                            || name_differs
                            || dobs.contains(&record::Line::Differs);
                        let label = match (ticked.len(), needs_check) {
                            (1, false) => format!("Approve as {}", ticked[0].role),
                            (1, true) => format!("Approve as {}…", ticked[0].role),
                            (n, false) => format!("Approve for {n} clients"),
                            (n, true) => format!("Approve for {n} clients…"),
                        };
                        ui.horizontal_wrapped(|ui| {
                            if ui
                                .add_enabled(!busy && !asking_this, widgets::primary_button(label))
                                .clicked()
                            {
                                let mut approval = Approval {
                                    clients,
                                    check: None,
                                    share: share.clone(),
                                    save,
                                };
                                if needs_check {
                                    ask = Some(Some((r.id.clone(), Some(approval))));
                                } else {
                                    // What matched: the contact, and the date of birth when it
                                    // is every ticked client's.
                                    let mut matched = vec![if by_email { "email" } else { "mobile" }];
                                    if dobs.iter().all(|d| *d == record::Line::Matches) {
                                        matched.push("dob");
                                    }
                                    approval.check = Some(Check::record_match(&matched));
                                    decision = Some((r.id.clone(), Some(approval)));
                                }
                            }
                            if ui.add_enabled(!busy, egui::Button::new("Deny")).clicked() {
                                decision = Some((r.id.clone(), None));
                            }
                        });
                    }

                    if !sug.found.is_empty()
                        && ticked.is_empty()
                        && ui.add_enabled(!busy, egui::Button::new("Deny")).clicked()
                    {
                        decision = Some((r.id.clone(), None));
                    }

                    // ---- Any other client, picked by hand.
                    let other = egui::CollapsingHeader::new(if sug.found.is_empty() {
                        "Choose the client"
                    } else {
                        "Other client…"
                    })
                    .id_salt(("portal-req-other", &r.id))
                    .default_open(sug.found.is_empty())
                    .show(ui, |ui| {
                        // The request beside the chosen client's record: its date of birth
                        // column, and whether the contact is on file for the client.
                        let cmp = chosen_client.as_ref().map(|c| {
                            if legacy {
                                record::compare_in(r, a.rows.get(&c.code).unwrap_or(&empty), &s.record, written)
                            } else {
                                let mut cmp = dob_of(&c.code);
                                let on_file = |by| {
                                    a.dir.iter().find(|d| d.client_id == c.id).map(|d| {
                                        let has = d.planned.people.iter().any(|p| {
                                            if by { p.email.is_some() } else { p.phone.is_some() }
                                        });
                                        let hit = sug.found.iter().any(|f| {
                                            f.client_id == c.id
                                                && f.by == if by { vf_fs::contacts::FoundBy::Email } else { vf_fs::contacts::FoundBy::Cell }
                                        });
                                        if hit {
                                            record::Line::Matches
                                        } else if has {
                                            record::Line::Differs
                                        } else {
                                            record::Line::NotInRecord
                                        }
                                    })
                                    .unwrap_or(record::Line::NotInRecord)
                                };
                                cmp.email = if by_email { on_file(true) } else { record::Line::NotOnRequest };
                                cmp.mobile = if by_email || confirmed.is_empty() { record::Line::NotOnRequest } else { on_file(false) };
                                cmp
                            }
                        });
                        // (Not said while the list is being read or could not be: it is empty then.)
                        if legacy
                            && let Some(c) = &chosen_client
                            && a.rows.get(&c.code).is_none()
                            && a.record_err.is_none()
                            && a.record_job.is_none()
                        {
                            ui.label(
                                RichText::new(format!(
                                    "{} (ID {}) is not in the client list, so there is nothing to \
                                     compare this request with.",
                                    c.name, c.code
                                ))
                                .color(theme::warn()),
                            );
                        }
                        if let (Some(c), Some(cmp)) = (&chosen_client, &cmp) {
                            ui.label(format!(
                                "Email: {}    Mobile: {}    Date of birth: {}",
                                cmp.email.words(),
                                cmp.mobile.words(),
                                cmp.dob.words(),
                            ));
                            if let Some(scan) = &r.id_scan {
                                ui.label(format!(
                                    "Name on the ID: {}    Client: {}",
                                    scan.full_name(),
                                    c.name
                                ));
                            }
                            let matched = cmp.matched();
                            if matched.is_empty() {
                                ui.label(
                                    RichText::new("This request does not match the client record.")
                                        .color(theme::warn()),
                                );
                                if cmp.only_dob() {
                                    ui.label(widgets::caption(
                                        "The date of birth is the client's, but a date of birth \
                                         alone is not a match: the email or mobile the person \
                                         confirmed must be the client's too.",
                                    ));
                                }
                                if !legacy {
                                    let spouse = role.to_lowercase().contains("spouse");
                                    let whose = if spouse { "spouse's" } else { "taxpayer's" };
                                    // What is on file there now, which the box would replace.
                                    let now = a
                                        .dir
                                        .iter()
                                        .find(|d| d.client_id == c.id)
                                        .and_then(|d| {
                                            d.planned.people.iter().find(|p| {
                                                (p.role == "Spouse") == spouse
                                            })
                                        })
                                        .and_then(|p| {
                                            if by_email { p.email.clone() } else { p.phone.clone() }
                                        });
                                    let replaces = now
                                        .map(|n| format!(" (replaces {n})"))
                                        .unwrap_or_default();
                                    ui.checkbox(
                                        &mut sug.save_other,
                                        format!(
                                            "Also keep {confirmed} as the {whose} {contact_word} in {}'s Portal contacts{replaces}",
                                            c.name
                                        ),
                                    );
                                }
                            } else {
                                ui.label(
                                    RichText::new("This request matches the client record.")
                                        .color(theme::health_ok()),
                                );
                                if cmp.name_differs {
                                    ui.label(
                                        RichText::new(
                                            "The name on the ID differs from the name on the \
                                             request, so Approve asks how you checked.",
                                        )
                                        .color(theme::warn()),
                                    );
                                }
                            }
                        }
                        ui.horizontal_wrapped(|ui| {
                            let shown = pick
                                .and_then(|i| labels.get(i).cloned())
                                .unwrap_or_else(|| "Choose the client…".into());
                            let l2 = labels.clone();
                            // (Named for a screen reader: the picker had no name.)
                            let cl = ui.label("Client");
                            let (picker, chosen) = widgets::search_combo(
                                ui,
                                ("portal-req", &r.id),
                                280.0,
                                shown,
                                move || l2,
                                *pick,
                                None,
                            );
                            picker.labelled_by(cl.id);
                            if let Some(c) = chosen {
                                *pick = c;
                                // Another client: the question, if open, starts over.
                                if asking_this {
                                    ask = Some(None);
                                }
                            }
                            let l = ui.label("Role");
                            ui.add(egui::TextEdit::singleline(role).desired_width(100.0))
                                .labelled_by(l.id);
                            let ok = pick.is_some() && !busy && !asking_this;
                            if ui.add_enabled(ok, egui::Button::new("Approve")).clicked()
                                && let Some(c) = &chosen_client
                            {
                                let matched = cmp.as_ref().map(record::Comparison::matched).unwrap_or_default();
                                let save = if !legacy && matched.is_empty() && sug.save_other {
                                    let spouse = role.to_lowercase().contains("spouse");
                                    contact_to_save(&confirmed, spouse)
                                        .map(|(k, v)| vec![(c.id.clone(), k, v)])
                                        .unwrap_or_default()
                                } else {
                                    Vec::new()
                                };
                                let mut approval = Approval {
                                    clients: vec![(c.clone(), role.trim().to_string())],
                                    check: None,
                                    share: share.clone(),
                                    save,
                                };
                                match approve_step(cmp.as_ref(), a.check_required) {
                                    ApproveStep::Matched(check) => {
                                        approval.check = Some(check);
                                        decision = Some((r.id.clone(), Some(approval)));
                                    }
                                    ApproveStep::Ask => {
                                        ask = Some(Some((r.id.clone(), Some(approval))));
                                    }
                                    ApproveStep::Plain => {
                                        decision = Some((r.id.clone(), Some(approval)));
                                    }
                                }
                            }
                            if sug.found.is_empty()
                                && ui.add_enabled(!busy, egui::Button::new("Deny")).clicked()
                            {
                                decision = Some((r.id.clone(), None));
                            }
                        });
                    });
                    let _ = other;
                    // How did you check? One choice, and its note where one is asked.
                    if asking_this {
                        ui.separator();
                        ui.label(RichText::new(format!("How did you check {}?", r.name)).strong());
                        ui.label(widgets::caption(
                            "It is kept with the approval on the portal and printed in its PDF. \
                             A text or email to the contact on the request does not count: that \
                             contact is what is being checked.",
                        ));
                        let (how, note) = &mut a.how;
                        for (i, (_, label, _)) in CHECKS.iter().enumerate() {
                            // (Clicking the one already chosen keeps its note.)
                            if ui.radio(*how == Some(i), *label).clicked() && *how != Some(i) {
                                *how = Some(i);
                                note.clear();
                            }
                        }
                        let wanted = how.and_then(|i| CHECKS[i].2);
                        if let Some(label) = wanted {
                            let l = ui.label(label);
                            ui.add(egui::TextEdit::singleline(note).desired_width(320.0))
                                .labelled_by(l.id);
                            limit_utf16(note, NOTE_MAX);
                        }
                        let ready = how.is_some() && (wanted.is_none() || !note.trim().is_empty());
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(
                                    ready && !busy,
                                    widgets::primary_button("Approve with this check"),
                                )
                                .clicked()
                                && let (Some(i), Some(approval)) = (*how, a.ask_targets.clone())
                            {
                                let check = Check {
                                    method: CHECKS[i].0.into(),
                                    matched: Vec::new(),
                                    note: note.trim().to_string(),
                                };
                                decision = Some((
                                    r.id.clone(),
                                    Some(Approval {
                                        check: Some(check),
                                        ..approval
                                    }),
                                ));
                            }
                            if ui.button("Cancel").clicked() {
                                ask = Some(None);
                            }
                        });
                    }
                });
            }
        }
    }
    if let Some(to) = ask {
        match to {
            Some((req, approval)) => {
                a.asking = Some(req);
                a.ask_targets = approval;
            }
            None => {
                a.asking = None;
                a.ask_targets = None;
            }
        }
        a.how = (None, String::new());
    }
    if let Some((req, to)) = decision {
        let out = a.audit.clone();
        // Without the app's channel (tests), the row comes back here to be recorded.
        let (tx, rx) = mpsc::channel::<AuditEntry>();
        let local = out.is_none();
        let audit = out.unwrap_or(AuditOut(tx));
        a.err = None;
        a.msg = None;
        let root = env.root.to_path_buf();
        let device = env.device_id.to_string();
        let catalog = a.catalog.clone();
        let b2 = a.b2.clone();
        a.req_job = Some(spawn(ui.ctx(), move || {
            let p = core::client(&s);
            let done = match &to {
                Some(ap) => approve_and_share(
                    &p,
                    &req,
                    ap,
                    &root,
                    &device,
                    catalog.as_deref(),
                    &b2,
                    &audit,
                ),
                None => p
                    .deny(&req)
                    .map(|()| {
                        audit.send(AuditEntry::new(
                            "portal_deny",
                            None,
                            serde_json::json!({ "request": req }),
                        ));
                        String::new()
                    })
                    .map_err(|e| e.message()),
            };
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

/// Approves a request (on a thread): the clients on the portal first, then the approval with a
/// role at each; then the confirmed contact kept in the clients' Portal contacts and the chosen
/// documents shared with the person. Answers what to say; an error once approved says so.
#[allow(clippy::too_many_arguments)]
fn approve_and_share(
    p: &vf_portal::api::Portal,
    req: &str,
    ap: &Approval,
    root: &Path,
    device: &str,
    catalog: Option<&Path>,
    b2: &B2First,
    audit: &AuditOut,
) -> Result<String, String> {
    for (c, _) in &ap.clients {
        p.put_client(&c.id, &c.code, &c.name, None)
            .map_err(|e| e.message())?;
    }
    let pairs: Vec<(String, String)> = ap
        .clients
        .iter()
        .map(|(c, r)| (c.id.clone(), r.clone()))
        .collect();
    let identity = p
        .approve_for(req, &pairs, ap.check.as_ref())
        .map_err(|e| e.message())?;
    let roles: Vec<serde_json::Value> = pairs
        .iter()
        .map(|(id, role)| serde_json::json!({ "client_id": id, "role": role }))
        .collect();
    audit.send(AuditEntry::new(
        "portal_approve",
        None,
        serde_json::json!({
            "request": req,
            "client_id": pairs[0].0,
            "role": pairs[0].1,
            "clients": roles,
            // How it was checked; never the record's own values.
            "check": ap.check.as_ref().map(|k| serde_json::json!({
                "method": k.method,
                "matched": k.matched,
                "note": k.note,
            })),
        }),
    ));
    let mut said = if pairs.len() == 1 {
        "Approved.".to_string()
    } else {
        format!("Approved for {} clients.", pairs.len())
    };
    let mut saved = 0;
    for (client, key, value) in &ap.save {
        match vf_fs::contacts::set_one_by_hand(root, client, key, value) {
            Ok(()) => {
                saved += 1;
                audit.send(AuditEntry::new(
                    "client_contacts_edit",
                    None,
                    serde_json::json!({ "client_id": client, "fields": [key], "from": "access_request" }),
                ));
            }
            Err(e) => said.push_str(&format!(" The contact was not kept: {e}.")),
        }
    }
    if saved > 0 {
        said.push_str(" Their contact is kept in the client's Portal contacts.");
    }
    if ap.share.is_empty() {
        return Ok(said);
    }
    // The person's identity, as the approval answered it (a portal older than 0.3 does not).
    if identity.is_empty() {
        return Err(format!(
            "{said} Not shared: this portal is too old to say who was approved; share from Browse."
        ));
    }
    let Some(catalog) = catalog else {
        return Err(format!("{said} Not shared: the catalog is not open here."));
    };
    let mut conn = vf_core::db::open(catalog, vf_core::db::Storage::Local)
        .map_err(|e| format!("{said} Not shared: {e}"))?;
    let chosen = BTreeMap::from([(identity, true)]);
    let mut n = 0;
    for (c, _) in &ap.clients {
        let mine: Vec<&ShareDoc> = ap
            .share
            .iter()
            .filter(|(x, _)| x.id == c.id)
            .map(|(_, d)| d)
            .collect();
        if mine.is_empty() {
            continue;
        }
        let files = p
            .files_for_client(&c.id)
            .map_err(|e| format!("{said} Not shared: {}", e.message()))?;
        let mut work = Vec::new();
        for d in mine {
            let hash = vf_fs::fingerprint::ensure(&mut conn, root, device, &d.doc_id)
                .map_err(|e| format!("{said} Not shared: {e}"))?;
            work.push((
                DocRef {
                    id: d.doc_id.clone(),
                    name: widgets::file_name(&d.rel_path).to_string(),
                    path: root.join(&d.rel_path),
                    hash,
                    tax_year: d.tax_year,
                },
                files.iter().find(|f| f.vf_document_id == d.doc_id).cloned(),
            ));
        }
        n += work.len();
        share_docs(p, b2, c, work, &chosen, &BTreeMap::new(), false, audit)
            .map_err(|e| format!("{said} Not shared: {e}"))?;
    }
    said.push_str(&format!(
        " {n} document{} shared with them.",
        if n == 1 { "" } else { "s" }
    ));
    Ok(said)
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
    /// Whether the document goes to B2 before the link is sent (Q-103).
    b2: B2First,
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
        b2: B2First,
    ) -> Self {
        let mut w = Self {
            b2,
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
        let b2 = self.b2.clone();
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
            // To B2 first (Q-103), before anything reaches the portal.
            b2.push(std::slice::from_ref(&d.id))?;
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

    /// Vibe Portal Addendum 01, layer B: a request that matches the client record approves in
    /// one step and says what matched; one that does not is asked about only when the portal
    /// requires a check.
    #[test]
    fn approve_sends_the_match_or_asks_how_it_was_checked() {
        use vf_portal::record::{Comparison, Line};
        let cmp = |email, dob, name_differs| Comparison {
            email,
            mobile: Line::NotOnRequest,
            dob,
            name_differs,
        };
        let matched = cmp(Line::Matches, Line::Matches, false);
        assert_eq!(
            approve_step(Some(&matched), true),
            ApproveStep::Matched(Check::record_match(&["email", "dob"]))
        );
        // Said even when the portal asks for nothing: it is kept with the approval.
        assert!(matches!(
            approve_step(Some(&matched), false),
            ApproveStep::Matched(_)
        ));
        // The date of birth alone does not count, flagged or not (Q-110).
        let flagged = cmp(Line::Differs, Line::Matches, true);
        assert_eq!(approve_step(Some(&flagged), true), ApproveStep::Ask);
        assert_eq!(approve_step(Some(&flagged), false), ApproveStep::Plain);
        let alone = cmp(Line::Differs, Line::Matches, false);
        assert_eq!(approve_step(Some(&alone), true), ApproveStep::Ask);
        // With the email, a flagged name asks how it was checked, whatever the portal asks for
        // (Q-111).
        let named = cmp(Line::Matches, Line::Matches, true);
        assert_eq!(approve_step(Some(&named), true), ApproveStep::Ask);
        assert_eq!(approve_step(Some(&named), false), ApproveStep::Ask);
        // The note is cut where the portal would refuse it: an emoji counts as two.
        let mut note = format!("{}\u{1F600}", "a".repeat(299));
        limit_utf16(&mut note, NOTE_MAX);
        assert_eq!(note, "a".repeat(299));
        let mut note = "a".repeat(300);
        limit_utf16(&mut note, NOTE_MAX);
        assert_eq!(note.len(), 300);
        // No columns chosen in Admin › Portal: nothing to compare.
        assert_eq!(approve_step(None, true), ApproveStep::Ask);
        assert_eq!(approve_step(None, false), ApproveStep::Plain);
        // The words of the portal's flags; one it adds later is shown as it is.
        assert_eq!(flag_words("expired"), "The ID has expired");
        assert_eq!(flag_words("something_new"), "something new");
    }

    struct RequestsPage {
        a: AdminPortal,
        conn: vf_core::db::Connection,
        root: tempfile::TempDir,
    }

    /// Admin › Portal requests against a portal on this PC, with one client ("Sample, Alex",
    /// ID 1001) picked for request r1 and `row` as that client's row of the client list.
    fn requests_page(
        answer: fn(&str, &str) -> (u16, String),
        row: &[(&str, &str)],
    ) -> (
        egui_kittest::Harness<'static, RequestsPage>,
        std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        Receiver<AuditEntry>,
    ) {
        let (url, seen) = fake_portal(answer);
        let (tx, audit) = mpsc::channel();
        let mut a = AdminPortal {
            loaded: true,
            read_at: Some(Instant::now()),
            record_read: true,
            audit: Some(AuditOut(tx)),
            cached_settings: Some(PortalSettings {
                version: 1,
                cabinet_id: "c".into(),
                url,
                key: "vpk_test".into(),
                fetch_minutes: 0,
                record: RecordColumns {
                    email: Some("Email".into()),
                    mobile: Some("Cell".into()),
                    dob: Some("DOB".into()),
                    dates: DateOrder::Auto,
                },
                b2_list: false,
                todo_lists: false,
                balances: false,
                approve_share: Default::default(),
            }),
            clients: vec![ClientRef {
                id: "0199aaaa-0000-7000-8000-000000000001".into(),
                code: "1001".into(),
                name: "Sample, Alex".into(),
            }],
            ..AdminPortal::default()
        };
        a.rows.insert(
            "1001",
            row.iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        );
        a.pick.insert("r1".into(), (Some(0), String::new()));
        let st = RequestsPage {
            a,
            conn: vf_core::db::Connection::open_in_memory().unwrap(),
            root: tempfile::tempdir().unwrap(),
        };
        let h = egui_kittest::Harness::builder()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_size([900.0, 800.0])
            .build_ui_state(
                |ui, st: &mut RequestsPage| {
                    let mut env = Env {
                        conn: &mut st.conn,
                        root: st.root.path(),
                        device_id: "dev-1",
                    };
                    admin_requests(ui, &mut st.a, &mut env);
                },
                st,
            );
        (h, seen, audit)
    }

    const APPROVE: &str = "POST /api/firm/v1/access-requests/r1/approve";

    /// Steps until the approval of r1 was sent; its body.
    fn approval(
        h: &mut egui_kittest::Harness<'static, RequestsPage>,
        seen: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ) -> serde_json::Value {
        let start = Instant::now();
        while requests(seen, APPROVE).is_empty() {
            assert!(
                start.elapsed() < crate::ui::test_util::PATIENCE,
                "never approved"
            );
            std::thread::sleep(Duration::from_millis(20));
            h.step();
        }
        let r = requests(seen, APPROVE);
        serde_json::from_str(r[0].splitn(3, ' ').nth(2).unwrap()).unwrap()
    }

    /// The request screen, clicked through: a request confirmed with the client's email
    /// approves in one step and says what matched.
    #[test]
    fn a_request_that_matches_the_record_approves_in_one_step() {
        let (mut h, seen, audit) = requests_page(
            |method, path| match (method, path) {
                ("GET", "/api/firm/v1/access-requests") => (
                    200,
                    r#"{"requestCheck":"required","requests":[{"id":"r1","name":"Alex Sample",
                        "email":"alex@example.test","phone":null,
                        "confirmedContact":"alex@example.test","clientAs":"Sample","note":"",
                        "createdAt":"2026-11-14T10:00:00Z"}]}"#
                        .into(),
                ),
                _ => (200, r#"{"ok":true}"#.into()),
            },
            &[("Email", "ALEX@example.test")],
        );
        wait_for(&mut h, "This request matches the client record.");
        h.get_by_label("Approve").click();
        let body = approval(&mut h, &seen);
        assert_eq!(
            body["check"],
            serde_json::json!({ "method": "record_match", "matched": ["email"], "note": "" })
        );
        let start = Instant::now();
        let row = loop {
            if let Ok(e) = audit.try_recv() {
                break e;
            }
            assert!(
                start.elapsed() < crate::ui::test_util::PATIENCE,
                "no audit row"
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(row.action, "portal_approve");
    }

    /// Q-111: a request confirmed with the client's email whose ID's name the portal flagged
    /// is asked about, even on a portal that asks for no check; and a client who is not in
    /// the client list is said to be missing.
    #[test]
    fn a_match_with_the_ids_name_flagged_asks_how_it_was_checked() {
        let (mut h, seen, _audit) = requests_page(
            |method, path| match (method, path) {
                ("GET", "/api/firm/v1/access-requests") => (
                    200,
                    r#"{"requestCheck":"off","requests":[{"id":"r1","name":"Alex Sample",
                        "email":"alex@example.test","phone":null,
                        "confirmedContact":"alex@example.test","clientAs":"Sample","note":"",
                        "createdAt":"2026-11-14T10:00:00Z",
                        "idScan":{"familyName":"OTHER","firstName":"PAT","dob":"1990-01-06"},
                        "idFlags":["name_differs"]}]}"#
                        .into(),
                ),
                _ => (200, r#"{"ok":true}"#.into()),
            },
            &[("Email", "alex@example.test"), ("DOB", "1/6/90")],
        );
        wait_for(&mut h, "This request matches the client record.");
        wait_for(&mut h, "so Approve asks how you checked");
        // A date the list writes with a two-digit year is not said to differ.
        wait_for(&mut h, "the record's date could not be read");
        h.get_by_label("Approve").click();
        wait_for(&mut h, "How did you check Alex Sample?");
        assert!(requests(&seen, APPROVE).is_empty(), "nothing sent yet");
        h.get_by_label("Met in person").click();
        h.run();
        h.get_by_label("Approve with this check").click();
        let body = approval(&mut h, &seen);
        assert_eq!(body["check"]["method"], "in_person");

        // A client the client list does not have.
        let (mut h, _seen, _audit) = requests_page(
            |method, path| match (method, path) {
                ("GET", "/api/firm/v1/access-requests") => (
                    200,
                    r#"{"requests":[{"id":"r1","name":"Alex Sample","email":null,"phone":null,
                        "confirmedContact":"alex@example.test","clientAs":"Sample","note":"",
                        "createdAt":"2026-11-14T10:00:00Z"}]}"#
                        .into(),
                ),
                _ => (200, r#"{"ok":true}"#.into()),
            },
            &[],
        );
        h.state_mut().a.rows.clear();
        wait_for(&mut h, "is not in the client list");
    }

    /// Review 2026-11-15: Save writes only when the cabinet's portal settings are still the
    /// ones the page was filled from; a change saved on another PC is shown, not written over.
    #[test]
    fn a_save_does_not_write_over_another_pcs_change() {
        const KEY: &str = "vpk_abcdefghijklmnopqrstuvwxyz0123456789";
        let root = tempfile::tempdir().unwrap();
        vf_fs::cabinet::create(
            root.path(),
            &vf_fs::cabinet::CabinetFile {
                cabinet_id: "cab-1".into(),
                firm_id: "firm-1".into(),
                schema_version: 1,
                layout_version: 1,
                created: "2026-11-15T00:00:00Z".into(),
                name: None,
            },
        )
        .unwrap();
        let mut conn = vf_core::db::Connection::open_in_memory().unwrap();
        let mut env = Env {
            conn: &mut conn,
            root: root.path(),
            device_id: "dev-1",
        };
        // Two PCs open the page before anything is saved, and type the address and key.
        let page = || {
            let mut a = AdminPortal::default();
            a.load(root.path());
            a.url = "https://files.example.test".into();
            a.key = KEY.into();
            a
        };
        let (mut one, mut two) = (page(), page());
        one.columns.dob = Some("DOB".into());
        save(&mut one, &egui::Context::default(), &mut env);
        assert_eq!(one.err, None);
        // The second PC's Save is refused, and its page is filled again from the cabinet.
        two.approve_share.years = 2;
        save(&mut two, &egui::Context::default(), &mut env);
        assert_eq!(two.err.as_deref(), Some(STALE));
        let kept = settings::load(root.path(), "cab-1").unwrap();
        assert_eq!(kept.record.dob.as_deref(), Some("DOB"));
        assert_eq!(kept.approve_share.years, 1);
        two.load(root.path());
        assert_eq!(two.columns.dob.as_deref(), Some("DOB"));
        two.approve_share.years = 2;
        save(&mut two, &egui::Context::default(), &mut env);
        assert_eq!(two.err, None);
        // The first PC saves again without reloading: its own last save is no longer there.
        save(&mut one, &egui::Context::default(), &mut env);
        assert_eq!(one.err.as_deref(), Some(STALE));
        let kept = settings::load(root.path(), "cab-1").unwrap();
        assert_eq!(kept.approve_share.years, 2);
    }

    const U1: &str = "0199aaaa-0000-7000-8000-0000000000a1";
    const U2: &str = "0199aaaa-0000-7000-8000-0000000000a2";

    /// One client of the directory: its people (name, email, role) as the Portal contacts give
    /// them, and its names.
    fn on_file(
        id: &str,
        code: &str,
        name: &str,
        people: &[(&str, &str, &str)],
    ) -> vf_fs::contacts::ClientContacts {
        vf_fs::contacts::ClientContacts {
            client_id: id.into(),
            code: code.into(),
            name: name.into(),
            partner: None,
            planned: vf_fs::contacts::Planned {
                people: people
                    .iter()
                    .map(|(n, e, r)| vf_fs::contacts::Invitee {
                        name: (*n).into(),
                        email: Some((*e).into()),
                        phone: None,
                        role: (*r).into(),
                        note: None,
                    })
                    .collect(),
                problems: vec![],
            },
            names: people
                .iter()
                .map(|(n, _, r)| ((*n).into(), (*r).into(), *r == "Spouse"))
                .collect(),
        }
    }

    /// Admin › Portal requests with Portal contacts (PA-106): no client picked, the settings
    /// naming only a date of birth column, `dir` as every client's contacts, and the catalog
    /// at `catalog` (for the documents Approve shares).
    fn match_page(
        answer: fn(&str, &str) -> (u16, String),
        dir: Vec<vf_fs::contacts::ClientContacts>,
        rows: &[(&str, &[(&str, &str)])],
        share: settings::ApproveShare,
        root: tempfile::TempDir,
        catalog: Option<std::path::PathBuf>,
    ) -> (
        egui_kittest::Harness<'static, RequestsPage>,
        std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        Receiver<AuditEntry>,
    ) {
        let (url, seen) = fake_portal(answer);
        let (tx, audit) = mpsc::channel();
        let mut a = AdminPortal {
            loaded: true,
            read_at: Some(Instant::now()),
            record_read: true,
            audit: Some(AuditOut(tx)),
            cached_settings: Some(PortalSettings {
                version: 1,
                cabinet_id: "c".into(),
                url,
                key: "vpk_test".into(),
                fetch_minutes: 0,
                record: RecordColumns {
                    email: None,
                    mobile: None,
                    dob: Some("DOB".into()),
                    dates: DateOrder::Auto,
                },
                b2_list: false,
                todo_lists: false,
                balances: false,
                approve_share: share,
            }),
            clients: dir
                .iter()
                .map(|c| ClientRef {
                    id: c.client_id.clone(),
                    code: c.code.clone(),
                    name: c.name.clone(),
                })
                .collect(),
            dir,
            catalog: catalog.clone(),
            ..AdminPortal::default()
        };
        for (code, row) in rows {
            a.rows.insert(
                code,
                row.iter()
                    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                    .collect(),
            );
        }
        let conn = match &catalog {
            Some(c) => vf_core::db::open(c, vf_core::db::Storage::Local).unwrap(),
            None => vf_core::db::Connection::open_in_memory().unwrap(),
        };
        let st = RequestsPage { a, conn, root };
        let h = egui_kittest::Harness::builder()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_size([900.0, 900.0])
            .build_ui_state(
                |ui, st: &mut RequestsPage| {
                    let mut env = Env {
                        conn: &mut st.conn,
                        root: st.root.path(),
                        device_id: "dev-1",
                    };
                    admin_requests(ui, &mut st.a, &mut env);
                },
                st,
            );
        (h, seen, audit)
    }

    /// Jane Sample asks with jane@example.test (confirmed), and an ID read with `dob`.
    fn jane_asks(dob: &str) -> String {
        format!(
            r#"{{"requestCheck":"required","requests":[{{"id":"r1","name":"Jane Sample",
                "email":"jane@example.test","phone":null,
                "confirmedContact":"jane@example.test","clientAs":"Sample","note":"",
                "createdAt":"2026-11-24T10:00:00Z",
                "idScan":{{"familyName":"SAMPLE","firstName":"JANE","dob":"{dob}"}},
                "idFlags":[]}}]}}"#
        )
    }

    /// PA-106 (owner 2026-11-24): the confirmed email is on file at two clients; both are
    /// offered ticked with her role at each, and one click approves both.
    #[test]
    fn a_contact_on_file_at_two_clients_approves_both_in_one_click() {
        let (mut h, seen, _audit) = match_page(
            |method, path| match (method, path) {
                ("GET", "/api/firm/v1/access-requests") => (200, jane_asks("1990-01-06")),
                ("POST", p) if p.ends_with("/approve") => {
                    (200, r#"{"ok":true,"identityId":"p1"}"#.into())
                }
                _ => (200, r#"{"ok":true}"#.into()),
            },
            vec![
                on_file(
                    U1,
                    "1042",
                    "Sample, John & Jane",
                    &[
                        ("John Sample", "john@example.test", "Taxpayer"),
                        ("Jane Sample", "jane@example.test", "Spouse"),
                    ],
                ),
                on_file(
                    U2,
                    "1077",
                    "Sample Farms LLC",
                    &[("Jane Sample", "jane@example.test", "Owner")],
                ),
            ],
            &[("1042", &[("DOB", "1/6/1990")])],
            Default::default(),
            tempfile::tempdir().unwrap(),
            None,
        );
        wait_for(&mut h, "The email they confirmed is on file for:");
        assert!(
            h.query_by_label_contains(
                "Sample, John & Jane (1042) as Spouse: Jane Sample's email on file"
            )
            .is_some()
        );
        assert!(
            h.query_by_label_contains("Sample Farms LLC (1077) as Owner")
                .is_some()
        );
        assert!(
            h.query_by_label_contains("date of birth on the ID matches the record")
                .is_some()
        );
        h.get_by_label("Approve for 2 clients").click();
        let body = approval(&mut h, &seen);
        assert_eq!(body["clientIds"], serde_json::json!([U1, U2]));
        assert_eq!(body["roles"][U1], "Spouse");
        assert_eq!(body["roles"][U2], "Owner");
        // The date of birth is said only when it is every ticked client's.
        assert_eq!(
            body["check"],
            serde_json::json!({ "method": "record_match", "matched": ["email"], "note": "" })
        );
        wait_for(&mut h, "Approved for 2 clients.");
    }

    /// PA-106: a date of birth on the ID that differs from the record asks how staff checked.
    #[test]
    fn a_contact_match_whose_date_of_birth_differs_asks() {
        let (mut h, seen, _audit) = match_page(
            |method, path| match (method, path) {
                ("GET", "/api/firm/v1/access-requests") => (200, jane_asks("1985-03-02")),
                ("POST", p) if p.ends_with("/approve") => {
                    (200, r#"{"ok":true,"identityId":"p1"}"#.into())
                }
                _ => (200, r#"{"ok":true}"#.into()),
            },
            vec![on_file(
                U1,
                "1042",
                "Sample, John & Jane",
                &[("Jane Sample", "jane@example.test", "Spouse")],
            )],
            &[("1042", &[("DOB", "1/6/1990")])],
            Default::default(),
            tempfile::tempdir().unwrap(),
            None,
        );
        wait_for(
            &mut h,
            "A date of birth differs, so Approve asks how you checked.",
        );
        h.get_by_label("Approve as Spouse…").click();
        wait_for(&mut h, "How did you check Jane Sample?");
        assert!(requests(&seen, APPROVE).is_empty(), "nothing sent yet");
        h.get_by_label("Met in person").click();
        h.run();
        h.get_by_label("Approve with this check").click();
        let body = approval(&mut h, &seen);
        assert_eq!(body["check"]["method"], "in_person");
        assert_eq!(body["roles"][U1], "Spouse");
    }

    /// PA-106: no contact on file, but the name is: a lead, approved only after saying how
    /// staff checked; the confirmed email is then kept in the client's Portal contacts as the
    /// spouse's.
    #[test]
    fn a_name_only_lead_asks_and_keeps_the_confirmed_contact() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("_Vibe")).unwrap();
        let (mut h, seen, audit) = match_page(
            |method, path| match (method, path) {
                ("GET", "/api/firm/v1/access-requests") => (200, jane_asks("1990-01-06")),
                ("POST", p) if p.ends_with("/approve") => {
                    (200, r#"{"ok":true,"identityId":"p1"}"#.into())
                }
                _ => (200, r#"{"ok":true}"#.into()),
            },
            {
                let mut c = on_file(
                    U1,
                    "1042",
                    "Sample, John & Jane",
                    &[("John Sample", "john@example.test", "Taxpayer")],
                );
                c.names.push(("Jane Sample".into(), "Spouse".into(), true));
                vec![c]
            },
            &[],
            Default::default(),
            root,
            None,
        );
        wait_for(
            &mut h,
            "Not a match: the email they confirmed is not on file.",
        );
        assert!(
            h.query_by_label_contains("Keep jane@example.test as their email")
                .is_some()
        );
        h.get_by_label("Approve as Spouse…").click();
        wait_for(&mut h, "How did you check Jane Sample?");
        h.get_by_label("Called the number in our records").click();
        h.run();
        crate::ui::test_util::type_into(&mut h, "Who you spoke to", "John Sample");
        h.get_by_label("Approve with this check").click();
        let body = approval(&mut h, &seen);
        assert_eq!(body["check"]["method"], "callback");
        wait_for(
            &mut h,
            "Their contact is kept in the client's Portal contacts.",
        );
        let root = h.state().root.path().to_path_buf();
        let rec = vf_fs::contacts::record(&root, U1).unwrap();
        assert_eq!(
            rec.hand.get("spouse_email").map(String::as_str),
            Some("jane@example.test")
        );
        let rows: Vec<AuditEntry> = audit.try_iter().collect();
        assert!(rows.iter().any(|r| r.action == "client_contacts_edit"
            && !r.detail.to_string().contains("example.test")));
    }

    /// PA-106: the documents of the doc types Admin › Portal names are offered on Approve, the
    /// latest year ticked and older ones not; the ticked ones are shared with the person.
    #[test]
    fn approve_shares_the_chosen_documents_with_the_person() {
        let root = tempfile::tempdir().unwrap();
        let catalog = root.path().join("catalog.db");
        {
            let conn = vf_core::db::open(&catalog, vf_core::db::Storage::Local).unwrap();
            conn.execute_batch(&format!(
                "INSERT INTO doc_type (id, name, device_id, updated_at) VALUES ('ret', 'Tax return', 'd', 't');
                 INSERT INTO client (id, ext_id, name, folder_rel, device_id, updated_at)
                   VALUES ('{U1}', '1042', 'Sample, John & Jane', 'Sample (1042)', 'd', 't');
                 INSERT INTO document (id, client_id, doc_type_id, tax_year, rel_path, size, filed_at, device_id, updated_at)
                   VALUES ('d25', '{U1}', 'ret', 2025, 'Sample (1042)/2025 return.pdf', 4, 't', 'd', 't'),
                          ('d24', '{U1}', 'ret', 2024, 'Sample (1042)/2024 return.pdf', 4, 't', 'd', 't');"
            ))
            .unwrap();
        }
        std::fs::create_dir_all(root.path().join("Sample (1042)")).unwrap();
        for y in ["2025", "2024"] {
            std::fs::write(
                root.path().join(format!("Sample (1042)/{y} return.pdf")),
                b"%PDF",
            )
            .unwrap();
        }
        let (mut h, seen, _audit) = match_page(
            |method, path| match (method, path) {
                ("GET", "/api/firm/v1/access-requests") => (200, jane_asks("1990-01-06")),
                ("POST", p) if p.ends_with("/approve") => {
                    (200, r#"{"ok":true,"identityId":"p1"}"#.into())
                }
                ("GET", p) if p.starts_with("/api/firm/v1/files") => {
                    (200, r#"{"files":[]}"#.into())
                }
                ("POST", p) if p.starts_with("/api/firm/v1/files") => {
                    (201, r#"{"fileId":"f25"}"#.into())
                }
                _ => (200, r#"{"ok":true}"#.into()),
            },
            vec![on_file(
                U1,
                "1042",
                "Sample, John & Jane",
                &[("Jane Sample", "jane@example.test", "Spouse")],
            )],
            &[],
            settings::ApproveShare {
                doc_types: vec!["ret".into()],
                years: 1,
            },
            root,
            Some(catalog),
        );
        wait_for(&mut h, "Also share with them:");
        assert!(h.query_by_label("2024 2024 return.pdf   (older)").is_some());
        h.get_by_label("Approve as Spouse").click();
        approval(&mut h, &seen);
        wait_for(&mut h, "1 document shared with them.");
        let log = seen.lock().unwrap().clone();
        let adds: Vec<_> = log
            .iter()
            .filter(|r| r.starts_with("POST /api/firm/v1/files"))
            .collect();
        assert_eq!(adds.len(), 1, "{log:?}");
        assert!(adds[0].contains("vfDocumentId=d25"), "{adds:?}");
        assert!(
            log.iter()
                .any(|r| r.starts_with("PUT /api/firm/v1/files/f25/visibility")
                    && r.contains("\"p1\": true")),
            "{log:?}"
        );
    }

    /// PA-106 (review 2026-11-24): a cabinet whose settings still name the old email column
    /// has it moved to Portal contacts when the page opens; the request is then found by the
    /// contact and approved in one click; the settings keep the date of birth column only.
    /// Under Other client…, keeping the contact is unticked and saves nothing.
    #[test]
    fn the_old_email_column_moves_over_and_finds_the_request() {
        let root = tempfile::tempdir().unwrap();
        vf_fs::cabinet::create(
            root.path(),
            &vf_fs::cabinet::CabinetFile {
                cabinet_id: "cab-1".into(),
                firm_id: "firm-1".into(),
                schema_version: 1,
                layout_version: 1,
                created: "2026-11-24T00:00:00Z".into(),
                name: None,
            },
        )
        .unwrap();
        let csv = root.path().join("clients.csv");
        std::fs::write(
            &csv,
            "ID,Name,E Mail\n1001,\"Sample, Alex\",alex@example.test\n1002,\"Other, Pat\",\n",
        )
        .unwrap();
        let map = filer_core::config::ColumnMap {
            id: Some("ID".into()),
            name: Some("Name".into()),
            partner: None,
            extras: vec!["E Mail".into()],
        };
        let list =
            vf_fs::clients::import_csv_tin(root.path(), &csv, &map, &Default::default(), None)
                .unwrap();
        vf_fs::clients::use_csv_in_intake(root.path(), &csv, &map).unwrap();
        let alex = list.match_by_id("1001").unwrap().uuid.to_string();
        let pat = list.match_by_id("1002").unwrap().uuid.to_string();
        let catalog = root.path().join("catalog.db");
        let conn = vf_core::db::open(&catalog, vf_core::db::Storage::Local).unwrap();
        conn.execute_batch(&format!(
            "INSERT INTO client (id, ext_id, name, device_id, updated_at)
               VALUES ('{alex}', '1001', 'Sample, Alex', 'd', 't'), ('{pat}', '1002', 'Other, Pat', 'd', 't');"
        ))
        .unwrap();
        let (url, seen) = fake_portal(|method, path| match (method, path) {
            ("GET", "/api/firm/v1/access-requests") => (
                200,
                r#"{"requestCheck":"off","requests":[{"id":"r1","name":"Alex Sample",
                    "email":"alex@example.test","phone":null,
                    "confirmedContact":"alex@example.test","clientAs":"Sample","note":"",
                    "createdAt":"2026-11-24T10:00:00Z"}]}"#
                    .into(),
            ),
            ("POST", p) if p.ends_with("/approve") => {
                (200, r#"{"ok":true,"identityId":"p1"}"#.into())
            }
            _ => (200, r#"{"ok":true}"#.into()),
        });
        let mut s =
            PortalSettings::new("cab-1", &url, "vpk_abcdefghijklmnopqrstuvwxyz0123456789", 0)
                .unwrap();
        s.record.email = Some("E_Mail".into());
        settings::save(root.path(), &s).unwrap();
        let st = RequestsPage {
            a: AdminPortal {
                audit: Some(AuditOut(mpsc::channel().0)),
                ..AdminPortal::default()
            },
            conn,
            root,
        };
        let mut h = egui_kittest::Harness::builder()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_size([900.0, 900.0])
            .build_ui_state(
                |ui, st: &mut RequestsPage| {
                    let mut env = Env {
                        conn: &mut st.conn,
                        root: st.root.path(),
                        device_id: "dev-1",
                    };
                    admin_requests(ui, &mut st.a, &mut env);
                },
                st,
            );
        wait_for(&mut h, "were moved over (1 clients)");
        let root = h.state().root.path().to_path_buf();
        let kept = settings::load(&root, "cab-1").unwrap();
        assert_eq!(kept.record.email, None);
        assert_eq!(
            vf_fs::contacts::columns(&root).unwrap().email.as_deref(),
            Some("E Mail")
        );
        wait_for(
            &mut h,
            "Sample, Alex (1001) as Taxpayer: Alex Sample's email on file",
        );
        // Other client…: picking Pat, the box to keep the contact is there, unticked.
        h.get_by_label("Other client…").click();
        h.run();
        h.get_by_label("Client").click();
        h.run();
        h.get_by_label("Other, Pat (1002)").click();
        h.run();
        wait_for(
            &mut h,
            "Also keep alex@example.test as the taxpayer's email in Other, Pat's Portal contacts",
        );
        h.get_by_label("Approve").click();
        approval(&mut h, &seen);
        wait_for(&mut h, "Approved.");
        assert!(
            vf_fs::contacts::record(&root, &pat).unwrap().is_empty(),
            "nothing kept unless ticked"
        );
    }

    /// Q-110: a date of birth that is the client's, with a contact that is not, is not a
    /// match. Approve asks how staff checked, and sends that.
    #[test]
    fn a_date_of_birth_alone_asks_how_it_was_checked() {
        let (mut h, seen, _audit) = requests_page(
            |method, path| match (method, path) {
                ("GET", "/api/firm/v1/access-requests") => (
                    200,
                    r#"{"requestCheck":"required","requests":[{"id":"r1","name":"Alex Sample",
                        "email":"other@example.test","phone":null,
                        "confirmedContact":"other@example.test","clientAs":"Sample","note":"",
                        "createdAt":"2026-11-14T10:00:00Z",
                        "idScan":{"familyName":"SAMPLE","firstName":"ALEX","dob":"1990-01-06"},
                        "idFlags":[]}]}"#
                        .into(),
                ),
                _ => (200, r#"{"ok":true}"#.into()),
            },
            &[("Email", "alex@example.test"), ("DOB", "1/6/1990")],
        );
        wait_for(&mut h, "This request does not match the client record.");
        wait_for(&mut h, "a date of birth alone is not a match");
        h.get_by_label("Approve").click();
        wait_for(&mut h, "How did you check Alex Sample?");
        assert!(requests(&seen, APPROVE).is_empty(), "nothing sent yet");
        h.get_by_label("Met in person").click();
        h.run();
        h.get_by_label("Approve with this check").click();
        let body = approval(&mut h, &seen);
        assert_eq!(
            body["check"],
            serde_json::json!({ "method": "in_person", "matched": [], "note": "" })
        );
    }

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
            request: None,
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
            invites: vec![],
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
                start.elapsed() < crate::ui::test_util::PATIENCE,
                "never showed {label}"
            );
            std::thread::sleep(Duration::from_millis(20));
            h.step();
        }
    }

    const SHARE_CLIENT: &str = "0199aaaa-0000-7000-8000-000000000002";

    /// A portal with one document on it (`f1`, held for nobody) and one invitation to Jane
    /// Sample, expired, not accepted.
    fn share_portal(method: &str, path: &str) -> (u16, String) {
        match (method, path) {
            ("GET", p) if p.contains("/users") => (
                200,
                r#"{"users":[],"invitations":[{"invitationId":"i1","name":"Jane Sample",
                  "email":"jane@example.test","phone":"+15552010002","roleLabel":"Spouse",
                  "expiresAt":"2026-11-01T00:00:00Z","status":"expired"}]}"#
                    .into(),
            ),
            ("GET", p) if p.starts_with("/api/firm/v1/files") => (
                200,
                r#"{"files":[{"fileId":"f1","clientId":"c","vfDocumentId":"d1","name":"W-2.pdf",
                  "size":1,"sha256":"h1","taxYear":2026,"sharedAt":"2026-11-20T00:00:00Z",
                  "sharedBy":"x","contentUpdatedAt":null,"users":[],"invites":[]}]}"#
                    .into(),
            ),
            ("POST", p) if p.ends_with("/invites") => (201, r#"{"invitationId":"i9"}"#.into()),
            _ => (200, r#"{"ok":true}"#.into()),
        }
    }

    fn share_window(url: String) -> egui_kittest::Harness<'static, ShareWindow> {
        let s = PortalSettings {
            version: 1,
            cabinet_id: "c".into(),
            url,
            key: "vpk_test".into(),
            fetch_minutes: 0,
            record: Default::default(),
            b2_list: false,
            todo_lists: false,
            balances: false,
            approve_share: Default::default(),
        };
        let (tx, _audit) = mpsc::channel();
        let w = ShareWindow::new(
            &egui::Context::default(),
            &s,
            ClientRef {
                id: SHARE_CLIENT.into(),
                code: "SAMP1001".into(),
                name: "Sample, John & Jane".into(),
            },
            vec![DocRef {
                id: "d1".into(),
                name: "W-2.pdf".into(),
                path: "W-2.pdf".into(),
                hash: "h1".into(),
                tax_year: Some(2026),
            }],
            AuditOut(tx),
            B2First::NotSetUp,
        );
        egui_kittest::Harness::builder()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_size([700.0, 700.0])
            .build_ui_state(move |ui, w: &mut ShareWindow| w.show(ui.ctx(), &s), w)
    }

    /// The requests after the window's first reads, as "METHOD path body".
    fn writes(seen: &std::sync::Arc<std::sync::Mutex<Vec<String>>>) -> Vec<String> {
        seen.lock()
            .unwrap()
            .iter()
            .filter(|r| !r.starts_with("GET ") && !r.starts_with("PUT /api/firm/v1/clients/"))
            // The bodies are pretty-printed: compared without their spaces and line breaks.
            .map(|r| {
                let (head, body) = r
                    .split_once(' ')
                    .map(|(m, rest)| {
                        let (path, body) = rest.split_once(' ').unwrap_or((rest, ""));
                        (format!("{m} {path}"), body.to_string())
                    })
                    .unwrap_or_default();
                let body: String = body.chars().filter(|c| !c.is_whitespace()).collect();
                format!("{head} {body}")
            })
            .collect()
    }

    /// Owner 2026-11-24: someone invited who has not accepted is given the document and sent
    /// the invitation again with one Save; the document is held first, so the new invitation
    /// takes it over (Vibe-Portal P98).
    #[test]
    fn share_with_someone_not_accepted_resends_in_one_step() {
        let (url, seen) = fake_portal(share_portal);
        let mut h = share_window(url);
        wait_for(&mut h, "Jane Sample (jane@example.test)");
        assert!(h.query_by_label_contains("invitation expired").is_some());
        h.get_by_label("Jane Sample (jane@example.test)").click();
        h.run();
        // An expired invitation can't be accepted: ticking them ticks Resend too.
        h.get_by_label("Save").click();
        wait_for(&mut h, "Invitation sent again to Jane Sample");
        let w = writes(&seen);
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(
            w[0].starts_with("PUT /api/firm/v1/files/f1/visibility ")
                && w[0].contains(r#""invites":{"i1":true}"#),
            "{w:?}"
        );
        assert!(
            w[1].starts_with(&format!(
                "POST /api/firm/v1/clients/{SHARE_CLIENT}/invites "
            )) && w[1].contains("jane@example.test")
                && w[1].contains("+15552010002")
                && w[1].contains(r#""roleLabel":"Spouse""#),
            "{w:?}"
        );
    }

    /// Owner 2026-11-24 (P113): the email of someone who already has a portal login is
    /// recognised in the Share window; one click adds them to the client at once and shows
    /// them the document, with no invitation.
    #[test]
    fn someone_with_a_login_is_added_at_once_from_the_share_window() {
        let (url, seen) = fake_portal(|method, path| match (method, path) {
            ("GET", p) if p.starts_with("/api/firm/v1/people") => (
                200,
                r#"{"people":[{"identityId":"pat-1","name":"Pat Sample","email":"pat@example.test",
                    "phone":null,"clients":[{"id":"other","name":"Sample Farms LLC"}]}]}"#
                    .into(),
            ),
            ("POST", p) if p.ends_with("/people") => (201, r#"{"identityId":"pat-1"}"#.into()),
            (m, p) => share_portal(m, p),
        });
        let mut h = share_window(url);
        wait_for(&mut h, "Jane Sample (jane@example.test)");
        h.get_by_label("Invite someone…").click();
        h.run();
        crate::ui::test_util::type_into(&mut h, "Email", "Pat@Example.test");
        wait_for(
            &mut h,
            "Pat Sample already has a portal login (sees: Sample Farms LLC)",
        );
        crate::ui::test_util::type_into(&mut h, "Role (e.g. Spouse)", "Owner");
        h.get_by_label("Add to this client and share").click();
        wait_for(&mut h, "Pat Sample was added to this client");
        let w = writes(&seen);
        assert!(
            w.iter().any(|r| r
                .starts_with(&format!("POST /api/firm/v1/clients/{SHARE_CLIENT}/people "))
                && r.contains(r#""identityId":"pat-1""#)
                && r.contains(r#""roleLabel":"Owner""#)),
            "{w:?}"
        );
        assert!(
            w.iter()
                .any(|r| r.starts_with("PUT /api/firm/v1/files/f1/visibility")
                    && r.contains(r#""users":{"pat-1":true}"#)),
            "{w:?}"
        );
        assert!(
            !w.iter().any(|r| r.contains("/invites")),
            "no invitation: {w:?}"
        );
    }

    /// Owner 2026-11-24: an older portal is said once in Admin › Portal.
    #[test]
    fn an_older_portal_is_said() {
        let st = |features: &[&str]| vf_portal::api::Status {
            version: "0.5.1".into(),
            firm_name: "Sample CPA Firm".into(),
            portal_url: "https://p.example.test".into(),
            storage: None,
            storage_moving: false,
            features: features.iter().map(|f| f.to_string()).collect(),
        };
        let w = old_portal_words(&st(&["requests"])).unwrap();
        assert!(w.starts_with("This portal is 0.5.1: update it to 0.5.2 or later"));
        assert_eq!(old_portal_words(&st(&["contacts"])), None);
    }

    /// PA-105 (owner 2026-11-24): invitations go out in groups with a pause between them,
    /// each client put on the portal once first; a refusal is kept and the rest go on.
    #[test]
    fn invitations_from_the_client_list_go_in_groups_with_a_pause() {
        use crate::ui::invite_list::{Item, Progress, send_all, summary};
        let (url, seen) = fake_portal(|method, path| match (method, path) {
            // Read just before sending: f was invited by another PC meanwhile.
            ("GET", "/api/firm/v1/contacts") => (
                200,
                r#"{"clients":[{"id":"c4","active":true,"people":[],"invitations":[
                    {"invitationId":"x","name":"f","email":"f@example.test","phone":null,
                     "roleLabel":"","status":"invited"}]}]}"#
                    .into(),
            ),
            ("POST", p) if p.contains("/clients/c2/") => {
                (400, r#"{"error":"invalid_email"}"#.into())
            }
            ("POST", _) => (201, r#"{"invitationId":"i"}"#.into()),
            _ => (200, r#"{"ok":true}"#.into()),
        });
        let s = PortalSettings {
            version: 1,
            cabinet_id: "c".into(),
            url,
            key: "vpk_test".into(),
            fetch_minutes: 0,
            record: Default::default(),
            b2_list: false,
            todo_lists: false,
            balances: false,
            approve_share: Default::default(),
        };
        let item = |client: &str, name: &str| Item {
            client_id: client.into(),
            code: "1001".into(),
            client: format!("Client {client}"),
            invitee: vf_fs::contacts::Invitee {
                name: name.into(),
                email: Some(format!("{name}@example.test")),
                phone: None,
                role: "Taxpayer".into(),
                note: None,
            },
        };
        let items = vec![
            item("c1", "a"),
            item("c1", "b"),
            item("c2", "c"),
            item("c3", "d"),
            item("c3", "e"),
            item("c4", "f"),
        ];
        let progress = std::sync::Mutex::new(Progress {
            total: items.len(),
            ..Default::default()
        });
        let stop = std::sync::atomic::AtomicBool::new(false);
        let (tx, audit) = mpsc::channel();
        let pause = Duration::from_millis(150);
        let t0 = Instant::now();
        send_all(
            &core::client(&s),
            &items,
            2,
            pause,
            &progress,
            &stop,
            &AuditOut(tx),
            &|| {},
        );
        // 5 invitations in groups of 2: two pauses, none after the last group.
        let took = t0.elapsed();
        assert!(took >= pause * 2, "{took:?}");
        assert!(took < pause * 3 + Duration::from_secs(2), "{took:?}");
        let log = seen.lock().unwrap().clone();
        let puts: Vec<_> = log.iter().filter(|r| r.starts_with("PUT ")).collect();
        assert_eq!(puts.len(), 3, "each client put once: {log:?}");
        let posts = log.iter().filter(|r| r.starts_with("POST ")).count();
        assert_eq!(posts, 5);
        let p = progress.lock().unwrap();
        assert!(p.finished);
        assert_eq!(p.sent.len(), 6);
        // f is said first, and not sent again.
        assert_eq!(p.sent[0].who, "f");
        assert_eq!(p.sent.iter().filter(|s| s.result.is_ok()).count(), 4);
        assert_eq!(
            p.sent[3].result,
            Err((
                "invalid_email".into(),
                "That email address does not look right.".into()
            ))
        );
        assert_eq!(
            summary(&p),
            "4 invited (4 by email, 0 by text) · 1 on the portal or invited already · 1 not sent"
        );
        let rows: Vec<_> = audit.try_iter().collect();
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].detail["from_client_list"], true);
        // No email or phone in the audit rows.
        assert!(!rows[0].detail.to_string().contains("example.test"));
    }

    /// PA-105: the Invite from client list page itself: the people ticked from the stored
    /// contacts are counted on the button (walkthrough 2026-11-24: it said 0) and invited.
    #[test]
    fn the_invite_page_counts_and_invites_the_ticked_people() {
        let root = tempfile::tempdir().unwrap();
        vf_fs::cabinet::create(
            root.path(),
            &vf_fs::cabinet::CabinetFile {
                cabinet_id: "cab-1".into(),
                firm_id: "firm-1".into(),
                schema_version: 1,
                layout_version: 1,
                created: "2026-11-24T00:00:00Z".into(),
                name: None,
            },
        )
        .unwrap();
        let csv = root.path().join("clients.csv");
        std::fs::write(
            &csv,
            "ID,Name,Email,Spouse Email\n1001,\"Sample, John & Jane\",john@example.test,jane@example.test\n",
        )
        .unwrap();
        let map = filer_core::config::ColumnMap {
            id: Some("ID".into()),
            name: Some("Name".into()),
            partner: None,
            extras: vec![],
        };
        let list =
            vf_fs::clients::import_csv_tin(root.path(), &csv, &map, &Default::default(), None)
                .unwrap();
        let cols = vf_fs::contacts::ContactColumns {
            email: Some("Email".into()),
            spouse_email: Some("Spouse Email".into()),
            ..Default::default()
        };
        let table = filer_core::clients::read_csv(&csv).unwrap();
        vf_fs::contacts::store_from_csv(
            root.path(),
            &list,
            &vf_fs::contacts::csv_rows(&table, &map, &cols).unwrap(),
        )
        .unwrap();
        let (url, seen) = fake_portal(|method, path| match (method, path) {
            ("GET", "/api/firm/v1/contacts") => (200, r#"{"clients":[]}"#.into()),
            ("POST", _) => (201, r#"{"invitationId":"i"}"#.into()),
            _ => (200, r#"{"ok":true}"#.into()),
        });
        settings::save(
            root.path(),
            &PortalSettings::new("cab-1", &url, "vpk_abcdefghijklmnopqrstuvwxyz0123456789", 0)
                .unwrap(),
        )
        .unwrap();
        struct St {
            page: crate::ui::invite_list::InvitePage,
            conn: vf_core::db::Connection,
            root: tempfile::TempDir,
        }
        let mut page = crate::ui::invite_list::InvitePage::default();
        page.second = Duration::from_millis(1);
        let st = St {
            page,
            conn: vf_core::db::Connection::open_in_memory().unwrap(),
            root,
        };
        let mut h = egui_kittest::Harness::builder()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_size([900.0, 800.0])
            .build_ui_state(
                |ui, st: &mut St| {
                    let mut env = Env {
                        conn: &mut st.conn,
                        root: st.root.path(),
                        device_id: "dev-1",
                    };
                    crate::ui::invite_list::page(ui, &mut st.page, &mut env);
                },
                st,
            );
        wait_for(&mut h, "Invite 2 people at 1 client");
        // What the first person ticked receives, as the portal sends it (owner 2026-11-24).
        h.get_by_label("What they receive (for John Sample)")
            .click();
        h.run();
        assert!(
            h.query_by_label_contains(
                "has invited you to its client portal for Sample, John & Jane"
            )
            .is_some()
        );
        // The search box (owner 2026-11-24) narrows the list, and Clear empties it.
        crate::ui::test_util::type_into(&mut h, "Search", "nobody");
        wait_for(&mut h, "Invite 0 people at 0 clients");
        assert!(h.query_by_label_contains("Sample, John & Jane").is_none());
        h.get_by_label("Clear").click();
        h.run();
        crate::ui::test_util::type_into(&mut h, "Search", "jane@");
        wait_for(&mut h, "Sample, John & Jane (1001)");
        h.get_by_label("Clear").click();
        h.run();
        h.get_by_label("Invite 2 people at 1 client").click();
        wait_for(&mut h, "2 invited (2 by email, 0 by text)");
        let posts: Vec<String> = seen
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.starts_with("POST "))
            .cloned()
            .collect();
        assert_eq!(posts.len(), 2, "{posts:?}");
        assert!(posts[1].contains("jane@example.test") && posts[1].contains("Spouse"));
    }

    /// Stop during the pause sends nothing more.
    #[test]
    fn stopping_between_groups_sends_no_more() {
        use crate::ui::invite_list::{Item, Progress, send_all};
        let (url, seen) = fake_portal(|method, _| match method {
            "POST" => (201, r#"{"invitationId":"i"}"#.into()),
            _ => (200, r#"{"ok":true}"#.into()),
        });
        let s = PortalSettings {
            version: 1,
            cabinet_id: "c".into(),
            url,
            key: "vpk_test".into(),
            fetch_minutes: 0,
            record: Default::default(),
            b2_list: false,
            todo_lists: false,
            balances: false,
            approve_share: Default::default(),
        };
        let items: Vec<Item> = (0..4)
            .map(|i| Item {
                client_id: format!("c{i}"),
                code: "1".into(),
                client: "x".into(),
                invitee: vf_fs::contacts::Invitee {
                    name: "n".into(),
                    email: Some(format!("p{i}@example.test")),
                    phone: None,
                    role: String::new(),
                    note: None,
                },
            })
            .collect();
        let progress = std::sync::Arc::new(std::sync::Mutex::new(Progress::default()));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (p2, s2) = (progress.clone(), stop.clone());
        let (tx, _audit) = mpsc::channel();
        let p = core::client(&s);
        let h = std::thread::spawn(move || {
            send_all(
                &p,
                &items,
                2,
                Duration::from_secs(30),
                &p2,
                &s2,
                &AuditOut(tx),
                &|| {},
            )
        });
        let start = Instant::now();
        while progress.lock().unwrap().next_group_at.is_none() {
            assert!(start.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(10));
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        h.join().unwrap();
        let p = progress.lock().unwrap();
        assert!(p.stopped && p.finished);
        assert_eq!(p.sent.len(), 2);
        assert_eq!(
            seen.lock()
                .unwrap()
                .iter()
                .filter(|r| r.starts_with("POST "))
                .count(),
            2
        );
    }

    /// Owner 2026-11-24: inviting someone from the Share window shares the documents with
    /// them in the same step.
    #[test]
    fn invite_from_share_window_shares_too() {
        let (url, seen) = fake_portal(share_portal);
        let mut h = share_window(url);
        wait_for(&mut h, "Jane Sample (jane@example.test)");
        h.get_by_label("Invite someone…").click();
        h.run();
        crate::ui::test_util::type_into(&mut h, "Email", "pat@example.test");
        crate::ui::test_util::type_into(&mut h, "Name", "Pat Sample");
        h.get_by_label("What they receive").click();
        h.run();
        assert!(
            h.query_by_label_contains(
                "Email · Your accounting firm invites you to its client portal"
            )
            .is_some()
        );
        h.get_by_label("Send invitation and share").click();
        wait_for(&mut h, "This document is theirs when they accept");
        let w = writes(&seen);
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(
            w[0].starts_with("POST ") && w[0].contains("pat@example.test"),
            "{w:?}"
        );
        assert!(
            w[1].starts_with("PUT /api/firm/v1/files/f1/visibility ")
                && w[1].contains(r#""invites":{"i9":true}"#),
            "{w:?}"
        );
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
            record: Default::default(),
            b2_list: false,
            todo_lists: false,
            balances: false,
            approve_share: Default::default(),
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
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
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
            record: Default::default(),
            b2_list: false,
            todo_lists: false,
            balances: false,
            approve_share: Default::default(),
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
            B2First::NotSetUp,
        );
        let h = egui_kittest::Harness::builder()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
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

    /// PA-80: with the setting ticked, the documents this PC knows to be in B2 go to the portal
    /// by client (their place in the client's folder, size and year; no content), and a client
    /// with nothing in B2 is not sent.
    #[test]
    fn the_documents_in_b2_are_sent_to_the_portal_by_client() {
        const ACME: &str = "0199aaaa-0000-7000-8000-000000000001";
        const EMPTY: &str = "0199aaaa-0000-7000-8000-000000000002";
        let t = tempfile::tempdir().unwrap();
        let catalog = t.path().join("catalog.db");
        let conn = vf_core::db::open(&catalog, vf_core::db::Storage::Local).unwrap();
        for (id, ext, name) in [
            (ACME, "1001", "Acme Farms LLC"),
            (EMPTY, "1002", "Birch, Lee"),
        ] {
            conn.execute(
                "INSERT INTO client (id, ext_id, name, folder_rel, device_id, updated_at)
                 VALUES (?1, ?2, ?3, ?3 || ' (' || ?2 || ')', 'dev-1', '2026-11-15T00:00:00Z')",
                [id, ext, name],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO sync_segment (client_id, rev) VALUES (?1, 3)",
                [id],
            )
            .unwrap();
        }
        for (id, path, year, sent) in [
            ("d1", "Income Tax/2025/1040.pdf", Some(2025), true),
            ("d2", "Permanent/Setup/EIN letter.pdf", None, true),
            // Held back by the firm's rule: never sent, so not listed.
            (
                "d3",
                "Workpapers & Support/2025/notes.pdf",
                Some(2025),
                false,
            ),
        ] {
            let rel = format!("Acme Farms LLC (1001)/{path}");
            conn.execute(
                "INSERT INTO document (id, client_id, rel_path, hash, size, tax_year, filed_at,
                                       device_id, updated_at)
                 VALUES (?1, ?2, ?3, 'h', 120, ?4, '2026-11-15T00:00:00Z', 'dev-1',
                         '2026-11-15T00:00:00Z')",
                vf_core::db::params![id, ACME, rel, year],
            )
            .unwrap();
            if sent {
                conn.execute(
                    "INSERT INTO sync_base (doc_id, client_id, rel_path, hash, rev, deleted)
                     VALUES (?1, ?2, ?3, 'h', 1, 0)",
                    vf_core::db::params![id, ACME, rel],
                )
                .unwrap();
            }
        }
        conn.execute(
            "INSERT INTO sync_state (device_id, last_manifest_rev) VALUES ('dev-1', 7)",
            [],
        )
        .unwrap();
        drop(conn);

        let (url, seen) = fake_portal(|method, path| match (method, path) {
            ("GET", "/api/firm/v1/cabinet") => (200, r#"{"max":20000,"clients":[]}"#.into()),
            _ => (200, r#"{"ok":true}"#.into()),
        });
        // The cabinet's own settings say whether the lists go, and where.
        let root = t.path().join("cabinet");
        vf_fs::cabinet::create(
            &root,
            &vf_fs::cabinet::CabinetFile {
                cabinet_id: "cab-1".into(),
                firm_id: "firm-1".into(),
                schema_version: 1,
                layout_version: 1,
                created: "2026-11-15T00:00:00Z".into(),
                name: None,
            },
        )
        .unwrap();
        let set = |url: String, on: bool| {
            settings::save(
                &root,
                &PortalSettings {
                    version: 1,
                    cabinet_id: "cab-1".into(),
                    url,
                    key: "vpk_test".into(),
                    fetch_minutes: 0,
                    record: Default::default(),
                    b2_list: on,
                    todo_lists: false,
                    balances: false,
                    approve_share: Default::default(),
                },
            )
            .unwrap();
        };
        set(url.clone(), true);
        let rep = core::send_lists(&root, &catalog, "dev-1");
        assert_eq!(
            rep,
            core::ListReport {
                sent: 1,
                max_docs: 20000,
                ..Default::default()
            }
        );
        let puts = requests(&seen, "PUT /api/firm/v1/cabinet/");
        assert_eq!(puts.len(), 1, "{puts:?}");
        assert!(puts[0].starts_with(&format!("PUT /api/firm/v1/cabinet/{ACME} ")));
        let body: serde_json::Value =
            serde_json::from_str(puts[0].splitn(3, ' ').nth(2).unwrap()).unwrap();
        assert_eq!(body["clientCode"], "1001");
        assert_eq!(body["displayName"], "Acme Farms LLC");
        assert_eq!(body["rev"], 3);
        assert_eq!(body["headRev"], 7);
        assert_eq!(body["hash"].as_str().unwrap().len(), 64);
        assert_eq!(
            body["files"],
            serde_json::json!([
                { "docId": "d1", "path": "Income Tax/2025/1040.pdf", "size": 120, "taxYear": 2025 },
                { "docId": "d2", "path": "Permanent/Setup/EIN letter.pdf", "size": 120, "taxYear": null },
            ])
        );

        // Taken out of B2 on a PC of this cabinet (the cabinet's own list says so) while this
        // PC's catalog still has it as exchanged: not listed.
        vf_sync::removed::add(&root, &[("d2".to_string(), "x".to_string())]).unwrap();
        let (url2, seen2) = fake_portal(|method, path| match (method, path) {
            ("GET", "/api/firm/v1/cabinet") => (200, r#"{"max":20000,"clients":[]}"#.into()),
            _ => (200, r#"{"ok":true}"#.into()),
        });
        set(url2, true);
        assert_eq!(core::send_lists(&root, &catalog, "dev-1").sent, 1);
        let put = requests(&seen2, "PUT /api/firm/v1/cabinet/");
        let body: serde_json::Value =
            serde_json::from_str(put[0].splitn(3, ' ').nth(2).unwrap()).unwrap();
        assert_eq!(body["files"].as_array().unwrap().len(), 1);
        assert_eq!(body["files"][0]["docId"], "d1");
        let none: std::collections::HashSet<String> = ["d2".to_string()].into();
        vf_sync::removed::clear(&root, &none).unwrap();

        // Unticked (on this PC or another: the cabinet's settings are read at each send), or
        // the portal no longer used: the portal is not called.
        let calls = seen.lock().unwrap().len();
        set(url.clone(), false);
        assert_eq!(
            core::send_lists(&root, &catalog, "dev-1"),
            core::ListReport::default()
        );
        settings::clear(&root).unwrap();
        assert_eq!(
            core::send_lists(&root, &catalog, "dev-1"),
            core::ListReport::default()
        );
        assert_eq!(seen.lock().unwrap().len(), calls);

        // A portal from before Vibe-Portal P90: said in words, nothing sent.
        let (url, seen) = fake_portal(|_, _| (404, r#"{"error":"not_found"}"#.into()));
        set(url, true);
        let rep = core::send_lists(&root, &catalog, "dev-1");
        assert_eq!(rep.problem.as_deref(), Some(vf_portal::listing::NEEDS_P90));
        assert!(requests(&seen, "PUT ").is_empty());
        assert_eq!(
            list_problem(&rep).unwrap(),
            "the portal cannot show the documents in B2 yet. Update the portal (on the appliance: \
             sudo vibe upgrade portal)."
        );

        // A PC that has never exchanged anything with the bucket does not call the portal.
        let bare = t.path().join("bare.db");
        drop(vf_core::db::open(&bare, vf_core::db::Storage::Local).unwrap());
        let (url, seen) = fake_portal(|_, _| (200, r#"{"max":20000,"clients":[]}"#.into()));
        set(url, true);
        assert_eq!(
            core::send_lists(&root, &bare, "dev-1"),
            core::ListReport::default()
        );
        assert!(seen.lock().unwrap().is_empty());

        // The portal refuses one client's list: it is named, and the next client's still goes.
        let conn = vf_core::db::open(&catalog, vf_core::db::Storage::Local).unwrap();
        conn.execute(
            "INSERT INTO document (id, client_id, rel_path, hash, size, tax_year, filed_at,
                                   device_id, updated_at)
             VALUES ('e1', ?1, 'Birch, Lee (1002)/Income Tax/2025/W-2.pdf', 'h', 9, 2025,
                     '2026-11-15T00:00:00Z', 'dev-1', '2026-11-15T00:00:00Z')",
            [EMPTY],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sync_base (doc_id, client_id, rel_path, hash, rev, deleted)
             VALUES ('e1', ?1, 'Birch, Lee (1002)/Income Tax/2025/W-2.pdf', 'h', 1, 0)",
            [EMPTY],
        )
        .unwrap();
        drop(conn);
        let (url, seen) = fake_portal(|method, path| match (method, path) {
            ("GET", "/api/firm/v1/cabinet") => (200, r#"{"max":20000,"clients":[]}"#.into()),
            ("PUT", p) if p.ends_with("0001") => (400, r#"{"error":"invalid"}"#.into()),
            _ => (200, r#"{"ok":true}"#.into()),
        });
        set(url, true);
        let rep = core::send_lists(&root, &catalog, "dev-1");
        assert_eq!(
            rep,
            core::ListReport {
                sent: 1,
                refused: vec!["Acme Farms LLC".into()],
                max_docs: 20000,
                ..Default::default()
            }
        );
        assert_eq!(requests(&seen, "PUT /api/firm/v1/cabinet/").len(), 2);
        assert_eq!(
            list_problem(&rep).unwrap(),
            "the portal did not take the list of documents in B2 of Acme Farms LLC; the other \
             clients' lists were sent."
        );
    }

    /// Code review of PA-80: a send that is running when the firm unticks the setting (on any
    /// PC) stops before its next list and takes the lists off the portal again, so none it
    /// sent after the firm's own removal stays; a list the portal keeps another PC's for is
    /// not counted as sent.
    #[test]
    fn a_send_under_way_when_the_setting_is_unticked_leaves_nothing_on_the_portal() {
        use vf_portal::listing::{ClientList, ListFile, Local, send_while};
        let list = |id: &str| ClientList {
            id: id.into(),
            code: "1".into(),
            name: format!("Client {id}"),
            rev: 1,
            files: vec![ListFile {
                doc_id: format!("{id}-d"),
                path: "a.pdf".into(),
                size: 1,
                tax_year: None,
            }],
        };
        let local = Local {
            head_rev: 1,
            live: ["a".to_string(), "b".to_string(), "c".to_string()].into(),
            clients: vec![list("a"), list("b"), list("c")],
            me: "PC1\\kurt".into(),
        };
        let portal = |url: String| {
            core::client(&PortalSettings {
                version: 1,
                cabinet_id: "c".into(),
                url,
                key: "vpk_test".into(),
                fetch_minutes: 0,
                record: Default::default(),
                b2_list: true,
                todo_lists: false,
                balances: false,
                approve_share: Default::default(),
            })
        };
        let (url, seen) = fake_portal(|method, path| match (method, path) {
            ("GET", "/api/firm/v1/cabinet") => (200, r#"{"max":20000,"clients":[]}"#.into()),
            ("DELETE", "/api/firm/v1/cabinet") => (200, r#"{"ok":true,"clients":1}"#.into()),
            _ => (200, r#"{"ok":true}"#.into()),
        });
        // Wanted for the first list, not for the second.
        let mut asked = 0;
        let rep = send_while(&portal(url), &local, &mut || {
            asked += 1;
            asked == 1
        });
        assert!(rep.stopped);
        assert_eq!(rep.sent, 1);
        assert_eq!(rep.problem, None);
        let calls: Vec<String> = seen
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.split(' ').take(2).collect::<Vec<_>>().join(" "))
            .collect();
        assert_eq!(
            calls,
            [
                "GET /api/firm/v1/cabinet",
                "PUT /api/firm/v1/cabinet/a",
                "DELETE /api/firm/v1/cabinet",
            ]
        );

        // Unticked just as the last list went: found at the end, and removed.
        let (url, seen) = fake_portal(|method, path| match (method, path) {
            ("GET", "/api/firm/v1/cabinet") => (200, r#"{"max":20000,"clients":[]}"#.into()),
            _ => (200, r#"{"ok":true}"#.into()),
        });
        let mut asked = 0;
        let rep = send_while(&portal(url), &local, &mut || {
            asked += 1;
            asked <= 3
        });
        assert!(rep.stopped);
        assert_eq!(rep.sent, 3);
        assert_eq!(requests(&seen, "DELETE /api/firm/v1/cabinet ").len(), 1);

        // Still wanted to the end: nothing is removed. A list the portal keeps another PC's
        // for is not sent, and not counted as sent.
        let (url, seen) = fake_portal(|method, path| match (method, path) {
            ("GET", "/api/firm/v1/cabinet") => (200, r#"{"max":20000,"clients":[]}"#.into()),
            ("PUT", p) if p.ends_with("/b") => (200, r#"{"ok":true,"kept":true}"#.into()),
            _ => (200, r#"{"ok":true}"#.into()),
        });
        let rep = send_while(&portal(url), &local, &mut || true);
        assert_eq!((rep.sent, rep.kept, rep.stopped), (2, 1, false));
        assert!(requests(&seen, "DELETE ").is_empty());
    }

    /// Code review of PA-80: the lists leave the portal they were sent to when the address
    /// changes, and Stop removes them whether or not the setting is still ticked.
    #[test]
    fn the_lists_leave_the_portal_they_were_sent_to() {
        const KEY: &str = "vpk_abcdefghijklmnopqrstuvwxyz0123456789";
        let answer: fn(&str, &str) -> (u16, String) = |method, path| match (method, path) {
            ("DELETE", "/api/firm/v1/cabinet") => (200, r#"{"ok":true,"clients":3}"#.into()),
            _ => (200, r#"{"ok":true}"#.into()),
        };
        let (old_url, old_seen) = fake_portal(answer);
        let (new_url, new_seen) = fake_portal(answer);
        let root = tempfile::tempdir().unwrap();
        vf_fs::cabinet::create(
            root.path(),
            &vf_fs::cabinet::CabinetFile {
                cabinet_id: "cab-1".into(),
                firm_id: "firm-1".into(),
                schema_version: 1,
                layout_version: 1,
                created: "2026-11-15T00:00:00Z".into(),
                name: None,
            },
        )
        .unwrap();
        let mut conn = vf_core::db::Connection::open_in_memory().unwrap();
        let mut env = Env {
            conn: &mut conn,
            root: root.path(),
            device_id: "dev-1",
        };
        let ctx = egui::Context::default();
        let done = |a: &mut AdminPortal| -> String {
            a.status
                .take()
                .expect("the lists are being removed")
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .unwrap()
        };
        let mut a = AdminPortal::default();
        a.load(root.path());
        a.url = old_url;
        a.key = KEY.into();
        a.b2_list = true;
        save(&mut a, &ctx, &mut env);
        assert_eq!(a.err, None);

        // Another portal, the setting still ticked: the old portal's lists go, the new one is
        // not asked to remove any.
        a.url = new_url;
        save(&mut a, &ctx, &mut env);
        assert_eq!(a.err, None);
        let said = done(&mut a);
        assert!(
            said.contains("removed from the portal (3 clients)"),
            "{said}"
        );
        assert!(said.contains("within about 10 minutes"), "{said}");
        assert_eq!(requests(&old_seen, "DELETE /api/firm/v1/cabinet").len(), 1);
        assert!(requests(&new_seen, "DELETE ").is_empty());

        // Unticked on disk already (an untick whose removal had failed): Stop removes them.
        let kept = settings::load(root.path(), "cab-1")
            .unwrap()
            .with_b2_list(false);
        settings::save(root.path(), &kept).unwrap();
        stop(&mut a, &ctx, &mut env);
        assert_eq!(a.err, None);
        let said = done(&mut a);
        assert!(said.starts_with("The portal is no longer used."), "{said}");
        assert_eq!(requests(&new_seen, "DELETE /api/firm/v1/cabinet").len(), 1);
        assert_eq!(settings::load(root.path(), "cab-1"), None);
    }

    /// QA of PA-80: a new key for the same portal keeps the lists; a removal that fails is a
    /// problem with what to do, naming the old portal when the address changed; Stop with the
    /// setting not ticked raises no alarm; ticking it against an older portal says so on Save.
    #[test]
    fn what_admin_portal_says_about_the_lists() {
        const KEY: &str = "vpk_abcdefghijklmnopqrstuvwxyz0123456789";
        const KEY2: &str = "vpk_zyxwvutsrqponmlkjihgfedcba9876543210";
        let (url, seen) = fake_portal(|method, path| match (method, path) {
            ("GET", "/api/firm/v1/cabinet") => (200, r#"{"max":20000,"clients":[]}"#.into()),
            _ => (200, r#"{"ok":true}"#.into()),
        });
        // A portal that fails every call, and one from before Vibe-Portal P90.
        let (down, _) = fake_portal(|_, _| (500, r#"{"error":"internal"}"#.into()));
        let (old, _) = fake_portal(|_, _| (404, r#"{"error":"not_found"}"#.into()));
        let root = tempfile::tempdir().unwrap();
        vf_fs::cabinet::create(
            root.path(),
            &vf_fs::cabinet::CabinetFile {
                cabinet_id: "cab-1".into(),
                firm_id: "firm-1".into(),
                schema_version: 1,
                layout_version: 1,
                created: "2026-11-15T00:00:00Z".into(),
                name: None,
            },
        )
        .unwrap();
        let mut conn = vf_core::db::Connection::open_in_memory().unwrap();
        let mut env = Env {
            conn: &mut conn,
            root: root.path(),
            device_id: "dev-1",
        };
        let ctx = egui::Context::default();
        let done = |a: &mut AdminPortal| -> Result<String, String> {
            a.status
                .take()
                .expect("the portal is being asked")
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
        };
        let mut a = AdminPortal::default();
        a.load(root.path());
        a.url = url.clone();
        a.key = KEY.into();
        a.b2_list = true;
        save(&mut a, &ctx, &mut env);
        assert!(done(&mut a).unwrap().contains("within about 10 minutes"));

        // A new key, the same portal: nothing is removed, nothing asked.
        a.key = KEY2.into();
        save(&mut a, &ctx, &mut env);
        assert_eq!(a.err, None);
        assert!(a.status.is_none());
        assert!(requests(&seen, "DELETE ").is_empty());

        // The address changed to a portal from before the lists: it is said here, as a problem,
        // and the old portal's lists were removed all the same.
        a.url = old.clone();
        save(&mut a, &ctx, &mut env);
        assert!(done(&mut a).is_ok());
        assert_eq!(requests(&seen, "DELETE /api/firm/v1/cabinet").len(), 1);
        a.b2_list = false;
        save(&mut a, &ctx, &mut env);
        // An older portal has none to remove: no alarm.
        assert!(done(&mut a).unwrap().starts_with("Saved for every PC."));
        a.b2_list = true;
        save(&mut a, &ctx, &mut env);
        let e = done(&mut a).unwrap_err();
        assert!(
            e.contains("update the portal") && e.contains("The setting is saved"),
            "{e}"
        );

        // Ticked on a portal that then cannot be reached, and the address changed: the removal
        // fails, as a problem that names the portal the lists are on.
        a.url = down.clone();
        save(&mut a, &ctx, &mut env);
        let _ = done(&mut a);
        a.url = url.clone();
        save(&mut a, &ctx, &mut env);
        let e = done(&mut a).unwrap_err();
        assert!(e.contains("could NOT be removed"), "{e}");
        assert!(
            e.contains(&format!("still on the portal used before ({down})")),
            "{e}"
        );
        // What was saved is still said.
        assert!(a.msg.as_deref().unwrap().starts_with("Saved for every PC."));

        // Unticked while it cannot be reached: tick and untick again.
        a.url = down.clone();
        save(&mut a, &ctx, &mut env);
        let _ = done(&mut a);
        a.b2_list = false;
        save(&mut a, &ctx, &mut env);
        let e = done(&mut a).unwrap_err();
        assert!(
            e.ends_with("Tick the setting, save, then untick it and save again."),
            "{e}"
        );
        // Stop, with the setting not ticked and the portal still not reached: no alarm, one
        // quiet sentence.
        stop(&mut a, &ctx, &mut env);
        let said = done(&mut a).unwrap();
        assert!(
            said.ends_with("If lists of documents in B2 were ever sent to this portal, they may still be there."),
            "{said}"
        );
    }

    /// QA of PA-80: everything that kept a list from going is said, with the limit and what to
    /// do; a new bucket's lists wait until the old bucket's are off the portal.
    #[test]
    fn what_browse_is_told_about_the_lists() {
        let mut r = core::ListReport {
            max_docs: 20000,
            max_path: Some(500),
            ..Default::default()
        };
        assert_eq!(list_problem(&r), None);
        r.too_many = vec!["Acme Farms LLC".into()];
        r.too_long = vec!["Birch, Lee".into(), "Cole Inc".into()];
        r.refused = vec!["Dane LLC".into()];
        assert_eq!(
            list_problem(&r).unwrap(),
            "Acme Farms LLC has more than 20000 documents in B2, the most the portal lists for \
             one client; its list was not sent, and the portal shows the last list sent, if any. \
             2 clients have a document in B2 whose folder and name together are longer than 500 \
             characters; their lists were not sent until that document is renamed or moved. The \
             portal did not take the list of documents in B2 of Dane LLC."
        );
        let failed = core::ListReport {
            problem: Some("The portal cannot be reached: timeout".into()),
            ..Default::default()
        };
        assert_eq!(
            list_problem(&failed).unwrap(),
            "the lists of documents in B2 were not all sent: The portal cannot be reached: timeout."
        );
        let unremoved = core::ListReport {
            stopped: true,
            unremoved: Some("The portal cannot be reached: timeout".into()),
            ..Default::default()
        };
        assert!(
            list_problem(&unremoved)
                .unwrap()
                .ends_with("save, then untick it and save.")
        );

        // A new bucket: marked only while the lists are being sent; the next send removes the
        // old bucket's lists first, and says so while it cannot.
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("cabinet");
        vf_fs::cabinet::create(
            &root,
            &vf_fs::cabinet::CabinetFile {
                cabinet_id: "cab-1".into(),
                firm_id: "firm-1".into(),
                schema_version: 1,
                layout_version: 1,
                created: "2026-11-15T00:00:00Z".into(),
                name: None,
            },
        )
        .unwrap();
        let catalog = t.path().join("catalog.db");
        drop(vf_core::db::open(&catalog, vf_core::db::Storage::Local).unwrap());
        let set = |url: String, on: bool| {
            settings::save(
                &root,
                &PortalSettings {
                    version: 1,
                    cabinet_id: "cab-1".into(),
                    url,
                    key: "vpk_test".into(),
                    fetch_minutes: 0,
                    record: Default::default(),
                    b2_list: on,
                    todo_lists: false,
                    balances: false,
                    approve_share: Default::default(),
                },
            )
            .unwrap();
        };
        let mark = root
            .join("_Vibe")
            .join("keys")
            .join("portal-lists-old-bucket");
        let (down, _) = fake_portal(|_, _| (500, r#"{"error":"internal"}"#.into()));
        set(down, false);
        core::forget_lists(&root);
        assert!(!mark.exists(), "not sending lists: nothing to forget");
        let (down, _) = fake_portal(|_, _| (500, r#"{"error":"internal"}"#.into()));
        set(down, true);
        core::forget_lists(&root);
        assert!(mark.exists());
        let rep = core::send_lists(&root, &catalog, "dev-1");
        assert!(
            rep.problem
                .as_deref()
                .unwrap()
                .starts_with(core::OLD_BUCKET)
        );
        assert!(mark.exists(), "tried again at the next send");
        let (up, seen) = fake_portal(|_, _| (200, r#"{"ok":true,"clients":4}"#.into()));
        set(up, true);
        assert_eq!(core::send_lists(&root, &catalog, "dev-1").problem, None);
        assert_eq!(requests(&seen, "DELETE /api/firm/v1/cabinet").len(), 1);
        assert!(!mark.exists());
    }

    /// PA-80: the setting is kept with the portal settings, and unticking it takes the lists
    /// off the portal at once.
    #[test]
    fn unticking_the_b2_list_removes_the_lists_from_the_portal() {
        const KEY: &str = "vpk_abcdefghijklmnopqrstuvwxyz0123456789";
        let (url, seen) = fake_portal(|method, path| match (method, path) {
            ("DELETE", "/api/firm/v1/cabinet") => (200, r#"{"ok":true,"clients":2}"#.into()),
            _ => (200, r#"{"ok":true}"#.into()),
        });
        let root = tempfile::tempdir().unwrap();
        vf_fs::cabinet::create(
            root.path(),
            &vf_fs::cabinet::CabinetFile {
                cabinet_id: "cab-1".into(),
                firm_id: "firm-1".into(),
                schema_version: 1,
                layout_version: 1,
                created: "2026-11-15T00:00:00Z".into(),
                name: None,
            },
        )
        .unwrap();
        let mut conn = vf_core::db::Connection::open_in_memory().unwrap();
        let mut env = Env {
            conn: &mut conn,
            root: root.path(),
            device_id: "dev-1",
        };
        let ctx = egui::Context::default();
        let mut a = AdminPortal::default();
        a.load(root.path());
        a.url = url;
        a.key = KEY.into();
        a.b2_list = true;
        save(&mut a, &ctx, &mut env);
        assert_eq!(a.err, None);
        assert!(settings::load(root.path(), "cab-1").unwrap().b2_list);
        // Ticked and saved: when the lists go is said, and nothing is removed.
        let said = a
            .status
            .take()
            .expect("the portal is asked whether it can take the lists")
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        assert!(said.contains("within about 10 minutes"), "{said}");
        assert!(requests(&seen, "DELETE ").is_empty());

        a.b2_list = false;
        save(&mut a, &ctx, &mut env);
        assert_eq!(a.err, None);
        assert!(!settings::load(root.path(), "cab-1").unwrap().b2_list);
        let said = a
            .status
            .take()
            .expect("the lists are being removed")
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        assert!(
            said.contains("removed from the portal (2 clients)"),
            "{said}"
        );
        assert_eq!(requests(&seen, "DELETE /api/firm/v1/cabinet").len(), 1);
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

    /// Q-103 (owner 2026-11-13): a shared document goes to B2 first. With B2 set up but
    /// locked on this PC the link is not sent and nothing reaches the portal; with B2 not set
    /// up the share goes ahead (the test above).
    #[test]
    fn a_secure_link_waits_for_b2_when_it_is_locked() {
        let (mut h, seen, audit, _t) = link_window(|method, path| match (method, path) {
            ("GET", p) if p.starts_with("/api/firm/v1/files?clientId=AB-1") => {
                (200, r#"{"files":[]}"#.into())
            }
            _ => (200, r#"{"ok":true}"#.into()),
        });
        wait_for(&mut h, "Not on the portal yet");
        h.state_mut().b2 = B2First::Locked;
        fill_and_send(&mut h);
        let start = std::time::Instant::now();
        while h.state().err.is_none() {
            h.step();
            assert!(
                start.elapsed() < crate::ui::test_util::PATIENCE,
                "no answer"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let err = h.state().err.clone().unwrap();
        assert!(err.contains("B2 sync is locked"), "{err}");
        assert!(
            requests(&seen, "POST").is_empty(),
            "nothing reached the portal"
        );
        assert_eq!(audit.try_iter().count(), 0);
        // Nothing to push is nothing to wait for, whatever the state.
        assert!(B2First::Locked.push(&[]).is_ok());
        assert!(B2First::NotSetUp.push(&["doc-1".to_string()]).is_ok());
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
