//! Cabinets in a OneDrive or Dropbox folder (PA-39, Q-76): this computer's choice, and what a
//! cloud cabinet changes.
//!
//! A cloud cabinet works as several Remote Desktop servers do (PA-36): each PC keeps its own
//! catalog (started from the copy another left in `_Vibe\catalog-seed\`, so nothing is read
//! again), notes its changes in its own change log, and text recognition and B2 sync run on one
//! main PC. Online-only files are left until they are on the PC (`vf_fs::cloud`).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};

pub use crate::server::Setting;

fn setting_path() -> Option<PathBuf> {
    std::env::var_os("PROGRAMDATA")
        .map(PathBuf::from)
        .map(|p| p.join("Vibe Files").join("cloud-mode.txt"))
}

/// This computer's choice (Admin › Devices › Cloud folder handling), kept in
/// `%PROGRAMDATA%\Vibe Files\cloud-mode.txt` like server mode's.
pub fn setting() -> Setting {
    match setting_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .as_deref()
        .map(str::trim)
    {
        Some("on") => Setting::On,
        Some("off") => Setting::Off,
        _ => Setting::Auto,
    }
}

/// Saves this computer's choice; takes effect at the next start of Vibe Files.
pub fn set_setting(s: Setting) -> Result<()> {
    let p = setting_path().context("this computer has no ProgramData folder")?;
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
    }
    let v = match s {
        Setting::Auto => "auto",
        Setting::On => "on",
        Setting::Off => "off",
    };
    std::fs::write(&p, v).with_context(|| format!("writing {}", p.display()))
}

/// The choice read once per run (a change takes effect at the next start).
/// `VIBE_FILES_CLOUD=1` or `0` decides instead (as `VIBE_FILES_SERVER` does for server mode).
fn chosen() -> Setting {
    static ON: std::sync::OnceLock<Setting> = std::sync::OnceLock::new();
    *ON.get_or_init(|| match std::env::var("VIBE_FILES_CLOUD").as_deref() {
        Ok("1") => Setting::On,
        Ok("0") => Setting::Off,
        _ => setting(),
    })
}

/// At start: online-only files are left alone unless this computer reads every file.
pub fn apply() {
    vf_fs::cloud::set_leave_online_only(chosen() != Setting::Off);
}

/// The cloud service the cabinet at `root` is in, as this computer handles it: `None` when it
/// is not in one (or the choice is Always off); "a cloud folder" when Always on finds none.
pub fn cabinet_provider(root: &Path) -> Option<String> {
    let found = detected(root);
    match chosen() {
        Setting::Off => None,
        Setting::On => Some(found.unwrap_or_else(|| "a cloud folder".into())),
        Setting::Auto => found,
    }
}

/// The cloud service Windows (or Dropbox) reports for `root`, whatever this computer's choice;
/// looked up once per cabinet (pages ask every frame).
pub fn detected(root: &Path) -> Option<String> {
    type Seen = std::collections::HashMap<PathBuf, Option<String>>;
    static SEEN: Mutex<Option<Seen>> = Mutex::new(None);
    if let Ok(g) = SEEN.lock()
        && let Some(p) = g.as_ref().and_then(|m| m.get(root))
    {
        return p.clone();
    }
    let found = vf_fs::cloud::provider(root);
    if let Ok(mut g) = SEEN.lock() {
        g.get_or_insert_with(Default::default)
            .insert(root.to_path_buf(), found.clone());
    }
    found
}

/// Whether the cabinet at `root` is handled as a cloud cabinet on this computer.
pub fn is_cloud(root: &Path) -> bool {
    cabinet_provider(root).is_some()
}

/// Conflicted copies found by [`check_conflicts`], waiting to be shown.
static CONFLICTS: Mutex<Option<Vec<String>>> = Mutex::new(None);

/// On opening a cloud cabinet: looks for conflicted copies of Vibe Files' own files (two PCs
/// changed one between syncs) on a thread; [`take_conflicts`] hands them over.
pub fn check_conflicts(ctx: &egui::Context, root: &Path) {
    if !is_cloud(root) {
        return;
    }
    let (ctx, root) = (ctx.clone(), root.to_path_buf());
    std::thread::Builder::new()
        .name("cloud-conflicts".into())
        .spawn(move || {
            let names: Vec<String> = vf_fs::safeguards::devices(&root)
                .into_iter()
                .map(|d| d.name)
                .collect();
            let found = vf_fs::cloud::conflicted_copies(&root, &names);
            if let Ok(mut g) = CONFLICTS.lock() {
                *g = Some(found);
            }
            ctx.request_repaint();
        })
        .ok();
}

