//! Scanner (P02 step 2, PA-19). Two ways in, listed together:
//! - **TWAIN** ([`crate::twain`]): the office's Fujitsu fi-7160 through PaperStream IP, or a
//!   remote-scanning utility's virtual source in a server session (QUESTIONS Q-02).
//! - **WIA**, through Windows' own scanner API (`Windows.Devices.Scanners`, which drives WIA
//!   2.0 drivers), for scanners whose driver offers it.
//!
//! A scan's pages become one PDF in the Scans folder through [`assemble_frames`] (blank pages
//! dropped by ink coverage, deskewed, at most 300 dpi). The Scans folder worker takes it from
//! there: barcode split, Filer's inbox.

use std::path::{Path, PathBuf};

use image::{DynamicImage, imageops};
use serde::{Deserialize, Serialize};

use crate::images::{self, Frame, Normalized};

/// Where the paper is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Document feeder (ADF).
    Feeder,
    Flatbed,
    /// Whatever the scanner picks.
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Color {
    Color,
    Gray,
    /// Black and white.
    Mono,
}

/// A scan profile. Kept per workstation in `config.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub source: Source,
    /// Both sides (feeder only).
    #[serde(default)]
    pub duplex: bool,
    pub dpi: u32,
    pub color: Color,
    #[serde(default = "yes")]
    pub deskew: bool,
    /// Drop pages whose ink coverage is under `blank_ink`.
    #[serde(default = "yes")]
    pub drop_blank: bool,
    /// Share of dark pixels (0–1) under which a page is blank.
    #[serde(default = "blank_ink")]
    pub blank_ink: f32,
}

fn yes() -> bool {
    true
}
fn blank_ink() -> f32 {
    BLANK_INK
}

/// Default blank threshold. Higher than the barcode split's (a scanned "blank" page carries
/// speckle and bleed-through from the other side).
pub const BLANK_INK: f32 = 0.004;

/// Profiles offered when the workstation has none of its own.
pub fn default_profiles() -> Vec<Profile> {
    let p = |name: &str, source, duplex, dpi, color| Profile {
        name: name.into(),
        source,
        duplex,
        dpi,
        color,
        deskew: true,
        drop_blank: true,
        blank_ink: BLANK_INK,
    };
    vec![
        p(
            "Stack, both sides (gray 300)",
            Source::Feeder,
            true,
            300,
            Color::Gray,
        ),
        p(
            "Stack, one side (gray 300)",
            Source::Feeder,
            false,
            300,
            Color::Gray,
        ),
        p(
            "Stack, both sides (color 200)",
            Source::Feeder,
            true,
            200,
            Color::Color,
        ),
        p(
            "Flatbed (color 300)",
            Source::Flatbed,
            false,
            300,
            Color::Color,
        ),
    ]
}

// ---------------------------------------------------------------------------------------------
// Assembly (no scanner needed)

/// What one scan produced.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Assembled {
    /// Pages scanned.
    pub scanned: usize,
    /// Pages dropped as blank.
    pub blank: usize,
    pub normalized: Normalized,
    /// Why the scanner stopped early (paper jam …); the pages before it are in the PDF.
    pub stopped: Option<String>,
}

impl Assembled {
    pub fn note(&self) -> String {
        let mut s = self.normalized.note();
        if self.blank > 0 {
            s.push_str(&format!(" · {} blank dropped", self.blank));
        }
        if let Some(why) = &self.stopped {
            s.push_str(&format!(
                " · the scanner stopped: {why} (the pages before it are kept; scan the rest)"
            ));
        }
        s
    }
}

/// Ink coverage of a scanned page, ignoring a 4 % margin (feeder edge shadows).
pub fn page_ink(rgb: &image::RgbImage) -> f32 {
    let small = DynamicImage::ImageRgb8(rgb.clone())
        .resize(800, 800, imageops::FilterType::Triangle)
        .to_luma8();
    let (w, h) = small.dimensions();
    let (mx, my) = (w / 25, h / 25);
    if w <= 2 * mx || h <= 2 * my {
        return crate::stack::ink(small.as_raw());
    }
    let inner = imageops::crop_imm(&small, mx, my, w - 2 * mx, h - 2 * my).to_image();
    crate::stack::ink(inner.as_raw())
}

/// Turns the page images of one scan, in order, into one PDF at `out`.
pub fn assemble(pages: &[PathBuf], profile: &Profile, out: &Path) -> Result<Assembled, String> {
    let mut frames: Vec<Frame> = Vec::new();
    for p in pages {
        frames.extend(images::decode(p)?);
    }
    assemble_frames(frames, profile, out)
}

