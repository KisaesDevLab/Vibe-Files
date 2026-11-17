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
    if n == NAME || (n.starts_with("!fcinfo.ini.") && n.ends_with(".tmp")) {
        return true;
    }
    // `!fcinfo (2).ini`: a second notes file a merge of two folders put beside the first.
    n.strip_prefix("!fcinfo (")
        .and_then(|r| r.strip_suffix(").ini"))
        .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
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
        Err(e) if crate::cabinet::is_absent(&p, &e) => Ok(Notes {
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
        // A file that is not the INI file expected (no section in it, or text in a form not
        // known here, which reads as letters with empty characters between them) is never
        // written into: FileCenter's file is left as it is.
        if now.exists
            && !text.trim().is_empty()
            && (text.contains('\0') || !text.lines().any(is_header))
        {
            return Err(format!(
                "{NAME} in this folder is not in a form Vibe Files can change, so it was left as it is. Edit these notes in FileCenter."
            ));
        }
        let text = with_memo(&text, &memo);
        let bytes = encode(&text, enc);
        let p = path(folder);
        let attrs = attributes(&p);
        crate::cabinet::write_replace(&p, &bytes)?;
        if let Some(a) = attrs {
            set_attributes(&p, a);
        }
        // Said in the cabinet's change log: the other servers (the one that sends to B2
        // among them) read this folder again and see the notes changed.
        crate::changes::note_paths([p.as_path()]);
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
    // Once for each version of the program, not on every open: it reads every document's
    // path. (Again after an update: a PC still on an earlier version, sharing this catalog,
    // may have listed the files again meanwhile.)
    const DONE: &str = "note_rows_forgotten";
    let version = env!("CARGO_PKG_VERSION");
    let done: Option<String> = conn
        .query_row("SELECT v FROM local_state WHERE k = ?1", [DONE], |r| {
            r.get(0)
        })
        .ok();
    if done.as_deref() == Some(version) {
        return Ok(0);
    }
    let mark = |conn: &vf_core::db::Connection| {
        conn.execute(
            "INSERT INTO local_state (k, v) VALUES (?1, ?2)
             ON CONFLICT(k) DO UPDATE SET v = excluded.v",
            [DONE, version],
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    };
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
        mark(conn)?;
        return Ok(0);
    }
    // By groups of ids: a cabinet can have one of these files in thousands of folders.
    let mut tx = Transaction::new("note_files_not_documents");
    for chunk in ids.chunks(400) {
        let marks = vec!["?"; chunk.len()].join(",");
        let p = || -> Vec<serde_json::Value> {
            chunk
                .iter()
                .map(|id| serde_json::Value::String(id.clone()))
                .collect()
        };
        for (table, col, set_null) in [
            ("document_tag", "doc_id", false),
            ("version", "doc_id", false),
            ("lock", "doc_id", false),
            ("legal_hold", "doc_id", false),
            ("consent_7216", "doc_id", true),
            ("disposition", "doc_id", false),
            ("document_extract", "doc_id", false),
            ("sync_base", "doc_id", false),
            ("sync_pending", "doc_id", false),
            ("sync_conflict", "doc_id", false),
            ("sync_conflict", "copy_doc_id", false),
            ("document", "id", false),
        ] {
            let sql = if set_null {
                format!("UPDATE {table} SET {col} = NULL WHERE {col} IN ({marks})")
            } else {
                format!("DELETE FROM {table} WHERE {col} IN ({marks})")
            };
            tx = tx.catalog(Stmt::new(&sql, p()));
        }
    }
    tx.audit(vf_core::audit::AuditEntry::new(
        "note_files_not_documents",
        None,
        serde_json::json!({ "rows": ids.len() }),
    ))
    .commit(conn, &mut NoFileOps, &Actor::device(device_id))
    .map_err(|e| e.to_string())?;
    mark(conn)?;
    Ok(ids.len())
}

// ---------------------------------------------------------------------------------------------
// The notes' backup to B2 (PA-79). The files are not documents, so they do not travel in the
// manifest: `vf-sync` keeps one object per client with its notes files. Here is what tells it
// which clients' notes changed, from listings made anyway (no folder is read for this alone).

/// The largest notes file backed up (FileCenter's are a few hundred bytes).
pub const BACKUP_MAX: u64 = 256 * 1024;

/// A file as a folder's listing gave it: its path, and its size, time and attributes.
pub type Listed = (PathBuf, Option<(u64, i64, u32)>);

/// What a client folder's listing says of its notes files: their paths, sizes and times, as
/// one short word. `None`: the folder has none.
pub fn stamp(listed: &[Listed]) -> Option<String> {
    use std::hash::{Hash, Hasher};
    let mut notes: Vec<(String, u64, i64)> = listed
        .iter()
        .filter(|(p, _)| {
            p.file_name()
                .is_some_and(|n| is_note_file(&n.to_string_lossy()))
        })
        .map(|(p, st)| {
            let (size, time) = st.map_or((0, 0), |s| (s.0, s.1));
            (p.to_string_lossy().to_lowercase(), size, time)
        })
        .collect();
    if notes.is_empty() {
        return None;
    }
    notes.sort();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    notes.hash(&mut h);
    Some(format!("{}:{:x}", notes.len(), h.finish()))
}

fn state(conn: &vf_core::db::Connection, k: &str) -> Option<String> {
    conn.query_row("SELECT v FROM local_state WHERE k = ?1", [k], |r| r.get(0))
        .ok()
}

fn set_state(conn: &vf_core::db::Connection, k: &str, v: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO local_state (k, v) VALUES (?1, ?2)
         ON CONFLICT(k) DO UPDATE SET v = excluded.v",
        [k, v],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// A client's folder was listed: when its notes files are not as last seen (or were never
/// seen), the client's notes wait to go to B2.
pub fn listed(conn: &vf_core::db::Connection, client_id: &str, stamp: Option<String>) {
    let Some(stamp) = stamp else { return };
    let k = format!("notes_seen:{client_id}");
    if state(conn, &k).as_deref() == Some(stamp.as_str()) {
        return;
    }
    if set_state(conn, &k, &stamp).is_ok() {
        let _ = mark_changed(conn, client_id);
    }
}

/// The notes of `client_id` changed (saved here): they wait to go to B2.
pub fn mark_changed(conn: &vf_core::db::Connection, client_id: &str) -> Result<(), String> {
    set_state(
        conn,
        &format!("notes_dirty:{client_id}"),
        &vf_core::new_id(),
    )
}

/// The clients whose notes wait to go to B2, each with its mark.
pub fn waiting(conn: &vf_core::db::Connection) -> Result<Vec<(String, String)>, String> {
    let mut st = conn
        .prepare("SELECT k, v FROM local_state WHERE k LIKE 'notes_dirty:%' ORDER BY k")
        .map_err(|e| e.to_string())?;
    let rows = st
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
        .map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .filter_map(|(k, v)| Some((k.strip_prefix("notes_dirty:")?.to_string(), v)))
        .collect())
}

/// The notes went up: the client waits no longer, unless they changed again meanwhile (its
/// mark is then another).
pub fn sent(conn: &vf_core::db::Connection, client_id: &str, mark: &str) -> Result<(), String> {
    conn.execute(
        "DELETE FROM local_state WHERE k = ?1 AND v = ?2",
        [&format!("notes_dirty:{client_id}"), mark],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// Every notes file under the client folder `folder`, by its path below it (`/`-separated),
/// with its bytes. A file over [`BACKUP_MAX`] or that cannot be read is left out.
pub fn collect(folder: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    let mut out = std::collections::BTreeMap::new();
    for p in crate::rebuild::cabinet_files(folder) {
        if !crate::rebuild::is_notes(&p) {
            continue;
        }
        if std::fs::metadata(&p).map_or(true, |m| m.len() > BACKUP_MAX) {
            continue;
        }
        if let Ok(bytes) = std::fs::read(&p) {
            out.insert(crate::rebuild::rel(folder, &p), bytes);
        }
    }
    out
}

/// [`collect`]; `Err` when the folder cannot be reached or one of its notes files is there
/// and cannot be read just now. Collected without it, the backup in B2 was replaced by one
/// with a file missing, or nothing was sent, and the notes were marked sent either way
/// (QA 2026-11-17).
pub fn collect_checked(
    folder: &Path,
) -> Result<std::collections::BTreeMap<String, Vec<u8>>, String> {
    if !folder.is_dir() {
        return Err(format!("{} cannot be reached just now", folder.display()));
    }
    let mut out = std::collections::BTreeMap::new();
    for p in crate::rebuild::cabinet_files(folder) {
        if !crate::rebuild::is_notes(&p) {
            continue;
        }
        let size = std::fs::metadata(&p)
            .map_err(|e| format!("{} cannot be read just now: {e}", p.display()))?
            .len();
        if size > BACKUP_MAX {
            continue;
        }
        let bytes = std::fs::read(&p)
            .map_err(|e| format!("{} cannot be read just now: {e}", p.display()))?;
        out.insert(crate::rebuild::rel(folder, &p), bytes);
    }
    Ok(out)
}

/// Whether `rel` can be a notes file's path below a client folder: plain folder names, no way
/// out of the folder, and a notes file's name at the end.
pub fn safe_backup_path(rel: &str) -> bool {
    let mut segs = rel.split('/').peekable();
    let mut last = "";
    while let Some(s) = segs.next() {
        if s.is_empty()
            || s == "."
            || s == ".."
            || s.contains(['\\', ':'])
            || (segs.peek().is_some() && s.eq_ignore_ascii_case(crate::cabinet::VIBE_DIR))
        {
            return false;
        }
        last = s;
    }
    is_note_file(last)
}

/// Puts a backed-up notes file back at `rel` below `folder`, only where there is none: notes
/// that are there are never written over. Returns whether it was written.
pub fn put_back(folder: &Path, rel: &str, bytes: &[u8]) -> Result<bool, String> {
    use std::io::Write;
    if !safe_backup_path(rel) || bytes.len() as u64 > BACKUP_MAX {
        return Err("a notes file's path or size is not one a backup has".into());
    }
    let p = folder.join(rel);
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&p)
    {
        Ok(mut f) => f
            .write_all(bytes)
            .and_then(|()| f.sync_all())
            .map(|()| true)
            .map_err(|e| format!("{}: {e}", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(format!("{}: {e}", p.display())),
    }
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
        assert!(is_note_file("!fcinfo (2).ini"));
        assert!(!is_note_file("!fcinfo (draft).ini"));
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
    fn a_file_that_cannot_be_understood_is_left_alone() {
        let (t, folder) = cabinet();
        // UTF-16 without its mark: read as letters with empty characters between them.
        let odd: Vec<u8> = "[Memo]\r\nkept\r\n"
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        for raw in [
            odd.as_slice(),
            b"just some text\r\nno sections\r\n".as_slice(),
        ] {
            std::fs::write(path(&folder), raw).unwrap();
            let shown = read(&folder).unwrap();
            assert_eq!(shown.memo, "");
            let e = save(t.path(), &folder, &shown, "mine", false).unwrap_err();
            assert!(e.contains("left as it is"), "{e}");
            assert_eq!(std::fs::read(path(&folder)).unwrap(), raw);
        }
        // An empty file is only empty: the notes go in.
        std::fs::write(path(&folder), "").unwrap();
        let shown = read(&folder).unwrap();
        done(save(t.path(), &folder, &shown, "mine", false).unwrap());
        assert_eq!(read(&folder).unwrap().memo, "mine");
    }

    #[test]
    fn what_tells_the_backup_that_notes_changed() {
        let t = tempfile::tempdir().unwrap();
        let conn = vf_core::db::open(&t.path().join("c.db"), vf_core::db::Storage::Local).unwrap();
        let at = |size: u64, time: i64| {
            vec![
                (PathBuf::from("C/!fcinfo.ini"), Some((size, time, 0))),
                (PathBuf::from("C/w2.pdf"), Some((9, 9, 0))),
            ]
        };
        assert_eq!(stamp(&[(PathBuf::from("C/w2.pdf"), None)]), None);
        // First seen: waits. Seen again the same: no new wait. Changed: waits again.
        listed(&conn, "c1", stamp(&at(10, 100)));
        let w = waiting(&conn).unwrap();
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].0, "c1");
        sent(&conn, "c1", &w[0].1).unwrap();
        listed(&conn, "c1", stamp(&at(10, 100)));
        assert!(waiting(&conn).unwrap().is_empty());
        listed(&conn, "c1", stamp(&at(12, 200)));
        let w = waiting(&conn).unwrap();
        assert_eq!(w.len(), 1);
        // Changed again while they went up: the older mark does not clear the wait.
        mark_changed(&conn, "c1").unwrap();
        sent(&conn, "c1", &w[0].1).unwrap();
        assert_eq!(waiting(&conn).unwrap().len(), 1);
    }

    #[test]
    fn notes_are_put_back_only_where_there_are_none() {
        let (_t, folder) = cabinet();
        std::fs::create_dir_all(folder.join("Payroll")).unwrap();
        std::fs::write(folder.join("!fcinfo.ini"), "top").unwrap();
        std::fs::write(folder.join("Payroll/!fcinfo.ini"), "sub").unwrap();
        std::fs::write(folder.join("Payroll/w2.pdf"), "x").unwrap();
        let all = collect(&folder);
        assert_eq!(
            all.keys().collect::<Vec<_>>(),
            ["!fcinfo.ini", "Payroll/!fcinfo.ini"]
        );
        // Notes that are there stay; a missing one comes back, its folder made.
        assert!(!put_back(&folder, "!fcinfo.ini", b"from the backup").unwrap());
        assert_eq!(std::fs::read(folder.join("!fcinfo.ini")).unwrap(), b"top");
        assert!(put_back(&folder, "Sales Tax/2025/!fcinfo.ini", b"back").unwrap());
        assert_eq!(
            std::fs::read(folder.join("Sales Tax/2025/!fcinfo.ini")).unwrap(),
            b"back"
        );
        for bad in [
            "../!fcinfo.ini",
            "a/../../!fcinfo.ini",
            "/!fcinfo.ini",
            "C:/x/!fcinfo.ini",
            "Payroll/w2.pdf",
            "_Vibe/!fcinfo.ini",
            "",
        ] {
            assert!(put_back(&folder, bad, b"x").is_err(), "{bad}");
        }
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

    /// QA 2026-11-17: a client folder that cannot be reached is an error for the backup, not
    /// "no notes" (they were marked sent with nothing, or part, sent).
    #[test]
    fn notes_of_a_folder_that_cannot_be_reached_are_not_no_notes() {
        let t = tempfile::tempdir().unwrap();
        assert!(collect_checked(&t.path().join("gone")).is_err());
        let folder = t.path().join("Invented Client (1001)");
        std::fs::create_dir_all(&folder).unwrap();
        assert!(collect_checked(&folder).unwrap().is_empty());
        std::fs::write(folder.join(NAME), "[Memo]\r\ncall in March\r\n").unwrap();
        assert_eq!(collect_checked(&folder).unwrap().len(), 1);
    }
}