/// What [`check_conflicts`] found, once: a notice for Browse when there is any.
pub fn take_conflicts() -> Option<String> {
    let found = CONFLICTS.lock().ok()?.take()?;
    if found.is_empty() {
        return None;
    }
    let shown: Vec<&str> = found.iter().take(5).map(String::as_str).collect();
    Some(format!(
        "{} conflicted copies of Vibe Files' own files are in the cabinet (two PCs changed the same file before the cloud service synced them): {}{}. Tell your administrator; see Help › Cabinets in OneDrive or Dropbox.",
        found.len(),
        shown.join(", "),
        if found.len() > shown.len() {
            ", …"
        } else {
            ""
        }
    ))
}

/// The cloud service a data folder (its nearest existing folder) is in, unless this computer's
/// choice is Always off. A SQLite catalog synced while open is damaged (PA-39 step 3).
pub fn data_in_cloud(data: &Path) -> Option<String> {
    if chosen() == Setting::Off {
        return None;
    }
    vf_fs::cloud::existing_ancestor(data).and_then(|d| vf_fs::cloud::provider(&d))
}

/// Moves a default data folder found in a cloud folder to `personal` (this login's
/// `%LOCALAPPDATA%\Vibe Files\data`) once: its settings and a consistent copy of its catalog,
/// so the catalog's history comes along. A note is left in the old folder. Returns the folder
/// to use: `personal`, or `old` when nothing could be moved (logged; the app still opens).
pub fn move_data(old: &Path, personal: &Path) -> PathBuf {
    let cat = crate::shared::CATALOG_FILE;
    let moved = (|| -> Result<bool> {
        std::fs::create_dir_all(personal)?;
        let mut any = false;
        let cfg = personal.join("config.toml");
        if !cfg.exists() && old.join("config.toml").is_file() {
            std::fs::copy(old.join("config.toml"), &cfg)?;
            any = true;
        }
        let to = personal.join(cat);
        if !to.exists() && old.join(cat).is_file() {
            let part = personal.join(format!("{cat}.part"));
            let _ = std::fs::remove_file(&part);
            let src = vf_core::db::Connection::open_with_flags(
                old.join(cat),
                vf_core::db::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )?;
            src.execute("VACUUM INTO ?1", [part.to_string_lossy().as_ref()])?;
            drop(src);
            std::fs::rename(&part, &to)?;
            any = true;
        }
        if any {
            let _ = std::fs::write(
                old.join("MOVED.txt"),
                format!(
                    "Vibe Files moved this data folder to {} on {}: this folder is in a OneDrive \
                     or Dropbox folder, where its catalog would be damaged by syncing.\r\n",
                    personal.display(),
                    vf_core::now_rfc3339()
                ),
            );
        }
        Ok(any)
    })();
    match moved {
        Ok(any) => {
            if any {
                tracing::warn!("data folder moved out of a cloud folder");
            }
            personal.to_path_buf()
        }
        Err(e) => {
            tracing::error!(
                kind = crate::logging::kind(&e.to_string()),
                "data folder in a cloud folder could not be moved"
            );
            old.to_path_buf()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PA-39 done-when: a cloud cabinet opens on a second PC from the seed copy the first left,
    /// reading no file (its files are online only there). Run alone, as it sets this process's
    /// PROGRAMDATA and cloud choice:
    /// `cargo test -p vf-app cloud_cabinet_second_pc -- --ignored --test-threads=1`
    #[test]
    #[ignore = "sets process-wide environment; run alone"]
    fn cloud_cabinet_second_pc() {
        let t = tempfile::tempdir().unwrap();
        // SAFETY: run alone (ignored, one thread): nothing else reads the environment.
        unsafe { std::env::set_var("VIBE_FILES_CLOUD", "1") };
        apply();
        let root = t.path().join("Dropbox").join("Cabinet");
        let firm = uuid::Uuid::new_v4();
        vf_fs::cabinet::create(
            &root,
            &vf_fs::cabinet::CabinetFile {
                cabinet_id: vf_core::new_id(),
                firm_id: firm.to_string(),
                schema_version: vf_core::db::SCHEMA_VERSION,
                layout_version: 1,
                created: vf_core::now_rfc3339(),
                name: None,
            },
        )
        .unwrap();
        let folder = root.join("Smith, John (1001)");
        std::fs::create_dir_all(folder.join("Income Tax/2025")).unwrap();
        let c = filer_core::clients::Client {
            id: "1001".into(),
            uuid: uuid::Uuid::new_v4(),
            name: "Smith, John".into(),
            extra: Default::default(),
        };
        filer_intake::sentinel::write_sentinel(&root, &folder, &c, firm).unwrap();
        for n in ["w2.pdf", "1099.pdf", "k1.pdf"] {
            std::fs::write(
                folder.join("Income Tax/2025").join(n),
                format!("synthetic {n}"),
            )
            .unwrap();
        }
        assert!(is_cloud(&root));

        // PC A: no seed yet; indexes the cabinet, then leaves its seed copy.
        // SAFETY: as above.
        unsafe { std::env::set_var("PROGRAMDATA", t.path().join("pc-a-programdata")) };
        let a = crate::session::open(&t.path().join("pc-a"), "pc-a", &root).unwrap();
        assert!(a.catalog.starts_with(t.path().join("pc-a-programdata")));
        let w = a.writer.clone().expect("a cloud cabinet notes its changes");
        assert!(crate::server::publish_seed(&a.catalog, &root, &w).unwrap());
        drop(a);
        vf_fs::changes::clear_writer();

        // PC B: the files are online only there.
        for n in ["w2.pdf", "1099.pdf", "k1.pdf"] {
            vf_fs::cloud::mark_online_only(&folder.join("Income Tax/2025").join(n));
        }
        // SAFETY: as above.
        unsafe { std::env::set_var("PROGRAMDATA", t.path().join("pc-b-programdata")) };
        let b = crate::session::open(&t.path().join("pc-b"), "pc-b", &root).unwrap();
        assert!(
            b.rebuild.is_none(),
            "started from the seed: nothing indexed again"
        );
        let n: i64 = b
            .conn
            .query_row(
                "SELECT count(*) FROM document WHERE deleted_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 3);
        // A rebuild there reads none of them and keeps them all.
        let cat = b.catalog.clone();
        assert!(cat.starts_with(t.path().join("pc-b-programdata")));
        drop(b);
        vf_fs::changes::clear_writer();
        let mut conn = vf_core::db::open(&cat, vf_core::db::Storage::Local).unwrap();
        let rep = vf_fs::rebuild::rebuild(&mut conn, &root, "pc-b").unwrap();
        // Same size and time as the seed recorded: taken as they are, nothing read.
        assert_eq!(rep.docs_unchanged, 3, "{rep}");
        assert_eq!(rep.docs_tombstoned + rep.docs_changed, 0, "{rep}");
    }

    #[test]
    fn a_data_folder_moves_with_its_catalog_and_settings() {
        let t = tempfile::tempdir().unwrap();
        let old = t
            .path()
            .join("OneDrive")
            .join("Desktop")
            .join("Vibe Files")
            .join("data");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("config.toml"), "device_id = \"dev-1\"\n").unwrap();
        let c = vf_core::db::open(
            &old.join(crate::shared::CATALOG_FILE),
            vf_core::db::Storage::Local,
        )
        .unwrap();
        c.execute(
            "INSERT INTO local_state (k, v) VALUES ('marker', 'kept')",
            [],
        )
        .unwrap();
        drop(c);
        let personal = t.path().join("Local").join("Vibe Files").join("data");
        assert_eq!(move_data(&old, &personal), personal);
        assert_eq!(
            std::fs::read_to_string(personal.join("config.toml")).unwrap(),
            "device_id = \"dev-1\"\n"
        );
        let c = vf_core::db::Connection::open(personal.join(crate::shared::CATALOG_FILE)).unwrap();
        let v: String = c
            .query_row("SELECT v FROM local_state WHERE k = 'marker'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(v, "kept");
        assert!(old.join("MOVED.txt").is_file());
        // Again: nothing is copied over what is there now.
        std::fs::write(old.join("config.toml"), "device_id = \"other\"\n").unwrap();
        assert_eq!(move_data(&old, &personal), personal);
        assert!(
            std::fs::read_to_string(personal.join("config.toml"))
                .unwrap()
                .contains("dev-1")
        );
    }
}