/// Turns the pages of one scan, in order, into one PDF at `out`.
pub fn assemble_frames(
    mut frames: Vec<Frame>,
    profile: &Profile,
    out: &Path,
) -> Result<Assembled, String> {
    let scanned = frames.len();
    if profile.drop_blank {
        frames.retain(|f| page_ink(&f.rgb) >= profile.blank_ink);
    }
    let blank = scanned - frames.len();
    if frames.is_empty() {
        return Err(format!("all {scanned} scanned pages were blank"));
    }
    let normalized = images::frames_to_pdf(frames, profile.deskew, out)?;
    Ok(Assembled {
        scanned,
        blank,
        normalized,
        stopped: None,
    })
}

// ---------------------------------------------------------------------------------------------
// Devices: TWAIN sources and WIA scanners

/// How a scanner is driven.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Twain,
    Wia,
}

/// A scanner this workstation can use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// `twain:<product name>` or the WIA device id. What `config.toml` remembers.
    pub id: String,
    /// Shown in the Scanner row, with the kind marked.
    pub name: String,
    pub kind: Kind,
}

const TWAIN_PREFIX: &str = "twain:";

/// What [`devices`] found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Devices {
    /// TWAIN sources first (the office's scanners), then WIA scanners.
    pub list: Vec<Device>,
    /// Why a kind is missing or a scanner cannot be used (no TWAIN manager, 32-bit-only
    /// sources, WIA failed). Shown under the Scanner row.
    pub notes: Vec<String>,
    /// Everything the lookup checked, as lines of text: Scanner details, which a person can
    /// copy and send when a scanner (a remote-scanning utility's in a server session) is
    /// missing and they cannot open a command prompt there.
    pub details: Vec<String>,
}

/// The scanners on this workstation: TWAIN sources (64-bit) and WIA scanners.
pub fn devices() -> Devices {
    let mut d = Devices::default();
    let mut details = session_details();
    match crate::twain::sources() {
        Ok(sources) => {
            details.push(format!(
                "TWAIN sources the 64-bit manager lists: {}",
                sources.len()
            ));
            for s in &sources {
                details.push(format!(
                    "  {} | maker {} | family {} | TWAIN {}",
                    s.name,
                    s.manufacturer,
                    s.family,
                    if s.twain2 { "2" } else { "1" }
                ));
            }
            d.list.extend(sources.into_iter().map(|s| Device {
                id: format!("{TWAIN_PREFIX}{}", s.name),
                name: format!("{} (TWAIN)", s.name),
                kind: Kind::Twain,
            }))
        }
        Err(e) => {
            details.push(format!("TWAIN sources: none listed ({e})"));
            d.notes.push(e)
        }
    }
    details.push("TWAIN manager (TWAINDSM.dll):".into());
    details.extend(
        crate::twain::dsm_report()
            .into_iter()
            .map(|l| format!("  {l}")),
    );
    for folder in ["twain_64", "twain_32"] {
        let files = crate::twain::source_files(folder);
        let count = if files.is_empty() {
            "none".to_string()
        } else {
            files.len().to_string()
        };
        details.push(format!(r"Source files in %SystemRoot%\{folder}: {count}"));
        details.extend(files.into_iter().map(|f| format!("  {f}")));
    }
    details.extend(installed_scan_software());
    details.extend(scan_processes());
    details.extend(dsm_log());
    // Drivers such as PaperStream IP install both; name the 32-bit ones only when no TWAIN
    // source could be listed (that is why the scanner is missing).
    let old = crate::twain::thirty_two_bit_only();
    if !old.is_empty() && !d.list.iter().any(|x| x.kind == Kind::Twain) {
        d.notes.push(format!(
            "Installed for 32-bit programs only, so Vibe Files (64-bit) cannot use: {}. \
             Install the scanner's 64-bit TWAIN driver.",
            old.join(", ")
        ));
    }
    match wia_devices() {
        Ok(w) => {
            details.push(format!("Windows (WIA) scanners: {}", w.len()));
            details.extend(w.iter().map(|x| format!("  {} | {}", x.name, x.id)));
            d.list.extend(w)
        }
        Err(e) => {
            details.push(format!("Windows (WIA) scanners: cannot list ({e})"));
            d.notes.push(e)
        }
    }
    d.details = details;
    d
}

