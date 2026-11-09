//! What is inside a file that is not a PDF (V27, owner 2026-11-09), read for the preview panes
//! without Office: a spreadsheet's cells (Excel `.xlsx`/`.xls`, OpenDocument, CSV, tab-separated
//! and QuickBooks `.iif`), a document's text (Word `.docx`, PowerPoint `.pptx`, RTF, plain text),
//! a mail's headers and text (`.eml`, Outlook `.msg`) or a picture.
//!
//! Contents only, not the printed look: no fonts, page layout or pictures inside documents.
//! Everything read is capped (file size, rows, columns, characters, unpacked size), so a huge or
//! hostile inbox file shows a note instead of filling memory. Called off the UI thread.

use std::io::Read;
use std::path::Path;

use quick_xml::events::Event;

/// Files larger than this are not read.
pub const MAX_FILE: u64 = 40 * 1024 * 1024;
/// A zip-based file (`.xlsx`, `.docx` …) whose parts unpack to more than this is not read.
const MAX_UNPACKED: u64 = 400 * 1024 * 1024;
/// Rows and columns shown per sheet, and sheets per workbook.
pub const MAX_ROWS: usize = 2000;
pub const MAX_COLS: usize = 60;
const MAX_SHEETS: usize = 40;
/// Characters of text shown.
pub const MAX_CHARS: usize = 200_000;
/// A picture is shrunk to fit this many pixels on its longer side.
const PICTURE_SIDE: u32 = 1600;

/// What a file holds, ready to draw.
#[derive(Debug)]
pub enum Content {
    Sheets(Vec<Sheet>),
    Text(Doc),
    Picture(Picture),
    /// Nothing to show, and why (one line).
    Nothing(String),
}

#[derive(Debug, Default)]
pub struct Sheet {
    pub name: String,
    /// Cell text, row by row (rows may be shorter than `cols`).
    pub rows: Vec<Vec<String>>,
    pub cols: usize,
    /// 0-based row and column of the first cell (a sheet whose data starts at C5 is labelled
    /// from C5).
    pub origin: (usize, usize),
    /// Set when not everything is shown ("First 2,000 of 5,120 rows shown.").
    pub cut: Option<String>,
}

#[derive(Debug, Default)]
pub struct Doc {
    /// Mail headers (From, To, Subject …), shown above the text.
    pub header: Vec<(String, String)>,
    pub paras: Vec<Para>,
    /// The text was longer than [`MAX_CHARS`].
    pub cut: bool,
}

#[derive(Debug, PartialEq)]
pub struct Para {
    pub text: String,
    pub heading: bool,
}

#[derive(Debug)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    /// RGBA, `width * height * 4` bytes.
    pub rgba: Vec<u8>,
    /// Pages in a multi-page TIFF (only the first is shown).
    pub pages: usize,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Workbook,
    Delimited(u8),
    Docx,
    Pptx,
    Rtf,
    Plain,
    Html,
    Eml,
    Msg,
    Image,
    OldOffice,
}

fn kind(path: &Path) -> Option<Kind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        // Not OpenDocument (.ods): its repeated rows and columns cannot be capped while read.
        "xlsx" | "xlsm" | "xlsb" | "xls" => Kind::Workbook,
        "csv" => Kind::Delimited(b','),
        "tsv" | "tab" | "iif" => Kind::Delimited(b'\t'),
        "docx" | "docm" => Kind::Docx,
        "pptx" | "pptm" => Kind::Pptx,
        "rtf" => Kind::Rtf,
        "txt" | "text" | "log" | "xml" | "json" | "md" | "ini" | "prn" | "qbo" | "ofx" | "qfx" => {
            Kind::Plain
        }
        "htm" | "html" => Kind::Html,
        "eml" => Kind::Eml,
        "msg" => Kind::Msg,
        "doc" | "ppt" | "pps" => Kind::OldOffice,
        e if crate::imagepdf::CONVERTIBLE.contains(&e) => Kind::Image,
        _ => return None,
    })
}

/// Whether the preview panes read this kind of file (by its extension).
pub fn viewable(path: &Path) -> bool {
    kind(path).is_some()
}

/// Reads `path` for a preview. Never fails: a file that cannot be read gives
/// [`Content::Nothing`] with the reason.
pub fn read(path: &Path) -> Content {
    let Some(kind) = kind(path) else {
        return Content::Nothing("No preview for this kind of file.".into());
    };
    if kind == Kind::OldOffice {
        return Content::Nothing(
            "Older Word and PowerPoint files (.doc, .ppt) are not shown here: open the file to see it."
                .into(),
        );
    }
    match std::fs::metadata(path) {
        Ok(m) if m.len() > MAX_FILE => {
            return Content::Nothing(format!(
                "This file is too large to show here ({} MB): open the file to see it.",
                m.len() / (1024 * 1024)
            ));
        }
        Ok(_) => {}
        Err(e) => return Content::Nothing(format!("Cannot read this file: {e}")),
    }
    let result = match kind {
        Kind::Workbook => workbook(path),
        Kind::Delimited(d) => delimited(path, d),
        Kind::Docx => docx(path).map(Content::Text),
        Kind::Pptx => pptx(path).map(Content::Text),
        Kind::Rtf => bytes(path).map(|b| Content::Text(plain_doc(&rtf_text(&b)))),
        Kind::Plain => bytes(path).map(|b| Content::Text(plain_doc(&decode_text(&b)))),
        Kind::Html => bytes(path).map(|b| Content::Text(plain_doc(&html_text(&decode_text(&b))))),
        Kind::Eml => bytes(path).and_then(|b| eml(&b)).map(Content::Text),
        Kind::Msg => msg(path).map(Content::Text),
        Kind::Image => picture(path).map(Content::Picture),
        Kind::OldOffice => unreachable!(),
    };
    result.unwrap_or_else(|e| Content::Nothing(format!("Cannot read this file: {e}")))
}

fn bytes(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| e.to_string())
}

/// Text from bytes: UTF-16 or UTF-8 (with or without a byte-order mark); anything else is read
/// as Windows-1252.
fn decode_text(b: &[u8]) -> String {
    if let Some(rest) = b.strip_prefix(&[0xFF, 0xFE]) {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    if let Some(rest) = b.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_be_bytes(*c))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    let b = b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(b);
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => b.iter().map(|&c| cp1252(c)).collect(),
    }
}

/// A Windows-1252 byte as a character.
fn cp1252(c: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8D}', 'Ž',
        '\u{8F}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9D}',
        'ž', 'Ÿ',
    ];
    match c {
        0x80..=0x9F => HIGH[(c - 0x80) as usize],
        _ => c as char,
    }
}

/// Text as paragraphs (one per line), cut at [`MAX_CHARS`].
fn plain_doc(text: &str) -> Doc {
    let mut doc = Doc::default();
    let mut chars = 0;
    for line in text.lines() {
        if chars >= MAX_CHARS {
            doc.cut = true;
            break;
        }
        // One long line (minified XML, a bank download with no line breaks) is cut too (QA).
        let mut text = line.trim_end().to_string();
        if clip(&mut text, MAX_CHARS - chars) {
            doc.cut = true;
        }
        chars += text.chars().count() + 1;
        doc.paras.push(Para {
            text,
            heading: false,
        });
    }
    // Trailing blank lines say nothing.
    while doc.paras.last().is_some_and(|p| p.text.is_empty()) {
        doc.paras.pop();
    }
    doc
}

// ---- Spreadsheets ----

/// Characters kept of one cell.
const MAX_CELL: usize = 1000;
/// An old `.xls` whose cells would fill more than this many grid places is not read: calamine
/// builds every sheet as one block from its first to its last cell when the file is opened.
const MAX_XLS_CELLS: u64 = 3_000_000;

