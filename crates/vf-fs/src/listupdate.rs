//! Clients updated from a list of names and IDs (PA-94, owner 2026-11-17, Q-131). The firm's
//! list (a CSV with the client's name and its ID, nothing else) is the truth; the cabinet's
//! clients came from folder names and carry old, wrong or no IDs. Each row of the list is
//! matched to a client: by its ID when a client has it already, else by how closely the
//! names agree ([`find`], 97 % unless the person says otherwise). A matched client takes the
//! list's ID and the list's spelling of its name, and its folder is renamed for both
//! ([`apply`], one change per client through `folders::set_client_id` / `rename_client`).
//!
//! Nothing is ticked for the person. A client that has another ID already is marked. A row
//! that matches two clients (the same client made twice: Admin › Similar clients merges
//! them first), and a client two rows match, are listed and never changed.
//!
//! Each change is written to `_Vibe/client-updates/<run>.jsonl` as it is made, so the run
//! can be put back ([`undo`]) from any computer, also after one that was cut short.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};

use filer_core::template;
use serde::{Deserialize, Serialize};
use vf_core::db::Connection;

use crate::folders::{self, ClientIdChange, ClientRename};
use crate::namematch::{self, Name, Tier};
use crate::{cabinet, clients};

/// The name match asked for unless the person changes it.
pub const NAME_MATCH: f64 = 0.97;
/// The folder under `_Vibe` holding each run's record.
pub const DIR: &str = "client-updates";

/// One row of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    pub id: String,
}

/// The list's columns, for the person to say which is which.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Columns {
    pub headers: Vec<String>,
    /// The guess: the column holding the IDs, and the one holding the names.
    pub id: Option<String>,
    pub name: Option<String>,
}

/// The columns of the CSV at `csv` (its first row is their names).
pub fn columns(csv: &Path) -> Result<Columns, String> {
    let table = filer_core::clients::read_csv(csv).map_err(|e| e.to_string())?;
    let by_header = |names: &[&str]| {
        table
            .headers
            .iter()
            .find(|h| names.contains(&h.trim().to_lowercase().as_str()))
            .cloned()
    };
    let mut id = by_header(&[
        "id",
        "client id",
        "client_id",
        "clientid",
        "client number",
        "client no",
        "number",
        "no",
        "code",
        "client code",
    ]);
    let mut name = by_header(&["name", "client name", "client_name", "clientname", "client"]);
    // Two columns and no telling names: the IDs are the shorter values.
    if (id.is_none() || name.is_none()) && table.headers.len() == 2 {
        let len = |col: usize| -> usize {
            table
                .rows
                .iter()
                .filter_map(|r| r.get(col))
                .map(|v| v.trim().chars().count())
                .sum()
        };
        let (short, long) = if len(0) <= len(1) { (0, 1) } else { (1, 0) };
        id = Some(table.headers[short].clone());
        name = Some(table.headers[long].clone());
    }
    Ok(Columns {
        headers: table.headers,
        id,
        name,
    })
}

/// The list's rows with a name and an ID, and how many rows had one of them blank.
pub fn read_rows(csv: &Path, id_col: &str, name_col: &str) -> Result<(Vec<Row>, usize), String> {
    if id_col == name_col {
        return Err("The ID and the name are in two different columns.".into());
    }
    let table = filer_core::clients::read_csv(csv).map_err(|e| e.to_string())?;
    let col = |h: &str| {
        table
            .headers
            .iter()
            .position(|x| x == h)
            .ok_or_else(|| format!("The list has no column {h}."))
    };
    let (i, n) = (col(id_col)?, col(name_col)?);
    let mut blank = 0;
    let mut rows = Vec::new();
    for r in &table.rows {
        let id = r.get(i).map(|v| v.trim()).unwrap_or_default();
        let name = r.get(n).map(|v| v.trim()).unwrap_or_default();
        if id.is_empty() || name.is_empty() {
            blank += 1;
            continue;
        }
        rows.push(Row {
            name: name.to_string(),
            id: id.to_string(),
        });
    }
    // Full taxpayer IDs never become client IDs (they go into folder names and the bucket).
    if clients::values_look_like_tins(rows.iter().map(|r| r.id.as_str())) {
        return Err(format!(
            "The column {id_col} holds taxpayer IDs (SSNs or EINs): it cannot be the client ID."
        ));
    }
    Ok((rows, blank))
}

/// A client of the cabinet, as the catalog has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Who {
    /// Client uuid.
    pub id: String,
    pub ext_id: String,
    pub name: String,
    pub folder: Option<String>,
    pub docs: usize,
}

/// Why a row and a client go together, and what the client has now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// The names match; the client has no ID.
    NoId,
    /// The names match; the client has another ID (marked: look before ticking).
    OtherId,
    /// The client has the row's ID; its name is written another way.
    SameId,
}

/// A row of the list and the one client it matches.
#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub row: Row,
    pub client: Who,
    /// How closely the names agree, 0 to 1, and how sure that is (see `namematch`).
    pub score: f64,
    pub tier: Tier,
    pub kind: Kind,
}

/// The nearest client or folder to a row that has no match: for a person to look at.
#[derive(Debug, Clone, PartialEq)]
pub struct Nearest {
    pub name: String,
    pub id: String,
    pub folder: Option<String>,
    pub score: f64,
    /// A top folder of the cabinet with no client file (not a client yet).
    pub unbound: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Found {
    pub matches: Vec<Match>,
    /// Rows and clients that are not changed, each with why, for the person to read.
    pub unclear: Vec<String>,
    /// Rows of the list read.
    pub rows: usize,
    /// Rows whose client has this ID and this name already.
    pub already: usize,
    /// Clients in the catalog.
    pub clients: usize,
    /// The discrepancies, one way: rows of the list no client matches, each with the
    /// nearest client or folder.
    pub no_client: Vec<(Row, Option<Nearest>)>,
    /// And the other: clients no row of the list is for, each with the nearest row.
    pub no_row: Vec<(Who, Option<(Row, f64)>)>,
    /// Top folders of the cabinet with no client file.
    pub unbound: Vec<String>,
}

fn who_text(w: &Who) -> String {
    if w.ext_id.trim().is_empty() {
        format!("{} (no ID)", w.name)
    } else {
        format!("{} ({})", w.name, w.ext_id)
    }
}

/// Whether match `a` is better than `b`: the surer tier, then the higher score.
fn better(a: (f64, Tier), b: (f64, Tier)) -> bool {
    a.1 < b.1 || (a.1 == b.1 && a.0 > b.0)
}

/// Whether `best` stands clear of `next`: a surer tier, or (two close ones) a score well
/// above. Two exact matches, or two by rule, are a tie.
fn clear_of(best: (f64, Tier), next: (f64, Tier)) -> bool {
    best.1 < next.1 || (best.1 == Tier::Near && best.0 - next.0 >= 0.03)
}

/// Names by the words they can be looked up by.
#[derive(Default)]
struct Index(HashMap<String, Vec<usize>>);

impl Index {
    fn add(&mut self, i: usize, n: &Name) {
        for k in n.lookup_keys() {
            let v = self.0.entry(k).or_default();
            if v.last() != Some(&i) {
                v.push(i);
            }
        }
    }

