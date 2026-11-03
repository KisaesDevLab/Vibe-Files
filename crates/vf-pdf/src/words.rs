//! Words on a page with their boxes: what OCR reads and what the invisible text layer writes.

use serde::{Deserialize, Serialize};

/// One recognized word. Box in pixels of the image given to the engine, origin top left.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Word {
    /// `[left, top, width, height]`.
    pub bbox: [u32; 4],
    pub text: String,
    /// 0–100.
    pub conf: f32,
    /// Line number on the page (0-based, reading order), for rebuilding the text.
    pub line: u32,
}

/// What the engine read on one page image.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OcrPage {
    pub width: u32,
    pub height: u32,
    pub words: Vec<Word>,
}

impl OcrPage {
    /// The page text: words joined by spaces, lines by newlines.
    pub fn text(&self) -> String {
        let mut out = String::new();
        let mut line = None;
        for w in &self.words {
            match line {
                Some(l) if l == w.line => out.push(' '),
                Some(_) => out.push('\n'),
                None => {}
            }
            out.push_str(&w.text);
            line = Some(w.line);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_joins_words_and_lines() {
        let w = |t: &str, line| Word {
            bbox: [0, 0, 1, 1],
            text: t.into(),
            conf: 90.0,
            line,
        };
        let p = OcrPage {
            width: 1,
            height: 1,
            words: vec![w("EIN", 0), w("12-3456789", 0), w("Acme", 1)],
        };
        assert_eq!(p.text(), "EIN 12-3456789\nAcme");
    }
}
