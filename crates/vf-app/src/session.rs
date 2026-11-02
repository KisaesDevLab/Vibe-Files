//! Opening a cabinet: create a new one, attach this PC to an existing one, or unlock the catalog
//! this data folder already holds (PHASES P01 step 1). No UI here; the wizard calls these.
//!
//! One data folder holds one catalog, for one cabinet (`config.toml` `cabinet`).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::json;
use vf_core::audit::AuditEntry;
use vf_core::db::{self, CatalogKey, Connection, Storage};
use vf_core::tx::{Actor, NoFileOps, Stmt, Transaction};
use vf_fs::cabinet::{self, CabinetFile};
use vf_fs::rebuild::RebuildReport;
use vf_security::keys::{self, Kdf, KeyFile};

use crate::config::Config;

pub const CATALOG_FILE: &str = "catalog.db";

/// An open cabinet.
#[allow(dead_code)] // `conn`, `data`, `config` are read by the Browse tab (P01 step 2)
pub struct Session {
    pub root: PathBuf,
    pub data: PathBuf,
    pub conn: Connection,
    pub actor: Actor,
    pub cabinet: CabinetFile,
    pub config: Config,
    pub key_id: String,
}

/// What this data folder holds, for the wizard's first screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalState {
    /// No catalog yet: create a cabinet or attach to one.
    Empty,
    /// A catalog for `cabinet`: ask for the passphrase.
    Locked { cabinet: PathBuf },
    /// Leftovers (a key file without a catalog, or a catalog without a cabinet path).
    Broken(String),
}

pub fn local_state(data: &Path) -> LocalState {
    let has_cat = data.join(CATALOG_FILE).is_file();
    let has_keys = data.join(keys::KEYS_FILE).is_file();
    let cabinet = Config::load_or_create(data).ok().and_then(|c| c.cabinet);
    match (has_cat, has_keys, cabinet) {
        (false, false, _) => LocalState::Empty,
        (true, true, Some(cabinet)) => LocalState::Locked { cabinet },
        _ => LocalState::Broken(format!(
            "{} holds a partial catalog (catalog.db / keys.json / config.toml cabinet do not match). \
             Move those files aside, then create or attach the cabinet again.",
            data.display()
        )),
    }
}

/// The Windows user name (until P06 adds Vibe Files users).
pub fn os_user() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "user".into())
}

fn ensure_empty_data(data: &Path) -> Result<()> {
    if data.join(CATALOG_FILE).exists() || data.join(keys::KEYS_FILE).exists() {
        bail!(
            "{} already holds a catalog. One data folder serves one cabinet.",
            data.display()
        );
    }
    Ok(())
}

fn now() -> String {
    vf_core::now_rfc3339()
}

/// Writes the key file and an empty, migrated catalog. On error nothing is left behind.
fn new_catalog(data: &Path, passphrase: &str, kdf: Kdf) -> Result<(Connection, String)> {
    keys::check_passphrase(passphrase)?;
    std::fs::create_dir_all(data).with_context(|| format!("creating {}", data.display()))?;
    ensure_empty_data(data)?;
    let raw = keys::generate_key();
    let kf = KeyFile::wrap(&raw, passphrase, kdf)?;
    kf.save_new(&data.join(keys::KEYS_FILE))?;
    match db::open(&data.join(CATALOG_FILE), &CatalogKey(raw), Storage::Local) {
        Ok(conn) => Ok((conn, kf.key_id)),
        Err(e) => {
            discard_catalog(data);
            Err(e.into())
        }
    }
}

/// Removes a catalog this module just created (never one that existed before).
fn discard_catalog(data: &Path) {
    for f in [
        CATALOG_FILE,
        "catalog.db-wal",
        "catalog.db-shm",
        keys::KEYS_FILE,
    ] {
        let _ = std::fs::remove_file(data.join(f));
    }
}

