//! `vibe-files rebuild <cabinet>`: reproduce the catalog's `cabinet`, `client`, `document` and
//! `version` rows from the folder tree (CLAUDE.md principle 2). Idempotent: a second run over an
//! unchanged tree changes nothing.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use filer_intake::commit::sha256_file;
use filer_intake::sentinel;
use serde_json::json;
use vf_core::audit::AuditEntry;
use vf_core::db::Connection;
use vf_core::tx::{Actor, NoFileOps, Stmt, Transaction};

use crate::cabinet::{self, VersionSidecar};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RebuildReport {
    pub clients: usize,
    pub unbound_folders: Vec<String>,
    pub duplicate_folders: Vec<String>,
    pub docs_new: usize,
    pub docs_changed: usize,
    pub docs_moved: usize,
    pub docs_unchanged: usize,
    pub docs_tombstoned: usize,
    pub versions_new: usize,
    /// Clients in the master list with no bound folder (catalog rows with no `folder_rel`).
    pub master_only: usize,
    pub relationships: usize,
    /// Relationships naming a CSV ID that is neither bound nor in the master list.
    pub dangling_relationships: usize,
    /// Master-list IDs whose uuid differs from the catalog's/sentinel's (the folder wins).
    pub uuid_mismatches: Vec<String>,
    /// The client master could not be read; clients came from sentinels only.
    pub master_error: Option<String>,
}

impl std::fmt::Display for RebuildReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "clients {} · documents: {} new, {} changed, {} moved, {} unchanged, {} tombstoned · versions {} new",
            self.clients,
            self.docs_new,
            self.docs_changed,
            self.docs_moved,
            self.docs_unchanged,
            self.docs_tombstoned,
            self.versions_new
        )?;
        if self.master_only + self.relationships > 0 {
            write!(
                f,
                " · {} clients without a folder · {} relationships",
                self.master_only, self.relationships
            )?;
        }
        if let Some(e) = &self.master_error {
            write!(
                f,
                "
client master not read (clients from folders only): {e}"
            )?;
        }
        if self.dangling_relationships > 0 {
            write!(
                f,
                "
relationships naming an unknown client ID: {}",
                self.dangling_relationships
            )?;
        }
        if !self.uuid_mismatches.is_empty() {
            write!(
                f,
                "
client IDs whose master-list uuid differs from their folder (folder kept): {}",
                self.uuid_mismatches.join(", ")
            )?;
        }
        if !self.unbound_folders.is_empty() {
            write!(
                f,
                "\nfolders without a valid sentinel (documents kept, no client): {}",
                self.unbound_folders.join(", ")
            )?;
        }
        if !self.duplicate_folders.is_empty() {
            write!(
                f,
                "\nduplicate sentinels (resolve in Conflicts): {}",
                self.duplicate_folders.join(", ")
            )?;
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RebuildError {
    #[error("{0}")]
    Cabinet(String),
    #[error("{0}")]
    Io(String),
    #[error(transparent)]
    Catalog(#[from] vf_core::Error),
}

impl From<vf_core::db::SqlError> for RebuildError {
    fn from(e: vf_core::db::SqlError) -> Self {
        RebuildError::Catalog(e.into())
    }
}

/// A file found on disk.
struct Found {
    rel: String,
    client_id: Option<String>,
    hash: String,
    size: u64,
}

/// Cabinet-relative path with forward slashes (stable across Windows/UNC roots).
fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Same rule as Filer's root scan (X28): `_x`, `.x`, `~x` are not client folders.
fn skipped(name: &str) -> bool {
    name.starts_with(['_', '.', '~'])
}

/// Files under `dir`, skipping `_Vibe` folders and Filer's partial-copy temp files.
fn files_under(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), RebuildError> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| RebuildError::Io(format!("cannot read {}: {e}", dir.display())))?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            if name != cabinet::VIBE_DIR {
                files_under(&e.path(), out)?;
            }
        } else if !name.starts_with("~vfd-partial-") {
            out.push(e.path());
        }
    }
    Ok(())
}

/// First path segment that is a plausible tax year (1990..=2100); `None` = permanent file.
fn year_from_path(rel: &str) -> Option<i64> {
    rel.split('/').find_map(|seg| {
        (seg.len() == 4)
            .then(|| seg.parse::<i64>().ok())
            .flatten()
            .filter(|y| (1990..=2100).contains(y))
    })
}

fn mime(rel: &str) -> &'static str {
    let ext = rel.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xls" => "application/vnd.ms-excel",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "doc" => "application/msword",
        "csv" => "text/csv",
        "txt" => "text/plain",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "tif" | "tiff" => "image/tiff",
        "heic" => "image/heic",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

