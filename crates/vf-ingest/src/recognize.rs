//! Auto-file (PHASES P08 steps 1–3) in front of Filer: a file that reaches Intake with no
//! client in its name (no barcode sheet, no print-capture profile) has its first page read.
//! A recognized form whose client is sure (`vf_extract::matching::THRESHOLD`) goes to Filer
//! as `<Client>_<ID>_<year> <doc type> <payer>.pdf` with a hint (doc type, year, the
//! proposal), so Filer's preview shows it matched; the person still confirms there. Below the
//! threshold it keeps its name and only the doc type and year are hinted: never auto-filed on
//! a guess.
//!
//! Only page 1 is read: its text layer, or OCR when it has none (a scan). A filled-in fillable
//! PDF is read at the page with its filled fields, each value with its printed label
//! ([`form_read`]).

use std::path::Path;

use pdfium_render::prelude::*;
use vf_extract::matching::{self, ClientRef, Proposal};
use vf_fs::hints::{Hint, Recognized};

use crate::pipeline::{Lookup, clean};

/// Characters a page needs before its own text is used rather than OCR.
const TEXT_MIN: usize = 40;
/// Resolution page 1 is read at when it needs OCR.
const DPI: f32 = 300.0;

/// What form recognition reads from a file: page text, and for a filled-in fillable PDF each
/// filled field's printed label and value.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FormRead {
    pub text: String,
    /// (printed label, value typed in), top to bottom.
    pub fields: Vec<(String, String)>,
}

/// What to read: a filled-in fillable PDF at the page with the most filled fields (Q-61: a
/// value typed into a field is drawn by the field, not part of the page's text), each value
/// with the printed label nearest above it (or just left of it); otherwise page 1
/// ([`page_one_text`]).
pub fn form_read(
    pdfium: &Pdfium,
    pdf: &Path,
    ocr: Option<&dyn vf_ocr::OcrEngine>,
) -> Option<FormRead> {
    let filled = filled_read(pdfium, pdf);
    if let Some((_, r)) = &filled
        && r.text.chars().filter(|c| !c.is_whitespace()).count() >= TEXT_MIN
        && vf_extract::recognize(&r.text).is_some()
    {
        return filled.map(|(_, r)| r);
    }
    // The filled page's own text does not say which form it is (an image behind the fields,
    // or a notes page): page 1 is read as before, OCR included; the fields still count when
    // they are on page 1 (QA 2026-11-05).
    let text = page_one_text(pdfium, pdf, ocr)?;
    let fields = match filled {
        Some((0, r)) => r.fields,
        _ => vec![],
    };
    Some(FormRead { text, fields })
}

/// The page with the most filled fields (0-based) and what it says.
fn filled_read(pdfium: &Pdfium, pdf: &Path) -> Option<(usize, FormRead)> {
    use vf_pdf::forms::FieldKind;
    let fields = vf_pdf::forms::read(pdf).ok()?;
    let typed = |f: &vf_pdf::forms::Field| {
        !f.value.trim().is_empty()
            && matches!(f.kind, FieldKind::Text { .. } | FieldKind::Choice { .. })
    };
    let mut per_page: std::collections::BTreeMap<usize, usize> = Default::default();
    for f in fields.iter().filter(|f| typed(f)) {
        *per_page.entry(f.page()).or_default() += 1;
    }
    let page = per_page
        .iter()
        .max_by_key(|(p, n)| (**n, std::cmp::Reverse(**p)))
        .map(|(p, _)| *p)?;
    let doc = pdfium.load_pdf_from_file(pdf, None).ok()?;
    let p = doc.pages().get(page as _).ok()?;
    let text = vf_pdf::text::page_text(&p);
    // The page's printed text runs: [left, bottom, right, top] and the text.
    let runs: Vec<([f32; 4], String)> = p
        .text()
        .ok()?
        .segments()
        .iter()
        .filter_map(|s| {
            let b = s.bounds();
            let t = s.text().trim().to_string();
            (!t.is_empty()).then_some((
                [
                    b.left().value,
                    b.bottom().value,
                    b.right().value,
                    b.top().value,
                ],
                t,
            ))
        })
        .collect();
    let mut out: Vec<(f32, f32, String, String)> = vec![];
    for f in &fields {
        let ticked =
            matches!(f.kind, FieldKind::Check { .. }) && vf_pdf::forms::Field::is_on(&f.value);
        if !(typed(f) || ticked) {
            continue;
        }
        let Some(w) = f.widgets.iter().find(|w| w.page == page) else {
            continue;
        };
        let [x0, y0, x1, y1] = w.rect;
        let (l, b, r, t) = (x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1));
        let Some(label) = label_for(&runs, [l, b, r, t]) else {
            continue;
        };
        let value = if ticked {
            "X".to_string()
        } else {
            f.value.clone()
        };
        out.push((t, l, label, value));
    }
    out.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.total_cmp(&b.1)));
    Some((
        page,
        FormRead {
            text,
            fields: out.into_iter().map(|(_, _, l, v)| (l, v)).collect(),
        },
    ))
}

