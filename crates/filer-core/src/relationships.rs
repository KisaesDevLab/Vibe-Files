//! Parent → child relationships between clients (P07a): `data/relationships.json`, edited in the
//! Relationships tab, with a CSV merge import and an export. Stored once and directional; nothing
//! is mirrored (QUESTIONS #43). A relationship whose ID is no longer in the client list is
//! "dangling": shown for re-linking, never deleted automatically.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::clients::{ClientList, CsvTable};

pub const RELATIONSHIPS_FILE: &str = "relationships.json";

/// The seeded role pick-list; any other text is allowed too.
pub const ROLES: [&str; 8] = [
    "shareholder",
    "partner",
    "member",
    "beneficiary",
    "grantor",
    "officer",
    "spouse",
    "related",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relationship {
    /// CSV ID of the parent (e.g. the entity).
    pub parent_id: String,
    /// CSV ID of the child (e.g. the shareholder).
    pub child_id: String,
    pub role: String,
    #[serde(default)]
    pub note: String,
}

impl Relationship {
    fn key(&self) -> (&str, &str, String) {
        (
            &self.parent_id,
            &self.child_id,
            self.role.trim().to_lowercase(),
        )
    }
}

/// Every relationship, in file order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Relationships {
    pub records: Vec<Relationship>,
}

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(RELATIONSHIPS_FILE)
}

/// `(relationships, savable, error)`. A missing file is empty and savable; an unreadable one is
/// empty, not savable (so it is never overwritten by accident) and reported.
pub fn load(data_dir: &Path) -> (Relationships, bool, Option<String>) {
    let file = path(data_dir);
    match std::fs::read_to_string(&file) {
        Ok(text) => match serde_json::from_str::<Vec<Relationship>>(&text) {
            Ok(records) => (Relationships { records }, true, None),
            Err(e) => {
                tracing::error!(error = %e, "relationships.json is invalid; not saving over it");
                (
                    Relationships::default(),
                    false,
                    Some(format!("relationships.json could not be read ({e}).")),
                )
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            (Relationships::default(), true, None)
        }
        Err(e) => (
            Relationships::default(),
            false,
            Some(format!("relationships.json could not be read ({e}).")),
        ),
    }
}

/// Temp file + rename, like the other data files.
pub fn save(data_dir: &Path, rel: &Relationships) -> anyhow::Result<()> {
    let file = path(data_dir);
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&rel.records)?)?;
    std::fs::rename(&tmp, &file)?;
    Ok(())
}

/// Why a relationship can't be added, if it can't.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddError {
    SameClient,
    EmptyRole,
    Duplicate,
}

impl std::fmt::Display for AddError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            AddError::SameClient => "A client can't be related to itself.",
            AddError::EmptyRole => "Choose a role.",
            AddError::Duplicate => "That relationship already exists.",
        })
    }
}