fn register_device(conn: &mut Connection, cfg: &Config, actor: &Actor) -> Result<()> {
    let t = now();
    Transaction::new("register device")
        .catalog(Stmt::new(
            "INSERT INTO device (id, name, first_seen, last_seen, mode) VALUES (?1, ?2, ?3, ?3, 'local')
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, last_seen = excluded.last_seen",
            vec![cfg.device_id.clone().into(), cfg.device_name.clone().into(), t.into()],
        ))
        .audit(AuditEntry::new(
            "device_register",
            None,
            json!({"device": cfg.device_id, "name": cfg.device_name}),
        ))
        .commit(conn, &mut NoFileOps, actor)?;
    Ok(())
}

fn finish(
    data: &Path,
    root: &Path,
    mut conn: Connection,
    key_id: String,
    cab: CabinetFile,
) -> Result<(Session, RebuildReport)> {
    let mut config = Config::load_or_create(data)?;
    let actor = Actor {
        user: Some(os_user()),
        device_id: config.device_id.clone(),
    };
    register_device(&mut conn, &config, &actor)?;
    vf_core::doctype::seed(&mut conn, &actor)?;
    let rep = vf_fs::rebuild::rebuild(&mut conn, root, &config.device_id)?;
    config.cabinet = Some(root.to_path_buf());
    config.save(data)?;
    Ok((
        Session {
            root: root.to_path_buf(),
            data: data.to_path_buf(),
            conn,
            actor,
            cabinet: cab,
            config,
            key_id,
        },
        rep,
    ))
}

/// New cabinet at `root` (an existing folder, not already a cabinet) with a new catalog here.
pub fn create_cabinet(
    data: &Path,
    root: &Path,
    firm_name: &str,
    passphrase: &str,
    kdf: Kdf,
) -> Result<(Session, RebuildReport)> {
    if firm_name.trim().is_empty() {
        bail!("Enter the firm name.");
    }
    if !root.is_dir() {
        bail!("{} is not a folder.", root.display());
    }
    if cabinet::cabinet_file(root).exists() {
        bail!(
            "{} is already a Vibe Files cabinet. Use \"Open existing cabinet\".",
            root.display()
        );
    }
    let (conn, key_id) = new_catalog(data, passphrase, kdf)?;
    let cab = CabinetFile {
        cabinet_id: vf_core::new_id(),
        firm_name: firm_name.trim().into(),
        firm_id: vf_core::new_id(), // a uuid, as SentinelV1 `firm_id` requires
        schema_version: db::SCHEMA_VERSION,
        layout_version: 1,
        created: now(),
    };
    if let Err(e) = cabinet::create(root, &cab) {
        drop(conn);
        discard_catalog(data);
        bail!(e);
    }
    finish(data, root, conn, key_id, cab)
}

/// Existing cabinet at `root`, new catalog on this PC rebuilt from the folder tree.
pub fn attach_cabinet(
    data: &Path,
    root: &Path,
    passphrase: &str,
    kdf: Kdf,
) -> Result<(Session, RebuildReport)> {
    let cab = cabinet::read(root).map_err(anyhow::Error::msg)?;
    let (conn, key_id) = new_catalog(data, passphrase, kdf)?;
    finish(data, root, conn, key_id, cab)
}

/// Opens the catalog this data folder holds.
pub fn unlock(data: &Path, passphrase: &str) -> Result<Session> {
    let config = Config::load_or_create(data)?;
    let Some(root) = config.cabinet.clone() else {
        bail!("This data folder has no cabinet yet.");
    };
    let kf = KeyFile::load(&data.join(keys::KEYS_FILE))?;
    let raw = kf.unwrap_key(passphrase)?;
    let mut conn = db::open(&data.join(CATALOG_FILE), &CatalogKey(raw), Storage::Local)?;
    let cab = cabinet::read(&root)
        .map_err(|e| anyhow::anyhow!("{e}\nIs the cabinet drive or share connected?"))?;
    let known: Option<String> = conn
        .query_row("SELECT id FROM cabinet LIMIT 1", [], |r| r.get(0))
        .ok();
    if known.as_deref().is_some_and(|id| id != cab.cabinet_id) {
        bail!(
            "{} is a different cabinet from the one this catalog was built for.",
            root.display()
        );
    }
    conn.execute(
        "UPDATE device SET last_seen = ?2 WHERE id = ?1",
        [config.device_id.as_str(), now().as_str()],
    )?;
    let actor = Actor {
        user: Some(os_user()),
        device_id: config.device_id.clone(),
    };
    // Catalogs from before P01 step 4 have no doc types yet.
    vf_core::doctype::seed(&mut conn, &actor)?;
    Ok(Session {
        root,
        data: data.to_path_buf(),
        conn,
        actor,
        cabinet: cab,
        config,
        key_id: kf.key_id,
    })
}

