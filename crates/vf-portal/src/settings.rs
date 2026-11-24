//! Where the portal address and firm key are kept: `_Vibe/keys/portal.json` in the cabinet,
//! beside the shared sync settings. `_Vibe/keys` is never pushed to B2. Anyone who can read the
//! cabinet share can use the portal key, as they can already read the files (same rule as
//! `sync-shared.json`, Q-43).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::record::RecordColumns;

pub const FILE: &str = "portal.json";
/// The firm key's prefix (Vibe Portal's `vpk_`).
pub const KEY_PREFIX: &str = "vpk_";
/// Minutes between fetches of client uploads, unless the firm sets another.
pub const DEFAULT_FETCH_MINUTES: u32 = 5;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortalSettings {
    pub version: u32,
    /// The cabinet these settings belong to (a copied `_Vibe` folder does not carry them over).
    pub cabinet_id: String,
    /// The portal address clients use, e.g. `https://files.examplecpa.com`, no trailing slash.
    pub url: String,
    /// The firm key (`vpk_…`).
    pub key: String,
    /// Fetch client uploads every this many minutes (0: only by hand).
    #[serde(default = "default_fetch")]
    pub fetch_minutes: u32,
    /// Which client-list columns hold the client's email, mobile and date of birth, for
    /// checking access requests (Vibe Portal Addendum 01, layer B). None chosen: no request
    /// can match the record.
    #[serde(default)]
    pub record: RecordColumns,
    /// Send the portal the list of documents in B2, by client, for its staff page (PA-80):
    /// names, sizes and tax years, never content. Off unless the firm ticks it.
    #[serde(default)]
    pub b2_list: bool,
    /// To-do lists in Vibe Files for every PC of the cabinet (PA-103, owner 2026-11-23): the
    /// client panel's To-do lists… and Admin › Portal › To-do templates. Off unless the firm
    /// ticks it; the portal has its own switch too.
    #[serde(default)]
    pub todo_lists: bool,
    /// What Approve offers to share with someone given access (PA-106, owner 2026-11-24).
    #[serde(default, skip_serializing_if = "ApproveShare::is_empty")]
    pub approve_share: ApproveShare,
}

/// The documents Admin › Portal requests offers to share on Approve: those of these doc types
/// from the client's most recent `years` tax years (and those with no tax year), ticked; older
/// ones of the same doc types listed unticked. No doc types: nothing is offered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApproveShare {
    /// Doc type ids.
    #[serde(default)]
    pub doc_types: Vec<String>,
    #[serde(default = "one")]
    pub years: u16,
}

fn one() -> u16 {
    1
}

impl Default for ApproveShare {
    fn default() -> Self {
        Self {
            doc_types: Vec::new(),
            years: 1,
        }
    }
}

impl ApproveShare {
    pub fn is_empty(&self) -> bool {
        self.doc_types.is_empty() && self.years == 1
    }
}

fn default_fetch() -> u32 {
    DEFAULT_FETCH_MINUTES
}

/// Never prints the key (QA 2026-11-09).
impl std::fmt::Debug for PortalSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PortalSettings")
            .field("cabinet_id", &self.cabinet_id)
            .field("url", &self.url)
            .field("key", &"vpk_…")
            .field("fetch_minutes", &self.fetch_minutes)
            .finish()
    }
}

