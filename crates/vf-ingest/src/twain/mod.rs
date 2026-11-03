//! TWAIN direct scan (PHASES PA-19, QUESTIONS Q-02): the office's Fujitsu fi-7160 through
//! PaperStream IP's 64-bit TWAIN driver at a local PC, or the remote-scanning utility's
//! virtual TWAIN source in a server session.
//!
//! The TWAIN Working Group's Data Source Manager (`TWAINDSM.dll`, 64-bit, LGPL-2.1; shipped
//! in `assets/` beside its licence) is loaded once per process. Every TWAIN call of one
//! operation runs on one dedicated thread that owns a hidden window (the sources' parent) and
//! pumps its messages. The DSM is reached through [`Entry`], so the tests drive the same code
//! against an in-process fake source ([`fake`]).
//!
//! The 64-bit DSM sees only 64-bit sources (`C:\Windows\twain_64`); a source installed only
//! for 32-bit programs (`twain_32`) is named by [`thirty_two_bit_only`] so the Scanner row can
//! say why it is missing.

#[cfg(any(test, feature = "test-support"))]
pub mod fake;
pub mod ffi;
mod session;

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ffi::*;
pub use session::{Page, Pixel, Scanned, Settings};

/// The way into the DSM: `DSM_Entry`.
pub trait Entry: Send {
    /// # Safety
    /// The pointers follow the TWAIN rules for the triplet `(dg, dat, msg)`.
    unsafe fn call(
        &mut self,
        origin: *mut Identity,
        dest: *mut Identity,
        dg: u32,
        dat: u16,
        msg: u16,
        data: *mut c_void,
    ) -> u16;
}

/// The loaded `TWAINDSM.dll`.
#[derive(Clone, Copy)]
pub struct Dsm {
    entry: EntryProc,
}

impl Entry for Dsm {
    unsafe fn call(
        &mut self,
        origin: *mut Identity,
        dest: *mut Identity,
        dg: u32,
        dat: u16,
        msg: u16,
        data: *mut c_void,
    ) -> u16 {
        unsafe { (self.entry)(origin, dest, dg, dat, msg, data) }
    }
}

/// Where `TWAINDSM.dll` is looked for: beside the exe, the repo's `assets/` in debug builds,
/// then Windows' System32 (where scanner drivers such as PaperStream IP often install it).
pub fn dsm_candidates(exe_dir: &Path) -> Vec<PathBuf> {
    let mut v = vec![exe_dir.join("TWAINDSM.dll")];
    if cfg!(debug_assertions) {
        v.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/TWAINDSM.dll"));
    }
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    v.push(Path::new(&root).join(r"System32\TWAINDSM.dll"));
    v
}

static DSM: OnceLock<Result<Dsm, String>> = OnceLock::new();

/// Loads the DSM once per process (it is never unloaded: sources may keep threads in it).
pub fn dsm() -> Result<Dsm, String> {
    DSM.get_or_init(|| {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(Path::to_path_buf))
            .unwrap_or_default();
        let candidates = dsm_candidates(&exe_dir);
        for path in candidates.iter().filter(|p| p.is_file()) {
            if let Ok(d) = load(path) {
                return Ok(d);
            }
        }
        Err(
            "TWAINDSM.dll was not found next to the program or in Windows' System32 \
             folder, so TWAIN scanners cannot be used. Put the 64-bit TWAINDSM.dll beside \
             vibe-files.exe."
                .into(),
        )
    })
    .clone()
}

fn load(path: &Path) -> Result<Dsm, String> {
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows::core::{HSTRING, s};
    let module = unsafe { LoadLibraryW(&HSTRING::from(path.as_os_str())) }
        .map_err(|e| format!("cannot load {}: {}", path.display(), e.message()))?;
    let proc_ = unsafe { GetProcAddress(module, s!("DSM_Entry")) }
        .ok_or_else(|| format!("{} has no DSM_Entry", path.display()))?;
    // SAFETY: DSM_Entry has the `DSMENTRYPROC` signature (twain.h).
    let entry: EntryProc = unsafe { std::mem::transmute(proc_) };
    Ok(Dsm { entry })
}

