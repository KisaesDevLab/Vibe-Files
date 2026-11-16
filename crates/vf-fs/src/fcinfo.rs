//! A client's notes, kept in FileCenter's `!fcinfo.ini` in the client's folder (owner
//! 2026-11-15, Q-115, PA-76). The firm uses FileCenter beside Vibe Files, so the file stays
//! FileCenter's: only the text under `[Memo]` is ever changed, every other line is written back
//! as it was read, in the file's own encoding and line endings.
//!
//! The file is not a document: the index, Browse's new-file look and sync leave it alone
//! ([`is_note_file`]).

use std::path::{Path, PathBuf};

/// FileCenter's name for the file.
pub const NAME: &str = "!fcinfo.ini";

/// Whether a file of this name is a notes file (or the half-written copy of one a save makes):
/// never taken into the catalog, never sent to or changed from the bucket.
pub fn is_note_file(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == NAME || (n.starts_with("!fcinfo.ini.") && n.ends_with(".tmp"))
}

/// [`is_note_file`] for a cabinet-relative path.
pub fn is_note_path(rel_path: &str) -> bool {
    is_note_file(rel_path.rsplit(['/', '\\']).next().unwrap_or(rel_path))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Enc {
    /// Windows-1252, what FileCenter's INI files are without a mark.
    Ansi,
    Utf8,
    Utf8Bom,
    Utf16Le,
}

/// The notes as read: the memo's text, and the file's bytes at that moment (a save compares
/// them with the file again, so a change made meanwhile is seen).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notes {
    /// The lines under `[Memo]`, joined with `\n`, without the blank lines that end them.
    pub memo: String,
    /// Whether the file was there.
    pub exists: bool,
    raw: Vec<u8>,
}

/// What a save did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Saved {
    /// Written (or nothing to write): the notes as they now are.
    Done(Notes),
    /// The file changed since the notes were shown (FileCenter or another PC saved it): nothing
    /// was written. These are the file's notes now.
    Changed(Notes),
}

pub fn path(folder: &Path) -> PathBuf {
    folder.join(NAME)
}

/// Reads the notes of `folder`. No file: empty notes. Asks the share: not for the window's
/// thread.
pub fn read(folder: &Path) -> Result<Notes, String> {
    let p = path(folder);
    match std::fs::read(&p) {
        Ok(raw) => {
            let (text, _) = decode(&raw);
            Ok(Notes {
                memo: memo_of(&text),
                exists: true,
                raw,
            })
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Notes {
            memo: String::new(),
            exists: false,
            raw: Vec::new(),
        }),
        Err(e) => Err(format!("{} cannot be read: {e}", p.display())),
    }
}

/// Writes `memo` as the notes of `folder` (a folder of the cabinet at `root`), making the file
/// when there is none. `shown` is what the person was looking at: when the file is no longer
/// that, nothing is written and [`Saved::Changed`] has the newer notes, unless `force`.
pub fn save(
    root: &Path,
    folder: &Path,
    shown: &Notes,
    memo: &str,
    force: bool,
) -> Result<Saved, String> {
    crate::lock::with(root, "notes", "the client notes", || {
        let now = read(folder)?;
        if !force && (now.exists != shown.exists || now.raw != shown.raw) {
            return Ok(Saved::Changed(now));
        }
        let memo = clean(memo);
        if now.exists && now.memo == memo {
            return Ok(Saved::Done(now));
        }
        if !now.exists && memo.is_empty() {
            return Ok(Saved::Done(now));
        }
        let (text, enc) = if now.exists {
            decode(&now.raw)
        } else {
            (NEW.to_string(), Enc::Ansi)
        };
        let text = with_memo(&text, &memo);
        let bytes = encode(&text, enc);
        let p = path(folder);
        let attrs = attributes(&p);
        crate::cabinet::write_replace(&p, &bytes)?;
        if let Some(a) = attrs {
            set_attributes(&p, a);
        }
        Ok(Saved::Done(Notes {
            memo,
            exists: true,
            raw: bytes,
        }))
    })
}

