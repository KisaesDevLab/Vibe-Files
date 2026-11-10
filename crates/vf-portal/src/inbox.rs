//! Client uploads into the cabinet's Portal inbox (PA-37).
//!
//! One PC fetches at a time (a lock file in `_Vibe/tmp/portal`, taken over after 15 minutes).
//! Each upload is downloaded into a file of this PC's own (`<upload>.<pc>-<random>.part`,
//! created new) with a note beside it naming the upload, checked against the portal's sha256
//! as it arrives, then claimed (`/fetched`). The claimed file is moved into `_Portal inbox` as
//! `<Client name>_<Client ID>_<file name>` without ever replacing a file, and logged in
//! `_Vibe/portal/received.jsonl`. A run that stops between the download and the move (a crash,
//! a lost answer) is finished by the next run from the note: the portal says who claimed each
//! upload, so a PC knows its own claim (QA 2026-11-09).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::api::{Claim, Portal, Upload};

/// The Portal inbox at the cabinet root (a `_` folder: never taken for a client folder).
pub const INBOX: &str = "_Portal inbox";
/// A fetch lock older than this was left by a PC that stopped.
pub const LOCK_STALE: Duration = Duration::from_secs(15 * 60);
const LOCK: &str = "fetch.lock";
/// Longest inbox file name (the share path comes on top).
const MAX_NAME: usize = 150;

pub fn inbox(root: &Path) -> PathBuf {
    root.join(INBOX)
}

pub fn tmp_dir(root: &Path) -> PathBuf {
    root.join(vf_fs::cabinet::VIBE_DIR)
        .join("tmp")
        .join("portal")
}

/// What was received, newest last, shared by every PC.
pub fn received_log(root: &Path) -> PathBuf {
    root.join(vf_fs::cabinet::VIBE_DIR)
        .join("portal")
        .join("received.jsonl")
}

/// A file name Windows accepts: no reserved characters, no trailing dots or spaces.
fn safe_part(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '-',
            c if c.is_control() => '-',
            c => c,
        })
        .collect();
    t.trim().trim_end_matches(['.', ' ']).to_string()
}

/// Windows device names that cannot be a file name, with any extension.
fn reserved(stem: &str) -> bool {
    let u = stem.trim().to_ascii_uppercase();
    matches!(u.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((u.starts_with("COM") || u.starts_with("LPT"))
            && u.len() == 4
            && u.as_bytes()[3].is_ascii_digit())
}

/// `<Client name>_<Client ID>_<file name>` (Vibe Portal P3), shortened to fit.
pub fn inbox_name(client_name: &str, client_code: &str, name: &str) -> String {
    let file = safe_part(name);
    let (stem, ext) = match file.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && e.len() <= 8 => (s.to_string(), format!(".{e}")),
        _ => (file.clone(), String::new()),
    };
    let mut head = safe_part(client_name);
    let code = safe_part(client_code);
    if !code.is_empty() {
        head = format!("{head}_{code}");
    }
    let head: String = head.chars().take(60).collect();
    let room = MAX_NAME.saturating_sub(head.chars().count() + ext.chars().count() + 1);
    let stem: String = stem.chars().take(room.max(10)).collect();
    let name = if head.is_empty() {
        format!("{stem}{ext}")
    } else {
        format!("{head}_{stem}{ext}")
    };
    let base = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s);
    if name.is_empty() || reserved(base) {
        format!("Client_{name}")
    } else {
        name
    }
}

/// Marks a client's file as downloaded from the internet (Windows' Zone.Identifier, zone 3), as a
/// browser would: Office then opens it in Protected View and blocks its macros. Best effort: a
/// cabinet on a drive without alternate data streams simply gets no mark.
pub fn mark_from_internet(path: &Path) {
    #[cfg(windows)]
    {
        let mut ads = path.as_os_str().to_owned();
        ads.push(":Zone.Identifier");
        let _ = std::fs::write(ads, "[ZoneTransfer]\r\nZoneId=3\r\n");
    }
    #[cfg(not(windows))]
    let _ = path;
}