/// A TWAIN source the DSM lists.
#[derive(Clone)]
pub struct SourceInfo {
    /// The product name: what the Scanner row shows and the choice is remembered by.
    pub name: String,
    pub manufacturer: String,
    pub family: String,
    /// The source speaks TWAIN 2 (callbacks; memory from the DSM).
    pub twain2: bool,
    identity: Identity,
}

impl std::fmt::Debug for SourceInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceInfo")
            .field("name", &self.name)
            .field("manufacturer", &self.manufacturer)
            .field("family", &self.family)
            .field("twain2", &self.twain2)
            .finish()
    }
}

/// An open DSM (TWAIN state 3). Closed on drop.
pub struct Manager {
    entry: Box<dyn Entry>,
    app: Identity,
    /// Memory functions from the DSM (TWAIN 2).
    mem: Option<EntryPoint>,
    parent: *mut c_void,
}

/// The identity this app shows to the DSM and the sources.
fn app_identity() -> Identity {
    let mut app = Identity::default();
    app.version.major = 1;
    app.version.language = TWLG_USA;
    app.version.country = TWCY_USA;
    app.version.info = str32("Vibe Files");
    app.protocol_major = 2;
    app.protocol_minor = 5;
    app.supported_groups = DG_CONTROL | DG_IMAGE | DF_APP2;
    app.manufacturer = str32("Kisaes LLC");
    app.product_family = str32("Vibe Files");
    app.product_name = str32("Vibe Files");
    app
}

impl Manager {
    /// Opens the DSM with `parent` (a window of the calling thread) as the sources' parent.
    pub fn open(entry: Box<dyn Entry>, parent: *mut c_void) -> Result<Self, String> {
        let mut m = Manager {
            entry,
            app: app_identity(),
            mem: None,
            parent,
        };
        let mut hwnd = parent;
        let rc = m.dsm(
            DG_CONTROL,
            DAT_PARENT,
            MSG_OPENDSM,
            &mut hwnd as *mut _ as *mut c_void,
        );
        if rc != TWRC_SUCCESS {
            // Not open: nothing to close on drop.
            let cc = m.status(None);
            std::mem::forget(m);
            return Err(format!(
                "the TWAIN manager did not open ({})",
                condition(cc)
            ));
        }
        if m.app.supported_groups & DF_DSM2 != 0 {
            let mut ep = EntryPoint {
                size: size_of::<EntryPoint>() as u32,
                dsm_entry: None,
                mem_allocate: None,
                mem_free: None,
                mem_lock: None,
                mem_unlock: None,
            };
            if m.dsm(
                DG_CONTROL,
                DAT_ENTRYPOINT,
                MSG_GET,
                &mut ep as *mut _ as *mut c_void,
            ) == TWRC_SUCCESS
            {
                m.mem = Some(ep);
            }
        }
        Ok(m)
    }

    /// A triplet to the DSM itself.
    fn dsm(&mut self, dg: u32, dat: u16, msg: u16, data: *mut c_void) -> u16 {
        unsafe {
            self.entry
                .call(&mut self.app, std::ptr::null_mut(), dg, dat, msg, data)
        }
    }

    /// The condition code after a failure (of the DSM, or of `source`).
    fn status(&mut self, source: Option<&mut Identity>) -> u16 {
        let mut st = Status::default();
        let dest = source.map_or(std::ptr::null_mut(), |s| s as *mut Identity);
        let rc = unsafe {
            self.entry.call(
                &mut self.app,
                dest,
                DG_CONTROL,
                DAT_STATUS,
                MSG_GET,
                &mut st as *mut _ as *mut c_void,
            )
        };
        if rc == TWRC_SUCCESS {
            st.condition_code
        } else {
            TWCC_BUMMER
        }
    }