    /// The entries that can be `n`: those sharing one of its two rarest words. (The same
    /// name shares every word, the rarest too; a name one word off still shares the next
    /// rarest. Looking under every word would bring half the cabinet for "Smith" or
    /// "Farms".)
    fn sharing(&self, n: &Name) -> Vec<usize> {
        let keys = n.lookup_keys();
        let mut lists: Vec<&Vec<usize>> = keys.iter().filter_map(|k| self.0.get(k)).collect();
        lists.sort_by_key(|l| l.len());
        let mut out: Vec<usize> = lists.into_iter().take(2).flatten().copied().collect();
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// The top folders of the cabinet that are no client's (and hold no client's folder).
fn unbound_folders(root: &Path, all: &[Who]) -> Result<Vec<String>, String> {
    let bound: HashSet<String> = all
        .iter()
        .filter_map(|c| c.folder.as_deref())
        .map(|f| f.split('/').next().unwrap_or(f).to_lowercase())
        .collect();
    let mut out: Vec<String> = std::fs::read_dir(root)
        .map_err(|e| format!("The cabinet's folders cannot be listed ({e})."))?
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with(['_', '.', '~']) && !bound.contains(&n.to_lowercase()))
        .collect();
    out.sort_by_key(|n| n.to_lowercase());
    Ok(out)
}

/// Matches the list's `rows` to the catalog's clients, by ID and by name (see `namematch`;
/// the client's name and its folder's name are both tried). `pct`: how closely names must
/// agree for a match that is only close (0 to 1); the same name, however it is written, is
/// a match at any `pct`. `root`: the cabinet, to also name its folders with no client
/// file; `None` leaves those out. Nothing is changed.
pub fn find(
    conn: &Connection,
    root: Option<&Path>,
    rows: &[Row],
    pct: f64,
) -> Result<Found, String> {
    let mut st = conn
        .prepare(
            "SELECT c.id, c.ext_id, c.name, c.folder_rel,
                    (SELECT count(*) FROM document d
                      WHERE d.client_id = c.id AND d.deleted_at IS NULL)
             FROM client c WHERE c.deleted_at IS NULL ORDER BY c.name COLLATE NOCASE, c.ext_id",
        )
        .map_err(|e| e.to_string())?;
    let all: Vec<Who> = st
        .query_map([], |r| {
            Ok(Who {
                id: r.get(0)?,
                ext_id: r.get(1)?,
                name: r.get(2)?,
                folder: r.get::<_, Option<String>>(3)?.filter(|f| !f.is_empty()),
                docs: r.get::<_, i64>(4)? as usize,
            })
        })
        .and_then(|rows| rows.collect())
        .map_err(|e| e.to_string())?;
    let mut out = Found {
        rows: rows.len(),
        clients: all.len(),
        ..Default::default()
    };
    // IDs are compared as Similar clients compares them: `01002`, `1002` and `10-02` are
    // one ID. "Right already" still means written the same.
    let loose = |s: &str| crate::similar::id_key(s);
    // An ID on the list twice names two clients or one twice: neither row is used.
    let mut seen: HashMap<String, usize> = HashMap::new();
    for r in rows {
        *seen.entry(loose(&r.id)).or_default() += 1;
    }
    let mut said: HashSet<String> = HashSet::new();
    let rows: Vec<&Row> = rows
        .iter()
        .filter(|r| {
            let twice = seen[&loose(&r.id)] > 1;
            if twice && said.insert(loose(&r.id)) {
                out.unclear.push(format!(
                    "The ID {} is on the list {} times: none of its rows is used.",
                    r.id.trim(),
                    seen[&loose(&r.id)]
                ));
            }
            !twice
        })
        .collect();
    // A client's name, and its folder's own name (the two can differ: a folder renamed in
    // Explorer, a name changed on the list).
    let cn: Vec<(Name, Option<Name>)> = all
        .iter()
        .map(|c| {
            let folder = c
                .folder
                .as_deref()
                .map(|f| f.rsplit('/').next().unwrap_or(f))
                .map(|f| Name::of(f, Some(&c.ext_id)));
            (Name::of(&c.name, Some(&c.ext_id)), folder)
        })
        .collect();
    let rn: Vec<Name> = rows
        .iter()
        .map(|r| Name::of(&r.name, Some(&r.id)))
        .collect();
    let against = |n: &Name, ci: usize| -> Option<(f64, Tier)> {
        let by_name = namematch::compare(n, &cn[ci].0);
        let by_folder = cn[ci].1.as_ref().and_then(|f| namematch::compare(n, f));
        match (by_name, by_folder) {
            (Some(a), Some(b)) => Some(if better(b, a) { b } else { a }),
            (a, b) => a.or(b),
        }
    };
    // A match: the same name, or close enough for the % asked.
    let is_match = |v: (f64, Tier)| v.1 != Tier::Near || v.0 >= pct;
    let mut clients_by_word = Index::default();
    for (ci, (name, folder)) in cn.iter().enumerate() {
        clients_by_word.add(ci, name);
        if let Some(f) = folder {
            clients_by_word.add(ci, f);
        }
    }
    let mut by_id: HashMap<String, Vec<usize>> = HashMap::new();
    for (ci, c) in all.iter().enumerate() {
        if !c.ext_id.trim().is_empty() {
            by_id.entry(loose(&c.ext_id)).or_default().push(ci);
        }
    }
    // The one client that has a row's ID, with how its name agrees with the row's.
    let holder_of = |ri: usize| -> Option<(usize, Option<(f64, Tier)>)> {
        match by_id.get(&loose(&rows[ri].id)).map(Vec::as_slice) {
            Some([ci]) => Some((*ci, against(&rn[ri], *ci))),
            _ => None,
        }
    };
    // A client that has an ID of the list *and* its name is that row's client, and no other
    // row's. An ID alone is not enough: the cabinet's IDs came from folder names, and an
    // old one can be another client's ID on the list.
    let claimed: HashSet<usize> = (0..rows.len())
        .filter_map(holder_of)
        .filter(|(_, v)| v.is_some_and(is_match))
        .map(|(ci, _)| ci)
        .collect();
    // Every client sharing a word with row `ri` that no row has by its ID, the best first.
    let candidates = |ri: usize| -> Vec<(usize, (f64, Tier))> {
        let mut v: Vec<(usize, (f64, Tier))> = clients_by_word
            .sharing(&rn[ri])
            .into_iter()
            .filter(|ci| !claimed.contains(ci))
            .filter_map(|ci| against(&rn[ri], ci).map(|s| (ci, s)))
            .collect();
        v.sort_by(|x, y| {
            x.1.1
                .cmp(&y.1.1)
                .then(y.1.0.total_cmp(&x.1.0))
                .then(x.0.cmp(&y.0))
        });
        v
    };
    let unbound = match root {
        Some(root) => unbound_folders(root, &all)?,
        None => vec![],
    };
    let un: Vec<Name> = unbound.iter().map(|f| Name::of(f, None)).collect();
    let mut unbound_by_word = Index::default();
    for (i, n) in un.iter().enumerate() {
        unbound_by_word.add(i, n);
    }
    // The nearest thing to a row with no match: a client, or a folder with no client file.
    let nearest = |ri: usize, cands: &[(usize, (f64, Tier))]| -> Option<Nearest> {
        let client = cands.first().map(|(ci, v)| {
            let c = &all[*ci];
            Nearest {
                name: c.name.clone(),
                id: c.ext_id.clone(),
                folder: c.folder.clone(),
                score: v.0,
                unbound: false,
            }
        });
        let folder = unbound_by_word
            .sharing(&rn[ri])
            .into_iter()
            .filter_map(|i| namematch::compare(&rn[ri], &un[i]).map(|v| (i, v)))
            .max_by(|x, y| x.1.0.total_cmp(&y.1.0))
            .map(|(i, v)| Nearest {
                name: unbound[i].clone(),
                id: String::new(),
                folder: Some(unbound[i].clone()),
                score: v.0,
                unbound: true,
            });
        match (client, folder) {
            (Some(c), Some(f)) => Some(if f.score > c.score { f } else { c }),
            (c, f) => c.or(f),
        }
        .filter(|n| n.score >= 0.5)
    };
    let mut touched: HashSet<usize> = HashSet::new();
    // Rows that have their client: not offered as "the nearest row" to another.
    let mut rows_done: HashSet<usize> = HashSet::new();
    // By name: (row, client, how well), one client per row; then one row per client.
    let mut by_name: Vec<(usize, usize, (f64, Tier))> = Vec::new();
    for (ri, r) in rows.iter().enumerate() {
        match holder_of(ri) {
            Some((ci, Some(v))) if is_match(v) => {
                let c = &all[ci];
                touched.insert(ci);
                rows_done.insert(ri);
                // On the client list with this ID but with no folder, while a client of
                // the same name has the folder (the list was imported beside clients made
                // from folders): one client twice. Not "right already".
                let twins: Vec<String> = if c.folder.is_none() {
                    candidates(ri)
                        .iter()
                        .filter(|(x, v)| is_match(*v) && all[*x].folder.is_some())
                        .map(|(x, _)| {
                            touched.insert(*x);
                            who_text(&all[*x])
                        })
                        .collect()
                } else {
                    vec![]
                };
                if !twins.is_empty() {
                    out.unclear.push(format!(
                        "{} ({}) is a client with no folder, and {} has a folder under that name: they look like one client. Merge them in Admin › Similar clients (keep {}), then find again.",
                        r.name,
                        r.id,
                        twins.join("; "),
                        r.id
                    ));
                } else if c.name.trim() == r.name.trim() && c.ext_id.trim() == r.id.trim() {
                    out.already += 1;
                } else {
                    out.matches.push(Match {
                        row: (*r).clone(),
                        client: c.clone(),
                        score: v.0,
                        tier: v.1,
                        kind: Kind::SameId,
                    });
                }
                continue;
            }
            // The ID is a client's whose name is another: an old ID that happens to be
            // this row's. Neither is changed (the ID cannot be given twice).
            Some((ci, _)) => {
                out.unclear.push(format!(
                    "The ID {} ({} on the list) is {}'s in the cabinet, and the names do not match: nothing is changed for it. If the list is right, that client gets its own ID first (its row may be in the matches); then find again.",
                    r.id,
                    r.name,
                    who_text(&all[ci])
                ));
                continue;
            }
            None => {}
        }
        // The ID written (loosely) on two or more clients: which one is not for this page.
        if let Some(many) = by_id.get(&loose(&r.id)) {
            for ci in many {
                touched.insert(*ci);
            }
            out.unclear.push(format!(
                "The ID {} ({} on the list) is on {} clients in the cabinet: {}. Merge them in Admin › Similar clients first, then find again.",
                r.id,
                r.name,
                many.len(),
                many.iter()
                    .map(|ci| who_text(&all[*ci]))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
            continue;
        }
        let cands = candidates(ri);
        let hits: Vec<&(usize, (f64, Tier))> = cands.iter().filter(|(_, v)| is_match(*v)).collect();
        match hits.as_slice() {
            [] => out.no_client.push(((*r).clone(), nearest(ri, &cands))),
            [one] => by_name.push((ri, one.0, one.1)),
            [best, next, ..] if clear_of(best.1, next.1) => by_name.push((ri, best.0, best.1)),
            [best, ..] => {
                // As good as each other: the same client made twice, or two of one name.
                let tied: Vec<String> = hits
                    .iter()
                    .filter(|(_, v)| !clear_of(best.1, *v))
                    .map(|(ci, _)| {
                        touched.insert(*ci);
                        who_text(&all[*ci])
                    })
                    .collect();
                out.unclear.push(format!(
                    "{} ({}) matches {} clients: {}. If they are one client, merge them in Admin › Similar clients first, then find again.",
                    r.name,
                    r.id,
                    tied.len(),
                    tied.join("; ")
                ));
            }
        }
    }
    type Scored = (usize, (f64, Tier));
    let mut rows_of: HashMap<usize, Vec<Scored>> = HashMap::new();
    for (ri, ci, v) in &by_name {
        rows_of.entry(*ci).or_default().push((*ri, *v));
    }
    let mut clients_in_order: Vec<usize> = rows_of.keys().copied().collect();
    clients_in_order.sort_unstable();
    for ci in clients_in_order {
        let mut its = rows_of.remove(&ci).expect("is there");
        its.sort_by(|x, y| {
            x.1.1
                .cmp(&y.1.1)
                .then(y.1.0.total_cmp(&x.1.0))
                .then(x.0.cmp(&y.0))
        });
        let c = &all[ci];
        touched.insert(ci);
        if its.len() > 1 && !clear_of(its[0].1, its[1].1) {
            out.unclear.push(format!(
                "{} is matched by {} rows of the list: {}. It is not changed.",
                who_text(c),
                its.len(),
                its.iter()
                    .map(|(i, _)| format!("{} ({})", rows[*i].name, rows[*i].id))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
            continue;
        }
        let (ri, v) = its[0];
        rows_done.insert(ri);
        out.matches.push(Match {
            row: rows[ri].clone(),
            client: c.clone(),
            score: v.0,
            tier: v.1,
            kind: if c.ext_id.trim().is_empty() {
                Kind::NoId
            } else {
                Kind::OtherId
            },
        });
        // The rows a better one took the client from have no client.
        for (i, v) in &its[1..] {
            out.no_client.push((
                rows[*i].clone(),
                Some(Nearest {
                    name: c.name.clone(),
                    id: c.ext_id.clone(),
                    folder: c.folder.clone(),
                    score: v.0.min(namematch::NEAR_MAX),
                    unbound: false,
                }),
            ));
        }
    }
    // The clients no row is for, each with the nearest row of the list.
    let mut rows_by_word = Index::default();
    for (ri, n) in rn.iter().enumerate() {
        rows_by_word.add(ri, n);
    }
    for (ci, c) in all.iter().enumerate() {
        if touched.contains(&ci) {
            continue;
        }
        let mut near: Vec<usize> = rows_by_word.sharing(&cn[ci].0);
        if let Some(f) = &cn[ci].1 {
            near.extend(rows_by_word.sharing(f));
        }
        let best = near
            .into_iter()
            .filter(|ri| !rows_done.contains(ri))
            .filter_map(|ri| against(&rn[ri], ci).map(|v| (ri, v.0)))
            .max_by(|x, y| x.1.total_cmp(&y.1).then(y.0.cmp(&x.0)))
            .filter(|(_, s)| *s >= 0.5)
            .map(|(ri, s)| (rows[ri].clone(), s));
        out.no_row.push((c.clone(), best));
    }
    out.unbound = unbound;
    out.matches.sort_by(|x, y| {
        x.tier
            .cmp(&y.tier)
            .then(x.kind.cmp(&y.kind))
            .then(y.score.total_cmp(&x.score))
            .then_with(|| x.row.name.to_lowercase().cmp(&y.row.name.to_lowercase()))
    });
    out.no_client.sort_by_key(|(r, _)| r.name.to_lowercase());
    out.unclear.sort();
    Ok(out)
}

fn csv_cell(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// The whole comparison as a CSV for a spreadsheet: every match, every row with no client,
/// every client with no row, every folder with no client file, and what is not changed.
pub fn report_csv(f: &Found) -> String {
    let mut out = String::from(
        "Status,List name,List ID,Client or folder,Client ID,Folder,Documents,Match %,Note\r\n",
    );
    let pct = |s: f64| format!("{:.0}", (s * 100.0).floor());
    let mut line = |cells: [&str; 9]| {
        out.push_str(&cells.map(csv_cell).join(","));
        out.push_str("\r\n");
    };
    for m in &f.matches {
        let status = match m.tier {
            Tier::Exact => "Match: same name",
            Tier::Rule => "Match: same name, written differently",
            Tier::Near => "Close: look",
        };
        let note = match m.kind {
            Kind::NoId => "the client has no ID",
            Kind::OtherId => "the client has another ID",
            Kind::SameId => "the client has this ID",
        };
        line([
            status,
            &m.row.name,
            &m.row.id,
            &m.client.name,
            &m.client.ext_id,
            m.client.folder.as_deref().unwrap_or(""),
            &m.client.docs.to_string(),
            &pct(m.score),
            note,
        ]);
    }
    for (r, n) in &f.no_client {
        match n {
            Some(n) => line([
                "On the list, no client",
                &r.name,
                &r.id,
                &n.name,
                &n.id,
                n.folder.as_deref().unwrap_or(""),
                "",
                &pct(n.score),
                if n.unbound {
                    "nearest: a folder with no client file (Admin › Folder import binds it)"
                } else {
                    "nearest client, not a match"
                },
            ]),
            None => line([
                "On the list, no client",
                &r.name,
                &r.id,
                "",
                "",
                "",
                "",
                "",
                "",
            ]),
        }
    }
    for (c, n) in &f.no_row {
        let (rn, ri, s, note) = match n {
            Some((r, s)) => (
                r.name.as_str(),
                r.id.as_str(),
                pct(*s),
                "nearest row, not a match",
            ),
            None => ("", "", String::new(), ""),
        };
        line([
            "Client, not on the list",
            rn,
            ri,
            &c.name,
            &c.ext_id,
            c.folder.as_deref().unwrap_or(""),
            &c.docs.to_string(),
            &s,
            note,
        ]);
    }
    for u in &f.unbound {
        line(["Folder with no client file", "", "", u, "", u, "", "", ""]);
    }
    for u in &f.unclear {
        line(["Not changed", "", "", "", "", "", "", "", u]);
    }
    out
}

/// One ticked match: client `client` (uuid) takes the list's `id` and `name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub client: String,
    pub id: String,
    pub name: String,
}

/// What a change would do, for the yes or no.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub old_id: String,
    pub old_name: String,
    /// The client folder: (now, after). `None`: no folder, or its name stays.
    pub folder: Option<(String, String)>,
}

/// A change and what it would do, or why it cannot be made.
pub type Previewed = (Change, Result<Planned, String>);

/// What each of `changes` would do, or why it cannot be made. Nothing is changed. (Worked
/// out from the client list and the template read once: [`apply`] plans each change again,
/// under the lock, when it makes it.)
pub fn preview(
    conn: &Connection,
    root: &Path,
    changes: &[Change],
) -> Result<Vec<Previewed>, String> {
    let (tree, _, err) = template::load_or_seed(&clients::master_dir(root));
    if let Some(e) = err {
        return Err(e);
    }
    let master = clients::load(root)?;
    let extras: HashMap<String, _> = master
        .clients()
        .iter()
        .map(|c| (c.uuid.to_string(), c.extra.clone()))
        .collect();
    let mut st = conn
        .prepare("SELECT id, ext_id, name, folder_rel FROM client WHERE deleted_at IS NULL")
        .map_err(|e| e.to_string())?;
    type Row4 = (String, String, String, Option<String>);
    let all: Vec<Row4> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .and_then(|rows| rows.collect())
        .map_err(|e| e.to_string())?;
    let by_uuid: HashMap<&str, &Row4> = all.iter().map(|r| (r.0.as_str(), r)).collect();
    let holder: HashMap<String, &Row4> = all
        .iter()
        .filter(|r| !r.1.trim().is_empty())
        .map(|r| (r.1.trim().to_uppercase(), r))
        .collect();
    let mut wanted: HashSet<String> = HashSet::new();
    let one = |c: &Change, wanted: &mut HashSet<String>| -> Result<Planned, String> {
        let Some((uuid, old_id, old_name, folder_rel)) = by_uuid.get(c.client.as_str()) else {
            return Err("That client is not in the catalog any more.".into());
        };
        if let Some(other) = holder.get(&c.id.trim().to_uppercase())
            && other.0 != *uuid
        {
            return Err(format!("{} is {}'s ID already.", c.id, other.2));
        }
        let mut folder = None;
        if let Some(cur) = folder_rel.as_ref().filter(|f| !f.is_empty()) {
            let client = filer_core::clients::Client {
                id: c.id.trim().to_string(),
                uuid: uuid::Uuid::parse_str(uuid).map_err(|e| e.to_string())?,
                name: c.name.trim().to_string(),
                extra: extras.get(uuid.as_str()).cloned().unwrap_or_default(),
            };
            let want = template::client_folder_name(&tree, &client).map_err(|e| e.to_string())?;
            let (parent, cur_name) = match cur.rsplit_once('/') {
                Some((p, n)) => (format!("{p}/"), n),
                None => (String::new(), cur.as_str()),
            };
            if want != cur_name {
                let new = format!("{parent}{want}");
                if want.eq_ignore_ascii_case(cur_name) {
                    return Err(format!(
                        "The folder would be {want}: only the capital letters differ, which Windows sees as the same name."
                    ));
                }
                if root.join(&new).exists() || !wanted.insert(new.to_lowercase()) {
                    return Err(format!("A folder named {want} is already in the cabinet."));
                }
                folder = Some((cur.clone(), new));
            }
        }
        Ok(Planned {
            old_id: old_id.clone(),
            old_name: old_name.clone(),
            folder,
        })
    };
    Ok(changes
        .iter()
        .map(|c| (c.clone(), one(c, &mut wanted)))
        .collect())
}

/// One change as it was made: what to put back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Done {
    /// Client uuid.
    pub client: String,
    pub old_id: String,
    pub old_name: String,
    pub old_folder: Option<String>,
    pub new_id: String,
    pub new_name: String,
    pub new_folder: Option<String>,
}

/// A line of a run's record.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Line {
    Done(Done),
    /// This client (uuid) was put back.
    Back(String),
}

/// A run: its record's file, and the changes of it that have not been put back, in the
/// order they were made.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Run {
    pub file: PathBuf,
    pub done: Vec<Done>,
}

fn dir(root: &Path) -> PathBuf {
    root.join(cabinet::VIBE_DIR).join(DIR)
}

fn record(file: &Path, line: &Line) -> Result<(), String> {
    let text = format!("{}\n", serde_json::to_string(line).expect("serializes"));
    cabinet::append_text(file, &text).map_err(|e| e.to_string())
}

/// The last run that still has changes to put back. `None`: there is none.
pub fn load_last(root: &Path) -> Result<Option<Run>, String> {
    let mut files: Vec<PathBuf> = match std::fs::read_dir(dir(root)) {
        Ok(d) => d
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
            .collect(),
        Err(e) if cabinet::is_absent(&dir(root), &e) => return Ok(None),
        Err(e) => {
            return Err(format!(
                "The record of the last update cannot be read ({e})."
            ));
        }
    };
    // Named by their time: the newest last.
    files.sort();
    let Some(file) = files.pop() else {
        return Ok(None);
    };
    let text = std::fs::read_to_string(&file)
        .map_err(|e| format!("The record of the last update cannot be read ({e})."))?;
    let mut done: Vec<Done> = Vec::new();
    for l in text.lines().filter(|l| !l.trim().is_empty()) {
        // A line cut short (the computer stopped as it was written) names no change.
        match serde_json::from_str::<Line>(l) {
            Ok(Line::Done(d)) => done.push(d),
            Ok(Line::Back(id)) => done.retain(|d| d.client != id),
            Err(_) => {}
        }
    }
    Ok((!done.is_empty()).then_some(Run { file, done }))
}

/// What became of one change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Made; `Some`: a note for the person (the client CSV could not follow).
    Changed(Option<String>),
    /// Not made, and why. The client is as it was.
    Refused(String),
}

fn client_row(conn: &Connection, uuid: &str) -> Result<(String, String, Option<String>), String> {
    conn.query_row(
        "SELECT ext_id, name, folder_rel FROM client WHERE id = ?1 AND deleted_at IS NULL",
        [uuid],
        |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get::<_, Option<String>>(2)?.filter(|f| !f.is_empty()),
            ))
        },
    )
    .map_err(|e| vf_core::db::missing_or(e, "That client is not in the catalog any more."))
}

