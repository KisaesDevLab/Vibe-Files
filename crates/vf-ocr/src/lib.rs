//! OCR (P03): the [`OcrEngine`] trait, the Tesseract engine, the job queue and the
//! searchable-PDF write-back.
//!
//! Tesseract runs as its own program (`tesseract.exe`, QUESTIONS Q-25): nothing native is
//! linked into Vibe Files. It is looked up in the configured path, then `tesseract\` beside
//! the exe (the portable layout), then Program Files, then `PATH`.

pub mod engine;
pub mod tesseract;

pub use engine::{OcrEngine, OcrError, OcrPage, Word};
pub use tesseract::Tesseract;