/// A client folder bound by a readable, non-duplicate sentinel.
struct Bound {
    id: String,
    ext_id: String,
    name: String,
    folder_rel: String,
    sentinel_hash: String,
}

/// Top-level folders of `root` in name order, each with its client id when bound. Bound
/// clients are returned once; duplicate and unbound folders go to the report.
#[allow(clippy::type_complexity)]
fn scan_folders(
    root: &Path,
    rep: &mut RebuildReport,
) -> Result<(Vec<Bound>, Vec<(PathBuf, Option<String>)>), RebuildError> {
    let mut tops: Vec<_> = std::fs::read_dir(root)
        .map_err(|e| RebuildError::Io(format!("cannot read {}: {e}", root.display())))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter(|e| !skipped(&e.file_name().to_string_lossy()))
        .map(|e| e.path())
        .collect();
    tops.sort();
    let mut bound: Vec<Bound> = Vec::new();
    let mut folders = Vec::with_capacity(tops.len());
    let mut seen_clients: HashSet<String> = HashSet::new();
    let mut seen_ext: HashSet<String> = HashSet::new();
    for folder in tops {
        let folder_rel = rel(root, &folder);
        let client_id = match sentinel::read_sentinel(&folder) {
            Ok(Some(s)) => {
                let id = s.client_id.to_string();
                if !seen_clients.insert(id.clone()) || !seen_ext.insert(s.tax_software_id.clone()) {
                    rep.duplicate_folders.push(folder_rel);
                    None
                } else {
                    let sentinel_hash = sha256_file(&sentinel::sentinel_path(&folder))
                        .map_err(|e| RebuildError::Io(e.to_string()))?;
                    bound.push(Bound {
                        id: id.clone(),
                        ext_id: s.tax_software_id,
                        name: s.display_name_at_creation,
                        folder_rel,
                        sentinel_hash,
                    });
                    rep.clients += 1;
                    Some(id)
                }
            }
            Ok(None) | Err(_) => {
                rep.unbound_folders.push(folder_rel);
                None
            }
        };
        folders.push((folder, client_id));
    }
    Ok((bound, folders))
}