/// Who and where: program, Windows login, computer, and whether this is a Remote Desktop
/// session (a remote-scanning utility's source usually exists only inside one).
fn session_details() -> Vec<String> {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_REMOTESESSION};
    let env = |k: &str| std::env::var(k).unwrap_or_else(|_| "?".into());
    // SAFETY: GetSystemMetrics has no preconditions.
    let remote = unsafe { GetSystemMetrics(SM_REMOTESESSION) } != 0;
    vec![
        format!(
            "Vibe Files {} ({}-bit)",
            env!("CARGO_PKG_VERSION"),
            usize::BITS
        ),
        format!(
            "Computer {} | login {} | session {} | {}",
            env("COMPUTERNAME"),
            env("USERNAME"),
            session_label(),
            if remote {
                "Remote Desktop session"
            } else {
                "local session"
            }
        ),
    ]
}

/// This process's Windows session: its number, and its name when Windows gives one
/// (`SESSIONNAME`, e.g. `RDP-Tcp#3`; Citrix published programs often have none).
fn session_label() -> String {
    use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows::Win32::System::Threading::GetCurrentProcessId;
    let mut sid = 0u32;
    // SAFETY: plain Win32 calls; `sid` receives the session id.
    let number = unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut sid) }
        .ok()
        .map(|()| sid.to_string());
    let name = std::env::var("SESSIONNAME").ok().filter(|n| !n.is_empty());
    match (number, name) {
        (Some(n), Some(name)) => format!("{n} ({name})"),
        (Some(n), None) => n,
        (None, Some(name)) => name,
        (None, None) => "?".into(),
    }
}

/// Words that name scanning software in folder and registry key names.
const SCAN_WORDS: [&str; 6] = ["scan", "twain", "voscan", "paperstream", "fujitsu", "pfu"];

fn names_scanning(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    SCAN_WORDS.iter().any(|w| n.contains(w))
}

/// Scanning software installed on this computer: program folders and registry keys whose
/// names mention scanning, and any TWAIN source or manager file inside those folders.
fn installed_scan_software() -> Vec<String> {
    let mut out = Vec::new();
    let mut folders = Vec::new();
    for var in ["ProgramFiles", "ProgramFiles(x86)", "ProgramData"] {
        let Some(base) = std::env::var_os(var) else {
            continue;
        };
        let Ok(rd) = std::fs::read_dir(PathBuf::from(base)) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() && names_scanning(&e.file_name().to_string_lossy()) {
                folders.push(p);
            }
        }
    }
    out.push(format!("Scanning program folders: {}", folders.len()));
    for f in &folders {
        out.push(format!("  {}", f.display()));
        for hit in twain_files_in(f, 4) {
            out.push(format!(
                "    {} [{}]",
                hit.display(),
                crate::twain::pe_bits(&hit)
            ));
        }
    }
    let keys = scan_registry_keys();
    out.push(format!("Scanning registry keys: {}", keys.len()));
    out.extend(keys.into_iter().map(|k| format!("  {k}")));
    out
}

/// Running programs whose names mention scanning (a remote-scanning utility's agent has to run
/// in the person's session for its scanner to appear there), with their session number.
fn scan_processes() -> Vec<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::ProcessStatus::{EnumProcesses, GetModuleBaseNameW};
    use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows::Win32::System::Threading::{
        GetCurrentProcessId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
    };
    let mut pids = vec![0u32; 4096];
    let mut used = 0u32;
    // SAFETY: `pids` holds `len * 4` bytes; `used` receives the bytes written.
    let ok = unsafe {
        EnumProcesses(
            pids.as_mut_ptr(),
            (pids.len() * std::mem::size_of::<u32>()) as u32,
            &mut used,
        )
    };
    if ok.is_err() {
        return vec!["Scanning programs running: cannot list programs".into()];
    }
    pids.truncate(used as usize / std::mem::size_of::<u32>());
    let session_of = |pid: u32| {
        let mut sid = 0u32;
        // SAFETY: `sid` receives the session id.
        unsafe { ProcessIdToSessionId(pid, &mut sid) }
            .ok()
            .map(|()| sid)
    };
    // SAFETY: no preconditions.
    let mine = session_of(unsafe { GetCurrentProcessId() });
    let mut found = Vec::new();
    for pid in pids.into_iter().filter(|p| *p != 0) {
        // SAFETY: the handle is closed below; failures (other users' programs) are skipped.
        let Ok(h) = (unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ,
                false,
                pid,
            )
        }) else {
            continue;
        };
        let mut buf = [0u16; 260];
        // SAFETY: `buf` is a writable UTF-16 buffer.
        let n = unsafe { GetModuleBaseNameW(h, None, &mut buf) } as usize;
        // SAFETY: opened above.
        let _ = unsafe { CloseHandle(h) };
        let name = String::from_utf16_lossy(&buf[..n.min(buf.len())]);
        if n > 0 && names_scanning(&name) {
            let session = session_of(pid);
            let here = if session.is_some() && session == mine {
                " (this session)"
            } else {
                ""
            };
            let sid = session.map_or("?".to_string(), |s| s.to_string());
            found.push(format!("  {name} | pid {pid} | session {sid}{here}"));
        }
    }
    found.sort();
    let mut out = vec![format!(
        "Scanning programs running: {} (this session is {})",
        found.len(),
        mine.map_or("?".to_string(), |s| s.to_string())
    )];
    out.extend(found);
    out
}

