//! The firm sync key (P11 step 2). Made once per cabinet: 32 random bytes, wrapped by the sync
//! passphrase (Argon2id, bound to the cabinet id). The wrapped copy lives in the bucket's
//! `cabinet.json` (so a bare PC restores from the bucket and the passphrase) and in
//! `_Vibe/keys/sync.json` (never pushed). The passphrase is the escrow; nothing holds the key in
//! the clear on disk.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use vf_security::keys::{self, KdfParams, KeyError, WrappedKey};

use crate::crypto::Keys;
use crate::layout::{self, BucketCabinet};
use crate::remote::Remote;
use crate::{Result, SyncError};

pub const KEY_ID: &str = "k-01";
pub const KEY_FILE: &str = "sync.json";
/// Argon2id cost for new sync keys.
pub const DEFAULT_KDF: KdfParams = KdfParams::DEFAULT;

pub fn key_file(root: &Path) -> PathBuf {
    root.join(vf_fs::cabinet::VIBE_DIR)
        .join(vf_fs::cabinet::KEYS_DIR)
        .join(KEY_FILE)
}

/// The bucket's `cabinet.json`, when this cabinet has been pushed.
pub fn bucket_cabinet(remote: &Remote) -> Result<Option<BucketCabinet>> {
    let Some(b) = remote.get(layout::CABINET)? else {
        return Ok(None);
    };
    serde_json::from_slice(&b)
        .map(Some)
        .map_err(|e| SyncError::Format(format!("cabinet.json: {e}")))
}

fn unwrap(w: &WrappedKey, passphrase: &str, cabinet_id: &str) -> Result<Keys> {
    match keys::unwrap_with_passphrase(w, passphrase, cabinet_id) {
        Ok(k) => Ok(Keys::derive(KEY_ID, &k)),
        Err(KeyError::WrongPassphrase) => Err(SyncError::WrongPassphrase),
        Err(e) => Err(SyncError::Local(e.to_string())),
    }
}

/// Sets up sync for the cabinet at `root`: joins the bucket's existing key when there is one
/// (the passphrase must open it), else makes the key and writes both wrapped copies.
pub fn init(root: &Path, remote: &Remote, passphrase: &str, params: KdfParams) -> Result<Keys> {
    init_with_firm(root, remote, passphrase, params).map(|(_, k)| k)
}

/// [`init`], also returning the firm key itself (to remember it without a second Argon2 run).
pub fn init_with_firm(
    root: &Path,
    remote: &Remote,
    passphrase: &str,
    params: KdfParams,
) -> Result<([u8; 32], Keys)> {
    if passphrase.chars().count() < keys::MIN_PASSPHRASE_LEN {
        return Err(SyncError::Local(format!(
            "The sync passphrase needs at least {} characters.",
            keys::MIN_PASSPHRASE_LEN
        )));
    }
    let cab = vf_fs::cabinet::read(root)?;
    if let Some(b) = bucket_cabinet(remote)? {
        let w = b
            .wrapped
            .get(KEY_ID)
            .ok_or_else(|| SyncError::Format("cabinet.json has no key".into()))?;
        let firm =
            keys::unwrap_with_passphrase(w, passphrase, &cab.cabinet_id).map_err(|e| match e {
                KeyError::WrongPassphrase => SyncError::WrongPassphrase,
                e => SyncError::Local(e.to_string()),
            })?;
        write_local(root, w)?;
        return Ok((firm, Keys::derive(KEY_ID, &firm)));
    }
    let firm = keys::generate_key().map_err(|e| SyncError::Local(e.to_string()))?;
    let w = keys::wrap_with_passphrase(&firm, passphrase, &cab.cabinet_id, params)
        .map_err(|e| SyncError::Local(e.to_string()))?;
    let b = BucketCabinet {
        layout_ver: layout::LAYOUT_VER,
        cabinet_id: cab.cabinet_id.clone(),
        firm_id: cab.firm_id.clone(),
        created: vf_core::now_rfc3339(),
        key_ids: vec![KEY_ID.into()],
        wrapped: BTreeMap::from([(KEY_ID.to_string(), w.clone())]),
        object_lock_days: None,
    };
    remote.put(
        layout::CABINET,
        serde_json::to_vec_pretty(&b).expect("serializes"),
    )?;
    write_local(root, &w)?;
    Ok((firm, Keys::derive(KEY_ID, &firm)))
}

