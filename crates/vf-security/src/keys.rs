//! Catalog key generation and wrapping.
//!
//! The catalog key is 32 random bytes. It is never stored in the clear: the cabinet holds a copy
//! wrapped by the recovery passphrase (Argon2id → ChaCha20-Poly1305) in
//! `<cabinet>/_Vibe/keys/recovery.json`, so any workstation (or a fresh machine) can open the
//! cabinet with the passphrase. P06 adds a DPAPI-wrapped copy per Windows user beside it.

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use serde::{Deserialize, Serialize};

/// Shortest recovery passphrase the wizard accepts.
pub const MIN_PASSPHRASE_LEN: usize = 12;

const FORMAT_VERSION: u32 = 1;
const KDF: &str = "argon2id";
const CIPHER: &str = "chacha20poly1305";

#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error("the recovery passphrase is wrong, or the key file was changed")]
    WrongPassphrase,
    #[error("the key file is not valid: {0}")]
    BadFile(String),
    #[error("the system random generator failed: {0}")]
    Random(String),
}

/// Argon2id cost. Stored in the wrapped key so stronger defaults later still open old cabinets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
}

impl KdfParams {
    /// 64 MiB, 3 passes, 1 lane (about half a second on an office PC).
    pub const DEFAULT: KdfParams = KdfParams {
        m_kib: 64 * 1024,
        t: 3,
        p: 1,
    };
}

/// A key wrapped by a passphrase: the on-disk JSON shape of `_Vibe/keys/recovery.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WrappedKey {
    pub v: u32,
    pub kdf: String,
    pub params: KdfParams,
    pub cipher: String,
    /// Hex.
    pub salt: String,
    /// Hex.
    pub nonce: String,
    /// Hex: the 32-byte key plus the 16-byte tag.
    pub ciphertext: String,
}

impl WrappedKey {
    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec_pretty(self).expect("serializes")
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, KeyError> {
        let w: WrappedKey =
            serde_json::from_slice(bytes).map_err(|e| KeyError::BadFile(e.to_string()))?;
        if w.v != FORMAT_VERSION || w.kdf != KDF || w.cipher != CIPHER {
            return Err(KeyError::BadFile(format!(
                "unsupported format v{} {} {}",
                w.v, w.kdf, w.cipher
            )));
        }
        Ok(w)
    }
}

/// 32 fresh random bytes.
pub fn generate_key() -> Result<[u8; 32], KeyError> {
    let mut k = [0u8; 32];
    getrandom::fill(&mut k).map_err(|e| KeyError::Random(e.to_string()))?;
    Ok(k)
}

/// Wraps `key` with `passphrase`. `context` (the cabinet id) is bound as associated data, so a
/// key file copied onto another cabinet does not open it.
pub fn wrap_with_passphrase(
    key: &[u8; 32],
    passphrase: &str,
    context: &str,
    params: KdfParams,
) -> Result<WrappedKey, KeyError> {
    let mut salt = [0u8; 16];
    let mut nonce = [0u8; 12];
    getrandom::fill(&mut salt).map_err(|e| KeyError::Random(e.to_string()))?;
    getrandom::fill(&mut nonce).map_err(|e| KeyError::Random(e.to_string()))?;
    let kek = zeroize::Zeroizing::new(derive(passphrase, &salt, params)?);
    let ct = ChaCha20Poly1305::new(Key::from_slice(&*kek))
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: key,
                aad: context.as_bytes(),
            },
        )
        .map_err(|_| KeyError::BadFile("encryption failed".into()))?;
    Ok(WrappedKey {
        v: FORMAT_VERSION,
        kdf: KDF.into(),
        params,
        cipher: CIPHER.into(),
        salt: to_hex(&salt),
        nonce: to_hex(&nonce),
        ciphertext: to_hex(&ct),
    })
}