/// Makes `changes` one client after another, each written to the run's record as it is
/// made. A change that is refused is said and the run goes on; `stop` ends it after the
/// client in hand. `done`: how many were tried (for the page).
pub fn apply(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    changes: &[Change],
    done: &AtomicUsize,
    stop: &AtomicBool,
) -> (Run, Vec<(Change, Outcome)>) {
    let mut run = Run {
        file: dir(root).join(format!("{}.jsonl", vf_core::new_id())),
        done: Vec::new(),
    };
    let mut out = Vec::new();
    for c in changes {
        if stop.load(Relaxed) {
            break;
        }
        let r = (|| -> Result<(Done, Option<String>), String> {
            let (old_id, old_name, old_folder) = client_row(conn, &c.client)?;
            let (new_folder, note) = if old_id == c.id.trim() {
                let plan = folders::plan_client_rename(conn, root, &c.client, &c.name)?;
                folders::rename_client(conn, root, &plan, device_id)?;
                (plan.folder.map(|(_, n)| n).or(old_folder.clone()), None)
            } else {
                let plan =
                    folders::plan_client_id_named(conn, root, &c.client, &c.id, Some(&c.name))?;
                let note = folders::set_client_id(conn, root, &plan, device_id)?;
                (plan.folder.map(|(_, n)| n).or(old_folder.clone()), note)
            };
            Ok((
                Done {
                    client: c.client.clone(),
                    old_id,
                    old_name,
                    old_folder,
                    new_id: c.id.trim().to_string(),
                    new_name: c.name.trim().to_string(),
                    new_folder,
                },
                note,
            ))
        })();
        done.fetch_add(1, Relaxed);
        match r {
            Ok((d, note)) => {
                // On record before the next: a change that is not cannot be put back from
                // the page, so the run ends there and says so.
                let recorded = record(&run.file, &Line::Done(d.clone()));
                run.done.push(d);
                match recorded {
                    Ok(()) => out.push((c.clone(), Outcome::Changed(note))),
                    Err(e) => {
                        run.done.pop();
                        out.push((
                            c.clone(),
                            Outcome::Changed(Some(format!(
                                "This change was made, but its record could not be written ({e}): Undo will not put it back. The run stopped here."
                            ))),
                        ));
                        break;
                    }
                }
            }
            Err(e) => out.push((c.clone(), Outcome::Refused(e))),
        }
    }
    (run, out)
}

