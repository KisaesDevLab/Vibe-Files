//! A merged letter laid out on Letter pages (8.5 × 11 in, 1 in margins) in Helvetica, with the
//! firm's name as letterhead when asked, and each signature block's rectangles recorded for the
//! portal (§4): the signature image's box above the line and the date's box beside it, in PDF
//! points from the page's bottom-left corner.

use pdf_writer::{Content, Finish, Name, Pdf, Rect, Ref, Str};
use serde::{Deserialize, Serialize};

use crate::{Block, Fields, Span, Template, merge, role_words, spans};

const W: f32 = 612.0;
const H: f32 = 792.0;
const MARGIN: f32 = 72.0;
const SIZE: f32 = 10.5;
const LEAD: f32 = 14.0;
const HEAD_SIZE: f32 = 13.0;
/// Height a signature block needs: the image box, the line, the name under it.
const SIGN_H: f32 = 74.0;

/// Helvetica advance widths (1/1000 em) for ASCII 32..=126 (as vf-pdf's forms).
const HELV: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667,
    611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500,
    222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];

fn width(text: &str, size: f32, bold: bool) -> f32 {
    let units: u32 = text
        .chars()
        .map(|c| match c as u32 {
            n @ 32..=126 => u32::from(HELV[(n - 32) as usize]),
            _ => 556,
        })
        .sum();
    // Helvetica-Bold runs about 6% wider (an average: lines break a little early, never late).
    units as f32 * size / 1000.0 * if bold { 1.06 } else { 1.0 }
}

/// `s` in WinAnsi (the standard fonts' encoding); what it lacks becomes `?`.
fn win_ansi(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| match c {
            '\u{20}'..='\u{7e}' => c as u8,
            '\u{a0}'..='\u{ff}' => c as u32 as u8,
            '€' => 0x80,
            '…' => 0x85,
            '‘' => 0x91,
            '’' => 0x92,
            '“' => 0x93,
            '”' => 0x94,
            '•' => 0x95,
            '–' => 0x96,
            '—' => 0x97,
            _ => b'?',
        })
        .collect()
}

/// Where a signer signs: page (1-based), the signature box and the date box.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignBlock {
    pub role: String,
    pub page: u32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub date_x: f32,
    pub date_y: f32,
    pub date_w: f32,
    pub date_h: f32,
}

#[derive(Debug, Clone)]
pub struct Laid {
    pub pdf: Vec<u8>,
    pub pages: u32,
    pub blocks: Vec<SignBlock>,
}

/// One word of a line, in its style.
#[derive(Clone)]
struct Word {
    text: String,
    bold: bool,
    italic: bool,
    w: f32,
}

struct Pages {
    pages: Vec<Content>,
    y: f32,
    top: f32,
}

impl Pages {
    fn new(top: f32) -> Self {
        Self {
            pages: vec![Content::new()],
            y: top,
            top,
        }
    }
    fn need(&mut self, h: f32) {
        if self.y - h < MARGIN {
            self.pages.push(Content::new());
            self.y = H - MARGIN;
            let _ = self.top;
        }
    }
    fn page(&mut self) -> &mut Content {
        self.pages.last_mut().expect("a page")
    }
    fn n(&self) -> u32 {
        self.pages.len() as u32
    }
}

fn font(bold: bool, italic: bool) -> Name<'static> {
    match (bold, italic) {
        (false, false) => Name(b"F1"),
        (true, false) => Name(b"F2"),
        (false, true) => Name(b"F3"),
        (true, true) => Name(b"F4"),
    }
}

fn words_of(spans: &[Span], size: f32) -> Vec<Word> {
    let mut out = Vec::new();
    for s in spans {
        let mut cur = String::new();
        for c in s.text.chars() {
            cur.push(c);
            if c == ' ' {
                out.push(Word {
                    w: width(&cur, size, s.bold),
                    text: std::mem::take(&mut cur),
                    bold: s.bold,
                    italic: s.italic,
                });
            }
        }
        if !cur.is_empty() {
            out.push(Word {
                w: width(&cur, size, s.bold),
                text: cur,
                bold: s.bold,
                italic: s.italic,
            });
        }
    }
    out
}