fn workbook(path: &Path) -> Result<Content, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let (names, mut sheets) = match ext.as_str() {
        "xls" => xls_sheets(path)?,
        "xlsb" => {
            use calamine::Reader;
            check_unpacked(path)?;
            let mut book: calamine::Xlsb<std::io::BufReader<std::fs::File>> =
                calamine::open_workbook(path).map_err(|e: calamine::XlsbError| e.to_string())?;
            let names = book.sheet_names();
            let mut sheets = Vec::new();
            for name in names.iter().take(MAX_SHEETS) {
                let sheet = match book.worksheet_cells_reader(name) {
                    Ok(mut r) => stream_sheet(name, || {
                        r.next_cell()
                            .map(|c| {
                                c.map(|c| {
                                    (
                                        c.get_position(),
                                        calamine::Data::from(c.get_value().clone()),
                                    )
                                })
                            })
                            .map_err(|e| e.to_string())
                    })?,
                    Err(_) => Sheet::named(name),
                };
                sheets.push(sheet);
            }
            (names, sheets)
        }
        _ => {
            use calamine::Reader;
            check_unpacked(path)?;
            let mut book: calamine::Xlsx<std::io::BufReader<std::fs::File>> =
                calamine::open_workbook(path).map_err(|e: calamine::XlsxError| e.to_string())?;
            let names = book.sheet_names();
            let mut sheets = Vec::new();
            for name in names.iter().take(MAX_SHEETS) {
                let sheet = match book.worksheet_cells_reader(name) {
                    Ok(mut r) => stream_sheet(name, || {
                        r.next_cell()
                            .map(|c| {
                                c.map(|c| {
                                    (
                                        c.get_position(),
                                        calamine::Data::from(c.get_value().clone()),
                                    )
                                })
                            })
                            .map_err(|e| e.to_string())
                    })?,
                    // A chart sheet or a sheet calamine cannot read: shown empty, not an error.
                    Err(_) => Sheet::named(name),
                };
                sheets.push(sheet);
            }
            (names, sheets)
        }
    };
    if names.len() > MAX_SHEETS
        && let Some(last) = sheets.last_mut()
    {
        let more = format!(
            "Only the first {MAX_SHEETS} of {} sheets are shown.",
            names.len()
        );
        last.cut = Some(match last.cut.take() {
            Some(c) => format!("{c} {more}"),
            None => more,
        });
    }
    if sheets.is_empty() {
        return Ok(Content::Nothing("This workbook has no sheets.".into()));
    }
    Ok(Content::Sheets(sheets))
}

impl Sheet {
    fn named(name: &str) -> Sheet {
        Sheet {
            name: name.to_string(),
            ..Sheet::default()
        }
    }
}

/// One sheet read cell by cell (`next` gives each cell's 0-based (row, column) and value): only
/// the cells inside the first [`MAX_ROWS`] × [`MAX_COLS`] from the first filled cell are kept,
/// so a sheet with one stray value far down never builds a huge grid (QA).
fn stream_sheet(
    name: &str,
    mut next: impl FnMut() -> Result<Option<((u32, u32), calamine::Data)>, String>,
) -> Result<Sheet, String> {
    let mut kept: Vec<(u32, u32, String)> = Vec::new();
    let (mut r0, mut r1, mut c0, mut c1) = (u32::MAX, 0u32, u32::MAX, 0u32);
    let limit = MAX_ROWS * MAX_COLS * 2;
    while let Some(((r, c), v)) = next()? {
        let mut text = cell_text(&v);
        if text.is_empty() {
            continue;
        }
        (r0, r1, c0, c1) = (r0.min(r), r1.max(r), c0.min(c), c1.max(c));
        let inside = ((r - r0) as usize) < MAX_ROWS && ((c - c0) as usize) < MAX_COLS;
        if inside && kept.len() < limit {
            clip(&mut text, MAX_CELL);
            kept.push((r, c, text));
        }
    }
    if kept.is_empty() {
        return Ok(Sheet::named(name));
    }
    let (height, width) = ((r1 - r0 + 1) as usize, (c1 - c0 + 1) as usize);
    let (rows_n, cols) = (height.min(MAX_ROWS), width.min(MAX_COLS));
    let mut rows = vec![Vec::<String>::new(); rows_n];
    for (r, c, text) in kept {
        let (ri, ci) = ((r - r0) as usize, (c - c0) as usize);
        if ri < rows_n && ci < cols {
            let row = &mut rows[ri];
            if row.len() <= ci {
                row.resize(ci + 1, String::new());
            }
            row[ci] = text;
        }
    }
    Ok(Sheet {
        name: name.to_string(),
        rows,
        cols,
        origin: (r0 as usize, c0 as usize),
        cut: cut_note(height, width),
    })
}

/// An old `.xls`: refused when its sheets would build too large a grid (checked from its cell
/// records before calamine opens it), else read whole.
fn xls_sheets(path: &Path) -> Result<(Vec<String>, Vec<Sheet>), String> {
    use calamine::Reader;
    if xls_grid_cells(path)? > MAX_XLS_CELLS {
        return Err("this workbook's sheets are too large to show here: open the file".into());
    }
    let mut book: calamine::Xls<std::io::BufReader<std::fs::File>> =
        calamine::open_workbook(path).map_err(|e: calamine::XlsError| e.to_string())?;
    let names = book.sheet_names();
    let mut sheets = Vec::new();
    for name in names.iter().take(MAX_SHEETS) {
        let sheet = match book.worksheet_range(name) {
            Ok(range) => {
                let mut cells = range.cells();
                stream_sheet(name, || {
                    Ok(cells.next().map(|(r, c, v)| {
                        let (r0, c0) = range.start().unwrap_or_default();
                        ((r as u32 + r0, c as u32 + c0), v.clone())
                    }))
                })?
            }
            Err(_) => Sheet::named(name),
        };
        sheets.push(sheet);
    }
    Ok((names, sheets))
}

/// The grid places calamine would build for an `.xls`: per sheet, the box from its first to its
/// last value cell (values and formulas), summed. Read from the BIFF records of the Workbook
/// stream.
fn xls_grid_cells(path: &Path) -> Result<u64, String> {
    let mut c = cfb::open(path).map_err(|_| "not a valid Excel file".to_string())?;
    let mut s = c
        .open_stream("/Workbook")
        .or_else(|_| c.open_stream("/Book"))
        .map_err(|_| "not a valid Excel file".to_string())?;
    let mut b = Vec::new();
    s.by_ref()
        .take(MAX_FILE)
        .read_to_end(&mut b)
        .map_err(|e| e.to_string())?;
    let mut total = 0u64;
    // (rows, cols) boxes of values and of formulas in the current sheet.
    let mut boxes = [(u16::MAX, 0u16, u16::MAX, 0u16); 2];
    let close = |boxes: &mut [(u16, u16, u16, u16); 2], total: &mut u64| {
        for bx in boxes.iter_mut() {
            if bx.0 != u16::MAX {
                *total += (bx.1 - bx.0 + 1) as u64 * (bx.3 - bx.2 + 1) as u64;
            }
            *bx = (u16::MAX, 0, u16::MAX, 0);
        }
    };
    let mut i = 0;
    while i + 4 <= b.len() {
        let typ = u16::from_le_bytes([b[i], b[i + 1]]);
        let len = u16::from_le_bytes([b[i + 2], b[i + 3]]) as usize;
        let data = b.get(i + 4..i + 4 + len).unwrap_or_default();
        i += 4 + len;
        let rc = |d: &[u8]| -> Option<(u16, u16)> {
            Some((
                u16::from_le_bytes(d.get(0..2)?.try_into().ok()?),
                u16::from_le_bytes(d.get(2..4)?.try_into().ok()?),
            ))
        };
        let mut add = |which: usize, r: u16, c: u16| {
            let bx = &mut boxes[which];
            *bx = (bx.0.min(r), bx.1.max(r), bx.2.min(c), bx.3.max(c));
        };
        match typ {
            // BOF of a new sheet, EOF of one.
            0x0809 | 0x000A => close(&mut boxes, &mut total),
            0x0203 | 0x0204 | 0x00D6 | 0x0205 | 0x027E | 0x00FD => {
                if let Some((r, c)) = rc(data) {
                    add(0, r, c);
                }
            }
            0x0006 => {
                if let Some((r, c)) = rc(data) {
                    add(0, r, c);
                    add(1, r, c);
                }
            }
            // MulRk: row, first column, …, last column.
            0x00BD if data.len() >= 6 => {
                if let Some((r, c)) = rc(data) {
                    let last = u16::from_le_bytes([data[data.len() - 2], data[data.len() - 1]]);
                    add(0, r, c);
                    add(0, r, last.max(c));
                }
            }
            _ => {}
        }
        if total > MAX_XLS_CELLS {
            break;
        }
    }
    close(&mut boxes, &mut total);
    Ok(total)
}

