//! Vibe Files' intake processors in front of Filer's inbox (P02 steps 3–5).
//!
//! Filer files only what lands in its inbox folders, by its rules (vendored, unchanged). Paper
//! and other arrivals that need work first land in a **staging folder** of this PC instead:
//! the Scans folder (scanner output, phone photos, barcode-sheet stacks) or the Print capture
//! folder (a PDF printer's output). [`process`] turns a staged file into the PDF(s) Filer
//! should see — split at barcode sheets, images made PDFs, print-capture names parsed — and
//! delivers them to Filer's first inbox under Filer's `Name_ID_rest.pdf` convention when the
//! client is known, with a [`Hint`] for what Filer's batch does not carry (doc type, year,
//! source). The staged original is kept in `<staging>/_processed/<date>/`.
//!
//! Staging is outside the cabinet (Filer refuses inboxes inside the root), so this is not a
//! catalog Transaction: nothing in the cabinet changes until Filer's Confirm, which is one.

use std::path::{Path, PathBuf};

use pdfium_render::prelude::Pdfium;
use vf_fs::hints::Hint;

use crate::sheet::{Cover, TaxYear};
use crate::{images, stack};

/// Where a staged file came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Scans,
    PrintCapture,
}

/// Staged originals are kept here (inside the staging folder).
pub const PROCESSED_DIR: &str = "_processed";
/// Files that could not be processed are moved here, with `<name>.error.txt` beside them.
pub const FAILED_DIR: &str = "_failed";

/// What a processor needs to know.
pub struct Context<'a> {
    /// Cabinet root (client list, doc types, hints file).
    pub root: &'a Path,
    /// Filer's inbox the results go to.
    pub inbox: &'a Path,
    pub device_id: &'a str,
    /// `None`: barcodes are not looked for (no pdfium.dll).
    pub pdfium: Option<&'a Pdfium>,
}

/// One file handed to Filer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivered {
    pub path: PathBuf,
    /// For the Intake strip: `pages 1–3 · barcode cover sheet`.
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Processed {
    pub staged: PathBuf,
    pub delivered: Vec<Delivered>,
    /// Where the original went.
    pub kept: PathBuf,
}

/// Names and labels the processors look up.
pub struct Lookup {
    /// CSV ID → client name.
    pub clients: std::collections::HashMap<String, String>,
    /// Doc type id → name.
    pub doc_types: std::collections::HashMap<String, String>,
}

impl Lookup {
    pub fn load(root: &Path) -> Result<Lookup, String> {
        let clients = filer_core::clients::load_cache(&vf_fs::clients::master_dir(root))
            .map_err(|e| format!("cannot read the client list: {e}"))?
            .map(|l| l.clients.into_iter().map(|c| (c.id, c.name)).collect())
            .unwrap_or_default();
        let doc_types = vf_fs::doctypes::load(root)?
            .unwrap_or_default()
            .into_iter()
            .map(|t| (t.id, t.name))
            .collect();
        Ok(Lookup { clients, doc_types })
    }
}

/// Processes one staged file. On error nothing was delivered and the file is where it was.
pub fn process(ctx: &Context<'_>, staged: &Path, origin: Origin) -> Result<Processed, String> {
    let name = staged
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or("no file name")?;
    let stem = Path::new(&name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.clone());
    let lookup = Lookup::load(ctx.root)?;
    if origin == Origin::PrintCapture {
        if !is_pdf(staged) {
            return Err(format!("{name}: the print capture folder takes PDFs"));
        }
        let tmp = tmp_in(ctx.inbox);
        copy_to(staged, &tmp)?;
        let (file, hint, note) = capture_name(ctx.root, &lookup, &name);
        let d = deliver(ctx, &tmp, &file, Some(hint), note).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })?;
        let kept = keep_original(staged)?;
        return Ok(Processed {
            staged: staged.to_path_buf(),
            delivered: vec![d],
            kept,
        });
    }
    // Images become a PDF first (step 4); it is then handled like a scanned PDF.
    let image = if images::is_image(staged) {
        let tmp = tmp_in(ctx.inbox);
        let n = images::to_pdf(staged, &tmp).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })?;
        Some((tmp, n.note()))
    } else if is_pdf(staged) {
        None
    } else {
        return Err(format!(
            "{name}: not a PDF or an image ({})",
            images::EXTENSIONS.join(", ")
        ));
    };
    let result = split_and_deliver(ctx, &lookup, staged, image.as_ref(), &name, &stem);
    if let Some((tmp, _)) = &image {
        let _ = std::fs::remove_file(tmp);
    }
    let out = result?;
    let kept = keep_original(staged)?;
    Ok(Processed {
        staged: staged.to_path_buf(),
        delivered: out,
        kept,
    })
}