/// The TWAIN manager's own log from its last start (it lists each source it tried and why it
/// left one out), last 80 lines.
fn dsm_log() -> Vec<String> {
    let path = crate::twain::dsm_log_path();
    match std::fs::read(&path) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
            let skip = lines.len().saturating_sub(80);
            let mut out = vec![format!(
                "TWAIN manager log ({}, last {} lines):",
                path.display(),
                lines.len() - skip
            )];
            out.extend(lines[skip..].iter().map(|l| format!("  {l}")));
            out
        }
        Err(_) => vec![format!(
            "TWAIN manager log: none written ({}); the manager may not have started",
            path.display()
        )],
    }
}

/// `.ds` sources and `TWAINDSM.dll` copies under `dir`, `depth` folders deep at most.
fn twain_files_in(dir: &Path, depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), 0)];
    while let Some((d, level)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            // Folder links are not followed (a loop would never end).
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                if level < depth {
                    stack.push((p, level + 1));
                }
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ds"))
                || p.file_name()
                    .is_some_and(|n| n.eq_ignore_ascii_case("twaindsm.dll"))
            {
                out.push(p);
                if out.len() >= 50 {
                    return out;
                }
            }
        }
    }
    out.sort();
    out
}

/// Registry keys under SOFTWARE (64-bit, 32-bit and this login's) whose names mention
/// scanning, with up to 20 of their sub-keys; makers' keys are searched one level down.
fn scan_registry_keys() -> Vec<String> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    let mut out = Vec::new();
    for (hive, hive_name, base) in [
        (HKEY_LOCAL_MACHINE, "HKLM", r"SOFTWARE"),
        (HKEY_LOCAL_MACHINE, "HKLM", r"SOFTWARE\WOW6432Node"),
        (HKEY_CURRENT_USER, "HKCU", r"Software"),
    ] {
        // Classes holds COM registrations, not products; WOW6432Node is read as its own base.
        for top in reg_subkeys(hive, base).into_iter().filter(|t| {
            !t.eq_ignore_ascii_case("Classes") && !t.eq_ignore_ascii_case("WOW6432Node")
        }) {
            let path = format!(r"{base}\{top}");
            if names_scanning(&top) {
                out.push(format!(r"{hive_name}\{path}"));
                for sub in reg_subkeys(hive, &path).into_iter().take(20) {
                    out.push(format!(r"{hive_name}\{path}\{sub}"));
                }
            } else {
                for sub in reg_subkeys(hive, &path) {
                    if names_scanning(&sub) {
                        out.push(format!(r"{hive_name}\{path}\{sub}"));
                    }
                }
            }
        }
    }
    out
}

/// The names of the sub-keys of `hive\path` (empty when it cannot be opened).
fn reg_subkeys(hive: windows::Win32::System::Registry::HKEY, path: &str) -> Vec<String> {
    use windows::Win32::System::Registry::{
        HKEY, KEY_READ, RegCloseKey, RegEnumKeyExW, RegOpenKeyExW,
    };
    use windows::core::{HSTRING, PWSTR};
    let mut key = HKEY::default();
    // SAFETY: `key` receives an open handle that is closed below.
    if unsafe { RegOpenKeyExW(hive, &HSTRING::from(path), None, KEY_READ, &mut key) }.is_err() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for i in 0.. {
        let mut buf = [0u16; 256];
        let mut len = buf.len() as u32;
        // SAFETY: `buf` holds `len` UTF-16 units.
        let rc = unsafe {
            RegEnumKeyExW(
                key,
                i,
                Some(PWSTR(buf.as_mut_ptr())),
                &mut len,
                None,
                None,
                None,
                None,
            )
        };
        if rc.is_err() {
            break;
        }
        out.push(String::from_utf16_lossy(&buf[..len as usize]));
    }
    // SAFETY: `key` was opened above.
    let _ = unsafe { RegCloseKey(key) };
    out
}

/// The device `remembered` names, found again by its name after the list changed (a remote
/// session reconnects and its virtual source comes back), else the first one.
pub fn pick<'a>(list: &'a [Device], remembered: Option<&str>) -> Option<&'a Device> {
    remembered
        .and_then(|id| list.iter().find(|d| d.id == id))
        .or_else(|| list.first())
}

