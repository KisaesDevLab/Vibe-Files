//! Vendored from Vibe Filer Desktop (see VENDORED.md): client master, config, folder template,
//! rules, relationships, filename parse, background worker. Module files are upstream's, unedited
//! except where VENDORED.md says so.

// Lints that only fire because upstream's binary crate is now a library (pub items are
// exported). Allowed here instead of editing vendored files.
#![allow(
    clippy::len_without_is_empty,
    clippy::result_unit_err,
    clippy::new_without_default
)]

pub mod clients;
pub mod config;
pub mod datafile;
pub mod parse;
pub mod relationships;
pub mod rulebuild;
pub mod rules;
pub mod template;
pub mod worker;
