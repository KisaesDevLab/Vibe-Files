//! What Vibe Files' own intake processors (P02 steps 3–5: barcode cover sheets, image→PDF,
//! print capture) know about a file they hand to Filer's inbox that Filer's batch record does
//! not carry: doc type, tax year, source. `<cabinet>/_Vibe/ingest_hints.jsonl`, append-only,
//! keyed by the SHA-256 of the file as delivered (Filer moves files without changing them, so
//! the filed copy has the same hash). [`crate::intake`] applies a hint when it records the
//! document. A file Filer changes before filing (merge, split) loses its hint.
//!
//! Same single-writer rule as Filer's other files in `_Vibe/` (QUESTIONS Q-19).

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cabinet::VIBE_DIR;

pub const FILE: &str = "ingest_hints.jsonl";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hint {
    pub v: u32,
    pub ts: String,
    pub device: String,
    pub sha256: String,
    /// The file name as delivered to Filer's inbox (for people reading the file).
    pub name: String,
    /// Doc type id.
    #[serde(default)]
    pub doc_type: Option<String>,
    /// A tax year; with `permanent` false and no year, the year folder decides.
    #[serde(default)]
    pub tax_year: Option<i32>,
    #[serde(default)]
    pub permanent: bool,
    /// `client` / `firm` / `government` / `third_party`.
    #[serde(default)]
    pub source: Option<String>,
    /// `barcode`, `image`, `print_capture:<profile>`, `recognized`.
    pub origin: String,
    /// What a form recognizer read (P08), when one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extract: Option<Recognized>,
}

/// A recognized form, as the pipeline saw it (TINs last 4 only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recognized {
    /// `w2`, `1099-int`, `k1-1065`, …
    pub form: String,
    #[serde(default)]
    pub payer_name: Option<String>,
    #[serde(default)]
    pub payer_tin4: Option<String>,
    #[serde(default)]
    pub recipient_name: Option<String>,
    #[serde(default)]
    pub recipient_tin4: Option<String>,
    /// Client uuid proposed, and how sure (percent).
    #[serde(default)]
    pub proposed_client: Option<String>,
    #[serde(default)]
    pub confidence: u8,
    /// Client uuids proposed for copies (a K-1's partner).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub copies: Vec<String>,
}

impl Hint {
    pub fn new(sha256: &str, name: &str, origin: &str, device: &str) -> Self {
        Hint {
            v: 1,
            ts: vf_core::now_rfc3339(),
            device: device.into(),
            sha256: sha256.into(),
            name: name.into(),
            doc_type: None,
            tax_year: None,
            permanent: false,
            source: None,
            origin: origin.into(),
            extract: None,
        }
    }

    /// Says something beyond where it came from.
    pub fn is_useful(&self) -> bool {
        self.doc_type.is_some()
            || self.tax_year.is_some()
            || self.permanent
            || self.source.is_some()
            || self.extract.is_some()
    }
}

pub fn path(root: &Path) -> PathBuf {
    root.join(VIBE_DIR).join(FILE)
}

pub fn append(root: &Path, hint: &Hint) -> Result<(), String> {
    let p = path(root);
    let line = serde_json::to_string(hint).expect("serializes");
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
        .map_err(|e| format!("cannot open {}: {e}", p.display()))?;
    f.write_all(format!("{line}\n").as_bytes())
        .and_then(|_| f.sync_all())
        .map_err(|e| format!("cannot write {}: {e}", p.display()))
}

/// Every hint by hash; a later line for the same hash wins. Unreadable lines are skipped.
pub fn load(root: &Path) -> HashMap<String, Hint> {
    std::fs::read_to_string(path(root))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Hint>(l).ok())
        .map(|h| (h.sha256.clone(), h))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn later_hint_wins_and_bad_lines_are_skipped() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join(VIBE_DIR)).unwrap();
        let mut h = Hint::new("abc", "x.pdf", "barcode", "dev");
        assert!(!h.is_useful());
        h.tax_year = Some(2024);
        append(t.path(), &h).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(path(t.path()))
            .unwrap()
            .write_all(b"{not json\n")
            .unwrap();
        h.tax_year = Some(2025);
        append(t.path(), &h).unwrap();
        let all = load(t.path());
        assert_eq!(all.len(), 1);
        assert_eq!(all["abc"].tax_year, Some(2025));
    }
}
