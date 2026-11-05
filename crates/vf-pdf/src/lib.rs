//! PDF tools without UI or catalog (P04): the invisible text layer, annotations, stamps,
//! flatten, form filling, PDF/A, redaction, page operations, compression. Paths in, a new file out; the
//! callers file the result through `vf_fs::content`.

pub mod annot;
pub mod compress;
pub mod flatten;
pub mod forms;
pub mod layer;
pub mod pages;
pub mod pdfa;
pub mod pii;
pub mod raster;
pub mod redact;
pub mod report;
pub mod text;
pub mod words;
