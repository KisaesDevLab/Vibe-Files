//! Barcode cover and separator sheets (PHASES P02 step 3).
//!
//! A sheet carries `VF1|{client_id}|{doc_type}|{tax_year}|{split}` as a QR code (top right) and
//! a Code128 barcode (bottom), so either survives a poor scan. `client_id` is the catalog /
//! sentinel id; `split` is `S` when the sheet starts a new document. A blank separator has no
//! client: the next document inherits the client and year of the cover before it.
//!
//! [`scan`] renders each page of a scanned stack, finds sheet pages, and [`split_stack`] writes
//! one PDF per segment with Filer's own `split_pdf` (sheet pages dropped).

use std::ops::Range;
use std::path::{Path, PathBuf};

use pdf_writer::{Content, Name, Pdf, Rect, Ref, Str};
use pdfium_render::prelude::{PdfRenderConfig, Pdfium};
use rxing::{BarcodeFormat, Writer};

const MAGIC: &str = "VF1";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Payload {
    /// Catalog client id (uuid); empty on a blank separator.
    pub client_id: String,
    /// A doc type name.
    pub doc_type: String,
    pub tax_year: Option<i64>,
    /// Starts a new document.
    pub split: bool,
}

impl Payload {
    pub fn separator() -> Self {
        Payload {
            split: true,
            ..Payload::default()
        }
    }

    pub fn is_separator(&self) -> bool {
        self.client_id.is_empty()
    }

    pub fn encode(&self) -> String {
        let clean = |s: &str| s.replace('|', "/");
        format!(
            "{MAGIC}|{}|{}|{}|{}",
            clean(&self.client_id),
            clean(&self.doc_type),
            self.tax_year.map(|y| y.to_string()).unwrap_or_default(),
            if self.split { "S" } else { "-" }
        )
    }

    /// The payload with every non-ASCII byte (and `%`) as `%XX`: what Code128 can carry.
    pub fn encode_ascii(&self) -> String {
        let mut out = String::new();
        for b in self.encode().bytes() {
            if b.is_ascii_graphic() && b != b'%' || b == b' ' {
                out.push(b as char);
            } else {
                out.push_str(&format!("%{b:02X}"));
            }
        }
        out
    }

    /// Reads either form ([`Payload::encode`] or [`Payload::encode_ascii`]).
    pub fn decode(s: &str) -> Option<Self> {
        let s = unescape(s.trim())?;
        let parts: Vec<&str> = s.split('|').collect();
        let [magic, client, doc_type, year, split] = parts.as_slice() else {
            return None;
        };
        if *magic != MAGIC {
            return None;
        }
        Some(Payload {
            client_id: (*client).to_owned(),
            doc_type: (*doc_type).to_owned(),
            tax_year: if year.is_empty() {
                None
            } else {
                Some(year.parse().ok()?)
            },
            split: *split == "S",
        })
    }
}

fn unescape(s: &str) -> Option<String> {
    if !s.contains('%') {
        return Some(s.to_owned());
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// One printed sheet: its payload and the human-readable lines above the barcodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sheet {
    pub payload: Payload,
    pub lines: Vec<String>,
}

impl Sheet {
    /// A cover sheet for a client (`name`, CSV `ext_id`) and optional doc type / year.
    pub fn cover(
        client_id: &str,
        name: &str,
        ext_id: &str,
        doc_type: &str,
        year: Option<i64>,
    ) -> Self {
        let mut lines = vec![format!("{name}  ({ext_id})")];
        if !doc_type.is_empty() {
            lines.push(format!("Doc type: {doc_type}"));
        }
        if let Some(y) = year {
            lines.push(format!("Tax year: {y}"));
        }
        Sheet {
            payload: Payload {
                client_id: client_id.into(),
                doc_type: doc_type.into(),
                tax_year: year,
                split: true,
            },
            lines,
        }
    }

    pub fn separator() -> Self {
        Sheet {
            payload: Payload::separator(),
            lines: vec!["Separator: the pages after this sheet are a new document".into()],
        }
    }
}

/// Text Helvetica (WinAnsi) can show: common punctuation mapped, anything else replaced.
fn win_ansi(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| match c {
            '—' => 0x97,
            '–' => 0x96,
            '§' => 0xA7,
            '’' => 0x92,
            c if (c as u32) < 0x7F && (c as u32) >= 0x20 => c as u8,
            _ => b'?',
        })
        .collect()
}

