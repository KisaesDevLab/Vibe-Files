//! Admin › Portal › Invite from client list (PA-105, owner 2026-11-24).
//!
//! The clients' stored emails and cells (`vf_fs::contacts`: the CSV's, or typed) beside who is
//! on the portal already (Vibe-Portal P111 `GET /contacts`, one read): staff tick the people to
//! invite and press one button. Invitations go out in groups (20 by default) with a pause
//! between them, so the firm's email provider does not take them for spam (owner 2026-11-24).
//! Vibe Files never changes a login on the portal: someone whose contact differs is shown, not
//! changed.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::RichText;
use vf_core::audit::AuditEntry;
use vf_fs::contacts::{ClientContacts, Invitee};
use vf_portal::PortalSettings;
use vf_portal::api::{Portal, PortalContacts};

use crate::portal as core;
use crate::ui::portal::{AuditOut, spawn};
use crate::ui::{theme, widgets};

/// Invitations sent before each pause (owner 2026-11-24).
pub const GROUP: usize = 20;
/// The pause between groups, in seconds (changeable on the page).
pub const PAUSE_SECS: u32 = 60;
/// Clients drawn at most; the rest are found by Search.
const DRAWN: usize = 300;

/// Where one person of the CSV stands on the portal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Not on the portal: invited when ticked (ticked to start).
    Invite,
    /// Their invitation expired: a new one is sent when ticked (ticked to start).
    InviteAgain,
    /// The client already has other people on the portal: maybe this is one of them with a
    /// new address. Not ticked to start; the words say who is there.
    Check(String),
    OnPortal,
    /// Invited, not accepted yet (not sent again: Share or Portal people resends).
    Waiting,
    /// On the portal with their access to this client turned off.
    AccessOff,
    /// On the portal with their whole account turned off.
    AccountOff,
    /// The client is switched off on the portal.
    ClientOff,
}

impl Status {
    pub fn sendable(&self) -> bool {
        matches!(
            self,
            Status::Invite | Status::InviteAgain | Status::Check(_)
        )
    }

    fn ticked_to_start(&self) -> bool {
        matches!(self, Status::Invite | Status::InviteAgain)
    }

    fn words(&self) -> String {
        match self {
            Status::Invite => String::new(),
            Status::InviteAgain => "invitation expired: a new one is sent".into(),
            Status::Check(w) => w.clone(),
            Status::OnPortal => "on the portal".into(),
            Status::Waiting => "invited, not accepted yet".into(),
            Status::AccessOff => "on the portal, access to this client turned off".into(),
            Status::AccountOff => "on the portal, account turned off".into(),
            Status::ClientOff => "the client is switched off on the portal".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub invitee: Invitee,
    pub status: Status,
}

/// One client: what the CSV says, and each person's place on the portal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientLine {
    pub c: ClientContacts,
    /// Someone can sign in to this client on the portal now.
    pub has_people: bool,
    pub lines: Vec<Line>,
}

fn same(a: &Option<String>, b: &Option<String>) -> bool {
    a.is_some() && a == b
}

/// Puts the CSV's people for a client beside the portal's (`None`: the client is not on the
/// portal). Contacts are compared as the portal stores them, and a person is matched by
/// contact, never by name: a new address for someone with a login is shown, not invited.
pub fn compare(c: ClientContacts, p: Option<&PortalContacts>) -> ClientLine {
    let people: Vec<_> = p
        .map(|p| {
            p.people
                .iter()
                .filter(|x| x.active && !x.account_off)
                .collect()
        })
        .unwrap_or_default();
    let lines = c
        .planned
        .people
        .iter()
        .map(|inv| {
            let status = match p {
                Some(p) if !p.active => Status::ClientOff,
                Some(p) => {
                    let person = p
                        .people
                        .iter()
                        .find(|x| same(&inv.email, &x.email) || same(&inv.phone, &x.phone));
                    // The invitation is found by the contact it went to: the email, else the
                    // number.
                    let invite = p.invitations.iter().find(|i| match &inv.email {
                        Some(_) => same(&inv.email, &i.email),
                        None => i.email.is_none() && same(&inv.phone, &i.phone),
                    });
                    match (person, invite) {
                        (Some(x), _) if x.account_off => Status::AccountOff,
                        (Some(x), _) if !x.active => Status::AccessOff,
                        (Some(_), _) => Status::OnPortal,
                        (None, Some(i)) if i.status == "expired" => Status::InviteAgain,
                        (None, Some(_)) => Status::Waiting,
                        (None, None) if !people.is_empty() => {
                            let named = people
                                .iter()
                                .find(|x| x.name.trim().eq_ignore_ascii_case(inv.name.trim()));
                            let contact = |x: &vf_portal::api::ContactPerson| {
                                x.email.clone().or(x.phone.clone()).unwrap_or_default()
                            };
                            Status::Check(match named {
                                Some(x) => format!(
                                    "the portal has {} with {}: the CSV differs. Tick only to invite this address too",
                                    x.name,
                                    contact(x)
                                ),
                                None => format!(
                                    "the portal already has {}: tick only if {} is someone new",
                                    people
                                        .iter()
                                        .map(|x| format!("{} ({})", x.name, contact(x)))
                                        .collect::<Vec<_>>()
                                        .join(", "),
                                    inv.name
                                ),
                            })
                        }
                        (None, None) => Status::Invite,
                    }
                }
                None => Status::Invite,
            };
            Line {
                invitee: inv.clone(),
                status,
            }
        })
        .collect();
    ClientLine {
        has_people: !people.is_empty(),
        c,
        lines,
    }
}

/// One invitation to send.
#[derive(Debug, Clone)]
pub struct Item {
    pub client_id: String,
    pub code: String,
    pub client: String,
    pub invitee: Invitee,
}

/// What became of one invitation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    pub client: String,
    pub who: String,
    /// Ok(by email); Err((portal's code, plain words)).
    pub result: Result<bool, (String, String)>,
}

