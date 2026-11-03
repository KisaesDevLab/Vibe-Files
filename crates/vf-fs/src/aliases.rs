//! Learned corrections for form matching (PHASES P08 step 4): "forms from this payer belong to
//! this client". Kept in the cabinet (`_Vibe/extract_aliases.json`) so every workstation
//! learns from a correction made on any of them. Written when Intake files a recognized form to
//! a different client than proposed.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cabinet::VIBE_DIR;

pub const FILE: &str = "extract_aliases.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Alias {
    /// The payer name as printed on the form.
    pub payer: String,
    /// Client uuid it belongs to.
    pub client_id: String,
    pub learned: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct FileV1 {
    version: u32,
    aliases: Vec<Alias>,
}

pub fn path(root: &Path) -> PathBuf {
    root.join(VIBE_DIR).join(FILE)
}

fn key(payer: &str) -> String {
    payer
        .to_uppercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn load(root: &Path) -> Vec<Alias> {
    std::fs::read_to_string(path(root))
        .ok()
        .and_then(|t| serde_json::from_str::<FileV1>(&t).ok())
        .map(|f| f.aliases)
        .unwrap_or_default()
}

/// Records that `payer`'s forms belong to `client_id` (replacing what was learned for it).
pub fn learn(root: &Path, payer: &str, client_id: &str) -> Result<(), String> {
    let payer = payer.trim();
    if payer.is_empty() {
        return Ok(());
    }
    let mut all = load(root);
    all.retain(|a| key(&a.payer) != key(payer));
    all.push(Alias {
        payer: payer.into(),
        client_id: client_id.into(),
        learned: vf_core::now_rfc3339(),
    });
    let f = FileV1 {
        version: 1,
        aliases: all,
    };
    crate::cabinet::write_replace(
        &path(root),
        &serde_json::to_vec_pretty(&f).expect("serializes"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learning_replaces_by_payer() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join(VIBE_DIR)).unwrap();
        learn(t.path(), "Summit Brokerage LLC", "c1").unwrap();
        learn(t.path(), "SUMMIT BROKERAGE, LLC", "c2").unwrap();
        learn(t.path(), " ", "c3").unwrap();
        let all = load(t.path());
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].client_id, "c2");
    }
}