/// Draws a barcode matrix as filled squares: `x, y` is the top-left corner in points.
fn draw_matrix(
    c: &mut Content,
    m: &rxing::common::BitMatrix,
    x: f32,
    y: f32,
    module_w: f32,
    module_h: f32,
) {
    for row in 0..m.height() {
        let mut col = 0;
        while col < m.width() {
            if m.get(col, row) {
                let start = col;
                while col < m.width() && m.get(col, row) {
                    col += 1;
                }
                c.rect(
                    x + start as f32 * module_w,
                    y - (row + 1) as f32 * module_h,
                    (col - start) as f32 * module_w,
                    module_h,
                );
            } else {
                col += 1;
            }
        }
    }
    c.set_fill_gray(0.0);
    c.fill_nonzero();
}

fn encode(
    contents: &str,
    format: BarcodeFormat,
    w: i32,
    h: i32,
) -> Result<rxing::common::BitMatrix, String> {
    rxing::MultiFormatWriter
        .encode(contents, &format, w, h)
        .map_err(|e| format!("cannot encode barcode: {e}"))
}

/// Writes the sheets as one US Letter PDF, one sheet per page.
pub fn write_sheets(path: &Path, sheets: &[Sheet]) -> Result<(), String> {
    if sheets.is_empty() {
        return Err("no sheets to write".into());
    }
    let n = sheets.len() as i32;
    let catalog = Ref::new(1);
    let pages = Ref::new(2);
    let font = Ref::new(3);
    let page_id = |i: i32| Ref::new(10 + i * 2);
    let content_id = |i: i32| Ref::new(11 + i * 2);
    let mut pdf = Pdf::new();
    pdf.catalog(catalog).pages(pages);
    pdf.pages(pages).kids((0..n).map(page_id)).count(n);
    pdf.type1_font(font).base_font(Name(b"Helvetica"));
    for (i, sheet) in sheets.iter().enumerate() {
        let i = i as i32;
        let mut p = pdf.page(page_id(i));
        p.media_box(Rect::new(0.0, 0.0, 612.0, 792.0));
        p.parent(pages);
        p.contents(content_id(i));
        p.resources().fonts().pair(Name(b"F1"), font);
        drop(p);
        let data = sheet.payload.encode();
        let mut c = Content::new();
        c.begin_text();
        c.set_font(Name(b"F1"), 22.0);
        c.next_line(54.0, 720.0);
        let title = if sheet.payload.is_separator() {
            "VIBE FILES SEPARATOR SHEET"
        } else {
            "VIBE FILES COVER SHEET"
        };
        c.show(Str(title.as_bytes()));
        c.set_font(Name(b"F1"), 14.0);
        for l in &sheet.lines {
            c.next_line(0.0, -26.0);
            c.show(Str(&win_ansi(l)));
        }
        c.set_font(Name(b"F1"), 10.0);
        c.next_line(0.0, -40.0);
        c.show(Str(
            b"Place this sheet on top of the pages it describes, face up, and scan the stack.",
        ));
        c.end_text();
        // QR: 2.2 in square, top right.
        let qr = encode(&data, BarcodeFormat::QR_CODE, 0, 0)?;
        let side = 158.0 / qr.width() as f32;
        draw_matrix(&mut c, &qr, 612.0 - 54.0 - 158.0, 792.0 - 54.0, side, side);
        // Code128: full width, 0.8 in tall, near the bottom.
        let bar = encode(&sheet.payload.encode_ascii(), BarcodeFormat::CODE_128, 0, 1)?;
        let module = (504.0 / bar.width() as f32).min(1.2);
        draw_matrix(
            &mut c,
            &bar,
            54.0,
            150.0,
            module,
            58.0 / bar.height() as f32,
        );
        pdf.stream(content_id(i), &c.finish());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, pdf.finish()).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Finds a sheet on one rendered page (RGBA bytes): QR first, Code128 if the QR is unreadable.
pub fn decode_rgba(rgba: &[u8], width: u32, height: u32) -> Option<Payload> {
    let luma: Vec<u8> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| ((p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000) as u8)
        .collect();
    // QR first (a copy of the buffer), then Code128 (the buffer itself).
    let read = |luma: Vec<u8>, format| {
        rxing::helpers::detect_in_luma(luma, width, height, Some(format))
            .ok()
            .and_then(|r| Payload::decode(r.getText()))
    };
    read(luma.clone(), BarcodeFormat::QR_CODE).or_else(|| read(luma, BarcodeFormat::CODE_128))
}

/// Sheet payload per page of a scanned PDF (`None` = an ordinary page).
pub fn scan(pdfium: &Pdfium, path: &Path) -> Result<Vec<Option<Payload>>, String> {
    let doc = pdfium
        .load_pdf_from_file(path, None)
        .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let cfg = PdfRenderConfig::new().set_target_width(1275); // 150 dpi on US Letter
    let mut out = Vec::new();
    for page in doc.pages().iter() {
        let bmp = page
            .render_with_config(&cfg)
            .map_err(|e| format!("cannot render a page of {}: {e}", path.display()))?;
        out.push(decode_rgba(
            &bmp.as_rgba_bytes(),
            bmp.width() as u32,
            bmp.height() as u32,
        ));
    }
    Ok(out)
}

/// One document of a split stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// Pages of the scanned PDF (0-based, end exclusive), sheet pages excluded.
    pub pages: Range<usize>,
    /// The cover that applies (a separator inherits the cover before it); `None` for pages
    /// before the first sheet.
    pub payload: Option<Payload>,
}