/// Cuts `s` to at most `max` characters (on a character boundary). True when it was cut.
fn clip(s: &mut String, max: usize) -> bool {
    match s.char_indices().nth(max) {
        Some((i, _)) => {
            s.truncate(i);
            s.push('…');
            true
        }
        None => false,
    }
}

fn cut_note(height: usize, width: usize) -> Option<String> {
    let rows = (height > MAX_ROWS).then(|| {
        format!(
            "first {} of {} rows",
            thousands(MAX_ROWS),
            thousands(height)
        )
    });
    let cols = (width > MAX_COLS).then(|| format!("first {MAX_COLS} of {width} columns"));
    match (rows, cols) {
        (None, None) => None,
        (Some(r), None) => Some(format!("Only the {r} are shown.")),
        (None, Some(c)) => Some(format!("Only the {c} are shown.")),
        (Some(r), Some(c)) => Some(format!("Only the {r} and the {c} are shown.")),
    }
}

fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn cell_text(d: &calamine::Data) -> String {
    use calamine::Data;
    match d {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Int(i) => i.to_string(),
        Data::Float(f) => number(*f),
        Data::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
        Data::DateTime(dt) => match dt.as_datetime() {
            Some(t) if dt.is_datetime() => {
                if t.time() == chrono::NaiveTime::MIN {
                    t.format("%m/%d/%Y").to_string()
                } else if t.date()
                    == chrono::NaiveDate::from_ymd_opt(1899, 12, 31).unwrap_or_default()
                    || t.date() == chrono::NaiveDate::from_ymd_opt(1899, 12, 30).unwrap_or_default()
                {
                    t.format("%I:%M %p").to_string()
                } else {
                    t.format("%m/%d/%Y %I:%M %p").to_string()
                }
            }
            _ => number(dt.as_f64()),
        },
        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
        Data::Error(e) => e.to_string(),
    }
}

/// A number as Excel's General format would show it, near enough.
fn number(f: f64) -> String {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{}", f as i64)
    } else {
        // Ten significant digits: 0.1 + 0.2 shows 0.3, not 0.30000000000000004.
        let s = format!(
            "{:.*}",
            10usize.saturating_sub(f.abs().log10().ceil().max(0.0) as usize),
            f
        );
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            s
        }
    }
}

fn delimited(path: &Path, delimiter: u8) -> Result<Content, String> {
    let text = decode_text(&bytes(path)?);
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .from_reader(text.as_bytes());
    let mut rows = Vec::new();
    let (mut height, mut width) = (0usize, 0usize);
    for record in reader.records() {
        let record = record.map_err(|e| e.to_string())?;
        height += 1;
        width = width.max(record.len());
        if rows.len() < MAX_ROWS {
            rows.push(
                record
                    .iter()
                    .take(MAX_COLS)
                    .map(|c| {
                        let mut c = c.to_string();
                        clip(&mut c, MAX_CELL);
                        c
                    })
                    .collect(),
            );
        }
    }
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(Content::Sheets(vec![Sheet {
        name,
        rows,
        cols: width.min(MAX_COLS),
        origin: (0, 0),
        cut: cut_note(height, width),
    }]))
}

// ---- Word, PowerPoint (Office Open XML) ----

/// Refuses a zip whose parts unpack to more than [`MAX_UNPACKED`] (a "zip bomb").
fn check_unpacked(path: &Path) -> Result<(), String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|_| "not a valid Office file".to_string())?;
    let mut total = 0u64;
    for i in 0..zip.len() {
        let f = zip.by_index_raw(i).map_err(|e| e.to_string())?;
        total = total.saturating_add(f.size());
    }
    if total > MAX_UNPACKED {
        return Err("it unpacks to more than 400 MB".into());
    }
    Ok(())
}

