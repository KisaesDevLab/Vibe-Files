//! Tax return delivery (ADDENDUM-tax-return-delivery.md §12, owner 2026-11-24; Vibe-Portal
//! D1/D2): Vibe Files builds a delivery (a card of steps on the client's portal home page) and
//! the portal runs it, logs it, and files a PDF of each finished step back as an upload tagged
//! with its delivery. Each delivery read is copied into the client's folder
//! (`<client>/_Vibe/delivery/<id>.jsonl`, one line per changed version, as to-do lists), shown
//! when the portal is out of reach.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::api::{Portal, PortalError, Result, Status};

pub const RETURN_TYPES: [&str; 10] = [
    "1040", "1120", "1120-S", "1065", "1041", "990", "990-PF", "709", "706", "5500",
];
/// Built in: signed by the taxpayer (and spouse).
const INDIVIDUAL_TYPES: [&str; 2] = ["1040", "709"];

/// Services an annual agreement can cover besides returns (ADDENDUM-annual-agreements.md §3.2,
/// owner 2026-11-25): not offered by Deliver tax return…, signed by whoever the client is.
pub const SERVICES: [&str; 8] = [
    "Bookkeeping",
    "Payroll",
    "Advisory",
    "Tax projections",
    "Tax consulting",
    "Audit representation",
    "FBAR",
    "Amended return",
];

/// The firm's own return types (owner 2026-11-24, Q41), beside the four built in:
/// `_Vibe/return_types.json`.
pub const RETURN_TYPES_FILE: &str = "return_types.json";

/// A return type and who signs for it: the taxpayer and spouse (as a 1040), or an officer or
/// partner with a title (as an entity).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReturnType {
    pub name: String,
    #[serde(default)]
    pub individual: bool,
    /// A service (bookkeeping, payroll…) rather than a return: agreements only.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub service: bool,
}

fn return_types_path(root: &Path) -> PathBuf {
    root.join(vf_fs::cabinet::VIBE_DIR).join(RETURN_TYPES_FILE)
}

fn own_return_types(root: &Path) -> Vec<ReturnType> {
    std::fs::read(return_types_path(root))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// The returns built in, the services built in, then the firm's own in the order added.
pub fn return_types(root: &Path) -> Vec<ReturnType> {
    let mut out: Vec<ReturnType> = RETURN_TYPES
        .iter()
        .map(|n| ReturnType {
            name: n.to_string(),
            individual: INDIVIDUAL_TYPES.contains(n),
            service: false,
        })
        .chain(SERVICES.iter().map(|n| ReturnType {
            name: n.to_string(),
            individual: false,
            service: true,
        }))
        .collect();
    for t in own_return_types(root) {
        if !out.iter().any(|x| x.name.eq_ignore_ascii_case(&t.name)) {
            out.push(t);
        }
    }
    out
}

/// The returns (not the services): what Deliver tax return… and pricing plans offer.
pub fn return_type_names(root: &Path) -> Vec<String> {
    return_types(root)
        .into_iter()
        .filter(|t| !t.service)
        .map(|t| t.name)
        .collect()
}

/// Returns and services: what an agreement and the letter templates offer.
pub fn service_names(root: &Path) -> Vec<String> {
    return_types(root).into_iter().map(|t| t.name).collect()
}

/// Which signature blocks a letter keeps: `"Individual"` (taxpayer and spouse) or `"Entity"`
/// (an officer or partner). Joined to the return type as `"1040|Individual"` for
/// `vf_letters::layout` (a block "only for" either matches).
pub fn signer_class(individual: bool) -> &'static str {
    if individual { "Individual" } else { "Entity" }
}

/// `"<return type>|<class>"` for laying a letter out for a return of this type.
pub fn letter_context(root: &Path, return_type: &str) -> String {
    format!(
        "{return_type}|{}",
        signer_class(is_individual(root, return_type))
    )
}

/// Signed by the taxpayer and spouse (1040, and the firm's own marked so).
pub fn is_individual(root: &Path, name: &str) -> bool {
    return_types(root)
        .iter()
        .find(|t| t.name.eq_ignore_ascii_case(name))
        .is_some_and(|t| t.individual)
}

