//! Calls to the portal's firm API (`/api/firm/v1`), authorised by the firm key. Every call names
//! this PC and Windows user, which the portal keeps in its history (Vibe Portal P15).

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::settings::PortalSettings;

#[derive(Debug, thiserror::Error)]
pub enum PortalError {
    #[error("The portal cannot be reached: {0}")]
    Network(String),
    #[error("The portal said no ({status}: {code})")]
    Http { status: u16, code: String },
    #[error("The portal's answer could not be read: {0}")]
    Format(String),
    #[error("{0}")]
    Local(String),
}

impl PortalError {
    /// The portal's stable error code, when it gave one.
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Http { code, .. } => Some(code),
            _ => None,
        }
    }

    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Http { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// Plain words for the screen.
    pub fn message(&self) -> String {
        match self.code() {
            Some("bad_firm_key") => "The portal key is not right, or it was revoked.".into(),
            Some("too_many_requests") => "The portal asks to wait a few minutes.".into(),
            Some("client_inactive") => "This client is switched off on the portal.".into(),
            Some("already_has_access") => "That person already has access to this client.".into(),
            Some("invalid_email") => "That email address does not look right.".into(),
            Some("invalid_phone") => "That phone number does not look right.".into(),
            Some("account_off") => "Their portal account is turned off.".into(),
            Some("contact_required") => "Enter an email address or a mobile number.".into(),
            Some("too_large") => "The file is larger than the portal allows.".into(),
            Some("not_a_user_of_this_client") => {
                "Someone chosen no longer has access to this client.".into()
            }
            Some("already_fetched") => "Another PC already fetched it.".into(),
            Some("not_fetched") => "The portal says it was never fetched.".into(),
            // Secure links (Vibe-Portal P49, P68).
            Some("file_not_found") => "The document is not on the portal any more.".into(),
            Some("cannot_watermark") => {
                "The PDF cannot be watermarked (locked with a password or damaged): send it without the watermark, or another copy.".into()
            }
            Some("too_many_shares") => {
                "You have sent as many secure links as the portal allows today.".into()
            }
            Some("recipient_has_too_many") => {
                "That person already has 5 working secure links for this client: end one on the portal's staff page first.".into()
            }
            Some("view_only_needs_preview") => {
                "View only works for a PDF or a picture the portal page can show: allow download for this file.".into()
            }
            Some("too_large_to_watermark") => {
                "The PDF is too large to watermark: send it without the watermark.".into()
            }
            Some("email_failed") => {
                "The portal could not send the email, so no link was made. Try again.".into()
            }
            // Checking who asks for access (Vibe Portal Addendum 01, layer B).
            Some("check_required") => {
                "The portal asks how you checked who this is before approving.".into()
            }
            Some("check_note_required") => {
                "Say who you spoke to, or how you know this person.".into()
            }
            // To-do lists (Vibe-Portal P109).
            Some("requests_off") => {
                "To-do lists are turned off on the portal: an admin turns them on in the portal's Settings.".into()
            }
            Some("item_answered") => {
                "The client has answered that item, so its wording and kind stay as they are.".into()
            }
            Some("list_closed") => "This list is closed or cancelled.".into(),
            Some("list_cancelled") => {
                "A cancelled list stays cancelled: send a new list instead.".into()
            }
            Some("list_has_answers") => {
                "The client has answered this list, so it is kept: cancel it instead.".into()
            }
            Some("staff_cannot_see_client") => {
                "That staff member cannot see this client on the portal: choose someone else.".into()
            }
            Some("check_invalid") => {
                "The portal did not accept that way of checking: update Vibe Portal, or approve on the portal's staff page.".into()
            }
            // Tax return delivery and annual agreements (Vibe Portal Addenda 02 and 03).
            Some("deliveries_off") => {
                "Tax return delivery is turned off on the portal: an admin turns it on in the portal's Settings.".into()
            }
            Some("client_not_found") => {
                "This client is not on the portal yet: share a document or invite someone first.".into()
            }
            Some("delivery_exists") => {
                "This client already has an open card for that year: open it (Read again), or cancel it first.".into()
            }
            Some("delivery_closed") => {
                "That card is finished or cancelled: send a new one.".into()
            }
            Some("no_steps") => "Tick at least one step to send.".into(),
            Some("agreement_steps") => {
                "An agreement has no tax return or Form 8879 step.".into()
            }
            Some("step_done") => "A step the client finished cannot be taken away.".into(),
            Some("letters_signed") => {
                "Someone has signed, so the letters and who signs are fixed: cancel and send a new card to change them.".into()
            }
            Some("signer_has_no_contact") => {
                "A second signer needs their own email or mobile number (their code goes there).".into()
            }
            Some("selection_signed") => {
                "The client signed their selection, so the prices and package letters are fixed.".into()
            }
            Some("fee_paid") => {
                "A payment has started or been made, so the fee and its package letters are fixed.".into()
            }
            Some("no_package_step") => {
                "Packages are chosen on their own step: tick Package to choose to offer them.".into()
            }
            Some("fee_needs_package") => {
                "The fee is chosen as a package: change the fee before taking Package to choose away.".into()
            }
            Some("balance_waits") => {
                "A bank payment is still clearing, so the client cannot pay the new amount yet.".into()
            }
            Some("already_paid") => "It is paid already.".into(),
            Some("fee_mode") | Some("service_lines") => {
                "The portal did not accept these fee lines: update Vibe Portal to 0.5.5 or later.".into()
            }
            Some("one_amount") => "Give one amount for the fee.".into(),
            Some("no_packages") => "Offer at least one package.".into(),
            Some("one_recommended") => "Recommend one package at most.".into(),
            Some("return_seen") => {
                "The client has seen the return: send it as a revised return.".into()
            }
            Some("no_8879_step") => "This card has no Form 8879 step.".into(),
            Some("already_confirmed") => "Form 8879 was confirmed already.".into(),
            Some("invalid_url") => "The eSignature link must start with https://.".into(),
            Some("not_a_pdf") => "The letter is not a PDF the portal can read.".into(),
            Some("invalid_blocks") => {
                "The letter's signature places could not be read: open the template and save it again.".into()
            }
            Some("payments_off") => {
                "Payments are not set up on the portal (its Settings › Payments).".into()
            }
            Some("stripe_refused") => "Stripe refused the request: see Settings › Payments › Test on the portal.".into(),
            Some("subscription_ended") => "That monthly charge has ended.".into(),
            Some("no_monthly_charge") => "This client has no monthly charge.".into(),
            Some("not_found") => "The portal does not have it any more: press Read again.".into(),
            _ => self.to_string(),
        }
    }
}

pub type Result<T> = std::result::Result<T, PortalError>;

/// A secure link to someone outside the client (Vibe-Portal P49, P68).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutsideLink {
    pub file_ids: Vec<String>,
    pub email: String,
    pub name: String,
    pub organization: String,
    pub message: String,
    pub can_download: bool,
    pub watermark: bool,
    pub days: u32,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub version: String,
    pub firm_name: String,
    pub portal_url: String,
    /// Where the portal keeps its (encrypted) files: "local" or "b2" (Vibe-Portal P51). Absent
    /// from portals before P51, which keep them on the appliance.
    #[serde(default)]
    pub storage: Option<String>,
    /// A move between the appliance and B2 is under way or not finished (P51).
    #[serde(default)]
    pub storage_moving: bool,
    /// What this portal offers that older ones do not (P109 `requests`, P111 `contacts`).
    #[serde(default)]
    pub features: Vec<String>,
}

