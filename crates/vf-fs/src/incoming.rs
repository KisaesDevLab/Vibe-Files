//! Folders Vibe Files processes before the Intake queue sees a file (PHASES P02 steps 3–5):
//! scans (cover sheets), phone photos (image → PDF) and the print-capture folder. Processed
//! files land in the Filer inbox; originals are moved to a `processed` folder, never deleted.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use pdfium_render::prelude::Pdfium;
use vf_core::db::Connection;

use crate::intake::Hints;

/// A file is processed once it has not changed for this long (scanners and printers write in
/// pieces).
pub const SETTLE: Duration = Duration::from_secs(3);

/// What a pass did, for the status line.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Pass {
    pub routed: usize,
    pub queued: usize,
    pub errors: Vec<String>,
}

fn settled(p: &Path, now: SystemTime) -> bool {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .is_ok_and(|t| now.duration_since(t).unwrap_or_default() >= SETTLE)
}

/// Settled files directly in `dir` with one of `exts` (lower case), oldest name first.
pub fn ready_files(dir: &Path, exts: &[&str]) -> Vec<PathBuf> {
    let now = SystemTime::now();
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_file())
                .filter(|p| {
                    let name = p.file_name().unwrap_or_default().to_string_lossy();
                    !name.starts_with(['~', '.'])
                })
                .filter(|p| {
                    p.extension().is_some_and(|e| {
                        exts.contains(&e.to_string_lossy().to_lowercase().as_str())
                    })
                })
                .filter(|p| settled(p, now))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// Moves `file` into `inbox` unchanged (free name).
pub fn queue_as_is(file: &Path, inbox: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(inbox).map_err(|e| e.to_string())?;
    let dst = filer_intake::commit::free_path(&inbox.join(file.file_name().unwrap_or_default()));
    std::fs::rename(file, &dst)
        .map_err(|e| format!("cannot move {} to the inbox: {e}", file.display()))?;
    Ok(dst)
}

/// One pass over the scans folder: stacks with cover sheets are split and routed, others are
/// queued as they are.
pub fn process_scans(
    conn: &Connection,
    pdfium: &Pdfium,
    scans: &Path,
    inbox: &Path,
    hints: &Hints,
) -> Pass {
    let mut pass = Pass::default();
    let processed = scans.join("processed");
    for f in ready_files(scans, &["pdf"]) {
        match crate::stack::route_scan(conn, pdfium, &f, inbox, &processed, hints) {
            Ok(Some(r)) => pass.routed += r.len(),
            Ok(None) => match queue_as_is(&f, inbox) {
                Ok(_) => pass.queued += 1,
                Err(e) => pass.errors.push(e),
            },
            Err(e) => pass.errors.push(format!("{}: {e}", f.display())),
        }
    }
    pass
}
