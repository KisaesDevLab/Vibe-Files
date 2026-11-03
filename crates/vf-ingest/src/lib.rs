//! Intake processors in front of Filer's inbox (P02): barcode cover/separator sheets, scanned
//! stacks split at them, a worker watching this PC's staging folders. Scanner (TWAIN/WIA),
//! image→PDF and the PDF-printer watcher arrive with their steps.

pub mod pipeline;
pub mod sheet;
pub mod stack;
pub mod worker;