/// A return type's name as the portal takes it: letters, digits, space, dot, dash and slash,
/// up to 24, starting with a letter or digit.
pub fn valid_return_type(name: &str) -> std::result::Result<String, String> {
    let n = name.trim();
    let ok = !n.is_empty()
        && n.chars().count() <= 24
        && n.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '-' | '/'));
    if ok {
        Ok(n.to_string())
    } else {
        Err("A return type is up to 24 letters, digits, spaces, dots, dashes or slashes, e.g. 990 or CA 540.".into())
    }
}

/// Adds one of the firm's own return types (for every PC).
pub fn add_return_type(
    root: &Path,
    name: &str,
    individual: bool,
) -> std::result::Result<ReturnType, String> {
    let name = valid_return_type(name)?;
    if return_types(root)
        .iter()
        .any(|t| t.name.eq_ignore_ascii_case(&name))
    {
        return Err(format!("{name} is a return type already."));
    }
    vf_fs::lock::with(root, "return_types", "the return types", || {
        let mut own = own_return_types(root);
        let t = ReturnType {
            name,
            individual,
            service: false,
        };
        own.push(t.clone());
        let bytes = serde_json::to_vec_pretty(&own).expect("serializes");
        vf_fs::cabinet::write_replace(&return_types_path(root), &bytes)?;
        Ok(t)
    })
}

/// Step kinds, in their order on the card (§1).
pub const LETTERS: &str = "letters";
pub const RETURN: &str = "return";
pub const PACKAGE: &str = "package";
pub const FEE: &str = "fee";
pub const ESIGN: &str = "esign_8879";

/// The doc type each step's PDF is filed as (§12.5), seeded when deliveries are turned on.
pub const STEP_DOC_TYPES: [(&str, &str); 5] = [
    (LETTERS, "Delivery — letters"),
    (RETURN, "Delivery — return record"),
    (PACKAGE, "Delivery — package selection"),
    (FEE, "Delivery — fee receipt"),
    (ESIGN, "Delivery — 8879 record"),
];
/// Where the step PDFs are filed in the client's folder.
pub const DELIVERY_FOLDER: &str = "Income Tax/{Year}/Delivery";

pub fn doc_type_of_step(step: &str) -> Option<&'static str> {
    STEP_DOC_TYPES
        .iter()
        .find(|(k, _)| *k == step)
        .map(|(_, n)| *n)
}

/// An annual agreement's step PDFs (ADDENDUM-annual-agreements.md §3.4): its own doc types,
/// filed in the client's Correspondence for the agreement's year.
pub const AGREEMENT_STEP_DOC_TYPES: [(&str, &str); 3] = [
    (LETTERS, "Agreement — signed letters"),
    (PACKAGE, "Agreement — package selection"),
    (FEE, "Agreement — payment record"),
];
pub const AGREEMENT_FOLDER: &str = "Correspondence/{Year}/Engagement";