/// One part of an Office file, at most [`MAX_UNPACKED`] bytes read.
fn zip_part(zip: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Result<Vec<u8>, String> {
    let f = zip
        .by_name(name)
        .map_err(|_| "not a valid Office file".to_string())?;
    let mut out = Vec::new();
    f.take(MAX_UNPACKED)
        .read_to_end(&mut out)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

fn open_zip(path: &Path) -> Result<zip::ZipArchive<std::fs::File>, String> {
    check_unpacked(path)?;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    zip::ZipArchive::new(file).map_err(|_| "not a valid Office file".to_string())
}

/// The value of a `&name;` or `&#n;` reference inside XML text.
fn xml_ref(r: &quick_xml::events::BytesRef) -> String {
    if let Ok(Some(c)) = r.resolve_char_ref() {
        return c.to_string();
    }
    match &**r {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        _ => "",
    }
    .to_string()
}

/// Adds `para` (cut to what is left of [`MAX_CHARS`]); returns false once full.
fn push_para(doc: &mut Doc, chars: &mut usize, mut text: String, heading: bool) -> bool {
    if *chars >= MAX_CHARS {
        doc.cut = true;
        return false;
    }
    if clip(&mut text, MAX_CHARS - *chars) {
        doc.cut = true;
    }
    *chars += text.chars().count() + 1;
    doc.paras.push(Para { text, heading });
    true
}

/// Adds `text` to a paragraph being read unless it has reached [`MAX_CHARS`] (QA: one huge
/// paragraph must not grow without end).
fn grow(para: &mut String, text: &str) {
    if para.len() < MAX_CHARS * 4 {
        para.push_str(text);
    }
}

/// A Word document's text: paragraphs (headings marked), and each table row on one line with
/// its cells between bars. A text box's paragraphs are their own; Word's second (fallback) copy
/// of a text box, tab-stop settings and field codes are left out.
fn docx(path: &Path) -> Result<Doc, String> {
    let mut zip = open_zip(path)?;
    let xml = zip_part(&mut zip, "word/document.xml")?;
    let mut reader = quick_xml::Reader::from_reader(xml.as_slice());
    let mut buf = Vec::new();
    let mut doc = Doc::default();
    let mut chars = 0usize;
    let (mut para, mut heading, mut in_text) = (String::new(), false, false);
    // Paragraphs a text box interrupted (it sits inside one).
    let mut outer: Vec<(String, bool)> = Vec::new();
    // Inside a table: the row's cells and the cell being read; outer tables' while in a nested
    // one.
    let mut table_depth = 0usize;
    let (mut cells, mut cell): (Vec<String>, String) = (Vec::new(), String::new());
    let mut tables: Vec<(Vec<String>, String)> = Vec::new();
    // Inside `mc:Fallback` (skipped) and `w:tabs` (tab stops, not tabs).
    let (mut fallback, mut in_tabs) = (0usize, false);
    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| e.to_string())?;
        if fallback > 0 {
            match &event {
                Event::Start(e) if e.local_name().as_ref() == "Fallback" => fallback += 1,
                Event::End(e) if e.local_name().as_ref() == "Fallback" => fallback -= 1,
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
            continue;
        }
        let mut full = false;
        match event {
            // An empty paragraph: a blank line (inside a table, nothing).
            Event::Empty(e) if e.local_name().as_ref() == "p" => {
                if table_depth == 0 {
                    full = !push_para(&mut doc, &mut chars, String::new(), false);
                }
            }
            Event::Start(e) => match e.local_name().as_ref() {
                "t" => in_text = true,
                "Fallback" => fallback = 1,
                "tabs" => in_tabs = true,
                "tab" if !in_tabs => para.push('\t'),
                "tbl" => {
                    table_depth += 1;
                    tables.push((std::mem::take(&mut cells), std::mem::take(&mut cell)));
                }
                "txbxContent" => {
                    outer.push((std::mem::take(&mut para), heading));
                    heading = false;
                }
                _ => {}
            },
            Event::Empty(e) => match e.local_name().as_ref() {
                "tab" if !in_tabs => para.push('\t'),
                "br" | "cr" => para.push('\n'),
                "pStyle" => {
                    let val = e
                        .attributes()
                        .flatten()
                        .find(|a| a.key.local_name().as_ref() == "val")
                        .map(|a| a.value.to_ascii_lowercase())
                        .unwrap_or_default();
                    heading = val.starts_with("heading") || val == "title";
                }
                _ => {}
            },
            Event::Text(t) if in_text => grow(&mut para, &t.xml10_content()),
            Event::GeneralRef(r) if in_text => grow(&mut para, &xml_ref(&r)),
            Event::End(e) => match e.local_name().as_ref() {
                "t" => in_text = false,
                "tabs" => in_tabs = false,
                "p" => {
                    let text = std::mem::take(&mut para);
                    if table_depth > 0 {
                        if !cell.is_empty() && !text.is_empty() {
                            cell.push(' ');
                        }
                        grow(&mut cell, text.trim());
                    } else {
                        full = !push_para(&mut doc, &mut chars, text, heading);
                    }
                    heading = false;
                }
                "txbxContent" => {
                    if let Some((p, h)) = outer.pop() {
                        (para, heading) = (p, h);
                    }
                }
                "tc" if table_depth > 0 => cells.push(std::mem::take(&mut cell)),
                "tr" if table_depth > 0 => {
                    let row = std::mem::take(&mut cells);
                    if row.iter().any(|c| !c.is_empty()) {
                        full = !push_para(&mut doc, &mut chars, row.join("  |  "), false);
                    }
                }
                "tbl" => {
                    table_depth = table_depth.saturating_sub(1);
                    if let Some((c, x)) = tables.pop() {
                        (cells, cell) = (c, x);
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
        if full {
            break;
        }
    }
    tidy(&mut doc);
    Ok(doc)
}

/// The slide parts in the presentation's own order (`p:sldIdLst` → relationships), else by
/// their numbers.
fn slide_parts(zip: &mut zip::ZipArchive<std::fs::File>) -> Vec<String> {
    let mut by_number: Vec<(u32, String)> = zip
        .file_names()
        .filter_map(|n| {
            let n = n.ok()?;
            let num = n
                .strip_prefix("ppt/slides/slide")?
                .strip_suffix(".xml")?
                .parse()
                .ok()?;
            Some((num, n.to_string()))
        })
        .collect();
    by_number.sort();
    let attrs = |xml: &[u8], tag: &str, keys: &[&str]| -> Vec<Vec<String>> {
        let mut out = Vec::new();
        let mut reader = quick_xml::Reader::from_reader(xml);
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e) | Event::Empty(e)) if e.local_name().as_ref() == tag => {
                    let all: Vec<(String, String)> = e
                        .attributes()
                        .flatten()
                        .map(|a| (a.key.local_name().as_ref().to_string(), a.value.to_string()))
                        .collect();
                    out.push(
                        keys.iter()
                            .map(|k| {
                                all.iter()
                                    .find(|(n, _)| n == k)
                                    .map(|(_, v)| v.clone())
                                    .unwrap_or_default()
                            })
                            .collect(),
                    );
                }
                Ok(Event::Eof) | Err(_) => break,
                _ => {}
            }
            buf.clear();
        }
        out
    };
    let ordered = (|| {
        let pres = zip_part(zip, "ppt/presentation.xml").ok()?;
        let rels = zip_part(zip, "ppt/_rels/presentation.xml.rels").ok()?;
        // `r:id` is read by its local name "id"; sldId also has a plain `id`: take the last.
        let rids: Vec<String> = {
            let mut reader = quick_xml::Reader::from_reader(pres.as_slice());
            let mut buf = Vec::new();
            let mut v = Vec::new();
            loop {
                match reader.read_event_into(&mut buf) {
                    Ok(Event::Start(e) | Event::Empty(e)) if e.local_name().as_ref() == "sldId" => {
                        if let Some(a) = e
                            .attributes()
                            .flatten()
                            .find(|a| a.key.as_ref().ends_with(":id"))
                        {
                            v.push(a.value.to_string());
                        }
                    }
                    Ok(Event::Eof) | Err(_) => break,
                    _ => {}
                }
                buf.clear();
            }
            v
        };
        let targets = attrs(&rels, "Relationship", &["Id", "Target"]);
        let parts: Vec<String> = rids
            .iter()
            .filter_map(|r| {
                let t = &targets.iter().find(|t| &t[0] == r)?[1];
                let t = t.trim_start_matches('/');
                Some(if t.starts_with("ppt/") {
                    t.to_string()
                } else {
                    format!("ppt/{t}")
                })
            })
            .filter(|p| by_number.iter().any(|(_, n)| n == p))
            .collect();
        (!parts.is_empty()).then_some(parts)
    })();
    ordered.unwrap_or_else(|| by_number.into_iter().map(|(_, n)| n).collect())
}

/// A presentation's text, slide by slide.
fn pptx(path: &Path) -> Result<Doc, String> {
    let mut zip = open_zip(path)?;
    let slides = slide_parts(&mut zip);
    let mut doc = Doc::default();
    let mut chars = 0usize;
    'slides: for (i, name) in slides.iter().enumerate() {
        if !push_para(&mut doc, &mut chars, format!("Slide {}", i + 1), true) {
            break;
        }
        let xml = zip_part(&mut zip, name)?;
        let mut reader = quick_xml::Reader::from_reader(xml.as_slice());
        let mut buf = Vec::new();
        let (mut para, mut in_text) = (String::new(), false);
        loop {
            match reader
                .read_event_into(&mut buf)
                .map_err(|e| e.to_string())?
            {
                Event::Start(e) if e.local_name().as_ref() == "t" => in_text = true,
                Event::Empty(e) if e.local_name().as_ref() == "br" => para.push('\n'),
                Event::Text(t) if in_text => grow(&mut para, &t.xml10_content()),
                Event::GeneralRef(r) if in_text => grow(&mut para, &xml_ref(&r)),
                Event::End(e) => match e.local_name().as_ref() {
                    "t" => in_text = false,
                    "p" => {
                        let text = std::mem::take(&mut para);
                        if !text.trim().is_empty() && !push_para(&mut doc, &mut chars, text, false)
                        {
                            break 'slides;
                        }
                    }
                    _ => {}
                },
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
        }
    }
    if slides.is_empty() {
        doc.paras.push(Para {
            text: "This presentation has no slides.".into(),
            heading: false,
        });
    }
    Ok(doc)
}

/// Runs of blank paragraphs become one; none at the ends.
fn tidy(doc: &mut Doc) {
    let mut out: Vec<Para> = Vec::with_capacity(doc.paras.len());
    for p in doc.paras.drain(..) {
        let blank = p.text.trim().is_empty();
        if blank && out.last().is_none_or(|l| l.text.trim().is_empty()) {
            continue;
        }
        out.push(p);
    }
    while out.last().is_some_and(|p| p.text.trim().is_empty()) {
        out.pop();
    }
    doc.paras = out;
}

// ---- RTF ----

/// Groups whose text is not part of the document.
const RTF_SKIP: [&str; 20] = [
    "fonttbl",
    "colortbl",
    "stylesheet",
    "info",
    "pict",
    "header",
    "footer",
    "headerl",
    "headerr",
    "footerl",
    "footerr",
    "object",
    "themedata",
    "colorschememapping",
    "latentstyles",
    "datastore",
    "listtable",
    "listoverridetable",
    "rsidtbl",
    "fldinst",
];

/// The text of an RTF document.
fn rtf_text(b: &[u8]) -> String {
    let mut out = String::new();
    // Per group: skipped, and the characters to drop after a \u (\ucN).
    let mut stack: Vec<(bool, usize)> = Vec::new();
    let (mut skip, mut uc) = (false, 1usize);
    let mut drop_next = 0usize;
    // The first half of a character outside the 16-bit range (`\u-10179?\u-8704?`).
    let mut high: Option<u32> = None;
    let mut i = 0;
    let emit = |out: &mut String, skip: bool, drop_next: &mut usize, c: char| {
        if *drop_next > 0 {
            *drop_next -= 1;
        } else if !skip {
            out.push(c);
        }
    };
    while i < b.len() {
        if out.len() > MAX_CHARS * 2 {
            break;
        }
        match b[i] {
            b'{' => {
                stack.push((skip, uc));
                i += 1;
            }
            b'}' => {
                (skip, uc) = stack.pop().unwrap_or((false, 1));
                i += 1;
            }
            b'\\' => {
                i += 1;
                let Some(&c) = b.get(i) else { break };
                if c.is_ascii_alphabetic() {
                    let start = i;
                    while i < b.len() && b[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    let word = std::str::from_utf8(&b[start..i]).unwrap_or_default();
                    let num_start = i;
                    if i < b.len() && b[i] == b'-' {
                        i += 1;
                    }
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                    let param: Option<i64> = std::str::from_utf8(&b[num_start..i])
                        .ok()
                        .and_then(|s| s.parse().ok());
                    if i < b.len() && b[i] == b' ' {
                        i += 1;
                    }
                    match word {
                        "par" | "line" | "row" | "sect" | "page" => {
                            emit(&mut out, skip, &mut 0, '\n')
                        }
                        "tab" => emit(&mut out, skip, &mut 0, '\t'),
                        "cell" => {
                            if !skip {
                                out.push_str("  |  ");
                            }
                        }
                        "uc" => uc = param.unwrap_or(1).max(0) as usize,
                        "u" => {
                            let n = param.unwrap_or(0);
                            let n = (if n < 0 { n + 65536 } else { n }) as u32;
                            let ch = match (high.take(), n) {
                                (_, 0xD800..=0xDBFF) => {
                                    high = Some(n);
                                    None
                                }
                                (Some(h), 0xDC00..=0xDFFF) => {
                                    char::from_u32(0x10000 + ((h - 0xD800) << 10) + (n - 0xDC00))
                                }
                                (_, n) => char::from_u32(n),
                            };
                            if let Some(ch) = ch {
                                emit(&mut out, skip, &mut 0, ch);
                            }
                            drop_next = uc;
                        }
                        "lquote" => emit(&mut out, skip, &mut 0, '\u{2018}'),
                        "rquote" => emit(&mut out, skip, &mut 0, '\u{2019}'),
                        "ldblquote" => emit(&mut out, skip, &mut 0, '\u{201C}'),
                        "rdblquote" => emit(&mut out, skip, &mut 0, '\u{201D}'),
                        "emdash" => emit(&mut out, skip, &mut 0, '\u{2014}'),
                        "endash" => emit(&mut out, skip, &mut 0, '\u{2013}'),
                        "bullet" => emit(&mut out, skip, &mut 0, '\u{2022}'),
                        "enspace" | "emspace" | "qmspace" => emit(&mut out, skip, &mut 0, ' '),
                        // Binary data: its bytes are not text and may hold braces.
                        "bin" => i = i.saturating_add(param.unwrap_or(0).max(0) as usize),
                        w if RTF_SKIP.contains(&w) => skip = true,
                        _ => {}
                    }
                } else {
                    i += 1;
                    match c {
                        b'*' => skip = true,
                        b'\'' => {
                            let hex = b.get(i..i + 2).and_then(|h| std::str::from_utf8(h).ok());
                            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                                emit(&mut out, skip, &mut drop_next, cp1252(v));
                            }
                            i += 2;
                        }
                        b'~' => emit(&mut out, skip, &mut drop_next, ' '),
                        b'_' => emit(&mut out, skip, &mut drop_next, '-'),
                        b'\n' | b'\r' => emit(&mut out, skip, &mut 0, '\n'),
                        b'\\' | b'{' | b'}' => emit(&mut out, skip, &mut drop_next, c as char),
                        _ => {}
                    }
                }
            }
            b'\r' | b'\n' => i += 1,
            c => {
                emit(&mut out, skip, &mut drop_next, cp1252(c));
                i += 1;
            }
        }
    }
    out
}

// ---- HTML (mail bodies, .htm files) ----

fn html_text(html: &str) -> String {
    use std::sync::LazyLock;
    // One pattern per element: the regex crate has no back-references (QA: `<head>` must end
    // at `</head>`, not at the first `</style>`).
    static HIDDEN: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r"(?is)<head\b.*?</head\s*>|<script\b.*?</script\s*>|<style\b.*?</style\s*>|<!--.*?-->",
        )
        .expect("valid")
    });
    static BREAK: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?i)<br\s*/?>|</(p|div|tr|li|h[1-6]|table)\s*>").expect("valid")
    });
    static CELL: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"(?i)</t[dh]\s*>").expect("valid"));
    static TAG: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"<[^>]*>").expect("valid"));
    static NUM: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"&#(x[0-9a-fA-F]+|[0-9]+);").expect("valid"));
    static BLANKS: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\n[ \t]*(\n[ \t]*){2,}").expect("valid"));
    let s = HIDDEN.replace_all(html, "");
    let s = s.replace(['\r', '\n'], " ");
    let s = BREAK.replace_all(&s, "\n");
    let s = CELL.replace_all(&s, "  ");
    let s = TAG.replace_all(&s, "");
    let s = NUM.replace_all(&s, |c: &regex::Captures| {
        let n = &c[1];
        let v = match n.strip_prefix('x') {
            Some(h) => u32::from_str_radix(h, 16).ok(),
            None => n.parse().ok(),
        };
        v.and_then(char::from_u32)
            .map(String::from)
            .unwrap_or_default()
    });
    let s = s
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&rsquo;", "\u{2019}")
        .replace("&lsquo;", "\u{2018}")
        .replace("&rdquo;", "\u{201D}")
        .replace("&ldquo;", "\u{201C}")
        .replace("&mdash;", "\u{2014}")
        .replace("&ndash;", "\u{2013}")
        .replace("&hellip;", "\u{2026}")
        .replace("&bull;", "\u{2022}")
        .replace("&copy;", "\u{00A9}")
        .replace("&amp;", "&");
    let lines: Vec<&str> = s.lines().map(str::trim).collect();
    BLANKS
        .replace_all(&lines.join("\n"), "\n\n")
        .trim()
        .to_string()
}