/// Takes out of the catalog the rows an earlier build made for notes files (it listed each as
/// a document). The rows are removed, not marked deleted: a deletion is sent to the bucket, and
/// another PC would move the file to its trash. The files are not touched. A notes file the
/// firm put in the trash itself keeps its row. Returns how many rows went.
///
/// Reads every document's path: not for the window's thread.
pub fn forget_rows(conn: &mut vf_core::db::Connection, device_id: &str) -> Result<usize, String> {
    use vf_core::tx::{Actor, NoFileOps, Stmt, Transaction};
    let ids: Vec<String> = {
        let mut st = conn
            .prepare(
                "SELECT id FROM document
                 WHERE (lower(rel_path) = '!fcinfo.ini' OR lower(rel_path) LIKE '%/!fcinfo.ini')
                   AND (deleted_at IS NULL OR tombstone_reason = 'missing on rebuild')",
            )
            .map_err(|e| e.to_string())?;
        st.query_map([], |r| r.get(0))
            .and_then(|rows| rows.collect())
            .map_err(|e| e.to_string())?
    };
    if ids.is_empty() {
        return Ok(0);
    }
    let mut tx = Transaction::new("note_files_not_documents");
    for id in &ids {
        let p = || vec![serde_json::Value::String(id.clone())];
        for sql in [
            "DELETE FROM document_tag WHERE doc_id = ?1",
            "DELETE FROM version WHERE doc_id = ?1",
            "DELETE FROM lock WHERE doc_id = ?1",
            "DELETE FROM legal_hold WHERE doc_id = ?1",
            "UPDATE consent_7216 SET doc_id = NULL WHERE doc_id = ?1",
            "DELETE FROM disposition WHERE doc_id = ?1",
            "DELETE FROM document_extract WHERE doc_id = ?1",
            "DELETE FROM sync_base WHERE doc_id = ?1",
            "DELETE FROM sync_pending WHERE doc_id = ?1",
            "DELETE FROM sync_conflict WHERE doc_id = ?1 OR copy_doc_id = ?1",
            "DELETE FROM document WHERE id = ?1",
        ] {
            tx = tx.catalog(Stmt::new(sql, p()));
        }
    }
    tx.audit(vf_core::audit::AuditEntry::new(
        "note_files_not_documents",
        None,
        serde_json::json!({ "rows": ids.len() }),
    ))
    .commit(conn, &mut NoFileOps, &Actor::device(device_id))
    .map_err(|e| e.to_string())?;
    Ok(ids.len())
}

/// A new file, as FileCenter writes one with nothing in it.
const NEW: &str = "[Fields]\r\n\r\n\r\n[Memo]\r\n\r\n[System]\r\n";

fn decode(raw: &[u8]) -> (String, Enc) {
    if let Some(rest) = raw.strip_prefix(&[0xFF, 0xFE]) {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        return (String::from_utf16_lossy(&units), Enc::Utf16Le);
    }
    if let Some(rest) = raw.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return (String::from_utf8_lossy(rest).into_owned(), Enc::Utf8Bom);
    }
    match std::str::from_utf8(raw) {
        // Plain ASCII is both: it stays ANSI, which is what FileCenter writes.
        Ok(s) if s.is_ascii() => (s.to_string(), Enc::Ansi),
        Ok(s) => (s.to_string(), Enc::Utf8),
        Err(_) => (
            encoding_rs::WINDOWS_1252
                .decode_without_bom_handling(raw)
                .0
                .into_owned(),
            Enc::Ansi,
        ),
    }
}

/// The file's bytes in its own encoding. An ANSI file given a character ANSI cannot hold
/// becomes UTF-16 with its mark, the other form Windows reads an INI file in.
fn encode(text: &str, enc: Enc) -> Vec<u8> {
    let utf16 = |text: &str| {
        let mut out = vec![0xFF, 0xFE];
        for u in text.encode_utf16() {
            out.extend_from_slice(&u.to_le_bytes());
        }
        out
    };
    match enc {
        Enc::Utf16Le => utf16(text),
        Enc::Utf8 => text.as_bytes().to_vec(),
        Enc::Utf8Bom => [&[0xEF, 0xBB, 0xBF], text.as_bytes()].concat(),
        Enc::Ansi => {
            let (bytes, _, lossy) = encoding_rs::WINDOWS_1252.encode(text);
            if lossy {
                utf16(text)
            } else {
                bytes.into_owned()
            }
        }
    }
}