/// Moves `src` into `dir` as `name`, or `name (2)`… — never replacing a file.
fn move_into(src: &Path, dir: &Path, name: &str) -> std::io::Result<PathBuf> {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (name.to_string(), String::new()),
    };
    for n in 1..10_000u32 {
        let dest = if n == 1 {
            dir.join(name)
        } else {
            dir.join(format!("{stem} ({n}){ext}"))
        };
        match filer_intake::commit::move_no_replace(src, &dest) {
            Ok(()) => {
                mark_from_internet(&dest);
                return Ok(dest);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists || dest.exists() => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("no free name in the Portal inbox"))
}

/// One received upload, for the log and the audit trail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Received {
    pub at: String,
    pub upload_id: String,
    pub client_id: String,
    pub client: String,
    /// The name the client gave it.
    pub name: String,
    /// Where it went, relative to the cabinet.
    pub saved_as: String,
    pub note: String,
    pub from: String,
    pub size: u64,
}

#[derive(Debug, Default)]
pub struct FetchReport {
    pub received: Vec<Received>,
    pub problems: Vec<String>,
    /// Cabinet documents on the portal now (Browse's marks); `None` when not read.
    pub on_portal: Option<HashSet<String>>,
    /// Another PC was fetching: nothing was fetched here this time.
    pub busy: bool,
}

/// The fetch lock; removed when dropped.
struct Lock(PathBuf);

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn take_lock(tmp: &Path, who: &str) -> Option<Lock> {
    let p = tmp.join(LOCK);
    for _ in 0..2 {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&p)
        {
            Ok(mut f) => {
                use std::io::Write;
                let _ = f.write_all(who.as_bytes());
                return Some(Lock(p));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let stale = std::fs::metadata(&p)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| SystemTime::now().duration_since(t).ok())
                    .is_some_and(|age| age > LOCK_STALE);
                if !stale {
                    return None;
                }
                let _ = std::fs::remove_file(&p);
            }
            Err(_) => return None,
        }
    }
    None
}

