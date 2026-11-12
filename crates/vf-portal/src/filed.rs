//! Whether what the Portal inbox received was filed (PA-43, Vibe-Portal P74).
//!
//! The Portal inbox is a work queue on the cabinet share, never in B2; the portal keeps each
//! upload until Vibe Files says Intake filed it (or someone dismissed it), so a lost inbox can be
//! fetched again. Filing is read from the audit trail every PC mirrors to
//! `_Vibe/audit/<yyyy-mm>.<device>.jsonl`: an Intake row (move, copy-verify-delete, combine,
//! split) that succeeded, whose source is `_Portal inbox\<the name it was saved as>`, written
//! after it was saved there and before anything else was saved under that name, while the file
//! is no longer in the inbox. (The portal takes no zip files, so Intake's zip import is not
//! one.) Not file contents:
//! OCR, combining and splitting change them. An Undo that puts the file back keeps it unfiled.
//!
//! Fetching again is by hand only (a PC whose records lag could otherwise fetch a filed upload a
//! second time): it is refused while the file is still in the inbox or the trail shows it filed.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::api::{Portal, Unfiled};
use crate::inbox::{self, INBOX, Received};

/// Intake operations that take a file out of an inbox (Filer's op log, `intake_<op>` audit rows),
/// each checked against Intake's real commit (`vf_fs::intake` tests).
const FILING: [&str; 4] = [
    "intake_move",
    "intake_copy_verify_delete",
    "intake_merge",
    "intake_split",
];

/// Where an upload fetched and not filed stands in this cabinet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Where {
    /// In the Portal inbox, waiting for Intake.
    InInbox,
    /// Not in the inbox and no Intake filing of it in the trail (moved or deleted by hand, or the
    /// inbox was lost): fetch it again, or dismiss it.
    Missing,
    /// This cabinet has no record of receiving it (the received list was lost with the share).
    NotReceived,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub upload: Unfiled,
    pub at: Where,
    /// Where it was saved, relative to the cabinet, when known.
    pub saved_as: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FiledReport {
    /// Uploads this run told the portal were filed (their names).
    pub reported: Vec<String>,
    /// Fetched and not filed, oldest fetch first.
    pub pending: Vec<Pending>,
    pub keep_days: u32,
    pub cap_days: u32,
    pub problems: Vec<String>,
}

/// One filing found in the trail: when, and the inbox file name (lower case).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Filing {
    at: DateTime<Utc>,
    name: String,
}

