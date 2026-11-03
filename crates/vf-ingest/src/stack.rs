//! A scanned stack split at its barcode sheets (P02 step 3).
//!
//! Every page is rendered and searched for a sheet barcode (QR first, then Code 128). A sheet
//! page starts a new document and is dropped; so is a blank page right after it (the back of
//! the sheet in a duplex scan). Pages before the first sheet form a document of their own.

use std::ops::Range;
use std::path::Path;

use pdfium_render::prelude::{PdfRenderConfig, Pdfium};
use rxing::{BarcodeFormat, DecodeHints};

use crate::sheet::{Cover, Sheet};

/// Rendering width for barcode search: about 200 dpi on a Letter page.
const RENDER_WIDTH: i32 = 1700;
/// A page with less than this share of dark pixels is blank.
pub const BLANK_INK: f32 = 0.002;

/// What one page held.
#[derive(Debug, Clone, PartialEq)]
pub struct PageScan {
    pub sheet: Option<Sheet>,
    /// Share of dark pixels, 0–1.
    pub ink: f32,
}

/// One document of the stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// 0-based page range in the scanned file.
    pub pages: Range<usize>,
    /// Started by a sheet (a blank separator gives `Some(None)`).
    pub sheet: Option<Option<Cover>>,
}

/// Renders and searches every page of `path`.
pub fn scan_pages(pdfium: &Pdfium, path: &Path) -> Result<Vec<PageScan>, String> {
    let doc = pdfium
        .load_pdf_from_file(path, None)
        .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let config = PdfRenderConfig::new().set_target_width(RENDER_WIDTH);
    doc.pages()
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let img = p
                .render_with_config(&config)
                .and_then(|b| b.as_image())
                .map_err(|e| format!("cannot render page {}: {e}", i + 1))?
                .to_luma8();
            let (w, h) = img.dimensions();
            let luma = img.into_raw();
            Ok(PageScan {
                ink: ink(&luma),
                sheet: decode(&luma, w, h),
            })
        })
        .collect()
}

/// Share of dark pixels.
pub fn ink(luma: &[u8]) -> f32 {
    if luma.is_empty() {
        return 0.0;
    }
    luma.iter().filter(|&&v| v < 128).count() as f32 / luma.len() as f32
}

/// The sheet whose barcode is on this page, if any.
pub fn decode(luma: &[u8], width: u32, height: u32) -> Option<Sheet> {
    // Cheap first: a page with almost no ink holds no barcode.
    if ink(luma) < BLANK_INK {
        return None;
    }
    [BarcodeFormat::QR_CODE, BarcodeFormat::CODE_128]
        .into_iter()
        .find_map(|format| {
            rxing::helpers::detect_in_luma_slice_with_hints(
                luma,
                width,
                height,
                Some(format),
                &mut DecodeHints::default(),
            )
            .ok()
            .and_then(|r| Sheet::parse(r.getText()))
        })
}

/// Splits the stack into documents. A stack with no sheet is one segment with `sheet: None`.
pub fn segments(pages: &[PageScan]) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut current: Option<Option<Cover>> = None;
    let mut i = 0;
    while i < pages.len() {
        if let Some(sheet) = &pages[i].sheet {
            if start < i {
                out.push(Segment {
                    pages: start..i,
                    sheet: current.take(),
                });
            }
            current = Some(sheet.0.clone());
            i += 1;
            // The back of the sheet in a duplex scan.
            if i < pages.len() && pages[i].sheet.is_none() && pages[i].ink < BLANK_INK {
                i += 1;
            }
            start = i;
            continue;
        }
        i += 1;
    }
    if start < pages.len() {
        out.push(Segment {
            pages: start..pages.len(),
            sheet: current,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet::TaxYear;

    fn page(ink: f32) -> PageScan {
        PageScan { sheet: None, ink }
    }

    fn cover(id: &str) -> PageScan {
        PageScan {
            sheet: Some(Sheet(Some(Cover {
                client_id: id.into(),
                doc_type: None,
                tax_year: Some(TaxYear::Year(2025)),
            }))),
            ink: 0.05,
        }
    }

    #[test]
    fn splits_at_sheets_and_drops_sheet_backs() {
        let sep = PageScan {
            sheet: Some(Sheet::separator()),
            ink: 0.05,
        };
        let pages = vec![
            page(0.1),  // 0: before any sheet
            cover("1"), // 1
            page(0.0),  // 2: back of the cover, dropped
            page(0.1),  // 3
            page(0.0),  // 4: a blank page inside a document stays
            cover("2"), // 5
            page(0.1),  // 6
            sep,        // 7
            cover("3"), // 8: sheet right after a sheet: the later one wins
            page(0.1),  // 9
            page(0.1),  // 10
        ];
        let s = segments(&pages);
        let ranges: Vec<_> = s.iter().map(|s| s.pages.clone()).collect();
        assert_eq!(ranges, vec![0..1, 3..5, 6..7, 9..11]);
        assert_eq!(s[0].sheet, None);
        assert_eq!(
            s[1].sheet.as_ref().unwrap().as_ref().unwrap().client_id,
            "1"
        );
        assert_eq!(
            s[3].sheet.as_ref().unwrap().as_ref().unwrap().client_id,
            "3"
        );
        assert_eq!(segments(&[page(0.1), page(0.1)]).len(), 1);
        assert!(segments(&[cover("1")]).is_empty());
    }
}
