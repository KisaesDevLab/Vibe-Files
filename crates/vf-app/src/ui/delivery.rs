//! Tax return delivery (ADDENDUM-tax-return-delivery.md §12, owner 2026-11-24; Vibe-Portal
//! D1/D2): the client panel's Deliver tax return… window and Admin › Portal › Deliveries.
//! Slice 2 builds the return step (a file already on the portal) and the Form 8879 step (the
//! client's UltraTax eSignature link, Confirm signed); letters, package and fee come in later
//! slices. Each delivery read is copied into the client's folder with an audit row when it
//! changed; every portal call runs on a thread with "Working…" shown.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};

use egui::RichText;
use vf_core::audit::AuditEntry;
use vf_portal::PortalSettings;
use vf_portal::api::PortalFile;
use vf_portal::delivery::{self, Delivery, DeliveryChange, DeliverySigner, NewDelivery, StepsIn};

use crate::portal as core;
use crate::ui::portal::{AuditOut, ClientRef, spawn};
use crate::ui::{theme, widgets};

// ---- The copy in the client's folder -----------------------------------------------------------

/// Deliveries read from the portal, for the app to copy into their client folders.
#[derive(Clone)]
pub struct ReadOut(pub(crate) Sender<Vec<Delivery>>);

impl ReadOut {
    pub(crate) fn send(&self, d: Vec<Delivery>) {
        if !d.is_empty() {
            let _ = self.0.send(d);
        }
    }
}

/// The copies on this PC (load test 2026-11-24: 500 deliveries took 2.3 s to copy at once, and
/// 0.1 s to read again unchanged, on the UI thread): what each delivery was last checked
/// against, so an unchanged one costs no disk read, and those still to copy, a slice a frame.
#[derive(Default)]
pub struct Copies {
    seen: HashMap<String, u64>,
    pending: std::collections::VecDeque<Delivery>,
}

impl Copies {
    /// Deliveries still to copy (the app calls again next frame).
    pub fn busy(&self) -> bool {
        !self.pending.is_empty()
    }
}

/// The time a frame gives to looking at deliveries, and the copies it writes at most (a copy
/// with its audit row costs about 10 ms to commit: 5 keep a frame near 50 ms; load test
/// 2026-11-24).
const COPY_SLICE: std::time::Duration = std::time::Duration::from_millis(60);
const COPY_WRITES: usize = 5;

fn fingerprint(d: &Delivery) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(d).unwrap_or_default().hash(&mut h);
    h.finish()
}

/// Writes the copies of deliveries read (only into a folder still bound to the client, only
/// when changed), in one Transaction with an audit row each (`portal_delivery_copy`). A
/// delivery unchanged since this PC last checked it is skipped; what does not fit in one slice
/// waits for the next frame.
pub fn record_reads(
    conn: &mut vf_core::db::Connection,
    root: &Path,
    device_id: &str,
    copies: &mut Copies,
    reads: Vec<Vec<Delivery>>,
) -> Result<(), String> {
    for d in reads.into_iter().flatten() {
        if copies.seen.get(&d.id) != Some(&fingerprint(&d)) {
            copies.pending.retain(|x| x.id != d.id);
            copies.pending.push_back(d);
        }
    }
    if copies.pending.is_empty() {
        return Ok(());
    }
    let start = std::time::Instant::now();
    let mut done: Vec<(String, u64)> = Vec::new();
    let mut writes = 0usize;
    let (pc, user) = core::who();
    let by = format!("{pc}\\{user}");
    let now = vf_core::now_rfc3339();
    let mut tx = vf_core::tx::Transaction::new("portal_delivery_copy").record_only();
    let mut any = false;
    let mut folders: HashMap<String, Option<String>> = HashMap::new();
    while let Some(d) = copies.pending.pop_front() {
        if start.elapsed() > COPY_SLICE || writes >= COPY_WRITES {
            copies.pending.push_front(d);
            break;
        }
        done.push((d.id.clone(), fingerprint(&d)));
        let folder = folders
            .entry(d.client_id.clone())
            .or_insert_with(|| {
                conn.query_row(
                    "SELECT folder_rel FROM client WHERE id = ?1 AND deleted_at IS NULL",
                    [&d.client_id],
                    |r| r.get::<_, Option<String>>(0),
                )
                .ok()
                .flatten()
                .filter(|f| {
                    filer_intake::sentinel::read_sentinel(&root.join(f))
                        .ok()
                        .flatten()
                        .map(|s| s.client_id)
                        == uuid::Uuid::parse_str(&d.client_id).ok()
                })
            })
            .clone();
        let Some(folder) = folder else { continue };
        let path = delivery::copy_path(root, &folder, &d.id);
        let last = delivery::newest_copy(&path);
        let Some(line) = delivery::copy_line(last.as_ref(), &d, &now, &by) else {
            continue;
        };
        any = true;
        writes += 1;
        tx = tx
            .file(vf_core::tx::FileOp::AppendLines {
                path,
                lines: vec![line],
                undo_lines: Vec::new(),
            })
            .audit(AuditEntry::new(
                "portal_delivery_copy",
                None,
                serde_json::json!({
                    "client_id": d.client_id,
                    "delivery_id": d.id,
                    "status": d.status,
                }),
            ));
    }
    if any {
        let mut fs = vf_fs::fileops::FilerFileOps::new(&[root]).map_err(|e| e.to_string())?;
        tx.commit(conn, &mut fs, &vf_core::tx::Actor::device(device_id))
            .map_err(|e| e.to_string())?;
    }
    copies.seen.extend(done);
    Ok(())
}

// ---- The Deliver tax return window -------------------------------------------------------------

enum Done {
    #[allow(clippy::type_complexity)]
    Loaded(
        Result<
            (
                Vec<Delivery>,
                Vec<PortalFile>,
                String,
                Vec<vf_portal::balance::BalanceStaff>,
            ),
            vf_portal::PortalError,
        >,
    ),
    Saved(Result<Box<Delivery>, String>, &'static str),
    Said(Result<String, String>),
}

enum Act {
    Send,
    MarkPaid,
    Confirm,
    Remind,
    Cancel,
    Report,
    Reload,
}

pub struct DeliverWindow {
    pub closed: bool,
    client: ClientRef,
    root: PathBuf,
    folder: Option<String>,
    deliveries: Vec<Delivery>,
    files: Vec<PortalFile>,
    offline: Option<String>,
    // The form.
    year: i32,
    return_type: String,
    title: String,
    with_return: bool,
    with_8879: bool,
    /// Step 1 (Addendum D3): the letters offered for the return type, ticked or not.
    with_letters: bool,
    letters: Vec<(vf_letters::Template, bool)>,
    signers: Vec<DeliverySigner>,
    /// The firm's name as the portal has it (the letters' {Firm name}).
    firm_name: String,
    /// Steps 3 and 4 (Addendum D4/D5): the mode, the client's return fee, the plan's packages
    /// with the price the client sees (an override kept), the add-ons offered.
    with_fee: bool,
    fee_mode: String,
    return_fee: String,
    amount: String,
    plan: Option<vf_portal::pricing::Plan>,
    prices: Vec<(vf_portal::pricing::PlanPackage, String, bool)>,
    addon_ticks: Vec<(vf_portal::pricing::PlanAddon, bool, String)>,
    mark_note: String,
    confirm_mark: bool,
    /// F4: who hears of the delivery's events, and reminders while a step waits on the client.
    staff: Vec<vf_portal::balance::BalanceStaff>,
    assigned: Option<String>,
    remind_days: u32,
    /// T27: a different return chosen after the client saw the first.
    confirm_revise: bool,
    return_file: Option<String>,
    /// The client's PDFs in the catalog (owner 2026-11-24, Q15: the window shares the return
    /// itself) and the one chosen as the return.
    docs: Vec<crate::ui::portal::DocRef>,
    return_doc: Option<String>,
    /// The return types offered: the four built in and the firm's own (Q41).
    types: Vec<String>,
    /// The return's content changed since the client opened it (found when sending).
    force_revise: bool,
    /// Documents go to B2 before they are shared (Q-103).
    b2: crate::ui::portal::B2First,
    link: String,
    notify: bool,
    confirm_cancel: bool,
    job: Option<Receiver<Done>>,
    said: Option<(String, bool)>,
    err: Option<String>,
    audit: AuditOut,
    reads: ReadOut,
}

/// Last calendar year: the tax year a return delivered now usually is.
fn default_year() -> i32 {
    use chrono::Datelike;
    chrono::Local::now().year() - 1
}

impl DeliverWindow {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ctx: &egui::Context,
        s: &PortalSettings,
        root: PathBuf,
        client: ClientRef,
        folder: Option<String>,
        audit: AuditOut,
        reads: ReadOut,
        docs: Vec<crate::ui::portal::DocRef>,
        b2: crate::ui::portal::B2First,
    ) -> Self {
        let year = default_year();
        let types = vf_portal::delivery::return_type_names(&root);
        let mut w = Self {
            types,
            closed: false,
            client,
            root,
            folder,
            deliveries: Vec::new(),
            files: Vec::new(),
            offline: None,
            year,
            return_type: "1040".into(),
            title: format!("Your {year} tax return"),
            with_return: true,
            with_8879: true,
            with_letters: false,
            letters: Vec::new(),
            signers: Vec::new(),
            firm_name: String::new(),
            with_fee: false,
            fee_mode: "packages".into(),
            return_fee: String::new(),
            amount: String::new(),
            plan: None,
            prices: Vec::new(),
            addon_ticks: Vec::new(),
            mark_note: String::new(),
            confirm_mark: false,
            staff: Vec::new(),
            assigned: None,
            remind_days: 7,
            confirm_revise: false,
            return_file: None,
            docs,
            return_doc: None,
            force_revise: false,
            b2,
            link: String::new(),
            notify: true,
            confirm_cancel: false,
            job: None,
            said: None,
            err: None,
            audit,
            reads,
        };
        w.offer_letters();
        w.offer_fee();
        w.load(ctx, s);
        w
    }