/// Client and relationship rows from the bound folders merged with the client master
/// (`crate::clients`). The folder decides a client's uuid and `folder_rel`; the master list
/// decides its name and CSV extras; the sidecar gives entity type and shares. Master-list clients
/// with no folder get a row with no `folder_rel`. Relationships not in the master are tombstoned.
fn client_stmts(
    conn: &Connection,
    root: &Path,
    bound: &[Bound],
    now: &str,
    device_id: &str,
    rep: &mut RebuildReport,
) -> Result<Vec<Stmt>, RebuildError> {
    let master = match crate::clients::load(root) {
        Ok(m) => Some(m),
        Err(e) => {
            rep.master_error = Some(e);
            None
        }
    };
    let empty = crate::clients::Master::default();
    let m = master.as_ref().unwrap_or(&empty);
    let listed: HashMap<&str, &filer_core::clients::Client> =
        m.clients().iter().map(|c| (c.id.as_str(), c)).collect();

    // Live catalog ext_id -> id, to keep the unique ext_id index intact.
    let mut catalog_ext: HashMap<String, String> = HashMap::new();
    {
        let mut st = conn.prepare("SELECT ext_id, id FROM client WHERE deleted_at IS NULL")?;
        for r in st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))? {
            let (e, i): (String, String) = r?;
            catalog_ext.insert(e, i);
        }
    }

    let upsert = |id: &str,
                  ext: &str,
                  name: &str,
                  folder_rel: Option<&str>,
                  sentinel_hash: Option<&str>,
                  extra: String| {
        Stmt::new(
            "INSERT INTO client (id, ext_id, name, entity_type, folder_rel, sentinel_hash, extra, device_id, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET
               ext_id = excluded.ext_id, name = excluded.name, entity_type = excluded.entity_type,
               folder_rel = excluded.folder_rel, sentinel_hash = excluded.sentinel_hash,
               extra = excluded.extra, deleted_at = NULL,
               device_id = excluded.device_id, updated_at = excluded.updated_at, rev = client.rev + 1
             WHERE (client.ext_id, client.name, client.entity_type, client.folder_rel,
                    client.sentinel_hash, client.extra, client.deleted_at)
               IS NOT (excluded.ext_id, excluded.name, excluded.entity_type, excluded.folder_rel,
                       excluded.sentinel_hash, excluded.extra, NULL)",
            vec![
                id.into(),
                ext.into(),
                name.into(),
                m.meta.entity_types.get(id).cloned().into(),
                folder_rel.map(str::to_owned).into(),
                sentinel_hash.map(str::to_owned).into(),
                extra.into(),
                device_id.into(),
                now.into(),
            ],
        )
    };
    let extras = |c: Option<&filer_core::clients::Client>| {
        c.map(|c| serde_json::to_string(&c.extra).expect("serializes"))
            .unwrap_or_else(|| "{}".into())
    };

    let mut stmts = Vec::new();
    let mut ext_to_id: HashMap<String, String> = HashMap::new();
    for b in bound {
        let l = listed.get(b.ext_id.as_str()).copied();
        if let Some(c) = l
            && c.uuid.to_string() != b.id
        {
            rep.uuid_mismatches.push(b.ext_id.clone());
        }
        let name = l.map(|c| c.name.as_str()).unwrap_or(&b.name);
        stmts.push(upsert(
            &b.id,
            &b.ext_id,
            name,
            Some(&b.folder_rel),
            Some(&b.sentinel_hash),
            extras(l),
        ));
        ext_to_id.insert(b.ext_id.clone(), b.id.clone());
    }
    for c in m.clients() {
        if ext_to_id.contains_key(&c.id) {
            continue;
        }
        let id = c.uuid.to_string();
        if catalog_ext.get(&c.id).is_some_and(|other| *other != id) {
            rep.uuid_mismatches.push(c.id.clone());
            continue;
        }
        stmts.push(upsert(&id, &c.id, &c.name, None, None, extras(Some(c))));
        ext_to_id.insert(c.id.clone(), id);
        rep.master_only += 1;
    }

    let Some(m) = master.as_ref() else {
        return Ok(stmts); // unreadable master: leave relationships as they are
    };
    let mut live: HashSet<(String, String, String)> = HashSet::new();
    {
        let mut st = conn.prepare(
            "SELECT parent_id, child_id, kind FROM client_relationship WHERE deleted_at IS NULL",
        )?;
        for r in st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))? {
            live.insert(r?);
        }
    }
    let mut kept: HashSet<(String, String, String)> = HashSet::new();
    for r in &m.rels.records {
        let (Some(p), Some(c)) = (ext_to_id.get(&r.parent_id), ext_to_id.get(&r.child_id)) else {
            rep.dangling_relationships += 1;
            continue;
        };
        let note = Some(r.note.trim())
            .filter(|n| !n.is_empty())
            .map(str::to_owned);
        stmts.push(Stmt::new(
            "INSERT INTO client_relationship (parent_id, child_id, kind, share_pct, note, device_id, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(parent_id, child_id, kind) DO UPDATE SET
               share_pct = excluded.share_pct, note = excluded.note, deleted_at = NULL,
               device_id = excluded.device_id, updated_at = excluded.updated_at,
               rev = client_relationship.rev + 1
             WHERE (client_relationship.share_pct, client_relationship.note, client_relationship.deleted_at)
               IS NOT (excluded.share_pct, excluded.note, NULL)",
            vec![
                p.clone().into(),
                c.clone().into(),
                r.role.trim().into(),
                m.meta.share(&r.parent_id, &r.child_id, &r.role).into(),
                note.into(),
                device_id.into(),
                now.into(),
            ],
        ));
        kept.insert((p.clone(), c.clone(), r.role.trim().to_owned()));
        rep.relationships += 1;
    }
    let mut gone: Vec<_> = live.difference(&kept).collect();
    gone.sort();
    for (p, c, k) in gone {
        stmts.push(Stmt::new(
            "UPDATE client_relationship SET deleted_at = ?4, device_id = ?5, updated_at = ?4, rev = rev + 1
             WHERE parent_id = ?1 AND child_id = ?2 AND kind = ?3 AND deleted_at IS NULL",
            vec![
                p.clone().into(),
                c.clone().into(),
                k.clone().into(),
                now.into(),
                device_id.into(),
            ],
        ));
    }
    Ok(stmts)
}

/// Brings the catalog's client and relationship rows in line with the folders and the client
/// master, without re-hashing documents. Run after any change to the master files.
pub fn sync_clients(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    action: &str,
) -> Result<RebuildReport, RebuildError> {
    let now = vf_core::now_rfc3339();
    let mut rep = RebuildReport::default();
    let (bound, _) = scan_folders(root, &mut rep)?;
    let stmts = client_stmts(conn, root, &bound, &now, device_id, &mut rep)?;
    let mut tx = Transaction::new(action).audit(AuditEntry::new(
        action,
        None,
        json!({
            "clients": rep.clients, "master_only": rep.master_only,
            "relationships": rep.relationships,
            "dangling_relationships": rep.dangling_relationships,
        }),
    ));
    for s in stmts {
        tx = tx.catalog(s);
    }
    let actor = Actor {
        user: None,
        device_id: device_id.into(),
    };
    tx.commit(conn, &mut NoFileOps, &actor)?;
    Ok(rep)
}

