//! The 32-bit scanning helper, `vibe-scan32.exe` (owner 2026-11-09). A remote-scanning set-up
//! (Citrix Workspace's scanner redirection, a Remote Desktop scanning utility) often reaches
//! 32-bit programs only: it hooks Windows' own 32-bit TWAIN manager (`twain_32.dll`) inside each
//! program, so no TWAIN source exists for the 64-bit manager Vibe Files uses. The helper is a
//! small 32-bit program shipped beside `vibe-files.exe` that lists and scans through the 32-bit
//! managers with this module's TWAIN code; Vibe Files runs it and builds the PDF as for any
//! TWAIN scan.
//!
//! Since 2026-11-17 the 64-bit drivers run the same way: `vibe-files.exe --scan-helper …` is
//! this helper too ([`Helper::native`]). A scanner's driver is another firm's code loaded
//! into the program that uses it: a fault in it closed Vibe Files with whatever was open, and
//! one that never came back held the scanner for good. In a helper, a fault ends the helper
//! (the pages already scanned are read back) and a wait can be stopped by ending it.
//!
//! Talk: `vibe-scan32 list` prints a [`ListReply`] (JSON) on its output; `vibe-scan32 scan
//! <request.json>` reads a [`ScanRequest`], writes each page's packed pixels to the request's
//! folder and prints a [`ScanReply`].

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::{Dsm, Entry, Page, Pixel, Settings};

/// The helper's file name, beside `vibe-files.exe`.
pub const HELPER_EXE: &str = "vibe-scan32.exe";

/// `vibe-files.exe <this> list | scan <request.json>`: the program itself as the helper for
/// the 64-bit drivers. Looked for first thing in `main`, before anything else is opened.
pub const HELPER_ARG: &str = "--scan-helper";

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
    /// The helper's version (must be the program's: the request format follows it).
    #[serde(default)]
    pub version: String,
    pub sources: Vec<Listed>,
    /// What was tried, for Scanner details.
    pub details: Vec<String>,
    /// The 64-bit helper: where `TWAINDSM.dll` was looked for and what was found.
    #[serde(default)]
    pub manager: Vec<String>,
    /// The 64-bit helper: why no manager could be used.
    #[serde(default)]
    pub error: Option<String>,
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

/// The 32-bit managers, in the order tried, as (what is loaded, the file it is): Windows' own
/// `twain_32.dll` loaded by its bare name, as 32-bit programs load it (a remote-scanning hook
/// may watch for that name, QA), then a 32-bit TWAINDSM.dll in the `twain32` folder beside the
/// helper or in Windows (a 32-bit program's System32 is SysWOW64). The 64-bit TWAINDSM.dll
/// beside the helper is not tried.
fn managers() -> Vec<(PathBuf, PathBuf)> {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    let mut v = vec![(
        PathBuf::from("twain_32.dll"),
        Path::new(&root).join("twain_32.dll"),
    )];
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    {
        let p = dir.join(r"twain32\TWAINDSM.dll");
        v.push((p.clone(), p));
    }
    let p = Path::new(&root).join(r"System32\TWAINDSM.dll");
    v.push((p.clone(), p));
    v
}

/// Each manager that loads, with what it lists (or why not).
/// A manager's file and what it gave: the loaded manager and its sources, or why not.
type Tried = (PathBuf, Result<(Dsm, Vec<super::SourceInfo>), String>);

fn each_manager() -> Vec<Tried> {
    // The 64-bit program as its own helper: the one 64-bit manager it has always used.
    if usize::BITS == 64 {
        let r = super::dsm().and_then(|dsm| {
            super::on_twain_thread(move || Ok(Box::new(dsm) as Box<dyn Entry>), |m| m.sources())
                .map(|s| (dsm, s))
        });
        let path = super::DSM_PATH
            .get()
            .cloned()
            .unwrap_or_else(|| PathBuf::from("TWAINDSM.dll"));
        return vec![(path, r)];
    }
    let mut out = Vec::new();
    for (load, path) in managers() {
        if !path.is_file() {
            out.push((path, Err("missing".into())));
            continue;
        }
        let r = super::load(&load).and_then(|dsm| {
            super::on_twain_thread(move || Ok(Box::new(dsm) as Box<dyn Entry>), |m| m.sources())
                .map(|s| (dsm, s))
        });
        out.push((path, r));
    }
    out
}

