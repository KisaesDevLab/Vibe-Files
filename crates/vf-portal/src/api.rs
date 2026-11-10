//! Calls to the portal's firm API (`/api/firm/v1`), authorised by the firm key. Every call names
//! this PC and Windows user, which the portal keeps in its history (Vibe Portal P15).

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use serde::de::DeserializeOwned;
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
            _ => self.to_string(),
        }
    }
}

pub type Result<T> = std::result::Result<T, PortalError>;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub version: String,
    pub firm_name: String,
    pub portal_url: String,
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

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UploadFrom {
    pub identity_id: String,
    pub name: String,
    pub email: Option<String>,
    pub phone: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
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
    #[serde(rename = "fileId")]
    file_id: Option<String>,
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

fn agent(global: Option<Duration>) -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(120)))
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
        Self {
            base: format!("{}/api/firm/v1", s.url.trim_end_matches('/')),
            key: zeroize::Zeroizing::new(s.key.clone()),
            pc: clean(pc),
            user: clean(user),
            quick: agent(Some(Duration::from_secs(30))),
            bulk: agent(None),
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
            .query("name", name)
            .query("sha256", &sha)
            .header("Content-Type", "application/octet-stream");
        if let Some(y) = &year {
            r = r.query("taxYear", y);
        }
        let f = std::fs::File::open(path).map_err(|e| PortalError::Local(e.to_string()))?;
        let res = r.send(ureq::SendBody::from_owned_reader(f));
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
            _ => {
                let e: Option<ErrorBody> = serde_json::from_str(&text).ok();
                let _ = e.as_ref().and_then(|e| e.file_id.as_ref());
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
            .query("name", name)
            .header("Content-Type", "application/octet-stream")
            .send(ureq::SendBody::from_owned_reader(f));
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

    /// Writes a client upload to `dest` and checks it against the portal's sha256.
    pub fn download_upload(&self, u: &Upload, dest: &Path) -> Result<()> {
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
        let mut out = std::fs::File::create(dest).map_err(|e| PortalError::Local(e.to_string()))?;
        let mut hasher = Sha256::new();
        let mut reader = res.body_mut().with_config().limit(u64::MAX).reader();
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let n = reader
                .read(&mut buf)
                .map_err(|e| PortalError::Network(e.to_string()))?;
            if n == 0 {
                break;
            }
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
        if got != u.sha256 {
            drop(out);
            let _ = std::fs::remove_file(dest);
            return Err(PortalError::Local(format!(
                "{} arrived damaged (its checksum does not match); it will be fetched again.",
                u.name
            )));
        }
        Ok(())
    }

    /// Marks an upload fetched. `Ok(false)`: another PC fetched it first.
    pub fn mark_fetched(&self, upload_id: &str) -> Result<bool> {
        match self.post_empty::<serde_json::Value>(&format!("/uploads/{upload_id}/fetched")) {
            Ok(_) => Ok(true),
            Err(e) if e.status() == Some(409) => Ok(false),
            Err(e) => Err(e),
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
