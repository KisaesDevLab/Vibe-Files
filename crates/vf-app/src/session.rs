//! Creating and opening a cabinet: the work behind the cabinet wizard (P01 step 1).
//!
//! The catalog lives at `<data>/catalog.db` (one cabinet per data folder), opened exactly as the
//! CLI opens it. Since PA-18 (QUESTIONS Q-27) it is a plain SQLite file and opening a cabinet
//! asks for nothing. A catalog encrypted by an earlier build is set aside and rebuilt from the
//! cabinet (the catalog is a rebuildable index, CLAUDE.md principle 2).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use vf_core::db::{self, Connection, Storage};
use vf_fs::cabinet::{self, CabinetFile};
use vf_fs::rebuild::RebuildReport;

pub const CATALOG_FILE: &str = "catalog.db";

/// An open cabinet.
pub struct Opened {
    pub root: PathBuf,
    pub cabinet: CabinetFile,
    pub conn: Connection,
    /// The catalog file, for workers that open their own connection (OCR, P03).
    pub catalog: PathBuf,
    /// Set when this open ran a rebuild (new cabinet, or a catalog that had never seen it).
    pub rebuild: Option<RebuildReport>,
    /// An encrypted catalog from before PA-18, renamed to this before the rebuild.
    pub set_aside: Option<PathBuf>,
}

/// What the wizard asks for when creating.
pub struct CreateRequest<'a> {
    pub root: &'a Path,
    pub name: &'a str,
}

/// Checks the wizard's create fields before any disk work. Messages are shown as-is.
pub fn check_create(r: &CreateRequest) -> Result<(), String> {
    if r.root.as_os_str().is_empty() {
        return Err("Choose the cabinet folder.".into());
    }
    if r.name.trim().is_empty() {
        return Err("Give the cabinet a name.".into());
    }
    if cabinet::cabinet_file(r.root).exists() {
        return Err("This folder is already a cabinet. Use Open instead.".into());
    }
    Ok(())
}

/// Creates a cabinet at `r.root` (the folder may already hold client folders), then indexes it.
pub fn create(data: &Path, device_id: &str, r: &CreateRequest) -> Result<Opened> {
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
    cabinet::create(r.root, &cab).map_err(anyhow::Error::msg)?;
    // The seeded doc-type list (Q-21); rebuild reads it into the catalog.
    vf_fs::doctypes::seed_if_missing(r.root).map_err(anyhow::Error::msg)?;
    std::fs::create_dir_all(data).with_context(|| format!("creating {}", data.display()))?;
    let mut conn = db::open(&catalog, Storage::Local)?;
    let rep = vf_fs::rebuild::rebuild(&mut conn, r.root, device_id)?;
    Ok(Opened {
        root: r.root.to_owned(),
        cabinet: cab,
        conn,
        catalog,
        rebuild: Some(rep),
        set_aside: None,
    })
}