/// The portal version Invite from client list, request matching by contacts and adding a
/// login at once need (Vibe-Portal P111–P113).
pub const CONTACTS_VERSION: &str = "0.5.2";

/// What the portal sends someone invited (Vibe-Portal `POST /clients/:id/invites`, its
/// `firm/routes.ts`; kept in step by hand): the email's subject and text. A text message is
/// the same words, shortened by [`text_message`].
pub fn invitation_words(firm: &str, client: &str, link: &str) -> (String, String) {
    (
        format!("{firm} invites you to its client portal"),
        format!(
            "{firm} has invited you to its client portal for {client}, where you can get files from the firm and send files to it.\n\nAccept the invitation (within 7 days): {link}"
        ),
    )
}

/// What the portal sends someone with a login given a client at once (P113).
pub fn added_words(firm: &str, client: &str, portal: &str) -> (String, String) {
    (
        format!("Your access to {firm}'s client portal"),
        format!(
            "{firm} has given you access to {client} on its client portal. Sign in at {portal}"
        ),
    )
}

/// A text message as the portal sends it (its `shortText`): at most 300 characters, the words
/// before a link cut short so the link stays whole.
pub fn text_message(body: &str) -> String {
    const MAX: usize = 300;
    if body.chars().count() <= MAX {
        return body.to_string();
    }
    match body.find("http") {
        None => format!("{}...", body.chars().take(MAX - 3).collect::<String>()),
        Some(at) => {
            let url = body[at..].split_whitespace().next().unwrap_or("");
            let room = MAX.saturating_sub(url.chars().count() + 4);
            let before = body[..at].trim_end();
            if before.chars().count() > room {
                format!("{}... {url}", before.chars().take(room).collect::<String>())
            } else {
                format!("{before} {url}")
            }
        }
    }
}

impl Status {
    /// The portal has what Invite from client list and request matching need (P111).
    pub fn has_contacts(&self) -> bool {
        self.features.iter().any(|f| f == "contacts")
    }