// ---- Mail ----

fn eml(b: &[u8]) -> Result<Doc, String> {
    use mailparse::MailHeaderMap;
    let mail = mailparse::parse_mail(b).map_err(|e| e.to_string())?;
    let mut header = Vec::new();
    for key in ["From", "To", "Cc", "Date", "Subject"] {
        if let Some(v) = mail.headers.get_first_value(key) {
            header.push((key.to_string(), v));
        }
    }
    let (mut plain, mut html, mut files) = (None, None, Vec::new());
    walk_mail(&mail, &mut plain, &mut html, &mut files);
    if !files.is_empty() {
        header.push(("Attachments".into(), files.join(", ")));
    }
    let body = plain
        .or_else(|| html.map(|h| html_text(&h)))
        .unwrap_or_default();
    let mut doc = plain_doc(&body);
    doc.header = header;
    Ok(doc)
}

fn walk_mail(
    part: &mailparse::ParsedMail,
    plain: &mut Option<String>,
    html: &mut Option<String>,
    files: &mut Vec<String>,
) {
    let disposition = part.get_content_disposition();
    let name = disposition
        .params
        .get("filename")
        .or_else(|| part.ctype.params.get("name"))
        .cloned();
    let attached = disposition.disposition == mailparse::DispositionType::Attachment;
    if attached || (name.is_some() && !part.ctype.mimetype.starts_with("text/")) {
        files.push(name.unwrap_or_else(|| "(unnamed)".into()));
        return;
    }
    if part.subparts.is_empty() {
        match part.ctype.mimetype.as_str() {
            "text/plain" if plain.is_none() => *plain = part.get_body().ok(),
            "text/html" if html.is_none() => *html = part.get_body().ok(),
            _ => {}
        }
    }
    for sub in &part.subparts {
        walk_mail(sub, plain, html, files);
    }
}

