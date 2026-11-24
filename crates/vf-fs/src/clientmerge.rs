//! Merge one client into another (owner 2026-11-14, Q-107): two clients that are the same
//! person or business, made twice under similar names, become one folder.
//!
//! [`plan`] is the preview and writes nothing. [`apply`] then, under the cabinet's client
//! lock:
//!
//! 1. In one Transaction: every document of the client merged away moves to the same place
//!    below the kept client's folder (Filer's move engine, never replacing a file: a name
//!    taken there keeps both, ` (2)`), with its catalog row, its record in the kept client's
//!    `docmeta.jsonl` (same id, doc type, year and tags on every PC) and the paths of its
//!    documents in the trash; the emptied folders are removed; the merged client's own
//!    `_Vibe` folder (sentinel and records) is set aside in
//!    `_Vibe/client-merges/<client>-<secs>/`.
//! 2. The client is taken off the client list and out of the client CSV; its relationships
//!    and legal holds become the kept client's.
//! 3. What was done is written to `_Vibe/client-merges/<tx>.json`. [`undo`] works from that
//!    file alone, on any PC (a session host's catalog is new every morning).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use filer_core::clients::Client;
use filer_core::relationships::Relationship;
use serde::{Deserialize, Serialize};
use serde_json::json;
use vf_core::audit::AuditEntry;
use vf_core::db::Connection;
use vf_core::tx::{FileOp, Stmt, Transaction};

use crate::clients::Share;
use crate::{cabinet, clients, docmeta, holds, rebuild};

pub const LABEL: &str = "client_merge";
pub const UNDO_LABEL: &str = "client_merge_undo";
const DIR: &str = "client-merges";

/// A client in a merge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Who {
    /// Client uuid (the sentinel's `client_id`).
    pub id: String,
    pub ext_id: String,
    pub name: String,
    /// Its folder, cabinet-relative.
    pub folder: Option<String>,
}

/// One document going to the kept client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Move {
    pub doc_id: String,
    pub from: String,
    pub to: String,
    /// The name was taken in the kept client's folder: this one got ` (2)`.
    pub kept_both: bool,
}

/// What merging `from` into `into` does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub from: Who,
    pub into: Who,
    pub moves: Vec<Move>,
    /// Documents of `from` in the trash: (id, path now, path after).
    pub trashed: Vec<(String, String, String)>,
    /// The folders of `from` (cabinet-relative, deepest first, its own last), removed.
    pub dirs: Vec<String>,
    /// Whether `from` has a `_Vibe` folder of its own to set aside.
    pub own_records: bool,
    /// Active legal holds on `from` that become holds on `into`.
    pub holds: usize,
    /// Relationships of `from` (before leaving out the ones `into` already has).
    pub relationships: usize,
}

impl Plan {
    pub fn kept_both(&self) -> usize {
        self.moves.iter().filter(|m| m.kept_both).count()
    }

    /// The documents that move (the client's notes files move too, and are not counted).
    pub fn documents(&self) -> usize {
        self.moves.iter().filter(|m| !m.is_notes()).count()
    }
}

impl Move {
    /// A notes file (`!fcinfo.ini`) going with its client: a file only, no document.
    pub fn is_notes(&self) -> bool {
        self.doc_id.is_empty()
    }
}

impl Merged {
    /// The documents that moved (see [`Plan::documents`]).
    pub fn documents(&self) -> usize {
        self.moves.iter().filter(|m| !m.is_notes()).count()
    }
}

/// What a merge did: the record its undo works from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Merged {
    pub tx_id: String,
    pub when: String,
    #[serde(default)]
    pub by: Option<String>,
    pub from: Who,
    pub into: Who,
    pub moves: Vec<Move>,
    #[serde(default)]
    pub trashed: Vec<(String, String, String)>,
    #[serde(default)]
    pub dirs: Vec<String>,
    /// Where the merged client's `_Vibe` folder went (cabinet-relative).
    #[serde(default)]
    pub set_aside: Option<String>,
    /// Its entry in the client list.
    #[serde(default)]
    pub client: Option<Client>,
    #[serde(default)]
    pub rels_removed: Vec<Relationship>,
    #[serde(default)]
    pub rels_added: Vec<Relationship>,
    #[serde(default)]
    pub shares_removed: Vec<Share>,
    #[serde(default)]
    pub shares_added: Vec<Share>,
    /// Its entity type, taxpayer-ID digits, partner and B2 choice in `clients.vf.json`.
    #[serde(default)]
    pub details: BTreeMap<String, String>,
    /// Legal holds (ids) that were on the merged client.
    #[serde(default)]
    pub holds: Vec<String>,
    /// Its row in the client CSV, as it was.
    #[serde(default)]
    pub csv_row: Option<String>,
    /// Something the person should know (the CSV could not be changed).
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub undone: Option<String>,
}

fn dir(root: &Path) -> PathBuf {
    root.join(cabinet::VIBE_DIR).join(DIR)
}

/// The cabinet's last merge (for Undo), when it has not been undone.
pub fn load_last(root: &Path) -> Option<Merged> {
    // The newest record decides (their names are their transaction ids, which sort by
    // time). When that one cannot be read, which merge is the last is not known and none is
    // offered: an older one used to be offered as "the last merge". An older record that
    // cannot be read hides nothing.
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir(root))
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    let newest: Merged = serde_json::from_slice(&std::fs::read(files.pop()?).ok()?).ok()?;
    Some(newest).filter(|m| m.undone.is_none())
}

/// Writes the merge's record. Not through [`cabinet::write_replace`]: the record is this
/// merge's own file, and it must be written whatever became of the cabinet lock meanwhile
/// (without it a merge that has moved the documents could not be undone).
fn save(root: &Path, m: &Merged) -> Result<(), String> {
    let file = dir(root).join(format!("{}.json", m.tx_id));
    let bytes = serde_json::to_vec_pretty(m).expect("serializes");
    std::fs::create_dir_all(dir(root))
        .and_then(|()| {
            let tmp = file.with_extension("json.tmp");
            std::fs::write(&tmp, &bytes).and_then(|()| std::fs::rename(&tmp, &file))
        })
        // A share that will not rename: written in place.
        .or_else(|_| std::fs::write(&file, &bytes))
        .map_err(|e| format!("cannot write {}: {e}", file.display()))
}

