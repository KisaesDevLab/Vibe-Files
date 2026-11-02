//! Vendored from Vibe Filer Desktop (see VENDORED.md): PDF merge/split/text/thumbnails (`pdf`)
//! and the split cut-point finder (`split`).

// Lints that only fire because upstream's binary crate is now a library (pub items are
// exported). Allowed here instead of editing vendored files.
#![allow(
    clippy::len_without_is_empty,
    clippy::result_unit_err,
    clippy::new_without_default
)]

pub mod pdf;
pub mod split;

// Upstream paths (`crate::clients`, …) resolve through these re-exports, so module files stay unedited.
pub use filer_core::{clients, config, parse};