/// A section's header: `[Name]` from the line's first column (a memo line set in by a space is
/// not one here).
fn is_header(line: &str) -> bool {
    let t = line.trim_end();
    t.len() >= 2 && t.starts_with('[') && t.ends_with(']')
}

fn is_memo_header(line: &str) -> bool {
    line.trim().eq_ignore_ascii_case("[memo]")
}

/// The text as typed, ready for the file: line ends as `\n`, no blank lines at the end, and a
/// line that would read as a section's header set in by a space.
fn clean(memo: &str) -> String {
    let lines: Vec<String> = memo
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .map(|l| {
            let l = l.trim_end();
            if is_header(l) {
                format!(" {l}")
            } else {
                l.to_string()
            }
        })
        .collect();
    let end = lines
        .iter()
        .rposition(|l| !l.is_empty())
        .map_or(0, |i| i + 1);
    lines[..end].join("\n")
}

fn memo_of(text: &str) -> String {
    let mut lines = text.lines().skip_while(|l| !is_memo_header(l));
    if lines.next().is_none() {
        return String::new();
    }
    let body: Vec<&str> = lines
        .take_while(|l| !is_header(l))
        .map(str::trim_end)
        .collect();
    let end = body
        .iter()
        .rposition(|l| !l.is_empty())
        .map_or(0, |i| i + 1);
    body[..end].join("\n")
}

/// `text` with the lines under `[Memo]` replaced by `memo`; every other line as it was.
fn with_memo(text: &str, memo: &str) -> String {
    let nl = if text.contains("\r\n") || !text.contains('\n') {
        "\r\n"
    } else {
        "\n"
    };
    // Lines with their own endings, so what is not the memo goes back byte for byte.
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut body = String::new();
    for l in memo.split('\n').filter(|_| !memo.is_empty()) {
        body.push_str(l);
        body.push_str(nl);
    }
    let Some(at) = lines.iter().position(|l| is_memo_header(l)) else {
        // No memo yet: before `[System]` when there is one, else at the end.
        let before = lines
            .iter()
            .position(|l| l.trim().eq_ignore_ascii_case("[system]"))
            .unwrap_or(lines.len());
        let mut out: String = lines[..before].concat();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push_str(nl);
        }
        out.push_str("[Memo]");
        out.push_str(nl);
        out.push_str(&body);
        out.push_str(nl);
        out.push_str(&lines[before..].concat());
        return out;
    };
    let end = lines[at + 1..]
        .iter()
        .position(|l| is_header(l))
        .map_or(lines.len(), |i| at + 1 + i);
    let mut out: String = lines[..at].concat();
    out.push_str(lines[at].trim_end_matches(['\r', '\n']));
    out.push_str(nl);
    out.push_str(&body);
    if end < lines.len() {
        // A blank line before the next section, as FileCenter leaves one.
        out.push_str(nl);
    }
    out.push_str(&lines[end..].concat());
    out
}

/// Hidden, system and archive, when the file has any of them (a save puts a new file in its
/// place).
fn attributes(p: &Path) -> Option<u32> {
    use std::os::windows::fs::MetadataExt;
    const KEPT: u32 = 0x2 | 0x4 | 0x20;
    let a = std::fs::metadata(p).ok()?.file_attributes() & KEPT;
    (a & (0x2 | 0x4) != 0).then_some(a)
}