fn list() -> ListReply {
    let mut reply = ListReply {
        version: env!("CARGO_PKG_VERSION").to_string(),
        ..ListReply::default()
    };
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
            Err(e) => {
                reply.details.push(format!("{shown}: {e}"));
                if usize::BITS == 64 {
                    reply.error = Some(e);
                }
            }
        }
    }
    if usize::BITS == 64 {
        reply.manager = super::dsm_report();
    }
    reply
}

/// Writes one scanned page into the request's folder: its packed pixel rows, then what it
/// is beside it (`page-NNNN.raw.json`, written last: a page with its note is whole). When
/// this program stops before it can answer, the pages already scanned are read back from
/// these ([`pages_left`]).
fn keep_page(out: &Path, n: usize, p: &Page) -> Result<PageFile, String> {
    let file = format!("page-{n:04}.raw");
    std::fs::write(out.join(&file), &p.data).map_err(|e| format!("cannot keep page {n}: {e}"))?;
    let page = PageFile {
        file,
        width: p.width,
        height: p.height,
        dpi: p.dpi,
        pixel: p.pixel,
    };
    let text = serde_json::to_string(&page).map_err(|e| e.to_string())?;
    std::fs::write(out.join(format!("{}.json", page.file)), text)
        .map_err(|e| format!("cannot keep page {n}: {e}"))?;
    Ok(page)
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
    let (source, settings, out) = (req.source.clone(), req.settings.clone(), req.out.clone());
    // Each page goes to disk as soon as it ends (QA: a long colour batch held whole would not
    // fit a 32-bit program's memory).
    let scanned = super::on_twain_thread(
        move || Ok(Box::new(dsm) as Box<dyn Entry>),
        move |m| {
            let mut files: Vec<PageFile> = Vec::new();
            let r = m.scan_each(&source, &settings, &mut |p| {
                files.push(keep_page(&out, files.len() + 1, &p)?);
                Ok(())
            });
            r.map(|s| (s, files))
        },
    );
    match scanned {
        Ok((s, pages)) => ScanReply {
            pages,
            stopped: s.stopped,
            cancelled: s.cancelled,
            error: None,
        },
        Err(e) => ScanReply {
            error: Some(e),
            ..ScanReply::default()
        },
    }
}

/// The helper program: `list`, or `scan <request.json>`. Prints one JSON reply.
pub fn helper_main(args: &[String]) -> i32 {
    // A fault in a driver ends this program at once, without Windows' "has stopped working"
    // window: nobody would see it (this program has no window of its own), and Vibe Files
    // would wait behind it for as long as it stayed up.
    // SAFETY: plain Win32 call.
    unsafe {
        use windows::Win32::System::Diagnostics::Debug::{
            SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SEM_NOOPENFILEERRORBOX, SetErrorMode,
        };
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX);
    }
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

/// The 32-bit helper beside the running program, when there is one.
pub fn helper_path() -> Option<PathBuf> {
    let p = vf_core::exe_dir()?.join(HELPER_EXE);
    p.is_file().then_some(p)
}

/// A helper program Vibe Files runs to reach scanners: the drivers load into it, not into
/// Vibe Files.
#[derive(Debug, Clone)]
pub struct Helper {
    exe: PathBuf,
    /// Arguments before the command.
    lead: Vec<OsString>,
    /// What it is called in a message.
    name: String,
}

impl Helper {
    /// `vibe-scan32.exe` beside the program: the 32-bit drivers.
    pub fn bits32() -> Option<Helper> {
        Some(Helper {
            exe: helper_path()?,
            lead: vec![],
            name: HELPER_EXE.into(),
        })
    }

    /// The running `vibe-files.exe` as its own helper: the 64-bit drivers. `None` in any
    /// other program (a test, a tool): those have no [`HELPER_ARG`].
    pub fn native() -> Option<Helper> {
        let exe = std::env::current_exe().ok()?;
        let is_app = exe
            .file_name()
            .is_some_and(|n| n.eq_ignore_ascii_case("vibe-files.exe"));
        is_app.then(|| Helper {
            exe,
            lead: vec![HELPER_ARG.into()],
            name: "the scanning helper".into(),
        })
    }