/// Profile → TWAIN settings. `driver_ui`: the driver's own window decides instead.
pub fn twain_settings(profile: &Profile, driver_ui: bool) -> crate::twain::Settings {
    use crate::twain::Pixel;
    crate::twain::Settings {
        feeder: match profile.source {
            Source::Feeder => Some(true),
            Source::Flatbed => Some(false),
            Source::Auto => None,
        },
        duplex: profile.duplex,
        dpi: profile.dpi,
        pixel: match profile.color {
            Color::Color => Pixel::Rgb,
            Color::Gray => Pixel::Gray,
            Color::Mono => Pixel::Bw,
        },
        discard_blank: profile.drop_blank,
        show_ui: driver_ui,
    }
}

/// The profile a TWAIN scan's pages are assembled with. With the driver's own window, the driver
/// (PaperStream's "blank page skip") decides what is blank: every page it sends is kept, front
/// and back, so a faint back side is never dropped by a second guess (owner 2026-11-04).
pub fn for_twain(profile: &Profile, driver_ui: bool) -> Profile {
    let mut p = profile.clone();
    if driver_ui {
        p.drop_blank = false;
    }
    p
}

/// The pages of one TWAIN scan as one PDF at `out`.
pub fn assemble_twain(
    scanned: crate::twain::Scanned,
    profile: &Profile,
    out: &Path,
) -> Result<Assembled, String> {
    if scanned.cancelled {
        return Err("scan cancelled".into());
    }
    if scanned.pages.is_empty() {
        return Err(match scanned.stopped {
            Some(why) => format!("the scanner stopped before the first page: {why}"),
            None => "the scanner returned no pages".into(),
        });
    }
    // Each packed page is dropped as its frame is built.
    let frames = scanned.pages.into_iter().map(|p| p.to_frame()).collect();
    let mut a = assemble_frames(frames, profile, out)?;
    a.stopped = scanned.stopped;
    Ok(a)
}

// ---------------------------------------------------------------------------------------------
// WIA

fn err(what: &str) -> impl Fn(windows::core::Error) -> String + '_ {
    move |e| format!("{what}: {}", e.message())
}

/// The WIA scanners on this workstation.
pub fn wia_devices() -> Result<Vec<Device>, String> {
    use windows::Devices::Enumeration::DeviceInformation;
    use windows::Devices::Scanners::ImageScanner;
    let selector = ImageScanner::GetDeviceSelector().map_err(err("scanner query"))?;
    let all = DeviceInformation::FindAllAsyncAqsFilter(&selector)
        .and_then(|op| op.join())
        .map_err(err("cannot list scanners"))?;
    let mut out = Vec::new();
    for i in 0..all.Size().map_err(err("cannot list scanners"))? {
        let d = all.GetAt(i).map_err(err("cannot list scanners"))?;
        out.push(Device {
            id: d.Id().map_err(err("scanner id"))?.to_string(),
            name: format!("{} (WIA)", d.Name().map_err(err("scanner name"))?),
            kind: Kind::Wia,
        });
    }
    Ok(out)
}