/// An Outlook `.msg`: a compound file whose MAPI properties are streams named by property id.
fn msg(path: &Path) -> Result<Doc, String> {
    let mut c = cfb::open(path).map_err(|_| "not a valid Outlook message".to_string())?;
    let mut header = Vec::new();
    let from_name = msg_prop(&mut c, "", 0x0C1A);
    let from_mail = msg_prop(&mut c, "", 0x5D01)
        .or_else(|| msg_prop(&mut c, "", 0x0C1F))
        .filter(|m| m.contains('@'));
    let from = match (from_name, from_mail) {
        (Some(n), Some(m)) if n != m => Some(format!("{n} <{m}>")),
        (Some(n), _) => Some(n),
        (None, m) => m,
    };
    if let Some(f) = from {
        header.push(("From".into(), f));
    }
    if let Some(t) = msg_prop(&mut c, "", 0x0E04) {
        header.push(("To".into(), t));
    }
    if let Some(t) = msg_prop(&mut c, "", 0x0E03) {
        header.push(("Cc".into(), t));
    }
    let body = msg_prop(&mut c, "", 0x1000);
    let html = if body.is_none() {
        let mut b = Vec::new();
        if let Ok(mut s) = c.open_stream("/__substg1.0_10130102") {
            let _ = s.by_ref().take(MAX_FILE).read_to_end(&mut b);
        }
        (!b.is_empty()).then(|| html_text(&decode_text(&b)))
    } else {
        None
    };
    let subject = msg_prop(&mut c, "", 0x0037);
    if let Some(d) = msg_date(&mut c) {
        header.push(("Date".into(), d));
    }
    if let Some(s) = subject {
        header.push(("Subject".into(), s));
    }
    // Attachments: storages named __attach_version1.0_#n, their long (else short) file name.
    let storages: Vec<String> = c
        .read_root_storage()
        .filter(|e| e.is_storage() && e.name().starts_with("__attach_version1.0_#"))
        .map(|e| e.path().to_string_lossy().into_owned())
        .collect();
    let mut files = Vec::new();
    for s in storages {
        let name = msg_prop(&mut c, &s, 0x3707)
            .or_else(|| msg_prop(&mut c, &s, 0x3704))
            .or_else(|| msg_prop(&mut c, &s, 0x3001))
            .unwrap_or_else(|| "(unnamed)".into());
        files.push(name);
    }
    if !files.is_empty() {
        header.push(("Attachments".into(), files.join(", ")));
    }
    let mut doc = plain_doc(&body.or(html).unwrap_or_default());
    doc.header = header;
    Ok(doc)
}

/// A MAPI string property of `storage` ("" for the message itself).
fn msg_prop(c: &mut cfb::CompoundFile<std::fs::File>, storage: &str, id: u16) -> Option<String> {
    for (ty, wide) in [("001F", true), ("001E", false)] {
        let name = format!("{storage}/__substg1.0_{id:04X}{ty}");
        let Ok(mut s) = c.open_stream(&name) else {
            continue;
        };
        let mut b = Vec::new();
        s.by_ref().take(MAX_FILE).read_to_end(&mut b).ok()?;
        let text = if wide {
            let units: Vec<u16> = b
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .collect();
            String::from_utf16_lossy(&units)
        } else {
            b.iter().map(|&c| cp1252(c)).collect()
        };
        let text = text.trim_end_matches('\0').trim().to_string();
        return (!text.is_empty()).then_some(text);
    }
    None
}

/// When the message was sent (else received), from the fixed-size property stream.
fn msg_date(c: &mut cfb::CompoundFile<std::fs::File>) -> Option<String> {
    let mut b = Vec::new();
    c.open_stream("/__properties_version1.0")
        .ok()?
        .take(1024 * 1024)
        .read_to_end(&mut b)
        .ok()?;
    // A top-level message's stream has a 32-byte header, then 16-byte entries:
    // tag (type, id), flags, 8-byte value.
    let mut found = [None, None];
    for e in b.get(32..)?.as_chunks::<16>().0 {
        let ty = u16::from_le_bytes([e[0], e[1]]);
        let id = u16::from_le_bytes([e[2], e[3]]);
        if ty != 0x0040 {
            continue;
        }
        let ft = u64::from_le_bytes(e[8..16].try_into().ok()?);
        match id {
            0x0039 => found[0] = Some(ft),
            0x0E06 => found[1] = Some(ft),
            _ => {}
        }
    }
    let ft = found[0].or(found[1]).filter(|&f| f > 0)?;
    let secs = (ft / 10_000_000) as i64 - 11_644_473_600;
    let t = chrono::DateTime::from_timestamp(secs, 0)?.with_timezone(&chrono::Local);
    Some(t.format("%m/%d/%Y %I:%M %p").to_string())
}

// ---- Pictures ----