    /// The letters offered for the return type (ticked as the template says) and who signs,
    /// from the client's Portal contacts (PA-105): taxpayer and spouse on a 1040, one person
    /// with a title for an entity (T22).
    fn offer_letters(&mut self) {
        let _ = vf_letters::store::seed_if_missing(&self.root);
        self.letters = vf_letters::store::load_all(&self.root)
            .unwrap_or_default()
            .into_iter()
            .filter(|t| t.kind == "letter" && t.offered_for(&self.return_type))
            .map(|t| {
                let on = t.default_ticked;
                (t, on)
            })
            .collect();
        let row = vf_fs::contacts::record(&self.root, &self.client.id)
            .map(|r| r.effective())
            .unwrap_or_default();
        let name = |a: &str, b: &str| format!("{} {}", a.trim(), b.trim()).trim().to_string();
        let opt = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
        self.signers = if vf_portal::delivery::is_individual(&self.root, &self.return_type) {
            let mut v = vec![DeliverySigner {
                id: String::new(),
                role: "taxpayer".into(),
                name: name(&row.first, &row.last),
                title: String::new(),
                email: opt(&row.email),
                phone: opt(&row.cell),
            }];
            if !row.spouse_first.trim().is_empty() {
                v.push(DeliverySigner {
                    id: String::new(),
                    role: "spouse".into(),
                    name: name(&row.spouse_first, &row.spouse_last),
                    title: String::new(),
                    email: opt(&row.spouse_email),
                    phone: opt(&row.spouse_cell),
                });
            }
            v
        } else {
            vec![DeliverySigner {
                id: String::new(),
                role: "officer".into(),
                name: name(&row.first, &row.last),
                title: String::new(),
                email: opt(&row.email),
                phone: opt(&row.cell),
            }]
        };
    }

