//! The engine contract. One implementation in v1 ([`crate::Tesseract`]); the ONNX slot is
//! reserved for v2.

use image::GrayImage;
pub use vf_pdf::words::{OcrPage, Word};

#[derive(Debug, thiserror::Error)]
pub enum OcrError {
    /// The engine is not installed or cannot be started. Jobs wait; they are not failed.
    #[error("OCR engine unavailable: {0}")]
    Unavailable(String),
    /// This page could not be read.
    #[error("OCR failed: {0}")]
    Failed(String),
}

pub trait OcrEngine: Send + Sync {
    /// Short name for logs and the catalog (`tesseract`).
    fn name(&self) -> &str;
    /// Reads one page image (gray, `dpi` dots per inch).
    fn recognize(&self, page: &GrayImage, dpi: u32) -> Result<OcrPage, OcrError>;
}
