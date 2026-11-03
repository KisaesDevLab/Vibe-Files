//! An in-process stand-in for the DSM and its sources, for tests (PA-19 step 5: "or a virtual
//! source"). The 64-bit DSM loads sources only from `C:\Windows\twain_64`, which needs an
//! administrator to fill, so the gate drives [`super::Manager`] against this instead. It
//! keeps TWAIN's state machine (3 DSM open, 4 source open, 5 enabled, 6 transfer ready,
//! 7 transferring) and refuses out-of-order triplets with `TWCC_SEQERROR`.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use super::Entry;
use super::ffi::*;

/// Window message the fake posts to deliver its messages to a TWAIN 1.x event loop.
const WM_FAKE: u32 = 0x8000 + 7;

/// One fake source.
#[derive(Clone, Debug)]
pub struct Scanner {
    pub name: String,
    pub manufacturer: String,
    /// TWAIN 2: messages by callback. Otherwise through the window (`MSG_PROCESSEVENT`), with
    /// unknown page length and pending count (-1), as older and virtual sources do.
    pub twain2: bool,
    pub feeder: bool,
    pub duplex: bool,
    /// Sides in the feeder.
    pub pages: usize,
    /// Pages (0-based) that come out blank.
    pub blank: Vec<usize>,
    /// The feeder jams after this many pages.
    pub jam_after: Option<usize>,
    /// The user closes the driver's window without scanning.
    pub cancel_in_ui: bool,
}

impl Scanner {
    pub fn fi7160() -> Self {
        Scanner {
            name: "fi-7160".into(),
            manufacturer: "FUJITSU".into(),
            twain2: true,
            feeder: true,
            duplex: true,
            pages: 4,
            blank: vec![],
            jam_after: None,
            cancel_in_ui: false,
        }
    }

    /// A remote-scanning utility's virtual source (TWAIN 1.x, events through the window).
    pub fn virtual_source() -> Self {
        Scanner {
            name: "Remote Scan".into(),
            manufacturer: "Example".into(),
            twain2: false,
            ..Self::fi7160()
        }
    }

    fn identity(&self, id: u32) -> Identity {
        let mut i = Identity {
            id,
            protocol_major: if self.twain2 { 2 } else { 1 },
            protocol_minor: if self.twain2 { 4 } else { 9 },
            supported_groups: DG_CONTROL | DG_IMAGE | if self.twain2 { DF_DS2 } else { 0 },
            manufacturer: str32(&self.manufacturer),
            product_family: str32("Fake"),
            product_name: str32(&self.name),
            ..Identity::default()
        };
        i.version.major = 1;
        i
    }
}

/// What the app did, for the tests to check after the TWAIN thread has finished.
#[derive(Debug, Default, Clone)]
pub struct Log {
    /// Capabilities set, in order: (cap, item).
    pub caps: Vec<(u16, u32)>,
    pub show_ui: Option<bool>,
    /// TWAIN state when the app closed the DSM (3 = everything was closed in order).
    pub closed_from: Option<u8>,
    /// Out-of-order triplets the fake refused.
    pub seq_errors: usize,
    pub callback_registered: bool,
}

/// The fake DSM.
pub struct Fake {
    scanners: Vec<Scanner>,
    /// TWAIN state (2 = DSM loaded, closed; 3 open; 4 source open; 5 enabled; 6 ready;
    /// 7 transferring).
    state: u8,
    next: usize,
    last_cc: u16,
    log: Arc<Mutex<Log>>,
    /// The open source: index and id.
    open: Option<(usize, u32)>,
    caps: HashMap<u16, u32>,
    callback: Option<EntryProc>,
    parent: *mut c_void,
    /// Pages ended so far.
    sent: usize,
    /// Rows of the current page already sent.
    row: u32,
    event: Option<u16>,
}

// SAFETY: the parent window pointer is only used on the thread that makes the calls.
unsafe impl Send for Fake {}