pub fn rebuild(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
) -> Result<RebuildReport, RebuildError> {
    let cab = cabinet::read(root).map_err(RebuildError::Cabinet)?;
    let now = vf_core::now_rfc3339();
    let mut rep = RebuildReport::default();
    let mut stmts: Vec<Stmt> = Vec::new();

    stmts.push(Stmt::new(
        "INSERT INTO cabinet (id, firm_id, root_path, schema_ver, layout_ver, created)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(id) DO UPDATE SET root_path = excluded.root_path
         WHERE cabinet.root_path IS NOT excluded.root_path",
        vec![
            cab.cabinet_id.clone().into(),
            cab.firm_id.clone().into(),
            root.to_string_lossy().into_owned().into(),
            cab.schema_version.into(),
            cab.layout_version.into(),
            cab.created.clone().into(),
        ],
    ));

    let (bound, folders) = scan_folders(root, &mut rep)?;
    let mut found: Vec<Found> = Vec::new();
    for (folder, client_id) in &folders {
        let mut paths = Vec::new();
        files_under(folder, &mut paths)?;
        for p in paths {
            let hash = sha256_file(&p)
                .map_err(|e| RebuildError::Io(format!("cannot hash {}: {e}", p.display())))?;
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            found.push(Found {
                rel: rel(root, &p),
                client_id: client_id.clone(),
                hash,
                size,
            });
        }
    }

    stmts.extend(client_stmts(conn, root, &bound, &now, device_id, &mut rep)?);

    // Documents: compare with live catalog rows by path; re-link moves by hash.
    let mut live: HashMap<String, (String, String)> = HashMap::new(); // rel -> (id, hash)
    {
        let mut st =
            conn.prepare("SELECT rel_path, id, hash FROM document WHERE deleted_at IS NULL")?;
        for r in st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))? {
            let (p, id, h): (String, String, String) = r?;
            live.insert(p, (id, h));
        }
    }
    let on_disk: HashSet<&str> = found.iter().map(|f| f.rel.as_str()).collect();
    // Missing rows, by hash, available to absorb a move.
    let mut missing_by_hash: BTreeMap<String, Vec<String>> = BTreeMap::new(); // hash -> ids
    let mut missing_ids: Vec<String> = Vec::new();
    let mut live_paths: Vec<_> = live.iter().collect();
    live_paths.sort();
    for (p, (id, h)) in live_paths {
        if !on_disk.contains(p.as_str()) {
            missing_by_hash
                .entry(h.clone())
                .or_default()
                .push(id.clone());
            missing_ids.push(id.clone());
        }
    }
    let mut moved: HashSet<String> = HashSet::new();
    let mut doc_id_by_rel: HashMap<String, String> = HashMap::new();
    for f in &found {
        let year = year_from_path(&f.rel)
            .map(serde_json::Value::from)
            .unwrap_or_default();
        match live.get(&f.rel) {
            Some((id, h)) if *h == f.hash => {
                rep.docs_unchanged += 1;
                doc_id_by_rel.insert(f.rel.clone(), id.clone());
            }
            Some((id, _)) => {
                // Changed outside the app: keep the old content's identity as a version row.
                stmts.push(Stmt::new(
                    "INSERT OR IGNORE INTO version (doc_id, hash, size, created, created_by, device_id, reason)
                     SELECT id, hash, size, ?2, 'rebuild', ?3, 'changed on disk' FROM document WHERE id = ?1",
                    vec![id.clone().into(), now.clone().into(), device_id.into()],
                ));
                stmts.push(Stmt::new(
                    "UPDATE document SET hash = ?2, size = ?3, device_id = ?4, updated_at = ?5, rev = rev + 1 WHERE id = ?1",
                    vec![id.clone().into(), f.hash.clone().into(), f.size.into(), device_id.into(), now.clone().into()],
                ));
                rep.docs_changed += 1;
                doc_id_by_rel.insert(f.rel.clone(), id.clone());
            }
            None => {
                if let Some(id) = missing_by_hash.get_mut(&f.hash).and_then(|v| v.pop()) {
                    stmts.push(Stmt::new(
                        "UPDATE document SET rel_path = ?2, client_id = ?3, device_id = ?4, updated_at = ?5, rev = rev + 1 WHERE id = ?1",
                        vec![id.clone().into(), f.rel.clone().into(), f.client_id.clone().into(), device_id.into(), now.clone().into()],
                    ));
                    moved.insert(id.clone());
                    rep.docs_moved += 1;
                    doc_id_by_rel.insert(f.rel.clone(), id);
                } else {
                    let id = vf_core::new_id();
                    stmts.push(Stmt::new(
                        "INSERT INTO document (id, client_id, tax_year, rel_path, hash, size, mime, status, filed_by, filed_at, device_id, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'received', 'rebuild', ?8, ?9, ?8)",
                        vec![
                            id.clone().into(), f.client_id.clone().into(), year, f.rel.clone().into(),
                            f.hash.clone().into(), f.size.into(), mime(&f.rel).into(), now.clone().into(), device_id.into(),
                        ],
                    ));
                    rep.docs_new += 1;
                    doc_id_by_rel.insert(f.rel.clone(), id);
                }
            }
        }
    }
    for id in missing_ids.iter().filter(|id| !moved.contains(*id)) {
        stmts.push(Stmt::new(
            "UPDATE document SET deleted_at = ?2, tombstone_reason = 'missing on rebuild', device_id = ?3, updated_at = ?2, rev = rev + 1 WHERE id = ?1",
            vec![id.clone().into(), now.clone().into(), device_id.into()],
        ));
        rep.docs_tombstoned += 1;
    }

    // Versions from sidecars.
    let vdir = root.join(cabinet::VIBE_DIR).join(cabinet::VERSIONS_DIR);
    if vdir.is_dir() {
        let mut sidecars = Vec::new();
        files_under(&vdir, &mut sidecars)?;
        for sc in sidecars
            .iter()
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
        {
            let hash = sc
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let content = sc.with_extension("");
            let Ok(size) = std::fs::metadata(&content).map(|m| m.len()) else {
                continue; // sidecar without its content: nothing to restore
            };
            let text = std::fs::read_to_string(sc)
                .map_err(|e| RebuildError::Io(format!("cannot read {}: {e}", sc.display())))?;
            let entries: Vec<VersionSidecar> = serde_json::from_str(&text)
                .map_err(|e| RebuildError::Io(format!("{} is not valid: {e}", sc.display())))?;
            for v in entries {
                let Some(doc_id) = doc_id_by_rel.get(&v.rel_path) else {
                    continue;
                };
                stmts.push(Stmt::new(
                    "INSERT OR IGNORE INTO version (doc_id, hash, size, created, created_by, device_id, reason)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    vec![
                        doc_id.clone().into(), hash.clone().into(), size.into(), v.created.into(),
                        v.created_by.into(), v.device_id.into(), v.reason.into(),
                    ],
                ));
                rep.versions_new += 1;
            }
        }
    }
    let before_versions: i64 = conn.query_row("SELECT count(*) FROM version", [], |r| r.get(0))?;

    let changed = rep.docs_new + rep.docs_changed + rep.docs_moved + rep.docs_tombstoned;
    let mut tx = Transaction::new("rebuild").audit(AuditEntry::new(
        "rebuild",
        None,
        json!({
            "clients": rep.clients, "docs_new": rep.docs_new, "docs_changed": rep.docs_changed,
            "docs_moved": rep.docs_moved, "docs_tombstoned": rep.docs_tombstoned,
            "unbound_folders": rep.unbound_folders.len(), "duplicate_folders": rep.duplicate_folders.len(),
            "documents_changed": changed,
        }),
    ));
    for s in stmts {
        tx = tx.catalog(s);
    }
    let actor = Actor {
        user: None,
        device_id: device_id.into(),
    };
    tx.commit(conn, &mut NoFileOps, &actor)?;
    let after_versions: i64 = conn.query_row("SELECT count(*) FROM version", [], |r| r.get(0))?;
    rep.versions_new = (after_versions - before_versions).max(0) as usize;
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use filer_core::clients::Client;
    use uuid::Uuid;
    use vf_core::db::{CatalogKey, Storage};

    use super::*;
    use crate::cabinet::CabinetFile;

    struct Cab {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        data: PathBuf,
    }

    fn client_folder(root: &Path, name: &str, id: &str, firm: Uuid) -> PathBuf {
        let folder = root.join(format!("{name} ({id})"));
        std::fs::create_dir_all(&folder).unwrap();
        let c = Client {
            id: id.into(),
            uuid: Uuid::new_v4(),
            name: name.into(),
            extra: BTreeMap::new(),
        };
        sentinel::write_sentinel(root, &folder, &c, firm).unwrap();
        folder
    }

    fn put(p: &Path, body: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn cab() -> Cab {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("cabinet");
        let data = tmp.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let firm = Uuid::new_v4();
        crate::cabinet::create(
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
        let a = client_folder(&root, "Smith, John", "1001", firm);
        put(&a.join("Income Tax/2025/w2.pdf"), "w2 2025");
        put(&a.join("Income Tax/2024/w2.pdf"), "w2 2024");
        put(&a.join("Permanent/Setup/ein letter.pdf"), "ein");
        put(&a.join("Income Tax/2025/~vfd-partial-abc-w2.pdf"), "temp");
        let b = client_folder(&root, "Acme LLC", "1002", firm);
        put(&b.join("Payroll/2025/941 Q1.pdf"), "941");
        put(&root.join("Loose Folder/notes.pdf"), "loose");
        put(&root.join("_Vibe/audit/2026-11.jsonl"), "{}");
        Cab {
            _tmp: tmp,
            root,
            data,
        }
    }

    fn open(c: &Cab) -> Connection {
        vf_core::db::open(
            &c.data.join("catalog.db"),
            &CatalogKey([4; 32]),
            Storage::Local,
        )
        .unwrap()
    }

    fn n(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn rebuild_counts_and_is_idempotent() {
        let c = cab();
        let mut conn = open(&c);
        let rep = rebuild(&mut conn, &c.root, "dev-1").unwrap();
        assert_eq!(rep.clients, 2);
        assert_eq!(rep.docs_new, 5, "{rep}");
        assert_eq!(rep.unbound_folders, ["Loose Folder"]);
        assert_eq!(n(&conn, "SELECT count(*) FROM client"), 2);
        assert_eq!(n(&conn, "SELECT count(*) FROM document"), 5);
        assert_eq!(
            n(
                &conn,
                "SELECT count(*) FROM document WHERE client_id IS NULL"
            ),
            1
        );
        assert_eq!(
            n(&conn, "SELECT count(*) FROM document WHERE tax_year = 2025"),
            2
        );
        assert_eq!(
            n(
                &conn,
                "SELECT count(*) FROM document WHERE tax_year IS NULL"
            ),
            2
        );
        assert_eq!(n(&conn, "SELECT count(*) FROM cabinet"), 1);

        let revs =
            n(&conn, "SELECT sum(rev) FROM document") + n(&conn, "SELECT sum(rev) FROM client");
        let rep2 = rebuild(&mut conn, &c.root, "dev-1").unwrap();
        assert_eq!(rep2.docs_unchanged, 5);
        assert_eq!(
            rep2.docs_new + rep2.docs_changed + rep2.docs_moved + rep2.docs_tombstoned,
            0
        );
        let revs2 =
            n(&conn, "SELECT sum(rev) FROM document") + n(&conn, "SELECT sum(rev) FROM client");
        assert_eq!(revs, revs2, "a second run changes no rows");
        assert!(vf_core::audit::verify(&conn).unwrap().ok());
    }

    #[test]
    fn changes_on_disk_become_versions_moves_and_tombstones() {
        let c = cab();
        let mut conn = open(&c);
        rebuild(&mut conn, &c.root, "dev-1").unwrap();
        let smith = c.root.join("Smith, John (1001)");
        let id_of = |conn: &Connection, rel: &str| -> String {
            conn.query_row("SELECT id FROM document WHERE rel_path = ?1", [rel], |r| {
                r.get(0)
            })
            .unwrap()
        };
        let moved_id = id_of(&conn, "Smith, John (1001)/Income Tax/2024/w2.pdf");

        std::fs::write(smith.join("Income Tax/2025/w2.pdf"), "w2 2025 corrected").unwrap();
        std::fs::create_dir_all(smith.join("Archive")).unwrap();
        std::fs::rename(
            smith.join("Income Tax/2024/w2.pdf"),
            smith.join("Archive/w2 2024.pdf"),
        )
        .unwrap();
        std::fs::remove_file(c.root.join("Acme LLC (1002)/Payroll/2025/941 Q1.pdf")).unwrap();

        let rep = rebuild(&mut conn, &c.root, "dev-1").unwrap();
        assert_eq!(
            (rep.docs_changed, rep.docs_moved, rep.docs_tombstoned),
            (1, 1, 1),
            "{rep}"
        );
        assert_eq!(
            id_of(&conn, "Smith, John (1001)/Archive/w2 2024.pdf"),
            moved_id
        );
        assert_eq!(n(&conn, "SELECT count(*) FROM version"), 1);
        assert_eq!(
            n(
                &conn,
                "SELECT count(*) FROM document WHERE deleted_at IS NOT NULL"
            ),
            1
        );
    }

    #[test]
    fn version_sidecars_restore_version_rows_on_a_fresh_catalog() {
        let c = cab();
        let old = "w2 2025 first draft";
        let hash = {
            let tmp = c.data.join("probe");
            std::fs::write(&tmp, old).unwrap();
            sha256_file(&tmp).unwrap()
        };
        let vpath = cabinet::version_path(&c.root, &hash);
        put(&vpath, old);
        let side = vec![VersionSidecar {
            rel_path: "Smith, John (1001)/Income Tax/2025/w2.pdf".into(),
            created: "2026-10-01T00:00:00Z".into(),
            created_by: Some("kwk".into()),
            device_id: "dev-0".into(),
            reason: Some("replaced".into()),
        }];
        std::fs::write(
            vpath.with_extension("json"),
            serde_json::to_vec(&side).unwrap(),
        )
        .unwrap();

        let mut conn = open(&c);
        let rep = rebuild(&mut conn, &c.root, "dev-1").unwrap();
        assert_eq!(rep.docs_new, 5, "version store is not scanned as documents");
        assert_eq!(rep.versions_new, 1);
        let rep2 = rebuild(&mut conn, &c.root, "dev-1").unwrap();
        assert_eq!(rep2.versions_new, 0);
    }

    #[test]
    fn not_a_cabinet_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let mut conn = vf_core::db::open(
            &tmp.path().join("c.db"),
            &CatalogKey([1; 32]),
            Storage::Local,
        )
        .unwrap();
        assert!(matches!(
            rebuild(&mut conn, tmp.path(), "d"),
            Err(RebuildError::Cabinet(_))
        ));
    }

    fn sentinel_uuid(c: &Cab, folder: &str) -> Uuid {
        sentinel::read_sentinel(&c.root.join(folder))
            .unwrap()
            .unwrap()
            .client_id
    }

    /// Imports `rows` (ID,Name) as the master list, keeping the bound folders' uuids.
    fn import(c: &Cab, rows: &str) {
        let csv = c.root.parent().unwrap().join("clients.csv");
        std::fs::write(&csv, format!("ClientID,Name\n{rows}")).unwrap();
        let known = BTreeMap::from([
            ("1001".to_string(), sentinel_uuid(c, "Smith, John (1001)")),
            ("1002".to_string(), sentinel_uuid(c, "Acme LLC (1002)")),
        ]);
        let map = filer_core::config::ColumnMap {
            id: Some("ClientID".into()),
            name: Some("Name".into()),
            extras: vec![],
        };
        crate::clients::import_csv(&c.root, &csv, &map, &known).unwrap();
    }

    fn relate(c: &Cab, parent: &str, child: &str, role: &str, share: Option<f64>) {
        let mut m = crate::clients::load(&c.root).unwrap();
        m.rels
            .add(filer_core::relationships::Relationship {
                parent_id: parent.into(),
                child_id: child.into(),
                role: role.into(),
                note: String::new(),
            })
            .unwrap();
        m.meta.set_share(parent, child, role, share);
        crate::clients::save_rels(&c.root, &m.rels).unwrap();
        crate::clients::save_meta(&c.root, &m.meta).unwrap();
    }

    /// Client and relationship rows without bookkeeping columns, for comparing catalogs.
    fn client_rows(conn: &Connection) -> Vec<String> {
        let mut out = Vec::new();
        let mut st = conn
            .prepare(
                "SELECT id, ext_id, name, entity_type, folder_rel, extra FROM client
                 WHERE deleted_at IS NULL ORDER BY ext_id",
            )
            .unwrap();
        for r in st
            .query_map([], |r| {
                Ok(format!(
                    "{}|{}|{}|{:?}|{:?}|{}",
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, String>(5)?
                ))
            })
            .unwrap()
        {
            out.push(r.unwrap());
        }
        let mut st = conn
            .prepare(
                "SELECT parent_id, child_id, kind, share_pct, note FROM client_relationship
                 WHERE deleted_at IS NULL ORDER BY parent_id, child_id, kind",
            )
            .unwrap();
        for r in st
            .query_map([], |r| {
                Ok(format!(
                    "rel {}>{} {} {:?} {:?}",
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<f64>>(3)?,
                    r.get::<_, Option<String>>(4)?
                ))
            })
            .unwrap()
        {
            out.push(r.unwrap());
        }
        out
    }

    #[test]
    fn master_list_merges_with_folders() {
        let c = cab();
        import(
            &c,
            "1001,John Smith\n1002,Acme LLC\n1003,Baker Family Trust\n",
        );
        let baker = crate::clients::load(&c.root).unwrap().list.unwrap().uuids["1003"].to_string();
        let mut m = crate::clients::load(&c.root).unwrap();
        m.meta.entity_types.insert(baker.clone(), "trust".into());
        crate::clients::save_meta(&c.root, &m.meta).unwrap();
        relate(&c, "1002", "1001", "shareholder", Some(40.0));
        relate(&c, "1003", "1001", "beneficiary", None);

        let mut conn = open(&c);
        let rep = rebuild(&mut conn, &c.root, "dev-1").unwrap();
        assert_eq!(
            (rep.clients, rep.master_only, rep.relationships),
            (2, 1, 2),
            "{rep}"
        );
        assert!(rep.uuid_mismatches.is_empty(), "{rep}");
        let s = sentinel_uuid(&c, "Smith, John (1001)").to_string();
        let name: String = conn
            .query_row("SELECT name FROM client WHERE id = ?1", [&s], |r| r.get(0))
            .unwrap();
        assert_eq!(
            name, "John Smith",
            "master-list name wins over the sentinel's"
        );
        let (folder, et): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT folder_rel, entity_type FROM client WHERE id = ?1",
                [&baker],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((folder, et.as_deref()), (None, Some("trust")));
        let share: Option<f64> = conn
            .query_row(
                "SELECT share_pct FROM client_relationship WHERE kind = 'shareholder'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(share, Some(40.0));

        // Idempotent: a second run changes no client or relationship row.
        let revs = n(
            &conn,
            "SELECT (SELECT sum(rev) FROM client) + (SELECT sum(rev) FROM client_relationship)",
        );
        rebuild(&mut conn, &c.root, "dev-1").unwrap();
        assert_eq!(
            n(
                &conn,
                "SELECT (SELECT sum(rev) FROM client) + (SELECT sum(rev) FROM client_relationship)"
            ),
            revs
        );

        // Principle 2: an empty catalog rebuilt from disk has the same rows.
        let fresh = vf_core::db::open(
            &c.data.join("fresh.db"),
            &CatalogKey([5; 32]),
            Storage::Local,
        )
        .unwrap();
        let mut fresh = fresh;
        rebuild(&mut fresh, &c.root, "dev-2").unwrap();
        assert_eq!(client_rows(&fresh), client_rows(&conn));
    }

    #[test]
    fn sync_clients_tombstones_removed_relationships_and_counts_dangling() {
        let c = cab();
        import(&c, "1001,John Smith\n1002,Acme LLC\n");
        relate(&c, "1002", "1001", "shareholder", Some(100.0));
        let mut conn = open(&c);
        rebuild(&mut conn, &c.root, "dev-1").unwrap();
        assert_eq!(
            n(
                &conn,
                "SELECT count(*) FROM client_relationship WHERE deleted_at IS NULL"
            ),
            1
        );

        let mut m = crate::clients::load(&c.root).unwrap();
        m.rels.records.clear();
        m.rels
            .records
            .push(filer_core::relationships::Relationship {
                parent_id: "1002".into(),
                child_id: "9999".into(),
                role: "partner".into(),
                note: String::new(),
            });
        crate::clients::save_rels(&c.root, &m.rels).unwrap();
        let rep = sync_clients(&mut conn, &c.root, "dev-1", "clients_edit").unwrap();
        assert_eq!(rep.dangling_relationships, 1);
        assert_eq!(
            n(
                &conn,
                "SELECT count(*) FROM client_relationship WHERE deleted_at IS NULL"
            ),
            0
        );
        assert_eq!(
            n(&conn, "SELECT count(*) FROM client_relationship"),
            1,
            "tombstone kept"
        );
        assert_eq!(
            n(
                &conn,
                "SELECT count(*) FROM audit WHERE action = 'clients_edit'"
            ),
            1
        );
    }

    #[test]
    fn folder_uuid_wins_over_a_mismatched_master_uuid() {
        let c = cab();
        // Imported without the folders' uuids: 1001 gets a different one in the list.
        let csv = c.root.parent().unwrap().join("clients.csv");
        std::fs::write(&csv, "ClientID,Name\n1001,John Smith\n").unwrap();
        let map = filer_core::config::ColumnMap {
            id: Some("ClientID".into()),
            name: Some("Name".into()),
            extras: vec![],
        };
        crate::clients::import_csv(&c.root, &csv, &map, &BTreeMap::new()).unwrap();
        let mut conn = open(&c);
        let rep = rebuild(&mut conn, &c.root, "dev-1").unwrap();
        assert_eq!(rep.uuid_mismatches, ["1001"]);
        assert_eq!(rep.master_only, 0);
        let s = sentinel_uuid(&c, "Smith, John (1001)").to_string();
        assert_eq!(
            n(
                &conn,
                &format!("SELECT count(*) FROM client WHERE id = '{s}'")
            ),
            1
        );
    }

    #[test]
    fn unreadable_master_still_rebuilds_from_folders() {
        let c = cab();
        std::fs::write(
            crate::clients::master_dir(&c.root).join("clients.json"),
            "{broken",
        )
        .unwrap();
        let mut conn = open(&c);
        let rep = rebuild(&mut conn, &c.root, "dev-1").unwrap();
        assert_eq!(rep.clients, 2);
        assert!(rep.master_error.is_some());
        assert!(rep.to_string().contains("client master not read"));
    }
}
