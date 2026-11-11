//! Cabinets kept in a OneDrive or Dropbox folder (PA-39).
//!
//! The files of such a folder may be "online only": placeholders whose contents are downloaded
//! the moment anything reads them. Listing a folder and reading a file's attributes download
//! nothing, so Vibe Files asks those before reading a file in the background (indexing, text
//! recognition, thumbnails), and leaves online-only files alone until someone opens them.
//!
//! What Windows reports was checked on a PC with both (2026-11-10): OneDrive registers a cloud
//! sync root and marks an online-only file `RECALL_ON_DATA_ACCESS` (`0x400020`); Dropbox
//! registers none and marks one `OFFLINE | SPARSE | REPARSE_POINT` (`0x1620`).

use std::path::{Path, PathBuf};

/// File attribute bits (winnt.h) that mean "the contents are not on this PC".
pub const RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
pub const RECALL_ON_OPEN: u32 = 0x0004_0000;
pub const OFFLINE: u32 = 0x0000_1000;

/// Whether attributes `a` mark a file whose contents are not on this PC.
pub fn online_only_attrs(a: u32) -> bool {
    a & (RECALL_ON_DATA_ACCESS | RECALL_ON_OPEN | OFFLINE) != 0
}

/// Whether metadata already read (a folder listing's) marks an online-only file.
pub fn online_only_meta(m: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    online_only_attrs(m.file_attributes())
}

