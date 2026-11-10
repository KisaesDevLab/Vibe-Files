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

/// A file's attributes, read by path (no handle is opened, so nothing is downloaded).
/// `None` when the file cannot be reached.
pub fn attributes(p: &Path) -> Option<u32> {
    use windows::Win32::Storage::FileSystem::{GetFileAttributesW, INVALID_FILE_ATTRIBUTES};
    use windows::core::HSTRING;
    let wide = HSTRING::from(p.as_os_str());
    // SAFETY: a valid, NUL-terminated wide string for the call's duration.
    let a = unsafe { GetFileAttributesW(&wide) };
    (a != INVALID_FILE_ATTRIBUTES).then_some(a)
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
    let wide = HSTRING::from(p.as_os_str());
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
    let wide = HSTRING::from(p.as_os_str());
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
    let wide = HSTRING::from(p.as_os_str());
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
}