    /// The pricing plan for the return type and year, its packages priced for the return fee.
    fn offer_fee(&mut self) {
        self.plan = vf_portal::pricing::load(&self.root, &self.return_type, self.year);
        self.prices = self
            .plan
            .as_ref()
            .map(|p| {
                p.packages
                    .iter()
                    .map(|k| (k.clone(), String::new(), true))
                    .collect()
            })
            .unwrap_or_default();
        self.addon_ticks = self
            .plan
            .as_ref()
            .map(|p| {
                p.addons
                    .iter()
                    .map(|a| {
                        (
                            a.clone(),
                            a.offered,
                            vf_portal::balance::money(a.cents)
                                .trim_start_matches('$')
                                .to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.reprice();
    }

    /// Each package's price worked out from the return fee (an override typed stays).
    fn reprice(&mut self) {
        let Some(fee) = vf_portal::balance::parse_money(&self.return_fee) else {
            return;
        };
        for (k, shown, _) in &mut self.prices {
            *shown = vf_portal::balance::money(vf_portal::pricing::package_price(fee, k))
                .trim_start_matches('$')
                .to_string();
        }
    }

    /// The priced snapshot to send (T29), or why it cannot be sent.
    fn fee_lines(&self) -> Result<Vec<delivery::FeeLineIn>, String> {
        let cents = |s: &str, what: &str| {
            vf_portal::balance::parse_money(s).ok_or_else(|| format!("{what}: dollars and cents"))
        };
        let line = |key: &str,
                    kind: &str,
                    name: &str,
                    text: &str,
                    c: u64,
                    rec: bool,
                    svc: Vec<vf_portal::pricing::Service>| delivery::FeeLineIn {
            key: key.into(),
            kind: kind.into(),
            name: name.into(),
            text: text.into(),
            cents: c,
            recommended: rec,
            services: svc,
            every: None,
        };
        let mut out = Vec::new();
        match self.fee_mode.as_str() {
            "amount" => out.push(line(
                "fee",
                "base",
                &format!("{} Form {}", self.year, self.return_type),
                "",
                cents(&self.amount, "The fee")?,
                false,
                vec![],
            )),
            mode => {
                if mode == "addons" {
                    let base = self
                        .plan
                        .as_ref()
                        .map(|p| p.base_name.clone())
                        .unwrap_or_else(|| "Tax return preparation".into());
                    out.push(line(
                        "base",
                        "base",
                        &base,
                        "",
                        cents(&self.return_fee, "The return fee")?,
                        false,
                        vec![],
                    ));
                } else {
                    for (k, shown, offered) in self.prices.iter().filter(|x| x.2) {
                        let _ = offered;
                        out.push(line(
                            &k.key,
                            "package",
                            &k.name,
                            "",
                            cents(shown, &k.name)?,
                            k.recommended,
                            k.services.clone(),
                        ));
                    }
                    if out.is_empty() {
                        return Err("Offer at least one package.".into());
                    }
                }
                for (a, on, price) in &self.addon_ticks {
                    if *on {
                        out.push(line(
                            &a.key,
                            "addon",
                            &a.name,
                            &a.text,
                            cents(price, &a.name)?,
                            false,
                            vec![],
                        ));
                    }
                }
            }
        }
        Ok(out)
    }

    /// The merge fields for this client and delivery.
    fn fields(&self) -> vf_letters::Fields {
        let mut f = vf_letters::Fields::new();
        let firm = Some(self.firm_name.clone())
            .filter(|f| !f.trim().is_empty())
            .or_else(|| vf_fs::cabinet::read(&self.root).ok().and_then(|c| c.name))
            .unwrap_or_default();
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
        let officer = self.signers.iter().find(|s| s.role == "officer");
        for (k, v) in [
            ("Client name", self.client.name.clone()),
            ("Client ID", self.client.code.clone()),
            ("Signer first names", firsts.join(" and ")),
            ("Taxpayer name", get("taxpayer")),
            ("Spouse name", get("spouse")),
            (
                "Signer name",
                officer.map(|s| s.name.clone()).unwrap_or_default(),
            ),
            (
                "Signer title",
                officer.map(|s| s.title.clone()).unwrap_or_default(),
            ),
            ("Tax year", self.year.to_string()),
            ("Next year", (self.year + 1).to_string()),
            ("Return type", self.return_type.clone()),
            ("Firm name", firm),
            (
                "Date",
                chrono::Local::now().format("%B %-d, %Y").to_string(),
            ),
        ] {
            f.insert(k.into(), v);
        }
        if self.with_fee {
            let fee = if self.fee_mode == "amount" {
                &self.amount
            } else {
                &self.return_fee
            };
            if let Some(c) = vf_portal::balance::parse_money(fee) {
                f.insert("Fee".into(), vf_portal::balance::money(c));
            }
            let options: Vec<String> = self
                .prices
                .iter()
                .filter(|x| x.2)
                .map(|(k, shown, _)| format!("{} ${shown}", k.name))
                .collect();
            if !options.is_empty() {
                f.insert("Fee options".into(), options.join("; "));
            }
        }
        f
    }

    /// The ticked letters laid out for this client: (template, laid), each with the firm's
    /// Terms and Conditions where it says (ADDENDUM-annual-agreements.md A5).
    fn laid_letters(&self) -> Vec<(vf_letters::Template, vf_letters::Laid)> {
        let f = self.fields();
        let all = vf_letters::store::load_all(&self.root).unwrap_or_default();
        let ctx = delivery::letter_context(&self.root, &self.return_type);
        self.letters
            .iter()
            .filter(|(_, on)| *on)
            .map(|(t, _)| {
                (
                    t.clone(),
                    vf_letters::layout(&vf_letters::store::as_sent(t, &all), &f, &ctx, None),
                )
            })
            .collect()
    }

    /// The fill-ins left in the ticked letters, as "letter: [X], [State]" (A17: Send refuses).
    fn fill_ins_left(&self) -> Option<String> {
        let all = vf_letters::store::load_all(&self.root).unwrap_or_default();
        let ctx = delivery::letter_context(&self.root, &self.return_type);
        let left: Vec<String> = self
            .letters
            .iter()
            .filter(|(_, on)| *on)
            .filter_map(|(t, _)| {
                let f = vf_letters::store::as_sent(t, &all).fill_ins(&ctx, None);
                (!f.is_empty()).then(|| format!("{}: {}", t.name, f.join(", ")))
            })
            .collect();
        (!left.is_empty()).then(|| {
            format!(
                "Fill in the letter template before sending (Admin › Portal › Letter templates). {}",
                left.join("; ")
            )
        })
    }

    fn load(&mut self, ctx: &egui::Context, s: &PortalSettings) {
        let s2 = s.clone();
        let id = self.client.id.clone();
        self.job = Some(spawn(ctx, move || {
            let p = core::client(&s2);
            Done::Loaded((|| {
                let mut ds = Vec::new();
                for d in p.deliveries(Some(&id))? {
                    // With its events (the list read leaves them out).
                    ds.push(p.delivery(&d.id)?);
                }
                // The client's annual agreements: one signed for this return skips its
                // letters (A11). None on a portal without agreements.
                ds.extend(p.agreements(&id).unwrap_or_default());
                let files = p.files_for_client(&id)?;
                let firm = p.status().map(|s| s.firm_name).unwrap_or_default();
                let staff = p.delivery_staff(&id).unwrap_or_default();
                Ok((ds, files, firm, staff))
            })())
        }));
    }

    /// The delivery this window is about: the open one for the year and return type chosen.
    fn current(&self) -> Option<&Delivery> {
        self.deliveries.iter().find(|d| {
            d.status == "open" && d.tax_year == self.year && d.return_type == self.return_type
        })
    }

    /// Fills the form from the delivery shown (or a default return file for a new one).
    fn fill(&mut self) {
        if let Some(d) = self.current().cloned() {
            self.title = d.title.clone();
            self.with_return = d.step(delivery::RETURN).is_some_and(|s| s.present);
            self.with_8879 = d.step(delivery::ESIGN).is_some_and(|s| s.present);
            self.with_letters = d.step(delivery::LETTERS).is_some_and(|s| s.present);
            self.with_fee = d.step(delivery::FEE).is_some_and(|s| s.present);
            self.assigned = d.assigned_staff_id.clone();
            self.remind_days = d.remind_days;
            if let Some(m) = d.fee.as_ref().and_then(|f| f.mode.clone()) {
                self.fee_mode = m;
            }
            if !d.signers.is_empty() {
                self.signers = d.signers.clone();
            }
            self.return_file = d.return_file.as_ref().map(|f| f.id.clone());
            // The cabinet document the portal's return file was shared from.
            self.return_doc = self
                .return_file
                .as_deref()
                .and_then(|id| self.files.iter().find(|f| f.file_id == id))
                .map(|f| f.vf_document_id.clone())
                .or(self.return_doc.clone());
            self.link = d.esign_link.clone().unwrap_or_default();
            self.notify = d.notify;
        } else if self.return_doc.is_none() {
            // The client's PDF for the year whose name says it is the return, those under
            // Income Tax first.
            let ty = self.return_type.to_lowercase();
            let year = self.year;
            let mut fits: Vec<&crate::ui::portal::DocRef> = self
                .docs
                .iter()
                .filter(|d| d.tax_year == Some(year))
                .filter(|d| {
                    let n = d.name.to_lowercase();
                    n.contains(&ty) || n.contains("return")
                })
                .collect();
            fits.sort_by_key(|d| {
                !d.path
                    .to_string_lossy()
                    .to_lowercase()
                    .contains("income tax")
            });
            self.return_doc = fits.first().map(|d| d.id.clone());
        }
        // A11: the engagement for this return was signed in the year's agreement.
        if self.current().is_none() && self.signed_agreement().is_some() {
            self.with_letters = false;
        }
    }

    /// The client's agreement covering this return (its returns' year and type) whose letters
    /// are signed (ADDENDUM-annual-agreements.md A11).
    fn signed_agreement(&self) -> Option<&Delivery> {
        self.deliveries.iter().find(|d| {
            d.is_agreement()
                && d.status != "cancelled"
                && d.return_year == Some(self.year)
                && d.covers
                    .iter()
                    .any(|c| c.eq_ignore_ascii_case(&self.return_type))
                && d.step(delivery::LETTERS).is_some_and(|s| s.state == "done")
        })
    }

    fn take_done(&mut self) {
        let Some(rx) = &self.job else { return };
        let Ok(done) = rx.try_recv() else { return };
        self.job = None;
        match done {
            Done::Loaded(Ok((ds, files, firm, staff))) => {
                self.firm_name = firm;
                self.staff = staff;
                self.reads.send(ds.clone());
                self.deliveries = ds;
                let mut files = files;
                files.sort_by(|a, b| b.shared_at.cmp(&a.shared_at));
                self.files = files;
                self.offline = None;
                self.fill();
            }
            Done::Loaded(Err(e)) => {
                if matches!(e, vf_portal::PortalError::Network(_))
                    && let Some(f) = &self.folder
                {
                    let copies = delivery::read_copies(&self.root, f);
                    if let Some(newest) = copies.iter().map(|c| c.read_at.clone()).max() {
                        self.offline = Some(format!(
                            "The portal cannot be reached: as last read {}.",
                            widgets::local_time(&newest)
                        ));
                        self.deliveries = copies.into_iter().map(|c| c.delivery).collect();
                        self.fill();
                    }
                }
                self.err = Some(e.message());
            }
            Done::Saved(Ok(d), what) => {
                let d = *d;
                self.audit.send(AuditEntry::new(
                    what,
                    None,
                    serde_json::json!({
                        "client_id": d.client_id,
                        "delivery_id": d.id,
                        "fields": ["steps", "return_file", "esign_link", "title", "notify"],
                    }),
                ));
                self.said = Some((
                    match what {
                        "portal_delivery_create" => "Sent to the client.".into(),
                        "portal_delivery_confirm_8879" => "Form 8879 confirmed.".into(),
                        "portal_delivery_cancel" => {
                            "Cancelled: the card has left the portal.".into()
                        }
                        _ => "Saved.".into(),
                    },
                    false,
                ));
                self.err = None;
                self.reads.send(vec![d.clone()]);
                match self.deliveries.iter().position(|x| x.id == d.id) {
                    Some(i) => self.deliveries[i] = d,
                    None => self.deliveries.insert(0, d),
                }
            }
            Done::Said(Ok(m)) => {
                self.said = Some((m, false));
                self.err = None;
            }
            Done::Saved(Err(e), _) if e == REVISED_ASK => {
                // Asked in the window, as for another document (T27).
                self.force_revise = true;
                self.confirm_revise = true;
            }
            Done::Saved(Err(e), _) | Done::Said(Err(e)) => self.err = Some(e),
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, s: &PortalSettings) {
        self.take_done();
        let busy = self.job.is_some();
        let mut open_win = true;
        let mut act = None;
        egui::Window::new(format!(
            "Deliver tax return — {} ({})",
            self.client.name, self.client.code
        ))
        .id(egui::Id::unique("portal-delivery"))
        .open(&mut open_win)
        .collapsible(false)
        .default_width(900.0)
        .default_height(820.0)
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
        let current = self.current().cloned();
        let sent = current.is_some();
        let can = !busy && self.offline.is_none();
        // The strip: year, return type, title, status.
        ui.horizontal_wrapped(|ui| {
            ui.label("Tax year");
            let before = (self.year, self.return_type.clone());
            ui.add_enabled(can, egui::DragValue::new(&mut self.year).range(1990..=2100));
            ui.label("Return type");
            ui.add_enabled_ui(can, |ui| {
                egui::ComboBox::from_id_salt("delivery-type")
                    .selected_text(&self.return_type)
                    .show_ui(ui, |ui| {
                        for t in self.types.clone() {
                            ui.selectable_value(&mut self.return_type, t.clone(), t);
                        }
                    });
            });
            if before != (self.year, self.return_type.clone()) {
                if before.1 != self.return_type {
                    self.offer_letters();
                }
                self.offer_fee();
                if self.current().is_none() {
                    self.title = format!("Your {} tax return", self.year);
                    self.return_file = None;
                    self.return_doc = None;
                }
                self.fill();
            }
            let (word, colour) = match current.as_ref().map(|d| d.status.as_str()) {
                Some("open") => ("Sent", theme::accent()),
                Some("complete") => ("Complete", theme::health_ok()),
                Some("cancelled") => ("Cancelled", theme::ink_muted()),
                _ => ("Not sent yet", theme::ink_muted()),
            };
            ui.label(RichText::new(word).strong().color(colour));
        });
        ui.horizontal(|ui| {
            ui.label("Card title");
            ui.add_enabled(
                can,
                egui::TextEdit::singleline(&mut self.title)
                    .desired_width(320.0)
                    .char_limit(120),
            );
        });
        ui.add_space(6.0);
        let step_state = |kind: &str| -> String {
            current
                .as_ref()
                .and_then(|d| d.step(kind))
                .filter(|s| s.present)
                .map(|s| delivery::state_words(&s.state).to_string())
                .unwrap_or_default()
        };
        // Section: letters to sign (Addendum D3).
        ui.group(|ui| {
            let letters_done = current
                .as_ref()
                .and_then(|d| d.step(delivery::LETTERS))
                .is_some_and(|s| s.state == "done");
            let sent_letters: Vec<delivery::DeliveryLetter> =
                current.as_ref().map(|d| d.letters.clone()).unwrap_or_default();
            ui.horizontal(|ui| {
                ui.add_enabled(
                    can && sent_letters.is_empty() && !letters_done,
                    egui::Checkbox::new(&mut self.with_letters, RichText::new("Letters to sign").strong()),
                );
                ui.label(RichText::new(step_state(delivery::LETTERS)).color(theme::ink_muted()));
            });
            if let Some(a) = self.signed_agreement() {
                let when = a
                    .step(delivery::LETTERS)
                    .and_then(|s| s.completed_at.clone())
                    .map(|t| widgets::local_time(&t))
                    .unwrap_or_default();
                ui.label(
                    RichText::new(format!(
                        "Engagement signed in the {} service agreement {when}: no letters needed (tick them to send some anyway).",
                        a.tax_year
                    ))
                    .color(theme::ink_secondary()),
                );
            }
            if !self.with_letters {
                return;
            }
            if !sent_letters.is_empty() {
                let signers = current.as_ref().map(|d| d.signers.clone()).unwrap_or_default();
                for l in &sent_letters {
                    let by: Vec<String> = l
                        .signatures
                        .iter()
                        .map(|g| {
                            let who = signers
                                .iter()
                                .find(|s| s.id == g.signer_id)
                                .map(|s| s.name.clone())
                                .unwrap_or_default();
                            format!("{who} {}", widgets::local_time(&g.signed_at))
                        })
                        .collect();
                    ui.label(format!(
                        "{} ({} page{}): {}",
                        l.name,
                        l.pages,
                        if l.pages == 1 { "" } else { "s" },
                        if by.is_empty() { "not signed yet".to_string() } else { format!("signed by {}", by.join(", ")) }
                    ));
                }
                return;
            }
            if self.letters.is_empty() {
                ui.label(widgets::caption("No letter templates for this return type: add them in Admin › Letter templates."));
            }
            for (tpl, on) in &mut self.letters {
                ui.add_enabled(can, egui::Checkbox::new(on, format!("{}{}", tpl.name, if tpl.sample { " (sample text)" } else { "" })));
            }
            ui.label(RichText::new("Who signs").small().color(theme::ink_muted()));
            for s in &mut self.signers {
                ui.horizontal(|ui| {
                    ui.label(vf_letters::role_words(&s.role));
                    widgets::named(
                        ui.add_enabled(can, egui::TextEdit::singleline(&mut s.name).desired_width(170.0)),
                        &format!("{} name", vf_letters::role_words(&s.role)),
                    );
                    if s.role == "officer" {
                        ui.add_enabled(
                            can,
                            egui::TextEdit::singleline(&mut s.title).desired_width(110.0).hint_text("Title"),
                        );
                    }
                    let mut email = s.email.clone().unwrap_or_default();
                    if ui
                        .add_enabled(can, egui::TextEdit::singleline(&mut email).desired_width(190.0).hint_text("Email"))
                        .changed()
                    {
                        s.email = Some(email.trim().to_string()).filter(|e| !e.is_empty());
                    }
                    let mut phone = s.phone.clone().unwrap_or_default();
                    if ui
                        .add_enabled(can, egui::TextEdit::singleline(&mut phone).desired_width(120.0).hint_text("Mobile"))
                        .changed()
                    {
                        s.phone = Some(phone.trim().to_string()).filter(|e| !e.is_empty());
                    }
                });
            }
            if self.signers.iter().skip(1).any(|s| s.email.is_none() && s.phone.is_none()) {
                ui.colored_label(
                    theme::warn(),
                    "A second signer needs their own email or mobile (their code goes there): add it here or in the client's Portal contacts.",
                );
            }
            if ui.add_enabled(!busy, egui::Button::new("Preview letters…")).clicked() {
                for (t, laid) in self.laid_letters() {
                    let dir = crate::ui::letter_templates::preview_dir();
                    let _ = std::fs::create_dir_all(&dir);
                    let p = dir.join(format!("{} - {}.pdf", self.client.code, vf_letters::store::safe_id(&t.id)));
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
        });
        // Section: the tax return PDF.
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.add_enabled(
                    can && !current
                        .as_ref()
                        .and_then(|d| d.step(delivery::RETURN))
                        .is_some_and(|s| s.state == "done"),
                    egui::Checkbox::new(&mut self.with_return, RichText::new("Tax return PDF").strong()),
                );
                ui.label(RichText::new(step_state(delivery::RETURN)).color(theme::ink_muted()));
            });
            if self.with_return {
                let shown = self
                    .return_doc
                    .as_deref()
                    .and_then(|id| self.docs.iter().find(|d| d.id == id))
                    .map(|d| d.name.clone())
                    .unwrap_or_else(|| "Choose the return…".into());
                ui.add_enabled_ui(can, |ui| {
                    egui::ComboBox::from_id_salt("delivery-return")
                        .selected_text(shown)
                        .width(420.0)
                        .show_ui(ui, |ui| {
                            for d in &self.docs {
                                ui.selectable_value(
                                    &mut self.return_doc,
                                    Some(d.id.clone()),
                                    format!(
                                        "{}{}",
                                        d.name,
                                        d.tax_year.map(|y| format!(" ({y})")).unwrap_or_default()
                                    ),
                                );
                            }
                        });
                });
                ui.label(widgets::caption(
                    "The client's PDFs in the cabinet. Sending shares the return on the portal with \
                     everyone of the client (B2 first, as Share on portal does); the client opens \
                     it when the card's earlier steps allow.",
                ));
            }
        });
        // Section: package and fee (Addendum D4/D5).
        ui.group(|ui| {
            let sent_fee = current
                .as_ref()
                .and_then(|d| d.fee.clone())
                .filter(|f| f.mode.is_some());
            let signed = sent_fee
                .as_ref()
                .is_some_and(|f| f.selection_signed_at.is_some());
            let paid = current
                .as_ref()
                .and_then(|d| d.step(delivery::FEE))
                .is_some_and(|s| s.state == "done");
            ui.horizontal(|ui| {
                ui.add_enabled(
                    can && sent_fee.is_none(),
                    egui::Checkbox::new(
                        &mut self.with_fee,
                        RichText::new("Package and fee").strong(),
                    ),
                );
                ui.label(RichText::new(step_state(delivery::FEE)).color(theme::ink_muted()));
            });
            if !self.with_fee {
                return;
            }
            if let Some(f) = &sent_fee {
                for l in f.lines.iter().filter(|l| {
                    f.mode.as_deref() != Some("packages")
                        || l.kind != "package"
                        || l.selected
                        || f.selection_signed_at.is_none()
                }) {
                    ui.label(format!(
                        "{}{}: {}",
                        l.name,
                        if l.selected { " (chosen)" } else { "" },
                        vf_portal::balance::money(l.cents)
                    ));
                }
                if signed {
                    ui.label(format!(
                        "Signed by {} · total {}",
                        f.selection_signed_by.clone().unwrap_or_default(),
                        vf_portal::balance::money(f.total_cents)
                    ));
                }
                let words = f.payment_words();
                ui.colored_label(
                    if words.starts_with("Bank payment failed") {
                        theme::error()
                    } else {
                        theme::ink_secondary()
                    },
                    words,
                );
                if !paid {
                    if ui
                        .add_enabled(can, egui::Button::new("Mark paid…"))
                        .clicked()
                    {
                        self.confirm_mark = true;
                    }
                    if self.confirm_mark {
                        ui.horizontal(|ui| {
                            ui.label("Note");
                            widgets::named(
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.mark_note)
                                        .desired_width(200.0)
                                        .hint_text("Check 1042"),
                                ),
                                "Mark paid note",
                            );
                            if ui.button("Mark paid").clicked() {
                                self.confirm_mark = false;
                                act = Some(Act::MarkPaid);
                            }
                            if ui.button("Keep it").clicked() {
                                self.confirm_mark = false;
                            }
                        });
                    }
                }
                return;
            }
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.fee_mode, "amount".to_string(), "One amount");
                ui.radio_value(
                    &mut self.fee_mode,
                    "addons".to_string(),
                    "Base fee and add-ons",
                );
                ui.radio_value(
                    &mut self.fee_mode,
                    "packages".to_string(),
                    "Packages and add-ons",
                );
            });
            if self.fee_mode == "amount" {
                ui.horizontal(|ui| {
                    ui.label("The fee $");
                    widgets::named(
                        ui.add(egui::TextEdit::singleline(&mut self.amount).desired_width(100.0)),
                        "Delivery fee",
                    );
                });
                return;
            }
            match &self.plan {
                Some(p) => {
                    ui.label(widgets::caption(format!(
                        "Pricing plan: {} {}",
                        p.tax_year, p.return_type
                    )));
                }
                None => {
                    ui.colored_label(
                        theme::warn(),
                        format!(
                            "No pricing plan for {} {}: add one in Admin › Portal › Pricing plans.",
                            self.year, self.return_type
                        ),
                    );
                }
            }
            ui.horizontal(|ui| {
                ui.label("This client's return fee $");
                if widgets::named(
                    ui.add(egui::TextEdit::singleline(&mut self.return_fee).desired_width(100.0)),
                    "Return fee",
                )
                .changed()
                {
                    self.reprice();
                }
            });
            if self.fee_mode == "packages" {
                let fee = vf_portal::balance::parse_money(&self.return_fee);
                for (k, shown, offered) in &mut self.prices {
                    ui.horizontal(|ui| {
                        ui.checkbox(offered, &k.name);
                        ui.label(widgets::caption(vf_portal::pricing::rule_words(k)));
                        ui.label("client sees $");
                        let worked = fee.map(|f| {
                            vf_portal::balance::money(vf_portal::pricing::package_price(f, k))
                        });
                        let differs = worked
                            .as_deref()
                            .is_some_and(|w| w.trim_start_matches('$') != shown.as_str());
                        let r = widgets::named(
                            ui.add(egui::TextEdit::singleline(shown).desired_width(90.0)),
                            &format!("{} price", k.name),
                        );
                        if differs {
                            ui.painter().rect_stroke(
                                r.rect.expand(2.0),
                                3.0,
                                egui::Stroke::new(1.5, theme::accent()),
                                egui::StrokeKind::Outside,
                            );
                            ui.label(widgets::caption(format!(
                                "plan: {}",
                                worked.unwrap_or_default()
                            )));
                        }
                        if k.recommended {
                            ui.label(widgets::caption("Recommended"));
                        }
                    });
                }
            }
            for (a, on, price) in &mut self.addon_ticks {
                ui.horizontal(|ui| {
                    ui.checkbox(on, &a.name);
                    ui.label("$");
                    ui.add(egui::TextEdit::singleline(price).desired_width(80.0));
                });
            }
            if let Err(e) = self.fee_lines() {
                ui.colored_label(theme::warn(), e);
            }
        });
        // Section: Form 8879.
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.add_enabled(
                    can && !current
                        .as_ref()
                        .and_then(|d| d.step(delivery::ESIGN))
                        .is_some_and(|s| s.state == "done"),
                    egui::Checkbox::new(&mut self.with_8879, RichText::new("Form 8879").strong()),
                );
                ui.label(RichText::new(step_state(delivery::ESIGN)).color(theme::ink_muted()));
            });
            if self.with_8879 {
                ui.horizontal(|ui| {
                    ui.label("UltraTax eSignature link");
                    widgets::named(
                        ui.add_enabled(
                            can,
                            egui::TextEdit::singleline(&mut self.link)
                                .desired_width(380.0)
                                .hint_text("https://…"),
                        ),
                        "eSignature link",
                    );
                });
                let waiting = current
                    .as_ref()
                    .and_then(|d| d.step(delivery::ESIGN))
                    .is_some_and(|s| s.state == "waiting");
                if waiting {
                    ui.colored_label(theme::warn(), "The client says they signed it.");
                }
                if ui
                    .add_enabled(can && waiting, egui::Button::new("Confirm signed"))
                    .on_hover_text("You checked UltraTax: Form 8879 is signed")
                    .clicked()
                {
                    act = Some(Act::Confirm);
                }
            }
        });
        ui.checkbox(
            &mut self.notify,
            "Tell the client by email and text when it is sent",
        );
        ui.horizontal(|ui| {
            ui.label("Remind the client");
            egui::ComboBox::from_id_salt("delivery-remind")
                .selected_text(match self.remind_days {
                    0 => "never".to_string(),
                    n => format!("every {n} days while a step waits"),
                })
                .show_ui(ui, |ui| {
                    for n in [0u32, 3, 7, 14] {
                        let words = if n == 0 {
                            "never".to_string()
                        } else {
                            format!("every {n} days while a step waits")
                        };
                        ui.selectable_value(&mut self.remind_days, n, words);
                    }
                });
            ui.label("Assigned to");
            let shown = self
                .assigned
                .as_deref()
                .and_then(|id| self.staff.iter().find(|s| s.id == id))
                .map(|s| s.name.clone())
                .unwrap_or_else(|| "Nobody (the admins)".into());
            egui::ComboBox::from_id_salt("delivery-assigned")
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.assigned, None, "Nobody (the admins)");
                    for s in &self.staff {
                        ui.selectable_value(&mut self.assigned, Some(s.id.clone()), &s.name);
                    }
                });
        });
        let link_ok = !self.with_8879
            || self.link.trim().is_empty()
            || self.link.trim().to_lowercase().starts_with("https://");
        if !link_ok {
            ui.colored_label(theme::error(), "The eSignature link starts with https://.");
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    can && sent,
                    egui::Button::new(RichText::new("Cancel delivery…").color(theme::error())),
                )
                .clicked()
            {
                self.confirm_cancel = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = if sent {
                    "Save changes"
                } else {
                    "Send to client"
                };
                if ui
                    .add_enabled(
                        can && link_ok
                            && (self.with_return
                                || self.with_8879
                                || self.with_letters
                                || self.with_fee),
                        widgets::primary_button(label),
                    )
                    .clicked()
                {
                    act = Some(Act::Send);
                }
                if ui
                    .add_enabled(can && sent, egui::Button::new("Remind now"))
                    .clicked()
                {
                    act = Some(Act::Remind);
                }
                if ui
                    .add_enabled(!busy && sent, egui::Button::new("Download report…"))
                    .clicked()
                {
                    act = Some(Act::Report);
                }
                if ui
                    .add_enabled(!busy, egui::Button::new("Read again"))
                    .clicked()
                {
                    act = Some(Act::Reload);
                }
            });
        });
        if self.confirm_revise {
            ui.group(|ui| {
                ui.label("Tell the client the return was updated and reopen the Form 8879 step? The letters, package and fee stand; paste a new eSignature link if UltraTax made one.");
                ui.horizontal(|ui| {
                    if ui.button("Send the revised return").clicked() {
                        act = Some(Act::Send);
                    }
                    if ui.button("Keep the first").clicked() {
                        self.confirm_revise = false;
                    }
                });
            });
        }
        if self.confirm_cancel {
            ui.group(|ui| {
                ui.label("Cancel this delivery? The card leaves the client's portal. What was done stays in its log.");
                ui.horizontal(|ui| {
                    if ui.button("Cancel delivery").clicked() {
                        self.confirm_cancel = false;
                        act = Some(Act::Cancel);
                    }
                    if ui.button("Keep it").clicked() {
                        self.confirm_cancel = false;
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
        match &current {
            Some(d) if !d.events.is_empty() => {
                for e in &d.events {
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
            }
            _ => {
                ui.label(widgets::caption("Nothing yet."));
            }
        }
        ui.label(widgets::caption(format!(
            "Each finished step's PDF, with its log page, is filed in the client's Income Tax\\{}\\Delivery folder.",
            self.year
        )));
        act
    }

    fn apply(&mut self, ctx: &egui::Context, s: &PortalSettings, act: Act) {
        self.said = None;
        self.err = None;
        let s2 = s.clone();
        let current = self.current().cloned();
        match act {
            Act::Reload => self.load(ctx, s),
            Act::Send => {
                if self.with_letters
                    && current.as_ref().is_none_or(|d| d.letters.is_empty())
                    && let Some(e) = self.fill_ins_left()
                {
                    self.err = Some(e);
                    return;
                }
                let steps = StepsIn {
                    letters: self.with_letters,
                    return_: self.with_return,
                    package: self.with_fee && self.fee_mode != "amount",
                    fee: self.with_fee,
                    esign8879: self.with_8879,
                };
                // The priced snapshot and each offered package's addendum (T20), while the
                // selection is not signed.
                let fee = if self.with_fee
                    && current.as_ref().is_none_or(|d| {
                        d.fee
                            .as_ref()
                            .is_none_or(|f| f.selection_signed_at.is_none())
                    }) {
                    match self.fee_lines() {
                        Ok(lines) => Some((self.fee_mode.clone(), lines)),
                        Err(e) => {
                            self.err = Some(e);
                            return;
                        }
                    }
                } else {
                    None
                };
                let addenda: Vec<(String, vf_letters::Template, vf_letters::Laid)> = match &fee {
                    Some((mode, lines)) if mode == "packages" && current.is_none() => {
                        let templates = vf_letters::store::load_all(&self.root).unwrap_or_default();
                        let base = self.fields();
                        lines
                            .iter()
                            .filter(|l| l.kind == "package")
                            .filter_map(|l| {
                                let t = templates.iter().find(|t| {
                                    t.kind == "addendum"
                                        && t.package
                                            .as_deref()
                                            .is_some_and(|p| p.eq_ignore_ascii_case(&l.name))
                                })?;
                                let mut f = base.clone();
                                f.insert("Package name".into(), l.name.clone());
                                f.insert("Package fee".into(), vf_portal::balance::money(l.cents));
                                let laid = vf_letters::layout(
                                    t,
                                    &f,
                                    &delivery::letter_context(&self.root, &self.return_type),
                                    Some(&l.name),
                                );
                                Some((l.key.clone(), t.clone(), laid))
                            })
                            .collect()
                    }
                    _ => Vec::new(),
                };
                // The letters, laid out now (sent once, while the delivery has none).
                let letters: Vec<(vf_letters::Template, vf_letters::Laid)> =
                    if self.with_letters && current.as_ref().is_none_or(|d| d.letters.is_empty()) {
                        self.laid_letters()
                    } else {
                        Vec::new()
                    };
                let signers = self.signers.clone();
                let with_letters = self.with_letters;
                let assigned = self.assigned.clone();
                let remind_days = self.remind_days;
                // T27: a return the client saw, replaced: asked first in the window.
                let shown = current
                    .as_ref()
                    .and_then(|d| d.return_file.as_ref())
                    .and_then(|r| self.files.iter().find(|f| f.file_id == r.id));
                let chosen_doc = self
                    .return_doc
                    .as_deref()
                    .and_then(|id| self.docs.iter().find(|d| d.id == id));
                let other_content = match (shown, chosen_doc) {
                    (Some(f), Some(d)) => {
                        f.vf_document_id != d.id || (!d.hash.is_empty() && f.sha256 != d.hash)
                    }
                    (None, Some(_)) => true,
                    _ => false,
                };
                let seen = self.force_revise
                    || current.as_ref().is_some_and(|d| {
                        d.step(delivery::RETURN).is_some_and(|s| s.state == "done")
                            && other_content
                            && self.with_return
                    });
                if seen && !self.confirm_revise {
                    self.confirm_revise = true;
                    return;
                }
                self.confirm_revise = false;
                self.force_revise = false;
                let title = self.title.trim().to_string();
                let doc = self
                    .return_doc
                    .as_deref()
                    .and_then(|id| self.docs.iter().find(|d| d.id == id))
                    .cloned()
                    .filter(|_| self.with_return);
                let b2 = self.b2.clone();
                let client = self.client.clone();
                let share_audit = self.audit.clone();
                let link = self.link.trim().to_string();
                let link = (self.with_8879 && !link.is_empty()).then_some(link);
                let notify = self.notify;
                let new = NewDelivery {
                    client_id: self.client.id.clone(),
                    tax_year: self.year,
                    return_type: self.return_type.clone(),
                    title: title.clone(),
                    steps: steps.clone(),
                    locks: delivery::Locks {
                        return_after_letters: true,
                        esign_after_letters: true,
                        esign_after_fee: true,
                    },
                    assigned_staff_id: self.assigned.clone(),
                    notify,
                    remind_days: self.remind_days,
                    kind: None,
                    covers: Vec::new(),
                    return_year: None,
                };
                let what = if current.is_none() {
                    "portal_delivery_create"
                } else {
                    "portal_delivery_change"
                };
                self.job = Some(spawn(ctx, move || {
                    let p = core::client(&s2);
                    let r = (|| {
                        let mut d = match &current {
                            None => p.create_delivery(&new)?,
                            Some(d) => p.change_delivery(
                                &d.id,
                                &DeliveryChange {
                                    title: Some(title),
                                    steps: Some(steps),
                                    notify: Some(notify),
                                    assigned_staff_id: Some(assigned),
                                    remind_days: Some(remind_days),
                                    ..Default::default()
                                },
                            )?,
                        };
                        // The return (Q15): put on the portal for nobody yet, set as the
                        // delivery's return (so it is locked), then shown to everyone of the
                        // client. A return already there with the same content is left as it
                        // is (staff may have limited who sees it).
                        if let Some(mut doc) = doc {
                            doc.hash =
                                filer_intake::commit::sha256_file(&doc.path).map_err(|e| {
                                    vf_portal::PortalError::Local(format!("{}: {e}", doc.name))
                                })?;
                            let is_return = |d: &Delivery, id: &str| {
                                d.return_file.as_ref().is_some_and(|r| r.id == id)
                            };
                            let done = d.step(delivery::RETURN).is_some_and(|s| s.state == "done");
                            // A file no one sees yet (a Send that stopped before it was shown)
                            // is shown to everyone; one staff limited stays as it is.
                            let unseen = |f: &vf_portal::api::PortalFile| {
                                f.invites.is_empty() && f.users.iter().all(|u| !u.visible)
                            };
                            let (file_id, added, replaced) = match p.file_for_doc(&doc.id)? {
                                Some(f) if f.sha256 == doc.hash => {
                                    let nobody = unseen(&f);
                                    (f.file_id, nobody, false)
                                }
                                Some(f) => {
                                    let nobody = unseen(&f);
                                    // New content for a return the client opened: only as a
                                    // revised return, asked for in the window.
                                    if done && is_return(&d, &f.file_id) && !seen {
                                        return Err(vf_portal::PortalError::Local(
                                            REVISED_ASK.into(),
                                        ));
                                    }
                                    let id = f.file_id.clone();
                                    crate::ui::portal::share_docs(
                                        &p,
                                        &b2,
                                        &client,
                                        vec![(doc.clone(), Some(f))],
                                        &Default::default(),
                                        &Default::default(),
                                        true,
                                        &share_audit,
                                    )
                                    .map_err(vf_portal::PortalError::Local)?;
                                    (id, nobody, true)
                                }
                                None => {
                                    b2.push(std::slice::from_ref(&doc.id))
                                        .map_err(vf_portal::PortalError::Local)?;
                                    let a = p.add(
                                        &client.id,
                                        &doc.id,
                                        &doc.name,
                                        doc.tax_year,
                                        &doc.path,
                                    )?;
                                    share_audit.send(AuditEntry::new(
                                        "portal_share",
                                        Some(&doc.id),
                                        serde_json::json!({
                                            "client_id": client.id,
                                            "sent": true,
                                            "updated": false,
                                        }),
                                    ));
                                    (a.file_id().to_string(), true, false)
                                }
                            };
                            if !is_return(&d, &file_id) || (replaced && done) {
                                d = if seen {
                                    p.revise_return(&d.id, &file_id)?
                                } else {
                                    p.set_delivery_return(&d.id, &file_id)?
                                };
                            }
                            // Shown to everyone of the client once it is the delivery's
                            // return (locked), for a file new on the portal or seen by no one.
                            if added {
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
                                    p.set_visibility_with(&file_id, &all, &held)?;
                                    share_audit.send(AuditEntry::new(
                                        "portal_share",
                                        Some(&doc.id),
                                        serde_json::json!({
                                            "client_id": client.id,
                                            "shown_to": all.keys().collect::<Vec<_>>(),
                                            "hidden_from": [],
                                            "held_for_invitations": held.keys().collect::<Vec<_>>(),
                                        }),
                                    ));
                                }
                            }
                        }
                        if let Some(l) = link
                            && d.esign_link.as_deref() != Some(l.as_str())
                        {
                            d = p.set_esign_link(&d.id, &l)?;
                        }
                        if let Some((mode, lines)) = &fee {
                            d = p.set_fee(&d.id, mode, lines)?;
                            for (key, t, laid) in &addenda {
                                let blocks =
                                    serde_json::to_string(&laid.blocks).expect("serializes");
                                d = p.add_letter_to(
                                    &d.id,
                                    "package",
                                    Some(key),
                                    &t.name,
                                    &t.doc_type,
                                    &t.id,
                                    t.version,
                                    &blocks,
                                    &laid.pdf,
                                )?;
                            }
                        }
                        if with_letters && d.letters.is_empty() {
                            for (t, laid) in &letters {
                                let blocks =
                                    serde_json::to_string(&laid.blocks).expect("serializes");
                                d = p.add_letter(
                                    &d.id,
                                    &t.name,
                                    &t.doc_type,
                                    &t.id,
                                    t.version,
                                    &blocks,
                                    &laid.pdf,
                                )?;
                            }
                            d = p.set_signers(&d.id, &signers)?;
                        }
                        p.delivery(&d.id)
                    })();
                    Done::Saved(r.map(Box::new).map_err(|e| e.message()), what)
                }));
            }
            Act::MarkPaid => {
                let Some(d) = current else { return };
                let note = self.mark_note.trim().to_string();
                self.job = Some(spawn(ctx, move || {
                    let p = core::client(&s2);
                    let r = p.mark_paid(&d.id, &note).and_then(|x| p.delivery(&x.id));
                    Done::Saved(
                        r.map(Box::new).map_err(|e| e.message()),
                        "portal_delivery_mark_paid",
                    )
                }));
            }
            Act::Confirm => {
                let Some(d) = current else { return };
                self.job = Some(spawn(ctx, move || {
                    let p = core::client(&s2);
                    let r = p.confirm_8879(&d.id).and_then(|x| p.delivery(&x.id));
                    Done::Saved(
                        r.map(Box::new).map_err(|e| e.message()),
                        "portal_delivery_confirm_8879",
                    )
                }));
            }
            Act::Cancel => {
                let Some(d) = current else { return };
                self.job = Some(spawn(ctx, move || {
                    let p = core::client(&s2);
                    let r = p.cancel_delivery(&d.id).and_then(|x| p.delivery(&x.id));
                    Done::Saved(
                        r.map(Box::new).map_err(|e| e.message()),
                        "portal_delivery_cancel",
                    )
                }));
            }
            Act::Remind => {
                let Some(d) = current else { return };
                let audit = self.audit.clone();
                self.job = Some(spawn(ctx, move || {
                    Done::Said(
                        core::client(&s2)
                            .remind_delivery(&d.id)
                            .map(|n| {
                                audit.send(AuditEntry::new(
                                    "portal_delivery_remind",
                                    None,
                                    serde_json::json!({ "client_id": d.client_id, "delivery_id": d.id, "told": n }),
                                ));
                                match n {
                                    0 => "Nobody could be told: invite someone to the portal first.".to_string(),
                                    1 => "1 message sent.".into(),
                                    n => format!("{n} messages sent."),
                                }
                            })
                            .map_err(|e| e.message()),
                    )
                }));
            }
            Act::Report => {
                let Some(d) = current else { return };
                let Some(dest) = rfd::FileDialog::new()
                    .set_title("Save the delivery report")
                    .set_file_name(format!(
                        "{} {} Delivery report - {}.pdf",
                        d.tax_year, d.return_type, self.client.code
                    ))
                    .add_filter("PDF", &["pdf"])
                    .save_file()
                else {
                    return;
                };
                if dest.starts_with(&self.root) {
                    self.err = Some("Save the report outside the cabinet; the step PDFs are filed in the client's folder already.".into());
                    return;
                }
                let audit = self.audit.clone();
                self.job = Some(spawn(ctx, move || {
                    Done::Said(
                        core::client(&s2)
                            .delivery_report(&d.id)
                            .map_err(|e| e.message())
                            .and_then(|bytes| {
                                filer_core::datafile::write_atomic(&dest, &bytes)
                                    .map_err(|e| format!("The report could not be saved: {e}"))
                            })
                            .map(|()| {
                                audit.send(AuditEntry::new(
                                    "portal_delivery_report",
                                    None,
                                    serde_json::json!({ "client_id": d.client_id, "delivery_id": d.id }),
                                ));
                                format!("Saved {}.", dest.display())
                            }),
                    )
                }));
            }
        }
    }
}

/// Said by the Send job when the return's content changed since the client opened it: the
/// window then asks, as for another document.
const REVISED_ASK: &str = "The return changed since the client opened it.";

/// The client's PDFs in the catalog, newest year first (the Deliver window's return).
pub fn client_pdfs(
    conn: &vf_core::db::Connection,
    root: &Path,
    client_id: &str,
) -> Vec<crate::ui::portal::DocRef> {
    let Ok(mut st) = conn.prepare(
        "SELECT id, rel_path, tax_year, hash FROM document
         WHERE client_id = ?1 AND deleted_at IS NULL AND lower(rel_path) LIKE '%.pdf'
         ORDER BY tax_year DESC, rel_path",
    ) else {
        return Vec::new();
    };
    st.query_map([client_id], |r| {
        let rel: String = r.get(1)?;
        Ok(crate::ui::portal::DocRef {
            id: r.get(0)?,
            name: widgets::file_name(&rel).to_string(),
            path: root.join(&rel),
            hash: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
            tax_year: r.get(2)?,
        })
    })
    .map(|rows| rows.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

// ---- Admin › Portal › Deliveries ---------------------------------------------------------------

#[derive(Default)]
pub struct DeliveriesPage {
    filter: Option<&'static str>,
    data: Option<Vec<Delivery>>,
    clients: HashMap<String, (ClientRef, Option<String>)>,
    job: Option<Receiver<Result<Vec<Delivery>, String>>>,
    loaded: bool,
    err: Option<String>,
    window: Option<DeliverWindow>,
    /// An agreement's row opens the client's Service agreement… window.
    agreement: Option<crate::ui::agreement::AgreementWindow>,
    audit: Option<AuditOut>,
    reads: Option<ReadOut>,
    /// Browse's B2-first state (the return is shared through B2 first).
    pub b2: crate::ui::portal::B2First,
    /// The portal settings, read again every 30 s (not every frame; load test 2026-11-24).
    settings: Option<(std::time::Instant, Option<vf_portal::PortalSettings>)>,
}

impl DeliveriesPage {
    pub fn reread(&mut self) {
        self.loaded = false;
        self.settings = None;
    }
    pub fn connected(&self) -> bool {
        self.audit.is_some()
    }
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

/// Whole days since an RFC 3339 time.
fn days_since(at: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(at)
        .map(|t| (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_days())
        .unwrap_or(0)
}

pub fn page(ui: &mut egui::Ui, page: &mut DeliveriesPage, env: &mut crate::ui::browse::Env) {
    ui.heading("Deliveries");
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
    let settings = page.settings.as_ref().and_then(|(_, s)| s.clone());
    let Some(s) = settings.filter(|s| s.deliveries) else {
        ui.label("Tax return delivery is off for this cabinet. Turn it on in Admin › Portal.");
        return;
    };
    ui.label(widgets::caption(
        "Every open tax return delivery and the step it waits on: the season's worklist. Click a \
         client to open its Deliver tax return window.",
    ));
    let ctx = ui.ctx().clone();
    if let Some(rx) = &page.job
        && let Ok(r) = rx.try_recv()
    {
        page.job = None;
        match r {
            Ok(d) => {
                if let Some(o) = &page.reads {
                    o.send(d.clone());
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
            core::client(&s2)
                .deliveries_all(None)
                .map_err(|e| e.message())
        }));
    }
    ui.horizontal(|ui| {
        ui.selectable_value(&mut page.filter, None, "All open");
        for (k, name) in [
            (delivery::RETURN, "Tax return"),
            (delivery::ESIGN, "Form 8879"),
        ] {
            ui.selectable_value(&mut page.filter, Some(k), name);
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
    let mut open: Option<(String, bool)> = None;
    if let Some(list) = &page.data {
        let rows: Vec<&Delivery> = list
            .iter()
            .filter(|d| d.status == "open")
            .filter(|d| {
                page.filter
                    .is_none_or(|f| d.waiting().is_some_and(|s| s.kind == f))
            })
            .collect();
        if rows.is_empty() {
            ui.label(widgets::caption("None."));
        }
        // Only the rows on screen are drawn (500 open deliveries in a season: load test
        // 2026-11-24).
        use egui_extras::{Column, TableBuilder};
        TableBuilder::new(ui)
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(
                Column::initial(320.0)
                    .at_least(160.0)
                    .clip(true)
                    .resizable(true),
            )
            .column(Column::exact(50.0))
            .column(Column::exact(70.0))
            .column(Column::initial(260.0).at_least(140.0).clip(true))
            .column(Column::remainder().at_least(50.0))
            .max_scroll_height(f32::INFINITY)
            .header(22.0, |mut h| {
                for name in ["Client", "Year", "Kind", "Step waiting", "Days"] {
                    h.col(|ui| {
                        ui.label(RichText::new(name).small().color(theme::ink_muted()));
                    });
                }
            })
            .body(|body| {
                body.rows(24.0, rows.len(), |mut row| {
                    let d = rows[row.index()];
                    row.col(|ui| {
                        let name = page
                            .clients
                            .get(&d.client_id)
                            .map(|(c, _)| format!("{} ({})", c.name, c.code))
                            .unwrap_or_else(|| "(not in this cabinet)".into());
                        if ui.link(name).clicked() {
                            open = Some((d.client_id.clone(), d.is_agreement()));
                        }
                    });
                    row.col(|ui| {
                        ui.label(d.tax_year.to_string());
                    });
                    row.col(|ui| {
                        ui.label(d.what());
                    });
                    row.col(|ui| {
                        ui.label(
                            d.waiting()
                                .map(|s| {
                                    format!("{} · {}", s.name, delivery::state_words(&s.state))
                                })
                                .unwrap_or_default(),
                        );
                    });
                    row.col(|ui| {
                        ui.label(days_since(&d.created_at).to_string());
                    });
                });
            });
    }
    if let Some((id, true)) = &open
        && let Some((c, _)) = page.clients.get(id).cloned()
        && let (Some(audit), Some(reads)) = (page.audit.clone(), page.reads.clone())
    {
        page.agreement = Some(crate::ui::agreement::AgreementWindow::new(
            &ctx,
            &s,
            env.root.to_path_buf(),
            c,
            audit,
            reads,
        ));
    } else if let Some((id, false)) = open
        && let Some((c, folder)) = page.clients.get(&id).cloned()
        && let (Some(audit), Some(reads)) = (page.audit.clone(), page.reads.clone())
    {
        let docs = client_pdfs(env.conn, env.root, &c.id);
        page.window = Some(DeliverWindow::new(
            &ctx,
            &s,
            env.root.to_path_buf(),
            c,
            folder,
            audit,
            reads,
            docs,
            page.b2.clone(),
        ));
    }
    if let Some(w) = &mut page.window {
        w.show(&ctx, &s);
        if w.closed {
            page.window = None;
            page.loaded = false;
        }
    }
    if let Some(w) = &mut page.agreement {
        w.show(&ctx, &s);
        if w.closed {
            page.agreement = None;
            page.loaded = false;
        }
    }
}

/// The client panel's line for one client, from the deliveries last read on this PC.
pub fn panel_line(read: Option<&Vec<Delivery>>, client_id: &str) -> Option<String> {
    let d = read?
        .iter()
        .filter(|d| d.client_id == client_id && d.status == "open" && !d.is_agreement())
        .max_by(|a, b| a.created_at.cmp(&b.created_at))?;
    Some(format!("Tax return delivery: {}", d.line()))
}

/// The client panel's lines for annual agreements: the newest agreement, and the monthly
/// charge.
pub fn agreement_lines(
    read: Option<&Vec<Delivery>>,
    subs: Option<&Vec<vf_portal::agreement::Subscription>>,
    client_id: &str,
) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(d) = read.and_then(|r| {
        r.iter()
            .filter(|d| d.client_id == client_id && d.is_agreement() && d.status != "cancelled")
            .max_by_key(|d| d.tax_year)
    }) {
        out.push(format!("Service agreement: {}", d.line()));
    }
    if let Some(s) = subs.and_then(|s| s.iter().find(|s| s.client_id == client_id && s.live())) {
        out.push(format!("Monthly charge: {}", s.words()));
    }
    out
}

#[cfg(test)]
mod tests {
    /// Intake's receipt and step rules follow the switches as Admin › Portal saves them (Q11):
    /// the settings file Vibe Files writes is the one Intake reads.
    #[test]
    fn intake_reads_the_switches_as_saved() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        std::fs::create_dir_all(root.join("_Vibe")).unwrap();
        assert_eq!(vf_ingest::parsers::portal_features(root), (false, false));
        for (b, d) in [(true, true), (true, false), (false, true)] {
            let s = vf_portal::PortalSettings::new(
                "cab",
                "https://portal.example.test",
                "vpk_abcdefghijklmnopqrstuvwxyz0123456789",
                0,
            )
            .unwrap()
            .with_balances(b)
            .with_deliveries(d);
            vf_portal::settings::save(root, &s).unwrap();
            assert_eq!(vf_ingest::parsers::portal_features(root), (b, d));
        }
    }
}