/// Whether background work leaves online-only files alone (PA-39). On unless this computer's
/// setting says to read every file (Admin › Devices: Cloud folder handling, Always off).
static LEAVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Sets whether background work leaves online-only files alone (at start).
pub fn set_leave_online_only(on: bool) {
    LEAVE.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Whether background work must not read `p` now: it is online only and this computer leaves
/// such files until they are on it.
pub fn leave(p: &Path) -> bool {
    LEAVE.load(std::sync::atomic::Ordering::Relaxed) && is_online_only(p)
}

/// [`leave`] for attributes already read (a folder listing's, [`stat`]'s).
pub fn leave_attrs(a: u32) -> bool {
    LEAVE.load(std::sync::atomic::Ordering::Relaxed) && online_only_attrs(a)
}

/// A file's attributes, read by path (no handle is opened, so nothing is downloaded).
/// `None` when the file cannot be reached.
pub fn attributes(p: &Path) -> Option<u32> {
    use windows::Win32::Storage::FileSystem::{GetFileAttributesW, INVALID_FILE_ATTRIBUTES};
    use windows::core::HSTRING;
    let wide = HSTRING::from(long_path(p).as_os_str());
    // SAFETY: a valid, NUL-terminated wide string for the call's duration.
    let a = unsafe { GetFileAttributesW(&wide) };
    (a != INVALID_FILE_ATTRIBUTES).then_some(a)
}

/// `p` in the `\\?\` form when it is long: Win32 calls given a plain path fail past 260
/// characters (the app has no long-path manifest), and a cabinet deep in a OneDrive folder
/// reaches that. As Filer's `commit::verbatim` (vendored, private there).
pub fn long_path(p: &Path) -> PathBuf {
    let s = p.as_os_str().to_string_lossy();
    if s.len() < 240 || s.starts_with(r"\\?\") || !p.is_absolute() {
        return p.to_path_buf();
    }
    // `\\?\` turns off Win32 normalization, so normalize first (`/`, `.` and `..`).
    let Ok(full) = std::path::absolute(p) else {
        return p.to_path_buf();
    };
    let s = full.as_os_str().to_string_lossy();
    match s.strip_prefix(r"\\") {
        Some(unc) => PathBuf::from(format!(r"\\?\UNC\{unc}")),
        None => PathBuf::from(format!(r"\\?\{s}")),
    }
}

/// A file's size, last-modified time (seconds since 1970) and attributes, read by path with
/// `GetFileAttributesExW`: from the folder's own records, without opening the file, so an
/// online-only file is never downloaded for it (opening a handle may, with some services).
pub fn stat(p: &Path) -> Option<(u64, i64, u32)> {
    use windows::Win32::Storage::FileSystem::{
        GetFileAttributesExW, GetFileExInfoStandard, WIN32_FILE_ATTRIBUTE_DATA,
    };
    use windows::core::HSTRING;
    let wide = HSTRING::from(long_path(p).as_os_str());
    let mut d = WIN32_FILE_ATTRIBUTE_DATA::default();
    // SAFETY: a valid, NUL-terminated wide string and a buffer of the documented type for the
    // call's duration.
    unsafe { GetFileAttributesExW(&wide, GetFileExInfoStandard, (&raw mut d).cast()) }.ok()?;
    let size = (u64::from(d.nFileSizeHigh) << 32) | u64::from(d.nFileSizeLow);
    let ft = (u64::from(d.ftLastWriteTime.dwHighDateTime) << 32)
        | u64::from(d.ftLastWriteTime.dwLowDateTime);
    // 100 ns ticks since 1601 to seconds since 1970, as the standard library counts them.
    let secs = (ft as i64 - 116_444_736_000_000_000).div_euclid(10_000_000);
    Some((size, secs, d.dwFileAttributes))
}

/// Whether `p`'s contents are only in the cloud: reading it would download it.
pub fn is_online_only(p: &Path) -> bool {
    attributes(p).is_some_and(online_only_attrs)
}

/// Marks a local file as a cloud service marks an online-only one (tests: `OFFLINE`, as Dropbox
/// does). Its contents stay readable; Vibe Files must not read them.
#[doc(hidden)]
pub fn mark_online_only(p: &Path) {
    use windows::Win32::Storage::FileSystem::{FILE_FLAGS_AND_ATTRIBUTES, SetFileAttributesW};
    use windows::core::HSTRING;
    let a = attributes(p).expect("file exists");
    let wide = HSTRING::from(long_path(p).as_os_str());
    // SAFETY: a valid, NUL-terminated wide string for the call's duration.
    unsafe { SetFileAttributesW(&wide, FILE_FLAGS_AND_ATTRIBUTES(a | OFFLINE)) }.unwrap();
    assert!(is_online_only(p));
}

/// The reverse: the file is on this PC now (as the cloud service marks it once downloaded).
#[doc(hidden)]
pub fn mark_local(p: &Path) {
    use windows::Win32::Storage::FileSystem::{FILE_FLAGS_AND_ATTRIBUTES, SetFileAttributesW};
    use windows::core::HSTRING;
    let a = attributes(p).expect("file exists");
    let wide = HSTRING::from(long_path(p).as_os_str());
    // SAFETY: a valid, NUL-terminated wide string for the call's duration.
    unsafe { SetFileAttributesW(&wide, FILE_FLAGS_AND_ATTRIBUTES(a & !OFFLINE)) }.unwrap();
    assert!(!is_online_only(p));
}

/// The cloud service `p` is synced by ("OneDrive - …", "Dropbox", …); `None` for a local disk or
/// a network share.
pub fn provider(p: &Path) -> Option<String> {
    sync_root_provider(p).or_else(|| dropbox_folder(p).map(|_| "Dropbox".to_string()))
}

/// The provider of the Windows cloud sync root `p` is in. `cldapi.dll` is loaded at run time:
/// Windows Server 2016 has none, and the app must still start there.
pub fn sync_root_provider(p: &Path) -> Option<String> {
    use windows::Win32::Storage::CloudFilters::{
        CF_SYNC_ROOT_INFO_CLASS, CF_SYNC_ROOT_INFO_PROVIDER, CF_SYNC_ROOT_PROVIDER_INFO,
    };
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows::core::{HRESULT, HSTRING, PCWSTR, s, w};
    type GetInfo = unsafe extern "system" fn(
        PCWSTR,
        CF_SYNC_ROOT_INFO_CLASS,
        *mut core::ffi::c_void,
        u32,
        *mut u32,
    ) -> HRESULT;
    // SAFETY: loading a system DLL by name and looking up a documented export; the pointer is
    // cast to that export's documented signature.
    let get: GetInfo = unsafe {
        let lib = LoadLibraryW(w!("cldapi.dll")).ok()?;
        let f = GetProcAddress(lib, s!("CfGetSyncRootInfoByPath"))?;
        std::mem::transmute::<unsafe extern "system" fn() -> isize, GetInfo>(f)
    };
    let wide = HSTRING::from(long_path(p).as_os_str());
    // SAFETY: zeroed is a valid value of this plain-data struct; the buffer and its length are
    // the struct's own, and the path is NUL-terminated for the call's duration.
    let info = unsafe {
        let mut info: CF_SYNC_ROOT_PROVIDER_INFO = std::mem::zeroed();
        get(
            PCWSTR(wide.as_ptr()),
            CF_SYNC_ROOT_INFO_PROVIDER,
            (&raw mut info).cast(),
            std::mem::size_of::<CF_SYNC_ROOT_PROVIDER_INFO>() as u32,
            std::ptr::null_mut(),
        )
        .ok()
        .ok()?;
        info
    };
    let n = info
        .ProviderName
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(info.ProviderName.len());
    let name = String::from_utf16_lossy(&info.ProviderName[..n]);
    Some(if name.trim().is_empty() {
        "a cloud service".into()
    } else {
        name.trim().to_string()
    })
}

/// The Dropbox folder `p` is in, from Dropbox's own `info.json` (its accounts' folders).
pub fn dropbox_folder(p: &Path) -> Option<PathBuf> {
    ["LOCALAPPDATA", "APPDATA"]
        .iter()
        .filter_map(std::env::var_os)
        .filter_map(|d| {
            std::fs::read_to_string(Path::new(&d).join("Dropbox").join("info.json")).ok()
        })
        .flat_map(|t| dropbox_paths(&t))
        .find(|f| within(p, f))
}

/// The account folders an `info.json` names.
fn dropbox_paths(text: &str) -> Vec<PathBuf> {
    let Ok(serde_json::Value::Object(accounts)) = serde_json::from_str(text) else {
        return Vec::new();
    };
    accounts
        .values()
        .filter_map(|a| a.get("path")?.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Conflicted copies of Vibe Files' own files in a cloud cabinet: when two PCs change one of
/// them between syncs, the service keeps both, Dropbox as `name (PC's conflicted copy
/// date).ext`, OneDrive as `name-PC.ext`. `computers` are the PCs that open the cabinet (their
/// names, for OneDrive's). Looked for in the cabinet's `_Vibe` folder and each client folder's.
/// Cabinet-relative paths.
pub fn conflicted_copies(root: &Path, computers: &[String]) -> Vec<String> {
    let suffixes: Vec<String> = computers
        .iter()
        .filter(|c| !c.trim().is_empty())
        .map(|c| format!("-{}", c.trim().to_lowercase()))
        .collect();
    let is_copy = |name: &str| {
        let n = name.to_lowercase();
        let stem = n.rsplit_once('.').map_or(n.as_str(), |(s, _)| s);
        n.contains("conflicted copy") || suffixes.iter().any(|s| stem.ends_with(s.as_str()))
    };
    let mut out = Vec::new();
    let look = |dir: &Path, depth: usize, out: &mut Vec<String>| {
        let mut stack = vec![(dir.to_path_buf(), 0usize)];
        while let Some((d, level)) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in entries.flatten() {
                let p = e.path();
                let Ok(t) = e.file_type() else { continue };
                if t.is_dir() {
                    if level < depth {
                        stack.push((p, level + 1));
                    }
                } else if is_copy(&e.file_name().to_string_lossy()) {
                    out.push(
                        p.strip_prefix(root)
                            .unwrap_or(&p)
                            .to_string_lossy()
                            .replace('\\', "/"),
                    );
                }
            }
        }
    };
    let vibe = crate::cabinet::VIBE_DIR;
    // The cabinet's own files: the client list, settings, and its bookkeeping folders. Not the
    // folders that hold client documents (trash, overwritten, split originals, versions, tmp):
    // a client's file there may have any name (code review).
    look(&root.join(vibe), 0, &mut out);
    for sub in ["audit", "changes", "devices", "parsers", "catalog-seed"] {
        look(&root.join(vibe).join(sub), 0, &mut out);
    }
    if let Ok(entries) = std::fs::read_dir(root) {
        for e in entries.flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) && e.file_name() != vibe {
                look(&e.path().join(vibe), 0, &mut out);
            }
        }
    }
    out.sort();
    out
}

