//! Client uploads into the cabinet's Portal inbox (PA-37).
//!
//! Each waiting upload is downloaded into `_Vibe/tmp/portal/`, checked against the portal's
//! sha256, marked fetched (one PC wins; the others drop their copy), then moved into
//! `_Portal inbox` as `<Client name>_<Client ID>_<file name>`, so Intake's usual reading finds
//! the client. What arrived is logged in `_Vibe/portal/received.jsonl` for every PC.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::api::Portal;

/// The Portal inbox at the cabinet root (a `_` folder: never taken for a client folder).
pub const INBOX: &str = "_Portal inbox";

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
    let stem: String = stem.chars().take(120).collect();
    let head: String = head.chars().take(80).collect();
    if head.is_empty() {
        format!("{stem}{ext}")
    } else {
        format!("{head}_{stem}{ext}")
    }
}

/// `dir/name`, or `name (2)`, `name (3)`… when taken.
pub fn free_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (name.to_string(), String::new()),
    };
    (2..10_000)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
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
    let uploads = match portal.uploads() {
        Ok(u) => u,
        Err(e) => {
            report.problems.push(e.message());
            return report;
        }
    };
    if uploads.is_empty() {
        return report;
    }
    let tmp = tmp_dir(root);
    let dir = inbox(root);
    if let Err(e) = std::fs::create_dir_all(&tmp).and_then(|()| std::fs::create_dir_all(&dir)) {
        report
            .problems
            .push(format!("The Portal inbox cannot be written: {e}"));
        return report;
    }
    for u in uploads {
        let part = tmp.join(format!("{}.part", safe_part(&u.upload_id)));
        if let Err(e) = portal.download_upload(&u, &part) {
            report.problems.push(format!(
                "{} (from {}): {}",
                u.name,
                u.client_name,
                e.message()
            ));
            continue;
        }
        match portal.mark_fetched(&u.upload_id) {
            Ok(true) => {}
            Ok(false) => {
                // Another PC fetched it first: it has it.
                let _ = std::fs::remove_file(&part);
                continue;
            }
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                report.problems.push(format!("{}: {}", u.name, e.message()));
                continue;
            }
        }
        let dest = free_path(&dir, &inbox_name(&u.client_name, &u.client_code, &u.name));
        let saved = match std::fs::rename(&part, &dest) {
            Ok(()) => dest,
            Err(e) => {
                // Marked fetched already: keep it where it is and say where.
                report.problems.push(format!(
                    "{} was received but could not be moved to the Portal inbox ({e}); it is in {}",
                    u.name,
                    part.display()
                ));
                part
            }
        };
        let rel = saved
            .strip_prefix(root)
            .unwrap_or(&saved)
            .to_string_lossy()
            .replace('\\', "/");
        let from = if u.from.name.trim().is_empty() {
            u.from
                .email
                .clone()
                .or(u.from.phone.clone())
                .unwrap_or_default()
        } else {
            u.from.name.clone()
        };
        report.received.push(Received {
            at: now.to_string(),
            upload_id: u.upload_id.clone(),
            client_id: u.client_id.clone(),
            client: u.client_name.clone(),
            name: u.original_name.clone().unwrap_or_else(|| u.name.clone()),
            saved_as: rel,
            note: u.note.clone(),
            from,
            size: u.size,
        });
    }
    append_received(root, &report.received);
    report
}

pub fn append_received(root: &Path, rows: &[Received]) {
    if rows.is_empty() {
        return;
    }
    let p = received_log(root);
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let mut text = String::new();
    for r in rows {
        text.push_str(&serde_json::to_string(r).expect("serializes"));
        text.push('\n');
    }
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
    {
        let _ = f.write_all(text.as_bytes());
    }
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
    use super::*;

    #[test]
    fn inbox_names_carry_the_client_and_are_safe() {
        assert_eq!(
            inbox_name("Smith, John", "1001", "2025 W-2.pdf"),
            "Smith, John_1001_2025 W-2.pdf"
        );
        assert_eq!(inbox_name("A/B: C", "", "x?.pdf."), "A-B- C_x-.pdf");
        assert_eq!(inbox_name("", "", "scan.jpg"), "scan.jpg");
        let long = inbox_name(&"N".repeat(300), "1", &format!("{}.pdf", "f".repeat(300)));
        assert!(long.chars().count() < 220 && long.ends_with(".pdf"));
    }

    #[test]
    fn a_taken_name_gets_a_number() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("a.pdf"), b"x").unwrap();
        std::fs::write(t.path().join("a (2).pdf"), b"x").unwrap();
        assert_eq!(free_path(t.path(), "a.pdf"), t.path().join("a (3).pdf"));
        assert_eq!(free_path(t.path(), "b.pdf"), t.path().join("b.pdf"));
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
        append_received(t.path(), &[r("1"), r("2")]);
        append_received(t.path(), &[r("3")]);
        let got: Vec<String> = recent_received(t.path(), 2)
            .into_iter()
            .map(|x| x.name)
            .collect();
        assert_eq!(got, ["3", "2"]);
    }
}