/// Splits the PDF (the staged file, or the PDF made from a staged image) at its sheets and
/// delivers the pieces.
fn split_and_deliver(
    ctx: &Context<'_>,
    lookup: &Lookup,
    staged: &Path,
    image: Option<&(PathBuf, String)>,
    name: &str,
    stem: &str,
) -> Result<Vec<Delivered>, String> {
    let pdf = image.map_or(staged, |(p, _)| p.as_path());
    let pages = match ctx.pdfium {
        Some(p) => stack::scan_pages(p, pdf)?,
        None => vec![],
    };
    let segments = stack::segments(&pages);
    let mut out = Vec::new();
    if segments.len() <= 1 && segments.first().is_none_or(|s| s.sheet.is_none()) {
        // No sheet (or no pdfium): the file goes to Filer as it is.
        let tmp = tmp_in(ctx.inbox);
        copy_to(pdf, &tmp)?;
        let (file, note) = match image {
            Some((_, note)) => (format!("{stem}.pdf"), note.clone()),
            None => (name.to_string(), "no barcode sheet".into()),
        };
        out.push(deliver(ctx, &tmp, &file, None, note)?);
    } else {
        let tmps: Vec<PathBuf> = segments.iter().map(|_| tmp_in(ctx.inbox)).collect();
        let ranges: Vec<_> = segments.iter().map(|s| s.pages.clone()).collect();
        filer_pdf::pdf::split_pdf(pdf, &ranges, &tmps)?;
        for (i, (seg, tmp)) in segments.iter().zip(&tmps).enumerate() {
            let pages = page_note(&seg.pages);
            let r = match &seg.sheet {
                Some(Some(cover)) => {
                    let (file, hint) = cover_name(cover, lookup, stem, i + 1);
                    deliver(
                        ctx,
                        tmp,
                        &file,
                        Some(hint),
                        format!("{pages} · barcode cover sheet"),
                    )
                }
                Some(None) => deliver(
                    ctx,
                    tmp,
                    &format!("{stem} part {}.pdf", i + 1),
                    None,
                    format!("{pages} · separator sheet"),
                ),
                None => deliver(
                    ctx,
                    tmp,
                    &format!("{stem} part {}.pdf", i + 1),
                    None,
                    format!("{pages} · before the first sheet"),
                ),
            };
            match r {
                Ok(d) => out.push(d),
                Err(e) => {
                    for t in &tmps {
                        let _ = std::fs::remove_file(t);
                    }
                    return Err(e);
                }
            }
        }
    }
    Ok(out)
}

pub fn is_pdf(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}

fn page_note(r: &std::ops::Range<usize>) -> String {
    if r.len() == 1 {
        format!("page {}", r.start + 1)
    } else {
        format!("pages {}–{}", r.start + 1, r.end)
    }
}

/// Filer's `Name_ID_rest.pdf` for a cover-sheet document, and its hint.
fn cover_name(cover: &Cover, lookup: &Lookup, stem: &str, part: usize) -> (String, Hint) {
    let client = lookup
        .clients
        .get(&cover.client_id)
        .cloned()
        .unwrap_or_else(|| "Client".into());
    let mut rest: Vec<String> = Vec::new();
    match cover.tax_year {
        Some(TaxYear::Year(y)) => rest.push(y.to_string()),
        Some(TaxYear::Permanent) => rest.push("Permanent".into()),
        None => {}
    }
    if let Some(t) = cover
        .doc_type
        .as_ref()
        .and_then(|t| lookup.doc_types.get(t))
    {
        rest.push(t.clone());
    }
    if rest.is_empty() {
        rest.push(format!("{stem} part {part}"));
    }
    let file = format!(
        "{}_{}_{}.pdf",
        clean(&client),
        cover.client_id,
        clean(&rest.join(" "))
    );
    let mut hint = Hint::new("", &file, "barcode", "");
    hint.doc_type = cover.doc_type.clone();
    match cover.tax_year {
        Some(TaxYear::Year(y)) => hint.tax_year = Some(y),
        Some(TaxYear::Permanent) => hint.permanent = true,
        None => {}
    }
    (file, hint)
}

