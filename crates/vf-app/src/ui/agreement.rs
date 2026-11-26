//! Annual service agreements (ADDENDUM-annual-agreements.md, owner 2026-11-25; Vibe Portal
//! Addendum 03): the client panel's Service agreement… and End engagement… windows, and Admin ›
//! Portal › Agreements (send to many clients; the partners who sign for the firm).
//!
//! An agreement is a card on the client's portal: Sign your letters, then Set up payment (A8).
//! Its letters are the templates for the services it covers (the AICPA kit's and the firm's
//! own), each with the Terms and Conditions where it says (A5), signed by the taxpayer and
//! spouse or an officer by the client's entity type, the firm's partner printed (A7). Its fee is
//! a line per service, once or monthly (A10); monthly lines are a charge on the firm's Stripe
//! account until cancelled (A9). A letter with a `[…]` fill-in left is never sent (A17).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;

use egui::RichText;
use vf_core::audit::AuditEntry;
use vf_letters::partners::Partner;
use vf_letters::{Fields, Template};
use vf_portal::PortalSettings;
use vf_portal::agreement::{ServiceLine, Subscription};
use vf_portal::balance::{money, parse_money};
use vf_portal::delivery::{Delivery, DeliverySigner, Locks, NewDelivery, StepsIn};

use crate::portal as core;
use crate::ui::delivery::ReadOut;
use crate::ui::portal::{AuditOut, ClientRef, spawn};
use crate::ui::widgets::UiExt;
use crate::ui::{theme, widgets};

/// The agreement's year by default: next year from October, else this year.
pub fn default_year() -> i32 {
    use chrono::Datelike;
    let now = chrono::Local::now();
    now.year() + i32::from(now.month() >= 10)
}

/// Everything one agreement sends (made on the UI thread, sent on a worker).
#[derive(Clone)]
pub struct Plan {
    pub client: ClientRef,
    pub year: i32,
    pub title: String,
    pub individual: bool,
    pub services: Vec<ServiceLine>,
    pub letters: Vec<Template>,
    pub pay: bool,
    pub signers: Vec<DeliverySigner>,
    pub partner: Partner,
    pub firm: String,
    pub notify: bool,
    pub remind_days: u32,
    pub assigned: Option<String>,
}

impl Plan {
    /// The letters' tags: who signs, and every service (a block "only for" one is kept).
    pub fn context(&self) -> String {
        let mut c = vec![vf_portal::delivery::signer_class(self.individual).to_string()];
        c.extend(self.services.iter().map(|s| s.service.clone()));
        c.join("|")
    }

    /// The merge fields for this client and agreement (a letter's own `{Fee}` is set per letter).
    pub fn fields(&self) -> Fields {
        let mut f = Fields::new();
        let firsts: Vec<String> = self
            .signers
            .iter()
            .filter_map(|s| s.name.split_whitespace().next().map(String::from))
            .collect();
        let get = |role: &str| {
            self.signers
                .iter()
                .find(|s| s.role == role)
                .map(|s| s.name.clone())
                .unwrap_or_default()
        };
        let first = self.signers.first();
        let (once, month) = vf_portal::agreement::totals(&self.services);
        let services: Vec<&str> = self.services.iter().map(|s| s.service.as_str()).collect();
        for (k, v) in [
            ("Client name", self.client.name.clone()),
            ("Client ID", self.client.code.clone()),
            ("Signer first names", firsts.join(" and ")),
            ("Taxpayer name", get("taxpayer")),
            ("Spouse name", get("spouse")),
            (
                "Signer name",
                first.map(|s| s.name.clone()).unwrap_or_default(),
            ),
            (
                "Signer title",
                first.map(|s| s.title.clone()).unwrap_or_default(),
            ),
            // The returns it covers are last year's (ADDENDUM-annual-agreements.md §3).
            ("Tax year", (self.year - 1).to_string()),
            ("Next year", self.year.to_string()),
            ("Agreement year", self.year.to_string()),
            ("Return type", services.join(", ")),
            ("Covered returns", services.join(", ")),
            ("Services", services.join(", ")),
            (
                "Fee lines",
                vf_portal::agreement::fee_lines_text(&self.services),
            ),
            ("One-time fee", money(once)),
            ("Monthly fee", money(month)),
            ("Partner name", self.partner.name.clone()),
            ("Partner title", self.partner.title.clone()),
            ("Firm name", self.firm.clone()),
            (
                "Date",
                chrono::Local::now().format("%B %-d, %Y").to_string(),
            ),
        ] {
            if !v.is_empty() {
                f.insert(k.into(), v);
            }
        }
        f
    }

    /// A letter's `{Fee}`: its services' fees ("$1,850.00", "$450.00 a month").
    fn fee_for(&self, t: &Template) -> String {
        let mine: Vec<ServiceLine> = self
            .services
            .iter()
            .filter(|s| t.offered_for(&s.service) && !t.return_types.is_empty())
            .cloned()
            .collect();
        let pick = if mine.is_empty() {
            &self.services
        } else {
            &mine
        };
        let (once, month) = vf_portal::agreement::totals(pick);
        match (once, month) {
            (0, 0) => String::new(),
            (o, 0) => money(o),
            (0, m) => format!("{} a month", money(m)),
            (o, m) => format!("{} and {} a month", money(o), money(m)),
        }
    }

    /// The letters as sent (Terms and Conditions in place), or the fill-ins left (A17).
    pub fn laid(&self, all: &[Template]) -> Result<Vec<(Template, vf_letters::Laid)>, String> {
        let ctx = self.context();
        let mut left = Vec::new();
        let mut out = Vec::new();
        let no_terms = vf_letters::store::terms(all).is_none();
        for t in &self.letters {
            // QA 2026-11-25: a letter that puts the Terms and Conditions before its signatures is
            // not sent without them (the paragraph was dropped silently).
            if no_terms && vf_letters::wants_terms(t) {
                left.push(format!(
                    "{}: there is no Terms and Conditions template",
                    t.name
                ));
                continue;
            }
            let sent = vf_letters::store::as_sent(t, all);
            let f = sent.fill_ins(&ctx, None);
            if !f.is_empty() {
                left.push(format!("{}: {}", t.name, f.join(", ")));
                continue;
            }
            let mut fields = self.fields();
            let fee = self.fee_for(t);
            if !fee.is_empty() {
                fields.insert("Fee".into(), fee);
            }
            out.push((t.clone(), vf_letters::layout(&sent, &fields, &ctx, None)));
        }
        if left.is_empty() {
            Ok(out)
        } else {
            Err(format!(
                "Fill in the letter templates before sending (Admin › Portal › Letter templates). {}",
                left.join("; ")
            ))
        }
    }

    /// What can stop it being sent, said as a person would.
    pub fn problem(&self) -> Option<String> {
        if self.services.is_empty() {
            return Some("Tick at least one service.".into());
        }
        if self.letters.is_empty() && !self.pay {
            return Some("Tick a letter to sign, or Pay on the portal.".into());
        }
        let (once, month) = vf_portal::agreement::totals(&self.services);
        if self.pay && once + month == 0 {
            return Some("Give a service a one-time or monthly fee, or choose Sign only.".into());
        }
        if !self.letters.is_empty()
            && (self.signers.is_empty() || self.signers.iter().any(|s| s.name.trim().is_empty()))
        {
            return Some("Add who signs in the client's Portal contacts.".into());
        }
        if !self.letters.is_empty()
            && self
                .signers
                .iter()
                .skip(1)
                .any(|s| s.email.is_none() && s.phone.is_none())
        {
            return Some(
                "The spouse needs their own email or mobile (their code to sign goes there): add it here or in the client's Portal contacts.".into(),
            );
        }
        None
    }
}

/// Sends an agreement: the card, its fee lines, its letters and who signs them. QA 2026-11-25:
/// the card is held (nobody told) until everything is attached; when a step fails it is cancelled,
/// so the client never hears of a card with no letters or fee and Send can simply be tried again.
pub fn send(
    p: &vf_portal::Portal,
    plan: &Plan,
    laid: &[(Template, vf_letters::Laid)],
) -> Result<Delivery, vf_portal::PortalError> {
    let pay = plan.pay && {
        let (o, m) = vf_portal::agreement::totals(&plan.services);
        o + m > 0
    };
    let mut d = p.create_delivery(&NewDelivery {
        client_id: plan.client.id.clone(),
        tax_year: plan.year,
        return_type: vf_portal::agreement::AGREEMENT.into(),
        title: plan.title.clone(),
        steps: StepsIn {
            letters: !laid.is_empty(),
            fee: pay,
            ..Default::default()
        },
        locks: Locks::default(),
        assigned_staff_id: plan.assigned.clone(),
        notify: plan.notify,
        hold: true,
        remind_days: plan.remind_days,
        kind: Some("agreement".into()),
        covers: plan.services.iter().map(|s| s.service.clone()).collect(),
        return_year: Some(plan.year - 1),
    })?;
    let id = d.id.clone();
    let rest = (|| {
        if pay {
            d = p.set_fee(
                &d.id,
                "lines",
                &vf_portal::agreement::fee_lines(&plan.services),
            )?;
        }
        for (t, l) in laid {
            let blocks = serde_json::to_string(&l.blocks).expect("serializes");
            d = p.add_letter(
                &d.id,
                &t.name,
                &t.doc_type,
                &t.id,
                t.version,
                &blocks,
                &l.pdf,
            )?;
        }
        if !laid.is_empty() {
            d = p.set_signers(&d.id, &plan.signers)?;
        }
        p.ready_delivery(&d.id)?;
        Ok(d)
    })();
    if rest.is_err() {
        // Best effort: a card left behind says so on the Deliveries page and can be cancelled.
        let _ = p.cancel_delivery(&id);
    }
    rest
}

