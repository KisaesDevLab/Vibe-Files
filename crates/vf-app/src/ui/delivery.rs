//! Tax return delivery (ADDENDUM-tax-return-delivery.md §12, owner 2026-11-24; Vibe-Portal
//! D1/D2): the client panel's Deliver… window and Admin › Portal › Deliveries.
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
use vf_portal::agreement::Subscription;
use vf_portal::api::PortalFile;
use vf_portal::delivery::{self, Delivery, DeliveryChange, DeliverySigner, NewDelivery, StepsIn};
use vf_portal::worklist::{self, Show};

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

// ---- The Deliver… window (tax return, financial statements, another document) ------------------

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
    /// Owner 2026-11-26: the client signed this step (letters or Form 8879) on paper.
    Paper(&'static str),
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
    /// PA-114: `return`, `statements` or `document`.
    kind: String,
    /// An Other document's name ("Payroll reports"): its return type on the portal.
    doc_name: String,
    year: i32,
    return_type: String,
    title: String,
    /// PA-114: the documents open only after the fee is paid.
    pay_first: bool,
    /// PA-114: the documents after the first (cabinet document ids; `None` not chosen yet).
    extra_docs: Vec<Option<String>>,
    /// QA 2026-11-26: the card's documents whose cabinet document is not found (trashed, moved
    /// to another client): kept on the card as they are unless Removed, never dropped unsaid.
    kept: Vec<(String, String)>,
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
    /// Send another was pressed on a finished card: the form is a new card's (owner 2026-11-26).
    fresh: bool,
    /// Revise the return… on a finished card: the form is editable to send a revision.
    revising: bool,
    /// Send another after a card whose fee was paid: Fee starts unticked, and says why.
    another_paid: bool,
    /// Owner 2026-11-26: how Form 8879 is signed: `link`, `office`, `mail`.
    esign_mode: String,
    /// A question (revise, cancel) was on screen last frame: it is scrolled into view once.
    ask_shown: bool,
    /// Signed on paper…: the step asked about, and the note.
    paper: Option<&'static str>,
    paper_note: String,
    job: Option<Receiver<Done>>,
    said: Option<(String, bool)>,
    err: Option<String>,
    audit: AuditOut,
    reads: ReadOut,
    /// Admin › Portal's "Assign new cards to" (owner 2026-11-26), put in Assigned to once, for
    /// a new card.
    default_staff: Option<vf_portal::settings::DefaultStaff>,
    default_applied: bool,
    /// Owner 2026-11-26: the letters' fill-ins typed for this client, by template id.
    fills: HashMap<String, std::collections::BTreeMap<String, String>>,
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
            kind: delivery::KIND_RETURN.into(),
            doc_name: String::new(),
            year,
            return_type: "1040".into(),
            title: format!("Your {year} tax return"),
            pay_first: false,
            extra_docs: Vec::new(),
            kept: Vec::new(),
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
            fresh: false,
            revising: false,
            another_paid: false,
            paper: None,
            esign_mode: "link".into(),
            ask_shown: false,
            paper_note: String::new(),
            job: None,
            said: None,
            err: None,
            audit,
            reads,
            default_staff: s.default_staff.clone(),
            default_applied: false,
            fills: HashMap::new(),
        };
        w.offer_letters();
        w.offer_fee();
        w.load(ctx, s);
        w
    }

    /// Shows an existing card (from the Deliveries page): its kind, year and return type.
    pub fn focus(&mut self, kind: &str, year: i32, return_type: &str) {
        self.kind = match kind {
            delivery::KIND_STATEMENTS | delivery::KIND_DOCUMENT => kind.to_string(),
            _ => delivery::KIND_RETURN.into(),
        };
        self.year = year;
        match self.kind.as_str() {
            delivery::KIND_RETURN => self.return_type = return_type.to_string(),
            delivery::KIND_DOCUMENT => self.doc_name = return_type.to_string(),
            _ => {}
        }
        if self.kind == delivery::KIND_RETURN {
            // QA 2026-11-26: a tax return's defaults (a card read fills its own).
            self.with_8879 = true;
            self.fee_mode = "packages".into();
        } else {
            self.with_8879 = false;
        }
        self.title = default_title(&self.kind, self.year, &self.rt());
        self.offer_letters();
        self.offer_fee();
        self.fill();
    }

    /// The card's return type on the portal: the return's, "Financial statements", or the
    /// document's name.
    fn rt(&self) -> String {
        rt_of(&self.kind, &self.return_type, &self.doc_name)
    }

    /// The letters offered for the return type (ticked as the template says) and who signs,
    /// from the client's Portal contacts (PA-105): taxpayer and spouse on a 1040, one person
    /// with a title for an entity (T22).
    fn offer_letters(&mut self) {
        let _ = vf_letters::store::seed_if_missing(&self.root);
        self.letters = vf_letters::store::load_all(&self.root)
            .unwrap_or_default()
            .into_iter()
            .filter(|t| t.kind == "letter" && t.offered_for(&self.rt()))
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
        self.signers = if vf_portal::delivery::is_individual(&self.root, &self.rt()) {
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
        // PA-114: pricing plans (packages, add-ons) are for tax returns; another card's fee is
        // one amount.
        self.plan = if self.kind == delivery::KIND_RETURN {
            vf_portal::pricing::load(&self.root, &self.return_type, self.year)
        } else {
            self.fee_mode = "amount".into();
            None
        };
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
                &fee_name(&self.kind, self.year, &self.rt()),
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
            ("Return type", self.rt()),
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
    /// A letter with the fill-ins typed for this client in place.
    fn filled(&self, t: &vf_letters::Template) -> vf_letters::Template {
        self.fills
            .get(&t.id)
            .map_or_else(|| t.clone(), |v| t.filled(v))
    }

    fn laid_letters(&self) -> Vec<(vf_letters::Template, vf_letters::Laid)> {
        let f = self.fields();
        let all = vf_letters::store::load_all(&self.root).unwrap_or_default();
        let ctx = delivery::letter_context(&self.root, &self.rt());
        self.letters
            .iter()
            .filter(|(_, on)| *on)
            .map(|(t, _)| {
                (
                    t.clone(),
                    vf_letters::layout(
                        &vf_letters::store::as_sent(&self.filled(t), &all),
                        &f,
                        &ctx,
                        None,
                    ),
                )
            })
            .collect()
    }

    /// The fill-ins left in the ticked letters, as "letter: [X], [State]" (A17: Send refuses).
    fn fill_ins_left(&self) -> Option<String> {
        let all = vf_letters::store::load_all(&self.root).unwrap_or_default();
        let ctx = delivery::letter_context(&self.root, &self.rt());
        let left: Vec<String> = self
            .letters
            .iter()
            .filter(|(_, on)| *on)
            .filter_map(|(t, _)| {
                let f = vf_letters::store::as_sent(&self.filled(t), &all).fill_ins(&ctx, None);
                (!f.is_empty()).then(|| format!("{}: {}", t.name, f.join(", ")))
            })
            .collect();
        (!left.is_empty()).then(|| {
            format!(
                "Fill in the letters before sending: type them under the letter, or change the template (Admin › Portal › Letter templates). {}",
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
                // Every kind (PA-114: statements and other documents too), with its events (the
                // list read leaves them out). The client's annual agreements are among them: one
                // signed for this return skips its letters (A11).
                for d in p.deliveries_all(Some(&id))? {
                    ds.push(p.delivery(&d.id)?);
                }
                let status = p.status().ok();
                let files = p.files_for_client(&id)?;
                let firm = status.map(|s| s.firm_name).unwrap_or_default();
                let staff = p.delivery_staff(&id).unwrap_or_default();
                Ok((ds, files, firm, staff))
            })())
        }));
    }

    /// The delivery this window is about: the open one for the year and return type chosen,
    /// else the newest finished one, shown read-only until Send another (owner 2026-11-26: a
    /// finished card opened as an empty form could be sent again by mistake).
    fn current(&self) -> Option<&Delivery> {
        let rt = self.rt();
        let fits = |d: &&Delivery| {
            d.kind == self.kind
                && d.tax_year == self.year
                && d.return_type.eq_ignore_ascii_case(&rt)
        };
        self.deliveries
            .iter()
            .filter(fits)
            .find(|d| d.status == "open")
            .or_else(|| {
                self.deliveries
                    .iter()
                    .filter(fits)
                    .filter(|d| !self.fresh && d.status == "complete")
                    .max_by(|a, b| a.created_at.cmp(&b.created_at))
            })
    }

    /// Send another: a new, empty card for the same kind and year.
    fn send_another(&mut self) {
        let paid = self
            .current()
            .and_then(|d| d.fee.as_ref())
            .is_some_and(|f| f.taken());
        self.fresh = true;
        self.revising = false;
        self.title = default_title(&self.kind, self.year, &self.rt());
        self.return_file = None;
        self.return_doc = None;
        self.extra_docs.clear();
        self.kept.clear();
        self.pay_first = false;
        self.offer_letters();
        self.offer_fee();
        self.fill();
        // Owner 2026-11-26: the client paid for the last card; a second bill is ticked on purpose.
        if paid {
            self.with_fee = false;
        }
        self.another_paid = paid;
    }

    /// Owner 2026-11-26: "Signed on paper…" for the letters or Form 8879 step, and its question.
    fn paper_ui(
        &mut self,
        ui: &mut egui::Ui,
        kind: &'static str,
        can: bool,
        current: Option<&Delivery>,
    ) -> Option<Act> {
        let waiting = current
            .and_then(|d| d.step(kind))
            .is_some_and(|s| s.present && s.state != "done");
        if ui
            .add_enabled(can && waiting, egui::Button::new("Signed on paper…"))
            .on_hover_text("The client signed on paper: mark the step done")
            .clicked()
        {
            self.paper = Some(kind);
            self.paper_note.clear();
        }
        if self.paper != Some(kind) || !waiting {
            return None;
        }
        let what = if kind == delivery::ESIGN {
            "Form 8879"
        } else {
            "the letters"
        };
        let mut act = None;
        ui.group(|ui| {
            ui.label(format!(
                "Mark {what} signed on paper? The step is done; the client is not asked to sign online. Put the signed copy in the client's folder as usual."
            ));
            ui.horizontal(|ui| {
                ui.label("Note");
                widgets::named(
                    ui.add(
                        egui::TextEdit::singleline(&mut self.paper_note)
                            .desired_width(260.0)
                            .char_limit(200)
                            .hint_text("Signed at the office"),
                    ),
                    "Signed on paper note",
                );
                if ui.button("Mark signed").clicked() {
                    act = Some(Act::Paper(kind));
                }
                if ui.button("Keep it").clicked() {
                    self.paper = None;
                }
            });
        });
        act
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
            // PA-114: the documents (one tax return keeps the return's own handling, T27), and
            // the pay-first lock. QA 2026-11-26: one not found in the cabinet is kept, said.
            if d.kind != delivery::KIND_RETURN || d.files.len() > 1 {
                let cards: Vec<(String, String)> = if d.files.is_empty() {
                    d.return_file
                        .iter()
                        .map(|f| (f.id.clone(), f.name.clone()))
                        .collect()
                } else {
                    d.files
                        .iter()
                        .map(|f| (f.id.clone(), f.name.clone()))
                        .collect()
                };
                let (found, kept) = match_card_files(&cards, |file_id| {
                    self.files
                        .iter()
                        .find(|f| f.file_id == file_id)
                        .map(|f| f.vf_document_id.clone())
                        .filter(|doc| self.docs.iter().any(|x| &x.id == doc))
                });
                self.return_doc = found.first().cloned();
                self.extra_docs = found.iter().skip(1).cloned().map(Some).collect();
                self.kept = kept;
            } else {
                self.extra_docs.clear();
                self.kept.clear();
            }
            self.pay_first = d.locks.return_after_fee == Some(true);
            self.link = d.esign_link.clone().unwrap_or_default();
            self.esign_mode = d.esign_mode().to_string();
            self.notify = d.notify;
        } else if self.return_doc.is_none() && self.kind == delivery::KIND_RETURN {
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
        if self.current().is_none() && !self.default_applied {
            self.default_applied = true;
            let (who, why) =
                crate::ui::portal::new_card_assignee(self.default_staff.as_ref(), &self.staff);
            if who.is_some() {
                self.assigned = who;
            }
            if let Some(w) = why {
                self.said = Some((w, true));
            }
        }
        // A11: the engagement for this return was signed in the year's agreement.
        if self.current().is_none() && self.signed_agreement().is_some() {
            self.with_letters = false;
        }
    }

    /// The client's agreement covering this return (its returns' year and type) whose letters
    /// are signed (ADDENDUM-annual-agreements.md A11).
    fn signed_agreement(&self) -> Option<&Delivery> {
        if self.kind != delivery::KIND_RETURN {
            return None;
        }
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
                        "portal_delivery_8879_paper" => "Form 8879 marked signed on paper.".into(),
                        "portal_delivery_letters_paper" => {
                            "The letters marked signed on paper.".into()
                        }
                        "portal_delivery_cancel" => {
                            "Cancelled: the card has left the portal.".into()
                        }
                        _ => "Saved.".into(),
                    },
                    false,
                ));
                self.err = None;
                self.revising = false;
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
            Done::Saved(Err(e), _) if e == REVISED_ASK || e == FILES_REVISE_ASK => {
                // Asked in the window, as for another document (T27).
                self.force_revise = true;
                self.confirm_revise = true;
            }
            // Owner 2026-11-26: an older portal takes no revision of a finished card.
            Done::Saved(Err(e), _) if self.revising && e.contains("delivery_closed") => {
                self.err = Some(NEEDS_PORTAL_058.into());
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
            "Deliver — {} ({})",
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
        // Owner 2026-11-26: a finished card is shown read-only; only kind and year can change.
        let finished = current.as_ref().is_some_and(|d| d.status == "complete") && !self.revising;
        let pick = !busy && self.offline.is_none();
        let can = pick && !finished;
        // The strip: kind, year, return type (or the document's name), title, status.
        ui.horizontal_wrapped(|ui| {
            let before = (self.kind.clone(), self.year, self.rt());
            // PA-114 (owner 2026-11-26, M1): what the card delivers.
            ui.label("Kind");
            ui.add_enabled_ui(pick, |ui| {
                let r = egui::ComboBox::from_id_salt("delivery-kind")
                    .selected_text(kind_words(&self.kind))
                    .show_ui(ui, |ui| {
                        for k in [
                            delivery::KIND_RETURN,
                            delivery::KIND_STATEMENTS,
                            delivery::KIND_DOCUMENT,
                        ] {
                            ui.selectable_value(&mut self.kind, k.to_string(), kind_words(k));
                        }
                    });
                combo_named(&r.response, "Kind", kind_words(&self.kind));
            });
            let l = ui.label(if self.kind == delivery::KIND_RETURN {
                "Tax year"
            } else {
                "Year"
            });
            ui.add_enabled(
                pick,
                egui::DragValue::new(&mut self.year).range(1990..=2100),
            )
            .labelled_by(l.id);
            match self.kind.as_str() {
                delivery::KIND_RETURN => {
                    ui.label("Return type");
                    ui.add_enabled_ui(pick, |ui| {
                        let r = egui::ComboBox::from_id_salt("delivery-type")
                            .selected_text(&self.return_type)
                            .show_ui(ui, |ui| {
                                for t in self.types.clone() {
                                    ui.selectable_value(&mut self.return_type, t.clone(), t);
                                }
                            });
                        combo_named(&r.response, "Return type", &self.return_type);
                    });
                }
                delivery::KIND_DOCUMENT => {
                    ui.label("What it is");
                    widgets::named(
                        ui.add_enabled(
                            pick && (!sent || finished),
                            egui::TextEdit::singleline(&mut self.doc_name)
                                .desired_width(180.0)
                                .char_limit(24)
                                .hint_text("Payroll reports"),
                        ),
                        "Document name",
                    );
                }
                _ => {}
            }
            if before != (self.kind.clone(), self.year, self.rt()) {
                self.fresh = false;
                self.revising = false;
                self.another_paid = false;
                if before.0 != self.kind || before.2 != self.rt() {
                    self.offer_letters();
                }
                if before.0 != self.kind && self.kind == delivery::KIND_RETURN {
                    // QA 2026-11-26: back to a tax return, its defaults come back.
                    self.with_8879 = true;
                    self.fee_mode = "packages".into();
                }
                self.offer_fee();
                if self.kind != delivery::KIND_RETURN {
                    self.with_8879 = false;
                }
                if self.current().is_none() {
                    self.title = default_title(&self.kind, self.year, &self.rt());
                    self.return_file = None;
                    self.return_doc = None;
                    self.extra_docs.clear();
                    self.pay_first = false;
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
            let l = ui.label("Card title");
            ui.add_enabled(
                can,
                egui::TextEdit::singleline(&mut self.title)
                    .desired_width(320.0)
                    .char_limit(120),
            )
            .labelled_by(l.id);
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
            let lctx = delivery::letter_context(&self.root, &self.rt());
            for (tpl, on) in &mut self.letters {
                ui.add_enabled(can, egui::Checkbox::new(on, format!("{}{}", tpl.name, if tpl.sample { " (sample text)" } else { "" })));
                if *on {
                    let names = tpl.fill_ins(&lctx, None);
                    if !names.is_empty() {
                        let v = self.fills.entry(tpl.id.clone()).or_default();
                        crate::ui::letter_templates::fill_in_boxes(ui, can, &tpl.id, &names, v);
                    }
                }
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
            if let Some(a) = self.paper_ui(ui, delivery::LETTERS, can, current.as_ref()) {
                act = Some(a);
            }
        });
        // Section: the documents (a tax return's PDF, or PA-114's statements or documents).
        ui.group(|ui| {
            let docs_done = current
                .as_ref()
                .and_then(|d| d.step(delivery::RETURN))
                .is_some_and(|s| s.state == "done");
            ui.horizontal(|ui| {
                ui.add_enabled(
                    can && !docs_done,
                    egui::Checkbox::new(
                        &mut self.with_return,
                        RichText::new(documents_words(&self.kind)).strong(),
                    ),
                );
                ui.label(RichText::new(step_state(delivery::RETURN)).color(theme::ink_muted()));
            });
            if self.with_return {
                let shown = self
                    .return_doc
                    .as_deref()
                    .and_then(|id| self.docs.iter().find(|d| d.id == id))
                    .map(|d| d.name.clone())
                    .unwrap_or_else(|| {
                        if self.kind == delivery::KIND_RETURN {
                            "Choose the return…".into()
                        } else {
                            "Choose a PDF…".into()
                        }
                    });
                ui.add_enabled_ui(can, |ui| {
                    let r = egui::ComboBox::from_id_salt("delivery-return")
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
                    combo_named(
                        &r.response,
                        "The first PDF",
                        &doc_words(&self.docs, self.return_doc.as_deref()),
                    );
                });
                // PA-114 (M7): more PDFs on one card, each opened by the client.
                let mut remove = None;
                for (i, extra) in self.extra_docs.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        let shown = extra
                            .as_deref()
                            .and_then(|id| self.docs.iter().find(|d| d.id == id))
                            .map(|d| d.name.clone())
                            .unwrap_or_else(|| "Choose a PDF…".into());
                        ui.add_enabled_ui(can && !docs_done, |ui| {
                            let r = egui::ComboBox::from_id_salt(("delivery-extra", i))
                                .selected_text(shown)
                                .width(420.0)
                                .show_ui(ui, |ui| {
                                    for d in &self.docs {
                                        ui.selectable_value(
                                            extra,
                                            Some(d.id.clone()),
                                            format!(
                                                "{}{}",
                                                d.name,
                                                d.tax_year
                                                    .map(|y| format!(" ({y})"))
                                                    .unwrap_or_default()
                                            ),
                                        );
                                    }
                                });
                            combo_named(
                                &r.response,
                                "Another PDF",
                                &doc_words(&self.docs, extra.as_deref()),
                            );
                        });
                        if ui
                            .add_enabled(can && !docs_done, egui::Button::new("Remove"))
                            .clicked()
                        {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    self.extra_docs.remove(i);
                }
                let mut drop_kept = None;
                for (i, (_, name)) in self.kept.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.colored_label(
                            theme::warn(),
                            format!("{name}: on the card, but not found in the client's folder (trashed or moved). It stays on the card unless removed."),
                        );
                        if ui.add_enabled(can && !docs_done, egui::Button::new("Remove")).clicked() {
                            drop_kept = Some(i);
                        }
                    });
                }
                if let Some(i) = drop_kept {
                    self.kept.remove(i);
                }
                if ui
                    .add_enabled(
                        can && !docs_done
                            && self.extra_docs.len() + 1 + self.kept.len() < delivery::MAX_FILES,
                        egui::Button::new("+ Add another PDF"),
                    )
                    .clicked()
                {
                    self.extra_docs.push(None);
                }
                if let Some(d) = &current {
                    let opened = d.files.iter().filter(|f| f.opened_at.is_some()).count();
                    if d.files.len() > 1 {
                        ui.label(widgets::caption(format!(
                            "{opened} of {} opened by the client.",
                            d.files.len()
                        )));
                    }
                }
                ui.label(widgets::caption(
                    "The client's PDFs in the cabinet. Sending shares them on the portal with \
                     everyone of the client (B2 first, as Share on portal does); the client opens \
                     them when the card's earlier steps allow, and the step is done once each is \
                     opened.",
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
                        RichText::new(if self.kind == delivery::KIND_RETURN {
                            "Package and fee"
                        } else {
                            "Fee"
                        })
                        .strong(),
                    ),
                );
                ui.label(RichText::new(step_state(delivery::FEE)).color(theme::ink_muted()));
            });
            if !self.with_fee {
                return;
            }
            // PA-114 (owner 2026-11-26, M2): pay first, per card.
            let docs_opened = current
                .as_ref()
                .and_then(|d| d.step(delivery::RETURN))
                .is_some_and(|s| s.state == "done");
            ui.add_enabled(
                can && self.with_return && !paid && !docs_opened,
                egui::Checkbox::new(&mut self.pay_first, PAY_FIRST_WORDS),
            )
            .on_hover_text(
                "The client sees the documents listed, locked, until they pay (a bank payment \
                 started is enough).",
            );
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
            if self.kind != delivery::KIND_RETURN {
                self.fee_mode = "amount".into();
            }
            ui.horizontal(|ui| {
                if self.kind != delivery::KIND_RETURN {
                    return;
                }
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
        // Section: Form 8879 (a tax return's only, PA-114).
        if self.kind == delivery::KIND_RETURN {
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.add_enabled(
                        can && !current
                            .as_ref()
                            .and_then(|d| d.step(delivery::ESIGN))
                            .is_some_and(|s| s.state == "done"),
                        egui::Checkbox::new(
                            &mut self.with_8879,
                            RichText::new("Form 8879").strong(),
                        ),
                    );
                    ui.label(RichText::new(step_state(delivery::ESIGN)).color(theme::ink_muted()));
                });
                if self.with_8879 {
                    // Owner 2026-11-26: how the client signs it.
                    ui.add_enabled_ui(can, |ui| {
                        ui.label("How it is signed");
                        ui.horizontal(|ui| {
                            ui.radio_value(
                                &mut self.esign_mode,
                                "link".to_string(),
                                "UltraTax eSignature link",
                            );
                            if self.esign_mode == "link" {
                                widgets::named(
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.link)
                                            .desired_width(380.0)
                                            .hint_text("https://…"),
                                    ),
                                    "eSignature link",
                                );
                            }
                        });
                        ui.radio_value(
                            &mut self.esign_mode,
                            "office".to_string(),
                            "Pick up and sign at our office",
                        );
                        ui.radio_value(
                            &mut self.esign_mode,
                            "mail".to_string(),
                            "We mailed the return: sign and mail Form 8879 back",
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
                    if let Some(a) = self.paper_ui(ui, delivery::ESIGN, can, current.as_ref()) {
                        act = Some(a);
                    }
                }
            });
        }
        ui.add_enabled(
            can,
            egui::Checkbox::new(
                &mut self.notify,
                "Tell the client by email and text when it is sent",
            ),
        );
        ui.add_enabled_ui(can, |ui| {
            ui.horizontal(|ui| {
                ui.label("Remind the client");
                let r = egui::ComboBox::from_id_salt("delivery-remind")
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
                combo_named(
                    &r.response,
                    "Remind the client",
                    &remind_choice_words(self.remind_days),
                );
                ui.label("Assigned to");
                let shown = self
                    .assigned
                    .as_deref()
                    .and_then(|id| self.staff.iter().find(|s| s.id == id))
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| "Nobody (the admins)".into());
                let r = egui::ComboBox::from_id_salt("delivery-assigned")
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.assigned, None, "Nobody (the admins)");
                        for s in &self.staff {
                            ui.selectable_value(&mut self.assigned, Some(s.id.clone()), &s.name);
                        }
                    });
                combo_named(
                    &r.response,
                    "Assigned to",
                    self.assigned
                        .as_deref()
                        .and_then(|id| self.staff.iter().find(|s| s.id == id))
                        .map_or("Nobody (the admins)", |s| s.name.as_str()),
                );
            })
        });
        let doc_name_ok = self.kind != delivery::KIND_DOCUMENT
            || delivery::valid_return_type(&self.doc_name).is_ok();
        if !doc_name_ok {
            ui.colored_label(
                theme::warn(),
                "Say what the document is (up to 24 letters, digits or spaces), e.g. Payroll reports.",
            );
        }
        let link_ok = !self.with_8879
            || self.esign_mode != "link"
            || self.link.trim().is_empty()
            || self.link.trim().to_lowercase().starts_with("https://");
        if !link_ok {
            ui.colored_label(theme::error(), "The eSignature link starts with https://.");
        }
        // Walkthrough 2026-11-26: a finished card's words and buttons on their own row, so the
        // row of right-hand buttons never covers them.
        ui.horizontal_wrapped(|ui| {
            if self.another_paid && current.is_none() {
                ui.colored_label(
                    theme::warn(),
                    format!(
                        "The last {} card's fee was paid. Tick Fee to bill again.",
                        self.year
                    ),
                );
            }
            if self.revising {
                ui.label(
                    RichText::new(
                        "Revising a finished card: choose the corrected PDF, then Save changes. The card opens again; the fee stays paid.",
                    )
                    .color(theme::ink_secondary()),
                );
                if ui.add_enabled(pick, egui::Button::new("Stop revising")).clicked() {
                    self.revising = false;
                    self.fill();
                }
            } else if finished {
                let when = current
                    .as_ref()
                    .and_then(|d| d.completed_at.as_deref())
                    .map(|t| format!("Finished {}.", widgets::local_time(t)))
                    .unwrap_or_else(|| "Finished.".into());
                ui.label(
                    RichText::new(format!("{when} Nothing here can be changed."))
                        .color(theme::ink_secondary()),
                );
                let revise = if self.kind == delivery::KIND_RETURN {
                    "Revise the return…"
                } else {
                    "Revise the documents…"
                };
                if ui
                    .add_enabled(pick, egui::Button::new(revise))
                    .on_hover_text("Send a corrected PDF on this card; the fee stays paid")
                    .clicked()
                {
                    self.revising = true;
                }
                if ui
                    .add_enabled(pick, egui::Button::new("Send another"))
                    .on_hover_text("A new, empty card for the same kind and year")
                    .clicked()
                {
                    self.send_another();
                }
            }
        });
        ui.horizontal(|ui| {
            if !finished
                && !self.revising
                && ui
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
                if !finished
                    && ui
                        .add_enabled(
                            can && link_ok
                                && doc_name_ok
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
        // Walkthrough 2026-11-26: a question below the window's fold is scrolled into view once,
        // when it appears (the Form 8879 choices made the window taller).
        let asking = self.confirm_revise || self.confirm_cancel;
        let scroll = asking && !self.ask_shown;
        self.ask_shown = asking;
        if self.confirm_revise {
            let g = ui.group(|ui| {
                let documents = self.kind != delivery::KIND_RETURN
                    || !self.extra_docs.is_empty()
                    || !self.kept.is_empty()
                    || current.as_ref().is_some_and(|d| d.files.len() > 1);
                if documents {
                    ui.label("The client has opened these documents. Send the changed ones as revised? The client is told, and each document is to be opened again; the letters and fee stand.");
                } else {
                    ui.label("Tell the client the return was updated and reopen the Form 8879 step? The letters, package and fee stand; paste a new eSignature link if UltraTax made one.");
                }
                ui.horizontal(|ui| {
                    if ui
                        .button(if documents {
                            "Send the revised documents"
                        } else {
                            "Send the revised return"
                        })
                        .clicked()
                    {
                        act = Some(Act::Send);
                    }
                    if ui.button("Keep the first").clicked() {
                        self.confirm_revise = false;
                    }
                });
            });
            if scroll {
                g.response.scroll_to_me(Some(egui::Align::Center));
            }
        }
        if self.confirm_cancel {
            let g = ui.group(|ui| {
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
            if scroll {
                g.response.scroll_to_me(Some(egui::Align::Center));
            }
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
            "Each finished step's PDF, with its log page, is filed in the client's {} folder.",
            delivery::folder_words(&self.kind, self.year)
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
                let is_return = self.kind == delivery::KIND_RETURN;
                let steps = StepsIn {
                    letters: self.with_letters,
                    return_: self.with_return,
                    // PA-114: a package and Form 8879 are a tax return's only.
                    package: is_return && self.with_fee && self.fee_mode != "amount",
                    fee: self.with_fee,
                    esign8879: is_return && self.with_8879,
                };
                // PA-114: the documents in order (the first, then the others chosen), once each.
                let mut chosen: Vec<crate::ui::portal::DocRef> = Vec::new();
                if self.with_return {
                    for id in std::iter::once(self.return_doc.as_deref())
                        .chain(self.extra_docs.iter().map(|x| x.as_deref()))
                        .flatten()
                    {
                        if !chosen.iter().any(|d| d.id == id)
                            && let Some(d) = self.docs.iter().find(|d| d.id == id)
                        {
                            chosen.push(d.clone());
                        }
                    }
                    if !is_return && chosen.is_empty() {
                        self.err = Some("Choose the PDF to deliver.".into());
                        return;
                    }
                }
                let pay_first = self.with_fee && self.with_return && self.pay_first;
                let kind = self.kind.clone();
                // The priced snapshot and each offered package's addendum (T20), while the
                // selection is not signed.
                // Walkthrough 2026-11-26: a fee paid (or a bank payment started) is left as it is: the
                // portal refuses a new one, and a revised return must not bill again.
                let fee = if self.with_fee
                    && current.as_ref().is_none_or(|d| {
                        d.fee
                            .as_ref()
                            .is_none_or(|f| f.selection_signed_at.is_none() && !f.taken())
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
                // Each addendum as sent (with the firm's Terms where the template asks), and
                // never with a fill-in left (A17; review 2026-11-25: addenda skipped both).
                let mut addenda_left: Vec<String> = Vec::new();
                let addenda: Vec<(String, vf_letters::Template, vf_letters::Laid)> = match &fee {
                    Some((mode, lines)) if mode == "packages" && current.is_none() => {
                        let templates = vf_letters::store::load_all(&self.root).unwrap_or_default();
                        let base = self.fields();
                        let ctx = delivery::letter_context(&self.root, &self.return_type);
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
                                let sent = vf_letters::store::as_sent(t, &templates);
                                let left = sent.fill_ins(&ctx, Some(&l.name));
                                if !left.is_empty() {
                                    addenda_left.push(format!("{}: {}", t.name, left.join(", ")));
                                }
                                let mut f = base.clone();
                                f.insert("Package name".into(), l.name.clone());
                                f.insert("Package fee".into(), vf_portal::balance::money(l.cents));
                                let laid = vf_letters::layout(&sent, &f, &ctx, Some(&l.name));
                                Some((l.key.clone(), t.clone(), laid))
                            })
                            .collect()
                    }
                    _ => Vec::new(),
                };
                if !addenda_left.is_empty() {
                    self.err = Some(format!(
                        "Fill in the package addendum before sending (Admin › Portal › Letter templates). {}",
                        addenda_left.join("; ")
                    ));
                    return;
                }
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
                // One tax return PDF: sent as the return (it can be revised, T27). Several, or
                // another kind's: sent as the card's documents (PA-114).
                let kept: Vec<String> = if self.with_return {
                    self.kept.iter().map(|(id, _)| id.clone()).collect()
                } else {
                    Vec::new()
                };
                let one_return = is_return
                    && chosen.len() <= 1
                    && kept.is_empty()
                    && current.as_ref().is_none_or(|d| d.files.len() <= 1);
                let (doc, many) = if one_return {
                    (chosen.into_iter().next(), Vec::new())
                } else {
                    (None, chosen)
                };
                let confirmed = seen;
                let b2 = self.b2.clone();
                let client = self.client.clone();
                let share_audit = self.audit.clone();
                let link = self.link.trim().to_string();
                let link = (self.with_8879 && self.esign_mode == "link" && !link.is_empty())
                    .then_some(link);
                let esign_mode = (is_return && self.with_8879).then(|| self.esign_mode.clone());
                let notify = self.notify;
                let locks = delivery::Locks {
                    return_after_letters: true,
                    esign_after_letters: true,
                    esign_after_fee: true,
                    // Sent only when ticked: an older portal ignores it, and is caught below.
                    return_after_fee: pay_first.then_some(true),
                };
                // A change sends the card's locks as they are, with pay first as now ticked.
                let change_locks = current.as_ref().map(|d| delivery::Locks {
                    return_after_fee: (pay_first || d.locks.return_after_fee.is_some())
                        .then_some(pay_first),
                    ..d.locks.clone()
                });
                let new = NewDelivery {
                    client_id: self.client.id.clone(),
                    tax_year: self.year,
                    return_type: self.rt(),
                    title: title.clone(),
                    steps: steps.clone(),
                    locks,
                    assigned_staff_id: self.assigned.clone(),
                    notify,
                    // QA 2026-11-25: told once the return, fee and letters are all on.
                    hold: true,
                    remind_days: self.remind_days,
                    kind: (!is_return).then(|| kind.clone()),
                    covers: Vec::new(),
                    return_year: None,
                    esign_mode: esign_mode.clone().filter(|m| m != "link"),
                };
                let what = if current.is_none() {
                    "portal_delivery_create"
                } else {
                    "portal_delivery_change"
                };
                self.job = Some(spawn(ctx, move || {
                    let p = core::client(&s2);
                    let created = std::cell::Cell::new(None::<String>);
                    let r = (|| {
                        let mut d = match &current {
                            None => {
                                let d = p
                                    .create_delivery(&new)
                                    .map_err(|e| create_error(e, is_return))?;
                                created.set(Some(d.id.clone()));
                                d
                            }
                            // Owner 2026-11-26: a finished card revised; the revision below
                            // opens it again (nothing else of it changes).
                            Some(d) if d.status == "complete" => d.clone(),
                            Some(d) => p.change_delivery(
                                &d.id,
                                &DeliveryChange {
                                    title: Some(title),
                                    steps: Some(steps),
                                    notify: Some(notify),
                                    assigned_staff_id: Some(assigned),
                                    remind_days: Some(remind_days),
                                    locks: change_locks,
                                    // Sent only when it changed (an older portal ignores it).
                                    esign_mode: esign_mode
                                        .clone()
                                        .filter(|m| m.as_str() != d.esign_mode()),
                                },
                            )?,
                        };
                        // PA-114: an older portal takes the card but not its kind or its lock.
                        if let Some(why) = old_portal(&d, &kind, pay_first) {
                            return Err(vf_portal::PortalError::Local(why.into()));
                        }
                        // Owner 2026-11-26: an older portal has no office or mail signing.
                        if esign_mode.as_deref().is_some_and(|m| m != "link")
                            && d.esign_mode.is_none()
                        {
                            return Err(vf_portal::PortalError::Local(NEEDS_PORTAL_058.into()));
                        }
                        // The return (Q15): put on the portal for nobody yet, set as the
                        // delivery's return (so it is locked), then shown to everyone of the
                        // client. A return already there with the same content is left as it
                        // is (staff may have limited who sees it).
                        // PA-114: several documents, or another kind's: each put on the portal for
                        // nobody, set as the card's documents (so they are locked), then shown.
                        if !many.is_empty() || !kept.is_empty() {
                            // QA 2026-11-26: first what each is on the portal (this client's),
                            // nothing changed yet; a document the client opened is never
                            // replaced unasked: asked, then sent as revised.
                            let mut plan = Vec::new();
                            for doc in &many {
                                plan.push(look_up(&p, &client, doc)?);
                            }
                            let seen_plan: Vec<(Option<String>, bool)> = plan
                                .iter()
                                .map(|(doc, f)| {
                                    (
                                        f.as_ref().map(|f| f.file_id.clone()),
                                        f.as_ref().is_some_and(|f| f.sha256 != doc.hash),
                                    )
                                })
                                .collect();
                            let change = files_change(&d, &seen_plan, &kept);
                            if change.revise && !confirmed {
                                return Err(vf_portal::PortalError::Local(FILES_REVISE_ASK.into()));
                            }
                            let mut ids = Vec::new();
                            let mut shown_now = Vec::new();
                            for (doc, f) in plan {
                                let (id, added) =
                                    put_document(&p, &b2, &client, &doc, f, &share_audit)?;
                                if added {
                                    shown_now.push((id.clone(), doc.id.clone()));
                                }
                                ids.push(id);
                            }
                            ids.extend(kept.iter().cloned());
                            if change.revise
                                || change.set_changes
                                || (ids != d.file_ids() && !change.any_opened)
                            {
                                d = p
                                    .set_delivery_files(&d.id, &ids, change.revise)
                                    .map_err(files_error)?;
                            }
                            for (file_id, doc_id) in &shown_now {
                                show_to_everyone(&p, &client, file_id, doc_id, &share_audit)?;
                            }
                        }
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
                            let (file_id, added, replaced) =
                                match p.file_for_client_doc(&client.id, &doc.id)? {
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
                                show_to_everyone(&p, &client, &file_id, &doc.id, &share_audit)?;
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
                        // Told now (once: a Send again after a stop tells nobody twice).
                        p.ready_delivery(&d.id)?;
                        p.delivery(&d.id)
                    })();
                    // PA-114: a card made on an older portal that could not take it all is
                    // cancelled (it would deliver the documents unlocked or as a tax return).
                    if let (Err(vf_portal::PortalError::Local(m)), Some(id)) = (&r, created.take())
                        && m == NEEDS_PORTAL
                    {
                        let _ = p.cancel_delivery(&id);
                    }
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
            Act::Paper(kind) => {
                let Some(d) = current else { return };
                let note = self.paper_note.trim().to_string();
                self.paper = None;
                self.job = Some(spawn(ctx, move || {
                    let p = core::client(&s2);
                    let r = p
                        .paper_signed(&d.id, kind, &note)
                        .map_err(|e| match (e.status(), e.code()) {
                            (Some(404), Some("not_found")) => {
                                vf_portal::PortalError::Local(NEEDS_PORTAL_058.into())
                            }
                            _ => e,
                        })
                        .and_then(|x| p.delivery(&x.id));
                    Done::Saved(
                        r.map(Box::new).map_err(|e| e.message()),
                        if kind == delivery::ESIGN {
                            "portal_delivery_8879_paper"
                        } else {
                            "portal_delivery_letters_paper"
                        },
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

/// PA-114: said when the portal is older than the card needs (and the card is cancelled).
pub const NEEDS_PORTAL: &str = "This needs Vibe Portal 0.5.6 or later: update the portal first.";
/// Signed on paper and revising a finished card (owner 2026-11-26).
pub const NEEDS_PORTAL_058: &str =
    "This needs Vibe Portal 0.5.8 or later: update the portal first.";
/// QA 2026-11-26: said by the Send job when documents the client opened would change: the
/// window asks, as for a revised return, then sends them as revised.
const FILES_REVISE_ASK: &str = "The client has opened these documents.";

/// What sending a card's documents changes (QA 2026-11-26).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilesChange {
    /// Documents added or taken off (not only their order).
    pub set_changes: bool,
    /// The client opened one that would change, or the list changes after they opened any:
    /// asked first, sent as revised.
    pub revise: bool,
    /// The client opened at least one.
    pub any_opened: bool,
}

/// `plan`: each chosen document's portal file (none: new to the portal) and whether its content
/// differs from the cabinet's; `kept`: the card's files kept as they are.
pub fn files_change(
    card: &Delivery,
    plan: &[(Option<String>, bool)],
    kept: &[String],
) -> FilesChange {
    let opened: Vec<&str> = card
        .files
        .iter()
        .filter(|f| f.opened_at.is_some())
        .map(|f| f.id.as_str())
        .collect();
    let done = card
        .step(delivery::RETURN)
        .is_some_and(|s| s.state == "done");
    let any_opened = !opened.is_empty() || done;
    let replaced_opened = plan
        .iter()
        .any(|(id, differs)| *differs && id.as_deref().is_some_and(|id| opened.contains(&id)));
    let mut now: Vec<String> = plan.iter().filter_map(|(id, _)| id.clone()).collect();
    now.extend(kept.iter().cloned());
    now.sort();
    let mut was = card.file_ids();
    was.sort();
    let set_changes = plan.iter().any(|(id, _)| id.is_none()) || now != was;
    FilesChange {
        set_changes,
        revise: replaced_opened || (set_changes && any_opened),
        any_opened,
    }
}

/// The card's files matched to the cabinet: the documents found (in order), and the files
/// whose document is not found, kept with their names (QA 2026-11-26: never dropped unsaid).
pub fn match_card_files(
    card: &[(String, String)],
    doc_of: impl Fn(&str) -> Option<String>,
) -> (Vec<String>, Vec<(String, String)>) {
    let mut found = Vec::new();
    let mut kept = Vec::new();
    for (id, name) in card {
        match doc_of(id) {
            Some(doc) if !found.contains(&doc) => found.push(doc),
            _ => kept.push((id.clone(), name.clone())),
        }
    }
    (found, kept)
}

/// Create's error: only an older portal's refusal of a kind it does not know (`400 invalid`)
/// says to update it; any other answer is the portal's own word.
pub fn create_error(e: vf_portal::PortalError, is_return: bool) -> vf_portal::PortalError {
    if !is_return && e.status() == Some(400) && e.code() == Some("invalid") {
        vf_portal::PortalError::Local(NEEDS_PORTAL.into())
    } else {
        e
    }
}

/// PUT /files' error: an older portal has no such route (`404 not_found`); a file the portal
/// will not put on the card (`file_not_found`: not this client's, taken off, seen by nobody)
/// is said in its own words.
pub fn files_error(e: vf_portal::PortalError) -> vf_portal::PortalError {
    match (e.status(), e.code()) {
        (Some(404), Some("not_found")) => vf_portal::PortalError::Local(NEEDS_PORTAL.into()),
        (_, Some("file_not_found")) => vf_portal::PortalError::Local(
            "A document could not be put on the card: on the portal it is not this client's, \
             was taken off, or nobody can see it. Press Read again and choose it again."
                .into(),
        ),
        _ => e,
    }
}

/// The client's open card to show when Deliver… opens: the newest open one when every open
/// card (agreements aside) is of one kind; else none (a new tax return).
pub fn open_card_to_show(
    read: Option<&Vec<Delivery>>,
    client_id: &str,
) -> Option<(String, i32, String)> {
    let open: Vec<&Delivery> = read?
        .iter()
        .filter(|d| d.client_id == client_id && d.status == "open" && !d.is_agreement())
        .collect();
    let first = open.iter().max_by(|a, b| a.created_at.cmp(&b.created_at))?;
    open.iter().all(|d| d.kind == first.kind).then(|| {
        (
            first.kind.clone(),
            first.tax_year,
            first.return_type.clone(),
        )
    })
}
/// PA-114 (owner 2026-11-26, M2): the fee's tick.
pub const PAY_FIRST_WORDS: &str = "Documents open only after the fee is paid";

/// Names a combo box for a screen reader, keeping its role and what it shows.
fn combo_named(r: &egui::Response, name: &str, value: &str) {
    let enabled = r.enabled();
    let (n, v) = (name.to_string(), value.to_string());
    r.widget_info(move || {
        let mut info = egui::WidgetInfo::labeled(egui::Role::ComboBox, enabled, &n);
        info.current_text_value = Some(v.clone());
        info
    });
}

/// What a PDF picker shows: the document's name, or "Choose a PDF…".
fn doc_words(docs: &[crate::ui::portal::DocRef], id: Option<&str>) -> String {
    id.and_then(|id| docs.iter().find(|d| d.id == id))
        .map(|d| d.name.clone())
        .unwrap_or_else(|| "Choose a PDF…".into())
}

/// What the reminder picker shows.
fn remind_choice_words(days: u32) -> String {
    match days {
        0 => "never".to_string(),
        n => format!("every {n} days while a step waits"),
    }
}

/// What the Kind picker says.
pub fn kind_words(kind: &str) -> &'static str {
    match kind {
        delivery::KIND_STATEMENTS => "Financial statements",
        delivery::KIND_DOCUMENT => "Other document",
        _ => "Tax return",
    }
}

/// The documents step's heading in the window.
fn documents_words(kind: &str) -> &'static str {
    match kind {
        delivery::KIND_STATEMENTS => "Financial statements PDF",
        delivery::KIND_DOCUMENT => "Documents",
        _ => "Tax return PDF",
    }
}

/// The card's return type on the portal for a kind: the return's, "Financial statements", or
/// the document's name.
pub fn rt_of(kind: &str, return_type: &str, doc_name: &str) -> String {
    match kind {
        delivery::KIND_STATEMENTS => delivery::STATEMENTS_TYPE.into(),
        delivery::KIND_DOCUMENT => doc_name.trim().to_string(),
        _ => return_type.to_string(),
    }
}

/// A new card's title: "Your 2025 tax return", "Your 2025 financial statements", "Your 2025
/// payroll reports".
pub fn default_title(kind: &str, year: i32, rt: &str) -> String {
    match kind {
        delivery::KIND_STATEMENTS => format!("Your {year} financial statements"),
        delivery::KIND_DOCUMENT if !rt.trim().is_empty() => {
            format!("Your {year} {}", rt.trim().to_lowercase())
        }
        delivery::KIND_DOCUMENT => format!("Your {year} documents"),
        _ => format!("Your {year} tax return"),
    }
}

/// The one-amount fee line's name.
fn fee_name(kind: &str, year: i32, rt: &str) -> String {
    match kind {
        delivery::KIND_STATEMENTS => format!("{year} financial statements"),
        delivery::KIND_DOCUMENT => format!("{year} {rt}"),
        _ => format!("{year} Form {rt}"),
    }
}

/// PA-114: a portal older than 0.5.6 makes the card but answers another kind, or no pay-first
/// lock: what to say (the caller cancels a card it just made).
pub fn old_portal(d: &Delivery, kind: &str, pay_first: bool) -> Option<&'static str> {
    let wrong_kind = kind != delivery::KIND_RETURN && d.kind != kind;
    let no_lock = pay_first && d.locks.return_after_fee != Some(true);
    (wrong_kind || no_lock).then_some(NEEDS_PORTAL)
}

/// One of the card's documents with its hash, and its portal file for this client (none: not on
/// the portal yet). Nothing is changed.
fn look_up(
    p: &vf_portal::api::Portal,
    client: &ClientRef,
    doc: &crate::ui::portal::DocRef,
) -> Result<(crate::ui::portal::DocRef, Option<PortalFile>), vf_portal::PortalError> {
    let mut doc = doc.clone();
    doc.hash = filer_intake::commit::sha256_file(&doc.path)
        .map_err(|e| vf_portal::PortalError::Local(format!("{}: {e}", doc.name)))?;
    let f = p.file_for_client_doc(&client.id, &doc.id)?;
    Ok((doc, f))
}

/// Puts one of the card's documents on the portal (PA-114): the file already there with the
/// same content as it is, new content replaced (the caller asked first when the client opened
/// it), or added for nobody yet. Its file id, and whether nobody sees it yet (shown once set).
fn put_document(
    p: &vf_portal::api::Portal,
    b2: &crate::ui::portal::B2First,
    client: &ClientRef,
    doc: &crate::ui::portal::DocRef,
    file: Option<PortalFile>,
    audit: &AuditOut,
) -> Result<(String, bool), vf_portal::PortalError> {
    use vf_portal::PortalError::Local;
    let unseen = |f: &PortalFile| f.invites.is_empty() && f.users.iter().all(|u| !u.visible);
    match file {
        Some(f) if f.sha256 == doc.hash => {
            let nobody = unseen(&f);
            Ok((f.file_id, nobody))
        }
        Some(f) => {
            let nobody = unseen(&f);
            let id = f.file_id.clone();
            crate::ui::portal::share_docs(
                p,
                b2,
                client,
                vec![(doc.clone(), Some(f))],
                &Default::default(),
                &Default::default(),
                true,
                audit,
            )
            .map_err(Local)?;
            Ok((id, nobody))
        }
        None => {
            b2.push(std::slice::from_ref(&doc.id)).map_err(Local)?;
            let a = p.add(&client.id, &doc.id, &doc.name, doc.tax_year, &doc.path)?;
            audit.send(AuditEntry::new(
                "portal_share",
                Some(&doc.id),
                serde_json::json!({ "client_id": client.id, "sent": true, "updated": false }),
            ));
            Ok((a.file_id().to_string(), true))
        }
    }
}

/// Shows a card's document to everyone of the client (and invitations not yet accepted), once
/// it is set on the card (so it is locked while the card says).
fn show_to_everyone(
    p: &vf_portal::api::Portal,
    client: &ClientRef,
    file_id: &str,
    doc_id: &str,
    audit: &AuditOut,
) -> Result<(), vf_portal::PortalError> {
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
        p.set_visibility_with(file_id, &all, &held)?;
        audit.send(AuditEntry::new(
            "portal_share",
            Some(doc_id),
            serde_json::json!({
                "client_id": client.id,
                "shown_to": all.keys().collect::<Vec<_>>(),
                "hidden_from": [],
                "held_for_invitations": held.keys().collect::<Vec<_>>(),
            }),
        ));
    }
    Ok(())
}

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

/// The deliveries and agreements as read, and the live monthly charges.
type Worklist = (Vec<Delivery>, Vec<Subscription>);

pub struct DeliveriesPage {
    show: Show,
    /// Also list those finished in `season` (a tax year).
    finished: bool,
    season: i32,
    /// The firm's "waiting too long" mark, in days (`_Vibe/deliveries.json`).
    overdue: u32,
    overdue_dirty: bool,
    data: Option<Worklist>,
    clients: HashMap<String, (ClientRef, Option<String>)>,
    job: Option<Receiver<Result<Worklist, String>>>,
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
    /// Remind ticked (owner 2026-11-26): the ticked cards' ids, the confirm, the run.
    ticked: std::collections::HashSet<String>,
    confirm: bool,
    remind: Option<Receiver<RemindMsg>>,
    remind_stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    remind_progress: Option<String>,
    remind_done: Option<String>,
    /// Mark ticked signed on paper (owner 2026-11-26): the question, its note, the run.
    confirm_sign: bool,
    sign_note: String,
    signing: Option<Receiver<String>>,
    /// The list's widths, sort and Filter box (owner 2026-11-26).
    grid: crate::ui::grid::Grid,
}

impl Default for DeliveriesPage {
    fn default() -> Self {
        Self {
            show: Show::default(),
            finished: false,
            season: 0,
            overdue: 0,
            overdue_dirty: false,
            data: None,
            clients: HashMap::new(),
            job: None,
            loaded: false,
            err: None,
            window: None,
            agreement: None,
            audit: None,
            reads: None,
            b2: Default::default(),
            settings: None,
            ticked: Default::default(),
            confirm: false,
            remind: None,
            remind_stop: None,
            remind_progress: None,
            remind_done: None,
            confirm_sign: false,
            sign_note: String::new(),
            signing: None,
            grid: crate::ui::grid::Grid::new("portal-deliveries"),
        }
    }
}

/// The Deliveries list's columns.
const DELIVERY_COLS: [crate::ui::grid::Col; 7] = [
    crate::ui::grid::Col::tick(),
    crate::ui::grid::Col::new("Client", 300.0),
    crate::ui::grid::Col::new("Year", 60.0),
    crate::ui::grid::Col::new("Kind", 90.0),
    crate::ui::grid::Col::new("Step waiting", 260.0),
    crate::ui::grid::Col::new("Days", 60.0),
    crate::ui::grid::Col::new("Monthly charge", 160.0),
];

/// Remind ticked sends this many, then pauses (as bulk invitations do).
const REMIND_GROUP: usize = 20;
const REMIND_PAUSE_S: u64 = 60;

/// From the Remind ticked worker.
enum RemindMsg {
    Progress(usize, usize),
    /// This many done, pausing before the next group.
    Pause(usize, usize),
    Done(String),
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

/// "Nov 20", local time, from an RFC 3339 time.
fn short_day(at: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(at)
        .map(|t| t.with_timezone(&chrono::Local).format("%b %-d").to_string())
        .unwrap_or_default()
}

/// The filters above the list (owner 2026-11-25: by step waiting, by kind, a failed month).
const SHOWS: [(Show, &str); 9] = [
    (Show::Open, "All open"),
    (Show::Step(delivery::RETURN), "Documents to open"),
    (Show::Step(delivery::ESIGN), "Form 8879"),
    (Show::Step(delivery::LETTERS), "Letters to sign"),
    (Show::Step(delivery::PACKAGE), "Package to choose"),
    (Show::Step(delivery::FEE), "Payment due"),
    (Show::Returns, "Tax returns"),
    (Show::Agreements, "Agreements"),
    (Show::Failed, "Monthly charge failed"),
];

pub fn page(ui: &mut egui::Ui, page: &mut DeliveriesPage, env: &mut crate::ui::browse::Env) {
    ui.heading("Deliveries and agreements");
    if page
        .settings
        .as_ref()
        .is_none_or(|(at, _)| at.elapsed() >= std::time::Duration::from_secs(30))
    {
        let s = vf_fs::cabinet::read(env.root)
            .ok()
            .and_then(|c| vf_portal::settings::load(env.root, &c.cabinet_id));
        page.settings = Some((std::time::Instant::now(), s));
        // QA 2026-11-25: not while staff are changing it (the value they type would be lost).
        if !page.overdue_dirty {
            page.overdue = worklist::overdue_days(env.root);
        }
    }
    let settings = page.settings.as_ref().and_then(|(_, s)| s.clone());
    let Some(s) = settings.filter(|s| s.deliveries) else {
        ui.label("Tax return delivery is off for this cabinet. Turn it on in Admin › Portal.");
        return;
    };
    ui.label(widgets::caption(
        "Every open tax return delivery and annual agreement, and the step each waits on: the \
         season's worklist. Click a client to open its window.",
    ));
    let ctx = ui.ctx().clone();
    if let Some(rx) = &page.job
        && let Ok(r) = rx.try_recv()
    {
        page.job = None;
        match r {
            Ok((d, subs)) => {
                if let Some(o) = &page.reads {
                    o.send(d.clone());
                }
                page.data = Some((d, subs));
                page.err = None;
            }
            Err(e) => page.err = Some(e),
        }
    }
    while let Some(rx) = &page.remind
        && let Ok(m) = rx.try_recv()
    {
        match m {
            RemindMsg::Progress(i, n) => {
                page.remind_progress = Some(format!("Reminding {i} of {n}…"));
            }
            RemindMsg::Pause(i, n) => {
                page.remind_progress = Some(format!(
                    "{i} of {n} reminded; a {REMIND_PAUSE_S} s pause before the next {REMIND_GROUP}…"
                ));
            }
            RemindMsg::Done(words) => {
                page.remind = None;
                page.remind_stop = None;
                page.remind_progress = None;
                page.remind_done = Some(words);
                page.ticked.clear();
                // Read again: each card's "reminded" time changed.
                page.loaded = false;
            }
        }
    }
    if let Some(rx) = &page.signing
        && let Ok(words) = rx.try_recv()
    {
        page.signing = None;
        page.remind_done = Some(words);
        page.ticked.clear();
        page.loaded = false;
    }
    // Busy: reminding or marking signed (the ticks and both buttons wait).
    let reminding = page.remind.is_some() || page.signing.is_some();
    if !page.loaded {
        page.loaded = true;
        page.clients = client_rows(env);
        let s2 = s.clone();
        page.job = Some(spawn(&ctx, move || {
            let p = core::client(&s2);
            let d = p.deliveries_all(None).map_err(|e| e.message())?;
            // A portal without agreements has no monthly charges.
            let subs = p.live_subscriptions().unwrap_or_default();
            Ok((d, subs))
        }));
    }
    if page.season == 0 {
        page.season = chrono::Datelike::year(&chrono::Local::now()) - 1;
    }
    ui.horizontal_wrapped(|ui| {
        ui.label("Show:");
        for (k, name) in SHOWS {
            ui.selectable_value(&mut page.show, k, name);
        }
    });
    ui.horizontal(|ui| {
        ui.checkbox(&mut page.finished, "Show finished")
            .on_hover_text("Also list those finished this season: tax returns, financial statements and other documents of that year, and the agreements covering them.");
        if page.finished {
            ui.label("Season (tax year)");
            ui.add(egui::DragValue::new(&mut page.season).range(2000..=2100));
        }
        ui.separator();
        ui.label("Waiting too long after");
        let r = ui.add(egui::DragValue::new(&mut page.overdue).range(1..=365));
        ui.label("days");
        // Saved for every PC once the number is settled (not on each step of a drag).
        if r.changed() {
            page.overdue_dirty = true;
        }
        if page.overdue_dirty && !r.dragged() && !r.has_focus() {
            page.overdue_dirty = false;
            match worklist::set_overdue_days(env.root, page.overdue) {
                Ok(n) => {
                    page.overdue = n;
                    if let Some(a) = &page.audit {
                        a.send(AuditEntry::new(
                            "deliveries_overdue_days",
                            None,
                            serde_json::json!({ "days": n }),
                        ));
                    }
                }
                Err(e) => page.err = Some(format!("Not saved: {e}")),
            }
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
    // The client, whether it is an agreement, and (PA-114) the card's kind, year and type.
    let mut open: Option<(String, bool, (String, i32, String))> = None;
    if let Some((list, subs)) = &page.data {
        let season = page.finished.then_some(page.season);
        let rows = worklist::rows(list, subs, page.show, season);
        let days: Vec<i64> = rows.iter().map(|r| days_since(&r.d.created_at)).collect();
        let name_of = |id: &str| {
            page.clients
                .get(id)
                .map(|(c, _)| format!("{} ({})", c.name, c.code))
                .unwrap_or_else(|| "(not in this cabinet)".into())
        };
        let step_of = |r: &worklist::Row| {
            let d = r.d;
            if r.open() {
                d.waiting().map(|s| d.step_words(s)).unwrap_or_default()
            } else {
                d.completed_at
                    .as_deref()
                    .map(|at| format!("Done {}", short_day(at)))
                    .unwrap_or_else(|| "Done".into())
            }
        };
        let monthly_of = |r: &worklist::Row| match r.monthly {
            Some(m) if m.failed_at.is_some() => format!(
                "Failed {}",
                m.failed_at.as_deref().map(short_day).unwrap_or_default()
            ),
            Some(m) => m.words(),
            None => String::new(),
        };
        // Owner 2026-11-26: filtered and sorted in the grid; the ticks, Tick all shown, Remind
        // and Mark signed act on the rows shown.
        let order = {
            use crate::ui::grid::Key;
            page.grid.order(
                rows.len(),
                |i| {
                    let r = &rows[i];
                    format!(
                        "{} {} {} {} {}",
                        name_of(&r.d.client_id),
                        r.d.tax_year,
                        r.d.what(),
                        step_of(r),
                        monthly_of(r)
                    )
                },
                |i, c| {
                    let r = &rows[i];
                    match c {
                        1 => Key::Text(name_of(&r.d.client_id)),
                        2 => Key::Num(r.d.tax_year as f64),
                        3 => Key::Text(r.d.what().to_string()),
                        4 => Key::Text(step_of(r)),
                        5 if r.open() => Key::Num(days[i] as f64),
                        6 => match monthly_of(r) {
                            m if m.is_empty() => Key::None,
                            m => Key::Text(m),
                        },
                        _ => Key::None,
                    }
                },
            )
        };
        let total = rows.len();
        let days: Vec<i64> = order.iter().map(|i| days[*i]).collect();
        let rows: Vec<worklist::Row> = order.iter().map(|i| rows[*i]).collect();
        if total == 0 {
            ui.label(widgets::caption("None."));
        } else {
            let open_n = rows.iter().filter(|r| r.open()).count();
            let late = rows
                .iter()
                .zip(&days)
                .filter(|(r, n)| r.overdue(**n, page.overdue))
                .count();
            ui.label(widgets::caption(format!(
                "{} listed · {open_n} open · {late} waiting over {} days",
                rows.len(),
                page.overdue
            )));
        }
        page.grid.filter_bar(ui, rows.len(), total);
        // Only the rows on screen are drawn (500 open deliveries in a season: load test
        // 2026-11-24).
        // The ticks kept are those still listed and still waiting on the client.
        let listed: std::collections::HashSet<&str> = rows
            .iter()
            .filter(|r| worklist::tickable(r.d))
            .map(|r| r.d.id.as_str())
            .collect();
        page.ticked.retain(|id| listed.contains(id.as_str()));
        page.grid
            .table(ui, &DELIVERY_COLS)
            .header(22.0, |mut h| page.grid.header(&mut h, &DELIVERY_COLS))
            .body(|body| {
                body.rows(24.0, rows.len(), |mut row| {
                    let r = rows[row.index()];
                    let n = days[row.index()];
                    let d = r.d;
                    row.col(|ui| {
                        // Owner 2026-11-26: ticked to remind, or to mark signed on paper.
                        let why = worklist::not_remindable(d);
                        let can_tick = worklist::tickable(d);
                        let mut on = page.ticked.contains(&d.id);
                        let resp = ui.add_enabled(
                            can_tick && !reminding,
                            egui::Checkbox::without_text(&mut on),
                        );
                        let resp = widgets::named(resp, "Tick row");
                        if !can_tick {
                            resp.on_disabled_hover_text(format!(
                                "Can't remind or mark signed: {}",
                                why.unwrap_or("nothing waits")
                            ));
                        } else if resp.changed() {
                            if on {
                                page.ticked.insert(d.id.clone());
                            } else {
                                page.ticked.remove(&d.id);
                            }
                            page.confirm = false;
                            page.confirm_sign = false;
                        }
                    });
                    row.col(|ui| {
                        if ui.link(name_of(&d.client_id)).clicked() {
                            open = Some((
                                d.client_id.clone(),
                                d.is_agreement(),
                                (d.kind.clone(), d.tax_year, d.return_type.clone()),
                            ));
                        }
                    });
                    row.col(|ui| {
                        ui.label(d.tax_year.to_string());
                    });
                    row.col(|ui| {
                        ui.label(d.what());
                    });
                    row.col(|ui| {
                        ui.label(step_of(&r));
                    });
                    row.col(|ui| {
                        if !r.open() {
                            ui.label("—");
                        } else if r.overdue(n, page.overdue) {
                            ui.colored_label(theme::warn(), n.to_string())
                                .on_hover_text(format!("Waiting over {} days", page.overdue));
                        } else {
                            ui.label(n.to_string());
                        }
                    });
                    row.col(|ui| match r.monthly {
                        Some(m) if m.failed_at.is_some() => {
                            ui.colored_label(
                                theme::error(),
                                format!(
                                    "Failed {}",
                                    m.failed_at.as_deref().map(short_day).unwrap_or_default()
                                ),
                            )
                            .on_hover_text(m.words());
                        }
                        Some(m) => {
                            ui.label(m.words());
                        }
                        None => {
                            ui.label("—");
                        }
                    });
                });
            });
        // Remind ticked (owner 2026-11-26): the portal's own reminder for each, one at a time.
        let names: HashMap<&str, String> = rows
            .iter()
            .map(|r| {
                let who = page
                    .clients
                    .get(&r.d.client_id)
                    .map(|(c, _)| format!("{} ({})", c.name, c.code))
                    .unwrap_or_else(|| "(not in this cabinet)".into());
                (r.d.id.as_str(), who)
            })
            .collect();
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!reminding, egui::Button::new("Tick all shown"))
                .on_hover_text("Every row listed that can be reminded or marked signed")
                .clicked()
            {
                page.ticked = listed.iter().map(|s| s.to_string()).collect();
                page.confirm = false;
            }
            if ui
                .add_enabled(!reminding, egui::Button::new("Tick waiting too long"))
                .on_hover_text(format!(
                    "Rows waiting over {} days on the client",
                    page.overdue
                ))
                .clicked()
            {
                page.ticked = rows
                    .iter()
                    .zip(&days)
                    .filter(|(r, n)| {
                        r.overdue(**n, page.overdue) && worklist::not_remindable(r.d).is_none()
                    })
                    .map(|(r, _)| r.d.id.clone())
                    .collect();
                page.confirm = false;
            }
            if !page.ticked.is_empty()
                && ui
                    .add_enabled(!reminding, egui::Button::new("Clear"))
                    .clicked()
            {
                page.ticked.clear();
                page.confirm = false;
            }
            let n = page.ticked.len();
            ui.label(format!("{n} ticked"));
            if reminding {
                if let Some(p) = &page.remind_progress {
                    ui.spinner();
                    ui.label(p);
                }
                if page.signing.is_some() {
                    ui.spinner();
                    ui.label("Marking signed on paper…");
                } else if ui.button("Stop").clicked()
                    && let Some(f) = &page.remind_stop
                {
                    f.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            } else if page.confirm_sign {
                let signable: Vec<&Delivery> = list
                    .iter()
                    .filter(|d| page.ticked.contains(&d.id))
                    .filter(|d| worklist::signable(d).is_some())
                    .collect();
                ui.label(format!(
                    "Mark the step each ticked card waits on (letters or Form 8879) signed on paper? {} card{}.",
                    signable.len(),
                    if signable.len() == 1 { "" } else { "s" }
                ));
                ui.label("Note");
                widgets::named(
                    ui.add(
                        egui::TextEdit::singleline(&mut page.sign_note)
                            .desired_width(220.0)
                            .char_limit(200)
                            .hint_text("Signed at the office"),
                    ),
                    "Signed on paper note",
                );
                if ui
                    .add(widgets::primary_button(format!("Mark {} signed", signable.len())))
                    .clicked()
                {
                    page.confirm_sign = false;
                    page.remind_done = None;
                    let jobs: Vec<(Delivery, String)> = list
                        .iter()
                        .filter(|d| page.ticked.contains(&d.id))
                        .map(|d| {
                            let who = names.get(d.id.as_str()).cloned().unwrap_or_default();
                            (d.clone(), who)
                        })
                        .collect();
                    page.signing = Some(sign_ticked(
                        &ctx,
                        &s,
                        page.audit.clone(),
                        jobs,
                        page.sign_note.trim().to_string(),
                    ));
                }
                if ui.button("Keep them").clicked() {
                    page.confirm_sign = false;
                }
            } else if page.confirm {
                ui.label(format!(
                    "Remind {n} client{} by email or text now?",
                    if n == 1 { "" } else { "s" }
                ));
                if ui.add(widgets::primary_button("Remind")).clicked() {
                    page.confirm = false;
                    page.remind_done = None;
                    // Only the rows waiting on the client (a row ticked to mark signed is not).
                    let jobs: Vec<(String, String, String)> = list
                        .iter()
                        .filter(|d| page.ticked.contains(&d.id))
                        .filter(|d| worklist::not_remindable(d).is_none())
                        .map(|d| {
                            let who = names.get(d.id.as_str()).cloned().unwrap_or_default();
                            (d.id.clone(), d.client_id.clone(), who)
                        })
                        .collect();
                    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                    page.remind_stop = Some(stop.clone());
                    page.remind_progress = Some(format!("Reminding 1 of {}…", jobs.len()));
                    page.remind = Some(remind_ticked(&ctx, &s, page.audit.clone(), jobs, stop));
                }
                if ui.button("Cancel").clicked() {
                    page.confirm = false;
                }
            } else {
                let remindable = list
                    .iter()
                    .filter(|d| page.ticked.contains(&d.id))
                    .filter(|d| worklist::not_remindable(d).is_none())
                    .count();
                let signable = list
                    .iter()
                    .filter(|d| page.ticked.contains(&d.id))
                    .filter(|d| worklist::signable(d).is_some())
                    .count();
                if ui
                    .add_enabled(remindable > 0, widgets::primary_button("Remind ticked"))
                    .on_hover_text(
                        "Each ticked client is told again by email or text, as Remind now does \
                         (3 times a day at most for one card).",
                    )
                    .clicked()
                {
                    page.confirm = true;
                }
                if ui
                    .add_enabled(
                        signable > 0,
                        egui::Button::new(format!("Mark ticked signed on paper… ({signable})")),
                    )
                    .on_hover_text(
                        "The ticked cards waiting on their letters or Form 8879: the client \
                         signed on paper",
                    )
                    .clicked()
                {
                    page.confirm_sign = true;
                    page.sign_note.clear();
                }
            }
        });
        if let Some(d) = &page.remind_done {
            ui.label(widgets::caption(d));
        }
    }
    if let Some((id, true, _)) = &open
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
    } else if let Some((id, false, (kind, year, return_type))) = open
        && let Some((c, folder)) = page.clients.get(&id).cloned()
        && let (Some(audit), Some(reads)) = (page.audit.clone(), page.reads.clone())
    {
        let docs = client_pdfs(env.conn, env.root, &c.id);
        let mut w = DeliverWindow::new(
            &ctx,
            &s,
            env.root.to_path_buf(),
            c,
            folder,
            audit,
            reads,
            docs,
            page.b2.clone(),
        );
        // The card clicked, whatever its kind (PA-114).
        w.focus(&kind, year, &return_type);
        page.window = Some(w);
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

/// Mark ticked signed on paper's worker (owner 2026-11-26): each ticked card's waiting step
/// (letters or Form 8879) marked signed on paper, one audit row each; the rest said as skipped.
fn sign_ticked(
    ctx: &egui::Context,
    s: &vf_portal::PortalSettings,
    audit: Option<AuditOut>,
    jobs: Vec<(Delivery, String)>,
    note: String,
) -> Receiver<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let ctx = ctx.clone();
    let s = s.clone();
    std::thread::spawn(move || {
        let p = core::client(&s);
        let mut marked = 0;
        let mut skipped: Vec<(String, String)> = Vec::new();
        for (d, who) in jobs {
            let Some(kind) = worklist::signable(&d) else {
                let why = d
                    .waiting()
                    .map(|w| format!("waits on {}", w.name.to_lowercase()))
                    .unwrap_or_else(|| "nothing waits".into());
                skipped.push((who, why));
                continue;
            };
            match p.paper_signed(&d.id, kind, &note) {
                Ok(_) => {
                    marked += 1;
                    if let Some(a) = &audit {
                        a.send(AuditEntry::new(
                            if kind == delivery::ESIGN {
                                "portal_delivery_8879_paper"
                            } else {
                                "portal_delivery_letters_paper"
                            },
                            None,
                            serde_json::json!({ "client_id": d.client_id, "delivery_id": d.id }),
                        ));
                    }
                }
                Err(e) => {
                    let why = match (e.status(), e.code()) {
                        (Some(404), Some("not_found")) => NEEDS_PORTAL_058.to_string(),
                        _ => e.message(),
                    };
                    skipped.push((who, why));
                }
            }
        }
        let _ = tx.send(worklist::signed_words(marked, &skipped));
        ctx.request_repaint();
    });
    rx
}

/// Remind ticked's worker: each card's reminder in turn (Stop ends the run), one audit row each.
fn remind_ticked(
    ctx: &egui::Context,
    s: &vf_portal::PortalSettings,
    audit: Option<AuditOut>,
    jobs: Vec<(String, String, String)>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Receiver<RemindMsg> {
    let (tx, rx) = std::sync::mpsc::channel();
    let ctx = ctx.clone();
    let s = s.clone();
    std::thread::spawn(move || {
        let p = core::client(&s);
        let n = jobs.len();
        let mut results = Vec::new();
        for (i, (id, client_id, who)) in jobs.into_iter().enumerate() {
            // In groups with a pause, as bulk invitations go, so the email provider does not take
            // a burst of reminders for spam.
            if i > 0 && i % REMIND_GROUP == 0 {
                let _ = tx.send(RemindMsg::Pause(i, n));
                ctx.request_repaint();
                for _ in 0..REMIND_PAUSE_S {
                    if stop.load(std::sync::atomic::Ordering::Relaxed) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
            }
            if stop.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            let _ = tx.send(RemindMsg::Progress(i + 1, n));
            ctx.request_repaint();
            let r = match p.remind_delivery(&id) {
                Ok(0) => worklist::Reminded::Nobody,
                Ok(told) => worklist::Reminded::Told(told),
                Err(e) if e.code() == Some("too_many_requests") => worklist::Reminded::TooOften,
                Err(e) => worklist::Reminded::Failed(e.message()),
            };
            if let (Some(a), worklist::Reminded::Told(told)) = (&audit, &r) {
                a.send(AuditEntry::new(
                    "portal_delivery_remind",
                    None,
                    serde_json::json!({ "client_id": client_id, "delivery_id": id, "told": told }),
                ));
            }
            results.push((who, r));
        }
        let mut words = worklist::remind_words(&results);
        if results.len() < n {
            words.push_str(&format!(" Stopped: {} not tried.", n - results.len()));
        }
        let _ = tx.send(RemindMsg::Done(words));
        ctx.request_repaint();
    });
    rx
}

/// The client panel's line for one client, from the deliveries last read on this PC: the newest
/// open card, else the newest finished one (owner 2026-11-26).
pub fn panel_line(read: Option<&Vec<Delivery>>, client_id: &str) -> Option<String> {
    let newest = |status: &str| {
        read?
            .iter()
            .filter(|d| d.client_id == client_id && d.status == status && !d.is_agreement())
            .max_by(|a, b| a.created_at.cmp(&b.created_at))
    };
    let d = newest("open").or_else(|| newest("complete"))?;
    Some(format!("{}: {}", d.heading(), d.line()))
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
    use super::*;

    /// PA-114 (owner 2026-11-26, M1/M2): a financial statements card has no Form 8879 and no
    /// package; its fee is one amount with the pay-first tick; a tax return keeps both.
    #[test]
    fn the_kind_decides_the_window_steps() {
        use egui_kittest::HarnessBuilder;
        use egui_kittest::kittest::Queryable;

        let t = tempfile::tempdir().unwrap();
        let root = t.path().to_path_buf();
        std::fs::create_dir_all(root.join("_Vibe")).unwrap();
        // Nothing listens here: the window's read fails, which this test does not need.
        let s = vf_portal::PortalSettings::new(
            "cab",
            "http://127.0.0.1:9",
            "vpk_abcdefghijklmnopqrstuvwxyz0123456789",
            0,
        )
        .unwrap()
        .with_deliveries(true);
        let ctx = egui::Context::default();
        let mut w = DeliverWindow::new(
            &ctx,
            &s,
            root,
            ClientRef {
                id: "c1".into(),
                code: "1001".into(),
                name: "Sample Trades LLC".into(),
            },
            None,
            crate::ui::portal::AuditOut::new(std::sync::mpsc::channel().0),
            ReadOut(std::sync::mpsc::channel().0),
            Vec::new(),
            Default::default(),
        );
        w.focus(delivery::KIND_STATEMENTS, 2025, "");
        w.with_fee = true;
        w.with_return = true;
        assert_eq!(w.title, "Your 2025 financial statements");
        assert_eq!(w.rt(), "Financial statements");
        assert_eq!(w.fee_mode, "amount", "no pricing plan for statements");
        let mut h = HarnessBuilder::default()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_size([1000.0, 1400.0])
            .build_ui_state(
                |ui, w: &mut DeliverWindow| {
                    w.job = None;
                    let _ = w.body(ui, false);
                },
                w,
            );
        h.run();
        assert!(
            h.query_by_label("Form 8879").is_none(),
            "no 8879 for statements"
        );
        assert!(h.query_by_label("Package and fee").is_none());
        assert!(h.query_by_label("Packages and add-ons").is_none());
        h.get_by_label("Fee");
        h.get_by_label("Financial statements PDF");
        h.get_by_label(PAY_FIRST_WORDS);
        h.get_by_label("+ Add another PDF");
        // Named for a screen reader, and still saying what it shows.
        {
            use egui_kittest::kittest::NodeT;
            let kind = h
                .query_all_by_role(egui::accesskit::Role::ComboBox)
                .find(|n| n.accesskit_node().label().as_deref() == Some("Kind"))
                .expect("the Kind picker");
            assert_eq!(
                kind.accesskit_node().value().as_deref(),
                Some("Financial statements")
            );
        }
        h.get_by_label_contains("Financials\\2025\\Delivery");

        h.state_mut().focus(delivery::KIND_RETURN, 2025, "1040");
        h.run();
        assert!(h.state().with_8879, "a tax return's 8879 comes back");
        assert_eq!(h.state().fee_mode, "packages");
        h.get_by_label("Form 8879");
        h.get_by_label("Package and fee");
        h.get_by_label("Tax return PDF");
        h.get_by_label_contains("Income Tax\\2025\\Delivery");
    }

    #[test]
    fn titles_types_and_an_older_portal() {
        assert_eq!(
            default_title("statements", 2025, "Financial statements"),
            "Your 2025 financial statements"
        );
        assert_eq!(
            default_title("document", 2025, "Payroll reports"),
            "Your 2025 payroll reports"
        );
        assert_eq!(default_title("document", 2025, " "), "Your 2025 documents");
        assert_eq!(
            default_title("return", 2025, "1040"),
            "Your 2025 tax return"
        );
        assert_eq!(rt_of("statements", "1040", ""), "Financial statements");
        assert_eq!(
            rt_of("document", "1040", " Payroll reports "),
            "Payroll reports"
        );
        assert_eq!(rt_of("return", "1065", "x"), "1065");
        assert_eq!(
            fee_name("statements", 2025, "Financial statements"),
            "2025 financial statements"
        );
        assert_eq!(fee_name("return", 2025, "1040"), "2025 Form 1040");
        // An older portal: a statements card comes back as a tax return, or the lock unanswered.
        let mut d: Delivery = serde_json::from_value(serde_json::json!({
            "id": "d1", "clientId": "c1", "taxYear": 2025, "returnType": "1040",
            "title": "Your 2025 tax return", "status": "open",
            "locks": {"returnAfterLetters": true, "esignAfterLetters": true, "esignAfterFee": true},
            "assignedStaffId": null, "remindedAt": null, "returnFile": null, "esignLink": null,
            "createdAt": "2026-11-26T12:00:00.000Z", "completedAt": null, "cancelledAt": null,
            "steps": []
        }))
        .unwrap();
        assert_eq!(old_portal(&d, "statements", false), Some(NEEDS_PORTAL));
        assert_eq!(old_portal(&d, "return", true), Some(NEEDS_PORTAL));
        assert_eq!(old_portal(&d, "return", false), None);
        d.kind = "statements".into();
        d.locks.return_after_fee = Some(true);
        assert_eq!(old_portal(&d, "statements", true), None);
    }

    /// A card as the portal answers it: `files` (id, opened), its documents step's state.
    fn card(kind: &str, files: &[(&str, bool)], step: &str) -> Delivery {
        serde_json::from_value(serde_json::json!({
            "id": "d1", "clientId": "c1", "taxYear": 2025, "returnType": "Financial statements",
            "kind": kind, "title": "t", "status": "open",
            "locks": {"returnAfterLetters": true, "esignAfterLetters": true, "esignAfterFee": true},
            "assignedStaffId": null, "remindedAt": null, "returnFile": null, "esignLink": null,
            "createdAt": "2026-11-26T12:00:00.000Z", "completedAt": null, "cancelledAt": null,
            "files": files.iter().map(|(id, opened)| serde_json::json!({
                "id": id, "name": format!("{id}.pdf"), "sha256": "x",
                "openedAt": opened.then_some("2026-11-26T13:00:00.000Z")
            })).collect::<Vec<_>>(),
            "steps": [{"kind": "return", "n": 2, "name": "Documents", "present": true,
                "state": step, "after": [], "saidAt": null, "completedAt": null,
                "completedBy": null, "rounds": 0}]
        }))
        .unwrap()
    }

    /// QA 2026-11-26 (1): a PDF the client opened is never replaced unasked, though the step is
    /// not done (another PDF not opened yet); one not opened is replaced freely.
    #[test]
    fn an_opened_pdf_is_changed_only_as_revised() {
        let d = card("statements", &[("f1", true), ("f2", false)], "open");
        let same = [
            (Some("f1".to_string()), false),
            (Some("f2".to_string()), false),
        ];
        let c = files_change(&d, &same, &[]);
        assert!(!c.revise && !c.set_changes && c.any_opened);
        // New content for f1, which the client opened: asked, then sent as revised.
        let f1_new = [
            (Some("f1".to_string()), true),
            (Some("f2".to_string()), false),
        ];
        assert!(files_change(&d, &f1_new, &[]).revise);
        // New content for f2, not opened: replaced without asking.
        let f2_new = [
            (Some("f1".to_string()), false),
            (Some("f2".to_string()), true),
        ];
        let c = files_change(&d, &f2_new, &[]);
        assert!(!c.revise && !c.set_changes);
        // A PDF added after one was opened: the list changes, asked first.
        let added = [
            (Some("f1".to_string()), false),
            (Some("f2".to_string()), false),
            (None, false),
        ];
        let c = files_change(&d, &added, &[]);
        assert!(c.set_changes && c.revise);
        // Nothing opened: added freely.
        let fresh = card("statements", &[("f1", false)], "open");
        let c = files_change(
            &fresh,
            &[(Some("f1".to_string()), false), (None, false)],
            &[],
        );
        assert!(c.set_changes && !c.revise);
        // Order only: no change of the set.
        let swapped = [
            (Some("f2".to_string()), false),
            (Some("f1".to_string()), false),
        ];
        assert!(!files_change(&fresh_two(), &swapped, &[]).set_changes);
    }

    fn fresh_two() -> Delivery {
        card("document", &[("f1", false), ("f2", false)], "open")
    }

    /// QA 2026-11-26 (2): a card document whose cabinet document is gone is kept, not dropped.
    #[test]
    fn a_card_file_not_in_the_cabinet_is_kept() {
        let cards = vec![
            ("f1".to_string(), "Q4 statements.pdf".to_string()),
            ("f2".to_string(), "Cover letter.pdf".to_string()),
            ("f3".to_string(), "Notes.pdf".to_string()),
        ];
        let (found, kept) = match_card_files(&cards, |id| match id {
            "f1" => Some("doc1".into()),
            "f3" => Some("doc3".into()),
            _ => None,
        });
        assert_eq!(found, ["doc1", "doc3"]);
        assert_eq!(kept, [("f2".to_string(), "Cover letter.pdf".to_string())]);
        // Sent with the kept one: the set is unchanged, nothing to ask.
        let d = card(
            "statements",
            &[("f1", true), ("f2", false), ("f3", false)],
            "open",
        );
        let plan = [
            (Some("f1".to_string()), false),
            (Some("f3".to_string()), false),
        ];
        let c = files_change(&d, &plan, &["f2".to_string()]);
        assert!(!c.set_changes && !c.revise);
        // Without it (dropped), the set changes and, one being opened, it would be asked.
        assert!(files_change(&d, &plan, &[]).revise);
    }

    /// QA 2026-11-26 (3): only an older portal's answers say to update it.
    #[test]
    fn only_an_older_portals_answers_say_update_it() {
        use vf_portal::PortalError;
        let http = |status: u16, code: &str| PortalError::Http {
            status,
            code: code.into(),
        };
        let words = |e: PortalError| e.message();
        assert_eq!(
            words(create_error(http(400, "invalid"), false)),
            NEEDS_PORTAL
        );
        // A tax return refused as invalid is the portal's own word, as is kind_steps.
        assert_ne!(
            words(create_error(http(400, "invalid"), true)),
            NEEDS_PORTAL
        );
        assert_ne!(
            words(create_error(http(400, "kind_steps"), false)),
            NEEDS_PORTAL
        );
        assert_eq!(words(files_error(http(404, "not_found"))), NEEDS_PORTAL);
        let nf = words(files_error(http(404, "file_not_found")));
        assert!(nf.contains("could not be put on the card"), "{nf}");
        assert_ne!(words(files_error(http(409, "files_seen"))), NEEDS_PORTAL);
    }

    /// QA 2026-11-26: Deliver… opens on the client's open card when all are of one kind.
    #[test]
    fn deliver_opens_on_the_open_cards_kind() {
        let mut s = card("statements", &[("f1", false)], "open");
        s.created_at = "2026-11-26T12:00:00.000Z".into();
        let mut r = card("return", &[], "open");
        r.return_type = "1040".into();
        r.created_at = "2026-11-20T12:00:00.000Z".into();
        let mut a = card("agreement", &[], "open");
        a.return_type = "Agreement".into();
        assert_eq!(
            open_card_to_show(Some(&vec![s.clone(), a.clone()]), "c1"),
            Some(("statements".into(), 2025, "Financial statements".into()))
        );
        assert_eq!(
            open_card_to_show(Some(&vec![s.clone(), r.clone()]), "c1"),
            None
        );
        assert_eq!(
            open_card_to_show(Some(&vec![r.clone()]), "c1"),
            Some(("return".into(), 2025, "1040".into()))
        );
        assert_eq!(open_card_to_show(Some(&vec![s]), "c2"), None);
        assert_eq!(open_card_to_show(None, "c1"), None);
    }

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
