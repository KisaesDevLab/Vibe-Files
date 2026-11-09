//! One scan through a TWAIN source (PA-19 step 2): open it, set the capabilities from the
//! scan profile (or leave them to the driver's own window), enable it, wait for its
//! "transfer ready", take every page by memory transfer, and close it again on every path.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU16, Ordering};

use super::ffi::*;
use super::{Manager, condition};

/// How the pixels come.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Pixel {
    /// 1 bit per pixel, 0 = black.
    Bw,
    Gray,
    Rgb,
}

impl Pixel {
    fn twain(self) -> u16 {
        match self {
            Pixel::Bw => TWPT_BW,
            Pixel::Gray => TWPT_GRAY,
            Pixel::Rgb => TWPT_RGB,
        }
    }
    fn from_twain(t: i16) -> Option<Self> {
        match t as u16 {
            TWPT_BW => Some(Pixel::Bw),
            TWPT_GRAY => Some(Pixel::Gray),
            TWPT_RGB => Some(Pixel::Rgb),
            _ => None,
        }
    }
    /// Bytes for one row of `width` pixels, unpadded.
    pub fn row_bytes(self, width: u32) -> usize {
        match self {
            Pixel::Bw => width.div_ceil(8) as usize,
            Pixel::Gray => width as usize,
            Pixel::Rgb => width as usize * 3,
        }
    }
}

/// What to ask the source for.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Settings {
    /// `Some(true)` feeder, `Some(false)` flatbed, `None` the driver's choice.
    pub feeder: Option<bool>,
    pub duplex: bool,
    pub dpi: u32,
    pub pixel: Pixel,
    /// Ask the driver to drop blank pages itself (Fujitsu offers it); ours runs anyway.
    pub discard_blank: bool,
    /// Show the driver's own window: its settings and profiles drive the scan, and only the
    /// transfer mechanism is set by this app.
    pub show_ui: bool,
}

/// One scanned page as the source sent it (rows packed, no padding).
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub width: u32,
    pub height: u32,
    pub dpi: f32,
    pub pixel: Pixel,
    pub data: Vec<u8>,
}

impl Page {
    /// As a frame for the image → PDF pipeline.
    pub fn to_frame(&self) -> crate::images::Frame {
        let (w, h) = (self.width, self.height);
        let row = self.pixel.row_bytes(w);
        let rgb = image::RgbImage::from_fn(w, h, |x, y| {
            let r = &self.data[y as usize * row..];
            match self.pixel {
                Pixel::Bw => {
                    let bit = r[x as usize / 8] >> (7 - x % 8) & 1;
                    let v = if bit == 1 { 255 } else { 0 };
                    image::Rgb([v, v, v])
                }
                Pixel::Gray => {
                    let v = r[x as usize];
                    image::Rgb([v, v, v])
                }
                Pixel::Rgb => {
                    let i = x as usize * 3;
                    image::Rgb([r[i], r[i + 1], r[i + 2]])
                }
            }
        });
        crate::images::Frame {
            rgb,
            dpi: (self.dpi > 0.0).then_some(self.dpi),
            orientation: 1,
        }
    }
}

/// What a scan produced.
#[derive(Debug, Default)]
pub struct Scanned {
    pub pages: Vec<Page>,
    /// Why the batch stopped early (paper jam, double feed, cover open …); the pages before
    /// it are kept.
    pub stopped: Option<String>,
    /// The scan was cancelled in the driver's window before any page.
    pub cancelled: bool,
}

/// Largest page taken, bytes (a 600 dpi colour page of 17 × 44 inches is about 800 MB).
#[cfg(target_pointer_width = "64")]
const MAX_PAGE_BYTES: usize = 900 * 1024 * 1024;
/// In the 32-bit scanning helper: what a 32-bit program can reliably get in one piece.
#[cfg(target_pointer_width = "32")]
const MAX_PAGE_BYTES: usize = 400 * 1024 * 1024;

