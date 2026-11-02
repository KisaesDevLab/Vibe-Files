//! `<data>/config.toml`: per-workstation settings (CLAUDE.md §Layout on disk).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const CONFIG_FILE: &str = "config.toml";
/// Pre-P01 home of the device id; moved into `config.toml` on first load.
const LEGACY_DEVICE_ID: &str = "device_id";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// This workstation (uuid v7). Every catalog row it changes carries it.
    pub device_id: String,
    /// Shown in Sync and conflict-copy names (QUESTIONS Q-14).
    pub device_name: String,
    /// The cabinet this data folder's catalog belongs to.
    #[serde(default)]
    pub cabinet: Option<PathBuf>,
    /// Folder a PDF printer saves into (P02 step 5); `None` = `<data>/Print capture`.
    #[serde(default)]
    pub print_capture: Option<PathBuf>,
}

/// The print-capture folder in effect.
pub fn print_capture(data: &Path, cfg: &Config) -> PathBuf {
    cfg.print_capture
        .clone()
        .unwrap_or_else(|| data.join("Print capture"))
}

pub fn path(data: &Path) -> PathBuf {
    data.join(CONFIG_FILE)
}

fn computer_name() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "this PC".into())
}

impl Config {
    /// Loads `config.toml`, creating it (and `data`) on first run. A device id from the
    /// pre-P01 `device_id` file is carried over and that file removed.
    pub fn load_or_create(data: &Path) -> Result<Config> {
        let p = path(data);
        if let Ok(text) = std::fs::read_to_string(&p) {
            return toml::from_str(&text).with_context(|| format!("{} is not valid", p.display()));
        }
        std::fs::create_dir_all(data).with_context(|| format!("creating {}", data.display()))?;
        let legacy = data.join(LEGACY_DEVICE_ID);
        let device_id = std::fs::read_to_string(&legacy)
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(vf_core::new_id);
        let cfg = Config {
            device_id,
            device_name: computer_name(),
            cabinet: None,
            print_capture: None,
        };
        cfg.save(data)?;
        let _ = std::fs::remove_file(legacy);
        Ok(cfg)
    }

    /// Atomic replace (write a temp file, then rename over).
    pub fn save(&self, data: &Path) -> Result<()> {
        let p = path(data);
        let tmp = p.with_extension("toml.tmp");
        std::fs::write(&tmp, toml::to_string_pretty(self)?)
            .with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &p).with_context(|| format!("replacing {}", p.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_load_creates_and_keeps_the_device_id() {
        let tmp = tempfile::tempdir().unwrap();
        let a = Config::load_or_create(tmp.path()).unwrap();
        let b = Config::load_or_create(tmp.path()).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.cabinet, None);
    }

    #[test]
    fn legacy_device_id_file_moves_into_config() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(LEGACY_DEVICE_ID), "dev-old\n").unwrap();
        let c = Config::load_or_create(tmp.path()).unwrap();
        assert_eq!(c.device_id, "dev-old");
        assert!(!tmp.path().join(LEGACY_DEVICE_ID).exists());
        let mut c = c;
        c.cabinet = Some(r"D:\Cabinet".into());
        c.save(tmp.path()).unwrap();
        assert_eq!(Config::load_or_create(tmp.path()).unwrap(), c);
    }
}