/// Renames an encrypted (pre-PA-18) catalog and its WAL / SHM files out of the way, so a plain
/// one can be built in its place. Returns the new name of the catalog file.
pub fn set_aside_encrypted(catalog: &Path) -> Result<Option<PathBuf>> {
    if db::is_plain_or_missing(catalog) {
        return Ok(None);
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let base = format!(
        "{}.sqlcipher-{secs}.bak",
        catalog.file_name().unwrap_or_default().to_string_lossy()
    );
    let target = catalog.with_file_name(&base);
    // WAL and SHM first: a stale -wal beside a fresh plain catalog would be replayed into it.
    for ext in ["-wal", "-shm"] {
        let side = PathBuf::from(format!("{}{ext}", catalog.display()));
        if side.exists() {
            let to = catalog.with_file_name(format!("{base}{ext}"));
            std::fs::rename(&side, &to)
                .with_context(|| format!("setting aside {}", side.display()))?;
        }
    }
    std::fs::rename(catalog, &target)
        .with_context(|| format!("setting aside {}", catalog.display()))?;
    Ok(Some(target))
}

/// Opens the cabinet at `root`. Rebuilds when this data folder's catalog has never indexed
/// the cabinet (first open on a workstation, or a catalog set aside by PA-18).
pub fn open(data: &Path, device_id: &str, root: &Path) -> Result<Opened> {
    let cab = cabinet::read(root).map_err(anyhow::Error::msg)?;
    std::fs::create_dir_all(data).with_context(|| format!("creating {}", data.display()))?;
    let catalog = data.join(CATALOG_FILE);
    let set_aside = set_aside_encrypted(&catalog)?;
    let mut conn = db::open(&catalog, Storage::Local)?;
    let (cabinets, known): (i64, i64) = conn.query_row(
        "SELECT count(*), coalesce(sum(id = ?1), 0) FROM cabinet",
        [&cab.cabinet_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    // One cabinet per data folder: never rebuild one cabinet over another's catalog.
    if cabinets > 0 && known == 0 {
        bail!(
            "The catalog in {} belongs to a different cabinet. Use a separate data folder for each cabinet.",
            data.display()
        );
    }
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
        catalog,
        rebuild,
        set_aside,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn req(root: &Path) -> CreateRequest<'_> {
        CreateRequest {
            root,
            name: "Kisaes Clients",
        }
    }

    /// PA-18 done-when: created and reopened with no passphrase anywhere; a plain SQLite file.
    #[test]
    fn create_then_open_on_a_fresh_workstation() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("cab");
        let a = create(&tmp.path().join("pc1"), "dev-1", &req(&root)).unwrap();
        assert_eq!(a.cabinet.name.as_deref(), Some("Kisaes Clients"));
        assert!(!cabinet::recovery_key_file(&root).exists(), "no key file");
        assert!(a.rebuild.is_some());
        drop(a);
        let head = std::fs::read(tmp.path().join("pc1").join(CATALOG_FILE)).unwrap();
        assert!(head.starts_with(b"SQLite format 3\0"));

        // Another PC: empty data folder. Opens and indexes.
        let b = open(&tmp.path().join("pc2"), "dev-2", &root).unwrap();
        assert!(b.rebuild.is_some());
        drop(b);
        // Second open on that PC: catalog already knows the cabinet, no rebuild.
        let c = open(&tmp.path().join("pc2"), "dev-2", &root).unwrap();
        assert!(c.rebuild.is_none());
    }

    /// PA-18 done-when: a workstation whose catalog was encrypted opens without a prompt and
    /// with every document indexed; the old catalog, WAL and SHM are kept aside.
    #[test]
    fn an_encrypted_catalog_is_set_aside_and_rebuilt() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("tree");
        let fx = vf_fixtures::generate(&root, 3).unwrap();
        let data = tmp.path().join("pc");
        std::fs::create_dir_all(&data).unwrap();
        // What SQLCipher leaves on disk: random salt, no SQLite header.
        std::fs::write(data.join(CATALOG_FILE), vec![0xA7; 4096]).unwrap();
        std::fs::write(data.join("catalog.db-wal"), b"").unwrap();
        std::fs::write(data.join("catalog.db-shm"), b"").unwrap();

        let o = open(&data, "dev", &root).unwrap();
        let aside = o.set_aside.clone().unwrap();
        assert!(
            aside
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains(".sqlcipher-")
        );
        assert_eq!(std::fs::read(&aside).unwrap(), vec![0xA7; 4096]);
        assert!(PathBuf::from(format!("{}-wal", aside.display())).is_file());
        assert!(PathBuf::from(format!("{}-shm", aside.display())).is_file());
        let rep = o.rebuild.as_ref().unwrap();
        assert_eq!(rep.docs_new, fx.docs.len());
        let snap = vf_core::browse::load(&o.conn).unwrap();
        assert_eq!(snap.docs.len(), fx.docs.len());
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
        let a = create(&tmp.path().join("d"), "dev", &req(&root)).unwrap();
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

    /// Without a per-cabinet key, the cabinet id is what keeps one data folder to one cabinet.
    #[test]
    fn a_data_folder_of_another_cabinet_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
        drop(create(&tmp.path().join("d"), "dev", &req(&a)).unwrap());
        drop(create(&tmp.path().join("other"), "dev", &req(&b)).unwrap());
        let e = open(&tmp.path().join("d"), "dev", &b).err().unwrap();
        assert!(e.to_string().contains("different cabinet"), "{e}");
    }

    #[test]
    fn create_refuses_an_existing_cabinet_and_empty_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("cab");
        drop(create(&tmp.path().join("d1"), "dev", &req(&root)).unwrap());
        assert!(
            check_create(&req(&root))
                .unwrap_err()
                .contains("already a cabinet")
        );
        let other = tmp.path().join("other");
        assert!(
            check_create(&CreateRequest {
                root: &other,
                name: " "
            })
            .unwrap_err()
            .contains("name")
        );
        // A data folder already holding another cabinet's catalog is refused before any write.
        let e = create(&tmp.path().join("d1"), "dev", &req(&other))
            .err()
            .unwrap();
        assert!(e.to_string().contains("another cabinet"), "{e}");
        assert!(!cabinet::cabinet_file(&other).exists());
    }
}