fn who(conn: &Connection, id: &str) -> Result<Who, String> {
    conn.query_row(
        "SELECT id, ext_id, name, folder_rel FROM client WHERE id = ?1 AND deleted_at IS NULL",
        [id],
        |r| {
            Ok(Who {
                id: r.get(0)?,
                ext_id: r.get(1)?,
                name: r.get(2)?,
                folder: r.get::<_, Option<String>>(3)?.filter(|f| !f.is_empty()),
            })
        },
    )
    .map_err(|e| vf_core::db::missing_or(e, "That client is not in the catalog."))
}

/// Every file and folder under `dir`, cabinet-relative with `/` (`base` is `dir`'s own
/// cabinet-relative path): (files, folders deepest first).
fn walk(dir: &Path, base: &str) -> Result<(Vec<String>, Vec<String>), String> {
    fn go(
        d: &Path,
        rel: &str,
        files: &mut Vec<String>,
        dirs: &mut Vec<String>,
    ) -> Result<(), String> {
        for e in std::fs::read_dir(d).map_err(|e| format!("{}: {e}", d.display()))? {
            let e = e.map_err(|e| e.to_string())?;
            let r = format!("{rel}/{}", e.file_name().to_string_lossy());
            if e.file_type().map_err(|e| e.to_string())?.is_dir() {
                go(&e.path(), &r, files, dirs)?;
                dirs.push(r);
            } else {
                files.push(r);
            }
        }
        Ok(())
    }
    let (mut files, mut dirs) = (vec![], vec![]);
    go(dir, base, &mut files, &mut dirs)?;
    Ok((files, dirs))
}

/// `rel` is inside folder `folder` (any case).
fn under(rel: &str, folder: &str) -> bool {
    rel.len() > folder.len()
        && rel.as_bytes()[folder.len()] == b'/'
        && rel.is_char_boundary(folder.len())
        && rel[..folder.len()].eq_ignore_ascii_case(folder)
}

fn few(names: &[String]) -> String {
    let mut s = names.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
    if names.len() > 3 {
        s.push_str(&format!(" and {} more", names.len() - 3));
    }
    s
}

/// What merging client `from` into client `into` (uuids) would do. Nothing is written.
pub fn plan(conn: &Connection, root: &Path, from: &str, into: &str) -> Result<Plan, String> {
    if from == into {
        return Err("Pick another client to merge into.".into());
    }
    let (from, into) = (who(conn, from)?, who(conn, into)?);
    let Some(into_folder) = into.folder.clone() else {
        return Err(format!("{} has no folder yet: create it first.", into.name));
    };
    if !root.join(&into_folder).is_dir() {
        return Err(format!("The folder {into_folder} is not on disk."));
    }
    let vibe = cabinet::VIBE_DIR;
    let (mut on_disk, mut dirs, mut own_records) = (vec![], vec![], false);
    let mut notes: Vec<String> = vec![];
    if let Some(f) = &from.folder {
        if !root.join(f).is_dir() {
            return Err(format!("The folder {f} is not on disk."));
        }
        let (files, all) = walk(&root.join(f), f)?;
        let own = format!("{f}/{vibe}");
        own_records = root.join(&own).is_dir();
        let mine = |r: &String| r.eq_ignore_ascii_case(&own) || under(r, &own);
        // The client's notes files (FileCenter's `!fcinfo.ini`, PA-76) are never documents,
        // so never "in the catalog": they go with the client, moved like its documents
        // below. Counted with the files "not in the catalog yet", a client with notes could
        // never be merged, and Refresh (the advice given) never changed that (QA 2026-11-17).
        let is_notes = |r: &String| crate::fcinfo::is_note_file(r.rsplit('/').next().unwrap_or(r));
        let (n, rest): (Vec<String>, Vec<String>) =
            files.into_iter().filter(|r| !mine(r)).partition(is_notes);
        notes = n;
        on_disk = rest;
        dirs = all.into_iter().filter(|r| !mine(r)).collect();
        dirs.push(f.clone());
    }
    // The client's documents: live ones move, the ones in the trash take the new path.
    let mut live: Vec<(String, String)> = vec![];
    let mut trashed = vec![];
    {
        let mut st = conn
            .prepare(
                "SELECT id, rel_path, deleted_at IS NOT NULL FROM document WHERE client_id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let rows = st
            .query_map([&from.id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, bool>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        for row in rows {
            let (id, rel, gone) = row.map_err(|e| e.to_string())?;
            if !gone {
                live.push((id, rel));
            } else if let Some(f) = from.folder.as_deref().filter(|f| under(&rel, f)) {
                let to = format!("{into_folder}{}", &rel[f.len()..]);
                trashed.push((id, rel, to));
            } else {
                trashed.push((id, rel.clone(), rel));
            }
        }
    }
    live.sort_by_key(|a| a.1.to_lowercase());
    let locked: Option<String> = conn
        .query_row(
            "SELECT l.user FROM document d JOIN lock l ON l.doc_id = d.id
             WHERE d.client_id = ?1 AND d.deleted_at IS NULL LIMIT 1",
            [&from.id],
            |r| r.get(0),
        )
        .ok();
    if let Some(u) = locked {
        return Err(format!(
            "A document of {} is checked out by {u}.",
            from.name
        ));
    }
    // The folder and the catalog must agree, or files would be left behind (or lost track of).
    let cataloged: HashSet<String> = live.iter().map(|(_, r)| r.to_lowercase()).collect();
    let strangers: Vec<String> = on_disk
        .iter()
        .filter(|r| !cataloged.contains(&r.to_lowercase()))
        .cloned()
        .collect();
    if !strangers.is_empty() {
        return Err(format!(
            "{} file{} in {}'s folder {} not in the catalog yet ({}). Open the client in Browse and press Refresh, then merge.",
            strangers.len(),
            if strangers.len() == 1 { "" } else { "s" },
            from.name,
            if strangers.len() == 1 { "is" } else { "are" },
            few(&strangers)
        ));
    }
    let there: HashSet<String> = on_disk.iter().map(|r| r.to_lowercase()).collect();
    let missing: Vec<String> = live
        .iter()
        .filter(|(_, r)| from.folder.as_deref().is_some_and(|f| under(r, f)))
        .filter(|(_, r)| !there.contains(&r.to_lowercase()))
        .map(|(_, r)| r.clone())
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "{} document{} of {} {} not on disk any more ({}). Open the client in Browse and press Refresh, then merge.",
            missing.len(),
            if missing.len() == 1 { "" } else { "s" },
            from.name,
            if missing.len() == 1 { "is" } else { "are" },
            few(&missing)
        ));
    }
    // Names taken in the kept client's folder (files and folders), and by this merge.
    let mut taken: HashSet<String> = {
        let (files, folders) = walk(&root.join(&into_folder), &into_folder)?;
        files
            .into_iter()
            .chain(folders)
            .map(|r| r.to_lowercase())
            .collect()
    };
    let mut moves = Vec::with_capacity(live.len());
    for (id, rel) in live {
        let Some(f) = from.folder.as_deref().filter(|f| under(&rel, f)) else {
            // Not in the client's folder: only its client changes.
            moves.push(Move {
                doc_id: id,
                from: rel.clone(),
                to: rel,
                kept_both: false,
            });
            continue;
        };
        let tail = &rel[f.len() + 1..];
        let (sub, name) = match tail.rsplit_once('/') {
            Some((d, n)) => (format!("{into_folder}/{d}"), n),
            None => (into_folder.clone(), tail),
        };
        let free = crate::rename::free_name(name, |n| {
            taken.contains(&format!("{sub}/{n}").to_lowercase())
        });
        let to = format!("{sub}/{free}");
        let n = root.join(&to).as_os_str().len();
        if n > crate::folders::MAX_PATH {
            return Err(format!(
                "{to} would be {n} characters long (limit {}); shorten the file's or the folder's name first.",
                crate::folders::MAX_PATH
            ));
        }
        taken.insert(to.to_lowercase());
        moves.push(Move {
            doc_id: id,
            kept_both: free != name,
            from: rel,
            to,
        });
    }
    // The notes files, to the same place in the kept client's folder; a name taken there
    // (the kept client has notes too) keeps both. No document id: they are files only.
    if let Some(f) = from.folder.as_deref() {
        for rel in notes {
            let tail = &rel[f.len() + 1..];
            let (sub, name) = match tail.rsplit_once('/') {
                Some((d, n)) => (format!("{into_folder}/{d}"), n),
                None => (into_folder.clone(), tail),
            };
            let free = crate::rename::free_name(name, |n| {
                taken.contains(&format!("{sub}/{n}").to_lowercase())
            });
            let to = format!("{sub}/{free}");
            taken.insert(to.to_lowercase());
            moves.push(Move {
                doc_id: String::new(),
                kept_both: free != name,
                from: rel,
                to,
            });
        }
    }
    let held = holds::load(root)?
        .iter()
        .filter(|h| h.active() && h.client_id.as_deref() == Some(from.id.as_str()))
        .count();
    let relationships = if from.ext_id.is_empty() {
        0
    } else {
        clients::load(root)?
            .rels
            .records
            .iter()
            .filter(|r| {
                r.parent_id.eq_ignore_ascii_case(&from.ext_id)
                    || r.child_id.eq_ignore_ascii_case(&from.ext_id)
            })
            .count()
    };
    Ok(Plan {
        from,
        into,
        moves,
        trashed,
        dirs,
        own_records,
        holds: held,
        relationships,
    })
}

