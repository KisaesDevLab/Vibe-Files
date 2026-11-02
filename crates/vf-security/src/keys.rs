//! The catalog key and its wrappings (CLAUDE.md §Security rules).
//!
//! The catalog key is 32 random bytes. It is never stored in the clear: `<data>/keys.json` holds
//! it encrypted (XChaCha20-Poly1305) under a key derived from the recovery passphrase set at
//! cabinet creation (Argon2id). P06 adds a DPAPI wrapping beside the passphrase one so a known
//! Windows user opens the catalog without typing it; the passphrase stays as the recovery path.

use std::path::Path;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};

pub const KEYS_FILE: &str = "keys.json";
/// Shortest recovery passphrase accepted.
pub const MIN_PASSPHRASE_CHARS: usize = 12;

#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error("the recovery passphrase is wrong")]
    WrongPassphrase,
    #[error("the recovery passphrase must be at least {MIN_PASSPHRASE_CHARS} characters")]
    WeakPassphrase,
    #[error("{0}")]
    Io(String),
    #[error("{0} is not a valid key file: {1}")]
    Invalid(String, String),
}

/// Argon2id cost. [`Kdf::DEFAULT`] for real cabinets; tests use [`Kdf::FAST`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kdf {
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
}

impl Kdf {
    /// 64 MiB, 3 passes: well under a second on an office PC, expensive to brute-force.
    pub const DEFAULT: Kdf = Kdf {
        m_kib: 64 * 1024,
        t: 3,
        p: 1,
    };
    /// For tests only.
    pub const FAST: Kdf = Kdf {
        m_kib: 64,
        t: 1,
        p: 1,
    };
}

/// One wrapping of the catalog key. Stored as `keys.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyFile {
    pub version: u32,
    /// Short id shown in Admin (`k-1a2b3c4d`); the same for every wrapping of one key.
    pub key_id: String,
    pub kdf: Kdf,
    /// Hex.
    pub salt: String,
    /// Hex, 24 bytes.
    pub nonce: String,
    /// Hex: the encrypted catalog key + Poly1305 tag.
    pub wrapped: String,
}

/// A fresh random 32-byte catalog key.
pub fn generate_key() -> [u8; 32] {
    let mut k = [0u8; 32];
    getrandom::fill(&mut k).expect("the OS random source works");
    k
}

pub fn check_passphrase(passphrase: &str) -> Result<(), KeyError> {
    if passphrase.chars().count() < MIN_PASSPHRASE_CHARS {
        Err(KeyError::WeakPassphrase)
    } else {
        Ok(())
    }
}

fn derive(passphrase: &str, salt: &[u8], kdf: Kdf) -> Result<[u8; 32], KeyError> {
    let params = argon2::Params::new(kdf.m_kib, kdf.t, kdf.p, Some(32))
        .map_err(|e| KeyError::Invalid("kdf".into(), e.to_string()))?;
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut out = [0u8; 32];
    argon
        .hash_password_into(passphrase.as_bytes(), salt, &mut out)
        .map_err(|e| KeyError::Invalid("kdf".into(), e.to_string()))?;
    Ok(out)
}

fn unhex(field: &str, s: &str) -> Result<Vec<u8>, KeyError> {
    hex::decode(s).map_err(|e| KeyError::Invalid(field.into(), e.to_string()))
}

