//! `<data>/config.toml`: per-workstation settings.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const CONFIG_FILE: &str = "config.toml";
/// P00 kept the device id alone in this file; [`load`] moves it into `config.toml`.
const LEGACY_DEVICE_ID: &str = "device_id";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// This workstation's id (uuid v7). Every changeable catalog row carries it.
    pub device_id: String,
    /// The cabinet opened last; the wizard offers it first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_cabinet: Option<PathBuf>,
}

/// Reads `<data>/config.toml`, creating `<data>` and the file on first run.
pub fn load(data: &Path) -> Result<Config> {
    let p = data.join(CONFIG_FILE);
    match std::fs::read_to_string(&p) {
        Ok(text) => toml::from_str(&text).with_context(|| format!("reading {}", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(data)
                .with_context(|| format!("creating {}", data.display()))?;
            let legacy = data.join(LEGACY_DEVICE_ID);
            let device_id = std::fs::read_to_string(&legacy)
                .map(|s| s.trim().to_owned())
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(vf_core::new_id);
            let c = Config {
                device_id,
                last_cabinet: None,
            };
            save(data, &c)?;
            let _ = std::fs::remove_file(legacy);
            Ok(c)
        }
        Err(e) => Err(e).with_context(|| format!("reading {}", p.display())),
    }
}

/// Writes `config.toml` through a temp file so a crash never leaves it half-written.
pub fn save(data: &Path, c: &Config) -> Result<()> {
    let p = data.join(CONFIG_FILE);
    let tmp = data.join(format!("{CONFIG_FILE}.tmp"));
    std::fs::write(&tmp, toml::to_string(c).expect("serializes"))
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &p).with_context(|| format!("writing {}", p.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_load_creates_and_keeps_the_device_id() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let a = load(&data).unwrap();
        assert!(data.join(CONFIG_FILE).is_file());
        assert_eq!(load(&data).unwrap(), a);
    }

    #[test]
    fn migrates_the_p00_device_id_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(LEGACY_DEVICE_ID), "0192-legacy\n").unwrap();
        assert_eq!(load(tmp.path()).unwrap().device_id, "0192-legacy");
        assert!(!tmp.path().join(LEGACY_DEVICE_ID).exists());
    }

    #[test]
    fn last_cabinet_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let mut c = load(tmp.path()).unwrap();
        c.last_cabinet = Some(PathBuf::from(r"\\server\share\Clients"));
        save(tmp.path(), &c).unwrap();
        assert_eq!(load(tmp.path()).unwrap(), c);
    }
}
