//! The firm PIN (QUESTIONS Q-29): users and roles are v2, so the few actions an admin would
//! gate (emptying the trash by hand) ask for one cabinet-wide PIN instead. Only an Argon2id
//! hash (PHC string) is stored; the PIN itself is never written or logged.

use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};

/// Shortest PIN accepted.
pub const MIN_PIN_LEN: usize = 4;

/// The PHC hash to store for `pin`.
pub fn hash(pin: &str) -> Result<String, String> {
    if pin.chars().count() < MIN_PIN_LEN {
        return Err(format!("the PIN needs at least {MIN_PIN_LEN} characters"));
    }
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|e| format!("random generator: {e}"))?;
    let salt = SaltString::encode_b64(&salt).map_err(|e| e.to_string())?;
    Argon2::default()
        .hash_password(pin.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

/// Whether `pin` matches the stored `phc` hash.
pub fn verify(pin: &str, phc: &str) -> bool {
    PasswordHash::new(phc)
        .map(|h| {
            Argon2::default()
                .verify_password(pin.as_bytes(), &h)
                .is_ok()
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_verify_and_differ_per_salt() {
        let a = hash("4821").unwrap();
        let b = hash("4821").unwrap();
        assert_ne!(a, b, "salted");
        assert!(a.starts_with("$argon2id$"));
        assert!(!a.contains("4821"));
        assert!(verify("4821", &a) && verify("4821", &b));
        assert!(!verify("4822", &a));
        assert!(!verify("4821", "not a hash"));
        assert!(hash("12").is_err());
    }
}
