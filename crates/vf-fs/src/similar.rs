//! Similar clients (owner 2026-11-16, Q-129, PA-90): the same person or business made twice
//! in the client list. [`find`] lists the pairs; a person ticks the ones that are the same
//! client; [`merge_many`] merges them with [`crate::clientmerge`] (one merge per pair, each
//! with its own record), and [`undo_many`] puts a run back.
//!
//! Two clients are a pair when
//! - their IDs are the same apart from capitals, spaces, `-`, `_` and leading zeros (two live
//!   clients cannot have exactly the same ID: the catalog refuses it), or
//! - their names match [`NAME_MATCH`] or better, as Intake scores names ("Last, First" and
//!   "First Last" are the same name).
//!
//! Nothing here merges on its own: every pair is a person's tick.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};

use filer_core::clients::{loose_id, normalize_name};
use vf_core::db::Connection;

use crate::clientmerge::{self, Merged};
use crate::{cabinet, clients, listupdate};

/// Two names are the same client's at this score or better.
pub const NAME_MATCH: f64 = 0.97;
/// The most pairs one find lists.
pub const LIMIT: usize = 5_000;
const DISMISSED_FILE: &str = "similar-dismissed.json";

/// One of the two clients of a pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Side {
    /// Client uuid.
    pub id: String,
    pub ext_id: String,
    pub name: String,
    /// Its folder, cabinet-relative.
    pub folder: Option<String>,
    /// Its documents (not the ones in the trash).
    pub docs: usize,
    /// The client CSV Intake uses lists its ID.
    pub in_csv: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Why {
    /// The IDs are the same apart from capitals, spaces, `-`, `_` and leading zeros.
    SameId,
    /// The names match; the IDs differ.
    SameName,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pair {
    pub a: Side,
    pub b: Side,
    pub why: Why,
    /// How well the names match (0 to 1), whatever `why` is.
    pub name_score: f64,
    /// The one kept by default: the one whose ID the client CSV lists, when only one of the
    /// two is (owner 2026-11-16). `Some(true)`: `a`. `None`: a person chooses.
    pub keep_a: Option<bool>,
}

/// What [`find`] gives back.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Found {
    pub pairs: Vec<Pair>,
    /// More than [`LIMIT`] pairs: the rest are not listed.
    pub more: bool,
    /// Clients compared.
    pub clients: usize,
    /// Pairs a person marked "not the same" (left out).
    pub dismissed: usize,
    /// Why no pair has a default to keep: no client CSV, or it cannot be read.
    pub csv_note: Option<String>,
}

/// The name as it is compared: normalized, and "First Last" for "Last, First" (what
/// `filer_core::clients::name_score` compares, kept here so a client's name is normalized
/// once, not once per pair).
pub(crate) fn name_keys(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let plain = normalize_name(name);
    if !plain.is_empty() {
        out.push(plain);
    }
    if let Some((last, first)) = name.split_once(',') {
        let swapped = normalize_name(&format!("{first} {last}"));
        if !swapped.is_empty() && !out.contains(&swapped) {
            out.push(swapped);
        }
    }
    out
}

pub(crate) fn score(a: &[String], b: &[String]) -> f64 {
    a.iter()
        .flat_map(|x| b.iter().map(move |y| strsim::jaro_winkler(x, y)))
        .fold(0.0, f64::max)
}

/// The ID as two similar clients' IDs are compared: [`loose_id`] without leading zeros.
pub fn id_key(id: &str) -> String {
    let l = loose_id(id);
    let t = l.trim_start_matches('0');
    // Nothing but zeros is one ID however many of them.
    if t.is_empty() && !l.is_empty() {
        "0".to_string()
    } else {
        t.to_string()
    }
}

fn dismissed_path(root: &Path) -> PathBuf {
    root.join(cabinet::VIBE_DIR).join(DISMISSED_FILE)
}

fn key(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

/// The pairs marked "not the same", each as its two client uuids in order. No file: none.
/// A file that cannot be read is an error, not "none": the next mark would write a file
/// with that one pair and every earlier mark would be gone (QA 2026-11-17).
pub fn dismissed(root: &Path) -> Result<BTreeSet<(String, String)>, String> {
    let file = dismissed_path(root);
    match std::fs::read(&file) {
        Ok(b) => serde_json::from_slice(&b)
            .map_err(|e| format!("{} cannot be read: {e}", file.display())),
        Err(e) if crate::cabinet::not_there(root, &e) => Ok(BTreeSet::new()),
        Err(e) => Err(format!("{} cannot be read: {e}", file.display())),
    }
}

/// Marks clients `a` and `b` (uuids) as not the same client: [`find`] leaves the pair out,
/// on every PC.
pub fn dismiss(root: &Path, a: &str, b: &str) -> Result<(), String> {
    crate::lock::with(root, "clients", "the client list", || {
        let mut all = dismissed(root)?;
        all.insert(key(a, b));
        let bytes = serde_json::to_vec_pretty(&all).map_err(|e| e.to_string())?;
        cabinet::write_replace(&dismissed_path(root), &bytes)
    })
}

/// Forgets every "not the same" mark. How many there were.
pub fn forget_dismissed(root: &Path) -> Result<usize, String> {
    crate::lock::with(root, "clients", "the client list", || {
        // Unreadable: started again, which is what was asked for.
        let n = dismissed(root).map_or(1, |d| d.len());
        if n > 0 {
            cabinet::write_replace(&dismissed_path(root), b"[]")?;
        }
        Ok(n)
    })
}

/// The pairs of similar clients in the catalog, same-ID pairs first, then by name score.
/// Reads the catalog and the client CSV; writes nothing.
pub fn find(conn: &Connection, root: &Path) -> Result<Found, String> {
    let (csv, csv_note) = match clients::csv_ids(root) {
        Ok(Some(ids)) => (ids, None),
        Ok(None) => (
            HashSet::new(),
            Some("No client CSV is set in Intake › Setup, so no pair has a client to keep by default: choose on each row.".to_string()),
        ),
        Err(e) => (
            HashSet::new(),
            Some(format!("The client CSV could not be read ({e}), so no pair has a client to keep by default: choose on each row.")),
        ),
    };
    let mut st = conn
        .prepare(
            "SELECT c.id, c.ext_id, c.name, c.folder_rel,
                    (SELECT count(*) FROM document d
                      WHERE d.client_id = c.id AND d.deleted_at IS NULL)
             FROM client c WHERE c.deleted_at IS NULL ORDER BY c.name COLLATE NOCASE, c.ext_id",
        )
        .map_err(|e| e.to_string())?;
    let sides: Vec<Side> = st
        .query_map([], |r| {
            let ext: String = r.get(1)?;
            Ok(Side {
                id: r.get(0)?,
                in_csv: false,
                ext_id: ext,
                name: r.get(2)?,
                folder: r.get::<_, Option<String>>(3)?.filter(|f| !f.is_empty()),
                docs: r.get::<_, i64>(4)? as usize,
            })
        })
        .and_then(|rows| rows.collect())
        .map_err(|e| e.to_string())?;
    // Which clients the CSV lists: the one whose ID is written there exactly; one that
    // differs only in capitals when no other client has it exactly (`AC20` and `ac20` with
    // `AC20` in the CSV: only the first is listed).
    let exact: HashSet<&str> = sides
        .iter()
        .map(|s| s.ext_id.trim())
        .filter(|id| csv.contains(*id))
        .collect();
    let exact_upper: HashSet<String> = exact.iter().map(|id| id.to_uppercase()).collect();
    let csv_upper: HashSet<String> = csv.iter().map(|id| id.to_uppercase()).collect();
    let listed: Vec<bool> = sides
        .iter()
        .map(|s| {
            let id = s.ext_id.trim();
            !id.is_empty()
                && (exact.contains(id)
                    || (csv_upper.contains(&id.to_uppercase())
                        && !exact_upper.contains(&id.to_uppercase())))
        })
        .collect();
    let mut sides = sides;
    for (s, l) in sides.iter_mut().zip(listed) {
        s.in_csv = l;
    }
    let keys: Vec<(Vec<String>, String)> = sides
        .iter()
        .map(|s| (name_keys(&s.name), id_key(&s.ext_id)))
        .collect();
    let not_same = dismissed(root)?;
    let mut out = Found {
        clients: sides.len(),
        csv_note,
        ..Default::default()
    };
    for i in 0..sides.len() {
        for j in i + 1..sides.len() {
            let same_id = !keys[i].1.is_empty() && keys[i].1 == keys[j].1;
            let s = score(&keys[i].0, &keys[j].0);
            if !same_id && s < NAME_MATCH {
                continue;
            }
            let (a, b) = (&sides[i], &sides[j]);
            if not_same.contains(&key(&a.id, &b.id)) {
                out.dismissed += 1;
                continue;
            }
            out.pairs.push(Pair {
                a: a.clone(),
                b: b.clone(),
                why: if same_id { Why::SameId } else { Why::SameName },
                name_score: s,
                keep_a: match (a.in_csv, b.in_csv) {
                    (true, false) => Some(true),
                    (false, true) => Some(false),
                    _ => None,
                },
            });
            // Kept within bounds while comparing (hundreds of clients under one placeholder
            // name are tens of thousands of pairs).
            if out.pairs.len() > LIMIT * 4 {
                rank(&mut out.pairs);
                out.pairs.truncate(LIMIT + 1);
            }
        }
    }
    rank(&mut out.pairs);
    if out.pairs.len() > LIMIT {
        out.pairs.truncate(LIMIT);
        out.more = true;
    }
    Ok(out)
}

/// Same-ID pairs first, then the best name matches.
fn rank(pairs: &mut [Pair]) {
    pairs.sort_by(|x, y| {
        x.why
            .cmp(&y.why)
            .then(y.name_score.total_cmp(&x.name_score))
            .then_with(|| x.a.name.to_lowercase().cmp(&y.a.name.to_lowercase()))
    });
}

/// The IDs in the CSV at `csv`, as [`id_key`] compares them: for marking, in every pair,
/// the client whose ID is on the firm's list as the one to keep. The ID column is told by
/// its name (`ID`, `Client ID` …) or, in a list of two columns, by its shorter values.
pub fn ids_on_list(csv: &Path) -> Result<HashSet<String>, String> {
    let cols = listupdate::columns(csv)?;
    let Some(id) = cols.id else {
        return Err("The list's ID column could not be told: name it ID in its first row.".into());
    };
    let table = filer_core::clients::read_csv(csv).map_err(|e| e.to_string())?;
    let col = table
        .headers
        .iter()
        .position(|h| *h == id)
        .ok_or("the ID column is gone")?;
    Ok(table
        .rows
        .iter()
        .filter_map(|r| r.get(col))
        .map(|v| id_key(v))
        .filter(|k| !k.is_empty())
        .collect())
}

/// One ticked pair: client `from` (uuid) is merged into client `into`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub from: String,
    pub into: String,
    /// An ID neither of the two has, typed by the person: after the merge the client that
    /// stays is given it, and its folder is renamed (owner 2026-11-17).
    pub new_id: Option<String>,
}