/// Scans with `profile` into `dir` and returns the page image files, in page order.
pub fn scan_pages(device_id: &str, profile: &Profile, dir: &Path) -> Result<Vec<PathBuf>, String> {
    use windows::Devices::Scanners::{
        ImageScanner, ImageScannerColorMode, ImageScannerFormat, ImageScannerResolution,
        ImageScannerScanSource,
    };
    use windows::Storage::StorageFolder;
    use windows::core::HSTRING;

    let scanner = ImageScanner::FromIdAsync(&HSTRING::from(device_id))
        .and_then(|op| op.join())
        .map_err(err("cannot open the scanner"))?;
    let source = match profile.source {
        Source::Feeder => ImageScannerScanSource::Feeder,
        Source::Flatbed => ImageScannerScanSource::Flatbed,
        Source::Auto => ImageScannerScanSource::AutoConfigured,
    };
    if !scanner
        .IsScanSourceSupported(source)
        .map_err(err("scanner"))?
    {
        return Err(format!(
            "this scanner has no {}",
            match profile.source {
                Source::Feeder => "document feeder",
                Source::Flatbed => "flatbed",
                Source::Auto => "automatic mode",
            }
        ));
    }
    let mode = match profile.color {
        Color::Color => ImageScannerColorMode::Color,
        Color::Gray => ImageScannerColorMode::Grayscale,
        Color::Mono => ImageScannerColorMode::Monochrome,
    };
    let res = ImageScannerResolution {
        DpiX: profile.dpi as f32,
        DpiY: profile.dpi as f32,
    };
    // Lossless first: the pages are compressed once, when the PDF is written.
    let formats = [
        ImageScannerFormat::Png,
        ImageScannerFormat::Tiff,
        ImageScannerFormat::Jpeg,
    ];
    macro_rules! configure {
        ($c:expr) => {{
            let c = $c.map_err(err("scanner settings"))?;
            if c.IsColorModeSupported(mode)
                .map_err(err("scanner settings"))?
            {
                c.SetColorMode(mode).map_err(err("color mode"))?;
            }
            c.SetDesiredResolution(res).map_err(err("resolution"))?;
            if let Some(f) = formats
                .iter()
                .find(|f| c.IsFormatSupported(**f).unwrap_or(false))
            {
                c.SetFormat(*f).map_err(err("file format"))?;
            }
            c
        }};
    }
    match profile.source {
        Source::Feeder => {
            let c = configure!(scanner.FeederConfiguration());
            if profile.duplex {
                if !c.CanScanDuplex().map_err(err("duplex"))? {
                    return Err("this scanner's feeder cannot scan both sides".into());
                }
                c.SetDuplex(true).map_err(err("duplex"))?;
            }
            c.SetMaxNumberOfPages(0).map_err(err("page count"))?;
        }
        Source::Flatbed => {
            configure!(scanner.FlatbedConfiguration());
        }
        Source::Auto => {}
    }

    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let abs = std::path::absolute(dir).map_err(|e| e.to_string())?;
    let folder = StorageFolder::GetFolderFromPathAsync(&HSTRING::from(abs.as_os_str()))
        .and_then(|op| op.join())
        .map_err(err("scan folder"))?;
    let result = scanner
        .ScanFilesToFolderAsync(source, &folder)
        .and_then(|op| op.join())
        .map_err(err("scan failed (is there paper in the feeder?)"))?;
    let files = result.ScannedFiles().map_err(err("scanned files"))?;
    let mut out = Vec::new();
    for i in 0..files.Size().map_err(err("scanned files"))? {
        let f = files.GetAt(i).map_err(err("scanned files"))?;
        out.push(PathBuf::from(
            f.Path().map_err(err("scanned file"))?.to_os_string(),
        ));
    }
    if out.is_empty() {
        return Err("the scanner returned no pages".into());
    }
    Ok(out)
}

/// Scans with `profile` and leaves one PDF in `scans_dir` for the Scans folder worker.
/// Returns its path and what was done. `driver_ui`: for a TWAIN source, show the driver's
/// own window and let its settings and profiles drive the scan.
pub fn scan_to(
    device_id: &str,
    profile: &Profile,
    driver_ui: bool,
    scans_dir: &Path,
) -> Result<(PathBuf, Assembled), String> {
    if let Some(source) = device_id.strip_prefix(TWAIN_PREFIX) {
        let scanned = crate::twain::scan(source, &twain_settings(profile, driver_ui))?;
        let profile = for_twain(profile, driver_ui);
        return deliver(scans_dir, |tmp| assemble_twain(scanned, &profile, tmp));
    }
    // Hidden from the worker (it skips dot names and folders).
    let work = scans_dir.join(format!(".scan-{}", uuid::Uuid::now_v7().simple()));
    let r = scan_pages(device_id, profile, &work)
        .and_then(|pages| deliver(scans_dir, |tmp| assemble(&pages, profile, tmp)));
    match &r {
        Ok(_) => {
            let _ = std::fs::remove_dir_all(&work);
        }
        Err(e) => {
            // Scanned pages are paper someone fed: never thrown away because the PDF could not
            // be built. They go to `_failed/<date>/` with the error (QA I8).
            let has_pages = std::fs::read_dir(&work)
                .map(|mut d| d.next().is_some())
                .unwrap_or(false);
            if !has_pages {
                let _ = std::fs::remove_dir_all(&work);
            } else if let Some(kept) = keep_failed_pages(&work, scans_dir, e) {
                return Err(format!(
                    "{e} (the scanned pages are kept in {})",
                    kept.display()
                ));
            } else {
                // Could not be moved: the pages stay where they are, never deleted (QA2 #7).
                return Err(format!(
                    "{e} (the scanned pages are kept in {})",
                    work.display()
                ));
            }
        }
    }
    r
}

