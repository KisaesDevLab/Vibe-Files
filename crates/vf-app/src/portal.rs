//! The firm's Vibe Portal (PA-37), the parts that are not windows: who this PC is, the Portal
//! inbox, and fetching client uploads into it.
//!
//! Uploads are fetched every few minutes (Admin › Portal) by whichever PC is running: each is
//! downloaded into `_Vibe/tmp/portal/`, checked against the portal's sha256, marked fetched (one
//! PC wins; the others drop their copy), then moved into `_Portal inbox` as
//! `<Client name>_<Client ID>_<file name>`, so Intake's usual reading finds the client.

use std::path::Path;

use vf_portal::{Portal, PortalSettings};

pub use vf_portal::filed::{FiledReport, Pending, Where};
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

/// Tells the portal what Intake filed and lists what it keeps unfiled (PA-43; on a thread).
pub fn check_filed(root: &Path, s: &PortalSettings) -> FiledReport {
    vf_portal::filed::check(root, &client(s))
}

/// Days an upload waits in the Portal inbox before Vibe Files says so.
pub const WAITING_DAYS: i64 = 7;
/// Days before the portal clears an upload never filed that Vibe Files starts saying so.
pub const CLEARING_DAYS: i64 = 14;

/// Browse's line about uploads fetched and not filed: missing from the inbox, or waiting long.
pub fn waiting_note(f: &FiledReport, now: chrono::DateTime<chrono::Utc>) -> Option<String> {
    let missing = f.pending.iter().filter(|p| p.at != Where::InInbox).count();
    let long = f
        .pending
        .iter()
        .filter(|p| p.at == Where::InInbox)
        .filter(|p| {
            chrono::DateTime::parse_from_rfc3339(&p.upload.fetched_at)
                .is_ok_and(|t| now.signed_duration_since(t).num_days() >= WAITING_DAYS)
        })
        .count();
    // The portal clears an upload never filed `cap_days` after the fetch (Vibe-Portal P74).
    let clearing = f
        .pending
        .iter()
        .filter(|p| {
            f.cap_days > 0
                && chrono::DateTime::parse_from_rfc3339(&p.upload.fetched_at).is_ok_and(|t| {
                    now.signed_duration_since(t).num_days() >= i64::from(f.cap_days) - CLEARING_DAYS
                })
        })
        .count();
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    let mut parts = Vec::new();
    if missing > 0 {
        parts.push(format!(
            "{missing} upload{} not in the Portal inbox and not filed (Admin › Portal can fetch {} again)",
            plural(missing),
            if missing == 1 { "it" } else { "them" }
        ));
    }
    if long > 0 {
        parts.push(format!(
            "{long} upload{} waiting in the Portal inbox over {WAITING_DAYS} days",
            plural(long)
        ));
    }
    if clearing > 0 {
        parts.push(format!(
            "{clearing} upload{} cleared from the portal within {CLEARING_DAYS} days unless filed",
            plural(clearing)
        ));
    }
    (!parts.is_empty()).then(|| format!("Portal: {}.", parts.join("; ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(at: Where, fetched_at: &str) -> Pending {
        let u: vf_portal::api::Unfiled = serde_json::from_value(serde_json::json!({
            "uploadId": "u", "clientId": "c", "clientCode": "1001", "clientName": "Smith, John",
            "name": "a.pdf", "originalName": null, "size": 1, "sha256": "x", "note": "",
            "batchId": null, "uploadedAt": fetched_at, "fetchedAt": fetched_at,
            "fetchedBy": "PC1", "from": {"identityId": "i", "name": "M", "email": null, "phone": null}
        }))
        .unwrap();
        Pending {
            upload: u,
            at,
            saved_as: None,
        }
    }

    #[test]
    fn browse_is_told_of_missing_and_long_waiting_uploads() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-20T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let mut f = FiledReport::default();
        assert_eq!(waiting_note(&f, now), None);
        f.pending
            .push(pending(Where::InInbox, "2026-11-19T12:00:00.000Z"));
        assert_eq!(waiting_note(&f, now), None, "a day is not long");
        f.pending
            .push(pending(Where::InInbox, "2026-11-10T12:00:00.000Z"));
        assert_eq!(
            waiting_note(&f, now).unwrap(),
            "Portal: 1 upload waiting in the Portal inbox over 7 days."
        );
        f.pending
            .push(pending(Where::Missing, "2026-11-19T12:00:00.000Z"));
        f.pending
            .push(pending(Where::NotReceived, "2026-11-19T12:00:00.000Z"));
        assert_eq!(
            waiting_note(&f, now).unwrap(),
            "Portal: 2 uploads not in the Portal inbox and not filed (Admin › Portal can fetch them again); 1 upload waiting in the Portal inbox over 7 days."
        );
        // Near the portal's clearing of uploads never filed.
        f.cap_days = 20;
        assert!(
            waiting_note(&f, now)
                .unwrap()
                .ends_with("; 1 upload cleared from the portal within 14 days unless filed.")
        );
    }

    #[test]
    fn the_portal_inbox_is_added_to_intake_once_and_is_not_a_client_folder() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        assert!(register_inbox(root).unwrap());
        assert!(!register_inbox(root).unwrap());
        let (fs, _) =
            filer_intake::config::Settings::load_or_default(&vf_fs::clients::master_dir(root));
        assert_eq!(fs.inboxes, vec![portable(&inbox(root))]);
        // Intake › Setup accepts it inside the root (V34: a `_` folder the root scan leaves out),
        // so Setup can still be saved.
        let issues = filer_intake::config::check_paths(Some(root), &fs.inboxes);
        assert!(
            issues
                .iter()
                .all(|i| i.level != filer_intake::config::Level::Error),
            "{issues:?}"
        );
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
