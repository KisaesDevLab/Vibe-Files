//! Users, roles, local auth, Windows auto-login, DPAPI, PII detectors, section 7216.
//!
//! P01 step 1 brought in [`keys`] (recovery-passphrase wrapping); the rest lands in P06.

pub mod keys;

pub mod pin;
