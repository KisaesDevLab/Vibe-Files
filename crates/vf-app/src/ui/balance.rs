//! Balance owed (Addendum tax-return delivery §16, owner 2026-11-24; Vibe-Portal D7): the client
//! panel's Balance owed… window and Admin › Portal › Balances. Staff post an amount; the client
//! pays it on the portal through the firm's own Stripe account. The amount lives on the
//! portal; each balance read is copied into the client's folder (`_Vibe/balance.jsonl`) with an
//! audit row when it changed, and a balance paid or left unpaid past its date is recorded once.
//! Every portal call runs on a thread with "Working…" shown.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};

use egui::RichText;
use vf_core::audit::AuditEntry;
use vf_portal::PortalSettings;
use vf_portal::balance::{
    self, Balance, BalanceStaff, Balances, Defaults, PostBalance, money, parse_money,
};

use crate::portal as core;
use crate::ui::portal::{AuditOut, ClientRef, spawn};
use crate::ui::{theme, widgets};

/// Remind every … days (0: never).
const REMIND_CHOICES: [u32; 4] = [0, 3, 7, 14];

// ---- The copy in the client's folder -----------------------------------------------------------

/// Balances read from the portal, for the app to copy into their client folders.
#[derive(Clone)]
pub struct ReadOut(pub(crate) Sender<Vec<Balance>>);

impl ReadOut {
    pub(crate) fn send(&self, b: Vec<Balance>) {
        if !b.is_empty() {
            let _ = self.0.send(b);
        }
    }
}

/// A balance as read, to be appended to its client's copy.
#[derive(Debug, Clone)]
pub struct BalanceCopy {
    pub path: PathBuf,
    pub line: String,
    pub client_id: String,
    pub balance_id: String,
    pub state: String,
    /// `paid` or `expired` when this read is the first to see it.
    pub change: Option<&'static str>,
}