/// How a run is going (shared with its thread).
#[derive(Debug, Default)]
pub struct Progress {
    pub total: usize,
    pub sent: Vec<Sent>,
    /// While pausing between groups: when the next group goes.
    pub next_group_at: Option<Instant>,
    pub finished: bool,
    pub stopped: bool,
}

/// Sends the invitations in groups of `group` with `pause` between groups (none after the
/// last), putting each client on the portal first. A failure is kept and the rest go on.
/// `stop` ends it between two invitations, or during a pause.
#[allow(clippy::too_many_arguments)]
pub fn send_all(
    p: &Portal,
    items: &[Item],
    group: usize,
    pause: Duration,
    progress: &Mutex<Progress>,
    stop: &AtomicBool,
    audit: &AuditOut,
    repaint: &dyn Fn(),
) {
    let group = group.max(1);
    // Read again just before sending: the page may be old, or another PC may have invited some
    // of them meanwhile; an invitation sent again would replace theirs (review 2026-11-24).
    let mut send: Vec<&Item> = Vec::new();
    match p.contacts() {
        Ok(fresh) => {
            for it in items {
                match already(&fresh, it) {
                    Some((code, words)) => progress.lock().unwrap().sent.push(Sent {
                        client: it.client.clone(),
                        who: it.invitee.name.clone(),
                        result: Err((code.into(), words.into())),
                    }),
                    None => send.push(it),
                }
            }
        }
        // Not read: sent as the page saw them (the portal still refuses someone with access).
        Err(_) => send.extend(items.iter()),
    }
    repaint();
    let mut put = BTreeSet::new();
    for (i, it) in send.into_iter().enumerate() {
        if i > 0 && i % group == 0 && !pause.is_zero() {
            let until = Instant::now() + pause;
            progress.lock().unwrap().next_group_at = Some(until);
            repaint();
            while Instant::now() < until && !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(50).min(pause));
            }
            progress.lock().unwrap().next_group_at = None;
        }
        if stop.load(Ordering::Relaxed) {
            progress.lock().unwrap().stopped = true;
            break;
        }
        let inv = &it.invitee;
        let result = (if put.contains(&it.client_id) {
            Ok(())
        } else {
            p.put_client(&it.client_id, &it.code, &it.client, None)
                .inspect(|_| {
                    put.insert(it.client_id.clone());
                })
        })
        .and_then(|_| {
            p.invite(
                &it.client_id,
                inv.email.as_deref(),
                inv.phone.as_deref(),
                &inv.name,
                &inv.role,
            )
        });
        let result = match result {
            Ok(_) => {
                let mut e =
                    crate::ui::portal::invite_audit(&it.client_id, inv.email.is_some(), &inv.role);
                e.detail["from_client_list"] = serde_json::json!(true);
                audit.send(e);
                Ok(inv.email.is_some())
            }
            Err(e) => Err((e.code().unwrap_or("").to_string(), e.message())),
        };
        progress.lock().unwrap().sent.push(Sent {
            client: it.client.clone(),
            who: inv.name.clone(),
            result,
        });
        repaint();
    }
    progress.lock().unwrap().finished = true;
    repaint();
}

