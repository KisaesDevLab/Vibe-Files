//! Users, roles, local auth, Windows auto-login, DPAPI, PII detectors, section 7216.
//!
//! P01 brings [`keys`] (the catalog key wrapped by the recovery passphrase); the rest lands with
//! P06.

pub mod keys;