/// Who signs for a client, from its Portal contacts: the taxpayer and spouse, or one person
/// (with a title) for a business.
pub fn signers_for(root: &Path, client_id: &str, individual: bool) -> Vec<DeliverySigner> {
    let row = vf_fs::contacts::record(root, client_id)
        .map(|r| r.effective())
        .unwrap_or_default();
    let name = |a: &str, b: &str| format!("{} {}", a.trim(), b.trim()).trim().to_string();
    let opt = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
    let mut v = vec![DeliverySigner {
        id: String::new(),
        role: if individual { "taxpayer" } else { "officer" }.into(),
        name: name(&row.first, &row.last),
        title: String::new(),
        email: opt(&row.email),
        phone: opt(&row.cell),
    }];
    if individual && !row.spouse_first.trim().is_empty() {
        v.push(DeliverySigner {
            id: String::new(),
            role: "spouse".into(),
            name: name(
                &row.spouse_first,
                if row.spouse_last.trim().is_empty() {
                    &row.last
                } else {
                    &row.spouse_last
                },
            ),
            title: String::new(),
            email: opt(&row.spouse_email),
            phone: opt(&row.spouse_cell),
        });
    }
    v
}

/// The letters for these services: each letter template offered for one of them; letters for
/// every service (the consents) offered unticked. Ticked, per service: the firm's own letters
/// for it (saved, ticked in a new delivery), else the built-in kit or service letter, else a
/// sample ticked in a new delivery. So one engagement letter is ticked for each service.
pub fn letters_for(all: &[Template], services: &[String]) -> Vec<(Template, bool)> {
    let offered: Vec<&Template> = all
        .iter()
        .filter(|t| t.kind == vf_letters::LETTER)
        .filter(|t| t.return_types.is_empty() || services.iter().any(|s| t.offered_for(s)))
        .collect();
    let built_in = |t: &Template| t.id.starts_with("aicpa-") || t.id.starts_with("service-");
    let mut ticked: Vec<&str> = Vec::new();
    for s in services {
        let mine: Vec<&&Template> = offered
            .iter()
            .filter(|t| !t.return_types.is_empty() && t.offered_for(s))
            .collect();
        let pick: Vec<&&Template> = [
            mine.iter()
                .filter(|t| t.default_ticked && !t.sample)
                .copied()
                .collect::<Vec<_>>(),
            mine.iter().filter(|t| built_in(t)).copied().collect(),
            mine.iter().filter(|t| t.default_ticked).copied().collect(),
        ]
        .into_iter()
        .find(|v| !v.is_empty())
        .unwrap_or_default();
        ticked.extend(pick.iter().map(|t| t.id.as_str()));
    }
    offered
        .into_iter()
        .map(|t| (t.clone(), ticked.contains(&t.id.as_str())))
        .collect()
}

/// An agreement's service lines, from its fee lines (Fill from last year).
pub fn services_of(d: &Delivery) -> Vec<ServiceLine> {
    let mut out: Vec<ServiceLine> = d
        .covers
        .iter()
        .map(|s| ServiceLine {
            service: s.clone(),
            ..Default::default()
        })
        .collect();
    if let Some(f) = &d.fee {
        for l in &f.lines {
            let i = match out.iter().position(|s| s.service == l.name) {
                Some(i) => i,
                None => {
                    out.push(ServiceLine {
                        service: l.name.clone(),
                        ..Default::default()
                    });
                    out.len() - 1
                }
            };
            if l.every == "month" {
                out[i].month_cents += l.cents;
            } else {
                out[i].once_cents += l.cents;
            }
        }
    }
    out
}

/// The last agreement's services with the monthly amount the client is charged now: when staff
/// changed the charge since (Change from the next charge) and the agreement has one monthly
/// line, that line takes the charged amount. Several monthly lines are kept as they were, with
/// a note to check them.
pub fn services_now(
    last: &Delivery,
    live: Option<&Subscription>,
) -> (Vec<ServiceLine>, Option<String>) {
    let mut lines = services_of(last);
    let Some(live) = live.filter(|s| s.live()) else {
        return (lines, None);
    };
    let (_, month) = vf_portal::agreement::totals(&lines);
    if month == live.monthly_cents {
        return (lines, None);
    }
    let monthly: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.month_cents > 0)
        .map(|(i, _)| i)
        .collect();
    if let [i] = monthly[..] {
        lines[i].month_cents = live.monthly_cents;
        (lines, None)
    } else {
        (
            lines,
            Some(format!(
                "The client is charged {} a month now, not the {} of the last agreement: check the monthly fees.",
                money(live.monthly_cents),
                money(month)
            )),
        )
    }
}

fn dollars(cents: u64) -> String {
    if cents == 0 {
        String::new()
    } else {
        money(cents).trim_start_matches('$').to_string()
    }
}

// ======================================================================================
// The client panel's Service agreement… window.

enum Done {
    #[allow(clippy::type_complexity)]
    Loaded(
        Result<
            (
                Vec<Delivery>,
                Vec<Subscription>,
                String,
                Vec<vf_portal::balance::BalanceStaff>,
            ),
            vf_portal::PortalError,
        >,
    ),
    Saved(Result<Box<Delivery>, String>, &'static str),
    Said(Result<String, String>),
}

struct Row {
    service: String,
    on: bool,
    once: String,
    month: String,
}

pub struct AgreementWindow {
    pub closed: bool,
    client: ClientRef,
    root: PathBuf,
    individual: bool,
    agreements: Vec<Delivery>,
    subscriptions: Vec<Subscription>,
    /// A change to the monthly charge is in progress: read the charge back when it is done
    /// (review 2026-11-25: cleared at once, it vanished even when the portal refused).
    monthly_changing: bool,
    /// Read the client's agreements and charge again (after a change that went through).
    reload: bool,
    year: i32,
    title: String,
    rows: Vec<Row>,
    letters: Vec<(Template, bool)>,
    pay: bool,
    signers: Vec<DeliverySigner>,
    partners: Vec<Partner>,
    partner: usize,
    firm_name: String,
    staff: Vec<vf_portal::balance::BalanceStaff>,
    assigned: Option<String>,
    notify: bool,
    remind_days: u32,
    monthly: String,
    confirm_stop: bool,
    confirm_cancel: bool,
    job: Option<Receiver<Done>>,
    said: Option<String>,
    err: Option<String>,
    audit: AuditOut,
    reads: ReadOut,
    /// The letter templates, read when the window opens and on Read again (QA 2026-11-25: they
    /// were read from the cabinet every frame; a read error was taken as no templates).
    templates: Result<Vec<Template>, String>,
    /// The client's agreements were read: Send waits for it, so this year's is never missed.
    loaded: bool,
}

impl AgreementWindow {
    pub fn new(
        ctx: &egui::Context,
        s: &PortalSettings,
        root: PathBuf,
        client: ClientRef,
        audit: AuditOut,
        reads: ReadOut,
    ) -> Self {
        let _ = vf_letters::store::seed_if_missing(&root);
        let individual = !vf_fs::contacts::client_is_business(&root, &client.id);
        let partners = vf_letters::partners::load(&root);
        let mine = vf_fs::contacts::client_partner(&root, &client.id);
        let partner = vf_letters::partners::default_for(&partners, mine.as_deref())
            .and_then(|p| partners.iter().position(|x| x == p))
            .unwrap_or(0);
        let year = default_year();
        let rows = vf_portal::delivery::service_names(&root)
            .into_iter()
            .map(|service| Row {
                service,
                on: false,
                once: String::new(),
                month: String::new(),
            })
            .collect();
        let mut w = Self {
            closed: false,
            signers: signers_for(&root, &client.id, individual),
            client,
            root,
            individual,
            agreements: Vec::new(),
            subscriptions: Vec::new(),
            monthly_changing: false,
            reload: false,
            year,
            title: format!("Your {year} service agreement"),
            rows,
            letters: Vec::new(),
            pay: true,
            partners,
            partner,
            firm_name: String::new(),
            staff: Vec::new(),
            assigned: None,
            notify: true,
            remind_days: 7,
            monthly: String::new(),
            confirm_stop: false,
            confirm_cancel: false,
            job: None,
            said: None,
            err: None,
            audit,
            reads,
            templates: Err(String::new()),
            loaded: false,
        };
        w.read_templates();
        w.firm_name = vf_fs::cabinet::read(&w.root)
            .ok()
            .and_then(|c| c.name)
            .unwrap_or_default();
        w.offer_letters();
        w.load(ctx, s);
        w
    }

    fn read_templates(&mut self) {
        self.templates = vf_letters::store::load_all(&self.root);
    }

    /// The templates as read (none when they could not be read; Send says why).
    fn all(&self) -> &[Template] {
        self.templates.as_deref().unwrap_or(&[])
    }

    /// The agreement's year (the walkthroughs).
    #[cfg(test)]
    pub fn set_year(&mut self, year: i32) {
        self.year = year;
        self.title = format!("Your {year} service agreement");
    }

    fn load(&mut self, ctx: &egui::Context, s: &PortalSettings) {
        let s2 = s.clone();
        let id = self.client.id.clone();
        self.job = Some(spawn(ctx, move || {
            let p = core::client(&s2);
            Done::Loaded((|| {
                let status = p.status()?;
                if !status.has_agreements() {
                    return Err(vf_portal::PortalError::Local(
                        "This portal has no annual agreements: update it to Vibe Portal 0.5.5 or later, and turn on tax return delivery in its Settings.".into(),
                    ));
                }
                let mut ds = Vec::new();
                for d in p.agreements(&id)? {
                    ds.push(p.delivery(&d.id)?);
                }
                let subs = p.subscriptions(&id).unwrap_or_default();
                let firm = status.firm_name;
                let staff = p.delivery_staff(&id).unwrap_or_default();
                Ok((ds, subs, firm, staff))
            })())
        }));
    }

    fn ticked(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter(|r| r.on)
            .map(|r| r.service.clone())
            .collect()
    }