fn when(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// The file name, when `path` (absolute, `\` or `/`, maybe UNC) is directly in a Portal inbox.
fn inbox_file(path: &str) -> Option<String> {
    let mut parts = path.rsplit(['\\', '/']).filter(|p| !p.is_empty());
    let name = parts.next()?;
    let dir = parts.next()?;
    dir.eq_ignore_ascii_case(INBOX).then(|| name.to_lowercase())
}

#[derive(Deserialize)]
struct Row {
    ts: String,
    action: String,
    #[serde(default)]
    detail: serde_json::Value,
}

/// Intake filings out of a Portal inbox in the trail, from the month of `since` on.
fn filings(root: &Path, since: &DateTime<Utc>) -> Vec<Filing> {
    let dir = root.join(vf_fs::cabinet::VIBE_DIR).join("audit");
    let from = since.format("%Y-%m").to_string();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries.filter_map(Result::ok) {
        let name = e.file_name().to_string_lossy().into_owned();
        // `<yyyy-mm>.<device>.jsonl`; months before `since` cannot hold its filing.
        if !name.ends_with(".jsonl") || name.get(..7).is_none_or(|m| m < from.as_str()) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(e.path()) else {
            continue;
        };
        for line in text.lines() {
            // Cheap first: most rows are not Intake's, and few come from the Portal inbox.
            if !line.contains("\"intake_")
                || !line
                    .to_ascii_lowercase()
                    .contains(&INBOX.to_ascii_lowercase())
            {
                continue;
            }
            let Ok(r) = serde_json::from_str::<Row>(line) else {
                continue;
            };
            if !FILING.contains(&r.action.as_str())
                || r.detail.get("ok").and_then(serde_json::Value::as_bool) != Some(true)
            {
                continue;
            }
            let (Some(at), Some(name)) = (
                when(&r.ts),
                r.detail
                    .get("src")
                    .and_then(serde_json::Value::as_str)
                    .and_then(inbox_file),
            ) else {
                continue;
            };
            out.push(Filing { at, name });
        }
    }
    out
}

/// What the received list says: the last arrival of each upload (a fetch again adds a row), and
/// every arrival under each inbox name (lower case).
struct Arrivals {
    by_upload: HashMap<String, Received>,
    by_name: HashMap<String, Vec<DateTime<Utc>>>,
}

fn arrivals(root: &Path) -> Arrivals {
    let mut a = Arrivals {
        by_upload: HashMap::new(),
        by_name: HashMap::new(),
    };
    for r in inbox::all_received(root) {
        if let (Some(name), Some(at)) = (inbox_file(&r.saved_as), when(&r.at)) {
            a.by_name.entry(name).or_default().push(at);
        }
        a.by_upload.insert(r.upload_id.clone(), r);
    }
    a
}

/// Whether `r` was filed: out of the inbox, and an Intake filing of its name after it was saved
/// and before the next upload was saved under the same name (that one's filing is its own).
fn was_filed(root: &Path, r: &Received, found: &[Filing], all: &Arrivals) -> bool {
    if root.join(&r.saved_as).exists() {
        return false;
    }
    let Some(saved) = when(&r.at) else {
        return false;
    };
    let Some(name) = inbox_file(&r.saved_as) else {
        return false;
    };
    let next = all
        .by_name
        .get(&name)
        .and_then(|v| v.iter().filter(|t| **t > saved).min().copied());
    found
        .iter()
        .any(|f| f.name == name && f.at >= saved && next.is_none_or(|n| f.at < n))
}

/// Uploads downloaded and claimed but not moved into the inbox yet (the next fetch moves them).
fn being_moved(root: &Path) -> HashSet<String> {
    let Ok(entries) = std::fs::read_dir(inbox::tmp_dir(root)) else {
        return HashSet::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| std::fs::read(p).ok())
        .filter_map(|b| serde_json::from_slice::<crate::api::Upload>(&b).ok())
        .map(|u| u.upload_id)
        .collect()
}

/// What each upload fetched and not filed is: filed (to report), or where it stands.
fn sort_out(root: &Path, list: Vec<Unfiled>) -> (Vec<(String, String)>, Vec<Pending>) {
    let all = arrivals(root);
    let received = &all.by_upload;
    let moving = being_moved(root);
    let gone: Vec<&Received> = list
        .iter()
        .filter_map(|u| received.get(&u.upload.upload_id))
        .filter(|r| !root.join(&r.saved_as).exists())
        .collect();
    // The trail is read only when something left the inbox, from its earliest arrival.
    let found = gone
        .iter()
        .filter_map(|r| when(&r.at))
        .min()
        .map(|since| filings(root, &since))
        .unwrap_or_default();
    let mut filed = Vec::new();
    let mut pending = Vec::new();
    for u in list {
        if moving.contains(&u.upload.upload_id) {
            continue;
        }
        let r = received.get(&u.upload.upload_id);
        let at = match r {
            Some(r) if was_filed(root, r, &found, &all) => {
                filed.push((u.upload.upload_id.clone(), u.upload.name.clone()));
                continue;
            }
            Some(r) if root.join(&r.saved_as).exists() => Where::InInbox,
            Some(_) => Where::Missing,
            None => Where::NotReceived,
        };
        pending.push(Pending {
            saved_as: r.map(|r| r.saved_as.clone()),
            upload: u,
            at,
        });
    }
    (filed, pending)
}

/// Reads the portal's list of uploads fetched and not filed, tells it which Intake filed, and
/// says where the rest stand. Runs on a thread.
pub fn check(root: &Path, portal: &Portal) -> FiledReport {
    let mut report = FiledReport::default();
    let list = match portal.unfiled() {
        Ok(l) => l,
        Err(e) => {
            report.problems.push(e.message());
            return report;
        }
    };
    report.keep_days = list.keep_days;
    report.cap_days = list.cap_days;
    let (filed, pending) = sort_out(root, list.uploads);
    for (id, name) in filed {
        match portal.mark_filed(&id) {
            Ok(()) => report.reported.push(name),
            Err(e) => report.problems.push(format!("{name}: {}", e.message())),
        }
    }
    report.pending = pending;
    report
}

/// Fetches an upload into the Portal inbox again, after the inbox lost it. Refused while it is
/// still in the inbox, or when the trail shows Intake filed it (then the portal is told).
pub fn fetch_again(
    root: &Path,
    portal: &Portal,
    u: &Unfiled,
    now: &str,
) -> Result<Received, String> {
    let tmp = inbox::tmp_dir(root);
    std::fs::create_dir_all(&tmp)
        .and_then(|()| std::fs::create_dir_all(inbox::inbox(root)))
        .map_err(|e| format!("The Portal inbox cannot be written: {e}"))?;
    let Some(_lock) = inbox::take_lock(&tmp, &portal.actor_name()) else {
        return Err("Another PC is fetching uploads: try again in a minute.".into());
    };
    let all = arrivals(root);
    if let Some(r) = all.by_upload.get(&u.upload.upload_id) {
        if root.join(&r.saved_as).exists() {
            return Err(format!(
                "It is still in the Portal inbox as {}.",
                r.saved_as.rsplit('/').next().unwrap_or(&r.saved_as)
            ));
        }
        let since = when(&r.at).unwrap_or_default();
        if was_filed(root, r, &filings(root, &since), &all) {
            let _ = portal.mark_filed(&u.upload.upload_id);
            return Err("Intake filed it already: it is not fetched again.".into());
        }
    }
    // No note beside it: the next fetch run must not take it for one of its own claims.
    let part = tmp.join(format!(
        "{}.again-{}.part",
        u.upload.upload_id,
        inbox::random_tag()
    ));
    portal
        .download_upload(&u.upload, &part)
        .map_err(|e| e.message())?;
    let name = inbox::inbox_name(&u.upload.client_name, &u.upload.client_code, &u.upload.name);
    let dest = inbox::move_into(&part, &inbox::inbox(root), &name).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        format!("It could not be put in the Portal inbox: {e}")
    })?;
    let row = inbox::received_row(root, &dest, &u.upload, now);
    inbox::append_received(root, std::slice::from_ref(&row))
        .map_err(|e| format!("The received list could not be written: {e}"))?;
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{Upload, UploadFrom};

    fn upload(id: &str, name: &str) -> Unfiled {
        Unfiled {
            upload: Upload {
                upload_id: id.into(),
                client_id: "c1".into(),
                client_code: "1001".into(),
                client_name: "Smith, John".into(),
                name: name.into(),
                original_name: None,
                size: 1,
                sha256: "x".into(),
                note: String::new(),
                batch_id: None,
                uploaded_at: "2026-11-10T10:00:00.000Z".into(),
                from: UploadFrom {
                    identity_id: "i1".into(),
                    name: "Mary".into(),
                    email: None,
                    phone: None,
                },
            },
            fetched_at: "2026-11-10T10:05:00.000Z".into(),
            fetched_by: Some("PC1\\kurt".into()),
        }
    }

    fn received(root: &Path, id: &str, saved: &str, at: &str) {
        inbox::append_received(
            root,
            &[Received {
                at: at.into(),
                upload_id: id.into(),
                client_id: "c1".into(),
                client: "Smith, John".into(),
                name: "a.pdf".into(),
                saved_as: saved.into(),
                note: String::new(),
                from: "Mary".into(),
                size: 1,
            }],
        )
        .unwrap();
    }

    /// An Intake audit row as a PC mirrors it, from the share's UNC path.
    fn trail(root: &Path, device: &str, ts: &str, action: &str, src: &str, ok: bool) {
        let dir = root.join("_Vibe").join("audit");
        std::fs::create_dir_all(&dir).unwrap();
        let line = serde_json::json!({
            "seq": 1, "ts": ts, "user": null, "device": device, "action": action, "doc": null,
            "detail": { "filer_batch": "b", "src": src, "dst": "Smith, John (1001)/x.pdf", "ok": ok },
        });
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(format!("{}.{device}.jsonl", &ts[..7])))
            .unwrap();
        writeln!(f, "{line}").unwrap();
    }

    fn put(root: &Path, rel: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"x").unwrap();
    }

    const SAVED: &str = "_Portal inbox/Smith, John_1001_a.pdf";
    const SRC: &str = r"\\server\Cabinet\_Portal inbox\Smith, John_1001_a.pdf";

    #[test]
    fn an_upload_still_in_the_inbox_is_waiting() {
        let t = tempfile::tempdir().unwrap();
        received(t.path(), "u1", SAVED, "2026-11-10T10:05:00Z");
        put(t.path(), SAVED);
        let (filed, pending) = sort_out(t.path(), vec![upload("u1", "a.pdf")]);
        assert!(filed.is_empty());
        assert_eq!(pending[0].at, Where::InInbox);
    }

    #[test]
    fn filed_by_any_pc_from_the_trail_however_it_left_the_inbox() {
        for action in FILING {
            let t = tempfile::tempdir().unwrap();
            received(t.path(), "u1", SAVED, "2026-11-10T10:05:00Z");
            // Another PC filed it, from the share's path in another case.
            trail(
                t.path(),
                "pc-2",
                "2026-11-10T11:00:00.123456Z",
                action,
                &SRC.replace("_Portal inbox", "_PORTAL INBOX"),
                true,
            );
            let (filed, pending) = sort_out(t.path(), vec![upload("u1", "a.pdf")]);
            assert_eq!(filed, vec![("u1".into(), "a.pdf".into())], "{action}");
            assert!(pending.is_empty());
        }
    }

    #[test]
    fn a_failed_or_skipped_row_or_another_folder_is_not_a_filing() {
        let t = tempfile::tempdir().unwrap();
        received(t.path(), "u1", SAVED, "2026-11-10T10:05:00Z");
        trail(
            t.path(),
            "pc-1",
            "2026-11-10T11:00:00Z",
            "intake_move",
            SRC,
            false,
        );
        trail(
            t.path(),
            "pc-1",
            "2026-11-10T11:00:00Z",
            "intake_skip",
            SRC,
            true,
        );
        trail(
            t.path(),
            "pc-1",
            "2026-11-10T11:00:00Z",
            "intake_move",
            r"\\server\Scans\Smith, John_1001_a.pdf",
            true,
        );
        let (filed, pending) = sort_out(t.path(), vec![upload("u1", "a.pdf")]);
        assert!(filed.is_empty());
        assert_eq!(pending[0].at, Where::Missing);
    }

    /// The same inbox name is free again once filed: an earlier upload's filing is not this one's.
    #[test]
    fn a_filing_before_the_upload_arrived_is_another_uploads() {
        let t = tempfile::tempdir().unwrap();
        trail(
            t.path(),
            "pc-1",
            "2026-11-10T09:00:00Z",
            "intake_move",
            SRC,
            true,
        );
        received(t.path(), "u2", SAVED, "2026-11-10T10:05:00Z");
        let (filed, pending) = sort_out(t.path(), vec![upload("u2", "a.pdf")]);
        assert!(filed.is_empty());
        assert_eq!(pending[0].at, Where::Missing);
        // Even when the trail's earlier month holds it.
        let t = tempfile::tempdir().unwrap();
        trail(
            t.path(),
            "pc-1",
            "2026-10-31T23:00:00Z",
            "intake_move",
            SRC,
            true,
        );
        received(t.path(), "u2", SAVED, "2026-11-01T00:05:00Z");
        assert!(sort_out(t.path(), vec![upload("u2", "a.pdf")]).0.is_empty());
    }

    /// A lost upload is not filed by the filing of a later one saved under its free name.
    #[test]
    fn a_later_uploads_filing_under_the_same_name_is_not_this_ones() {
        let t = tempfile::tempdir().unwrap();
        // u1 saved as X, then lost outside Intake; u2 saved as X later, and filed.
        received(t.path(), "u1", SAVED, "2026-11-10T10:05:00Z");
        received(t.path(), "u2", SAVED, "2026-11-11T09:00:00Z");
        trail(
            t.path(),
            "pc-1",
            "2026-11-11T10:00:00Z",
            "intake_move",
            SRC,
            true,
        );
        let (filed, pending) =
            sort_out(t.path(), vec![upload("u1", "a.pdf"), upload("u2", "a.pdf")]);
        assert_eq!(filed, vec![("u2".into(), "a.pdf".into())]);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].upload.upload.upload_id, "u1");
        assert_eq!(pending[0].at, Where::Missing);
    }

    /// Undo put it back: the file is in the inbox again, so it is not filed.
    #[test]
    fn undone_filing_is_not_filed() {
        let t = tempfile::tempdir().unwrap();
        received(t.path(), "u1", SAVED, "2026-11-10T10:05:00Z");
        trail(
            t.path(),
            "pc-1",
            "2026-11-10T11:00:00Z",
            "intake_move",
            SRC,
            true,
        );
        put(t.path(), SAVED);
        let (filed, pending) = sort_out(t.path(), vec![upload("u1", "a.pdf")]);
        assert!(filed.is_empty());
        assert_eq!(pending[0].at, Where::InInbox);
    }

    #[test]
    fn fetched_again_counts_from_the_last_arrival() {
        let t = tempfile::tempdir().unwrap();
        received(t.path(), "u1", SAVED, "2026-11-10T10:05:00Z");
        trail(
            t.path(),
            "pc-1",
            "2026-11-10T11:00:00Z",
            "intake_move",
            SRC,
            true,
        );
        // Fetched again later as (2): its own filing is needed.
        let again = "_Portal inbox/Smith, John_1001_a (2).pdf";
        received(t.path(), "u1", again, "2026-11-12T08:00:00Z");
        let (filed, pending) = sort_out(t.path(), vec![upload("u1", "a.pdf")]);
        assert!(filed.is_empty());
        assert_eq!(pending[0].saved_as.as_deref(), Some(again));
        assert_eq!(pending[0].at, Where::Missing);
    }

    #[test]
    fn not_received_here_and_being_moved() {
        let t = tempfile::tempdir().unwrap();
        // Downloaded and claimed, moved by the next fetch: not listed.
        let tmp = inbox::tmp_dir(t.path());
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(
            tmp.join("u2.PC1-ab.json"),
            serde_json::to_vec(&upload("u2", "b.pdf").upload).unwrap(),
        )
        .unwrap();
        let (filed, pending) =
            sort_out(t.path(), vec![upload("u1", "a.pdf"), upload("u2", "b.pdf")]);
        assert!(filed.is_empty());
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].at, Where::NotReceived);
        assert_eq!(pending[0].saved_as, None);
    }

    #[test]
    fn inbox_files_are_recognised_in_any_path_form() {
        assert_eq!(inbox_file(SRC).as_deref(), Some("smith, john_1001_a.pdf"));
        assert_eq!(
            inbox_file(r"D:\Cabinet\_portal inbox\X.PDF").as_deref(),
            Some("x.pdf")
        );
        assert_eq!(inbox_file(SAVED).as_deref(), Some("smith, john_1001_a.pdf"));
        assert_eq!(inbox_file(r"D:\Cabinet\_Portal inbox\sub\x.pdf"), None);
        assert_eq!(inbox_file("x.pdf"), None);
    }
}
