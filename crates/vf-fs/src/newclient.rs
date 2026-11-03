//! New client (P01 step 3): a folder from the cabinet template with its SentinelV1, in one
//! Transaction. Folder name and folders come from Filer's template code
//! (`template::client_folder_name`, `template::initial_dirs`), exactly as Filer creates a client
//! folder; the sentinel is written by Filer's `write_sentinel`.

use std::path::{Path, PathBuf};

use filer_core::clients::{Client, ClientList};
use filer_core::template;
use serde_json::json;
use uuid::Uuid;
use vf_core::audit::AuditEntry;
use vf_core::db::Connection;
use vf_core::tx::{FileOp, Stmt, Transaction};

use crate::cabinet;
use crate::clients::{self, master_dir};

/// What the user asks for.
#[derive(Debug, Clone, Default)]
pub struct NewClient {
    pub ext_id: String,
    pub name: String,
    /// Key from [`clients::ENTITY_TYPES`].
    pub entity_type: Option<String>,
}

/// What [`create`] will do, for the preview.
#[derive(Debug, Clone)]
pub struct Plan {
    /// With its uuid: from the master list, the catalog, or new.
    pub client: Client,
    /// The client is already in the master list (its name there is used).
    pub in_master: bool,
    pub entity_type: Option<String>,
    pub folder: PathBuf,
    pub folder_rel: String,
    /// Folders to create, cabinet-relative, parents first (the client folder first).
    pub dirs: Vec<String>,
    pub firm_id: String,
}

/// Works out the client folder for `req` and checks it can be created.
pub fn plan(conn: &Connection, root: &Path, req: &NewClient) -> Result<Plan, String> {
    let ext = req.ext_id.trim();
    if ext.is_empty() {
        return Err("Enter the client ID.".into());
    }
    if clients::is_tin_value(ext) {
        return Err(
            "That ID looks like an SSN or EIN. Use the tax software's client ID: the ID goes into folder names, the audit trail and the bucket."
                .into(),
        );
    }
    let cab = cabinet::read(root)?;
    let master = clients::load(root)?;
    let listed = master
        .list
        .as_ref()
        .and_then(|l| l.match_by_id(ext))
        .cloned();
    let name = match &listed {
        Some(c) => c.name.clone(),
        None if req.name.trim().is_empty() => return Err("Enter the client name.".into()),
        None => req.name.trim().to_owned(),
    };
    let row: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT id, folder_rel FROM client WHERE ext_id = ?1 AND deleted_at IS NULL",
            [ext],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    if let Some((_, Some(f))) = &row {
        return Err(format!("Client {ext} already has a folder: {f}"));
    }
    let uuid = match (&listed, &row) {
        (Some(c), _) => c.uuid,
        (None, Some((id, _))) => {
            Uuid::parse_str(id).map_err(|e| format!("catalog id {id} is not a uuid: {e}"))?
        }
        (None, None) => master
            .list
            .as_ref()
            .and_then(|l| l.uuids.get(ext).copied())
            .unwrap_or_else(Uuid::new_v4),
    };
    let client = Client {
        id: ext.to_owned(),
        uuid,
        name,
        extra: listed.as_ref().map(|c| c.extra.clone()).unwrap_or_default(),
    };

    let (tree, _savable, err) = template::load_or_seed(&master_dir(root));
    if let Some(e) = err {
        return Err(e);
    }
    let folder_name = template::client_folder_name(&tree, &client).map_err(|e| e.to_string())?;
    let folder = root.join(&folder_name);
    if folder.exists() {
        return Err(format!(
            "A folder named \"{folder_name}\" already exists. Binding existing folders is the folder import (P10)."
        ));
    }
    let dirs = template::initial_dirs(&tree, &client)
        .map_err(|e| e.to_string())?
        .iter()
        .map(|d| d.to_string_lossy().replace('\\', "/"))
        .collect();
    Ok(Plan {
        in_master: listed.is_some(),
        entity_type: req.entity_type.clone().filter(|e| !e.is_empty()),
        client,
        folder,
        folder_rel: folder_name,
        dirs,
        firm_id: cab.firm_id,
    })
}

/// A master list for a cabinet that has none yet (no CSV imported).
pub(crate) fn empty_list() -> ClientList {
    serde_json::from_value(json!({
        "source": "",
        "source_modified": "1970-01-01T00:00:00Z",
        "imported_at": vf_core::now_rfc3339(),
        "column_map": {},
        "skipped_rows": 0,
        "clients": [],
        "uuids": {},
    }))
    .expect("empty client list")
}