/// One pair of a preview: what merging it would do, or why it cannot be merged.
pub type Previewed = (Choice, Result<Preview, String>);

/// What merging one pair would do, for the preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    pub from: String,
    pub into: String,
    /// Documents that move to the kept client.
    pub moves: usize,
    /// Of those, the ones whose name is taken there: both are kept (` (2)`).
    pub kept_both: usize,
    pub holds: usize,
    pub relationships: usize,
    /// A client of this pair is in an earlier pair too: the numbers are as things stand
    /// now, before that merge.
    pub after_another: bool,
    /// The kept client has no ID, so the merged client's relationships cannot become its
    /// own: they are dropped.
    pub drops_relationships: bool,
    /// The kept client has no folder: the other's folder becomes its folder, (now, after
    /// its rename for the kept client's name and ID; the same when its name stays).
    pub adopts: Option<(String, String)>,
    /// The ID the client that stays is given after the merge ([`Choice::new_id`]).
    pub new_id: Option<String>,
}

/// Whether merging `from` into `into` is the kept client taking the other's folder: the
/// one kept has none and the other has one (owner 2026-11-17: the client with the right ID
/// came from the firm's list and has no folder; the folder is under the old ID). Returns
/// the kept client's ID and name.
fn adopting(conn: &Connection, from: &str, into: &str) -> Result<Option<(String, String)>, String> {
    let (_, _, from_folder) = listupdate::client_row(conn, from)?;
    let (id, name, into_folder) = listupdate::client_row(conn, into)?;
    Ok((into_folder.is_none() && from_folder.is_some()).then_some((id, name)))
}

/// `id` after the merges in `gone` (merged client → kept client).
fn now_is(gone: &HashMap<String, String>, id: &str) -> String {
    let mut at = id.to_string();
    let mut steps = 0;
    while let Some(next) = gone.get(&at) {
        at = next.clone();
        steps += 1;
        if steps > gone.len() {
            break;
        }
    }
    at
}

/// What merging each of `choices` would do, in order. Nothing is written. A pair that cannot
/// be merged says why. `Err`: no merge can be made at all now (the client CSV Intake uses
/// cannot be reached: every merge takes the merged client out of it).
pub fn preview(
    conn: &Connection,
    root: &Path,
    choices: &[Choice],
) -> Result<Vec<Previewed>, String> {
    clients::intake_csv_reachable(root).map_err(|e| {
        // Intake's settings unreadable is said as that; else the CSV is out of reach.
        if e.starts_with("Intake's settings") {
            e
        } else {
            "The client CSV Intake uses cannot be reached from this PC. Every merge takes the merged client out of it, so nothing can be merged until it can.".to_string()
        }
    })?;
    let mut gone: HashMap<String, String> = HashMap::new();
    let mut touched: HashSet<String> = HashSet::new();
    // Clients that took a kept client's ID earlier in the run.
    let mut took: HashSet<String> = HashSet::new();
    Ok(choices
        .iter()
        .map(|c| {
            let (from, into) = (now_is(&gone, &c.from), now_is(&gone, &c.into));
            let after_another = [&c.from, &c.into, &from, &into]
                .iter()
                .any(|id| touched.contains(id.as_str()));
            let r = if from == into {
                Err("These two are one client already after the merges above.".to_string())
            } else if took.contains(&from) {
                Err(TWICE.to_string())
            } else if let Some((id, name)) = adopting(conn, &from, &into).unwrap_or(None) {
                // The kept client's place on the list goes to the client with the folder,
                // which then takes its ID and name: checked as those two steps.
                clientmerge::plan(conn, root, &into, &from).and_then(|p| {
                    let change = listupdate::Change {
                        client: from.clone(),
                        id,
                        name,
                        twin: None,
                    };
                    let freed: HashSet<String> = [into.clone()].into();
                    let planned = listupdate::preview_freeing(conn, root, &[change], &freed)?
                        .pop()
                        .map(|(_, r)| r)
                        .unwrap_or_else(|| Err("nothing planned".into()))?;
                    let folder = p.into.folder.clone().unwrap_or_default();
                    Ok(Preview {
                        from: format!("{}  {}", p.into.name, p.into.ext_id),
                        into: format!("{}  {}", p.from.name, p.from.ext_id),
                        moves: 0,
                        kept_both: 0,
                        holds: p.holds,
                        relationships: p.relationships,
                        drops_relationships: false,
                        after_another,
                        adopts: Some(planned.folder.unwrap_or((folder.clone(), folder))),
                        new_id: None,
                    })
                })
            } else {
                clientmerge::plan(conn, root, &from, &into).map(|p| Preview {
                    from: format!("{}  {}", p.from.name, p.from.ext_id),
                    into: format!("{}  {}", p.into.name, p.into.ext_id),
                    moves: p
                        .moves
                        .iter()
                        .filter(|m| m.from != m.to && !m.is_notes())
                        .count(),
                    kept_both: p.kept_both(),
                    holds: p.holds,
                    relationships: p.relationships,
                    drops_relationships: p.relationships > 0 && p.into.ext_id.trim().is_empty(),
                    after_another,
                    adopts: None,
                    new_id: None,
                })
            };
            // A new ID for the client that stays: free (the two's own IDs count as free: one
            // of them goes) and fit to be an ID.
            let r = r.and_then(|mut p| {
                let Some(id) = c.new_id.as_deref().map(str::trim).filter(|i| !i.is_empty()) else {
                    return Ok(p);
                };
                let stays = if p.adopts.is_some() { &from } else { &into };
                let (_, name, _) = listupdate::client_row(conn, stays)?;
                let change = listupdate::Change {
                    client: stays.clone(),
                    id: id.to_string(),
                    name,
                    twin: None,
                };
                if clients::is_tin_value(id) {
                    return Err(
                        "That ID looks like an SSN or EIN. Use the tax software's client ID."
                            .into(),
                    );
                }
                if id.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
                    return Err("An ID cannot hold / \\ : * ? \" < > |.".into());
                }
                let freed: HashSet<String> = [from.clone(), into.clone()].into();
                listupdate::preview_freeing(conn, root, &[change], &freed)?
                    .pop()
                    .map(|(_, r)| r)
                    .unwrap_or_else(|| Err("nothing planned".into()))?;
                p.new_id = Some(id.to_string());
                Ok(p)
            });
            match &r {
                // The identity that stays is the folder's.
                Ok(p) if p.adopts.is_some() => {
                    took.insert(from.clone());
                    gone.insert(into.clone(), from.clone());
                }
                Ok(_) => {
                    gone.insert(from.clone(), into.clone());
                }
                Err(_) => {}
            }
            touched.extend([c.from.clone(), c.into.clone(), from, into]);
            (c.clone(), r)
        })
        .collect())
}