    /// Where the portal's files are, in words for Admin › Portal.
    pub fn storage_words(&self) -> &'static str {
        match (self.storage.as_deref(), self.storage_moving) {
            (_, true) => "Its files are being moved between the appliance and B2.",
            (Some("b2"), false) => "Its files are kept, encrypted, in the firm's own B2 bucket.",
            _ => "Its files are kept, encrypted, on the appliance.",
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub identity_id: String,
    pub name: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub role_label: String,
    pub active: bool,
    pub last_sign_in_at: Option<String>,
}

impl User {
    /// The name, else the email, else the phone.
    pub fn label(&self) -> String {
        if !self.name.trim().is_empty() {
            self.name.clone()
        } else {
            self.email
                .clone()
                .or_else(|| self.phone.clone())
                .unwrap_or_default()
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Invitation {
    pub invitation_id: String,
    pub name: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub role_label: String,
    pub expires_at: String,
    /// `invited` or `expired`.
    pub status: String,
}

/// A client someone on the portal can see.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PersonClient {
    pub id: String,
    pub name: String,
}

/// One client's people and waiting invitations (Vibe-Portal P111 `GET /contacts`); emails and
/// phones as the portal stores them (lowercase, E.164).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PortalContacts {
    pub id: String,
    pub active: bool,
    #[serde(default)]
    pub people: Vec<ContactPerson>,
    #[serde(default)]
    pub invitations: Vec<WaitingInvite>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContactPerson {
    pub identity_id: String,
    pub name: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    #[serde(default)]
    pub role_label: String,
    /// Their access to this client is on.
    pub active: bool,
    /// Their whole account is turned off.
    #[serde(default)]
    pub account_off: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WaitingInvite {
    pub invitation_id: String,
    pub name: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    #[serde(default)]
    pub role_label: String,
    /// `invited` or `expired`.
    pub status: String,
}

/// Someone already on the portal (Vibe-Portal P63 `GET /people`).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    pub identity_id: String,
    pub name: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    #[serde(default)]
    pub clients: Vec<PersonClient>,
}

impl Person {
    /// The name, else the email, else the phone.
    pub fn label(&self) -> String {
        if !self.name.trim().is_empty() {
            self.name.clone()
        } else {
            self.email
                .clone()
                .or_else(|| self.phone.clone())
                .unwrap_or_default()
        }
    }

    /// Whether they can already see client `client_id`.
    pub fn sees(&self, client_id: &str) -> bool {
        self.clients
            .iter()
            .any(|c| c.id.eq_ignore_ascii_case(client_id))
    }
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct Users {
    pub users: Vec<User>,
    pub invitations: Vec<Invitation>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AccessRequest {
    pub id: String,
    pub name: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    /// The contact that received and returned the code; only it is used when approved.
    pub confirmed_contact: Option<String>,
    pub client_as: String,
    pub note: String,
    pub created_at: String,
    /// What the barcode on the back of their ID said (Vibe Portal Addendum 01, layer A); absent
    /// when the firm asks for no scan, the barcode could not be read, or the portal is older.
    #[serde(default)]
    pub id_scan: Option<IdScan>,
    /// The portal's flags on the ID: `expired`, `name_differs`, `unreadable`, `no_barcode`.
    #[serde(default)]
    pub id_flags: Vec<String>,
}

/// The fields of an ID's barcode as the portal read them. Dates are `YYYY-MM-DD`. The licence
/// number is never among them.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct IdScan {
    pub family_name: String,
    pub first_name: String,
    pub middle_name: String,
    pub dob: Option<String>,
    pub expires: Option<String>,
    pub issued: Option<String>,
    pub address: IdAddress,
    pub issuer: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct IdAddress {
    pub street: String,
    pub city: String,
    pub state: String,
    pub zip: String,
}

impl IdScan {
    /// "ALEXANDER JAMES SAMPLE".
    pub fn full_name(&self) -> String {
        [&self.first_name, &self.middle_name, &self.family_name]
            .into_iter()
            .filter(|p| !p.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// "2570 24TH STREET, SACRAMENTO, CA, 95818".
    pub fn address_line(&self) -> String {
        let a = &self.address;
        let town = [&a.city, &a.state]
            .into_iter()
            .filter(|p| !p.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        [a.street.clone(), town, a.zip.clone()]
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// The requests waiting, and whether the firm asks for a check before approving.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccessRequests {
    pub requests: Vec<AccessRequest>,
    /// The portal's "Check before approving" is on: Approve needs a [`Check`]. `false` from a
    /// portal that does not say (one from before the setting).
    pub check_required: bool,
}

/// How staff checked who is asking, sent with an approval (Vibe Portal Addendum 01, layer B).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    /// `record_match`, `callback`, `in_person` or `other`.
    pub method: String,
    /// For `record_match`: what matched the client record (`email`, `mobile`, `dob`). Nothing
    /// of the record itself is sent.
    pub matched: Vec<String>,
    /// For `callback`, who they spoke to; for `other`, how they know the person.
    pub note: String,
}

impl Check {
    pub fn record_match(matched: &[&str]) -> Self {
        Self {
            method: "record_match".into(),
            matched: matched.iter().map(|m| (*m).to_string()).collect(),
            note: String::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileUser {
    pub identity_id: String,
    pub visible: bool,
    /// When they last opened the current content (None: new to them).
    pub viewed_at: Option<String>,
    pub first_viewed_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PortalFile {
    pub file_id: String,
    pub client_id: String,
    pub vf_document_id: String,
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub tax_year: Option<i32>,
    pub shared_at: String,
    pub shared_by: String,
    pub content_updated_at: Option<String>,
    #[serde(default)]
    pub users: Vec<FileUser>,
    /// Invitations not accepted yet that the file is held for (Vibe-Portal P98): the person
    /// sees it when they accept. An older portal sends none.
    #[serde(default)]
    pub invites: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UploadFrom {
    pub identity_id: String,
    pub name: String,
    pub email: Option<String>,
    pub phone: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Upload {
    pub upload_id: String,
    pub client_id: String,
    pub client_code: String,
    pub client_name: String,
    pub name: String,
    pub original_name: Option<String>,
    pub size: u64,
    pub sha256: String,
    pub note: String,
    pub batch_id: Option<String>,
    pub uploaded_at: String,
    pub from: UploadFrom,
    /// Sent for an item of a to-do list (Vibe-Portal P109); absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<UploadRequest>,
    /// A paid balance's receipt, made by the portal (Addendum D7); absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub balance: Option<UploadBalance>,
    /// A delivery's step PDF, made by the portal (Addendum D2); absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<UploadDelivery>,
    /// A monthly charge's receipt, made by the portal (annual agreements); absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription: Option<UploadSubscription>,
}

/// The delivery step a step PDF records.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UploadDelivery {
    pub id: String,
    pub step: String,
    pub year: Option<i32>,
    /// `return` (a tax return delivery), `agreement` (an annual agreement's step PDF), or
    /// (PA-114) `statements` or `document`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// The monthly charge a receipt upload is for.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UploadSubscription {
    pub subscription_id: String,
}

/// The balance a receipt upload is for.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UploadBalance {
    pub balance_id: String,
}

/// The to-do item an upload answers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UploadRequest {
    pub list_id: String,
    pub list_title: String,
    pub item_id: String,
    pub prompt: String,
    #[serde(default)]
    pub doc_type: String,
}

/// An upload Vibe Files fetched but has not filed or dismissed (Vibe-Portal P74).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Unfiled {
    #[serde(flatten)]
    pub upload: Upload,
    pub fetched_at: String,
    pub fetched_by: Option<String>,
}

/// The portal's list of uploads fetched and not filed, and how long it keeps them.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UnfiledList {
    /// "unfiled": an answer to this question, not an older portal's list of waiting uploads.
    pub state: String,
    /// Days the portal keeps an upload after it was filed or dismissed.
    pub keep_days: u32,
    /// Days after the fetch the portal clears an upload never filed.
    pub cap_days: u32,
    pub uploads: Vec<Unfiled>,
}

/// What an older portal (before Vibe-Portal P74) is told.
pub const NEEDS_P74: &str = "The portal does not keep uploads until they are filed yet: update it (sudo vibe upgrade portal).";

/// A portal history row, for Vibe Files' audit trail.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub id: u64,
    pub at: String,
    pub kind: String,
    pub actor_kind: String,
    pub actor: String,
    pub client_id: Option<String>,
    pub file_id: Option<String>,
    pub upload_id: Option<String>,
    pub identity_id: Option<String>,
    /// The cabinet document a portal file came from, when the event is about one.
    pub vf_document_id: Option<String>,
    #[serde(default)]
    pub detail: serde_json::Value,
}

/// Who has an upload after `/fetched`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    /// This PC (now, or an earlier call whose answer was lost).
    Ours,
    /// Another PC, or the client removed it.
    Other,
}

/// What adding a document to the portal did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Added {
    /// A new portal file.
    New(String),
    /// Already there with the same content.
    Existing(String),
    /// Already there with other content: use [`Portal::replace`].
    Changed(String),
}

impl Added {
    pub fn file_id(&self) -> &str {
        match self {
            Self::New(f) | Self::Existing(f) | Self::Changed(f) => f,
        }
    }
}

/// The portal's answer `text` with a status that is not success, as an error: its error code,
/// or for a redirect, that the address must change.
pub(crate) fn http_error(status: u16, text: &str) -> PortalError {
    if (300..400).contains(&status) {
        return PortalError::Local(
            "The portal address redirects elsewhere: use the address it redirects to (https://…)."
                .into(),
        );
    }
    let code = serde_json::from_str::<ErrorBody>(text)
        .ok()
        .and_then(|b| b.error)
        .unwrap_or_else(|| "error".into());
    PortalError::Http { status, code }
}

#[derive(Deserialize)]
struct ErrorBody {
    error: Option<String>,
    #[serde(rename = "fetchedBy")]
    fetched_by: Option<String>,
}

/// Largest JSON answer read (a big firm's file list).
const JSON_LIMIT: u64 = 64 * 1024 * 1024;

pub struct Portal {
    base: String,
    key: zeroize::Zeroizing<String>,
    pc: String,
    user: String,
    /// JSON calls: a 30-second limit.
    quick: ureq::Agent,
    /// File transfers: no overall limit (large files on slow lines), only connect and wait.
    bulk: ureq::Agent,
}

/// The address is this PC (the only place plain http is allowed).
pub fn is_loopback(url: &str) -> bool {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url);
    let host = rest.split('/').next().unwrap_or("");
    let host = host.rsplit_once(':').map_or(host, |(h, _)| h);
    matches!(host, "localhost" | "[::1]") || host.starts_with("127.")
}

fn agent(global: Option<Duration>, https_only: bool) -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        // The Windows certificate store, so a firm's own CA or an inspecting firewall works
        // (QA 2026-11-09: the feature alone does not switch it on).
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .https_only(https_only)
        // A redirect (http to https, another host) is an error to fix in the address.
        .max_redirects(0)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(120)))
        // A transfer that stalls ends instead of holding the fetch forever.
        .timeout_per_read(Some(Duration::from_secs(60)))
        .timeout_per_write(Some(Duration::from_secs(60)))
        .timeout_global(global)
        .user_agent(format!("Vibe Files/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .new_agent()
}

/// Header values fit for the portal's history: printable ASCII, at most 64 characters.
fn clean(v: &str) -> String {
    v.chars()
        .filter(|c| c.is_ascii() && !c.is_ascii_control())
        .take(64)
        .collect()
}

#[cfg(test)]
fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl Portal {
    /// A client for the portal in `s`, naming this PC and Windows user.
    pub fn new(s: &PortalSettings, pc: &str, user: &str) -> Self {
        let https_only = !is_loopback(&s.url);
        Self {
            base: format!("{}/api/firm/v1", s.url.trim_end_matches('/')),
            key: zeroize::Zeroizing::new(s.key.clone()),
            pc: clean(pc),
            user: clean(user),
            quick: agent(Some(Duration::from_secs(30)), https_only),
            bulk: agent(None, https_only),
        }
    }

    pub fn pc(&self) -> String {
        self.pc.clone()
    }

    /// How the portal names this PC and user in its history (its keys.ts `PC\user`).
    pub fn actor_name(&self) -> String {
        if self.pc.is_empty() && self.user.is_empty() {
            "Vibe Files".into()
        } else {
            let or = |s: &str| {
                if s.is_empty() {
                    "?".to_string()
                } else {
                    s.to_string()
                }
            };
            format!("{}\\{}", or(&self.pc), or(&self.user))
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn auth<B>(&self, r: ureq::RequestBuilder<B>) -> ureq::RequestBuilder<B> {
        r.header("Authorization", &format!("Bearer {}", self.key.as_str()))
            .header("X-Vibe-PC", &self.pc)
            .header("X-Vibe-User", &self.user)
    }

    /// Turns an answer into `T`, or the portal's error.
    fn read<T: DeserializeOwned>(
        res: std::result::Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Result<T> {
        let mut res = res.map_err(|e| PortalError::Network(e.to_string()))?;
        let status = res.status().as_u16();
        if (300..400).contains(&status) {
            return Err(http_error(status, ""));
        }
        let text = res
            .body_mut()
            .with_config()
            .limit(JSON_LIMIT)
            .read_to_string()
            .map_err(|e| PortalError::Network(e.to_string()))?;
        if !(200..300).contains(&status) {
            return Err(http_error(status, &text));
        }
        serde_json::from_str(&text).map_err(|e| PortalError::Format(e.to_string()))
    }

    pub(crate) fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, &str)]) -> Result<T> {
        let mut r = self.auth(self.quick.get(&self.url(path)));
        for (k, v) in query {
            r = r.query(k, v);
        }
        Self::read(r.call())
    }

    pub(crate) fn send_json<T: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T> {
        let url = self.url(path);
        let r = match method {
            "POST" => self.auth(self.quick.post(&url)).send_json(body),
            "PUT" => self.auth(self.quick.put(&url)).send_json(body),
            "PATCH" => self.auth(self.quick.patch(&url)).send_json(body),
            _ => unreachable!("send_json: {method}"),
        };
        Self::read(r)
    }

    /// A JSON body that can be large (a client's list of documents, PA-80): no overall time
    /// limit, only the limits on each wait.
    pub(crate) fn put_json_large<T: DeserializeOwned>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T> {
        Self::read(self.auth(self.bulk.put(&self.url(path))).send_json(body))
    }

    /// A POST of raw bytes (a letter PDF, Addendum D3), on the agent with no overall limit.
    pub(crate) fn post_bytes<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
        bytes: &[u8],
    ) -> Result<T> {
        let mut r = self
            .auth(self.bulk.post(&self.url(path)))
            .header("Content-Type", "application/octet-stream");
        for (k, v) in query {
            r = r.query(k, v);
        }
        Self::read(r.send(bytes))
    }

    pub(crate) fn post_empty<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        Self::read(self.auth(self.quick.post(&self.url(path))).send_empty())
    }

    /// A GET whose answer is read by the caller (a file), on the agent with no overall limit.
    pub(crate) fn raw_get(
        &self,
        path: &str,
    ) -> std::result::Result<ureq::http::Response<ureq::Body>, ureq::Error> {
        self.auth(self.bulk.get(&self.url(path))).call()
    }

    pub(crate) fn delete<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        Self::read(self.auth(self.quick.delete(&self.url(path))).call())
    }

