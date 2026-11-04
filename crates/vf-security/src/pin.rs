//! The firm PIN (QUESTIONS Q-29): users and roles are v2, so the few actions an admin would
//! gate (emptying the trash by hand) ask for one cabinet-wide PIN instead. Only an Argon2id
//! hash (PHC string) is stored; the PIN itself is never written or logged.

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};

/// Shortest PIN accepted (the hash sits in the shared cabinet: short PINs fall to a search).
pub const MIN_PIN_LEN: usize = 6;

/// Argon2id cost for new PIN hashes (64 MiB, 3 passes); old hashes keep theirs.
fn hasher() -> Argon2<'static> {
    Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        Params::new(64 * 1024, 3, 1, None).expect("valid Argon2 parameters"),
    )
}

/// Wrong PINs in a row in this process, and when the last one was.
static FAILS: std::sync::Mutex<(u32, Option<std::time::Instant>)> =
    std::sync::Mutex::new((0, None));

/// Seconds to wait before the next try, after wrong PINs (none for the first three; then 30 s,
/// doubling to 10 minutes).
pub fn wait_secs() -> u64 {
    let f = FAILS.lock().unwrap_or_else(|e| e.into_inner());
    if f.0 < 3 {
        return 0;
    }
    let need = (30u64 << (f.0 - 3).min(5)).min(600);
    let gone = f.1.map_or(need, |t| t.elapsed().as_secs());
    need.saturating_sub(gone)
}

/// [`verify`] with the back-off: `Err(seconds)` while a wait is due (the PIN is not checked).
pub fn check(pin: &str, phc: &str) -> Result<bool, u64> {
    let w = wait_secs();
    if w > 0 {
        return Err(w);
    }
    let ok = verify(pin, phc);
    let mut f = FAILS.lock().unwrap_or_else(|e| e.into_inner());
    if ok {
        *f = (0, None);
    } else {
        *f = (f.0 + 1, Some(std::time::Instant::now()));
    }
    Ok(ok)
}

/// The PHC hash to store for `pin`.
pub fn hash(pin: &str) -> Result<String, String> {
    if pin.chars().count() < MIN_PIN_LEN {
        return Err(format!(
            "the admin password needs at least {MIN_PIN_LEN} characters"
        ));
    }
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|e| format!("random generator: {e}"))?;
    let salt = SaltString::encode_b64(&salt).map_err(|e| e.to_string())?;
    hasher()
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
        let a = hash("482193").unwrap();
        let b = hash("482193").unwrap();
        assert_ne!(a, b, "salted");
        assert!(a.starts_with("$argon2id$"));
        assert!(!a.contains("482193"));
        assert!(verify("482193", &a) && verify("482193", &b));
        assert!(!verify("482194", &a));
        assert!(!verify("482193", "not a hash"));
        assert!(hash("12").is_err());
    }
}
