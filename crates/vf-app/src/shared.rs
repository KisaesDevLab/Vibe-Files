//! One catalog for every Windows login on one PC (owner 2026-11-05, Q-67: a Remote Desktop
//! server with 10+ people on one large cabinet indexes it once, not once per person).
//!
//! The shared catalog lives at `%PROGRAMDATA%\Vibe Files\catalogs\<cabinet id>\catalog.db`, on
//! the PC's own disk (SQLite in WAL mode is safe between processes on one machine; never on a
//! network share). Each login keeps its own data folder for everything else: settings, device
//! id (so each person's undo stack is their own), logs, caches. Sharing is an Admin action
//! that copies this login's catalog there and lets every login of the PC write to it.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub const CATALOG_FILE: &str = "catalog.db";

/// Where shared catalogs live on this PC.
pub fn shared_root() -> Option<PathBuf> {
    std::env::var_os("PROGRAMDATA")
        .map(PathBuf::from)
        .map(|p| p.join("Vibe Files").join("catalogs"))
}

/// This cabinet's shared catalog on this PC, when it has one.
pub fn shared_catalog(cabinet_id: &str) -> Option<PathBuf> {
    shared_catalog_in(&shared_root()?, cabinet_id)
}

fn shared_catalog_in(base: &Path, cabinet_id: &str) -> Option<PathBuf> {
    let safe = vf_fs::changes::safe_id(cabinet_id);
    let p = base.join(safe).join(CATALOG_FILE);
    p.is_file().then_some(p)
}

/// Shares this login's catalog (`conn` on `catalog`) with every login of this PC: a copy in the
/// shared place, writable by its users. Returns where it is. Takes effect at the next open.
pub fn share(conn: &vf_core::db::Connection, cabinet_id: &str) -> Result<PathBuf> {
    let base = shared_root().context("this PC has no ProgramData folder")?;
    share_in(conn, &base, cabinet_id)
}

/// This cabinet's shared-catalog folder under `base`, made writable for every login of this
/// PC (made if missing).
pub fn prepare_dir(base: &Path, cabinet_id: &str) -> Result<PathBuf> {
    let safe = vf_fs::changes::safe_id(cabinet_id);
    let dir = base.join(&safe);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    grant_users(&dir)?;
    Ok(dir)
}

fn share_in(conn: &vf_core::db::Connection, base: &Path, cabinet_id: &str) -> Result<PathBuf> {
    let safe = vf_fs::changes::safe_id(cabinet_id);
    let target = base.join(&safe).join(CATALOG_FILE);
    if target.exists() {
        anyhow::bail!(
            "This PC already shares a catalog for this cabinet: {}",
            target.display()
        );
    }
    prepare_dir(base, cabinet_id)?;
    // A consistent copy, WAL included.
    conn.execute("VACUUM INTO ?1", [target.to_string_lossy().as_ref()])
        .with_context(|| format!("copying the catalog to {}", target.display()))?;
    Ok(target)
}

/// Lets every login of this PC change `dir`.
fn grant_users(dir: &Path) -> Result<()> {
    // Every login of this PC may change it (BUILTIN\Users, by SID: names differ by language).
    #[cfg(windows)]
    {
        let ok = std::process::Command::new("icacls")
            .arg(dir)
            .args(["/grant", "*S-1-5-32-545:(OI)(CI)M", "/T", "/Q"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            anyhow::bail!(
                "The other logins could not be given access to {}. Run Vibe Files as an administrator once to share the catalog.",
                dir.display()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shared_catalog_is_a_copy_found_by_cabinet() {
        let t = tempfile::tempdir().unwrap();
        let base = t.path().join("catalogs");
        let conn =
            vf_core::db::open(&t.path().join("mine.db"), vf_core::db::Storage::Local).unwrap();
        conn.execute(
            "INSERT INTO local_state (k, v) VALUES ('probe', 'here')",
            [],
        )
        .unwrap();
        assert!(shared_catalog_in(&base, "cab-1").is_none());
        let p = share_in(&conn, &base, "cab-1").unwrap();
        assert_eq!(shared_catalog_in(&base, "cab-1"), Some(p.clone()));
        let copy = vf_core::db::open(&p, vf_core::db::Storage::Local).unwrap();
        let v: String = copy
            .query_row("SELECT v FROM local_state WHERE k = 'probe'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(v, "here");
        assert!(share_in(&conn, &base, "cab-1").is_err(), "once");
    }
}
