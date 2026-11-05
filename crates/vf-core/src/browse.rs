//! Read model for the Browse tab: one load of the live catalog rows, then the client tree and
//! folder filters are built in memory (tree expansion never touches SQLite).
//!
//! Tree shape (PA-23, owner 2026-11-04): Cabinet → client → the client's folders as they are on
//! disk (`Income Tax` → `2025`), so the tree and its breadcrumb read like Explorer. Tax year and
//! doc type stay document attributes (grid columns, views, search), not tree levels.
//!
//! The year view (owner 2026-11-04, a toggle): client → tax year (newest first, then "No tax
//! year") → the folders holding that year's documents, with the year segment taken out
//! (`Income Tax/2025/x.pdf` sits under 2025 as `Income Tax`).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use rusqlite::Connection;

use crate::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientRow {
    pub id: String,
    pub ext_id: String,
    pub name: String,
    pub folder_rel: Option<String>,
    pub entity_type: Option<String>,
    /// The firm's partner for the client (owner 2026-11-05), when set.
    pub partner: Option<String>,
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
    /// Live doc types (id, name) in list order, archived ones too (their documents show them).
    pub doc_types: Vec<(String, String)>,
    /// Ids of the archived doc types: left out of pick lists (owner 2026-11-04).
    pub archived_types: std::collections::HashSet<String>,
    /// Doc type id → its default folder below the client folder (`Income Tax/{Year}`).
    pub doc_type_folders: std::collections::HashMap<String, String>,
    /// A large cabinet's snapshot holds only some documents (PA-34): the counts the tree and
    /// views show, and the page of a cabinet-wide view. `None`: every document is loaded.
    pub windowed: Option<Windowed>,
}

/// Rows of a cabinet-wide view a windowed snapshot holds (the first ones by path).
pub const PAGE: usize = 1000;

/// A cabinet-wide view, for its page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageKind {
    Cabinet,
    Unassigned,
    NeedsDocType,
    NeedsTaxYear,
    /// Filed at or after this RFC 3339 time.
    ReceivedSince(String),
}

/// Which documents a windowed snapshot holds (owner 2026-11-05, Q-67: a cabinet of ~900K
/// documents is never loaded whole).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Window {
    /// Every document of these clients (uuids): the client shown, those open in the tree.
    pub clients: BTreeSet<String>,
    /// A cabinet-wide view's first [`PAGE`] rows (by path), with its total.
    pub page: Option<PageKind>,
    /// The page's name filter (any case, part of the path).
    pub name_filter: String,
    /// These documents (Recent, Pinned, a search's results).
    pub ids: BTreeSet<String>,
    /// "Received this week" counts from this time.
    pub since: String,
}

/// What a windowed snapshot knows beyond its documents.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Windowed {
    /// Documents per client uuid (the tree's counts).
    pub client_counts: HashMap<String, usize>,
    pub unassigned: usize,
    /// The page of [`Window::page`]: its documents' ids in path order, and how many match.
    pub page: Option<(PageKind, Vec<String>, usize)>,
    pub needs_doc_type: usize,
    pub needs_tax_year: usize,
    pub received: usize,
}

impl Snapshot {
    /// The partners clients have, each once (any case), sorted.
    pub fn partners(&self) -> Vec<String> {
        let mut seen = std::collections::BTreeMap::new();
        for p in self.clients.iter().filter_map(|c| c.partner.as_deref()) {
            seen.entry(p.to_lowercase())
                .or_insert_with(|| p.to_string());
        }
        seen.into_values().collect()
    }
}

/// The document columns `DocRow` is read from.
const DOC_SELECT: &str =
    "SELECT d.id, d.client_id, t.name, d.tax_year, d.rel_path, d.hash, d.size, d.status,
        d.source, d.filed_by, d.updated_at, l.user,
        (SELECT count(*) FROM version v WHERE v.doc_id = d.id),
        d.doc_type_id,
        (SELECT group_concat(tag, char(31)) FROM
           (SELECT tag FROM document_tag g WHERE g.doc_id = d.id ORDER BY tag COLLATE NOCASE)),
        d.filed_at
 FROM document d
 LEFT JOIN doc_type t ON t.id = d.doc_type_id
 LEFT JOIN lock l ON l.doc_id = d.id";

