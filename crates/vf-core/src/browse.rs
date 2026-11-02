//! Read-side queries behind the Browse tab (PHASES P01 step 2): the client tree with live counts,
//! a client's Permanent / year buckets, the folders inside a bucket, and the document grid.
//!
//! Folders are not catalog rows: a folder is the parent path of a document relative to its
//! client folder, so the tree always matches what is on disk.

use std::collections::BTreeMap;

use rusqlite::{Connection, params_from_iter};

use crate::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientRow {
    pub id: String,
    pub ext_id: String,
    pub name: String,
    pub entity_type: Option<String>,
    pub folder_rel: Option<String>,
    pub docs: i64,
}

/// A client's top-level grouping: documents with no tax year, or one tax year.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Bucket {
    Permanent,
    Year(i64),
}

impl Bucket {
    pub fn label(self) -> String {
        match self {
            Bucket::Permanent => "Permanent".into(),
            Bucket::Year(y) => y.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketRow {
    pub bucket: Bucket,
    pub docs: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderRow {
    /// Relative to the client folder, `/`-separated; `""` is the client folder itself.
    pub folder: String,
    pub docs: i64,
}

/// Which documents the grid shows.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Scope {
    Client(String),
    Bucket(String, Bucket),
    Folder(String, Bucket, String),
    /// Documents in folders without a client sentinel.
    Unfiled,
    /// Explicit document ids (virtual folders, search results).
    Ids(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocRow {
    pub id: String,
    pub client_id: Option<String>,
    pub rel_path: String,
    pub doc_type_id: Option<String>,
    pub doc_type: Option<String>,
    pub tax_year: Option<i64>,
    pub status: String,
    pub source: Option<String>,
    pub size: i64,
    pub hash: String,
    pub filed_by: Option<String>,
    pub filed_at: String,
    pub updated_at: String,
    /// Comma-joined, sorted.
    pub tags: String,
}

impl DocRow {
    pub fn name(&self) -> &str {
        self.rel_path.rsplit('/').next().unwrap_or(&self.rel_path)
    }
    pub fn hash_short(&self) -> &str {
        &self.hash[..self.hash.len().min(8)]
    }
}

/// Live clients whose name or ID contains `filter` (case-insensitive), with document counts,
/// sorted by name.
pub fn clients(conn: &Connection, filter: &str) -> Result<Vec<ClientRow>> {
    let like = format!("%{}%", filter.trim().replace(['%', '_'], ""));
    let mut st = conn.prepare_cached(
        "SELECT c.id, c.ext_id, c.name, c.entity_type, c.folder_rel,
                (SELECT count(*) FROM document d WHERE d.client_id = c.id AND d.deleted_at IS NULL)
         FROM client c
         WHERE c.deleted_at IS NULL AND (c.name LIKE ?1 OR c.ext_id LIKE ?1)
         ORDER BY c.name COLLATE NOCASE, c.ext_id",
    )?;
    let rows = st.query_map([like], |r| {
        Ok(ClientRow {
            id: r.get(0)?,
            ext_id: r.get(1)?,
            name: r.get(2)?,
            entity_type: r.get(3)?,
            folder_rel: r.get(4)?,
            docs: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// `Permanent` first (when it has documents), then years newest first.
pub fn buckets(conn: &Connection, client_id: &str) -> Result<Vec<BucketRow>> {
    let mut st = conn.prepare_cached(
        "SELECT tax_year, count(*) FROM document
         WHERE client_id = ?1 AND deleted_at IS NULL
         GROUP BY tax_year ORDER BY tax_year IS NOT NULL, tax_year DESC",
    )?;
    let rows = st.query_map([client_id], |r| {
        let y: Option<i64> = r.get(0)?;
        Ok(BucketRow {
            bucket: y.map_or(Bucket::Permanent, Bucket::Year),
            docs: r.get(1)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn client_prefix(conn: &Connection, client_id: &str) -> Result<String> {
    let folder: Option<String> = conn.query_row(
        "SELECT folder_rel FROM client WHERE id = ?1",
        [client_id],
        |r| r.get(0),
    )?;
    Ok(folder.map(|f| format!("{f}/")).unwrap_or_default())
}

/// Folder of `rel_path` relative to `prefix` (`""` when directly in it).
fn folder_of<'a>(rel_path: &'a str, prefix: &str) -> &'a str {
    let inner = rel_path.strip_prefix(prefix).unwrap_or(rel_path);
    inner.rfind('/').map_or("", |i| &inner[..i])
}

fn bucket_clause(b: Bucket) -> (&'static str, Option<i64>) {
    match b {
        Bucket::Permanent => ("tax_year IS NULL", None),
        Bucket::Year(y) => ("tax_year = ?2", Some(y)),
    }
}

/// Folders holding the bucket's documents, sorted by path.
pub fn folders(conn: &Connection, client_id: &str, bucket: Bucket) -> Result<Vec<FolderRow>> {
    let prefix = client_prefix(conn, client_id)?;
    let (clause, year) = bucket_clause(bucket);
    let sql = format!(
        "SELECT rel_path FROM document WHERE client_id = ?1 AND deleted_at IS NULL AND {clause}"
    );
    let mut st = conn.prepare_cached(&sql)?;
    let mut counts: BTreeMap<String, i64> = BTreeMap::new();
    let mut add = |p: String| *counts.entry(folder_of(&p, &prefix).to_owned()).or_default() += 1;
    match year {
        Some(y) => {
            for p in st.query_map(rusqlite::params![client_id, y], |r| r.get(0))? {
                add(p?);
            }
        }
        None => {
            for p in st.query_map([client_id], |r| r.get(0))? {
                add(p?);
            }
        }
    }
    Ok(counts
        .into_iter()
        .map(|(folder, docs)| FolderRow { folder, docs })
        .collect())
}

const DOC_COLS: &str = "d.id, d.client_id, d.rel_path, d.doc_type_id, t.name, d.tax_year, d.status,
     d.source, d.size, d.hash, d.filed_by, d.filed_at, d.updated_at,
     (SELECT group_concat(tag, ', ') FROM (SELECT tag FROM document_tag g WHERE g.doc_id = d.id ORDER BY tag))";

fn doc_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<DocRow> {
    Ok(DocRow {
        id: r.get(0)?,
        client_id: r.get(1)?,
        rel_path: r.get(2)?,
        doc_type_id: r.get(3)?,
        doc_type: r.get(4)?,
        tax_year: r.get(5)?,
        status: r.get(6)?,
        source: r.get(7)?,
        size: r.get(8)?,
        hash: r.get(9)?,
        filed_by: r.get(10)?,
        filed_at: r.get(11)?,
        updated_at: r.get(12)?,
        tags: r.get::<_, Option<String>>(13)?.unwrap_or_default(),
    })
}

/// Documents in `scope`, sorted by path.
pub fn documents(conn: &Connection, scope: &Scope) -> Result<Vec<DocRow>> {
    let base = format!(
        "SELECT {DOC_COLS} FROM document d LEFT JOIN doc_type t ON t.id = d.doc_type_id
         WHERE d.deleted_at IS NULL"
    );
    let mut params: Vec<rusqlite::types::Value> = Vec::new();
    let filter = match scope {
        Scope::Client(c) => {
            params.push(c.clone().into());
            " AND d.client_id = ?1".to_owned()
        }
        Scope::Bucket(c, b) | Scope::Folder(c, b, _) => {
            params.push(c.clone().into());
            match b {
                Bucket::Permanent => " AND d.client_id = ?1 AND d.tax_year IS NULL".to_owned(),
                Bucket::Year(y) => {
                    params.push((*y).into());
                    " AND d.client_id = ?1 AND d.tax_year = ?2".to_owned()
                }
            }
        }
        Scope::Unfiled => " AND d.client_id IS NULL".to_owned(),
        Scope::Ids(ids) => {
            if ids.is_empty() {
                return Ok(Vec::new());
            }
            let marks: Vec<String> = (1..=ids.len()).map(|i| format!("?{i}")).collect();
            params.extend(ids.iter().map(|i| i.clone().into()));
            format!(" AND d.id IN ({})", marks.join(","))
        }
    };
    let sql = format!("{base}{filter} ORDER BY d.rel_path");
    let mut st = conn.prepare_cached(&sql)?;
    let mut rows: Vec<DocRow> = st
        .query_map(params_from_iter(params), doc_row)?
        .collect::<rusqlite::Result<_>>()?;
    if let Scope::Folder(c, _, folder) = scope {
        let prefix = client_prefix(conn, c)?;
        rows.retain(|d| folder_of(&d.rel_path, &prefix) == folder);
    }
    Ok(rows)
}

/// `(live documents, live clients)`.
pub fn totals(conn: &Connection) -> Result<(i64, i64)> {
    Ok(conn.query_row(
        "SELECT (SELECT count(*) FROM document WHERE deleted_at IS NULL),
                (SELECT count(*) FROM client WHERE deleted_at IS NULL)",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?)
}

/// Changes whenever anything in the catalog changes (every mutation writes an audit row), so the
/// UI can keep counts live without re-querying every frame.
pub fn generation(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT coalesce(max(seq), 0) FROM audit", [], |r| r.get(0))?)
}

/// One audit row as the inspector shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    pub ts: String,
    pub user: Option<String>,
    pub action: String,
    pub detail: String,
}

/// The last `n` audit rows for one document, newest first.
pub fn recent_activity(conn: &Connection, doc_id: &str, n: usize) -> Result<Vec<Activity>> {
    let mut st = conn.prepare_cached(
        "SELECT ts, user, action, detail FROM audit WHERE doc_id = ?1 ORDER BY seq DESC LIMIT ?2",
    )?;
    let rows = st.query_map(rusqlite::params![doc_id, n as i64], |r| {
        Ok(Activity {
            ts: r.get(0)?,
            user: r.get(1)?,
            action: r.get(2)?,
            detail: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn version_count(conn: &Connection, doc_id: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT count(*) FROM version WHERE doc_id = ?1",
        [doc_id],
        |r| r.get(0),
    )?)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::db::{CatalogKey, Storage, open};

    pub fn catalog() -> (tempfile::TempDir, Connection) {
        let tmp = tempfile::tempdir().unwrap();
        let conn = open(
            &tmp.path().join("c.db"),
            &CatalogKey([3; 32]),
            Storage::Local,
        )
        .unwrap();
        (tmp, conn)
    }

    pub fn add_client(c: &Connection, id: &str, ext: &str, name: &str) {
        c.execute(
            "INSERT INTO client (id, ext_id, name, folder_rel, device_id, updated_at)
             VALUES (?1, ?2, ?3, ?4, 'd', 't')",
            [id, ext, name, &format!("{name} ({ext})")],
        )
        .unwrap();
    }

    pub fn add_doc(c: &Connection, id: &str, client: Option<&str>, year: Option<i64>, rel: &str) {
        c.execute(
            "INSERT INTO document (id, client_id, tax_year, rel_path, hash, size, filed_at, device_id, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 10, '2026-11-01T00:00:00Z', 'd', 't')",
            rusqlite::params![id, client, year, rel, format!("{id:0>64}")],
        )
        .unwrap();
    }

    fn sample() -> (tempfile::TempDir, Connection) {
        let (t, c) = catalog();
        add_client(&c, "c1", "1042", "Anderson Farms LLC");
        add_client(&c, "c2", "1001", "Baker, Ann");
        let f = "Anderson Farms LLC (1042)";
        add_doc(
            &c,
            "d1",
            Some("c1"),
            Some(2025),
            &format!("{f}/Income Tax/2025/w2.pdf"),
        );
        add_doc(
            &c,
            "d2",
            Some("c1"),
            Some(2025),
            &format!("{f}/Income Tax/2025/1099.pdf"),
        );
        add_doc(
            &c,
            "d3",
            Some("c1"),
            Some(2025),
            &format!("{f}/Bookkeeping/2025/bank.pdf"),
        );
        add_doc(
            &c,
            "d4",
            Some("c1"),
            Some(2024),
            &format!("{f}/Income Tax/2024/w2.pdf"),
        );
        add_doc(
            &c,
            "d5",
            Some("c1"),
            None,
            &format!("{f}/Permanent/Setup/ein.pdf"),
        );
        add_doc(&c, "d6", Some("c1"), None, &format!("{f}/readme.txt"));
        add_doc(&c, "d7", None, None, "Loose/scan.pdf");
        (t, c)
    }

    #[test]
    fn clients_filter_and_count() {
        let (_t, c) = sample();
        let all = clients(&c, "").unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(
            (all[0].name.as_str(), all[0].docs),
            ("Anderson Farms LLC", 6)
        );
        assert_eq!(clients(&c, "1001").unwrap()[0].name, "Baker, Ann");
        assert_eq!(clients(&c, "farms").unwrap().len(), 1);
        assert_eq!(totals(&c).unwrap(), (7, 2));
    }

    #[test]
    fn buckets_permanent_first_then_years_desc() {
        let (_t, c) = sample();
        let b: Vec<_> = buckets(&c, "c1")
            .unwrap()
            .into_iter()
            .map(|b| (b.bucket, b.docs))
            .collect();
        assert_eq!(
            b,
            [
                (Bucket::Permanent, 2),
                (Bucket::Year(2025), 3),
                (Bucket::Year(2024), 1)
            ]
        );
    }

    #[test]
    fn folders_group_by_parent_path() {
        let (_t, c) = sample();
        let f: Vec<_> = folders(&c, "c1", Bucket::Year(2025))
            .unwrap()
            .into_iter()
            .map(|f| (f.folder, f.docs))
            .collect();
        assert_eq!(
            f,
            [
                ("Bookkeeping/2025".to_owned(), 1),
                ("Income Tax/2025".to_owned(), 2)
            ]
        );
        let p: Vec<_> = folders(&c, "c1", Bucket::Permanent)
            .unwrap()
            .into_iter()
            .map(|f| f.folder)
            .collect();
        assert_eq!(p, ["", "Permanent/Setup"]);
    }

    #[test]
    fn documents_by_scope() {
        let (_t, c) = sample();
        let ids = |s: Scope| -> Vec<String> {
            documents(&c, &s)
                .unwrap()
                .into_iter()
                .map(|d| d.id)
                .collect()
        };
        assert_eq!(ids(Scope::Client("c1".into())).len(), 6);
        assert_eq!(
            ids(Scope::Bucket("c1".into(), Bucket::Year(2025))),
            ["d3", "d2", "d1"]
        );
        assert_eq!(
            ids(Scope::Folder(
                "c1".into(),
                Bucket::Year(2025),
                "Income Tax/2025".into()
            )),
            ["d2", "d1"]
        );
        assert_eq!(ids(Scope::Unfiled), ["d7"]);
        assert_eq!(
            ids(Scope::Ids(vec!["d4".into(), "d7".into()])),
            ["d4", "d7"]
        );
        let d = &documents(&c, &Scope::Ids(vec!["d1".into()])).unwrap()[0];
        assert_eq!((d.name(), d.hash_short()), ("w2.pdf", "00000000"));
    }

    /// P01 done-when: tree expansion under 100 ms at 5,000 documents.
    #[test]
    fn tree_expansion_is_fast_at_5000_docs() {
        let (_t, mut c) = catalog();
        let tx = c.transaction().unwrap();
        for i in 0..50 {
            add_client(
                &tx,
                &format!("c{i}"),
                &format!("{}", 1000 + i),
                &format!("Client {i:02}"),
            );
        }
        let folders_ = ["Income Tax", "Bookkeeping", "Payroll", "Correspondence"];
        for n in 0..5000 {
            let ci = n % 50;
            let year = 2018 + (n / 50 % 8) as i64;
            let rel = format!(
                "Client {ci:02} ({})/{}/{year}/doc{n}.pdf",
                1000 + ci,
                folders_[n % 4]
            );
            add_doc(
                &tx,
                &format!("d{n}"),
                Some(&format!("c{ci}")),
                Some(year),
                &rel,
            );
        }
        tx.commit().unwrap();
        // Warm the statement cache once, as the UI does on first paint.
        clients(&c, "").unwrap();
        let t = std::time::Instant::now();
        let list = clients(&c, "").unwrap();
        let b = buckets(&c, "c7").unwrap();
        let f = folders(&c, "c7", b[0].bucket).unwrap();
        let docs = documents(
            &c,
            &Scope::Folder("c7".into(), b[0].bucket, f[0].folder.clone()),
        )
        .unwrap();
        let took = t.elapsed();
        assert_eq!(list.len(), 50);
        assert_eq!(list.iter().map(|c| c.docs).sum::<i64>(), 5000);
        assert!(!docs.is_empty());
        assert!(took.as_millis() < 100, "tree expansion took {took:?}");
    }
}
