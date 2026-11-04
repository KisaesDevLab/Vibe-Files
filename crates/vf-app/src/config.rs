//! `<data>/config.toml`: per-workstation settings.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const CONFIG_FILE: &str = "config.toml";
/// P00 kept the device id alone in this file; [`load`] moves it into `config.toml`.
const LEGACY_DEVICE_ID: &str = "device_id";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// This workstation's id (uuid v7). Every changeable catalog row carries it.
    pub device_id: String,
    /// The cabinet opened last; the wizard offers it first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_cabinet: Option<PathBuf>,
    /// Browse views kept on this PC until users exist (QUESTIONS Q-23; per-user rows in P06).
    #[serde(default)]
    pub prefs: Prefs,
    /// This PC's intake staging folders (P02).
    #[serde(default)]
    pub ingest: Ingest,
    /// This PC's OCR engine (P03).
    #[serde(default)]
    pub ocr: Ocr,
    /// Viewer settings (P04).
    #[serde(default)]
    pub viewer: ViewerPrefs,
    /// How the window looks on this PC (text size; owner 2026-11-04).
    #[serde(default)]
    pub look: Look,
    /// B2 sync (P11).
    #[serde(default)]
    pub sync: SyncConfig,
    /// Ask GitHub for a newer release at start (P13; QUESTIONS Q-30). Nothing is installed.
    #[serde(default = "yes")]
    pub update_check: bool,
}

fn yes() -> bool {
    true
}

/// B2 sync of the open cabinet from this workstation (P11). The application key is sealed with
/// the firm sync key (opened by the sync passphrase), never kept in the clear.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncConfig {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub endpoint: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub region: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bucket: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key_id: String,
    /// Hex: the application key sealed by `vf_sync::setup::seal_secret`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub app_key_sealed: String,
    /// The cabinet these settings are for.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cabinet_id: String,
    /// Push (after a pull) every this many minutes while unlocked; 0 = only by hand.
    #[serde(default = "default_auto")]
    pub auto_minutes: u32,
    /// Automatic pushes only from this hour to this hour (local time, 0-23); `None` = any time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<(u8, u8)>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub paused: bool,
    /// B2 Object Lock days the firm set on the bucket (QUESTIONS Q-10: off by default; 30
    /// suggested).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_lock_days: Option<u32>,
}

fn default_auto() -> u32 {
    15
}

impl Default for SyncConfig {
    fn default() -> Self {
        SyncConfig {
            endpoint: String::new(),
            region: String::new(),
            bucket: String::new(),
            key_id: String::new(),
            app_key_sealed: String::new(),
            cabinet_id: String::new(),
            auto_minutes: default_auto(),
            window: None,
            paused: false,
            object_lock_days: None,
        }
    }
}

impl SyncConfig {
    pub fn configured_for(&self, cabinet_id: &str) -> bool {
        !self.endpoint.is_empty()
            && !self.app_key_sealed.is_empty()
            && self.cabinet_id == cabinet_id
    }

    pub fn s3(&self) -> vf_sync::remote::S3Config {
        vf_sync::remote::S3Config {
            endpoint: self.endpoint.clone(),
            region: self.region.clone(),
            bucket: self.bucket.clone(),
            key_id: self.key_id.clone(),
        }
    }

    /// Whether `hour` (local) is inside the window.
    pub fn in_window(&self, hour: u8) -> bool {
        match self.window {
            None => true,
            Some((a, b)) if a <= b => (a..b).contains(&hour),
            Some((a, b)) => hour >= a || hour < b,
        }
    }
}

/// How the window looks on this PC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Look {
    /// Text and controls size in percent (egui zoom): 100 = as designed.
    #[serde(default = "hundred")]
    pub text_size: u16,
    /// The dark theme.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dark: bool,
}

fn hundred() -> u16 {
    100
}

impl Default for Look {
    fn default() -> Self {
        Look {
            text_size: 100,
            dark: false,
        }
    }
}

/// Text sizes offered (percent).
pub const TEXT_SIZES: [u16; 7] = [80, 90, 100, 110, 125, 150, 175];

/// Viewer settings of this workstation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewerPrefs {
    /// Author on annotations, stamps and tickmarks; empty = from the Windows user name. Until
    /// users exist (PA-15) this is per workstation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initials: Option<String>,
    /// Tickmark color by name (`annot::TICK_COLORS`); none = green.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tick_color: Option<String>,
}

impl ViewerPrefs {
    /// The initials to sign with: the setting, else the Windows user name's (`kwkcpa` → `KWK`).
    pub fn initials(&self) -> String {
        if let Some(i) = self.initials.as_ref().filter(|i| !i.trim().is_empty()) {
            return i.trim().to_string();
        }
        std::env::var("USERNAME")
            .ok()
            .map(|u| {
                u.chars()
                    .filter(|c| c.is_alphanumeric())
                    .take(3)
                    .collect::<String>()
            })
            .filter(|s| !s.is_empty())
            .map(|s| s.to_uppercase())
            .unwrap_or_else(|| "VF".into())
    }
}

/// OCR settings of this workstation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ocr {
    /// `tesseract.exe` or its folder; empty = beside the exe, Program Files, `PATH` (Q-25).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tesseract: Option<PathBuf>,
    /// Pause OCR on this PC.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub paused: bool,
}