/// Wraps words into lines no wider than `max`.
fn wrap(words: Vec<Word>, max: f32) -> Vec<Vec<Word>> {
    let mut lines: Vec<Vec<Word>> = Vec::new();
    let mut line: Vec<Word> = Vec::new();
    let mut w = 0.0;
    for word in words {
        let trimmed = width(word.text.trim_end(), SIZE, word.bold);
        if !line.is_empty() && w + trimmed > max {
            lines.push(std::mem::take(&mut line));
            w = 0.0;
        }
        w += word.w;
        line.push(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

fn draw_line(p: &mut Pages, x: f32, line: &[Word], size: f32) {
    let y = p.y;
    let page = p.page();
    let mut cx = x;
    for w in line {
        page.begin_text();
        page.set_font(font(w.bold, w.italic), size);
        page.next_line(cx, y);
        page.show(Str(&win_ansi(&w.text)));
        page.end_text();
        cx += w.w;
    }
}

fn paragraph(
    p: &mut Pages,
    text: &str,
    fields: &Fields,
    x: f32,
    max: f32,
    size: f32,
    bold_all: bool,
) {
    let mut s = spans(&merge(text, fields));
    if bold_all {
        for sp in &mut s {
            sp.bold = true;
        }
    }
    for line in wrap(words_of(&s, size), max) {
        p.need(LEAD);
        p.y -= LEAD;
        draw_line(p, x, &line, size);
    }
}

fn signer_name(role: &str, fields: &Fields) -> String {
    let get = |k: &str| fields.get(k).cloned().unwrap_or_default();
    match role {
        "taxpayer" => get("Taxpayer name"),
        "spouse" => get("Spouse name"),
        _ => {
            let (n, t) = (get("Signer name"), get("Signer title"));
            if t.is_empty() { n } else { format!("{n}, {t}") }
        }
    }
}

/// Lays `t` out for a client: merged, the blocks that apply to the return type and package, the
/// spouse's block left out when there is no spouse.
pub fn layout(t: &Template, fields: &Fields, return_type: &str, package: Option<&str>) -> Laid {
    let has_spouse = fields
        .get("Spouse name")
        .is_some_and(|s| !s.trim().is_empty());
    let mut top = H - MARGIN;
    let mut p = Pages::new(top);
    if t.letterhead {
        let firm = fields.get("Firm name").cloned().unwrap_or_default();
        let w = width(&firm, 15.0, true);
        let page = p.page();
        page.begin_text();
        page.set_font(Name(b"F2"), 15.0);
        page.next_line((W - w) / 2.0, H - 54.0);
        page.show(Str(&win_ansi(&firm)));
        page.end_text();
        page.set_line_width(0.6);
        page.move_to(MARGIN, H - 64.0);
        page.line_to(W - MARGIN, H - 64.0);
        page.stroke();
        top = H - 84.0;
        p.y = top;
    }
    if t.sample {
        let y = p.y - 4.0;
        let page = p.page();
        page.set_fill_rgb(0.75, 0.2, 0.1);
        page.begin_text();
        page.set_font(Name(b"F2"), 9.0);
        page.next_line(MARGIN, y);
        page.show(Str(&win_ansi("Sample — review before use")));
        page.end_text();
        page.set_fill_rgb(0.0, 0.0, 0.0);
        p.y -= 16.0;
    }
    let max = W - 2.0 * MARGIN;
    let mut blocks: Vec<SignBlock> = Vec::new();
    for b in t.blocks.iter().filter(|b| b.applies(return_type, package)) {
        match b {
            Block::Heading { text, .. } => {
                p.need(LEAD * 2.0);
                p.y -= 6.0;
                // Bold before measuring: each word is as wide as it is drawn.
                let mut s = spans(&merge(text, fields));
                for sp in &mut s {
                    sp.bold = true;
                }
                for line in wrap(words_of(&s, HEAD_SIZE), max) {
                    p.y -= HEAD_SIZE + 4.0;
                    draw_line(&mut p, MARGIN, &line, HEAD_SIZE);
                }
                p.y -= 2.0;
            }
            Block::Paragraph { text, .. } => {
                paragraph(&mut p, text, fields, MARGIN, max, SIZE, false);
                p.y -= 6.0;
            }
            Block::List { items, .. } => {
                for it in items {
                    p.need(LEAD);
                    let y = p.y - LEAD;
                    let page = p.page();
                    page.begin_text();
                    page.set_font(Name(b"F1"), SIZE);
                    page.next_line(MARGIN + 4.0, y);
                    page.show(Str(&win_ansi("•")));
                    page.end_text();
                    paragraph(&mut p, it, fields, MARGIN + 16.0, max - 16.0, SIZE, false);
                }
                p.y -= 6.0;
            }
            Block::Table { rows, .. } => {
                let cols = rows.iter().map(Vec::len).max().unwrap_or(0).max(1);
                let cw = max / cols as f32;
                for (r, row) in rows.iter().enumerate() {
                    // Each cell wrapped in its column; the row as tall as its tallest cell.
                    let cells: Vec<Vec<Vec<Word>>> = (0..cols)
                        .map(|c| {
                            let text = row.get(c).map(String::as_str).unwrap_or("");
                            let mut s = spans(&merge(text, fields));
                            if r == 0 {
                                for sp in &mut s {
                                    sp.bold = true;
                                }
                            }
                            wrap(words_of(&s, SIZE), cw - 8.0)
                        })
                        .collect();
                    let n = cells.iter().map(Vec::len).max().unwrap_or(1).max(1);
                    let h = n as f32 * LEAD + 6.0;
                    p.need(h);
                    let y0 = p.y;
                    for (c, lines) in cells.iter().enumerate() {
                        let mut y = y0;
                        for l in lines {
                            y -= LEAD;
                            p.y = y;
                            draw_line(&mut p, MARGIN + c as f32 * cw + 4.0, l, SIZE);
                        }
                    }
                    let page = p.page();
                    page.set_line_width(0.4);
                    page.rect(MARGIN, y0 - h, max, h);
                    page.stroke();
                    p.y = y0 - h;
                }
                p.y -= 8.0;
            }
            Block::Signature { role, .. } => {
                if role == "spouse" && !has_spouse {
                    continue;
                }
                p.need(SIGN_H);
                let line_y = p.y - 50.0;
                let page_n = p.n();
                let name = signer_name(role, fields);
                let page = p.page();
                page.set_line_width(0.6);
                page.move_to(MARGIN, line_y);
                page.line_to(MARGIN + 250.0, line_y);
                page.move_to(MARGIN + 300.0, line_y);
                page.line_to(MARGIN + 430.0, line_y);
                page.stroke();
                page.begin_text();
                page.set_font(Name(b"F1"), 9.0);
                page.next_line(MARGIN, line_y - 12.0);
                let under = if name.is_empty() {
                    role_words(role).to_string()
                } else {
                    format!("{name} ({})", role_words(role))
                };
                page.show(Str(&win_ansi(&under)));
                page.end_text();
                page.begin_text();
                page.set_font(Name(b"F1"), 9.0);
                page.next_line(MARGIN + 300.0, line_y - 12.0);
                page.show(Str(&win_ansi("Date")));
                page.end_text();
                blocks.push(SignBlock {
                    role: role.clone(),
                    page: page_n,
                    x: MARGIN,
                    y: line_y + 1.0,
                    w: 250.0,
                    h: 40.0,
                    date_x: MARGIN + 300.0,
                    date_y: line_y + 2.0,
                    date_w: 130.0,
                    date_h: 14.0,
                });
                p.y -= SIGN_H;
            }
        }
    }
    let pages = p.n();
    Laid {
        pdf: write(p.pages),
        pages,
        blocks,
    }
}

fn write(contents: Vec<Content>) -> Vec<u8> {
    let mut pdf = Pdf::new();
    let catalog = Ref::new(1);
    let tree = Ref::new(2);
    let fonts = [Ref::new(3), Ref::new(4), Ref::new(5), Ref::new(6)];
    let names: [&[u8]; 4] = [
        b"Helvetica",
        b"Helvetica-Bold",
        b"Helvetica-Oblique",
        b"Helvetica-BoldOblique",
    ];
    pdf.catalog(catalog).pages(tree);
    for (r, n) in fonts.iter().zip(names) {
        pdf.type1_font(*r)
            .base_font(Name(n))
            .encoding_predefined(Name(b"WinAnsiEncoding"));
    }
    let mut next = 7;
    let mut kids = Vec::new();
    for c in contents {
        let page_ref = Ref::new(next);
        let content_ref = Ref::new(next + 1);
        next += 2;
        kids.push(page_ref);
        let mut page = pdf.page(page_ref);
        page.media_box(Rect::new(0.0, 0.0, W, H));
        page.parent(tree);
        page.contents(content_ref);
        let mut res = page.resources();
        let mut f = res.fonts();
        for (i, r) in fonts.iter().enumerate() {
            let key = [b'F', b'1' + i as u8];
            f.pair(Name(&key), *r);
        }
        f.finish();
        res.finish();
        page.finish();
        pdf.stream(content_ref, &c.finish());
    }
    let count = kids.len() as i32;
    pdf.pages(tree).kids(kids).count(count);
    pdf.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Template;

    fn sample() -> Template {
        Template {
            id: "engagement-1040".into(),
            version: 1,
            name: "Engagement letter".into(),
            return_types: vec!["1040".into()],
            kind: "letter".into(),
            package: None,
            doc_type: "Engagement letter".into(),
            letterhead: true,
            default_ticked: true,
            sample: true,
            blocks: vec![
                Block::Heading {
                    text: "Engagement letter for {Tax year}".into(),
                    only_for: vec![],
                },
                Block::Paragraph {
                    text: "Dear {Signer first names}, thank you for choosing **{Firm name}**. "
                        .repeat(20),
                    only_for: vec![],
                },
                Block::Table {
                    rows: vec![
                        vec!["Service".into(), "Fee".into()],
                        vec!["Form 1040".into(), "{Fee}".into()],
                    ],
                    only_for: vec![],
                },
                Block::Signature {
                    role: "taxpayer".into(),
                    only_for: vec![],
                },
                Block::Signature {
                    role: "spouse".into(),
                    only_for: vec![],
                },
            ],
        }
    }

    #[test]
    fn lays_out_a_joint_letter_with_two_signature_blocks_inside_the_page() {
        let mut f = Fields::new();
        for (k, v) in [
            ("Firm name", "Sample CPA Firm"),
            ("Tax year", "2025"),
            ("Signer first names", "Jordan and Casey"),
            ("Taxpayer name", "Jordan Sample"),
            ("Spouse name", "Casey Sample"),
            ("Fee", "$1,265.68"),
        ] {
            f.insert(k.into(), v.into());
        }
        let laid = layout(&sample(), &f, "1040", None);
        assert!(laid.pdf.starts_with(b"%PDF"));
        assert_eq!(
            laid.blocks
                .iter()
                .map(|b| b.role.as_str())
                .collect::<Vec<_>>(),
            ["taxpayer", "spouse"]
        );
        for b in &laid.blocks {
            assert!(b.page >= 1 && b.page <= laid.pages);
            assert!(b.y >= MARGIN && b.y + b.h <= H - MARGIN, "{b:?}");
            assert!(b.x + b.w <= W - MARGIN && b.date_x + b.date_w <= W - MARGIN);
        }
        // Without a spouse, the spouse's block is left out.
        f.remove("Spouse name");
        let single = layout(&sample(), &f, "1040", None);
        assert_eq!(single.blocks.len(), 1);
    }

    #[test]
    fn a_long_letter_breaks_pages_and_a_block_never_splits() {
        let mut t = sample();
        t.blocks.insert(
            1,
            Block::Paragraph {
                text: "Words to fill the page. ".repeat(400),
                only_for: vec![],
            },
        );
        let laid = layout(&t, &Fields::new(), "1040", None);
        assert!(laid.pages >= 2);
        assert!(laid.blocks.iter().all(|b| b.y >= MARGIN));
    }
}