/// Print capture (step 5): the name a parser profile reads, as Filer's `Name_ID_rest.pdf`, and
/// a hint with the profile's doc type, year and source (`firm`). A name no profile reads goes
/// through as it is, still marked as the firm's.
fn capture_name(root: &Path, lookup: &Lookup, name: &str) -> (String, Hint, String) {
    let _ = crate::parsers::seed_if_missing(root);
    let (profiles, _) = crate::parsers::load(root);
    let Some(p) = crate::parsers::parse(&profiles, name) else {
        let mut h = Hint::new("", name, "print_capture", "");
        h.source = Some("firm".into());
        return (
            name.to_string(),
            h,
            "print capture · no parser profile matched".into(),
        );
    };
    let client = lookup
        .clients
        .get(&p.client_id)
        .cloned()
        .unwrap_or_else(|| "Client".into());
    // `<year> <profile> <doc type or document name>`: Filer's rules match words in it (the
    // firm's rules send "UltraTax" to `Income Tax/{Year}`; the tax rules match "Return",
    // "Organizer", …).
    let rest: Vec<String> = p
        .year
        .map(|y| y.to_string())
        .into_iter()
        .chain([p.profile.clone()])
        .chain(p.doc_type.clone().or(p.doc.clone()))
        .collect();
    let rest = if rest.is_empty() {
        "print".to_string()
    } else {
        rest.join(" ")
    };
    let file = format!("{}_{}_{}.pdf", clean(&client), p.client_id, clean(&rest));
    let mut h = Hint::new("", &file, &format!("print_capture:{}", p.profile), "");
    h.tax_year = p.year;
    h.source = Some(p.source.clone());
    h.doc_type = p.doc_type.as_ref().and_then(|t| {
        lookup
            .doc_types
            .iter()
            .find(|(_, n)| *n == t)
            .map(|(id, _)| id.clone())
    });
    let note = format!(
        "{} filename profile{}",
        p.profile,
        p.doc_type.map(|t| format!(" · {t}")).unwrap_or_default()
    );
    (file, h, note)
}

/// A file-name part without characters Windows refuses.
pub fn clean(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    t.trim().trim_end_matches('.').to_string()
}

/// A temporary name in `dir` Filer's watcher skips (dot file, `.tmp`).
fn tmp_in(dir: &Path) -> PathBuf {
    dir.join(format!(".vf-{}.tmp", uuid::Uuid::new_v4()))
}

fn copy_to(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::copy(src, dst)
        .map(drop)
        .map_err(|e| format!("cannot copy {} to the inbox: {e}", src.display()))
}

/// `dir/name`, or `dir/stem (2).ext`, … when taken.
pub fn free_name(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let p = Path::new(name);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();
    let ext = p
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .expect("a free name")
}

/// Records the hint (when it says anything), then renames `tmp` into the inbox as `name`.
fn deliver(
    ctx: &Context<'_>,
    tmp: &Path,
    name: &str,
    hint: Option<Hint>,
    note: String,
) -> Result<Delivered, String> {
    let dst = free_name(ctx.inbox, name);
    if let Some(mut h) = hint.filter(Hint::is_useful) {
        h.sha256 = filer_intake::commit::sha256_file(tmp)
            .map_err(|e| format!("cannot hash {}: {e}", tmp.display()))?;
        h.name = dst
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        h.device = ctx.device_id.into();
        // Before the file appears in the inbox, so the hint is there when Filer files it.
        vf_fs::hints::append(ctx.root, &h)?;
    }
    std::fs::rename(tmp, &dst).map_err(|e| {
        let _ = std::fs::remove_file(tmp);
        format!("cannot deliver {name} to {}: {e}", ctx.inbox.display())
    })?;
    Ok(Delivered { path: dst, note })
}

fn dated_dir(staged: &Path, sub: &str) -> Result<PathBuf, String> {
    let dir = staged.parent().ok_or("no folder")?.join(sub).join(today());
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    Ok(dir)
}

fn today() -> String {
    let now = vf_core::now_rfc3339();
    now.get(..10).unwrap_or("undated").to_string()
}

/// Moves the staged original to `_processed/<date>/`.
fn keep_original(staged: &Path) -> Result<PathBuf, String> {
    let dir = dated_dir(staged, PROCESSED_DIR)?;
    let name = staged.file_name().ok_or("no file name")?.to_string_lossy();
    let dst = free_name(&dir, &name);
    std::fs::rename(staged, &dst)
        .map_err(|e| format!("cannot move {} aside: {e}", staged.display()))?;
    Ok(dst)
}

/// Moves a file that failed to `_failed/<date>/` with the error beside it.
pub fn set_aside(staged: &Path, error: &str) -> Result<PathBuf, String> {
    let dir = dated_dir(staged, FAILED_DIR)?;
    let name = staged.file_name().ok_or("no file name")?.to_string_lossy();
    let dst = free_name(&dir, &name);
    std::fs::rename(staged, &dst)
        .map_err(|e| format!("cannot move {} aside: {e}", staged.display()))?;
    let _ = std::fs::write(
        dst.with_file_name(format!(
            "{}.error.txt",
            dst.file_name().unwrap().to_string_lossy()
        )),
        error,
    );
    Ok(dst)
}
