//! Object keys and manifest shapes: docs/BUCKET_LAYOUT.md, layout v1.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use vf_security::keys::WrappedKey;

pub const LAYOUT_VER: u32 = 1;
pub const CABINET: &str = "cabinet.json";
pub const HEAD: &str = "manifest/HEAD";

/// A path a manifest names (relative, `/`-separated): no root, drive, `..`, `.`, empty part,
/// backslash or character Windows forbids. Anything else is refused before a byte is written
/// (a corrupted or malicious manifest must not write outside the cabinet).
pub fn safe_rel(p: &str) -> bool {
    !p.is_empty()
        && p.len() < 1024
        && !p.starts_with('/')
        && !p.contains('\\')
        && !p
            .chars()
            .any(|c| (c as u32) < 32 || r#"<>:"|?*"#.contains(c))
        && p.split('/').all(|seg| {
            !seg.is_empty()
                && seg != "."
                && seg != ".."
                && !seg.ends_with(' ')
                && !seg.ends_with('.')
        })
}

/// A SHA-256 or object name: 64 lowercase hex characters.
pub fn safe_hex(h: &str) -> bool {
    h.len() == 64
        && h.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The cabinet master files a segment may carry (`parsers/<name>.toml` included).
pub fn master_file_allowed(rel: &str) -> bool {
    MASTER_FILES.contains(&rel)
        || rel
            .strip_prefix("parsers/")
            .is_some_and(|n| n.ends_with(".toml") && safe_rel(n) && !n.contains('/'))
}

fn check_doc(d: &DocEntry) -> Result<(), String> {
    if !safe_rel(&d.rel_path) {
        return Err("a document path".into());
    }
    if !safe_hex(&d.hash) || !safe_hex(&d.object) {
        return Err("a document hash".into());
    }
    if d.versions
        .iter()
        .any(|v| !safe_hex(&v.hash) || !safe_hex(&v.object))
    {
        return Err("a version hash".into());
    }
    Ok(())
}

impl ClientSegment {
    /// Refuses a segment naming anything outside its folder.
    pub fn check(&self) -> Result<(), String> {
        if !safe_rel(&self.folder_rel) || self.folder_rel.contains('/') {
            return Err("the client folder".into());
        }
        if self.empty_dirs.iter().any(|d| !safe_rel(d)) {
            return Err("a template folder".into());
        }
        self.documents.iter().try_for_each(check_doc)
    }
}

impl CabinetSegment {
    pub fn check(&self) -> Result<(), String> {
        if let Some(k) = self.files.keys().find(|k| !master_file_allowed(k)) {
            return Err(format!(
                "an unexpected master file ({} characters)",
                k.len()
            ));
        }
        self.documents.iter().try_for_each(check_doc)
    }
}

/// An audit month key: `yyyy-mm`.
pub fn safe_month(m: &str) -> bool {
    // `yyyy-mm`, or `yyyy-mm.<device>` (one mirror file per month and device).
    let (month, device) = m.split_once('.').unwrap_or((m, ""));
    let b = month.as_bytes();
    b.len() == 7
        && b[4] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || c.is_ascii_digit())
        && device.len() <= 64
        && device
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

pub fn object_key(name: &str) -> String {
    format!("objects/{}/{name}", name.get(..2).unwrap_or("00"))
}

pub fn segment_key(client_id: &str, rev: u64) -> String {
    format!("manifest/clients/{client_id}/{rev}")
}

pub fn cabinet_segment_key(rev: u64) -> String {
    format!("manifest/cabinet/{rev}")
}

/// A segment written by one push: `<rev>-<writer>` (two pushes racing never write the same key).
pub fn segment_key_by(client_id: &str, rev: u64, writer: &str) -> String {
    format!("manifest/clients/{client_id}/{rev}-{writer}")
}

pub fn cabinet_segment_key_by(rev: u64, writer: &str) -> String {
    format!("manifest/cabinet/{rev}-{writer}")
}

pub fn device_key(device_id: &str) -> String {
    format!("devices/{device_id}")
}

pub fn audit_key(month: &str) -> String {
    format!("audit/{month}")
}

/// `cabinet.json` in the bucket: not encrypted, no secrets in the clear. `wrapped` is the firm
/// sync key wrapped by the sync passphrase (Argon2id + ChaCha20-Poly1305, bound to the cabinet
/// id), so a bare PC restores from the bucket and the passphrase alone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BucketCabinet {
    pub layout_ver: u32,
    pub cabinet_id: String,
    pub firm_id: String,
    pub created: String,
    pub key_ids: Vec<String>,
    /// key id → wrapped key.
    pub wrapped: BTreeMap<String, WrappedKey>,
    /// B2 Object Lock retention the firm chose for this bucket (days), for display; the lock
    /// itself is a bucket setting made in B2.
    #[serde(default)]
    pub object_lock_days: Option<u32>,
}

/// `manifest/HEAD`: the commit point. Written last on every push.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Head {
    pub rev: u64,
    pub written_at: String,
    pub device_id: String,
    #[serde(default)]
    pub device_name: String,
    /// Random per write: read back to check this push won (B2 has no conditional PUT).
    #[serde(default)]
    pub writer: String,
    /// client_id → segment rev.
    pub segments: BTreeMap<String, u64>,
    pub cabinet_segment_rev: u64,
    /// The push that wrote each client's current segment (its key's suffix).
    #[serde(default)]
    pub segment_writers: BTreeMap<String, String>,
    #[serde(default)]
    pub cabinet_writer: String,
}