    /// The letters for the services ticked (what was ticked before stays ticked).
    fn offer_letters(&mut self) {
        let all = self.all().to_vec();
        let before: Vec<(String, bool)> = self
            .letters
            .iter()
            .map(|(t, on)| (t.id.clone(), *on))
            .collect();
        self.letters = letters_for(&all, &self.ticked())
            .into_iter()
            .map(|(t, on)| {
                let on = before
                    .iter()
                    .find(|(id, _)| *id == t.id)
                    .map_or(on, |(_, b)| *b);
                (t, on)
            })
            .collect();
    }

    /// This year's agreement (open or done), if sent.
    fn current(&self) -> Option<&Delivery> {
        self.agreements
            .iter()
            .find(|d| d.tax_year == self.year && d.status != "cancelled")
    }

    fn live(&self) -> Option<&Subscription> {
        self.subscriptions.iter().find(|s| s.live())
    }

    /// Fill from last year: the services and fees of the newest agreement before this year.
    fn fill_from_last(&mut self) {
        let Some(last) = self
            .agreements
            .iter()
            .filter(|d| d.tax_year < self.year)
            .max_by_key(|d| d.tax_year)
            .cloned()
        else {
            self.err = Some("This client has no earlier agreement.".into());
            return;
        };
        let (lines, note) = services_now(&last, self.live());
        if note.is_some() {
            self.err = note;
        }
        for r in &mut self.rows {
            match lines.iter().find(|l| l.service == r.service) {
                Some(l) => {
                    r.on = true;
                    r.once = dollars(l.once_cents);
                    r.month = dollars(l.month_cents);
                }
                None => {
                    r.on = false;
                    r.once.clear();
                    r.month.clear();
                }
            }
        }
        self.offer_letters();
    }

    fn plan(&self) -> Result<Plan, String> {
        let mut services = Vec::new();
        for r in self.rows.iter().filter(|r| r.on) {
            let cents = |s: &str, what: &str| -> Result<u64, String> {
                if s.trim().is_empty() {
                    Ok(0)
                } else {
                    parse_money(s)
                        .ok_or_else(|| format!("{}: {what} in dollars and cents", r.service))
                }
            };
            services.push(ServiceLine {
                service: r.service.clone(),
                once_cents: cents(&r.once, "the one-time fee")?,
                month_cents: cents(&r.month, "the monthly fee")?,
            });
        }
        Ok(Plan {
            client: self.client.clone(),
            year: self.year,
            title: self.title.trim().to_string(),
            individual: self.individual,
            services,
            letters: self
                .letters
                .iter()
                .filter(|(_, on)| *on)
                .map(|(t, _)| t.clone())
                .collect(),
            pay: self.pay,
            signers: self.signers.clone(),
            partner: self.partners.get(self.partner).cloned().unwrap_or_default(),
            firm: self.firm_name.clone(),
            notify: self.notify,
            remind_days: self.remind_days,
            assigned: self.assigned.clone(),
        })
    }

    fn take_done(&mut self) {
        let Some(rx) = &self.job else { return };
        let Ok(done) = rx.try_recv() else { return };
        self.job = None;
        match done {
            Done::Loaded(Ok((ds, subs, firm, staff))) => {
                self.reads.send(ds.clone());
                self.agreements = ds;
                self.subscriptions = subs;
                self.loaded = true;
                if !firm.is_empty() {
                    self.firm_name = firm;
                }
                self.staff = staff;
                if let Some(s) = self.live() {
                    self.monthly = dollars(s.monthly_cents);
                }
            }
            Done::Loaded(Err(e)) => self.err = Some(e.message()),
            Done::Saved(Ok(d), what) => {
                self.audit.send(AuditEntry::new(
                    what,
                    None,
                    serde_json::json!({
                        "client_id": d.client_id,
                        "delivery_id": d.id,
                        "kind": "agreement",
                        "year": d.tax_year,
                        "covers": d.covers,
                    }),
                ));
                self.reads.send(vec![(*d).clone()]);
                self.agreements.retain(|x| x.id != d.id);
                self.agreements.insert(0, *d);
                self.said = Some(match what {
                    "portal_agreement_send" => "Sent to the client.".into(),
                    "portal_agreement_cancel" => "The agreement was cancelled.".into(),
                    _ => "Saved.".into(),
                });
            }
            Done::Saved(Err(e), _) => self.err = Some(e),
            Done::Said(Ok(s)) => {
                self.said = Some(s);
                self.reload = std::mem::take(&mut self.monthly_changing);
            }
            Done::Said(Err(e)) => {
                self.err = Some(e);
                self.monthly_changing = false;
            }
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, s: &PortalSettings) {
        self.take_done();
        if std::mem::take(&mut self.reload) && self.job.is_none() {
            let said = self.said.clone();
            self.load(ctx, s);
            self.said = said;
        }
        let busy = self.job.is_some();
        let mut open_win = true;
        let mut act: Option<&'static str> = None;
        egui::Window::new(format!(
            "Service agreement — {} ({})",
            self.client.name, self.client.code
        ))
        .id(egui::Id::unique("portal-agreement"))
        .open(&mut open_win)
        .collapsible(false)
        .default_width(860.0)
        .default_height(760.0)
        .show(ctx, |ui| {
            act = self.actions(ui, busy);
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if let Some(a) = self.body(ui, busy) {
                        act = Some(a);
                    }
                });
        });
        if !open_win {
            self.closed = true;
        }
        if let Some(a) = act {
            self.apply(ctx, s, a);
        }
    }