/// Why someone ticked is not invited after all, by the portal as read just before sending:
/// on it already (by email or cell), or invited and not accepted yet (an expired invitation is
/// sent again).
fn already(fresh: &[PortalContacts], it: &Item) -> Option<(&'static str, &'static str)> {
    let c = fresh
        .iter()
        .find(|c| c.id.eq_ignore_ascii_case(&it.client_id))?;
    let inv = &it.invitee;
    if c.people
        .iter()
        .any(|x| same(&inv.email, &x.email) || same(&inv.phone, &x.phone))
    {
        return Some(("already_has_access", "already on the portal"));
    }
    let waiting = c.invitations.iter().any(|i| {
        i.status == "invited"
            && match &inv.email {
                Some(_) => same(&inv.email, &i.email),
                None => i.email.is_none() && same(&inv.phone, &i.phone),
            }
    });
    waiting.then_some(("already_invited", "invited already, not accepted yet"))
}

/// The end of a run in a few words: "52 invited (47 by email, 5 by text) · 3 already on the
/// portal · 2 not sent".
pub fn summary(p: &Progress) -> String {
    let email = p.sent.iter().filter(|s| s.result == Ok(true)).count();
    let text = p.sent.iter().filter(|s| s.result == Ok(false)).count();
    let already = p
        .sent
        .iter()
        .filter(|s| {
            matches!(&s.result, Err((c, _)) if c == "already_has_access" || c == "already_invited")
        })
        .count();
    let failed = p.sent.iter().filter(|s| s.result.is_err()).count() - already;
    let mut parts = vec![format!(
        "{} invited ({email} by email, {text} by text)",
        email + text
    )];
    if already > 0 {
        parts.push(format!("{already} on the portal or invited already"));
    }
    if failed > 0 {
        parts.push(format!("{failed} not sent"));
    }
    let left = p.total.saturating_sub(p.sent.len());
    if left > 0 {
        parts.push(format!("{left} left when stopped"));
    }
    parts.join(" · ")
}

struct Run {
    progress: Arc<Mutex<Progress>>,
    stop: Arc<AtomicBool>,
}

/// The page or the cabinet closed while sending: the rest are not sent (review 2026-11-24).
impl Drop for Run {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// The clients with contacts, and how many have none.
type Loaded = Result<(Vec<ClientLine>, usize, String), String>;

pub struct InvitePage {
    settings: Option<PortalSettings>,
    read_at: Option<Instant>,
    loading: Option<Receiver<Loaded>>,
    rows: Option<Vec<ClientLine>>,
    /// Clients of the list with no email or cell.
    without: usize,
    /// The firm's name on the portal (What they receive).
    firm: String,
    /// Ticked people: (client id, index in its lines).
    ticked: BTreeSet<(String, usize)>,
    show_all: bool,
    partner: Option<String>,
    find: String,
    pub group: usize,
    pub pause_secs: u32,
    /// One second of pause (tests make it shorter).
    pub second: Duration,
    run: Option<Run>,
    /// The last run's end, kept on screen.
    report: Option<(String, Vec<Sent>)>,
    err: Option<String>,
    audit: AuditOut,
    audit_rx: Receiver<AuditEntry>,
    /// The app's portal channel (recorded on any page).
    pub app_audit: Option<AuditOut>,
}

impl Default for InvitePage {
    fn default() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self {
            settings: None,
            read_at: None,
            loading: None,
            rows: None,
            without: 0,
            firm: String::new(),
            ticked: BTreeSet::new(),
            show_all: false,
            partner: None,
            find: String::new(),
            group: GROUP,
            pause_secs: PAUSE_SECS,
            second: Duration::from_secs(1),
            run: None,
            report: None,
            err: None,
            audit: AuditOut::new(tx),
            audit_rx: rx,
            app_audit: None,
        }
    }
}