    /// The sources the DSM knows (64-bit ones), in its order.
    pub fn sources(&mut self) -> Result<Vec<SourceInfo>, String> {
        let mut out = Vec::new();
        let mut msg = MSG_GETFIRST;
        loop {
            let mut id = Identity::default();
            let rc = self.dsm(
                DG_CONTROL,
                DAT_IDENTITY,
                msg,
                &mut id as *mut _ as *mut c_void,
            );
            match rc {
                TWRC_SUCCESS => out.push(SourceInfo {
                    name: read_str32(&id.product_name),
                    manufacturer: read_str32(&id.manufacturer),
                    family: read_str32(&id.product_family),
                    twain2: id.supported_groups & DF_DS2 != 0,
                    identity: id,
                }),
                TWRC_ENDOFLIST => break,
                _ => {
                    let cc = self.status(None);
                    // No sources installed is an empty list, not an error.
                    if cc == TWCC_NODS || cc == TWCC_SUCCESS || msg == MSG_GETNEXT {
                        break;
                    }
                    return Err(format!("cannot list TWAIN sources ({})", condition(cc)));
                }
            }
            msg = MSG_GETNEXT;
        }
        Ok(out)
    }

    /// The parent window the sources were given.
    pub fn parent(&self) -> *mut c_void {
        self.parent
    }
}

impl Drop for Manager {
    fn drop(&mut self) {
        let mut hwnd = self.parent;
        self.dsm(
            DG_CONTROL,
            DAT_PARENT,
            MSG_CLOSEDSM,
            &mut hwnd as *mut _ as *mut c_void,
        );
    }
}

/// A condition code in words, for messages.
pub fn condition(cc: u16) -> String {
    match cc {
        TWCC_SUCCESS => "no reason given".into(),
        TWCC_LOWMEMORY => "out of memory".into(),
        TWCC_NODS => "no TWAIN source is installed".into(),
        TWCC_MAXCONNECTIONS => "the scanner is in use by another program".into(),
        TWCC_OPERATIONERROR => "the scanner reported an error".into(),
        TWCC_BADCAP | TWCC_CAPUNSUPPORTED => "the scanner does not offer that setting".into(),
        TWCC_BADVALUE => "the scanner refused a setting".into(),
        TWCC_SEQERROR => "TWAIN sequence error".into(),
        TWCC_DENIED => "access denied".into(),
        TWCC_PAPERJAM => "paper jam".into(),
        TWCC_PAPERDOUBLEFEED => "double feed (two sheets went through together)".into(),
        TWCC_CHECKDEVICEONLINE => "the scanner is off or not connected".into(),
        TWCC_INTERLOCK => "the scanner's cover is open".into(),
        TWCC_DAMAGEDCORNER => "a sheet has a damaged or folded corner".into(),
        TWCC_NOMEDIA => "there is no paper in the feeder".into(),
        other => format!("TWAIN condition {other}"),
    }
}

/// Runs `f` on a new thread that owns a hidden window (the sources' parent) with an open DSM
/// from `entry`, and returns its result. Every TWAIN call of `f` happens on that thread.
pub fn on_twain_thread<R, F>(
    entry: impl FnOnce() -> Result<Box<dyn Entry>, String> + Send + 'static,
    f: F,
) -> Result<R, String>
where
    R: Send + 'static,
    F: FnOnce(&mut Manager) -> Result<R, String> + Send + 'static,
{
    std::thread::Builder::new()
        .name("twain".into())
        .spawn(move || {
            let window = HiddenWindow::new()?;
            let mut m = Manager::open(entry()?, window.0)?;
            let r = f(&mut m);
            drop(m);
            drop(window);
            r
        })
        .map_err(|e| format!("cannot start the scanner thread: {e}"))?
        .join()
        .map_err(|_| "the scanner thread stopped unexpectedly".to_string())?
}

/// The real DSM as an [`Entry`].
pub fn dsm_entry() -> Result<Box<dyn Entry>, String> {
    Ok(Box::new(dsm()?))
}

