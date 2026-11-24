//! Delete a client whose folder holds no documents (owner 2026-11-24): its folder and the empty
//! folders in it go, its own records (`<folder>/_Vibe`: the sentinel, document details, to-do
//! copies) are set aside in `_Vibe/client-deletes/`, and the client leaves the catalog, the
//! client list, Intake's client CSV, its relationships and its details (entity type, partner,
//! Portal contacts…). Nothing on the portal changes. The last delete can be undone from its
//! record in the cabinet, on any PC, like a client merge (`clientmerge`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use filer_core::clients::Client;
use filer_core::relationships::Relationship;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;
use vf_core::audit::AuditEntry;
use vf_core::db::Connection;
use vf_core::tx::{FileOp, Stmt, Transaction};

use crate::clients::{self, Share};
use crate::{cabinet, rebuild};

pub const LABEL: &str = "client_delete";
pub const UNDO_LABEL: &str = "client_delete_undo";
const DIR: &str = "client-deletes";

/// What deleting a client would do, for the dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Client uuid.
    pub id: String,
    pub ext_id: String,
    pub name: String,
    /// Its folder (cabinet-relative), when it has one on disk.
    pub folder: Option<String>,
    /// The folders removed (cabinet-relative), deepest first, the client folder last.
    pub dirs: Vec<String>,
    /// The folder has Vibe Files' own records (`_Vibe`): set aside, for Undo.
    pub own_records: bool,
    /// Relationships it is in (removed with it).
    pub relationships: usize,
}

/// A delete, as recorded for Undo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Deleted {
    pub tx_id: String,
    pub when: String,
    pub by: String,
    pub id: String,
    pub ext_id: String,
    pub name: String,
    pub folder: Option<String>,
    pub dirs: Vec<String>,
    /// Where its `_Vibe` was set aside (cabinet-relative), when it had one.
    pub set_aside: Option<String>,
    /// Its row of the client list, and its entries of the list's ID map.
    pub client: Option<Client>,
    #[serde(default)]
    pub uuids: Vec<(String, Uuid)>,
    #[serde(default)]
    pub rels: Vec<Relationship>,
    #[serde(default)]
    pub shares: Vec<Share>,
    /// Its details: entity_type, tin_last4, partner, b2 (key → value).
    #[serde(default)]
    pub details: BTreeMap<String, String>,
    #[serde(default)]
    pub contacts: Option<crate::contacts::ContactRecord>,
    /// Its row of Intake's client CSV, as it was.
    #[serde(default)]
    pub csv_row: Option<String>,
    /// What could not be done after the folder went (said, not an error).
    #[serde(default)]
    pub note: Option<String>,
    /// When it was undone.
    #[serde(default)]
    pub undone: Option<String>,
}

fn dir(root: &Path) -> PathBuf {
    root.join(cabinet::VIBE_DIR).join(DIR)
}

fn save(root: &Path, d: &Deleted) -> Result<(), String> {
    std::fs::create_dir_all(dir(root)).map_err(|e| e.to_string())?;
    let file = dir(root).join(format!("{}.json", d.tx_id));
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(d).expect("serializes"))
        .and_then(|()| std::fs::rename(&tmp, &file))
        .map_err(|e| format!("{}: {e}", file.display()))
}

/// The cabinet's last client delete, when it has not been undone (the newest record decides).
pub fn load_last(root: &Path) -> Option<Deleted> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir(root))
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    let last = files.pop()?;
    let d: Deleted = serde_json::from_slice(&std::fs::read(last).ok()?).ok()?;
    d.undone.is_none().then_some(d)
}

/// `rel` is inside folder `folder` (any case).
fn under(rel: &str, folder: &str) -> bool {
    rel.len() > folder.len()
        && rel.as_bytes()[folder.len()] == b'/'
        && rel.is_char_boundary(folder.len())
        && rel[..folder.len()].eq_ignore_ascii_case(folder)
}