fn load_job(ctx: &egui::Context, s: PortalSettings, root: PathBuf) -> Receiver<Loaded> {
    spawn(ctx, move || {
        let (csv, none) = vf_fs::contacts::read(&root)?;
        let portal = core::client(&s).contacts().map_err(|e| {
            if e.status() == Some(404) {
                "This portal is too old to invite from the client list: update it to 0.5.2 or later."
                    .to_string()
            } else {
                e.message()
            }
        })?;
        let rows = csv
            .into_iter()
            .map(|c| {
                let p = portal
                    .iter()
                    .find(|p| p.id.eq_ignore_ascii_case(&c.client_id));
                compare(c, p)
            })
            .collect();
        // The firm's name as the portal writes it in invitations (for What they receive).
        let firm = core::client(&s)
            .status()
            .map(|st| st.firm_name)
            .unwrap_or_default();
        Ok((rows, none, firm))
    })
}

impl InvitePage {
    /// Read the contacts and the portal again (the page was entered, or Read again).
    pub fn reread(&mut self) {
        if self.run.is_none() {
            self.rows = None;
            self.loading = None;
            self.read_at = None;
        }
    }

    fn shown(&self, r: &ClientLine) -> bool {
        if !self.show_all && r.has_people {
            return false;
        }
        if let Some(p) = &self.partner
            && r.c.partner.as_deref() != Some(p.as_str())
        {
            return false;
        }
        let f = self.find.trim().to_lowercase();
        f.is_empty()
            || r.c.name.to_lowercase().contains(&f)
            || r.c.code.to_lowercase().contains(&f)
            || r.lines.iter().any(|l| {
                // A cell is found by its digits however it is typed ("201-0002").
                let digits: String = f.chars().filter(char::is_ascii_digit).collect();
                l.invitee.name.to_lowercase().contains(&f)
                    || l.invitee.by().contains(&f)
                    || (digits.len() >= 3
                        && l.invitee
                            .phone
                            .as_deref()
                            .is_some_and(|p| p.contains(&digits)))
            })
    }

    /// How many people are ticked, at how many clients, of those shown (counted, not copied:
    /// every frame).
    fn counts(&self) -> (usize, usize) {
        let Some(rows) = &self.rows else {
            return (0, 0);
        };
        let (mut people, mut clients) = (0, 0);
        for r in rows.iter().filter(|r| self.shown(r)) {
            let n = r
                .lines
                .iter()
                .enumerate()
                .filter(|(i, l)| {
                    l.status.sendable() && self.ticked.contains(&(r.c.client_id.clone(), *i))
                })
                .count();
            people += n;
            clients += usize::from(n > 0);
        }
        (people, clients)
    }

    /// The first ticked person of the clients shown (found, not the whole list copied).
    fn first_item(&self) -> Option<Item> {
        let rows = self.rows.as_ref()?;
        rows.iter().filter(|r| self.shown(r)).find_map(|r| {
            r.lines.iter().enumerate().find_map(|(i, l)| {
                (l.status.sendable() && self.ticked.contains(&(r.c.client_id.clone(), i))).then(
                    || Item {
                        client_id: r.c.client_id.clone(),
                        code: r.c.code.clone(),
                        client: r.c.name.clone(),
                        invitee: l.invitee.clone(),
                    },
                )
            })
        })
    }

    /// The ticked people of the clients shown, in list order.
    fn items(&self) -> Vec<Item> {
        let Some(rows) = &self.rows else {
            return Vec::new();
        };
        rows.iter()
            .filter(|r| self.shown(r))
            .flat_map(|r| {
                r.lines
                    .iter()
                    .enumerate()
                    .filter(move |(i, l)| {
                        l.status.sendable() && self.ticked.contains(&(r.c.client_id.clone(), *i))
                    })
                    .map(move |(_, l)| Item {
                        client_id: r.c.client_id.clone(),
                        code: r.c.code.clone(),
                        client: r.c.name.clone(),
                        invitee: l.invitee.clone(),
                    })
            })
            .collect()
    }

