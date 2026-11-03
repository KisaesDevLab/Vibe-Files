//! The engine contract. One implementation in v1 ([`crate::Tesseract`]); the ONNX slot is
//! reserved for v2.

use image::GrayImage;
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum OcrError {
    /// The engine is not installed or cannot be started. Jobs wait; they are not failed.
    #[error("OCR engine unavailable: {0}")]
    Unavailable(String),
    /// This page could not be read.
    #[error("OCR failed: {0}")]
    Failed(String),
}

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

pub trait OcrEngine: Send + Sync {
    /// Short name for logs and the catalog (`tesseract`).
    fn name(&self) -> &str;
    /// Reads one page image (gray, `dpi` dots per inch).
    fn recognize(&self, page: &GrayImage, dpi: u32) -> Result<OcrPage, OcrError>;
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