/// One scan at a time per process (the callback below is process-wide).
static SCAN: Mutex<()> = Mutex::new(());
/// The last message a TWAIN 2 source sent through the callback.
static PENDING: AtomicU16 = AtomicU16::new(MSG_NULL);

unsafe extern "system" fn callback(
    _origin: *mut Identity,
    _dest: *mut Identity,
    _dg: u32,
    _dat: u16,
    msg: u16,
    _data: *mut c_void,
) -> u16 {
    PENDING.store(msg, Ordering::SeqCst);
    TWRC_SUCCESS
}

unsafe extern "system" fn global_alloc(size: u32) -> Handle {
    use windows::Win32::System::Memory::{GHND, GlobalAlloc};
    // SAFETY: plain Win32 allocation.
    unsafe { GlobalAlloc(GHND, size as usize) }.map_or(std::ptr::null_mut(), |h| h.0)
}

unsafe extern "system" fn global_free(h: Handle) {
    use windows::Win32::Foundation::GlobalFree;
    use windows::Win32::Foundation::HGLOBAL;
    // SAFETY: `h` came from `global_alloc`.
    let _ = unsafe { GlobalFree(Some(HGLOBAL(h))) };
}

unsafe extern "system" fn global_lock(h: Handle) -> *mut c_void {
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::Memory::GlobalLock;
    // SAFETY: `h` came from `global_alloc`.
    unsafe { GlobalLock(HGLOBAL(h)) }
}

unsafe extern "system" fn global_unlock(h: Handle) {
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::Memory::GlobalUnlock;
    // SAFETY: `h` came from `global_alloc` and was locked once.
    let _ = unsafe { GlobalUnlock(HGLOBAL(h)) };
}

/// TWAIN state of the open source, so every exit closes what was opened.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum State {
    Open = 4,
    Enabled = 5,
    Ready = 6,
}

struct Ds<'a> {
    m: &'a mut Manager,
    id: Identity,
    state: State,
}

