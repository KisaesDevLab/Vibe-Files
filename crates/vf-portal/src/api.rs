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
            Some("contact_required") => "Enter an email address or a mobile number.".into(),
            Some("too_large") => "The file is larger than the portal allows.".into(),
            Some("not_a_user_of_this_client") => {
                "Someone chosen no longer has access to this client.".into()
            }
            Some("already_fetched") => "Another PC already fetched it.".into(),
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
}

impl Status {
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
}

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
            return Err(PortalError::Local(
                "The portal address redirects elsewhere: use the address it redirects to (https://…)."
                    .into(),
            ));
        }
        let text = res
            .body_mut()
            .with_config()
            .limit(JSON_LIMIT)
            .read_to_string()
            .map_err(|e| PortalError::Network(e.to_string()))?;
        if !(200..300).contains(&status) {
            let code = serde_json::from_str::<ErrorBody>(&text)
                .ok()
                .and_then(|b| b.error)
                .unwrap_or_else(|| "error".into());
            return Err(PortalError::Http { status, code });
        }
        serde_json::from_str(&text).map_err(|e| PortalError::Format(e.to_string()))
    }

    fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, &str)]) -> Result<T> {
        let mut r = self.auth(self.quick.get(&self.url(path)));
        for (k, v) in query {
            r = r.query(k, v);
        }
        Self::read(r.call())
    }

    fn send_json<T: DeserializeOwned>(
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

    fn post_empty<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        Self::read(self.auth(self.quick.post(&self.url(path))).send_empty())
    }

    fn delete<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
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

    pub fn access_requests(&self) -> Result<Vec<AccessRequest>> {
        #[derive(Deserialize)]
        struct R {
            requests: Vec<AccessRequest>,
        }
        self.get::<R>("/access-requests", &[]).map(|r| r.requests)
    }

    pub fn approve(&self, request_id: &str, client_id: &str, role: &str) -> Result<()> {
        self.send_json::<serde_json::Value>(
            "POST",
            &format!("/access-requests/{request_id}/approve"),
            &serde_json::json!({ "clientId": client_id, "roleLabel": role }),
        )
        .map(|_| ())
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
        self.send_json::<serde_json::Value>(
            "PUT",
            &format!("/files/{file_id}/visibility"),
            &serde_json::json!({ "users": users }),
        )
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
            let code = serde_json::from_str::<ErrorBody>(&text)
                .ok()
                .and_then(|b| b.error)
                .unwrap_or_else(|| "error".into());
            return Err(PortalError::Http { status, code });
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
    /// whose answer was lost is still known as this PC's.
    pub fn mark_fetched(&self, upload_id: &str) -> Result<Claim> {
        let res = self
            .auth(
                self.quick
                    .post(&self.url(&format!("/uploads/{upload_id}/fetched"))),
            )
            .send_empty();
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
            _ => {
                let code = serde_json::from_str::<ErrorBody>(&text)
                    .ok()
                    .and_then(|b| b.error)
                    .unwrap_or_else(|| "error".into());
                Err(PortalError::Http { status, code })
            }
        }
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
        };
        (Portal::new(&s, "PC1", "kurt"), seen)
    }

    /// Vibe-Portal P63: people already on the portal, searched by what was typed.
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
