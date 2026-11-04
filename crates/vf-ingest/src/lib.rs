//! Intake processors in front of Filer's inbox (P02): barcode cover/separator sheets, scanned
//! stacks split at them, a worker watching this PC's staging folders. Scanner (TWAIN/WIA),
//! image→PDF and the PDF-printer watcher arrive with their steps.

pub mod images;
pub mod parsers;
pub mod pipeline;
pub mod recognize;
pub mod scanner;
pub mod sheet;
pub mod stack;
pub mod twain;
pub mod ultratax;
pub mod worker;