    fn start(&mut self, ctx: &egui::Context, s: &PortalSettings) {
        let items = self.items();
        if items.is_empty() {
            return;
        }
        let progress = Arc::new(Mutex::new(Progress {
            total: items.len(),
            ..Default::default()
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let (p2, s2) = (progress.clone(), stop.clone());
        let audit = self.app_audit.clone().unwrap_or_else(|| self.audit.clone());
        let portal = core::client(s);
        let group = self.group;
        let pause = self.second * self.pause_secs;
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            send_all(&portal, &items, group, pause, &p2, &s2, &audit, &|| {
                ctx2.request_repaint()
            });
        });
        self.report = None;
        self.err = None;
        self.run = Some(Run { progress, stop });
    }
}

/// "What they receive": the invitation `it` gets, as the portal sends it.
fn what_they_receive(ui: &mut egui::Ui, firm: &str, url: &str, it: &Item) {
    let firm = if firm.trim().is_empty() {
        "Your accounting firm"
    } else {
        firm
    };
    let link = format!("{}/invite#t=…", url.trim_end_matches('/'));
    let (subject, body) = vf_portal::api::invitation_words(firm, &it.client, &link);
    egui::CollapsingHeader::new(format!("What they receive (for {})", it.invitee.name))
        .id_salt("invite-what-they-receive")
        .show(ui, |ui| {
            if it.invitee.email.is_some() {
                ui.label(RichText::new(format!("Email · {subject}")).strong());
                ui.label(body);
                ui.label(widgets::caption(
                    "A cell given with the email is kept for them to confirm when they accept; the invitation itself goes by email.",
                ));
            } else {
                ui.label(RichText::new("Text message").strong());
                ui.label(vf_portal::api::text_message(&body));
            }
        });
}

/// Draws the page.
pub fn page(ui: &mut egui::Ui, page: &mut InvitePage, env: &mut crate::ui::browse::Env) {
    let rows: Vec<AuditEntry> = page.audit_rx.try_iter().collect();
    if let Some(e) = crate::ui::portal::record(env, "portal_invite_bulk", rows) {
        page.err = Some(e.trim().to_string());
    }
    if page
        .read_at
        .is_none_or(|t| t.elapsed() >= Duration::from_secs(30))
    {
        page.read_at = Some(Instant::now());
        page.settings = crate::ui::todo::cabinet_settings(env.root);
    }
    let ctx = ui.ctx().clone();
    ui.heading("Invite from client list");
    ui.label(widgets::caption(
        "The emails and cells are the clients' Portal contacts: from the client CSV at its last Import, or typed in a client's panel. \
         Tick who to invite and press Invite: each client is put on the portal and its people are sent an invitation. \
         A joint couple gets two logins, or one in both names when they share an email.",
    ));
    let Some(s) = page.settings.clone() else {
        ui.label("The portal is not set up for this cabinet: set it up in Admin › Portal first.");
        return;
    };
    // A run in progress, and its end.
    if let Some(run) = &page.run {
        let p = run.progress.lock().unwrap();
        if p.finished {
            let words = summary(&p);
            let sent = p.sent.clone();
            drop(p);
            page.run = None;
            page.report = Some((words, sent));
            page.ticked.clear();
            page.rows = None;
            page.loading = None;
        } else {
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.spinner();
                let mut t = format!("Sent {} of {}", p.sent.len(), p.total);
                if let Some(at) = p.next_group_at {
                    let left = at.saturating_duration_since(Instant::now()).as_secs();
                    t.push_str(&format!(" · next group in {}:{:02}", left / 60, left % 60));
                }
                ui.label(RichText::new(t).strong());
                if ui.button("Stop").clicked() {
                    run.stop.store(true, Ordering::Relaxed);
                }
            });
            ui.label(widgets::caption(
                "Keep Vibe Files open until it is done. If it is closed, the rest are not sent: open this page again and the people already invited show as waiting.",
            ));
            ctx.request_repaint_after(Duration::from_millis(250));
            return;
        }
    }
    if let Some((words, sent)) = &page.report {
        ui.add_space(4.0);
        ui.label(RichText::new(words).color(theme::health_ok()).strong());
        for x in sent {
            if let Err((code, w)) = &x.result
                && code != "already_has_access"
                && code != "already_invited"
            {
                ui.label(
                    RichText::new(format!("{} · {}: {w}", x.client, x.who)).color(theme::warn()),
                );
            }
        }
        ui.label(widgets::caption(
            "The portal hands each invitation to the email or text service as it is made; one that bounces is in the portal's own log.",
        ));
    }
    // Reading.
    if page.rows.is_none() && page.loading.is_none() {
        page.err = None;
        page.loading = Some(load_job(&ctx, s.clone(), env.root.to_path_buf()));
    }
    if let Some(rx) = &page.loading
        && let Ok(r) = rx.try_recv()
    {
        page.loading = None;
        match r {
            Ok((rows, without, firm)) => {
                page.without = without;
                page.firm = firm;
                page.ticked = rows
                    .iter()
                    .flat_map(|r| {
                        r.lines
                            .iter()
                            .enumerate()
                            .filter(|(_, l)| l.status.ticked_to_start())
                            .map(|(i, _)| (r.c.client_id.clone(), i))
                    })
                    .collect();
                page.rows = Some(rows);
            }
            Err(e) => {
                page.err = Some(e);
                page.rows = Some(Vec::new());
            }
        }
    }
    if page.loading.is_some() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(widgets::caption("Reading the contacts and the portal…"));
        });
        return;
    }
    ui.add_space(6.0);
    // Owner 2026-11-24: a search box for the list.
    ui.horizontal(|ui| {
        let l = ui.label("Search");
        ui.add(
            egui::TextEdit::singleline(&mut page.find)
                .hint_text("Client, ID, person, email or cell")
                .desired_width(320.0),
        )
        .labelled_by(l.id);
        if !page.find.is_empty()
            && ui
                .small_button("Clear")
                .on_hover_text("Clear the search")
                .clicked()
        {
            page.find.clear();
        }
    });
    ui.horizontal_wrapped(|ui| {
        let l = ui.label("Show");
        egui::ComboBox::from_id_salt("invite-show")
            .selected_text(if page.show_all {
                "Every client"
            } else {
                "Not on the portal yet"
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut page.show_all, false, "Not on the portal yet");
                ui.selectable_value(&mut page.show_all, true, "Every client");
            })
            .response
            .labelled_by(l.id);
        let partners: BTreeSet<String> = page
            .rows
            .iter()
            .flatten()
            .filter_map(|r| r.c.partner.clone())
            .collect();
        if !partners.is_empty() {
            let l = ui.label("Partner");
            egui::ComboBox::from_id_salt("invite-partner")
                .selected_text(page.partner.clone().unwrap_or_else(|| "All".into()))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut page.partner, None, "All");
                    for p in partners {
                        ui.selectable_value(&mut page.partner, Some(p.clone()), p);
                    }
                })
                .response
                .labelled_by(l.id);
        }
        if ui.button("Read again").clicked() {
            page.report = None;
            page.reread();
        }
    });
    ui.horizontal_wrapped(|ui| {
        let l = ui.label("Send in groups of");
        ui.add(egui::DragValue::new(&mut page.group).range(1..=100))
            .labelled_by(l.id);
        let l = ui.label("with a pause of");
        ui.add(egui::DragValue::new(&mut page.pause_secs).range(0..=3600))
            .labelled_by(l.id);
        ui.label("seconds between groups");
    });
    widgets::message_line(ui, page.err.as_deref());
    let Some(rows) = page.rows.take() else {
        return;
    };
    let shown: Vec<&ClientLine> = rows.iter().filter(|r| page.shown(r)).collect();
    let problems: Vec<(&ClientLine, &String)> = shown
        .iter()
        .flat_map(|r| r.c.planned.problems.iter().map(move |p| (*r, p)))
        .collect();
    ui.horizontal(|ui| {
        if ui.button("Tick all shown").clicked() {
            for r in &shown {
                for (i, l) in r.lines.iter().enumerate() {
                    if l.status.ticked_to_start() {
                        page.ticked.insert((r.c.client_id.clone(), i));
                    }
                }
            }
        }
        if ui.button("Untick all").clicked() {
            page.ticked.clear();
        }
        ui.label(widgets::caption(format!(
            "{} clients shown of {} with contacts; {} without an email or cell",
            shown.len(),
            rows.len(),
            page.without
        )));
    });
    if !problems.is_empty() {
        egui::CollapsingHeader::new(format!(
            "People in the CSV who can't be invited ({})",
            problems.len()
        ))
        .id_salt("invite-problems")
        .show(ui, |ui| {
            for (r, p) in &problems {
                ui.label(widgets::caption(format!(
                    "{} ({}): {p}",
                    r.c.name, r.c.code
                )));
            }
        });
    }
    ui.separator();
    // A long list is drawn in part (review 2026-11-24: 2,000 clients every frame); Search
    // narrows it, and Tick all shown and Invite take every client the filters show.
    if shown.len() > DRAWN {
        ui.label(widgets::caption(format!(
            "The first {DRAWN} of {} clients are listed: search to find others. Tick all shown and Invite take all {}.",
            shown.len(),
            shown.len()
        )));
    }
    for r in shown.iter().take(DRAWN) {
        if r.lines.is_empty() {
            continue;
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(format!("{} ({})", r.c.name, r.c.code)).strong());
            if let Some(p) = &r.c.partner {
                ui.label(widgets::caption(p));
            }
        });
        for (i, l) in r.lines.iter().enumerate() {
            let key = (r.c.client_id.clone(), i);
            let mut on = page.ticked.contains(&key);
            ui.horizontal_wrapped(|ui| {
                ui.add_space(16.0);
                let label = format!(
                    "{} · {} · {}",
                    l.invitee.name,
                    l.invitee.by(),
                    l.invitee.role
                );
                let c = ui.add_enabled(l.status.sendable(), egui::Checkbox::new(&mut on, label));
                if c.changed() {
                    if on {
                        page.ticked.insert(key.clone());
                    } else {
                        page.ticked.remove(&key);
                    }
                }
                let w = l.status.words();
                if !w.is_empty() {
                    let color = if matches!(l.status, Status::Check(_)) {
                        theme::warn()
                    } else {
                        theme::ink_muted()
                    };
                    ui.label(RichText::new(w).small().color(color));
                }
                if let Some(n) = &l.invitee.note {
                    ui.label(widgets::caption(n));
                }
            });
        }
    }
    // The list goes back before the ticked people are gathered from it (they were gathered
    // from an empty list: walkthrough 2026-11-24).
    drop(shown);
    page.rows = Some(rows);
    // What the first person ticked would receive (owner 2026-11-24: read only).
    if let Some(it) = page.first_item() {
        what_they_receive(ui, &page.firm, &s.url, &it);
    }
    let (people, clients) = page.counts();
    ui.separator();
    let label = format!(
        "Invite {} {} at {} {}",
        people,
        if people == 1 { "person" } else { "people" },
        clients,
        if clients == 1 { "client" } else { "clients" }
    );
    let go = ui.add_enabled(people > 0, widgets::primary_button(label));
    if people > page.group {
        let groups = people.div_ceil(page.group);
        let wait = (groups - 1) as u64 * page.pause_secs as u64;
        let pauses = if wait < 60 {
            format!("{wait} s")
        } else {
            format!("about {} min", wait.div_ceil(60))
        };
        ui.label(widgets::caption(format!(
            "{groups} groups of up to {}; {pauses} of pauses.",
            page.group
        )));
    }
    if go.clicked() {
        page.start(&ctx, &s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_fs::contacts::Planned;
    use vf_portal::api::{ContactPerson, WaitingInvite};

    fn inv(name: &str, email: Option<&str>, phone: Option<&str>) -> Invitee {
        Invitee {
            name: name.into(),
            email: email.map(String::from),
            phone: phone.map(String::from),
            role: "Taxpayer".into(),
            note: None,
        }
    }

    fn client(people: Vec<Invitee>) -> ClientContacts {
        ClientContacts {
            client_id: "c1".into(),
            code: "1042".into(),
            name: "Sample, John & Jane".into(),
            partner: None,
            planned: Planned {
                people,
                problems: vec![],
            },
            names: vec![],
        }
    }

    fn person(name: &str, email: Option<&str>, phone: Option<&str>) -> ContactPerson {
        ContactPerson {
            identity_id: name.into(),
            name: name.into(),
            email: email.map(String::from),
            phone: phone.map(String::from),
            role_label: String::new(),
            active: true,
            account_off: false,
        }
    }

    fn portal(people: Vec<ContactPerson>, invitations: Vec<WaitingInvite>) -> PortalContacts {
        PortalContacts {
            id: "c1".into(),
            active: true,
            people,
            invitations,
        }
    }

    fn statuses(l: &ClientLine) -> Vec<Status> {
        l.lines.iter().map(|x| x.status.clone()).collect()
    }

    #[test]
    fn a_client_not_on_the_portal_invites_everyone() {
        let l = compare(
            client(vec![
                inv("John", Some("john@example.test"), None),
                inv("Jane", None, Some("+15552010002")),
            ]),
            None,
        );
        assert_eq!(statuses(&l), vec![Status::Invite, Status::Invite]);
        assert!(!l.has_people);
    }

    /// Matched by contact (email or cell), never by name.
    #[test]
    fn people_on_the_portal_and_waiting_invitations_are_not_sent_again() {
        let waiting = |email: Option<&str>, phone: Option<&str>, status: &str| WaitingInvite {
            invitation_id: "i".into(),
            name: String::new(),
            email: email.map(String::from),
            phone: phone.map(String::from),
            role_label: String::new(),
            status: status.into(),
        };
        let l = compare(
            client(vec![
                inv("John", Some("john@example.test"), Some("+15552010001")),
                inv("Jane", Some("jane@example.test"), None),
                inv("Alex", None, Some("+15552010003")),
                inv("Pat", Some("pat@example.test"), None),
            ]),
            Some(&portal(
                vec![person("J. Sample", None, Some("+15552010001"))],
                vec![
                    waiting(Some("jane@example.test"), None, "invited"),
                    waiting(None, Some("+15552010003"), "expired"),
                ],
            )),
        );
        let s = statuses(&l);
        assert_eq!(s[0], Status::OnPortal);
        assert_eq!(s[1], Status::Waiting);
        assert_eq!(s[2], Status::InviteAgain);
        // Pat is not on the portal, but the client has someone: maybe Pat is them.
        assert!(matches!(&s[3], Status::Check(w) if w.contains("J. Sample (+15552010001)")));
        assert!(l.has_people);
    }

    #[test]
    fn a_new_address_for_someone_with_a_login_is_shown_not_ticked() {
        let l = compare(
            client(vec![inv(
                "John Sample",
                Some("john.new@example.test"),
                None,
            )]),
            Some(&portal(
                vec![person("John Sample", Some("john@example.test"), None)],
                vec![],
            )),
        );
        let Status::Check(w) = &l.lines[0].status else {
            panic!("{:?}", l.lines[0].status)
        };
        assert!(
            w.contains("john@example.test") && w.contains("the CSV differs"),
            "{w}"
        );
        assert!(!l.lines[0].status.ticked_to_start());
    }

    #[test]
    fn turned_off_people_and_clients_are_not_invited() {
        let mut off = person("John", Some("john@example.test"), None);
        off.account_off = true;
        let mut no_access = person("Jane", Some("jane@example.test"), None);
        no_access.active = false;
        let l = compare(
            client(vec![
                inv("John", Some("john@example.test"), None),
                inv("Jane", Some("jane@example.test"), None),
            ]),
            Some(&portal(vec![off, no_access], vec![])),
        );
        assert_eq!(statuses(&l), vec![Status::AccountOff, Status::AccessOff]);
        // Nobody who can sign in: the client counts as not on the portal yet.
        assert!(!l.has_people);
        let mut p = portal(vec![], vec![]);
        p.active = false;
        let l = compare(
            client(vec![inv("John", Some("john@example.test"), None)]),
            Some(&p),
        );
        assert_eq!(statuses(&l), vec![Status::ClientOff]);
    }

    #[test]
    fn the_summary_counts_what_happened() {
        let s = |r: Result<bool, (&str, &str)>| Sent {
            client: "c".into(),
            who: "w".into(),
            result: r.map_err(|(a, b)| (a.to_string(), b.to_string())),
        };
        let p = Progress {
            total: 6,
            sent: vec![
                s(Ok(true)),
                s(Ok(true)),
                s(Ok(false)),
                s(Err(("already_has_access", "x"))),
                s(Err(("invalid_email", "x"))),
            ],
            stopped: true,
            ..Default::default()
        };
        assert_eq!(
            summary(&p),
            "3 invited (2 by email, 1 by text) · 1 on the portal or invited already · 1 not sent · 1 left when stopped"
        );
    }
}