/// The doc type of a step PDF of a delivery of this kind.
pub fn doc_type_of(kind: Option<&str>, step: &str) -> Option<&'static str> {
    if kind == Some("agreement") {
        AGREEMENT_STEP_DOC_TYPES
            .iter()
            .find(|(k, _)| *k == step)
            .map(|(_, n)| *n)
    } else {
        doc_type_of_step(step)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Locks {
    pub return_after_letters: bool,
    pub esign_after_letters: bool,
    pub esign_after_fee: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryStep {
    pub kind: String,
    pub n: u8,
    pub name: String,
    pub present: bool,
    /// `absent`, `coming`, `locked`, `open`, `waiting`, `done`.
    pub state: String,
    #[serde(default)]
    pub after: Vec<u8>,
    pub said_at: Option<String>,
    pub completed_at: Option<String>,
    pub completed_by: Option<String>,
    #[serde(default)]
    pub rounds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryEvent {
    pub at: String,
    #[serde(default)]
    pub step: String,
    pub kind: String,
    #[serde(default)]
    pub words: String,
    #[serde(default)]
    pub by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LetterSignature {
    pub signer_id: String,
    pub signed_at: String,
    pub via: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryLetter {
    pub id: String,
    pub name: String,
    pub pages: u32,
    pub sha256: String,
    #[serde(default)]
    pub template_id: String,
    #[serde(default)]
    pub template_version: u32,
    pub signed_sha256: Option<String>,
    #[serde(default)]
    pub signatures: Vec<LetterSignature>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeliverySigner {
    #[serde(default)]
    pub id: String,
    /// `taxpayer`, `spouse`, `officer`.
    pub role: String,
    pub name: String,
    #[serde(default)]
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FeeLineOut {
    pub key: String,
    pub kind: String,
    pub name: String,
    pub cents: u64,
    #[serde(default)]
    pub recommended: bool,
    #[serde(default)]
    pub selected: bool,
    /// An agreement's line: `once` or `month`.
    #[serde(default = "once")]
    pub every: String,
}

fn once() -> String {
    "once".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FeePayment {
    pub provider: String,
    pub method: Option<String>,
    /// `open`, `started`, `paid`, `failed`, `expired`, `wrong_amount` (portal 0.5.5).
    pub state: String,
    pub amount_cents: u64,
    #[serde(default)]
    pub card_fee_cents: u64,
    #[serde(default)]
    pub note: String,
    pub paid_at: Option<String>,
    pub started_at: Option<String>,
    pub failed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryFee {
    /// `amount`, `addons`, `packages`, or none yet.
    pub mode: Option<String>,
    #[serde(default)]
    pub lines: Vec<FeeLineOut>,
    #[serde(default)]
    pub total_cents: u64,
    pub selection_signed_at: Option<String>,
    pub selection_signed_by: Option<String>,
    #[serde(default)]
    pub payments: Vec<FeePayment>,
}

impl DeliveryFee {
    /// "Paid $1,642.68 by card", "Bank payment started", "Bank payment failed", "Not paid".
    pub fn payment_words(&self) -> String {
        // QA 2026-11-25: money taken for the wrong amount is said until the fee is paid, even when
        // a later try failed (staff have it to sort out).
        let latest = self
            .payments
            .iter()
            .rev()
            .find(|p| p.state != "open" && p.state != "expired");
        let wrong = self
            .payments
            .iter()
            .rev()
            .find(|p| p.state == "wrong_amount");
        let pick = match latest {
            Some(p) if p.state == "paid" => Some(p),
            _ => wrong.or(latest),
        };
        match pick {
            Some(p) if p.state == "paid" => format!(
                "Paid {} {}",
                crate::balance::money(p.amount_cents + p.card_fee_cents),
                match (p.provider.as_str(), p.method.as_deref()) {
                    ("manual", _) => format!(
                        "(marked paid{})",
                        if p.note.is_empty() {
                            String::new()
                        } else {
                            format!(": {}", p.note)
                        }
                    ),
                    (_, Some("bank")) => "by bank account".into(),
                    _ => "by card".into(),
                }
            ),
            // Portal 0.5.5: a Stripe payment of another amount than the fee is kept apart.
            Some(p) if p.state == "wrong_amount" => format!(
                "Paid {}, not the fee: check it in Stripe",
                crate::balance::money(p.amount_cents + p.card_fee_cents)
            ),
            Some(p) if p.state == "started" => "Bank payment started".into(),
            Some(p) if p.state == "failed" => "Bank payment failed: not paid".into(),
            _ => "Not paid".into(),
        }
    }
}

/// A line of the priced snapshot Vibe Files sends.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FeeLineIn {
    pub key: String,
    /// `base`, `package`, `addon`, or `service` (an agreement's line).
    pub kind: String,
    pub name: String,
    pub text: String,
    pub cents: u64,
    pub recommended: bool,
    pub services: Vec<crate::pricing::Service>,
    /// An agreement's line: `once` or `month` (none: once).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub every: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReturnFile {
    pub id: String,
    pub name: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Delivery {
    pub id: String,
    pub client_id: String,
    pub tax_year: i32,
    pub return_type: String,
    /// `return`, or `agreement` (an annual service agreement: letters, package, fee).
    #[serde(default = "return_kind")]
    pub kind: String,
    /// An agreement's returns and services, and the tax year of the returns it covers.
    #[serde(default)]
    pub covers: Vec<String>,
    #[serde(default)]
    pub return_year: Option<i32>,
    pub title: String,
    /// `open`, `complete`, `cancelled`.
    pub status: String,
    pub locks: Locks,
    pub assigned_staff_id: Option<String>,
    #[serde(default)]
    pub notify: bool,
    #[serde(default)]
    pub remind_days: u32,
    pub reminded_at: Option<String>,
    pub return_file: Option<ReturnFile>,
    pub esign_link: Option<String>,
    /// Step 1's letters (Addendum D3).
    #[serde(default)]
    pub letters: Vec<DeliveryLetter>,
    #[serde(default)]
    pub signers: Vec<DeliverySigner>,
    /// Steps 3 and 4 (Addendum D4/D5).
    #[serde(default)]
    pub fee: Option<DeliveryFee>,
    #[serde(default)]
    pub created_by: String,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub cancelled_at: Option<String>,
    pub steps: Vec<DeliveryStep>,
    /// Only on one delivery's read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<DeliveryEvent>,
}

fn return_kind() -> String {
    "return".into()
}

impl Delivery {
    pub fn is_agreement(&self) -> bool {
        self.kind == "agreement"
    }

    pub fn step(&self, kind: &str) -> Option<&DeliveryStep> {
        self.steps.iter().find(|s| s.kind == kind)
    }

    /// The steps included, done and in all: (3, 5).
    pub fn progress(&self) -> (usize, usize) {
        let present: Vec<&DeliveryStep> = self.steps.iter().filter(|s| s.present).collect();
        (
            present.iter().filter(|s| s.state == "done").count(),
            present.len(),
        )
    }

    /// The step waiting on someone: the first included and not done, in card order.
    pub fn waiting(&self) -> Option<&DeliveryStep> {
        self.steps.iter().find(|s| s.present && s.state != "done")
    }

    /// What it is, in a list: "1040" or "Agreement".
    pub fn what(&self) -> &str {
        if self.is_agreement() {
            "Agreement"
        } else {
            &self.return_type
        }
    }

    /// "Tax return delivery: 3 of 5 done", for the client panel.
    pub fn line(&self) -> String {
        let (done, all) = self.progress();
        match self.status.as_str() {
            "complete" => format!("{} {}: complete", self.tax_year, self.what()),
            "cancelled" => format!("{} {}: cancelled", self.tax_year, self.what()),
            _ => format!(
                "{} {}: {done} of {all} done{}",
                self.tax_year,
                self.what(),
                self.waiting()
                    .map(|s| format!(" · {} {}", s.name, state_words(&s.state)))
                    .unwrap_or_default()
            ),
        }
    }
}

/// A step's state in words.
pub fn state_words(state: &str) -> &'static str {
    match state {
        "coming" => "not added yet",
        "locked" => "locked",
        "open" => "with the client",
        "waiting" => "waiting for you to confirm",
        "done" => "done",
        _ => "",
    }
}

/// What Send to client makes.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NewDelivery {
    pub client_id: String,
    pub tax_year: i32,
    pub return_type: String,
    pub title: String,
    pub steps: StepsIn,
    pub locks: Locks,
    pub assigned_staff_id: Option<String>,
    pub notify: bool,
    /// Not told at create: [`Portal::ready_delivery`] tells the client once the fee and letters
    /// are attached (QA 2026-11-25; an older portal ignores it and tells at create).
    pub hold: bool,
    pub remind_days: u32,
    /// An annual agreement (none: a tax return delivery).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub covers: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub return_year: Option<i32>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StepsIn {
    pub letters: bool,
    #[serde(rename = "return")]
    pub return_: bool,
    pub package: bool,
    pub fee: bool,
    pub esign8879: bool,
}

/// What Save changes sends (only what is set).
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryChange {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps: Option<StepsIn>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locks: Option<Locks>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assigned_staff_id: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notify: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remind_days: Option<u32>,
}

impl Status {
    /// The portal has tax return delivery, turned on (Addendum D1).
    pub fn has_deliveries(&self) -> bool {
        self.features.iter().any(|f| f == "deliveries")
    }

    /// The portal has annual agreements (Vibe Portal 0.5.5), with deliveries on.
    pub fn has_agreements(&self) -> bool {
        self.features.iter().any(|f| f == "agreements")
    }
}

#[derive(Deserialize)]
struct One {
    delivery: Delivery,
}

/// Largest report read.
const REPORT_LIMIT: u64 = 64 * 1024 * 1024;

impl Portal {
    /// Deliveries, newest first (summaries without events); one client's when given.
    pub fn deliveries(&self, client_id: Option<&str>) -> Result<Vec<Delivery>> {
        #[derive(Deserialize)]
        struct R {
            deliveries: Vec<Delivery>,
        }
        let q: Vec<(&str, &str)> = client_id.map(|c| vec![("clientId", c)]).unwrap_or_default();
        self.get::<R>("/deliveries", &q).map(|r| r.deliveries)
    }

    /// Deliveries and agreements both (the Deliveries page, the copies in client folders): a
    /// portal without agreements lists its deliveries.
    pub fn deliveries_all(&self, client_id: Option<&str>) -> Result<Vec<Delivery>> {
        #[derive(Deserialize)]
        struct R {
            deliveries: Vec<Delivery>,
        }
        let mut q: Vec<(&str, &str)> = vec![("kind", "all")];
        if let Some(c) = client_id {
            q.push(("clientId", c));
        }
        self.get::<R>("/deliveries", &q).map(|r| r.deliveries)
    }

    /// A client's annual agreements, newest first (as listed: without their events). An older
    /// portal ignores `kind` and lists every delivery: only agreements are kept (review
    /// 2026-11-25: the return deliveries came back twice).
    pub fn agreements(&self, client_id: &str) -> Result<Vec<Delivery>> {
        #[derive(Deserialize)]
        struct R {
            deliveries: Vec<Delivery>,
        }
        self.get::<R>(
            "/deliveries",
            &[("kind", "agreement"), ("clientId", client_id)],
        )
        .map(|r| {
            r.deliveries
                .into_iter()
                .filter(Delivery::is_agreement)
                .collect()
        })
    }

    /// The deliveries of a tax year and return type (a pricing plan's, Q44).
    pub fn deliveries_for(&self, tax_year: i32, return_type: &str) -> Result<Vec<Delivery>> {
        #[derive(Deserialize)]
        struct R {
            deliveries: Vec<Delivery>,
        }
        let y = tax_year.to_string();
        self.get::<R>(
            "/deliveries",
            &[("taxYear", y.as_str()), ("returnType", return_type)],
        )
        .map(|r| r.deliveries)
    }

    pub fn delivery(&self, id: &str) -> Result<Delivery> {
        self.get::<One>(&format!("/deliveries/{id}"), &[])
            .map(|r| r.delivery)
    }

    pub fn create_delivery(&self, d: &NewDelivery) -> Result<Delivery> {
        self.send_json::<One>(
            "POST",
            "/deliveries",
            &serde_json::to_value(d).expect("serializes"),
        )
        .map(|r| r.delivery)
    }

    pub fn change_delivery(&self, id: &str, c: &DeliveryChange) -> Result<Delivery> {
        self.send_json::<One>(
            "PATCH",
            &format!("/deliveries/{id}"),
            &serde_json::to_value(c).expect("serializes"),
        )
        .map(|r| r.delivery)
    }

    /// Step 2: the return, a file already on the portal for this client.
    pub fn set_delivery_return(&self, id: &str, file_id: &str) -> Result<Delivery> {
        self.send_json::<One>(
            "PUT",
            &format!("/deliveries/{id}/return"),
            &serde_json::json!({ "fileId": file_id }),
        )
        .map(|r| r.delivery)
    }

    /// T27: a revised return replaces the one the client saw: the card shows Updated, steps 2
    /// and 5 reopen (a new Form 8879 is signed); letters, package and fee stand.
    pub fn revise_return(&self, id: &str, file_id: &str) -> Result<Delivery> {
        self.send_json::<One>(
            "PUT",
            &format!("/deliveries/{id}/return"),
            &serde_json::json!({ "fileId": file_id, "revised": true }),
        )
        .map(|r| r.delivery)
    }

    /// Staff a delivery of this client may be assigned to.
    pub fn delivery_staff(&self, client_id: &str) -> Result<Vec<crate::balance::BalanceStaff>> {
        #[derive(Deserialize)]
        struct R {
            staff: Vec<crate::balance::BalanceStaff>,
        }
        self.get::<R>("/delivery-staff", &[("clientId", client_id)])
            .map(|r| r.staff)
    }

    /// Step 5: the client's UltraTax eSignature link (https only).
    pub fn set_esign_link(&self, id: &str, url: &str) -> Result<Delivery> {
        self.send_json::<One>(
            "PUT",
            &format!("/deliveries/{id}/esign-link"),
            &serde_json::json!({ "url": url }),
        )
        .map(|r| r.delivery)
    }

    /// Step 1: one letter as laid out, with its signature blocks (`blocks` is vf-letters'
    /// JSON). Refused once anyone signed (`letters_signed`).
    #[allow(clippy::too_many_arguments)]
    pub fn add_letter(
        &self,
        id: &str,
        name: &str,
        doc_type: &str,
        template_id: &str,
        template_version: u32,
        blocks_json: &str,
        pdf: &[u8],
    ) -> Result<Delivery> {
        self.add_letter_to(
            id,
            "letters",
            None,
            name,
            doc_type,
            template_id,
            template_version,
            blocks_json,
            pdf,
        )
    }

    /// A letter for step 1 (`letters`) or a package's addendum for step 3 (`package`, tied to
    /// its fee line's key; T20).
    #[allow(clippy::too_many_arguments)]
    pub fn add_letter_to(
        &self,
        id: &str,
        step: &str,
        package_line: Option<&str>,
        name: &str,
        doc_type: &str,
        template_id: &str,
        template_version: u32,
        blocks_json: &str,
        pdf: &[u8],
    ) -> Result<Delivery> {
        let blocks = crate::delivery::b64url(blocks_json.as_bytes());
        let version = template_version.to_string();
        let mut q = vec![
            ("name", name),
            ("docType", doc_type),
            ("templateId", template_id),
            ("templateVersion", version.as_str()),
            ("step", step),
            ("blocks", blocks.as_str()),
        ];
        if let Some(k) = package_line {
            q.push(("packageLineId", k));
        }
        self.post_bytes::<One>(&format!("/deliveries/{id}/letters"), &q, pdf)
            .map(|r| r.delivery)
    }

    /// Steps 3 and 4: the priced snapshot (T29). Refused once the selection is signed.
    pub fn set_fee(&self, id: &str, mode: &str, lines: &[FeeLineIn]) -> Result<Delivery> {
        self.send_json::<One>(
            "PUT",
            &format!("/deliveries/{id}/fee"),
            &serde_json::json!({ "mode": mode, "lines": lines }),
        )
        .map(|r| r.delivery)
    }

    /// Step 4 paid outside Stripe, with a note (a check number).
    pub fn mark_paid(&self, id: &str, note: &str) -> Result<Delivery> {
        self.send_json::<One>(
            "POST",
            &format!("/deliveries/{id}/mark-paid"),
            &serde_json::json!({ "note": note }),
        )
        .map(|r| r.delivery)
    }

    pub fn remove_letter(&self, id: &str, letter_id: &str) -> Result<Delivery> {
        self.delete::<One>(&format!("/deliveries/{id}/letters/{letter_id}"))
            .map(|r| r.delivery)
    }

    /// Who signs step 1's letters (replacing the list).
    pub fn set_signers(&self, id: &str, signers: &[DeliverySigner]) -> Result<Delivery> {
        let list: Vec<serde_json::Value> = signers
            .iter()
            .map(|s| {
                serde_json::json!({
                    "role": s.role, "name": s.name, "title": s.title,
                    "email": s.email, "phone": s.phone,
                })
            })
            .collect();
        self.send_json::<One>(
            "PUT",
            &format!("/deliveries/{id}/signers"),
            &serde_json::json!({ "signers": list }),
        )
        .map(|r| r.delivery)
    }

    pub fn confirm_8879(&self, id: &str) -> Result<Delivery> {
        self.post_empty::<One>(&format!("/deliveries/{id}/confirm-8879"))
            .map(|r| r.delivery)
    }

    pub fn cancel_delivery(&self, id: &str) -> Result<Delivery> {
        self.post_empty::<One>(&format!("/deliveries/{id}/cancel"))
            .map(|r| r.delivery)
    }

    /// Tells the client a card held at create is ready, once (a second call tells nobody); how
    /// many messages went. An older portal has no such route and told them at create: 0.
    pub fn ready_delivery(&self, id: &str) -> Result<u32> {
        #[derive(Deserialize)]
        struct R {
            told: u32,
        }
        match self.post_empty::<R>(&format!("/deliveries/{id}/ready")) {
            Ok(r) => Ok(r.told),
            Err(e) if e.status() == Some(404) => Ok(0),
            Err(e) => Err(e),
        }
    }

    /// Tells the client again; how many messages went.
    pub fn remind_delivery(&self, id: &str) -> Result<u32> {
        #[derive(Deserialize)]
        struct R {
            told: u32,
        }
        self.post_empty::<R>(&format!("/deliveries/{id}/remind"))
            .map(|r| r.told)
    }

    /// The whole delivery's report PDF.
    pub fn delivery_report(&self, id: &str) -> Result<Vec<u8>> {
        use std::io::Read;
        let mut res = self
            .raw_get(&format!("/deliveries/{id}/report.pdf"))
            .map_err(|e| PortalError::Network(e.to_string()))?;
        let status = res.status().as_u16();
        if status != 200 {
            let text = res.body_mut().read_to_string().unwrap_or_default();
            return Err(crate::api::http_error(status, &text));
        }
        let mut bytes = Vec::new();
        res.body_mut()
            .with_config()
            .limit(REPORT_LIMIT)
            .reader()
            .read_to_end(&mut bytes)
            .map_err(|e| PortalError::Network(e.to_string()))?;
        if !bytes.starts_with(b"%PDF") {
            return Err(PortalError::Format("the report is not a PDF".into()));
        }
        Ok(bytes)
    }
}

/// Base64url without padding (the letter blocks travel in the query string).
pub fn b64url(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        out.push(A[(n >> 18) as usize & 63] as char);
        out.push(A[(n >> 12) as usize & 63] as char);
        if c.len() > 1 {
            out.push(A[(n >> 6) as usize & 63] as char);
        }
        if c.len() > 2 {
            out.push(A[n as usize & 63] as char);
        }
    }
    out
}

// ---- The copy in the client's folder ----------------------------------------------------------

pub const COPY_DIR: &str = "delivery";

/// `<cabinet>/<client folder>/_Vibe/delivery/<id>.jsonl`.
pub fn copy_path(root: &Path, folder_rel: &str, id: &str) -> PathBuf {
    root.join(folder_rel)
        .join(vf_fs::cabinet::VIBE_DIR)
        .join(COPY_DIR)
        .join(format!("{}.jsonl", crate::todo::safe_id(id)))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CopyLine {
    pub v: u32,
    pub read_at: String,
    pub read_by: String,
    pub delivery: Delivery,
}

/// The newest line of a copy (`None` when there is none).
pub fn newest_copy(path: &Path) -> Option<CopyLine> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .and_then(|l| serde_json::from_str(l).ok())
}

/// The line to append when the delivery changed since the copy's last line (events count).
pub fn copy_line(
    last: Option<&CopyLine>,
    d: &Delivery,
    read_at: &str,
    read_by: &str,
) -> Option<String> {
    if last.is_some_and(|l| l.delivery == *d) {
        return None;
    }
    Some(
        serde_json::to_string(&CopyLine {
            v: 1,
            read_at: read_at.into(),
            read_by: read_by.into(),
            delivery: d.clone(),
        })
        .expect("serializes"),
    )
}

/// The newest copy of each delivery in a client folder, newest first.
pub fn read_copies(root: &Path, folder_rel: &str) -> Vec<CopyLine> {
    let dir = root
        .join(folder_rel)
        .join(vf_fs::cabinet::VIBE_DIR)
        .join(COPY_DIR);
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<CopyLine> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| newest_copy(&e.path()))
        .collect();
    out.sort_by(|a, b| b.delivery.created_at.cmp(&a.delivery.created_at));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_payment_of_the_wrong_amount_is_said() {
        let fee: DeliveryFee = serde_json::from_str(
            r#"{"mode":"amount","totalCents":35000,"selectionSignedAt":null,
                "selectionSignedBy":null,"payments":[{"provider":"stripe","method":"card",
                "state":"wrong_amount","amountCents":30000,"cardFeeCents":900,
                "paidAt":"2026-11-25T12:00:00Z","startedAt":null,"failedAt":null}]}"#,
        )
        .unwrap();
        assert_eq!(
            fee.payment_words(),
            "Paid $309.00, not the fee: check it in Stripe"
        );
    }

    #[test]
    fn the_firms_own_return_types() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        std::fs::create_dir_all(root.join("_Vibe")).unwrap();
        assert_eq!(return_type_names(root), RETURN_TYPES);
        assert!(is_individual(root, "1040") && !is_individual(root, "1065"));
        add_return_type(root, " CA 540 ", true).unwrap();
        add_return_type(root, "CA 100", false).unwrap();
        assert!(add_return_type(root, "ca 540", false).is_err());
        // Built in since annual agreements (2026-11-25).
        assert!(add_return_type(root, "990", false).is_err());
        assert!(add_return_type(root, "Bookkeeping", false).is_err());
        assert!(add_return_type(root, "<b>", false).is_err());
        assert!(add_return_type(root, &"9".repeat(25), false).is_err());
        let mut want: Vec<&str> = RETURN_TYPES.to_vec();
        want.extend(["CA 540", "CA 100"]);
        assert_eq!(return_type_names(root), want);
        assert!(is_individual(root, "CA 540") && !is_individual(root, "CA 100"));
        assert!(is_individual(root, "709") && !is_individual(root, "990"));
        // Services are offered to agreements and letters, not to Deliver tax return….
        assert!(service_names(root).contains(&"Payroll".to_string()));
        assert!(!return_type_names(root).contains(&"Payroll".to_string()));
        assert_eq!(letter_context(root, "1040"), "1040|Individual");
        assert_eq!(letter_context(root, "1065"), "1065|Entity");
    }

    /// A delivery as the portal's tests answer it (Vibe-Portal deliveries.test.ts).
    pub(crate) const DELIVERY_JSON: &str = r#"{
      "id":"d1111111-1111-4111-8111-111111111111","clientId":"11111111-1111-4111-8111-111111111111",
      "taxYear":2025,"returnType":"1040","title":"Your 2025 tax return","status":"open",
      "locks":{"returnAfterLetters":true,"esignAfterLetters":true,"esignAfterFee":true},
      "assignedStaffId":null,"notify":true,"remindDays":0,"remindedAt":null,
      "returnFile":{"id":"f1","name":"2025 Form 1040 - Smith.pdf","sha256":"ab"},
      "esignLink":"https://esign.example-tax.test/s/abc","createdBy":"PC1\\kurt",
      "createdAt":"2026-11-24T12:00:00.000Z","completedAt":null,"cancelledAt":null,
      "steps":[
        {"kind":"letters","n":1,"name":"Letters","present":false,"state":"absent","after":[],"saidAt":null,"completedAt":null,"completedBy":null,"rounds":0},
        {"kind":"return","n":2,"name":"Tax return","present":true,"state":"done","after":[],"saidAt":null,"completedAt":"2026-11-24T13:00:00.000Z","completedBy":"Jordan","rounds":1},
        {"kind":"package","n":3,"name":"Package selection","present":false,"state":"absent","after":[],"saidAt":null,"completedAt":null,"completedBy":null,"rounds":0},
        {"kind":"fee","n":4,"name":"Fee","present":false,"state":"absent","after":[],"saidAt":null,"completedAt":null,"completedBy":null,"rounds":0},
        {"kind":"esign_8879","n":5,"name":"Form 8879","present":true,"state":"waiting","after":[],"saidAt":"2026-11-24T14:00:00.000Z","completedAt":null,"completedBy":null,"rounds":0}
      ],
      "events":[{"at":"2026-11-24T12:00:00.000Z","step":"","kind":"created","words":"Sent to the client","by":"PC1\\kurt"}]}"#;

    #[test]
    fn reads_a_delivery_and_says_where_it_is() {
        let d: Delivery = serde_json::from_str(DELIVERY_JSON).unwrap();
        assert_eq!(d.progress(), (1, 2));
        assert_eq!(
            d.line(),
            "2025 1040: 1 of 2 done · Form 8879 waiting for you to confirm"
        );
        assert_eq!(doc_type_of_step(ESIGN), Some("Delivery — 8879 record"));
        let steps = StepsIn {
            return_: true,
            esign8879: true,
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(&steps).unwrap(),
            serde_json::json!({ "letters": false, "return": true, "package": false, "fee": false, "esign8879": true })
        );
    }

    #[test]
    fn the_copy_gets_a_line_only_when_the_delivery_changed() {
        let t = tempfile::tempdir().unwrap();
        let d: Delivery = serde_json::from_str(DELIVERY_JSON).unwrap();
        let p = copy_path(t.path(), "Smith, John (1001)", &d.id);
        let first = copy_line(None, &d, "now", "PC1\\kurt").unwrap();
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, format!("{first}\n")).unwrap();
        let last = newest_copy(&p);
        assert_eq!(copy_line(last.as_ref(), &d, "later", "PC2\\ann"), None);
        let mut done = d.clone();
        done.status = "complete".into();
        assert!(copy_line(last.as_ref(), &done, "later", "PC2\\ann").is_some());
        assert_eq!(read_copies(t.path(), "Smith, John (1001)").len(), 1);
    }
}
