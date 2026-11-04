//! Print (PHASES PA-13): the Windows print dialog (`PrintDlgEx`), then each page rendered with
//! pdfium at the printer's resolution (at most 300 dpi) and drawn onto the printer's device
//! context, one spooled job per document. No dependence on the PDF app associated with `.pdf`.
//! Annotations and stamps print as shown.

use std::path::Path;

use pdfium_render::prelude::{PdfPageRenderRotation, PdfRenderConfig};
use windows::Win32::Foundation::{GlobalFree, HGLOBAL, HWND};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteDC, GetDeviceCaps, HDC, HORZRES,
    LOGPIXELSX, SRCCOPY, StretchDIBits, VERTRES,
};
use windows::Win32::Storage::Xps::{AbortDoc, DOCINFOW, EndDoc, EndPage, StartDocW, StartPage};
use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
use windows::Win32::UI::Controls::Dialogs::{
    DEVNAMES, PD_CURRENTPAGE, PD_NOCURRENTPAGE, PD_NOPAGENUMS, PD_NOSELECTION, PD_PAGENUMS,
    PD_RESULT_PRINT, PD_RETURNDC, PRINTDLGEX_FLAGS, PRINTDLGEXW, PRINTPAGERANGE, PrintDlgExW,
};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
use windows::core::PCWSTR;

/// Highest resolution pages are rendered at (higher prints no sharper on office printers and
/// costs memory).
const MAX_DPI: i32 = 300;

/// What the dialog chose. The device context is the printer's, ready for a job.
#[derive(Debug)]
pub struct Choice {
    /// `HDC` as a number, so the job can run on its own thread.
    hdc: isize,
    /// 1-based inclusive page ranges; empty = every page.
    pub ranges: Vec<(u32, u32)>,
    pub copies: u32,
    pub printer: String,
}

// The device context is only used by one thread at a time (the dialog's, then the job's).
unsafe impl Send for Choice {}

