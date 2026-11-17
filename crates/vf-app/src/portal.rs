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
pub use vf_portal::listing::ListReport;

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
    // Read, changed and written as one under the lock Intake's settings are saved under
    // (a save on another PC in between was lost; QA 2026-11-17).
    vf_fs::lock::with(root, "intake-settings", "Intake's settings", || {
        let (mut fs, savable) = filer_intake::config::Settings::load_or_default(&master);
        // Settings that could not be read are Filer's defaults: saving those would replace
        // the firm's inboxes, client CSV and column map with one inbox.
        if !savable {
            return Err("Intake's settings cannot be read just now, so the Portal inbox was not added to them. Save the portal settings again in a moment.".to_string());
        }
        // Intake's settings are shared by every PC: keep the path every PC reaches (a
        // mapped drive's \\server\share form), and compare in that form (QA 2026-11-09).
        let dir = portable(&dir);
        if fs.inboxes.iter().any(|p| portable(p) == dir) {
            return Ok(false);
        }
        fs.inboxes.push(dir);
        fs.save(&master).map_err(|e| e.to_string())?;
        Ok(true)
    })
}

/// Fetches every waiting upload into the Portal inbox (on a thread).
pub fn fetch(root: &Path, s: &PortalSettings, now: &str) -> FetchReport {
    vf_portal::inbox::fetch(root, &client(s), now)
}

/// Tells the portal what Intake filed and lists what it keeps unfiled (PA-43; on a thread).
pub fn check_filed(root: &Path, s: &PortalSettings) -> FiledReport {
    vf_portal::filed::check(root, &client(s))
}

/// Minutes between sends of the lists of documents in B2 (PA-80).
pub const LIST_MINUTES: u64 = 10;