impl Fake {
    pub fn new(scanners: Vec<Scanner>) -> Self {
        Fake {
            scanners,
            state: 2,
            next: 0,
            last_cc: TWCC_SUCCESS,
            log: Arc::default(),
            open: None,
            caps: HashMap::new(),
            callback: None,
            parent: std::ptr::null_mut(),
            sent: 0,
            row: 0,
            event: None,
        }
    }

    /// The log, shared with the test.
    pub fn log(&self) -> Arc<Mutex<Log>> {
        self.log.clone()
    }

    fn fail(&mut self, cc: u16) -> u16 {
        self.last_cc = cc;
        TWRC_FAILURE
    }

    fn seq(&mut self) -> u16 {
        self.log.lock().unwrap().seq_errors += 1;
        self.fail(TWCC_SEQERROR)
    }

    fn scanner(&self) -> &Scanner {
        &self.scanners[self.open.expect("a source is open").0]
    }

    fn dpi(&self) -> f32 {
        self.caps
            .get(&ICAP_XRESOLUTION)
            .map_or(100.0, |v| Fix32::from_item(*v).to_f32())
    }

    fn pixel(&self) -> u16 {
        self.caps
            .get(&ICAP_PIXELTYPE)
            .map_or(TWPT_GRAY, |v| *v as u16)
    }

    /// A 2 x 2.5 inch page at the set resolution.
    fn size(&self) -> (u32, u32) {
        let d = self.dpi();
        ((2.0 * d) as u32, (2.5 * d) as u32)
    }

    /// Whether pixel (x, y) of page `n` is dark: bands whose spacing depends on the page.
    pub fn dark(&self, n: usize, x: u32, y: u32) -> bool {
        let (w, _) = self.size();
        !self.scanner().blank.contains(&n)
            && x > w / 10
            && x < w - w / 10
            && (y / 6).is_multiple_of(n as u32 + 2)
    }

    /// Row `y` of page `n` in the current pixel type, unpadded.
    fn row_bytes(&self, n: usize, y: u32) -> Vec<u8> {
        let (w, _) = self.size();
        match self.pixel() {
            TWPT_BW => {
                let mut r = vec![0xffu8; w.div_ceil(8) as usize];
                for x in 0..w {
                    if self.dark(n, x, y) {
                        r[x as usize / 8] &= !(0x80 >> (x % 8));
                    }
                }
                r
            }
            TWPT_RGB => (0..w)
                .flat_map(|x| {
                    if self.dark(n, x, y) {
                        [20, 30, 140]
                    } else {
                        [250, 250, 250]
                    }
                })
                .collect(),
            _ => (0..w)
                .map(|x| if self.dark(n, x, y) { 15 } else { 245 })
                .collect(),
        }
    }