/// Puts a run back, the last change first: each client gets the ID, name and folder it had.
/// Stops at the first that cannot be put back (it and the ones before it stay changed).
/// The client CSV is left as it is. Returns, per client tried, its name and what happened.
pub fn undo(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    run: &Run,
    done: &AtomicUsize,
) -> Vec<(String, Result<(), String>)> {
    let mut out = Vec::new();
    for d in run.done.iter().rev() {
        let r = (|| -> Result<(), String> {
            let (ext_id, name, folder) = client_row(conn, &d.client)?;
            if ext_id != d.new_id || name != d.new_name || folder != d.new_folder {
                return Err(
                    "It was changed again since (its ID, name or folder): not put back.".into(),
                );
            }
            let folder_back = match (&d.new_folder, &d.old_folder) {
                (Some(n), Some(o)) if n != o => {
                    if root.join(o).exists() {
                        return Err(format!("A folder named {o} is in the cabinet again."));
                    }
                    Some((n.clone(), o.clone()))
                }
                _ => None,
            };
            if d.old_id == d.new_id {
                let plan = ClientRename {
                    client_id: d.client.clone(),
                    ext_id: d.old_id.clone(),
                    old_name: d.new_name.clone(),
                    new_name: d.old_name.clone(),
                    folder: folder_back,
                };
                folders::rename_client(conn, root, &plan, device_id)
            } else {
                // Another client may have been given the old ID meanwhile.
                let taken: Option<String> = vf_core::db::row_or_none(conn.query_row(
                    "SELECT name FROM client WHERE ext_id = ?1 COLLATE NOCASE AND id <> ?2
                       AND deleted_at IS NULL AND ?1 <> ''",
                    [&d.old_id, &d.client],
                    |r| r.get(0),
                ))?;
                if let Some(n) = taken {
                    return Err(format!("{} is {n}'s ID now.", d.old_id));
                }
                let plan = ClientIdChange {
                    client_id: d.client.clone(),
                    old_id: d.new_id.clone(),
                    new_id: d.old_id.clone(),
                    name: d.new_name.clone(),
                    folder: folder_back,
                    folder_now: d.new_folder.clone(),
                    new_name: Some(d.old_name.clone()),
                    leave_csv: true,
                };
                folders::set_client_id(conn, root, &plan, device_id).map(|_| ())
            }
        })();
        done.fetch_add(1, Relaxed);
        let failed = r.is_err();
        if !failed {
            // The change is put back whether or not that can be written; a record that
            // could not be would offer it again, and the check above then refuses it.
            let _ = record(&run.file, &Line::Back(d.client.clone()));
        }
        out.push((format!("{} ({})", d.new_name, d.new_id), r));
        if failed {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use vf_core::db::Storage;

    use super::*;
    use crate::cabinet::CabinetFile;
    use crate::newclient::{self, NewClient};

    struct Cab {
        _t: tempfile::TempDir,
        root: PathBuf,
        conn: Connection,
    }

    fn cab() -> Cab {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("cabinet");
        cabinet::create(
            &root,
            &CabinetFile {
                cabinet_id: vf_core::new_id(),
                firm_id: uuid::Uuid::new_v4().to_string(),
                schema_version: vf_core::db::SCHEMA_VERSION,
                layout_version: 1,
                created: vf_core::now_rfc3339(),
                name: None,
            },
        )
        .unwrap();
        let conn = vf_core::db::open(&t.path().join("c.db"), Storage::Local).unwrap();
        Cab { _t: t, root, conn }
    }

    /// An invented client with a folder and one document; returns its uuid.
    fn client(c: &mut Cab, ext: &str, name: &str) -> String {
        let p = newclient::plan(
            &c.conn,
            &c.root,
            &NewClient {
                ext_id: ext.into(),
                name: name.into(),
                entity_type: None,
                partner: String::new(),
            },
        )
        .unwrap();
        newclient::create(&mut c.conn, &c.root, &p, "dev").unwrap();
        let file = c.root.join(&p.folder_rel).join("Correspondence/letter.pdf");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, "invented").unwrap();
        crate::rebuild::rebuild(&mut c.conn, &c.root, "dev").unwrap();
        p.client.uuid.to_string()
    }

    fn row(name: &str, id: &str) -> Row {
        Row {
            name: name.into(),
            id: id.into(),
        }
    }

    fn folder_of(c: &Cab, uuid: &str) -> Option<String> {
        client_row(&c.conn, uuid).unwrap().2
    }

    #[test]
    fn the_lists_columns_are_guessed_and_its_rows_read() {
        let t = tempfile::tempdir().unwrap();
        let csv = t.path().join("clients.csv");
        std::fs::write(
            &csv,
            "Client Name,Client ID\nAcme Holdings LLC,ACME1002\n,BLANK1\nZeta Farms,ZETA1009\n",
        )
        .unwrap();
        let cols = columns(&csv).unwrap();
        assert_eq!(cols.id.as_deref(), Some("Client ID"));
        assert_eq!(cols.name.as_deref(), Some("Client Name"));
        let (rows, blank) = read_rows(&csv, "Client ID", "Client Name").unwrap();
        assert_eq!(
            rows,
            [
                row("Acme Holdings LLC", "ACME1002"),
                row("Zeta Farms", "ZETA1009")
            ]
        );
        assert_eq!(blank, 1);
        // Headers that tell nothing: the shorter values are the IDs.
        std::fs::write(&csv, "A,B\n1002,Acme Holdings LLC\n1009,Zeta Farms\n").unwrap();
        let cols = columns(&csv).unwrap();
        assert_eq!(
            (cols.id.as_deref(), cols.name.as_deref()),
            (Some("A"), Some("B"))
        );
        // Taxpayer IDs are never client IDs.
        std::fs::write(&csv, "Name,ID\nAcme,12-3456789\nZeta,98-7654321\n").unwrap();
        assert!(
            read_rows(&csv, "ID", "Name")
                .unwrap_err()
                .contains("taxpayer")
        );
    }

    #[test]
    fn rows_are_matched_by_id_then_by_name() {
        let mut c = cab();
        let acme = client(&mut c, "OLD1", "Acme Holdings LLC");
        let zeta = client(&mut c, "ZETA1009", "Zeta Farms Inc");
        let _right = client(&mut c, "RIGHT1", "Right Already LLC");
        let _twin_a = client(&mut c, "T1", "Cole Family Trust");
        let _twin_b = client(&mut c, "T2", "Cole Family Trust.");
        let _other = client(&mut c, "NOPE9", "Nothing Like It");
        let rows = [
            row("Acme Holdings, LLC", "ACME1002"), // by name: the client has another ID
            row("Zeta Farms, Inc.", "ZETA1009"),   // by ID: the name is written another way
            row("Right Already LLC", "RIGHT1"),    // as it is
            row("Cole Family Trust", "COLE7"),     // two clients: listed, not changed
            row("Brand New Client", "NEW1"),       // no client
            row("Twice One", "DUP5"),
            row("Twice Two", "dup5"),
        ];
        let f = find(&c.conn, None, &rows, NAME_MATCH).unwrap();
        assert_eq!(
            (f.rows, f.already, f.no_client.len(), f.clients),
            (7, 1, 1, 6)
        );
        let got: Vec<(&str, &str, Kind)> = f
            .matches
            .iter()
            .map(|m| (m.row.id.as_str(), m.client.id.as_str(), m.kind))
            .collect();
        assert_eq!(
            got,
            [
                ("ACME1002", acme.as_str(), Kind::OtherId),
                ("ZETA1009", zeta.as_str(), Kind::SameId),
            ]
        );
        assert!(f.matches[0].score >= NAME_MATCH);
        assert_eq!(f.unclear.len(), 2, "{:?}", f.unclear);
        assert!(f.unclear.iter().any(|u| u.contains("matches 2 clients")));
        assert!(
            f.unclear
                .iter()
                .any(|u| u.contains("DUP5") && u.contains("2 times"))
        );
        // A client with no ID is said so.
        c.conn
            .execute("UPDATE client SET ext_id = '' WHERE id = ?1", [&acme])
            .unwrap();
        let f = find(&c.conn, None, &rows, NAME_MATCH).unwrap();
        assert_eq!(f.matches[0].kind, Kind::NoId);
        // A client two rows match is not changed.
        let rows = [
            row("Acme Holdings LLC", "A1"),
            row("Acme Holdings, LLC", "A2"),
        ];
        let f = find(&c.conn, None, &rows, NAME_MATCH).unwrap();
        assert!(f.matches.is_empty());
        assert!(
            f.unclear[0].contains("matched by 2 rows"),
            "{:?}",
            f.unclear
        );
        // An old ID that is another client's ID on the list: the ID alone is no match. The
        // client with it is still found by its own row, by name.
        let rows = [
            row("Somebody Quite Different", "NOPE9"),
            row("Nothing Like It", "REAL7"),
        ];
        let f = find(&c.conn, None, &rows, NAME_MATCH).unwrap();
        assert_eq!(f.matches.len(), 1, "{f:?}");
        assert_eq!(
            (f.matches[0].row.id.as_str(), f.matches[0].kind),
            ("REAL7", Kind::OtherId)
        );
        assert!(
            f.unclear
                .iter()
                .any(|u| u.contains("NOPE9") && u.contains("do not match")),
            "{:?}",
            f.unclear
        );
        // The list's ID on a client with no folder, beside one of that name with the
        // folder: one client twice, said; not "right already".
        c.conn
            .execute(
                "INSERT INTO client (id, ext_id, name, device_id, updated_at)
                 VALUES ('listed-only', 'ZF9', 'Zeta Farms Inc', 'dev', 't')",
                [],
            )
            .unwrap();
        c.conn
            .execute("UPDATE client SET ext_id = 'OLDZ' WHERE id = ?1", [&zeta])
            .unwrap();
        let f = find(&c.conn, None, &[row("Zeta Farms Inc", "ZF9")], NAME_MATCH).unwrap();
        assert_eq!((f.already, f.matches.len()), (0, 0), "{f:?}");
        assert!(f.unclear[0].contains("no folder"), "{:?}", f.unclear);
        c.conn
            .execute("DELETE FROM client WHERE id = 'listed-only'", [])
            .unwrap();
        // A lower % finds more.
        let rows = [row("Acme Holding", "A1")];
        assert!(find(&c.conn, None, &rows, 0.99).unwrap().matches.is_empty());
        assert_eq!(find(&c.conn, None, &rows, 0.90).unwrap().matches.len(), 1);
    }

    #[test]
    fn clients_take_the_lists_id_and_name_and_are_put_back() {
        let mut c = cab();
        let acme = client(&mut c, "OLD1", "Acme Holdings LLC");
        let zeta = client(&mut c, "ZETA1009", "Zeta Farms Inc");
        let held = client(&mut c, "HELD1", "Somebody Else");
        let changes = [
            Change {
                client: acme.clone(),
                id: "ACME1002".into(),
                name: "Acme Holdings, LLC".into(),
            },
            Change {
                client: zeta.clone(),
                id: "ZETA1009".into(),
                name: "Zeta Farms, Inc.".into(),
            },
            // An ID another client has: refused, and the run goes on.
            Change {
                client: held.clone(),
                id: "ZETA1009".into(),
                name: "Somebody Else".into(),
            },
        ];
        let pv = preview(&c.conn, &c.root, &changes).unwrap();
        assert_eq!(
            pv[0].1.as_ref().unwrap().folder,
            Some((
                "Acme Holdings LLC (OLD1)".to_string(),
                "Acme Holdings, LLC (ACME1002)".to_string()
            ))
        );
        assert!(pv[2].1.as_ref().unwrap_err().contains("ID already"));
        assert!(
            c.root.join("Acme Holdings LLC (OLD1)").is_dir(),
            "nothing changed"
        );

        let (done, stop) = (AtomicUsize::new(0), AtomicBool::new(false));
        let (run, out) = apply(&mut c.conn, &c.root, "dev", &changes, &done, &stop);
        assert_eq!(done.load(Relaxed), 3);
        assert!(matches!(out[0].1, Outcome::Changed(_)), "{:?}", out[0].1);
        assert!(matches!(out[1].1, Outcome::Changed(_)), "{:?}", out[1].1);
        assert!(matches!(out[2].1, Outcome::Refused(_)), "{:?}", out[2].1);
        assert_eq!(run.done.len(), 2);
        let (id, name, folder) = client_row(&c.conn, &acme).unwrap();
        assert_eq!(
            (id.as_str(), name.as_str(), folder.as_deref()),
            (
                "ACME1002",
                "Acme Holdings, LLC",
                Some("Acme Holdings, LLC (ACME1002)")
            )
        );
        assert!(
            c.root
                .join("Acme Holdings, LLC (ACME1002)/Correspondence/letter.pdf")
                .is_file()
        );
        assert!(!c.root.join("Acme Holdings LLC (OLD1)").exists());
        assert_eq!(
            folder_of(&c, &zeta).as_deref(),
            Some("Zeta Farms, Inc. (ZETA1009)")
        );
        // The client list follows.
        let listed = clients::load(&c.root).unwrap();
        let l = listed
            .clients()
            .iter()
            .find(|x| x.uuid.to_string() == acme)
            .unwrap();
        assert_eq!(
            (l.id.as_str(), l.name.as_str()),
            ("ACME1002", "Acme Holdings, LLC")
        );
        // And its document's path in the catalog.
        let path: String = c
            .conn
            .query_row(
                "SELECT rel_path FROM document WHERE client_id = ?1 AND deleted_at IS NULL",
                [&acme],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            path,
            "Acme Holdings, LLC (ACME1002)/Correspondence/letter.pdf"
        );

        // On record, from any computer; and put back from it.
        let last = load_last(&c.root).unwrap().unwrap();
        assert_eq!(last, run);
        let back = undo(&mut c.conn, &c.root, "dev", &last, &AtomicUsize::new(0));
        assert!(back.iter().all(|(_, r)| r.is_ok()), "{back:?}");
        let (id, name, folder) = client_row(&c.conn, &acme).unwrap();
        assert_eq!(
            (id.as_str(), name.as_str(), folder.as_deref()),
            (
                "OLD1",
                "Acme Holdings LLC",
                Some("Acme Holdings LLC (OLD1)")
            )
        );
        assert!(
            c.root
                .join("Acme Holdings LLC (OLD1)/Correspondence/letter.pdf")
                .is_file()
        );
        assert_eq!(
            folder_of(&c, &zeta).as_deref(),
            Some("Zeta Farms Inc (ZETA1009)")
        );
        assert!(
            load_last(&c.root).unwrap().is_none(),
            "nothing left to put back"
        );
    }

    /// A client with no ID (made from a folder's name) takes the list's ID and is put back
    /// to having none; a row of the client CSV with a blank ID is not taken for its row.
    #[test]
    fn a_client_with_no_id_takes_one_and_is_put_back() {
        let mut c = cab();
        let acme = client(&mut c, "TMP1", "Acme Holdings LLC");
        // Without an ID, in a folder named by its name alone, as a folder import leaves it.
        let blank = ClientIdChange {
            client_id: acme.clone(),
            old_id: "TMP1".into(),
            new_id: String::new(),
            name: "Acme Holdings LLC".into(),
            folder: Some((
                "Acme Holdings LLC (TMP1)".into(),
                "Acme Holdings LLC".into(),
            )),
            folder_now: Some("Acme Holdings LLC (TMP1)".into()),
            new_name: None,
            leave_csv: true,
        };
        folders::set_client_id(&mut c.conn, &c.root, &blank, "dev").unwrap();
        assert_eq!(
            client_row(&c.conn, &acme).unwrap(),
            (
                String::new(),
                "Acme Holdings LLC".to_string(),
                Some("Acme Holdings LLC".to_string())
            )
        );
        // The client CSV Intake uses has a row with a blank ID of its own.
        let csv = c.root.parent().unwrap().join("intake.csv");
        std::fs::write(
            &csv,
            "id,name\r\n,Somebody Unnumbered\r\nX1,Other Client\r\n",
        )
        .unwrap();
        filer_core::config::Settings {
            root: Some(c.root.clone()),
            csv_path: Some(csv.clone()),
            column_map: filer_core::config::ColumnMap {
                id: Some("id".into()),
                name: Some("name".into()),
                extras: vec![],
                partner: None,
            },
            ..Default::default()
        }
        .save(&clients::master_dir(&c.root))
        .unwrap();

        let rows = [row("Acme Holdings, LLC", "ACME1002")];
        let f = find(&c.conn, None, &rows, NAME_MATCH).unwrap();
        assert_eq!(f.matches.len(), 1, "{f:?}");
        assert_eq!(f.matches[0].kind, Kind::NoId);
        let changes = [Change {
            client: acme.clone(),
            id: "ACME1002".into(),
            name: "Acme Holdings, LLC".into(),
        }];
        let (run, out) = apply(
            &mut c.conn,
            &c.root,
            "dev",
            &changes,
            &AtomicUsize::new(0),
            &AtomicBool::new(false),
        );
        assert!(matches!(out[0].1, Outcome::Changed(None)), "{:?}", out[0].1);
        assert_eq!(
            client_row(&c.conn, &acme).unwrap(),
            (
                "ACME1002".to_string(),
                "Acme Holdings, LLC".to_string(),
                Some("Acme Holdings, LLC (ACME1002)".to_string())
            )
        );
        let s =
            filer_intake::sentinel::read_sentinel(&c.root.join("Acme Holdings, LLC (ACME1002)"))
                .unwrap()
                .unwrap();
        assert_eq!(s.tax_software_id, "ACME1002");
        let text = std::fs::read_to_string(&csv).unwrap();
        assert!(
            text.contains(",Somebody Unnumbered"),
            "the blank row is as it was: {text}"
        );
        assert!(text.contains("ACME1002"), "the client is added: {text}");

        let back = undo(&mut c.conn, &c.root, "dev", &run, &AtomicUsize::new(0));
        assert!(back.iter().all(|(_, r)| r.is_ok()), "{back:?}");
        assert_eq!(
            client_row(&c.conn, &acme).unwrap(),
            (
                String::new(),
                "Acme Holdings LLC".to_string(),
                Some("Acme Holdings LLC".to_string())
            )
        );
        assert!(
            c.root
                .join("Acme Holdings LLC/Correspondence/letter.pdf")
                .is_file()
        );
        // Without an ID it is off the client list again (the list holds clients by ID).
        let listed = clients::load(&c.root).unwrap();
        assert!(!listed.clients().iter().any(|x| x.uuid.to_string() == acme));
    }

    /// Review 2026-11-17: IDs are compared loosely, a folder's own name counts, and what
    /// does not match is listed both ways with the nearest thing to it, and saved as a CSV.
    #[test]
    fn the_discrepancies_are_listed_both_ways() {
        let mut c = cab();
        let smith = client(&mut c, "01002", "Smith, John");
        let dining = client(&mut c, "OLD7", "Dining Hall LLC");
        let _jon = client(&mut c, "OLD8", "Smith, Jon");
        let _stray = client(&mut c, "OLD9", "Nobody On The List");
        // The catalog's name for one client went wrong; its folder still says who it is.
        c.conn
            .execute(
                "UPDATE client SET name = 'Something Else Entirely' WHERE id = ?1",
                [&dining],
            )
            .unwrap();
        // A folder nobody bound to a client.
        std::fs::create_dir(c.root.join("Baker Farms 2019")).unwrap();
        let rows = [
            row("Smith, John A", "1002"),
            row("The Dining Hall, L.L.C.", "DINI1999"),
            row("Smith, Joan", "JOAN1"),
            row("Baker Farms", "BAKE1"),
        ];
        let f = find(&c.conn, Some(&c.root), &rows, NAME_MATCH).unwrap();
        let got: Vec<(&str, &str, Kind, Tier)> = f
            .matches
            .iter()
            .map(|m| (m.row.id.as_str(), m.client.id.as_str(), m.kind, m.tier))
            .collect();
        assert_eq!(
            got,
            [
                ("DINI1999", dining.as_str(), Kind::OtherId, Tier::Rule),
                ("1002", smith.as_str(), Kind::SameId, Tier::Rule),
            ],
            "{f:#?}"
        );
        // Rows with no client, each with the nearest thing: a client that is not the same
        // person, and a folder with no client file.
        let none: Vec<(&str, Option<(&str, bool)>)> = f
            .no_client
            .iter()
            .map(|(r, n)| {
                (
                    r.id.as_str(),
                    n.as_ref().map(|n| (n.name.as_str(), n.unbound)),
                )
            })
            .collect();
        assert_eq!(
            none,
            [
                ("BAKE1", Some(("Baker Farms 2019", true))),
                ("JOAN1", Some(("Smith, Jon", false))),
            ]
        );
        assert!(f.no_client[1].1.as_ref().unwrap().score <= 0.6);
        // Clients no row is for, with the nearest row.
        let lone: Vec<(&str, Option<&str>)> = f
            .no_row
            .iter()
            .map(|(c, r)| (c.name.as_str(), r.as_ref().map(|(r, _)| r.id.as_str())))
            .collect();
        assert_eq!(
            lone,
            [("Nobody On The List", None), ("Smith, Jon", Some("JOAN1"))]
        );
        assert_eq!(f.unbound, ["Baker Farms 2019"]);
        let csv = report_csv(&f);
        assert_eq!(csv.lines().count(), 1 + 2 + 2 + 2 + 1, "{csv}");
        for line in [
            r#""Match: same name, written differently","The Dining Hall, L.L.C.",DINI1999,"#,
            r#""On the list, no client",Baker Farms,BAKE1,Baker Farms 2019,"#,
            r#""Client, not on the list",,,Nobody On The List,OLD9,"#,
            "Folder with no client file,,,Baker Farms 2019,",
        ] {
            assert!(csv.contains(line), "{line} is not in:\n{csv}");
        }
        // Two clients that are almost one name are never offered as a match, at any %.
        let f = find(&c.conn, None, &[row("Smith, Joan", "JOAN1")], 0.80).unwrap();
        assert!(f.matches.is_empty(), "{:?}", f.matches);
    }

    /// When a row matches one client surely and another only closely, the sure one is
    /// its client; two as sure as each other are listed, not picked.
    #[test]
    fn a_row_takes_its_clear_best_client() {
        let mut c = cab();
        let trust = client(&mut c, "T1", "Cole Family Trust");
        let _second = client(&mut c, "T2", "Cole Family Trust II");
        let f = find(&c.conn, None, &[row("Cole Family Trust", "COLE7")], 0.85).unwrap();
        assert_eq!(f.matches.len(), 1, "{f:#?}");
        assert_eq!(f.matches[0].client.id, trust);
        assert_eq!(f.matches[0].tier, Tier::Exact);
        assert!(f.unclear.is_empty());
    }

    /// How long matching takes at the firm's size (run by hand: `--ignored --nocapture`).
    #[test]
    #[ignore]
    fn matching_eight_thousand_rows_to_eight_thousand_clients() {
        let c = cab();
        let words = [
            "Acme", "Baker", "Cole", "Dunn", "Evans", "Farms", "Grove", "Hill", "Iron", "Jones",
            "Kline", "Lake", "Moss", "North", "Oak", "Pine", "Quarry", "Reed", "Stone", "Trust",
        ];
        let name = |i: usize| {
            format!(
                "{} {} {} {}",
                words[i % 20],
                words[(i / 20) % 20],
                words[(i / 400) % 20],
                ["LLC", "Inc", "Trust", "Holdings"][i % 4]
            )
        };
        for i in 0..8000 {
            c.conn
                .execute(
                    "INSERT INTO client (id, ext_id, name, device_id, updated_at)
                     VALUES (?1, ?2, ?3, 'dev', 't')",
                    (format!("u{i}"), format!("OLD{i}"), name(i)),
                )
                .unwrap();
        }
        let rows: Vec<Row> = (0..8000).map(|i| row(&name(i), &format!("N{i}"))).collect();
        let t = std::time::Instant::now();
        let f = find(&c.conn, None, &rows, NAME_MATCH).unwrap();
        println!(
            "8000 x 8000: {:?}; {} matches, {} unclear, {} with no client",
            t.elapsed(),
            f.matches.len(),
            f.unclear.len(),
            f.no_client.len()
        );
    }

    #[test]
    fn a_run_can_be_stopped_and_a_client_changed_since_is_not_put_back() {
        let mut c = cab();
        let acme = client(&mut c, "OLD1", "Acme Holdings LLC");
        let zeta = client(&mut c, "OLD2", "Zeta Farms Inc");
        let changes = [
            Change {
                client: acme.clone(),
                id: "ACME1002".into(),
                name: "Acme Holdings LLC".into(),
            },
            Change {
                client: zeta.clone(),
                id: "ZETA1009".into(),
                name: "Zeta Farms Inc".into(),
            },
        ];
        // Stopped before it began: nothing is changed.
        let (run, out) = apply(
            &mut c.conn,
            &c.root,
            "dev",
            &changes,
            &AtomicUsize::new(0),
            &AtomicBool::new(true),
        );
        assert!(run.done.is_empty() && out.is_empty());
        assert!(load_last(&c.root).unwrap().is_none());
        let (run, _) = apply(
            &mut c.conn,
            &c.root,
            "dev",
            &changes,
            &AtomicUsize::new(0),
            &AtomicBool::new(false),
        );
        assert_eq!(run.done.len(), 2);
        // The second client is given yet another ID by hand; the undo stops at it, and the
        // first stays changed too (the last change is put back first).
        let p = folders::plan_client_id(&c.conn, &c.root, &zeta, "HAND3").unwrap();
        folders::set_client_id(&mut c.conn, &c.root, &p, "dev").unwrap();
        let back = undo(&mut c.conn, &c.root, "dev", &run, &AtomicUsize::new(0));
        assert_eq!(back.len(), 1);
        assert!(back[0].1.as_ref().unwrap_err().contains("changed again"));
        assert_eq!(client_row(&c.conn, &acme).unwrap().0, "ACME1002");
        assert_eq!(load_last(&c.root).unwrap().unwrap().done.len(), 2);
    }
}