/// Every file and folder below `dir`, as `base/...` paths.
fn walk(dir: &Path, base: &str) -> Result<(Vec<String>, Vec<String>), String> {
    fn go(
        d: &Path,
        rel: &str,
        files: &mut Vec<String>,
        dirs: &mut Vec<String>,
    ) -> Result<(), String> {
        for e in std::fs::read_dir(d).map_err(|e| format!("{}: {e}", d.display()))? {
            let e = e.map_err(|e| e.to_string())?;
            let r = format!("{rel}/{}", e.file_name().to_string_lossy());
            if e.file_type().map_err(|e| e.to_string())?.is_dir() {
                go(&e.path(), &r, files, dirs)?;
                dirs.push(r);
            } else {
                files.push(r);
            }
        }
        Ok(())
    }
    let (mut files, mut dirs) = (vec![], vec![]);
    go(dir, base, &mut files, &mut dirs)?;
    Ok((files, dirs))
}

fn count(conn: &Connection, sql: &str, id: &str) -> Result<i64, String> {
    conn.query_row(sql, [id], |r| r.get(0))
        .map_err(|e| e.to_string())
}

/// Checks that client `id` (uuid) can be deleted: no documents in the catalog (live, or in the
/// trash), no file in its folder but Vibe Files' own records, no legal hold on it.
pub fn plan(conn: &Connection, root: &Path, id: &str) -> Result<Plan, String> {
    let (ext_id, name, folder): (String, String, Option<String>) = conn
        .query_row(
            "SELECT ext_id, name, folder_rel FROM client WHERE id = ?1 AND deleted_at IS NULL",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|e| vf_core::db::missing_or(e, "That client is not in the catalog any more."))?;
    let live = count(
        conn,
        "SELECT count(*) FROM document WHERE client_id = ?1 AND deleted_at IS NULL",
        id,
    )?;
    if live > 0 {
        return Err(format!(
            "{name} has {live} document{}: move them to another client or put them in the trash, then delete the client.",
            if live == 1 { "" } else { "s" }
        ));
    }
    // Documents in the trash keep their place in the client's folder: restored, they would
    // come back to a client that is gone.
    // (In the trash: its own folder, named by its id, is in the cabinet's trash folder.)
    let trashed = {
        let mut st = conn
            .prepare("SELECT id FROM document WHERE client_id = ?1 AND deleted_at IS NOT NULL")
            .map_err(|e| e.to_string())?;
        let ids: Vec<String> = st
            .query_map([id], |r| r.get(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        let bin = cabinet::trash_dir(root);
        ids.iter().filter(|d| bin.join(d).is_dir()).count()
    };
    if trashed > 0 {
        return Err(format!(
            "{trashed} document{} of {name} {} in the trash: empty them from the trash (Admin › Trash) or restore and move them first.",
            if trashed == 1 { "" } else { "s" },
            if trashed == 1 { "is" } else { "are" }
        ));
    }
    let held = crate::holds::load(root)?
        .into_iter()
        .any(|h| h.released.is_none() && h.client_id.as_deref() == Some(id));
    if held {
        return Err(format!(
            "{name} is under a legal hold: release it first (Admin › Legal holds)."
        ));
    }
    let mut dirs = vec![];
    let mut own_records = false;
    let folder = folder.filter(|f| !f.is_empty() && root.join(f).is_dir());
    if let Some(f) = &folder {
        let (files, all) = walk(&root.join(f), f)?;
        let own = format!("{f}/{}", cabinet::VIBE_DIR);
        own_records = root.join(&own).is_dir();
        let mine = |r: &String| r.eq_ignore_ascii_case(&own) || under(r, &own);
        let others: Vec<&String> = files.iter().filter(|r| !mine(r)).collect();
        if !others.is_empty() {
            let few: Vec<&str> = others.iter().take(3).map(|s| s.as_str()).collect();
            return Err(format!(
                "{f} still holds {} file{} ({}{}): move them or put them in the trash first.",
                others.len(),
                if others.len() == 1 { "" } else { "s" },
                few.join(", "),
                if others.len() > 3 { ", …" } else { "" }
            ));
        }
        dirs = all.into_iter().filter(|r| !mine(r)).collect();
        // Deepest first: each is empty when it is removed.
        dirs.sort_by_key(|d| std::cmp::Reverse(d.matches('/').count()));
        dirs.push(f.clone());
    }
    let relationships = clients::load(root)?
        .rels
        .records
        .iter()
        .filter(|r| {
            !ext_id.is_empty()
                && (r.parent_id.eq_ignore_ascii_case(&ext_id)
                    || r.child_id.eq_ignore_ascii_case(&ext_id))
        })
        .count();
    Ok(Plan {
        id: id.into(),
        ext_id,
        name,
        folder,
        dirs,
        own_records,
        relationships,
    })
}

/// Deletes the planned client (checked again first). Returns its record, for Undo.
pub fn apply(
    conn: &mut Connection,
    root: &Path,
    planned: &Plan,
    device_id: &str,
) -> Result<Deleted, String> {
    crate::lock::with(root, "clients", "the client list", || {
        let p = plan(conn, root, &planned.id)?;
        if p != *planned {
            return Err("The client's folder changed since this was shown: look again.".into());
        }
        // Intake's CSV must lose the client too, or it is back at the next import.
        clients::intake_csv_reachable(root).map_err(|e| {
            if e.starts_with("Intake's settings") {
                e
            } else {
                "The client CSV Intake uses cannot be reached from this PC: the client must be taken out of it too.".to_string()
            }
        })?;
        let now = vf_core::now_rfc3339();
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let mut tx = Transaction::new(LABEL);
        let mut set_aside = None;
        if let Some(f) = p.folder.as_deref().filter(|_| p.own_records) {
            let aside = format!("{}/{DIR}/{}-{secs}", cabinet::VIBE_DIR, p.id);
            tx = tx.file(FileOp::Move {
                src: root.join(f).join(cabinet::VIBE_DIR),
                dst: root.join(&aside).join(cabinet::VIBE_DIR),
            });
            set_aside = Some(aside);
        }
        for d in &p.dirs {
            tx = tx.file(FileOp::Rmdir { path: root.join(d) });
        }
        let gone = |at: Option<&str>| {
            Stmt::new(
                "UPDATE client SET deleted_at = ?2, device_id = ?3, updated_at = ?4, rev = rev + 1
                 WHERE id = ?1",
                vec![
                    p.id.clone().into(),
                    at.into(),
                    device_id.into(),
                    now.clone().into(),
                ],
            )
        };
        tx = tx
            .catalog(gone(Some(&now)))
            .catalog_undo(gone(None))
            .audit(AuditEntry::new(
                LABEL,
                None,
                json!({ "client": p.ext_id, "name": p.name, "folder": p.folder }),
            ));
        let done = crate::fileops::commit(conn, root, tx, device_id)?;
        let mut d = Deleted {
            tx_id: done.tx_id,
            when: now,
            by: vf_core::windows_login().unwrap_or_default(),
            id: p.id.clone(),
            ext_id: p.ext_id.clone(),
            name: p.name.clone(),
            folder: p.folder.clone(),
            dirs: p.dirs.clone(),
            set_aside,
            client: None,
            uuids: vec![],
            rels: vec![],
            shares: vec![],
            details: BTreeMap::new(),
            contacts: None,
            csv_row: None,
            note: None,
            undone: None,
        };
        // The record first: whatever follows can be undone.
        let mut notes: Vec<String> = vec![];
        if let Err(e) = save(root, &d) {
            notes.push(format!(
                "Its record could not be written ({e}): it cannot be undone."
            ));
        }
        if let Err(e) = lists(root, &mut d) {
            notes.push(format!("The client list was not changed: {e}"));
        }
        if !d.ext_id.is_empty() {
            match clients::remove_from_csv(root, &d.ext_id) {
                Ok(row) => d.csv_row = row,
                Err(e) => notes.push(format!(
                    "{} is still in the client CSV ({e}): take the row out, or the client is back at the next import.",
                    d.ext_id
                )),
            }
        }
        if let Err(e) = rebuild::sync_clients_edited(conn, root, device_id, "clients_edit", &[]) {
            notes.push(format!(
                "The client list in this catalog was not brought up to date ({e}); it is at the next open."
            ));
        }
        if !notes.is_empty() {
            d.note = Some(notes.join(" "));
        }
        let _ = save(root, &d);
        Ok(d)
    })
}

/// The client list, relationships and details without the client (recorded in `d`).
fn lists(root: &Path, d: &mut Deleted) -> Result<(), String> {
    let mut master = clients::load(root)?;
    if let Some(mut list) = master.list.take() {
        if let Some(i) = list.clients.iter().position(|c| c.uuid.to_string() == d.id) {
            d.client = Some(list.clients.remove(i));
        }
        d.uuids = list
            .uuids
            .iter()
            .filter(|(_, v)| v.to_string() == d.id)
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        list.uuids.retain(|_, v| v.to_string() != d.id);
        clients::save_list(root, &list)?;
    }
    if !d.ext_id.is_empty() {
        let is_it = |x: &str| x.eq_ignore_ascii_case(&d.ext_id);
        let mut rels = master.rels.clone();
        let (mine, rest): (Vec<Relationship>, Vec<Relationship>) = rels
            .records
            .drain(..)
            .partition(|r| is_it(&r.parent_id) || is_it(&r.child_id));
        if !mine.is_empty() {
            d.rels = mine;
            rels.records = rest;
            clients::save_rels(root, &rels)?;
        }
    }
    let mut meta = master.meta.clone();
    let ext = d.ext_id.clone();
    let (mine, rest): (Vec<Share>, Vec<Share>) = meta.shares.drain(..).partition(|s| {
        !ext.is_empty()
            && (s.parent_id.eq_ignore_ascii_case(&ext) || s.child_id.eq_ignore_ascii_case(&ext))
    });
    d.shares = mine;
    meta.shares = rest;
    for (key, map) in [
        ("entity_type", &mut meta.entity_types),
        ("tin_last4", &mut meta.tin_last4),
        ("partner", &mut meta.partners),
        ("b2", &mut meta.b2_clients),
    ] {
        if let Some(v) = map.remove(&d.id) {
            d.details.insert(key.into(), v);
        }
    }
    d.contacts = meta.contacts.remove(&d.id);
    if meta != master.meta {
        clients::save_meta(root, &meta)?;
    }
    Ok(())
}

/// Puts a deleted client back: its folders, its own records, its place in the catalog, the
/// client list, the CSV, its relationships and details.
pub fn undo(
    conn: &mut Connection,
    root: &Path,
    d: &Deleted,
    device_id: &str,
) -> Result<(), String> {
    crate::lock::with(root, "clients", "the client list", || {
        if d.undone.is_some() {
            return Err("This delete was undone already.".into());
        }
        if let Some(f) = &d.folder
            && root.join(f).exists()
        {
            return Err(format!(
                "A folder named {f} is in the cabinet again: the client cannot be put back into it."
            ));
        }
        let now = vf_core::now_rfc3339();
        let mut tx = Transaction::new(UNDO_LABEL);
        // Shallowest first; the client folder is made first.
        let mut dirs = d.dirs.clone();
        dirs.sort_by_key(|x| x.matches('/').count());
        for x in &dirs {
            tx = tx.file(FileOp::Mkdir { path: root.join(x) });
        }
        if let (Some(f), Some(aside)) = (&d.folder, &d.set_aside)
            && root.join(aside).join(cabinet::VIBE_DIR).is_dir()
        {
            tx = tx.file(FileOp::Move {
                src: root.join(aside).join(cabinet::VIBE_DIR),
                dst: root.join(f).join(cabinet::VIBE_DIR),
            });
        }
        tx = tx
            .catalog(Stmt::new(
                "UPDATE client SET deleted_at = NULL, device_id = ?2, updated_at = ?3, rev = rev + 1
                 WHERE id = ?1",
                vec![d.id.clone().into(), device_id.into(), now.clone().into()],
            ))
            .audit(AuditEntry::new(
                UNDO_LABEL,
                None,
                json!({ "client": d.ext_id, "name": d.name, "folder": d.folder }),
            ));
        crate::fileops::commit(conn, root, tx, device_id)?;
        let mut master = clients::load(root)?;
        if let (Some(mut list), Some(c)) = (master.list.take(), d.client.clone()) {
            if !list.clients.iter().any(|x| x.uuid == c.uuid) {
                list.clients.push(c);
            }
            for (k, v) in &d.uuids {
                list.uuids.entry(k.clone()).or_insert(*v);
            }
            clients::save_list(root, &list)?;
        }
        if !d.rels.is_empty() {
            let mut rels = master.rels.clone();
            for r in &d.rels {
                if !rels.records.contains(r) {
                    rels.records.push(r.clone());
                }
            }
            clients::save_rels(root, &rels)?;
        }
        let mut meta = master.meta.clone();
        for s in &d.shares {
            if !meta.shares.contains(s) {
                meta.shares.push(s.clone());
            }
        }
        for (key, v) in &d.details {
            let map = match key.as_str() {
                "entity_type" => &mut meta.entity_types,
                "tin_last4" => &mut meta.tin_last4,
                "partner" => &mut meta.partners,
                _ => &mut meta.b2_clients,
            };
            map.entry(d.id.clone()).or_insert_with(|| v.clone());
        }
        if let Some(c) = &d.contacts {
            meta.contacts
                .entry(d.id.clone())
                .or_insert_with(|| c.clone());
        }
        if meta != master.meta {
            clients::save_meta(root, &meta)?;
        }
        if let Some(row) = &d.csv_row {
            clients::restore_csv_row(root, &d.ext_id, row)?;
        }
        rebuild::sync_clients_edited(conn, root, device_id, "clients_edit", &[])
            .map_err(|e| e.to_string())?;
        let mut done = d.clone();
        done.undone = Some(now);
        save(root, &done)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cabinet::CabinetFile;
    use vf_core::db::Storage;

    fn put(p: &Path, body: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    const EMPTY: &str = "Empty Farms LLC (2001)";
    const FULL: &str = "Busy, Pat (2002)";

    /// A cabinet with an invented client whose folder holds only empty folders and its own
    /// records, and another with a document; their CSV; a catalog. (cabinet, CSV, catalog)
    fn cabinet(dir: &Path) -> (PathBuf, PathBuf, Connection) {
        let root = dir.join("cabinet");
        let firm = Uuid::new_v4();
        cabinet::create(
            &root,
            &CabinetFile {
                cabinet_id: vf_core::new_id(),
                firm_id: firm.to_string(),
                schema_version: vf_core::db::SCHEMA_VERSION,
                layout_version: 1,
                created: vf_core::now_rfc3339(),
                name: None,
            },
        )
        .unwrap();
        for (folder, id, name) in [
            (EMPTY, "2001", "Empty Farms LLC"),
            (FULL, "2002", "Busy, Pat"),
        ] {
            let f = root.join(folder);
            std::fs::create_dir_all(f.join("Income Tax/2025")).unwrap();
            std::fs::create_dir_all(f.join("Permanent/Setup")).unwrap();
            filer_intake::sentinel::write_sentinel(
                &root,
                &f,
                &Client {
                    id: id.into(),
                    uuid: Uuid::new_v4(),
                    name: name.into(),
                    extra: BTreeMap::new(),
                },
                firm,
            )
            .unwrap();
        }
        put(
            &root.join(FULL).join("Income Tax/2025/w2.pdf"),
            "an invented W-2",
        );
        let csv = dir.join("clients.csv");
        std::fs::write(
            &csv,
            "id,name\r\n2001,Empty Farms LLC\r\n2002,\"Busy, Pat\"\r\n",
        )
        .unwrap();
        filer_core::config::Settings {
            root: Some(root.clone()),
            csv_path: Some(csv.clone()),
            column_map: filer_core::config::ColumnMap {
                id: Some("id".into()),
                name: Some("name".into()),
                extras: vec![],
                partner: None,
            },
            ..Default::default()
        }
        .save(&clients::master_dir(&root))
        .unwrap();
        let mut conn = vf_core::db::open(&dir.join("catalog.db"), Storage::Local).unwrap();
        rebuild::rebuild(&mut conn, &root, "dev").unwrap();
        clients::ensure_listed(&conn, &root).unwrap();
        clients::save_rels(
            &root,
            &filer_core::relationships::Relationships {
                records: vec![Relationship {
                    parent_id: "2001".into(),
                    child_id: "2002".into(),
                    role: "Shareholder".into(),
                    note: String::new(),
                }],
            },
        )
        .unwrap();
        (root, csv, conn)
    }

    fn id_of(conn: &Connection, ext: &str) -> String {
        conn.query_row("SELECT id FROM client WHERE ext_id = ?1", [ext], |r| {
            r.get(0)
        })
        .unwrap()
    }

    /// Owner 2026-11-24: a client whose folder holds only empty folders is deleted with its
    /// folder, leaves the list, the CSV and its relationships, and Undo puts it all back.
    #[test]
    fn an_empty_client_is_deleted_and_put_back() {
        let t = tempfile::tempdir().unwrap();
        let (root, csv, mut conn) = cabinet(t.path());
        let id = id_of(&conn, "2001");
        crate::contacts::set_by_hand(
            &root,
            &id,
            &crate::contacts::Row {
                email: "pat@example.test".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let p = plan(&conn, &root, &id).unwrap();
        assert_eq!(p.folder.as_deref(), Some(EMPTY));
        assert!(p.own_records);
        assert_eq!(p.relationships, 1);
        assert_eq!(p.dirs.last().map(String::as_str), Some(EMPTY));
        let d = apply(&mut conn, &root, &p, "dev").unwrap();
        assert_eq!(d.note, None, "{:?}", d.note);
        assert!(!root.join(EMPTY).exists(), "the folder is gone");
        assert!(!std::fs::read_to_string(&csv).unwrap().contains("2001"));
        let m = clients::load(&root).unwrap();
        assert!(m.list.unwrap().match_by_id("2001").is_none());
        assert!(m.rels.records.is_empty());
        assert!(!m.meta.contacts.contains_key(&id));
        let gone: Option<String> = conn
            .query_row("SELECT deleted_at FROM client WHERE id = ?1", [&id], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(gone.is_some());
        assert_eq!(load_last(&root).map(|x| x.id), Some(id.clone()));

        undo(&mut conn, &root, &d, "dev").unwrap();
        assert!(root.join(EMPTY).join("Income Tax/2025").is_dir());
        assert!(
            root.join(EMPTY)
                .join(cabinet::VIBE_DIR)
                .join("client.json")
                .is_file()
        );
        assert!(
            std::fs::read_to_string(&csv)
                .unwrap()
                .contains("2001,Empty Farms LLC")
        );
        let m = clients::load(&root).unwrap();
        assert!(m.list.unwrap().match_by_id("2001").is_some());
        assert_eq!(m.rels.records.len(), 1);
        assert_eq!(
            m.meta.contacts.get(&id).map(|c| c.effective().email),
            Some("pat@example.test".into())
        );
        let gone: Option<String> = conn
            .query_row("SELECT deleted_at FROM client WHERE id = ?1", [&id], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(gone.is_none());
        assert!(load_last(&root).is_none(), "undone: not offered again");
    }

    /// A client with a document, or a stray file in its folder, is not deleted.
    #[test]
    fn a_client_with_a_document_or_a_file_is_kept() {
        let t = tempfile::tempdir().unwrap();
        let (root, _csv, conn) = cabinet(t.path());
        let busy = id_of(&conn, "2002");
        let e = plan(&conn, &root, &busy).unwrap_err();
        assert!(e.contains("has 1 document"), "{e}");
        let empty = id_of(&conn, "2001");
        put(
            &root.join(EMPTY).join("Permanent/Setup/notes.txt"),
            "a stray file",
        );
        let e = plan(&conn, &root, &empty).unwrap_err();
        assert!(e.contains("still holds 1 file"), "{e}");
    }
}