/// The printed label of a field whose widget is `[left, bottom, right, top]`: the nearest run
/// above it that starts over it (a box's caption sits at its top left, the value under it),
/// else the nearest run just left of it on its line (a check box's caption).
fn label_for(runs: &[([f32; 4], String)], [l, b, r, t]: [f32; 4]) -> Option<String> {
    let above = runs
        .iter()
        .filter(|([rl, rb, _, _], _)| {
            *rb >= t - 4.0 && *rb - t <= 40.0 && *rl >= l - 16.0 && *rl <= r - 2.0
        })
        .min_by(|a, b| {
            let d = |x: &([f32; 4], String)| (x.0[1] - t).abs() + 0.05 * (x.0[0] - l).abs();
            d(a).total_cmp(&d(b))
        });
    if let Some((_, s)) = above {
        return Some(s.clone());
    }
    let mid = (b + t) / 2.0;
    runs.iter()
        .filter(|([_, rb, rr, rt], _)| {
            *rb <= mid && *rt >= mid && *rr <= l + 2.0 && l - *rr <= 200.0
        })
        .min_by(|a, b| (l - a.0[2]).total_cmp(&(l - b.0[2])))
        .map(|(_, s)| s.clone())
}

/// What the form reading found in a file waiting in Intake's inbox (Q-61): for Intake's row,
/// the client (its list ID, only when sure) and year, and the note; the doc type, year and
/// reading are kept as a hint for when the file is filed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxReading {
    pub client_ext_id: Option<String>,
    pub year: Option<u16>,
    pub note: String,
}

/// Reads a file in Intake's inbox whose name no rule names; records the hint (by the file's
/// SHA-256) so filing it sets the doc type, year and reading. `None`: not a form read here.
pub fn read_inbox_file(
    pdfium: &Pdfium,
    root: &Path,
    file: &Path,
    ocr: Option<&dyn vf_ocr::OcrEngine>,
    device_id: &str,
) -> Option<InboxReading> {
    let lookup = Lookup::load(root).ok()?;
    let original = file.file_name()?.to_string_lossy().into_owned();
    let read = form_read(pdfium, file, ocr)?;
    let o = decide_read(root, &lookup, &read, &original)?;
    let mut hint = o.hint;
    hint.sha256 = filer_intake::commit::sha256_file(file).ok()?;
    hint.name = original;
    hint.device = device_id.into();
    if hint.is_useful() {
        vf_fs::hints::append(root, &hint).ok()?;
    }
    Some(InboxReading {
        client_ext_id: o
            .proposal
            .confident()
            .then(|| o.proposal.primary.as_ref().map(|m| m.ext_id.clone()))
            .flatten(),
        year: hint.tax_year.and_then(|y| u16::try_from(y).ok()),
        note: o.note,
    })
}