/// The raw catalog key for CLI use (`VF_PASSPHRASE`), until P06.
pub fn key_from_passphrase(data: &Path, passphrase: &str) -> Result<CatalogKey> {
    let kf = KeyFile::load(&data.join(keys::KEYS_FILE))?;
    Ok(CatalogKey(kf.unwrap_key(passphrase)?))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub const PASS: &str = "correct horse battery";

    pub fn new_cabinet() -> (tempfile::TempDir, Session) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Cabinet");
        std::fs::create_dir(&root).unwrap();
        let (s, _) = create_cabinet(
            &tmp.path().join("data"),
            &root,
            "Kisaes LLC",
            PASS,
            Kdf::FAST,
        )
        .unwrap();
        (tmp, s)
    }

    /// The P00 fixture cabinet (5 clients, 60 PDFs) attached to a fresh catalog.
    pub fn fixture_cabinet() -> (tempfile::TempDir, Session) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Cabinet");
        vf_fixtures::generate(&root, 7).unwrap();
        let (s, rep) = attach_cabinet(&tmp.path().join("data"), &root, PASS, Kdf::FAST).unwrap();
        assert_eq!((rep.clients, rep.docs_new), (5, 60));
        (tmp, s)
    }

    #[test]
    fn create_then_unlock() {
        let (tmp, s) = new_cabinet();
        let data = tmp.path().join("data");
        let root = s.root.clone();
        let cab_id = s.cabinet.cabinet_id.clone();
        assert_eq!(s.cabinet.firm_name, "Kisaes LLC");
        assert_eq!(cabinet::read(&root).unwrap().cabinet_id, cab_id);
        drop(s);
        assert_eq!(local_state(&data), LocalState::Locked { cabinet: root });
        assert!(unlock(&data, "wrong passphrase!!").is_err());
        let s = unlock(&data, PASS).unwrap();
        let id: String = s
            .conn
            .query_row("SELECT id FROM cabinet", [], |r| r.get(0))
            .unwrap();
        assert_eq!(id, cab_id);
        assert!(vf_core::audit::verify(&s.conn).unwrap().ok());
    }

    #[test]
    fn create_refuses_an_existing_cabinet_and_a_used_data_folder() {
        let (tmp, s) = new_cabinet();
        let other = tmp.path().join("data2");
        assert!(create_cabinet(&other, &s.root, "X", PASS, Kdf::FAST).is_err());
        assert!(!other.join(CATALOG_FILE).exists());
        let root2 = tmp.path().join("Cabinet2");
        std::fs::create_dir(&root2).unwrap();
        assert!(create_cabinet(&s.data, &root2, "X", PASS, Kdf::FAST).is_err());
        assert!(!cabinet::cabinet_file(&root2).exists());
    }

    #[test]
    fn attach_rebuilds_an_existing_cabinet() {
        let (tmp, s) = new_cabinet();
        let root = s.root.clone();
        std::fs::create_dir_all(root.join("Loose")).unwrap();
        std::fs::write(root.join("Loose/a.pdf"), b"%PDF-1.4 a").unwrap();
        drop(s);
        let (s2, rep) = attach_cabinet(&tmp.path().join("laptop"), &root, PASS, Kdf::FAST).unwrap();
        assert_eq!(rep.docs_new, 1);
        assert_eq!(s2.config.cabinet.as_deref(), Some(root.as_path()));
    }

    #[test]
    fn weak_passphrase_leaves_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Cabinet");
        std::fs::create_dir(&root).unwrap();
        let data = tmp.path().join("data");
        assert!(create_cabinet(&data, &root, "Firm", "short", Kdf::FAST).is_err());
        assert_eq!(local_state(&data), LocalState::Empty);
        assert!(!cabinet::cabinet_file(&root).exists());
    }
}
