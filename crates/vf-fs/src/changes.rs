//! Changes between servers (PA-36, Q-70/Q-71). Several Remote Desktop servers share one cabinet
//! on a file share, each with its own catalog. Every change one server makes to the cabinet
//! (a Transaction's file ops, an undo) is noted in `_Vibe/changes/<writer>.log` as the client
//! folders it touched; the other servers read the logs they have not seen yet and re-index
//! just those folders (`rebuild::rebuild_folders`: files, moves, deletions, and the metadata
//! in each folder's `docmeta.jsonl`).
//!
//! Off until [`set_writer`] is called (a single PC never writes these logs).

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use vf_core::tx::FileOp;

use crate::cabinet::VIBE_DIR;

/// Folder of the change logs, in `_Vibe`.
pub const DIR: &str = "changes";

/// The cabinet and this catalog's writer id, once noting is on.
static WRITER: Mutex<Option<(PathBuf, String)>> = Mutex::new(None);

/// Notes this catalog's changes to `root` from now on, as `writer` (one per server catalog).
pub fn set_writer(root: &Path, writer: &str) {
    if let Ok(mut w) = WRITER.lock() {
        *w = Some((root.to_path_buf(), safe_id(writer)));
    }
}

/// Stops noting (a cabinet closed).
pub fn clear_writer() {
    if let Ok(mut w) = WRITER.lock() {
        *w = None;
    }
}

/// A writer id as used in log file names.
pub fn safe_id(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect()
}

/// The length of `writer`'s log (how far a catalog copy made now has seen it).
pub fn log_len(root: &Path, writer: &str) -> u64 {
    std::fs::metadata(log_path(root, &safe_id(writer)))
        .map(|m| m.len())
        .unwrap_or(0)
}

fn log_path(root: &Path, writer: &str) -> PathBuf {
    root.join(VIBE_DIR).join(DIR).join(format!("{writer}.log"))
}

/// The cabinet's top-level folders (client folders) `ops` touched; `_Vibe` itself is left out.
pub fn folders_of<'a>(root: &Path, ops: impl IntoIterator<Item = &'a FileOp>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for op in ops {
        let paths: Vec<&Path> = match op {
            FileOp::Move { src, dst } | FileOp::Copy { src, dst } => vec![src, dst],
            FileOp::Mkdir { path } | FileOp::Rmdir { path } => vec![path],
            FileOp::AppendLines { path, .. } => vec![path],
            FileOp::WriteSentinel { folder, .. } => vec![folder],
        };
        for p in paths {
            if let Ok(rel) = p.strip_prefix(root)
                && let Some(first) = rel.components().next()
            {
                let f = first.as_os_str().to_string_lossy().into_owned();
                if !f.eq_ignore_ascii_case(VIBE_DIR) {
                    out.insert(f);
                }
            }
        }
    }
    out
}

/// Notes that `ops` changed the cabinet (when noting is on). Never fails the change itself.
pub fn note<'a>(ops: impl IntoIterator<Item = &'a FileOp>) {
    let Some((root, _)) = WRITER.lock().ok().and_then(|w| w.clone()) else {
        return;
    };
    note_folders(folders_of(&root, ops));
}

/// Notes files changed outside a Transaction's file ops (Filer's Intake moves them itself).
pub fn note_paths<'a>(paths: impl IntoIterator<Item = &'a Path>) {
    let Some((root, _)) = WRITER.lock().ok().and_then(|w| w.clone()) else {
        return;
    };
    let ops: Vec<FileOp> = paths
        .into_iter()
        .map(|p| FileOp::Mkdir {
            path: p.to_path_buf(),
        })
        .collect();
    note_folders(folders_of(&root, &ops));
}

fn note_folders(folders: BTreeSet<String>) {
    let Some((root, writer)) = WRITER.lock().ok().and_then(|w| w.clone()) else {
        return;
    };
    if folders.is_empty() {
        return;
    }
    let path = log_path(&root, &writer);
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(&root));
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        // The others then catch it at the folder's next open or the weekly sweep.
        return;
    };
    let ts = vf_core::now_rfc3339();
    let mut text = String::new();
    for folder in folders {
        text.push_str(&format!("{ts}\t{folder}\n"));
    }
    let _ = f.write_all(text.as_bytes());
}

/// How far each other writer's log has been read (bytes), kept in the catalog.
pub type Seen = BTreeMap<String, u64>;

const SEEN_KEY: &str = "changes_seen";

/// What this catalog has read of the logs; `None` before its first look.
pub fn load_seen(conn: &vf_core::db::Connection) -> Option<Seen> {
    conn.query_row("SELECT v FROM local_state WHERE k = ?1", [SEEN_KEY], |r| {
        r.get::<_, String>(0)
    })
    .ok()
    .and_then(|v| serde_json::from_str(&v).ok())
}

