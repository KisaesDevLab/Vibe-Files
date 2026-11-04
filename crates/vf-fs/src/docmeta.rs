//! Document metadata edits (P01 step 5) and their on-disk copy (QUESTIONS Q-22).
//!
//! Doc type, tax year, status, source and tags live in the catalog and, so a rebuild keeps them,
//! in an append-only `_Vibe/docmeta.jsonl` inside the document's client folder (documents outside
//! client folders: `<cabinet>/_Vibe/docmeta.jsonl`). Each line is the document's full metadata
//! after a change, keyed by content hash and path; the last line for a document wins.
//!
//! An edit of any number of documents is one Transaction: catalog rows, one audit row, the
//! appended lines (`FileOp::AppendLines`), one undo record. Undo appends the old values.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;
use vf_core::audit::AuditEntry;
use vf_core::db::Connection;
use vf_core::tx::{FileOp, Stmt, Transaction};

use crate::cabinet::VIBE_DIR;

pub const FILE: &str = "docmeta.jsonl";
pub const LABEL: &str = "doc_meta_edit";
pub const STATUSES: [(&str, &str); 4] = [
    ("received", "Received"),
    ("in_work", "In work"),
    ("final", "Final"),
    ("delivered", "Delivered"),
];
pub const SOURCES: [(&str, &str); 4] = [
    ("client", "Client"),
    ("firm", "Firm"),
    ("government", "Government"),
    ("third_party", "Third party"),
];

/// One line of `docmeta.jsonl`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub v: u32,
    pub ts: String,
    pub device: String,
    pub hash: String,
    /// Relative to the client folder (or the cabinet, for documents outside client folders),
    /// forward slashes, so renaming the client folder does not break it.
    pub path: String,
    pub doc_type: Option<String>,
    /// For a reader without the catalog, and for re-linking if the id is gone.
    pub doc_type_name: Option<String>,
    /// `None` = permanent file.
    pub tax_year: Option<i32>,
    pub status: String,
    pub source: Option<String>,
    pub tags: Vec<String>,
}

/// Tag on a document whose doc type or tax year a folder import guessed (a year from the file
/// name, a doc type from a folder name): it is not disposed of until a person sets them; doing
/// so removes the tag (QA P12).
pub const GUESSED_TAG: &str = "import:guessed";

/// A document's current metadata, from the catalog.
#[derive(Debug, Clone, PartialEq)]
pub struct Meta {
    pub id: String,
    pub rel_path: String,
    pub hash: String,
    /// The client folder, when the document is in one.
    pub folder_rel: Option<String>,
    pub doc_type: Option<String>,
    pub doc_type_name: Option<String>,
    pub tax_year: Option<i32>,
    pub status: String,
    pub source: Option<String>,
    pub tags: Vec<String>,
}

/// What to change. `None` leaves a field as it is (the inspector's "Mixed").
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MetaEdit {
    /// `Some(None)` clears the doc type.
    pub doc_type: Option<Option<String>>,
    /// `Some(None)` = Permanent.
    pub tax_year: Option<Option<i32>>,
    pub status: Option<String>,
    pub source: Option<Option<String>>,
    /// Replaces the tag set.
    pub tags: Option<Vec<String>>,
}

impl MetaEdit {
    pub fn is_empty(&self) -> bool {
        *self == MetaEdit::default()
    }
}

pub fn load(conn: &Connection, id: &str) -> Result<Meta, String> {
    let mut m = conn
        .query_row(
            "SELECT d.id, d.rel_path, d.hash, c.folder_rel, d.doc_type_id, t.name, d.tax_year,
                    d.status, d.source
             FROM document d
             LEFT JOIN client c ON c.id = d.client_id
             LEFT JOIN doc_type t ON t.id = d.doc_type_id
             WHERE d.id = ?1 AND d.deleted_at IS NULL",
            [id],
            |r| {
                Ok(Meta {
                    id: r.get(0)?,
                    rel_path: r.get(1)?,
                    hash: r.get(2)?,
                    folder_rel: r.get(3)?,
                    doc_type: r.get(4)?,
                    doc_type_name: r.get(5)?,
                    tax_year: r.get(6)?,
                    status: r.get(7)?,
                    source: r.get(8)?,
                    tags: vec![],
                })
            },
        )
        .map_err(|e| format!("document {id}: {e}"))?;
    let mut st = conn
        .prepare_cached("SELECT tag FROM document_tag WHERE doc_id = ?1 ORDER BY tag")
        .map_err(|e| e.to_string())?;
    m.tags = st
        .query_map([id], |r| r.get(0))
        .and_then(|rows| rows.collect())
        .map_err(|e| e.to_string())?;
    Ok(m)
}

