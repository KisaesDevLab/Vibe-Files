//! Zip import (P07b): pre-scan a zip without extracting it (entries, sizes, junk, blocked and
//! unsafe entries, guards), and extract single entries safely (temp file, SHA-256, rename).
//! Guards and the junk / blocked lists match T&B's zip import.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Refuse zips with more entries than this.
pub const MAX_ENTRIES: usize = 2_000;
/// Refuse zips with any entry larger than this (uncompressed).
pub const MAX_ENTRY_BYTES: u64 = 100 * 1024 * 1024;
/// Refuse zips larger than this in total (uncompressed).
pub const MAX_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;

/// Executable types that are never extracted.
pub const BLOCKED: [&str; 22] = [
    "exe", "com", "bat", "cmd", "msi", "scr", "pif", "cpl", "js", "jse", "vbs", "vbe", "wsf",
    "wsh", "ps1", "sh", "jar", "app", "dll", "sys", "reg", "lnk",
];

/// Files dropped silently (operating-system leftovers).
const JUNK_NAMES: [&str; 3] = [".ds_store", "thumbs.db", "desktop.ini"];

/// A zip file, by its first bytes (`PK\x03\x04` or, for an empty zip, `PK\x05\x06`).
pub fn is_zip(path: &Path) -> bool {
    let mut head = [0u8; 4];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok()
        && (head == *b"PK\x03\x04" || head == *b"PK\x05\x06")
}

