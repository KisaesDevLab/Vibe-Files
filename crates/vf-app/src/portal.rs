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

/// The path as every PC can reach it: a mapped drive becomes its `\\server\share\…` form
/// (Windows resolves that when the path is made canonical); a path that cannot be resolved
/// stays as given.
pub fn portable(p: &Path) -> std::path::PathBuf {
    let Ok(c) = std::fs::canonicalize(p) else {
        return p.to_path_buf();
    };
    let s = c.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        std::path::PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        std::path::PathBuf::from(rest)
    } else {
        c
    }
}

/// Adds the Portal inbox to Intake's shared inboxes (and makes the folder), once.
pub fn register_inbox(root: &Path) -> Result<bool, String> {
    let dir = inbox(root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    let master = vf_fs::clients::master_dir(root);
    std::fs::create_dir_all(&master).map_err(|e| e.to_string())?;
    let (mut fs, _) = filer_intake::config::Settings::load_or_default(&master);
    // Intake's settings are shared by every PC: keep the path every PC reaches (a mapped
    // drive's \\server\share form), and compare in that form (QA 2026-11-09).
    let dir = portable(&dir);
    if fs.inboxes.iter().any(|p| portable(p) == dir) {
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
        assert_eq!(fs.inboxes, vec![portable(&inbox(root))]);
        // The same folder reached another way is not added again.
        let other = root.join(".").join(INBOX);
        let (mut fs2, _) =
            filer_intake::config::Settings::load_or_default(&vf_fs::clients::master_dir(root));
        fs2.inboxes = vec![other];
        fs2.save(&vf_fs::clients::master_dir(root)).unwrap();
        assert!(!register_inbox(root).unwrap());
        assert!(vf_fs::cabinet::client_folders(root).unwrap().is_empty());
    }
}
