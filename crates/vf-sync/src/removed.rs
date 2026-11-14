//! Documents taken out of B2 (owner 2026-11-14, Q-104, Q-106): the bucket only. The file
//! stays in the cabinet on every computer, and no rule sends it again; pushing or sharing it
//! by hand puts it back.
//!
//! The list is in the cabinet, `_Vibe/b2_removed.json`, not in a catalog: a session host's
//! catalog is new every morning, and every computer that pushes must leave these out.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const FILE: &str = "b2_removed.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Catalog id (the same on every computer through the client folder's records).
    pub id: String,
    /// Where it was when it was taken out (for a person reading the file).
    pub rel_path: String,
    pub at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct FileV1 {
    version: u32,
    docs: Vec<Entry>,
}

fn path(root: &Path) -> PathBuf {
    root.join(vf_fs::cabinet::VIBE_DIR).join(FILE)
}

/// The list. A missing file is an empty list; one that cannot be read is an error (a push
/// then stops, rather than send what was taken out).
pub fn load(root: &Path) -> Result<Vec<Entry>, String> {
    match std::fs::read_to_string(path(root)) {
        Ok(t) => serde_json::from_str::<FileV1>(&t)
            .map(|f| f.docs)
            .map_err(|e| format!("{FILE} could not be read: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(format!("{FILE} could not be read: {e}")),
    }
}

pub fn ids(root: &Path) -> Result<HashSet<String>, String> {
    Ok(load(root)?.into_iter().map(|e| e.id).collect())
}

fn save(root: &Path, docs: Vec<Entry>) -> Result<(), String> {
    vf_fs::cabinet::write_replace(
        &path(root),
        &serde_json::to_vec_pretty(&FileV1 { version: 1, docs }).expect("serializes"),
    )
}

/// Adds documents (id, cabinet path) to the list.
pub fn add(root: &Path, docs: &[(String, String)]) -> Result<(), String> {
    vf_fs::lock::with(
        root,
        "b2-removed",
        "the list of documents taken out of B2",
        || {
            let mut all = load(root)?;
            let now = vf_core::now_rfc3339();
            let by = vf_core::windows_login();
            for (id, rel) in docs {
                if !all.iter().any(|e| &e.id == id) {
                    all.push(Entry {
                        id: id.clone(),
                        rel_path: rel.clone(),
                        at: now.clone(),
                        by: by.clone(),
                    });
                }
            }
            save(root, all)
        },
    )
}

/// Takes documents off the list (they are pushed by hand again). Returns how many were on it.
pub fn clear(root: &Path, ids: &HashSet<String>) -> Result<usize, String> {
    if !path(root).is_file() {
        return Ok(0);
    }
    vf_fs::lock::with(
        root,
        "b2-removed",
        "the list of documents taken out of B2",
        || {
            let all = load(root)?;
            let before = all.len();
            let kept: Vec<Entry> = all.into_iter().filter(|e| !ids.contains(&e.id)).collect();
            let n = before - kept.len();
            if n > 0 {
                save(root, kept)?;
            }
            Ok(n)
        },
    )
}