fn set_attributes(p: &Path, a: u32) {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{FILE_FLAGS_AND_ATTRIBUTES, SetFileAttributesW};
    let wide: Vec<u16> = p.as_os_str().encode_wide().chain(Some(0)).collect();
    // Not kept is not a failed save: the notes are written.
    let _ = unsafe {
        SetFileAttributesW(
            windows::core::PCWSTR(wide.as_ptr()),
            FILE_FLAGS_AND_ATTRIBUTES(a),
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape of a FileCenter file, with invented text.
    const SAMPLE: &str = "[Fields]\r\nKind=Example\r\n\r\n[Memo]\r\nA\r\nfirst line\r\nsecond line\r\n\r\n[System]\r\nStamp=1\r\n";

    fn cabinet() -> (tempfile::TempDir, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let folder = t.path().join("Example Client (1000)");
        std::fs::create_dir_all(&folder).unwrap();
        (t, folder)
    }

    fn done(s: Saved) -> Notes {
        match s {
            Saved::Done(n) => n,
            Saved::Changed(_) => panic!("the file was said to have changed"),
        }
    }

    #[test]
    fn names_of_notes_files() {
        assert!(is_note_file("!fcinfo.ini"));
        assert!(is_note_file("!FCInfo.INI"));
        assert!(is_note_file("!fcinfo.ini.0192abc.tmp"));
        assert!(!is_note_file("fcinfo.ini"));
        assert!(!is_note_file("!fcinfo.ini.bak"));
        assert!(is_note_path("Example Client (1000)/Payroll/!fcinfo.ini"));
        assert!(!is_note_path("Example Client (1000)/!fcinfo.ini/w2.pdf"));
    }

    #[test]
    fn the_memo_is_read_and_only_the_memo_is_changed() {
        let (t, folder) = cabinet();
        std::fs::write(path(&folder), SAMPLE).unwrap();
        let shown = read(&folder).unwrap();
        assert!(shown.exists);
        assert_eq!(shown.memo, "A\nfirst line\nsecond line");
        // Nothing changed: nothing written.
        let same = done(save(t.path(), &folder, &shown, &shown.memo, false).unwrap());
        assert_eq!(std::fs::read(path(&folder)).unwrap(), SAMPLE.as_bytes());
        let after =
            done(save(t.path(), &folder, &same, "A\nfirst line\nthird\n\n", false).unwrap());
        assert_eq!(after.memo, "A\nfirst line\nthird");
        assert_eq!(
            std::fs::read_to_string(path(&folder)).unwrap(),
            "[Fields]\r\nKind=Example\r\n\r\n[Memo]\r\nA\r\nfirst line\r\nthird\r\n\r\n[System]\r\nStamp=1\r\n"
        );
        assert_eq!(read(&folder).unwrap(), after);
        // No temp file is left beside it.
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 1);
    }

    #[test]
    fn a_file_is_made_on_the_first_save() {
        let (t, folder) = cabinet();
        let shown = read(&folder).unwrap();
        assert!(!shown.exists && shown.memo.is_empty());
        // Nothing typed: no file.
        done(save(t.path(), &folder, &shown, "  \n", false).unwrap());
        assert!(!path(&folder).exists());
        let after = done(save(t.path(), &folder, &shown, "call in March", false).unwrap());
        assert!(after.exists);
        assert_eq!(
            std::fs::read_to_string(path(&folder)).unwrap(),
            "[Fields]\r\n\r\n\r\n[Memo]\r\ncall in March\r\n\r\n[System]\r\n"
        );
        assert_eq!(read(&folder).unwrap().memo, "call in March");
    }

    #[test]
    fn a_change_made_elsewhere_is_not_written_over_unseen() {
        let (t, folder) = cabinet();
        std::fs::write(path(&folder), SAMPLE).unwrap();
        let shown = read(&folder).unwrap();
        let theirs = SAMPLE.replace("second line", "their line");
        std::fs::write(path(&folder), &theirs).unwrap();
        let Saved::Changed(now) = save(t.path(), &folder, &shown, "mine", false).unwrap() else {
            panic!("saved over the other change");
        };
        assert_eq!(now.memo, "A\nfirst line\ntheir line");
        assert_eq!(std::fs::read_to_string(path(&folder)).unwrap(), theirs);
        // Asked for: mine goes in, the rest of their file stays.
        let after = done(save(t.path(), &folder, &shown, "mine", true).unwrap());
        assert_eq!(after.memo, "mine");
        assert!(
            std::fs::read_to_string(path(&folder))
                .unwrap()
                .ends_with("[Memo]\r\nmine\r\n\r\n[System]\r\nStamp=1\r\n")
        );
        // A file that appeared since an empty one was shown is seen too.
        let (t2, f2) = cabinet();
        let none = read(&f2).unwrap();
        std::fs::write(path(&f2), SAMPLE).unwrap();
        assert!(matches!(
            save(t2.path(), &f2, &none, "mine", false).unwrap(),
            Saved::Changed(_)
        ));
    }

    #[test]
    fn other_shapes_of_file() {
        let (t, folder) = cabinet();
        // No memo section: one is added before [System].
        std::fs::write(path(&folder), "[Fields]\nA=1\n[System]\nB=2\n").unwrap();
        let shown = read(&folder).unwrap();
        assert_eq!(shown.memo, "");
        done(save(t.path(), &folder, &shown, "note", false).unwrap());
        assert_eq!(
            std::fs::read_to_string(path(&folder)).unwrap(),
            "[Fields]\nA=1\n[Memo]\nnote\n\n[System]\nB=2\n"
        );
        // The memo last, with no line end after it.
        std::fs::write(path(&folder), "[Memo]\r\nold").unwrap();
        let shown = read(&folder).unwrap();
        assert_eq!(shown.memo, "old");
        done(save(t.path(), &folder, &shown, "new\n[not a section]", false).unwrap());
        assert_eq!(
            std::fs::read_to_string(path(&folder)).unwrap(),
            "[Memo]\r\nnew\r\n [not a section]\r\n"
        );
        assert_eq!(read(&folder).unwrap().memo, "new\n [not a section]");
        // Cleared.
        let shown = read(&folder).unwrap();
        done(save(t.path(), &folder, &shown, "", false).unwrap());
        assert_eq!(
            std::fs::read_to_string(path(&folder)).unwrap(),
            "[Memo]\r\n"
        );
    }

    #[test]
    fn the_files_encoding_is_kept() {
        let (t, folder) = cabinet();
        // ANSI (Windows-1252): é is one byte, and stays one.
        let ansi = b"[Fields]\r\nName=Caf\xE9\r\n[Memo]\r\nr\xE9sum\xE9\r\n[System]\r\n";
        std::fs::write(path(&folder), ansi).unwrap();
        let shown = read(&folder).unwrap();
        assert_eq!(shown.memo, "résumé");
        done(save(t.path(), &folder, &shown, "résumé sent", false).unwrap());
        let raw = std::fs::read(path(&folder)).unwrap();
        assert!(raw.starts_with(b"[Fields]\r\nName=Caf\xE9\r\n[Memo]\r\nr\xE9sum\xE9 sent\r\n"));
        // A character ANSI cannot hold: the file becomes UTF-16 with its mark.
        let shown = read(&folder).unwrap();
        done(save(t.path(), &folder, &shown, "→ call", false).unwrap());
        let raw = std::fs::read(path(&folder)).unwrap();
        assert_eq!(&raw[..2], &[0xFF, 0xFE]);
        let again = read(&folder).unwrap();
        assert_eq!(again.memo, "→ call");
        // And a UTF-16 file stays one.
        done(save(t.path(), &folder, &again, "plain", false).unwrap());
        let raw = std::fs::read(path(&folder)).unwrap();
        assert_eq!(&raw[..2], &[0xFF, 0xFE]);
        assert_eq!(read(&folder).unwrap().memo, "plain");
        let (text, _) = decode(&raw);
        assert!(text.contains("Name=Café"));
    }

    #[test]
    fn a_hidden_file_stays_hidden() {
        let (t, folder) = cabinet();
        std::fs::write(path(&folder), SAMPLE).unwrap();
        set_attributes(&path(&folder), 0x2 | 0x20);
        let shown = read(&folder).unwrap();
        done(save(t.path(), &folder, &shown, "changed", false).unwrap());
        assert_eq!(read(&folder).unwrap().memo, "changed");
        assert_eq!(attributes(&path(&folder)), Some(0x2 | 0x20));
    }
}