fn set_doc(id: &str, rel: &str, client: &str, device_id: &str, now: &str) -> Stmt {
    Stmt::new(
        "UPDATE document SET rel_path = ?2, client_id = ?3, device_id = ?4, updated_at = ?5,
           rev = rev + 1
         WHERE id = ?1",
        vec![
            id.into(),
            rel.into(),
            client.into(),
            device_id.into(),
            now.into(),
        ],
    )
}

/// Merges as planned. The plan is made again first: when the documents changed since the
/// preview, nothing is done.
pub fn apply(
    conn: &mut Connection,
    root: &Path,
    plan: &Plan,
    device_id: &str,
) -> Result<Merged, String> {
    crate::lock::with(root, "clients", "the client list", || {
        clients::ensure_listed(conn, root)?;
        let fresh = self::plan(conn, root, &plan.from.id, &plan.into.id)?;
        if &fresh != plan {
            return Err(
                "The clients' documents changed since the preview. Look at it again, then merge."
                    .into(),
            );
        }
        // The CSV must take the change too, or the client is back at the next import.
        clients::intake_csv_reachable(root).map_err(|e| {
            // Intake's settings unreadable is said as that; else the CSV is out of reach.
            if e.starts_with("Intake's settings") {
                e
            } else {
                "The client CSV Intake uses cannot be reached from this PC: the merged client must be taken out of it too.".to_string()
            }
        })?;
        let (from, into) = (&plan.from, &plan.into);
        let into_folder = into.folder.clone().expect("planned with a folder");
        let now = vf_core::now_rfc3339();
        let mut tx = Transaction::new(LABEL);
        let mut lines: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
        for m in &plan.moves {
            // A notes file: moved, and nothing of the catalog or the records to change.
            if m.doc_id.is_empty() {
                tx = tx.file(FileOp::Move {
                    src: root.join(&m.from),
                    dst: root.join(&m.to),
                });
                continue;
            }
            let meta = docmeta::load(conn, &m.doc_id)?;
            if m.from != m.to {
                tx = tx.file(FileOp::Move {
                    src: root.join(&m.from),
                    dst: root.join(&m.to),
                });
            }
            // The kept client's records take it: the same document on every PC.
            let there = docmeta::Meta {
                rel_path: m.to.clone(),
                folder_rel: Some(into_folder.clone()),
                ..meta
            };
            let (file, line) = docmeta::record_line(root, &there, &now, device_id);
            lines.entry(file).or_default().push(line);
            tx = tx
                .catalog(set_doc(&m.doc_id, &m.to, &into.id, device_id, &now))
                .catalog_undo(set_doc(&m.doc_id, &m.from, &from.id, device_id, &now));
        }
        for (id, old, new) in &plan.trashed {
            tx = tx
                .catalog(set_doc(id, new, &into.id, device_id, &now))
                .catalog_undo(set_doc(id, old, &from.id, device_id, &now));
        }
        for (path, lines) in lines {
            tx = tx.file(FileOp::AppendLines {
                path,
                lines,
                undo_lines: vec![],
            });
        }
        // The merged client's own records set aside, then its emptied folders removed.
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let mut set_aside = None;
        if let Some(f) = from.folder.as_deref().filter(|_| plan.own_records) {
            let aside = format!("{}/{DIR}/{}-{secs}", cabinet::VIBE_DIR, from.id);
            tx = tx.file(FileOp::Move {
                src: root.join(f).join(cabinet::VIBE_DIR),
                dst: root.join(&aside).join(cabinet::VIBE_DIR),
            });
            set_aside = Some(aside);
        }
        for d in &plan.dirs {
            tx = tx.file(FileOp::Rmdir { path: root.join(d) });
        }
        let gone = |at: Option<&str>| {
            Stmt::new(
                "UPDATE client SET deleted_at = ?2, device_id = ?3, updated_at = ?4, rev = rev + 1
                 WHERE id = ?1",
                vec![
                    from.id.clone().into(),
                    at.into(),
                    device_id.into(),
                    now.clone().into(),
                ],
            )
        };
        tx = tx
            .catalog(gone(Some(&now)))
            .catalog_undo(gone(None))
            .audit(AuditEntry::new(
                LABEL,
                None,
                json!({
                    "from": from.ext_id, "from_folder": from.folder,
                    "into": into.ext_id, "into_folder": into.folder,
                    "documents": plan.documents(), "kept_both": plan.kept_both(),
                }),
            ));
        let done = crate::fileops::commit(conn, root, tx, device_id)?;

        // From here on the documents are with the kept client: the lists follow, and what
        // cannot follow is said, never a reason to leave the merge half recorded.
        let mut m = Merged {
            tx_id: done.tx_id,
            when: now,
            by: vf_core::windows_login(),
            from: from.clone(),
            into: into.clone(),
            moves: plan.moves.clone(),
            trashed: plan.trashed.clone(),
            dirs: plan.dirs.clone(),
            set_aside,
            client: None,
            rels_removed: vec![],
            rels_added: vec![],
            shares_removed: vec![],
            shares_added: vec![],
            details: BTreeMap::new(),
            holds: vec![],
            csv_row: None,
            note: None,
            undone: None,
        };
        // The record first, as it stands: whatever fails from here on, the merge can be
        // undone (code review 2026-11-14).
        let mut notes: Vec<String> = vec![];
        if let Err(e) = save(root, &m) {
            notes.push(format!(
                "The merge's record could not be written ({e}): it cannot be undone from this window."
            ));
        }
        if let Err(e) = lists(root, &mut m) {
            notes.push(format!("The client list was not changed: {e}"));
        }
        let paths: HashMap<String, String> = m
            .moves
            .iter()
            .map(|x| (x.from.to_lowercase(), x.to.clone()))
            .collect();
        match holds::move_client(conn, root, device_id, &from.id, &into.id, &paths) {
            Ok(ids) => m.holds = ids,
            Err(e) => notes.push(format!("The legal holds were not changed: {e}")),
        }
        // Two clients whose IDs differ only in capitals: the CSV row is taken out only when
        // it is the merged client's own (written exactly so), never the kept client's.
        let kept_ones_row = from.ext_id.trim().eq_ignore_ascii_case(into.ext_id.trim())
            && !clients::csv_ids(root)
                .ok()
                .flatten()
                .is_some_and(|ids| ids.contains(from.ext_id.trim()));
        if !from.ext_id.is_empty() && !kept_ones_row {
            match clients::remove_from_csv(root, &from.ext_id) {
                Ok(row) => m.csv_row = row,
                Err(e) => notes.push(format!(
                    "{} is still in the client CSV ({e}): take the row out, or the client is back at the next import.",
                    from.ext_id
                )),
            }
        }
        // The documents have moved: what follows is said, never an error for the merge.
        if let Err(e) = rebuild::sync_clients_edited(conn, root, device_id, "clients_edit", &[]) {
            notes.push(format!(
                "The client list in this catalog was not brought up to date ({e}); it is at the next open."
            ));
        }
        if !notes.is_empty() {
            m.note = Some(notes.join(" "));
        }
        if let Err(e) = save(root, &m) {
            m.note = Some(format!(
                "{} The merge's record could not be completed ({e}).",
                m.note.clone().unwrap_or_default()
            ));
        }
        Ok(m)
    })
}

