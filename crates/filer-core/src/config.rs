//! Settings stored in `data/config.json`, plus the checks the Setup screen runs on them.

use std::path::{Component, Path, PathBuf, Prefix};

use serde::{Deserialize, Serialize};

use crate::parse;

pub const CONFIG_FILE: &str = "config.json";
/// A name match at or above this is offered as a one-click suggestion.
pub const DEFAULT_FUZZY_SUGGEST: f32 = 0.85;
/// A name match at or above this resolves without a click (shown with a `fuzzy` pill).
pub const DEFAULT_FUZZY_AUTO: f32 = 0.97;
pub const FUZZY_THRESHOLD_RANGE: std::ops::RangeInclusive<f32> = 0.70..=0.99;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Client folder tree root (local or UNC).
    pub root: Option<PathBuf>,
    pub inboxes: Vec<PathBuf>,
    /// Files in the inboxes' sub-folders are queued too (on by default; owner 2026-11-02).
    pub inbox_subfolders: bool,
    /// Client master CSV (P02).
    pub csv_path: Option<PathBuf>,
    pub column_map: ColumnMap,
    /// Regex for the ID part of `ClientName_ID_rest.ext`.
    pub id_pattern: String,
    /// UltraTax PDF names: which part is where (tried before `ClientName_ID_rest`).
    pub ultratax: parse::UltraTaxLayout,
    pub fuzzy_suggest: f32,
    pub fuzzy_auto: f32,
    /// Generated once (see [`Settings::ensure_firm_id`]); written into new sentinels.
    pub firm_id: Option<uuid::Uuid>,
    pub window: Option<WindowState>,
    /// Theme and zoom: saved as soon as they change, not with Setup's Save (V1, V2).
    pub display: Display,
}

/// Light, dark, or whatever Windows uses (V1).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    /// The header button cycles System → Light → Dark.
    pub fn next(self) -> Self {
        match self {
            ThemeChoice::System => ThemeChoice::Light,
            ThemeChoice::Light => ThemeChoice::Dark,
            ThemeChoice::Dark => ThemeChoice::System,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::System => "System",
            ThemeChoice::Light => "Light",
            ThemeChoice::Dark => "Dark",
        }
    }
}

/// Zoom (text and layout together, egui's Ctrl+Plus) allowed by Setup and the shortcuts (V2).
pub const ZOOM_RANGE: std::ops::RangeInclusive<f32> = 0.9..=1.5;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Display {
    pub theme: ThemeChoice,
    pub zoom: f32,
}

impl Default for Display {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::System,
            zoom: 1.0,
        }
    }
}

impl Display {
    /// Zoom kept inside [`ZOOM_RANGE`] and on whole percent steps.
    pub fn clamp_zoom(z: f32) -> f32 {
        let z = if z.is_finite() { z } else { 1.0 };
        (z.clamp(*ZOOM_RANGE.start(), *ZOOM_RANGE.end()) * 100.0).round() / 100.0
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            root: None,
            inboxes: Vec::new(),
            inbox_subfolders: true,
            csv_path: None,
            column_map: ColumnMap::default(),
            id_pattern: parse::DEFAULT_ID_PATTERN.to_string(),
            ultratax: parse::UltraTaxLayout::default(),
            fuzzy_suggest: DEFAULT_FUZZY_SUGGEST,
            fuzzy_auto: DEFAULT_FUZZY_AUTO,
            firm_id: None,
            window: None,
            display: Display::default(),
        }
    }
}

/// Which CSV columns hold the client ID and name, and which extra columns become tokens (P02).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ColumnMap {
    pub id: Option<String>,
    pub name: Option<String>,
    pub extras: Vec<String>,
}

