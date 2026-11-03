//! PDF tools without UI or catalog (P04): the invisible text layer, annotations, stamps,
//! flatten, PDF/A, redaction, page operations, compression. Paths in, a new file out; the
//! callers file the result through `vf_fs::content`.

pub mod annot;
pub mod layer;
pub mod pii;
pub mod redact;
pub mod text;
pub mod words;