/// Reverses [`wrap_with_passphrase`]. A wrong passphrase, wrong `context`, or any changed byte
/// gives [`KeyError::WrongPassphrase`].
pub fn unwrap_with_passphrase(
    w: &WrappedKey,
    passphrase: &str,
    context: &str,
) -> Result<[u8; 32], KeyError> {
    let salt = from_hex(&w.salt)?;
    let nonce = from_hex(&w.nonce)?;
    let ct = from_hex(&w.ciphertext)?;
    if nonce.len() != 12 {
        return Err(KeyError::BadFile("nonce length".into()));
    }
    let mut kek = derive(passphrase, &salt, w.params)?;
    let r = ChaCha20Poly1305::new(Key::from_slice(&kek)).decrypt(
        Nonce::from_slice(&nonce),
        Payload {
            msg: &ct,
            aad: context.as_bytes(),
        },
    );
    // The key-encryption key and the plain key's copies are wiped (QA X7).
    zeroize::Zeroize::zeroize(&mut kek);
    let mut pt = zeroize::Zeroizing::new(r.map_err(|_| KeyError::WrongPassphrase)?);
    let key: [u8; 32] = pt
        .as_slice()
        .try_into()
        .map_err(|_| KeyError::BadFile("key length".into()))?;
    zeroize::Zeroize::zeroize(&mut *pt);
    Ok(key)
}

fn derive(passphrase: &str, salt: &[u8], p: KdfParams) -> Result<[u8; 32], KeyError> {
    // A key file (or the bucket's cabinet.json) asking for absurd cost is refused, not run.
    if p.m_kib > 1024 * 1024 || p.t > 16 || p.p > 16 || p.m_kib < 8 {
        return Err(KeyError::BadFile(
            "unreasonable key derivation settings".into(),
        ));
    }
    let params =
        Params::new(p.m_kib, p.t, p.p, Some(32)).map_err(|e| KeyError::BadFile(e.to_string()))?;
    let mut out = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), salt, &mut out)
        .map_err(|e| KeyError::BadFile(e.to_string()))?;
    Ok(out)
}

fn to_hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(s: &str) -> Result<Vec<u8>, KeyError> {
    if !s.len().is_multiple_of(2) || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(KeyError::BadFile("not hex".into()));
    }
    Ok((0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("checked hex"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cheap parameters so the tests are fast; the format is the same.
    const FAST: KdfParams = KdfParams {
        m_kib: 256,
        t: 1,
        p: 1,
    };

    #[test]
    fn round_trip_through_json() {
        let key = generate_key().unwrap();
        let w = wrap_with_passphrase(&key, "correct horse battery", "cab-1", FAST).unwrap();
        let back = WrappedKey::from_json(&w.to_json()).unwrap();
        assert_eq!(
            unwrap_with_passphrase(&back, "correct horse battery", "cab-1").unwrap(),
            key
        );
    }

    #[test]
    fn wrong_passphrase_or_cabinet_fails() {
        let key = generate_key().unwrap();
        let w = wrap_with_passphrase(&key, "correct horse battery", "cab-1", FAST).unwrap();
        assert!(matches!(
            unwrap_with_passphrase(&w, "wrong horse battery", "cab-1"),
            Err(KeyError::WrongPassphrase)
        ));
        assert!(matches!(
            unwrap_with_passphrase(&w, "correct horse battery", "cab-2"),
            Err(KeyError::WrongPassphrase)
        ));
    }

    #[test]
    fn a_flipped_byte_fails() {
        let key = generate_key().unwrap();
        let mut w = wrap_with_passphrase(&key, "correct horse battery", "cab-1", FAST).unwrap();
        let mut ct = from_hex(&w.ciphertext).unwrap();
        ct[3] ^= 1;
        w.ciphertext = to_hex(&ct);
        assert!(matches!(
            unwrap_with_passphrase(&w, "correct horse battery", "cab-1"),
            Err(KeyError::WrongPassphrase)
        ));
    }

    #[test]
    fn each_wrap_is_salted() {
        let key = generate_key().unwrap();
        let a = wrap_with_passphrase(&key, "correct horse battery", "c", FAST).unwrap();
        let b = wrap_with_passphrase(&key, "correct horse battery", "c", FAST).unwrap();
        assert_ne!(a.salt, b.salt);
        assert_ne!(a.ciphertext, b.ciphertext);
    }

    #[test]
    fn rejects_unknown_format() {
        let key = generate_key().unwrap();
        let mut w = wrap_with_passphrase(&key, "correct horse battery", "c", FAST).unwrap();
        w.v = 99;
        assert!(WrappedKey::from_json(&w.to_json()).is_err());
    }
}