/// The copies to write for balances read: only into a folder still bound to the client, and
/// only when the balance changed since its copy's last line.
pub fn copies_for(
    conn: &vf_core::db::Connection,
    root: &Path,
    read: &[Balance],
    by: &str,
) -> Vec<BalanceCopy> {
    let mut by_client: HashMap<&str, Vec<&Balance>> = HashMap::new();
    for b in read {
        by_client.entry(b.client_id.as_str()).or_default().push(b);
    }
    let now = vf_core::now_rfc3339();
    let mut out = Vec::new();
    for (client, list) in by_client {
        let folder: Option<String> = conn
            .query_row(
                "SELECT folder_rel FROM client WHERE id = ?1 AND deleted_at IS NULL",
                [client],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        let Some(folder) = folder else { continue };
        let bound = filer_intake::sentinel::read_sentinel(&root.join(&folder))
            .ok()
            .flatten()
            .map(|s| s.client_id);
        if bound.is_none() || bound != uuid::Uuid::parse_str(client).ok() {
            continue;
        }
        let path = balance::copy_path(root, &folder);
        let copy = balance::read_copy(root, &folder);
        for b in list {
            if let Some(line) = balance::copy_line(&copy, b, &now, by) {
                out.push(BalanceCopy {
                    path: path.clone(),
                    line,
                    client_id: b.client_id.clone(),
                    balance_id: b.id.clone(),
                    state: b.state.clone(),
                    change: balance::change_of(&copy, b),
                });
            }
        }
    }
    out
}

/// Writes the copies in one Transaction with an audit row for each (CLAUDE.md principle 5):
/// `portal_balance_paid` and `portal_balance_expired` the first time a PC sees those, else
/// `portal_balance_copy`. Field names only, never amounts (§16.3). Append-only.
pub fn write_copies(
    conn: &mut vf_core::db::Connection,
    root: &Path,
    device_id: &str,
    copies: Vec<BalanceCopy>,
) -> Result<(), String> {
    if copies.is_empty() {
        return Ok(());
    }
    let mut lines: Vec<(PathBuf, Vec<String>)> = Vec::new();
    let mut tx = vf_core::tx::Transaction::new("portal_balance_copy").record_only();
    for c in copies {
        let action = match c.change {
            Some(balance::PAID) => "portal_balance_paid",
            Some(balance::EXPIRED) => "portal_balance_expired",
            _ => "portal_balance_copy",
        };
        tx = tx.audit(AuditEntry::new(
            action,
            None,
            serde_json::json!({
                "client_id": c.client_id,
                "balance_id": c.balance_id,
                "state": c.state,
            }),
        ));
        match lines.iter_mut().find(|(p, _)| *p == c.path) {
            Some((_, l)) => l.push(c.line),
            None => lines.push((c.path, vec![c.line])),
        }
    }
    for (path, lines) in lines {
        tx = tx.file(vf_core::tx::FileOp::AppendLines {
            path,
            lines,
            undo_lines: Vec::new(),
        });
    }
    let mut fs = vf_fs::fileops::FilerFileOps::new(&[root]).map_err(|e| e.to_string())?;
    tx.commit(conn, &mut fs, &vf_core::tx::Actor::device(device_id))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// "PC1\kurt", as the copies say who read them.
fn reader() -> String {
    let (pc, user) = core::who();
    format!("{pc}\\{user}")
}

/// Writes what the portal said of balances into the client folders (from the app's frame).
pub fn record_reads(
    conn: &mut vf_core::db::Connection,
    root: &Path,
    device_id: &str,
    reads: Vec<Vec<Balance>>,
) -> Result<(), String> {
    let all: Vec<Balance> = reads.into_iter().flatten().collect();
    if all.is_empty() {
        return Ok(());
    }
    let copies = copies_for(conn, root, &all, &reader());
    write_copies(conn, root, device_id, copies)
}

// ---- The Balance owed window ------------------------------------------------------------------

/// What the hold does, under its tick (PA-114, owner 2026-11-26).
pub const HOLD_WORDS: &str = "They see their files listed, marked 'Opens when your balance is \
     paid'; files shared meanwhile wait too. Paying, Clear balance or the balance's removal \
     releases them.";

enum Done {
    Loaded(Result<(Vec<Balance>, Vec<BalanceStaff>, Defaults), vf_portal::PortalError>),
    /// The post, whether it is a new balance, and whether the files' hold was asked for.
    Posted(Result<Box<balance::Posted>, String>, bool, bool),
    Cleared(Result<balance::Cleared, String>),
    Reminded(Result<u32, String>),
}

enum Act {
    Post,
    Clear,
    Remind,
    Reload,
}

pub struct BalanceWindow {
    pub closed: bool,
    client: ClientRef,
    root: PathBuf,
    folder: Option<String>,
    /// The client's balances, newest first (from the portal, or the copy when out of reach).
    balances: Vec<Balance>,
    /// "as of <time>" when the copy is shown.
    offline: Option<String>,
    staff: Vec<BalanceStaff>,
    amount: String,
    memo: String,
    assigned: Option<String>,
    expire_on: bool,
    expire_days: u32,
    notify: bool,
    remind_days: u32,
    /// PA-114: the client's shared files held until this is paid.
    hold_files: bool,
    /// The form was filled from what was read (not again on every read).
    filled: bool,
    confirm_clear: bool,
    job: Option<Receiver<Done>>,
    said: Option<(String, bool)>,
    err: Option<String>,
    audit: AuditOut,
    reads: ReadOut,
    /// Admin › Portal's "Assign new cards to" (owner 2026-11-26).
    default_staff: Option<vf_portal::settings::DefaultStaff>,
}

impl BalanceWindow {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ctx: &egui::Context,
        s: &PortalSettings,
        root: PathBuf,
        client: ClientRef,
        folder: Option<String>,
        audit: AuditOut,
        reads: ReadOut,
    ) -> Self {
        let mut w = Self {
            closed: false,
            client,
            root,
            folder,
            balances: Vec::new(),
            offline: None,
            staff: Vec::new(),
            amount: String::new(),
            memo: String::new(),
            assigned: None,
            expire_on: true,
            expire_days: 14,
            notify: true,
            remind_days: 7,
            hold_files: false,
            filled: false,
            confirm_clear: false,
            job: None,
            said: None,
            err: None,
            audit,
            reads,
            default_staff: s.default_staff.clone(),
        };
        w.load(ctx, s);
        w
    }

    fn load(&mut self, ctx: &egui::Context, s: &PortalSettings) {
        let s2 = s.clone();
        let id = self.client.id.clone();
        self.job = Some(spawn(ctx, move || {
            let p = core::client(&s2);
            Done::Loaded((|| {
                let mine = p.client_balances(&id)?;
                let staff = p.balance_staff(&id)?;
                let defaults = p.balances().map(|b| b.defaults).unwrap_or_default();
                Ok((mine, staff, defaults))
            })())
        }));
    }

    /// The balance open on the portal (due, else started).
    fn open(&self) -> Option<&Balance> {
        self.balances
            .iter()
            .find(|b| b.state == balance::DUE)
            .or_else(|| self.balances.iter().find(|b| b.state == balance::STARTED))
    }

    fn fill(&mut self, d: Defaults) {
        if self.filled {
            return;
        }
        self.filled = true;
        self.expire_on = d.expire_on;
        self.expire_days = d.expire_days.max(1);
        if let Some(b) = self.open().cloned() {
            self.amount = money(b.amount_cents).trim_start_matches('$').to_string();
            self.memo = b.memo.clone();
            self.assigned = b.assigned_staff_id.clone();
            self.notify = b.notify;
            self.remind_days = b.remind_days;
            self.hold_files = b.hold_files == Some(true);
            self.expire_on = b.expires_at.is_some();
            if let Some(n) = b.days_left {
                self.expire_days = n.max(1);
            }
        } else {
            let (who, why) =
                crate::ui::portal::new_card_assignee(self.default_staff.as_ref(), &self.staff);
            self.assigned = who;
            if let Some(w) = why {
                self.said = Some((w, true));
            }
        }
    }

    fn take_done(&mut self) {
        let Some(rx) = &self.job else { return };
        let Ok(done) = rx.try_recv() else { return };
        self.job = None;
        match done {
            Done::Loaded(Ok((mine, staff, defaults))) => {
                self.reads.send(mine.clone());
                self.balances = mine;
                self.staff = staff;
                self.offline = None;
                self.fill(defaults);
            }
            Done::Loaded(Err(e)) => {
                // Out of reach: the copy in the client folder, said so. Any other answer is
                // the portal's word, shown as it is.
                if matches!(e, vf_portal::PortalError::Network(_))
                    && let Some(f) = &self.folder
                {
                    let copy = balance::read_copy(&self.root, f);
                    if let Some(newest) = copy.iter().map(|c| c.read_at.as_str()).max() {
                        self.offline = Some(format!(
                            "The portal cannot be reached: as last read {}.",
                            widgets::local_time(newest)
                        ));
                        self.balances = copy.into_iter().map(|c| c.balance).collect();
                    }
                }
                self.err = Some(e.message());
            }
            Done::Posted(Ok(p), is_new, hold) => {
                let was = self.balances.iter().position(|b| b.id == p.balance.id);
                let note = if p.pending {
                    let pending = self
                        .balances
                        .iter()
                        .find(|b| b.state == balance::STARTED)
                        .map(|b| money(b.amount_cents))
                        .unwrap_or_default();
                    format!(
                        "Posted. A payment of {pending} is pending: the new amount is due once it settles."
                    )
                } else if is_new {
                    "Posted to the portal.".into()
                } else {
                    "Saved.".into()
                };
                self.audit.send(AuditEntry::new(
                    if is_new {
                        "portal_balance_post"
                    } else {
                        "portal_balance_change"
                    },
                    None,
                    serde_json::json!({
                        "client_id": self.client.id,
                        "balance_id": p.balance.id,
                        "fields": ["amount", "memo", "expires", "notify", "remind_days", "assigned", "hold_files"],
                        "pending": p.pending,
                    }),
                ));
                // A new amount replaces the due one on the portal; read the whole history
                // again to show it as the portal has it (one small read).
                match was {
                    Some(i) => self.balances[i] = p.balance.clone(),
                    None => self.balances.insert(0, p.balance.clone()),
                }
                // PA-114: an older portal posts the balance and leaves the files open.
                // QA 2026-11-26: said beside the pending payment's note, not instead of it.
                let (note, warn) = match balance::hold_not_taken(hold, &p.balance) {
                    Some(w) if p.pending => (format!("{note} {w}"), true),
                    Some(w) => (w.to_string(), true),
                    None => (note, p.pending),
                };
                self.reads.send(vec![p.balance]);
                self.said = Some((note, warn));
                self.err = None;
            }
            Done::Cleared(Ok(c)) => {
                self.audit.send(AuditEntry::new(
                    "portal_balance_clear",
                    None,
                    serde_json::json!({ "client_id": self.client.id, "pending": c.pending }),
                ));
                self.said = Some((
                    if c.pending {
                        "Cleared. A bank payment had started: if it clears, its receipt is still filed.".into()
                    } else {
                        "Cleared: the card has left the portal.".into()
                    },
                    c.pending,
                ));
                self.err = None;
                self.filled = false;
                self.amount.clear();
                self.memo.clear();
            }
            Done::Reminded(Ok(n)) => {
                self.audit.send(AuditEntry::new(
                    "portal_balance_remind",
                    None,
                    serde_json::json!({ "client_id": self.client.id, "told": n }),
                ));
                self.said = Some(match n {
                    0 => (
                        "Nobody could be told: invite someone to the portal first.".into(),
                        true,
                    ),
                    1 => ("1 message sent.".into(), false),
                    n => (format!("{n} messages sent."), false),
                });
                self.err = None;
            }
            Done::Posted(Err(e), _, _) | Done::Cleared(Err(e)) | Done::Reminded(Err(e)) => {
                self.err = Some(e);
            }
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, s: &PortalSettings) {
        self.take_done();
        let busy = self.job.is_some();
        let mut open_win = true;
        let mut act = None;
        egui::Window::new(format!("Balance owed — {}", self.client.name))
            .id(egui::Id::unique("portal-balance"))
            .open(&mut open_win)
            .collapsible(false)
            .default_width(520.0)
            .default_height(420.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| act = self.body(ui, busy));
            });
        if !open_win {
            self.closed = true;
        }
        if let Some(a) = act {
            self.apply(ctx, s, a);
        }
    }

    fn form(&mut self, ui: &mut egui::Ui, busy: bool) {
        let enabled = !busy && self.offline.is_none();
        ui.add_enabled_ui(enabled, |ui| {
            egui::Grid::new("balance-form")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Amount");
                    ui.horizontal(|ui| {
                        ui.label("$");
                        widgets::named(
                            ui.add(
                                egui::TextEdit::singleline(&mut self.amount)
                                    .desired_width(120.0)
                                    .hint_text("0.00"),
                            ),
                            "Balance amount",
                        );
                    });
                    ui.end_row();
                    ui.label("Memo");
                    widgets::named(
                        ui.add(
                            egui::TextEdit::singleline(&mut self.memo)
                                .desired_width(320.0)
                                .char_limit(balance::MEMO_MAX)
                                .hint_text("2025 bookkeeping, Oct–Dec"),
                        ),
                        "Balance memo",
                    );
                    ui.end_row();
                    ui.label("Assigned to");
                    let shown = self
                        .assigned
                        .as_deref()
                        .and_then(|id| self.staff.iter().find(|s| s.id == id))
                        .map(|s| s.name.clone())
                        .unwrap_or_else(|| "Nobody (Settings › Payments' person)".into());
                    egui::ComboBox::from_id_salt("balance-assigned")
                        .selected_text(shown)
                        .width(320.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.assigned,
                                None,
                                "Nobody (Settings › Payments' person)",
                            );
                            for s in &self.staff {
                                ui.selectable_value(
                                    &mut self.assigned,
                                    Some(s.id.clone()),
                                    &s.name,
                                );
                            }
                        });
                    ui.end_row();
                    ui.label("");
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.expire_on, "Remove from the portal after");
                        ui.add_enabled(
                            self.expire_on,
                            egui::DragValue::new(&mut self.expire_days).range(1..=365),
                        );
                        ui.label("days");
                    });
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(&mut self.notify, "Tell the client by email and text");
                    ui.end_row();
                    // PA-114 (owner 2026-11-26, M3-M6).
                    ui.label("");
                    ui.vertical(|ui| {
                        ui.checkbox(
                            &mut self.hold_files,
                            "Hold the client's files until this is paid",
                        );
                        if self.hold_files {
                            ui.label(widgets::caption(HOLD_WORDS));
                        }
                    });
                    ui.end_row();
                    ui.label("Remind");
                    egui::ComboBox::from_id_salt("balance-remind")
                        .selected_text(remind_words(self.remind_days))
                        .show_ui(ui, |ui| {
                            for n in REMIND_CHOICES {
                                ui.selectable_value(&mut self.remind_days, n, remind_words(n));
                            }
                        });
                    ui.end_row();
                });
        });
    }

    fn body(&mut self, ui: &mut egui::Ui, busy: bool) -> Option<Act> {
        let mut act = None;
        if busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Working…");
            });
        }
        if let Some(o) = &self.offline {
            ui.colored_label(theme::warn(), o);
        }
        let open = self.open().cloned();
        match &open {
            Some(b) => {
                ui.label(RichText::new(b.line()).strong());
            }
            None => {
                ui.label(RichText::new("No balance on the portal.").color(theme::ink_muted()));
            }
        }
        if open.as_ref().is_some_and(|b| b.state == balance::STARTED) {
            ui.label(widgets::caption(
                "A bank payment has started. Changing the amount posts a new balance, due once \
                 this payment settles.",
            ));
        }
        ui.add_space(6.0);
        self.form(ui, busy);
        ui.add_space(6.0);
        let amount = parse_money(&self.amount);
        let problem = match amount {
            None if !self.amount.trim().is_empty() => Some("The amount is dollars and cents."),
            Some(c) if c < balance::MIN_CENTS => Some("The amount is at least $0.50."),
            _ => None,
        };
        if let Some(p) = problem {
            ui.colored_label(theme::error(), p);
        }
        let can = !busy && self.offline.is_none();
        ui.horizontal(|ui| {
            let has_open = open.is_some();
            if ui
                .add_enabled(
                    can && has_open,
                    egui::Button::new(RichText::new("Clear balance").color(theme::error())),
                )
                .clicked()
            {
                self.confirm_clear = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = if open.as_ref().is_some_and(|b| b.state == balance::DUE) {
                    "Save changes"
                } else {
                    "Post to portal"
                };
                if ui
                    .add_enabled(
                        can && problem.is_none() && amount.is_some(),
                        widgets::primary_button(label),
                    )
                    .clicked()
                {
                    act = Some(Act::Post);
                }
                if ui
                    .add_enabled(
                        can && open.as_ref().is_some_and(|b| b.state == balance::DUE),
                        egui::Button::new("Remind now"),
                    )
                    .on_hover_text("Tell the client again by email and text (3 a day at most)")
                    .clicked()
                {
                    act = Some(Act::Remind);
                }
                if ui
                    .add_enabled(!busy, egui::Button::new("Read again"))
                    .clicked()
                {
                    act = Some(Act::Reload);
                }
            });
        });
        if self.confirm_clear {
            ui.group(|ui| {
                ui.label("Clear the balance? The card leaves the portal. A bank payment already started is still followed, and its receipt is filed if it clears.");
                ui.horizontal(|ui| {
                    if ui.button("Clear balance").clicked() {
                        self.confirm_clear = false;
                        act = Some(Act::Clear);
                    }
                    if ui.button("Keep it").clicked() {
                        self.confirm_clear = false;
                    }
                });
            });
        }
        if let Some((m, warn)) = &self.said {
            ui.colored_label(
                if *warn {
                    theme::warn()
                } else {
                    theme::health_ok()
                },
                m,
            );
        }
        if let Some(e) = &self.err {
            ui.colored_label(theme::error(), e);
        }
        ui.separator();
        ui.label(
            RichText::new("WHAT HAS HAPPENED")
                .small()
                .color(theme::ink_muted()),
        );
        if self.balances.is_empty() {
            ui.label(widgets::caption("Nothing yet."));
        }
        for b in &self.balances {
            ui.label(RichText::new(b.line()).strong());
            if !b.memo.is_empty() {
                ui.label(widgets::caption(&b.memo));
            }
            for e in &b.events {
                ui.label(
                    RichText::new(format!(
                        "{}  {}{}",
                        widgets::local_time(&e.at),
                        e.words,
                        if e.by.is_empty() {
                            String::new()
                        } else {
                            format!(" · {}", e.by)
                        }
                    ))
                    .small(),
                );
            }
            ui.add_space(4.0);
        }
        act
    }

    fn apply(&mut self, ctx: &egui::Context, s: &PortalSettings, act: Act) {
        self.said = None;
        self.err = None;
        let s2 = s.clone();
        let id = self.client.id.clone();
        match act {
            Act::Reload => {
                self.filled = false;
                self.load(ctx, s);
            }
            Act::Post => {
                let Some(cents) = parse_money(&self.amount) else {
                    return;
                };
                let post = PostBalance {
                    amount_cents: cents,
                    memo: self.memo.trim().to_string(),
                    expire_days: self.expire_on.then_some(self.expire_days.clamp(1, 365)),
                    notify: self.notify,
                    remind_days: self.remind_days,
                    assigned_staff_id: self.assigned.clone(),
                    hold_files: self.hold_files,
                };
                let hold = self.hold_files;
                // New unless it changes the due balance with the same amount.
                let is_new = !self.open().is_some_and(|b| b.amount_cents == cents);
                self.job = Some(spawn(ctx, move || {
                    Done::Posted(
                        core::client(&s2)
                            .post_balance(&id, &post)
                            .map(Box::new)
                            .map_err(|e| e.message()),
                        is_new,
                        hold,
                    )
                }));
            }
            Act::Clear => {
                self.job = Some(spawn(ctx, move || {
                    Done::Cleared(
                        core::client(&s2)
                            .clear_balance(&id)
                            .map_err(|e| e.message()),
                    )
                }));
            }
            Act::Remind => {
                self.job = Some(spawn(ctx, move || {
                    Done::Reminded(
                        core::client(&s2)
                            .remind_balance(&id)
                            .map_err(|e| e.message()),
                    )
                }));
            }
        }
    }
}