/// What this PC's catalog knows to be in B2, by client, as the portal lists it (PA-80). Its own
/// read-only connection: the window's is not waited on.
pub fn read_lists(catalog: &Path, device: &str) -> Result<vf_portal::listing::Local, String> {
    use vf_core::db::{Connection, OpenFlags};
    let conn = Connection::open_with_flags(catalog, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    conn.busy_timeout(std::time::Duration::from_secs(10))
        .map_err(|e| e.to_string())?;
    let l = vf_sync::listed::listed(&conn, device).map_err(|e| e.to_string())?;
    Ok(vf_portal::listing::Local {
        head_rev: l.head_rev,
        live: l.live,
        me: String::new(),
        clients: l
            .clients
            .into_iter()
            .map(|c| vf_portal::listing::ClientList {
                id: c.id,
                // Within what the portal takes, so one odd client does not stop its list.
                code: c.code.trim().chars().take(64).collect(),
                name: match c.name.trim() {
                    "" => "(no name)".into(),
                    n => n.chars().take(200).collect(),
                },
                rev: c.rev,
                files: c
                    .docs
                    .into_iter()
                    .map(|d| vf_portal::listing::ListFile {
                        doc_id: d.doc_id,
                        path: d.path,
                        size: d.size.max(0) as u64,
                        tax_year: d.tax_year.filter(|y| (1900..=2200).contains(y)),
                    })
                    .collect(),
            })
            .collect(),
    })
}

/// The cabinet's portal settings as they are now, when the lists of documents in B2 are to
/// be sent. Read from the cabinet each time, not from what a window read a while ago: another
/// PC may just have unticked the setting, or stopped using the portal, and removed the lists.
fn listing_settings(root: &Path) -> Option<PortalSettings> {
    let id = vf_fs::cabinet::read(root).ok()?.cabinet_id;
    vf_portal::settings::load(root, &id).filter(|s| s.b2_list)
}

/// Sends the portal the lists of documents in B2 that differ from what it holds (PA-80; on a
/// thread). Only while the firm has it ticked, only where B2 sync runs (the main computer of a
/// server or cloud cabinet), and only once this PC has exchanged something with the bucket.
pub fn send_lists(root: &Path, catalog: &Path, device: &str) -> ListReport {
    let Some(s) = listing_settings(root) else {
        return ListReport::default();
    };
    // B2 sync was set up on a new bucket since: the portal's lists are of the old one and
    // outrank every new list. They go first, from whichever PC gets there; until they have,
    // nothing is sent (it would not be taken) and the reason is said.
    if old_bucket_mark(root).exists() {
        match client(&s).clear_cabinet_lists() {
            Ok(_) => {}
            Err(e) if e.status() == Some(404) => {}
            Err(e) => {
                return ListReport {
                    problem: Some(format!("{OLD_BUCKET} ({})", e.message())),
                    ..Default::default()
                };
            }
        }
        let _ = std::fs::remove_file(old_bucket_mark(root));
    }
    if !crate::server::runs_here(root) {
        return ListReport::default();
    }
    let mut local = match read_lists(catalog, device) {
        Ok(l) => l,
        Err(e) => {
            return ListReport {
                problem: Some(format!("the catalog could not be read ({e})")),
                ..Default::default()
            };
        }
    };
    if local.head_rev == 0 && local.clients.is_empty() {
        return ListReport::default();
    }
    // Taken out of B2 on a PC of this cabinet (Sync to B2 › Remove…): this PC's catalog may
    // still have it as exchanged until its next push; the cabinet's own list of them says so.
    if let Ok(out) = vf_sync::removed::ids(root)
        && !out.is_empty()
    {
        for c in &mut local.clients {
            c.files.retain(|f| !out.contains(&f.doc_id));
        }
    }
    let p = client(&s);
    local.me = p.actor_name();
    // Still wanted, by this portal (a new key for it changes nothing): asked before each list (the cabinet's file is read at most
    // every two seconds).
    let mut asked: Option<(std::time::Instant, bool)> = None;
    vf_portal::listing::send_while(&p, &local, &mut || {
        if let Some((at, yes)) = asked
            && at.elapsed() < std::time::Duration::from_secs(2)
        {
            return yes;
        }
        let yes = listing_settings(root).is_some_and(|now| now.url == s.url);
        asked = Some((std::time::Instant::now(), yes));
        yes
    })
}

/// Said while the portal still holds the lists of a bucket this cabinet no longer uses.
pub const OLD_BUCKET: &str = "The portal still has the lists of documents of the B2 bucket used before, and they could not be removed, so the new bucket's lists are not sent yet. It is tried again every 10 minutes";

/// Marks that B2 sync was set up on a new bucket (in `_Vibe/keys`, never sent to B2).
fn old_bucket_mark(root: &Path) -> std::path::PathBuf {
    root.join(vf_fs::cabinet::VIBE_DIR)
        .join(vf_fs::cabinet::KEYS_DIR)
        .join("portal-lists-old-bucket")
}

/// B2 sync was set up for this cabinet on a new bucket (its segments counted from 1 again):
/// the lists the portal holds were made from the old bucket and would outrank every new one.
/// Marked in the cabinet; the next send (on any PC) removes them before sending, and says so
/// while it cannot. Nothing when the lists are not being sent.
pub fn forget_lists(root: &Path) {
    if listing_settings(root).is_some() {
        let p = old_bucket_mark(root);
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if std::fs::write(&p, b"").is_err() {
            tracing::warn!("portal: a new bucket could not be marked for the lists");
        }
    }
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
    fn intakes_settings_that_cannot_be_read_are_not_written_over() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let master = vf_fs::clients::master_dir(root);
        std::fs::create_dir_all(&master).unwrap();
        // As a save cut short, or a build newer than this one, leaves it.
        let config = filer_intake::config::Settings::path(&master);
        std::fs::write(&config, "{ \"inboxes\": [\"S:/Scans\"], ").unwrap();
        let e = register_inbox(root).unwrap_err();
        assert!(e.contains("cannot be read"), "{e}");
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            "{ \"inboxes\": [\"S:/Scans\"], ",
            "left as it was"
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