/// Page 1's text: its own text layer, else OCR (when an engine is given).
pub fn page_one_text(
    pdfium: &Pdfium,
    pdf: &Path,
    ocr: Option<&dyn vf_ocr::OcrEngine>,
) -> Option<String> {
    let doc = pdfium.load_pdf_from_file(pdf, None).ok()?;
    let page = doc.pages().get(0).ok()?;
    let own = vf_pdf::text::page_text(&page);
    if own.chars().filter(|c| !c.is_whitespace()).count() >= TEXT_MIN {
        return Some(own);
    }
    let engine = ocr?;
    let (w, h) = (page.width().value, page.height().value);
    let scale = (DPI / 72.0).min(4200.0 / w.max(h).max(1.0));
    let config = PdfRenderConfig::new().set_target_width((w * scale) as i32);
    let img = page
        .render_with_config(&config)
        .and_then(|b| b.as_image())
        .ok()?
        .to_luma8();
    let read = engine.recognize(&img, (scale * 72.0).round() as u32).ok()?;
    // Words back into lines, in reading order.
    let mut lines: Vec<(u32, Vec<String>)> = Vec::new();
    for w in read.words {
        match lines.last_mut() {
            Some((n, ws)) if *n == w.line => ws.push(w.text),
            _ => lines.push((w.line, vec![w.text])),
        }
    }
    Some(
        lines
            .into_iter()
            .map(|(_, ws)| ws.join(" "))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// The client list as the matcher sees it: from the cabinet's client master and its last-4s.
pub fn client_refs(root: &Path) -> Vec<ClientRef> {
    let Ok(m) = vf_fs::clients::load(root) else {
        return vec![];
    };
    m.list
        .map(|l| {
            l.clients
                .into_iter()
                .map(|c| {
                    let id = c.uuid.to_string();
                    ClientRef {
                        tin4: m.meta.tin_last4.get(&id).cloned(),
                        id,
                        ext_id: c.id,
                        name: c.name,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// What recognition decided for one file.
pub struct Outcome {
    /// The name to deliver it under (the original's when the client is not sure).
    pub file: String,
    pub hint: Hint,
    /// For the Intake strip.
    pub note: String,
    pub proposal: Proposal,
}

/// Reads `text` (page 1) and decides the file's name and hint. `None`: not a form.
pub fn decide(root: &Path, lookup: &Lookup, text: &str, original: &str) -> Option<Outcome> {
    decide_extract(root, lookup, vf_extract::recognize(text)?, original)
}

/// [`decide`] for what [`form_read`] read (a fillable form's fields included).
pub fn decide_read(root: &Path, lookup: &Lookup, r: &FormRead, original: &str) -> Option<Outcome> {
    decide_extract(
        root,
        lookup,
        vf_extract::recognize_filled(&r.text, &r.fields)?,
        original,
    )
}

fn decide_extract(
    root: &Path,
    lookup: &Lookup,
    e: vf_extract::Extract,
    original: &str,
) -> Option<Outcome> {
    let clients = client_refs(root);
    let aliases: Vec<matching::Alias> = vf_fs::aliases::load(root)
        .into_iter()
        .map(|a| matching::Alias {
            payer: a.payer,
            client_id: a.client_id,
        })
        .collect();
    let p = matching::propose(&e, &clients, &aliases);
    let mut hint = Hint::new("", original, "recognized", "");
    hint.tax_year = e.tax_year;
    hint.doc_type = lookup
        .doc_types
        .iter()
        .find(|(_, n)| n.as_str() == p.doc_type)
        .map(|(id, _)| id.clone());
    hint.source = Some("third_party".into());
    hint.extract = Some(Recognized {
        form: e.form.key().into(),
        payer_name: e.payer_name.clone(),
        payer_tin4: e.payer_tin4.as_ref().map(|l| l.as_str().to_string()),
        recipient_name: e.recipient_name.clone(),
        recipient_tin4: e.recipient_tin4.as_ref().map(|l| l.as_str().to_string()),
        proposed_client: p.primary.as_ref().map(|m| m.client_id.clone()),
        confidence: p
            .primary
            .as_ref()
            .map_or(0, |m| (m.confidence * 100.0).round() as u8),
        copies: p.copies.iter().map(|m| m.client_id.clone()).collect(),
    });
    let what = format!(
        "{}{}",
        p.doc_type,
        e.tax_year.map(|y| format!(" {y}")).unwrap_or_default()
    );
    let (file, note) = match &p.primary {
        Some(m) if p.confident() => {
            let rest = [
                e.tax_year.map(|y| y.to_string()),
                Some(p.doc_type.to_string()),
                e.payer_name.clone(),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
            let mut note = format!(
                "recognized {what} for {} ({}, {:.0}%)",
                m.name,
                m.why,
                m.confidence * 100.0
            );
            for c in &p.copies {
                note.push_str(&format!(" · also for {} ({})", c.name, c.why));
            }
            (
                format!("{}_{}_{}.pdf", clean(&m.name), m.ext_id, clean(&rest)),
                note,
            )
        }
        Some(m) => (
            original.to_string(),
            format!(
                "recognized {what}; client not sure ({} for {}, {:.0}%): choose it in the preview",
                m.why,
                m.name,
                m.confidence * 100.0
            ),
        ),
        None => (
            original.to_string(),
            format!("recognized {what}; no client matched: choose it in the preview"),
        ),
    };
    Some(Outcome {
        file,
        hint,
        note,
        proposal: p,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cabinet() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        std::fs::create_dir_all(root.join("_Vibe")).unwrap();
        vf_fs::doctypes::seed_if_missing(root).unwrap();
        let csv = root.join("clients.csv");
        let mut text = String::from("ID,Name,TIN\n");
        for (ext, name, tin) in vf_extract::synthetic::CLIENTS {
            text.push_str(&format!("{ext},\"{name}\",{tin}\n"));
        }
        std::fs::write(&csv, text).unwrap();
        let map = filer_core::config::ColumnMap {
            id: Some("ID".into()),
            name: Some("Name".into()),
            extras: vec![],
        };
        vf_fs::clients::import_csv_tin(root, &csv, &map, &Default::default(), Some("TIN")).unwrap();
        t
    }

    #[test]
    fn a_sure_match_is_named_for_its_client_and_a_k1_names_both() {
        let t = cabinet();
        let lookup = Lookup::load(t.path()).unwrap();
        let cases = vf_extract::synthetic::cases();
        let w2 = cases
            .iter()
            .find(|c| c.name.starts_with("w2 2025 REYE5521"))
            .unwrap();
        let o = decide(t.path(), &lookup, &w2.text, "scan0001.pdf").unwrap();
        assert!(
            o.file.starts_with("Reyes, Dana & Lee_REYE5521_2025 W-2"),
            "{}",
            o.file
        );
        assert!(o.note.contains("TIN and name"), "{}", o.note);
        assert!(o.hint.doc_type.is_some() && o.hint.tax_year == Some(2025));
        let k1 = cases
            .iter()
            .find(|c| c.name.starts_with("k1-1120s"))
            .unwrap();
        let o = decide(t.path(), &lookup, &k1.text, "scan0002.pdf").unwrap();
        assert!(o.file.contains("_BELL2002_"), "{}", o.file);
        assert!(o.note.contains("also for Wagner, Pat"), "{}", o.note);
        assert_eq!(o.hint.extract.as_ref().unwrap().copies.len(), 1);
        // Not a form: nothing to decide.
        assert!(decide(t.path(), &lookup, "Engagement letter", "x.pdf").is_none());
    }

    #[test]
    fn an_unsure_match_keeps_its_name() {
        let t = cabinet();
        let lookup = Lookup::load(t.path()).unwrap();
        let text = "Form 1099-INT Interest Income\n2025\nRECIPIENT'S name\nPat Wagner";
        let o = decide(t.path(), &lookup, text, "scan0003.pdf").unwrap();
        assert_eq!(o.file, "scan0003.pdf");
        assert!(o.note.contains("client not sure"), "{}", o.note);
        assert_eq!(o.hint.tax_year, Some(2025), "the year is still hinted");
    }
}