fn remind_words(n: u32) -> String {
    match n {
        0 => "Never".into(),
        n => format!("Every {n} days while it is due"),
    }
}

// ---- Admin › Portal › Balances ----------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    Due,
    Started,
    PaidThisMonth,
    ExpiredThisMonth,
    All,
}

impl Filter {
    const ALL: [Filter; 5] = [
        Filter::Due,
        Filter::Started,
        Filter::PaidThisMonth,
        Filter::ExpiredThisMonth,
        Filter::All,
    ];

    fn words(self) -> &'static str {
        match self {
            Filter::Due => "Due",
            Filter::Started => "Payment started",
            Filter::PaidThisMonth => "Paid this month",
            Filter::ExpiredThisMonth => "Expired this month",
            Filter::All => "All",
        }
    }

    /// Whether `b` is shown; `month` is this month (`yyyy-mm`).
    pub fn shows(self, b: &Balance, month: &str) -> bool {
        match self {
            Filter::Due => b.state == balance::DUE,
            Filter::Started => b.state == balance::STARTED,
            Filter::PaidThisMonth => b.paid_at.as_deref().and_then(|p| p.get(..7)) == Some(month),
            Filter::ExpiredThisMonth => {
                b.state == balance::EXPIRED && b.ended_month() == Some(month)
            }
            Filter::All => b.state != balance::REPLACED,
        }
    }
}