fn write_local(root: &Path, w: &WrappedKey) -> Result<()> {
    let p = key_file(root);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| SyncError::Local(e.to_string()))?;
    }
    vf_fs::cabinet::write_replace(&p, &w.to_json())?;
    Ok(())
}

/// The keys from the passphrase: the local wrapped copy, else the bucket's.
pub fn unlock(root: &Path, remote: &Remote, cabinet_id: &str, passphrase: &str) -> Result<Keys> {
    if let Ok(b) = std::fs::read(key_file(root)) {
        let w = WrappedKey::from_json(&b).map_err(|e| SyncError::Local(e.to_string()))?;
        return unwrap(&w, passphrase, cabinet_id);
    }
    let b = bucket_cabinet(remote)?.ok_or(SyncError::NotSetUp)?;
    let w = b.wrapped.get(KEY_ID).ok_or(SyncError::NotSetUp)?;
    let k = unwrap(w, passphrase, cabinet_id)?;
    write_local(root, w)?;
    Ok(k)
}

/// Seals a secret (the B2 application key) for `config.toml`, so it is not kept in the clear.
pub fn seal_secret(k: &Keys, secret: &str) -> String {
    crate::crypto::hex(&k.seal(secret.as_bytes(), "secret"))
}

pub fn open_secret(k: &Keys, sealed: &str) -> Result<String> {
    let b: Vec<u8> = (0..sealed.len())
        .step_by(2)
        .filter_map(|i| sealed.get(i..i + 2))
        .filter_map(|h| u8::from_str_radix(h, 16).ok())
        .collect();
    let p = k.open(&b, "secret")?;
    String::from_utf8(p).map_err(|e| SyncError::Local(e.to_string()))
}

/// The keys from the passphrase and the local wrapped copy only (no network), so the sealed
/// application key can be opened before the bucket is reached.
pub fn unlock_local(root: &Path, cabinet_id: &str, passphrase: &str) -> Result<Keys> {
    let b = std::fs::read(key_file(root)).map_err(|_| SyncError::NotSetUp)?;
    let w = WrappedKey::from_json(&b).map_err(|e| SyncError::Local(e.to_string()))?;
    unwrap(&w, passphrase, cabinet_id)
}

/// Records the firm's Object Lock choice in the bucket's `cabinet.json` (display only: the lock
/// is set on the bucket in B2).
pub fn set_object_lock(remote: &Remote, days: Option<u32>) -> Result<()> {
    let mut b = bucket_cabinet(remote)?.ok_or(SyncError::NotSetUp)?;
    b.object_lock_days = days;
    remote.put(
        layout::CABINET,
        serde_json::to_vec_pretty(&b).expect("serializes"),
    )?;
    Ok(())
}

/// The firm sync key itself from the passphrase and the local wrapped copy (to remember it).
pub fn firm_key_local(root: &Path, cabinet_id: &str, passphrase: &str) -> Result<[u8; 32]> {
    let b = std::fs::read(key_file(root)).map_err(|_| SyncError::NotSetUp)?;
    let w = WrappedKey::from_json(&b).map_err(|e| SyncError::Local(e.to_string()))?;
    keys::unwrap_with_passphrase(&w, passphrase, cabinet_id).map_err(|e| match e {
        KeyError::WrongPassphrase => SyncError::WrongPassphrase,
        e => SyncError::Local(e.to_string()),
    })
}

/// Where this PC keeps the firm key sealed for the Windows login (QUESTIONS Q-38): in the
/// workstation's data folder, never in the cabinet or the bucket.
pub fn remembered_file(data: &Path, cabinet_id: &str) -> PathBuf {
    data.join(format!("sync-key-{cabinet_id}.dpapi"))
}

/// Remembers the firm key on this PC for this Windows login (DPAPI), so sync unlocks itself.
pub fn remember(data: &Path, cabinet_id: &str, firm: &[u8; 32]) -> Result<()> {
    let sealed = vf_security::dpapi::protect(firm, cabinet_id.as_bytes())?;
    std::fs::create_dir_all(data).map_err(|e| SyncError::Local(e.to_string()))?;
    vf_fs::cabinet::write_replace(&remembered_file(data, cabinet_id), &sealed)?;
    Ok(())
}

/// The remembered keys, when this login on this PC has them.
pub fn recall(data: &Path, cabinet_id: &str) -> Option<Keys> {
    let sealed = std::fs::read(remembered_file(data, cabinet_id)).ok()?;
    // Both copies of the key are wiped once the keys are derived (QA2 #10).
    let plain = zeroize::Zeroizing::new(
        vf_security::dpapi::unprotect(&sealed, cabinet_id.as_bytes()).ok()?,
    );
    let firm: zeroize::Zeroizing<[u8; 32]> =
        zeroize::Zeroizing::new(plain.as_slice().try_into().ok()?);
    Some(Keys::derive(KEY_ID, &firm))
}

