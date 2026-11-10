//! The firm's Vibe Portal (PA-37), the parts that are not windows: who this PC is, the Portal
//! inbox, and fetching client uploads into it.
//!
//! Uploads are fetched every few minutes (Admin › Portal) by whichever PC is running: each is
//! downloaded into `_Vibe/tmp/portal/`, checked against the portal's sha256, marked fetched (one
//! PC wins; the others drop their copy), then moved into `_Portal inbox` as
//! `<Client name>_<Client ID>_<file name>`, so Intake's usual reading finds the client.

use std::path::Path;

use vf_portal::{Portal, PortalSettings};

pub use vf_portal::inbox::{FetchReport, INBOX, inbox, recent_received};

/// This PC and Windows user, as the portal's history shows them (Vibe Portal P15).
pub fn who() -> (String, String) {
    (
        vf_sync::computer_name(),
        std::env::var("USERNAME").unwrap_or_default(),
    )
}

pub fn client(s: &PortalSettings) -> Portal {
    let (pc, user) = who();
    Portal::new(s, &pc, &user)
}

/// Adds the Portal inbox to Intake's shared inboxes (and makes the folder), once.
pub fn register_inbox(root: &Path) -> Result<bool, String> {
    let dir = inbox(root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    let master = vf_fs::clients::master_dir(root);
    std::fs::create_dir_all(&master).map_err(|e| e.to_string())?;
    let (mut fs, _) = filer_intake::config::Settings::load_or_default(&master);
    if fs.inboxes.iter().any(|p| p == &dir) {
        return Ok(false);
    }
    fs.inboxes.push(dir);
    fs.save(&master).map_err(|e| e.to_string())?;
    Ok(true)
}

/// Fetches every waiting upload into the Portal inbox (on a thread).
pub fn fetch(root: &Path, s: &PortalSettings, now: &str) -> FetchReport {
    vf_portal::inbox::fetch(root, &client(s), now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_portal_inbox_is_added_to_intake_once_and_is_not_a_client_folder() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        assert!(register_inbox(root).unwrap());
        assert!(!register_inbox(root).unwrap());
        let (fs, _) =
            filer_intake::config::Settings::load_or_default(&vf_fs::clients::master_dir(root));
        assert_eq!(fs.inboxes, vec![inbox(root)]);
        assert!(vf_fs::cabinet::client_folders(root).unwrap().is_empty());
    }
}