/// Outer position and inner size, in egui points.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct WindowState {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Settings {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join(CONFIG_FILE)
    }

    /// Loads settings. A missing file gives defaults. An unreadable or corrupt file also gives
    /// defaults, but returns `false` so the caller does not overwrite it on exit.
    pub fn load_or_default(data_dir: &Path) -> (Self, bool) {
        let path = Self::path(data_dir);
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(s) => (s, true),
                Err(e) => {
                    tracing::error!(path = %path.display(), error = %e, "config.json is invalid; not saving over it");
                    (Self::default(), false)
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Self::default(), true),
            Err(e) => {
                tracing::error!(path = %path.display(), error = %e, "cannot read config.json; not saving over it");
                (Self::default(), false)
            }
        }
    }

    /// Writes to a temp file next to `config.json`, then renames over it.
    pub fn save(&self, data_dir: &Path) -> anyhow::Result<()> {
        crate::datafile::save_json(&Self::path(data_dir), self)
    }

    /// Generates the firm ID if there is none yet. Returns `true` when it did, so the caller saves.
    pub fn ensure_firm_id(&mut self) -> bool {
        if self.firm_id.is_some() {
            return false;
        }
        self.firm_id = Some(uuid::Uuid::new_v4());
        true
    }

    /// Equal in everything the Setup screen edits (ignores window geometry).
    pub fn same_setup(&self, other: &Settings) -> bool {
        let mut a = self.clone();
        a.window = other.window;
        a.display = other.display;
        a == *other
    }
}

// ---------------------------------------------------------------------------
// Validation

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Blocks saving.
    Error,
    /// Shown, does not block.
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub level: Level,
    pub msg: String,
}

impl Issue {
    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            level: Level::Error,
            msg: msg.into(),
        }
    }
    fn info(msg: impl Into<String>) -> Self {
        Self {
            level: Level::Info,
            msg: msg.into(),
        }
    }
}

/// Checks that need no disk access. Cheap enough to run every frame.
pub fn check_fields(s: &Settings) -> Vec<Issue> {
    let mut out = Vec::new();
    if let Err(msg) = parse::validate_id_pattern(&s.id_pattern) {
        out.push(Issue::error(msg));
    }
    if let Err(msg) = s.ultratax.validate() {
        out.push(Issue::error(msg));
    }
    for (what, v) in [
        ("Suggest at", s.fuzzy_suggest),
        ("Auto-resolve at", s.fuzzy_auto),
    ] {
        if !FUZZY_THRESHOLD_RANGE.contains(&v) {
            out.push(Issue::error(format!(
                "{what} must be between {:.2} and {:.2}.",
                FUZZY_THRESHOLD_RANGE.start(),
                FUZZY_THRESHOLD_RANGE.end()
            )));
        }
    }
    if s.fuzzy_auto < s.fuzzy_suggest {
        out.push(Issue::error("Auto-resolve at must be at least Suggest at."));
    }
    out
}

/// Checks that touch the disk (exists, writable, canonical paths). Can be slow on UNC paths, so
/// the UI runs this on a worker thread.
pub fn check_paths(root: Option<&Path>, inboxes: &[PathBuf]) -> Vec<Issue> {
    let mut out = Vec::new();

    let root_canon = match root {
        None => {
            out.push(Issue::error("Choose the client root folder."));
            None
        }
        Some(r) => match checked_dir(r, "Client root") {
            Ok(c) => Some(c),
            Err(issue) => {
                out.push(issue);
                None
            }
        },
    };

    if inboxes.is_empty() {
        out.push(Issue::info(
            "No inbox folders. Files can still be added by drag-drop or Browse.",
        ));
    }

    let mut seen: Vec<PathBuf> = Vec::new();
    for inbox in inboxes {
        let canon = match checked_dir(inbox, "Inbox") {
            Ok(c) => c,
            Err(issue) => {
                out.push(issue);
                continue;
            }
        };
        if seen.contains(&canon) {
            out.push(Issue::error(format!(
                "Inbox {} is listed twice.",
                inbox.display()
            )));
            continue;
        }
        if let Some(root) = &root_canon {
            if canon.starts_with(root) {
                out.push(Issue::error(format!(
                    "Inbox {} is inside the client root. Pick a folder outside it.",
                    inbox.display()
                )));
            } else if root.starts_with(&canon) {
                out.push(Issue::error(format!(
                    "The client root is inside inbox {}. Pick an inbox outside the root.",
                    inbox.display()
                )));
            } else if !same_volume(&canon, root) {
                out.push(Issue::info(format!(
                    "Inbox {} is on a different drive than the root (cross-drive: files will be copied and verified).",
                    inbox.display()
                )));
            }
        }
        seen.push(canon);
    }
    out
}