/// Creates the client: master files first (so a crash before the Transaction leaves a
/// master-only client the next rebuild shows), then folders + sentinel + catalog row + audit in
/// one Transaction. Returns the Transaction id (undo with `vf_core::tx::undo`).
pub fn create(
    conn: &mut Connection,
    root: &Path,
    plan: &Plan,
    device_id: &str,
) -> Result<String, String> {
    let mut master = clients::load(root)?;
    let c = &plan.client;
    if !plan.in_master {
        let mut list = master.list.take().unwrap_or_else(empty_list);
        list.clients.push(c.clone());
        list.uuids.insert(c.id.clone(), c.uuid);
        clients::save_list(root, &list)?;
    }
    if let Some(et) = &plan.entity_type {
        master
            .meta
            .entity_types
            .insert(c.uuid.to_string(), et.clone());
        clients::save_meta(root, &master.meta)?;
    }

    let id = c.uuid.to_string();
    let prev: Option<Option<String>> = conn
        .query_row("SELECT folder_rel FROM client WHERE id = ?1", [&id], |r| {
            r.get(0)
        })
        .ok();
    let now = vf_core::now_rfc3339();
    let entity_type = match &plan.entity_type {
        Some(e) => Some(e.clone()),
        None => master.meta.entity_types.get(&id).cloned(),
    };
    let mut tx = Transaction::new("client_create");
    for d in &plan.dirs {
        tx = tx.file(FileOp::Mkdir { path: root.join(d) });
    }
    tx = tx
        .file(FileOp::WriteSentinel {
            root: root.to_owned(),
            folder: plan.folder.clone(),
            client_id: id.clone(),
            ext_id: c.id.clone(),
            name: c.name.clone(),
            firm_id: plan.firm_id.clone(),
        })
        .catalog(Stmt::new(
            "INSERT INTO client (id, ext_id, name, entity_type, folder_rel, extra, device_id, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET
               ext_id = excluded.ext_id, name = excluded.name, entity_type = excluded.entity_type,
               folder_rel = excluded.folder_rel, extra = excluded.extra, deleted_at = NULL,
               device_id = excluded.device_id, updated_at = excluded.updated_at, rev = client.rev + 1",
            vec![
                id.clone().into(),
                c.id.clone().into(),
                c.name.clone().into(),
                entity_type.into(),
                plan.folder_rel.clone().into(),
                serde_json::to_string(&c.extra).expect("serializes").into(),
                device_id.into(),
                now.clone().into(),
            ],
        ))
        // Undo takes the folder away; the client stays in the master list (written above), so
        // its row stays too, folder-less, which is what a rebuild would give.
        .catalog_undo(Stmt::new(
            "UPDATE client SET folder_rel = ?2, rev = rev + 1, updated_at = ?3 WHERE id = ?1",
            vec![
                id.clone().into(),
                prev.flatten().into(),
                now.clone().into(),
            ],
        ))
        .audit(AuditEntry::new(
            "client_create",
            None,
            json!({ "client": c.id, "folder": plan.folder_rel, "dirs": plan.dirs.len() }),
        ));
    crate::fileops::commit(conn, root, tx, device_id).map(|c| c.tx_id)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use filer_intake::sentinel;
    use vf_core::db::Storage;

    use super::*;
    use crate::cabinet::CabinetFile;

    struct Cab {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        conn: Connection,
    }

    fn cab() -> Cab {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("cabinet");
        cabinet::create(
            &root,
            &CabinetFile {
                cabinet_id: vf_core::new_id(),
                firm_id: Uuid::new_v4().to_string(),
                schema_version: vf_core::db::SCHEMA_VERSION,
                layout_version: 1,
                created: vf_core::now_rfc3339(),
                name: None,
            },
        )
        .unwrap();
        let conn = vf_core::db::open(&tmp.path().join("catalog.db"), Storage::Local).unwrap();
        Cab {
            _tmp: tmp,
            root,
            conn,
        }
    }

    fn req(ext: &str, name: &str) -> NewClient {
        NewClient {
            ext_id: ext.into(),
            name: name.into(),
            entity_type: Some("s_corp".into()),
        }
    }

    /// Every folder under `dir`, relative, forward slashes.
    fn dirs_under(base: &Path) -> BTreeSet<String> {
        fn walk(base: &Path, d: &Path, out: &mut BTreeSet<String>) {
            for e in std::fs::read_dir(d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() && p.file_name().unwrap() != "_Vibe" {
                    out.insert(
                        p.strip_prefix(base)
                            .unwrap()
                            .to_string_lossy()
                            .replace('\\', "/"),
                    );
                    walk(base, &p, out);
                }
            }
        }
        let mut out = BTreeSet::new();
        walk(base, base, &mut out);
        out
    }

    /// P01 done-when: the folder tree matches the template and the sentinel is valid SentinelV1.
    #[test]
    fn creates_template_tree_and_valid_sentinel() {
        let mut c = cab();
        let p = plan(&c.conn, &c.root, &req("1042", "Anderson Farms LLC")).unwrap();
        assert_eq!(p.folder_rel, "Anderson Farms LLC (1042)");
        create(&mut c.conn, &c.root, &p, "dev-1").unwrap();

        let tree = template::default_tree();
        let want: BTreeSet<String> = template::initial_dirs(&tree, &p.client)
            .unwrap()
            .iter()
            .map(|d| d.to_string_lossy().replace('\\', "/"))
            .collect();
        assert_eq!(
            dirs_under(&c.root),
            want,
            "exactly the template's initial folders"
        );
        assert!(want.contains("Anderson Farms LLC (1042)/Permanent/Setup"));

        let s = sentinel::read_sentinel(&p.folder).unwrap().unwrap();
        assert_eq!(
            s.firm_id.to_string(),
            cabinet::read(&c.root).unwrap().firm_id
        );
        let list = clients::load(&c.root).unwrap().list.unwrap();
        assert_eq!(s.client_id, list.uuids["1042"]);
        assert_eq!(s.tax_software_id, "1042");
        assert_eq!(s.version, sentinel::SENTINEL_VERSION);

        let (folder, et): (String, String) = c
            .conn
            .query_row(
                "SELECT folder_rel, entity_type FROM client WHERE ext_id = '1042'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (folder.as_str(), et.as_str()),
            ("Anderson Farms LLC (1042)", "s_corp")
        );

        // Rebuild agrees (and fills the sentinel hash).
        let rep = crate::rebuild::rebuild(&mut c.conn, &c.root, "dev-1").unwrap();
        assert_eq!((rep.clients, rep.master_only), (1, 0), "{rep}");
        assert!(rep.uuid_mismatches.is_empty());

        // A second create for the same ID is refused.
        let e = plan(&c.conn, &c.root, &req("1042", "Anderson Farms LLC")).unwrap_err();
        assert!(e.contains("already has a folder"), "{e}");
    }

    #[test]
    fn undo_removes_folders_and_sentinel() {
        let mut c = cab();
        let p = plan(&c.conn, &c.root, &req("2001", "Baker Trust")).unwrap();
        let tx_id = create(&mut c.conn, &c.root, &p, "dev-1").unwrap();
        crate::fileops::undo(&mut c.conn, &c.root, &tx_id, "dev-1").unwrap();
        assert!(!p.folder.exists(), "folders the transaction made are gone");
        let folder: Option<String> = c
            .conn
            .query_row(
                "SELECT folder_rel FROM client WHERE ext_id = '2001' AND deleted_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            folder, None,
            "a folder-less client, as a rebuild would show"
        );
        let rep = crate::rebuild::rebuild(&mut c.conn, &c.root, "dev-1").unwrap();
        assert_eq!((rep.clients, rep.master_only), (0, 1), "{rep}");
    }

    #[test]
    fn master_list_client_uses_its_uuid_and_name() {
        let mut c = cab();
        let csv = c.root.parent().unwrap().join("c.csv");
        std::fs::write(&csv, "ClientID,Name\n3001,Cole Farms\n").unwrap();
        let map = filer_core::config::ColumnMap {
            id: Some("ClientID".into()),
            name: Some("Name".into()),
            extras: vec![],
        };
        let list = clients::import_csv(&c.root, &csv, &map, &Default::default()).unwrap();
        crate::rebuild::sync_clients(&mut c.conn, &c.root, "dev-1", "clients_import").unwrap();
        let p = plan(&c.conn, &c.root, &req("3001", "ignored")).unwrap();
        assert!(p.in_master);
        assert_eq!(p.client.name, "Cole Farms");
        assert_eq!(p.client.uuid, list.uuids["3001"]);
        create(&mut c.conn, &c.root, &p, "dev-1").unwrap();
        assert_eq!(
            clients::load(&c.root).unwrap().clients().len(),
            1,
            "not added twice"
        );
    }

    #[test]
    fn refuses_missing_fields_and_an_existing_folder() {
        let c = cab();
        assert!(plan(&c.conn, &c.root, &req("", "X")).is_err());
        assert!(plan(&c.conn, &c.root, &req("9", " ")).is_err());
        std::fs::create_dir(c.root.join("Dale (4001)")).unwrap();
        let e = plan(&c.conn, &c.root, &req("4001", "Dale")).unwrap_err();
        assert!(e.contains("already exists"), "{e}");
    }
}