/// Moves a failed scan's page images to `<scans>/_failed/<date>/scan-…/` with the error.
/// `None` when there were no pages (nothing to keep) or the move failed.
fn keep_failed_pages(work: &Path, scans_dir: &Path, error: &str) -> Option<PathBuf> {
    let has_pages = std::fs::read_dir(work).ok()?.flatten().next().is_some();
    if !has_pages {
        return None;
    }
    let day = vf_core::now_rfc3339()
        .get(..10)
        .unwrap_or("undated")
        .to_string();
    let dir = scans_dir.join(crate::pipeline::FAILED_DIR).join(day);
    std::fs::create_dir_all(&dir).ok()?;
    let name = work
        .file_name()?
        .to_string_lossy()
        .trim_start_matches('.')
        .to_string();
    let dst = crate::pipeline::rename_free(work, &dir, &name).ok()?;
    let _ = std::fs::write(dst.join("error.txt"), error);
    Some(dst)
}

/// Writes the scan's PDF under a hidden name, then gives it its `Scan <date>.pdf` name.
pub fn deliver(
    scans_dir: &Path,
    make: impl FnOnce(&Path) -> Result<Assembled, String>,
) -> Result<(PathBuf, Assembled), String> {
    let out = crate::pipeline::free_name(scans_dir, &format!("Scan {}.pdf", stamp()));
    let tmp = scans_dir.join(format!(".scan-{}.pdf.tmp", uuid::Uuid::now_v7().simple()));
    let a = make(&tmp).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    let name = out
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let out = crate::pipeline::rename_free(&tmp, scans_dir, &name).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("cannot write {}: {e}", out.display())
    })?;
    Ok((out, a))
}

