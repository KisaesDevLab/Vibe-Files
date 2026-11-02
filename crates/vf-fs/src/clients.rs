//! Client master (PHASES P01 step 3): new client from the cabinet template, CSV import with
//! folder discovery, entity type, and relationships.
//!
//! - Folder template and filename tokens are Filer Desktop's (`filer_core::template`); the
//!   cabinet's template lives at `<cabinet>/_Vibe/template.json` (seeded with Filer's default).
//! - A client folder is bound by a SentinelV1 written with Filer's `Sentinel` type.
//! - Relationships live in the catalog and are mirrored, after every change, to
//!   `<cabinet>/_Vibe/relationships.json` in Filer's format (CSV IDs, role, note), so `rebuild`
//!   restores them. Filer has no share field: a share is kept at the end of the note as
//!   `[share 40%]` (see [`encode_note`]).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use filer_core::clients::{Client, ClientList};
use filer_core::relationships::{self, Relationship, Relationships};
use filer_core::template::{self, TemplateNode};
use filer_intake::sentinel::{self, SENTINEL_VERSION, Sentinel};
use serde_json::json;
use uuid::Uuid;
use vf_core::audit::AuditEntry;
use vf_core::db::Connection;
use vf_core::tx::{Actor, Committed, FileOp, NoFileOps, Stmt, Transaction};

use crate::cabinet::{CabinetFile, VIBE_DIR};
use crate::fileops::FilerFileOps;

/// Entity types offered in the client editor: `(stored value, label)`.
pub const ENTITY_TYPES: [(&str, &str); 10] = [
    ("individual", "Individual"),
    ("sole_prop", "Sole proprietor"),
    ("partnership", "Partnership"),
    ("s_corp", "S corporation"),
    ("c_corp", "C corporation"),
    ("llc", "LLC (disregarded)"),
    ("trust", "Trust"),
    ("estate", "Estate"),
    ("nonprofit", "Nonprofit"),
    ("other", "Other"),
];

pub fn entity_label(v: &str) -> &str {
    ENTITY_TYPES
        .iter()
        .find(|(k, _)| *k == v)
        .map_or(v, |(_, l)| l)
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Catalog(#[from] vf_core::Error),
}

impl From<vf_core::db::SqlError> for ClientError {
    fn from(e: vf_core::db::SqlError) -> Self {
        ClientError::Catalog(e.into())
    }
}

type Result<T> = std::result::Result<T, ClientError>;

fn invalid<T>(msg: impl Into<String>) -> Result<T> {
    Err(ClientError::Invalid(msg.into()))
}

/// The cabinet's folder template (Filer format), seeded with Filer's default on first use.
pub fn template(root: &Path) -> TemplateNode {
    template::load_or_seed(&root.join(VIBE_DIR)).0
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Sentinel JSON exactly as Filer's `write_sentinel` produces it.
fn sentinel_json(client: &Client, firm_id: Uuid) -> String {
    let s = Sentinel {
        version: SENTINEL_VERSION,
        client_id: client.uuid,
        firm_id,
        tax_software_id: client.id.clone(),
        display_name_at_creation: client.name.clone(),
        created_at: chrono::Utc::now(),
        created_by: None,
    };
    serde_json::to_string_pretty(&s).expect("sentinel serializes")
}

fn sha256_hex(s: &str) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn firm_uuid(cab: &CabinetFile) -> Result<Uuid> {
    Uuid::parse_str(&cab.firm_id)
        .map_err(|_| ClientError::Invalid("cabinet.json has no valid firm id".into()))
}

fn live_ext_ids(conn: &Connection) -> Result<HashMap<String, String>> {
    let mut st = conn.prepare("SELECT ext_id, id FROM client WHERE deleted_at IS NULL")?;
    let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite_result::Result<_>>()?)
}

mod rusqlite_result {
    pub type Result<T> = std::result::Result<T, vf_core::db::SqlError>;
}

/// File ops creating a client's folder tree (template, year folders left out) and sentinel.
fn folder_ops(
    root: &Path,
    tpl: &TemplateNode,
    client: &Client,
    firm: Uuid,
) -> Result<(String, Vec<FileOp>, String)> {
    let dirs =
        template::initial_dirs(tpl, client).map_err(|e| ClientError::Invalid(e.to_string()))?;
    let Some(top) = dirs.first() else {
        return invalid("the folder template is empty");
    };
    let folder = root.join(top);
    let mut ops = Vec::new();
    for d in &dirs {
        let full =
            template::checked_join(root, d).map_err(|e| ClientError::Invalid(e.to_string()))?;
        ops.push(FileOp::Mkdir { path: full });
    }
    let json = sentinel_json(client, firm);
    let hash = sha256_hex(&json);
    ops.push(FileOp::WriteNew {
        path: sentinel::sentinel_path(&folder),
        contents: json,
    });
    Ok((rel(root, &folder), ops, hash))
}

/// What a new client needs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewClient {
    pub ext_id: String,
    pub name: String,
    pub entity_type: Option<String>,
    pub extra: BTreeMap<String, String>,
}