/// The TWAIN sources this workstation's 64-bit DSM lists.
pub fn sources() -> Result<Vec<SourceInfo>, String> {
    on_twain_thread(dsm_entry, |m| m.sources())
}

/// Scans one batch from the TWAIN source named `source`.
pub fn scan(source: &str, settings: &Settings) -> Result<Scanned, String> {
    let (source, settings) = (source.to_string(), settings.clone());
    on_twain_thread(dsm_entry, move |m| m.scan(&source, &settings))
}

/// Sources installed only for 32-bit programs (`%SystemRoot%\twain_32`, Microsoft's WIA
/// bridge left out). Vibe Files is 64-bit and cannot load them; the Scanner row names them.
pub fn thirty_two_bit_only() -> Vec<String> {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    let base = Path::new(&root).join("twain_32");
    let mut out = Vec::new();
    let mut stack = vec![base.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ds"))
                && !p
                    .file_name()
                    .is_some_and(|n| n.eq_ignore_ascii_case("wiatwain.ds"))
            {
                // The folder under twain_32 names the maker or product.
                let name = p
                    .strip_prefix(&base)
                    .map(|r| r.with_extension("").display().to_string())
                    .unwrap_or_default();
                out.push(name);
            }
        }
    }
    out.sort();
    out
}

unsafe extern "system" fn wndproc(
    hwnd: windows::Win32::Foundation::HWND,
    msg: u32,
    wp: windows::Win32::Foundation::WPARAM,
    lp: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    unsafe { windows::Win32::UI::WindowsAndMessaging::DefWindowProcW(hwnd, msg, wp, lp) }
}

/// A hidden top-level window owned by the current thread.
struct HiddenWindow(*mut c_void);

impl HiddenWindow {
    fn new() -> Result<Self, String> {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::WindowsAndMessaging::*;
        use windows::core::w;
        static CLASS: OnceLock<()> = OnceLock::new();
        let instance =
            unsafe { GetModuleHandleW(None) }.map_err(|e| format!("window: {}", e.message()))?;
        CLASS.get_or_init(|| {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                lpszClassName: w!("VibeFilesTwain"),
                ..Default::default()
            };
            unsafe { RegisterClassW(&wc) };
        });
        let hwnd: HWND = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("VibeFilesTwain"),
                w!("Vibe Files scanner"),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                400,
                300,
                None,
                None,
                Some(instance.into()),
                None,
            )
        }
        .map_err(|e| format!("cannot create the scanner window: {}", e.message()))?;
        Ok(HiddenWindow(hwnd.0))
    }
}

impl Drop for HiddenWindow {
    fn drop(&mut self) {
        let _ = unsafe {
            windows::Win32::UI::WindowsAndMessaging::DestroyWindow(
                windows::Win32::Foundation::HWND(self.0),
            )
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_the_fake_sources() {
        let list = on_twain_thread(
            || {
                Ok(Box::new(fake::Fake::new(vec![
                    fake::Scanner::fi7160(),
                    fake::Scanner::virtual_source(),
                ])) as Box<dyn Entry>)
            },
            |m| m.sources(),
        )
        .unwrap();
        let names: Vec<&str> = list.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["fi-7160", "Remote Scan"]);
        assert_eq!(list[0].manufacturer, "FUJITSU");
        assert!(list[0].twain2 && !list[1].twain2);
    }

    #[test]
    fn no_sources_is_an_empty_list() {
        let list = on_twain_thread(
            || Ok(Box::new(fake::Fake::new(vec![])) as Box<dyn Entry>),
            |m| m.sources(),
        )
        .unwrap();
        assert!(list.is_empty());
    }

    /// The shipped 64-bit DSM loads, opens with the hidden window and lists this PC's sources
    /// (none on a build machine; the fi-7160's on an office PC).
    #[test]
    fn the_real_dsm_opens_and_lists() {
        let list = sources().unwrap();
        assert!(list.iter().all(|s| !s.name.is_empty()), "{list:?}");
        let _ = thirty_two_bit_only();
    }
}
