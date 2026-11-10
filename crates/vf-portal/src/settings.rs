//! Where the portal address and firm key are kept: `_Vibe/keys/portal.json` in the cabinet,
//! beside the shared sync settings. `_Vibe/keys` is never pushed to B2. Anyone who can read the
//! cabinet share can use the portal key, as they can already read the files (same rule as
//! `sync-shared.json`, Q-43).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

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
        })
    }
}

/// The settings for this cabinet, when the firm has set them.
pub fn load(root: &Path, cabinet_id: &str) -> Option<PortalSettings> {
    let bytes = zeroize::Zeroizing::new(std::fs::read(file(root)).ok()?);
    let s: PortalSettings = serde_json::from_slice(&bytes).ok()?;
    (s.version == 1 && s.cabinet_id == cabinet_id && !s.url.is_empty() && !s.key.is_empty())
        .then_some(s)
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