/// Folders the new client would get, relative to the cabinet (for the wizard's preview).
pub fn preview_folders(tpl: &TemplateNode, nc: &NewClient) -> Result<Vec<String>> {
    let c = Client {
        id: nc.ext_id.trim().into(),
        uuid: Uuid::nil(),
        name: nc.name.trim().into(),
        extra: nc.extra.clone(),
    };
    Ok(template::initial_dirs(tpl, &c)
        .map_err(|e| ClientError::Invalid(e.to_string()))?
        .iter()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect())
}

/// Creates the client folder tree from the cabinet template, writes its SentinelV1 and adds the
/// catalog row, as one Transaction. Returns the new client id.
pub fn create_client(
    conn: &mut Connection,
    root: &Path,
    cab: &CabinetFile,
    tpl: &TemplateNode,
    nc: &NewClient,
    actor: &Actor,
) -> Result<(String, Committed)> {
    let (ext, name) = (nc.ext_id.trim(), nc.name.trim());
    if ext.is_empty() || name.is_empty() {
        return invalid("Enter the client ID and name.");
    }
    if live_ext_ids(conn)?.contains_key(ext) {
        return invalid(format!("Client ID {ext} is already in the cabinet."));
    }
    let client = Client {
        id: ext.into(),
        uuid: Uuid::new_v4(),
        name: name.into(),
        extra: nc.extra.clone(),
    };
    let (folder_rel, ops, sentinel_hash) = folder_ops(root, tpl, &client, firm_uuid(cab)?)?;
    if root.join(&folder_rel).exists() {
        return invalid(format!(
            "{folder_rel} already exists. Import the client list to bind existing folders."
        ));
    }
    let id = client.uuid.to_string();
    let now = vf_core::now_rfc3339();
    let mut tx = Transaction::new(&format!("new client {name}"))
        .catalog(Stmt::new(
            "INSERT INTO client (id, ext_id, name, entity_type, folder_rel, sentinel_hash, extra, device_id, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            vec![
                id.clone().into(),
                ext.into(),
                name.into(),
                nc.entity_type.clone().into(),
                folder_rel.clone().into(),
                sentinel_hash.into(),
                serde_json::to_string(&nc.extra).expect("map").into(),
                actor.device_id.clone().into(),
                now.into(),
            ],
        ))
        .catalog_undo(Stmt::new("DELETE FROM client WHERE id = ?1", vec![id.clone().into()]))
        .audit(AuditEntry::new(
            "client_create",
            None,
            json!({"client": id, "ext_id": ext, "folder": folder_rel}),
        ));
    for op in ops {
        tx = tx.file(op);
    }
    let mut fs = FilerFileOps::new(&[root]).map_err(|e| ClientError::Invalid(e.to_string()))?;
    let done = tx.commit(conn, &mut fs, actor)?;
    Ok((id, done))
}

/// One CSV row, matched against the catalog and the folder tree.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportRow {
    pub client: Client,
    /// Catalog id when the CSV ID is already in the cabinet.
    pub existing: Option<String>,
    /// Name or extras differ from the catalog.
    pub changed: bool,
    pub folder: FolderState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderState {
    /// Already bound (catalog `folder_rel`).
    Bound(String),
    /// A folder with the template's name exists and has no sentinel: bind it.
    Discovered(String),
    /// No folder yet.
    Missing,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportPlan {
    pub rows: Vec<ImportRow>,
    /// Rows Filer's import skipped (empty ID or name).
    pub skipped: usize,
}

impl ImportPlan {
    pub fn new_count(&self) -> usize {
        self.rows.iter().filter(|r| r.existing.is_none()).count()
    }
    pub fn changed_count(&self) -> usize {
        self.rows.iter().filter(|r| r.changed).count()
    }
    pub fn discovered_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| matches!(r.folder, FolderState::Discovered(_)))
            .count()
    }
    pub fn missing_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| r.folder == FolderState::Missing)
            .count()
    }
}