/// The entry's path inside the zip, normalized: `\` → `/`, no leading `/`, empty segments dropped.
/// `Err` for anything that could escape the destination: `.` or `..` segments, a drive or stream
/// (`:`), or nothing left.
pub fn safe_entry_path(name: &str) -> Result<String, ()> {
    let unified = name.replace('\\', "/");
    let mut parts = Vec::new();
    for seg in unified.split('/') {
        match seg {
            "" => continue,
            "." | ".." => return Err(()),
            s if s.contains(':') => return Err(()),
            s => parts.push(s),
        }
    }
    if parts.is_empty() {
        return Err(());
    }
    Ok(parts.join("/"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// Imported.
    File,
    /// A zip inside the zip: imported as a file, not expanded.
    NestedZip,
    /// OS leftovers, dropped silently.
    Junk,
    /// An executable type; never extracted.
    Blocked,
    /// A path that could escape the destination; rejected.
    Unsafe,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub index: usize,
    /// The name as stored in the zip.
    pub raw_name: String,
    /// Normalized relative path (`a/b/c.pdf`); `None` for unsafe entries.
    pub path: Option<String>,
    pub size: u64,
    pub kind: EntryKind,
}

impl Entry {
    /// Imported (a file or a nested zip).
    pub fn importable(&self) -> bool {
        matches!(self.kind, EntryKind::File | EntryKind::NestedZip)
    }

    pub fn file_name(&self) -> &str {
        let p = self.path.as_deref().unwrap_or(&self.raw_name);
        p.rsplit('/').next().unwrap_or(p)
    }
}

/// What a zip holds, read from its directory without extracting anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreScan {
    pub path: PathBuf,
    /// Every non-folder entry.
    pub entries: Vec<Entry>,
    pub total_bytes: u64,
    /// The zip's own size on disk.
    pub zip_bytes: u64,
    /// Why the zip is refused (guards, encryption), if it is.
    pub refused: Option<String>,
    /// Password-protected entries were found.
    pub encrypted: bool,
}

impl PreScan {
    pub fn importable(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.importable())
    }

    pub fn of_kind(&self, kind: EntryKind) -> Vec<&Entry> {
        self.entries.iter().filter(|e| e.kind == kind).collect()
    }

    /// First-level folders and files with counts: `Bank (12), Tax (30), notes.pdf`.
    pub fn top_level(&self) -> String {
        let mut seen: Vec<(String, usize, bool)> = Vec::new();
        for e in self.importable() {
            let p = e.path.as_deref().unwrap_or_default();
            let (first, folder) = match p.split_once('/') {
                Some((f, _)) => (f.to_string(), true),
                None => (p.to_string(), false),
            };
            match seen.iter_mut().find(|(n, _, _)| *n == first) {
                Some((_, n, _)) => *n += 1,
                None => seen.push((first, 1, folder)),
            }
        }
        seen.iter()
            .map(|(n, c, folder)| {
                if *folder {
                    format!("{n}/ ({c})")
                } else {
                    n.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Why a zip with these entries is refused, if it is (pure, for tests).
pub fn check_guards(sizes: &[u64], encrypted: bool) -> Option<String> {
    if encrypted {
        return Some("password-protected archives are not supported".into());
    }
    if sizes.len() > MAX_ENTRIES {
        return Some(format!(
            "it has {} entries; the limit is {MAX_ENTRIES}",
            sizes.len()
        ));
    }
    if let Some(big) = sizes.iter().find(|s| **s > MAX_ENTRY_BYTES) {
        return Some(format!(
            "an entry is {} MB; the limit is 100 MB per entry",
            big / (1024 * 1024)
        ));
    }
    let total: u64 = sizes.iter().sum();
    if total > MAX_TOTAL_BYTES {
        return Some(format!(
            "it is {} MB uncompressed; the limit is 1 GB",
            total / (1024 * 1024)
        ));
    }
    None
}

fn classify(raw: &str) -> (Option<String>, EntryKind) {
    let Ok(path) = safe_entry_path(raw) else {
        return (None, EntryKind::Unsafe);
    };
    let lower = path.to_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    if lower.starts_with("__macosx/") || JUNK_NAMES.contains(&name) {
        return (Some(path), EntryKind::Junk);
    }
    // Names Windows can't hold as they are (reserved device names, trailing dots or spaces,
    // characters like `?` or `*`) are not written under the client root (CLAUDE.md §7).
    if path
        .split('/')
        .any(|seg| crate::template::safe_file_name(seg).as_deref() != Ok(seg))
    {
        return (None, EntryKind::Unsafe);
    }
    let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    if BLOCKED.contains(&ext) {
        return (Some(path), EntryKind::Blocked);
    }
    if ext == "zip" {
        return (Some(path), EntryKind::NestedZip);
    }
    (Some(path), EntryKind::File)
}

/// Reads the zip's directory: entries, sizes and the guards. Nothing is extracted.
pub fn prescan(path: &Path) -> Result<PreScan, String> {
    if !is_zip(path) {
        return Err("not a zip".into());
    }
    let zip_bytes = std::fs::metadata(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?
        .len();
    let file =
        std::fs::File::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("not a readable zip: {e}"))?;
    let mut entries = Vec::new();
    let mut encrypted = false;
    for index in 0..archive.len() {
        let f = archive
            .by_index_raw(index)
            .map_err(|e| format!("entry {index} cannot be read: {e}"))?;
        if f.is_dir() {
            continue;
        }
        encrypted |= f.encrypted();
        let raw = f
            .name()
            .map(|n| n.into_owned())
            .unwrap_or_else(|_| format!("entry-{index}"));
        let (p, kind) = classify(&raw);
        entries.push(Entry {
            index,
            raw_name: raw,
            path: p,
            size: f.size(),
            kind,
        });
    }
    let sizes: Vec<u64> = entries.iter().map(|e| e.size).collect();
    Ok(PreScan {
        path: path.to_path_buf(),
        total_bytes: sizes.iter().sum(),
        refused: check_guards(&sizes, encrypted),
        encrypted,
        entries,
        zip_bytes,
    })
}

/// Files written by [`extract_flat`]: (written file, entry path in the zip, bytes).
pub type Extracted = Vec<(PathBuf, String, u64)>;

/// Send to queue (P07b): extracts every importable entry flat into `dir` (created), names that
/// clash inside the zip get ` (2)`, ` (3)`… Returns (written file, entry path, bytes). Stops at
/// the first error; files already written stay (they are the app's own temp files).
pub fn extract_flat(scan: &PreScan, dir: &Path) -> Result<Extracted, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let mut x = Extractor::open(&scan.path)?;
    let mut out = Vec::new();
    for e in scan.importable() {
        let dest = crate::commit::free_path(&dir.join(e.file_name()));
        let (bytes, _) = x
            .extract(e.index, &dest)
            .map_err(|err| format!("{}: {err}", e.path.as_deref().unwrap_or(&e.raw_name)))?;
        out.push((dest, e.path.clone().unwrap_or_default(), bytes));
    }
    Ok(out)
}

/// An open zip to extract entries from.
/// Extraction stopped because the zip's real bytes passed [`MAX_TOTAL_BYTES`].
const OVER_TOTAL: &str = "the zip unpacks to more than 1 GB";

pub struct Extractor {
    archive: zip::ZipArchive<std::fs::File>,
    /// Bytes written so far: the 1 GB total is enforced on real bytes, not just the headers.
    written: u64,
}

impl Extractor {
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = std::fs::File::open(path)
            .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
        Ok(Self {
            archive: zip::ZipArchive::new(file).map_err(|e| format!("not a readable zip: {e}"))?,
            written: 0,
        })
    }

    /// The zip has unpacked to more than [`MAX_TOTAL_BYTES`]: stop.
    pub fn over_total(&self) -> bool {
        self.written > MAX_TOTAL_BYTES
    }

    /// Writes entry `index` to `dest` (which must not exist) through a temp file next to it,
    /// hashing as it goes; at most [`MAX_ENTRY_BYTES`] are read whatever the header says.
    /// Returns (bytes, sha256).
    pub fn extract(&mut self, index: usize, dest: &Path) -> Result<(u64, String), String> {
        let name = dest
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let tmp = dest.with_file_name(format!(
            "~vfd-partial-{}-{name}",
            uuid::Uuid::new_v4().simple()
        ));
        let already = self.written;
        let result = (|| -> Result<(u64, String), String> {
            let entry = self
                .archive
                .by_index(index)
                .map_err(|e| format!("cannot read the entry: {e}"))?;
            let mut out = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)
                .map_err(|e| format!("cannot create {}: {e}", tmp.display()))?;
            let mut limited = entry.take(MAX_ENTRY_BYTES + 1);
            let mut hasher = Sha256::new();
            let mut buf = vec![0u8; 64 * 1024];
            let mut total: u64 = 0;
            loop {
                let n = limited
                    .read(&mut buf)
                    .map_err(|e| format!("cannot unpack: {e}"))?;
                if n == 0 {
                    break;
                }
                total += n as u64;
                if total > MAX_ENTRY_BYTES {
                    return Err("the entry is larger than 100 MB".into());
                }
                if already + total > MAX_TOTAL_BYTES {
                    return Err(OVER_TOTAL.into());
                }
                hasher.update(&buf[..n]);
                out.write_all(&buf[..n])
                    .map_err(|e| format!("cannot write: {e}"))?;
            }
            out.sync_all().map_err(|e| format!("cannot write: {e}"))?;
            drop(out);
            let sha = format!("{:x}", hasher.finalize());
            crate::commit::move_no_replace(&tmp, dest)
                .map_err(|e| format!("cannot put {} in place: {e}", dest.display()))?;
            Ok((total, sha))
        })();
        match &result {
            Ok((n, _)) => self.written += n,
            Err(e) => {
                // Our own partial file; never the user's.
                let _ = std::fs::remove_file(&tmp);
                if e == OVER_TOTAL {
                    self.written = MAX_TOTAL_BYTES + 1;
                }
            }
        }
        result
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Writes a zip with `entries` (name, bytes); a name ending in `/` is a folder.
    pub fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = std::fs::File::create(path).unwrap();
        let mut w = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, body) in entries {
            if name.ends_with('/') {
                w.add_directory(*name, opts).unwrap();
            } else {
                w.start_file(*name, opts).unwrap();
                w.write_all(body).unwrap();
            }
        }
        w.finish().unwrap();
    }

    #[test]
    fn safe_entry_paths_match_tb() {
        assert_eq!(safe_entry_path("a/b/c.pdf").unwrap(), "a/b/c.pdf");
        assert_eq!(safe_entry_path("a\\b\\c.pdf").unwrap(), "a/b/c.pdf");
        assert_eq!(safe_entry_path("/a/b.pdf").unwrap(), "a/b.pdf");
        assert_eq!(safe_entry_path("a//b.pdf").unwrap(), "a/b.pdf");
        for bad in [
            "../escape.pdf",
            "a/../../b.pdf",
            "./a.pdf",
            "a/./b",
            "",
            "/",
            "C:/x.pdf",
            "a/b:c",
        ] {
            assert!(safe_entry_path(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn guards() {
        assert!(check_guards(&[1, 2, 3], false).is_none());
        assert!(
            check_guards(&[1], true)
                .unwrap()
                .contains("password-protected")
        );
        assert!(
            check_guards(&vec![0; MAX_ENTRIES + 1], false)
                .unwrap()
                .contains("2000")
        );
        assert!(
            check_guards(&[MAX_ENTRY_BYTES + 1], false)
                .unwrap()
                .contains("per entry")
        );
        let many = vec![MAX_ENTRY_BYTES; 11];
        assert!(check_guards(&many, false).unwrap().contains("1 GB"));
    }

    #[test]
    fn prescan_classifies_and_extract_is_safe() {
        let dir = tempfile::tempdir().unwrap();
        let z = dir.path().join("Q4 scans.zip");
        make_zip(
            &z,
            &[
                ("2024/", b""),
                ("2024/Bank/Jan.pdf", b"jan"),
                ("2024/Bank/Feb.pdf", b"feb"),
                ("notes.txt", b"n"),
                ("__MACOSX/2024/._Jan.pdf", b"x"),
                (".DS_Store", b"x"),
                ("Thumbs.db", b"x"),
                ("tools/setup.exe", b"MZ"),
                ("../escape.pdf", b"evil"),
                ("old.zip", b"PK\x03\x04nested"),
            ],
        );
        let bad = dir.path().join("names.zip");
        make_zip(
            &bad,
            &[
                ("ok.pdf", b"1"),
                ("CON.pdf", b"2"),
                ("dir./a.pdf", b"3"),
                ("what?.pdf", b"4"),
                ("trailing .pdf ", b"5"),
            ],
        );
        let names = prescan(&bad).unwrap();
        assert_eq!(
            names.importable().count(),
            1,
            "only ok.pdf is safe on Windows"
        );
        assert_eq!(names.of_kind(EntryKind::Unsafe).len(), 4);
        assert!(is_zip(&z));
        let txt = dir.path().join("x.txt");
        std::fs::write(&txt, "not a zip").unwrap();
        assert!(!is_zip(&txt));
        assert_eq!(prescan(&txt).unwrap_err(), "not a zip");

        let s = prescan(&z).unwrap();
        assert!(s.refused.is_none());
        assert_eq!(s.importable().count(), 4, "{:?}", s.entries);
        assert_eq!(s.of_kind(EntryKind::Junk).len(), 3);
        assert_eq!(s.of_kind(EntryKind::Blocked).len(), 1);
        assert_eq!(s.of_kind(EntryKind::Unsafe)[0].raw_name, "../escape.pdf");
        assert_eq!(s.of_kind(EntryKind::NestedZip).len(), 1);
        assert_eq!(s.top_level(), "2024/ (2), notes.txt, old.zip");

        let jan = s
            .entries
            .iter()
            .find(|e| e.file_name() == "Jan.pdf")
            .unwrap();
        let dest = dir.path().join("Jan.pdf");
        let mut x = Extractor::open(&z).unwrap();
        let (bytes, sha) = x.extract(jan.index, &dest).unwrap();
        assert_eq!((bytes, std::fs::read(&dest).unwrap()), (3, b"jan".to_vec()));
        assert_eq!(sha, crate::commit::sha256_file(&dest).unwrap());
        // Never over an existing file; no temp left behind.
        assert!(x.extract(jan.index, &dest).is_err());
        let leftovers = std::fs::read_dir(dir.path())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("~vfd")
            })
            .count();
        assert_eq!(leftovers, 0);
    }
}
