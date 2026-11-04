//! Ten-key tapes kept per document (PHASES PA-14 step 6): `<cabinet>/_Vibe/tapes.jsonl`, one
//! JSON line per saved tape. A document's history is found by its catalog id or, after a
//! rebuild gave it a new one, by its path, so the file on disk is all a rebuild needs
//! (principle 2). Tapes hold only what the person picked or typed.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use vf_core::tape::Line;

pub const FILE: &str = "tapes.jsonl";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    pub doc_id: String,
    /// Cabinet-relative path of the document when the tape was saved.
    pub rel_path: String,
    /// RFC 3339 UTC.
    pub saved_at: String,
    /// Initials.
    pub by: String,
    pub name: String,
    pub lines: Vec<Line>,
}

pub fn path(root: &Path) -> PathBuf {
    root.join(crate::cabinet::VIBE_DIR).join(FILE)
}

/// Appends a saved tape.
pub fn save(root: &Path, t: &Saved) -> Result<(), String> {
    let p = path(root);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let line = serde_json::to_string(t).map_err(|e| e.to_string())?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
        .map_err(|e| format!("{}: {e}", p.display()))?;
    writeln!(f, "{line}").map_err(|e| format!("{}: {e}", p.display()))?;
    f.sync_all().map_err(|e| e.to_string())
}

/// The tapes saved on a document, newest first.
pub fn history(root: &Path, doc_id: &str, rel_path: &str) -> Vec<Saved> {
    let Ok(text) = std::fs::read_to_string(path(root)) else {
        return vec![];
    };
    let mut v: Vec<Saved> = text
        .lines()
        .filter_map(|l| serde_json::from_str::<Saved>(l).ok())
        .filter(|t| t.doc_id == doc_id || t.rel_path.eq_ignore_ascii_case(rel_path))
        .collect();
    v.sort_by(|a, b| b.saved_at.cmp(&a.saved_at));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Saved tapes come back newest first, by id or (after a rebuild) by path.
    #[test]
    fn history_by_id_or_path() {
        let t = tempfile::tempdir().unwrap();
        let mut tape = vf_core::tape::Tape::new("Interest");
        tape.keys("1284.17 + 45 + T");
        let rec = |at: &str, id: &str| Saved {
            doc_id: id.into(),
            rel_path: "Invented Client (INVE1001)/Income Tax/2025/1099-INT.pdf".into(),
            saved_at: at.into(),
            by: "KWK".into(),
            name: "Interest".into(),
            lines: tape.lines().to_vec(),
        };
        save(t.path(), &rec("2026-11-04T10:00:00Z", "d1")).unwrap();
        save(t.path(), &rec("2026-11-04T11:00:00Z", "d1")).unwrap();
        let h = history(t.path(), "d1", "x");
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].saved_at, "2026-11-04T11:00:00Z");
        // A rebuild gave the document a new id: found by its path.
        let h = history(
            t.path(),
            "new-id",
            "invented client (INVE1001)/Income Tax/2025/1099-INT.pdf",
        );
        assert_eq!(h.len(), 2);
        let back = vf_core::tape::Tape::from_lines("Interest", h[0].lines.clone());
        assert_eq!(back.grand(), 132917);
        assert!(history(t.path(), "other", "other.pdf").is_empty());
    }
}