/// Cuts at every sheet page. Sheets themselves are left out; a sheet with no pages after it
/// (two sheets in a row, or a sheet last) makes no segment.
pub fn segments(pages: &[Option<Payload>]) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut current: Option<Payload> = None;
    let mut start = 0;
    let flush = |start: usize, end: usize, p: &Option<Payload>, out: &mut Vec<Segment>| {
        if end > start {
            out.push(Segment {
                pages: start..end,
                payload: p.clone(),
            });
        }
    };
    for (i, p) in pages.iter().enumerate() {
        if let Some(sheet) = p {
            flush(start, i, &current, &mut out);
            current = if sheet.is_separator() {
                current.clone().map(|c| Payload {
                    doc_type: String::new(),
                    ..c
                })
            } else {
                Some(sheet.clone())
            };
            start = i + 1;
        }
    }
    flush(start, pages.len(), &current, &mut out);
    out
}

/// Writes each segment of `path` to the matching `outs` path (Filer's `split_pdf`).
pub fn split_stack(path: &Path, segs: &[Segment], outs: &[PathBuf]) -> Result<(), String> {
    let ranges: Vec<Range<usize>> = segs.iter().map(|s| s.pages.clone()).collect();
    filer_pdf::pdf::split_pdf(path, &ranges, outs)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn payload_round_trips() {
        let p = Payload {
            client_id: "0190a1b2-0000-7000-8000-000000000001".into(),
            doc_type: "Return copy — federal".into(),
            tax_year: Some(2025),
            split: true,
        };
        assert_eq!(Payload::decode(&p.encode()), Some(p.clone()));
        assert!(p.encode_ascii().is_ascii());
        assert_eq!(Payload::decode(&p.encode_ascii()), Some(p));
        assert_eq!(
            Payload::decode(&Payload::separator().encode()),
            Some(Payload::separator())
        );
        assert_eq!(Payload::decode("VF2|a|b|2025|S"), None);
        assert_eq!(Payload::decode("hello"), None);
        assert!(
            Payload {
                doc_type: "a|b".into(),
                ..Payload::default()
            }
            .encode()
            .contains("a/b")
        );
    }

    #[test]
    fn segments_cut_at_sheets_and_separators_inherit() {
        let cover = |c: &str| {
            Some(Payload {
                client_id: c.into(),
                doc_type: "W-2".into(),
                tax_year: Some(2025),
                split: true,
            })
        };
        let pages = vec![
            None,
            cover("A"),
            None,
            None,
            Some(Payload::separator()),
            None,
            cover("B"),
            cover("C"),
            None,
        ];
        let s = segments(&pages);
        assert_eq!(s.len(), 4);
        assert_eq!((s[0].pages.clone(), s[0].payload.is_none()), (0..1, true));
        assert_eq!(s[1].pages, 2..4);
        assert_eq!(s[1].payload.as_ref().unwrap().client_id, "A");
        assert_eq!(s[2].pages, 5..6);
        let sep = s[2].payload.as_ref().unwrap();
        assert_eq!(
            (sep.client_id.as_str(), sep.doc_type.as_str(), sep.tax_year),
            ("A", "", Some(2025))
        );
        assert_eq!(
            s[3].payload.as_ref().unwrap().client_id,
            "C",
            "B had no pages"
        );
    }

    pub fn pdfium() -> &'static Pdfium {
        let dir = filer_pdf::pdf::find_pdfium(Path::new("")).expect("pdfium.dll for tests");
        filer_pdf::pdf::pdfium(&dir).unwrap()
    }

    #[test]
    fn printed_sheets_decode() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("sheets.pdf");
        let a = Sheet::cover(
            "0190a1b2-0000-7000-8000-00000000000a",
            "Anderson Farms LLC",
            "1042",
            "Return copy — federal",
            Some(2025),
        );
        write_sheets(&p, &[a.clone(), Sheet::separator()]).unwrap();
        let found = scan(pdfium(), &p).unwrap();
        assert_eq!(found, vec![Some(a.payload), Some(Payload::separator())]);
    }
}