    /// The window's buttons and messages, above the form (always in view).
    fn actions(&mut self, ui: &mut egui::Ui, busy: bool) -> Option<&'static str> {
        let mut act = None;
        let current = self.current().cloned();
        let sent = current.is_some();
        let can = !busy;
        let plan = self.plan();
        if let Some(e) = &self.err {
            ui.colored_label(theme::error(), e);
        }
        if let Some(s) = &self.said {
            ui.colored_label(theme::health_ok(), s);
        }
        let problem = plan
            .as_ref()
            .err()
            .cloned()
            .or_else(|| plan.as_ref().ok().and_then(Plan::problem));
        if !sent && let Some(p) = &problem {
            ui.label(RichText::new(p).color(theme::ink_muted()));
        }
        if !busy && !self.loaded {
            ui.label(
                RichText::new(
                    "Send waits until the client's agreements are read: press Read again.",
                )
                .color(theme::ink_muted()),
            );
        }
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(can, egui::Button::new("Read again"))
                .clicked()
            {
                act = Some("reload");
            }
            if ui
                .add_enabled(can && !sent, egui::Button::new("Fill from last year"))
                .clicked()
            {
                self.fill_from_last();
            }
            if ui
                .add_enabled(can && plan.is_ok(), egui::Button::new("Preview letters…"))
                .clicked()
            {
                act = Some("preview");
            }
            let open = current.as_ref().is_some_and(|d| d.status == "open");
            if ui
                .add_enabled(can && open, egui::Button::new("Remind now"))
                .clicked()
            {
                act = Some("remind");
            }
            if ui
                .add_enabled(
                    can && open,
                    egui::Button::new(RichText::new("Cancel agreement…").color(theme::error())),
                )
                .clicked()
            {
                self.confirm_cancel = true;
            }
            if ui
                .add_enabled(
                    can && self.loaded && !sent && problem.is_none(),
                    widgets::primary_button("Send to client"),
                )
                .clicked()
            {
                act = Some("send");
            }
        });
        if self.confirm_cancel {
            ui.horizontal_wrapped(|ui| {
                ui.label("Cancel this agreement? The card leaves the client's portal; a monthly charge already running is not stopped.");
                if ui.button("Cancel agreement").clicked() {
                    self.confirm_cancel = false;
                    act = Some("cancel");
                }
                if ui.button("Keep it").clicked() {
                    self.confirm_cancel = false;
                }
            });
        }
        ui.separator();
        act
    }

    fn body(&mut self, ui: &mut egui::Ui, busy: bool) -> Option<&'static str> {
        let mut act = None;
        if busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Working…");
            });
        }
        let current = self.current().cloned();
        let sent = current.is_some();
        let can = !busy;
        ui.horizontal_wrapped(|ui| {
            ui.label("Agreement year");
            let before = self.year;
            ui.add_enabled(can, egui::DragValue::new(&mut self.year).range(1990..=2100));
            if before != self.year && self.title == format!("Your {before} service agreement") {
                self.title = format!("Your {} service agreement", self.year);
            }
            ui.label(
                RichText::new(format!("(returns for {})", self.year - 1))
                    .small()
                    .color(theme::ink_muted()),
            );
            ui.label("Card title");
            ui.add_enabled(
                can && !sent,
                egui::TextEdit::singleline(&mut self.title).desired_width(260.0),
            );
            let (word, colour) = match current.as_ref().map(|d| d.status.as_str()) {
                Some("open") => ("Sent", theme::accent()),
                Some("complete") => ("Signed and set up", theme::health_ok()),
                _ => ("Not sent", theme::ink_muted()),
            };
            ui.label(RichText::new(word).strong().color(colour));
        });
        if let Some(d) = &current {
            ui.label(RichText::new(d.line()).color(theme::ink_secondary()));
            if let Some(f) = &d.fee {
                ui.label(RichText::new(f.payment_words()).color(theme::ink_secondary()));
            }
        }
        // The monthly charge (A9, A13): change it from the next charge, or stop it.
        if let Some(sub) = self.live().cloned() {
            ui.group(|ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Monthly charge").strong());
                    let colour = if sub.failed_at.is_some() {
                        theme::warn()
                    } else {
                        theme::ink_secondary()
                    };
                    ui.label(RichText::new(sub.words()).color(colour));
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("New monthly amount $");
                    widgets::named(
                        ui.add_enabled(
                            can,
                            egui::TextEdit::singleline(&mut self.monthly).desired_width(90.0),
                        ),
                        "New monthly amount",
                    );
                    if ui
                        .add_enabled(can, egui::Button::new("Change from the next charge"))
                        .on_hover_text("The client is told. A card's fee is added as the portal's Settings say.")
                        .clicked()
                    {
                        act = Some("change_monthly");
                    }
                    if ui
                        .add_enabled(can, egui::Button::new(RichText::new("Stop monthly charge…").color(theme::error())))
                        .clicked()
                    {
                        self.confirm_stop = true;
                    }
                });
                if self.confirm_stop {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Stop the monthly charge? No more payments are taken; the client is told.");
                        if ui.button("Stop it").clicked() {
                            self.confirm_stop = false;
                            act = Some("stop_monthly");
                        }
                        if ui.button("Keep it").clicked() {
                            self.confirm_stop = false;
                        }
                    });
                }
            });
        }
        if sent {
            ui.label(
                RichText::new("To change a sent agreement, cancel it and send a new one. Letters signed stay in its log.")
                    .small()
                    .color(theme::ink_muted()),
            );
        }
        // Who signs and who signs for the firm.
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Signed by").strong());
            for sg in &mut self.signers {
                widgets::named(
                    ui.add_enabled(
                        can && !sent,
                        egui::TextEdit::singleline(&mut sg.name).desired_width(150.0),
                    ),
                    &format!("{} name", vf_letters::role_words(&sg.role)),
                );
                if sg.role == "officer" {
                    ui.add_enabled(
                        can && !sent,
                        egui::TextEdit::singleline(&mut sg.title)
                            .hint_text("Title")
                            .desired_width(110.0),
                    );
                }
                // A second signer's code goes to their own email or mobile.
                let mut email = sg.email.clone().unwrap_or_default();
                if widgets::named(
                    ui.add_enabled(
                        can && !sent,
                        egui::TextEdit::singleline(&mut email)
                            .hint_text("Email")
                            .desired_width(170.0),
                    ),
                    &format!("{} email", vf_letters::role_words(&sg.role)),
                )
                .changed()
                {
                    sg.email = Some(email.trim().to_string()).filter(|e| !e.is_empty());
                }
            }
            ui.label(
                RichText::new(if self.individual {
                    "(taxpayer and spouse)"
                } else {
                    "(for the business)"
                })
                .small()
                .color(theme::ink_muted()),
            );
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Signed for the firm by");
            if self.partners.is_empty() {
                ui.label(
                    RichText::new("— add partners in Admin › Portal › Agreements")
                        .color(theme::warn()),
                );
            } else {
                ui.add_enabled_ui(can && !sent, |ui| {
                    egui::ComboBox::from_id_salt("agreement-partner")
                        .selected_text(
                            self.partners
                                .get(self.partner)
                                .map(|p| p.name.clone())
                                .unwrap_or_default(),
                        )
                        .show_ui(ui, |ui| {
                            for (i, p) in self.partners.iter().enumerate() {
                                ui.selectable_value(
                                    &mut self.partner,
                                    i,
                                    format!("{}, {}", p.name, p.title),
                                );
                            }
                        });
                });
            }
        });
        // Services and fees.
        ui.add_space(4.0);
        ui.label(
            RichText::new("SERVICES AND FEES")
                .small()
                .color(theme::ink_muted()),
        );
        let mut changed = false;
        // Every return and service as a tick; the fees for those ticked below.
        ui.horizontal_wrapped(|ui| {
            for r in &mut self.rows {
                if ui
                    .add_enabled(
                        can && !sent,
                        egui::Checkbox::new(&mut r.on, r.service.as_str()),
                    )
                    .changed()
                {
                    changed = true;
                }
            }
        });
        if self.rows.iter().any(|r| r.on) {
            egui::Grid::new("agreement-services")
                .num_columns(3)
                .striped(true)
                .min_col_width(80.0)
                .show(ui, |ui| {
                    ui.label("");
                    ui.label(RichText::new("One-time $").small());
                    ui.label(RichText::new("Monthly $").small());
                    ui.end_row();
                    for r in self.rows.iter_mut().filter(|r| r.on) {
                        ui.label(r.service.as_str());
                        widgets::named(
                            ui.add_enabled(
                                can && !sent,
                                egui::TextEdit::singleline(&mut r.once).desired_width(90.0),
                            ),
                            &format!("{} one-time fee", r.service),
                        );
                        widgets::named(
                            ui.add_enabled(
                                can && !sent,
                                egui::TextEdit::singleline(&mut r.month).desired_width(90.0),
                            ),
                            &format!("{} monthly fee", r.service),
                        );
                        ui.end_row();
                    }
                });
        }
        if changed {
            self.offer_letters();
        }
        let plan = self.plan();
        if let Ok(p) = &plan {
            let (o, m) = vf_portal::agreement::totals(&p.services);
            ui.label(format!(
                "Total {} one-time · {} a month",
                money(o),
                money(m)
            ));
        }
        ui.horizontal(|ui| {
            ui.label("Payment");
            ui.add_enabled_ui(can && !sent, |ui| {
                ui.radio_value(&mut self.pay, true, "Pay on the portal");
                ui.radio_value(&mut self.pay, false, "Sign only");
            });
        });
        // Letters.
        ui.add_space(4.0);
        ui.label(
            RichText::new("LETTERS TO SIGN")
                .small()
                .color(theme::ink_muted()),
        );
        let all: &[Template] = self.templates.as_deref().unwrap_or(&[]);
        let lctx = plan.as_ref().map(|p| p.context()).unwrap_or_default();
        for (t, on) in &mut self.letters {
            ui.horizontal_wrapped(|ui| {
                ui.add_enabled(can && !sent, egui::Checkbox::new(on, t.name.as_str()));
                let left = vf_letters::store::as_sent(t, all).fill_ins(&lctx, None);
                if !left.is_empty() {
                    ui.label(
                        RichText::new(format!("fill in: {}", left.join(", ")))
                            .small()
                            .color(theme::warn()),
                    );
                }
                if t.sample {
                    ui.label(RichText::new("Sample").small().color(theme::ink_muted()));
                }
            });
        }
        if self.letters.is_empty() {
            ui.label(RichText::new("Tick a service to see its letters.").color(theme::ink_muted()));
        }
        ui.label(
            RichText::new("The Terms and Conditions are part of each letter that says {Terms and Conditions}.")
                .small()
                .color(theme::ink_muted()),
        );
        ui.horizontal_wrapped(|ui| {
            ui.label("Assigned to");
            ui.add_enabled_ui(can && !sent, |ui| {
                egui::ComboBox::from_id_salt("agreement-staff")
                    .selected_text(
                        self.assigned
                            .as_ref()
                            .and_then(|id| self.staff.iter().find(|s| &s.id == id))
                            .map(|s| s.name.clone())
                            .unwrap_or_else(|| "Nobody (the admins)".into()),
                    )
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.assigned, None, "Nobody (the admins)");
                        for st in &self.staff {
                            ui.selectable_value(&mut self.assigned, Some(st.id.clone()), &st.name);
                        }
                    });
            });
            ui.add_enabled(
                can && !sent,
                egui::Checkbox::new(&mut self.notify, "Tell the client by email and text"),
            );
            ui.label("Remind every");
            ui.add_enabled(
                can && !sent,
                egui::DragValue::new(&mut self.remind_days).range(0..=60),
            );
            ui.label("days");
        });
        act
    }

    fn apply(&mut self, ctx: &egui::Context, s: &PortalSettings, act: &'static str) {
        self.err = None;
        self.said = None;
        let s2 = s.clone();
        match act {
            "reload" => {
                self.read_templates();
                self.load(ctx, s);
            }
            "preview" => {
                // Said as Send says it (review 2026-11-25: Preview did nothing, silently).
                let plan = match self.plan() {
                    Ok(p) => p,
                    Err(e) => {
                        self.err = Some(e);
                        return;
                    }
                };
                let all = self.all().to_vec();
                let ctxs = plan.context();
                let dir = crate::ui::letter_templates::preview_dir();
                let _ = std::fs::create_dir_all(&dir);
                for t in &plan.letters {
                    let mut f = plan.fields();
                    let fee = plan.fee_for(t);
                    if !fee.is_empty() {
                        f.insert("Fee".into(), fee);
                    }
                    let laid =
                        vf_letters::layout(&vf_letters::store::as_sent(t, &all), &f, &ctxs, None);
                    let p = dir.join(format!(
                        "{} - {}.pdf",
                        self.client.code,
                        vf_letters::store::safe_id(&t.id)
                    ));
                    match std::fs::write(&p, &laid.pdf) {
                        Ok(()) => {
                            if let Err(e) = crate::shell::run(&crate::shell::ShellAction::Open(p)) {
                                self.err = Some(e);
                            }
                        }
                        Err(e) => self.err = Some(e.to_string()),
                    }
                }
            }
            "send" => {
                let plan = match self.plan() {
                    Ok(p) => p,
                    Err(e) => {
                        self.err = Some(e);
                        return;
                    }
                };
                if let Some(p) = plan.problem() {
                    self.err = Some(p);
                    return;
                }
                let all = match &self.templates {
                    Ok(all) => all.clone(),
                    Err(e) => {
                        self.err = Some(format!("The letter templates cannot be read: {e}"));
                        return;
                    }
                };
                let laid = match plan.laid(&all) {
                    Ok(l) => l,
                    Err(e) => {
                        self.err = Some(e);
                        return;
                    }
                };
                self.job = Some(spawn(ctx, move || {
                    let p = core::client(&s2);
                    Done::Saved(
                        send(&p, &plan, &laid)
                            .map(Box::new)
                            .map_err(|e| e.message()),
                        "portal_agreement_send",
                    )
                }));
            }
            "remind" => {
                let Some(id) = self.current().map(|d| d.id.clone()) else {
                    return;
                };
                self.job = Some(spawn(ctx, move || {
                    Done::Said(
                        core::client(&s2)
                            .remind_delivery(&id)
                            .map(|n| format!("Reminded ({n} messages)."))
                            .map_err(|e| e.message()),
                    )
                }));
            }
            "cancel" => {
                let Some(id) = self.current().map(|d| d.id.clone()) else {
                    return;
                };
                self.job = Some(spawn(ctx, move || {
                    Done::Saved(
                        core::client(&s2)
                            .cancel_delivery(&id)
                            .map(Box::new)
                            .map_err(|e| e.message()),
                        "portal_agreement_cancel",
                    )
                }));
            }
            "change_monthly" => {
                let Some(sub) = self.live().cloned() else {
                    return;
                };
                let Some(cents) = parse_money(&self.monthly).filter(|c| *c >= 50) else {
                    self.err =
                        Some("The monthly amount: dollars and cents, at least $0.50.".into());
                    return;
                };
                let audit = self.audit.clone();
                let client = self.client.id.clone();
                self.job = Some(spawn(ctx, move || {
                    Done::Said(
                        core::client(&s2)
                            .change_subscription(&sub.id, cents)
                            .map(|n| {
                                audit.send(AuditEntry::new(
                                    "portal_monthly_change",
                                    None,
                                    serde_json::json!({
                                        "client_id": client,
                                        "subscription_id": n.id,
                                        "from_cents": sub.monthly_cents,
                                        "to_cents": n.monthly_cents,
                                    }),
                                ));
                                format!(
                                    "Changed to {} from the next charge; the client was told.",
                                    money(n.total_cents())
                                )
                            })
                            .map_err(|e| e.message()),
                    )
                }));
            }
            "stop_monthly" => {
                let Some(sub) = self.live().cloned() else {
                    return;
                };
                let audit = self.audit.clone();
                let client = self.client.id.clone();
                self.job = Some(spawn(ctx, move || {
                    Done::Said(
                        core::client(&s2)
                            .stop_subscription(&sub.id)
                            .map(|()| {
                                audit.send(AuditEntry::new(
                                    "portal_monthly_stop",
                                    None,
                                    serde_json::json!({ "client_id": client, "subscription_id": sub.id }),
                                ));
                                "The monthly charge was stopped; the client was told.".to_string()
                            })
                            .map_err(|e| e.message()),
                    )
                }));
            }
            _ => {}
        }
        // A change to the charge is read back once the portal has answered.
        if matches!(act, "change_monthly" | "stop_monthly") && self.job.is_some() {
            self.monthly_changing = true;
        }
    }
}

