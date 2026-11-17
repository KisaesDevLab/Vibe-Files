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
    /// The catalog is this PC's shared one (every login of the PC uses it, Q-67).
    pub shared: bool,
    /// This copy marked as running on the catalog (other sessions' recovery leaves its
    /// changes in progress alone).
    pub _live: Option<vf_fs::fileops::Live>,
    /// The device id B2 sync uses for this catalog: on a shared catalog one for the catalog
    /// (every login of the server shares its sync state, PA-34 review); else this PC's.
    pub sync_device: String,
    /// Server mode (PA-36): this catalog's id in the cabinet's change logs.
    pub writer: Option<String>,
    /// [`open_staged`]: the catalog has the clients and no documents yet; the index goes on
    /// behind the window (`crate::indexer`, PA-60).
    pub staged: Option<vf_fs::rebuild::Staged>,
    /// The other servers' changes were read while opening. `false` after [`open_staged`] on a
    /// server: they are read behind the window, at once (owner 2026-11-15, Q-112).
    pub caught_up: bool,
}

/// What opening a cabinet is doing now and since when, said under the spinner of the opening
/// screen (owner 2026-11-15: on a session host the log cannot always be reached, and
/// "Opening the cabinet…" alone does not say what is being waited for).
static STEP: std::sync::Mutex<Option<(&'static str, std::time::Instant)>> =
    std::sync::Mutex::new(None);

/// Opening has reached `what` (also logged, with how long the step before it took).
pub fn step(what: &'static str) {
    let mut s = STEP.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((before, at)) = *s {
        tracing::info!(
            step = before,
            ms = at.elapsed().as_millis() as u64,
            "opening: step done"
        );
    }
    *s = Some((what, std::time::Instant::now()));
}

/// Opening is over (the cabinet shows, or the opening screen says why not).
pub fn steps_done() {
    let mut s = STEP.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((before, at)) = s.take() {
        tracing::info!(
            step = before,
            ms = at.elapsed().as_millis() as u64,
            "opening: step done"
        );
    }
}

/// The step opening is in, and for how long.
pub fn current_step() -> Option<(&'static str, std::time::Duration)> {
    STEP.lock()
        .unwrap_or_else(|e| e.into_inner())
        .map(|(what, at)| (what, at.elapsed()))
}

/// The device id sync uses for `conn`'s catalog: kept in the catalog, set by the first login
/// that opens it.
fn catalog_sync_device(conn: &Connection, device_id: &str) -> String {
    let _ = conn.execute(
        "INSERT OR IGNORE INTO local_state (k, v) VALUES ('sync_device_id', ?1)",
        [device_id],
    );
    conn.query_row(
        "SELECT v FROM local_state WHERE k = 'sync_device_id'",
        [],
        |r| r.get(0),
    )
    .unwrap_or_else(|_| device_id.to_string())
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
    vf_fs::retention::seed_if_missing(r.root).map_err(anyhow::Error::msg)?;
    std::fs::create_dir_all(data).with_context(|| format!("creating {}", data.display()))?;
    let mut conn = db::open(&catalog, Storage::Local)?;
    let live = vf_fs::fileops::hold_live(data);
    // From the folder listing (PA-60): the files already in the folder are not read.
    let rep = vf_fs::rebuild::index(&mut conn, r.root, device_id)?;
    Ok(Opened {
        root: r.root.to_owned(),
        cabinet: cab,
        conn,
        catalog,
        rebuild: Some(rep),
        set_aside: None,
        shared: false,
        _live: live,
        sync_device: device_id.to_string(),
        writer: None,
        staged: None,
        caught_up: true,
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
    open_how(data, device_id, root, false)
}

/// [`open`] for the window (PA-60): when the catalog does not know the cabinet's documents yet,
/// only the clients are written here and [`Opened::staged`] is what the index behind the window
/// goes on with, so nobody waits for the folder listing.
pub fn open_staged(data: &Path, device_id: &str, root: &Path) -> Result<Opened> {
    open_how(data, device_id, root, true)
}

fn open_how(data: &Path, device_id: &str, root: &Path, behind: bool) -> Result<Opened> {
    step("Reading the cabinet's own file on the share");
    let cab = cabinet::read(root).map_err(anyhow::Error::msg)?;
    std::fs::create_dir_all(data).with_context(|| format!("creating {}", data.display()))?;
    // This PC's shared catalog for the cabinet, when an admin shared one (Q-67).
    let shared_at = crate::shared::shared_catalog(&cab.cabinet_id);
    // Several servers (PA-36): every login of this server uses one catalog, made now if
    // needed (started from the cabinet's seed copy or the old catalog beside the exe).
    // A cabinet in OneDrive or Dropbox (PA-39) works the same way on each PC: its catalog
    // outside the cloud folder, started from the cabinet's seed copy (nothing read again), or
    // from this data folder's catalog when there is no seed yet.
    let in_server_mode = crate::server::server_mode();
    let cloud = !in_server_mode && crate::cloudmode::is_cloud(root);
    let server = in_server_mode || cloud;
    let shared_at = match shared_at {
        Some(p) => Some(p),
        None if server => {
            let legacy = if cloud {
                Some(data.join(CATALOG_FILE))
            } else {
                vf_core::exe_dir().map(|d| d.join("data").join(CATALOG_FILE))
            };
            step(
                "Making this computer's catalog from the copy in the cabinet (another login may be making it: this one waits)",
            );
            Some(crate::server::provision(
                &cab.cabinet_id,
                root,
                legacy.as_deref(),
            )?)
        }
        None => None,
    };
    let shared = shared_at.is_some();
    let catalog = shared_at.unwrap_or_else(|| data.join(CATALOG_FILE));
    step("Opening the catalog (after an update it is tidied once, which can take minutes)");
    let set_aside = set_aside_encrypted(&catalog)?;
    // Busy: another login on this computer is updating or tidying the catalog after an
    // update (minutes, on a large one). Waited for, said on the opening screen; it used to
    // fail after ten seconds with "database is locked" and go back to the first screen.
    let mut conn = {
        let started = std::time::Instant::now();
        loop {
            match db::open(&catalog, Storage::Local) {
                Err(e)
                    if e.is_busy() && started.elapsed() < std::time::Duration::from_secs(600) =>
                {
                    step("Waiting for the catalog: another login on this computer is updating it");
                    std::thread::sleep(std::time::Duration::from_secs(2));
                }
                r => break r?,
            }
        }
    };
    step("Reading the client list");
    // Taxpayer-ID columns a client list may carry (Filer's own wizard) are removed first.
    if let Ok(bad) = vf_fs::clients::strip_tins_on_disk(root)
        && !bad.is_empty()
    {
        tracing::warn!(
            columns = bad.len(),
            "taxpayer ID columns removed from the client list"
        );
    }
    // This copy marked as running first: its own rows from now on are not another's to
    // recover. Then a change cut short by a crash is put back before anything else reads the
    // cabinet (on a shared catalog, only those of copies no longer running).
    step("Putting back changes that were cut short");
    let live = vf_fs::fileops::hold_live(catalog.parent().unwrap_or(data));
    let rec = vf_fs::fileops::recover(&conn, root).map_err(anyhow::Error::msg)?;
    if rec.transactions > 0 {
        tracing::warn!(
            transactions = rec.transactions,
            reversed = rec.reversed,
            problems = rec.problems.len(),
            "recovered interrupted changes"
        );
    }
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
    step("Reading the cabinet's doc types and retention schedule");
    // Cabinets made before doc types existed get the seeded list now.
    let seeded = vf_fs::doctypes::seed_if_missing(root).map_err(anyhow::Error::msg)?;
    // P07: the seeded retention schedule, and the doc types' default classes (once).
    let retention = vf_fs::retention::seed_if_missing(root).map_err(anyhow::Error::msg)?;
    if (seeded || retention) && known > 0 {
        vf_fs::retention::sync(&mut conn, root, device_id, "retention_seed")
            .map_err(anyhow::Error::msg)?;
    }
    // A first index cut short (it commits in parts, PA-34) runs again until it finishes.
    // Several servers (PA-36): before a first index (hours) or after starting from a copy, the
    // change logs' ends are marked, so what other servers do meanwhile is read afterwards.
    if server {
        let w = crate::server::writer_id(&conn);
        if let Err(e) = vf_fs::changes::mark_start(&conn, root, &w) {
            tracing::warn!(kind = crate::logging::kind(&e), "change logs not marked");
        }
    }
    // From the folder listing (PA-60): no file is read. Ids, details and known fingerprints
    // come from each client folder's records, so every PC has the same catalog.
    // Behind the window: a catalog new to the cabinet, or any index cut short (owner
    // 2026-11-15, Q-112: nobody waits at "Opening the cabinet…" while files are listed; a
    // rebuild of another kind cut short used to run again here, in front of the window).
    step("Listing the client folders");
    let label = vf_fs::rebuild::unfinished_label(&conn);
    // Or a catalog that is behind Intake's folder index (PA-83): folders bound to clients
    // after it was made. Indexed the same way, behind the window.
    let stale =
        behind && known > 0 && label.is_none() && vf_fs::rebuild::behind_folder_index(&conn, root);
    let why = if stale {
        // How many, not which (folder names are client names: not for the log).
        let n = vf_fs::rebuild::folders_not_taken(&conn).len();
        tracing::info!(
            folders = n,
            "the catalog is behind Intake's folder index: indexing"
        );
        Some(format!(
            "Intake's folder list had {n} client folder{} this catalog did not",
            if n == 1 { "" } else { "s" }
        ))
    } else if let Some(l) = &label {
        tracing::info!(label = %l, "the last index was not finished: indexing");
        Some("the index before it was not finished".to_string())
    } else if known == 0 {
        tracing::info!("this catalog is new to the cabinet: indexing");
        Some("this catalog was new to the cabinet".to_string())
    } else {
        None
    };
    // Kept with the catalog: Admin › Devices says when and why the last index started.
    if let Some(why) = &why {
        let _ = conn.execute(
            "INSERT INTO local_state (k, v) VALUES ('index_started', ?1)
             ON CONFLICT(k) DO UPDATE SET v = excluded.v",
            [format!("{}|{why}", vf_core::now_rfc3339())],
        );
    }
    let in_parts = behind && (known == 0 || label.is_some() || stale);
    let mut staged = None;
    let rebuild = if in_parts {
        staged = Some(vf_fs::rebuild::index_start(&mut conn, root, device_id)?);
        None
    } else if known == 0 || label.is_some() {
        Some(vf_fs::rebuild::index(&mut conn, root, device_id)?)
    } else {
        None
    };
    // Several servers (PA-36): this catalog notes its changes for the others, and takes
    // theirs since it last looked.
    let writer = server.then(|| crate::server::writer_id(&conn));
    // For the window, theirs are read behind it (`server::Watch`, at once): a server started
    // from a copy up to a week old re-reads every folder changed since, thousands of files.
    let caught_up = !(behind && server);
    if let Some(w) = &writer {
        vf_fs::changes::set_writer(root, w);
    }
    if let Some(w) = writer.as_ref().filter(|_| caught_up) {
        step("Reading the changes made on other servers");
        match vf_fs::changes::catch_up(&mut conn, root, w, device_id) {
            Ok(f) if !f.is_empty() => {
                tracing::info!(folders = f.len(), "other servers' changes read")
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(
                kind = crate::logging::kind(&e),
                "other servers' changes not read"
            ),
        }
    }
    let sync_device = if shared {
        catalog_sync_device(&conn, device_id)
    } else {
        device_id.to_string()
    };
    Ok(Opened {
        root: root.to_owned(),
        cabinet: cab,
        conn,
        catalog,
        rebuild,
        set_aside,
        shared,
        _live: live,
        sync_device,
        writer,
        staged,
        caught_up,
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

    /// Q-112 (owner 2026-11-15): an index of any kind that was cut short goes on behind the
    /// window at the next open; nobody waits at "Opening the cabinet…" for it.
    #[test]
    fn an_index_cut_short_goes_on_behind_the_window() {
        let tmp = tempfile::tempdir().unwrap();
        let (data, root) = (tmp.path().join("pc1"), tmp.path().join("cab"));
        drop(create(&data, "dev-1", &req(&root)).unwrap());
        // A whole-cabinet rebuild (not a first index) that did not finish.
        {
            let c = db::open(&data.join(CATALOG_FILE), Storage::Local).unwrap();
            c.execute(
                "INSERT INTO local_state (k, v) VALUES ('rebuild_unfinished', 'rebuild')",
                [],
            )
            .unwrap();
        }
        let mut o = open_staged(&data, "dev-1", &root).unwrap();
        assert!(o.rebuild.is_none(), "not in front of the window");
        assert!(o.caught_up, "not a server: nothing of others to read");
        let staged = o
            .staged
            .take()
            .expect("left for the index behind the window");
        let shared = crate::indexer::Shared::new(&staged);
        crate::indexer::run(&mut o.conn, &root, "dev-1", &staged, &shared, &|| {});
        assert!(shared.finished());
        assert_eq!(vf_fs::rebuild::unfinished_label(&o.conn), None);
        drop(o);
        let again = open_staged(&data, "dev-1", &root).unwrap();
        assert!(again.staged.is_none() && again.rebuild.is_none());
    }

    /// PA-83 (owner 2026-11-16): a catalog made before a folder was bound to its client
    /// (Intake's Setup binds folders without the catalog) showed the client with no folder
    /// and no documents for good. When Intake's folder index has a bound folder the catalog
    /// has not, the cabinet is indexed behind the window, once for that scan.
    #[test]
    fn a_catalog_behind_intakes_folder_index_is_indexed() {
        let tmp = tempfile::tempdir().unwrap();
        let (pc1, pc2, root) = (
            tmp.path().join("pc1"),
            tmp.path().join("pc2"),
            tmp.path().join("cab"),
        );
        let mut a = create(&pc1, "dev-1", &req(&root)).unwrap();
        // The second PC's catalog is made now: it knows the cabinet, and no client.
        drop(open(&pc2, "dev-2", &root).unwrap());
        // A client's folder, bound, with a file; under a name of its own.
        let plan = vf_fs::newclient::plan(
            &a.conn,
            &root,
            &vf_fs::newclient::NewClient {
                ext_id: "1042".into(),
                name: "Anderson Farms LLC".into(),
                entity_type: None,
                partner: String::new(),
            },
        )
        .unwrap();
        vf_fs::newclient::create(&mut a.conn, &root, &plan, "dev-1").unwrap();
        drop(a);
        let folder = root.join("01 Anderson");
        std::fs::rename(root.join(&plan.folder_rel), &folder).unwrap();
        std::fs::write(folder.join("letter.pdf"), "invented").unwrap();
        let index = |at: &str| {
            let f = serde_json::json!({
                "root": root, "scanned_at": at,
                "folders": [{ "path": folder, "client_id": "1042", "status": "bound" }],
            });
            std::fs::write(root.join("_Vibe/folders.json"), f.to_string()).unwrap();
        };
        index("2026-11-12T22:57:00Z");
        assert_eq!(
            vf_fs::rebuild::bound_folder_of(&root, "1042").as_deref(),
            Some("01 Anderson")
        );

        let mut o = open_staged(&pc2, "dev-2", &root).unwrap();
        let staged = o.staged.take().expect("indexed behind the window");
        let shared = crate::indexer::Shared::new(&staged);
        crate::indexer::run(&mut o.conn, &root, "dev-2", &staged, &shared, &|| {});
        let (folder_rel, docs): (String, i64) = o
            .conn
            .query_row(
                "SELECT c.folder_rel, (SELECT count(*) FROM document d WHERE d.client_id = c.id
                                        AND d.deleted_at IS NULL)
                 FROM client c WHERE c.ext_id = '1042'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((folder_rel.as_str(), docs), ("01 Anderson", 1));
        drop(o);
        // In line now: no index at the next open, nor after another scan of the root.
        assert!(open_staged(&pc2, "dev-2", &root).unwrap().staged.is_none());
        index("2026-11-16T11:07:00Z");
        assert!(open_staged(&pc2, "dev-2", &root).unwrap().staged.is_none());

        // Owner 2026-11-17: a folder Intake's index binds that no index can take (here: its
        // own client file names another client) is indexed for once, not after every scan.
        let odd = |at: &str| {
            let f = serde_json::json!({
                "root": root, "scanned_at": at,
                "folders": [
                    { "path": folder, "client_id": "1042", "status": "bound" },
                    { "path": folder, "client_id": "9999", "status": "bound" },
                ],
            });
            std::fs::write(root.join("_Vibe/folders.json"), f.to_string()).unwrap();
        };
        odd("2026-11-17T08:00:00Z");
        let mut o = open_staged(&pc2, "dev-2", &root).unwrap();
        let staged = o.staged.take().expect("tried once");
        let shared = crate::indexer::Shared::new(&staged);
        crate::indexer::run(&mut o.conn, &root, "dev-2", &staged, &shared, &|| {});
        assert_eq!(vf_fs::rebuild::folders_not_taken(&o.conn).len(), 1);
        drop(o);
        odd("2026-11-18T08:00:00Z");
        assert!(
            open_staged(&pc2, "dev-2", &root).unwrap().staged.is_none(),
            "the same folder is not indexed for again after another scan"
        );
    }
    /// PA-86: Admin › Devices says what the catalog in use holds, and "Index the cabinet
    /// again" has the next open index it behind the window.
    #[test]
    fn the_catalogs_facts_and_indexing_it_again() {
        let tmp = tempfile::tempdir().unwrap();
        let (data, root) = (tmp.path().join("pc1"), tmp.path().join("cab"));
        let o = create(&data, "dev-1", &req(&root)).unwrap();
        let file = data.join(CATALOG_FILE);
        let f = crate::ui::devices::facts(&file).unwrap();
        assert_eq!((f.documents, f.clients, f.clients_with_folder), (0, 0, 0));
        assert!(f.bytes.is_some_and(|b| b > 0) && !f.index_due);
        vf_fs::rebuild::ask_for_index(&o.conn).unwrap();
        drop(o);
        assert!(crate::ui::devices::facts(&file).unwrap().index_due);
        let mut again = open_staged(&data, "dev-1", &root).unwrap();
        let staged = again.staged.take().expect("indexed behind the window");
        let shared = crate::indexer::Shared::new(&staged);
        crate::indexer::run(&mut again.conn, &root, "dev-1", &staged, &shared, &|| {});
        drop(again);
        assert!(!crate::ui::devices::facts(&file).unwrap().index_due);
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