fn random_tag() -> String {
    let mut b = [0u8; 4];
    let _ = getrandom::fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn from_label(u: &Upload) -> String {
    if u.from.name.trim().is_empty() {
        u.from
            .email
            .clone()
            .or(u.from.phone.clone())
            .unwrap_or_default()
    } else {
        u.from.name.clone()
    }
}

/// Moves a claimed download into the inbox and logs it.
fn finish(root: &Path, part: &Path, meta: &Path, u: &Upload, now: &str, report: &mut FetchReport) {
    let dir = inbox(root);
    match move_into(part, &dir, &inbox_name(&u.client_name, &u.client_code, &u.name)) {
        Ok(dest) => {
            let _ = std::fs::remove_file(meta);
            let rel = dest
                .strip_prefix(root)
                .unwrap_or(&dest)
                .to_string_lossy()
                .replace('\\', "/");
            let row = Received {
                at: now.to_string(),
                upload_id: u.upload_id.clone(),
                client_id: u.client_id.clone(),
                client: u.client_name.clone(),
                name: u.original_name.clone().unwrap_or_else(|| u.name.clone()),
                saved_as: rel,
                note: u.note.clone(),
                from: from_label(u),
                size: u.size,
            };
            if let Err(e) = append_received(root, std::slice::from_ref(&row)) {
                report.problems.push(format!("The received list could not be written: {e}"));
            }
            report.received.push(row);
        }
        // Claimed already: kept with its note, and moved by the next run.
        Err(e) => report.problems.push(format!(
            "{} was received but could not be moved to the Portal inbox yet ({e}); it will be tried again.",
            u.name
        )),
    }
}

/// Claims a download; `true` when it is ours to move.
fn claim(portal: &Portal, part: &Path, meta: &Path, u: &Upload, report: &mut FetchReport) -> bool {
    match portal.mark_fetched(&u.upload_id) {
        Ok(Claim::Ours) => true,
        Ok(Claim::Other) => {
            // Another PC has it.
            let _ = std::fs::remove_file(part);
            let _ = std::fs::remove_file(meta);
            false
        }
        Err(e) => {
            // Unknown whether the claim reached the portal: kept for the next run.
            report.problems.push(format!("{}: {}", u.name, e.message()));
            false
        }
    }
}

/// Fetches every waiting upload into the Portal inbox. Runs on a thread.
pub fn fetch(root: &Path, portal: &Portal, now: &str) -> FetchReport {
    let mut report = FetchReport::default();
    match portal.all_files() {
        Ok(files) => {
            report.on_portal = Some(files.into_iter().map(|f| f.vf_document_id).collect());
        }
        Err(e) => {
            report.problems.push(e.message());
            return report;
        }
    }
    let tmp = tmp_dir(root);
    if let Err(e) =
        std::fs::create_dir_all(&tmp).and_then(|()| std::fs::create_dir_all(inbox(root)))
    {
        report
            .problems
            .push(format!("The Portal inbox cannot be written: {e}"));
        return report;
    }
    let Some(_lock) = take_lock(&tmp, &portal.actor_name()) else {
        report.busy = true;
        return report;
    };

    // Finish what an earlier run downloaded but did not move.
    let mut done: HashSet<String> = HashSet::new();
    if let Ok(entries) = std::fs::read_dir(&tmp) {
        let metas: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        for meta in metas {
            let part = meta.with_extension("part");
            let u: Option<Upload> = std::fs::read(&meta)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok());
            let Some(u) = u else {
                let _ = std::fs::remove_file(&meta);
                continue;
            };
            let whole = std::fs::metadata(&part).is_ok_and(|m| m.len() == u.size)
                && crate::api::file_sha256(&part).is_ok_and(|h| h == u.sha256);
            if !whole {
                let _ = std::fs::remove_file(&part);
                let _ = std::fs::remove_file(&meta);
                continue;
            }
            done.insert(u.upload_id.clone());
            if claim(portal, &part, &meta, &u, &mut report) {
                finish(root, &part, &meta, &u, now, &mut report);
            }
        }
    }
    // Downloads cut off before their note was written.
    if let Ok(entries) = std::fs::read_dir(&tmp) {
        for e in entries.filter_map(Result::ok) {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "part") && !p.with_extension("json").exists() {
                let old = e
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| SystemTime::now().duration_since(t).ok())
                    .is_some_and(|age| age > LOCK_STALE);
                if old {
                    let _ = std::fs::remove_file(&p);
                }
            }
        }
    }

    let uploads = match portal.uploads() {
        Ok(u) => u,
        Err(e) => {
            report.problems.push(e.message());
            return report;
        }
    };
    let tag = format!("{}-{}", safe_part(&portal.pc()), random_tag());
    for u in uploads.into_iter().filter(|u| !done.contains(&u.upload_id)) {
        let stem = format!("{}.{tag}", safe_part(&u.upload_id));
        let part = tmp.join(format!("{stem}.part"));
        let meta = tmp.join(format!("{stem}.json"));
        if let Err(e) = portal.download_upload(&u, &part) {
            report.problems.push(format!(
                "{} (from {}): {}",
                u.name,
                u.client_name,
                e.message()
            ));
            continue;
        }
        if let Err(e) = std::fs::write(&meta, serde_json::to_vec(&u).expect("serializes")) {
            let _ = std::fs::remove_file(&part);
            report.problems.push(format!("{}: {e}", u.name));
            continue;
        }
        if claim(portal, &part, &meta, &u, &mut report) {
            finish(root, &part, &meta, &u, now, &mut report);
        }
    }
    report
}

pub fn append_received(root: &Path, rows: &[Received]) -> std::io::Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let p = received_log(root);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut text = String::new();
    for r in rows {
        text.push_str(&serde_json::to_string(r).expect("serializes"));
        text.push('\n');
    }
    use std::io::Write;
    // One write per call in append mode: rows from several PCs do not interleave.
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)?
        .write_all(text.as_bytes())
}