// ======================================================================================
// End engagement… (A12): the closure letter, read on the portal and filed; open agreements
// cancelled and the monthly charge stopped.

pub struct EndEngagementWindow {
    pub closed: bool,
    client: ClientRef,
    root: PathBuf,
    individual: bool,
    template: Option<Template>,
    blanks: Vec<(String, String)>,
    signers: Vec<DeliverySigner>,
    partners: Vec<Partner>,
    partner: usize,
    confirm: bool,
    job: Option<Receiver<Result<String, String>>>,
    said: Option<String>,
    err: Option<String>,
    audit: AuditOut,
}

impl EndEngagementWindow {
    pub fn new(root: PathBuf, client: ClientRef, audit: AuditOut) -> Self {
        let _ = vf_letters::store::seed_if_missing(&root);
        let template = vf_letters::store::load_all(&root)
            .unwrap_or_default()
            .into_iter()
            .find(|t| t.kind == vf_letters::CLOSURE);
        let individual = !vf_fs::contacts::client_is_business(&root, &client.id);
        let blanks = template
            .as_ref()
            .map(|t| {
                t.fill_ins(vf_portal::delivery::signer_class(individual), None)
                    .into_iter()
                    .map(|f| (f, String::new()))
                    .collect()
            })
            .unwrap_or_default();
        let partners = vf_letters::partners::load(&root);
        let mine = vf_fs::contacts::client_partner(&root, &client.id);
        let partner = vf_letters::partners::default_for(&partners, mine.as_deref())
            .and_then(|p| partners.iter().position(|x| x == p))
            .unwrap_or(0);
        Self {
            closed: false,
            signers: signers_for(&root, &client.id, individual),
            client,
            root,
            individual,
            template,
            blanks,
            partners,
            partner,
            confirm: false,
            job: None,
            said: None,
            err: None,
            audit,
        }
    }

    /// The letter for this client: the fill-ins replaced by what staff typed.
    fn laid(&self) -> Result<vf_letters::Laid, String> {
        let Some(t) = &self.template else {
            return Err("There is no closure letter template (Admin › Portal › Letter templates, kind Closure letter).".into());
        };
        let mut t = t.clone();
        for b in &mut t.blocks {
            let fix = |s: &mut String| {
                for (k, v) in &self.blanks {
                    if !v.trim().is_empty() {
                        *s = s.replace(k.as_str(), v.trim());
                    }
                }
            };
            match b {
                vf_letters::Block::Heading { text, .. }
                | vf_letters::Block::Paragraph { text, .. } => fix(text),
                vf_letters::Block::List { items, .. } => items.iter_mut().for_each(fix),
                vf_letters::Block::Table { rows, .. } => rows.iter_mut().flatten().for_each(fix),
                vf_letters::Block::Signature { .. } => {}
            }
        }
        let class = vf_portal::delivery::signer_class(self.individual);
        let left = t.fill_ins(class, None);
        if !left.is_empty() {
            return Err(format!("Fill in: {}", left.join(", ")));
        }
        let plan = Plan {
            client: self.client.clone(),
            year: default_year(),
            title: String::new(),
            individual: self.individual,
            services: Vec::new(),
            letters: Vec::new(),
            pay: false,
            signers: self.signers.clone(),
            partner: self.partners.get(self.partner).cloned().unwrap_or_default(),
            firm: vf_fs::cabinet::read(&self.root)
                .ok()
                .and_then(|c| c.name)
                .unwrap_or_default(),
            notify: false,
            remind_days: 0,
            assigned: None,
        };
        Ok(vf_letters::layout(&t, &plan.fields(), class, None))
    }

    pub fn show(&mut self, ctx: &egui::Context, s: &PortalSettings) {
        if let Some(rx) = &self.job
            && let Ok(r) = rx.try_recv()
        {
            self.job = None;
            match r {
                Ok(m) => self.said = Some(m),
                Err(e) => self.err = Some(e),
            }
        }
        let busy = self.job.is_some();
        let mut open_win = true;
        let mut go = false;
        let mut preview = false;
        egui::Window::new(format!("End engagement — {} ({})", self.client.name, self.client.code))
            .id(egui::Id::unique("portal-end-engagement"))
            .open(&mut open_win)
            .collapsible(false)
            .default_width(620.0)
            .show(ctx, |ui| {
                if busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Working…");
                    });
                }
                ui.label("Ends the engagement on the portal (the client's open service agreements are cancelled and their monthly charge is stopped), then sends the engagement closure letter to the client's portal (to read, not to sign) and files it in the client's folder through Intake.");
                ui.add_space(4.0);
                if self.template.is_none() {
                    ui.colored_label(theme::warn(), "There is no closure letter template.");
                }
                egui::Grid::new("closure-blanks").num_columns(2).show(ui, |ui| {
                    for (k, v) in &mut self.blanks {
                        ui.label(k.as_str());
                        widgets::named(
                            ui.add_enabled(!busy, egui::TextEdit::singleline(v).desired_width(320.0)),
                            k,
                        );
                        ui.end_row();
                    }
                });
                if !self.partners.is_empty() {
                    ui.horizontal(|ui| {
                        ui.label("Signed for the firm by");
                        egui::ComboBox::from_id_salt("closure-partner")
                            .selected_text(self.partners.get(self.partner).map(|p| p.name.clone()).unwrap_or_default())
                            .show_ui(ui, |ui| {
                                for (i, p) in self.partners.iter().enumerate() {
                                    ui.selectable_value(&mut self.partner, i, &p.name);
                                }
                            });
                    });
                }
                if let Some(e) = &self.err {
                    ui.colored_label(theme::error(), e);
                }
                if let Some(m) = &self.said {
                    ui.colored_label(theme::health_ok(), m);
                }
                ui.horizontal(|ui| {
                    if ui.add_enabled(!busy, egui::Button::new("Preview…")).clicked() {
                        preview = true;
                    }
                    if ui
                        .add_enabled(!busy && self.said.is_none(), egui::Button::new(RichText::new("Send and end engagement…").color(theme::error())))
                        .clicked()
                    {
                        self.confirm = true;
                    }
                });
                if self.confirm {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("End this engagement? Open agreements are cancelled and the monthly charge stops now.");
                        if ui.button("End engagement").clicked() {
                            self.confirm = false;
                            go = true;
                        }
                        if ui.button("Keep it").clicked() {
                            self.confirm = false;
                        }
                    });
                }
            });
        if !open_win {
            self.closed = true;
        }
        if preview {
            self.err = None;
            match self.laid() {
                Ok(l) => {
                    let dir = crate::ui::letter_templates::preview_dir();
                    let _ = std::fs::create_dir_all(&dir);
                    let p = dir.join(format!("{} - closure.pdf", self.client.code));
                    if let Err(e) = std::fs::write(&p, &l.pdf)
                        .map_err(|e| e.to_string())
                        .and_then(|()| crate::shell::run(&crate::shell::ShellAction::Open(p)))
                    {
                        self.err = Some(e);
                    }
                }
                Err(e) => self.err = Some(e),
            }
        }
        if go {
            self.err = None;
            let laid = match self.laid() {
                Ok(l) => l,
                Err(e) => {
                    self.err = Some(e);
                    return;
                }
            };
            let s2 = s.clone();
            let root = self.root.clone();
            let client = self.client.clone();
            let audit = self.audit.clone();
            let doc_type = self
                .template
                .as_ref()
                .map(|t| t.doc_type.clone())
                .unwrap_or_default();
            self.job = Some(spawn(ctx, move || {
                end_engagement(&s2, &root, &client, &laid.pdf, &doc_type, &audit)
            }));
        }
    }
}

