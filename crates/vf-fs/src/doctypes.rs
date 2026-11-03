//! The firm's doc-type list (P01 step 4): `<cabinet>/_Vibe/doc_types.json`, mirrored into the
//! catalog `doc_type` table (QUESTIONS Q-21). The file is the source rebuild restores from; every
//! edit writes the file first, then the catalog in one Transaction ([`sync`]).
//!
//! A doc type's default folder is a Filer template path below the client folder, e.g.
//! `Income Tax/{Year}` (only `{Client}`, `{ID}`, `{Year}` tokens).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use filer_core::template;
use serde::{Deserialize, Serialize};
use serde_json::json;
use vf_core::audit::AuditEntry;
use vf_core::db::Connection;
use vf_core::tx::{Actor, NoFileOps, Stmt, Transaction};

use crate::clients::master_dir;

pub const FILE: &str = "doc_types.json";

/// Seeded list: (name, default folder, client-visible). Folders are the default Filer template's.
pub const SEED: &[(&str, &str, bool)] = &[
    ("W-2", "Income Tax/{Year}", false),
    ("1099-INT", "Income Tax/{Year}", false),
    ("1099-DIV", "Income Tax/{Year}", false),
    ("1099-B", "Income Tax/{Year}", false),
    ("1099-R", "Income Tax/{Year}", false),
    ("1099-NEC", "Income Tax/{Year}", false),
    ("1099-MISC", "Income Tax/{Year}", false),
    ("1099-K", "Income Tax/{Year}", false),
    ("1099-G", "Income Tax/{Year}", false),
    ("1098", "Income Tax/{Year}", false),
    ("1095", "Income Tax/{Year}", false),
    ("K-1 (1065)", "Income Tax/{Year}", false),
    ("K-1 (1120-S)", "Income Tax/{Year}", false),
    ("K-1 (1041)", "Income Tax/{Year}", false),
    ("SSA-1099", "Income Tax/{Year}", false),
    ("Brokerage statement", "Income Tax/{Year}", false),
    ("Property tax statement", "Income Tax/{Year}", false),
    ("Charitable receipt", "Income Tax/{Year}", false),
    ("Organizer", "Income Tax/{Year}", true),
    ("8879 e-file authorization", "Income Tax/{Year}", true),
    ("Return copy — federal", "Income Tax/{Year}", true),
    ("Return copy — state", "Income Tax/{Year}", true),
    ("Extension", "Income Tax/{Year}", true),
    ("Estimate vouchers", "Income Tax/{Year}", true),
    ("Engagement letter", "Correspondence/{Year}", true),
    ("Notice / correspondence", "Correspondence/{Year}", false),
    ("§7216 consent", "Correspondence/{Year}", false),
    ("Workpaper", "Workpapers & Support/{Year}", false),
    (
        "Depreciation schedule",
        "Workpapers & Support/{Year}",
        false,
    ),
    ("Trial balance", "Bookkeeping/{Year}", false),
    ("Bank statement", "Bookkeeping/{Year}", false),
    ("Financial statements", "Financials/{Year}", true),
    ("Payroll report", "Payroll/{Year}", false),
    ("Payroll tax return", "Payroll/{Year}", false),
    ("Sales tax return", "Sales Tax/{Year}", false),
    ("Formation documents", "Permanent/Setup", false),
    ("EIN letter", "Permanent/Setup", false),
    ("S election (2553)", "Permanent/Setup", false),
    ("Operating agreement", "Permanent/Setup", false),
    ("POA (2848)", "Permanent/Setup", false),
    (
        "Tax information authorization (8821)",
        "Permanent/Setup",
        false,
    ),
    ("Stock basis", "Permanent/Stock Basis", false),
    ("Invoice", "Other/{Year}", true),
    ("Misc", "Other/{Year}", false),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocType {
    /// uuid v7; stable across edits and rebuilds.
    pub id: String,
    pub name: String,
    /// Template path below the client folder; empty = no default.
    #[serde(default)]
    pub default_folder: String,
    #[serde(default)]
    pub client_visible: bool,
    /// Retention class id (`crate::retention`); `None`: never disposed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FileV1 {
    version: u32,
    /// In display order.
    types: Vec<DocType>,
}

pub fn path(root: &Path) -> PathBuf {
    master_dir(root).join(FILE)
}

/// `Ok(None)` when the cabinet has no list yet.
pub fn load(root: &Path) -> Result<Option<Vec<DocType>>, String> {
    let p = path(root);
    match std::fs::read_to_string(&p) {
        Ok(t) => {
            let f: FileV1 =
                serde_json::from_str(&t).map_err(|e| format!("{FILE} could not be read: {e}"))?;
            if f.version != 1 {
                return Err(format!(
                    "{FILE} is version {}; this build reads 1",
                    f.version
                ));
            }
            Ok(Some(f.types))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{FILE} could not be read: {e}")),
    }
}

/// Temp file + rename. Checks the list first.
pub fn save(root: &Path, types: &[DocType]) -> Result<(), String> {
    check(types)?;
    let f = FileV1 {
        version: 1,
        types: types.to_vec(),
    };
    crate::cabinet::write_replace(
        &path(root),
        &serde_json::to_vec_pretty(&f).expect("serializes"),
    )
    .map_err(|e| format!("saving {FILE}: {e}"))
}

/// Writes the seeded list when the cabinet has none. Returns whether it did.
pub fn seed_if_missing(root: &Path) -> Result<bool, String> {
    if load(root)?.is_some() {
        return Ok(false);
    }
    let types: Vec<DocType> = SEED
        .iter()
        .map(|(n, f, v)| DocType {
            id: vf_core::new_id(),
            name: (*n).into(),
            default_folder: (*f).into(),
            client_visible: *v,
            retention: None,
        })
        .collect();
    save(root, &types)?;
    Ok(true)
}

/// Problems with one doc type's fields (the editor shows them).
pub fn check_fields(name: &str, default_folder: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("Give the doc type a name.".into());
    }
    let f = default_folder.trim();
    if f.is_empty() {
        return Ok(());
    }
    let mut errors = Vec::new();
    for seg in f.split(['/', '\\']) {
        if seg.trim().is_empty() {
            errors.push("The default folder has an empty part (two slashes in a row?).".into());
            continue;
        }
        errors.extend(template::validate_segment(seg, &template::TREE_TOKENS));
    }
    match errors.first() {
        Some(e) => Err(e.clone()),
        None => Ok(()),
    }
}

/// The whole list: fields valid, names unique ignoring case, ids unique.
pub fn check(types: &[DocType]) -> Result<(), String> {
    let mut names = HashSet::new();
    let mut ids = HashSet::new();
    for t in types {
        check_fields(&t.name, &t.default_folder).map_err(|e| format!("{}: {e}", t.name))?;
        if !names.insert(t.name.trim().to_lowercase()) {
            return Err(format!(
                "There are two doc types named \"{}\".",
                t.name.trim()
            ));
        }
        if !ids.insert(t.id.as_str()) {
            return Err(format!("Doc type id {} is used twice.", t.id));
        }
    }
    Ok(())
}

/// Catalog statements making `doc_type` match `types`: upsert each (sort = list position),
/// tombstone live rows not in the list. Documents keep pointing at a tombstoned type.
pub fn catalog_stmts(
    conn: &Connection,
    types: &[DocType],
    now: &str,
    device_id: &str,
) -> Result<Vec<Stmt>, String> {
    let live: Vec<String> = conn
        .prepare("SELECT id FROM doc_type WHERE deleted_at IS NULL")
        .and_then(|mut st| {
            st.query_map([], |r| r.get(0))?
                .collect::<Result<Vec<String>, _>>()
        })
        .map_err(|e| e.to_string())?;
    let mut stmts = Vec::new();
    for (i, t) in types.iter().enumerate() {
        let folder = Some(t.default_folder.trim()).filter(|f| !f.is_empty());
        stmts.push(Stmt::new(
            "INSERT INTO doc_type (id, name, default_folder_tpl, client_visible, sort, device_id,
               updated_at, retention_class_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET
               name = excluded.name, default_folder_tpl = excluded.default_folder_tpl,
               client_visible = excluded.client_visible, sort = excluded.sort, deleted_at = NULL,
               retention_class_id = excluded.retention_class_id,
               device_id = excluded.device_id, updated_at = excluded.updated_at,
               rev = doc_type.rev + 1
             WHERE (doc_type.name, doc_type.default_folder_tpl, doc_type.client_visible,
                    doc_type.sort, doc_type.deleted_at, doc_type.retention_class_id)
               IS NOT (excluded.name, excluded.default_folder_tpl, excluded.client_visible,
                       excluded.sort, NULL, excluded.retention_class_id)",
            vec![
                t.id.clone().into(),
                t.name.trim().into(),
                folder.map(str::to_owned).into(),
                (t.client_visible as i64).into(),
                ((i as i64 + 1) * 10).into(),
                device_id.into(),
                now.into(),
                t.retention.clone().into(),
            ],
        ));
    }
    let keep: HashSet<&str> = types.iter().map(|t| t.id.as_str()).collect();
    for id in live.iter().filter(|id| !keep.contains(id.as_str())) {
        stmts.push(Stmt::new(
            // A tombstoned name must not block a new type with the same name (UNIQUE(name)).
            "UPDATE doc_type SET deleted_at = ?2, name = name || ' (deleted ' || id || ')',
               device_id = ?3, updated_at = ?2, rev = rev + 1
             WHERE id = ?1 AND deleted_at IS NULL",
            vec![id.clone().into(), now.into(), device_id.into()],
        ));
    }
    Ok(stmts)
}

/// Brings the catalog in line with `doc_types.json` as one audited Transaction.
pub fn sync(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    action: &str,
) -> Result<usize, String> {
    let types = load(root)?.unwrap_or_default();
    let now = vf_core::now_rfc3339();
    let stmts = catalog_stmts(conn, &types, &now, device_id)?;
    let mut tx = Transaction::new(action).audit(AuditEntry::new(
        action,
        None,
        json!({ "doc_types": types.len() }),
    ));
    for s in stmts {
        tx = tx.catalog(s);
    }
    let actor = Actor::device(device_id);
    tx.commit(conn, &mut NoFileOps, &actor)
        .map_err(|e| e.to_string())?;
    Ok(types.len())
}

/// Edits to the list. Each loads the file, changes it, checks and saves it; call [`sync`] after.
pub enum Edit {
    Add {
        name: String,
        default_folder: String,
        client_visible: bool,
        /// Retention class id.
        retention: Option<String>,
    },
    Update(DocType),
    Remove(String),
    /// Move one place up (`-1`) or down (`1`).
    Move(String, i32),
}

pub fn apply(root: &Path, edit: Edit) -> Result<(), String> {
    let mut types = load(root)?.unwrap_or_default();
    let pos = |types: &[DocType], id: &str| {
        types
            .iter()
            .position(|t| t.id == id)
            .ok_or_else(|| "That doc type is not in the list any more.".to_string())
    };
    match edit {
        Edit::Add {
            name,
            default_folder,
            client_visible,
            retention,
        } => types.push(DocType {
            id: vf_core::new_id(),
            name: name.trim().into(),
            default_folder: default_folder.trim().into(),
            client_visible,
            retention,
        }),
        Edit::Update(t) => {
            let i = pos(&types, &t.id)?;
            types[i] = DocType {
                name: t.name.trim().into(),
                default_folder: t.default_folder.trim().into(),
                ..t
            };
        }
        Edit::Remove(id) => {
            let i = pos(&types, &id)?;
            types.remove(i);
        }
        Edit::Move(id, by) => {
            let i = pos(&types, &id)?;
            let j = i as i64 + by as i64;
            if j < 0 || j >= types.len() as i64 {
                return Ok(());
            }
            types.swap(i, j as usize);
        }
    }
    save(root, &types)
}

#[cfg(test)]
mod tests {
    use vf_core::db::Storage;

    use super::*;

    fn setup() -> (tempfile::TempDir, PathBuf, Connection) {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("cab");
        std::fs::create_dir_all(&root).unwrap();
        let conn = vf_core::db::open(&t.path().join("c.db"), Storage::Local).unwrap();
        (t, root, conn)
    }

    fn n(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn seed_is_valid_and_written_once() {
        let (_t, root, mut conn) = setup();
        assert!(SEED.len() >= 40);
        assert!(seed_if_missing(&root).unwrap());
        assert!(!seed_if_missing(&root).unwrap(), "only when missing");
        let types = load(&root).unwrap().unwrap();
        check(&types).unwrap();
        // Every seeded folder is a folder of the default Filer template.
        let tpl = template::folder_templates(&template::default_tree());
        for t in &types {
            assert!(
                tpl.contains(&t.default_folder),
                "{} -> {}",
                t.name,
                t.default_folder
            );
        }
        sync(&mut conn, &root, "dev", "doc_types_seed").unwrap();
        assert_eq!(
            n(
                &conn,
                "SELECT count(*) FROM doc_type WHERE deleted_at IS NULL"
            ),
            SEED.len() as i64
        );
    }

    #[test]
    fn edits_round_trip_into_the_catalog() {
        let (_t, root, mut conn) = setup();
        seed_if_missing(&root).unwrap();
        sync(&mut conn, &root, "dev", "doc_types_seed").unwrap();
        apply(
            &root,
            Edit::Add {
                name: "Crypto statement".into(),
                default_folder: "Income Tax/{Year}".into(),
                client_visible: false,
                retention: None,
            },
        )
        .unwrap();
        let w2 = load(&root).unwrap().unwrap()[0].clone();
        apply(
            &root,
            Edit::Update(DocType {
                name: "W-2 / W-2c".into(),
                ..w2.clone()
            }),
        )
        .unwrap();
        let misc = load(&root)
            .unwrap()
            .unwrap()
            .iter()
            .find(|t| t.name == "Misc")
            .unwrap()
            .id
            .clone();
        apply(&root, Edit::Remove(misc.clone())).unwrap();
        apply(&root, Edit::Move(w2.id.clone(), 1)).unwrap();
        sync(&mut conn, &root, "dev", "doc_types_edit").unwrap();

        let name: String = conn
            .query_row("SELECT name FROM doc_type WHERE id = ?1", [&w2.id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(name, "W-2 / W-2c");
        let sort: i64 = conn
            .query_row("SELECT sort FROM doc_type WHERE id = ?1", [&w2.id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(sort, 20, "moved down one place");
        assert_eq!(
            n(
                &conn,
                &format!(
                    "SELECT count(*) FROM doc_type WHERE id = '{misc}' AND deleted_at IS NOT NULL"
                )
            ),
            1,
            "removed = tombstoned"
        );
        // A new type may reuse a removed name.
        apply(
            &root,
            Edit::Add {
                name: "Misc".into(),
                default_folder: String::new(),
                client_visible: false,
                retention: None,
            },
        )
        .unwrap();
        sync(&mut conn, &root, "dev", "doc_types_edit").unwrap();
        // Idempotent: a second sync changes nothing.
        let revs = n(&conn, "SELECT sum(rev) FROM doc_type");
        sync(&mut conn, &root, "dev", "doc_types_edit").unwrap();
        assert_eq!(n(&conn, "SELECT sum(rev) FROM doc_type"), revs);
    }

    #[test]
    fn bad_edits_are_refused_and_nothing_is_saved() {
        let (_t, root, _conn) = setup();
        seed_if_missing(&root).unwrap();
        let before = std::fs::read(path(&root)).unwrap();
        let add = |name: &str, folder: &str| {
            apply(
                &root,
                Edit::Add {
                    name: name.into(),
                    default_folder: folder.into(),
                    client_visible: false,
                    retention: None,
                },
            )
        };
        assert!(add("w-2", "").unwrap_err().contains("two doc types"));
        assert!(add(" ", "").is_err());
        assert!(add("Thing", "Income Tax/{Month}").is_err(), "unknown token");
        assert!(add("Thing", "Income Tax//x").is_err());
        assert_eq!(std::fs::read(path(&root)).unwrap(), before);
    }
}
