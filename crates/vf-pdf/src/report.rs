//! A plain table as a PDF (P05 audit export, later the Safeguards appendix): landscape
//! Letter, Courier, a title block, a header row repeated on each page, page numbers. Cells
//! longer than their column are cut with `...` (the CSV export keeps everything).

use pdf_writer::{Content, Name, Pdf, Rect, Ref, Str};

use crate::layer::win_ansi;

const W: f32 = 792.0;
const H: f32 = 612.0;
const MARGIN: f32 = 36.0;
const SIZE: f32 = 7.0;
const LEAD: f32 = 9.5;
/// Courier's advance is 0.6 em.
const CHAR: f32 = SIZE * 0.6;

/// A column: its heading and width in characters.
pub struct Column<'a> {
    pub heading: &'a str,
    pub width: usize,
}

/// `cell` fitted to `width` characters.
fn fit(cell: &str, width: usize) -> String {
    let one_line: String = cell
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let n = one_line.chars().count();
    if n <= width {
        format!("{one_line:<width$}")
    } else {
        let mut s: String = one_line.chars().take(width.saturating_sub(3)).collect();
        s.push_str("...");
        s
    }
}

fn line(cols: &[Column], cells: &[String]) -> String {
    cols.iter()
        .zip(cells)
        .map(|(c, v)| fit(v, c.width))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The table as PDF bytes. `title` and `subtitle` head the first page.
pub fn table(title: &str, subtitle: &str, cols: &[Column], rows: &[Vec<String>]) -> Vec<u8> {
    let per_first = ((H - 2.0 * MARGIN - 40.0) / LEAD) as usize - 2;
    let per_page = ((H - 2.0 * MARGIN) / LEAD) as usize - 3;
    let mut chunks: Vec<&[Vec<String>]> = Vec::new();
    let (first, mut rest) = rows.split_at(rows.len().min(per_first));
    chunks.push(first);
    while !rest.is_empty() {
        let (a, b) = rest.split_at(rest.len().min(per_page));
        chunks.push(a);
        rest = b;
    }
    let header: Vec<String> = cols.iter().map(|c| c.heading.to_string()).collect();
    let total = chunks.len();

    let mut pdf = Pdf::new();
    let (catalog, tree, font, bold) = (Ref::new(1), Ref::new(2), Ref::new(3), Ref::new(4));
    let page_ids: Vec<Ref> = (0..total).map(|i| Ref::new(5 + 2 * i as i32)).collect();
    pdf.catalog(catalog).pages(tree);
    pdf.pages(tree)
        .kids(page_ids.iter().copied())
        .count(total as i32);
    for (i, chunk) in chunks.iter().enumerate() {
        let mut c = Content::new();
        let text = |c: &mut Content, f: &[u8], size: f32, x: f32, y: f32, s: &str| {
            c.begin_text();
            c.set_font(Name(f), size);
            c.next_line(x, y);
            c.show(Str(&win_ansi(s)));
            c.end_text();
        };
        let mut y = H - MARGIN;
        if i == 0 {
            text(&mut c, b"F2", 14.0, MARGIN, y - 10.0, title);
            text(&mut c, b"F1", 8.0, MARGIN, y - 26.0, subtitle);
            y -= 40.0;
        }
        text(&mut c, b"F2", SIZE, MARGIN, y, &line(cols, &header));
        y -= LEAD * 1.5;
        for row in *chunk {
            text(&mut c, b"F1", SIZE, MARGIN, y, &line(cols, row));
            y -= LEAD;
        }
        let foot = format!("Page {} of {total}", i + 1);
        let fx = W - MARGIN - foot.len() as f32 * CHAR;
        text(&mut c, b"F1", SIZE, fx, MARGIN / 2.0, &foot);

        let (page, content) = (page_ids[i], Ref::new(6 + 2 * i as i32));
        let mut p = pdf.page(page);
        p.media_box(Rect::new(0.0, 0.0, W, H));
        p.parent(tree);
        p.contents(content);
        let mut res = p.resources();
        let mut fonts = res.fonts();
        fonts.pair(Name(b"F1"), font);
        fonts.pair(Name(b"F2"), bold);
        drop(fonts);
        drop(res);
        drop(p);
        pdf.stream(content, &c.finish());
    }
    pdf.type1_font(font)
        .base_font(Name(b"Courier"))
        .encoding_predefined(Name(b"WinAnsiEncoding"));
    pdf.type1_font(bold)
        .base_font(Name(b"Courier-Bold"))
        .encoding_predefined(Name(b"WinAnsiEncoding"));
    pdf.finish()
}

/// Characters that fit across a page.
pub const LINE_CHARS: usize = ((W - 2.0 * MARGIN) / CHAR) as usize;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_flow_over_pages_with_the_header_on_each() {
        let cols = [
            Column {
                heading: "seq",
                width: 5,
            },
            Column {
                heading: "detail",
                width: 20,
            },
        ];
        let rows: Vec<Vec<String>> = (1..=120)
            .map(|i| vec![i.to_string(), format!("row {i} {}", "x".repeat(i % 30))])
            .collect();
        let bytes = table("Audit trail", "All rows", &cols, &rows);
        let d = lopdf::Document::load_mem(&bytes).unwrap();
        let pages = d.get_pages();
        assert!(pages.len() >= 3, "{}", pages.len());
        let first = d.extract_text(&[1]).unwrap();
        assert!(first.contains("Audit trail") && first.contains("seq"));
        let last = d.extract_text(&[pages.len() as u32]).unwrap();
        assert!(last.contains("seq"), "header repeats");
        assert!(last.contains("row 120"));
        assert_eq!(fit("abcdefghij", 6), "abc...");
        assert_eq!(fit("ab\ncd", 6), "ab cd ");
        assert!(LINE_CHARS > 150);
    }
}
