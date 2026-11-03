//! `<cabinet>/_Vibe/cabinet.json` and the on-disk names under `_Vibe/`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const VIBE_DIR: &str = "_Vibe";
pub const CABINET_FILE: &str = "cabinet.json";
pub const VERSIONS_DIR: &str = "versions";
pub const KEYS_DIR: &str = "keys";
pub const RECOVERY_KEY_FILE: &str = "recovery.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CabinetFile {
    pub cabinet_id: String,
    /// = SentinelV1 `firm_id` of every client folder in this cabinet.
    pub firm_id: String,
    pub schema_version: u32,
    /// `docs/BUCKET_LAYOUT.md` version.
    pub layout_version: u32,
    pub created: String,
    /// Display name chosen in the wizard. Absent in cabinets made by the P00 CLI/fixtures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl CabinetFile {
    /// The wizard name, else the root folder's name.
    pub fn display_name(&self, root: &Path) -> String {
        self.name.clone().unwrap_or_else(|| {
            root.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| root.display().to_string())
        })
    }
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
    write_new(
        &cabinet_file(root),
        &serde_json::to_vec_pretty(file).expect("serializes"),
    )
}

/// `<cabinet>/_Vibe/keys/recovery.json`: the catalog key wrapped by the recovery passphrase
/// (`vf_security::keys::WrappedKey`). Kept in the cabinet so every workstation can open it.
pub fn recovery_key_file(root: &Path) -> PathBuf {
    root.join(VIBE_DIR).join(KEYS_DIR).join(RECOVERY_KEY_FILE)
}

/// Writes `bytes` to a new file (creating parent folders), flushed to disk. Never replaces.
pub fn write_new(p: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(p)
        .map_err(|e| format!("cannot create {}: {e}", p.display()))?;
    f.write_all(bytes)
        .and_then(|()| f.sync_all())
        .map_err(|e| format!("cannot write {}: {e}", p.display()))
}

/// The `firm_id` of the first readable client sentinel directly under `root` (folders in name
/// order), so a cabinet created over an existing Filer Desktop tree keeps the firm id its
/// sentinels already carry. `None` when there are no sentinels.
pub fn discover_firm_id(root: &Path) -> Option<String> {
    let mut tops: Vec<PathBuf> = std::fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter(|e| !e.file_name().to_string_lossy().starts_with(['_', '.', '~']))
        .map(|e| e.path())
        .collect();
    tops.sort();
    tops.iter().find_map(|f| {
        filer_intake::sentinel::read_sentinel(f)
            .ok()
            .flatten()
            .map(|s| s.firm_id.to_string())
    })
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use filer_core::clients::Client;
    use filer_intake::sentinel;
    use uuid::Uuid;

    use super::*;

    fn bind(root: &Path, name: &str, firm: Uuid) {
        let folder = root.join(name);
        std::fs::create_dir(&folder).unwrap();
        let c = Client {
            id: name.into(),
            uuid: Uuid::new_v4(),
            name: name.into(),
            extra: BTreeMap::new(),
        };
        sentinel::write_sentinel(root, &folder, &c, firm).unwrap();
    }

    #[test]
    fn firm_id_comes_from_the_first_sentinel() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(discover_firm_id(tmp.path()), None);
        std::fs::create_dir(tmp.path().join("Aaa no sentinel")).unwrap();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        bind(tmp.path(), "Ccc (2)", b);
        bind(tmp.path(), "Bbb (1)", a);
        assert_eq!(discover_firm_id(tmp.path()), Some(a.to_string()));
    }

    #[test]
    fn write_new_never_replaces() {
        let tmp = tempfile::tempdir().unwrap();
        let p = recovery_key_file(tmp.path());
        write_new(&p, b"one").unwrap();
        assert!(write_new(&p, b"two").is_err());
        assert_eq!(std::fs::read(&p).unwrap(), b"one");
    }

    #[test]
    fn name_is_optional_on_disk() {
        let old = r#"{"cabinet_id":"c","firm_id":"f","schema_version":1,"layout_version":1,"created":"t"}"#;
        let f: CabinetFile = serde_json::from_str(old).unwrap();
        assert_eq!(f.name, None);
        assert_eq!(f.display_name(Path::new("C:/Firm/Clients")), "Clients");
    }
}