/// Date and time (UTC) for a file name, `2026-11-03 141502`.
fn stamp() -> String {
    let now = vf_core::now_rfc3339();
    now.get(..19)
        .unwrap_or("undated")
        .replace('T', " ")
        .replace(':', "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn page(text: bool) -> RgbImage {
        let mut p = RgbImage::from_pixel(850, 1100, Rgb([250, 250, 250]));
        // Feeder shadow along the edge: not ink.
        for y in 0..1100 {
            for x in 0..12 {
                p.put_pixel(x, y, Rgb([40, 40, 40]));
            }
        }
        if text {
            for line in 0..30 {
                for y in 100 + line * 30..108 + line * 30 {
                    for x in 100..750 {
                        p.put_pixel(x, y, Rgb([10, 10, 10]));
                    }
                }
            }
        }
        p
    }

    #[test]
    fn blank_pages_are_dropped_by_ink() {
        assert!(page_ink(&page(false)) < BLANK_INK, "edge shadow is not ink");
        assert!(page_ink(&page(true)) > BLANK_INK);

        let t = tempfile::tempdir().unwrap();
        let files: Vec<PathBuf> = [true, false, true, false, true]
            .iter()
            .enumerate()
            .map(|(i, text)| {
                let p = t.path().join(format!("page{i}.png"));
                page(*text).save(&p).unwrap();
                p
            })
            .collect();
        let profile = &default_profiles()[0];
        let out = t.path().join("scan.pdf");
        let a = assemble(&files, profile, &out).unwrap();
        assert_eq!((a.scanned, a.blank, a.normalized.pages), (5, 2, 3));
        assert_eq!(a.note(), "3 pages · 2 blank dropped");
        assert_eq!(lopdf::Document::load(&out).unwrap().get_pages().len(), 3);

        let keep = Profile {
            drop_blank: false,
            ..profile.clone()
        };
        assert_eq!(assemble(&files, &keep, &out).unwrap().normalized.pages, 5);
        let only_blank = [files[1].clone()];
        assert!(assemble(&only_blank, profile, &out).is_err());
    }

    #[test]
    fn profiles_round_trip_with_defaults() {
        let p: Profile =
            toml::from_str("name='x'\nsource='feeder'\ndpi=300\ncolor='gray'").unwrap();
        assert!(p.deskew && p.drop_blank && !p.duplex);
        assert_eq!(p.blank_ink, BLANK_INK);
        let back: Profile =
            toml::from_str(&toml::to_string(&default_profiles()[0]).unwrap()).unwrap();
        assert_eq!(back, default_profiles()[0]);
    }

    /// Lists this workstation's scanners without failing when there are none (CI, laptops).
    #[test]
    fn device_list_works_with_or_without_a_scanner() {
        let d = devices();
        assert!(d.list.iter().all(|d| !d.id.is_empty()), "{d:?}");
        // The shipped TWAIN manager loads in debug builds: no note about it missing.
        assert!(
            !d.notes.iter().any(|n| n.contains("TWAINDSM")),
            "{:?}",
            d.notes
        );
        // Scanner details name every place looked at, and which manager loaded.
        let text = d.details.join("\n");
        println!("{text}");
        for want in [
            "-bit)",
            "session",
            "TWAIN manager (TWAINDSM.dll):",
            " - loaded",
            r"%SystemRoot%\twain_64",
            r"%SystemRoot%\twain_32",
            "Scanning program folders:",
            "Scanning registry keys:",
            "Windows (WIA) scanners:",
        ] {
            assert!(text.contains(want), "{want} missing from:\n{text}");
        }
    }

    #[test]
    fn a_remembered_scanner_is_found_by_id_else_the_first() {
        let dev = |id: &str, kind| Device {
            id: id.into(),
            name: id.into(),
            kind,
        };
        let list = [dev("twain:fi-7160", Kind::Twain), dev("wia-1", Kind::Wia)];
        assert_eq!(pick(&list, Some("wia-1")).unwrap().id, "wia-1");
        assert_eq!(pick(&list, Some("twain:gone")).unwrap().id, "twain:fi-7160");
        assert_eq!(pick(&list, None).unwrap().id, "twain:fi-7160");
        assert!(pick(&[], None).is_none());
    }

    /// PA-19 step 3: a TWAIN batch from the fake fi-7160 (duplex, blank backs) becomes one PDF
    /// through the same assembly as every other scan, blank backs dropped.
    #[test]
    fn twain_batch_becomes_one_pdf_with_blank_backs_dropped() {
        use crate::twain::fake::{Fake, Scanner};
        use crate::twain::{Entry, on_twain_thread};
        let profile = default_profiles()[0].clone(); // stack, both sides, gray 300
        assert!(profile.duplex && profile.drop_blank);
        let s = twain_settings(&profile, false);
        let fake = Fake::new(vec![Scanner {
            pages: 6,
            blank: vec![1, 3, 5],
            ..Scanner::fi7160()
        }]);
        let scanned = on_twain_thread(
            move || Ok(Box::new(fake) as Box<dyn Entry>),
            move |m| m.scan("fi-7160", &s),
        )
        .unwrap();
        assert_eq!(scanned.pages.len(), 6);
        let t = tempfile::tempdir().unwrap();
        let (out, a) = deliver(t.path(), |tmp| assemble_twain(scanned, &profile, tmp)).unwrap();
        assert!(
            out.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("Scan ")
        );
        assert_eq!((a.scanned, a.blank, a.normalized.pages), (6, 3, 3));
        assert_eq!(lopdf::Document::load(&out).unwrap().get_pages().len(), 3);
        // Nothing hidden left behind.
        assert_eq!(std::fs::read_dir(t.path()).unwrap().count(), 1);
    }

    /// With the driver's own window, a two-sided batch keeps every page the driver sends, faint
    /// back sides included (owner 2026-11-04: all pages processed).
    #[test]
    fn driver_window_scan_keeps_every_front_and_back() {
        use crate::twain::fake::{Fake, Scanner};
        use crate::twain::{Entry, on_twain_thread};
        let profile = default_profiles()[0].clone(); // both sides, drop blank
        let s = twain_settings(&profile, true);
        assert!(s.show_ui);
        let fake = Fake::new(vec![Scanner {
            pages: 8,
            blank: vec![1, 3, 5, 7],
            ..Scanner::fi7160()
        }]);
        let scanned = on_twain_thread(
            move || Ok(Box::new(fake) as Box<dyn Entry>),
            move |m| m.scan("fi-7160", &s),
        )
        .unwrap();
        assert_eq!(scanned.pages.len(), 8, "fronts and backs transferred");
        let t = tempfile::tempdir().unwrap();
        let p = for_twain(&profile, true);
        let (out, a) = deliver(t.path(), |tmp| assemble_twain(scanned, &p, tmp)).unwrap();
        assert_eq!((a.scanned, a.blank, a.normalized.pages), (8, 0, 8));
        assert_eq!(lopdf::Document::load(&out).unwrap().get_pages().len(), 8);
        // Without the driver's window the profile's blank drop still applies.
        assert!(for_twain(&profile, false).drop_blank);
    }

    #[test]
    fn twain_cancel_and_jam_are_reported() {
        use crate::twain::Scanned;
        let profile = &default_profiles()[0];
        let t = tempfile::tempdir().unwrap();
        let out = t.path().join("x.pdf");
        let cancelled = Scanned {
            cancelled: true,
            ..Default::default()
        };
        assert_eq!(
            assemble_twain(cancelled, profile, &out).unwrap_err(),
            "scan cancelled"
        );
        let jammed = Scanned {
            stopped: Some("paper jam".into()),
            ..Default::default()
        };
        assert_eq!(
            assemble_twain(jammed, profile, &out).unwrap_err(),
            "the scanner stopped before the first page: paper jam"
        );
        let a = Assembled {
            stopped: Some("paper jam".into()),
            ..Default::default()
        };
        assert!(a.note().contains("the scanner stopped: paper jam"));
    }
}