pub struct BalancesPage {
    filter: Filter,
    data: Option<Balances>,
    clients: HashMap<String, (ClientRef, Option<String>)>,
    job: Option<Receiver<Result<Balances, String>>>,
    loaded: bool,
    err: Option<String>,
    window: Option<BalanceWindow>,
    audit: Option<AuditOut>,
    reads: Option<ReadOut>,
}

impl Default for BalancesPage {
    fn default() -> Self {
        Self {
            filter: Filter::Due,
            data: None,
            clients: HashMap::new(),
            job: None,
            loaded: false,
            err: None,
            window: None,
            audit: None,
            reads: None,
        }
    }
}

impl BalancesPage {
    /// Read again when the page is opened.
    pub fn reread(&mut self) {
        self.loaded = false;
    }

    pub fn connected(&self) -> bool {
        self.audit.is_some()
    }

    /// Where the page's window sends audit rows and reads (the app's portal channels).
    pub fn connect(&mut self, audit: AuditOut, reads: ReadOut) {
        self.audit = Some(audit);
        self.reads = Some(reads);
    }
}

fn client_rows(env: &mut crate::ui::browse::Env) -> HashMap<String, (ClientRef, Option<String>)> {
    let Ok(mut stmt) = env
        .conn
        .prepare("SELECT id, ext_id, name, folder_rel FROM client WHERE deleted_at IS NULL")
    else {
        return HashMap::new();
    };
    stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            (
                ClientRef {
                    id: r.get(0)?,
                    code: r.get(1)?,
                    name: r.get(2)?,
                },
                r.get::<_, Option<String>>(3)?,
            ),
        ))
    })
    .map(|rows| rows.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

pub fn page(ui: &mut egui::Ui, page: &mut BalancesPage, env: &mut crate::ui::browse::Env) {
    ui.heading("Balances");
    let settings = vf_fs::cabinet::read(env.root)
        .ok()
        .and_then(|c| vf_portal::settings::load(env.root, &c.cabinet_id));
    let Some(s) = settings.filter(|s| s.balances) else {
        ui.label("Balance owed is off for this cabinet. Turn it on in Admin › Portal.");
        return;
    };
    ui.label(widgets::caption(
        "Balances posted on the portal, and those that ended in the last 45 days. A balance \
         paid or left unpaid past its date clears by itself; its receipt is filed in Billing.",
    ));
    let ctx = ui.ctx().clone();
    if let Some(rx) = &page.job
        && let Ok(r) = rx.try_recv()
    {
        page.job = None;
        match r {
            Ok(d) => {
                if let Some(o) = &page.reads {
                    o.send(d.balances.clone());
                }
                page.data = Some(d);
                page.err = None;
            }
            Err(e) => page.err = Some(e),
        }
    }
    if !page.loaded {
        page.loaded = true;
        page.clients = client_rows(env);
        let s2 = s.clone();
        page.job = Some(spawn(&ctx, move || {
            core::client(&s2).balances().map_err(|e| e.message())
        }));
    }
    ui.horizontal(|ui| {
        for f in Filter::ALL {
            ui.selectable_value(&mut page.filter, f, f.words());
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(page.job.is_none(), egui::Button::new("Read again"))
                .clicked()
            {
                page.loaded = false;
            }
            if page.job.is_some() {
                ui.spinner();
            }
        });
    });
    if let Some(e) = &page.err {
        ui.colored_label(theme::error(), e);
    }
    let month = chrono::Utc::now().format("%Y-%m").to_string();
    let mut open: Option<String> = None;
    if let Some(d) = &page.data {
        let rows: Vec<&Balance> = d
            .balances
            .iter()
            .filter(|b| page.filter.shows(b, &month))
            .collect();
        if rows.is_empty() {
            ui.label(widgets::caption("None."));
        }
        egui::Grid::new("balances-list")
            .num_columns(7)
            .striped(true)
            .spacing([14.0, 6.0])
            .show(ui, |ui| {
                for h in [
                    "Client",
                    "Amount",
                    "Memo",
                    "Assigned",
                    "Posted",
                    "Days left",
                    "State",
                ] {
                    ui.label(RichText::new(h).small().color(theme::ink_muted()));
                }
                ui.end_row();
                for b in rows {
                    let name = page
                        .clients
                        .get(&b.client_id)
                        .map(|(c, _)| format!("{} ({})", c.name, c.code))
                        .unwrap_or_else(|| "(not in this cabinet)".into());
                    if ui.link(&name).clicked() {
                        open = Some(b.client_id.clone());
                    }
                    ui.label(money(b.amount_cents));
                    ui.label(&b.memo);
                    ui.label(d.staff_name(b.assigned_staff_id.as_deref()));
                    ui.label(widgets::local_time(&b.posted_at));
                    ui.label(match (b.state.as_str(), b.days_left) {
                        (balance::DUE, Some(n)) => n.to_string(),
                        (balance::DUE, None) => "no limit".into(),
                        _ => String::new(),
                    });
                    ui.label(b.state_words());
                    ui.end_row();
                }
            });
    }
    if let Some(id) = open
        && let Some((c, folder)) = page.clients.get(&id).cloned()
        && let (Some(audit), Some(reads)) = (page.audit.clone(), page.reads.clone())
    {
        page.window = Some(BalanceWindow::new(
            &ctx,
            &s,
            env.root.to_path_buf(),
            c,
            folder,
            audit,
            reads,
        ));
    }
    if let Some(w) = &mut page.window {
        w.show(&ctx, &s);
        if w.closed {
            page.window = None;
            page.loaded = false;
        }
    }
}

