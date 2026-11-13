//! Text recognition for Intake (reading a scanned form so it can be recognised and filed): the
//! [`OcrEngine`] trait and the Tesseract engine. The job queue that read every document's text
//! and the searchable-PDF write-back were removed (owner 2026-11-13, PA-65).
//!
//! Tesseract runs as its own program (`tesseract.exe`, QUESTIONS Q-25): nothing native is
//! linked into Vibe Files. It is looked up in the configured path, then `tesseract\` beside
//! the exe (the portable layout), then Program Files, then `PATH`.

pub mod engine;
pub use vf_pdf::layer;
pub mod tesseract;

pub use engine::{OcrEngine, OcrError, OcrPage, Word};
pub use tesseract::Tesseract;
