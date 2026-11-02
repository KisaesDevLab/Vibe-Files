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

/// Moves a file that could not be processed to `<its folder>/failed/`, so the next pass does not
/// retry it (and report it) again; the message says where it went. Put it back to retry.
pub fn set_aside_failed(file: &Path, why: &str) -> String {
    let dir = file.parent().unwrap_or(Path::new("")).join("failed");
    let kept = std::fs::create_dir_all(&dir).ok().and_then(|()| {
        let dst = filer_intake::commit::free_path(&dir.join(file.file_name().unwrap_or_default()));
        std::fs::rename(file, &dst).ok().map(|()| dst)
    });
    match kept {
        Some(dst) => format!("{}: {why} (moved to {})", file.display(), dst.display()),
        None => format!("{}: {why}", file.display()),
    }
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
                Err(e) => pass.errors.push(set_aside_failed(&f, &e)),
            },
            Err(e) => pass.errors.push(set_aside_failed(&f, &e)),
        }
    }
    pass
}

/// One pass over the photo folders: every settled image (JPG / PNG / TIFF / HEIC) becomes a
/// one-page PDF of the same name in the inbox (`vf_ingest::image_pdf`). Images dropped straight
/// into the inbox are converted in place. Originals go to `<folder>/processed`.
pub fn process_photos(dirs: &[PathBuf], inbox: &Path) -> Pass {
    let mut pass = Pass::default();
    let exts = vf_ingest::image_pdf::IMAGE_EXTS;
    for dir in dirs {
        let processed = dir.join("processed");
        for f in ready_files(dir, &exts) {
            let stem = f
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let work = processed.join(format!("~work-{}", vf_core::new_id()));
            let pdf = work.join(format!("{stem}.pdf"));
            let r = std::fs::create_dir_all(&work)
                .map_err(|e| e.to_string())
                .and_then(|()| vf_ingest::image_pdf::image_to_pdf(&f, &pdf))
                .and_then(|()| queue_as_is(&pdf, inbox).map(|_| ()))
                .and_then(|()| {
                    let kept = filer_intake::commit::free_path(
                        &processed.join(f.file_name().unwrap_or_default()),
                    );
                    std::fs::rename(&f, &kept)
                        .map_err(|e| format!("cannot move {} aside: {e}", f.display()))
                });
            let _ = std::fs::remove_dir(&work);
            match r {
                Ok(()) => pass.queued += 1,
                Err(e) if f.exists() => pass.errors.push(set_aside_failed(&f, &e)),
                Err(e) => pass.errors.push(format!("{}: {e}", f.display())),
            }
        }
    }
    pass
}

#[cfg(test)]
mod photo_tests {
    use super::*;

    /// P02 done-when: a HEIC phone photo becomes a 1-page PDF and routes by rule.
    #[cfg(feature = "heic")]
    #[test]
    fn heic_photo_becomes_a_one_page_pdf_that_routes_by_rule() {
        let src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/images/synthetic-page.heic");
        photo_routes_by_rule(
            &|p: &Path| {
                std::fs::copy(&src, p).unwrap();
            },
            "heic",
        );
    }

    /// The same path with a JPEG (runs on every build, HEIC or not).
    #[test]
    fn jpeg_photo_becomes_a_one_page_pdf_that_routes_by_rule() {
        photo_routes_by_rule(
            &|p: &Path| {
                vf_ingest::image_pdf::test_page(900, 1200, 0.0)
                    .to_rgb8()
                    .save(p)
                    .unwrap();
            },
            "jpg",
        );
    }

    fn photo_routes_by_rule(make: &dyn Fn(&Path), ext: &str) {
        use filer_intake::plan::RowStatus;

        use crate::clients::{NewClient, create_client, template as cab_template};
        use crate::intake::prepare_filer_dir;

        let mut w = crate::clients::tests::world();
        vf_core::doctype::seed(&mut w.conn, &w.actor).unwrap();
        let tpl = cab_template(&w.root);
        let nc = NewClient {
            ext_id: "1042".into(),
            name: "Anderson Farms LLC".into(),
            ..NewClient::default()
        };
        create_client(&mut w.conn, &w.root, &w.cab, &tpl, &nc, &w.actor).unwrap();
        let base = w.root.parent().unwrap().to_path_buf();
        let (filer, inbox, photos) = (
            base.join("data/filer"),
            base.join("data/Inbox"),
            base.join("data/Photos"),
        );
        prepare_filer_dir(&mut w.conn, &w.actor, &filer, &w.root, &w.cab, &tpl, &inbox).unwrap();
        std::fs::create_dir_all(&photos).unwrap();
        let photo = photos.join(format!("Anderson Farms LLC_1042_2025 Payroll.{ext}"));
        make(&photo);
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&photo)
            .unwrap()
            .set_modified(old)
            .unwrap();

        let pass = process_photos(std::slice::from_ref(&photos), &inbox);
        assert_eq!(
            (pass.queued, pass.errors.len()),
            (1, 0),
            "{:?}",
            pass.errors
        );
        let pdf = inbox.join("Anderson Farms LLC_1042_2025 Payroll.pdf");
        assert_eq!(filer_pdf::pdf::page_count(&pdf).unwrap(), 1);
        assert!(
            photos
                .join("processed")
                .join(photo.file_name().unwrap())
                .is_file(),
            "photo kept"
        );

        let desk = crate::testkit::Desk {
            root: &w.root,
            filer: &filer,
            inbox: &inbox,
            tpl: &tpl,
            cab: &w.cab,
        };
        let out = desk.plan(std::slice::from_ref(&pdf));
        let p = &out.proposals[0];
        assert_eq!(p.status, RowStatus::Ready, "{:?}", p.dest);
        assert_eq!(p.rule_name, "Payroll");
        assert!(
            p.dest
                .as_ref()
                .unwrap()
                .ends_with("Payroll/2025/Anderson Farms LLC_1042_2025 Payroll.pdf"),
            "{:?}",
            p.dest
        );
    }
}