/// The client panel's line for one client, from the balances last read on this PC.
pub fn panel_line(read: Option<&Balances>, client_id: &str) -> String {
    let Some(d) = read else {
        return "Balance owed: not read yet".into();
    };
    d.balances
        .iter()
        .filter(|b| b.client_id == client_id && b.state != balance::REPLACED)
        .find(|b| b.is_open())
        .map(|b| format!("Balance owed: {}", b.line()))
        .unwrap_or_else(|| "Balance owed: none".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(state: &str) -> Balance {
        serde_json::from_value(serde_json::json!({
            "id": format!("b-{state}"), "clientId": "c1", "amountCents": 24000, "memo": "m",
            "state": state, "expiresAt": null, "daysLeft": null, "notify": true, "remindDays": 0,
            "remindedAt": null, "assignedStaffId": null, "postedBy": "x",
            "postedAt": "2026-11-24T12:00:00Z", "startedAt": null,
            "paidAt": if state == "paid" { serde_json::json!("2026-11-25T10:00:00Z") } else { serde_json::Value::Null },
            "method": null, "cardFeeCents": 0,
            "endedAt": if state == "expired" { serde_json::json!("2026-11-26T10:00:00Z") } else { serde_json::Value::Null },
            "endedBy": null, "clearedPending": false, "receiptUploadId": null
        }))
        .unwrap()
    }

    /// §16.3: a balance read is copied into its client's folder only when it changed; paid is
    /// recorded once (whichever PC sees it first); a folder bound to another client gets none.
    #[test]
    fn balances_read_are_copied_once_and_paid_is_recorded_once() {
        const C1: &str = "018f0000-0000-7000-8000-000000000001";
        const C2: &str = "018f0000-0000-7000-8000-000000000002";
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let bind = |folder: &str, client: &str| {
            let v = root.join(folder).join("_Vibe");
            std::fs::create_dir_all(&v).unwrap();
            std::fs::write(
                v.join("client.json"),
                format!(
                    r#"{{"version":1,"client_id":"{client}","firm_id":"{C2}","tax_software_id":"1","display_name_at_creation":"x","created_at":"2026-11-01T00:00:00Z","created_by":null}}"#
                ),
            )
            .unwrap();
        };
        bind("Smith, John (1001)", C1);
        // Client 2's catalog folder is bound to client 1: no copy goes there.
        bind("Elsewhere (1002)", C1);
        let mut conn =
            vf_core::db::open(&t.path().join("catalog.db"), vf_core::db::Storage::Local).unwrap();
        for (id, ext, folder) in [
            (C1, "1001", "Smith, John (1001)"),
            (C2, "1002", "Elsewhere (1002)"),
        ] {
            conn.execute(
                "INSERT INTO client (id, ext_id, name, folder_rel, device_id, updated_at)
                 VALUES (?1, ?2, 'x', ?3, 'dev', '2026-11-01T00:00:00Z')",
                [id, ext, folder],
            )
            .unwrap();
        }
        let mut due = b("due");
        due.client_id = C1.into();
        let mut other = b("due");
        other.id = "b-other".into();
        other.client_id = C2.into();
        let audits = |conn: &vf_core::db::Connection, action: &str| -> i64 {
            conn.query_row(
                "SELECT count(*) FROM audit WHERE action = ?1",
                [action],
                |r| r.get(0),
            )
            .unwrap()
        };
        record_reads(&mut conn, root, "dev-1", vec![vec![due.clone(), other]]).unwrap();
        let path = root.join("Smith, John (1001)/_Vibe/balance.jsonl");
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 1);
        assert!(!root.join("Elsewhere (1002)/_Vibe/balance.jsonl").exists());
        // Read again unchanged (the days left count down): nothing new.
        let mut tomorrow = due.clone();
        tomorrow.days_left = Some(3);
        record_reads(&mut conn, root, "dev-1", vec![vec![tomorrow]]).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 1);
        assert_eq!(audits(&conn, "portal_balance_copy"), 1);
        // Paid: a line and one paid row, though two PCs read it.
        let mut paid = due.clone();
        paid.state = vf_portal::balance::PAID.into();
        paid.paid_at = Some("2026-11-25T10:00:00Z".into());
        record_reads(&mut conn, root, "dev-1", vec![vec![paid.clone()]]).unwrap();
        record_reads(&mut conn, root, "dev-2", vec![vec![paid]]).unwrap();
        assert_eq!(audits(&conn, "portal_balance_paid"), 1);
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 2);
        // Field names only in the audit trail, never the amount or memo.
        let detail: String = conn
            .query_row(
                "SELECT detail FROM audit WHERE action = 'portal_balance_paid'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            !detail.contains("24000") && !detail.contains("\"m\""),
            "{detail}"
        );
    }

    #[test]
    fn filters_and_the_panel_line() {
        let m = "2026-11";
        assert!(Filter::Due.shows(&b("due"), m));
        assert!(!Filter::Due.shows(&b("started"), m));
        assert!(Filter::PaidThisMonth.shows(&b("paid"), m));
        assert!(!Filter::PaidThisMonth.shows(&b("paid"), "2026-12"));
        assert!(Filter::ExpiredThisMonth.shows(&b("expired"), m));
        assert!(!Filter::All.shows(&b("replaced"), m));
        let d = Balances {
            balances: vec![b("paid"), b("due")],
            defaults: Defaults::default(),
            staff: vec![],
        };
        assert_eq!(
            panel_line(Some(&d), "c1"),
            "Balance owed: $240.00 on the portal"
        );
        assert_eq!(panel_line(Some(&d), "c2"), "Balance owed: none");
        assert_eq!(panel_line(None, "c1"), "Balance owed: not read yet");
    }
}