impl KeyFile {
    /// Wraps `key` with `passphrase`. The key id is bound into the ciphertext as associated data.
    pub fn wrap(key: &[u8; 32], passphrase: &str, kdf: Kdf) -> Result<KeyFile, KeyError> {
        check_passphrase(passphrase)?;
        let mut salt = [0u8; 16];
        let mut nonce = [0u8; 24];
        getrandom::fill(&mut salt).expect("random");
        getrandom::fill(&mut nonce).expect("random");
        let mut id = [0u8; 4];
        getrandom::fill(&mut id).expect("random");
        let key_id = format!("k-{}", hex::encode(id));
        let kek = derive(passphrase, &salt, kdf)?;
        let wrapped = XChaCha20Poly1305::new(&kek.into())
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: key,
                    aad: key_id.as_bytes(),
                },
            )
            .expect("encrypting 32 bytes cannot fail");
        Ok(KeyFile {
            version: 1,
            key_id,
            kdf,
            salt: hex::encode(salt),
            nonce: hex::encode(nonce),
            wrapped: hex::encode(wrapped),
        })
    }

    /// The catalog key, or [`KeyError::WrongPassphrase`].
    pub fn unwrap_key(&self, passphrase: &str) -> Result<[u8; 32], KeyError> {
        let salt = unhex("salt", &self.salt)?;
        let nonce = unhex("nonce", &self.nonce)?;
        if nonce.len() != 24 {
            return Err(KeyError::Invalid("nonce".into(), "must be 24 bytes".into()));
        }
        let wrapped = unhex("wrapped", &self.wrapped)?;
        let kek = derive(passphrase, &salt, self.kdf)?;
        let plain = XChaCha20Poly1305::new(&kek.into())
            .decrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: &wrapped,
                    aad: self.key_id.as_bytes(),
                },
            )
            .map_err(|_| KeyError::WrongPassphrase)?;
        plain
            .try_into()
            .map_err(|_| KeyError::Invalid("wrapped".into(), "not a 32-byte key".into()))
    }

    pub fn load(path: &Path) -> Result<KeyFile, KeyError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| KeyError::Io(format!("cannot read {}: {e}", path.display())))?;
        serde_json::from_str(&text)
            .map_err(|e| KeyError::Invalid(path.display().to_string(), e.to_string()))
    }

    /// Writes a new key file; refuses to replace one (losing it would lose the catalog).
    pub fn save_new(&self, path: &Path) -> Result<(), KeyError> {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| KeyError::Io(format!("cannot create {}: {e}", path.display())))?;
        f.write_all(&serde_json::to_vec_pretty(self).expect("serializes"))
            .and_then(|()| f.sync_all())
            .map_err(|e| KeyError::Io(format!("cannot write {}: {e}", path.display())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASS: &str = "correct horse battery";

    #[test]
    fn wrap_then_unwrap_gives_the_key_back() {
        let key = generate_key();
        let kf = KeyFile::wrap(&key, PASS, Kdf::FAST).unwrap();
        assert!(kf.key_id.starts_with("k-"));
        assert!(!kf.wrapped.contains(&hex::encode(key)));
        assert_eq!(kf.unwrap_key(PASS).unwrap(), key);
    }

    #[test]
    fn wrong_passphrase_and_tampering_are_refused() {
        let key = generate_key();
        let kf = KeyFile::wrap(&key, PASS, Kdf::FAST).unwrap();
        assert!(matches!(
            kf.unwrap_key("correct horse battery!"),
            Err(KeyError::WrongPassphrase)
        ));
        let mut other_id = kf.clone();
        other_id.key_id = "k-00000000".into();
        assert!(matches!(
            other_id.unwrap_key(PASS),
            Err(KeyError::WrongPassphrase)
        ));
    }

    #[test]
    fn short_passphrase_is_refused() {
        assert!(matches!(
            KeyFile::wrap(&generate_key(), "short", Kdf::FAST),
            Err(KeyError::WeakPassphrase)
        ));
    }

    #[test]
    fn save_round_trips_and_never_overwrites() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join(KEYS_FILE);
        let key = generate_key();
        let kf = KeyFile::wrap(&key, PASS, Kdf::FAST).unwrap();
        kf.save_new(&p).unwrap();
        assert_eq!(KeyFile::load(&p).unwrap(), kf);
        assert!(kf.save_new(&p).is_err());
    }

    #[test]
    fn default_kdf_unwraps() {
        let key = generate_key();
        let kf = KeyFile::wrap(&key, PASS, Kdf::DEFAULT).unwrap();
        assert_eq!(kf.unwrap_key(PASS).unwrap(), key);
    }
}