/// How one pair of a run ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Merged(Box<Ran>),
    /// Nothing to do (the two became one client earlier in the run), or the run was stopped.
    Skipped(String),
    /// Refused: nothing of this pair was changed.
    Failed(String),
}

/// A client with a folder that took a kept client's ID in this run is not given another's:
/// it would end with the second ID and the first kept client gone.
const TWICE: &str = "The client with the folder has taken another kept client's ID in this run already. Merge those two kept clients first.";

/// Whether the pair is surely one client: the same ID, or the same name however it is
/// punctuated (`namematch`'s exact tier). Only these are ticked for the person many at a
/// time: the pairs are found by how alike the letters are, which lists `Smith, John` with
/// `Smith, Jon`.
pub fn surely_one(p: &Pair) -> bool {
    use crate::namematch::{Name, Tier, compare};
    p.why == Why::SameId
        || compare(
            &Name::of(&p.a.name, Some(&p.a.ext_id)),
            &Name::of(&p.b.name, Some(&p.b.ext_id)),
        )
        .is_some_and(|(_, t)| t == Tier::Exact)
}

/// One merge of a run, as its undo needs it.
#[derive(Debug, Clone, PartialEq)]
pub struct Ran {
    pub merged: Merged,
    /// The kept client had no folder and took the other's (see [`Preview::adopts`]).
    pub adopted: Option<Adopted>,
    /// The client that stays was given a new ID after the merge ([`Choice::new_id`]).
    pub reid: Option<listupdate::Done>,
}

/// What a kept client taking the other's folder changed besides the merge.
#[derive(Debug, Clone, PartialEq)]
pub struct Adopted {
    /// The client with the folder, given the kept client's ID and name.
    pub done: listupdate::Done,
    /// Its own row in the client CSV (under its old ID), taken out.
    pub old_csv_row: Option<String>,
}

/// The kept client `kept` (no folder) and the client `other` (the folder): one client with
/// the kept one's ID and name and the other's folder, documents and identity in the
/// catalog. Nothing is moved file by file: the folder is renamed. As two steps that are
/// each undone by their own undo: `kept` is merged into `other` (its place on the client
/// list, its relationships and holds; it has no documents), then `other` takes its ID and
/// name. The catalog identity that stays is the folder's, so its documents, versions and
/// B2 objects are untouched.
pub(crate) fn adopt(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    other: &str,
    kept: &str,
    id: String,
    name: String,
) -> Result<Ran, String> {
    let plan = clientmerge::plan(conn, root, kept, other)?;
    let merged = clientmerge::apply(conn, root, &plan, device_id)?;
    let change = listupdate::Change {
        client: other.to_string(),
        id,
        name,
        twin: None,
    };
    // The same ID and name already (the two differed in nothing else): the merge is all.
    let (now_id, now_name, _) = listupdate::client_row(conn, other)?;
    if now_id == change.id.trim() && now_name == change.name.trim() {
        return Ok(Ran {
            merged,
            adopted: None,
            reid: None,
        });
    }
    match listupdate::change_one(conn, root, device_id, &change, true) {
        Ok((done, _)) => {
            // The client CSV: the kept client's own row is the firm's, and stays; the
            // row under the folder's old ID goes.
            // (A blank ID is no row's: not looked for.)
            let old_csv_row = if done.old_id.trim().is_empty() {
                None
            } else {
                clients::remove_from_csv(root, &done.old_id).ok().flatten()
            };
            // The kept client's partner, when the folder's client has none (the merge
            // sets the merged client's details aside for its undo).
            if let Some(partner) = merged.details.get("partner")
                && clients::load(root).is_ok_and(|m| !m.meta.partners.contains_key(other))
            {
                let _ = clients::set_partner(root, other, Some(partner));
            }
            if let Some(row) = &merged.csv_row {
                let _ = clients::restore_csv_row(root, &merged.from.ext_id, row);
            }
            Ok(Ran {
                merged,
                adopted: Some(Adopted { done, old_csv_row }),
                reid: None,
            })
        }
        // The folder's client could not take the ID: the two are as they were.
        Err(e) => match clientmerge::undo(conn, root, &merged, device_id) {
            Ok(_) => Err(e),
            Err(u) => Err(format!(
                "{e} The kept client's place on the client list could not be put back ({u}): it is merged into the one with the folder, under that one's ID."
            )),
        },
    }
}

