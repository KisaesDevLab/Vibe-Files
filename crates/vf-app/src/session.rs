//! Creating and opening a cabinet: the work behind the cabinet wizard (P01 step 1).
//!
//! The catalog lives at `<data>/catalog.db` (one cabinet per data folder), opened exactly as the
//! CLI opens it. Its key is random; the cabinet keeps it wrapped by the recovery passphrase in
//! `_Vibe/keys/recovery.json` so any workstation can open the cabinet.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use vf_core::db::{self, CatalogKey, Connection, Storage};
use vf_fs::cabinet::{self, CabinetFile};
use vf_fs::rebuild::RebuildReport;
use vf_security::keys::{self, KdfParams, MIN_PASSPHRASE_LEN, WrappedKey};

pub const CATALOG_FILE: &str = "catalog.db";

/// An open cabinet.
pub struct Opened {
    pub root: PathBuf,
    pub cabinet: CabinetFile,
    pub conn: Connection,
    /// Set when this open ran a rebuild (new cabinet, or a catalog that had never seen it).
    pub rebuild: Option<RebuildReport>,
}

/// What the wizard asks for when creating.
pub struct CreateRequest<'a> {
    pub root: &'a Path,
    pub name: &'a str,
    pub passphrase: &'a str,
    pub confirm: &'a str,
}

/// Checks the wizard's create fields before any disk work. Messages are shown as-is.
pub fn check_create(r: &CreateRequest) -> Result<(), String> {
    if r.root.as_os_str().is_empty() {
        return Err("Choose the cabinet folder.".into());
    }
    if r.name.trim().is_empty() {
        return Err("Give the cabinet a name.".into());
    }
    if r.passphrase.chars().count() < MIN_PASSPHRASE_LEN {
        return Err(format!(
            "The recovery passphrase needs at least {MIN_PASSPHRASE_LEN} characters."
        ));
    }
    if r.passphrase != r.confirm {
        return Err("The two passphrases do not match.".into());
    }
    if cabinet::cabinet_file(r.root).exists() {
        return Err("This folder is already a cabinet. Use Open instead.".into());
    }
    Ok(())
}

/// Creates a cabinet at `r.root` (the folder may already hold client folders), then indexes it.
pub fn create(data: &Path, device_id: &str, r: &CreateRequest, kdf: KdfParams) -> Result<Opened> {
    check_create(r).map_err(anyhow::Error::msg)?;
    let catalog = data.join(CATALOG_FILE);
    if catalog.exists() {
        bail!(
            "{} already holds a catalog for another cabinet. Use a separate data folder for each cabinet.",
            data.display()
        );
    }
    std::fs::create_dir_all(r.root).with_context(|| format!("creating {}", r.root.display()))?;

    let cab = CabinetFile {
        cabinet_id: vf_core::new_id(),
        // Keep the firm id an existing Filer Desktop tree's sentinels already carry.
        firm_id: cabinet::discover_firm_id(r.root).unwrap_or_else(vf_core::new_id),
        schema_version: db::SCHEMA_VERSION,
        layout_version: 1,
        created: vf_core::now_rfc3339(),
        name: Some(r.name.trim().to_owned()),
    };
    let raw = keys::generate_key()?;
    let wrapped = keys::wrap_with_passphrase(&raw, r.passphrase, &cab.cabinet_id, kdf)?;

    // Key file first: a cabinet.json without its key could never be opened.
    let key_file = cabinet::recovery_key_file(r.root);
    cabinet::write_new(&key_file, &wrapped.to_json()).map_err(anyhow::Error::msg)?;
    if let Err(e) = cabinet::create(r.root, &cab) {
        let _ = std::fs::remove_file(&key_file);
        bail!(e);
    }

    // The seeded doc-type list (Q-21); rebuild reads it into the catalog.
    vf_fs::doctypes::seed_if_missing(r.root).map_err(anyhow::Error::msg)?;
    std::fs::create_dir_all(data).with_context(|| format!("creating {}", data.display()))?;
    let mut conn = db::open(&catalog, &CatalogKey(raw), Storage::Local)?;
    let rep = vf_fs::rebuild::rebuild(&mut conn, r.root, device_id)?;
    Ok(Opened {
        root: r.root.to_owned(),
        cabinet: cab,
        conn,
        rebuild: Some(rep),
    })
}