    /// The sources this helper's managers list.
    pub fn list(&self) -> Result<ListReply, String> {
        let r: ListReply = self.run(&["list".as_ref()], Some(Duration::from_secs(45)), None)?;
        // Another version's helper may not read this version's requests (QA).
        if r.version != env!("CARGO_PKG_VERSION") {
            return Err(format!(
                "{} is from another version ({}); put the one from this version's zip beside vibe-files.exe",
                self.name,
                if r.version.is_empty() {
                    "older"
                } else {
                    r.version.as_str()
                }
            ));
        }
        Ok(r)
    }
}

/// One helper at a time (QA: a listing must not reach the scanner while a scan uses it).
static ONE_HELPER: Mutex<()> = Mutex::new(());

/// A job that ends every helper when Vibe Files ends (closed, crashed or closed from another
/// session), so a scanner is never left held by an orphaned helper (QA).
fn kill_with_us(child: &std::process::Child) {
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    // The job's handle, kept open for the life of this process (as a number: HANDLE is not
    // Sync).
    static JOB: OnceLock<Option<isize>> = OnceLock::new();
    let job = JOB.get_or_init(|| {
        // SAFETY: plain Win32 calls; the handle is never closed (Windows closes it at exit,
        // which ends the helpers).
        unsafe {
            let job = CreateJobObjectW(None, None).ok()?;
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const std::ffi::c_void,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
            .ok()?;
            Some(job.0 as isize)
        }
    });
    if let Some(job) = job {
        // SAFETY: both handles are live; failure leaves the helper outside the job.
        let _ = unsafe {
            AssignProcessToJobObject(
                HANDLE(*job as *mut std::ffi::c_void),
                HANDLE(child.as_raw_handle()),
            )
        };
    }
}

impl Helper {
    /// Runs the helper with `args`, without a console window, and reads its JSON reply.
    /// `timeout`: ended after this long (a listing; a scan waits for the person at the
    /// scanner). `stop`: ended as soon as this is set (the person pressed Stop).
    fn run<T: for<'de> Deserialize<'de>>(
        &self,
        args: &[&std::ffi::OsStr],
        timeout: Option<Duration>,
        stop: Option<&AtomicBool>,
    ) -> Result<T, String> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let name = &self.name;
        let _one = ONE_HELPER.lock().unwrap_or_else(|e| e.into_inner());
        let mut child = std::process::Command::new(&self.exe)
            .args(&self.lead)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("cannot start {name}: {e}"))?;
        kill_with_us(&child);
        let mut stdout = child.stdout.take().ok_or("no output")?;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut s = String::new();
            let _ = stdout.read_to_string(&mut s);
            let _ = tx.send(s);
        });
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(e) => return Err(format!("{name}: {e}")),
            }
            if stop.is_some_and(|s| s.load(Ordering::SeqCst)) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(STOPPED.into());
            }
            if timeout.is_some_and(|t| started.elapsed() > t) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{name} did not answer within {} seconds",
                    timeout.unwrap_or_default().as_secs()
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        // The answer is in by the time the helper has ended, unless a program it started
        // keeps its output open (QA): then not waited for long.
        let out = rx.recv_timeout(Duration::from_secs(5)).unwrap_or_default();
        let line = out.lines().rev().find(|l| l.trim_start().starts_with('{'));
        let Some(line) = line else {
            // Ended without an answer: a fault in the driver ends it with Windows' code for
            // the fault (0xC0000005 and the like), said so the cause can be looked up.
            return Err(match status.code() {
                Some(code) if code != 0 => format!(
                    "the scanner's driver stopped working ({name} ended with code 0x{:08X})",
                    code as u32
                ),
                _ => format!("{name} gave no answer"),
            });
        };
        serde_json::from_str(line).map_err(|e| format!("{name} answered oddly: {e}"))
    }

    /// Scans one batch from `source` through the helper. The pages stay in files (see
    /// [`ScannedFiles`]): a long batch is never in this program's memory whole. `stop`:
    /// set by the person; the helper is ended and the pages scanned so far are kept.
    pub fn scan(
        &self,
        source: &str,
        settings: &Settings,
        stop: &AtomicBool,
    ) -> Result<ScannedFiles, String> {
        let dir = std::env::temp_dir().join(format!("vibe-scan-{}", uuid::Uuid::now_v7().simple()));
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
        let reply = (|| {
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
            self.run::<ScanReply>(&["scan".as_ref(), req_path.as_os_str()], None, Some(stop))
        })();
        let files = match reply {
            Ok(reply) => {
                // Whole pages only, up to the first that is not (a short file is not drawn).
                let named = reply.pages.len();
                let pages: Vec<PageFile> = reply
                    .pages
                    .into_iter()
                    .take_while(|p| is_whole(&dir, p))
                    .collect();
                let damaged = (pages.len() < named).then(|| {
                    format!(
                        "page {} came back damaged; it and the pages after it are not in the scan",
                        pages.len() + 1
                    )
                });
                match reply.error {
                    // An error before any page is the error.
                    Some(e) if pages.is_empty() => Err(e),
                    // An error after some pages (a full disk): the pages are kept.
                    e => Ok(ScannedFiles {
                        stopped: e.or(damaged).or(reply.stopped),
                        cancelled: reply.cancelled,
                        pages,
                        dir: dir.clone(),
                    }),
                }
            }
            // The helper was stopped, or stopped by itself (a fault in the driver): the
            // pages scanned before that are paper someone fed. They are kept, with the
            // reason.
            Err(why) => {
                let pages = pages_left(&dir);
                let by_person = why == STOPPED;
                if pages.is_empty() && !by_person {
                    Err(why)
                } else {
                    let n = pages.len();
                    Ok(ScannedFiles {
                        stopped: (n > 0).then(|| {
                            format!(
                                "{why}; the {n} page{} scanned before that {} kept",
                                if n == 1 { "" } else { "s" },
                                if n == 1 { "is" } else { "are" }
                            )
                        }),
                        cancelled: n == 0,
                        pages,
                        dir: dir.clone(),
                    })
                }
            }
        };
        if !matches!(&files, Ok(f) if !f.pages.is_empty()) {
            let _ = std::fs::remove_dir_all(&dir);
        }
        files
    }
}