/// Merges each of `choices` in order, each as its own merge (its own Transaction and record,
/// its own undo). A pair that is refused does not stop the rest. A client merged away earlier
/// in the run stands for the client it went into (A into B, then B into C: all three are C).
/// `done` counts pairs dealt with; `stop` ends the run between two pairs.
pub fn merge_many(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    choices: &[Choice],
    done: &AtomicUsize,
    stop: &AtomicBool,
) -> Vec<(Choice, Outcome)> {
    let mut gone: HashMap<String, String> = HashMap::new();
    let mut took: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(choices.len());
    for c in choices {
        if stop.load(Relaxed) {
            out.push((
                c.clone(),
                Outcome::Skipped("Stopped before this pair.".into()),
            ));
            continue;
        }
        let (from, into) = (now_is(&gone, &c.from), now_is(&gone, &c.into));
        let r = if from == into {
            Outcome::Skipped("Already one client after an earlier merge of this run.".into())
        } else {
            // Planned now, as things are after the merges before it.
            match adopting(conn, &from, &into) {
                Err(e) => Outcome::Failed(e),
                _ if took.contains(&from) => Outcome::Failed(TWICE.to_string()),
                Ok(Some((id, name))) => {
                    match adopt(conn, root, device_id, &from, &into, id, name) {
                        Ok(r) => {
                            // The identity that stays is the folder's.
                            took.insert(from.clone());
                            gone.insert(into, from);
                            Outcome::Merged(Box::new(r))
                        }
                        Err(e) => Outcome::Failed(e),
                    }
                }
                Ok(None) => match clientmerge::plan(conn, root, &from, &into)
                    .and_then(|p| clientmerge::apply(conn, root, &p, device_id))
                {
                    Ok(m) => {
                        gone.insert(from, into);
                        Outcome::Merged(Box::new(Ran {
                            merged: m,
                            adopted: None,
                            reid: None,
                        }))
                    }
                    Err(e) => Outcome::Failed(e),
                },
            }
        };
        // The new ID, for the client that stays (the folder's, where the kept one had none).
        let r = match (
            r,
            c.new_id.as_deref().map(str::trim).filter(|i| !i.is_empty()),
        ) {
            (Outcome::Merged(mut ran), Some(id)) => {
                let stays = now_is(&gone, &c.into);
                let set = listupdate::client_row(conn, &stays).and_then(|(_, name, _)| {
                    let change = listupdate::Change {
                        client: stays.clone(),
                        id: id.to_string(),
                        name,
                        twin: None,
                    };
                    listupdate::change_one(conn, root, device_id, &change, false)
                });
                match set {
                    Ok((d, _)) => ran.reid = Some(d),
                    // Merged all the same: said, so the ID is set by hand.
                    Err(e) => {
                        let said = format!("Merged, but the ID {id} could not be given: {e}");
                        ran.merged.note = Some(match ran.merged.note.take() {
                            Some(n) => format!("{n} {said}"),
                            None => said,
                        });
                    }
                }
                Outcome::Merged(ran)
            }
            (r, _) => r,
        };
        out.push((c.clone(), r));
        done.fetch_add(1, Relaxed);
    }
    out
}

/// One merge put back: its documents that went back, and what of the client's own could
/// not be (see [`clientmerge::undo_noted`]).
pub type PutBack = Result<(usize, Option<String>), String>;