impl Head {
    /// The key of `client`'s current segment (`rev` = `segments[client]`).
    pub fn client_key(&self, client: &str, rev: u64) -> String {
        match self.segment_writers.get(client) {
            Some(w) if !w.is_empty() => segment_key_by(client, rev, w),
            _ => segment_key(client, rev),
        }
    }

    pub fn cabinet_key(&self) -> String {
        if self.cabinet_writer.is_empty() {
            cabinet_segment_key(self.cabinet_segment_rev)
        } else {
            cabinet_segment_key_by(self.cabinet_segment_rev, &self.cabinet_writer)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VersionEntry {
    pub hash: String,
    pub object: String,
    pub size: i64,
    pub created: String,
    pub created_by: Option<String>,
    pub device_id: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocEntry {
    pub doc_id: String,
    /// Relative to the segment's folder (cabinet-relative in the cabinet segment).
    pub rel_path: String,
    pub hash: String,
    pub object: String,
    pub size: i64,
    pub mime: Option<String>,
    pub doc_type_id: Option<String>,
    pub tax_year: Option<i32>,
    pub status: String,
    pub source: Option<String>,
    pub filed_by: Option<String>,
    pub filed_at: String,
    pub device_id: String,
    pub rev: i64,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub versions: Vec<VersionEntry>,
    pub deleted_at: Option<String>,
    pub tombstone_reason: Option<String>,
}

/// `manifest/clients/<client_id>/<rev>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientSegment {
    pub client_id: String,
    pub segment_rev: u64,
    pub folder_rel: String,
    /// `_Vibe/client.json` as its exact text (SentinelV1), so a restore writes the same bytes.
    pub sentinel: String,
    pub documents: Vec<DocEntry>,
    pub empty_dirs: Vec<String>,
    #[serde(default)]
    pub written_by: String,
}

/// `manifest/cabinet/<rev>`: the cabinet's shared master files (client list, relationships,
/// template, doc types, retention classes, legal holds, Filer settings and rules, parser
/// profiles) by `_Vibe`-relative path, and documents outside client folders.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CabinetSegment {
    pub segment_rev: u64,
    pub files: BTreeMap<String, String>,
    pub documents: Vec<DocEntry>,
    #[serde(default)]
    pub written_by: String,
}

/// `devices/<device_id>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceRecord {
    pub device_id: String,
    pub name: String,
    pub mode: String,
    pub last_seen: String,
    /// The manifest rev this device has applied.
    pub acked_rev: u64,
}

/// `_Vibe`-relative master files the cabinet segment carries. Never: `keys/`, Filer's working
/// folders, `locks/`, versions (objects), audit (its own keys), per-PC state.
pub const MASTER_FILES: &[&str] = &[
    "cabinet.json",
    "clients.json",
    "relationships.json",
    "template.json",
    "clients.vf.json",
    "doc_types.json",
    "retention.json",
    "legal_holds.json",
    "extract_aliases.json",
    "trash_pin.json",
    "config.json",
    "rules.json",
];
