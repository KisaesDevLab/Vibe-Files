//! Text recognition: the [`OcrEngine`] trait and the Tesseract engine. Used by Intake (reading
//! a scanned form so it can be recognised and filed) and by the Viewer's OCR action, which
//! writes a text layer into one document a person asked to have read ([`document`], owner
//! 2026-11-15, PA-77). The job queue that read every document's text on its own and wrote
//! text layers unasked was removed (owner 2026-11-13, PA-65).
//!
//! Tesseract runs as its own program (`tesseract.exe`, QUESTIONS Q-25): nothing native is
//! linked into Vibe Files. It is looked up in the configured path, then `tesseract\` beside
//! the exe (the portable layout), then Program Files, then `PATH`.

pub mod document;
pub mod engine;
pub use vf_pdf::layer;
pub mod tesseract;

pub use engine::{OcrEngine, OcrError, OcrPage, Word};
pub use tesseract::Tesseract;