fn doc_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<DocRow> {
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
}

/// Documents matching `cond` (SQL over `d`), with `params`.
fn docs_where(
    conn: &Connection,
    cond: &str,
    params: &[&dyn rusqlite::ToSql],
    tail: &str,
) -> Result<Vec<DocRow>> {
    let sql = format!("{DOC_SELECT} WHERE d.deleted_at IS NULL AND ({cond}) {tail}");
    let mut st = conn.prepare_cached(&sql)?;
    let rows = st
        .query_map(params, doc_row)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// The condition and parameters of a cabinet-wide view's page (`?1` = the name filter).
fn page_cond(kind: &PageKind) -> (&'static str, Option<String>) {
    match kind {
        PageKind::Cabinet => ("1", None),
        PageKind::Unassigned => ("d.client_id IS NULL", None),
        PageKind::NeedsDocType => ("d.doc_type_id IS NULL", None),
        PageKind::NeedsTaxYear => (
            "d.tax_year IS NULL AND ('/' || lower(d.rel_path) || '/') NOT LIKE '%/permanent/%'",
            None,
        ),
        PageKind::ReceivedSince(t) => ("d.filed_at >= ?2", Some(t.clone())),
    }
}

/// A large cabinet's snapshot (PA-34): clients, relationships and doc types whole; documents
/// only for `w` (see [`Window`]); counts for the tree and the views from the catalog.
pub fn load_window(conn: &Connection, w: &Window) -> Result<Snapshot> {
    load_window_reusing(conn, w, None)
}

/// [`load_window`], reusing the counts of `counts` when the catalog has not changed since (only
/// the window moved: another client opened, another view).
pub fn load_window_reusing(
    conn: &Connection,
    w: &Window,
    counts: Option<&Windowed>,
) -> Result<Snapshot> {
    let mut s = load_base(conn)?;
    let mut win = Windowed::default();
    let count = |cond: &str, p: &[&dyn rusqlite::ToSql]| -> Result<usize> {
        Ok(conn.query_row(
            &format!("SELECT count(*) FROM document d WHERE d.deleted_at IS NULL AND ({cond})"),
            p,
            |r| r.get::<_, i64>(0),
        )? as usize)
    };
    if let Some(c) = counts {
        win.client_counts = c.client_counts.clone();
        win.unassigned = c.unassigned;
        win.needs_doc_type = c.needs_doc_type;
        win.needs_tax_year = c.needs_tax_year;
        win.received = c.received;
    } else {
        let mut st = conn.prepare_cached(
            "SELECT client_id, count(*) FROM document WHERE deleted_at IS NULL GROUP BY client_id",
        )?;
        let rows = st.query_map([], |r| {
            Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)? as usize))
        })?;
        for r in rows {
            match r? {
                (Some(c), n) => {
                    win.client_counts.insert(c, n);
                }
                (None, n) => win.unassigned = n,
            }
        }
        win.needs_doc_type = count(page_cond(&PageKind::NeedsDocType).0, &[])?;
        win.needs_tax_year = count(page_cond(&PageKind::NeedsTaxYear).0, &[])?;
        win.received = count("d.filed_at >= ?1", &[&w.since])?;
    }

    let mut docs: BTreeMap<String, DocRow> = BTreeMap::new();
    let clients: Vec<&String> = w.clients.iter().collect();
    for chunk in clients.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(",");
        let p: Vec<&dyn rusqlite::ToSql> =
            chunk.iter().map(|c| *c as &dyn rusqlite::ToSql).collect();
        for d in docs_where(conn, &format!("d.client_id IN ({marks})"), &p, "")? {
            docs.insert(d.id.clone(), d);
        }
    }
    let ids: Vec<&String> = w.ids.iter().collect();
    for chunk in ids.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(",");
        let p: Vec<&dyn rusqlite::ToSql> =
            chunk.iter().map(|c| *c as &dyn rusqlite::ToSql).collect();
        for d in docs_where(conn, &format!("d.id IN ({marks})"), &p, "")? {
            docs.insert(d.id.clone(), d);
        }
    }
    if let Some(kind) = &w.page {
        let (cond, extra) = page_cond(kind);
        // `%` and `_` typed in the filter are themselves, not wildcards.
        let typed = w
            .name_filter
            .trim()
            .to_lowercase()
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let needle = format!("%{typed}%");
        let cond = format!("({cond}) AND lower(d.rel_path) LIKE ?1 ESCAPE '\\'");
        let extra = extra.unwrap_or_default();
        let p: [&dyn rusqlite::ToSql; 2] = [&needle, &extra];
        let used: &[&dyn rusqlite::ToSql] = if cond.contains("?2") { &p } else { &p[..1] };
        let total = count(&cond, used)?;
        let rows = docs_where(
            conn,
            &cond,
            used,
            &format!("ORDER BY d.rel_path LIMIT {PAGE}"),
        )?;
        let page_ids = rows.iter().map(|d| d.id.clone()).collect();
        for d in rows {
            docs.insert(d.id.clone(), d);
        }
        win.page = Some((kind.clone(), page_ids, total));
    }
    let mut docs: Vec<DocRow> = docs.into_values().collect();
    docs.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    s.docs = docs;
    s.windowed = Some(win);
    Ok(s)
}