impl Relationships {
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Adds `r` unless it relates a client to itself, has no role, or already exists (same parent,
    /// child and role).
    pub fn add(&mut self, mut r: Relationship) -> Result<(), AddError> {
        r.parent_id = r.parent_id.trim().to_string();
        r.child_id = r.child_id.trim().to_string();
        r.role = r.role.trim().to_string();
        if r.parent_id == r.child_id {
            return Err(AddError::SameClient);
        }
        if r.role.is_empty() {
            return Err(AddError::EmptyRole);
        }
        if self.records.iter().any(|x| x.key() == r.key()) {
            return Err(AddError::Duplicate);
        }
        self.records.push(r);
        Ok(())
    }

    /// Children of `parent`, only those with one of `roles` (empty: every role).
    #[cfg_attr(not(test), allow(dead_code))] // used by the plan builder (R2c)
    pub fn children<'a>(&'a self, parent: &str, roles: &[String]) -> Vec<&'a Relationship> {
        self.records
            .iter()
            .filter(|r| r.parent_id == parent && role_matches(&r.role, roles))
            .collect()
    }

    /// Parents of `child`, only those with one of `roles` (empty: every role).
    #[cfg_attr(not(test), allow(dead_code))] // used by the plan builder (R2c)
    pub fn parents<'a>(&'a self, child: &str, roles: &[String]) -> Vec<&'a Relationship> {
        self.records
            .iter()
            .filter(|r| r.child_id == child && role_matches(&r.role, roles))
            .collect()
    }

    /// Relationships `id` takes part in, as parent or child.
    pub fn related_count(&self, id: &str) -> usize {
        self.records
            .iter()
            .filter(|r| r.parent_id == id || r.child_id == id)
            .count()
    }

    /// Indexes of relationships with an ID that is not in `clients`.
    pub fn dangling(&self, clients: &ClientList) -> Vec<usize> {
        self.records
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                clients.match_by_id(&r.parent_id).is_none()
                    || clients.match_by_id(&r.child_id).is_none()
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Adds every relationship that isn't there yet. Returns (added, already there or invalid).
    pub fn merge(&mut self, incoming: Vec<Relationship>) -> (usize, usize) {
        let mut added = 0;
        let mut skipped = 0;
        for r in incoming {
            match self.add(r) {
                Ok(()) => added += 1,
                Err(_) => skipped += 1,
            }
        }
        (added, skipped)
    }

    /// `parent_id,child_id,role,note` with a header row.
    pub fn to_csv(&self) -> anyhow::Result<String> {
        let mut w = csv::Writer::from_writer(Vec::new());
        w.write_record(["parent_id", "child_id", "role", "note"])?;
        for r in &self.records {
            w.write_record([&r.parent_id, &r.child_id, &r.role, &r.note])?;
        }
        Ok(String::from_utf8(w.into_inner()?)?)
    }
}

#[cfg_attr(not(test), allow(dead_code))] // used by children()/parents()
fn role_matches(role: &str, roles: &[String]) -> bool {
    roles.is_empty()
        || roles
            .iter()
            .any(|x| x.trim().eq_ignore_ascii_case(role.trim()))
}

/// Which CSV columns hold the parent ID, child ID, role and (optionally) note.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelColumns {
    pub parent: Option<String>,
    pub child: Option<String>,
    pub role: Option<String>,
    pub note: Option<String>,
}

impl RelColumns {
    /// Guesses from the usual header names.
    pub fn guess(headers: &[String]) -> Self {
        let find = |names: &[&str]| {
            headers
                .iter()
                .find(|h| names.contains(&h.trim().to_lowercase().as_str()))
                .cloned()
        };
        Self {
            parent: find(&["parent_id", "parent", "parent id", "entity", "entity_id"]),
            child: find(&["child_id", "child", "child id", "owner", "owner_id"]),
            role: find(&["role", "relationship", "type"]),
            note: find(&["note", "notes", "comment"]),
        }
    }
}

/// Outcome of reading relationship rows from a CSV.
#[derive(Debug, Default)]
pub struct CsvRows {
    pub rows: Vec<Relationship>,
    /// Rows with an empty parent, child or role.
    pub blank: usize,
    /// Rows naming an ID that is not in the client list (both IDs must exist).
    pub unknown: usize,
}