/// The nearest folder of `p` that exists (a data folder may not be made yet).
pub fn existing_ancestor(p: &Path) -> Option<PathBuf> {
    p.ancestors().find(|a| a.exists()).map(Path::to_path_buf)
}

/// Whether `p` is `folder` or inside it (compared as Windows does, ignoring case).
fn within(p: &Path, folder: &Path) -> bool {
    let norm = |x: &Path| {
        let s = x.to_string_lossy().replace('/', "\\");
        let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
        s.trim_end_matches('\\').to_lowercase()
    };
    let (p, f) = (norm(p), norm(folder));
    !f.is_empty() && (p == f || p.starts_with(&format!("{f}\\")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn online_only_is_read_from_the_attributes() {
        // As seen on a PC with both services (2026-11-10).
        assert!(online_only_attrs(0x0040_0020), "OneDrive, online only");
        assert!(
            online_only_attrs(0x0040_1620),
            "OneDrive, online only (older marks)"
        );
        assert!(online_only_attrs(0x1620), "Dropbox, online only");
        assert!(!online_only_attrs(0x420), "OneDrive, on this PC");
        assert!(!online_only_attrs(0x0018_0020), "OneDrive, kept on this PC");
        assert!(!online_only_attrs(0x20), "an ordinary file");
    }

    #[test]
    fn a_local_folder_is_no_cloud_folder() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("a.pdf");
        std::fs::write(&f, b"x").unwrap();
        assert!(!is_online_only(&f));
        assert!(!online_only_meta(&std::fs::metadata(&f).unwrap()));
        assert!(attributes(&t.path().join("missing.pdf")).is_none());
        // The temp folder is not inside OneDrive or Dropbox on a build PC.
        assert_eq!(provider(t.path()), None);
        // The same size and time as the standard library reads, without opening the file.
        let m = std::fs::metadata(&f).unwrap();
        let secs = m
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let (size, t2, attrs) = stat(&f).unwrap();
        assert_eq!((size, t2), (m.len(), secs));
        assert!(!online_only_attrs(attrs));
        assert!(stat(&t.path().join("missing.pdf")).is_none());
    }

    /// Code review: a file past 260 characters (deep in a OneDrive folder) is still read by its
    /// attributes, not taken for unreadable (and so read whole).
    #[test]
    fn long_paths_are_read_too() {
        let t = tempfile::tempdir().unwrap();
        let mut dir = t.path().to_path_buf();
        for i in 0..8 {
            dir = dir.join(format!("Income Tax and Workpapers folder {i:02}"));
        }
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("Smith, John_1001_2025_1040_GovernmentCopyTaxReturn.pdf");
        assert!(f.as_os_str().len() > 300);
        std::fs::write(&f, b"x").unwrap();
        assert_eq!(stat(&f).map(|s| s.0), Some(1));
        assert!(!is_online_only(&f));
        mark_online_only(&f);
        assert!(is_online_only(&f));
        mark_local(&f);
        assert!(!is_online_only(&f));
    }

    #[test]
    fn dropbox_folders_come_from_its_info_file() {
        let text = r#"{"personal": {"path": "C:\\Users\\Pat\\Dropbox", "host": 1},
                       "business": {"path": "D:\\Firm Dropbox"}}"#;
        assert_eq!(dropbox_paths(text).len(), 2);
        let pat = Path::new(r"C:\Users\Pat\Dropbox");
        assert!(within(
            Path::new(r"c:\users\pat\dropbox\Cabinet\Smith (1001)"),
            pat
        ));
        assert!(within(pat, pat));
        assert!(within(Path::new(r"\\?\C:\Users\Pat\Dropbox\x"), pat));
        assert!(!within(Path::new(r"C:\Users\Pat\Dropbox Old\x"), pat));
        assert!(dropbox_paths("not json").is_empty());
    }

    #[test]
    fn conflicted_copies_of_vibe_files_are_found() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let put = |rel: &str| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "x").unwrap();
        };
        put("_Vibe/audit/2026-11.jsonl");
        put("_Vibe/audit/2026-11 (FRONT-DESK's conflicted copy 2026-11-10).jsonl");
        put("_Vibe/clients-OFFICE-2.json");
        // Client documents kept by Vibe Files may have any name: not its own files.
        put("_Vibe/trash/w2-OFFICE-2.pdf");
        put("_Vibe/overwritten/scan (FRONT-DESK's conflicted copy 2026-11-10).pdf");
        put("Smith (1001)/_Vibe/docmeta.jsonl");
        put("Smith (1001)/_Vibe/docmeta-office-2.jsonl");
        put("Smith (1001)/Income Tax/w2-OFFICE-2.pdf");
        let got = conflicted_copies(root, &["OFFICE-2".into(), "FRONT-DESK".into()]);
        assert_eq!(
            got,
            [
                "Smith (1001)/_Vibe/docmeta-office-2.jsonl",
                "_Vibe/audit/2026-11 (FRONT-DESK's conflicted copy 2026-11-10).jsonl",
                "_Vibe/clients-OFFICE-2.json",
            ]
        );
    }
}