/// What [`Helper::run`] says when the person stopped it.
const STOPPED: &str = "the scan was stopped";

/// What a scan through a helper produced: the pages as files in `dir`, read one at a time
/// when the PDF is made. The folder is the caller's to remove ([`ScannedFiles::discard`])
/// once the scan is safe somewhere else.
#[derive(Debug)]
pub struct ScannedFiles {
    pub dir: PathBuf,
    pub pages: Vec<PageFile>,
    /// Why the batch stopped early; the pages before it are kept.
    pub stopped: Option<String>,
    /// Cancelled (in the driver's window, or by Stop) before any page.
    pub cancelled: bool,
}

impl ScannedFiles {
    /// Page `i` (0-based), read from its file.
    pub fn page(&self, i: usize) -> Result<Page, String> {
        let p = self.pages.get(i).ok_or("no such page")?;
        let data = std::fs::read(self.dir.join(&p.file))
            .map_err(|e| format!("cannot read scanned page {}: {e}", i + 1))?;
        if data.len() != p.pixel.row_bytes(p.width) * p.height as usize {
            return Err(format!("scanned page {} came back damaged", i + 1));
        }
        Ok(Page {
            width: p.width,
            height: p.height,
            dpi: p.dpi,
            pixel: p.pixel,
            data,
        })
    }