impl Ds<'_> {
    fn call(&mut self, dg: u32, dat: u16, msg: u16, data: *mut c_void) -> u16 {
        unsafe {
            self.m
                .entry
                .call(&mut self.m.app, &mut self.id, dg, dat, msg, data)
        }
    }

    fn status(&mut self) -> u16 {
        let mut id = self.id;
        self.m.status(Some(&mut id))
    }

    /// Sets a one-value capability. `Err` carries the condition code.
    fn set(&mut self, cap: u16, item_type: u16, item: u32) -> Result<(), u16> {
        let (alloc, lock, unlock, free) = match self.m.mem {
            Some(mem) => (
                mem.mem_allocate.ok_or(TWCC_LOWMEMORY)?,
                mem.mem_lock.ok_or(TWCC_LOWMEMORY)?,
                mem.mem_unlock.ok_or(TWCC_LOWMEMORY)?,
                mem.mem_free.ok_or(TWCC_LOWMEMORY)?,
            ),
            // A TWAIN 1.x manager (Windows' own twain_32.dll, which remote-scanning tools hook)
            // has no memory functions: containers are Windows global memory, as TWAIN 1.x says.
            None => (
                global_alloc as MemAllocate,
                global_lock as MemLock,
                global_unlock as MemUnlock,
                global_free as MemFree,
            ),
        };
        let h = unsafe { alloc(size_of::<OneValue>() as u32) };
        if h.is_null() {
            return Err(TWCC_LOWMEMORY);
        }
        unsafe {
            let p = lock(h) as *mut OneValue;
            if p.is_null() {
                free(h);
                return Err(TWCC_LOWMEMORY);
            }
            p.write_unaligned(OneValue { item_type, item });
            unlock(h);
        }
        let mut c = Capability {
            cap,
            con_type: TWON_ONEVALUE,
            container: h,
        };
        let rc = self.call(
            DG_CONTROL,
            DAT_CAPABILITY,
            MSG_SET,
            &mut c as *mut _ as *mut c_void,
        );
        unsafe { free(h) };
        match rc {
            TWRC_SUCCESS | TWRC_CHECKSTATUS => Ok(()),
            _ => Err(self.status()),
        }
    }

    fn set_bool(&mut self, cap: u16, v: bool) -> Result<(), u16> {
        self.set(cap, TWTY_BOOL, v as u32)
    }

    fn set_u16(&mut self, cap: u16, v: u16) -> Result<(), u16> {
        self.set(cap, TWTY_UINT16, v as u32)
    }

    fn configure(&mut self, s: &Settings) -> Result<(), String> {
        self.set_u16(ICAP_XFERMECH, TWSX_MEMORY).map_err(|cc| {
            format!(
                "the scanner cannot hand pages over in memory ({})",
                condition(cc)
            )
        })?;
        let _ = self.set(CAP_XFERCOUNT, TWTY_INT16, 0xffff); // -1: all pages
        let _ = self.set_u16(ICAP_COMPRESSION, TWCP_NONE);
        if s.show_ui {
            return Ok(()); // the driver's window and its profile decide the rest
        }
        let _ = self.set_bool(CAP_INDICATORS, false); // no driver progress window
        if let Some(feeder) = s.feeder {
            match self.set_bool(CAP_FEEDERENABLED, feeder) {
                Ok(()) => {}
                Err(_) if feeder => return Err("this scanner has no document feeder".into()),
                Err(_) => {} // no feeder to turn off
            }
            if feeder {
                let _ = self.set_bool(CAP_AUTOFEED, true);
            }
        }
        if s.duplex {
            self.set_bool(CAP_DUPLEXENABLED, true)
                .map_err(|_| "this scanner cannot scan both sides".to_string())?;
        } else {
            let _ = self.set_bool(CAP_DUPLEXENABLED, false);
        }
        self.set_u16(ICAP_PIXELTYPE, s.pixel.twain())
            .map_err(|cc| format!("color mode refused ({})", condition(cc)))?;
        if s.pixel == Pixel::Bw {
            let _ = self.set_u16(ICAP_PIXELFLAVOR, TWPF_CHOCOLATE);
        }
        let depth = match s.pixel {
            Pixel::Bw => 1,
            Pixel::Gray => 8,
            Pixel::Rgb => 24,
        };
        let _ = self.set_u16(ICAP_BITDEPTH, depth);
        let _ = self.set_u16(ICAP_UNITS, TWUN_INCHES);
        let res = Fix32::from_f32(s.dpi as f32).to_item();
        self.set(ICAP_XRESOLUTION, TWTY_FIX32, res)
            .map_err(|cc| format!("{} dpi refused ({})", s.dpi, condition(cc)))?;
        let _ = self.set(ICAP_YRESOLUTION, TWTY_FIX32, res);
        let _ = self.set(
            ICAP_AUTODISCARDBLANKPAGES,
            TWTY_INT32,
            if s.discard_blank {
                TWBP_AUTO
            } else {
                TWBP_DISABLE
            } as u32,
        );
        Ok(())
    }

    fn enable(&mut self, show_ui: bool) -> Result<(), String> {
        PENDING.store(MSG_NULL, Ordering::SeqCst);
        let mut ui = UserInterface {
            show_ui: show_ui as u16,
            modal_ui: 0,
            parent: self.m.parent,
        };
        match self.call(
            DG_CONTROL,
            DAT_USERINTERFACE,
            MSG_ENABLEDS,
            &mut ui as *mut _ as *mut c_void,
        ) {
            TWRC_SUCCESS | TWRC_CHECKSTATUS => {
                self.state = State::Enabled;
                Ok(())
            }
            TWRC_CANCEL => Err("cancelled".into()),
            _ => {
                let cc = self.status();
                Err(format!("the scanner did not start ({})", condition(cc)))
            }
        }
    }

    /// Pumps this thread's messages until the source says transfer ready or close.
    fn wait(&mut self) -> u16 {
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
        };
        loop {
            let p = PENDING.swap(MSG_NULL, Ordering::SeqCst);
            if p == MSG_XFERREADY || p == MSG_CLOSEDSREQ || p == MSG_CLOSEDSOK {
                return p;
            }
            let mut msg = MSG::default();
            let mut any = false;
            while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
                any = true;
                let mut ev = Event {
                    event: &mut msg as *mut MSG as *mut c_void,
                    message: MSG_NULL,
                };
                let rc = self.call(
                    DG_CONTROL,
                    DAT_EVENT,
                    MSG_PROCESSEVENT,
                    &mut ev as *mut _ as *mut c_void,
                );
                if rc == TWRC_DSEVENT {
                    let m = ev.message;
                    if m == MSG_XFERREADY || m == MSG_CLOSEDSREQ || m == MSG_CLOSEDSOK {
                        return m;
                    }
                } else {
                    unsafe {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
                let p = PENDING.swap(MSG_NULL, Ordering::SeqCst);
                if p == MSG_XFERREADY || p == MSG_CLOSEDSREQ || p == MSG_CLOSEDSOK {
                    return p;
                }
            }
            if !any {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }

    /// Takes one page by memory transfer. `Ok(None)`: the source cancelled this page.
    fn transfer_page(&mut self) -> Result<Option<Page>, u16> {
        let mut info = ImageInfo::default();
        if self.call(
            DG_IMAGE,
            DAT_IMAGEINFO,
            MSG_GET,
            &mut info as *mut _ as *mut c_void,
        ) != TWRC_SUCCESS
        {
            return Err(self.status());
        }
        let pixel = Pixel::from_twain(info.pixel_type).ok_or(TWCC_BADVALUE)?;
        // Only uncompressed, chunky pixels are read: compressed or planar data read as plain
        // pixels would be a garbled page (QA I12). The driver was asked for TWCP_NONE.
        let (compression, planar) = (info.compression, info.planar);
        if compression != TWCP_NONE || planar != 0 {
            return Err(TWCC_BADVALUE);
        }
        let width = info.image_width.max(0) as u32;
        let known_height = (info.image_length > 0).then_some(info.image_length as u32);
        // Sizes from the driver are checked, not trusted (QA: on 32-bit they could wrap).
        if width > 100_000 || known_height.is_some_and(|h| h > 100_000) {
            return Err(TWCC_BADVALUE);
        }
        let row = pixel.row_bytes(width);
        let page_bytes = row
            .checked_mul(known_height.unwrap_or(0) as usize)
            .filter(|&n| n <= MAX_PAGE_BYTES)
            .ok_or(TWCC_LOWMEMORY)?;
        let mut setup = SetupMemXfer::default();
        if self.call(
            DG_CONTROL,
            DAT_SETUPMEMXFER,
            MSG_GET,
            &mut setup as *mut _ as *mut c_void,
        ) != TWRC_SUCCESS
        {
            return Err(self.status());
        }
        let size = match setup.preferred {
            0 => setup.min_buf_size.max(64 * 1024),
            p => p.max(setup.min_buf_size),
        }
        .min(setup.max_buf_size.max(setup.min_buf_size).max(64 * 1024));
        let mut buf = vec![0u8; size as usize];
        // Memory that cannot be had is an error, not an abort (QA).
        let mut data = Vec::new();
        data.try_reserve_exact(page_bytes)
            .map_err(|_| TWCC_LOWMEMORY)?;
        data.resize(page_bytes, 0);
        let mut rows_seen = 0u32;
        loop {
            let mut x = ImageMemXfer {
                compression: TWON_DONTCARE16,
                bytes_per_row: 0,
                columns: 0,
                rows: 0,
                x_offset: 0,
                y_offset: 0,
                bytes_written: 0,
                memory: Memory {
                    flags: TWMF_APPOWNS | TWMF_POINTER,
                    length: size,
                    the_mem: buf.as_mut_ptr() as *mut c_void,
                },
            };
            let rc = self.call(
                DG_IMAGE,
                DAT_IMAGEMEMXFER,
                MSG_GET,
                &mut x as *mut _ as *mut c_void,
            );
            match rc {
                TWRC_SUCCESS | TWRC_XFERDONE => {
                    self.state = State::Ready; // 7 while strips flow; ENDXFER brings it back
                    let bpr = x.bytes_per_row as usize;
                    let x0 = match pixel {
                        Pixel::Bw => x.x_offset as usize / 8,
                        Pixel::Gray => x.x_offset as usize,
                        Pixel::Rgb => x.x_offset as usize * 3,
                    };
                    let take = bpr.min(row.saturating_sub(x0));
                    // The driver's sizes are checked, not trusted: a strip must fit the buffer
                    // it was written to, and the page may not grow past 100,000 rows.
                    let (y0, rows) = (x.y_offset as usize, x.rows as usize);
                    let fits = rows.checked_mul(bpr).is_some_and(|n| n <= buf.len())
                        && y0.checked_add(rows).is_some_and(|e| e <= 100_000);
                    if !fits {
                        return Err(TWCC_BADVALUE);
                    }
                    let need = (y0 + rows)
                        .checked_mul(row)
                        .filter(|&n| n <= MAX_PAGE_BYTES)
                        .ok_or(TWCC_LOWMEMORY)?;
                    if data.len() < need {
                        data.try_reserve(need - data.len())
                            .map_err(|_| TWCC_LOWMEMORY)?;
                        data.resize(need, 0);
                    }
                    for r in 0..rows {
                        let src = &buf[r * bpr..r * bpr + take];
                        let at = (y0 + r) * row + x0;
                        data[at..at + take].copy_from_slice(src);
                    }
                    rows_seen = rows_seen.max((y0 + rows) as u32);
                    if rc == TWRC_XFERDONE {
                        break;
                    }
                }
                TWRC_CANCEL => return Ok(None),
                _ => return Err(self.status()),
            }
        }
        let height = known_height.unwrap_or(rows_seen);
        data.truncate(row * height as usize);
        Ok(Some(Page {
            width,
            height,
            dpi: info.x_resolution.to_f32(),
            pixel,
            data,
        }))
    }

    /// Ends the current transfer; returns how many are still pending (-1 = unknown).
    fn end_transfer(&mut self) -> i16 {
        let mut p = PendingXfers::default();
        if self.call(
            DG_CONTROL,
            DAT_PENDINGXFERS,
            MSG_ENDXFER,
            &mut p as *mut _ as *mut c_void,
        ) != TWRC_SUCCESS
        {
            return self.reset();
        }
        let n = p.count as i16;
        self.state = if n == 0 { State::Enabled } else { State::Ready };
        n
    }

    /// Drops every pending transfer (back to state 5).
    fn reset(&mut self) -> i16 {
        let mut p = PendingXfers::default();
        self.call(
            DG_CONTROL,
            DAT_PENDINGXFERS,
            MSG_RESET,
            &mut p as *mut _ as *mut c_void,
        );
        self.state = State::Enabled;
        0
    }

    /// Takes every page, handing each to `sink` as it ends (the 32-bit helper writes it to
    /// disk at once, so a long batch never sits in memory whole).
    fn transfer_all(
        &mut self,
        out: &mut Scanned,
        sink: &mut dyn FnMut(Page) -> Result<(), String>,
    ) {
        loop {
            let failed = match self.transfer_page() {
                Ok(Some(p)) => sink(p).err(),
                Ok(None) => None,
                Err(cc) => Some(condition(cc)),
            };
            match failed {
                None => {}
                Some(why) => {
                    out.stopped = Some(why);
                    // Finish the page's transfer, then drop the rest.
                    let mut p = PendingXfers::default();
                    self.call(
                        DG_CONTROL,
                        DAT_PENDINGXFERS,
                        MSG_ENDXFER,
                        &mut p as *mut _ as *mut c_void,
                    );
                    if p.count != 0 {
                        self.reset();
                    }
                    self.state = State::Enabled;
                    return;
                }
            }
            if self.end_transfer() == 0 {
                return;
            }
        }
    }

    /// Back down to state 3 from wherever the source is.
    fn close(&mut self) {
        if self.state >= State::Ready {
            self.reset();
        }
        if self.state >= State::Enabled {
            let mut ui = UserInterface {
                show_ui: 0,
                modal_ui: 0,
                parent: self.m.parent,
            };
            self.call(
                DG_CONTROL,
                DAT_USERINTERFACE,
                MSG_DISABLEDS,
                &mut ui as *mut _ as *mut c_void,
            );
        }
        let mut id = self.id;
        self.m.dsm(
            DG_CONTROL,
            DAT_IDENTITY,
            MSG_CLOSEDS,
            &mut id as *mut _ as *mut c_void,
        );
    }
}

impl Manager {
    /// Scans one batch from the source named `source` (state 3 → 4 … → 3).
    pub fn scan(&mut self, source: &str, s: &Settings) -> Result<Scanned, String> {
        let mut pages = Vec::new();
        let mut out = self.scan_each(source, s, &mut |p| {
            pages.push(p);
            Ok(())
        })?;
        out.pages = pages;
        Ok(out)
    }

    /// [`Self::scan`], each page handed to `sink` as it ends instead of kept in the result.
    pub fn scan_each(
        &mut self,
        source: &str,
        s: &Settings,
        sink: &mut dyn FnMut(Page) -> Result<(), String>,
    ) -> Result<Scanned, String> {
        let _one = SCAN.lock().unwrap_or_else(|e| e.into_inner());
        let found = self
            .sources()?
            .into_iter()
            .find(|x| x.name == source)
            .ok_or_else(|| format!("the TWAIN source \"{source}\" is not on this workstation"))?;
        let mut id = found.identity;
        if self.dsm(
            DG_CONTROL,
            DAT_IDENTITY,
            MSG_OPENDS,
            &mut id as *mut _ as *mut c_void,
        ) != TWRC_SUCCESS
        {
            let cc = self.status(None);
            return Err(format!("cannot open \"{source}\" ({})", condition(cc)));
        }
        let mut ds = Ds {
            m: self,
            id,
            state: State::Open,
        };
        // Callbacks go through a TWAIN 2 manager only; under a 1.x manager a TWAIN 2 source
        // sends its messages through the window, as a 1.x source does.
        if found.twain2 && ds.m.mem.is_some() {
            let mut cb = Callback2 {
                proc_: callback as *mut c_void,
                ref_con: 0,
                message: 0,
            };
            // A source that refuses callbacks sends its messages through the window.
            ds.call(
                DG_CONTROL,
                DAT_CALLBACK2,
                MSG_REGISTER_CALLBACK,
                &mut cb as *mut _ as *mut c_void,
            );
        }
        let r = ds.configure(s).and_then(|()| {
            let mut out = Scanned::default();
            match ds.enable(s.show_ui) {
                Ok(()) => {}
                Err(e) if e == "cancelled" => {
                    out.cancelled = true;
                    return Ok(out);
                }
                Err(e) => return Err(e),
            }
            match ds.wait() {
                MSG_XFERREADY => {
                    ds.state = State::Ready;
                    ds.transfer_all(&mut out, sink);
                }
                _ => out.cancelled = true,
            }
            Ok(out)
        });
        ds.close();
        r
    }
}

#[cfg(test)]
mod tests {
    use super::super::fake::{Fake, Log, Scanner};
    use super::super::{Entry, on_twain_thread};
    use super::*;
    use std::collections::HashMap;

    fn settings() -> Settings {
        Settings {
            feeder: Some(true),
            duplex: true,
            dpi: 100,
            pixel: Pixel::Gray,
            discard_blank: true,
            show_ui: false,
        }
    }

    fn run(scanner: Scanner, s: Settings) -> (Result<Scanned, String>, Log) {
        let name = scanner.name.clone();
        let fake = Fake::new(vec![scanner]);
        let log = fake.log();
        let r = on_twain_thread(
            move || Ok(Box::new(fake) as Box<dyn Entry>),
            move |m| m.scan(&name, &s),
        );
        let log = log.lock().unwrap().clone();
        (r, log)
    }

    fn assert_closed_in_order(log: &Log) {
        assert_eq!(log.closed_from, Some(3), "{log:?}");
        assert_eq!(log.seq_errors, 0, "{log:?}");
    }

    #[test]
    fn duplex_batch_from_a_twain2_source() {
        let (r, log) = run(Scanner::fi7160(), settings());
        let r = r.unwrap();
        assert!(r.stopped.is_none() && !r.cancelled);
        assert_eq!(r.pages.len(), 4);
        for p in &r.pages {
            assert_eq!((p.width, p.height, p.pixel), (200, 250, Pixel::Gray));
            assert!((p.dpi - 100.0).abs() < 0.01);
            assert_eq!(p.data.len(), 200 * 250);
        }
        // Page 0: rows 0-5 banded, 6-11 light (spacing 2); page 1: spacing 3.
        let px = |p: &Page, x: usize, y: usize| p.data[y * 200 + x];
        assert_eq!(px(&r.pages[0], 100, 0), 15);
        assert_eq!(px(&r.pages[0], 100, 6), 245);
        assert_eq!(px(&r.pages[0], 100, 12), 15);
        assert_eq!(px(&r.pages[1], 100, 12), 245);
        assert_eq!(px(&r.pages[1], 100, 18), 15);
        // Last rows arrive (strips of 37 rows with padded rows).
        assert_eq!(px(&r.pages[0], 100, 240), 15);
        assert_eq!(px(&r.pages[0], 100, 249), 245);
        assert!(log.callback_registered);
        let set: HashMap<u16, u32> = log.caps.iter().copied().collect();
        assert_eq!(set[&ICAP_XFERMECH], TWSX_MEMORY as u32);
        assert_eq!(set[&CAP_FEEDERENABLED], 1);
        assert_eq!(set[&CAP_DUPLEXENABLED], 1);
        assert_eq!(set[&ICAP_PIXELTYPE], TWPT_GRAY as u32);
        assert_eq!(set[&ICAP_XRESOLUTION], 100);
        assert_eq!(set[&ICAP_AUTODISCARDBLANKPAGES], TWBP_AUTO as u32);
        assert_eq!(set[&CAP_INDICATORS], 0, "no driver progress window");
        assert_eq!(set[&ICAP_BITDEPTH], 8);
        assert_closed_in_order(&log);
    }

    /// A TWAIN 1.x virtual source (as a remote-scanning utility shows): no callback, events
    /// through the window, unknown page length and pending count.
    #[test]
    fn virtual_source_through_window_events_in_black_and_white() {
        let s = Settings {
            pixel: Pixel::Bw,
            ..settings()
        };
        let (r, log) = run(Scanner::virtual_source(), s);
        let r = r.unwrap();
        assert_eq!(r.pages.len(), 4);
        let p = &r.pages[0];
        assert_eq!((p.width, p.height, p.pixel), (200, 250, Pixel::Bw));
        assert_eq!(p.data.len(), 25 * 250);
        let f = p.to_frame();
        assert_eq!(f.rgb.get_pixel(100, 0).0, [0, 0, 0]);
        assert_eq!(f.rgb.get_pixel(100, 6).0, [255, 255, 255]);
        assert_eq!(f.rgb.get_pixel(5, 0).0, [255, 255, 255], "margin");
        assert_eq!(f.dpi, Some(100.0));
        assert!(!log.callback_registered);
        assert_closed_in_order(&log);
    }

    /// The same source through a TWAIN 1.x manager (Windows' twain_32.dll, as in the 32-bit
    /// scanning helper): no memory functions from the manager, so the settings go in Windows
    /// global memory; no callback.
    #[test]
    fn a_twain1_manager_gets_settings_in_global_memory() {
        let name = Scanner::fi7160().name;
        let fake = Fake::legacy(vec![Scanner::fi7160()]);
        let log = fake.log();
        let r = on_twain_thread(
            move || Ok(Box::new(fake) as Box<dyn Entry>),
            move |m| m.scan(&name, &settings()),
        );
        let log = log.lock().unwrap().clone();
        assert_eq!(r.unwrap().pages.len(), 4);
        let set: HashMap<u16, u32> = log.caps.iter().copied().collect();
        assert_eq!(set[&ICAP_XFERMECH], TWSX_MEMORY as u32);
        assert_eq!(set[&ICAP_XRESOLUTION], 100);
        assert!(!log.callback_registered);
        assert_closed_in_order(&log);
    }

    #[test]
    fn color_pages() {
        let s = Settings {
            pixel: Pixel::Rgb,
            dpi: 80,
            ..settings()
        };
        let (r, _) = run(Scanner::fi7160(), s);
        let p = &r.unwrap().pages[0];
        assert_eq!((p.width, p.height, p.pixel), (160, 200, Pixel::Rgb));
        assert_eq!(p.to_frame().rgb.get_pixel(80, 0).0, [20, 30, 140]);
    }

    #[test]
    fn paper_jam_keeps_the_pages_before_it() {
        let (r, log) = run(
            Scanner {
                jam_after: Some(2),
                ..Scanner::fi7160()
            },
            settings(),
        );
        let r = r.unwrap();
        assert_eq!(r.pages.len(), 2);
        assert_eq!(r.stopped.as_deref(), Some("paper jam"));
        assert_closed_in_order(&log);
    }

    #[test]
    fn duplex_on_a_one_sided_scanner_is_refused_and_closed() {
        let (r, log) = run(
            Scanner {
                duplex: false,
                ..Scanner::fi7160()
            },
            settings(),
        );
        assert_eq!(r.unwrap_err(), "this scanner cannot scan both sides");
        assert_closed_in_order(&log);
        let (r, _) = run(
            Scanner {
                feeder: false,
                ..Scanner::fi7160()
            },
            settings(),
        );
        assert_eq!(r.unwrap_err(), "this scanner has no document feeder");
    }

    /// "Use the driver's own window": only the transfer is set; the driver's profile decides
    /// sides, color and resolution.
    #[test]
    fn drivers_own_window_sets_only_the_transfer() {
        let s = Settings {
            show_ui: true,
            dpi: 600,
            pixel: Pixel::Rgb,
            ..settings()
        };
        let (r, log) = run(Scanner::fi7160(), s);
        assert_eq!(r.unwrap().pages.len(), 4);
        assert_eq!(log.show_ui, Some(true));
        let caps: Vec<u16> = log.caps.iter().map(|c| c.0).collect();
        assert_eq!(caps, [ICAP_XFERMECH, CAP_XFERCOUNT, ICAP_COMPRESSION]);
        assert_closed_in_order(&log);

        let (r, log) = run(
            Scanner {
                cancel_in_ui: true,
                ..Scanner::fi7160()
            },
            Settings {
                show_ui: true,
                ..settings()
            },
        );
        let r = r.unwrap();
        assert!(r.cancelled && r.pages.is_empty());
        assert_closed_in_order(&log);
    }

    #[test]
    fn a_source_that_is_gone_is_named() {
        let fake = Fake::new(vec![Scanner::fi7160()]);
        let r = on_twain_thread(
            move || Ok(Box::new(fake) as Box<dyn Entry>),
            |m| m.scan("fi-8170", &settings()),
        );
        assert_eq!(
            r.unwrap_err(),
            "the TWAIN source \"fi-8170\" is not on this workstation"
        );
    }
}
