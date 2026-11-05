//! A client's name and the folders inside its client folder, edited from Browse (PA-29, owner
//! 2026-11-05). Every change is a Transaction, so the undo stack takes it back:
//!
//! - [`rename_client`]: the name is saved in the client list (`clients.json`; a later CSV import
//!   puts the tax software's name back, Q-60) and the client folder is renamed to the template's
//!   name for it (`{Client} ({ID})` by default). Files inside move with the folder; catalog paths
//!   follow. The sentinel keeps the name it was created with (SentinelV1 records it as such).
//! - [`new_folder`], [`rename_folder`], [`delete_folder`]: folders under a client folder.
//!   Delete takes only an empty folder (folders inside it may be empty folders too).

use std::path::{Path, PathBuf};

use filer_core::template;
use serde_json::json;
use vf_core::audit::AuditEntry;
use vf_core::db::Connection;
use vf_core::tx::{FileOp, Stmt, Transaction};

use crate::clients;
use crate::docmeta;

pub const RENAME_CLIENT: &str = "client_rename";
pub const NEW_FOLDER: &str = "folder_create";
pub const RENAME_FOLDER: &str = "folder_rename";
pub const DELETE_FOLDER: &str = "folder_delete";

/// Filer's limit for a path (X104).
const MAX_PATH: usize = 240;

/// A folder name typed by a person: refused (not quietly changed) when Windows can't use it.
pub fn check_folder_name(name: &str) -> Result<String, String> {
    let t = name.trim();
    if t.is_empty() {
        return Err("Type a folder name.".into());
    }
    if t == "." || t == ".." {
        return Err("That is not a folder name.".into());
    }
    if t.eq_ignore_ascii_case(crate::cabinet::VIBE_DIR) {
        return Err("_Vibe is kept for Vibe Files' own files.".into());
    }
    let safe = template::safe_file_name(t).map_err(|e| format!("{e}."))?;
    if safe != t {
        return Err(
            "A folder name can't contain \\ / : * ? \" < > | or end with a dot or space.".into(),
        );
    }
    Ok(t.to_owned())
}

/// The client folder (cabinet-relative) that `rel` is in or is.
fn client_folder_of(conn: &Connection, rel: &str) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT folder_rel FROM client
         WHERE deleted_at IS NULL AND folder_rel IS NOT NULL AND folder_rel <> ''
           AND (lower(?1) = lower(folder_rel)
                OR lower(substr(?1, 1, length(folder_rel) + 1)) = lower(folder_rel || '/'))
         ORDER BY length(folder_rel) DESC LIMIT 1",
        [rel],
        |r| r.get::<_, String>(0),
    )
    // As `rel` spells it (Windows paths are not case-sensitive; SQLite's lower() is ASCII-only,
    // so the lengths agree).
    .map(|cf| rel.get(..cf.len()).map_or(cf.clone(), str::to_owned))
    .map(Some)
    .or_else(|e| {
        if e.to_string().contains("no rows") {
            Ok(None)
        } else {
            Err(e.to_string())
        }
    })
}

/// `rel` is a folder in a client folder, not in its `_Vibe`: the client folder.
fn inside_client(conn: &Connection, root: &Path, rel: &str) -> Result<String, String> {
    let rel = rel.trim_matches('/');
    let cf = client_folder_of(conn, rel)?
        .ok_or("Folders can be added, renamed and deleted inside a client folder only.")?;
    let inner = rel.strip_prefix(cf.as_str()).unwrap_or("");
    if inner
        .split('/')
        .any(|s| s.eq_ignore_ascii_case(crate::cabinet::VIBE_DIR))
    {
        return Err("_Vibe holds Vibe Files' own files.".into());
    }
    let p = template::checked_join(root, Path::new(rel)).map_err(|e| e.to_string())?;
    if !p.is_dir() {
        return Err(format!("The folder {rel} is not on disk any more."));
    }
    Ok(cf)
}

fn too_long(root: &Path, rel: &str) -> Option<String> {
    let n = root.join(rel).as_os_str().len();
    (n > MAX_PATH).then(|| {
        format!("The path would be {n} characters long (limit {MAX_PATH}); use a shorter name.")
    })
}