fn picture(path: &Path) -> Result<Picture, String> {
    let img = crate::imagepdf::decode_upright(path)?;
    let img = if img.width() > PICTURE_SIDE || img.height() > PICTURE_SIDE {
        img.thumbnail(PICTURE_SIDE, PICTURE_SIDE)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let pages = tiff_pages(path);
    Ok(Picture {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
        pages,
    })
}

/// Pages in a TIFF (1 for any other picture, or a TIFF that cannot be counted).
fn tiff_pages(path: &Path) -> usize {
    let tiff = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("tif") || e.eq_ignore_ascii_case("tiff"));
    if !tiff {
        return 1;
    }
    let Ok(f) = std::fs::File::open(path) else {
        return 1;
    };
    let Ok(mut d) = tiff::decoder::Decoder::new(std::io::BufReader::new(f)) else {
        return 1;
    };
    let mut n = 1;
    while d.more_images() && n < 10_000 {
        if d.next_image().is_err() {
            break;
        }
        n += 1;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn text_of(c: &Content) -> Vec<String> {
        match c {
            Content::Text(d) => d.paras.iter().map(|p| p.text.clone()).collect(),
            other => panic!("expected text, got {other:?}"),
        }
    }

    fn write_zip(path: &Path, parts: &[(&str, &str)]) {
        let f = std::fs::File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        for (name, body) in parts {
            z.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            z.write_all(body.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }

    #[test]
    fn an_excel_workbook_shows_every_sheet_with_dates_and_numbers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Trial balance.xlsx");
        let mut book = rust_xlsxwriter::Workbook::new();
        let date = rust_xlsxwriter::Format::new().set_num_format("mm/dd/yyyy");
        let s = book.add_worksheet().set_name("TB").unwrap();
        s.write(0, 0, "Account").unwrap();
        s.write(0, 1, "Amount").unwrap();
        s.write(1, 0, "Cash").unwrap();
        s.write(1, 1, 1234.5).unwrap();
        s.write(2, 0, "Rent & lease").unwrap();
        s.write(2, 1, 900).unwrap();
        let d = rust_xlsxwriter::ExcelDateTime::from_ymd(2025, 12, 31).unwrap();
        s.write_with_format(3, 1, &d, &date).unwrap();
        let s2 = book.add_worksheet().set_name("Notes").unwrap();
        s2.write(4, 2, "late entry").unwrap();
        book.save(&path).unwrap();

        let Content::Sheets(sheets) = read(&path) else {
            panic!("expected sheets")
        };
        assert_eq!(sheets.len(), 2);
        assert_eq!(sheets[0].name, "TB");
        assert_eq!(sheets[0].rows[0], vec!["Account", "Amount"]);
        assert_eq!(sheets[0].rows[1], vec!["Cash", "1234.5"]);
        assert_eq!(sheets[0].rows[2], vec!["Rent & lease", "900"]);
        assert_eq!(sheets[0].rows[3], vec!["", "12/31/2025"]);
        assert!(sheets[0].cut.is_none());
        // Data starting at C5 is labelled from C5.
        assert_eq!(sheets[1].origin, (4, 2));
        assert_eq!(sheets[1].rows[0], vec!["late entry"]);
    }

    #[test]
    fn a_long_csv_is_cut_with_a_note() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bank.csv");
        let mut s = String::from("Date,Payee,Amount\n");
        for i in 0..2500 {
            s.push_str(&format!("01/02/2025,\"Smith, John\",{i}\n"));
        }
        std::fs::write(&path, s).unwrap();
        let Content::Sheets(sheets) = read(&path) else {
            panic!("expected a sheet")
        };
        assert_eq!(sheets[0].rows.len(), MAX_ROWS);
        assert_eq!(sheets[0].rows[1], vec!["01/02/2025", "Smith, John", "0"]);
        assert_eq!(
            sheets[0].cut.as_deref(),
            Some("Only the first 2,000 of 2,501 rows are shown.")
        );
    }

    #[test]
    fn a_word_document_gives_headings_paragraphs_and_table_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Engagement letter.docx");
        let body = r#"<?xml version="1.0"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Engagement</w:t></w:r></w:p>
<w:p><w:r><w:t xml:space="preserve">Fees &amp; </w:t></w:r><w:r><w:t>terms</w:t></w:r></w:p>
<w:p/>
<w:p/>
<w:tbl><w:tr><w:tc><w:p><w:r><w:t>Service</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Fee</w:t></w:r></w:p></w:tc></w:tr>
<w:tr><w:tc><w:p><w:r><w:t>1040</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>$450</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
<w:p><w:r><w:t>Signed</w:t></w:r></w:p>
</w:body></w:document>"#;
        write_zip(&path, &[("word/document.xml", body)]);
        let c = read(&path);
        assert_eq!(
            text_of(&c),
            vec![
                "Engagement",
                "Fees & terms",
                "",
                "Service  |  Fee",
                "1040  |  $450",
                "Signed"
            ]
        );
        let Content::Text(d) = c else { unreachable!() };
        assert!(d.paras[0].heading && !d.paras[1].heading);
    }

    #[test]
    fn a_presentation_gives_its_text_slide_by_slide() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deck.pptx");
        let slide = |t: &str| {
            format!(
                r#"<p:sld xmlns:p="p" xmlns:a="a"><a:p><a:r><a:t>{t}</a:t></a:r></a:p></p:sld>"#
            )
        };
        let (s1, s2, s10) = (slide("Tax plan"), slide("Next steps"), slide("Questions"));
        write_zip(
            &path,
            &[
                ("ppt/slides/slide10.xml", s10.as_str()),
                ("ppt/slides/slide2.xml", s2.as_str()),
                ("ppt/slides/slide1.xml", s1.as_str()),
            ],
        );
        assert_eq!(
            text_of(&read(&path)),
            vec![
                "Slide 1",
                "Tax plan",
                "Slide 2",
                "Next steps",
                "Slide 3",
                "Questions"
            ]
        );
    }

    #[test]
    fn rtf_and_plain_text_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let rtf = dir.path().join("memo.rtf");
        std::fs::write(
            &rtf,
            r"{\rtf1\ansi{\fonttbl{\f0 Arial;}}{\*\generator Word;}\f0 Dear client,\par Caf\'e9 "
                .to_string()
                + "\\"
                + r"u8212? done\par}",
        )
        .unwrap();
        assert_eq!(text_of(&read(&rtf)), vec!["Dear client,", "Café — done"]);

        let txt = dir.path().join("notes.txt");
        // Windows-1252, not UTF-8.
        std::fs::write(&txt, b"Smith \x96 1099\r\n\r\n").unwrap();
        assert_eq!(text_of(&read(&txt)), vec!["Smith – 1099"]);
    }

    #[test]
    fn a_mail_shows_its_headers_text_and_attachment_names() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("w2.eml");
        std::fs::write(
            &path,
            "From: Pat <pat@example.com>\r\nTo: office@example.com\r\nSubject: My W-2\r\n\
Date: Mon, 3 Feb 2025 09:00:00 -0600\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\n\
Content-Type: text/html; charset=utf-8\r\n\r\n<p>Here it is&nbsp;&amp; thanks</p>\r\n--b\r\n\
Content-Type: application/pdf; name=\"w2.pdf\"\r\nContent-Disposition: attachment; filename=\"w2.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\r\nJVBERg==\r\n--b--\r\n",
        )
        .unwrap();
        let Content::Text(d) = read(&path) else {
            panic!("expected text")
        };
        let h: Vec<(&str, &str)> = d
            .header
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert!(h.contains(&("Subject", "My W-2")));
        assert!(h.contains(&("From", "Pat <pat@example.com>")));
        assert!(h.contains(&("Attachments", "w2.pdf")));
        assert_eq!(d.paras[0].text, "Here it is & thanks");
    }

    #[test]
    fn an_outlook_message_shows_its_headers_text_and_attachment_names() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("1099.msg");
        let wide = |s: &str| -> Vec<u8> { s.encode_utf16().flat_map(u16::to_le_bytes).collect() };
        {
            let mut c = cfb::create(&path).unwrap();
            c.create_storage("/__attach_version1.0_#00000000").unwrap();
            let mut put = |name: &str, data: &[u8]| {
                c.create_stream(name).unwrap().write_all(data).unwrap();
            };
            put("/__substg1.0_0037001F", &wide("1099 for 2025"));
            put("/__substg1.0_0C1A001F", &wide("Pat Smith"));
            put("/__substg1.0_5D01001F", &wide("pat@example.com"));
            put("/__substg1.0_0E04001F", &wide("Office"));
            put("/__substg1.0_1000001F", &wide("See attached.\r\nPat"));
            // Sent 2025-02-03 15:00 UTC.
            let ft = (1_738_594_800u64 + 11_644_473_600) * 10_000_000;
            let mut props = vec![0u8; 32];
            props.extend_from_slice(&0x0040u16.to_le_bytes());
            props.extend_from_slice(&0x0039u16.to_le_bytes());
            props.extend_from_slice(&[0; 4]);
            props.extend_from_slice(&ft.to_le_bytes());
            put("/__properties_version1.0", &props);
            put(
                "/__attach_version1.0_#00000000/__substg1.0_3707001F",
                &wide("1099-NEC.pdf"),
            );
        }
        let Content::Text(d) = read(&path) else {
            panic!("expected text")
        };
        let h: Vec<(&str, &str)> = d
            .header
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert!(
            h.contains(&("From", "Pat Smith <pat@example.com>")),
            "{h:?}"
        );
        assert!(h.contains(&("To", "Office")));
        assert!(h.contains(&("Subject", "1099 for 2025")));
        assert!(h.contains(&("Attachments", "1099-NEC.pdf")));
        assert!(
            h.iter().any(|(k, v)| *k == "Date" && v.contains("2025")),
            "{h:?}"
        );
        assert_eq!(
            d.paras.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(),
            vec!["See attached.", "Pat"]
        );
    }

    #[test]
    fn a_large_picture_is_shrunk_and_tiff_pages_are_counted() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("photo.png");
        image::RgbImage::from_pixel(3200, 800, image::Rgb([200, 10, 10]))
            .save(&png)
            .unwrap();
        let Content::Picture(p) = read(&png) else {
            panic!("expected a picture")
        };
        assert_eq!((p.width, p.height, p.pages), (1600, 400, 1));
        assert_eq!(p.rgba.len(), 1600 * 400 * 4);

        let tif = dir.path().join("scan.tif");
        {
            let f = std::fs::File::create(&tif).unwrap();
            let mut enc = tiff::encoder::TiffEncoder::new(f).unwrap();
            for _ in 0..3 {
                enc.write_image::<tiff::encoder::colortype::Gray8>(10, 10, &[255; 100])
                    .unwrap();
            }
        }
        let Content::Picture(p) = read(&tif) else {
            panic!("expected a picture")
        };
        assert_eq!(p.pages, 3);
    }

    #[test]
    fn broken_old_and_unknown_files_give_a_note() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("broken.xlsx");
        std::fs::write(&bad, b"not a zip").unwrap();
        assert!(matches!(read(&bad), Content::Nothing(why) if why.starts_with("Cannot read")));
        let doc = dir.path().join("old.doc");
        std::fs::write(&doc, b"x").unwrap();
        assert!(matches!(read(&doc), Content::Nothing(why) if why.contains(".doc")));
        assert!(!viewable(Path::new("setup.exe")));
        assert!(viewable(Path::new("Book1.XLSX")));
    }

    #[test]
    fn numbers_read_like_excel_general() {
        assert_eq!(number(0.1 + 0.2), "0.3");
        assert_eq!(number(-42.0), "-42");
        assert_eq!(number(1234567.891), "1234567.891");
        assert_eq!(thousands(1234567), "1,234,567");
    }

    /// QA: one stray value far down and right does not build a huge grid (calamine's whole-sheet
    /// block ran out of memory on a 5 KB file).
    #[test]
    fn a_stray_far_cell_does_not_build_a_huge_grid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stray.xlsx");
        let mut book = rust_xlsxwriter::Workbook::new();
        let s = book.add_worksheet();
        s.write(0, 0, "Top").unwrap();
        s.write(1_048_575, 16_383, "Far").unwrap();
        book.save(&path).unwrap();
        let start = std::time::Instant::now();
        let Content::Sheets(sheets) = read(&path) else {
            panic!("expected sheets")
        };
        assert!(start.elapsed() < std::time::Duration::from_secs(10));
        assert_eq!(sheets[0].rows.len(), MAX_ROWS);
        assert_eq!(sheets[0].rows[0], vec!["Top"]);
        assert_eq!(
            sheets[0].cut.as_deref(),
            Some(
                "Only the first 2,000 of 1,048,576 rows and the first 60 of 16384 columns are shown."
            )
        );
    }

    /// QA: tab stops are not tabs; a text box once (not Word's fallback copy too), apart from
    /// its paragraph; nested tables keep their rows; an empty `w:t` does not let field codes in.
    #[test]
    fn word_tab_stops_text_boxes_nested_tables_and_field_codes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("letter.docx");
        let body = r#"<?xml version="1.0"?>