/// Puts the merges of a run back, the last one first, and stops at the first that cannot be
/// (the ones before it in the run may hold its documents: A into B, then B into C; with C's
/// merge not put back, putting A's back would restore an empty client; QA 2026-11-17). For
/// each one tried (in that order): the client merged away, and how many documents went back
/// or why it could not be undone. The merges not tried are still merged.
pub fn undo_many(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    merged: &[Ran],
) -> Vec<(String, PutBack)> {
    let mut out = Vec::new();
    for ran in merged.iter().rev() {
        let m = &ran.merged;
        // A kept client that took the other's folder: the folder's client gets its own ID,
        // name and CSV row back first, then the kept one its place on the list.
        // A new ID given after the merge goes first: the client gets its own back.
        let reid = match &ran.reid {
            None => Ok(()),
            Some(d) => listupdate::put_back_one(conn, root, device_id, d),
        };
        let renamed = match (&reid, &ran.adopted) {
            (Err(e), _) => Err(e.clone()),
            (_, None) => Ok(()),
            (_, Some(a)) => listupdate::put_back_one(conn, root, device_id, &a.done).map(|()| {
                if let Some(row) = &a.old_csv_row {
                    let _ = clients::restore_csv_row(root, &a.done.old_id, row);
                }
                // (The merge's undo puts the kept client's row back: not twice.)
                if m.csv_row.is_some() {
                    let _ = clients::remove_from_csv(root, &m.from.ext_id);
                }
            }),
        };
        // With what the undo could not put back of the client's own (its place on the
        // list, its relationships, its holds), for the page to say.
        let r = renamed.and_then(|()| clientmerge::undo_noted(conn, root, m, device_id));
        let failed = r.is_err();
        out.push((format!("{}  {}", m.from.name, m.from.ext_id), r));
        if failed {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cabinet::CabinetFile;
    use filer_core::clients::Client;
    use std::collections::BTreeMap;
    use uuid::Uuid;
    use vf_core::db::Storage;

    /// A cabinet of invented clients, each with a folder and one document, and a client CSV
    /// listing `in_csv`.
    fn cabinet(dir: &Path, all: &[(&str, &str)], in_csv: &[&str]) -> (PathBuf, Connection) {
        let root = dir.join("cabinet");
        let firm = Uuid::new_v4();
        cabinet::create(
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
        for (name, id) in all {
            let folder = root.join(format!("{} ({id})", name.replace(',', "")));
            std::fs::create_dir_all(folder.join("Income Tax/2025")).unwrap();
            filer_intake::sentinel::write_sentinel(
                &root,
                &folder,
                &Client {
                    id: (*id).into(),
                    uuid: Uuid::new_v4(),
                    name: (*name).into(),
                    extra: BTreeMap::new(),
                },
                firm,
            )
            .unwrap();
            std::fs::write(
                folder.join("Income Tax/2025/return.pdf"),
                format!("the return of {id}"),
            )
            .unwrap();
        }
        crate::doctypes::seed_if_missing(&root).unwrap();
        let csv = dir.join("clients.csv");
        let mut text = String::from("id,name\r\n");
        for (name, id) in all.iter().filter(|(_, id)| in_csv.contains(id)) {
            text.push_str(&format!("{id},\"{name}\"\r\n"));
        }
        std::fs::write(&csv, text).unwrap();
        filer_core::config::Settings {
            root: Some(root.clone()),
            csv_path: Some(csv),
            column_map: filer_core::config::ColumnMap {
                id: Some("id".into()),
                name: Some("name".into()),
                extras: vec![],
                partner: None,
            },
            ..Default::default()
        }
        .save(&clients::master_dir(&root))
        .unwrap();
        let mut conn = vf_core::db::open(&dir.join("catalog.db"), Storage::Local).unwrap();
        crate::rebuild::rebuild(&mut conn, &root, "dev").unwrap();
        clients::ensure_listed(&conn, &root).unwrap();
        (root, conn)
    }

    fn uuid_of(conn: &Connection, ext: &str) -> String {
        conn.query_row(
            "SELECT id FROM client WHERE ext_id = ?1 AND deleted_at IS NULL",
            [ext],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn live(conn: &Connection) -> Vec<String> {
        let mut st = conn
            .prepare("SELECT ext_id FROM client WHERE deleted_at IS NULL ORDER BY ext_id")
            .unwrap();
        st.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[test]
    fn the_score_is_intakes_name_score() {
        for (a, b) in [
            ("Smith, John", "John Smith"),
            ("Acme LLC", "Acme L.L.C."),
            ("Baker Co", "Acme LLC"),
            ("O'Neil Farms", "ONeil Farms Inc"),
        ] {
            assert_eq!(
                score(&name_keys(a), &name_keys(b)),
                filer_core::clients::name_score(a, b),
                "{a} / {b}"
            );
        }
        assert_eq!(id_key("01-ha_1001"), "1HA1001");
        assert_eq!(
            (id_key("000"), id_key("0"), id_key("")),
            ("0".into(), "0".into(), String::new())
        );
    }

    #[test]
    fn pairs_are_found_by_id_and_by_name_and_the_csvs_client_is_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, conn) = cabinet(
            tmp.path(),
            &[
                ("Smith, John", "1001"),
                ("John Smith", "5001"),
                ("Acme LLC", "AC-20"),
                ("Acme Holdings of Ohio", "ac20"),
                ("Baker Co", "3001"),
                ("Baker Co", "3002"),
                ("Unrelated Farms", "4001"),
            ],
            &["1001", "AC-20", "3001", "3002", "4001"],
        );
        let f = find(&conn, &root).unwrap();
        assert_eq!(f.clients, 7);
        assert!(f.csv_note.is_none() && !f.more);
        let said: Vec<(Why, &str, &str, Option<bool>)> = f
            .pairs
            .iter()
            .map(|p| (p.why, p.a.ext_id.as_str(), p.b.ext_id.as_str(), p.keep_a))
            .collect();
        // Same ID first (names nothing alike); then the names. The CSV's client is kept;
        // with both in the CSV a person chooses.
        assert_eq!(said.len(), 3, "{said:?}");
        assert_eq!(said[0].0, Why::SameId);
        let ids: BTreeSet<&str> = [said[0].1, said[0].2].into();
        assert_eq!(ids, ["AC-20", "ac20"].into());
        let kept = if said[0].3 == Some(true) {
            said[0].1
        } else {
            said[0].2
        };
        assert_eq!(kept, "AC-20");
        assert!(said[1..].iter().all(|s| s.0 == Why::SameName));
        let smith = said
            .iter()
            .find(|s| s.1 == "5001" || s.2 == "5001")
            .unwrap();
        let kept = if smith.3 == Some(true) {
            smith.1
        } else {
            smith.2
        };
        assert_eq!(kept, "1001");
        let baker = said.iter().find(|s| s.1 == "3001").unwrap();
        assert_eq!(baker.3, None, "both in the CSV: no default");
        assert_eq!(f.pairs[0].a.docs, 1);

        // "Not the same" is remembered, for every PC (a file in the cabinet).
        dismiss(&root, &uuid_of(&conn, "3002"), &uuid_of(&conn, "3001")).unwrap();
        let f = find(&conn, &root).unwrap();
        assert_eq!((f.pairs.len(), f.dismissed), (2, 1));
        assert_eq!(forget_dismissed(&root).unwrap(), 1);
        assert_eq!(find(&conn, &root).unwrap().pairs.len(), 3);
    }

    /// QA 2026-11-17: IDs that differ only in capitals. The CSV lists the one written as it
    /// has it; merging the other leaves the kept client's CSV row alone.
    #[test]
    fn ids_that_differ_in_capitals_keep_the_csvs_row() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, mut conn) = cabinet(
            tmp.path(),
            &[("Acme LLC", "AC20"), ("Acme Holdings of Ohio", "ac20")],
            &["AC20"],
        );
        let f = find(&conn, &root).unwrap();
        assert_eq!(f.pairs.len(), 1);
        let p = &f.pairs[0];
        let (kept, gone) = if p.keep_a == Some(true) {
            (&p.a, &p.b)
        } else {
            (&p.b, &p.a)
        };
        assert_eq!(
            (kept.ext_id.as_str(), gone.in_csv),
            ("AC20", false),
            "{p:?}"
        );
        let choice = Choice {
            from: gone.id.clone(),
            into: kept.id.clone(),
            new_id: None,
        };
        let (done, stop) = (AtomicUsize::new(0), AtomicBool::new(false));
        let out = merge_many(&mut conn, &root, "dev", &[choice], &done, &stop);
        assert!(matches!(out[0].1, Outcome::Merged(_)), "{:?}", out[0].1);
        let csv = std::fs::read_to_string(tmp.path().join("clients.csv")).unwrap();
        assert!(csv.contains("AC20"), "the kept client's row stays: {csv}");
    }

    /// QA 2026-11-17: a mark file that cannot be read is said, and not written over; with the
    /// CSV out of reach nothing can be merged, said before anything is ticked through.
    #[test]
    fn an_unreadable_mark_file_and_a_csv_out_of_reach_are_said() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, conn) = cabinet(
            tmp.path(),
            &[("Baker Co", "3001"), ("Baker Co", "3002")],
            &["3001"],
        );
        let (a, b) = (uuid_of(&conn, "3001"), uuid_of(&conn, "3002"));
        std::fs::write(dismissed_path(&root), "not json").unwrap();
        assert!(dismiss(&root, &a, &b).is_err());
        assert!(find(&conn, &root).is_err());
        assert_eq!(
            std::fs::read_to_string(dismissed_path(&root)).unwrap(),
            "not json"
        );
        std::fs::remove_file(dismissed_path(&root)).unwrap();
        std::fs::remove_file(tmp.path().join("clients.csv")).unwrap();
        let choice = Choice {
            from: b,
            into: a,
            new_id: None,
        };
        let e = preview(&conn, &root, &[choice]).unwrap_err();
        assert!(e.contains("cannot be reached"), "{e}");
    }

    #[test]
    fn without_a_csv_no_pair_has_a_default() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, conn) = cabinet(
            tmp.path(),
            &[("Baker Co", "3001"), ("Baker Co", "3002")],
            &["3001"],
        );
        let master = clients::master_dir(&root);
        let (mut s, _) = filer_core::config::Settings::load_or_default(&master);
        s.csv_path = None;
        s.save(&master).unwrap();
        let f = find(&conn, &root).unwrap();
        assert_eq!(f.pairs.len(), 1);
        assert_eq!(f.pairs[0].keep_a, None);
        assert!(f.csv_note.is_some());
    }

    /// A chain (A~B, B~C) with keep choices that pull two ways ends as one client, a refused
    /// pair does not stop the run, and the run is put back last first.
    /// Owner 2026-11-17: the client to keep is the one with the right ID, which came from
    /// the firm's list and has no folder; the folder is under the old ID. Keeping it makes
    /// the other's folder its folder (renamed, nothing moved file by file), and the undo
    /// puts both back.
    #[test]
    fn a_kept_client_with_no_folder_takes_the_others_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, mut conn) = cabinet(tmp.path(), &[("6 R Farms LLC", "6RFA1001")], &[]);
        // The firm's list brought the same client under its real ID: on the client list
        // and in the client CSV, with no folder.
        let listed = Uuid::new_v4();
        let mut list = clients::load(&root).unwrap().list.unwrap();
        list.uuids.insert("7974_6RFA".into(), listed);
        list.clients.push(Client {
            id: "7974_6RFA".into(),
            uuid: listed,
            name: "6 R Farms, LLC".into(),
            extra: BTreeMap::new(),
        });
        clients::save_list(&root, &list).unwrap();
        let csv = tmp.path().join("clients.csv");
        std::fs::write(
            &csv,
            "id,name\r\n7974_6RFA,\"6 R Farms, LLC\"\r\n6RFA1001,6 R Farms LLC\r\n",
        )
        .unwrap();
        crate::rebuild::sync_clients(&mut conn, &root, "dev", "clients_import").unwrap();
        assert_eq!(live(&conn), ["6RFA1001", "7974_6RFA"]);
        let folder_client = uuid_of(&conn, "6RFA1001");
        let kept = uuid_of(&conn, "7974_6RFA");
        assert_eq!(kept, listed.to_string());

        let choices = [Choice {
            from: folder_client.clone(),
            into: kept.clone(),
            new_id: None,
        }];
        let p = preview(&conn, &root, &choices).unwrap();
        let pv = p[0]
            .1
            .as_ref()
            .expect("a client with no folder can be kept");
        assert_eq!(pv.moves, 0);
        assert_eq!(
            pv.adopts,
            Some((
                "6 R Farms LLC (6RFA1001)".to_string(),
                "6 R Farms, LLC (7974_6RFA)".to_string()
            ))
        );
        assert!(
            root.join("6 R Farms LLC (6RFA1001)").is_dir(),
            "nothing changed"
        );

        let (done, stop) = (AtomicUsize::new(0), AtomicBool::new(false));
        let out = merge_many(&mut conn, &root, "dev", &choices, &done, &stop);
        let Outcome::Merged(ran) = &out[0].1 else {
            panic!("{:?}", out[0].1);
        };
        assert!(ran.adopted.is_some());
        // One client: the kept one's ID and name, the folder and its documents.
        assert_eq!(live(&conn), ["7974_6RFA"]);
        let (name, folder): (String, String) = conn
            .query_row(
                "SELECT name, folder_rel FROM client WHERE deleted_at IS NULL",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (name.as_str(), folder.as_str()),
            ("6 R Farms, LLC", "6 R Farms, LLC (7974_6RFA)")
        );
        assert!(
            root.join("6 R Farms, LLC (7974_6RFA)/Income Tax/2025/return.pdf")
                .is_file()
        );
        assert!(!root.join("6 R Farms LLC (6RFA1001)").exists());
        let doc: (String, String) = conn
            .query_row(
                "SELECT rel_path, client_id FROM document WHERE deleted_at IS NULL",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            doc,
            (
                "6 R Farms, LLC (7974_6RFA)/Income Tax/2025/return.pdf".to_string(),
                folder_client.clone()
            ),
            "the document is the same document, of the same catalog client"
        );
        let on_list = clients::load(&root).unwrap();
        let ids: Vec<&str> = on_list.clients().iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["7974_6RFA"]);
        let text = std::fs::read_to_string(&csv).unwrap();
        assert!(
            text.contains("7974_6RFA,\"6 R Farms, LLC\""),
            "the firm's row stays: {text}"
        );
        assert!(
            !text.contains("6RFA1001"),
            "the old ID's row is gone: {text}"
        );

        // And back: two clients, the folder under its old name, both rows.
        let back = undo_many(&mut conn, &root, "dev", &[(**ran).clone()]);
        assert!(back.iter().all(|(_, r)| r.is_ok()), "{back:?}");
        assert_eq!(live(&conn), ["6RFA1001", "7974_6RFA"]);
        assert!(
            root.join("6 R Farms LLC (6RFA1001)/Income Tax/2025/return.pdf")
                .is_file()
        );
        assert!(!root.join("6 R Farms, LLC (7974_6RFA)").exists());
        let text = std::fs::read_to_string(&csv).unwrap();
        assert!(
            text.contains("7974_6RFA") && text.contains("6RFA1001"),
            "{text}"
        );
        assert_eq!(text.matches("7974_6RFA").count(), 1, "not twice: {text}");
    }

    /// Only what is surely one client is ticked many at a time; and a client with a folder
    /// is not given two kept clients' IDs in one run.
    #[test]
    fn pairs_alike_by_letters_only_are_not_surely_one() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, mut conn) = cabinet(
            tmp.path(),
            &[
                ("Smith, John", "1001"),
                ("Smith, Jon", "1002"),
                ("Baker Co", "3001"),
                ("Baker Co.", "3002"),
                ("Cole Trust", "06001"),
            ],
            &[],
        );
        let f = find(&conn, &root).unwrap();
        let pair = |a: &str, b: &str| {
            f.pairs
                .iter()
                .find(|p| {
                    [p.a.ext_id.as_str(), p.b.ext_id.as_str()] == [a, b]
                        || [p.a.ext_id.as_str(), p.b.ext_id.as_str()] == [b, a]
                })
                .unwrap_or_else(|| panic!("no pair {a} / {b}: {:?}", f.pairs))
        };
        assert!(
            !surely_one(pair("1001", "1002")),
            "John and Jon are two people"
        );
        assert!(surely_one(pair("3001", "3002")));

        // Two list clients of one name, no folders, and the one folder: the folder's
        // client takes the first kept client's ID, and is refused the second.
        for id in ["K1", "K2"] {
            let uuid = Uuid::new_v4();
            let mut list = clients::load(&root).unwrap().list.unwrap();
            list.uuids.insert(id.into(), uuid);
            list.clients.push(Client {
                id: id.into(),
                uuid,
                name: "Cole Trust".into(),
                extra: BTreeMap::new(),
            });
            clients::save_list(&root, &list).unwrap();
        }
        crate::rebuild::sync_clients(&mut conn, &root, "dev", "clients_import").unwrap();
        let folder = uuid_of(&conn, "06001");
        let choices = [
            Choice {
                from: folder.clone(),
                into: uuid_of(&conn, "K1"),
                new_id: None,
            },
            Choice {
                from: folder.clone(),
                into: uuid_of(&conn, "K2"),
                new_id: None,
            },
        ];
        let p = preview(&conn, &root, &choices).unwrap();
        assert!(p[0].1.is_ok(), "{:?}", p[0].1);
        assert!(
            p[1].1.as_ref().is_err_and(|e| e.contains("already")),
            "{:?}",
            p[1].1
        );
        let out = merge_many(
            &mut conn,
            &root,
            "dev",
            &choices,
            &AtomicUsize::new(0),
            &AtomicBool::new(false),
        );
        assert!(matches!(out[0].1, Outcome::Merged(_)), "{:?}", out[0].1);
        assert!(
            matches!(&out[1].1, Outcome::Failed(e) if e.contains("already")),
            "{:?}",
            out[1].1
        );
        assert!(live(&conn).contains(&"K1".to_string()) && live(&conn).contains(&"K2".to_string()));
        assert!(!live(&conn).contains(&"06001".to_string()));
        // The merge half alone is not undone while the ID is the folder client's.
        let Outcome::Merged(ran) = &out[0].1 else {
            unreachable!()
        };
        let e = clientmerge::undo(&mut conn, &root, &ran.merged, "dev").unwrap_err();
        assert!(e.contains("cannot be undone by itself"), "{e}");
    }

    /// Owner 2026-11-18: after merges made on one PC, the cabinet did not open ("UNIQUE
    /// constraint failed: client.ext_id"). Each PC has its own catalog: the other one still
    /// has the client that went, alive under the ID the folder's client has now. Its index
    /// (what Open cabinet runs) takes the cabinet as it is.
    #[test]
    fn another_pcs_catalog_takes_the_merges_made_here() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, mut conn) = cabinet(
            tmp.path(),
            &[
                ("6 R Farms LLC", "6RFA1001"),
                ("Baker Co", "3001"),
                ("Baker Co.", "3002"),
                ("Cole Trust", "5001"),
                ("Cole Trust.", "5002"),
            ],
            &[],
        );
        let listed = Uuid::new_v4();
        let mut list = clients::load(&root).unwrap().list.unwrap();
        list.uuids.insert("7974_6RFA".into(), listed);
        list.clients.push(Client {
            id: "7974_6RFA".into(),
            uuid: listed,
            name: "6 R Farms, LLC".into(),
            extra: BTreeMap::new(),
        });
        clients::save_list(&root, &list).unwrap();
        crate::rebuild::sync_clients(&mut conn, &root, "dev", "clients_import").unwrap();
        // The second PC's catalog, in line with the first.
        let mut other = vf_core::db::open(&tmp.path().join("pc2.db"), Storage::Local).unwrap();
        crate::rebuild::rebuild(&mut other, &root, "dev2").unwrap();
        assert_eq!(live(&other), live(&conn));

        let choices = [
            // The kept client has no folder: the folder's client takes its ID.
            Choice {
                from: uuid_of(&conn, "6RFA1001"),
                into: uuid_of(&conn, "7974_6RFA"),
                new_id: None,
            },
            // Merged under the ID of the one that goes.
            Choice {
                from: uuid_of(&conn, "3002"),
                into: uuid_of(&conn, "3001"),
                new_id: Some("3002".into()),
            },
            // A plain merge.
            Choice {
                from: uuid_of(&conn, "5002"),
                into: uuid_of(&conn, "5001"),
                new_id: None,
            },
        ];
        let out = merge_many(
            &mut conn,
            &root,
            "dev",
            &choices,
            &AtomicUsize::new(0),
            &AtomicBool::new(false),
        );
        let ran: Vec<Ran> = out
            .iter()
            .map(|(_, r)| match r {
                Outcome::Merged(ran) => (**ran).clone(),
                r => panic!("{r:?}"),
            })
            .collect();
        assert_eq!(live(&conn), ["3002", "5001", "7974_6RFA"]);

        let folders = |c: &Connection| -> Vec<(String, Option<String>)> {
            let mut st = c
                .prepare(
                    "SELECT ext_id, folder_rel FROM client WHERE deleted_at IS NULL ORDER BY ext_id",
                )
                .unwrap();
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        // What Open cabinet runs, on the other PC and on this one.
        crate::rebuild::sync_clients(&mut other, &root, "dev2", "clients_import")
            .expect("the other PC's catalog takes the clients");
        assert_eq!(folders(&other), folders(&conn));
        crate::rebuild::index(&mut other, &root, "dev2").expect("the other PC opens");
        assert_eq!(folders(&other), folders(&conn));
        crate::rebuild::index(&mut conn, &root, "dev").expect("this PC opens");
        assert_eq!(live(&conn), ["3002", "5001", "7974_6RFA"]);
        // A catalog made new sees the same.
        let mut fresh = vf_core::db::open(&tmp.path().join("pc3.db"), Storage::Local).unwrap();
        crate::rebuild::index(&mut fresh, &root, "dev3").expect("a new catalog opens");
        assert_eq!(folders(&fresh), folders(&conn));

        // Put back here; the other PC takes that too.
        let back = undo_many(&mut conn, &root, "dev", &ran);
        assert!(back.iter().all(|(_, r)| r.is_ok()), "{back:?}");
        let all = ["3001", "3002", "5001", "5002", "6RFA1001", "7974_6RFA"];
        assert_eq!(live(&conn), all);
        crate::rebuild::index(&mut other, &root, "dev2").expect("the other PC opens again");
        assert_eq!(folders(&other), folders(&conn));
        crate::rebuild::index(&mut conn, &root, "dev").expect("this PC opens again");
        assert_eq!(live(&conn), all);
    }

    /// Owner 2026-11-18: after merges on one PC, another PC listed the whole cabinet (8,000
    /// client folders) to take them. It reads the folders that changed, and no others: the
    /// renamed folders under their new and old names, and those the cabinet no longer has.
    #[test]
    fn another_pc_takes_the_merges_by_reading_only_the_folders_that_changed() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, mut conn) = cabinet(
            tmp.path(),
            &[
                ("6 R Farms LLC", "6RFA1001"),
                ("Baker Co", "3001"),
                ("Baker Co.", "3002"),
                ("Cole Trust", "5001"),
                ("Cole Trust.", "5002"),
                ("Untouched One", "8001"),
                ("Untouched Two", "8002"),
            ],
            &[],
        );
        let listed = Uuid::new_v4();
        let mut list = clients::load(&root).unwrap().list.unwrap();
        list.uuids.insert("7974_6RFA".into(), listed);
        list.clients.push(Client {
            id: "7974_6RFA".into(),
            uuid: listed,
            name: "6 R Farms, LLC".into(),
            extra: BTreeMap::new(),
        });
        clients::save_list(&root, &list).unwrap();
        crate::rebuild::sync_clients(&mut conn, &root, "dev", "clients_import").unwrap();
        let mut other = vf_core::db::open(&tmp.path().join("pc2.db"), Storage::Local).unwrap();
        crate::rebuild::rebuild(&mut other, &root, "dev2").unwrap();
        // Intake's folder index, as its scan of the root writes it.
        let scan = |conn: &Connection, at: &str| {
            let mut st = conn
                .prepare(
                    "SELECT ext_id, folder_rel FROM client
                     WHERE deleted_at IS NULL AND folder_rel IS NOT NULL",
                )
                .unwrap();
            let folders: Vec<serde_json::Value> = st
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .unwrap()
                .map(|r| {
                    let (id, f) = r.unwrap();
                    serde_json::json!({ "path": root.join(f), "client_id": id, "status": "bound" })
                })
                .collect();
            let f = serde_json::json!({ "root": root, "scanned_at": at, "folders": folders });
            std::fs::write(root.join("_Vibe/folders.json"), f.to_string()).unwrap();
        };
        scan(&conn, "2026-11-17T08:00:00Z");
        assert!(!crate::rebuild::behind_folder_index(&other, &root));

        let choices = [
            Choice {
                from: uuid_of(&conn, "6RFA1001"),
                into: uuid_of(&conn, "7974_6RFA"),
                new_id: None,
            },
            Choice {
                from: uuid_of(&conn, "3002"),
                into: uuid_of(&conn, "3001"),
                new_id: Some("BAKE7".into()),
            },
            Choice {
                from: uuid_of(&conn, "5002"),
                into: uuid_of(&conn, "5001"),
                new_id: None,
            },
        ];
        let out = merge_many(
            &mut conn,
            &root,
            "dev",
            &choices,
            &AtomicUsize::new(0),
            &AtomicBool::new(false),
        );
        assert!(
            out.iter().all(|(_, r)| matches!(r, Outcome::Merged(_))),
            "{out:?}"
        );
        scan(&conn, "2026-11-18T08:00:00Z");

        let rows = |c: &Connection| -> Vec<(String, Option<String>)> {
            let mut st = c
                .prepare(
                    "SELECT ext_id, folder_rel FROM client WHERE deleted_at IS NULL ORDER BY ext_id",
                )
                .unwrap();
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        let docs = |c: &Connection| -> Vec<(String, String)> {
            let mut st = c
                .prepare(
                    "SELECT d.rel_path, coalesce(c.ext_id, '') FROM document d
                     LEFT JOIN client c ON c.id = d.client_id AND c.deleted_at IS NULL
                     WHERE d.deleted_at IS NULL ORDER BY d.rel_path",
                )
                .unwrap();
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert!(crate::rebuild::behind_folder_index(&other, &root));
        let behind = crate::rebuild::folders_behind(&other, &root);
        assert_eq!(
            behind,
            [
                "6 R Farms LLC (6RFA1001)",
                "6 R Farms, LLC (7974_6RFA)",
                "Baker Co (3001)",
                "Baker Co (BAKE7)",
                "Baker Co. (3002)",
                "Cole Trust. (5002)",
            ],
            "the folders that changed, and neither untouched one"
        );
        assert_eq!(
            crate::rebuild::take_folders_behind(&mut other, &root, "dev2").unwrap(),
            behind.len()
        );
        assert_eq!(crate::rebuild::unfinished_label(&other), None);
        assert_eq!(rows(&other), rows(&conn));
        // The folder that was merged into kept its name and its ID: the file it gained is
        // found as any file put in a folder is (when the client is opened, or the sweep).
        let gained = "Cole Trust (5001)/Income Tax/2025/return (2).pdf";
        let but_gained = |c: &Connection| -> Vec<(String, String)> {
            docs(c).into_iter().filter(|(p, _)| p != gained).collect()
        };
        assert_eq!(docs(&other), but_gained(&conn));
        crate::rebuild::reread_folders(&mut other, &root, "dev2", &["Cole Trust (5001)".into()])
            .unwrap();
        assert_eq!(docs(&other), docs(&conn));
        assert_eq!(docs(&other).len(), 7, "{:?}", docs(&other));
        assert!(docs(&other).iter().all(|(_, client)| !client.is_empty()));
        // In line: nothing more to read, now or after Intake's next scan.
        assert!(crate::rebuild::folders_behind(&other, &root).is_empty());
        scan(&conn, "2026-11-19T08:00:00Z");
        assert!(!crate::rebuild::behind_folder_index(&other, &root));
    }

    /// Owner 2026-11-17: an ID neither has can be typed; the two are merged and the one
    /// that stays is given it, its folder renamed. The undo gives each its own again.
    #[test]
    fn two_clients_are_merged_under_a_new_id() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, mut conn) = cabinet(
            tmp.path(),
            &[
                ("Baker Co", "3001"),
                ("Baker Co.", "3002"),
                ("Other", "9000"),
            ],
            &[],
        );
        let (a, b) = (uuid_of(&conn, "3001"), uuid_of(&conn, "3002"));
        let choice = |id: &str| Choice {
            from: b.clone(),
            into: a.clone(),
            new_id: Some(id.into()),
        };
        // Another client's ID is refused before anything is done; one of the two's own is not.
        let p = preview(&conn, &root, &[choice("9000")]).unwrap();
        assert!(
            p[0].1.as_ref().is_err_and(|e| e.contains("ID already")),
            "{:?}",
            p[0].1
        );
        let p = preview(&conn, &root, &[choice("3002")]).unwrap();
        assert!(p[0].1.is_ok(), "{:?}", p[0].1);
        let p = preview(&conn, &root, &[choice("BAKE7777")]).unwrap();
        assert_eq!(p[0].1.as_ref().unwrap().new_id.as_deref(), Some("BAKE7777"));

        let out = merge_many(
            &mut conn,
            &root,
            "dev",
            &[choice("BAKE7777")],
            &AtomicUsize::new(0),
            &AtomicBool::new(false),
        );
        let Outcome::Merged(ran) = &out[0].1 else {
            panic!("{:?}", out[0].1);
        };
        assert!(ran.reid.is_some(), "{:?}", ran.merged.note);
        assert_eq!(live(&conn), ["9000", "BAKE7777"]);
        let folder = root.join("Baker Co (BAKE7777)/Income Tax/2025");
        assert!(folder.join("return.pdf").is_file());
        assert!(
            folder.join("return (2).pdf").is_file(),
            "both clients' files"
        );
        assert!(!root.join("Baker Co (3001)").exists() && !root.join("Baker Co. (3002)").exists());

        let back = undo_many(&mut conn, &root, "dev", &[(**ran).clone()]);
        assert!(back.iter().all(|(_, r)| r.is_ok()), "{back:?}");
        assert_eq!(live(&conn), ["3001", "3002", "9000"]);
        assert!(
            root.join("Baker Co (3001)/Income Tax/2025/return.pdf")
                .is_file()
        );
        assert!(
            root.join("Baker Co. (3002)/Income Tax/2025/return.pdf")
                .is_file()
        );
    }

    /// The firm's list marks the one to keep: its IDs, compared as IDs are.
    #[test]
    fn the_ids_on_a_list_are_read_for_marking_the_one_to_keep() {
        let tmp = tempfile::tempdir().unwrap();
        let csv = tmp.path().join("firm.csv");
        std::fs::write(
            &csv,
            "Client Name,Client ID\r\n\"6 R Farms, LLC\",7974_6RFA\r\nOther,00-12\r\n",
        )
        .unwrap();
        let ids = ids_on_list(&csv).unwrap();
        assert!(ids.contains(&id_key("7974-6rfa")) && ids.contains(&id_key("12")));
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn a_run_merges_a_chain_goes_past_a_refusal_and_is_put_back() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, mut conn) = cabinet(
            tmp.path(),
            &[
                ("Baker Co", "3001"),
                ("Baker Co.", "3002"),
                ("Baker Company", "3003"),
                ("Cole Trust", "6001"),
                ("Cole Trust", "6002"),
            ],
            &["3001", "3002", "3003", "6001", "6002"],
        );
        let id = |conn: &Connection, e: &str| uuid_of(conn, e);
        let (a, b, c) = (id(&conn, "3001"), id(&conn, "3002"), id(&conn, "3003"));
        let (d, e) = (id(&conn, "6001"), id(&conn, "6002"));
        // A file in 6002's folder the catalog does not have: that merge is refused.
        std::fs::write(root.join("Cole Trust (6002)/stray.pdf"), "not cataloged").unwrap();
        let choices = vec![
            Choice {
                from: b.clone(),
                into: a.clone(),
                new_id: None,
            }, // 3002 into 3001
            Choice {
                from: e.clone(),
                into: d.clone(),
                new_id: None,
            }, // refused
            Choice {
                from: b.clone(),
                into: c.clone(),
                new_id: None,
            }, // 3002 is 3001 now: 3001 into 3003
            Choice {
                from: a.clone(),
                into: c.clone(),
                new_id: None,
            }, // one client already
        ];
        let p = preview(&conn, &root, &choices).unwrap();
        assert!(p[0].1.is_ok() && p[1].1.is_err(), "{p:?}");
        assert!(p[2].1.as_ref().is_ok_and(|x| x.after_another), "{p:?}");
        assert!(p[3].1.is_err());
        assert_eq!(live(&conn).len(), 5, "a preview writes nothing");

        let (done, stop) = (AtomicUsize::new(0), AtomicBool::new(false));
        let out = merge_many(&mut conn, &root, "dev", &choices, &done, &stop);
        assert!(matches!(out[0].1, Outcome::Merged(_)), "{:?}", out[0].1);
        assert!(
            matches!(&out[1].1, Outcome::Failed(e) if e.contains("not in the catalog")),
            "{:?}",
            out[1].1
        );
        assert!(matches!(out[2].1, Outcome::Merged(_)), "{:?}", out[2].1);
        assert!(matches!(out[3].1, Outcome::Skipped(_)), "{:?}", out[3].1);
        assert_eq!(done.load(Relaxed), 4);
        assert_eq!(live(&conn), ["3003", "6001", "6002"]);
        let docs: i64 = conn
            .query_row(
                "SELECT count(*) FROM document WHERE client_id = ?1 AND deleted_at IS NULL",
                [&c],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(docs, 3, "all three returns are the kept client's");
        assert!(
            root.join("Baker Company (3003)/Income Tax/2025/return (3).pdf")
                .is_file()
        );
        assert!(!root.join("Baker Co (3001)").exists());

        // Put back, the last merge first: three clients and their folders again.
        let merged: Vec<Ran> = out
            .into_iter()
            .filter_map(|(_, o)| match o {
                Outcome::Merged(m) => Some(*m),
                _ => None,
            })
            .collect();
        let back = undo_many(&mut conn, &root, "dev", &merged);
        assert!(back.iter().all(|(_, r)| r.is_ok()), "{back:?}");
        assert_eq!(live(&conn), ["3001", "3002", "3003", "6001", "6002"]);
        for f in [
            "Baker Co (3001)",
            "Baker Co. (3002)",
            "Baker Company (3003)",
        ] {
            assert!(
                root.join(f).join("Income Tax/2025/return.pdf").is_file(),
                "{f}"
            );
        }

        // QA 2026-11-17: an undo that cannot be made stops the rest (they may hold its
        // documents): the earlier merge of the chain is not put back as an empty client.
        let out = merge_many(&mut conn, &root, "dev", &choices, &done, &stop);
        let merged: Vec<Ran> = out
            .into_iter()
            .filter_map(|(_, o)| match o {
                Outcome::Merged(m) => Some(*m),
                _ => None,
            })
            .collect();
        assert_eq!(merged.len(), 2);
        // The folder of the last one merged away is in the cabinet again.
        std::fs::create_dir_all(root.join("Baker Co (3001)")).unwrap();
        let back = undo_many(&mut conn, &root, "dev", &merged);
        assert_eq!(back.len(), 1, "stopped at the first: {back:?}");
        assert!(back[0].1.is_err());
        assert_eq!(
            live(&conn),
            ["3003", "6001", "6002"],
            "nothing half put back"
        );
        std::fs::remove_dir(root.join("Baker Co (3001)")).unwrap();
        let back = undo_many(&mut conn, &root, "dev", &merged);
        assert!(back.iter().all(|(_, r)| r.is_ok()), "{back:?}");
        assert_eq!(live(&conn).len(), 5);

        // Stopped: nothing more is merged.
        stop.store(true, Relaxed);
        let out = merge_many(&mut conn, &root, "dev", &choices[..1], &done, &stop);
        assert!(matches!(out[0].1, Outcome::Skipped(_)));
        assert_eq!(live(&conn).len(), 5);
    }
}