/// How many live documents the catalog holds (a large cabinet's Browse loads windows).
pub fn document_count(conn: &Connection) -> Result<usize> {
    Ok(conn.query_row(
        "SELECT count(*) FROM document WHERE deleted_at IS NULL",
        [],
        |r| r.get::<_, i64>(0),
    )? as usize)
}

pub fn load(conn: &Connection) -> Result<Snapshot> {
    let mut s = load_base(conn)?;
    s.docs = docs_where(conn, "1", &[], "ORDER BY d.rel_path")?;
    Ok(s)
}

/// Everything but the documents.
fn load_base(conn: &Connection) -> Result<Snapshot> {
    let mut st = conn.prepare(
        "SELECT id, ext_id, name, folder_rel, entity_type, partner FROM client
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
                partner: r.get(5)?,
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
        "SELECT id, name, default_folder_tpl, archived FROM doc_type WHERE deleted_at IS NULL
         ORDER BY sort, name",
    )?;
    let rows = st
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, bool>(3)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let doc_type_folders = rows
        .iter()
        .filter_map(|(id, _, f, _)| f.clone().map(|f| (id.clone(), f)))
        .collect();
    let archived_types = rows.iter().filter(|r| r.3).map(|r| r.0.clone()).collect();
    let doc_types = rows.into_iter().map(|(id, n, _, _)| (id, n)).collect();
    Ok(Snapshot {
        clients,
        docs: Vec::new(),
        relationships,
        doc_types,
        archived_types,
        doc_type_folders,
        windowed: None,
    })
}

impl Snapshot {
    /// The doc types to offer in a pick list: the ones not archived, plus `keep` (the current
    /// value) when that one is archived.
    pub fn pickable_doc_types(&self, keep: Option<&str>) -> Vec<(String, String)> {
        self.doc_types
            .iter()
            .filter(|(id, _)| !self.archived_types.contains(id) || Some(id.as_str()) == keep)
            .cloned()
            .collect()
    }
}

/// A tree node the grid can show.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Scope {
    /// Every document in the cabinet.
    Cabinet,
    Client(usize),
    /// A folder inside a client folder (`Income Tax/2025`, `/`-separated) and everything below it.
    Folder {
        client: usize,
        folder: String,
    },
    /// Year view: a client's documents of one tax year (`None` = no tax year).
    Year {
        client: usize,
        year: Option<i32>,
    },
    /// Year view: a year's documents in one folder (the folder without its year segment).
    YearFolder {
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
pub struct ClientNode {
    /// Documents anywhere in the client folder.
    pub count: usize,
    /// Every folder inside the client folder (`Income Tax`, `Income Tax/2025`, …), from
    /// documents and from disk, with the documents in it and below it.
    pub folders: BTreeMap<String, usize>,
    /// Year view: years newest first, then no tax year.
    pub years: Vec<YearNode>,
}

/// A year in the year view.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct YearNode {
    /// `None` = no tax year.
    pub year: Option<i32>,
    pub count: usize,
    /// (folder without its year segment, count); `""` = directly in the client folder.
    pub folders: Vec<(String, usize)>,
}

/// A year view label: the year, or "No tax year".
pub fn year_node_label(y: Option<i32>) -> String {
    y.map_or_else(|| "No tax year".to_string(), |y| y.to_string())
}

