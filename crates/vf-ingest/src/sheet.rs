//! Barcode cover and separator sheets (P02 step 3).
//!
//! A sheet carries one payload, printed twice: as a QR code and as a Code 128 barcode, so a
//! smudge on one still decodes. Payload, versioned and ASCII only:
//!
//! - cover sheet: `VF1|C|<client ID>|<doc type id, 32 hex, or empty>|<tax year, `P` = permanent, or empty>`
//! - blank separator: `VF1|S`
//!
//! The client is the client-list ID (the `{ID}` Filer routes on), not the catalog uuid, so a
//! sheet printed before a rebuild still routes. Both kinds start a new document at the sheet;
//! the sheet page itself is never filed.

use std::path::Path;

use pdf_writer::{Content, Name, Pdf, Rect, Ref, Str};
use rxing::{BarcodeFormat, MultiFormatWriter, Writer};

pub const PREFIX: &str = "VF1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaxYear {
    Year(i32),
    Permanent,
}

/// What a cover sheet says about the document after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cover {
    /// Client-list ID (`{ID}`).
    pub client_id: String,
    /// Doc type id (uuid).
    pub doc_type: Option<String>,
    pub tax_year: Option<TaxYear>,
}

/// A decoded sheet: a cover sheet, or a blank separator (`None`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sheet(pub Option<Cover>);

impl Sheet {
    pub fn separator() -> Self {
        Sheet(None)
    }

    pub fn encode(&self) -> Result<String, String> {
        let Some(c) = &self.0 else {
            return Ok(format!("{PREFIX}|S"));
        };
        if c.client_id.is_empty()
            || !c.client_id.is_ascii()
            || c.client_id.contains('|')
            || c.client_id.chars().any(|ch| ch.is_ascii_control())
        {
            return Err(format!(
                "Client ID {:?} cannot go on a barcode (ASCII, no |).",
                c.client_id
            ));
        }
        let doc_type = match &c.doc_type {
            Some(t) => uuid::Uuid::parse_str(t)
                .map_err(|e| format!("doc type id {t}: {e}"))?
                .simple()
                .to_string(),
            None => String::new(),
        };
        let year = match c.tax_year {
            Some(TaxYear::Year(y)) => y.to_string(),
            Some(TaxYear::Permanent) => "P".into(),
            None => String::new(),
        };
        Ok(format!("{PREFIX}|C|{}|{doc_type}|{year}", c.client_id))
    }

    /// Reads a payload; anything that is not a Vibe Files sheet is `None`.
    pub fn parse(text: &str) -> Option<Self> {
        let mut f = text.trim().split('|');
        if f.next()? != PREFIX {
            return None;
        }
        match f.next()? {
            "S" => f.next().is_none().then(Sheet::separator),
            "C" => {
                let client_id = f.next()?.to_string();
                let doc_type = match f.next()? {
                    "" => None,
                    t => Some(uuid::Uuid::parse_str(t).ok()?.to_string()),
                };
                let tax_year = match f.next()? {
                    "" => None,
                    "P" => Some(TaxYear::Permanent),
                    y => Some(TaxYear::Year(y.parse().ok()?)),
                };
                if client_id.is_empty() || f.next().is_some() {
                    return None;
                }
                Some(Sheet(Some(Cover {
                    client_id,
                    doc_type,
                    tax_year,
                })))
            }
            _ => None,
        }
    }
}

/// The words printed on a sheet for people.
#[derive(Debug, Clone, Default)]
pub struct Labels {
    pub client: String,
    pub doc_type: String,
    pub tax_year: String,
    /// The client's folder, and below it the doc type's folder when known
    /// (`Stockton Family Seed (STOC3812)\Income Tax\2025`).
    pub folder: String,
}

const W: f32 = 612.0;
const H: f32 = 792.0;