/// Opens the cabinet at `root` with its recovery passphrase. Rebuilds when this data folder's
/// catalog has never indexed the cabinet (first open on a workstation).
pub fn open(data: &Path, device_id: &str, root: &Path, passphrase: &str) -> Result<Opened> {
    let cab = cabinet::read(root).map_err(anyhow::Error::msg)?;
    let key_file = cabinet::recovery_key_file(root);
    let bytes = std::fs::read(&key_file).map_err(|_| {
        anyhow::anyhow!(
            "{} has no recovery key file ({}). Cabinets made with the command line open only with --key-file.",
            root.display(),
            key_file.display()
        )
    })?;
    let raw =
        keys::unwrap_with_passphrase(&WrappedKey::from_json(&bytes)?, passphrase, &cab.cabinet_id)?;
    std::fs::create_dir_all(data).with_context(|| format!("creating {}", data.display()))?;
    let mut conn = match db::open(&data.join(CATALOG_FILE), &CatalogKey(raw), Storage::Local) {
        Err(vf_core::Error::BadKey) => bail!(
            "The catalog in {} belongs to a different cabinet. Use a separate data folder for each cabinet.",
            data.display()
        ),
        r => r?,
    };
    let known: i64 = conn.query_row(
        "SELECT count(*) FROM cabinet WHERE id = ?1",
        [&cab.cabinet_id],
        |r| r.get(0),
    )?;
    // Cabinets made before doc types existed get the seeded list now.
    let seeded = vf_fs::doctypes::seed_if_missing(root).map_err(anyhow::Error::msg)?;
    if seeded && known > 0 {
        vf_fs::doctypes::sync(&mut conn, root, device_id, "doc_types_seed")
            .map_err(anyhow::Error::msg)?;
    }
    let rebuild = if known == 0 {
        Some(vf_fs::rebuild::rebuild(&mut conn, root, device_id)?)
    } else {
        None
    };
    Ok(Opened {
        root: root.to_owned(),
        cabinet: cab,
        conn,
        rebuild,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Cheap Argon2 cost for tests; the file format is the same.
    pub const FAST: KdfParams = KdfParams {
        m_kib: 256,
        t: 1,
        p: 1,
    };
    const PASS: &str = "correct horse battery";

    fn req<'a>(root: &'a Path, pass: &'a str) -> CreateRequest<'a> {
        CreateRequest {
            root,
            name: "Kisaes Clients",
            passphrase: pass,
            confirm: pass,
        }
    }

    #[test]
    fn create_then_open_on_a_fresh_workstation() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("cab");
        let a = create(&tmp.path().join("pc1"), "dev-1", &req(&root, PASS), FAST).unwrap();
        assert_eq!(a.cabinet.name.as_deref(), Some("Kisaes Clients"));
        assert!(cabinet::recovery_key_file(&root).is_file());
        assert!(a.rebuild.is_some());
        drop(a);

        // Another PC: empty data folder, passphrase only. Opens and indexes.
        let b = open(&tmp.path().join("pc2"), "dev-2", &root, PASS).unwrap();
        assert!(b.rebuild.is_some());
        drop(b);
        // Second open on that PC: catalog already knows the cabinet, no rebuild.
        let c = open(&tmp.path().join("pc2"), "dev-2", &root, PASS).unwrap();
        assert!(c.rebuild.is_none());
    }

    /// Pointing the wizard at an existing Filer Desktop tree: the clients are indexed and the
    /// cabinet keeps the firm id its sentinels carry.
    #[test]
    fn create_over_an_existing_client_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("tree");
        let fx = vf_fixtures::generate(&root, 7).unwrap();
        let fixture_cab = cabinet::read(&root).unwrap();
        // Make it a bare Filer tree: no cabinet yet.
        std::fs::remove_dir_all(root.join(cabinet::VIBE_DIR)).unwrap();
        let a = create(&tmp.path().join("d"), "dev", &req(&root, PASS), FAST).unwrap();
        assert_eq!(a.cabinet.firm_id, fixture_cab.firm_id);
        let snap = vf_core::browse::load(&a.conn).unwrap();
        assert_eq!(
            (snap.clients.len(), snap.docs.len()),
            (fx.clients, fx.docs.len())
        );
        let rep = a.rebuild.unwrap();
        assert_eq!(rep.clients, fx.clients);
        assert_eq!(rep.docs_new, fx.docs.len());
    }

    #[test]
    fn wrong_passphrase_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("cab");
        drop(create(&tmp.path().join("d"), "dev", &req(&root, PASS), FAST).unwrap());
        let e = open(&tmp.path().join("d"), "dev", &root, "not the passphrase")
            .err()
            .unwrap();
        assert!(e.to_string().contains("passphrase is wrong"), "{e}");
    }

    #[test]
    fn create_refuses_an_existing_cabinet_and_weak_input() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("cab");
        drop(create(&tmp.path().join("d1"), "dev", &req(&root, PASS), FAST).unwrap());
        assert!(
            check_create(&req(&root, PASS))
                .unwrap_err()
                .contains("already a cabinet")
        );
        let other = tmp.path().join("other");
        assert!(
            check_create(&req(&other, "short"))
                .unwrap_err()
                .contains("12")
        );
        let mismatch = CreateRequest {
            confirm: "correct horse batterY",
            ..req(&other, PASS)
        };
        assert!(check_create(&mismatch).unwrap_err().contains("match"));
        // A data folder already holding another cabinet's catalog is refused before any write.
        let e = create(&tmp.path().join("d1"), "dev", &req(&other, PASS), FAST)
            .err()
            .unwrap();
        assert!(e.to_string().contains("another cabinet"), "{e}");
        assert!(!cabinet::cabinet_file(&other).exists());
    }
}
