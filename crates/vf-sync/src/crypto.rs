//! Client-side encryption of everything in the bucket (docs/BUCKET_LAYOUT.md §Encryption).
//!
//! The firm sync key (32 random bytes, wrapped by the sync passphrase through
//! `vf_security::keys`) never leaves the firm. Two keys are derived from it with HKDF-SHA256:
//! the content key (XChaCha20-Poly1305 for every object) and the naming key (HMAC-SHA256 of a
//! content hash gives its object name, so the listing reveals no plaintext hash).

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hmac::{Hmac, Mac};
use sha2::Sha256;

/// Object body prefix.
const MAGIC: &[u8; 4] = b"VF1\0";

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum CryptoError {
    #[error("not a Vibe Files object")]
    Format,
    #[error("object sealed with key {0}, which this cabinet does not have")]
    UnknownKey(String),
    #[error("the object does not decrypt: wrong key, or it was changed")]
    Tampered,
}

/// The keys derived from one firm sync key.
#[derive(Clone)]
pub struct Keys {
    pub key_id: String,
    content: [u8; 32],
    naming: [u8; 32],
}

impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Keys({})", self.key_id) // never the key bytes
    }
}

fn hkdf(firm: &[u8; 32], info: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    hkdf::Hkdf::<Sha256>::new(Some(b"vibe-files sync v1"), firm)
        .expand(info, &mut out)
        .expect("32 bytes is a valid HKDF length");
    out
}

impl Keys {
    pub fn derive(key_id: &str, firm: &[u8; 32]) -> Keys {
        Keys {
            key_id: key_id.into(),
            content: hkdf(firm, b"content"),
            naming: hkdf(firm, b"naming"),
        }
    }

    /// The object name for content with SHA-256 `hash` (hex).
    pub fn object_name(&self, hash: &str) -> String {
        let mut m = <Hmac<Sha256> as Mac>::new_from_slice(&self.naming).expect("any key length");
        m.update(hash.as_bytes());
        hex(&m.finalize().into_bytes())
    }

    /// `VF1\0 | key_id (u16 LE length + UTF-8) | nonce (24) | ciphertext + tag`. `aad` binds the
    /// object to its role (its key in the bucket), so objects cannot be swapped.
    pub fn seal(&self, plain: &[u8], aad: &str) -> Vec<u8> {
        let mut nonce = [0u8; 24];
        getrandom::fill(&mut nonce).expect("system random generator");
        let ct = XChaCha20Poly1305::new((&self.content).into())
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plain,
                    aad: aad.as_bytes(),
                },
            )
            .expect("encryption does not fail");
        let id = self.key_id.as_bytes();
        let mut out = Vec::with_capacity(4 + 2 + id.len() + 24 + ct.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&(id.len() as u16).to_le_bytes());
        out.extend_from_slice(id);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ct);
        out
    }

    pub fn open(&self, body: &[u8], aad: &str) -> Result<Vec<u8>, CryptoError> {
        if body.len() < 6 || &body[..4] != MAGIC {
            return Err(CryptoError::Format);
        }
        let n = u16::from_le_bytes([body[4], body[5]]) as usize;
        let rest = &body[6..];
        if rest.len() < n + 24 + 16 {
            return Err(CryptoError::Format);
        }
        let id = std::str::from_utf8(&rest[..n]).map_err(|_| CryptoError::Format)?;
        if id != self.key_id {
            return Err(CryptoError::UnknownKey(id.into()));
        }
        let nonce = &rest[n..n + 24];
        XChaCha20Poly1305::new((&self.content).into())
            .decrypt(
                XNonce::from_slice(nonce),
                Payload {
                    msg: &rest[n + 24..],
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| CryptoError::Tampered)
    }
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// SHA-256 of `b`, hex.
pub fn sha256(b: &[u8]) -> String {
    use sha2::Digest;
    hex(&Sha256::digest(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open_round_trip_and_tamper() {
        let k = Keys::derive("k-01", &[7u8; 32]);
        let body = k.seal(b"hello", "objects/ab");
        assert_eq!(k.open(&body, "objects/ab").unwrap(), b"hello");
        assert_eq!(k.open(&body, "objects/cd"), Err(CryptoError::Tampered));
        let mut bad = body.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert_eq!(k.open(&bad, "objects/ab"), Err(CryptoError::Tampered));
        let other = Keys::derive("k-02", &[7u8; 32]);
        assert_eq!(
            other.open(&body, "objects/ab"),
            Err(CryptoError::UnknownKey("k-01".into()))
        );
        assert!(!body.windows(5).any(|w| w == b"hello"));
    }

    #[test]
    fn names_are_keyed() {
        let a = Keys::derive("k-01", &[1u8; 32]);
        let b = Keys::derive("k-01", &[2u8; 32]);
        let h = sha256(b"x");
        assert_eq!(a.object_name(&h), a.object_name(&h));
        assert_ne!(a.object_name(&h), b.object_name(&h));
        assert_ne!(a.object_name(&h), h);
        assert_eq!(a.object_name(&h).len(), 64);
    }
}