impl Drop for PortalSettings {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

pub fn file(root: &Path) -> PathBuf {
    root.join(vf_fs::cabinet::VIBE_DIR)
        .join(vf_fs::cabinet::KEYS_DIR)
        .join(FILE)
}

/// The address as kept: trimmed, `https://` added when no scheme was typed, no trailing slash.
/// `None` when it is not an http(s) address.
pub fn normalize_url(raw: &str) -> Option<String> {
    let t = raw.trim().trim_end_matches('/');
    let with = if t.contains("://") {
        t.to_string()
    } else {
        format!("https://{t}")
    };
    let rest = with
        .strip_prefix("https://")
        .or_else(|| with.strip_prefix("http://"))?;
    let host = rest.split('/').next().unwrap_or("");
    // name[:port]: a name of letters, digits, dots and dashes; a port of digits.
    let (name, port) = match host.rsplit_once(':') {
        Some((n, p)) => (n, Some(p)),
        None => (host, None),
    };
    let ok = name.chars().any(|c| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
        && port
            .is_none_or(|p| !p.is_empty() && p.len() <= 5 && p.chars().all(|c| c.is_ascii_digit()));
    ok.then_some(with)
}

impl PortalSettings {
    /// Checks and tidies what was typed in Admin › Portal.
    pub fn new(cabinet_id: &str, url: &str, key: &str, fetch_minutes: u32) -> Result<Self, String> {
        let url = normalize_url(url)
            .ok_or_else(|| "The portal address must start with https://.".to_string())?;
        // The key and client files would cross the network in clear (QA 2026-11-09).
        if url.starts_with("http://") && !crate::api::is_loopback(&url) {
            return Err(
                "The portal address must start with https:// (http:// only on this PC).".into(),
            );
        }
        let key = key.trim();
        if !key.starts_with(KEY_PREFIX) || key.len() < KEY_PREFIX.len() + 20 {
            return Err(format!(
                "The portal key starts with {KEY_PREFIX}: paste the whole key the portal showed."
            ));
        }
        Ok(Self {
            version: 1,
            cabinet_id: cabinet_id.into(),
            url,
            key: key.into(),
            fetch_minutes: fetch_minutes.min(24 * 60),
            record: RecordColumns::default(),
            b2_list: false,
            todo_lists: false,
            approve_share: Default::default(),
        })
    }

    /// The same settings with the list of documents in B2 sent to the portal, or not.
    pub fn with_b2_list(mut self, on: bool) -> Self {
        self.b2_list = on;
        self
    }

    /// The same settings with what Approve offers to share (PA-106).
    pub fn with_approve_share(mut self, share: ApproveShare) -> Self {
        self.approve_share = share;
        self
    }

    /// The same settings with to-do lists on or off for every PC (PA-103).
    pub fn with_todo_lists(mut self, on: bool) -> Self {
        self.todo_lists = on;
        self
    }

    /// The same settings with the client-record columns chosen in Admin › Portal.
    pub fn with_record(mut self, record: RecordColumns) -> Self {
        self.record = record;
        self
    }
}

/// The settings for this cabinet, when the firm has set them.
pub fn load(root: &Path, cabinet_id: &str) -> Option<PortalSettings> {
    let bytes = zeroize::Zeroizing::new(std::fs::read(file(root)).ok()?);
    let s: PortalSettings = serde_json::from_slice(&bytes).ok()?;
    (s.version == 1 && s.cabinet_id == cabinet_id && !s.url.is_empty() && !s.key.is_empty())
        .then_some(s)
}

/// The settings file is there and could not be read or understood just now (the share for a
/// moment, another PC saving it). [`load`] gives `None` then, as it does for "not set": who
/// acts on "not set" (taking the lists of documents off the portal) asks this first.
pub fn unreadable(root: &Path) -> bool {
    match std::fs::read(file(root)) {
        Ok(b) => {
            let b = zeroize::Zeroizing::new(b);
            serde_json::from_slice::<PortalSettings>(&b).is_err()
        }
        Err(e) => !vf_fs::cabinet::is_absent(&file(root), &e),
    }
}

pub fn save(root: &Path, s: &PortalSettings) -> Result<(), String> {
    let p = file(root);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let bytes = zeroize::Zeroizing::new(serde_json::to_vec_pretty(s).expect("serializes"));
    vf_fs::cabinet::write_replace(&p, &bytes)
}

/// Stops using the portal on every PC (the portal keeps its files; Vibe Files stops calling it).
pub fn clear(root: &Path) -> Result<(), String> {
    match std::fs::remove_file(file(root)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "vpk_abcdefghijklmnopqrstuvwxyz0123456789";

    #[test]
    fn addresses_are_tidied_and_checked() {
        assert_eq!(
            normalize_url(" files.examplecpa.com/ ").as_deref(),
            Some("https://files.examplecpa.com")
        );
        assert_eq!(
            normalize_url("http://192.168.1.20:3101").as_deref(),
            Some("http://192.168.1.20:3101")
        );
        assert_eq!(normalize_url("ftp://x"), None);
        assert_eq!(normalize_url("https://"), None);
        assert_eq!(normalize_url("https://bad host"), None);
    }

    #[test]
    fn plain_http_only_on_this_pc_and_the_key_never_printed() {
        assert!(PortalSettings::new("c", "http://files.example.test", KEY, 5).is_err());
        assert!(PortalSettings::new("c", "http://localhost:3101", KEY, 5).is_ok());
        assert!(PortalSettings::new("c", "http://127.0.0.1:3101", KEY, 5).is_ok());
        let s = PortalSettings::new("c", "files.example.test", KEY, 5).unwrap();
        assert!(!format!("{s:?}").contains(&KEY[4..]));
    }

    #[test]
    fn a_key_must_look_like_a_portal_key() {
        assert!(PortalSettings::new("c", "files.example.test", "secret", 5).is_err());
        let s = PortalSettings::new("c", "files.example.test", &format!(" {KEY} "), 5).unwrap();
        assert_eq!(s.key, KEY);
    }

    #[test]
    fn the_record_columns_are_kept_and_older_files_have_none() {
        let t = tempfile::tempdir().unwrap();
        let s = PortalSettings::new("cab-1", "https://files.example.test", KEY, 5)
            .unwrap()
            .with_record(RecordColumns {
                email: Some("Email".into()),
                mobile: None,
                dob: Some("Date_of_Birth".into()),
                dates: crate::record::DateOrder::DayFirst,
            });
        save(t.path(), &s).unwrap();
        assert_eq!(load(t.path(), "cab-1"), Some(s));
        // A file written before the columns existed.
        std::fs::write(
            file(t.path()),
            format!(
                r#"{{"version":1,"cabinet_id":"cab-1","url":"https://files.example.test","key":"{KEY}","fetch_minutes":5}}"#
            ),
        )
        .unwrap();
        let old = load(t.path(), "cab-1").unwrap();
        assert!(!old.record.any());
        // Nor the list of documents in B2: off until ticked.
        assert!(!old.b2_list);
        let on = old.with_b2_list(true);
        save(t.path(), &on).unwrap();
        assert!(load(t.path(), "cab-1").unwrap().b2_list);
    }

    #[test]
    fn saved_settings_belong_to_their_cabinet() {
        let t = tempfile::tempdir().unwrap();
        let s = PortalSettings::new("cab-1", "https://files.example.test", KEY, 5).unwrap();
        save(t.path(), &s).unwrap();
        assert_eq!(load(t.path(), "cab-1"), Some(s));
        assert_eq!(load(t.path(), "cab-2"), None);
        clear(t.path()).unwrap();
        assert_eq!(load(t.path(), "cab-1"), None);
    }
}
