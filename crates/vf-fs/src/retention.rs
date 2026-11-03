//! Retention classes (PHASES P07 step 1, QUESTIONS Q-05: seeded, editable):
//! `<cabinet>/_Vibe/retention.json`, mirrored into the catalog `retention_class` table the way
//! doc types are (the file is what rebuild restores from). Each doc type names its class
//! (`DocType::retention`); a document without a doc type has no class and is never disposed.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;
use vf_core::audit::AuditEntry;
use vf_core::db::Connection;
use vf_core::retention::Anchor;
use vf_core::tx::{Actor, NoFileOps, Stmt, Transaction};

use crate::clients::master_dir;

pub const FILE: &str = "retention.json";

/// The seeded schedule (PHASES P07 step 1): (name, years, anchor).
pub const SEED: &[(&str, Option<i32>, &str)] = &[
    ("Returns & workpapers", Some(7), "tax_year_end"),
    ("Client source documents", Some(4), "tax_year_end"),
    ("Engagement letters", Some(7), "engagement_close"),
    ("Permanent file", None, "never"),
    ("Correspondence", Some(4), "tax_year_end"),
    ("Notices", Some(7), "tax_year_end"),
];

/// The seeded class for a seeded doc type, by name (`None`: no class).
pub fn default_class(doc_type: &str) -> Option<&'static str> {
    let n = doc_type.trim();
    Some(match n {
        "Return copy — federal"
        | "Return copy — state"
        | "8879 e-file authorization"
        | "Extension"
        | "Estimate vouchers"
        | "Workpaper"
        | "Depreciation schedule"
        | "Trial balance"
        | "Financial statements"
        | "Payroll report"
        | "Payroll tax return"
        | "Sales tax return" => "Returns & workpapers",
        "Engagement letter" | "§7216 consent" => "Engagement letters",
        "Formation documents"
        | "EIN letter"
        | "S election (2553)"
        | "Operating agreement"
        | "POA (2848)"
        | "Tax information authorization (8821)"
        | "Stock basis" => "Permanent file",
        "Notice / correspondence" => "Notices",
        "Invoice" | "Misc" => "Correspondence",
        _ if n.starts_with("1099")
            || n.starts_with("K-1")
            || matches!(
                n,
                "W-2"
                    | "1098"
                    | "1095"
                    | "SSA-1099"
                    | "Brokerage statement"
                    | "Property tax statement"
                    | "Charitable receipt"
                    | "Organizer"
                    | "Bank statement"
            ) =>
        {
            "Client source documents"
        }
        _ => return None,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Class {
    /// uuid v7; stable across edits and rebuilds.
    pub id: String,
    pub name: String,
    /// `None` only with anchor `never`.
    #[serde(default)]
    pub years: Option<i32>,
    /// `tax_year_end`, `engagement_close`, `filed_at` or `never`.
    pub anchor: String,
}

impl Class {
    pub fn anchor(&self) -> Anchor {
        Anchor::parse(&self.anchor).unwrap_or(Anchor::Never)
    }

    /// "7 years from the end of the tax year".
    pub fn describe(&self) -> String {
        match (self.anchor(), self.years) {
            (Anchor::Never, _) | (_, None) => "kept permanently".into(),
            (a, Some(y)) => format!("{y} years {}", a.describe()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FileV1 {
    version: u32,
    classes: Vec<Class>,
}

pub fn path(root: &Path) -> PathBuf {
    master_dir(root).join(FILE)
}

/// `Ok(None)` when the cabinet has no schedule yet.
pub fn load(root: &Path) -> Result<Option<Vec<Class>>, String> {
    match std::fs::read_to_string(path(root)) {
        Ok(t) => {
            let f: FileV1 =
                serde_json::from_str(&t).map_err(|e| format!("{FILE} could not be read: {e}"))?;
            if f.version != 1 {
                return Err(format!(
                    "{FILE} is version {}; this build reads 1",
                    f.version
                ));
            }
            Ok(Some(f.classes))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{FILE} could not be read: {e}")),
    }
}

/// Problems with one class's fields.
pub fn check_fields(name: &str, years: Option<i32>, anchor: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("Give the retention class a name.".into());
    }
    match (Anchor::parse(anchor), years) {
        (None, _) => Err(format!("\"{anchor}\" is not a retention anchor.")),
        (Some(Anchor::Never), _) => Ok(()),
        (Some(_), None) => Err("Say how many years.".into()),
        (Some(_), Some(y)) if !(1..=100).contains(&y) => {
            Err("Years must be between 1 and 100.".into())
        }
        _ => Ok(()),
    }
}

pub fn check(classes: &[Class]) -> Result<(), String> {
    let mut names = HashSet::new();
    let mut ids = HashSet::new();
    for c in classes {
        check_fields(&c.name, c.years, &c.anchor).map_err(|e| format!("{}: {e}", c.name))?;
        if !names.insert(c.name.trim().to_lowercase()) {
            return Err(format!(
                "There are two classes named \"{}\".",
                c.name.trim()
            ));
        }
        if !ids.insert(c.id.as_str()) {
            return Err(format!("Class id {} is used twice.", c.id));
        }
    }
    Ok(())
}

pub fn save(root: &Path, classes: &[Class]) -> Result<(), String> {
    check(classes)?;
    let f = FileV1 {
        version: 1,
        classes: classes.to_vec(),
    };
    crate::cabinet::write_replace(
        &path(root),
        &serde_json::to_vec_pretty(&f).expect("serializes"),
    )
    .map_err(|e| format!("saving {FILE}: {e}"))
}

/// Writes the seeded schedule when the cabinet has none, and gives every doc type that has no
/// class yet its default one (once: an existing cabinet gets it on its first open after P07).
/// Returns whether it wrote.
pub fn seed_if_missing(root: &Path) -> Result<bool, String> {
    if load(root)?.is_some() {
        return Ok(false);
    }
    let classes: Vec<Class> = SEED
        .iter()
        .map(|(n, y, a)| Class {
            id: vf_core::new_id(),
            name: (*n).into(),
            years: *y,
            anchor: (*a).into(),
        })
        .collect();
    save(root, &classes)?;
    if let Some(mut types) = crate::doctypes::load(root)? {
        let mut changed = false;
        for t in types.iter_mut().filter(|t| t.retention.is_none()) {
            if let Some(name) = default_class(&t.name)
                && let Some(c) = classes.iter().find(|c| c.name == name)
            {
                t.retention = Some(c.id.clone());
                changed = true;
            }
        }
        if changed {
            crate::doctypes::save(root, &types)?;
        }
    }
    Ok(true)
}

/// Catalog statements making `retention_class` match `classes` (upsert; tombstone the rest).
pub fn catalog_stmts(
    conn: &Connection,
    classes: &[Class],
    now: &str,
    device_id: &str,
) -> Result<Vec<Stmt>, String> {
    let live: Vec<String> = conn
        .prepare("SELECT id FROM retention_class WHERE deleted_at IS NULL")
        .and_then(|mut st| {
            st.query_map([], |r| r.get(0))?
                .collect::<Result<Vec<String>, _>>()
        })
        .map_err(|e| e.to_string())?;
    let mut stmts = Vec::new();
    for c in classes {
        stmts.push(Stmt::new(
            "INSERT INTO retention_class (id, name, years, anchor, device_id, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
               name = excluded.name, years = excluded.years, anchor = excluded.anchor,
               deleted_at = NULL, device_id = excluded.device_id,
               updated_at = excluded.updated_at, rev = retention_class.rev + 1
             WHERE (retention_class.name, retention_class.years, retention_class.anchor,
                    retention_class.deleted_at)
               IS NOT (excluded.name, excluded.years, excluded.anchor, NULL)",
            vec![
                c.id.clone().into(),
                c.name.trim().into(),
                c.years.into(),
                c.anchor.clone().into(),
                device_id.into(),
                now.into(),
            ],
        ));
    }
    let keep: HashSet<&str> = classes.iter().map(|c| c.id.as_str()).collect();
    for id in live.iter().filter(|id| !keep.contains(id.as_str())) {
        stmts.push(Stmt::new(
            "UPDATE retention_class SET deleted_at = ?2, name = name || ' (deleted ' || id || ')',
               device_id = ?3, updated_at = ?2, rev = rev + 1
             WHERE id = ?1 AND deleted_at IS NULL",
            vec![id.clone().into(), now.into(), device_id.into()],
        ));
    }
    Ok(stmts)
}

/// Brings the catalog's classes, then doc types (which point at them), in line with the
/// cabinet's files, as one audited Transaction.
pub fn sync(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    action: &str,
) -> Result<usize, String> {
    let classes = load(root)?.unwrap_or_default();
    let types = crate::doctypes::load(root)?.unwrap_or_default();
    let now = vf_core::now_rfc3339();
    let mut tx = Transaction::new(action).audit(AuditEntry::new(
        action,
        None,
        json!({ "retention_classes": classes.len() }),
    ));
    for s in catalog_stmts(conn, &classes, &now, device_id)? {
        tx = tx.catalog(s);
    }
    for s in crate::doctypes::catalog_stmts(conn, &types, &now, device_id)? {
        tx = tx.catalog(s);
    }
    tx.commit(conn, &mut NoFileOps, &Actor::device(device_id))
        .map_err(|e| e.to_string())?;
    Ok(classes.len())
}

/// Edits to the schedule; call [`sync`] after.
pub enum Edit {
    Add {
        name: String,
        years: Option<i32>,
        anchor: String,
    },
    Update(Class),
    /// Refused while a doc type uses it.
    Remove(String),
}

pub fn apply(root: &Path, edit: Edit) -> Result<(), String> {
    let mut classes = load(root)?.unwrap_or_default();
    let pos = |classes: &[Class], id: &str| {
        classes
            .iter()
            .position(|c| c.id == id)
            .ok_or_else(|| "That class is not in the schedule any more.".to_string())
    };
    match edit {
        Edit::Add {
            name,
            years,
            anchor,
        } => classes.push(Class {
            id: vf_core::new_id(),
            name: name.trim().into(),
            years: if anchor == "never" { None } else { years },
            anchor,
        }),
        Edit::Update(c) => {
            let i = pos(&classes, &c.id)?;
            classes[i] = Class {
                name: c.name.trim().into(),
                years: if c.anchor == "never" { None } else { c.years },
                ..c
            };
        }
        Edit::Remove(id) => {
            let i = pos(&classes, &id)?;
            let users: Vec<String> = crate::doctypes::load(root)?
                .unwrap_or_default()
                .into_iter()
                .filter(|t| t.retention.as_deref() == Some(id.as_str()))
                .map(|t| t.name)
                .collect();
            if !users.is_empty() {
                return Err(format!(
                    "{} is used by {}; give those doc types another class first.",
                    classes[i].name,
                    users.join(", ")
                ));
            }
            classes.remove(i);
        }
    }
    save(root, &classes)
}

#[cfg(test)]
mod tests {
    use vf_core::db::Storage;

    use super::*;

    #[test]
    fn seed_maps_every_seeded_doc_type_and_syncs() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        crate::doctypes::seed_if_missing(root).unwrap();
        assert!(seed_if_missing(root).unwrap());
        assert!(!seed_if_missing(root).unwrap());
        let classes = load(root).unwrap().unwrap();
        assert_eq!(classes.len(), 6);
        let types = crate::doctypes::load(root).unwrap().unwrap();
        let unmapped: Vec<&str> = types
            .iter()
            .filter(|t| t.retention.is_none())
            .map(|t| t.name.as_str())
            .collect();
        assert!(unmapped.is_empty(), "{unmapped:?}");
        let class_of = |name: &str| {
            let t = types.iter().find(|t| t.name == name).unwrap();
            classes
                .iter()
                .find(|c| Some(&c.id) == t.retention.as_ref())
                .unwrap()
                .name
                .clone()
        };
        assert_eq!(class_of("W-2"), "Client source documents");
        assert_eq!(class_of("K-1 (1120-S)"), "Client source documents");
        assert_eq!(class_of("Return copy — federal"), "Returns & workpapers");
        assert_eq!(class_of("EIN letter"), "Permanent file");

        let mut conn = vf_core::db::open(&t.path().join("c.db"), Storage::Local).unwrap();
        sync(&mut conn, root, "dev", "retention_seed").unwrap();
        let (n, linked): (i64, i64) = conn
            .query_row(
                "SELECT (SELECT count(*) FROM retention_class WHERE deleted_at IS NULL),
                        (SELECT count(*) FROM doc_type WHERE retention_class_id IS NOT NULL)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((n, linked), (6, types.len() as i64));
    }

    #[test]
    fn edits_are_checked_and_a_used_class_stays() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        crate::doctypes::seed_if_missing(root).unwrap();
        seed_if_missing(root).unwrap();
        apply(
            root,
            Edit::Add {
                name: "Payroll records".into(),
                years: Some(4),
                anchor: "filed_at".into(),
            },
        )
        .unwrap();
        assert!(
            apply(
                root,
                Edit::Add {
                    name: "payroll RECORDS".into(),
                    years: Some(4),
                    anchor: "filed_at".into(),
                }
            )
            .is_err()
        );
        assert!(
            apply(
                root,
                Edit::Add {
                    name: "Bad".into(),
                    years: None,
                    anchor: "filed_at".into(),
                }
            )
            .is_err()
        );
        let classes = load(root).unwrap().unwrap();
        let used = classes.iter().find(|c| c.name == "Notices").unwrap();
        assert!(
            apply(root, Edit::Remove(used.id.clone()))
                .unwrap_err()
                .contains("Notice")
        );
        let new = classes
            .iter()
            .find(|c| c.name == "Payroll records")
            .unwrap();
        apply(root, Edit::Remove(new.id.clone())).unwrap();
        assert_eq!(load(root).unwrap().unwrap().len(), 6);
        assert_eq!(
            classes[0].describe(),
            "7 years from the end of the tax year"
        );
        assert_eq!(classes[3].describe(), "kept permanently");
    }
}
