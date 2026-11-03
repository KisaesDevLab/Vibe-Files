//! Read model for the Browse tab: one load of the live catalog rows, then the client tree and
//! folder filters are built in memory (tree expansion never touches SQLite).
//!
//! Tree shape (UI_SPEC §1): Cabinet → client → `Permanent` / tax years (descending) → folders.
//! A folder is the document's directory inside its client folder with the `{Year}` segment
//! removed, so `Income Tax/2025/x.pdf` sits under year 2025 as `Income Tax`.

use std::collections::BTreeMap;

use rusqlite::Connection;

use crate::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientRow {
    pub id: String,
    pub ext_id: String,
    pub name: String,
    pub folder_rel: Option<String>,
    pub entity_type: Option<String>,
}

/// A live `client_relationship` row (ids are client uuids).
#[derive(Debug, Clone, PartialEq)]
pub struct RelRow {
    pub parent_id: String,
    pub child_id: String,
    pub kind: String,
    pub share_pct: Option<f64>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocRow {
    pub id: String,
    pub client_id: Option<String>,
    pub doc_type_id: Option<String>,
    /// Doc type name.
    pub doc_type: Option<String>,
    /// `None` = permanent file.
    pub tax_year: Option<i32>,
    pub rel_path: String,
    pub hash: String,
    pub size: i64,
    pub status: String,
    pub source: Option<String>,
    pub filed_by: Option<String>,
    /// When it entered the cabinet (RFC 3339 UTC).
    pub filed_at: String,
    pub updated_at: String,
    pub locked_by: Option<String>,
    pub versions: i64,
    /// Sorted.
    pub tags: Vec<String>,
}

impl DocRow {
    /// File name (last path segment).
    pub fn name(&self) -> &str {
        self.rel_path.rsplit('/').next().unwrap_or(&self.rel_path)
    }
}

/// Live clients (by name) and documents (by path).
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub clients: Vec<ClientRow>,
    pub docs: Vec<DocRow>,
    pub relationships: Vec<RelRow>,
    /// Live doc types (id, name) in list order.
    pub doc_types: Vec<(String, String)>,
    /// Doc type id → its default folder below the client folder (`Income Tax/{Year}`).
    pub doc_type_folders: std::collections::HashMap<String, String>,
}