/// Matches a client list (from Filer's CSV import, `filer_core::clients::apply_mapping`)
/// against the catalog and the cabinet's top-level folders. Nothing changes yet.
pub fn plan_import(
    conn: &Connection,
    root: &Path,
    tpl: &TemplateNode,
    list: &ClientList,
) -> Result<ImportPlan> {
    let mut st = conn.prepare(
        "SELECT ext_id, id, name, extra, folder_rel FROM client WHERE deleted_at IS NULL",
    )?;
    let known: HashMap<String, (String, String, String, Option<String>)> = st
        .query_map([], |r| {
            Ok((r.get(0)?, (r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
        })?
        .collect::<rusqlite_result::Result<_>>()?;
    let mut rows = Vec::new();
    for c in &list.clients {
        let mut c = c.clone();
        let (existing, changed, bound) = match known.get(&c.id) {
            Some((id, name, extra, folder)) => {
                c.uuid = Uuid::parse_str(id).unwrap_or(c.uuid);
                let extra_now: BTreeMap<String, String> =
                    serde_json::from_str(extra).unwrap_or_default();
                (
                    Some(id.clone()),
                    *name != c.name || extra_now != c.extra,
                    folder.clone(),
                )
            }
            None => (None, false, None),
        };
        let folder = match bound {
            Some(f) => FolderState::Bound(f),
            None => match template::client_folder_name(tpl, &c) {
                Ok(name) if root.join(&name).is_dir() => {
                    match sentinel::read_sentinel(&root.join(&name)) {
                        Ok(None) => FolderState::Discovered(name),
                        // A sentinel for someone else, or unreadable: leave it to Conflicts.
                        _ => FolderState::Missing,
                    }
                }
                _ => FolderState::Missing,
            },
        };
        rows.push(ImportRow {
            client: c,
            existing,
            changed,
            folder,
        });
    }
    Ok(ImportPlan {
        rows,
        skipped: list.skipped_rows,
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub added: usize,
    pub updated: usize,
    pub bound: usize,
    pub created_folders: usize,
    pub without_folder: usize,
}

/// Applies an import plan as one Transaction: adds new clients, updates changed names and
/// extras, binds discovered folders (sentinel + their documents get the client), and, with
/// `create_missing`, creates folders from the template for clients that have none.
pub fn commit_import(
    conn: &mut Connection,
    root: &Path,
    cab: &CabinetFile,
    tpl: &TemplateNode,
    plan: &ImportPlan,
    create_missing: bool,
    actor: &Actor,
) -> Result<(ImportReport, Option<Committed>)> {
    let firm = firm_uuid(cab)?;
    let now = vf_core::now_rfc3339();
    let mut rep = ImportReport::default();
    let mut tx = Transaction::new("import clients");
    for r in &plan.rows {
        let mut c = r.client.clone();
        if c.uuid.is_nil() {
            c.uuid = Uuid::new_v4();
        }
        let id = r.existing.clone().unwrap_or_else(|| c.uuid.to_string());
        c.uuid = Uuid::parse_str(&id).unwrap_or(c.uuid);
        let extra = serde_json::to_string(&c.extra).expect("map");
        // Folder: bind, create, or none.
        let mut folder_rel: Option<String> = None;
        let mut sentinel_hash: Option<String> = None;
        match &r.folder {
            FolderState::Bound(_) => {}
            FolderState::Discovered(name) => {
                let json = sentinel_json(&c, firm);
                sentinel_hash = Some(sha256_hex(&json));
                tx = tx.file(FileOp::WriteNew {
                    path: sentinel::sentinel_path(&root.join(name)),
                    contents: json,
                });
                folder_rel = Some(name.clone());
                rep.bound += 1;
            }
            FolderState::Missing if create_missing => {
                let (f, ops, h) = folder_ops(root, tpl, &c, firm)?;
                for op in ops {
                    tx = tx.file(op);
                }
                folder_rel = Some(f);
                sentinel_hash = Some(h);
                rep.created_folders += 1;
            }
            FolderState::Missing => rep.without_folder += 1,
        }
        match &r.existing {
            None => {
                tx = tx
                    .catalog(Stmt::new(
                        "INSERT INTO client (id, ext_id, name, folder_rel, sentinel_hash, extra, device_id, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        vec![
                            id.clone().into(), c.id.clone().into(), c.name.clone().into(),
                            folder_rel.clone().into(), sentinel_hash.clone().into(), extra.into(),
                            actor.device_id.clone().into(), now.clone().into(),
                        ],
                    ))
                    .catalog_undo(Stmt::new("DELETE FROM client WHERE id = ?1", vec![id.clone().into()]));
                rep.added += 1;
            }
            Some(_) => {
                let old: (String, String, Option<String>, Option<String>) = conn.query_row(
                    "SELECT name, extra, folder_rel, sentinel_hash FROM client WHERE id = ?1",
                    [&id],
                    |q| Ok((q.get(0)?, q.get(1)?, q.get(2)?, q.get(3)?)),
                )?;
                if r.changed || folder_rel.is_some() {
                    tx = tx
                        .catalog(Stmt::new(
                            "UPDATE client SET name = ?2, extra = ?3,
                               folder_rel = coalesce(?4, folder_rel), sentinel_hash = coalesce(?5, sentinel_hash),
                               device_id = ?6, updated_at = ?7, rev = rev + 1 WHERE id = ?1",
                            vec![
                                id.clone().into(), c.name.clone().into(), extra.into(),
                                folder_rel.clone().into(), sentinel_hash.clone().into(),
                                actor.device_id.clone().into(), now.clone().into(),
                            ],
                        ))
                        .catalog_undo(Stmt::new(
                            "UPDATE client SET name = ?2, extra = ?3, folder_rel = ?4, sentinel_hash = ?5,
                               rev = rev + 1 WHERE id = ?1",
                            vec![id.clone().into(), old.0.into(), old.1.into(), old.2.into(), old.3.into()],
                        ));
                }
                if r.changed {
                    rep.updated += 1;
                }
            }
        }
        if let Some(f) = &folder_rel {
            // Documents already cataloged under a folder that had no sentinel join the client.
            tx = tx
                .catalog(Stmt::new(
                    "UPDATE document SET client_id = ?1, device_id = ?3, updated_at = ?4, rev = rev + 1
                     WHERE client_id IS NULL AND deleted_at IS NULL AND substr(rel_path, 1, length(?2) + 1) = ?2 || '/'",
                    vec![id.clone().into(), f.clone().into(), actor.device_id.clone().into(), now.clone().into()],
                ))
                .catalog_undo(Stmt::new(
                    "UPDATE document SET client_id = NULL, rev = rev + 1
                     WHERE client_id = ?1 AND substr(rel_path, 1, length(?2) + 1) = ?2 || '/'",
                    vec![id.clone().into(), f.clone().into()],
                ));
        }
    }
    if rep.added + rep.updated + rep.bound + rep.created_folders == 0 {
        return Ok((rep, None));
    }
    tx = tx.audit(AuditEntry::new(
        "client_import",
        None,
        json!({
            "added": rep.added, "updated": rep.updated, "bound": rep.bound,
            "created_folders": rep.created_folders, "without_folder": rep.without_folder,
        }),
    ));
    let mut fs = FilerFileOps::new(&[root]).map_err(|e| ClientError::Invalid(e.to_string()))?;
    let done = tx.commit(conn, &mut fs, actor)?;
    Ok((rep, Some(done)))
}

/// Sets (or clears) a client's entity type.
pub fn set_entity_type(
    conn: &mut Connection,
    actor: &Actor,
    client_id: &str,
    entity_type: Option<&str>,
) -> Result<Committed> {
    if let Some(e) = entity_type
        && !ENTITY_TYPES.iter().any(|(k, _)| *k == e)
    {
        return invalid(format!("unknown entity type {e}"));
    }
    let old: Option<String> = conn.query_row(
        "SELECT entity_type FROM client WHERE id = ?1",
        [client_id],
        |r| r.get(0),
    )?;
    let set = "UPDATE client SET entity_type = ?2, device_id = ?3, updated_at = ?4, rev = rev + 1 WHERE id = ?1";
    let now = vf_core::now_rfc3339();
    Ok(Transaction::new("entity type")
        .catalog(Stmt::new(
            set,
            vec![
                client_id.into(),
                entity_type.into(),
                actor.device_id.clone().into(),
                now.clone().into(),
            ],
        ))
        .catalog_undo(Stmt::new(
            set,
            vec![
                client_id.into(),
                old.clone().into(),
                actor.device_id.clone().into(),
                now.into(),
            ],
        ))
        .audit(AuditEntry::new(
            "client_edit",
            None,
            json!({"client": client_id, "field": "entity_type", "old": old, "new": entity_type}),
        ))
        .commit(conn, &mut NoFileOps, actor)?)
}

// ---------------------------------------------------------------------------
// Relationships

/// A relationship as the client editor shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct RelRow {
    pub parent_id: String,
    pub child_id: String,
    pub kind: String,
    pub share_pct: Option<f64>,
    pub note: String,
    pub parent_name: String,
    pub child_name: String,
}

/// Live relationships where `client_id` is the parent or the child.
pub fn relationships(conn: &Connection, client_id: &str) -> Result<Vec<RelRow>> {
    let mut st = conn.prepare(
        "SELECT r.parent_id, r.child_id, r.kind, r.share_pct, coalesce(r.note, ''), p.name, c.name
         FROM client_relationship r JOIN client p ON p.id = r.parent_id JOIN client c ON c.id = r.child_id
         WHERE r.deleted_at IS NULL AND (r.parent_id = ?1 OR r.child_id = ?1)
         ORDER BY r.kind, p.name, c.name",
    )?;
    let rows = st.query_map([client_id], |r| {
        Ok(RelRow {
            parent_id: r.get(0)?,
            child_id: r.get(1)?,
            kind: r.get(2)?,
            share_pct: r.get(3)?,
            note: r.get(4)?,
            parent_name: r.get(5)?,
            child_name: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<rusqlite_result::Result<_>>()?)
}

type RelState = Option<(Option<f64>, Option<String>, Option<String>)>; // share, note, deleted_at

fn rel_state(conn: &Connection, p: &str, c: &str, kind: &str) -> Result<RelState> {
    let mut st = conn.prepare(
        "SELECT share_pct, note, deleted_at FROM client_relationship
         WHERE parent_id = ?1 AND child_id = ?2 AND kind = ?3",
    )?;
    let mut rows = st.query_map([p, c, kind], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    Ok(rows.next().transpose()?)
}

fn restore_rel(p: &str, c: &str, kind: &str, before: &RelState) -> Stmt {
    match before {
        None => Stmt::new(
            "DELETE FROM client_relationship WHERE parent_id = ?1 AND child_id = ?2 AND kind = ?3",
            vec![p.into(), c.into(), kind.into()],
        ),
        Some((share, note, deleted)) => Stmt::new(
            "UPDATE client_relationship SET share_pct = ?4, note = ?5, deleted_at = ?6, rev = rev + 1
             WHERE parent_id = ?1 AND child_id = ?2 AND kind = ?3",
            vec![p.into(), c.into(), kind.into(), (*share).into(), note.clone().into(), deleted.clone().into()],
        ),
    }
}

/// Adds (or revives) a relationship. `kind` is a Filer role (lower-cased); `share_pct` 0–100.
#[allow(clippy::too_many_arguments)]
pub fn add_relationship(
    conn: &mut Connection,
    root: &Path,
    actor: &Actor,
    parent_id: &str,
    child_id: &str,
    kind: &str,
    share_pct: Option<f64>,
    note: &str,
) -> Result<Committed> {
    let kind = kind.trim().to_lowercase();
    if parent_id == child_id {
        return invalid("A client cannot be related to itself.");
    }
    if kind.is_empty() {
        return invalid("Choose a role.");
    }
    if share_pct.is_some_and(|s| !(0.0..=100.0).contains(&s)) {
        return invalid("The share must be between 0 and 100%.");
    }
    let before = rel_state(conn, parent_id, child_id, &kind)?;
    if before.as_ref().is_some_and(|b| b.2.is_none()) {
        return invalid("That relationship already exists.");
    }
    let now = vf_core::now_rfc3339();
    let note = note.trim();
    let done = Transaction::new("add relationship")
        .catalog(Stmt::new(
            "INSERT INTO client_relationship (parent_id, child_id, kind, share_pct, note, device_id, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(parent_id, child_id, kind) DO UPDATE SET share_pct = excluded.share_pct,
               note = excluded.note, deleted_at = NULL, device_id = excluded.device_id,
               updated_at = excluded.updated_at, rev = client_relationship.rev + 1",
            vec![
                parent_id.into(), child_id.into(), kind.clone().into(), share_pct.into(),
                (!note.is_empty()).then(|| note.to_owned()).into(),
                actor.device_id.clone().into(), now.into(),
            ],
        ))
        .catalog_undo(restore_rel(parent_id, child_id, &kind, &before))
        .audit(AuditEntry::new(
            "relationship_add",
            None,
            json!({"parent": parent_id, "child": child_id, "kind": kind, "share_pct": share_pct}),
        ))
        .commit(conn, &mut NoFileOps, actor)?;
    mirror_relationships(conn, root)?;
    Ok(done)
}

/// Tombstones a relationship.
pub fn remove_relationship(
    conn: &mut Connection,
    root: &Path,
    actor: &Actor,
    parent_id: &str,
    child_id: &str,
    kind: &str,
) -> Result<Committed> {
    let before = rel_state(conn, parent_id, child_id, kind)?;
    if !before.as_ref().is_some_and(|b| b.2.is_none()) {
        return invalid("That relationship is not there.");
    }
    let now = vf_core::now_rfc3339();
    let done = Transaction::new("remove relationship")
        .catalog(Stmt::new(
            "UPDATE client_relationship SET deleted_at = ?4, device_id = ?5, updated_at = ?4, rev = rev + 1
             WHERE parent_id = ?1 AND child_id = ?2 AND kind = ?3",
            vec![parent_id.into(), child_id.into(), kind.into(), now.into(), actor.device_id.clone().into()],
        ))
        .catalog_undo(restore_rel(parent_id, child_id, kind, &before))
        .audit(AuditEntry::new(
            "relationship_remove",
            None,
            json!({"parent": parent_id, "child": child_id, "kind": kind}),
        ))
        .commit(conn, &mut NoFileOps, actor)?;
    mirror_relationships(conn, root)?;
    Ok(done)
}

/// `note [share 40%]`; a share with no note is just `[share 40%]`.
pub fn encode_note(note: &str, share_pct: Option<f64>) -> String {
    let note = note.trim();
    match share_pct {
        None => note.to_owned(),
        Some(s) => {
            let s = format!("[share {}%]", fmt_pct(s));
            if note.is_empty() {
                s
            } else {
                format!("{note} {s}")
            }
        }
    }
}

fn fmt_pct(s: f64) -> String {
    let t = format!("{s:.4}");
    t.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// Inverse of [`encode_note`].
pub fn decode_note(note: &str) -> (String, Option<f64>) {
    let n = note.trim_end();
    if let Some(start) = n.rfind("[share ")
        && let Some(body) = n[start + 7..].strip_suffix("%]")
        && let Ok(v) = body.trim().parse::<f64>()
    {
        return (n[..start].trim_end().to_owned(), Some(v));
    }
    (n.to_owned(), None)
}

/// Every live relationship in Filer's format (CSV IDs).
pub fn to_filer(conn: &Connection) -> Result<Relationships> {
    let mut st = conn.prepare(
        "SELECT p.ext_id, c.ext_id, r.kind, coalesce(r.note, ''), r.share_pct
         FROM client_relationship r JOIN client p ON p.id = r.parent_id JOIN client c ON c.id = r.child_id
         WHERE r.deleted_at IS NULL AND p.deleted_at IS NULL AND c.deleted_at IS NULL
         ORDER BY p.ext_id, c.ext_id, r.kind",
    )?;
    let rows = st.query_map([], |r| {
        let note: String = r.get(3)?;
        let share: Option<f64> = r.get(4)?;
        Ok(Relationship {
            parent_id: r.get(0)?,
            child_id: r.get(1)?,
            role: r.get(2)?,
            note: encode_note(&note, share),
        })
    })?;
    Ok(Relationships {
        records: rows.collect::<rusqlite_result::Result<_>>()?,
    })
}

/// Writes `<cabinet>/_Vibe/relationships.json` from the catalog (temp file + rename, Filer's
/// `relationships::save`). A derived file: rewritten in full after every change and undo.
pub fn mirror_relationships(conn: &Connection, root: &Path) -> Result<()> {
    let rels = to_filer(conn)?;
    relationships::save(&root.join(VIBE_DIR), &rels)
        .map_err(|e| ClientError::Invalid(format!("cannot write relationships.json: {e}")))
}

/// Statements that add the relationships in `rels` (Filer format) that the catalog lacks.
/// `ext_to_id` maps CSV IDs to catalog client ids; relationships naming an unknown ID are
/// skipped (Filer's "dangling", kept in the file for re-linking).
pub fn import_filer_stmts(
    rels: &Relationships,
    ext_to_id: &HashMap<String, String>,
    device_id: &str,
    now: &str,
) -> Vec<Stmt> {
    rels.records
        .iter()
        .filter_map(|r| {
            let p = ext_to_id.get(r.parent_id.trim())?;
            let c = ext_to_id.get(r.child_id.trim())?;
            let kind = r.role.trim().to_lowercase();
            if kind.is_empty() || p == c {
                return None;
            }
            let (note, share) = decode_note(&r.note);
            Some(Stmt::new(
                "INSERT OR IGNORE INTO client_relationship (parent_id, child_id, kind, share_pct, note, device_id, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                vec![
                    p.clone().into(), c.clone().into(), kind.into(), share.into(),
                    (!note.is_empty()).then_some(note).into(), device_id.into(), now.into(),
                ],
            ))
        })
        .collect()
}

/// Reads `<cabinet>/_Vibe/relationships.json` if present.
pub fn load_mirror(root: &Path) -> Option<Relationships> {
    let dir: PathBuf = root.join(VIBE_DIR);
    if !relationships::path(&dir).is_file() {
        return None;
    }
    let (rels, _, err) = relationships::load(&dir);
    err.is_none().then_some(rels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_core::db::{CatalogKey, Storage};

    pub struct World {
        pub _tmp: tempfile::TempDir,
        pub root: PathBuf,
        pub conn: Connection,
        pub cab: CabinetFile,
        pub actor: Actor,
    }

    pub fn world() -> World {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Cabinet");
        std::fs::create_dir(&root).unwrap();
        let cab = CabinetFile {
            cabinet_id: vf_core::new_id(),
            firm_name: "Firm".into(),
            firm_id: Uuid::new_v4().to_string(),
            schema_version: vf_core::db::SCHEMA_VERSION,
            layout_version: 1,
            created: "2026-11-02T00:00:00Z".into(),
        };
        crate::cabinet::create(&root, &cab).unwrap();
        let conn = vf_core::db::open(
            &tmp.path().join("c.db"),
            &CatalogKey([5; 32]),
            Storage::Local,
        )
        .unwrap();
        World {
            _tmp: tmp,
            root,
            conn,
            cab,
            actor: Actor {
                user: Some("kwk".into()),
                device_id: "dev".into(),
            },
        }
    }

    fn nc(id: &str, name: &str) -> NewClient {
        NewClient {
            ext_id: id.into(),
            name: name.into(),
            entity_type: Some("s_corp".into()),
            ..NewClient::default()
        }
    }

    /// P01 done-when: the new client wizard creates a folder tree matching the template and a
    /// valid SentinelV1.
    #[test]
    fn new_client_matches_template_and_sentinel_reads_back() {
        let mut w = world();
        let tpl = template(&w.root);
        let (id, done) = create_client(
            &mut w.conn,
            &w.root,
            &w.cab,
            &tpl,
            &nc("1042", "Anderson Farms LLC"),
            &w.actor,
        )
        .unwrap();
        let folder = w.root.join("Anderson Farms LLC (1042)");
        for d in preview_folders(&tpl, &nc("1042", "Anderson Farms LLC")).unwrap() {
            assert!(w.root.join(&d).is_dir(), "{d} missing");
        }
        assert!(folder.join("Permanent/Setup").is_dir());
        assert!(
            !folder.join("Income Tax/2025").exists(),
            "year folders wait for a document"
        );
        let s = sentinel::read_sentinel(&folder).unwrap().unwrap();
        assert_eq!(s.client_id.to_string(), id);
        assert_eq!(s.firm_id.to_string(), w.cab.firm_id);
        assert_eq!(s.tax_software_id, "1042");
        let (folder_rel, et): (String, String) = w
            .conn
            .query_row(
                "SELECT folder_rel, entity_type FROM client WHERE id = ?1",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (folder_rel.as_str(), et.as_str()),
            ("Anderson Farms LLC (1042)", "s_corp")
        );
        // A rebuild sees the same client.
        let rep = crate::rebuild::rebuild(&mut w.conn, &w.root, "dev").unwrap();
        assert_eq!(rep.clients, 1);
        // Duplicate ID refused; undo removes the folder tree.
        assert!(
            create_client(
                &mut w.conn,
                &w.root,
                &w.cab,
                &tpl,
                &nc("1042", "Other"),
                &w.actor
            )
            .is_err()
        );
        let mut fs = FilerFileOps::new(&[&w.root]).unwrap();
        vf_core::tx::undo(&mut w.conn, &mut fs, &w.actor, &done.tx_id).unwrap();
        assert!(!folder.exists());
    }

    #[test]
    fn import_adds_binds_and_creates() {
        let mut w = world();
        let tpl = template(&w.root);
        // An existing folder with a document but no sentinel.
        let loose = w.root.join("Baker, Ann (1001)/Income Tax/2025");
        std::fs::create_dir_all(&loose).unwrap();
        std::fs::write(loose.join("w2.pdf"), b"%PDF w2").unwrap();
        crate::rebuild::rebuild(&mut w.conn, &w.root, "dev").unwrap();
        let unfiled: i64 = w
            .conn
            .query_row(
                "SELECT count(*) FROM document WHERE client_id IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(unfiled, 1);

        let csv = w.root.parent().unwrap().join("clients.csv");
        std::fs::write(
            &csv,
            "ID,Name,Partner\n1001,\"Baker, Ann\",KWK\n1002,Cole Co,JM\n,blank,\n",
        )
        .unwrap();
        let table = filer_core::clients::read_csv(&csv).unwrap();
        let map = filer_core::config::ColumnMap {
            id: Some("ID".into()),
            name: Some("Name".into()),
            extras: vec!["Partner".into()],
        };
        let list = filer_core::clients::apply_mapping(&table, &map).unwrap();
        let plan = plan_import(&w.conn, &w.root, &tpl, &list).unwrap();
        assert_eq!(
            (
                plan.new_count(),
                plan.discovered_count(),
                plan.missing_count(),
                plan.skipped
            ),
            (2, 1, 1, 1)
        );
        let (rep, done) =
            commit_import(&mut w.conn, &w.root, &w.cab, &tpl, &plan, true, &w.actor).unwrap();
        assert_eq!((rep.added, rep.bound, rep.created_folders), (2, 1, 1));
        assert!(done.is_some());
        assert!(
            sentinel::read_sentinel(&w.root.join("Baker, Ann (1001)"))
                .unwrap()
                .is_some()
        );
        assert!(w.root.join("Cole Co (1002)/Permanent").is_dir());
        let unfiled: i64 = w
            .conn
            .query_row(
                "SELECT count(*) FROM document WHERE client_id IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(unfiled, 0, "the bound folder's document joined the client");
        // Re-import: nothing to do, and a rebuild agrees with the catalog.
        let plan = plan_import(&w.conn, &w.root, &tpl, &list).unwrap();
        assert_eq!((plan.new_count(), plan.changed_count()), (0, 0));
        assert!(
            commit_import(&mut w.conn, &w.root, &w.cab, &tpl, &plan, true, &w.actor)
                .unwrap()
                .1
                .is_none()
        );
        let rep = crate::rebuild::rebuild(&mut w.conn, &w.root, "dev").unwrap();
        assert_eq!(rep.clients, 2);
        assert_eq!(rep.docs_moved + rep.docs_new + rep.docs_tombstoned, 0);
    }

    #[test]
    fn note_encoding_round_trips() {
        for (note, share) in [
            ("", None),
            ("officer", None),
            ("", Some(40.0)),
            ("via trust", Some(33.3333)),
        ] {
            assert_eq!(
                decode_note(&encode_note(note, share)),
                (note.to_owned(), share)
            );
        }
        assert_eq!(encode_note("", Some(50.0)), "[share 50%]");
        assert_eq!(
            decode_note("just text [share x%]"),
            ("just text [share x%]".into(), None)
        );
    }

    /// P01 done-when: relationships round-trip through filer-core's existing format.
    #[test]
    fn relationships_round_trip_through_filer_format() {
        let mut w = world();
        let tpl = template(&w.root);
        let ids: Vec<String> = [
            ("1100", "Delgado Holdings LLC"),
            ("1150", "Delgado, Maria"),
            ("1151", "Delgado, Luis"),
        ]
        .iter()
        .map(|(e, n)| {
            create_client(&mut w.conn, &w.root, &w.cab, &tpl, &nc(e, n), &w.actor)
                .unwrap()
                .0
        })
        .collect();
        add_relationship(
            &mut w.conn,
            &w.root,
            &w.actor,
            &ids[0],
            &ids[1],
            "Partner",
            Some(40.0),
            "",
        )
        .unwrap();
        add_relationship(
            &mut w.conn,
            &w.root,
            &w.actor,
            &ids[0],
            &ids[2],
            "partner",
            Some(60.0),
            "via trust",
        )
        .unwrap();
        add_relationship(
            &mut w.conn,
            &w.root,
            &w.actor,
            &ids[1],
            &ids[2],
            "spouse",
            None,
            "",
        )
        .unwrap();
        assert!(
            add_relationship(
                &mut w.conn,
                &w.root,
                &w.actor,
                &ids[1],
                &ids[2],
                "spouse",
                None,
                ""
            )
            .is_err()
        );
        let r = remove_relationship(&mut w.conn, &w.root, &w.actor, &ids[1], &ids[2], "spouse")
            .unwrap();
        let mut fs = NoFileOps;
        vf_core::tx::undo(&mut w.conn, &mut fs, &w.actor, &r.tx_id).unwrap();
        mirror_relationships(&w.conn, &w.root).unwrap();

        // The mirror is plain Filer: its loader reads it.
        let (filer, ok, err) = relationships::load(&w.root.join(VIBE_DIR));
        assert!(ok && err.is_none());
        assert_eq!(filer.len(), 3);
        assert!(
            filer
                .records
                .iter()
                .any(|r| r.child_id == "1150" && r.note == "[share 40%]")
        );

        // A fresh catalog rebuilt from the tree gets the same relationships back.
        let before = relationships(&w.conn, &ids[0]).unwrap();
        let tmp2 = tempfile::tempdir().unwrap();
        let mut c2 = vf_core::db::open(
            &tmp2.path().join("c.db"),
            &CatalogKey([6; 32]),
            Storage::Local,
        )
        .unwrap();
        crate::rebuild::rebuild(&mut c2, &w.root, "dev2").unwrap();
        let after = relationships(&c2, &ids[0]).unwrap();
        assert_eq!(before, after);
        assert_eq!(to_filer(&c2).unwrap(), to_filer(&w.conn).unwrap());
    }

    #[test]
    fn entity_type_edit_and_undo() {
        let mut w = world();
        let tpl = template(&w.root);
        let (id, _) =
            create_client(&mut w.conn, &w.root, &w.cab, &tpl, &nc("1", "A"), &w.actor).unwrap();
        assert!(set_entity_type(&mut w.conn, &w.actor, &id, Some("bogus")).is_err());
        let d = set_entity_type(&mut w.conn, &w.actor, &id, Some("trust")).unwrap();
        vf_core::tx::undo(&mut w.conn, &mut NoFileOps, &w.actor, &d.tx_id).unwrap();
        let et: String = w
            .conn
            .query_row("SELECT entity_type FROM client WHERE id = ?1", [&id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(et, "s_corp");
    }
}
