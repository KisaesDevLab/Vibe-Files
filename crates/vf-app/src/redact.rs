//! Redacting a document (P04 step 5): `vf_pdf::redact` makes the file, the verify step must
//! pass, and the result is filed as a new document `<name> (redacted).pdf` beside the
//! original (which is untouched), tagged with the original's id. The audit row records how
//! many of each pattern were removed.

use std::collections::BTreeMap;
use std::path::Path;

use pdfium_render::prelude::Pdfium;
use vf_core::db::Connection;
use vf_fs::content::{self, Added, NewDoc};
use vf_pdf::pii::{Found, Kind};
use vf_pdf::redact::{self, Redaction};

/// What to remove: pattern matches (counted by kind in the audit) and boxes drawn by hand.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub found: Vec<Found>,
    pub drawn: Vec<Redaction>,
}

impl Plan {
    pub fn boxes(&self) -> Vec<Redaction> {
        self.found
            .iter()
            .flat_map(|f| {
                f.rects.iter().map(|r| Redaction {
                    page: f.page,
                    rect: *r,
                })
            })
            .chain(self.drawn.iter().copied())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.found.len() + self.drawn.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// `{"ssn": 3, "ein": 1, "drawn": 2}`.
    pub fn counts(&self) -> BTreeMap<&'static str, usize> {
        let mut c = BTreeMap::new();
        for f in &self.found {
            *c.entry(f.kind.key()).or_insert(0) += 1;
        }
        if !self.drawn.is_empty() {
            c.insert("drawn", self.drawn.len());
        }
        c
    }
}

/// Redacts document `doc_id` per `plan` and files the copy. Refuses (and files nothing) when
/// the verify step still finds a removed value.
pub fn redact_document(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    pdfium: &Pdfium,
    doc_id: &str,
    plan: &Plan,
) -> Result<Added, String> {
    if plan.is_empty() {
        return Err("Nothing to redact.".into());
    }
    let (rel, client_id, doc_type_id, tax_year, source): (
        String,
        Option<String>,
        Option<String>,
        Option<i32>,
        Option<String>,
    ) = conn
        .query_row(
            "SELECT rel_path, client_id, doc_type_id, tax_year, source FROM document
             WHERE id = ?1 AND deleted_at IS NULL",
            [doc_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .map_err(|_| "the document is not in the catalog".to_string())?;
    let tmp = content::tmp_file(root, "pdf")?;
    let r = redact::apply(pdfium, &root.join(&rel), &tmp, &plan.boxes()).and_then(|_| {
        let values: Vec<String> = plan.found.iter().map(|f| f.value.clone()).collect();
        redact::verify(pdfium, &tmp, &values)
            .map_err(|e| format!("Not saved: the check after redaction found {e}."))
    });
    if let Err(e) = r {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    let name = redacted_name(&rel);
    content::add(
        conn,
        root,
        device_id,
        &tmp,
        NewDoc {
            rel_path: name,
            client_id,
            doc_type_id,
            tax_year,
            source,
            tags: vec![format!("redacted-from:{doc_id}")],
            label: "redact".into(),
            detail: serde_json::json!({
                "from": doc_id,
                "patterns": plan.counts(),
                "boxes": plan.boxes().len(),
                "verified": true,
            }),
        },
    )
}

/// `Folder/W-2.pdf` → `Folder/W-2 (redacted).pdf`.
pub fn redacted_name(rel: &str) -> String {
    match rel.rsplit_once('.') {
        Some((stem, ext)) if !stem.ends_with('/') => format!("{stem} (redacted).{ext}"),
        _ => format!("{rel} (redacted)"),
    }
}

/// Default selection: SSNs, account numbers and dates of birth on; EINs off (usually the
/// payer's, kept: UI_SPEC §4).
pub fn default_on(kind: Kind) -> bool {
    kind != Kind::Ein
}

#[cfg(test)]
mod tests {
    use vf_core::db::Storage;

    use super::*;

    fn pdfium() -> &'static Pdfium {
        let dir = filer_pdf::pdf::find_pdfium(Path::new("")).unwrap();
        filer_pdf::pdf::pdfium(&dir).unwrap()
    }

    /// PHASES P04 done-when (second half): after redaction text extraction finds none, and the
    /// audit row records the pattern counts.
    #[test]
    fn redacted_copy_is_filed_with_pattern_counts() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("cab");
        std::fs::create_dir_all(root.join("A (1)/2025")).unwrap();
        let mut conn = vf_core::db::open(&t.path().join("c.db"), Storage::Local).unwrap();
        let rel = "A (1)/2025/W-2.pdf";
        vf_fixtures::write_pdf(
            &root.join(rel),
            &[
                "W-2 - tax year 2025".into(),
                "Payer EIN: 07-1234567".into(),
                "Recipient SSN: 987-65-4321".into(),
                "Spouse SSN: 987-65-4322".into(),
            ],
        )
        .unwrap();
        let hash = filer_intake::commit::sha256_file(&root.join(rel)).unwrap();
        conn.execute(
            "INSERT INTO document (id, tax_year, rel_path, hash, size, filed_at, device_id, updated_at)
             VALUES ('d1', 2025, ?1, ?2, 1, 't', 'dev', 't')",
            (rel, &hash),
        )
        .unwrap();
        let found = redact::find(pdfium(), &root.join(rel), &Kind::ALL).unwrap();
        let plan = Plan {
            found: found.into_iter().filter(|f| default_on(f.kind)).collect(),
            drawn: vec![],
        };
        assert_eq!(plan.counts().get("ssn"), Some(&2));
        let added = redact_document(&mut conn, &root, "dev", pdfium(), "d1", &plan).unwrap();
        assert_eq!(added.rel_path, "A (1)/2025/W-2 (redacted).pdf");

        let doc = pdfium()
            .load_pdf_from_file(&root.join(&added.rel_path), None)
            .unwrap();
        let text = vf_pdf::text::page_text(&doc.pages().first().unwrap());
        assert!(!text.contains("987-65-432"), "{text}");
        assert!(text.contains("07-1234567"), "EIN kept by default: {text}");
        let detail: String = conn
            .query_row(
                "SELECT detail FROM audit WHERE action = 'redact' AND doc_id = ?1",
                [&added.doc_id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            detail.contains("\"ssn\":2") && detail.contains("\"verified\":true"),
            "{detail}"
        );
        assert!(!detail.contains("4321"), "no value in the audit: {detail}");
        let tag: String = conn
            .query_row(
                "SELECT tag FROM document_tag WHERE doc_id = ?1",
                [&added.doc_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tag, "redacted-from:d1");
        assert_eq!(
            filer_intake::commit::sha256_file(&root.join(rel)).unwrap(),
            hash,
            "original untouched"
        );
    }

    #[test]
    fn names() {
        assert_eq!(
            redacted_name("A/B/W-2 2025.pdf"),
            "A/B/W-2 2025 (redacted).pdf"
        );
        assert_eq!(redacted_name("x.pdf"), "x (redacted).pdf");
    }
}