/// Writes one page per sheet to `path` (replaced if it exists).
pub fn write_pdf(path: &Path, sheets: &[(Sheet, Labels)]) -> Result<(), String> {
    if sheets.is_empty() {
        return Err("no sheets".into());
    }
    let mut pdf = Pdf::new();
    let catalog = Ref::new(1);
    let tree = Ref::new(2);
    let font = Ref::new(3);
    let bold = Ref::new(4);
    let mut next = 5;
    let mut page_ids = Vec::new();
    let mut pages = Vec::new();
    for (sheet, labels) in sheets {
        let (page, content) = (Ref::new(next), Ref::new(next + 1));
        next += 2;
        page_ids.push(page);
        pages.push((page, content, draw(sheet, labels)?));
    }
    pdf.catalog(catalog).pages(tree);
    pdf.pages(tree)
        .kids(page_ids.iter().copied())
        .count(page_ids.len() as i32);
    for (page, content_id, bytes) in pages {
        let mut p = pdf.page(page);
        p.media_box(Rect::new(0.0, 0.0, W, H));
        p.parent(tree);
        p.contents(content_id);
        let mut res = p.resources();
        let mut fonts = res.fonts();
        fonts.pair(Name(b"F1"), font);
        fonts.pair(Name(b"F2"), bold);
        drop(fonts);
        drop(res);
        drop(p);
        pdf.stream(content_id, &bytes);
    }
    pdf.type1_font(font)
        .base_font(Name(b"Helvetica"))
        .encoding_predefined(Name(b"WinAnsiEncoding"));
    pdf.type1_font(bold)
        .base_font(Name(b"Helvetica-Bold"))
        .encoding_predefined(Name(b"WinAnsiEncoding"));
    vf_fs::cabinet::write_replace(path, &pdf.finish())
}

fn draw(sheet: &Sheet, labels: &Labels) -> Result<Vec<u8>, String> {
    let payload = sheet.encode()?;
    let mut c = Content::new();
    let text = |c: &mut Content, font: &[u8], size: f32, x: f32, y: f32, s: &str| {
        c.begin_text();
        c.set_font(Name(font), size);
        c.next_line(x, y);
        c.show(Str(&win_ansi(s)));
        c.end_text();
    };
    let (title, lines) = match &sheet.0 {
        Some(_) => (
            "COVER SHEET",
            vec![
                format!("Client:  {}", labels.client),
                format!("Folder:  {}", or_dash(&labels.folder)),
                format!("Doc type:  {}", or_dash(&labels.doc_type)),
                format!("Tax year:  {}", or_dash(&labels.tax_year)),
            ],
        ),
        None => (
            "SEPARATOR SHEET",
            if labels.client.is_empty() {
                vec!["A new document starts after this sheet.".into()]
            } else {
                vec![
                    format!("Client:  {}", labels.client),
                    format!("Folder:  {}", or_dash(&labels.folder)),
                    "A new document starts after this sheet.".into(),
                ]
            },
        ),
    };
    text(&mut c, b"F2", 30.0, 72.0, 700.0, title);
    for (i, l) in lines.iter().enumerate() {
        text(&mut c, b"F1", 16.0, 72.0, 660.0 - 24.0 * i as f32, l);
    }

    // QR code, 3 inches square, centred.
    let qr = MultiFormatWriter
        .encode(&payload, &BarcodeFormat::QR_CODE, 0, 0)
        .map_err(|e| format!("QR code: {e}"))?;
    let side = 216.0;
    let module = side / qr.width() as f32;
    let (x0, y0) = ((W - side) / 2.0, 300.0);
    for y in 0..qr.height() {
        let mut x = 0;
        while x < qr.width() {
            if !qr.get(x, y) {
                x += 1;
                continue;
            }
            let start = x;
            while x < qr.width() && qr.get(x, y) {
                x += 1;
            }
            c.rect(
                x0 + start as f32 * module,
                y0 + side - (y + 1) as f32 * module,
                (x - start) as f32 * module,
                module,
            );
        }
    }

    // Code 128, 7 inches wide.
    let bars = MultiFormatWriter
        .encode(&payload, &BarcodeFormat::CODE_128, 0, 0)
        .map_err(|e| format!("Code 128: {e}"))?;
    let width = 504.0;
    let module = width / bars.width() as f32;
    let (x0, y0) = ((W - width) / 2.0, 170.0);
    let mut x = 0;
    while x < bars.width() {
        if !bars.get(x, 0) {
            x += 1;
            continue;
        }
        let start = x;
        while x < bars.width() && bars.get(x, 0) {
            x += 1;
        }
        c.rect(
            x0 + start as f32 * module,
            y0,
            (x - start) as f32 * module,
            72.0,
        );
    }
    c.fill_nonzero();

    text(&mut c, b"F1", 9.0, x0, 152.0, &payload);
    text(
        &mut c,
        b"F1",
        10.0,
        72.0,
        90.0,
        "Vibe Files: put this sheet on top of the document and scan the stack. It is not filed.",
    );
    Ok(c.finish().to_vec())
}