    /// Removes the page files.
    pub fn discard(&self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Whether the page's file is there and as long as its sizes say (its length is asked, the
/// file is not read).
fn is_whole(dir: &Path, p: &PageFile) -> bool {
    let want = (p.pixel.row_bytes(p.width) as u64).checked_mul(p.height as u64);
    let have = std::fs::metadata(dir.join(&p.file)).map(|m| m.len()).ok();
    want.is_some() && want == have
}

/// The whole pages the helper left in `dir`, in order, up to the first that is missing or
/// not whole: each page's file with what it is beside it (`page-NNNN.raw.json`).
fn pages_left(dir: &Path) -> Vec<PageFile> {
    let mut pages = Vec::new();
    for n in 1.. {
        let file = format!("page-{n:04}.raw");
        let Some(p) = std::fs::read(dir.join(format!("{file}.json")))
            .ok()
            .and_then(|b| serde_json::from_slice::<PageFile>(&b).ok())
        else {
            break;
        };
        // The name is this program's own, whatever the note says.
        let p = PageFile { file, ..p };
        if !is_whole(dir, &p) {
            break;
        }
        pages.push(p);
    }
    pages
}

/// The 32-bit sources, from the helper. `None`: no helper beside the program.
pub fn list32() -> Option<Result<ListReply, String>> {
    Some(Helper::bits32()?.list())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This test program as a helper: `stand_in_helper` below, told what to do by
    /// `VF_STAND_IN`.
    fn stand_in(what: &str) -> Helper {
        // SAFETY: the tests that set it hold `STAND_INS`, and each sets it just before
        // starting its own child.
        unsafe { std::env::set_var("VF_STAND_IN", what) };
        Helper {
            exe: std::env::current_exe().unwrap(),
            lead: [
                "--exact",
                "twain::helper::tests::stand_in_helper",
                "--ignored",
                "--nocapture",
            ]
            .map(OsString::from)
            .to_vec(),
            name: "the stand-in".into(),
        }
    }

    static STAND_INS: Mutex<()> = Mutex::new(());

    fn settings() -> Settings {
        Settings {
            feeder: Some(true),
            duplex: true,
            dpi: 100,
            pixel: Pixel::Gray,
            discard_blank: false,
            show_ui: false,
        }
    }

    /// Not a test: the helper the tests below run. `ok:N` scans N pages and answers;
    /// `die:N` scans N pages, starts another and ends as a faulting driver ends a program;
    /// `hang:N` scans N pages and never answers.
    #[test]
    #[ignore]
    fn stand_in_helper() {
        let Ok(what) = std::env::var("VF_STAND_IN") else {
            return;
        };
        let (mode, n) = what.split_once(':').unwrap();
        let n: usize = n.parse().unwrap();
        let req_path = std::env::args().next_back().unwrap();
        let out = match std::fs::read_to_string(req_path) {
            Ok(t) => serde_json::from_str::<ScanRequest>(&t).unwrap().out,
            // A listing: no request.
            Err(_) => std::env::temp_dir(),
        };
        let page = Page {
            width: 16,
            height: 4,
            dpi: 100.0,
            pixel: Pixel::Gray,
            data: vec![200; 16 * 4],
        };
        let pages: Vec<PageFile> = (1..=n)
            .map(|i| keep_page(&out, i, &page).unwrap())
            .collect();
        match mode {
            "ok" => {
                let reply = ScanReply {
                    pages,
                    ..ScanReply::default()
                };
                println!("{}", serde_json::to_string(&reply).unwrap());
            }
            "die" => {
                // The page being written when the driver faulted: no note, half its rows.
                std::fs::write(out.join(format!("page-{:04}.raw", n + 1)), [0u8; 20]).unwrap();
                std::process::exit(0xC000_0005u32 as i32);
            }
            _ => {
                // Its pages are written: said, for the test that then presses Stop.
                if let Ok(ready) = std::env::var("VF_STAND_IN_READY") {
                    std::fs::write(ready, "ready").unwrap();
                }
                std::thread::sleep(Duration::from_secs(600))
            }
        }
    }

    #[test]
    fn a_scan_comes_back_as_page_files() {
        let _one = STAND_INS.lock().unwrap_or_else(|e| e.into_inner());
        let s = stand_in("ok:3")
            .scan("x", &settings(), &AtomicBool::new(false))
            .unwrap();
        assert_eq!(s.pages.len(), 3);
        assert!(s.stopped.is_none() && !s.cancelled);
        let p = s.page(2).unwrap();
        assert_eq!((p.width, p.height, p.data.len()), (16, 4, 64));
        // A file cut short after the scan is refused, not drawn.
        std::fs::write(s.dir.join(&s.pages[0].file), [7u8; 5]).unwrap();
        assert!(s.page(0).is_err());
        s.discard();
        assert!(!s.dir.exists());
    }

    /// A driver that faults ends the helper, not this program: the pages scanned before it
    /// come back with the reason and Windows' code for the fault.
    #[test]
    fn a_helper_that_dies_part_way_gives_its_pages_and_the_reason() {
        let _one = STAND_INS.lock().unwrap_or_else(|e| e.into_inner());
        let s = stand_in("die:3")
            .scan("x", &settings(), &AtomicBool::new(false))
            .unwrap();
        assert_eq!(s.pages.len(), 3, "the whole pages, not the one cut short");
        let why = s.stopped.clone().unwrap();
        assert!(why.contains("stopped working"), "{why}");
        assert!(why.contains("0xC0000005"), "{why}");
        assert!(
            why.contains("3 pages scanned before that are kept"),
            "{why}"
        );
        s.discard();
        // With no page at all it is an error, and nothing is left behind.
        let e = stand_in("die:0")
            .scan("x", &settings(), &AtomicBool::new(false))
            .unwrap_err();
        assert!(e.contains("0xC0000005"), "{e}");
    }

    /// A helper that never answers is ended when the person presses Stop, promptly, and
    /// the pages scanned so far are kept.
    #[test]
    fn a_helper_that_never_answers_is_stopped() {
        let _one = STAND_INS.lock().unwrap_or_else(|e| e.into_inner());
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let t = tempfile::tempdir().unwrap();
        let ready = t.path().join("ready");
        // SAFETY: as in `stand_in`.
        unsafe { std::env::set_var("VF_STAND_IN_READY", &ready) };
        let helper = stand_in("hang:2");
        let presser = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                // Stop is pressed once the stand-in has scanned its pages, however long a
                // loaded machine takes to start it.
                let waited = Instant::now();
                while !ready.is_file() && waited.elapsed() < Duration::from_secs(120) {
                    std::thread::sleep(Duration::from_millis(50));
                }
                stop.store(true, Ordering::SeqCst);
                Instant::now()
            })
        };
        let s = helper.scan("x", &settings(), &stop).unwrap();
        let pressed = presser.join().unwrap();
        assert!(
            pressed.elapsed() < Duration::from_secs(5),
            "stopped at once"
        );
        assert_eq!(s.pages.len(), 2);
        assert!(
            s.stopped
                .as_deref()
                .unwrap()
                .contains("the scan was stopped")
        );
        assert!(!s.cancelled);
        s.discard();
        // And a listing that never answers gives up by itself.
        // SAFETY: as in `stand_in`.
        unsafe { std::env::remove_var("VF_STAND_IN_READY") };
        let e = stand_in("hang:0")
            .run::<ListReply>(&["list".as_ref()], Some(Duration::from_secs(2)), None)
            .unwrap_err();
        assert!(e.contains("did not answer"), "{e}");
    }

    /// The built 32-bit helper answers a listing from this (64-bit) process: set `VF_SCAN32` to
    /// its path (`target\i686-pc-windows-msvc\release\vibe-scan32.exe`); skipped otherwise.
    #[test]
    fn the_built_helper_answers_a_listing() {
        let Some(exe) = std::env::var_os("VF_SCAN32").map(PathBuf::from) else {
            return;
        };
        let helper = Helper {
            exe,
            lead: vec![],
            name: HELPER_EXE.into(),
        };
        let reply: ListReply = helper
            .run(&["list".as_ref()], Some(Duration::from_secs(60)), None)
            .unwrap();
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

    /// When the helper stops before it answers, the pages it had scanned are found in its
    /// folder, up to the first one that is not whole.
    #[test]
    fn pages_the_helper_left_are_found() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path();
        let page = Page {
            width: 16,
            height: 4,
            dpi: 300.0,
            pixel: Pixel::Gray,
            data: vec![200; 64],
        };
        assert!(pages_left(dir).is_empty());
        for n in 1..=4 {
            keep_page(dir, n, &page).unwrap();
        }
        // The page being written when it stopped.
        std::fs::write(dir.join("page-0003.raw"), [0u8; 10]).unwrap();
        let left = pages_left(dir);
        assert_eq!(left.len(), 2, "the whole pages before the one cut short");
        assert_eq!((left[0].width, left[0].height), (16, 4));
    }
}