    /// Delivers a message to the app: by callback (TWAIN 2) or through the window.
    fn notify(&mut self, msg: u16) {
        if msg == MSG_XFERREADY {
            self.state = 6;
        }
        match self.callback {
            Some(cb) => unsafe {
                let mut id = Identity::default();
                cb(
                    &mut id,
                    std::ptr::null_mut(),
                    DG_CONTROL,
                    DAT_NULL,
                    msg,
                    std::ptr::null_mut(),
                );
            },
            None => {
                self.event = Some(msg);
                unsafe {
                    let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                        Some(windows::Win32::Foundation::HWND(self.parent)),
                        WM_FAKE,
                        Default::default(),
                        Default::default(),
                    );
                }
            }
        }
    }

    /// Triplets to the DSM itself.
    unsafe fn dsm(&mut self, origin: *mut Identity, dat: u16, msg: u16, data: *mut c_void) -> u16 {
        match (dat, msg) {
            (DAT_PARENT, MSG_OPENDSM) => {
                if self.state != 2 {
                    return self.seq();
                }
                if data.is_null() || unsafe { *(data as *mut *mut c_void) }.is_null() {
                    return self.fail(TWCC_BADVALUE);
                }
                unsafe {
                    (*origin).id = 1;
                    (*origin).supported_groups |= DF_DSM2;
                }
                self.state = 3;
                TWRC_SUCCESS
            }
            (DAT_PARENT, MSG_CLOSEDSM) => {
                self.log.lock().unwrap().closed_from = Some(self.state);
                if self.state != 3 {
                    return self.seq();
                }
                self.state = 2;
                TWRC_SUCCESS
            }
            (DAT_ENTRYPOINT, MSG_GET) => {
                let ep = data as *mut EntryPoint;
                unsafe {
                    (*ep).dsm_entry = None;
                    (*ep).mem_allocate = Some(mem_allocate);
                    (*ep).mem_free = Some(mem_free);
                    (*ep).mem_lock = Some(mem_lock);
                    (*ep).mem_unlock = Some(mem_unlock);
                }
                TWRC_SUCCESS
            }
            (DAT_IDENTITY, MSG_GETFIRST | MSG_GETNEXT) => {
                if self.state < 3 {
                    return self.seq();
                }
                if msg == MSG_GETFIRST {
                    if self.scanners.is_empty() {
                        return self.fail(TWCC_NODS);
                    }
                    self.next = 0;
                }
                let Some(s) = self.scanners.get(self.next) else {
                    return TWRC_ENDOFLIST;
                };
                unsafe { *(data as *mut Identity) = s.identity(100 + self.next as u32) };
                self.next += 1;
                TWRC_SUCCESS
            }
            (DAT_IDENTITY, MSG_OPENDS) => {
                if self.state != 3 {
                    return self.seq();
                }
                let id = data as *mut Identity;
                let name = read_str32(unsafe { &(*id).product_name });
                let Some(i) = self.scanners.iter().position(|s| s.name == name) else {
                    return self.fail(TWCC_NODS);
                };
                unsafe { (*id).id = 100 + i as u32 };
                self.open = Some((i, 100 + i as u32));
                self.caps.clear();
                self.callback = None;
                self.state = 4;
                TWRC_SUCCESS
            }
            (DAT_IDENTITY, MSG_CLOSEDS) => {
                if self.state != 4 {
                    return self.seq();
                }
                self.open = None;
                self.state = 3;
                TWRC_SUCCESS
            }
            (DAT_STATUS, MSG_GET) => {
                unsafe {
                    *(data as *mut Status) = Status {
                        condition_code: self.last_cc,
                        data: 0,
                    }
                };
                self.last_cc = TWCC_SUCCESS;
                TWRC_SUCCESS
            }
            _ => self.fail(TWCC_BADPROTOCOL),
        }
    }

    /// Triplets to the open source.
    unsafe fn ds(&mut self, dat: u16, msg: u16, data: *mut c_void) -> u16 {
        match (dat, msg) {
            (DAT_STATUS, MSG_GET) => unsafe { self.dsm(std::ptr::null_mut(), dat, msg, data) },
            (DAT_CALLBACK2, MSG_REGISTER_CALLBACK) => {
                if self.state != 4 {
                    return self.seq();
                }
                if !self.scanner().twain2 {
                    return self.fail(TWCC_BADPROTOCOL);
                }
                let cb = unsafe { (data as *mut Callback2).read_unaligned() };
                // SAFETY: the app passes its TWAIN callback.
                self.callback =
                    Some(unsafe { std::mem::transmute::<*mut c_void, EntryProc>(cb.proc_) });
                self.log.lock().unwrap().callback_registered = true;
                TWRC_SUCCESS
            }
            (DAT_CAPABILITY, MSG_SET) => {
                if self.state != 4 {
                    return self.fail(TWCC_CAPSEQERROR);
                }
                let c = unsafe { (data as *mut Capability).read_unaligned() };
                if c.con_type != TWON_ONEVALUE {
                    return self.fail(TWCC_BADVALUE);
                }
                let one = unsafe { (c.container as *mut OneValue).read_unaligned() };
                self.log.lock().unwrap().caps.push((c.cap, one.item));
                let s = self.scanner();
                let refused = match c.cap {
                    CAP_FEEDERENABLED => one.item != 0 && !s.feeder,
                    CAP_DUPLEXENABLED => one.item != 0 && !s.duplex,
                    ICAP_XFERMECH => one.item as u16 != TWSX_MEMORY,
                    _ => false,
                };
                if refused {
                    return self.fail(TWCC_BADVALUE);
                }
                self.caps.insert(c.cap, one.item);
                TWRC_SUCCESS
            }
            (DAT_USERINTERFACE, MSG_ENABLEDS) => {
                if self.state != 4 {
                    return self.seq();
                }
                let ui = unsafe { (data as *mut UserInterface).read_unaligned() };
                self.parent = ui.parent;
                self.log.lock().unwrap().show_ui = Some(ui.show_ui != 0);
                self.state = 5;
                self.sent = 0;
                self.row = 0;
                let first = if ui.show_ui != 0 && self.scanner().cancel_in_ui {
                    MSG_CLOSEDSREQ
                } else {
                    MSG_XFERREADY
                };
                self.notify(first);
                TWRC_SUCCESS
            }
            (DAT_USERINTERFACE, MSG_DISABLEDS) => {
                if self.state != 5 {
                    return self.seq();
                }
                self.state = 4;
                TWRC_SUCCESS
            }
            (DAT_EVENT, MSG_PROCESSEVENT) => {
                if self.state < 5 {
                    return self.seq();
                }
                let ev = data as *mut Event;
                let m =
                    unsafe { (*ev).event as *const windows::Win32::UI::WindowsAndMessaging::MSG };
                if unsafe { (*m).message } == WM_FAKE
                    && let Some(e) = self.event.take()
                {
                    unsafe { (*ev).message = e };
                    return TWRC_DSEVENT;
                }
                unsafe { (*ev).message = MSG_NULL };
                TWRC_NOTDSEVENT
            }
            (DAT_IMAGEINFO, MSG_GET) => {
                if self.state != 6 {
                    return self.seq();
                }
                let (w, h) = self.size();
                let px = self.pixel();
                let bits = if px == TWPT_BW { 1 } else { 8 };
                let spp = if px == TWPT_RGB { 3 } else { 1 };
                let res = Fix32::from_f32(self.dpi());
                let info = ImageInfo {
                    x_resolution: res,
                    y_resolution: res,
                    image_width: w as i32,
                    image_length: if self.scanner().twain2 { h as i32 } else { -1 },
                    samples_per_pixel: spp,
                    bits_per_sample: [bits, bits, bits, 0, 0, 0, 0, 0],
                    bits_per_pixel: bits * spp,
                    planar: 0,
                    pixel_type: px as i16,
                    compression: TWCP_NONE,
                };
                unsafe { (data as *mut ImageInfo).write_unaligned(info) };
                TWRC_SUCCESS
            }
            (DAT_SETUPMEMXFER, MSG_GET) => {
                if self.state < 4 {
                    return self.seq();
                }
                let s = SetupMemXfer {
                    min_buf_size: 1024,
                    max_buf_size: 1 << 20,
                    preferred: 16 * 1024,
                };
                unsafe { (data as *mut SetupMemXfer).write_unaligned(s) };
                TWRC_SUCCESS
            }
            (DAT_IMAGEMEMXFER, MSG_GET) => {
                if self.state != 6 && self.state != 7 {
                    return self.seq();
                }
                if self.scanner().jam_after == Some(self.sent) {
                    return self.fail(TWCC_PAPERJAM);
                }
                let x = data as *mut ImageMemXfer;
                let mem = unsafe { (*x).memory };
                if mem.flags != TWMF_APPOWNS | TWMF_POINTER || mem.the_mem.is_null() {
                    return self.fail(TWCC_BADVALUE);
                }
                let (w, h) = self.size();
                let row = self.row_bytes(self.sent, 0).len();
                let bpr = row.div_ceil(4) * 4; // padded rows, as drivers send them
                let rows = ((mem.length as usize / bpr) as u32)
                    .clamp(1, 37)
                    .min(h - self.row);
                let out = mem.the_mem as *mut u8;
                for r in 0..rows {
                    let bytes = self.row_bytes(self.sent, self.row + r);
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            bytes.as_ptr(),
                            out.add(r as usize * bpr),
                            bytes.len(),
                        );
                    }
                }
                let strip = ImageMemXfer {
                    compression: TWCP_NONE,
                    bytes_per_row: bpr as u32,
                    columns: w,
                    rows,
                    x_offset: 0,
                    y_offset: self.row,
                    bytes_written: rows * bpr as u32,
                    memory: mem,
                };
                unsafe { x.write_unaligned(strip) };
                self.row += rows;
                self.state = 7;
                if self.row >= h {
                    TWRC_XFERDONE
                } else {
                    TWRC_SUCCESS
                }
            }
            (DAT_PENDINGXFERS, MSG_ENDXFER) => {
                if self.state != 6 && self.state != 7 {
                    return self.seq();
                }
                self.sent += 1;
                self.row = 0;
                let s = self.scanner();
                let left = s.pages.saturating_sub(self.sent);
                let count = if left == 0 {
                    0
                } else if s.twain2 {
                    left as u16
                } else {
                    0xffff
                };
                unsafe {
                    (data as *mut PendingXfers).write_unaligned(PendingXfers { count, eoj: 0 })
                };
                self.state = if count == 0 { 5 } else { 6 };
                TWRC_SUCCESS
            }
            (DAT_PENDINGXFERS, MSG_RESET) => {
                if self.state != 6 {
                    return self.seq();
                }
                unsafe { (data as *mut PendingXfers).write_unaligned(PendingXfers::default()) };
                self.state = 5;
                TWRC_SUCCESS
            }
            _ => self.fail(TWCC_BADPROTOCOL),
        }
    }
}