fn or_dash(s: &str) -> &str {
    if s.is_empty() { "—" } else { s }
}

/// Text for the standard Type 1 fonts (WinAnsiEncoding): ASCII as is, a few common marks
/// mapped, anything else `?`.
fn win_ansi(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| match c {
            ' '..='~' => c as u8,
            '—' => 0x97,
            '–' => 0x96,
            '·' => 0xB7,
            '’' => 0x92,
            '‘' => 0x91,
            '“' => 0x93,
            '”' => 0x94,
            '§' => 0xA7,
            'é' => 0xE9,
            'ñ' => 0xF1,
            _ => b'?',
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cover() -> Sheet {
        Sheet(Some(Cover {
            client_id: "1042".into(),
            doc_type: Some("0193a0b4-7c3e-7d22-9f00-1234567890ab".into()),
            tax_year: Some(TaxYear::Year(2025)),
        }))
    }

    #[test]
    fn payload_round_trips() {
        let c = cover();
        let p = c.encode().unwrap();
        assert_eq!(p, "VF1|C|1042|0193a0b47c3e7d229f001234567890ab|2025");
        assert_eq!(Sheet::parse(&p), Some(c));
        assert_eq!(Sheet::parse("VF1|S"), Some(Sheet::separator()));
        let perm = Sheet(Some(Cover {
            client_id: "A-7".into(),
            doc_type: None,
            tax_year: Some(TaxYear::Permanent),
        }));
        assert_eq!(Sheet::parse(&perm.encode().unwrap()), Some(perm));
        for bad in [
            "",
            "VF2|S",
            "VF1|X",
            "VF1|S|x",
            "VF1|C||abc|2025",
            "VF1|C|1|nothex|2025",
            "VF1|C|1||20x5",
            "https://example.com",
        ] {
            assert_eq!(Sheet::parse(bad), None, "{bad}");
        }
        let pipe = Sheet(Some(Cover {
            client_id: "1|2".into(),
            doc_type: None,
            tax_year: None,
        }));
        assert!(pipe.encode().is_err());
    }

    #[test]
    fn writes_a_pdf_page_per_sheet() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("sheets.pdf");
        let labels = Labels {
            client: "Anderson Farms LLC (1042)".into(),
            doc_type: "Return copy — federal".into(),
            tax_year: "2025".into(),
            folder: r"Anderson Farms LLC (1042)\Income Tax\2025".into(),
        };
        let client_separator = Labels {
            client: "Anderson Farms LLC (1042)".into(),
            folder: "Anderson Farms LLC (1042)".into(),
            ..Labels::default()
        };
        write_pdf(
            &p,
            &[
                (cover(), labels),
                (Sheet::separator(), Labels::default()),
                (Sheet::separator(), client_separator),
            ],
        )
        .unwrap();
        let d = lopdf::Document::load(&p).unwrap();
        assert_eq!(d.get_pages().len(), 3);
        // The client and folder are printed for people on the cover and client separator.
        let first = d.extract_text(&[1]).unwrap();
        assert!(first.contains("Folder:") && first.contains("Income Tax"), "{first}");
        let third = d.extract_text(&[3]).unwrap();
        assert!(third.contains("Client:") && third.contains("Anderson Farms LLC"), "{third}");
    }
}