/// Reads relationships from `table` with `cols`. Rows whose IDs aren't in `clients` are skipped
/// and counted.
pub fn rows_from_csv(
    table: &CsvTable,
    cols: &RelColumns,
    clients: &ClientList,
) -> Result<CsvRows, String> {
    let col = |h: &Option<String>| {
        h.as_ref()
            .and_then(|h| table.headers.iter().position(|x| x == h))
    };
    let (Some(p), Some(c), Some(r)) = (col(&cols.parent), col(&cols.child), col(&cols.role)) else {
        return Err("Choose the parent ID, child ID and role columns.".into());
    };
    let n = col(&cols.note);
    let cell = |row: &[String], i: usize| row.get(i).map_or("", |s| s.trim()).to_string();
    let mut out = CsvRows::default();
    for row in &table.rows {
        let rel = Relationship {
            parent_id: cell(row, p),
            child_id: cell(row, c),
            role: cell(row, r),
            note: n.map(|n| cell(row, n)).unwrap_or_default(),
        };
        if rel.parent_id.is_empty() || rel.child_id.is_empty() || rel.role.is_empty() {
            out.blank += 1;
        } else if clients.match_by_id(&rel.parent_id).is_none()
            || clients.match_by_id(&rel.child_id).is_none()
        {
            out.unknown += 1;
        } else {
            out.rows.push(rel);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ColumnMap;

    fn rel(p: &str, c: &str, role: &str) -> Relationship {
        Relationship {
            parent_id: p.into(),
            child_id: c.into(),
            role: role.into(),
            note: String::new(),
        }
    }

    fn clients(csv: &str) -> ClientList {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("c.csv");
        std::fs::write(&f, csv).unwrap();
        let table = crate::clients::read_csv(&f).unwrap();
        let map = ColumnMap {
            id: Some("id".into()),
            name: Some("name".into()),
            extras: vec![],
        };
        crate::clients::apply_mapping(&table, &map).unwrap()
    }

    #[test]
    fn crud_queries_and_save_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = Relationships::default();
        r.add(rel("1234", "2093", "shareholder")).unwrap();
        r.add(rel("1234", "1042", "partner")).unwrap();
        r.add(rel("1234", "2093", "officer")).unwrap(); // a second role for the same pair
        assert_eq!(
            r.add(rel("1234", "2093", "Shareholder")),
            Err(AddError::Duplicate)
        );
        assert_eq!(
            r.add(rel("1234", "1234", "partner")),
            Err(AddError::SameClient)
        );
        assert_eq!(r.add(rel("1234", "5", " ")), Err(AddError::EmptyRole));

        assert_eq!(r.children("1234", &[]).len(), 3);
        assert_eq!(
            r.children("1234", &["shareholder".into(), "partner".into()])
                .len(),
            2
        );
        assert_eq!(r.parents("2093", &[]).len(), 2);
        assert!(r.parents("1234", &[]).is_empty(), "not mirrored");
        assert_eq!(r.related_count("2093"), 2);

        save(dir.path(), &r).unwrap();
        let (loaded, ok, err) = load(dir.path());
        assert!(ok && err.is_none());
        assert_eq!(loaded, r);

        r.records.remove(1);
        assert_eq!(r.children("1234", &[]).len(), 2);
    }

    #[test]
    fn dangling_after_a_csv_refresh_is_flagged_not_deleted() {
        let before = clients("id,name\n1234,Riverbend\n2093,Baker\n5120,Morales\n");
        let mut r = Relationships::default();
        r.add(rel("1234", "2093", "partner")).unwrap();
        r.add(rel("1234", "5120", "partner")).unwrap();
        assert!(r.dangling(&before).is_empty());
        let after = clients("id,name\n1234,Riverbend\n2093,Baker\n");
        assert_eq!(r.dangling(&after), vec![1]);
        assert_eq!(r.len(), 2, "kept");
    }

    #[test]
    fn csv_import_merges_and_skips_unknown_ids() {
        let list = clients("id,name\n1234,Riverbend\n2093,Baker\n1042,Hendricks\n");
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("rel.csv");
        std::fs::write(
            &f,
            "parent_id,child_id,role,note\n1234,2093,partner,40%\n1234,1042,partner,\n1234,9999,partner,\n,2093,partner,\n",
        )
        .unwrap();
        let table = crate::clients::read_csv(&f).unwrap();
        let cols = RelColumns::guess(&table.headers);
        assert_eq!(cols.note.as_deref(), Some("note"));
        let rows = rows_from_csv(&table, &cols, &list).unwrap();
        assert_eq!((rows.rows.len(), rows.blank, rows.unknown), (2, 1, 1));

        let mut r = Relationships::default();
        r.add(rel("1234", "2093", "partner")).unwrap();
        assert_eq!(r.merge(rows.rows), (1, 1), "merge, never replace");
        assert_eq!(r.records[0].note, "", "existing record untouched");
        let csv = r.to_csv().unwrap();
        assert!(csv.starts_with("parent_id,child_id,role,note"));
        assert_eq!(csv.lines().count(), 3);
    }

    #[test]
    fn unreadable_file_is_not_savable() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(path(dir.path()), "{ nope").unwrap();
        let (r, ok, err) = load(dir.path());
        assert!(r.records.is_empty() && !ok && err.is_some());
    }
}