/// The closure letter shared with everyone of the client on the portal and put in the Portal
/// inbox for Intake to file; then the engagement ended on the portal.
fn end_engagement(
    s: &PortalSettings,
    root: &Path,
    client: &ClientRef,
    pdf: &[u8],
    doc_type: &str,
    audit: &AuditOut,
) -> Result<String, String> {
    let p = core::client(s);
    // The engagement ends on the portal first (review 2026-11-25): a letter is never shared or
    // filed for an engagement still running. Ending it again ends nothing more, so a letter
    // that failed to go out is sent by pressing End engagement… again.
    let (n, stopped) = p.end_engagement(&client.id).map_err(|e| e.message())?;
    let ended = format!(
        "{n} open agreement{} cancelled{}",
        if n == 1 { "" } else { "s" },
        if stopped {
            "; the monthly charge was stopped"
        } else {
            ""
        }
    );
    let day = chrono::Local::now().format("%Y-%m-%d").to_string();
    let name = format!("{day} Engagement closure letter.pdf");
    // The copy the portal shares, written beside the inbox first.
    let record = |file_id: Option<&str>| {
        audit.send(AuditEntry::new(
            "portal_engagement_end",
            None,
            serde_json::json!({
                "client_id": client.id,
                "closure_file_id": file_id,
                "agreements_cancelled": n,
                "monthly_stopped": stopped,
            }),
        ));
    };
    let not_sent = |e: String| {
        record(None);
        format!(
            "The engagement was ended on the portal ({ended}), but the closure letter was not \
             shared: {e}. Press End engagement… again to share it (nothing more is ended)."
        )
    };
    let tmp = vf_portal::inbox::tmp_dir(root);
    std::fs::create_dir_all(&tmp).map_err(|e| not_sent(e.to_string()))?;
    let local = tmp.join(format!("closure-{}.pdf", uuid::Uuid::now_v7()));
    std::fs::write(&local, pdf).map_err(|e| not_sent(e.to_string()))?;
    let shared = (|| -> Result<String, vf_portal::PortalError> {
        let doc_id = format!("closure-{}", uuid::Uuid::now_v7());
        let year = chrono::Datelike::year(&chrono::Local::now());
        let a = p.add(&client.id, &doc_id, &name, Some(year), &local)?;
        let people = p.users(&client.id)?;
        let all: std::collections::BTreeMap<String, bool> = people
            .users
            .iter()
            .filter(|u| u.active)
            .map(|u| (u.identity_id.clone(), true))
            .collect();
        let held: std::collections::BTreeMap<String, bool> = people
            .invitations
            .iter()
            .map(|i| (i.invitation_id.clone(), true))
            .collect();
        if !all.is_empty() || !held.is_empty() {
            p.set_visibility_with(a.file_id(), &all, &held)?;
        }
        Ok(a.file_id().to_string())
    })();
    let _ = std::fs::remove_file(&local);
    let file_id = shared.map_err(|e| not_sent(e.message()))?;
    let filed =
        vf_portal::agreement::file_in_inbox(root, &client.name, &client.code, &name, pdf, doc_type);
    record(Some(&file_id));
    let mut said = format!("The closure letter is on the client's portal. {ended}.");
    match filed {
        Ok(_) => said.push_str(" Intake will file the letter in the client's folder."),
        Err(e) => said.push_str(&format!(
            " The letter could not be put in the Portal inbox ({e}): file it by hand."
        )),
    }
    Ok(said)
}

// ======================================================================================
// Admin › Portal › Agreements: the partners, and sending next year's agreements to many
// clients at once (A6), each from their last agreement.

/// A client of the cabinet for the list.
#[derive(Clone)]
struct Candidate {
    client: ClientRef,
    tick: bool,
    last: Option<Delivery>,
    this_year: Option<String>,
    /// The client's live monthly charge (its amount carries into the renewal).
    live: Option<Subscription>,
}

enum Bulk {
    Read(Result<(Vec<Delivery>, Vec<Subscription>, String), String>),
    Progress(String),
    Done(String),
    /// Nothing was sent: a fill-in left, or the templates could not be read (said as an error).
    Stopped(String),
}

pub struct AgreementsPage {
    pub year: i32,
    candidates: Vec<Candidate>,
    search: String,
    show_all: bool,
    firm: String,
    loaded: bool,
    partners: Vec<Partner>,
    partners_said: Option<String>,
    group: u32,
    pause_s: u32,
    rx: Option<Receiver<Bulk>>,
    stop: Option<Arc<AtomicBool>>,
    progress: Option<String>,
    done: Option<String>,
    err: Option<String>,
    settings: Option<(std::time::Instant, Option<PortalSettings>)>,
}

impl Default for AgreementsPage {
    fn default() -> Self {
        Self {
            year: default_year(),
            candidates: Vec::new(),
            search: String::new(),
            show_all: false,
            firm: String::new(),
            loaded: false,
            partners: Vec::new(),
            partners_said: None,
            group: 20,
            pause_s: 60,
            rx: None,
            stop: None,
            progress: None,
            done: None,
            err: None,
            settings: None,
        }
    }
}

impl AgreementsPage {
    pub fn reread(&mut self) {
        self.loaded = false;
        self.settings = None;
        self.partners_said = None;
        self.partners.clear();
    }
}

/// Admin › Portal › Agreements, with the portal settings read at most every 30 s.
pub fn page(
    ui: &mut egui::Ui,
    page: &mut AgreementsPage,
    env: &mut crate::ui::browse::Env,
    audit: Option<AuditOut>,
) {
    if page
        .settings
        .as_ref()
        .is_none_or(|(at, _)| at.elapsed() >= std::time::Duration::from_secs(30))
    {
        let s = vf_fs::cabinet::read(env.root)
            .ok()
            .and_then(|c| vf_portal::settings::load(env.root, &c.cabinet_id));
        page.settings = Some((std::time::Instant::now(), s));
    }
    let Some(s) = page
        .settings
        .as_ref()
        .and_then(|(_, s)| s.clone())
        .filter(|s| s.deliveries)
    else {
        ui.heading("Agreements");
        ui.label("Annual agreements come with tax return delivery: turn it on in Admin › Portal.");
        return;
    };
    // Without the app's channel (tests) the rows are not kept.
    let audit = audit.unwrap_or_else(|| AuditOut::new(std::sync::mpsc::channel().0));
    show_page(ui, page, env.conn, env.root, &s, &audit);
}

