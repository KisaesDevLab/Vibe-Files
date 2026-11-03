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
    /// Browse views kept on this PC until users exist (QUESTIONS Q-23; per-user rows in P06).
    #[serde(default)]
    pub prefs: Prefs,
}

/// Pinned documents, recent documents and favorite places in the tree.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prefs {
    /// Document ids.
    #[serde(default)]
    pub pinned: Vec<String>,
    /// Document ids, newest first, at most [`RECENT_MAX`].
    #[serde(default)]
    pub recent: Vec<String>,
    #[serde(default)]
    pub favorites: Vec<Favorite>,
}

pub const RECENT_MAX: usize = 50;

/// A place in the Browse tree. `client` is the client uuid; `year: None` is Permanent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Favorite {
    Client {
        client: String,
    },
    Year {
        client: String,
        year: Option<i32>,
    },
    Folder {
        client: String,
        year: Option<i32>,
        folder: String,
    },
}

impl Prefs {
    /// Puts `ids` at the front of Recent (newest first, no repeats, capped).
    pub fn touch(&mut self, ids: &[String]) {
        self.recent.retain(|r| !ids.contains(r));
        let mut v: Vec<String> = ids.iter().take(RECENT_MAX).cloned().collect();
        v.append(&mut self.recent);
        v.truncate(RECENT_MAX);
        self.recent = v;
    }

    /// Pins `ids`, or unpins them when all are pinned already. Returns whether they are pinned now.
    pub fn toggle_pins(&mut self, ids: &[String]) -> bool {
        if ids.iter().all(|i| self.pinned.contains(i)) {
            self.pinned.retain(|p| !ids.contains(p));
            false
        } else {
            for i in ids {
                if !self.pinned.contains(i) {
                    self.pinned.push(i.clone());
                }
            }
            true
        }
    }
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
                prefs: Prefs::default(),
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
    fn prefs_round_trip_and_rules() {
        let tmp = tempfile::tempdir().unwrap();
        let mut c = load(tmp.path()).unwrap();
        c.prefs.favorites = vec![
            Favorite::Client {
                client: "c1".into(),
            },
            Favorite::Year {
                client: "c1".into(),
                year: None,
            },
            Favorite::Folder {
                client: "c1".into(),
                year: Some(2025),
                folder: "Income Tax".into(),
            },
        ];
        assert!(c.prefs.toggle_pins(&["d1".into(), "d2".into()]));
        assert!(
            c.prefs.toggle_pins(&["d2".into(), "d3".into()]),
            "not all pinned: pin"
        );
        assert_eq!(c.prefs.pinned, ["d1", "d2", "d3"]);
        assert!(
            !c.prefs.toggle_pins(&["d1".into(), "d3".into()]),
            "all pinned: unpin"
        );
        assert_eq!(c.prefs.pinned, ["d2"]);
        c.prefs.touch(&["a".into(), "b".into()]);
        c.prefs.touch(&["b".into(), "c".into()]);
        assert_eq!(c.prefs.recent, ["b", "c", "a"]);
        let many: Vec<String> = (0..80).map(|i| i.to_string()).collect();
        c.prefs.touch(&many);
        assert_eq!(c.prefs.recent.len(), RECENT_MAX);
        save(tmp.path(), &c).unwrap();
        assert_eq!(load(tmp.path()).unwrap(), c);
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