pub fn load(conn: &Connection) -> Result<Snapshot> {
    let mut st = conn.prepare(
        "SELECT id, ext_id, name, folder_rel, entity_type FROM client
         WHERE deleted_at IS NULL ORDER BY name COLLATE NOCASE, ext_id",
    )?;
    let clients = st
        .query_map([], |r| {
            Ok(ClientRow {
                id: r.get(0)?,
                ext_id: r.get(1)?,
                name: r.get(2)?,
                folder_rel: r.get(3)?,
                entity_type: r.get(4)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut st = conn.prepare(
        "SELECT d.id, d.client_id, t.name, d.tax_year, d.rel_path, d.hash, d.size, d.status,
                d.source, d.filed_by, d.updated_at, l.user,
                (SELECT count(*) FROM version v WHERE v.doc_id = d.id),
                d.doc_type_id,
                (SELECT group_concat(tag, char(31)) FROM
                   (SELECT tag FROM document_tag g WHERE g.doc_id = d.id ORDER BY tag COLLATE NOCASE)),
                d.filed_at
         FROM document d
         LEFT JOIN doc_type t ON t.id = d.doc_type_id
         LEFT JOIN lock l ON l.doc_id = d.id
         WHERE d.deleted_at IS NULL
         ORDER BY d.rel_path",
    )?;
    let docs = st
        .query_map([], |r| {
            Ok(DocRow {
                id: r.get(0)?,
                client_id: r.get(1)?,
                doc_type: r.get(2)?,
                tax_year: r.get(3)?,
                rel_path: r.get(4)?,
                hash: r.get(5)?,
                size: r.get(6)?,
                status: r.get(7)?,
                source: r.get(8)?,
                filed_by: r.get(9)?,
                updated_at: r.get(10)?,
                locked_by: r.get(11)?,
                versions: r.get(12)?,
                doc_type_id: r.get(13)?,
                tags: r
                    .get::<_, Option<String>>(14)?
                    .map(|t| t.split('\u{1f}').map(str::to_owned).collect())
                    .unwrap_or_default(),
                filed_at: r.get(15)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut st = conn.prepare(
        "SELECT parent_id, child_id, kind, share_pct, note FROM client_relationship
         WHERE deleted_at IS NULL ORDER BY parent_id, child_id, kind",
    )?;
    let relationships = st
        .query_map([], |r| {
            Ok(RelRow {
                parent_id: r.get(0)?,
                child_id: r.get(1)?,
                kind: r.get(2)?,
                share_pct: r.get(3)?,
                note: r.get(4)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut st = conn.prepare(
        "SELECT id, name, default_folder_tpl FROM doc_type WHERE deleted_at IS NULL
         ORDER BY sort, name",
    )?;
    let rows = st
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let doc_type_folders = rows
        .iter()
        .filter_map(|(id, _, f)| f.clone().map(|f| (id.clone(), f)))
        .collect();
    let doc_types = rows.into_iter().map(|(id, n, _)| (id, n)).collect();
    Ok(Snapshot {
        clients,
        docs,
        relationships,
        doc_types,
        doc_type_folders,
    })
}

/// A tree node the grid can show. `year: None` is the Permanent node.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Scope {
    /// Every document in the cabinet.
    Cabinet,
    Client(usize),
    Year {
        client: usize,
        year: Option<i32>,
    },
    Folder {
        client: usize,
        year: Option<i32>,
        folder: String,
    },
    /// Documents with no client (outside any bound client folder).
    Unassigned,
    /// No doc type yet.
    NeedsDocType,
    /// No tax year, and not under a `Permanent` folder (where no year is expected).
    NeedsTaxYear,
    /// Filed at or after this RFC 3339 UTC time.
    ReceivedSince(String),
    /// A list of document ids kept outside the catalog (Pinned, Recent).
    Ids {
        list: IdList,
        ids: Vec<String>,
    },
    /// A saved search (P03 step 6): the documents it found when it was opened.
    Saved {
        name: String,
        ids: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IdList {
    Recent,
    Pinned,
}

impl IdList {
    pub fn label(self) -> &'static str {
        match self {
            IdList::Recent => "Recent",
            IdList::Pinned => "Pinned",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct YearNode {
    pub year: Option<i32>,
    pub count: usize,
    /// (folder, count), by folder name. `""` = directly in the client folder.
    pub folders: Vec<(String, usize)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientNode {
    pub count: usize,
    /// Permanent first (when present), then years descending.
    pub years: Vec<YearNode>,
}

/// The tree plus each document's place in it. Indices refer to `Snapshot::clients` / `docs`.
#[derive(Debug, Clone, Default)]
pub struct Index {
    pub clients: Vec<ClientNode>,
    pub unassigned: usize,
    /// Per document: (client index, folder key).
    pub place: Vec<(Option<usize>, String)>,
}

impl Index {
    pub fn build(s: &Snapshot) -> Index {
        let by_id: BTreeMap<&str, usize> = s
            .clients
            .iter()
            .enumerate()
            .map(|(i, c)| (c.id.as_str(), i))
            .collect();
        // client → year → folder → count
        let mut acc: Vec<BTreeMap<Option<i32>, BTreeMap<String, usize>>> =
            vec![BTreeMap::new(); s.clients.len()];
        let mut place = Vec::with_capacity(s.docs.len());
        let mut unassigned = 0;
        for d in &s.docs {
            let ci = d.client_id.as_deref().and_then(|id| by_id.get(id).copied());
            let folder = match ci {
                Some(ci) => folder_key(&s.clients[ci], d),
                None => String::new(),
            };
            match ci {
                Some(ci) => {
                    *acc[ci]
                        .entry(d.tax_year)
                        .or_default()
                        .entry(folder.clone())
                        .or_default() += 1
                }
                None => unassigned += 1,
            }
            place.push((ci, folder));
        }
        let clients = acc
            .into_iter()
            .map(|years| {
                let mut ys: Vec<YearNode> = years
                    .into_iter()
                    .map(|(year, folders)| YearNode {
                        year,
                        count: folders.values().sum(),
                        folders: folders.into_iter().collect(),
                    })
                    .collect();
                // None (Permanent) first, then years descending.
                ys.sort_by_key(|y| (y.year.is_some(), std::cmp::Reverse(y.year)));
                ClientNode {
                    count: ys.iter().map(|y| y.count).sum(),
                    years: ys,
                }
            })
            .collect();
        Index {
            clients,
            unassigned,
            place,
        }
    }

    /// Adds client `ci`'s folders that exist on disk (`dirs`: leaf folders relative to the
    /// client folder, `/`-separated), so the template's folders show before anything is filed in
    /// them. A folder's first segment that is a tax year (1990..=2100) puts it under that year,
    /// the way filed documents are placed; folders with no year go under Permanent / no year.
    pub fn add_folders(&mut self, ci: usize, dirs: &[String]) {
        let Some(node) = self.clients.get_mut(ci) else {
            return;
        };
        for dir in dirs {
            let segs: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
            let year_at = segs.iter().position(|s| {
                s.len() == 4 && s.parse::<i32>().is_ok_and(|y| (1990..=2100).contains(&y))
            });
            let year = year_at.map(|i| segs[i].parse::<i32>().expect("checked"));
            let folder: String = segs
                .iter()
                .enumerate()
                .filter(|(i, _)| Some(*i) != year_at)
                .map(|(_, s)| *s)
                .collect::<Vec<_>>()
                .join("/");
            let y = match node.years.iter_mut().position(|y| y.year == year) {
                Some(i) => &mut node.years[i],
                None => {
                    node.years.push(YearNode {
                        year,
                        count: 0,
                        folders: vec![],
                    });
                    node.years.last_mut().expect("just pushed")
                }
            };
            if !y.folders.iter().any(|(f, _)| *f == folder) {
                y.folders.push((folder, 0));
                y.folders.sort();
            }
        }
        node.years
            .sort_by_key(|y| (y.year.is_some(), std::cmp::Reverse(y.year)));
    }

    /// Indices into `Snapshot::docs` inside `scope`, in path order.
    pub fn docs_in(&self, s: &Snapshot, scope: &Scope) -> Vec<usize> {
        let ids: std::collections::HashSet<&str> = match scope {
            Scope::Ids { ids, .. } | Scope::Saved { ids, .. } => {
                ids.iter().map(String::as_str).collect()
            }
            _ => Default::default(),
        };
        (0..s.docs.len())
            .filter(|&i| {
                let (ci, folder) = &self.place[i];
                let d = &s.docs[i];
                match scope {
                    Scope::Cabinet => true,
                    Scope::Unassigned => ci.is_none(),
                    Scope::Client(c) => *ci == Some(*c),
                    Scope::Year { client, year } => *ci == Some(*client) && d.tax_year == *year,
                    Scope::Folder {
                        client,
                        year,
                        folder: f,
                    } => *ci == Some(*client) && d.tax_year == *year && folder == f,
                    Scope::NeedsDocType => d.doc_type_id.is_none(),
                    Scope::NeedsTaxYear => {
                        let permanent = |p: &str| {
                            p.split('/')
                                .any(|seg| seg.eq_ignore_ascii_case("permanent"))
                        };
                        d.tax_year.is_none() && !permanent(folder) && !permanent(&d.rel_path)
                    }
                    Scope::ReceivedSince(t) => d.filed_at.as_str() >= t.as_str(),
                    Scope::Ids { .. } | Scope::Saved { .. } => ids.contains(d.id.as_str()),
                }
            })
            .collect()
    }
}

/// The document's directory inside its client folder, minus the segment naming its tax year.
fn folder_key(c: &ClientRow, d: &DocRow) -> String {
    let inner = match c.folder_rel.as_deref() {
        Some(f) if !f.is_empty() => d
            .rel_path
            .strip_prefix(f)
            .and_then(|r| r.strip_prefix('/'))
            .unwrap_or(&d.rel_path),
        _ => &d.rel_path,
    };
    let dir = inner.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
    let year = d.tax_year.map(|y| y.to_string());
    let mut dropped = false;
    dir.split('/')
        .filter(|seg| {
            if !dropped && year.as_deref() == Some(*seg) {
                dropped = true;
                false
            } else {
                !seg.is_empty()
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(id: &str, name: &str) -> ClientRow {
        ClientRow {
            id: id.into(),
            ext_id: id.into(),
            name: name.into(),
            folder_rel: Some(format!("{name} ({id})")),
            entity_type: None,
        }
    }

    fn doc(client: Option<&str>, year: Option<i32>, rel: &str) -> DocRow {
        DocRow {
            id: rel.into(),
            client_id: client.map(Into::into),
            doc_type_id: None,
            doc_type: None,
            tax_year: year,
            rel_path: rel.into(),
            hash: "ab".repeat(32),
            size: 1,
            status: "received".into(),
            source: None,
            filed_by: None,
            filed_at: "2026-11-02T00:00:00Z".into(),
            updated_at: "2026-11-02T00:00:00Z".into(),
            locked_by: None,
            versions: 0,
            tags: vec![],
        }
    }

    fn snap() -> Snapshot {
        Snapshot {
            clients: vec![client("1", "Acme"), client("2", "Baker")],
            docs: vec![
                doc(Some("1"), Some(2024), "Acme (1)/Income Tax/2024/a.pdf"),
                doc(Some("1"), Some(2025), "Acme (1)/Income Tax/2025/b.pdf"),
                doc(Some("1"), Some(2025), "Acme (1)/Income Tax/2025/c.pdf"),
                doc(Some("1"), Some(2025), "Acme (1)/Other/Practice/2025/d.pdf"),
                doc(Some("1"), None, "Acme (1)/Permanent/Setup/e.pdf"),
                doc(Some("1"), None, "Acme (1)/f.pdf"),
                doc(Some("2"), Some(2025), "Baker (2)/Payroll/2025/g.pdf"),
                doc(None, None, "Loose/h.pdf"),
            ],
            relationships: vec![],
            doc_types: vec![],
            doc_type_folders: Default::default(),
        }
    }

    /// Folders on disk show under the client before anything is filed in them.
    #[test]
    fn empty_template_folders_join_the_tree() {
        let s = Snapshot {
            clients: vec![client("c1", "Stockton Family Seed")],
            ..Default::default()
        };
        let mut ix = Index::build(&s);
        assert!(ix.clients[0].years.is_empty());
        ix.add_folders(
            0,
            &[
                "Income Tax/2025".into(),
                "Income Tax/2024".into(),
                "Bookkeeping/2025".into(),
                "Permanent/Setup".into(),
                "Not Yet Filed".into(),
                "Other/Practice/2025".into(),
            ],
        );
        let years: Vec<_> = ix.clients[0].years.iter().map(|y| y.year).collect();
        assert_eq!(years, [None, Some(2025), Some(2024)]);
        let folders = |i: usize| -> Vec<&str> {
            ix.clients[0].years[i]
                .folders
                .iter()
                .map(|(f, _)| f.as_str())
                .collect()
        };
        assert_eq!(folders(0), ["Not Yet Filed", "Permanent/Setup"]);
        assert_eq!(folders(1), ["Bookkeeping", "Income Tax", "Other/Practice"]);
        assert_eq!(ix.clients[0].count, 0);
        // Adding again changes nothing.
        let before = ix.clients[0].clone();
        ix.add_folders(0, &["Income Tax/2025".into()]);
        assert_eq!(ix.clients[0], before);
    }

    #[test]
    fn tree_groups_permanent_then_years_descending() {
        let s = snap();
        let ix = Index::build(&s);
        let acme = &ix.clients[0];
        assert_eq!(acme.count, 6);
        let years: Vec<_> = acme.years.iter().map(|y| (y.year, y.count)).collect();
        assert_eq!(years, [(None, 2), (Some(2025), 3), (Some(2024), 1)]);
        assert_eq!(
            acme.years[1].folders,
            [
                ("Income Tax".to_string(), 2),
                ("Other/Practice".to_string(), 1)
            ]
        );
        assert_eq!(
            acme.years[0].folders,
            [(String::new(), 1), ("Permanent/Setup".to_string(), 1)]
        );
        assert_eq!(ix.clients[1].count, 1);
        assert_eq!(ix.unassigned, 1);
    }

    #[test]
    fn scopes_filter_documents() {
        let s = snap();
        let ix = Index::build(&s);
        let names = |sc: Scope| -> Vec<String> {
            ix.docs_in(&s, &sc)
                .into_iter()
                .map(|i| s.docs[i].name().to_owned())
                .collect()
        };
        assert_eq!(names(Scope::Cabinet).len(), 8);
        assert_eq!(names(Scope::Client(1)), ["g.pdf"]);
        assert_eq!(
            names(Scope::Year {
                client: 0,
                year: Some(2025)
            }),
            ["b.pdf", "c.pdf", "d.pdf"]
        );
        assert_eq!(
            names(Scope::Folder {
                client: 0,
                year: Some(2025),
                folder: "Income Tax".into()
            }),
            ["b.pdf", "c.pdf"]
        );
        assert_eq!(names(Scope::Unassigned), ["h.pdf"]);
    }

    #[test]
    fn view_scopes() {
        let mut s = snap();
        s.docs[0].doc_type_id = Some("w2".into());
        s.docs[1].filed_at = "2026-11-05T09:00:00Z".into();
        let ix = Index::build(&s);
        let names = |sc: Scope| -> Vec<String> {
            ix.docs_in(&s, &sc)
                .into_iter()
                .map(|i| s.docs[i].name().to_owned())
                .collect()
        };
        assert_eq!(names(Scope::NeedsDocType).len(), 7);
        // f.pdf (client folder root) and h.pdf (loose) have no year and are not Permanent.
        assert_eq!(names(Scope::NeedsTaxYear), ["f.pdf", "h.pdf"]);
        assert_eq!(
            names(Scope::ReceivedSince("2026-11-03T00:00:00Z".into())),
            ["b.pdf"]
        );
        assert_eq!(
            names(Scope::Ids {
                list: IdList::Pinned,
                ids: vec!["Acme (1)/Income Tax/2025/c.pdf".into(), "gone".into()],
            }),
            ["c.pdf"]
        );
    }

    #[test]
    fn year_segment_dropped_once_only() {
        let c = client("1", "Acme");
        let d = doc(Some("1"), Some(2025), "Acme (1)/Income Tax/2025/2025/x.pdf");
        assert_eq!(folder_key(&c, &d), "Income Tax/2025");
    }

    #[test]
    fn loads_live_rows_from_the_catalog() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&tmp.path().join("c.db"), crate::db::Storage::Local).unwrap();
        conn.execute_batch(
            "INSERT INTO client (id, ext_id, name, folder_rel, device_id, updated_at)
               VALUES ('c1', '1001', 'Acme', 'Acme (1001)', 'd', 't');
             INSERT INTO document (id, client_id, tax_year, rel_path, hash, size, filed_at, device_id, updated_at)
               VALUES ('d1', 'c1', 2025, 'Acme (1001)/Income Tax/2025/a.pdf', 'h', 10, 't', 'd', 't'),
                      ('d2', 'c1', 2025, 'Acme (1001)/Income Tax/2025/b.pdf', 'h', 10, 't', 'd', 't');
             UPDATE document SET deleted_at = 't' WHERE id = 'd2';
             INSERT INTO lock (doc_id, user, device_id, since) VALUES ('d1', 'kwk', 'd', 't');",
        )
        .unwrap();
        let s = load(&conn).unwrap();
        assert_eq!(s.clients.len(), 1);
        assert_eq!(s.docs.len(), 1, "tombstoned document left out");
        assert_eq!(s.docs[0].locked_by.as_deref(), Some("kwk"));
        assert_eq!(s.docs[0].status, "received");
    }
}
