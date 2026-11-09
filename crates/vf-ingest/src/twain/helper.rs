//! The 32-bit scanning helper, `vibe-scan32.exe` (owner 2026-11-09). A remote-scanning set-up
//! (Citrix Workspace's scanner redirection, a Remote Desktop scanning utility) often reaches
//! 32-bit programs only: it hooks Windows' own 32-bit TWAIN manager (`twain_32.dll`) inside each
//! program, so no TWAIN source exists for the 64-bit manager Vibe Files uses. The helper is a
//! small 32-bit program shipped beside `vibe-files.exe` that lists and scans through the 32-bit
//! managers with this module's TWAIN code; Vibe Files runs it and builds the PDF as for any
//! TWAIN scan.
//!
//! Talk: `vibe-scan32 list` prints a [`ListReply`] (JSON) on its output; `vibe-scan32 scan
//! <request.json>` reads a [`ScanRequest`], writes each page's packed pixels to the request's
//! folder and prints a [`ScanReply`].

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::{Dsm, Entry, Page, Pixel, Scanned, Settings};

/// The helper's file name, beside `vibe-files.exe`.
pub const HELPER_EXE: &str = "vibe-scan32.exe";

/// A source a 32-bit manager lists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Listed {
    pub name: String,
    pub manufacturer: String,
    pub family: String,
    pub twain2: bool,
    /// The manager that lists it.
    pub dsm: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ListReply {
    pub sources: Vec<Listed>,
    /// What was tried, for Scanner details.
    pub details: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScanRequest {
    pub source: String,
    pub settings: Settings,
    /// Where the pages are written.
    pub out: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageFile {
    /// File name in the request's folder: the page's packed pixel rows.
    pub file: String,
    pub width: u32,
    pub height: u32,
    pub dpi: f32,
    pub pixel: Pixel,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ScanReply {
    pub pages: Vec<PageFile>,
    pub stopped: Option<String>,
    pub cancelled: bool,
    pub error: Option<String>,
}

// ---- In the helper (32-bit) ----

/// The 32-bit managers, in the order tried: Windows' own (what remote-scanning tools hook),
/// then a TWAINDSM.dll beside the helper or in Windows (a 32-bit program's System32 is
/// SysWOW64).
fn managers() -> Vec<PathBuf> {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    let mut v = vec![Path::new(&root).join("twain_32.dll")];
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    {
        v.push(dir.join("TWAINDSM.dll"));
    }
    v.push(Path::new(&root).join(r"System32\TWAINDSM.dll"));
    v
}

/// Each manager that loads, with what it lists (or why not).
/// A manager's file and what it gave: the loaded manager and its sources, or why not.
type Tried = (PathBuf, Result<(Dsm, Vec<super::SourceInfo>), String>);

fn each_manager() -> Vec<Tried> {
    let mut out = Vec::new();
    for path in managers() {
        if !path.is_file() {
            out.push((path, Err("missing".into())));
            continue;
        }
        let r = super::load(&path).and_then(|dsm| {
            super::on_twain_thread(move || Ok(Box::new(dsm) as Box<dyn Entry>), |m| m.sources())
                .map(|s| (dsm, s))
        });
        out.push((path, r));
    }
    out
}

fn list() -> ListReply {
    let mut reply = ListReply::default();
    reply.details.push(format!(
        "helper {} ({}-bit)",
        env!("CARGO_PKG_VERSION"),
        usize::BITS
    ));
    for (path, r) in each_manager() {
        let shown = format!("{} [{}]", path.display(), super::pe_bits(&path));
        match r {
            Ok((_, sources)) => {
                reply
                    .details
                    .push(format!("{shown}: {} source(s)", sources.len()));
                for s in sources {
                    reply.details.push(format!(
                        "  {} | maker {} | family {} | TWAIN {}",
                        s.name,
                        s.manufacturer,
                        s.family,
                        if s.twain2 { "2" } else { "1" }
                    ));
                    if !reply.sources.iter().any(|x| x.name == s.name) {
                        reply.sources.push(Listed {
                            name: s.name,
                            manufacturer: s.manufacturer,
                            family: s.family,
                            twain2: s.twain2,
                            dsm: path.display().to_string(),
                        });
                    }
                }
            }
            Err(e) => reply.details.push(format!("{shown}: {e}")),
        }
    }
    reply
}

fn scan(req: &ScanRequest) -> ScanReply {
    let found = each_manager().into_iter().find_map(|(_, r)| {
        r.ok()
            .filter(|(_, s)| s.iter().any(|x| x.name == req.source))
            .map(|(dsm, _)| dsm)
    });
    let Some(dsm) = found else {
        return ScanReply {
            error: Some(format!(
                "the scanner \"{}\" is not listed in this session",
                req.source
            )),
            ..ScanReply::default()
        };
    };
    let (source, settings) = (req.source.clone(), req.settings.clone());
    let scanned = super::on_twain_thread(
        move || Ok(Box::new(dsm) as Box<dyn Entry>),
        move |m| m.scan(&source, &settings),
    );
    let scanned = match scanned {
        Ok(s) => s,
        Err(e) => {
            return ScanReply {
                error: Some(e),
                ..ScanReply::default()
            };
        }
    };
    let mut reply = ScanReply {
        stopped: scanned.stopped,
        cancelled: scanned.cancelled,
        ..ScanReply::default()
    };
    for (i, p) in scanned.pages.into_iter().enumerate() {
        let file = format!("page-{:04}.raw", i + 1);
        if let Err(e) = std::fs::write(req.out.join(&file), &p.data) {
            reply.error = Some(format!("cannot keep page {}: {e}", i + 1));
            break;
        }
        reply.pages.push(PageFile {
            file,
            width: p.width,
            height: p.height,
            dpi: p.dpi,
            pixel: p.pixel,
        });
    }
    reply
}

/// The helper program: `list`, or `scan <request.json>`. Prints one JSON reply.
pub fn helper_main(args: &[String]) -> i32 {
    let json = match args.get(1).map(String::as_str) {
        Some("list") => serde_json::to_string(&list()),
        Some("scan") => {
            let req = args
                .get(2)
                .ok_or_else(|| "no request file".to_string())
                .and_then(|p| std::fs::read_to_string(p).map_err(|e| e.to_string()))
                .and_then(|t| serde_json::from_str::<ScanRequest>(&t).map_err(|e| e.to_string()));
            match req {
                Ok(r) => serde_json::to_string(&scan(&r)),
                Err(e) => serde_json::to_string(&ScanReply {
                    error: Some(format!("bad request: {e}")),
                    ..ScanReply::default()
                }),
            }
        }
        _ => {
            eprintln!("usage: {HELPER_EXE} list | scan <request.json>");
            return 2;
        }
    };
    match json {
        Ok(j) => {
            println!("{j}");
            0
        }
        Err(_) => 1,
    }
}

// ---- In Vibe Files (64-bit) ----

/// The helper beside the running program, when there is one.
pub fn helper_path() -> Option<PathBuf> {
    let p = vf_core::exe_dir()?.join(HELPER_EXE);
    p.is_file().then_some(p)
}

/// Runs the helper with `args`, without a console window, and reads its JSON reply. `timeout`:
/// ended after this long (a listing; a scan waits for the person at the scanner).
fn run<T: for<'de> Deserialize<'de>>(
    exe: &Path,
    args: &[&std::ffi::OsStr],
    timeout: Option<Duration>,
) -> Result<T, String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut child = std::process::Command::new(exe)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("cannot start {HELPER_EXE}: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("no output")?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(e) => return Err(format!("{HELPER_EXE}: {e}")),
        }
        if timeout.is_some_and(|t| started.elapsed() > t) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "{HELPER_EXE} did not answer within {} seconds",
                timeout.unwrap_or_default().as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let out = reader.join().unwrap_or_default();
    let line = out.lines().rev().find(|l| l.trim_start().starts_with('{'));
    let line = line.ok_or_else(|| format!("{HELPER_EXE} gave no answer"))?;
    serde_json::from_str(line).map_err(|e| format!("{HELPER_EXE} answered oddly: {e}"))
}

/// The 32-bit sources, from the helper. `None`: no helper beside the program.
pub fn list32() -> Option<Result<ListReply, String>> {
    let exe = helper_path()?;
    Some(run(&exe, &["list".as_ref()], Some(Duration::from_secs(45))))
}

/// Scans one batch from the 32-bit source `source` through the helper.
pub fn scan32(source: &str, settings: &Settings) -> Result<Scanned, String> {
    let exe = helper_path().ok_or_else(|| format!("{HELPER_EXE} is not beside the program"))?;
    let dir = std::env::temp_dir().join(format!("vibe-scan32-{}", uuid::Uuid::now_v7().simple()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    let result = (|| {
        let req = ScanRequest {
            source: source.to_string(),
            settings: settings.clone(),
            out: dir.clone(),
        };
        let req_path = dir.join("request.json");
        std::fs::write(
            &req_path,
            serde_json::to_string(&req).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        // The scanner's own window opens in the helper: let it come to the front.
        // SAFETY: plain Win32 call.
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(
                windows::Win32::UI::WindowsAndMessaging::ASFW_ANY,
            );
        }
        let reply: ScanReply = run(&exe, &["scan".as_ref(), req_path.as_os_str()], None)?;
        read_reply(&dir, reply)
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// The pages a [`ScanReply`] names, read back from `dir`.
fn read_reply(dir: &Path, reply: ScanReply) -> Result<Scanned, String> {
    let mut pages = Vec::new();
    for p in &reply.pages {
        let data = std::fs::read(dir.join(&p.file))
            .map_err(|e| format!("cannot read a scanned page: {e}"))?;
        if data.len() != p.pixel.row_bytes(p.width) * p.height as usize {
            return Err("a scanned page came back damaged".into());
        }
        pages.push(Page {
            width: p.width,
            height: p.height,
            dpi: p.dpi,
            pixel: p.pixel,
            data,
        });
    }
    // An error after some pages (a full disk): the pages are kept, the reason shown.
    if let Some(e) = reply.error {
        if pages.is_empty() {
            return Err(e);
        }
        return Ok(Scanned {
            pages,
            stopped: Some(e),
            cancelled: reply.cancelled,
        });
    }
    Ok(Scanned {
        pages,
        stopped: reply.stopped,
        cancelled: reply.cancelled,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_reads_its_pages_back_and_checks_their_size() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("page-0001.raw"), vec![7u8; 3 * 2]).unwrap();
        let page = PageFile {
            file: "page-0001.raw".into(),
            width: 3,
            height: 2,
            dpi: 200.0,
            pixel: Pixel::Gray,
        };
        let reply = ScanReply {
            pages: vec![page.clone()],
            stopped: Some("paper jam".into()),
            ..ScanReply::default()
        };
        // Through JSON, as between the two programs.
        let reply: ScanReply =
            serde_json::from_str(&serde_json::to_string(&reply).unwrap()).unwrap();
        let s = read_reply(dir.path(), reply).unwrap();
        assert_eq!(s.pages.len(), 1);
        assert_eq!((s.pages[0].width, s.pages[0].height), (3, 2));
        assert_eq!(s.stopped.as_deref(), Some("paper jam"));
        // A short file is refused, not drawn.
        std::fs::write(dir.path().join("page-0001.raw"), vec![7u8; 5]).unwrap();
        let bad = ScanReply {
            pages: vec![page],
            ..ScanReply::default()
        };
        assert!(read_reply(dir.path(), bad).is_err());
        // An error before any page is the error.
        let none = ScanReply {
            error: Some("the scanner is off".into()),
            ..ScanReply::default()
        };
        assert_eq!(
            read_reply(dir.path(), none).unwrap_err(),
            "the scanner is off"
        );
    }

    /// The built 32-bit helper answers a listing from this (64-bit) process: set `VF_SCAN32` to
    /// its path (`target\i686-pc-windows-msvc\release\vibe-scan32.exe`); skipped otherwise.
    #[test]
    fn the_built_helper_answers_a_listing() {
        let Some(exe) = std::env::var_os("VF_SCAN32").map(PathBuf::from) else {
            return;
        };
        let reply: ListReply =
            run(&exe, &["list".as_ref()], Some(Duration::from_secs(60))).unwrap();
        assert!(reply.details[0].contains("(32-bit)"), "{:?}", reply.details);
        assert!(
            reply
                .details
                .iter()
                .any(|l| l.contains("twain_32.dll [32-bit]: ")),
            "{:?}",
            reply.details
        );
    }

    #[test]
    fn a_request_goes_through_json_whole() {
        let req = ScanRequest {
            source: "Citrix TWAIN".into(),
            settings: Settings {
                feeder: Some(true),
                duplex: true,
                dpi: 300,
                pixel: Pixel::Bw,
                discard_blank: false,
                show_ui: true,
            },
            out: PathBuf::from(r"C:\Temp\x"),
        };
        let back: ScanRequest =
            serde_json::from_str(&serde_json::to_string(&req).unwrap()).unwrap();
        assert_eq!(back, req);
    }
}