/// `folder` without the segment naming `year` (the first one only).
pub fn without_year(folder: &str, year: Option<i32>) -> String {
    let year = year.map(|y| y.to_string());
    let mut dropped = false;
    folder
        .split('/')
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

fn sort_years(ys: &mut [YearNode]) {
    // Years newest first, no tax year last.
    ys.sort_by_key(|y| (y.year.is_none(), std::cmp::Reverse(y.year)));
}

impl ClientNode {
    /// The folders directly inside `parent` (`""` = the client folder), in tree order: by name,
    /// except that tax-year folders come newest first.
    pub fn children(&self, parent: &str) -> Vec<(&str, usize)> {
        let mut v: Vec<(&str, usize)> = self
            .folders
            .iter()
            .filter(|(f, _)| parent_of(f) == parent)
            .map(|(f, n)| (f.as_str(), *n))
            .collect();
        v.sort_by(|a, b| {
            let (na, nb) = (leaf(a.0), leaf(b.0));
            match (year_of(na), year_of(nb)) {
                (Some(x), Some(y)) => y.cmp(&x),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => na.to_lowercase().cmp(&nb.to_lowercase()),
            }
        });
        v
    }

    /// Whether `folder` has folders inside it.
    pub fn has_children(&self, folder: &str) -> bool {
        self.folders.keys().any(|f| parent_of(f) == folder)
    }
}

/// The folder holding `f` (`""` at the top of the client folder).
pub fn parent_of(f: &str) -> &str {
    f.rsplit_once('/').map(|(p, _)| p).unwrap_or("")
}

/// The last segment of a folder path: its name in the tree.
pub fn leaf(f: &str) -> &str {
    f.rsplit('/').next().unwrap_or(f)
}

fn year_of(seg: &str) -> Option<i32> {
    (seg.len() == 4)
        .then(|| seg.parse::<i32>().ok())
        .flatten()
        .filter(|y| (1990..=2100).contains(y))
}

/// `folder` and the folders above it (`a/b/c` → `a`, `a/b`, `a/b/c`).
fn with_ancestors(folder: &str) -> impl Iterator<Item = &str> {
    folder
        .match_indices('/')
        .map(|(i, _)| &folder[..i])
        .chain((!folder.is_empty()).then_some(folder))
}

/// The tree plus each document's place in it. Indices refer to `Snapshot::clients` / `docs`.
#[derive(Debug, Clone, Default)]
pub struct Index {
    pub clients: Vec<ClientNode>,
    pub unassigned: usize,
    /// Per document: (client index, its folder inside the client folder; `""` = the top).
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
        let mut clients = vec![ClientNode::default(); s.clients.len()];
        let mut years: Vec<BTreeMap<Option<i32>, BTreeMap<String, usize>>> =
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
                    let node = &mut clients[ci];
                    node.count += 1;
                    for f in with_ancestors(&folder) {
                        *node.folders.entry(f.to_owned()).or_default() += 1;
                    }
                    *years[ci]
                        .entry(d.tax_year)
                        .or_default()
                        .entry(without_year(&folder, d.tax_year))
                        .or_default() += 1;
                }
                None => unassigned += 1,
            }
            place.push((ci, folder));
        }
        // A windowed snapshot: the counts of every client, loaded or not (PA-34).
        if let Some(w) = &s.windowed {
            for (node, c) in clients.iter_mut().zip(&s.clients) {
                node.count = w.client_counts.get(&c.id).copied().unwrap_or(0);
            }
            unassigned = w.unassigned;
        }
        for (node, ys) in clients.iter_mut().zip(years) {
            node.years = ys
                .into_iter()
                .map(|(year, folders)| YearNode {
                    year,
                    count: folders.values().sum(),
                    folders: folders.into_iter().collect(),
                })
                .collect();
            sort_years(&mut node.years);
        }
        Index {
            clients,
            unassigned,
            place,
        }
    }

    /// Adds client `ci`'s folders that exist on disk (`dirs`: folders relative to the client
    /// folder, `/`-separated), so the template's folders show before anything is filed in them.
    pub fn add_folders(&mut self, ci: usize, dirs: &[String]) {
        let Some(node) = self.clients.get_mut(ci) else {
            return;
        };
        for dir in dirs {
            let dir = dir.trim_matches('/');
            for f in with_ancestors(dir) {
                node.folders.entry(f.to_owned()).or_default();
            }
            // Year view: a folder for a tax year (`Income Tax/2025`) shows under that year
            // before anything is filed in it. Folders with no year segment are not listed
            // under "No tax year": that holds documents without a year, not folders.
            let Some(year) = dir.split('/').find_map(year_of) else {
                continue;
            };
            let folder = without_year(dir, Some(year));
            let y = match node.years.iter().position(|y| y.year == Some(year)) {
                Some(i) => &mut node.years[i],
                None => {
                    node.years.push(YearNode {
                        year: Some(year),
                        ..Default::default()
                    });
                    node.years.last_mut().expect("just pushed")
                }
            };
            if !y.folders.iter().any(|(f, _)| *f == folder) {
                y.folders.push((folder, 0));
                y.folders.sort();
            }
        }
        sort_years(&mut node.years);
    }

    /// Indices into `Snapshot::docs` inside `scope`, in path order.
    pub fn docs_in(&self, s: &Snapshot, scope: &Scope) -> Vec<usize> {
        if let Some(kind) = page_kind(scope)
            && let Some(w) = &s.windowed
        {
            let Some((k, ids, _)) = &w.page else {
                return Vec::new();
            };
            if *k != kind {
                return Vec::new();
            }
            let at: HashMap<&str, usize> = s
                .docs
                .iter()
                .enumerate()
                .map(|(i, d)| (d.id.as_str(), i))
                .collect();
            return ids
                .iter()
                .filter_map(|id| at.get(id.as_str()).copied())
                .collect();
        }
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
                    Scope::Folder { client, folder: f } => {
                        *ci == Some(*client)
                            && (folder == f
                                || folder
                                    .strip_prefix(f.as_str())
                                    .is_some_and(|r| r.starts_with('/')))
                    }
                    Scope::Year { client, year } => *ci == Some(*client) && d.tax_year == *year,
                    Scope::YearFolder {
                        client,
                        year,
                        folder: f,
                    } => {
                        *ci == Some(*client)
                            && d.tax_year == *year
                            && without_year(folder, d.tax_year) == *f
                    }
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

/// The cabinet-wide view a scope is, if it is one (a windowed snapshot pages it).
pub fn page_kind(scope: &Scope) -> Option<PageKind> {
    Some(match scope {
        Scope::Cabinet => PageKind::Cabinet,
        Scope::Unassigned => PageKind::Unassigned,
        Scope::NeedsDocType => PageKind::NeedsDocType,
        Scope::NeedsTaxYear => PageKind::NeedsTaxYear,
        Scope::ReceivedSince(t) => PageKind::ReceivedSince(t.clone()),
        _ => return None,
    })
}

/// The document's folder inside its client folder (`""` = directly in it).
fn folder_key(c: &ClientRow, d: &DocRow) -> String {
    let inner = match c.folder_rel.as_deref() {
        Some(f) if !f.is_empty() => d
            .rel_path
            .strip_prefix(f)
            .and_then(|r| r.strip_prefix('/'))
            .unwrap_or(&d.rel_path),
        _ => &d.rel_path,
    };
    inner
        .rsplit_once('/')
        .map(|(dir, _)| dir)
        .unwrap_or("")
        .split('/')
        .filter(|seg| !seg.is_empty())
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
            partner: None,
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
            archived_types: Default::default(),
            doc_type_folders: Default::default(),
            windowed: None,
        }
    }

    /// Folders on disk show under the client before anything is filed in them, as on disk.
    #[test]
    fn empty_template_folders_join_the_tree() {
        let s = Snapshot {
            clients: vec![client("c1", "Stockton Family Seed")],
            ..Default::default()
        };
        let mut ix = Index::build(&s);
        assert!(ix.clients[0].folders.is_empty());
        ix.add_folders(
            0,
            &[
                "Income Tax/2024".into(),
                "Income Tax/2025".into(),
                "Bookkeeping".into(),
                "Permanent/Setup".into(),
                "Not Yet Filed".into(),
                "Other/Practice".into(),
            ],
        );
        let node = &ix.clients[0];
        let names = |p: &str| -> Vec<String> {
            node.children(p)
                .iter()
                .map(|(f, _)| f.to_string())
                .collect()
        };
        assert_eq!(
            names(""),
            [
                "Bookkeeping",
                "Income Tax",
                "Not Yet Filed",
                "Other",
                "Permanent"
            ]
        );
        // Tax-year folders newest first.
        assert_eq!(names("Income Tax"), ["Income Tax/2025", "Income Tax/2024"]);
        assert_eq!(names("Permanent"), ["Permanent/Setup"]);
        assert!(node.has_children("Other") && !node.has_children("Bookkeeping"));
        assert_eq!(node.count, 0);
        // Adding again changes nothing.
        let before = ix.clients[0].clone();
        ix.add_folders(0, &["Income Tax/2025".into()]);
        assert_eq!(ix.clients[0], before);
    }

    /// A document with no tax year in `Income Tax` sits in `Income Tax`, not under a
    /// "Permanent" heading (owner report 2026-11-04).
    #[test]
    fn tree_is_the_folders_on_disk_with_counts_below() {
        let s = snap();
        let ix = Index::build(&s);
        let acme = &ix.clients[0];
        assert_eq!(acme.count, 6);
        let top: Vec<_> = acme.children("").into_iter().collect();
        assert_eq!(top, [("Income Tax", 3), ("Other", 1), ("Permanent", 1)]);
        assert_eq!(
            acme.children("Income Tax"),
            [("Income Tax/2025", 2), ("Income Tax/2024", 1)]
        );
        assert_eq!(acme.children("Permanent"), [("Permanent/Setup", 1)]);
        assert_eq!(ix.clients[1].count, 1);
        assert_eq!(ix.unassigned, 1);
    }

    /// The year view (owner 2026-11-04 toggle): years newest first, "No tax year" last, the
    /// year segment taken out of the folders; empty year folders on disk show under their year.
    #[test]
    fn year_view_groups_by_tax_year() {
        let s = snap();
        let mut ix = Index::build(&s);
        let acme = &ix.clients[0];
        let years: Vec<_> = acme.years.iter().map(|y| (y.year, y.count)).collect();
        assert_eq!(years, [(Some(2025), 3), (Some(2024), 1), (None, 2)]);
        assert_eq!(
            acme.years[0].folders,
            [
                ("Income Tax".to_string(), 2),
                ("Other/Practice".to_string(), 1)
            ]
        );
        assert_eq!(year_node_label(None), "No tax year");
        let names = |ix: &Index, sc: Scope| -> Vec<String> {
            ix.docs_in(&s, &sc)
                .into_iter()
                .map(|i| s.docs[i].name().to_owned())
                .collect()
        };
        assert_eq!(
            names(
                &ix,
                Scope::YearFolder {
                    client: 0,
                    year: Some(2025),
                    folder: "Income Tax".into()
                }
            ),
            ["b.pdf", "c.pdf"]
        );
        assert_eq!(
            names(
                &ix,
                Scope::Year {
                    client: 0,
                    year: None
                }
            ),
            ["e.pdf", "f.pdf"]
        );
        ix.add_folders(0, &["Payroll/2026".into(), "Bookkeeping".into()]);
        let acme = &ix.clients[0];
        assert_eq!(acme.years[0].year, Some(2026));
        assert_eq!(acme.years[0].folders, [("Payroll".to_string(), 0)]);
        assert_eq!(acme.years.last().unwrap().year, None);
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
        let folder = |f: &str| Scope::Folder {
            client: 0,
            folder: f.into(),
        };
        // A folder shows what is in it and below it.
        assert_eq!(names(folder("Income Tax")), ["a.pdf", "b.pdf", "c.pdf"]);
        assert_eq!(names(folder("Income Tax/2025")), ["b.pdf", "c.pdf"]);
        assert_eq!(names(folder("Other/Practice")), ["d.pdf"]);
        // "Income" is not a prefix match of "Income Tax".
        assert!(names(folder("Income")).is_empty());
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
    fn folder_is_the_path_inside_the_client_folder() {
        let c = client("1", "Acme");
        let d = doc(Some("1"), Some(2025), "Acme (1)/Income Tax/2025/x.pdf");
        assert_eq!(folder_key(&c, &d), "Income Tax/2025");
        let d = doc(Some("1"), None, "Acme (1)/x.pdf");
        assert_eq!(folder_key(&c, &d), "");
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
