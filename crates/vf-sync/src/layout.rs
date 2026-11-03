//! Object keys and manifest shapes: docs/BUCKET_LAYOUT.md, layout v1.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use vf_security::keys::WrappedKey;

pub const LAYOUT_VER: u32 = 1;
pub const CABINET: &str = "cabinet.json";
pub const HEAD: &str = "manifest/HEAD";

pub fn object_key(name: &str) -> String {
    format!("objects/{}/{name}", &name[..2])
}

pub fn segment_key(client_id: &str, rev: u64) -> String {
    format!("manifest/clients/{client_id}/{rev}")
}

pub fn cabinet_segment_key(rev: u64) -> String {
    format!("manifest/cabinet/{rev}")
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