impl Drop for Choice {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteDC(HDC(self.hdc as *mut core::ffi::c_void));
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The pages of a document of `count` pages the choice covers (0-based).
pub fn pages_of(ranges: &[(u32, u32)], count: usize) -> Vec<usize> {
    if ranges.is_empty() {
        return (0..count).collect();
    }
    let mut v: Vec<usize> = ranges
        .iter()
        .flat_map(|&(a, b)| (a.max(1)..=b.max(a)).map(|p| p as usize - 1))
        .filter(|&p| p < count)
        .collect();
    // `1-3,2` prints page 2 once; the order chosen is kept.
    let mut seen = std::collections::HashSet::new();
    v.retain(|p| seen.insert(*p));
    v
}

/// The Windows print dialog. `pages`: the document's page count (a range can be chosen);
/// `current`: the page shown (1-based, "Current page"). `None` = cancelled.
pub fn dialog(pages: Option<u32>, current: Option<u32>) -> Result<Option<Choice>, String> {
    let mut ranges = [PRINTPAGERANGE::default(); 10];
    let mut flags = PD_RETURNDC.0 | PD_NOSELECTION.0;
    if pages.is_none() {
        flags |= PD_NOPAGENUMS.0;
    }
    if current.is_none() {
        flags |= PD_NOCURRENTPAGE.0;
    }
    let mut pd = PRINTDLGEXW {
        lStructSize: std::mem::size_of::<PRINTDLGEXW>() as u32,
        hwndOwner: unsafe { GetForegroundWindow() },
        Flags: PRINTDLGEX_FLAGS(flags),
        nMaxPageRanges: ranges.len() as u32,
        lpPageRanges: ranges.as_mut_ptr(),
        nMinPage: 1,
        nMaxPage: pages.unwrap_or(1).max(1),
        nCopies: 1,
        nStartPage: u32::MAX, // START_PAGE_GENERAL
        ..Default::default()
    };
    if pd.hwndOwner == HWND::default() {
        return Err("no window to show the print dialog on".into());
    }
    unsafe { PrintDlgExW(&mut pd) }.map_err(|e| format!("the print dialog failed: {e}"))?;
    let printer = unsafe { device_name(pd.hDevNames) };
    unsafe {
        if !pd.hDevMode.is_invalid() {
            let _ = GlobalFree(Some(pd.hDevMode));
        }
        if !pd.hDevNames.is_invalid() {
            let _ = GlobalFree(Some(pd.hDevNames));
        }
    }
    if pd.dwResultAction != PD_RESULT_PRINT {
        if !pd.hDC.is_invalid() {
            unsafe {
                let _ = DeleteDC(pd.hDC);
            }
        }
        return Ok(None);
    }
    let chosen = PRINTDLGEX_FLAGS(pd.Flags.0);
    let ranges: Vec<(u32, u32)> = if chosen.0 & PD_CURRENTPAGE.0 != 0 {
        let c = current.unwrap_or(1);
        vec![(c, c)]
    } else if chosen.0 & PD_PAGENUMS.0 != 0 {
        ranges[..pd.nPageRanges as usize]
            .iter()
            .map(|r| (r.nFromPage, r.nToPage))
            .collect()
    } else {
        vec![]
    };
    Ok(Some(Choice {
        hdc: pd.hDC.0 as isize,
        ranges,
        copies: pd.nCopies.max(1),
        printer,
    }))
}

/// The printer name in a `DEVNAMES` block.
unsafe fn device_name(h: HGLOBAL) -> String {
    if h.is_invalid() {
        return String::new();
    }
    let p = unsafe { GlobalLock(h) } as *const DEVNAMES;
    if p.is_null() {
        return String::new();
    }
    let dn = unsafe { &*p };
    let base = p as *const u16;
    let name = unsafe { PCWSTR(base.add(dn.wDeviceOffset as usize)).to_string() };
    let _ = unsafe { GlobalUnlock(h) };
    name.unwrap_or_default()
}

/// A device context for `printer` by name, without the dialog ("Microsoft Print to PDF" in the
/// tests).
#[cfg(test)]
pub fn device(printer: &str) -> Result<Choice, String> {
    use windows::Win32::Graphics::Gdi::CreateDCW;
    let name = wide(printer);
    let hdc = unsafe { CreateDCW(PCWSTR::null(), PCWSTR(name.as_ptr()), PCWSTR::null(), None) };
    if hdc.is_invalid() {
        return Err(format!("the printer {printer} is not available"));
    }
    Ok(Choice {
        hdc: hdc.0 as isize,
        ranges: vec![],
        copies: 1,
        printer: printer.into(),
    })
}

/// Prints `pdf`'s chosen pages on the chosen printer as one job (`copies` times). The device
/// context stays open for the next document (closed when the choice is dropped). `output`: a
/// file for printers that write one (Print to PDF). Returns the pages printed (per copy).
pub fn print_pdf(
    choice: &Choice,
    pdf: &Path,
    title: &str,
    output: Option<&Path>,
) -> Result<usize, String> {
    let hdc = HDC(choice.hdc as *mut core::ffi::c_void);
    (|| {
        let dir = crate::ui::intake::pdfium_dir().ok_or("pdfium.dll not found")?;
        let pdfium = filer_pdf::pdf::pdfium(&dir)?;
        let doc = pdfium
            .load_pdf_from_file(pdf, None)
            .map_err(|e| format!("cannot open the PDF: {e}"))?;
        let count = doc.pages().len() as usize;
        let pages = pages_of(&choice.ranges, count);
        if pages.is_empty() {
            return Err("no pages in that range".to_string());
        }
        let (dpi, w, h) = unsafe {
            (
                GetDeviceCaps(Some(hdc), LOGPIXELSX),
                GetDeviceCaps(Some(hdc), HORZRES),
                GetDeviceCaps(Some(hdc), VERTRES),
            )
        };
        if w <= 0 || h <= 0 || dpi <= 0 {
            return Err("the printer gave no page size".into());
        }
        let title_w = wide(title);
        let out_w = output.map(|p| wide(&p.to_string_lossy()));
        let info = DOCINFOW {
            cbSize: std::mem::size_of::<DOCINFOW>() as i32,
            lpszDocName: PCWSTR(title_w.as_ptr()),
            lpszOutput: out_w
                .as_ref()
                .map_or(PCWSTR::null(), |o| PCWSTR(o.as_ptr())),
            ..Default::default()
        };
        // Render at most 300 dpi, then stretch to the printer's resolution.
        let scale = (MAX_DPI as f32 / dpi as f32).min(1.0);
        for _ in 0..choice.copies {
            if unsafe { StartDocW(hdc, &info) } <= 0 {
                return Err("the printer refused the job".into());
            }
            // A page that fails ends the job with AbortDoc: nothing half printed is left open
            // on the device context the next document uses.
            let job = || -> Result<(), String> {
                for &i in &pages {
                    let page = doc
                        .pages()
                        .get(i as i32)
                        .map_err(|e| format!("page {}: {e}", i + 1))?;
                    let (pw, ph) = (page.width().value, page.height().value);
                    // A landscape page on portrait paper (or the reverse) is turned to fit.
                    let turn = (pw > ph) != (w > h);
                    let (aw, ah) = if turn { (ph, pw) } else { (pw, ph) };
                    let fit = (w as f32 / aw).min(h as f32 / ah);
                    let (tw, th) = ((aw * fit) as i32, (ah * fit) as i32);
                    let mut cfg = PdfRenderConfig::new()
                        .set_target_width(((tw as f32) * scale).max(1.0) as i32)
                        .render_annotations(true)
                        .render_form_data(true);
                    if turn {
                        cfg = cfg.rotate(PdfPageRenderRotation::Degrees90, true);
                    }
                    let img = page
                        .render_with_config(&cfg)
                        .map_err(|e| format!("page {}: {e}", i + 1))?
                        .as_image()
                        .map_err(|e| format!("page {}: {e}", i + 1))?
                        .to_rgba8();
                    let (iw, ih) = (img.width() as i32, img.height() as i32);
                    // GDI wants BGRA, top-down with a negative height.
                    let mut bgra = img.into_raw();
                    for px in bgra.as_chunks_mut::<4>().0 {
                        px.swap(0, 2);
                    }
                    let bmi = BITMAPINFO {
                        bmiHeader: BITMAPINFOHEADER {
                            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                            biWidth: iw,
                            biHeight: -ih,
                            biPlanes: 1,
                            biBitCount: 32,
                            biCompression: BI_RGB.0,
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    unsafe {
                        if StartPage(hdc) <= 0 {
                            return Err("the printer refused a page".into());
                        }
                        StretchDIBits(
                            hdc,
                            (w - tw) / 2,
                            (h - th) / 2,
                            tw,
                            th,
                            0,
                            0,
                            iw,
                            ih,
                            Some(bgra.as_ptr().cast()),
                            &bmi,
                            DIB_RGB_COLORS,
                            SRCCOPY,
                        );
                        if EndPage(hdc) <= 0 {
                            return Err(format!("page {}: the printer refused it", i + 1));
                        }
                    }
                }
                Ok(())
            };
            if let Err(e) = job() {
                unsafe {
                    let _ = AbortDoc(hdc);
                }
                return Err(e);
            }
            if unsafe { EndDoc(hdc) } <= 0 {
                return Err("the printer did not finish the job".into());
            }
        }
        Ok(pages.len())
    })()
}

/// Hands a non-PDF file to its program's Print command (ShellExecute "print").
pub fn print_with_its_app(path: &Path) -> Result<(), String> {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
    let verb = wide("print");
    let file = wide(&path.to_string_lossy());
    let r = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_HIDE,
        )
    };
    if r.0 as isize > 32 {
        Ok(())
    } else {
        Err(format!(
            "{} has no program that prints it",
            path.file_name().unwrap_or_default().to_string_lossy()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_ranges() {
        assert_eq!(pages_of(&[], 3), [0, 1, 2]);
        assert_eq!(pages_of(&[(2, 3)], 3), [1, 2]);
        assert_eq!(pages_of(&[(1, 1), (3, 9)], 4), [0, 2, 3]);
        assert_eq!(
            pages_of(&[(1, 3), (2, 2)], 4),
            [0, 1, 2],
            "overlap printed once"
        );
    }

    /// PA-13 done-when 1: pages 2-3 of a 3-page PDF printed to "Microsoft Print to PDF" give a
    /// 2-page PDF (skipped where that printer is not installed).
    #[test]
    fn prints_a_page_range_to_microsoft_print_to_pdf() {
        crate::ui::test_util::pdfium_first();
        let Ok(mut c) = device("Microsoft Print to PDF") else {
            eprintln!("Microsoft Print to PDF is not installed: skipped");
            return;
        };
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("three.pdf");
        filer_pdf::pdf::write_test_pdf(&src, &["Invented one", "Invented two", "Invented three"]);
        let out = t.path().join("printed.pdf");
        c.ranges = vec![(2, 3)];
        assert_eq!(print_pdf(&c, &src, "three", Some(&out)).unwrap(), 2);
        // The spooler writes the file a moment later.
        let start = std::time::Instant::now();
        while !out.is_file() || std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0) == 0 {
            assert!(
                start.elapsed() < std::time::Duration::from_secs(60),
                "no output"
            );
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
        let doc = lopdf::Document::load(&out).unwrap();
        assert_eq!(doc.get_pages().len(), 2);
    }
}