/// Exists, is a folder, and accepts a new file. Returns the canonical path.
fn checked_dir(path: &Path, what: &str) -> Result<PathBuf, Issue> {
    let canon = std::fs::canonicalize(path)
        .map_err(|e| Issue::error(format!("{what} {} cannot be opened: {e}", path.display())))?;
    if !canon.is_dir() {
        return Err(Issue::error(format!(
            "{what} {} is not a folder.",
            path.display()
        )));
    }
    probe_writable(&canon)
        .map_err(|e| Issue::error(format!("{what} {} is not writable: {e}", path.display())))?;
    Ok(canon)
}

/// Creates a uniquely named file opened delete-on-close, so Windows removes it when the handle
/// drops. The app never calls `remove_file` for this.
fn probe_writable(dir: &Path) -> std::io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_FLAG_DELETE_ON_CLOSE, FILE_GENERIC_WRITE,
    };
    let name = format!(".vibe-write-test-{}", uuid::Uuid::new_v4());
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .access_mode(FILE_GENERIC_WRITE | DELETE)
        .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
        .open(dir.join(name))
        .map(drop)
}

/// Lowercased volume of a path: `c:` for drive paths, `\\server\share` for UNC.
/// Verbatim (`\\?\`) forms from `canonicalize` map to the same key. A mapped drive letter and
/// the UNC path behind it count as different volumes, which errs toward copy-and-verify.
pub fn volume_key(path: &Path) -> Option<String> {
    let Some(Component::Prefix(p)) = path.components().next() else {
        return None;
    };
    let key = match p.kind() {
        Prefix::Disk(d) | Prefix::VerbatimDisk(d) => format!("{}:", d as char),
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => format!(
            r"\\{}\{}",
            server.to_string_lossy(),
            share.to_string_lossy()
        ),
        Prefix::DeviceNS(n) | Prefix::Verbatim(n) => n.to_string_lossy().into_owned(),
    };
    Some(key.to_lowercase())
}