/// Folders Vibe Files processes before Filer sees the files (`vf_ingest::pipeline`). Outside
/// the cabinet and not one of Filer's inboxes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Ingest {
    /// Scanner output, phone photos, barcode-sheet stacks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scans: Option<PathBuf>,
    /// Where a PDF printer saves (step 5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub print_capture: Option<PathBuf>,
    /// This PC's scanner: `twain:<source name>` or a WIA device id (P02 step 2, PA-19). The
    /// workstation's default; [`Ingest::scanner_by_user`] wins for a user who chose one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scanner: Option<String>,
    /// Scanner chosen by each Windows user (several people in sessions on one server share
    /// this `config.toml`; each session's remote-scanning source is its own).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub scanner_by_user: std::collections::BTreeMap<String, String>,
    /// TWAIN: show the driver's own window, so its settings and profiles (PaperStream IP)
    /// drive the scan instead of the Vibe Files profile.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub driver_ui: bool,
    /// Last scan profile used, by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan_profile: Option<String>,
    /// This PC's scan profiles; empty = `vf_ingest::scanner::default_profiles()`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scan_profiles: Vec<vf_ingest::scanner::Profile>,
}

/// The Windows user name (`USERNAME`), or "" when unset.
pub fn windows_user() -> String {
    std::env::var("USERNAME").unwrap_or_default()
}

impl Ingest {
    /// The scanner `user` chose, else this workstation's.
    pub fn scanner_for(&self, user: &str) -> Option<&str> {
        self.scanner_by_user
            .get(user)
            .or(self.scanner.as_ref())
            .map(String::as_str)
    }

    /// Remembers `id` for `user` and as the workstation's default.
    pub fn choose_scanner(&mut self, user: &str, id: &str) {
        self.scanner = Some(id.to_string());
        if !user.is_empty() {
            self.scanner_by_user
                .insert(user.to_string(), id.to_string());
        }
    }

    pub fn profiles(&self) -> Vec<vf_ingest::scanner::Profile> {
        if self.scan_profiles.is_empty() {
            vf_ingest::scanner::default_profiles()
        } else {
            self.scan_profiles.clone()
        }
    }
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
    /// Browse's tree by tax year (client → year → folder) instead of the folders on disk.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tree_by_year: bool,
}

pub const RECENT_MAX: usize = 50;

/// A place in the Browse tree. `client` is the client uuid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Favorite {
    Client {
        client: String,
    },
    /// A year in the year view.
    Year {
        client: String,
        year: Option<i32>,
    },
    /// A folder inside the client folder (`Income Tax/2025`); with a year, a year view folder
    /// (the folder without its year segment).
    Folder {
        client: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
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
/// A `config.toml` that cannot be read is renamed `config.toml.bad-<time>` (so nothing saves
/// over it) and a fresh one made, keeping the workstation's device id when it can be found in
/// the old text (QA A2). Returns the new config and the old file's new name.
pub fn set_aside_bad(data: &Path) -> Result<(Config, PathBuf)> {
    let p = data.join(CONFIG_FILE);
    let text = std::fs::read_to_string(&p).unwrap_or_default();
    let device_id = text
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once('=')?;
            (k.trim() == "device_id").then(|| v.trim().trim_matches('"').to_string())
        })
        .filter(|d| !d.is_empty())
        .unwrap_or_else(vf_core::new_id);
    let aside = data.join(format!(
        "{CONFIG_FILE}.bad-{}",
        vf_core::now_rfc3339().replace(':', "")
    ));
    std::fs::rename(&p, &aside).with_context(|| format!("setting aside {}", p.display()))?;
    let c = Config {
        device_id,
        last_cabinet: None,
        prefs: Prefs::default(),
        ingest: Ingest::default(),
        ocr: Ocr::default(),
        viewer: ViewerPrefs::default(),
        look: Look::default(),
        sync: SyncConfig::default(),
        update_check: true,
    };
    save(data, &c)?;
    Ok((c, aside))
}

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
                ingest: Ingest::default(),
                ocr: Ocr::default(),
                viewer: ViewerPrefs::default(),
                look: Look::default(),
                sync: SyncConfig::default(),
                update_check: true,
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
    vf_fs::cabinet::write_replace(
        &data.join(CONFIG_FILE),
        toml::to_string(c).expect("serializes").as_bytes(),
    )
    .map_err(anyhow::Error::msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// QA A2: a config.toml that cannot be read is never saved over; the device id stays.
    #[test]
    fn an_unreadable_config_is_kept_aside_with_its_device_id() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(
            t.path().join(CONFIG_FILE),
            "device_id = \"0192-dev\"
[sync
broken = ",
        )
        .unwrap();
        assert!(load(t.path()).is_err());
        let (c, aside) = set_aside_bad(t.path()).unwrap();
        assert_eq!(c.device_id, "0192-dev");
        assert!(std::fs::read_to_string(aside).unwrap().contains("broken"));
        assert_eq!(load(t.path()).unwrap().device_id, "0192-dev");
    }

    /// PA-19 step 4: on a server, each session user keeps their own scanner (their own
    /// remote-scanning source); a user who never chose one gets the workstation's.
    #[test]
    fn scanner_choice_is_kept_per_windows_user() {
        let mut i = Ingest::default();
        assert_eq!(i.scanner_for("ann"), None);
        i.choose_scanner("ann", "twain:Remote Scan");
        i.choose_scanner("bob", "twain:fi-7160");
        i.driver_ui = true;
        assert_eq!(i.scanner_for("ann"), Some("twain:Remote Scan"));
        assert_eq!(i.scanner_for("bob"), Some("twain:fi-7160"));
        assert_eq!(
            i.scanner_for("cy"),
            Some("twain:fi-7160"),
            "workstation default"
        );
        let back: Ingest = toml::from_str(&toml::to_string(&i).unwrap()).unwrap();
        assert_eq!(back, i);
        let old: Ingest = toml::from_str("scanner = 'wia-1'").unwrap();
        assert_eq!(old.scanner_for("ann"), Some("wia-1"));
        assert!(!old.driver_ui);
    }

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