<w:document xmlns:w="w" xmlns:mc="mc" xmlns:wps="wps" xmlns:v="v"><w:body>
<w:p><w:pPr><w:tabs><w:tab w:val="left" w:pos="720"/></w:tabs></w:pPr><w:r><w:t>Dear client</w:t></w:r></w:p>
<w:p><w:r><w:t>Before </w:t></w:r><w:r><mc:AlternateContent><mc:Choice><wps:txbx><w:txbxContent><w:p><w:r><w:t>Boxed</w:t></w:r></w:p></w:txbxContent></wps:txbx></mc:Choice><mc:Fallback><v:textbox><w:txbxContent><w:p><w:r><w:t>Boxed</w:t></w:r></w:p></w:txbxContent></v:textbox></mc:Fallback></mc:AlternateContent></w:r><w:r><w:t>after</w:t></w:r></w:p>
<w:tbl><w:tr><w:tc><w:p><w:r><w:t>A</w:t></w:r></w:p></w:tc><w:tc><w:tbl><w:tr><w:tc><w:p><w:r><w:t>X</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Y</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:tc><w:tc><w:p><w:r><w:t>C</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
<w:p><w:r><w:t/></w:r><w:r><w:instrText>PAGE \* MERGEFORMAT</w:instrText></w:r><w:r><w:t>Page 1</w:t></w:r></w:p>
</w:body></w:document>"#;
        write_zip(&path, &[("word/document.xml", body)]);
        assert_eq!(
            text_of(&read(&path)),
            vec![
                "Dear client",
                "Boxed",
                "Before after",
                "X  |  Y",
                "A  |    |  C",
                "Page 1"
            ]
        );
    }

    /// QA: a single huge line is cut; RTF quotes, dashes and characters beyond 16 bits; HTML's
    /// head is left out up to `</head>`.
    #[test]
    fn long_lines_rtf_symbols_and_html_head() {
        let dir = tempfile::tempdir().unwrap();
        let one = dir.path().join("bank.qbo");
        std::fs::write(&one, "x".repeat(MAX_CHARS * 3)).unwrap();
        let Content::Text(d) = read(&one) else {
            panic!("expected text")
        };
        assert!(d.cut);
        assert!(d.paras[0].text.chars().count() <= MAX_CHARS + 1);

        let rtf = dir.path().join("memo.rtf");
        std::fs::write(
            &rtf,
            r"{\rtf1\ansi\uc1 \ldblquote Hi\rdblquote \emdash  ok \u-10179?\u-8704? end\par}",
        )
        .unwrap();
        assert_eq!(
            text_of(&read(&rtf)),
            vec!["\u{201C}Hi\u{201D}\u{2014} ok \u{1F600} end"]
        );

        let html = dir.path().join("page.htm");
        std::fs::write(
            &html,
            "<html><head><style>p{}</style><title>Hidden title</title></head><body><p>Shown &rsquo;</p></body></html>",
        )
        .unwrap();
        assert_eq!(text_of(&read(&html)), vec!["Shown \u{2019}"]);
    }

    /// QA: a picture wider than the limit is refused before its pixels are read.
    #[test]
    fn a_picture_too_large_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("wide.png");
        image::GrayImage::new(20_000, 1).save(&png).unwrap();
        assert!(matches!(read(&png), Content::Nothing(why) if why.contains("too large")));
    }

    /// QA: slides in the presentation's own order, not their part numbers.
    #[test]
    fn slides_follow_the_presentations_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deck.pptx");
        let slide = |t: &str| {
            format!(
                r#"<p:sld xmlns:p="p" xmlns:a="a"><a:p><a:r><a:t>{t}</a:t></a:r></a:p></p:sld>"#
            )
        };
        let (s1, s2) = (slide("Second"), slide("First"));
        let pres = r#"<p:presentation xmlns:p="p" xmlns:r="r"><p:sldIdLst><p:sldId id="256" r:id="rId3"/><p:sldId id="257" r:id="rId2"/></p:sldIdLst></p:presentation>"#;
        let rels = r#"<Relationships><Relationship Id="rId2" Target="slides/slide1.xml"/><Relationship Id="rId3" Target="slides/slide2.xml"/></Relationships>"#;
        write_zip(
            &path,
            &[
                ("ppt/presentation.xml", pres),
                ("ppt/_rels/presentation.xml.rels", rels),
                ("ppt/slides/slide1.xml", s1.as_str()),
                ("ppt/slides/slide2.xml", s2.as_str()),
            ],
        );
        assert_eq!(
            text_of(&read(&path)),
            vec!["Slide 1", "First", "Slide 2", "Second"]
        );
    }
}
