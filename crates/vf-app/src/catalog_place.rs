//! Where a computer's catalog for a cabinet is kept (PA-84, Q-123, owner 2026-11-16: "a
//! reusable model for the catalog location to make future changes easier").
//!
//! One place decides it. Every part of the program that needs the folder asks [`root`]; none
//! builds the path itself. To add another place: a variant of [`Place`], its word in
//! [`Place::parse`] and [`Place::word`], and its folder in [`Place::root_with`]. Nothing else
//! changes: the catalog is made there (`server::provision`), found there
//! (`shared::shared_catalog`), moved there from the place used before ([`earlier_catalogs`]),
//! and opened in the way its disk allows ([`register`]).
//!
//! The firm chooses with one line in `firm.toml` beside `vibe-files.exe`:
//!
//! ```toml
//! catalogs = 'exe'           # <exe folder>\catalogs\<computer name>\<cabinet id>\catalog.db
//! catalogs = 'programdata'   # %PROGRAMDATA%\Vibe Files\catalogs\<cabinet id>\catalog.db (as with no line)
//! catalogs = 'D:\VibeFiles'  # D:\VibeFiles\<computer name>\<cabinet id>\catalog.db
//! ```
//!
//! Each computer has its own folder in a place several computers can reach: a catalog is one
//! computer's, never opened by two. A place on a network share is opened without SQLite's
//! write-ahead log (it needs memory shared between processes, which a share does not give):
//! slower, and a reader waits for a writer.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The folder name catalogs are kept under, in every place.
const DIR: &str = "catalogs";

/// A place a computer's catalogs can be kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// `%PROGRAMDATA%\Vibe Files\catalogs`: this computer's own disk (the default).
    ProgramData,
    /// `<exe folder>\catalogs\<computer name>`: beside the program, a folder per computer.
    BesideExe,
    /// `<folder>\<computer name>`: a folder the firm names, a folder per computer.
    Folder(PathBuf),
}

impl Place {
    /// What a `catalogs = '…'` line means. No line, a blank one or `programdata`: the default.
    pub fn parse(setting: Option<&str>) -> Place {
        match setting.map(str::trim) {
            None | Some("") => Place::ProgramData,
            Some(s) if s.eq_ignore_ascii_case("programdata") => Place::ProgramData,
            Some(s) if s.eq_ignore_ascii_case("exe") => Place::BesideExe,
            Some(s) => Place::Folder(PathBuf::from(s)),
        }
    }

    /// The word (or folder) that names this place in `firm.toml`.
    pub fn word(&self) -> String {
        match self {
            Place::ProgramData => "programdata".into(),
            Place::BesideExe => "exe".into(),
            Place::Folder(p) => p.display().to_string(),
        }
    }

    /// The folder that holds one sub-folder per cabinet, given this computer's ProgramData
    /// folder, the program's folder and the computer's name. `None` when the place needs one
    /// of them and it is not known.
    pub fn root_with(
        &self,
        programdata: Option<&Path>,
        exe_dir: Option<&Path>,
        computer: &str,
    ) -> Option<PathBuf> {
        let computer = safe_name(computer);
        match self {
            Place::ProgramData => Some(programdata?.join("Vibe Files").join(DIR)),
            Place::BesideExe => Some(exe_dir?.join(DIR).join(computer)),
            Place::Folder(p) => Some(p.join(computer)),
        }
    }
}

/// A computer name as a folder name.
fn safe_name(computer: &str) -> String {
    let s: String = computer
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() { "computer".into() } else { s }
}

fn programdata() -> Option<PathBuf> {
    std::env::var_os("PROGRAMDATA").map(PathBuf::from)
}

/// The place this run uses: `VIBE_FILES_CATALOGS` when set (for a trial on one computer),
/// else the firm's line in `firm.toml`, else the default. Read once.
pub fn current() -> &'static Place {
    static PLACE: OnceLock<Place> = OnceLock::new();
    PLACE.get_or_init(|| {
        let env = std::env::var("VIBE_FILES_CATALOGS").ok();
        let firm = vf_core::exe_dir().and_then(|d| crate::config::firm(&d).catalogs());
        Place::parse(env.as_deref().or(firm.as_deref()))
    })
}