    // ---- status, clients, people --------------------------------------------------------

    /// Admin › Portal's Test: the firm name and version, when the address and key are right.
    pub fn status(&self) -> Result<Status> {
        self.get("/status", &[])
    }

    /// Creates or updates the client on the portal (its id is the cabinet's client id).
    pub fn put_client(&self, id: &str, code: &str, name: &str, active: Option<bool>) -> Result<()> {
        let mut body = serde_json::json!({ "clientCode": code, "displayName": name });
        if let Some(a) = active {
            body["active"] = serde_json::json!(a);
        }
        self.send_json::<serde_json::Value>("PUT", &format!("/clients/{id}"), &body)
            .map(|_| ())
    }

    pub fn users(&self, client_id: &str) -> Result<Users> {
        self.get(&format!("/clients/{client_id}/users"), &[])
    }

    /// People already on the portal whose name, email or mobile number has every word of
    /// `q` (2 characters or more; 20 at most come back), with the clients they can see. A
    /// portal older than Vibe-Portal P63 answers 404.
    pub fn people(&self, q: &str) -> Result<Vec<Person>> {
        #[derive(Deserialize)]
        struct R {
            people: Vec<Person>,
        }
        self.get::<R>("/people", &[("q", q.trim())])
            .map(|r| r.people)
    }

    /// Every client on the portal with its people and invitations not accepted yet, in one
    /// read (Vibe-Portal P111). A portal older than 0.5.2 answers 404.
    pub fn contacts(&self) -> Result<Vec<PortalContacts>> {
        #[derive(Deserialize)]
        struct R {
            clients: Vec<PortalContacts>,
        }
        self.get::<R>("/contacts", &[]).map(|r| r.clients)
    }

    /// Gives someone who already has a portal login (found with [`Self::people`]) this client at
    /// once, without an invitation (Vibe-Portal P113); the portal tells them. A portal older
    /// than 0.5.2 answers 404 `not_found`.
    pub fn add_person(&self, client_id: &str, identity_id: &str, role: &str) -> Result<()> {
        self.send_json::<serde_json::Value>(
            "POST",
            &format!("/clients/{client_id}/people"),
            &serde_json::json!({ "identityId": identity_id, "roleLabel": role }),
        )
        .map(|_| ())
    }

    /// Invites someone; the portal sends the link to the email when given, else by text.
    pub fn invite(
        &self,
        client_id: &str,
        email: Option<&str>,
        phone: Option<&str>,
        name: &str,
        role: &str,
    ) -> Result<String> {
        let mut body = serde_json::json!({ "name": name, "roleLabel": role });
        if let Some(e) = email.filter(|e| !e.trim().is_empty()) {
            body["email"] = serde_json::json!(e.trim());
        }
        if let Some(p) = phone.filter(|p| !p.trim().is_empty()) {
            body["phone"] = serde_json::json!(p.trim());
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct R {
            invitation_id: String,
        }
        self.send_json::<R>("POST", &format!("/clients/{client_id}/invites"), &body)
            .map(|r| r.invitation_id)
    }

    /// Sends a secure link to someone outside the client (Vibe-Portal P68) for files already on
    /// the portal; the portal emails it. Returns when the link expires (RFC 3339). A portal
    /// older than P68 answers 404.
    pub fn send_outside(&self, client_id: &str, l: &OutsideLink) -> Result<String> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct R {
            expires_at: String,
        }
        let body = serde_json::json!({
            "clientId": client_id,
            "fileIds": l.file_ids,
            "email": l.email.trim(),
            "name": l.name.trim(),
            "organization": l.organization.trim(),
            "message": l.message.trim(),
            "canDownload": l.can_download,
            "watermark": l.watermark,
            "days": l.days,
        });
        self.send_json::<R>("POST", "/shares", &body)
            .map(|r| r.expires_at)
    }

    pub fn withdraw_invite(&self, client_id: &str, invitation_id: &str) -> Result<()> {
        self.delete::<serde_json::Value>(&format!("/clients/{client_id}/invites/{invitation_id}"))
            .map(|_| ())
    }

