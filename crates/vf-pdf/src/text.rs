//! A page's text with where each character is (pdfium): words for selection and markup, the
//! character run the PII finder searches and maps back to boxes (redaction).

use pdfium_render::prelude::PdfPage;

/// A word on a page: its box in user space and its text.
#[derive(Debug, Clone, PartialEq)]
pub struct WordBox {
    pub rect: [f32; 4],
    pub text: String,
}

/// The page's characters in reading order, each with its box in user space (`None` for
/// generated spaces and line breaks).
pub fn chars(page: &PdfPage<'_>) -> Vec<(char, Option<[f32; 4]>)> {
    let Ok(text) = page.text() else {
        return vec![];
    };
    text.chars()
        .iter()
        .map(|ch| {
            let c = ch.unicode_char().unwrap_or(' ');
            let b = ch.loose_bounds().ok().map(|b| {
                [
                    b.left().value,
                    b.bottom().value,
                    b.right().value,
                    b.top().value,
                ]
            });
            (c, b.filter(|_| !c.is_whitespace()))
        })
        .collect()
}

/// The page's words with their boxes, in reading order.
pub fn words(page: &PdfPage<'_>) -> Vec<WordBox> {
    let mut out: Vec<WordBox> = Vec::new();
    let mut cur: Option<WordBox> = None;
    for (c, b) in chars(page) {
        match b {
            Some(r) => match &mut cur {
                Some(w) => {
                    w.text.push(c);
                    w.rect = union(w.rect, r);
                }
                None => {
                    cur = Some(WordBox {
                        rect: r,
                        text: c.to_string(),
                    })
                }
            },
            None => out.extend(cur.take()),
        }
    }
    out.extend(cur);
    out
}

pub fn union(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}

/// Whether two user-space boxes overlap.
pub fn overlaps(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[2] && a[2] > b[0] && a[1] < b[3] && a[3] > b[1]
}