/// The newest `n` received uploads, newest first.
pub fn recent_received(root: &Path, n: usize) -> Vec<Received> {
    let Ok(text) = std::fs::read_to_string(received_log(root)) else {
        return Vec::new();
    };
    let mut rows: Vec<Received> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    rows.reverse();
    rows.truncate(n);
    rows
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    #[test]
    fn fetched_files_are_marked_from_the_internet() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("in.tmp");
        std::fs::write(&src, b"x").unwrap();
        let dest = super::move_into(&src, dir.path(), "Smith_1001_W-2.doc").unwrap();
        let mut ads = dest.as_os_str().to_owned();
        ads.push(":Zone.Identifier");
        let mark = std::fs::read_to_string(ads).unwrap();
        assert!(mark.contains("ZoneId=3"), "{mark}");
    }

    use super::*;

    #[test]
    fn inbox_names_carry_the_client_and_are_safe() {
        assert_eq!(
            inbox_name("Smith, John", "1001", "2025 W-2.pdf"),
            "Smith, John_1001_2025 W-2.pdf"
        );
        assert_eq!(inbox_name("A/B: C", "", "x?.pdf."), "A-B- C_x-.pdf");
        assert_eq!(inbox_name("", "", "scan.jpg"), "scan.jpg");
        assert_eq!(inbox_name(".", "", "CON.pdf"), "Client_CON.pdf");
        assert_eq!(inbox_name("", "", "com1.txt"), "Client_com1.txt");
        let long = inbox_name(&"N".repeat(300), "1", &format!("{}.pdf", "f".repeat(300)));
        assert!(
            long.chars().count() <= MAX_NAME && long.ends_with(".pdf"),
            "{long}"
        );
    }

    #[test]
    fn moving_into_the_inbox_never_replaces_a_file() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("in");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.pdf"), b"first").unwrap();
        let src = t.path().join("x.part");
        std::fs::write(&src, b"second").unwrap();
        let dest = move_into(&src, &dir, "a.pdf").unwrap();
        assert_eq!(dest, dir.join("a (2).pdf"));
        assert_eq!(std::fs::read(dir.join("a.pdf")).unwrap(), b"first");
        assert_eq!(std::fs::read(dest).unwrap(), b"second");
    }

    #[test]
    fn one_pc_holds_the_fetch_lock_until_it_goes_stale() {
        let t = tempfile::tempdir().unwrap();
        let a = take_lock(t.path(), "PC1\\a").expect("first");
        assert!(take_lock(t.path(), "PC2\\b").is_none());
        drop(a);
        assert!(take_lock(t.path(), "PC2\\b").is_some());
        // A stale lock (a PC that stopped) is taken over.
        let p = t.path().join(LOCK);
        std::fs::write(&p, b"PC9\\x").unwrap();
        let f = std::fs::File::options().write(true).open(&p).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(16 * 60))
            .unwrap();
        drop(f);
        assert!(take_lock(t.path(), "PC2\\b").is_some());
    }

    #[test]
    fn received_uploads_are_logged_newest_first() {
        let t = tempfile::tempdir().unwrap();
        let r = |n: &str| Received {
            at: "2026-11-09T15:00:00Z".into(),
            upload_id: n.into(),
            client_id: "c".into(),
            client: "Smith".into(),
            name: n.into(),
            saved_as: format!("{INBOX}/{n}"),
            note: String::new(),
            from: "Mary".into(),
            size: 1,
        };
        append_received(t.path(), &[r("1"), r("2")]).unwrap();
        append_received(t.path(), &[r("3")]).unwrap();
        let got: Vec<String> = recent_received(t.path(), 2)
            .into_iter()
            .map(|x| x.name)
            .collect();
        assert_eq!(got, ["3", "2"]);
    }
}
