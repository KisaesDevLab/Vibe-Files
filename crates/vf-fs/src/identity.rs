//! A Windows login's identity in the cabinet (PA-69, owner 2026-11-13: session hosts are reset
//! to a clean image every night and "I have no way of ensuring AppData\Local is maintained").
//!
//! A PC's id lives in its own settings. On a session host that is wiped, each morning gave
//! every login a new id: a new Intake folder in the cabinet every day (yesterday's last batch
//! and its undo left behind in the old one), and a different name on everything the person
//! did. The cabinet now remembers the id each login was given, in `_Vibe/logins/<login>.json`,
//! and a login with one there uses it whatever host it is on.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cabinet;

const DIR: &str = "logins";

#[derive(Serialize, Deserialize)]
struct LoginFile {
    login: String,
    device_id: String,
    since: String,
}

/// The file of `login` (as Windows gives it, `DOMAIN\user`).
fn path(root: &Path, login: &str) -> PathBuf {
    let safe: String = login
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    root.join(cabinet::VIBE_DIR)
        .join(DIR)
        .join(format!("{safe}.json"))
}

/// The id `login` works under in this cabinet: the one the cabinet has for it; else `mine`
/// (this PC's own), which the cabinet then remembers for the login. A cabinet that cannot be
/// read or written leaves `mine` as it is.
pub fn login_device(root: &Path, login: &str, mine: &str) -> String {
    if login.trim().is_empty() || cabinet::read(root).is_err() {
        return mine.to_string();
    }
    let p = path(root, login);
    let read = |p: &Path| -> Option<String> {
        let f: LoginFile = serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()?;
        (!f.device_id.trim().is_empty()).then_some(f.device_id)
    };
    if let Some(id) = read(&p) {
        return id;
    }
    // Under the cabinet lock: the same login starting on two hosts at once gets one id.
    crate::lock::with(root, "logins", "the list of logins", || {
        if let Some(id) = read(&p) {
            return Ok(id);
        }
        let f = LoginFile {
            login: login.trim().to_string(),
            device_id: mine.to_string(),
            since: vf_core::now_rfc3339(),
        };
        cabinet::write_replace(&p, &serde_json::to_vec_pretty(&f).expect("serializes"))?;
        Ok(mine.to_string())
    })
    .unwrap_or_else(|_| mine.to_string())
}

/// [`login_device`] for a session that stays open: the login's id is its own while the
/// session holds it (the lock returned, kept alive until dropped). A second session of the
/// same login open at the same time on another host (a disconnected session left running)
/// works under `mine`, its host's own id, so the two never share one Intake folder.
pub fn login_session(root: &Path, login: &str, mine: &str) -> (String, Option<crate::lock::Held>) {
    if login.trim().is_empty() || cabinet::read(root).is_err() {
        return (mine.to_string(), None);
    }
    let lock = path(root, login).with_extension("lock");
    match crate::lock::take_at(
        &lock,
        Some(std::time::Duration::ZERO),
        crate::lock::BEAT,
        crate::lock::STALE,
    ) {
        Ok(held) => (login_device(root, login, mine), Some(held)),
        Err(_) => (mine.to_string(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A login keeps the id it was first given, whatever id the host it starts on has.
    #[test]
    fn a_login_keeps_its_id_across_hosts() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("cab");
        cabinet::create(
            &root,
            &cabinet::CabinetFile {
                cabinet_id: vf_core::new_id(),
                firm_id: uuid::Uuid::new_v4().to_string(),
                schema_version: vf_core::db::SCHEMA_VERSION,
                layout_version: 1,
                created: vf_core::now_rfc3339(),
                name: None,
            },
        )
        .unwrap();
        // Monday, host 3: the login's first id is remembered.
        assert_eq!(login_device(&root, "FIRM\\pat", "id-monday"), "id-monday");
        // Tuesday, host 5, wiped overnight (a new id in its settings): Monday's id.
        assert_eq!(login_device(&root, "FIRM\\pat", "id-tuesday"), "id-monday");
        assert_eq!(login_device(&root, "firm\\PAT", "id-x"), "id-monday");
        // Another login has its own.
        assert_eq!(login_device(&root, "FIRM\\lee", "id-lee"), "id-lee");
        assert!(root.join("_Vibe/logins/firm_pat.json").is_file());
        // A session holds the login's id; a second one open at the same time gets its host's
        // own; once the first is closed the id is free again.
        let (id, held) = login_session(&root, "FIRM\\pat", "id-host-5");
        assert_eq!(id, "id-monday");
        assert!(held.is_some());
        let (second, none) = login_session(&root, "FIRM\\pat", "id-host-7");
        assert_eq!(second, "id-host-7");
        assert!(none.is_none());
        drop(held);
        let (again, held) = login_session(&root, "FIRM\\pat", "id-host-7");
        assert_eq!(again, "id-monday");
        drop(held);
        assert!(!root.join("_Vibe/logins/firm_pat.lock").exists());
        // Not a cabinet, or no login: the PC's own id.
        assert_eq!(login_device(tmp.path(), "FIRM\\pat", "id-own"), "id-own");
        assert_eq!(login_device(&root, " ", "id-own"), "id-own");
    }
}