    pub fn set_user_active(&self, client_id: &str, identity_id: &str, active: bool) -> Result<()> {
        self.send_json::<serde_json::Value>(
            "PATCH",
            &format!("/clients/{client_id}/users/{identity_id}"),
            &serde_json::json!({ "active": active }),
        )
        .map(|_| ())
    }

    pub fn access_requests(&self) -> Result<AccessRequests> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct R {
            requests: Vec<AccessRequest>,
            #[serde(default)]
            request_check: Option<String>,
        }
        self.get::<R>("/access-requests", &[])
            .map(|r| AccessRequests {
                requests: r.requests,
                check_required: r.request_check.as_deref() == Some("required"),
            })
    }

    /// Approves a request for a client. `check`: how staff checked who is asking; left out, the
    /// portal refuses the approval when the firm requires one.
    pub fn approve(
        &self,
        request_id: &str,
        client_id: &str,
        role: &str,
        check: Option<&Check>,
    ) -> Result<()> {
        let mut body = serde_json::json!({ "clientId": client_id, "roleLabel": role });
        if let Some(c) = check {
            body["check"] = serde_json::json!(c);
        }
        self.send_json::<serde_json::Value>(
            "POST",
            &format!("/access-requests/{request_id}/approve"),
            &body,
        )
        .map(|_| ())
    }

    /// Approves a request for one or more clients, each with its role (Vibe-Portal P112; an
    /// older portal gives every client the first role). Answers the person's identity id.
    pub fn approve_for(
        &self,
        request_id: &str,
        clients: &[(String, String)],
        check: Option<&Check>,
    ) -> Result<String> {
        let ids: Vec<&String> = clients.iter().map(|(id, _)| id).collect();
        let roles: BTreeMap<&String, &String> = clients.iter().map(|(id, r)| (id, r)).collect();
        let first = clients.first().map(|(_, r)| r.as_str()).unwrap_or("");
        let mut body = serde_json::json!({ "clientIds": ids, "roleLabel": first, "roles": roles });
        if let Some(c) = check {
            body["check"] = serde_json::json!(c);
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct R {
            #[serde(default)]
            identity_id: String,
        }
        self.send_json::<R>(
            "POST",
            &format!("/access-requests/{request_id}/approve"),
            &body,
        )
        .map(|r| r.identity_id)
    }

    pub fn deny(&self, request_id: &str) -> Result<()> {
        self.post_empty::<serde_json::Value>(&format!("/access-requests/{request_id}/deny"))
            .map(|_| ())
    }

    // ---- files to clients ---------------------------------------------------------------

    fn files_where(&self, query: &[(&str, &str)]) -> Result<Vec<PortalFile>> {
        #[derive(Deserialize)]
        struct R {
            files: Vec<PortalFile>,
        }
        self.get::<R>("/files", query).map(|r| r.files)
    }

    pub fn files_for_client(&self, client_id: &str) -> Result<Vec<PortalFile>> {
        self.files_where(&[("clientId", client_id)])
    }

    /// The portal copy of a cabinet document for this client (QA 2026-11-26: a document moved
    /// to another client in the cabinet is not taken for this client's).
    pub fn file_for_client_doc(&self, client_id: &str, doc_id: &str) -> Result<Option<PortalFile>> {
        self.files_where(&[("clientId", client_id), ("vfDocumentId", doc_id)])
            .map(|f| {
                f.into_iter()
                    .find(|f| f.client_id == client_id && f.vf_document_id == doc_id)
            })
    }

    /// The portal copy of a cabinet document, if it is on the portal.
    pub fn file_for_doc(&self, doc_id: &str) -> Result<Option<PortalFile>> {
        self.files_where(&[("vfDocumentId", doc_id)])
            .map(|f| f.into_iter().next())
    }

    /// Every file on the portal (for the marks in Browse).
    pub fn all_files(&self) -> Result<Vec<PortalFile>> {
        self.files_where(&[("all", "1")])
    }

    /// Sends a cabinet file to the portal for `client_id` (not shown to anyone yet).
    pub fn add(
        &self,
        client_id: &str,
        doc_id: &str,
        name: &str,
        tax_year: Option<i32>,
        path: &Path,
    ) -> Result<Added> {
        let sha = file_sha256(path)?;
        let year = tax_year.map(|y| y.to_string());
        let mut r = self
            .auth(self.bulk.post(&self.url("/files")))
            .query("clientId", client_id)
            .query("vfDocumentId", doc_id)
            .query("sha256", &sha)
            .header(NAME_HEADER, &header_text(name))
            .header("Content-Type", "application/octet-stream");
        if let Some(y) = &year {
            r = r.query("taxYear", y);
        }
        let f = std::fs::File::open(path).map_err(|e| PortalError::Local(e.to_string()))?;
        let res = r.send(f);
        let mut res = res.map_err(|e| PortalError::Network(e.to_string()))?;
        let status = res.status().as_u16();
        let text = res
            .body_mut()
            .read_to_string()
            .map_err(|e| PortalError::Network(e.to_string()))?;
        let body: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        let file_id = body
            .get("fileId")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        match (status, file_id) {
            (201, Some(f)) => Ok(Added::New(f)),
            (200, Some(f)) => Ok(Added::Existing(f)),
            (409, Some(f))
                if body.get("error").and_then(|v| v.as_str()) == Some("changed_use_replace") =>
            {
                Ok(Added::Changed(f))
            }
            (409, None) if body.get("error").and_then(|v| v.as_str()) == Some("already_added") => {
                // Another PC added it at the same moment.
                match self.file_for_doc(doc_id)? {
                    Some(f) if f.sha256 == sha => Ok(Added::Existing(f.file_id)),
                    Some(f) => Ok(Added::Changed(f.file_id)),
                    None => Err(PortalError::Http {
                        status,
                        code: "already_added".into(),
                    }),
                }
            }
            _ => {
                let e: Option<ErrorBody> = serde_json::from_str(&text).ok();
                Err(PortalError::Http {
                    status,
                    code: e.and_then(|e| e.error).unwrap_or_else(|| "error".into()),
                })
            }
        }
    }

    /// "Update on portal": replaces the portal copy's content with the cabinet file's.
    pub fn replace(&self, file_id: &str, name: &str, path: &Path) -> Result<()> {
        let sha = file_sha256(path)?;
        let f = std::fs::File::open(path).map_err(|e| PortalError::Local(e.to_string()))?;
        let res = self
            .auth(
                self.bulk
                    .put(&self.url(&format!("/files/{file_id}/content"))),
            )
            .query("sha256", &sha)
            .header(NAME_HEADER, &header_text(name))
            .header("Content-Type", "application/octet-stream")
            .send(f);
        Self::read::<serde_json::Value>(res).map(|_| ())
    }

    /// Shows or hides a portal file for each person (D8).
    pub fn set_visibility(&self, file_id: &str, users: &BTreeMap<String, bool>) -> Result<()> {
        self.set_visibility_with(file_id, users, &BTreeMap::new())
    }

    /// [`Self::set_visibility`], also holding the file for invitations not accepted yet
    /// (`invites`: invitation id → held; Vibe-Portal P98). A resent invitation keeps them.
    pub fn set_visibility_with(
        &self,
        file_id: &str,
        users: &BTreeMap<String, bool>,
        invites: &BTreeMap<String, bool>,
    ) -> Result<()> {
        let mut body = serde_json::json!({ "users": users });
        if !invites.is_empty() {
            body["invites"] = serde_json::json!(invites);
        }
        self.send_json::<serde_json::Value>("PUT", &format!("/files/{file_id}/visibility"), &body)
            .map(|_| ())
    }

    /// Removes the file from the portal (the cabinet keeps the original).
    pub fn remove(&self, file_id: &str) -> Result<()> {
        self.delete::<serde_json::Value>(&format!("/files/{file_id}"))
            .map(|_| ())
    }

    // ---- files from clients -------------------------------------------------------------

    pub fn uploads(&self) -> Result<Vec<Upload>> {
        #[derive(Deserialize)]
        struct R {
            uploads: Vec<Upload>,
        }
        self.get::<R>("/uploads", &[]).map(|r| r.uploads)
    }

    /// Writes a client upload to `dest` (a new file) and checks it against the portal's size and
    /// sha256. Nothing is left at `dest` when it fails.
    pub fn download_upload(&self, u: &Upload, dest: &Path) -> Result<()> {
        let r = self.download_inner(u, dest);
        if r.is_err() {
            let _ = std::fs::remove_file(dest);
        }
        r
    }

    fn download_inner(&self, u: &Upload, dest: &Path) -> Result<()> {
        let res = self
            .auth(
                self.bulk
                    .get(&self.url(&format!("/uploads/{}/content", u.upload_id))),
            )
            .call();
        let mut res = res.map_err(|e| PortalError::Network(e.to_string()))?;
        let status = res.status().as_u16();
        if status != 200 {
            let text = res.body_mut().read_to_string().unwrap_or_default();
            return Err(http_error(status, &text));
        }
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dest)
            .map_err(|e| PortalError::Local(e.to_string()))?;
        let mut hasher = Sha256::new();
        // Never more than the portal said (a bad answer cannot fill the disk).
        let mut reader = res.body_mut().with_config().limit(u.size + 1).reader();
        let mut buf = vec![0u8; 256 * 1024];
        let mut written = 0u64;
        loop {
            let n = reader
                .read(&mut buf)
                .map_err(|e| PortalError::Network(e.to_string()))?;
            if n == 0 {
                break;
            }
            written += n as u64;
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n])
                .map_err(|e| PortalError::Local(e.to_string()))?;
        }
        out.sync_all()
            .map_err(|e| PortalError::Local(e.to_string()))?;
        let got: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if written != u.size || got != u.sha256 {
            return Err(PortalError::Local(format!(
                "{} arrived damaged (its checksum does not match); it will be fetched again.",
                u.name
            )));
        }
        Ok(())
    }

    /// Claims an upload. The portal says who has it when it was claimed already, so a claim
    /// whose answer was lost is still known as this PC's. Vibe Files reports filing
    /// (`tracksFiling`, Vibe-Portal P74): the portal keeps the upload until then. An older portal
    /// ignores it.
    pub fn mark_fetched(&self, upload_id: &str) -> Result<Claim> {
        let res = self
            .auth(
                self.quick
                    .post(&self.url(&format!("/uploads/{upload_id}/fetched"))),
            )
            .send_json(serde_json::json!({ "tracksFiling": true }));
        let mut res = res.map_err(|e| PortalError::Network(e.to_string()))?;
        let status = res.status().as_u16();
        let text = res.body_mut().read_to_string().unwrap_or_default();
        match status {
            200..=299 => Ok(Claim::Ours),
            409 | 404 => {
                let by = serde_json::from_str::<ErrorBody>(&text)
                    .ok()
                    .and_then(|b| b.fetched_by);
                Ok(if by.as_deref() == Some(self.actor_name().as_str()) {
                    Claim::Ours
                } else {
                    Claim::Other
                })
            }
            _ => Err(http_error(status, &text)),
        }
    }

    /// Uploads fetched and not filed or dismissed (Vibe-Portal P74). An older portal answers
    /// 404: [`NEEDS_P74`].
    pub fn unfiled(&self) -> Result<UnfiledList> {
        let l: UnfiledList = self.get("/uploads/unfiled", &[]).map_err(|e| {
            if e.status() == Some(404) {
                PortalError::Local(NEEDS_P74.into())
            } else {
                e
            }
        })?;
        if l.state != "unfiled" {
            return Err(PortalError::Local(NEEDS_P74.into()));
        }
        Ok(l)
    }

    /// Intake filed it: the portal's keep period starts (Vibe-Portal P74). Saying it again is
    /// fine.
    pub fn mark_filed(&self, upload_id: &str) -> Result<()> {
        self.post_empty::<serde_json::Value>(&format!("/uploads/{upload_id}/filed"))
            .map(|_| ())
    }

    /// Someone said it is not to be filed (junk, a duplicate): the portal's keep period starts.
    pub fn mark_dismissed(&self, upload_id: &str) -> Result<()> {
        self.post_empty::<serde_json::Value>(&format!("/uploads/{upload_id}/dismissed"))
            .map(|_| ())
    }

    /// Portal history after `since` (an event id), oldest first, at most `limit`.
    pub fn events(&self, since: u64, limit: u32) -> Result<Vec<Event>> {
        #[derive(Deserialize)]
        struct R {
            events: Vec<Event>,
        }
        let s = since.to_string();
        let l = limit.to_string();
        self.get::<R>("/events", &[("since", &s), ("limit", &l)])
            .map(|r| r.events)
    }
}