pub fn save_seen(conn: &vf_core::db::Connection, seen: &Seen) -> Result<(), String> {
    conn.execute(
        "INSERT INTO local_state (k, v) VALUES (?1, ?2) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
        [SEEN_KEY, &serde_json::to_string(seen).expect("serializes")],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// The client folders other writers changed since `seen` (updated to the logs' ends). A log
/// seen for the first time is read from its start only when `from_start`; else from its end
/// (a new server seeded from a catalog copy starts where the copy left off).
pub fn changed_since(root: &Path, me: &str, seen: &mut Seen, from_start: bool) -> BTreeSet<String> {
    let me = safe_id(me);
    let mut out = BTreeSet::new();
    let Ok(rd) = std::fs::read_dir(root.join(VIBE_DIR).join(DIR)) else {
        return out;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(writer) = name.strip_suffix(".log") else {
            continue;
        };
        if writer == me {
            continue;
        }
        let Ok(mut f) = std::fs::File::open(e.path()) else {
            continue;
        };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        let start = match seen.get(writer) {
            Some(&at) if at <= len => at,
            // Shorter than before: the log was replaced; read it all.
            Some(_) => 0,
            None if from_start => 0,
            None => len,
        };
        if f.seek(std::io::SeekFrom::Start(start)).is_err() {
            continue;
        }
        // Whole lines only: a last line still being written (no newline yet) is read next time.
        let mut read = start;
        let mut r = std::io::BufReader::new(f);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match r.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) if buf.last() != Some(&b'\n') => break,
                Ok(n) => {
                    read += n as u64;
                    let line = String::from_utf8_lossy(&buf);
                    if let Some((_, folder)) = line.trim_end().split_once('\t')
                        && !folder.trim().is_empty()
                    {
                        out.insert(folder.trim().to_string());
                    }
                }
            }
        }
        seen.insert(writer.to_string(), read);
    }
    out
}

/// A catalog's first look, made before it is built (a first index takes hours; a copy may be
/// old): every log's current end is marked as read, so changes noted from now on, during the
/// build, are read afterwards. Nothing when the catalog has looked before.
pub fn mark_start(conn: &vf_core::db::Connection, root: &Path, me: &str) -> Result<(), String> {
    if load_seen(conn).is_some() {
        return Ok(());
    }
    let mut seen = Seen::new();
    changed_since(root, me, &mut seen, false);
    save_seen(conn, &seen)
}

/// Brings this catalog up to date with the other servers' changes: re-indexes the folders
/// their logs name since last time. Returns the folders re-indexed.
pub fn catch_up(
    conn: &mut vf_core::db::Connection,
    root: &Path,
    me: &str,
    device_id: &str,
) -> Result<Vec<String>, String> {
    // Intake moving files here right now: looked at next time (nothing marked as read).
    if crate::intake::filing_now(root) {
        return Ok(vec![]);
    }
    let loaded = load_seen(conn);
    let first = loaded.is_none();
    let mut seen = loaded.unwrap_or_default();
    // A catalog that has seen no log yet was just built from the disk: it starts at the logs'
    // ends. After that, a writer it has not seen before is read from its start.
    let folders: Vec<String> = changed_since(root, me, &mut seen, !first)
        .into_iter()
        .collect();
    if first && folders.is_empty() {
        // The first look only marks where the logs end.
        save_seen(conn, &seen)?;
        return Ok(vec![]);
    }
    if !folders.is_empty() {
        // As files found in the folders (not a folder import's): text recognition set to new
        // filings reads them on the main server (QA).
        crate::rebuild::reread_folders(conn, root, device_id, &folders)
            .map_err(|e| e.to_string())?;
    }
    save_seen(conn, &seen)?;
    Ok(folders)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Code review: a line still being written (no newline yet) is not read until it is whole.
    #[test]
    fn a_half_written_line_waits() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let log = log_path(root, "server-a");
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::fs::write(&log, "t1\tAcme LLC (1002)\nt2\tSmith, Jo").unwrap();
        let mut seen = Seen::new();
        let got = changed_since(root, "server-b", &mut seen, true);
        assert_eq!(got.into_iter().collect::<Vec<_>>(), ["Acme LLC (1002)"]);
        std::fs::write(&log, "t1\tAcme LLC (1002)\nt2\tSmith, John (1001)\n").unwrap();
        let got = changed_since(root, "server-b", &mut seen, true);
        assert_eq!(got.into_iter().collect::<Vec<_>>(), ["Smith, John (1001)"]);
    }

    #[test]
    fn logs_name_the_folders_and_are_read_once() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let ops = [
            FileOp::Move {
                src: root.join("Inbox Client/a.pdf"),
                dst: root.join("Smith, John (1001)/Income Tax/2025/a.pdf"),
            },
            FileOp::AppendLines {
                path: root.join("Acme LLC (1002)/_Vibe/docmeta.jsonl"),
                lines: vec![],
                undo_lines: vec![],
            },
            FileOp::AppendLines {
                path: root.join("_Vibe/audit/2026-11.jsonl"),
                lines: vec![],
                undo_lines: vec![],
            },
        ];
        let f = folders_of(root, &ops);
        assert_eq!(
            f.into_iter().collect::<Vec<_>>(),
            ["Acme LLC (1002)", "Inbox Client", "Smith, John (1001)"]
        );
        // Server A notes; server B reads it once.
        set_writer(root, "server-a");
        note(&ops);
        clear_writer();
        let mut seen = Seen::new();
        let got = changed_since(root, "server-b", &mut seen, true);
        assert_eq!(got.len(), 3);
        assert!(changed_since(root, "server-b", &mut seen, true).is_empty());
        // Its own log is not news to A.
        assert!(changed_since(root, "server-a", &mut Seen::new(), true).is_empty());
        // A log seen for the first time without from_start: from its end.
        assert!(changed_since(root, "server-c", &mut Seen::new(), false).is_empty());
    }
}