/// Creates folder `name` in `parent` (a client folder or a folder in one). Returns its
/// cabinet-relative path.
pub fn new_folder(
    conn: &mut Connection,
    root: &Path,
    parent: &str,
    name: &str,
    device_id: &str,
) -> Result<String, String> {
    let parent = parent.trim_matches('/');
    inside_client(conn, root, parent)?;
    let name = check_folder_name(name)?;
    let rel = format!("{parent}/{name}");
    if root.join(&rel).exists() {
        return Err(format!("{name} is already there."));
    }
    if let Some(e) = too_long(root, &rel) {
        return Err(e);
    }
    let tx = Transaction::new(NEW_FOLDER)
        .file(FileOp::Mkdir {
            path: root.join(&rel),
        })
        .audit(AuditEntry::new(NEW_FOLDER, None, json!({ "folder": rel })));
    crate::fileops::commit(conn, root, tx, device_id)?;
    Ok(rel)
}

/// Every file under `dir` (relative to it, `/`-separated), and every folder, deepest first.
fn walk(dir: &Path) -> Result<(Vec<String>, Vec<PathBuf>), String> {
    fn go(
        base: &Path,
        d: &Path,
        files: &mut Vec<String>,
        dirs: &mut Vec<PathBuf>,
    ) -> Result<(), String> {
        for e in std::fs::read_dir(d).map_err(|e| format!("{}: {e}", d.display()))? {
            let e = e.map_err(|e| e.to_string())?;
            let p = e.path();
            if e.file_type().map_err(|e| e.to_string())?.is_dir() {
                go(base, &p, files, dirs)?;
                dirs.push(p);
            } else {
                files.push(
                    p.strip_prefix(base)
                        .unwrap_or(&p)
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
        Ok(())
    }
    let (mut files, mut dirs) = (vec![], vec![]);
    go(dir, dir, &mut files, &mut dirs)?;
    Ok((files, dirs))
}

/// Moves folder `old` to `new` (both cabinet-relative) in `tx`, with the catalog paths of the
/// documents in it. `docmeta` records are rewritten when the folder is inside the client folder
/// (they are keyed relative to it); the client folder itself moves its records with it.
fn move_folder(
    conn: &Connection,
    root: &Path,
    old: &str,
    new: &str,
    whole_client: bool,
    device_id: &str,
    mut tx: Transaction,
) -> Result<(Transaction, usize), String> {
    let dst = root.join(new);
    if dst.exists() {
        return Err(format!("{new} is already there."));
    }
    let (files, _) = walk(&root.join(old))?;
    if let Some(f) = files
        .iter()
        .find(|f| too_long(root, &format!("{new}/{f}")).is_some())
    {
        return Err(format!(
            "{f} would get a path longer than {MAX_PATH} characters; use a shorter name."
        ));
    }
    // Any case: the catalog may spell a folder as the template does (`Income Tax`) while the
    // disk has `income tax` (QA 2026-11-05).
    let under = |col: &str| format!("lower(substr({col}, 1, length(?1) + 1)) = lower(?1 || '/')");
    let locked: Option<String> = conn
        .query_row(
            &format!(
                "SELECT l.user FROM document d JOIN lock l ON l.doc_id = d.id
                 WHERE d.deleted_at IS NULL AND {} LIMIT 1",
                under("d.rel_path")
            ),
            [old],
            |r| r.get(0),
        )
        .ok();
    if let Some(u) = locked {
        return Err(format!("A document in it is checked out by {u}."));
    }
    let ids: Vec<String> = {
        let mut st = conn
            .prepare(&format!(
                "SELECT id FROM document WHERE deleted_at IS NULL AND {}",
                under("rel_path")
            ))
            .map_err(|e| e.to_string())?;
        st.query_map([old], |r| r.get(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?
    };
    let now = vf_core::now_rfc3339();
    if !whole_client && !ids.is_empty() {
        let metas: Vec<docmeta::Meta> = ids
            .iter()
            .map(|id| docmeta::load(conn, id))
            .collect::<Result<_, _>>()?;
        let folders: Vec<String> = metas
            .iter()
            .filter_map(|m| m.folder_rel.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let ix = docmeta::Index::load(root, &folders);
        let mut by_file: std::collections::BTreeMap<PathBuf, (Vec<String>, Vec<String>)> =
            Default::default();
        for m in &metas {
            if ix.get(&m.rel_path, &m.hash).is_none() {
                continue;
            }
            let moved = docmeta::Meta {
                rel_path: format!("{new}{}", m.rel_path.get(old.len()..).unwrap_or("")),
                ..m.clone()
            };
            let (file, line) = docmeta::record_line(root, &moved, &now, device_id);
            let (_, back) = docmeta::record_line(root, m, &now, device_id);
            let e = by_file.entry(file).or_default();
            e.0.push(line);
            e.1.push(back);
        }
        tx = tx.file(FileOp::Move {
            src: root.join(old),
            dst,
        });
        for (path, (lines, undo_lines)) in by_file {
            tx = tx.file(FileOp::AppendLines {
                path,
                lines,
                undo_lines,
            });
        }
    } else {
        tx = tx.file(FileOp::Move {
            src: root.join(old),
            dst,
        });
    }
    let repath = |from: &str, to: &str| {
        Stmt::new(
            &format!(
                "UPDATE document SET rel_path = ?2 || substr(rel_path, length(?1) + 1),
                   device_id = ?3, updated_at = ?4, rev = rev + 1
                 WHERE {}",
                under("rel_path")
            ),
            vec![from.into(), to.into(), device_id.into(), now.clone().into()],
        )
    };
    tx = tx.catalog(repath(old, new)).catalog_undo(repath(new, old));
    Ok((tx, ids.len()))
}

/// Renames folder `rel` (inside a client folder, not the client folder itself) to `name`.
/// Returns its new cabinet-relative path.
pub fn rename_folder(
    conn: &mut Connection,
    root: &Path,
    rel: &str,
    name: &str,
    device_id: &str,
) -> Result<String, String> {
    let rel = rel.trim_matches('/');
    let cf = inside_client(conn, root, rel)?;
    if rel == cf {
        return Err("Rename the client to rename its folder.".into());
    }
    let name = check_folder_name(name)?;
    let (parent, old_name) = rel.rsplit_once('/').unwrap_or(("", rel));
    if name == old_name {
        return Err("That is its name already.".into());
    }
    let new = format!("{parent}/{name}");
    if name.eq_ignore_ascii_case(old_name) {
        return Err("Only the capital letters differ; Windows sees that as the same name.".into());
    }
    let (tx, docs) = move_folder(
        conn,
        root,
        rel,
        &new,
        false,
        device_id,
        Transaction::new(RENAME_FOLDER),
    )?;
    let tx = tx.audit(AuditEntry::new(
        RENAME_FOLDER,
        None,
        json!({ "folder": rel, "to": new, "documents": docs }),
    ));
    crate::fileops::commit(conn, root, tx, device_id)?;
    Ok(new)
}

/// Deletes folder `rel` (inside a client folder, not the client folder itself) when it holds
/// no files: it and the empty folders in it are removed.
pub fn delete_folder(
    conn: &mut Connection,
    root: &Path,
    rel: &str,
    device_id: &str,
) -> Result<(), String> {
    let rel = rel.trim_matches('/');
    let cf = inside_client(conn, root, rel)?;
    if rel == cf {
        return Err("A client folder is not deleted here.".into());
    }
    let path = root.join(rel);
    let (files, mut dirs) = walk(&path)?;
    if !files.is_empty() {
        return Err(format!(
            "{rel} still holds {} file{}; move them or put them in the trash first.",
            files.len(),
            if files.len() == 1 { "" } else { "s" }
        ));
    }
    dirs.push(path);
    let mut tx = Transaction::new(DELETE_FOLDER);
    for d in dirs {
        tx = tx.file(FileOp::Rmdir { path: d });
    }
    let tx = tx.audit(AuditEntry::new(
        DELETE_FOLDER,
        None,
        json!({ "folder": rel }),
    ));
    crate::fileops::commit(conn, root, tx, device_id)?;
    Ok(())
}

/// What renaming a client does, for the dialog's preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientRename {
    /// Client uuid.
    pub client_id: String,
    pub ext_id: String,
    pub old_name: String,
    pub new_name: String,
    /// The client folder: (now, after). `None`: no folder, or its name stays.
    pub folder: Option<(String, String)>,
}

/// Plans renaming client `client_id` (uuid) to `name`.
pub fn plan_client_rename(
    conn: &Connection,
    root: &Path,
    client_id: &str,
    name: &str,
) -> Result<ClientRename, String> {
    let new_name = name.trim().to_owned();
    if new_name.is_empty() {
        return Err("Type the client's name.".into());
    }
    let (ext_id, old_name, folder_rel): (String, String, Option<String>) = conn
        .query_row(
            "SELECT ext_id, name, folder_rel FROM client WHERE id = ?1 AND deleted_at IS NULL",
            [client_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|_| "That client is not in the catalog.".to_string())?;
    let mut folder = None;
    if let Some(cur) = folder_rel.filter(|f| !f.is_empty()) {
        let (tree, _, err) = template::load_or_seed(&clients::master_dir(root));
        if let Some(e) = err {
            return Err(e);
        }
        let listed = clients::load(root)?
            .clients()
            .iter()
            .find(|c| c.uuid.to_string() == client_id)
            .cloned();
        let c = filer_core::clients::Client {
            id: ext_id.clone(),
            uuid: uuid::Uuid::parse_str(client_id).map_err(|e| e.to_string())?,
            name: new_name.clone(),
            extra: listed.map(|c| c.extra).unwrap_or_default(),
        };
        let want = template::client_folder_name(&tree, &c).map_err(|e| e.to_string())?;
        // The client folder may sit below the cabinet root: renamed where it is.
        let (parent, cur_name) = match cur.rsplit_once('/') {
            Some((p, n)) => (format!("{p}/"), n),
            None => (String::new(), cur.as_str()),
        };
        if want != cur_name {
            let new = format!("{parent}{want}");
            if want.eq_ignore_ascii_case(cur_name) {
                return Err(format!(
                    "The folder would be {want}: only the capital letters differ, which Windows sees as the same name."
                ));
            }
            if root.join(&new).exists() {
                return Err(format!("A folder named {want} is already in the cabinet."));
            }
            folder = Some((cur.clone(), new));
        }
    }
    if new_name == old_name && folder.is_none() {
        return Err("That is the client's name already.".into());
    }
    Ok(ClientRename {
        client_id: client_id.to_owned(),
        ext_id,
        old_name,
        new_name,
        folder,
    })
}

/// Renames the client: its folder (one Transaction, undoable), then the client list, then the
/// catalog's client rows from the list and the folders.
pub fn rename_client(
    conn: &mut Connection,
    root: &Path,
    plan: &ClientRename,
    device_id: &str,
) -> Result<(), String> {
    // A client bound by its folder only (no list yet) is put on the list first.
    clients::ensure_listed(conn, root)?;
    if let Some((old, new)) = &plan.folder {
        let now = vf_core::now_rfc3339();
        let set_folder = |f: &str| {
            Stmt::new(
                "UPDATE client SET folder_rel = ?2, device_id = ?3, updated_at = ?4, rev = rev + 1
                 WHERE id = ?1",
                vec![
                    plan.client_id.clone().into(),
                    f.into(),
                    device_id.into(),
                    now.clone().into(),
                ],
            )
        };
        let tx = Transaction::new(RENAME_CLIENT)
            .catalog(set_folder(new))
            .catalog_undo(set_folder(old));
        let (tx, docs) = move_folder(conn, root, old, new, true, device_id, tx)?;
        let tx = tx.audit(AuditEntry::new(
            RENAME_CLIENT,
            None,
            json!({ "client": plan.ext_id, "folder": old, "to": new, "documents": docs }),
        ));
        crate::fileops::commit(conn, root, tx, device_id)?;
    }
    let mut list = clients::load(root)?
        .list
        .unwrap_or_else(crate::newclient::empty_list);
    let Some(c) = list
        .clients
        .iter_mut()
        .find(|c| c.uuid.to_string() == plan.client_id)
    else {
        return Err(format!(
            "{} is not on the client list; the folder was renamed, the name was not.",
            plan.ext_id
        ));
    };
    c.name = plan.new_name.clone();
    clients::save_list(root, &list)?;
    // Catalog rows following the list: not an undo of its own.
    crate::rebuild::sync_clients(conn, root, device_id, "clients_edit")
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use vf_core::db::Storage;

    use super::*;
    use crate::cabinet::{self, CabinetFile};
    use crate::newclient::{self, NewClient};

    struct Cab {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        conn: Connection,
    }

    /// A cabinet with one invented client `Smith, Pat (SMIT1001)` and a document in it.
    fn cab() -> (Cab, String) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("cabinet");
        cabinet::create(
            &root,
            &CabinetFile {
                cabinet_id: vf_core::new_id(),
                firm_id: uuid::Uuid::new_v4().to_string(),
                schema_version: vf_core::db::SCHEMA_VERSION,
                layout_version: 1,
                created: vf_core::now_rfc3339(),
                name: None,
            },
        )
        .unwrap();
        let mut conn = vf_core::db::open(&tmp.path().join("catalog.db"), Storage::Local).unwrap();
        let req = NewClient {
            ext_id: "SMIT1001".into(),
            name: "Smith, Pat".into(),
            entity_type: None,
        };
        let p = newclient::plan(&conn, &root, &req).unwrap();
        newclient::create(&mut conn, &root, &p, "dev").unwrap();
        let id = p.client.uuid.to_string();
        let rel = format!("{}/Correspondence/letter.pdf", p.folder_rel);
        std::fs::create_dir_all(root.join(&rel).parent().unwrap()).unwrap();
        std::fs::write(root.join(&rel), b"invented").unwrap();
        conn.execute(
            "INSERT INTO document (id, client_id, rel_path, hash, size, filed_at, device_id, updated_at)
             VALUES ('d1', ?1, ?2, ?3, 8, 't', 'dev', 't')",
            (&id, &rel, format!("{:064x}", 1)),
        )
        .unwrap();
        (
            Cab {
                _tmp: tmp,
                root,
                conn,
            },
            id,
        )
    }

    fn doc_path(c: &Cab) -> String {
        c.conn
            .query_row("SELECT rel_path FROM document WHERE id = 'd1'", [], |r| {
                r.get(0)
            })
            .unwrap()
    }

    #[test]
    fn folder_names_windows_cannot_use_are_refused() {
        assert_eq!(check_folder_name(" Bank ").unwrap(), "Bank");
        for bad in ["", "a/b", "a:b", "x?", "CON", "Bank.", "_vibe", ".."] {
            assert!(check_folder_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn client_rename_renames_its_folder_and_undoes() {
        let (mut c, id) = cab();
        let p = plan_client_rename(&c.conn, &c.root, &id, "Smith-Jones, Pat").unwrap();
        assert_eq!(
            p.folder,
            Some((
                "Smith, Pat (SMIT1001)".into(),
                "Smith-Jones, Pat (SMIT1001)".into()
            ))
        );
        rename_client(&mut c.conn, &c.root, &p, "dev").unwrap();
        assert!(
            c.root
                .join("Smith-Jones, Pat (SMIT1001)/Correspondence/letter.pdf")
                .is_file()
        );
        assert!(!c.root.join("Smith, Pat (SMIT1001)").exists());
        assert_eq!(
            doc_path(&c),
            "Smith-Jones, Pat (SMIT1001)/Correspondence/letter.pdf"
        );
        let (name, folder): (String, String) = c
            .conn
            .query_row(
                "SELECT name, folder_rel FROM client WHERE id = ?1",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (name.as_str(), folder.as_str()),
            ("Smith-Jones, Pat", "Smith-Jones, Pat (SMIT1001)")
        );
        let listed = clients::load(&c.root).unwrap();
        assert_eq!(listed.clients()[0].name, "Smith-Jones, Pat");
        // The sentinel still names the client (by uuid) in its new place.
        let s = filer_intake::sentinel::read_sentinel(&c.root.join(&folder))
            .unwrap()
            .unwrap();
        assert_eq!(s.client_id.to_string(), id);

        // Undo puts the folder back.
        let stack = crate::fileops::undo_stack(&c.conn, "dev").unwrap();
        let tx_id = stack
            .iter()
            .find(|e| e.label == RENAME_CLIENT)
            .unwrap()
            .tx_id
            .clone();
        crate::fileops::undo(&mut c.conn, &c.root, &tx_id, "dev").unwrap();
        assert!(
            c.root
                .join("Smith, Pat (SMIT1001)/Correspondence/letter.pdf")
                .is_file()
        );
        assert_eq!(
            doc_path(&c),
            "Smith, Pat (SMIT1001)/Correspondence/letter.pdf"
        );

        // Undo is the folder's: the name stays until it is renamed again, which then needs no
        // folder change.
        let back = plan_client_rename(&c.conn, &c.root, &id, "Smith, Pat").unwrap();
        assert_eq!(
            (back.old_name.as_str(), back.folder.clone()),
            ("Smith-Jones, Pat", None)
        );
        rename_client(&mut c.conn, &c.root, &back, "dev").unwrap();
        // The same name again, or a folder already taken, is refused.
        assert!(plan_client_rename(&c.conn, &c.root, &id, "Smith, Pat").is_err());
        std::fs::create_dir(c.root.join("Taken (SMIT1001)")).unwrap();
        assert!(plan_client_rename(&c.conn, &c.root, &id, "Taken").is_err());
    }

    #[test]
    fn folders_are_added_renamed_and_deleted_inside_a_client_folder() {
        let (mut c, _) = cab();
        let cf = "Smith, Pat (SMIT1001)";
        let made = new_folder(&mut c.conn, &c.root, cf, "Bank", "dev").unwrap();
        assert_eq!(made, format!("{cf}/Bank"));
        assert!(c.root.join(&made).is_dir());
        assert!(new_folder(&mut c.conn, &c.root, cf, "Bank", "dev").is_err());
        assert!(new_folder(&mut c.conn, &c.root, "", "Loose", "dev").is_err());
        assert!(new_folder(&mut c.conn, &c.root, &format!("{cf}/_Vibe"), "x", "dev").is_err());

        // A trashed document of the folder, and a row spelt in another case (the catalog says
        // `correspondence`, the disk `Correspondence`): both follow the rename (QA 2026-11-05).
        c.conn
            .execute(
                "INSERT INTO document (id, client_id, rel_path, hash, size, filed_at, device_id, updated_at, deleted_at)
                 VALUES ('gone', NULL, ?1, 'h', 1, 't', 'dev', 't', 't')",
                [format!("{cf}/Correspondence/old.pdf")],
            )
            .unwrap();
        c.conn
            .execute(
                "INSERT INTO document (id, client_id, rel_path, hash, size, filed_at, device_id, updated_at)
                 VALUES ('lower', NULL, ?1, 'h2', 1, 't', 'dev', 't')",
                [format!("{cf}/correspondence/note.pdf")],
            )
            .unwrap();
        // Renaming moves the documents' catalog paths with the folder.
        let to = rename_folder(
            &mut c.conn,
            &c.root,
            &format!("{cf}/Correspondence"),
            "Letters",
            "dev",
        )
        .unwrap();
        assert_eq!(to, format!("{cf}/Letters"));
        assert_eq!(doc_path(&c), format!("{cf}/Letters/letter.pdf"));
        let path_of = |id: &str| -> String {
            c.conn
                .query_row("SELECT rel_path FROM document WHERE id = ?1", [id], |r| {
                    r.get(0)
                })
                .unwrap()
        };
        assert_eq!(path_of("gone"), format!("{cf}/Letters/old.pdf"));
        assert_eq!(path_of("lower"), format!("{cf}/Letters/note.pdf"));
        assert!(c.root.join(format!("{cf}/Letters/letter.pdf")).is_file());
        assert!(rename_folder(&mut c.conn, &c.root, cf, "Other", "dev").is_err());

        // Only an empty folder is deleted (empty folders inside it go too); undo brings it back.
        assert!(delete_folder(&mut c.conn, &c.root, &format!("{cf}/Letters"), "dev").is_err());
        std::fs::create_dir(c.root.join(format!("{cf}/Bank/2025"))).unwrap();
        delete_folder(&mut c.conn, &c.root, &format!("{cf}/Bank"), "dev").unwrap();
        assert!(!c.root.join(format!("{cf}/Bank")).exists());
        let stack = crate::fileops::undo_stack(&c.conn, "dev").unwrap();
        assert_eq!(stack[0].label, DELETE_FOLDER);
        let tx_id = stack[0].tx_id.clone();
        crate::fileops::undo(&mut c.conn, &c.root, &tx_id, "dev").unwrap();
        assert!(c.root.join(format!("{cf}/Bank/2025")).is_dir());
    }
}
