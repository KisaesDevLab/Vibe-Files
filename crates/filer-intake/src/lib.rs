//! Vendored from Vibe Filer Desktop (see VENDORED.md): intake watcher, zip import, plan, commit
//! (every document move), undo, op log, recovery, sentinels and folder scan, and the Filer UI
//! that becomes Vibe Files' Intake tab.

// Lints that only fire because upstream's binary crate is now a library (pub items are
// exported). Allowed here instead of editing vendored files.
#![allow(
    clippy::len_without_is_empty,
    clippy::result_unit_err,
    clippy::new_without_default
)]

pub mod app;
pub mod commit;
pub mod intake;
pub mod logging;
pub mod oplog;
pub mod pid;
pub mod plan;
pub mod recovery;
pub mod sentinel;
pub mod ui;
pub mod undo;
pub mod zipimport;

// Upstream paths (`crate::clients`, `crate::pdf`, …) resolve through these re-exports, so module
// files stay unedited.
pub use filer_core::{clients, config, parse, relationships, rulebuild, rules, template, worker};
pub use filer_pdf::{pdf, split};