/// Forgets the remembered key on this PC.
pub fn forget(data: &Path, cabinet_id: &str) {
    let _ = std::fs::remove_file(remembered_file(data, cabinet_id));
}

/// The firm key and the bucket settings kept in the cabinet for everyone who opens it
/// (owner 2026-11-04, Q-43): `_Vibe/keys/sync-shared.json`. `_Vibe/keys` is never pushed, so
/// the bucket stays encrypted with a key it does not hold; anyone who can read the cabinet
/// share can sync, as they can already read the files. The passphrase stays the escrow.
pub const SHARED_FILE: &str = "sync-shared.json";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Shared {
    pub version: u32,
    pub cabinet_id: String,
    /// The firm key, hex.
    pub firm_key: String,
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    /// The bucket's application key id (not secret).
    pub key_id: String,
    /// The application key sealed by the firm key ([`seal_secret`]).
    pub app_key_sealed: String,
}

impl Drop for Shared {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.firm_key);
    }
}

pub fn shared_file(root: &Path) -> PathBuf {
    root.join(vf_fs::cabinet::VIBE_DIR)
        .join(vf_fs::cabinet::KEYS_DIR)
        .join(SHARED_FILE)
}

/// Keeps the firm key and the bucket settings in the cabinet for everyone.
pub fn share(
    root: &Path,
    cabinet_id: &str,
    firm: &[u8; 32],
    s3: &crate::remote::S3Config,
    app_key_sealed: &str,
) -> Result<()> {
    let s = Shared {
        version: 1,
        cabinet_id: cabinet_id.into(),
        firm_key: firm.iter().map(|b| format!("{b:02x}")).collect(),
        endpoint: s3.endpoint.clone(),
        region: s3.region.clone(),
        bucket: s3.bucket.clone(),
        key_id: s3.key_id.clone(),
        app_key_sealed: app_key_sealed.into(),
    };
    let p = shared_file(root);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| SyncError::Local(e.to_string()))?;
    }
    let bytes = zeroize::Zeroizing::new(serde_json::to_vec_pretty(&s).expect("serializes"));
    vf_fs::cabinet::write_replace(&p, &bytes)?;
    Ok(())
}

/// The shared settings and keys for this cabinet, when the firm keeps them in the cabinet.
pub fn shared(root: &Path, cabinet_id: &str) -> Option<(Shared, Keys)> {
    let text = zeroize::Zeroizing::new(std::fs::read(shared_file(root)).ok()?);
    let s: Shared = serde_json::from_slice(&text).ok()?;
    if s.version != 1 || s.cabinet_id != cabinet_id || s.firm_key.len() != 64 {
        return None;
    }
    let mut firm = zeroize::Zeroizing::new([0u8; 32]);
    for (i, b) in firm.iter_mut().enumerate() {
        *b = u8::from_str_radix(s.firm_key.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    let keys = Keys::derive(KEY_ID, &firm);
    Some((s, keys))
}

/// Stops keeping the key in the cabinet: everyone needs the passphrase (or their own
/// remembered copy) again.
pub fn unshare(root: &Path) {
    let _ = std::fs::remove_file(shared_file(root));
}

#[cfg(test)]
mod shared_tests {
    use super::*;

    #[test]
    fn shared_key_round_trips_and_is_bound_to_the_cabinet() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let firm = [7u8; 32];
        let s3 = crate::remote::S3Config {
            endpoint: "https://s3.example.invalid".into(),
            region: "us-west-004".into(),
            bucket: "invented-bucket".into(),
            key_id: "invented-key-id".into(),
        };
        share(root, "cab-1", &firm, &s3, "sealed").unwrap();
        let (s, keys) = shared(root, "cab-1").unwrap();
        assert_eq!(s.bucket, "invented-bucket");
        assert_eq!(s.app_key_sealed, "sealed");
        // The same keys as the firm key gives.
        let direct = Keys::derive(KEY_ID, &firm);
        let sealed = seal_secret(&direct, "x");
        assert_eq!(open_secret(&keys, &sealed).unwrap(), "x");
        assert!(shared(root, "another cabinet").is_none());
        unshare(root);
        assert!(shared(root, "cab-1").is_none());
    }
}