/// sha256 of a file, hex.
/// The portal reads a file's name from this header, not from the URL, so the name of a shared
/// file stays out of the web server's access log (Vibe-Portal P47).
const NAME_HEADER: &str = "X-Vibe-Name";

/// Percent-encodes text for a header, as JavaScript's `encodeURIComponent` would read it back:
/// every byte except ASCII letters, digits and `-_.~` becomes `%XX`.
fn header_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn file_sha256(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path).map_err(|e| PortalError::Local(e.to_string()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| PortalError::Local(e.to_string()))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_says_where_the_files_are() {
        let read = |json: &str| serde_json::from_str::<Status>(json).unwrap();
        let base = r#""version":"1","firmName":"F","portalUrl":"https://p""#;
        let old = read(&format!("{{{base}}}"));
        assert_eq!(old.storage, None);
        assert!(old.storage_words().contains("on the appliance"));
        let b2 = read(&format!(
            r#"{{{base},"storage":"b2","storageMoving":false}}"#
        ));
        assert!(b2.storage_words().contains("B2 bucket"));
        let moving = read(&format!(
            r#"{{{base},"storage":"b2","storageMoving":true}}"#
        ));
        assert!(moving.storage_words().contains("being moved"));
    }

    /// One answer from a portal on this PC: the request line it got, and the client to call it.
    fn one_answer(status: &str, body: &'static str) -> (Portal, std::thread::JoinHandle<String>) {
        use std::io::{BufRead, BufReader, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let status = status.to_string();
        let seen = std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut first = String::new();
            r.read_line(&mut first).unwrap();
            let mut auth = String::new();
            let mut len = 0usize;
            loop {
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                let lower = line.to_ascii_lowercase();
                if lower.starts_with("authorization:") {
                    auth = line.trim().to_string();
                }
                if let Some(n) = lower.strip_prefix("content-length:") {
                    len = n.trim().parse().unwrap_or(0);
                }
                if line == "\r\n" || line.is_empty() {
                    break;
                }
            }
            let mut sent = vec![0u8; len];
            std::io::Read::read_exact(&mut r, &mut sent).unwrap();
            let sent = String::from_utf8(sent).unwrap();
            let mut w = s;
            write!(
                w,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            if sent.is_empty() {
                format!("{} | {auth}", first.trim())
            } else {
                format!("{} | {auth} | {sent}", first.trim())
            }
        });
        let s = PortalSettings {
            version: 1,
            cabinet_id: "c".into(),
            url: format!("http://127.0.0.1:{port}"),
            key: "vpk_test".into(),
            fetch_minutes: 0,
            record: Default::default(),
            b2_list: false,
            todo_lists: false,
            balances: false,
            deliveries: false,
            approve_share: Default::default(),
        };
        (Portal::new(&s, "PC1", "kurt"), seen)
    }

    /// PA-103 (Vibe-Portal P109): to-do calls reach the right paths, with the right bodies.
    #[test]
    fn to_do_calls_use_the_firm_api() {
        use crate::todo::{ItemIn, ListChange, NewList};
        const LIST: &str = r#"{"list":{"id":"l1","clientId":"c1","title":"T","dueDate":null,
            "status":"closed","complete":false,"done":0,"total":1,"createdAt":"2026-11-09T15:00:00Z",
            "items":[]}}"#;
        let (p, seen) = one_answer("200 OK", LIST);
        let l = p
            .change_list(
                "l1",
                &ListChange {
                    status: Some("closed".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(l.status, "closed");
        let flat = |s: String| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
        assert_eq!(
            flat(seen.join().unwrap()),
            flat(r#"PATCH /api/firm/v1/request-lists/l1 HTTP/1.1 | authorization: Bearer vpk_test | {"status":"closed"}"#.into())
        );
        let (p, seen) = one_answer("201 Created", r#"{"lists":[]}"#);
        p.create_lists(&NewList {
            client_ids: vec!["c1".into()],
            title: "T".into(),
            note: String::new(),
            due_date: None,
            remind_every_days: 0,
            template_id: None,
            items: vec![ItemIn {
                prompt: "W-2".into(),
                ..ItemIn::blank()
            }],
            notify: true,
            assigned_staff_id: Some("s1".into()),
        })
        .unwrap();
        let sent = flat(seen.join().unwrap());
        assert!(
            sent.starts_with("POST/api/firm/v1/request-listsHTTP/1.1"),
            "{sent}"
        );
        assert!(sent.contains(r#""clientIds":["c1"]"#), "{sent}");
        assert!(sent.contains(r#""assignedStaffId":"s1""#), "{sent}");
        assert!(!sent.contains("templateId"), "{sent}");
        // A list with answers is not deleted: the portal's reason in words.
        let (p, seen) = one_answer("409 Conflict", r#"{"error":"list_has_answers"}"#);
        let err = p.delete_list("l1").unwrap_err();
        assert!(
            seen.join()
                .unwrap()
                .starts_with("DELETE /api/firm/v1/request-lists/l1 ")
        );
        assert!(err.message().contains("cancel it instead"));
        // Turned off on the portal.
        let (p, _) = one_answer("404 Not Found", r#"{"error":"requests_off"}"#);
        assert!(
            p.todo_lists("c1")
                .unwrap_err()
                .message()
                .contains("turned off")
        );
    }

    /// Vibe-Portal P63: people already on the portal, searched by what was typed.
    #[test]
    fn the_invitation_words_are_the_portals() {
        let (subject, body) = invitation_words(
            "Sample CPA Firm",
            "Sample, John",
            "https://p.example.test/invite#t=x",
        );
        assert_eq!(subject, "Sample CPA Firm invites you to its client portal");
        assert!(body.ends_with("(within 7 days): https://p.example.test/invite#t=x"));
        // Short enough: the text is the email's words as they are.
        assert_eq!(text_message(&body), body);
        // A long client name: the words are cut, the link kept whole.
        let long = invitation_words(
            "Firm",
            &"Long Name ".repeat(40),
            "https://p.example.test/invite#t=x",
        )
        .1;
        let t = text_message(&long);
        assert!(t.chars().count() <= 300, "{}", t.len());
        assert!(t.ends_with("... https://p.example.test/invite#t=x"));
    }

    #[test]
    fn people_are_searched_on_the_portal() {
        let (p, seen) = one_answer(
            "200 OK",
            r#"{"people":[{"identityId":"i1","name":"","email":"a@example.test","phone":null,
                "clients":[{"id":"AB-1","name":"Smith, John"}]}]}"#,
        );
        let people = p.people("  john smith ").unwrap();
        assert_eq!(
            seen.join().unwrap(),
            "GET /api/firm/v1/people?q=john%20smith HTTP/1.1 | authorization: Bearer vpk_test"
        );
        assert_eq!(people.len(), 1);
        assert_eq!(people[0].label(), "a@example.test");
        assert!(people[0].sees("ab-1"));
        assert!(!people[0].sees("other"));
        // A portal from before P63.
        let (p, _) = one_answer("404 Not Found", r#"{"error":"not_found"}"#);
        assert_eq!(p.people("john").unwrap_err().status(), Some(404));
    }

    /// Vibe Portal Addendum 01: the requests carry what the ID's barcode said and the flags,
    /// and say whether the firm asks for a check; an approval says how it was checked.
    #[test]
    fn requests_carry_the_id_scan_and_approvals_the_check() {
        let (p, _) = one_answer(
            "200 OK",
            r#"{"requestCheck":"required","requests":[{"id":"r1","name":"Alexander Sample",
                "email":"alex@example.test","phone":null,"confirmedContact":"alex@example.test",
                "clientAs":"Sample","note":"","createdAt":"2026-11-14T10:00:00Z",
                "idPhoto":{"contentType":"image/jpeg","size":10},"idBack":null,
                "idScan":{"familyName":"SAMPLE","firstName":"ALEXANDER","middleName":"JAMES",
                  "dob":"1990-01-06","expires":"2027-01-06","issued":"2022-08-24",
                  "address":{"street":"2570 24TH STREET","city":"SACRAMENTO","state":"CA",
                    "zip":"95818"},"issuer":"CA, USA"},
                "idFlags":["expired"]}]}"#,
        );
        let list = p.access_requests().unwrap();
        assert!(list.check_required);
        let r = &list.requests[0];
        let scan = r.id_scan.as_ref().unwrap();
        assert_eq!(scan.full_name(), "ALEXANDER JAMES SAMPLE");
        assert_eq!(
            scan.address_line(),
            "2570 24TH STREET, SACRAMENTO, CA, 95818"
        );
        assert_eq!(scan.dob.as_deref(), Some("1990-01-06"));
        assert_eq!(r.id_flags, vec!["expired".to_string()]);

        // A portal from before the addendum: no check asked, no scan, no flags.
        let (p, _) = one_answer(
            "200 OK",
            r#"{"requests":[{"id":"r1","name":"A","email":null,"phone":null,
                "confirmedContact":null,"clientAs":"","note":"","createdAt":"x"}]}"#,
        );
        let old = p.access_requests().unwrap();
        assert!(!old.check_required);
        assert_eq!(old.requests[0].id_scan, None);
        assert!(old.requests[0].id_flags.is_empty());

        let (p, seen) = one_answer("200 OK", r#"{"ok":true}"#);
        p.approve(
            "r1",
            "AB-1",
            "Spouse",
            Some(&Check::record_match(&["email", "dob"])),
        )
        .unwrap();
        let seen = seen.join().unwrap();
        assert!(
            seen.starts_with("POST /api/firm/v1/access-requests/r1/approve HTTP/1.1 | "),
            "{seen}"
        );
        let body: serde_json::Value =
            serde_json::from_str(seen.rsplit(" | ").next().unwrap()).unwrap();
        assert_eq!(
            body,
            serde_json::json!({
                "clientId": "AB-1", "roleLabel": "Spouse",
                "check": { "method": "record_match", "matched": ["email", "dob"], "note": "" }
            })
        );
        // With no check, the body is what an older Vibe Files sent.
        let (p, seen) = one_answer("200 OK", r#"{"ok":true}"#);
        p.approve("r1", "AB-1", "", None).unwrap();
        let seen = seen.join().unwrap();
        let body: serde_json::Value =
            serde_json::from_str(seen.rsplit(" | ").next().unwrap()).unwrap();
        assert_eq!(
            body,
            serde_json::json!({ "clientId": "AB-1", "roleLabel": "" })
        );
        // The portal's refusal, in words.
        let (p, _) = one_answer("400 Bad Request", r#"{"error":"check_required"}"#);
        assert_eq!(
            p.approve("r1", "AB-1", "", None).unwrap_err().message(),
            "The portal asks how you checked who this is before approving."
        );
    }

    /// Vibe-Portal P68: a secure link to someone outside, for files already on the portal.
    #[test]
    fn a_secure_link_is_sent_through_the_portal() {
        let (p, seen) = one_answer(
            "201 Created",
            r#"{"shareId":"s1","expiresAt":"2026-11-18T15:00:00.000Z"}"#,
        );
        let l = OutsideLink {
            file_ids: vec!["f1".into()],
            email: " pat@first-bank.test ".into(),
            name: "Pat Roe".into(),
            organization: "First Bank".into(),
            message: "".into(),
            can_download: true,
            watermark: true,
            days: 7,
        };
        assert_eq!(
            p.send_outside("AB-1", &l).unwrap(),
            "2026-11-18T15:00:00.000Z"
        );
        let seen = seen.join().unwrap();
        assert!(
            seen.starts_with(
                "POST /api/firm/v1/shares HTTP/1.1 | authorization: Bearer vpk_test | "
            ),
            "{seen}"
        );
        let body: serde_json::Value =
            serde_json::from_str(seen.rsplit(" | ").next().unwrap()).unwrap();
        assert_eq!(
            body,
            serde_json::json!({
                "clientId": "AB-1", "fileIds": ["f1"], "email": "pat@first-bank.test",
                "name": "Pat Roe", "organization": "First Bank", "message": "",
                "canDownload": true, "watermark": true, "days": 7
            })
        );
        // A portal from before P68.
        let (p, _) = one_answer("404 Not Found", r#"{"error":"not_found"}"#);
        assert_eq!(p.send_outside("AB-1", &l).unwrap_err().status(), Some(404));
        let (p, _) = one_answer(
            "429 Too Many Requests",
            r#"{"error":"recipient_has_too_many"}"#,
        );
        assert!(
            p.send_outside("AB-1", &l)
                .unwrap_err()
                .message()
                .contains("5 working")
        );
    }

    /// Vibe-Portal P74: the claim says Vibe Files reports filing.
    #[test]
    fn a_claim_says_filing_is_reported() {
        let (p, seen) = one_answer("200 OK", r#"{"ok":true}"#);
        assert_eq!(p.mark_fetched("u1").unwrap(), Claim::Ours);
        let seen = seen.join().unwrap();
        assert!(
            seen.starts_with("POST /api/firm/v1/uploads/u1/fetched HTTP/1.1 | "),
            "{seen}"
        );
        let body: serde_json::Value =
            serde_json::from_str(seen.rsplit(" | ").next().unwrap()).unwrap();
        assert_eq!(body, serde_json::json!({ "tracksFiling": true }));
    }

    /// Vibe-Portal P74: the list of uploads not filed; an older portal is told to update.
    #[test]
    fn uploads_not_filed_are_listed() {
        let (p, seen) = one_answer(
            "200 OK",
            r#"{"state":"unfiled","keepDays":30,"capDays":180,"uploads":[{"uploadId":"u1",
                "clientId":"c1","clientCode":"1001","clientName":"Smith, John","name":"a.pdf",
                "originalName":null,"size":3,"sha256":"ab","note":"","batchId":null,
                "uploadedAt":"2026-11-10T10:00:00.000Z","fetchedAt":"2026-11-10T10:05:00.000Z",
                "fetchedBy":"PC1\\kurt","from":{"identityId":"i1","name":"Mary","email":null,
                "phone":null}}]}"#,
        );
        let l = p.unfiled().unwrap();
        assert!(
            seen.join()
                .unwrap()
                .starts_with("GET /api/firm/v1/uploads/unfiled ")
        );
        assert_eq!((l.keep_days, l.cap_days), (30, 180));
        assert_eq!(l.uploads[0].upload.upload_id, "u1");
        assert_eq!(l.uploads[0].fetched_by.as_deref(), Some("PC1\\kurt"));
        // A portal from before P74.
        let (p, _) = one_answer("404 Not Found", r#"{"error":"not_found"}"#);
        assert_eq!(p.unfiled().unwrap_err().message(), NEEDS_P74);
        // One that ignored the path's meaning and sent something else.
        let (p, _) = one_answer(
            "200 OK",
            r#"{"state":"x","keepDays":1,"capDays":1,"uploads":[]}"#,
        );
        assert_eq!(p.unfiled().unwrap_err().message(), NEEDS_P74);
        let (p, seen) = one_answer("200 OK", r#"{"ok":true}"#);
        p.mark_filed("u1").unwrap();
        assert!(
            seen.join()
                .unwrap()
                .starts_with("POST /api/firm/v1/uploads/u1/filed ")
        );
        let (p, seen) = one_answer("200 OK", r#"{"ok":true,"already":"filed"}"#);
        p.mark_dismissed("u1").unwrap();
        assert!(
            seen.join()
                .unwrap()
                .starts_with("POST /api/firm/v1/uploads/u1/dismissed ")
        );
    }

    #[test]
    fn names_are_percent_encoded_for_their_header() {
        assert_eq!(header_text("Return 2025.pdf"), "Return%202025.pdf");
        assert_eq!(header_text("Révisé\r\n.pdf"), "R%C3%A9vis%C3%A9%0D%0A.pdf");
    }

    #[test]
    fn header_values_are_made_safe() {
        assert_eq!(clean("THGR18C-XA003"), "THGR18C-XA003");
        assert_eq!(clean("évé\r\nX-Evil: 1"), "vX-Evil: 1");
        assert_eq!(clean(&"a".repeat(100)).len(), 64);
    }

    #[test]
    fn errors_read_in_plain_words() {
        let e = PortalError::Http {
            status: 401,
            code: "bad_firm_key".into(),
        };
        assert!(e.message().contains("portal key"));
        assert_eq!(e.code(), Some("bad_firm_key"));
    }

    #[test]
    fn file_hash_matches() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.txt");
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(file_sha256(&p).unwrap(), sha256_hex(b"abc"));
    }
}