/// True when both paths are on the same volume (so `std::fs::rename` can move between them).
pub fn same_volume(a: &Path, b: &Path) -> bool {
    matches!((volume_key(a), volume_key(b)), (Some(x), Some(y)) if x == y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn errors(issues: &[Issue]) -> Vec<&str> {
        issues
            .iter()
            .filter(|i| i.level == Level::Error)
            .map(|i| i.msg.as_str())
            .collect()
    }

    #[test]
    fn save_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let s = Settings {
            root: Some(r"\\server\clients".into()),
            inboxes: vec![r"C:\Scans".into()],
            fuzzy_suggest: 0.8,
            firm_id: Some(uuid::Uuid::new_v4()),
            window: Some(WindowState {
                x: 10.0,
                y: 20.0,
                width: 1000.0,
                height: 700.0,
            }),
            ..Settings::default()
        };
        s.save(dir.path()).unwrap();
        assert_eq!(Settings::load_or_default(dir.path()), (s, true));
    }

    #[test]
    fn p00_config_loads_with_new_defaults() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            Settings::path(dir.path()),
            r#"{"window":{"x":1.0,"y":2.0,"width":3.0,"height":4.0}}"#,
        )
        .unwrap();
        let (s, ok) = Settings::load_or_default(dir.path());
        assert!(ok);
        assert_eq!(s.fuzzy_suggest, DEFAULT_FUZZY_SUGGEST);
        assert_eq!(s.fuzzy_auto, DEFAULT_FUZZY_AUTO);
        assert_eq!(s.id_pattern, parse::DEFAULT_ID_PATTERN);
        assert_eq!(s.firm_id, None);
        assert!(s.window.is_some());
    }

    #[test]
    fn v010_config_gets_two_thresholds_and_a_firm_id() {
        // A v0.1.0 config: single threshold and a firm tag (X115).
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            Settings::path(dir.path()),
            r#"{"fuzzy_threshold":0.9,"firm_tag":"KISAES"}"#,
        )
        .unwrap();
        let (mut s, ok) = Settings::load_or_default(dir.path());
        assert!(ok);
        assert_eq!((s.fuzzy_suggest, s.fuzzy_auto), (0.85, 0.97));
        assert!(s.ensure_firm_id());
        let id = s.firm_id;
        assert!(!s.ensure_firm_id(), "generated once");
        assert_eq!(s.firm_id, id);
    }

    #[test]
    fn missing_file_is_default_and_savable() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            Settings::load_or_default(dir.path()),
            (Settings::default(), true)
        );
    }

    #[test]
    fn corrupt_file_is_not_savable() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(Settings::path(dir.path()), "{ nope").unwrap();
        assert_eq!(
            Settings::load_or_default(dir.path()),
            (Settings::default(), false)
        );
    }

    #[test]
    fn same_setup_ignores_window() {
        let a = Settings::default();
        let mut b = a.clone();
        b.window = Some(WindowState {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        });
        assert!(a.same_setup(&b));
        b.fuzzy_suggest = 0.95;
        assert!(!a.same_setup(&b));
    }

    #[test]
    fn field_checks() {
        assert!(check_fields(&Settings::default()).is_empty());
        let bad = Settings {
            id_pattern: r"\d*".into(),
            fuzzy_suggest: 0.5,
            ..Settings::default()
        };
        assert_eq!(errors(&check_fields(&bad)).len(), 2);
        let inverted = Settings {
            fuzzy_suggest: 0.95,
            fuzzy_auto: 0.90,
            ..Settings::default()
        };
        assert_eq!(errors(&check_fields(&inverted)).len(), 1);
    }

    #[test]
    fn volume_keys() {
        assert_eq!(volume_key(Path::new(r"C:\a\b")).as_deref(), Some("c:"));
        assert_eq!(volume_key(Path::new(r"\\?\c:\a")).as_deref(), Some("c:"));
        assert_eq!(
            volume_key(Path::new(r"\\Server\Share\x")).as_deref(),
            Some(r"\\server\share")
        );
        assert_eq!(
            volume_key(Path::new(r"\\?\UNC\server\share\x")).as_deref(),
            Some(r"\\server\share")
        );
        assert_eq!(volume_key(Path::new(r"relative\x")), None);
        assert!(same_volume(Path::new(r"C:\a"), Path::new(r"\\?\C:\b")));
        assert!(!same_volume(Path::new(r"C:\a"), Path::new(r"D:\a")));
        assert!(!same_volume(
            Path::new(r"\\srv\one\a"),
            Path::new(r"\\srv\two\a")
        ));
        assert!(!same_volume(Path::new("rel"), Path::new("rel")));
    }

    #[test]
    fn probe_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        probe_writable(dir.path()).unwrap();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn path_checks() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        let inbox = tmp.path().join("inbox");
        let nested = root.join("scans");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(&inbox).unwrap();

        assert!(errors(&check_paths(None, &[])).len() == 1);
        assert!(errors(&check_paths(Some(&root), std::slice::from_ref(&inbox))).is_empty());
        // No inbox is allowed, with an info note.
        let issues = check_paths(Some(&root), &[]);
        assert!(errors(&issues).is_empty() && issues.len() == 1);

        let e = check_paths(Some(&root), std::slice::from_ref(&nested));
        assert!(errors(&e)[0].contains("inside the client root"), "{e:?}");

        let e = check_paths(Some(&nested), std::slice::from_ref(&root));
        assert!(errors(&e)[0].contains("root is inside inbox"), "{e:?}");

        let e = check_paths(Some(&root), &[inbox.clone(), inbox.clone()]);
        assert!(errors(&e)[0].contains("listed twice"), "{e:?}");

        let e = check_paths(Some(&tmp.path().join("missing")), &[]);
        assert!(errors(&e)[0].contains("cannot be opened"), "{e:?}");

        let file = tmp.path().join("f.txt");
        std::fs::write(&file, "x").unwrap();
        let e = check_paths(Some(&file), &[]);
        assert!(errors(&e)[0].contains("not a folder"), "{e:?}");
    }
}