impl Entry for Fake {
    unsafe fn call(
        &mut self,
        origin: *mut Identity,
        dest: *mut Identity,
        _dg: u32,
        dat: u16,
        msg: u16,
        data: *mut c_void,
    ) -> u16 {
        if origin.is_null() {
            return self.fail(TWCC_BADPROTOCOL);
        }
        if dest.is_null() {
            unsafe { self.dsm(origin, dat, msg, data) }
        } else if self.open.map(|o| o.1) == Some(unsafe { (*dest).id }) {
            unsafe { self.ds(dat, msg, data) }
        } else {
            self.fail(TWCC_BADDEST)
        }
    }
}

// The DSM's memory functions: a block with its length in front.
const HEAD: usize = 16;

fn layout(size: usize) -> std::alloc::Layout {
    std::alloc::Layout::from_size_align(size + HEAD, 16).expect("layout")
}

unsafe extern "system" fn mem_allocate(size: u32) -> Handle {
    unsafe {
        let p = std::alloc::alloc_zeroed(layout(size as usize));
        if p.is_null() {
            return std::ptr::null_mut();
        }
        *(p as *mut usize) = size as usize;
        p.add(HEAD) as Handle
    }
}

unsafe extern "system" fn mem_free(h: Handle) {
    if h.is_null() {
        return;
    }
    unsafe {
        let p = (h as *mut u8).sub(HEAD);
        let size = *(p as *mut usize);
        std::alloc::dealloc(p, layout(size));
    }
}

unsafe extern "system" fn mem_lock(h: Handle) -> *mut c_void {
    h
}

unsafe extern "system" fn mem_unlock(_h: Handle) {}