fn same_rel(a: &Relationship, b: &Relationship) -> bool {
    a.parent_id.eq_ignore_ascii_case(&b.parent_id)
        && a.child_id.eq_ignore_ascii_case(&b.child_id)
        && a.role.eq_ignore_ascii_case(&b.role)
}

fn same_share(a: &Share, b: &Share) -> bool {
    a.parent_id.eq_ignore_ascii_case(&b.parent_id)
        && a.child_id.eq_ignore_ascii_case(&b.child_id)
        && a.role.eq_ignore_ascii_case(&b.role)
}

/// The client list, relationships and details after the merge (recorded in `m`).
fn lists(root: &Path, m: &mut Merged) -> Result<(), String> {
    let mut master = clients::load(root)?;
    if let Some(mut list) = master.list.take()
        && let Some(i) = list
            .clients
            .iter()
            .position(|c| c.uuid.to_string() == m.from.id)
    {
        m.client = Some(list.clients.remove(i));
        list.uuids.retain(|_, v| v.to_string() != m.from.id);
        clients::save_list(root, &list)?;
    }
    let (old, new) = (m.from.ext_id.clone(), m.into.ext_id.clone());
    if old.is_empty() {
        return Ok(());
    }
    let is_old = |x: &str| x.eq_ignore_ascii_case(&old);
    let swap = |x: &str| {
        if is_old(x) {
            new.clone()
        } else {
            x.to_string()
        }
    };
    // Relationships: the kept client takes them, but for one with itself or one it has.
    let mut rels = master.rels.clone();
    let (mine, mut rest): (Vec<Relationship>, Vec<Relationship>) = rels
        .records
        .drain(..)
        .partition(|r| is_old(&r.parent_id) || is_old(&r.child_id));
    if !mine.is_empty() {
        for r in &mine {
            let moved = Relationship {
                parent_id: swap(&r.parent_id),
                child_id: swap(&r.child_id),
                ..r.clone()
            };
            if new.is_empty()
                || moved.parent_id.eq_ignore_ascii_case(&moved.child_id)
                || rest.iter().any(|x| same_rel(x, &moved))
            {
                continue;
            }
            rest.push(moved.clone());
            m.rels_added.push(moved);
        }
        m.rels_removed = mine;
        rels.records = rest;
        clients::save_rels(root, &rels)?;
    }
    let mut meta = master.meta.clone();
    let (mine, mut rest): (Vec<Share>, Vec<Share>) = meta
        .shares
        .drain(..)
        .partition(|r| is_old(&r.parent_id) || is_old(&r.child_id));
    for r in &mine {
        let moved = Share {
            parent_id: swap(&r.parent_id),
            child_id: swap(&r.child_id),
            ..r.clone()
        };
        if new.is_empty()
            || moved.parent_id.eq_ignore_ascii_case(&moved.child_id)
            || rest.iter().any(|x| same_share(x, &moved))
        {
            continue;
        }
        rest.push(moved.clone());
        m.shares_added.push(moved);
    }
    m.shares_removed = mine;
    meta.shares = rest;
    for (key, map) in [
        ("entity_type", &mut meta.entity_types),
        ("tin_last4", &mut meta.tin_last4),
        ("partner", &mut meta.partners),
        ("b2", &mut meta.b2_clients),
    ] {
        if let Some(v) = map.remove(&m.from.id) {
            m.details.insert(key.into(), v);
        }
    }
    // Its Portal contacts (PA-105): kept with the merge record, and given to the kept client
    // when it has none (review 2026-11-24).
    if let Some(rec) = meta.contacts.remove(&m.from.id) {
        m.details.insert(
            "contacts".into(),
            serde_json::to_string(&rec).unwrap_or_default(),
        );
        if !meta.contacts.contains_key(&m.into.id) {
            meta.contacts.insert(m.into.id.clone(), rec);
            m.details.insert("contacts_to_kept".into(), "1".into());
        }
    }
    if meta != master.meta {
        clients::save_meta(root, &meta)?;
    }
    Ok(())
}