/// Tags as entered: comma separated, trimmed, no empties or duplicates (case-insensitive), sorted.
pub fn parse_tags(text: &str) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out: Vec<String> = text
        .split(',')
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty() && seen.insert(t.to_lowercase()))
        .collect();
    out.sort_by_key(|t| t.to_lowercase());
    out
}

fn check(conn: &Connection, e: &MetaEdit) -> Result<(), String> {
    if let Some(s) = &e.status
        && !STATUSES.iter().any(|(k, _)| k == s)
    {
        return Err(format!("unknown status {s}"));
    }
    if let Some(Some(s)) = &e.source
        && !SOURCES.iter().any(|(k, _)| k == s)
    {
        return Err(format!("unknown source {s}"));
    }
    if let Some(Some(y)) = e.tax_year
        && !(1900..=2100).contains(&y)
    {
        return Err(format!("{y} is not a tax year"));
    }
    if let Some(Some(t)) = &e.doc_type {
        let ok: i64 = conn
            .query_row(
                "SELECT count(*) FROM doc_type WHERE id = ?1 AND deleted_at IS NULL",
                [t],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if ok == 0 {
            return Err("That doc type is not in the list any more.".into());
        }
    }
    Ok(())
}

fn apply_edit(m: &Meta, e: &MetaEdit, type_names: &HashMap<String, String>) -> Meta {
    let mut n = m.clone();
    if let Some(t) = &e.doc_type {
        n.doc_type = t.clone();
        n.doc_type_name = t.as_ref().and_then(|t| type_names.get(t).cloned());
    }
    if let Some(y) = e.tax_year {
        n.tax_year = y;
    }
    if let Some(s) = &e.status {
        n.status = s.clone();
    }
    if let Some(s) = &e.source {
        n.source = s.clone();
    }
    if let Some(t) = &e.tags {
        n.tags = t.clone();
    } else if e.doc_type.is_some() || e.tax_year.is_some() {
        // A person set the doc type or year: the import's guess is confirmed or corrected.
        n.tags.retain(|t| t != GUESSED_TAG);
    }
    n
}

/// Where a document's metadata lines go, and its path as written there.
fn file_for(root: &Path, m: &Meta) -> (PathBuf, String) {
    if let Some(f) = &m.folder_rel
        && let Some(inner) = m.rel_path.strip_prefix(&format!("{f}/"))
    {
        return (root.join(f).join(VIBE_DIR).join(FILE), inner.to_owned());
    }
    (root.join(VIBE_DIR).join(FILE), m.rel_path.clone())
}

/// The `docmeta.jsonl` file for `m` and the line recording its current metadata.
pub fn record_line(root: &Path, m: &Meta, ts: &str, device: &str) -> (PathBuf, String) {
    let (file, path) = file_for(root, m);
    let r = Record {
        v: 1,
        ts: ts.into(),
        device: device.into(),
        hash: m.hash.clone(),
        path,
        doc_type: m.doc_type.clone(),
        doc_type_name: m.doc_type_name.clone(),
        tax_year: m.tax_year,
        status: m.status.clone(),
        source: m.source.clone(),
        tags: m.tags.clone(),
    };
    (file, serde_json::to_string(&r).expect("serializes"))
}

/// Catalog statements that set a document's metadata to `m`. Tag rows are replaced.
fn set_stmts(m: &Meta, device: &str, now: &str) -> Vec<Stmt> {
    let mut v = vec![
        Stmt::new(
            "UPDATE document SET doc_type_id = ?2, tax_year = ?3, status = ?4, source = ?5,
               device_id = ?6, updated_at = ?7, rev = rev + 1
             WHERE id = ?1 AND deleted_at IS NULL",
            vec![
                m.id.clone().into(),
                m.doc_type.clone().into(),
                m.tax_year.into(),
                m.status.clone().into(),
                m.source.clone().into(),
                device.into(),
                now.into(),
            ],
        ),
        Stmt::new(
            "DELETE FROM document_tag WHERE doc_id = ?1",
            vec![m.id.clone().into()],
        ),
    ];
    for t in &m.tags {
        v.push(Stmt::new(
            "INSERT INTO document_tag (doc_id, tag) VALUES (?1, ?2)",
            vec![m.id.clone().into(), t.clone().into()],
        ));
    }
    v
}

/// Applies `edit` to `ids` as one Transaction. Returns (transaction id, documents changed).
pub fn edit(
    conn: &mut Connection,
    root: &Path,
    ids: &[String],
    edit: &MetaEdit,
    device_id: &str,
) -> Result<(String, usize), String> {
    if edit.is_empty() {
        return Err("Nothing to change.".into());
    }
    let items: Vec<(String, MetaEdit)> = ids.iter().map(|i| (i.clone(), edit.clone())).collect();
    edit_each(conn, root, &items, LABEL, device_id)
}

/// Applies a different edit to each document, as one Transaction labelled `label` (the folder
/// import sets each file's own doc type and year). Returns (transaction id, documents changed).
pub fn edit_each(
    conn: &mut Connection,
    root: &Path,
    items: &[(String, MetaEdit)],
    label: &str,
    device_id: &str,
) -> Result<(String, usize), String> {
    for (_, e) in items {
        check(conn, e)?;
    }
    let type_names: HashMap<String, String> = conn
        .prepare("SELECT id, name FROM doc_type")
        .and_then(|mut st| st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
        .map_err(|e| e.to_string())?;
    let now = vf_core::now_rfc3339();
    let mut tx = Transaction::new(label);
    // Per file: (new lines, undo lines).
    let mut files: BTreeMap<PathBuf, (Vec<String>, Vec<String>)> = BTreeMap::new();
    let mut undo: Vec<Stmt> = Vec::new();
    let mut changed = 0;
    for (id, edit) in items {
        let old = load(conn, id)?;
        let new = apply_edit(&old, edit, &type_names);
        if new == old {
            continue;
        }
        changed += 1;
        for s in set_stmts(&new, device_id, &now) {
            tx = tx.catalog(s);
        }
        // Undo runs its statements newest first: push them in reverse.
        undo.extend(set_stmts(&old, device_id, &now).into_iter().rev());
        let (file, line) = record_line(root, &new, &now, device_id);
        let (_, undo_line) = record_line(root, &old, &now, device_id);
        let e = files.entry(file).or_default();
        e.0.push(line);
        e.1.push(undo_line);
    }
    if changed == 0 {
        return Err("The selected documents already have these values.".into());
    }
    for s in undo.into_iter().rev() {
        tx = tx.catalog_undo(s);
    }
    for (path, (lines, undo_lines)) in files {
        tx = tx.file(FileOp::AppendLines {
            path,
            lines,
            undo_lines,
        });
    }
    let any = |f: fn(&MetaEdit) -> bool| items.iter().any(|(_, e)| f(e));
    let fields: Vec<&str> = [
        ("doc_type", any(|e| e.doc_type.is_some())),
        ("tax_year", any(|e| e.tax_year.is_some())),
        ("status", any(|e| e.status.is_some())),
        ("source", any(|e| e.source.is_some())),
        ("tags", any(|e| e.tags.is_some())),
    ]
    .into_iter()
    .filter_map(|(k, on)| on.then_some(k))
    .collect();
    tx = tx.audit(AuditEntry::new(
        label,
        if changed == 1 {
            items.first().map(|(i, _)| i.as_str())
        } else {
            None
        },
        json!({ "documents": changed, "fields": fields }),
    ));
    let c = crate::fileops::commit(conn, root, tx, device_id)?;
    Ok((c.tx_id, changed))
}

/// Labels of the Browse changes "Undo last" offers (metadata edits, renames, moves).
pub const UNDOABLE: [&str; 3] = [LABEL, crate::rename::LABEL, crate::moveto::LABEL];

/// Every `docmeta.jsonl` record of the cabinet, for rebuild: the latest per (cabinet path, hash)
/// and per hash.
#[derive(Debug, Default)]
pub struct Index {
    by_path: HashMap<(String, String), Record>,
    by_hash: HashMap<String, Record>,
    pub bad_lines: usize,
}

impl Index {
    /// `folders`: cabinet-relative client folders (each may hold `_Vibe/docmeta.jsonl`).
    pub fn load(root: &Path, folders: &[String]) -> Index {
        let mut ix = Index::default();
        ix.read(&root.join(VIBE_DIR).join(FILE), "");
        for f in folders {
            ix.read(&root.join(f).join(VIBE_DIR).join(FILE), f);
        }
        ix
    }

    fn read(&mut self, file: &Path, folder: &str) {
        let Ok(text) = std::fs::read_to_string(file) else {
            return;
        };
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            match serde_json::from_str::<Record>(line) {
                Ok(r) => {
                    let rel = if folder.is_empty() {
                        r.path.clone()
                    } else {
                        format!("{folder}/{}", r.path)
                    };
                    self.by_hash.insert(r.hash.clone(), r.clone());
                    self.by_path.insert((rel, r.hash.clone()), r);
                }
                Err(_) => self.bad_lines += 1,
            }
        }
    }

    /// The record for a document at `rel` (cabinet-relative) with `hash`: same path first, else
    /// the latest record for that content (the file was renamed or moved).
    pub fn get(&self, rel: &str, hash: &str) -> Option<&Record> {
        self.by_path
            .get(&(rel.to_owned(), hash.to_owned()))
            .or_else(|| self.by_hash.get(hash))
    }
}

#[cfg(test)]
mod tests {
    use vf_core::db::Storage;

    use super::*;

    struct Fx {
        _t: tempfile::TempDir,
        root: PathBuf,
        conn: Connection,
        ids: Vec<String>,
        w2: String,
    }

    /// One client folder with `n` documents (catalog rows + files), doc types seeded.
    fn fx(n: usize) -> Fx {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("cab");
        let folder = "Acme LLC (1001)";
        std::fs::create_dir_all(root.join(folder).join("Income Tax/2025")).unwrap();
        let mut conn = vf_core::db::open(&t.path().join("c.db"), Storage::Local).unwrap();
        crate::doctypes::seed_if_missing(&root).unwrap();
        crate::doctypes::sync(&mut conn, &root, "dev", "seed").unwrap();
        conn.execute(
            "INSERT INTO client (id, ext_id, name, folder_rel, device_id, updated_at)
             VALUES ('c1', '1001', 'Acme LLC', ?1, 'dev', 't')",
            [folder],
        )
        .unwrap();
        let mut ids = Vec::new();
        for i in 0..n {
            let rel = format!("{folder}/Income Tax/2025/doc {i:03}.pdf");
            std::fs::write(root.join(&rel), format!("doc {i}")).unwrap();
            let id = format!("d{i:03}");
            conn.execute(
                "INSERT INTO document (id, client_id, tax_year, rel_path, hash, size, filed_at, device_id, updated_at)
                 VALUES (?1, 'c1', 2025, ?2, ?3, 5, 't', 'dev', 't')",
                (&id, &rel, format!("{i:064x}")),
            )
            .unwrap();
            ids.push(id);
        }
        let w2: String = conn
            .query_row("SELECT id FROM doc_type WHERE name = 'W-2'", [], |r| {
                r.get(0)
            })
            .unwrap();
        Fx {
            _t: t,
            root,
            conn,
            ids,
            w2,
        }
    }

    fn n(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    /// P01 done-when: a bulk edit of 200 documents is one Transaction, one audit batch, one undo.
    #[test]
    fn bulk_edit_of_200_is_one_transaction_one_audit_one_undo() {
        let mut f = fx(200);
        let audit_before = n(&f.conn, "SELECT count(*) FROM audit");
        let e = MetaEdit {
            doc_type: Some(Some(f.w2.clone())),
            status: Some("final".into()),
            source: Some(Some("client".into())),
            tags: Some(parse_tags("scanned, 2025")),
            ..Default::default()
        };
        let (tx_id, changed) = edit(&mut f.conn, &f.root, &f.ids, &e, "dev").unwrap();
        assert_eq!(changed, 200);
        assert_eq!(
            n(&f.conn, "SELECT count(*) FROM audit") - audit_before,
            1,
            "one audit row"
        );
        assert_eq!(
            n(
                &f.conn,
                "SELECT count(*) FROM undo_record WHERE label = 'doc_meta_edit'"
            ),
            1,
            "one undo record"
        );
        assert_eq!(
            n(
                &f.conn,
                &format!(
                    "SELECT count(*) FROM document WHERE doc_type_id = '{}' AND status = 'final'",
                    f.w2
                )
            ),
            200
        );
        assert_eq!(n(&f.conn, "SELECT count(*) FROM document_tag"), 400);
        let file = f.root.join("Acme LLC (1001)/_Vibe/docmeta.jsonl");
        assert_eq!(std::fs::read_to_string(&file).unwrap().lines().count(), 200);

        assert_eq!(
            vf_core::tx::last_undoable(&f.conn, "dev", &UNDOABLE).unwrap(),
            tx_id
        );
        crate::fileops::undo(&mut f.conn, &f.root, &tx_id, "dev").unwrap();
        assert_eq!(
            n(
                &f.conn,
                "SELECT count(*) FROM document WHERE doc_type_id IS NULL AND status = 'received'"
            ),
            200
        );
        assert_eq!(n(&f.conn, "SELECT count(*) FROM document_tag"), 0);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap().lines().count(),
            400,
            "undo appends the old values"
        );
        assert!(vf_core::tx::last_undoable(&f.conn, "dev", &UNDOABLE).is_none());
        // And the file's last word is the old state.
        let ix = Index::load(&f.root, &["Acme LLC (1001)".into()]);
        let r = ix
            .get(
                "Acme LLC (1001)/Income Tax/2025/doc 007.pdf",
                &format!("{:064x}", 7),
            )
            .unwrap();
        assert_eq!((r.status.as_str(), r.doc_type.as_ref()), ("received", None));
    }

    #[test]
    fn partial_edit_leaves_other_fields_and_tags_round_trip() {
        let mut f = fx(3);
        edit(
            &mut f.conn,
            &f.root,
            &f.ids[..1],
            &MetaEdit {
                tags: Some(parse_tags("K-1, k-1 , urgent,")),
                tax_year: Some(None),
                ..Default::default()
            },
            "dev",
        )
        .unwrap();
        let m = load(&f.conn, &f.ids[0]).unwrap();
        assert_eq!(m.tags, ["K-1", "urgent"]);
        assert_eq!(m.tax_year, None, "Permanent");
        assert_eq!(m.status, "received", "untouched");
        let other = load(&f.conn, &f.ids[1]).unwrap();
        assert_eq!(other.tax_year, Some(2025));
    }

    #[test]
    fn bad_edits_are_refused() {
        let mut f = fx(1);
        let ids = f.ids.clone();
        let mut bad = |e: MetaEdit| edit(&mut f.conn, &f.root, &ids, &e, "dev").unwrap_err();
        assert!(bad(MetaEdit::default()).contains("Nothing"));
        assert!(
            bad(MetaEdit {
                status: Some("lost".into()),
                ..Default::default()
            })
            .contains("status")
        );
        assert!(
            bad(MetaEdit {
                tax_year: Some(Some(25)),
                ..Default::default()
            })
            .contains("tax year")
        );
        assert!(
            bad(MetaEdit {
                doc_type: Some(Some("nope".into())),
                ..Default::default()
            })
            .contains("doc type")
        );
        assert!(
            bad(MetaEdit {
                status: Some("received".into()),
                ..Default::default()
            })
            .contains("already")
        );
        assert_eq!(
            n(
                &f.conn,
                "SELECT count(*) FROM undo_record WHERE label = 'doc_meta_edit'"
            ),
            0
        );
    }
}
