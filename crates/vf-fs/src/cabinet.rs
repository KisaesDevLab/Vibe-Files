//! `<cabinet>/_Vibe/cabinet.json` and the on-disk names under `_Vibe/`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const VIBE_DIR: &str = "_Vibe";
pub const CABINET_FILE: &str = "cabinet.json";
pub const VERSIONS_DIR: &str = "versions";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CabinetFile {
    pub cabinet_id: String,
    /// = SentinelV1 `firm_id` of every client folder in this cabinet.
    pub firm_id: String,
    pub schema_version: u32,
    /// `docs/BUCKET_LAYOUT.md` version.
    pub layout_version: u32,
    pub created: String,
}

pub fn cabinet_file(root: &Path) -> PathBuf {
    root.join(VIBE_DIR).join(CABINET_FILE)
}

pub fn read(root: &Path) -> Result<CabinetFile, String> {
    let p = cabinet_file(root);
    let text = std::fs::read_to_string(&p).map_err(|e| {
        format!(
            "{} is not a Vibe Files cabinet (cannot read {}: {e})",
            root.display(),
            p.display()
        )
    })?;
    serde_json::from_str(&text).map_err(|e| format!("{} is not valid: {e}", p.display()))
}

/// Writes a new cabinet.json; refuses to replace one.
pub fn create(root: &Path, file: &CabinetFile) -> Result<(), String> {
    use std::io::Write;
    let dir = root.join(VIBE_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let p = cabinet_file(root);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&p)
        .map_err(|e| format!("cannot create {}: {e}", p.display()))?;
    f.write_all(&serde_json::to_vec_pretty(file).expect("serializes"))
        .and_then(|()| f.sync_all())
        .map_err(|e| format!("cannot write {}: {e}", p.display()))
}

/// One entry of a version sidecar `_Vibe/versions/<aa>/<hash>.json` (a JSON array): which
/// document path held this content before it was replaced. Lets `rebuild` restore version rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionSidecar {
    /// Cabinet-relative path (forward slashes) of the document this was a version of.
    pub rel_path: String,
    pub created: String,
    pub created_by: Option<String>,
    pub device_id: String,
    pub reason: Option<String>,
}

pub fn version_path(root: &Path, hash: &str) -> PathBuf {
    root.join(VIBE_DIR)
        .join(VERSIONS_DIR)
        .join(&hash[..2])
        .join(hash)
}