/// Puts a merge back, from its record alone: the documents still where the merge put them go
/// back to the merged client's folder, with its own records, its place on the client list and
/// in the CSV, its relationships and its legal holds. A document moved or renamed since stays
/// with the kept client. Returns how many documents went back.
pub fn undo(
    conn: &mut Connection,
    root: &Path,
    m: &Merged,
    device_id: &str,
) -> Result<usize, String> {
    undo_noted(conn, root, m, device_id).map(|(back, _)| back)
}

/// [`undo`], with what could not be put back as well once the documents were (the client's
/// place on the list, its relationships, its holds): to tell the person.
pub fn undo_noted(
    conn: &mut Connection,
    root: &Path,
    m: &Merged,
    device_id: &str,
) -> Result<(usize, Option<String>), String> {
    crate::lock::with(root, "clients", "the client list", || {
        if m.undone.is_some() {
            return Err("That merge was undone already.".into());
        }
        // The merged client's ID given to another client since (a kept client with no
        // folder took the other's folder and that one its ID, PA-95): putting it back
        // would make two clients of one ID.
        if !m.from.ext_id.trim().is_empty() {
            let holder: Option<String> = vf_core::db::row_or_none(conn.query_row(
                "SELECT name FROM client WHERE ext_id = ?1 COLLATE NOCASE AND id <> ?2
                   AND deleted_at IS NULL",
                [&m.from.ext_id, &m.from.id],
                |r| r.get(0),
            ))?;
            if let Some(name) = holder {
                return Err(format!(
                    "{} is {name}'s ID now: this merge cannot be undone by itself. Undo it where it was made (Admin › Similar clients or Clients from a list).",
                    m.from.ext_id
                ));
            }
        }
        let now = vf_core::now_rfc3339();
        let mut tx = Transaction::new(UNDO_LABEL);
        if let Some(f) = &m.from.folder {
            if root.join(f).exists() {
                return Err(format!(
                    "A folder named {f} is in the cabinet again: the merge cannot be undone into it."
                ));
            }
            match &m.set_aside {
                Some(aside) if root.join(aside).join(cabinet::VIBE_DIR).is_dir() => {
                    tx = tx.file(FileOp::Move {
                        src: root.join(aside).join(cabinet::VIBE_DIR),
                        dst: root.join(f).join(cabinet::VIBE_DIR),
                    });
                }
                _ => {
                    tx = tx.file(FileOp::Mkdir { path: root.join(f) });
                }
            }
        }
        // This catalog may never have known the client (a host new this morning).
        let known = vf_core::db::row_or_none(conn.query_row(
            "SELECT 1 FROM client WHERE id = ?1",
            [&m.from.id],
            |_| Ok(()),
        ))?
        .is_some();
        if !known {
            tx = tx.catalog(Stmt::new(
                "INSERT INTO client (id, ext_id, name, folder_rel, device_id, updated_at, deleted_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                vec![
                    m.from.id.clone().into(),
                    m.from.ext_id.clone().into(),
                    m.from.name.clone().into(),
                    m.from.folder.clone().into(),
                    device_id.into(),
                    now.clone().into(),
                ],
            ));
        }
        tx = tx.catalog(Stmt::new(
            "UPDATE client SET deleted_at = NULL, folder_rel = ?2, device_id = ?3,
               updated_at = ?4, rev = rev + 1
             WHERE id = ?1",
            vec![
                m.from.id.clone().into(),
                m.from.folder.clone().into(),
                device_id.into(),
                now.clone().into(),
            ],
        ));
        let mut back = 0;
        let mut made: HashSet<String> = HashSet::new();
        for x in &m.moves {
            if x.from != x.to {
                if !root.join(&x.to).is_file() || root.join(&x.from).exists() {
                    continue;
                }
                tx = tx.file(FileOp::Move {
                    src: root.join(&x.to),
                    dst: root.join(&x.from),
                });
                // Its folders come back with it.
                let mut d = x.from.as_str();
                while let Some((parent, _)) = d.rsplit_once('/') {
                    made.insert(parent.to_lowercase());
                    d = parent;
                }
            }
            back += 1;
            // By where it is, not by its id: any PC's catalog can undo the merge.
            tx = tx.catalog(Stmt::new(
                "UPDATE document SET rel_path = ?2, client_id = ?3, device_id = ?4,
                   updated_at = ?5, rev = rev + 1
                 WHERE deleted_at IS NULL AND rel_path = ?1",
                vec![
                    x.to.clone().into(),
                    x.from.clone().into(),
                    m.from.id.clone().into(),
                    device_id.into(),
                    now.clone().into(),
                ],
            ));
        }
        for (id, old, new) in &m.trashed {
            tx = tx.catalog(Stmt::new(
                "UPDATE document SET rel_path = ?2, client_id = ?3, rev = rev + 1
                 WHERE id = ?1 AND deleted_at IS NOT NULL AND rel_path = ?4",
                vec![
                    id.clone().into(),
                    old.clone().into(),
                    m.from.id.clone().into(),
                    new.clone().into(),
                ],
            ));
        }
        // The folders that were empty (the others came back with their files).
        for d in m.dirs.iter().rev() {
            if !made.contains(&d.to_lowercase()) && Some(d) != m.from.folder.as_ref() {
                tx = tx.file(FileOp::Mkdir { path: root.join(d) });
            }
        }
        tx = tx.audit(AuditEntry::new(
            UNDO_LABEL,
            None,
            json!({
                "from": m.from.ext_id, "into": m.into.ext_id, "merge": m.tx_id,
                "documents": back, "left_with_kept_client": m.moves.len() - back,
            }),
        ));
        crate::fileops::commit(conn, root, tx, device_id)?;

        // The documents are back: from here on nothing is an error for the undo. What
        // could not be put back as well is said, and the merge is marked undone whatever
        // happens (it used to stop at the first of these with a bare error, the undo still
        // offered, and the next press said "a folder named … is in the cabinet again";
        // QA 2026-11-17).
        let mut notes: Vec<String> = Vec::new();
        // The lists as they were, where they still can be.
        let lists = (|| -> Result<(), String> {
            let mut master = clients::load(root)?;
            if let Some(c) = &m.client {
                let mut list = master
                    .list
                    .take()
                    .unwrap_or_else(crate::newclient::empty_list);
                if !list.clients.iter().any(|x| x.uuid == c.uuid) {
                    list.uuids.insert(c.id.clone(), c.uuid);
                    list.clients.push(c.clone());
                    clients::save_list(root, &list)?;
                }
            }
            if !m.rels_removed.is_empty() {
                let mut rels = master.rels.clone();
                rels.records
                    .retain(|r| !m.rels_added.iter().any(|a| a == r));
                for r in &m.rels_removed {
                    if !rels.records.iter().any(|x| same_rel(x, r)) {
                        rels.records.push(r.clone());
                    }
                }
                clients::save_rels(root, &rels)?;
            }
            let mut meta = master.meta.clone();
            meta.shares
                .retain(|r| !m.shares_added.iter().any(|a| a == r));
            for r in &m.shares_removed {
                if !meta.shares.iter().any(|x| same_share(x, r)) {
                    meta.shares.push(r.clone());
                }
            }
            if let Some(rec) = m
                .details
                .get("contacts")
                .and_then(|j| serde_json::from_str::<crate::contacts::ContactRecord>(j).ok())
            {
                if m.details.contains_key("contacts_to_kept")
                    && meta.contacts.get(&m.into.id) == Some(&rec)
                {
                    meta.contacts.remove(&m.into.id);
                }
                meta.contacts.entry(m.from.id.clone()).or_insert(rec);
            }
            for (key, v) in &m.details {
                if key.starts_with("contacts") {
                    continue;
                }
                let map = match key.as_str() {
                    "entity_type" => &mut meta.entity_types,
                    "tin_last4" => &mut meta.tin_last4,
                    "partner" => &mut meta.partners,
                    _ => &mut meta.b2_clients,
                };
                map.entry(m.from.id.clone()).or_insert_with(|| v.clone());
            }
            if meta != master.meta {
                clients::save_meta(root, &meta)?;
            }
            Ok(())
        })();
        if let Err(e) = lists {
            notes.push(format!(
                "The client's place on the client list, its relationships or its details were not all put back ({e}): look at {} in Browse and add what is missing.",
                m.from.name
            ));
        }
        let paths: HashMap<String, String> = m
            .moves
            .iter()
            .map(|x| (x.to.to_lowercase(), x.from.clone()))
            .collect();
        if let Err(e) = holds::move_back(conn, root, device_id, &m.holds, &m.from.id, &paths) {
            notes.push(format!(
                "Its legal holds were not moved back ({e}): they are still on {}.",
                m.into.name
            ));
        }
        if let Some(row) = &m.csv_row {
            // Not a reason to fail: the client is on the list again either way.
            let _ = clients::restore_csv_row(root, &m.from.ext_id, row);
        }
        let mut done = m.clone();
        done.undone = Some(now);
        if !notes.is_empty() {
            done.note = Some(notes.join(" "));
        }
        if let Err(e) = save(root, &done) {
            notes.push(format!(
                "The merge's record could not be marked undone ({e}): it may be offered again; it is undone."
            ));
        }
        let reread: Vec<String> = m.from.folder.iter().cloned().collect();
        if let Err(e) = rebuild::sync_clients_edited(conn, root, device_id, "clients_edit", &reread)
        {
            notes.push(format!(
                "This catalog's client list was not brought up to date ({e}); it is at the next open."
            ));
        }
        Ok((back, (!notes.is_empty()).then(|| notes.join(" "))))
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cabinet::CabinetFile;
    use uuid::Uuid;
    use vf_core::db::Storage;

    fn put(p: &Path, body: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    const KEPT: &str = "Acme LLC (1001)";
    const DUP: &str = "Acme L.L.C. (1002)";

    /// A cabinet where one invented client was made twice (1001 and 1002) and a third owns
    /// part of the second; its client CSV; a catalog. (cabinet, CSV, catalog, kept, merged)
    fn cabinet(dir: &Path) -> (PathBuf, PathBuf, Connection, String, String) {
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
        for (name, id) in [
            ("Acme LLC", "1001"),
            ("Acme L.L.C.", "1002"),
            ("Baker Co", "1003"),
        ] {
            let folder = root.join(format!("{name} ({id})"));
            std::fs::create_dir_all(folder.join("Income Tax/2025")).unwrap();
            filer_intake::sentinel::write_sentinel(
                &root,
                &folder,
                &Client {
                    id: id.into(),
                    uuid: Uuid::new_v4(),
                    name: name.into(),
                    extra: BTreeMap::new(),
                },
                firm,
            )
            .unwrap();
        }
        put(
            &root.join(KEPT).join("Income Tax/2025/k1.pdf"),
            "the kept k-1",
        );
        put(
            &root.join(DUP).join("Income Tax/2025/k1.pdf"),
            "another k-1, same name",
        );
        put(
            &root.join(DUP).join("Income Tax/2024/return.pdf"),
            "a return",
        );
        put(
            &root.join(DUP).join("Permanent/Setup/ein.pdf"),
            "an EIN letter",
        );
        std::fs::create_dir_all(root.join(DUP).join("Payroll/2025")).unwrap();
        crate::doctypes::seed_if_missing(&root).unwrap();
        let csv = dir.join("clients.csv");
        std::fs::write(
            &csv,
            "id,name\r\n1001,Acme LLC\r\n1002,\"Acme L.L.C.\"\r\n1003,Baker Co\r\n",
        )
        .unwrap();
        filer_core::config::Settings {
            root: Some(root.clone()),
            csv_path: Some(csv.clone()),
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
        rebuild::rebuild(&mut conn, &root, "dev").unwrap();
        clients::ensure_listed(&conn, &root).unwrap();
        let rel = |parent: &str, child: &str| Relationship {
            parent_id: parent.into(),
            child_id: child.into(),
            role: "Shareholder".into(),
            note: String::new(),
        };
        clients::save_rels(
            &root,
            &filer_core::relationships::Relationships {
                records: vec![
                    rel("1003", "1002"),
                    rel("1001", "1002"),
                    rel("1003", "1001"),
                ],
            },
        )
        .unwrap();
        let id = |conn: &Connection, ext: &str| -> String {
            conn.query_row("SELECT id FROM client WHERE ext_id = ?1", [ext], |r| {
                r.get(0)
            })
            .unwrap()
        };
        let (kept, dup) = (id(&conn, "1001"), id(&conn, "1002"));
        (root, csv, conn, kept, dup)
    }

    fn docs_of(conn: &Connection, client: &str) -> Vec<String> {
        let mut st = conn
            .prepare(
                "SELECT rel_path FROM document WHERE client_id = ?1 AND deleted_at IS NULL
                 ORDER BY rel_path",
            )
            .unwrap();
        st.query_map([client], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    /// QA 2026-11-17: once an undo has moved the documents back, a list that cannot be put
    /// back as well (the share for a moment) is said, not an error: the merge is undone, and
    /// marked so, and is not offered again.
    #[test]
    fn a_client_with_a_notes_file_is_merged_and_its_notes_go_with_it() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, _csv, mut conn, kept, dup) = cabinet(tmp.path());
        put(
            &root.join(DUP).join("!fcinfo.ini"),
            "[Memo]\r\ncall in March\r\n",
        );
        put(
            &root.join(KEPT).join("!fcinfo.ini"),
            "[Memo]\r\nthe kept one's\r\n",
        );
        let p = plan(&conn, &root, &dup, &kept).expect("notes are not a reason to refuse");
        let m = apply(&mut conn, &root, &p, "dev").unwrap();
        assert!(!root.join(DUP).exists(), "its folder is gone");
        assert_eq!(
            std::fs::read_to_string(root.join(KEPT).join("!fcinfo.ini")).unwrap(),
            "[Memo]\r\nthe kept one's\r\n",
            "the kept client's notes are as they were"
        );
        assert_eq!(
            std::fs::read_to_string(root.join(KEPT).join("!fcinfo (2).ini")).unwrap(),
            "[Memo]\r\ncall in March\r\n",
            "the merged client's notes are beside them"
        );
        // And back with it.
        undo(&mut conn, &root, &m, "dev").unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join(DUP).join("!fcinfo.ini")).unwrap(),
            "[Memo]\r\ncall in March\r\n"
        );
        assert!(!root.join(KEPT).join("!fcinfo (2).ini").exists());
    }

    /// PA-105 (review 2026-11-24): the merged client's Portal contacts go to the kept client
    /// when it has none, and back on undo.
    #[test]
    fn portal_contacts_go_with_a_merge_and_back() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, _csv, mut conn, kept, dup) = cabinet(tmp.path());
        let row = crate::contacts::Row {
            email: "pat@example.test".into(),
            ..Default::default()
        };
        crate::contacts::set_by_hand(&root, &dup, &row).unwrap();
        let p = plan(&conn, &root, &dup, &kept).unwrap();
        let m = apply(&mut conn, &root, &p, "dev").unwrap();
        assert_eq!(
            crate::contacts::record(&root, &kept)
                .unwrap()
                .effective()
                .email,
            "pat@example.test"
        );
        assert!(crate::contacts::record(&root, &dup).unwrap().is_empty());
        undo(&mut conn, &root, &m, "dev").unwrap();
        assert!(crate::contacts::record(&root, &kept).unwrap().is_empty());
        assert_eq!(
            crate::contacts::record(&root, &dup)
                .unwrap()
                .effective()
                .email,
            "pat@example.test"
        );
    }

    /// The newest record decides what the last merge is: an older one that cannot be read
    /// hides nothing, and a newest one that cannot be read offers none.
    #[test]
    fn the_newest_record_decides_the_last_merge() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, _csv, mut conn, kept, dup) = cabinet(tmp.path());
        let p = plan(&conn, &root, &dup, &kept).unwrap();
        let m = apply(&mut conn, &root, &p, "dev").unwrap();
        std::fs::write(dir(&root).join("00000000.json"), "not a record").unwrap();
        assert_eq!(load_last(&root).map(|l| l.tx_id), Some(m.tx_id.clone()));
        std::fs::write(dir(&root).join("zzzzzzzz.json"), "not a record").unwrap();
        assert!(load_last(&root).is_none());
    }

    #[test]
    fn an_undo_whose_lists_cannot_be_put_back_is_still_an_undo() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, _csv, mut conn, kept, dup) = cabinet(tmp.path());
        let p = plan(&conn, &root, &dup, &kept).unwrap();
        let m = apply(&mut conn, &root, &p, "dev").unwrap();
        // The relationships cannot be read when the undo comes to them.
        let rels = clients::master_dir(&root).join("relationships.json");
        let text = std::fs::read(&rels).unwrap();
        std::fs::remove_file(&rels).unwrap();
        std::fs::create_dir(&rels).unwrap();
        let (back, note) = undo_noted(&mut conn, &root, &m, "dev").unwrap();
        assert_eq!(back, m.moves.len(), "every document went back");
        assert!(
            note.as_deref()
                .is_some_and(|n| n.contains("were not all put back")),
            "{note:?}"
        );
        assert!(root.join(DUP).join("Income Tax/2024/return.pdf").is_file());
        // Marked undone: not offered as the last merge, and not undone twice.
        assert!(load_last(&root).is_none());
        std::fs::remove_dir(&rels).unwrap();
        std::fs::write(&rels, text).unwrap();
    }

    #[test]
    fn a_client_is_merged_into_another_and_put_back() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, csv, mut conn, kept, dup) = cabinet(tmp.path());
        // A hold on the client merged away, and one on one of its documents.
        holds::place(
            &mut conn,
            &root,
            "dev",
            holds::Target::Client(dup.clone()),
            "an examination",
        )
        .unwrap();
        let ein: String = conn
            .query_row(
                "SELECT id FROM document WHERE rel_path LIKE '%ein.pdf'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        holds::place(
            &mut conn,
            &root,
            "dev",
            holds::Target::Docs(vec![ein.clone()]),
            "a request",
        )
        .unwrap();

        // A file the catalog does not know yet: nothing is merged.
        put(&root.join(DUP).join("new.pdf"), "just scanned");
        let e = plan(&conn, &root, &dup, &kept).unwrap_err();
        assert!(
            e.contains("not in the catalog yet") && e.contains("new.pdf"),
            "{e}"
        );
        std::fs::remove_file(root.join(DUP).join("new.pdf")).unwrap();
        assert!(plan(&conn, &root, &dup, &dup).is_err());

        let p = plan(&conn, &root, &dup, &kept).unwrap();
        assert_eq!(
            (p.moves.len(), p.kept_both(), p.holds, p.relationships),
            (3, 1, 1, 2)
        );
        let m = apply(&mut conn, &root, &p, "dev").unwrap();
        assert_eq!(m.note, None);

        // The documents are the kept client's, in the same places; the same name keeps both.
        assert_eq!(
            docs_of(&conn, &kept),
            [
                format!("{KEPT}/Income Tax/2024/return.pdf"),
                format!("{KEPT}/Income Tax/2025/k1 (2).pdf"),
                format!("{KEPT}/Income Tax/2025/k1.pdf"),
                format!("{KEPT}/Permanent/Setup/ein.pdf"),
            ]
        );
        for d in docs_of(&conn, &kept) {
            assert!(root.join(&d).is_file(), "{d}");
        }
        assert_eq!(
            std::fs::read_to_string(root.join(KEPT).join("Income Tax/2025/k1.pdf")).unwrap(),
            "the kept k-1"
        );
        // The merged client: no folder, not in the catalog, the list, the CSV.
        assert!(!root.join(DUP).exists());
        assert!(docs_of(&conn, &dup).is_empty());
        let gone: bool = conn
            .query_row(
                "SELECT deleted_at IS NOT NULL FROM client WHERE id = ?1",
                [&dup],
                |r| r.get(0),
            )
            .unwrap();
        assert!(gone);
        let master = clients::load(&root).unwrap();
        assert!(master.clients().iter().all(|c| c.id != "1002"));
        assert_eq!(
            std::fs::read_to_string(&csv).unwrap(),
            "id,name\r\n1001,Acme LLC\r\n1003,Baker Co\r\n"
        );
        // Its relationships are the kept client's: not one with itself, not one twice.
        let rels: Vec<(String, String)> = master
            .rels
            .records
            .iter()
            .map(|r| (r.parent_id.clone(), r.child_id.clone()))
            .collect();
        assert_eq!(rels, [("1003".to_string(), "1001".to_string())]);
        // The holds follow: the client's, and the document's by where it is now.
        let held = holds::load(&root).unwrap();
        assert!(
            held.iter()
                .any(|h| h.client_id.as_deref() == Some(kept.as_str()))
        );
        assert!(
            held.iter()
                .all(|h| h.client_id.as_deref() != Some(dup.as_str()))
        );
        assert!(held.iter().any(|h| {
            h.docs
                .iter()
                .any(|d| d.rel_path == format!("{KEPT}/Permanent/Setup/ein.pdf"))
        }));
        assert!(crate::trash::held(&conn, &ein));

        // Another PC, its catalog new this morning: the same documents (by id), no such client.
        let mut other = vf_core::db::open(&tmp.path().join("other.db"), Storage::Local).unwrap();
        rebuild::rebuild(&mut other, &root, "dev-2").unwrap();
        assert_eq!(docs_of(&other, &kept).len(), 4);
        let same: String = other
            .query_row("SELECT rel_path FROM document WHERE id = ?1", [&ein], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(same, format!("{KEPT}/Permanent/Setup/ein.pdf"));
        let n: i64 = other
            .query_row(
                "SELECT count(*) FROM client WHERE ext_id = '1002' AND deleted_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0);

        // Undone there, from the record in the cabinet alone.
        let last = load_last(&root).unwrap();
        assert_eq!(last, m);
        assert_eq!(undo(&mut other, &root, &last, "dev-2").unwrap(), 3);
        assert_eq!(
            docs_of(&other, &dup),
            [
                format!("{DUP}/Income Tax/2024/return.pdf"),
                format!("{DUP}/Income Tax/2025/k1.pdf"),
                format!("{DUP}/Permanent/Setup/ein.pdf"),
            ]
        );
        assert_eq!(
            std::fs::read_to_string(root.join(DUP).join("Income Tax/2025/k1.pdf")).unwrap(),
            "another k-1, same name"
        );
        assert_eq!(
            docs_of(&other, &kept),
            [format!("{KEPT}/Income Tax/2025/k1.pdf")]
        );
        assert!(root.join(DUP).join("Payroll/2025").is_dir());
        assert!(filer_intake::sentinel::sentinel_path(&root.join(DUP)).is_file());
        let master = clients::load(&root).unwrap();
        assert!(master.clients().iter().any(|c| c.id == "1002"));
        assert_eq!(master.rels.records.len(), 3);
        assert!(
            std::fs::read_to_string(&csv)
                .unwrap()
                .contains("1002,\"Acme L.L.C.\"")
        );
        let held = holds::load(&root).unwrap();
        assert!(
            held.iter()
                .any(|h| h.client_id.as_deref() == Some(dup.as_str()))
        );
        assert!(load_last(&root).is_none());
        assert!(undo(&mut other, &root, &last, "dev-2").is_err());
        // And a catalog made after sees the client as it was.
        let mut third = vf_core::db::open(&tmp.path().join("third.db"), Storage::Local).unwrap();
        rebuild::rebuild(&mut third, &root, "dev-3").unwrap();
        assert_eq!(docs_of(&third, &dup).len(), 3);
    }
}