/// The page (Admin › Portal › Agreements).
pub fn show_page(
    ui: &mut egui::Ui,
    page: &mut AgreementsPage,
    conn: &vf_core::db::Connection,
    root: &Path,
    s: &PortalSettings,
    audit: &AuditOut,
) {
    ui.heading("Agreements");
    ui.label("Annual service agreements: the letters for the services a client takes, signed on the portal, and their fees once or monthly. Send one from a client's details (Service agreement…), or many here.");
    // Partners (A7, A15).
    egui::CollapsingHeader::new("Partners (who signs for the firm)")
        .default_open(page.partners.is_empty())
        .show(ui, |ui| {
            if page.partners.is_empty() && page.partners_said.is_none() {
                page.partners = vf_letters::partners::load(root);
            }
            egui::Grid::new("partners").num_columns(4).striped(true).show(ui, |ui| {
                ui.label(RichText::new("Name").small());
                ui.label(RichText::new("Title").small());
                ui.label(RichText::new("Client CSV's Partner value").small());
                ui.label("");
                ui.end_row();
                let mut remove = None;
                for (i, p) in page.partners.iter_mut().enumerate() {
                    widgets::named(
                        ui.add_sized([180.0, 22.0], egui::TextEdit::singleline(&mut p.name)),
                        "Partner name",
                    );
                    widgets::named(
                        ui.add_sized([140.0, 22.0], egui::TextEdit::singleline(&mut p.title)),
                        "Partner title",
                    );
                    ui.add_sized(
                        [90.0, 22.0],
                        egui::TextEdit::singleline(&mut p.code).hint_text("initials"),
                    );
                    if ui.small_btn("Remove").clicked() {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
                if let Some(i) = remove {
                    page.partners.remove(i);
                }
            });
            ui.horizontal(|ui| {
                if ui.button("Add partner").clicked() {
                    page.partners.push(Partner {
                        title: "Partner".into(),
                        ..Default::default()
                    });
                }
                if ui.button("Save partners").clicked() {
                    match vf_letters::partners::save(root, &page.partners) {
                        Ok(saved) => {
                            audit.send(AuditEntry::new(
                                "partners_save",
                                None,
                                serde_json::json!({ "partners": saved.iter().map(|p| &p.name).collect::<Vec<_>>() }),
                            ));
                            page.partners = saved;
                            page.partners_said = Some("Saved for every PC.".into());
                        }
                        Err(e) => page.partners_said = Some(e),
                    }
                }
                if let Some(m) = &page.partners_said {
                    ui.label(m);
                }
            });
            ui.label(
                RichText::new("A client's agreement is signed by the partner its client CSV's Partner column names (by the value here, or by name), else the first.")
                    .small()
                    .color(theme::ink_muted()),
            );
        });
    ui.separator();
    // Bulk send.
    if let Some(rx) = &page.rx {
        while let Ok(m) = rx.try_recv() {
            match m {
                Bulk::Read(Ok((all, subs, firm))) => {
                    page.progress = None;
                    page.firm = firm;
                    for c in &mut page.candidates {
                        let mine: Vec<&Delivery> = all
                            .iter()
                            .filter(|d| d.client_id == c.client.id && d.is_agreement())
                            .collect();
                        c.this_year = mine
                            .iter()
                            .find(|d| d.tax_year == page.year && d.status != "cancelled")
                            .map(|d| d.status.clone());
                        c.live = subs
                            .iter()
                            .find(|s| s.client_id == c.client.id && s.live())
                            .cloned();
                        c.last = mine
                            .iter()
                            .filter(|d| d.tax_year < page.year && d.status != "cancelled")
                            .max_by_key(|d| d.tax_year)
                            .map(|d| (*d).clone());
                    }
                }
                Bulk::Read(Err(e)) => {
                    page.err = Some(e);
                    page.progress = None;
                }
                Bulk::Progress(p) => page.progress = Some(p),
                Bulk::Done(p) => {
                    // Read again next frame (what was sent shows as sent), keeping the result.
                    page.stop = None;
                    page.loaded = false;
                    page.done = Some(p);
                }
                Bulk::Stopped(e) => {
                    page.stop = None;
                    page.progress = None;
                    page.err = Some(e);
                }
            }
        }
    }
    let busy = page.stop.is_some();
    if !page.loaded && !busy {
        page.candidates = clients_of(conn)
            .into_iter()
            .map(|client| Candidate {
                client,
                tick: false,
                last: None,
                this_year: None,
                live: None,
            })
            .collect();
        let s2 = s.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ui.ctx().clone();
        std::thread::spawn(move || {
            let p = core::client(&s2);
            let r = p
                .deliveries_all(None)
                .map(|d| {
                    (
                        d,
                        p.live_subscriptions().unwrap_or_default(),
                        p.status().map(|s| s.firm_name).unwrap_or_default(),
                    )
                })
                .map_err(|e| e.message());
            let _ = tx.send(Bulk::Read(r));
            ctx.request_repaint();
        });
        page.rx = Some(rx);
        page.progress = Some("Reading the portal…".into());
        page.err = None;
        page.loaded = true;
    }
    ui.horizontal_wrapped(|ui| {
        ui.label("Agreement year");
        let before = page.year;
        ui.add_enabled(
            !busy,
            egui::DragValue::new(&mut page.year).range(1990..=2100),
        );
        if before != page.year {
            page.loaded = false;
            page.progress = None;
        }
        ui.label("Search");
        ui.add(egui::TextEdit::singleline(&mut page.search).desired_width(160.0));
        ui.checkbox(&mut page.show_all, "Show clients sent already");
        if ui
            .add_enabled(!busy, egui::Button::new("Read again"))
            .clicked()
        {
            page.loaded = false;
            page.progress = None;
        }
    });
    if let Some(p) = &page.progress {
        ui.horizontal(|ui| {
            if busy || p.starts_with("Reading") {
                ui.spinner();
            }
            ui.label(p);
        });
    }
    if let Some(d) = &page.done {
        ui.colored_label(theme::health_ok(), d);
    }
    if let Some(e) = &page.err {
        ui.colored_label(theme::error(), e);
    }
    let q = page.search.trim().to_lowercase();
    let shown: Vec<usize> = page
        .candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| page.show_all || c.this_year.is_none())
        .filter(|(_, c)| {
            q.is_empty()
                || c.client.name.to_lowercase().contains(&q)
                || c.client.code.to_lowercase().contains(&q)
        })
        .map(|(i, _)| i)
        .take(300)
        .collect();
    use egui_extras::{Column, TableBuilder};
    TableBuilder::new(ui)
        .striped(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::initial(300.0).at_least(160.0).clip(true))
        .column(Column::initial(260.0).at_least(120.0).clip(true))
        .column(Column::exact(100.0))
        .column(Column::remainder().at_least(90.0))
        .max_scroll_height(380.0)
        .header(22.0, |mut h| {
            for n in ["Client", "Services (last agreement)", "One-time", "Monthly"] {
                h.col(|ui| {
                    ui.label(RichText::new(n).small().color(theme::ink_muted()));
                });
            }
        })
        .body(|body| {
            body.rows(24.0, shown.len(), |mut row| {
                let c = &mut page.candidates[shown[row.index()]];
                let lines = c
                    .last
                    .as_ref()
                    .map(|l| services_now(l, c.live.as_ref()).0)
                    .unwrap_or_default();
                let (o, m) = vf_portal::agreement::totals(&lines);
                // The client's name is its tick (a larger target than a box alone).
                row.col(|ui| {
                    ui.add_enabled(
                        !busy && c.last.is_some() && c.this_year.is_none(),
                        egui::Checkbox::new(
                            &mut c.tick,
                            format!("{} ({})", c.client.name, c.client.code),
                        ),
                    );
                });
                row.col(|ui| {
                    let words = match (&c.this_year, &c.last) {
                        (Some(st), _) => format!("sent ({st})"),
                        (None, Some(_)) => lines
                            .iter()
                            .map(|l| l.service.as_str())
                            .collect::<Vec<_>>()
                            .join(", "),
                        (None, None) => "no earlier agreement: send from the client".into(),
                    };
                    ui.label(words);
                });
                row.col(|ui| {
                    ui.label(if c.last.is_some() {
                        money(o)
                    } else {
                        "—".into()
                    });
                });
                row.col(|ui| {
                    ui.label(if c.last.is_some() {
                        money(m)
                    } else {
                        "—".into()
                    });
                });
            });
        });
    let ticked: Vec<Candidate> = page.candidates.iter().filter(|c| c.tick).cloned().collect();
    ui.horizontal_wrapped(|ui| {
        ui.label(format!("{} ticked", ticked.len()));
        ui.label("· in groups of");
        ui.add_enabled(!busy, egui::DragValue::new(&mut page.group).range(1..=200));
        ui.label("with a pause of");
        ui.add_enabled(!busy, egui::DragValue::new(&mut page.pause_s).range(0..=3600));
        ui.label("s");
        if busy {
            if ui.button("Stop").clicked()
                && let Some(f) = &page.stop
            {
                f.store(true, Ordering::Relaxed);
            }
        } else if ui
            .add_enabled(!ticked.is_empty(), widgets::primary_button(format!(
                    "Send {} agreement{}",
                    ticked.len(),
                    filer_intake::ui::theme::plural(ticked.len())
                )))
            .on_hover_text("Each from the client's last agreement: the same services and fees, its letters as the templates now say, signed by the client's partner.")
            .clicked()
        {
            start_bulk(ui.ctx(), page, root, s, audit, ticked);
        }
    });
}

fn clients_of(conn: &vf_core::db::Connection) -> Vec<ClientRef> {
    let Ok(mut st) = conn.prepare(
        "SELECT id, ext_id, name FROM client WHERE deleted_at IS NULL ORDER BY name COLLATE NOCASE",
    ) else {
        return Vec::new();
    };
    st.query_map([], |r| {
        Ok(ClientRef {
            id: r.get(0)?,
            code: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
            name: r.get(2)?,
        })
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

/// Each ticked client's plan from their last agreement, laid out (on the bulk worker). A fill-in
/// left in a template, or templates that cannot be read, stop the whole run before anything is
/// sent (A17).
fn plans_for(
    root: &Path,
    firm: &str,
    year: i32,
    ticked: Vec<Candidate>,
) -> Result<Vec<Result<Ready, String>>, String> {
    let all = vf_letters::store::load_all(root)?;
    let partners = vf_letters::partners::load(root);
    let firm = if firm.is_empty() {
        vf_fs::cabinet::read(root)
            .ok()
            .and_then(|c| c.name)
            .unwrap_or_default()
    } else {
        firm.to_string()
    };
    let mut plans: Vec<Result<Ready, String>> = Vec::new();
    for c in ticked {
        let Some(last) = &c.last else { continue };
        let (services, note) = services_now(last, c.live.as_ref());
        if let Some(n) = note {
            plans.push(Err(format!("{} ({}): {n}", c.client.name, c.client.code)));
            continue;
        }
        let individual = !vf_fs::contacts::client_is_business(root, &c.client.id);
        let names: Vec<String> = services.iter().map(|s| s.service.clone()).collect();
        let mine = vf_fs::contacts::client_partner(root, &c.client.id);
        let plan = Plan {
            client: c.client.clone(),
            year,
            title: format!("Your {year} service agreement"),
            individual,
            letters: letters_for(&all, &names)
                .into_iter()
                .filter(|(_, on)| *on)
                .map(|(t, _)| t)
                .collect(),
            services,
            pay: last.fee.is_some(),
            signers: signers_for(root, &c.client.id, individual),
            partner: vf_letters::partners::default_for(&partners, mine.as_deref())
                .cloned()
                .unwrap_or_default(),
            firm: firm.clone(),
            notify: true,
            remind_days: last.remind_days,
            assigned: last.assigned_staff_id.clone(),
        };
        // QA 2026-11-25: letters that came out empty where the last agreement had some (a
        // template removed or renamed) are not sent as a fee-only agreement.
        if plan.letters.is_empty() && !last.letters.is_empty() {
            plans.push(Err(format!(
                "{} ({}): no letter template for its services (its last agreement had letters)",
                c.client.name, c.client.code
            )));
            continue;
        }
        let r = match plan.problem() {
            Some(p) => Err(format!("{} ({}): {p}", c.client.name, c.client.code)),
            None => plan
                .laid(&all)
                .map(|l| (plan.clone(), l))
                .map_err(|e| format!("{} ({}): {e}", c.client.name, c.client.code)),
        };
        plans.push(r);
    }
    if let Some(Err(e)) = plans
        .iter()
        .find(|p| p.as_ref().is_err_and(|e| e.contains("Fill in")))
    {
        return Err(e.clone());
    }
    Ok(plans)
}

/// A plan with its letters laid out, ready to send.
type Ready = (Plan, Vec<(Template, vf_letters::Laid)>);

/// Builds each ticked client's plan from their last agreement and sends them on a worker, a
/// group at a time with a pause between, stopping when asked.
fn start_bulk(
    ctx: &egui::Context,
    page: &mut AgreementsPage,
    root: &Path,
    s: &PortalSettings,
    audit: &AuditOut,
    ticked: Vec<Candidate>,
) {
    page.err = None;
    let firm = page.firm.clone();
    let year = page.year;
    let root = root.to_path_buf();
    let stop = Arc::new(AtomicBool::new(false));
    page.stop = Some(stop.clone());
    let (tx, rx) = std::sync::mpsc::channel();
    page.rx = Some(rx);
    let s2 = s.clone();
    let group = page.group.max(1) as usize;
    let pause = std::time::Duration::from_secs(u64::from(page.pause_s));
    let audit = audit.clone();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let p = core::client(&s2);
        // QA 2026-11-25: built here, not on the window's thread; templates that cannot be read
        // stop the run (they were taken as none, and fee-only agreements went out).
        let plans = match plans_for(&root, &firm, year, ticked) {
            Ok(plans) => plans,
            Err(e) => {
                let _ = tx.send(Bulk::Stopped(e));
                ctx.request_repaint();
                return;
            }
        };
        let (mut sent, mut problems) = (0usize, Vec::new());
        let total = plans.len();
        for (i, r) in plans.into_iter().enumerate() {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            if i > 0 && i % group == 0 {
                let _ = tx.send(Bulk::Progress(format!("Sent {sent} of {total}; a pause…")));
                ctx.request_repaint();
                let until = std::time::Instant::now() + pause;
                while std::time::Instant::now() < until && !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                if stop.load(Ordering::Relaxed) {
                    break;
                }
            }
            // QA 2026-11-25: asked of the portal for this client (the page's list holds the newest
            // 500 cards only), so a client is never asked to sign and pay twice in a year.
            let r = r.and_then(|(plan, laid)| match p.agreements(&plan.client.id) {
                Ok(had)
                    if had
                        .iter()
                        .any(|d| d.tax_year == plan.year && d.status != "cancelled") =>
                {
                    Err(format!(
                        "{} ({}): already has a {} agreement",
                        plan.client.name, plan.client.code, plan.year
                    ))
                }
                Ok(_) => Ok((plan, laid)),
                Err(e) => Err(format!(
                    "{} ({}): {}",
                    plan.client.name,
                    plan.client.code,
                    e.message()
                )),
            });
            match r {
                Ok((plan, laid)) => match send(&p, &plan, &laid) {
                    Ok(d) => {
                        sent += 1;
                        audit.send(AuditEntry::new(
                            "portal_agreement_send",
                            None,
                            serde_json::json!({
                                "client_id": d.client_id,
                                "delivery_id": d.id,
                                "kind": "agreement",
                                "year": d.tax_year,
                                "covers": d.covers,
                                "bulk": true,
                            }),
                        ));
                    }
                    Err(e) => problems.push(format!("{}: {}", plan.client.name, e.message())),
                },
                Err(e) => problems.push(e),
            }
            let _ = tx.send(Bulk::Progress(format!("Sent {sent} of {total}…")));
            ctx.request_repaint();
        }
        let mut said = format!("Sent {sent} of {total} agreements.");
        if stop.load(Ordering::Relaxed) {
            said.push_str(" Stopped.");
        }
        if !problems.is_empty() {
            said.push_str(&format!(" Not sent: {}", problems.join("; ")));
        }
        let _ = tx.send(Bulk::Done(said));
        ctx.request_repaint();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(root: &Path, individual: bool, services: Vec<ServiceLine>, ids: &[&str]) -> Plan {
        let all = vf_letters::store::load_all(root).unwrap();
        Plan {
            client: ClientRef {
                id: "c1".into(),
                code: "1042".into(),
                name: "Sample Farms LLC".into(),
            },
            year: 2027,
            title: "Your 2027 service agreement".into(),
            individual,
            services,
            letters: all
                .into_iter()
                .filter(|t| ids.contains(&t.id.as_str()))
                .collect(),
            pay: true,
            signers: vec![DeliverySigner {
                id: String::new(),
                role: if individual { "taxpayer" } else { "officer" }.into(),
                name: "Pat Sample".into(),
                title: "Managing member".into(),
                email: Some("pat@example.test".into()),
                phone: None,
            }],
            partner: Partner {
                name: "Morgan Example".into(),
                title: "Partner".into(),
                code: "ME".into(),
            },
            firm: "Example CPA".into(),
            notify: true,
            remind_days: 7,
            assigned: None,
        }
    }

    #[test]
    fn a_letter_asking_for_the_terms_is_not_sent_without_them() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        std::fs::create_dir_all(root.join("_Vibe")).unwrap();
        vf_letters::store::seed_if_missing(root).unwrap();
        let services = vec![ServiceLine {
            service: "Bookkeeping".into(),
            once_cents: 0,
            month_cents: 45_000,
        }];
        let p = plan(root, false, services, &["service-bookkeeping"]);
        assert!(p.letters.iter().all(vf_letters::wants_terms));
        let none: Vec<Template> = vf_letters::store::load_all(root)
            .unwrap()
            .into_iter()
            .filter(|t| t.kind != vf_letters::TERMS)
            .collect();
        let e = p.laid(&none).unwrap_err();
        assert!(e.contains("no Terms and Conditions template"), "{e}");
    }

    #[test]
    fn an_agreements_letters_fields_and_fill_ins() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        std::fs::create_dir_all(root.join("_Vibe")).unwrap();
        vf_letters::store::seed_if_missing(root).unwrap();
        let all = vf_letters::store::load_all(root).unwrap();
        let services = vec![
            ServiceLine {
                service: "1120-S".into(),
                once_cents: 185_000,
                month_cents: 0,
            },
            ServiceLine {
                service: "Bookkeeping".into(),
                once_cents: 0,
                month_cents: 45_000,
            },
        ];
        // The letters offered: the 1120-S's and bookkeeping's ticked, the consents not.
        let offered = letters_for(&all, &["1120-S".into(), "Bookkeeping".into()]);
        // One engagement letter per service: the kit's 1120-S letter, not the old entity sample.
        assert!(
            offered
                .iter()
                .any(|(t, on)| t.id == "engagement-entity" && !on)
        );
        let ticked: Vec<&str> = offered
            .iter()
            .filter(|(_, on)| *on)
            .map(|(t, _)| t.id.as_str())
            .collect();
        assert!(ticked.contains(&"aicpa-1120s") && ticked.contains(&"service-bookkeeping"));
        assert!(offered.iter().any(|(t, on)| t.id == "consent-use" && !on));
        // The kit's 1120-S letter has fill-ins: never sent until the firm fills them (A17).
        let p = plan(
            root,
            false,
            services.clone(),
            &["aicpa-1120s", "service-bookkeeping"],
        );
        let e = p.laid(&all).unwrap_err();
        assert!(e.contains("[X]") && e.contains("1120-S"), "{e}");
        // The Terms and Conditions' own fill-ins hold back every letter they are part of.
        let p = plan(root, false, services.clone(), &["service-bookkeeping"]);
        let e = p.laid(&all).unwrap_err();
        assert!(e.contains("[Portal Name]"), "{e}");
        // The firm fills them in (Admin › Portal › Letter templates).
        let mut terms = all
            .iter()
            .find(|t| t.kind == vf_letters::TERMS)
            .unwrap()
            .clone();
        let mut json = serde_json::to_string(&terms.blocks).unwrap();
        for (k, v) in [
            ("[X]", "30"),
            ("[Portal Name]", "Example Portal"),
            ("[State Name]", "Ohio"),
            (
                "[Name of State or Federal Court Desired]",
                "Franklin County, Ohio",
            ),
        ] {
            json = json.replace(k, v);
        }
        terms.blocks = serde_json::from_str(&json).unwrap();
        terms.sample = false;
        vf_letters::store::save(root, &terms).unwrap();
        let all = vf_letters::store::load_all(root).unwrap();
        // The bookkeeping letter alone: laid out with the Terms, the officer signing, the
        // partner printed and the fees in its words.
        let f = p.fields();
        assert_eq!(
            f["Fee lines"],
            "1120-S: $1,850.00\nBookkeeping: $450.00 a month"
        );
        assert_eq!(f["Tax year"], "2026");
        assert_eq!(f["Partner name"], "Morgan Example");
        assert_eq!(p.fee_for(&p.letters[0]), "$450.00 a month");
        let laid = p.laid(&all).unwrap();
        assert_eq!(laid.len(), 1);
        let roles: Vec<&str> = laid[0].1.blocks.iter().map(|b| b.role.as_str()).collect();
        assert_eq!(roles, ["officer"]);
        // An individual: taxpayer (no spouse given).
        let p = plan(root, true, services, &["service-bookkeeping"]);
        let laid = p.laid(&all).unwrap();
        assert_eq!(
            laid[0]
                .1
                .blocks
                .iter()
                .map(|b| b.role.as_str())
                .collect::<Vec<_>>(),
            ["taxpayer"]
        );
        assert!(p.problem().is_none());
        let mut none = p.clone();
        none.services.clear();
        assert!(none.problem().is_some());
    }

    #[test]
    fn services_from_last_years_agreement() {
        let mut d: Delivery = serde_json::from_str(
            r#"{"id":"a1","clientId":"c1","taxYear":2026,"returnType":"Agreement",
            "kind":"agreement","covers":["1120-S","Bookkeeping"],"returnYear":2025,
            "title":"Your 2026 service agreement","status":"complete",
            "locks":{"returnAfterLetters":false,"esignAfterLetters":false,"esignAfterFee":false},
            "assignedStaffId":null,"returnFile":null,"esignLink":null,"remindedAt":null,
            "createdAt":"2026-01-05T12:00:00.000Z","completedAt":null,"cancelledAt":null,
            "steps":[]}"#,
        )
        .unwrap();
        assert!(d.is_agreement() && d.what() == "Agreement");
        d.fee = Some(vf_portal::delivery::DeliveryFee {
            mode: Some("lines".into()),
            lines: vec![
                vf_portal::delivery::FeeLineOut {
                    key: "a".into(),
                    kind: "service".into(),
                    name: "1120-S".into(),
                    cents: 185_000,
                    recommended: false,
                    selected: false,
                    every: "once".into(),
                },
                vf_portal::delivery::FeeLineOut {
                    key: "b".into(),
                    kind: "service".into(),
                    name: "Bookkeeping".into(),
                    cents: 45_000,
                    recommended: false,
                    selected: false,
                    every: "month".into(),
                },
            ],
            total_cents: 0,
            selection_signed_at: None,
            selection_signed_by: None,
            payments: vec![],
        });
        let s = services_of(&d);
        assert_eq!(
            s.iter()
                .map(|l| (l.service.as_str(), l.once_cents, l.month_cents))
                .collect::<Vec<_>>(),
            [("1120-S", 185_000, 0), ("Bookkeeping", 0, 45_000)]
        );
    }
}