/// The folder this computer's catalogs are kept in (one sub-folder per cabinet). The first
/// call tells the catalog code how a catalog there is to be opened.
pub fn root() -> Option<PathBuf> {
    static ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root = current().root_with(
            programdata().as_deref(),
            vf_core::exe_dir().as_deref(),
            &crate::server::computer(),
        );
        if let Some(r) = &root {
            register(r);
        }
        root
    })
    .clone()
}

/// A place on a network share is opened without the write-ahead log.
fn register(root: &Path) {
    if crate::server::is_network(root) {
        vf_core::db::on_network(root);
        tracing::info!("this computer's catalogs are on a network share: opened without WAL");
    }
}

/// Whether the place in use is on a network share (no access rights are set there: the
/// share's own apply; and Admin says so).
pub fn on_network() -> bool {
    root().is_some_and(|r| crate::server::is_network(&r))
}

/// This cabinet's catalog in the places that are not the one in use, when a file is there: a
/// catalog made before the place was changed, to start the new one from.
pub fn earlier_catalogs(cabinet_id: &str) -> Vec<PathBuf> {
    let now = root();
    let (pd, exe, computer) = (programdata(), vf_core::exe_dir(), crate::server::computer());
    [Place::ProgramData, Place::BesideExe]
        .iter()
        .filter_map(|p| p.root_with(pd.as_deref(), exe.as_deref(), &computer))
        .filter(|r| Some(r) != now.as_ref())
        .map(|r| {
            r.join(vf_fs::changes::safe_id(cabinet_id))
                .join(crate::shared::CATALOG_FILE)
        })
        .filter(|f| f.is_file())
        .collect()
}

/// For Admin: where this computer's catalogs are kept, in words.
pub fn describe() -> String {
    let at = root().map_or_else(|| "not known".to_string(), |r| r.display().to_string());
    let how = if on_network() {
        " It is on a network share: slower than this computer's own disk, and one person's change makes the others wait a moment."
    } else {
        ""
    };
    format!(
        "Catalogs of this computer are kept in {at} (firm.toml: catalogs = '{}').{how}",
        current().word()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PA-84: one model for the place. Each setting gives its folder; a place several
    /// computers reach has a folder per computer.
    #[test]
    fn each_place_gives_its_folder() {
        let (pd, exe) = (
            Path::new(r"C:\ProgramData"),
            Path::new(r"\\srv\apps\Vibe Files"),
        );
        let root = |s: Option<&str>| Place::parse(s).root_with(Some(pd), Some(exe), "HOST-7");
        for none in [None, Some(""), Some(" ProgramData ")] {
            assert_eq!(Place::parse(none), Place::ProgramData);
            assert_eq!(
                root(none),
                Some(PathBuf::from(r"C:\ProgramData\Vibe Files\catalogs"))
            );
        }
        assert_eq!(
            root(Some("exe")),
            Some(PathBuf::from(r"\\srv\apps\Vibe Files\catalogs\HOST-7"))
        );
        assert_eq!(
            root(Some(r"D:\VibeFiles")),
            Some(PathBuf::from(r"D:\VibeFiles\HOST-7"))
        );
        // What a place needs and is not known: no folder, not a wrong one.
        assert_eq!(Place::BesideExe.root_with(Some(pd), None, "H"), None);
        assert_eq!(Place::ProgramData.root_with(None, Some(exe), "H"), None);
        // A computer name is a folder name.
        assert_eq!(safe_name(r"a\b:c"), "a_b_c");
        assert_eq!(safe_name(" "), "computer");
        // The word written in firm.toml reads back as the place.
        for p in [
            Place::ProgramData,
            Place::BesideExe,
            Place::Folder(PathBuf::from(r"D:\VibeFiles")),
        ] {
            assert_eq!(Place::parse(Some(&p.word())), p);
        }
    }
}
